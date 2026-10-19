import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { Archive, ArrowLeft, Check, Copy, Ellipsis, FileDown, RotateCcw, Trash2 } from "lucide-react";
import { AnimatePresence, motion, useIsPresent, useReducedMotion } from "motion/react";
import {
  Banner,
  DeleteDialog,
  hhmm,
  host,
  lengthText,
  mmss,
  phase,
  Present,
  ro,
  RouteNotice,
  SelectField,
} from "@/components/common";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  archiveMeeting,
  BUILTIN,
  connectionLabel,
  exportMarkdown,
  getMeeting,
  getSettings,
  getTranscript,
  NEWER_VERSION,
  onSlackFinished,
  resumeMeeting,
  retrySummary,
  saveSummary,
  SCHEMA_VERSION,
  sendToSlack,
  SLACK_MAX,
  slackLength,
  STATUS_LABEL,
  type MeetingDetail,
  type MeetingItem,
  type Progress,
  type Segment,
  type Settings,
  type Status,
  type Webhook,
} from "@/lib/ipc";
import { swapVariants, TITLE_GLIDE_MS, titleMove } from "@/lib/motion";
import { cn } from "@/lib/utils";
import {
  BODY,
  caretRect,
  DocView,
  Grow,
  SendBar,
  Skeleton,
  Steps,
  TITLE,
  TranscriptPanel,
} from "@/screens/MeetingParts";
import type { SettingsTab } from "@/screens/Settings";

/** Autosave waits this long after the last keystroke. */
const SAVE_DELAY_MS = 800;
/** The Slack length is counted again this long after the last keystroke. */
const LENGTH_DELAY_MS = 200;

/** With a summary: the document itself, editable in `review`. */
const REVIEWABLE: Status[] = ["review", "sent", "archived"];
// `transcript.json` exists from `transcribed` on; a `paused` meeting has it only if it stopped
// while summarizing, which is when it has a connection.
const TRANSCRIBED: Status[] = ["transcribed", "summarizing", "summarize_failed", ...REVIEWABLE];
/** Room kept between the caret and the send bar over the page's bottom, or App's header over its top. */
const CARET_ROOM = 16;
/** App's sticky header (h-12). */
const HEADER = 48;

/** SND-04: the channel last sent to while it still exists, else the first (so the only one). */
const defaultChannel = (webhooks: Webhook[], last: string | null) =>
  last && webhooks.some((w) => w.name === last) ? last : (webhooks[0]?.name ?? "");

/** The local UTC offset at the meeting's start, in minutes, for the Slack header's time. */
const offsetOf = (createdAt: string) => -new Date(createdAt).getTimezoneOffset();

const day = (d: Date) => `${d.getMonth() + 1}월 ${d.getDate()}일`;
/** "10월 15일 수 10:00". */
const startOf = (iso: string) => `${day(new Date(iso))} ${"일월화수목금토"[new Date(iso).getDay()]} ${hhmm(iso)}`;
/** When it was sent: the time, and the day too when it was not the meeting's. */
const sentAt = (at: string, start: string) =>
  new Date(at).toDateString() === new Date(start).toDateString() ? hhmm(at) : `${day(new Date(at))} ${hhmm(at)}`;

// Called within the click, as WebKit wants. The webview may have no clipboard or refuse it: either
// way the recorder is told.
const copy = async (text: string) => {
  try {
    await navigator.clipboard.writeText(text);
    return "복사했습니다";
  } catch {
    return "클립보드에 복사하지 못했습니다";
  }
};

type Edit = { title: string; body: string };

/** Autosave state per meeting, outliving the screen like ipc.ts's model download: the edit not on
 * disk yet (typed, or failed and waiting to go again), the last save, and its number. Saves run
 * one at a time and in order, so an older body never lands after a newer one. */
type Saves = { pending: Edit | null; last: Promise<unknown>; latest: number };
// ponytail: entries stay for the app's run (a few bytes per meeting opened); deleting drops one.
const saves = new Map<string, Saves>();
const savesOf = (id: string) => {
  let s = saves.get(id);
  if (!s) saves.set(id, (s = { pending: null, last: Promise.resolve(), latest: 0 }));
  return s;
};

/** One meeting as one document (design-system 6.6): being written, then reviewed and sent, in the
 * same place under the same header. LLM output is untrusted: title and summary are plain text in
 * form fields or text nodes, never HTML (D6, NFR-07). */
export default function Meeting({
  id,
  listed,
  progress,
  barSlot,
  onHome,
  onNotice,
  onSetup,
}: {
  id: string;
  /** The meeting as the list has it, so the header is there at once (and its title can glide in
   * from its Home row) before the meeting is read. */
  listed?: MeetingItem;
  /** The meeting's last `meeting://progress`, if it has been in line. */
  progress?: Progress;
  /** App's bottom stack: the send bar goes there, above the dock. */
  barSlot?: HTMLElement | null;
  /** Back to Home: from 홈으로 with its click, after 전송 안 함 or 삭제 without. */
  onHome: (e?: MouseEvent) => void;
  /** The app's banner: a save that fails after the screen has closed is told there. */
  onNotice: (message: string) => void;
  /** Opens a settings tab: LLM connections, or Slack when there is no webhook to send to. */
  onSetup: (tab: SettingsTab) => void;
}) {
  const s = savesOf(id);
  // On its way out the send bar and the transcript panel leave with it.
  const present = useIsPresent();
  const reduce = useReducedMotion();
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  // `title` and `body` hold the meeting's text: read once it has a summary.
  const [loaded, setLoaded] = useState(false);
  const [saveState, setSaveState] = useState("");
  const [saveError, setSaveError] = useState("");
  // A failed action's, or the settings'. A read's own failure is apart: the next read clears it.
  const [error, setError] = useState("");
  const [readError, setReadError] = useState("");
  // A copy's result shows briefly where it was asked; the saved `.md` path stays.
  const [copied, setCopied] = useState<{ at: "summary" | "transcript"; text: string } | null>(null);
  const [exported, setExported] = useState("");
  const [channel, setChannel] = useState("");
  // REV-02, from slack.rs: what the send would carry, header included.
  const [length, setLength] = useState<number | null>(null);
  const [sending, setSending] = useState(false);
  // SND-06: why the last send failed, also when this screen opens again.
  const [sendError, setSendError] = useState("");
  // Sent while open: the send bar stays to show where it went.
  const [sentHere, setSentHere] = useState(false);
  // SUM-06: the connection picked for 요약 다시 실행, until then the default.
  const [chosen, setChosen] = useState("");
  const [asking, setAsking] = useState<"archive" | "delete" | null>(null);
  // The summary arrived while open: it comes in once, read-only, before it can be edited.
  const [reveal, setReveal] = useState(false);
  // REV-01: the summary reads as a document; a click on a line or 고치기 swaps the editor in
  // (design-system 6.6). `instant`: a key did it, so the two do not crossfade.
  const [editing, setEditing] = useState({ open: false, instant: true });
  // The title glides in from its Home row on arrival, never later when the meta line wraps.
  const [arriving, setArriving] = useState(true);
  const [panel, setPanel] = useState({ open: false, instant: false });
  const [segments, setSegments] = useState<Segment[] | null>(null);
  const [transcriptError, setTranscriptError] = useState("");
  const timer = useRef(0);
  const fields = useRef(false);
  const latest = useRef(0);
  const ready = useRef<Promise<void>>(Promise.resolve());
  const seen = useRef<Status | null>(null);
  const more = useRef<HTMLButtonElement>(null);
  const toggle = useRef<HTMLButtonElement>(null);
  const fix = useRef<HTMLButtonElement>(null);
  // Where the editor puts the caret when it opens, and where on screen that line was clicked.
  const caret = useRef<{ at: number; top?: number } | null>(null);
  // How focus is leaving the editor: by a key (no crossfade), or by a press on the send bar (the
  // editor stays, for a send that fails).
  const leaving = useRef<"key" | "bar" | null>(null);
  // Esc: the caret's line and where it was on screen, for the document to keep it there.
  const back = useRef<{ line: number; top: number | null } | null>(null);

  /** Saves the pending edit now (REV-01). Only the latest save's reply shows. A failed edit stays
   * pending, so the next flush sends it again. Rejects when the latest save failed. */
  const flush = () => {
    window.clearTimeout(timer.current);
    const edit = s.pending;
    if (edit) {
      s.pending = null;
      const request = ++s.latest;
      s.last = s.last
        .catch(() => {})
        .then(() => saveSummary(id, edit.title, edit.body))
        .then(
          (meta) => {
            if (request !== s.latest) return;
            setSaveError("");
            if (!s.pending) setSaveState("저장됨");
            // The title is stored as one line. Mirror that unless typing went on meanwhile, keeping
            // a space typed at the end before a pause.
            setTitle((t) => (t === edit.title ? meta.title + (/\s$/.test(edit.title) ? " " : "") : t));
          },
          (e) => {
            if (request === s.latest) {
              s.pending ??= edit;
              setSaveState("");
              setSaveError(`저장하지 못했습니다: ${e}`);
            }
            throw e;
          },
        );
    }
    return s.last;
  };

  const edit = (next: Edit) => {
    setTitle(next.title);
    setBody(next.body);
    s.pending = next;
    setSaveState("저장 중…");
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => flush().catch(() => {}), SAVE_DELAY_MS);
  };

  /** The fields fill once, when the meeting first reads with a summary. An edit an earlier visit
   * could not save comes back into them and goes again. */
  const apply = (d: MeetingDetail) => {
    const was = seen.current;
    seen.current = d.meta.status;
    setDetail(d);
    // In review, `error` is only ever a failed send's (the queue clears it before review).
    setSendError(d.meta.status === "review" ? (d.meta.error ?? "") : "");
    if (was === "review" && d.meta.status === "sent") setSentHere(true);
    if (fields.current || !REVIEWABLE.includes(d.meta.status)) return;
    fields.current = true;
    if (was && !REVIEWABLE.includes(was) && !reduce) setReveal(true);
    const unsaved = s.pending;
    setTitle(unsaved?.title ?? d.meta.title);
    setBody(unsaved?.body ?? d.summary);
    setLoaded(true);
    if (unsaved) {
      setSaveState("저장 중…");
      flush().catch(() => {});
    }
  };

  // Only the latest read may land: an older one can resolve after a newer one.
  const read = () => {
    const request = ++latest.current;
    getMeeting(id).then(
      (d) => {
        if (request !== latest.current) return;
        setReadError("");
        apply(d);
      },
      (e) => request === latest.current && setReadError(String(e)),
    );
  };

  // A send an earlier visit started may still be out (`sending`): its end is listened for before
  // the first read, so it cannot fall between, and read then.
  useEffect(() => {
    const unlisten = onSlackFinished((done) => done === id && read());
    ready.current = unlisten.then(
      () => {},
      (e) => setError(`전송 결과 알림을 준비하지 못했습니다: ${e}`),
    );
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, [id]);

  // Read after the last save of an earlier visit has landed, and again when the status or the
  // finished chunks change while it is processed.
  useEffect(() => {
    ready.current.then(() => s.last.catch(() => {})).then(read);
  }, [id, progress?.status, progress?.chunk]);

  // Leaving (홈으로, the header) saves a pending edit right away. The screen is gone by the time
  // it fails, so the app says so; reopening the meeting sends the edit again.
  useEffect(
    () => () =>
      void flush().catch((e) => onNotice(`회의 문서에서 고친 내용을 저장하지 못했습니다: ${e}`)),
    [],
  );

  // The recorder's name for the meta line, the webhooks to send to, the connections to retry with.
  useEffect(() => {
    getSettings().then(
      ({ settings }) => {
        setSettings(settings);
        setChannel(defaultChannel(settings.webhooks, settings.last_channel));
      },
      (e) => setError(`설정을 읽지 못했습니다: ${e}`),
    );
  }, []);

  // Counted by Rust from the very text a send builds, a moment after typing stops.
  const createdAt = detail?.meta.created_at;
  const countable = detail?.meta.status === "review" && !detail.meta.no_speech;
  useEffect(() => {
    if (!createdAt || !countable) return;
    let live = true;
    const t = window.setTimeout(
      () =>
        slackLength(id, title, body, offsetOf(createdAt)).then(
          (n) => live && setLength(n),
          () => live && setLength(null),
        ),
      LENGTH_DELAY_MS,
    );
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [id, createdAt, countable, title, body]);

  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(null), 3000);
    return () => window.clearTimeout(t);
  }, [copied]);

  useEffect(() => {
    const t = window.setTimeout(() => setArriving(false), TITLE_GLIDE_MS);
    return () => window.clearTimeout(t);
  }, []);

  /** Opens the editor with the caret at the start of `line`. `top`: where that line is on screen,
   * so it stays under the pointer. */
  const write = (line: number, instant: boolean, top?: number) => {
    caret.current = { at: line && body.split("\n").slice(0, line).join("\n").length + 1, top };
    setEditing({ open: true, instant });
  };
  const placeCaret = (field: HTMLTextAreaElement | null) => {
    const c = caret.current;
    if (!field || !c) return;
    caret.current = null;
    field.focus({ preventScroll: true });
    field.setSelectionRange(c.at, c.at);
    const box = caretRect(field);
    if (c.top != null && box?.height) window.scrollBy(0, box.top - c.top);
  };
  // The page scrolls, not the field, and a browser brings the caret only to the window's edge:
  // under the send bar and the dock, or going up, under the header. It is moved clear of them, once
  // the text has its new layout.
  const lift = (field: HTMLTextAreaElement) =>
    requestAnimationFrame(() => {
      const box = caretRect(field);
      if (!box?.height) return;
      const stack = barSlot?.parentElement?.getBoundingClientRect().top;
      if (box.top < HEADER + CARET_ROOM) window.scrollBy(0, box.top - HEADER - CARET_ROOM);
      else if (stack && box.bottom > stack - CARET_ROOM) window.scrollBy(0, box.bottom - stack + CARET_ROOM);
    });
  // After Esc the document keeps the caret's line where it was, and focus goes to 고치기 without
  // taking the page to it.
  useLayoutEffect(() => {
    const b = back.current;
    if (editing.open || !b) return;
    back.current = null;
    const line = fix.current?.parentElement?.querySelector(`[data-line="${b.line}"]`);
    if (line && b.top != null) window.scrollBy(0, line.getBoundingClientRect().top - b.top);
    fix.current?.focus({ preventScroll: true });
  }, [editing.open]);

  /** Opens or closes the transcript, read the first time it opens. Focus goes back to 전사본. */
  const showPanel = (open: boolean, instant: boolean) => {
    setPanel({ open, instant });
    if (!open) toggle.current?.focus();
    else if (!segments) {
      setTranscriptError("");
      getTranscript(id).then(setSegments, (e) => setTranscriptError(`전사본을 읽지 못했습니다: ${e}`));
    }
  };
  useEffect(() => {
    if (!panel.open) return;
    // An Esc an open menu, dialog or list (Radix) or the editor took is marked `defaultPrevented`.
    // Their element may be gone by now: without exit animations Radix unmounts it at once.
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) showPanel(false, true);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [panel.open]);

  // Both act on what is on disk, so a pending edit goes first, and a failed save stops them.
  const archive = () => {
    setError("");
    flush().then(
      () => archiveMeeting(id).then(() => onHome(), (e) => setError(String(e))),
      () => {},
    );
  };
  const exportFile = () => {
    setError("");
    setExported("");
    flush().then(
      () =>
        exportMarkdown(id).then(
          (path) => path && setExported(`저장했습니다: ${path}`),
          (e) => setError(String(e)),
        ),
      () => {},
    );
  };
  // SND-05: only this click sends. A pending edit is saved first, and a failed save stops it.
  const send = () => {
    if (!createdAt) return;
    setError("");
    setSendError("");
    setSending(true);
    flush()
      .then(
        () =>
          sendToSlack(id, channel, title, body, offsetOf(createdAt)).then(
            (meta) => {
              seen.current = meta.status;
              setDetail((d) => d && { ...d, meta });
              setTitle(meta.title);
              setSentHere(true);
            },
            (e) => setSendError(String(e)),
          ),
        () => {},
      )
      .finally(() => setSending(false));
  };
  const copyText = (at: "summary" | "transcript", text: string) => {
    setError("");
    copy(text).then((note) => setCopied({ at, text: note }));
  };
  // SUM-06: back in line; its next event takes the error and the buttons away.
  const queued = (request: Promise<MeetingDetail["meta"]>) =>
    request.then(
      (meta) => {
        setDetail((d) => d && { ...d, meta });
        setError("");
      },
      (e) => setError(String(e)),
    );

  const meta = detail?.meta ?? listed;
  const now = meta ? phase(meta, progress) : null;
  const newer = !!meta && meta.schema_version > SCHEMA_VERSION;
  const reviewable = !!meta && REVIEWABLE.includes(meta.status);
  // This screen's send, or one an earlier visit started that is still out.
  const inFlight = sending || !!detail?.sending;
  // `sent`, `archived` and a newer app's meeting stay read-only; with no speech there is no text.
  const editable = meta?.status === "review" && !newer && loaded && !meta.no_speech;
  // Not while it arrives, nor while a send could change what is on disk.
  const canEdit = editable && !reveal && !inFlight;
  const over = Math.max(0, (length ?? 0) - SLACK_MAX);
  const canSend = editable && !!channel && over === 0 && !inFlight;
  const transcribeFailed = meta?.status === "transcribe_failed" && now === null && !newer;
  const summaryFailed = meta?.status === "summarize_failed" && now === null && !newer;
  // Stopped after transcribing, with no place in line: no LLM connection.
  const stopped = meta?.status === "transcribed" && now === null && !newer;
  // Events come before the meeting is read again: a summary's first one can meet a transcribing meta.
  const writing = now === "running" && (progress?.status === "summarizing" || meta?.status === "summarizing");
  const requests = progress?.status === "summarizing" && progress.chunks > 1 ? progress : null;
  const hasTranscript =
    !!meta && (TRANSCRIBED.includes(meta.status) || (meta.status === "paused" && !!meta.llm_connection));
  const text = loaded && !meta?.no_speech;
  const connections = settings?.llm_connections ?? null;
  // The meeting's own connection first if it still exists, then the default, then the first.
  const names = connections?.map((c) => c.name) ?? [];
  const connection =
    chosen || ([meta?.llm_connection, settings?.default_llm].find((n) => n && names.includes(n)) ?? names[0] ?? "");
  const hint = stopped
    ? "LLM 연결을 추가하면 요약합니다"
    : summaryFailed && connections?.length === 0
      ? "LLM 연결을 추가하면 요약을 다시 실행할 수 있습니다"
      : "";
  // REV-01: the prompt writes "미정" for an owner or a date nobody named (architecture 6.2).
  const undecided = body
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.includes("미정"));
  const c = meta?.llm_connection;

  // The meta line (`caption`): when, how long, who recorded, where it was summarized (SUM-07),
  // where it went, and the autosave.
  const facts = meta
    ? [
        startOf(meta.created_at),
        meta.duration_ms > 0 && lengthText(meta.duration_ms),
        settings?.recorder_name && `녹음 ${settings.recorder_name}`,
        c &&
          reviewable &&
          !meta.no_speech &&
          (c === BUILTIN
            ? `${connectionLabel(c)}에서 요약함`
            : meta.summarize_local
              ? `이 PC에서 ${c}${ro(c)} 요약함`
              : `${c}${ro(c)} 보내 요약함`),
        meta.status === "sent" && meta.sent && (
          <>
            <Check aria-hidden className="size-3.5 text-ok-line" />
            {`${meta.sent.channel}${ro(meta.sent.channel)} 보냄 · ${sentAt(meta.sent.at, meta.created_at)} · 수정 ${meta.sent.edit_count}곳`}
          </>
        ),
        meta.status === "archived" && STATUS_LABEL.archived,
        // Nothing is saved from a sent meeting.
        editable && saveState,
      ].filter(Boolean)
    : [];

  const focusMore = (e: Event) => {
    e.preventDefault();
    more.current?.focus();
  };

  return (
    <main className={`mx-auto box-content max-w-doc px-8 pt-8 ${editable || sentHere ? "pb-20" : "pb-8"}`}>
      <div className="flex items-center justify-between gap-3">
        <Button variant="ghost" size="sm" className="-ml-3 text-ink-2 hover:text-ink" onClick={onHome}>
          <ArrowLeft aria-hidden />
          홈으로
        </Button>
        <div className="flex min-w-0 items-center gap-1">
          {/* Two regions, so the path is not read out again when a copy result goes. */}
          <span role="status" className="truncate text-caption text-ink-2">
            {copied?.at === "summary" ? copied.text : ""}
          </span>
          <span role="status" title={exported} className="truncate text-caption text-ink-2">
            {exported}
          </span>
          {/* REV-01: always there to read once it exists, even with no speech. */}
          {hasTranscript && (
            <Button
              ref={toggle}
              variant="ghost"
              size="sm"
              aria-expanded={panel.open}
              onClick={(e) => showPanel(!panel.open, e.detail === 0)}
            >
              전사본
            </Button>
          )}
          {meta && (
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <Button ref={more} variant="ghost" size="icon" aria-label="더보기">
                  <Ellipsis aria-hidden />
                </Button>
              </DropdownMenuTrigger>
              {/* A dialog opened from here keeps focus; it puts it back here when it closes. */}
              <DropdownMenuContent onCloseAutoFocus={(e) => asking && e.preventDefault()}>
                {text && (
                  <>
                    <DropdownMenuItem onSelect={() => copyText("summary", `# ${title}\n\n${body}`)}>
                      <Copy aria-hidden />
                      요약 복사
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={exportFile}>
                      <FileDown aria-hidden />
                      .md로 저장
                    </DropdownMenuItem>
                  </>
                )}
                {meta.status === "review" && !newer && (
                  <DropdownMenuItem disabled={inFlight} onSelect={() => setAsking("archive")}>
                    <Archive aria-hidden />
                    전송 안 함
                  </DropdownMenuItem>
                )}
                {(text || (meta.status === "review" && !newer)) && <DropdownMenuSeparator />}
                <DropdownMenuItem disabled={inFlight} onSelect={() => setAsking("delete")}>
                  <Trash2 aria-hidden className="text-danger-line" />
                  삭제
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>

      {meta && (
        <header className="mt-6 space-y-2">
          <p className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-caption text-ink-2">
            {facts.map((fact, i) => (
              <span
                key={i}
                className="flex items-center gap-1 not-first:before:mr-0.5 not-first:before:content-['·']"
              >
                {fact}
              </span>
            ))}
          </p>
          {/* Its own box, never the field: the title glides in from its Home row by position only,
              so the glyphs never stretch (design-system 6.1). It glides on arrival and on the way
              out (the glide back into its row takes this timing); any other move, such as the meta
              line wrapping, is instant. */}
          <motion.div
            layoutId={`title-${id}`}
            layout="position"
            transition={arriving || !present ? titleMove : { duration: 0 }}
          >
            {/* Focus lands here when this screen opens from a key (App.tsx). */}
            <h1 tabIndex={-1} className={cn("outline-hidden", !editable && TITLE, !editable && !meta.title && "text-ink-2")}>
              {editable ? (
                <Grow
                  aria-label="제목"
                  placeholder="제목 없음"
                  className={TITLE}
                  value={title}
                  readOnly={inFlight}
                  // One line: Enter does not break it, and a pasted break becomes a space, with the
                  // caret left where it was (React would set the new value and put it at the end).
                  onKeyDown={(e) => e.key === "Enter" && e.preventDefault()}
                  onChange={(e) => {
                    const field = e.target;
                    if (field.value.includes("\n")) {
                      const at = field.selectionStart;
                      field.value = field.value.replace(/\n/g, " ");
                      field.setSelectionRange(at, at);
                    }
                    edit({ title: field.value, body });
                  }}
                >
                  {/* Focus: a 2px brand line opening from the left. */}
                  <span
                    aria-hidden
                    className="absolute inset-x-0 -bottom-1 h-0.5 origin-left scale-x-0 rounded-full bg-brand peer-focus:scale-x-100 motion-safe:transition-transform motion-safe:duration-200 motion-safe:ease-out"
                  />
                </Grow>
              ) : (
                meta.title || (reviewable ? "제목 없음" : "제목은 요약 뒤에 붙어요")
              )}
            </h1>
          </motion.div>
        </header>
      )}

      <div className="mt-8 space-y-6">
        {readError && <Banner kind="error">{readError}</Banner>}
        {error && <Banner kind="error">{error}</Banner>}
        {saveError && <Banner kind="error">{saveError}</Banner>}
        {/* SND-06: the cause; the send button turns into 다시 보내기, and 요약 복사 is in ⋯. */}
        {editable && sendError && <Banner kind="error">{sendError}</Banner>}
        {newer && <Banner kind="warning">{NEWER_VERSION}</Banner>}

        {detail && reviewable && meta?.no_speech && (
          // SUM-10 (6.5): no LLM was asked, so there is nothing to edit, copy or send.
          <Banner kind="warning">음성이 거의 감지되지 않았습니다</Banner>
        )}
        {text && (
          <>
            {/* The one being left pops out of the flow, so the two cross in place. No `initial={false}`:
                it would hold back the arrival's reveal inside; the first entrance is instant anyway. */}
            <div className="relative">
              <AnimatePresence mode="popLayout" custom={editing.instant}>
                {editable && editing.open ? (
                  <motion.div
                    key="edit"
                    custom={editing.instant}
                    variants={swapVariants}
                    initial="out"
                    animate="in"
                    exit="out"
                  >
                    <Present>
                      <Grow
                        ref={placeCaret}
                        aria-label="요약"
                        marks
                        className={BODY}
                        value={body}
                        readOnly={inFlight}
                        onChange={(e) => edit({ title, body: e.target.value })}
                        onSelect={(e) => lift(e.currentTarget)}
                        onFocus={() => (leaving.current = null)}
                        onKeyDown={(e) => {
                          if (e.key === "Tab") leaving.current = "key";
                          if (e.key === "Escape" && !e.nativeEvent.isComposing) {
                            e.preventDefault();
                            const field = e.currentTarget;
                            const box = caretRect(field);
                            back.current = {
                              line: field.value.slice(0, field.selectionStart).split("\n").length - 1,
                              top: box?.height ? box.top : null,
                            };
                            setEditing({ open: false, instant: true });
                          }
                        }}
                        onBlur={(e) => {
                          // Still focused: another app took the window, and the caret waits here.
                          if (document.activeElement === e.currentTarget || leaving.current === "bar") return;
                          const instant = leaving.current === "key";
                          setEditing((d) => (d.open ? { open: false, instant } : d));
                        }}
                      >
                        {/* Focus: a 2px brand line down the left margin. */}
                        <span
                          aria-hidden
                          className="absolute inset-y-0 -left-5 w-0.5 origin-top scale-y-0 rounded-full bg-brand peer-focus:scale-y-100 motion-safe:transition-transform motion-safe:duration-200 motion-safe:ease-out"
                        />
                      </Grow>
                    </Present>
                  </motion.div>
                ) : (
                  <motion.div
                    key="view"
                    custom={editing.instant}
                    variants={swapVariants}
                    initial="out"
                    animate="in"
                    exit="out"
                  >
                    {/* The wash reaches past the text as a ring, so nothing moves (as the marker). */}
                    <Present
                      className={cn(
                        canEdit &&
                          "cursor-text rounded-xs hover:bg-sunken/60 hover:ring-8 hover:ring-sunken/60 motion-safe:transition-[background-color,box-shadow] motion-safe:duration-120",
                      )}
                    >
                      {/* Always there for the keyboard and screen readers; quiet beside the text. */}
                      {editable && (
                        <Button
                          ref={fix}
                          variant="ghost"
                          size="sm"
                          className={cn("float-right ml-3 h-7 text-ink-2 hover:text-ink", reveal && "invisible")}
                          disabled={!canEdit}
                          onClick={(e) => write(0, e.detail === 0)}
                        >
                          고치기
                        </Button>
                      )}
                      <DocView
                        text={body}
                        reveal={reveal}
                        onRevealed={() => setReveal(false)}
                        onEdit={canEdit ? (line, top) => write(line, false, top) : undefined}
                      />
                    </Present>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
            {/* For screen readers above all: the marks are only drawn. Typed into, so not live. */}
            {undecided.length > 0 && (
              <details className="text-label">
                <summary className="flex min-h-6 w-fit cursor-pointer items-center gap-2 rounded-md text-ink-2 hover:text-ink">
                  <span aria-hidden className="size-2.5 rounded-sm bg-marker" />
                  {`확인할 항목 ${undecided.length}개`}
                </summary>
                <div className="mt-2 space-y-1 pl-4.5">
                  <p className="text-ink-2">담당자나 기한이 미정인 줄</p>
                  {/* As written: the lines carry their own "- ". */}
                  <ul className="space-y-0.5">
                    {undecided.map((line, i) => (
                      <li key={i}>{line}</li>
                    ))}
                  </ul>
                </div>
              </details>
            )}
          </>
        )}

        {detail && meta && !reviewable && !newer && (now || transcribeFailed || summaryFailed || stopped) && (
          <>
            <div className="space-y-4">
              <Steps meta={meta} progress={progress} failed={transcribeFailed || summaryFailed} />
              <RouteNotice id={id} status={meta.status} />
            </div>
            {/* Hidden once a retry has it back in line. */}
            {now === null && meta.error && <Banner kind="error">{meta.error}</Banner>}
            {hint && <p>{hint}</p>}
            {(transcribeFailed || summaryFailed || stopped) && (
              <div className="flex flex-wrap items-end gap-2">
                {transcribeFailed && (
                  <Button variant="outline" onClick={() => queued(resumeMeeting(id))}>
                    <RotateCcw aria-hidden />
                    다시 시도
                  </Button>
                )}
                {summaryFailed && !!connections?.length && (
                  <>
                    <div className="min-w-56">
                      <SelectField
                        label="요약할 LLM 연결"
                        value={connection}
                        // SUM-07: where each one would send the transcript.
                        options={connections.map((c) => ({
                          value: c.name,
                          label: `${c.name} (${c.is_local ? "이 PC" : `${host(c.base_url)}로 전송`})`,
                        }))}
                        onChange={setChosen}
                      />
                    </div>
                    <Button variant="outline" onClick={() => queued(retrySummary(id, connection))}>
                      <RotateCcw aria-hidden />
                      요약 다시 실행
                    </Button>
                  </>
                )}
                {/* A missing or refused key (6.4) is fixed there too. */}
                {(stopped || summaryFailed) && (
                  <Button variant="outline" onClick={() => onSetup("llm")}>
                    요약(LLM) 설정
                  </Button>
                )}
              </div>
            )}
            {now && (
              <div className="space-y-4 pt-2">
                {writing && (
                  <p className="text-label text-ink-2 tabular-nums">
                    {["요약을 쓰고 있어요", requests && `${Math.min(requests.chunk + 1, requests.chunks)}/${requests.chunks}`]
                      .filter(Boolean)
                      .join(" · ")}
                  </p>
                )}
                <Skeleton writing={writing} />
              </div>
            )}
          </>
        )}
      </div>

      {barSlot &&
        createPortal(
          // A press here keeps the editor open (React events pass through the portal).
          <div className="contents" onPointerDown={() => (leaving.current = "bar")}>
            <SendBar
            shown={present && !!text && (editable || sentHere)}
            webhooks={settings?.webhooks ?? null}
            channel={channel}
            length={length}
            inFlight={inFlight}
            failed={!!sendError}
            canSend={canSend}
            sentTo={sentHere && meta?.status === "sent" ? meta.sent?.channel : undefined}
            onChannel={setChannel}
            onSend={send}
            onSetup={() => onSetup("slack")}
            />
          </div>,
          barSlot,
        )}
      <TranscriptPanel
        open={panel.open && present}
        instant={panel.instant}
        segments={segments}
        error={transcriptError}
        copied={copied?.at === "transcript" ? copied.text : ""}
        onCopy={() =>
          segments &&
          copyText("transcript", segments.map((seg) => `[${mmss(seg.start_ms)}] ${seg.text.trim()}`).join("\n"))
        }
        onClose={() => showPanel(false, false)}
      />

      {/* REV-03: an archived meeting can never be sent, so it is confirmed first. */}
      <AlertDialog open={asking === "archive"} onOpenChange={(open) => !open && setAsking(null)}>
        <AlertDialogContent onCloseAutoFocus={focusMore}>
          <AlertDialogHeader>
            <AlertDialogTitle>전송하지 않을까요?</AlertDialogTitle>
            <AlertDialogDescription>
              전송 안 함으로 두면 이 회의는 나중에 Slack으로 보낼 수 없습니다. 요약과 전사본은 남습니다.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>취소</AlertDialogCancel>
            <AlertDialogAction onClick={archive}>전송 안 함</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <DeleteDialog
        id={id}
        open={asking === "delete"}
        onOpenChange={(open) => !open && setAsking(null)}
        onCloseAutoFocus={focusMore}
        onDeleted={() => {
          // Nothing is left to save, and leaving must not say otherwise. A save still out is no
          // longer the latest: if it fails, its edit does not come back.
          saves.delete(id);
          s.pending = null;
          s.latest++;
          s.last = Promise.resolve();
          onHome();
        }}
        onError={setError}
      />
    </main>
  );
}
