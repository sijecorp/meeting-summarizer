import { useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { Check, ChevronRight, CircleAlert, FileAudio, TriangleAlert } from "lucide-react";
import { motion } from "motion/react";
import {
  Banner,
  Bars,
  DeleteButton,
  Dots,
  downloadNote,
  EngineStarting,
  engineStarting,
  hhmm,
  Kbd,
  lengthText,
  phase,
  progressText,
  ro,
  SHIMMER,
  SHORTCUT,
  statusLabel,
} from "@/components/common";
import { Button } from "@/components/ui/button";
import { Progress as ProgressBar } from "@/components/ui/progress";
import {
  getSettings,
  modelDownload,
  modelDownloadFailure,
  modelStatus,
  onModelDownload,
  resumeMeeting,
  SCHEMA_VERSION,
  startModelDownload,
  STATUS_LABEL,
  type MeetingItem,
  type ModelDownload,
  type Progress,
  type Status,
} from "@/lib/ipc";
import { listVariants, rowVariants, TITLE_GLIDE_MS, titleMove } from "@/lib/motion";
import type { SettingsTab } from "@/screens/Settings";

/** Something transcribing, summarizing or sending needs, and the settings tab that fixes it. */
type SetupItem = { text?: string; action: string; tab: SettingsTab };

const FAILED: Status[] = ["transcribe_failed", "summarize_failed"];
/** Why a meeting with its transcript waits out of line for its route (plan 5.4). */
const waitOf = (m: MeetingItem) => (m.status === "transcribed" || m.status === "paused" ? m.wait : undefined);
// A setup line item, after a dot when it is not the first.
const ITEM = "flex items-center gap-1 not-first:before:mr-1 not-first:before:text-ink-2 not-first:before:content-['·']";

/** A row's name: its title, or until the summary gives one, what it is ("42분 녹음"): a column of
 * "제목 없음" says nothing. */
function nameOf(m: MeetingItem) {
  if (m.title) return m.title;
  if (m.status === "recording" && !m.interrupted) return "녹음 중인 회의";
  return m.duration_ms ? `${lengthText(m.duration_ms)} 녹음` : "제목 없는 녹음";
}

/** "지금 할 일" (design-system 6.4): recording, in line, ready for review, failed, waiting for a
 * connection to replace a deleted one, or left behind by a crash. A meeting from a newer app
 * version is read-only here, so never. */
const needsYou = (m: MeetingItem, p: Progress | undefined) =>
  m.interrupted ||
  (m.schema_version <= SCHEMA_VERSION &&
    (m.status === "recording" ||
      phase(m, p) !== null ||
      m.status === "review" ||
      FAILED.includes(m.status) ||
      waitOf(m) === "connection_deleted"));

const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();
const yesterdayOf = (today: Date) => new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
/** A date group's name: 오늘, 어제, else "10월 12일 일요일" (with the year when it is not this one). */
function dayGroup(d: Date, today: Date) {
  if (sameDay(d, today)) return "오늘";
  if (sameDay(d, yesterdayOf(today))) return "어제";
  return d.toLocaleDateString("ko-KR", {
    year: d.getFullYear() === today.getFullYear() ? undefined : "numeric",
    month: "long",
    day: "numeric",
    weekday: "long",
  });
}
/** The day under a "지금 할 일" row's time when it is not today. */
function shortDay(d: Date, today: Date) {
  if (sameDay(d, today)) return null;
  if (sameDay(d, yesterdayOf(today))) return "어제";
  return `${d.getMonth() + 1}월 ${d.getDate()}일`;
}

/** The right side of a row: a mark that moves while something happens, and words that always say
 * it too (never color or motion alone). `note` is one line under the title; `bar` a 1px line under
 * the row. */
type State = {
  mark?: ReactNode;
  text: ReactNode;
  /** Before `text`, e.g. "전사" before "42% · 약 3분 남음". */
  lead?: string;
  shimmer?: boolean;
  note?: string;
  bar?: ReactNode;
};
function stateOf(m: MeetingItem, p: Progress | undefined, name: string): State {
  if (m.interrupted) {
    return {
      mark: <TriangleAlert aria-hidden className="size-4 text-warn-line" />,
      text: "중단된 녹음",
      // STO-03: the WAV files hold what the last 5 s flush wrote.
      note: "녹음 중에 앱이 꺼졌습니다. 마지막으로 저장된 곳까지 처리하거나 삭제할 수 있습니다",
    };
  }
  if (m.schema_version > SCHEMA_VERSION) return { text: "새 버전 앱의 회의" };
  const now = phase(m, p);
  if (m.status === "recording") {
    return {
      mark: <span aria-hidden className="size-2 rounded-full bg-rec motion-safe:animate-breathe" />,
      text: STATUS_LABEL.recording,
    };
  }
  if (now === "running") {
    const line = p && progressText(p);
    // Events come before the list is read again: a summary's first one can meet a list that still
    // says transcribing.
    const summarizing = (p?.status === "summarizing" ? p.status : m.status) === "summarizing";
    return {
      mark: summarizing ? undefined : <Bars className="h-3 text-brand" />,
      text: engineStarting(p) ? <EngineStarting since={p.engine_since} /> : (line?.text ?? STATUS_LABEL[m.status]),
      // "전사 42% · 약 3분 남음": the percent alone would not say what it counts.
      lead: p?.status === "transcribing" && p.chunks > 0 ? "전사" : undefined,
      shimmer: summarizing,
      // Plan 5.1-12: the one reason a row says, since plugging in helps.
      note: p?.slow === "saver" ? "에너지 절약 모드 · 전원을 연결하면 빨라질 수 있습니다" : undefined,
      bar: p && line?.bar && (
        <ProgressBar
          value={p.percent}
          aria-label={`${line.label} (${name})`}
          className="absolute inset-x-0 bottom-0 h-px rounded-none"
        />
      ),
    };
  }
  if (now === "waiting") return { mark: <Dots />, text: statusLabel(m, p) };
  // Plan 5.4, 8: out of line until its route resolves. The summary model comes by itself; a
  // deleted connection needs another picked in its document.
  if (waitOf(m) === "connection_deleted") {
    return {
      mark: <TriangleAlert aria-hidden className="size-4 text-warn-line" />,
      text: "요약 대기",
      note: "이 회의에 정한 연결이 삭제됐습니다",
    };
  }
  if (waitOf(m) === "summary_model") return { text: "요약 대기" };
  if (m.status === "review") {
    return { mark: <span aria-hidden className="size-2 rounded-full bg-brand" />, text: STATUS_LABEL.review };
  }
  if (m.status === "sent") {
    return {
      mark: <Check aria-hidden className="size-4 text-ok-line" />,
      text: m.sent ? `${m.sent.channel}${ro(m.sent.channel)} 보냄` : STATUS_LABEL.sent,
    };
  }
  if (FAILED.includes(m.status)) {
    return {
      mark: <CircleAlert aria-hidden className="size-4 text-danger-line" />,
      text: STATUS_LABEL[m.status],
      // The full error is in the meeting's document.
      note: m.error ?? (m.status === "transcribe_failed" ? "전사하지 못했습니다" : "요약하지 못했습니다"),
    };
  }
  return { text: STATUS_LABEL[m.status] ?? m.status };
}

function Row({
  meeting: m,
  progress: p,
  day,
  action,
  morph,
  onPress,
  onOpen,
  children,
}: {
  meeting: MeetingItem;
  progress?: Progress;
  /** Set in "지금 할 일" for a meeting from another day. */
  day: string | null;
  /** `opens`: it opens the meeting, so pressing it may start the title's glide. */
  action: { label: string; run: (e: MouseEvent) => void; opens?: boolean } | null;
  /** Its title glides to or from its document (design-system 6.1). */
  morph: boolean;
  /** A pointer went down on something that opens it. */
  onPress: () => void;
  /** Opens the meeting; an interrupted recording has nothing to open. */
  onOpen?: (e: MouseEvent) => void;
  /** More buttons after the action (an interrupted recording's 삭제). */
  children?: ReactNode;
}) {
  const name = nameOf(m);
  const state = stateOf(m, p, name);
  const body = (
    <>
      <span className="w-16 shrink-0 leading-tight">
        <span className="block text-label text-ink-2 tabular-nums">{hhmm(m.created_at)}</span>
        {day && <span className="block text-caption text-ink-2">{day}</span>}
      </span>
      <span className="min-w-0 flex-1">
        {/* Its own box, gliding by position only, so the glyphs never stretch. Motion reads
            `layoutId` once, when the element is made, so a new one is made when it changes. */}
        <motion.span
          key={String(morph)}
          layoutId={morph ? `title-${m.id}` : undefined}
          layout={morph ? "position" : undefined}
          transition={titleMove}
          className={`block truncate text-body font-medium ${m.title ? "" : "text-ink-2"}`}
        >
          {name}
        </motion.span>
        {state.note && <span className="block truncate text-label text-ink-2">{state.note}</span>}
      </span>
      <span className="flex shrink-0 items-center gap-2 text-label text-ink-2 tabular-nums">
        {state.mark}
        {state.lead && <span>{state.lead}</span>}
        <span className={state.shimmer ? SHIMMER : undefined}>{state.text}</span>
      </span>
    </>
  );
  return (
    <motion.li variants={rowVariants} className="relative flex min-h-14 items-center gap-3 px-3 hover:bg-sunken">
      {onOpen ? (
        // The button stretches over the whole row (after:inset-0), so the progress line and the
        // note open the meeting too, without nesting the action in the button.
        <button
          className="flex min-w-0 flex-1 items-center gap-4 py-2.5 text-left outline-hidden after:absolute after:inset-0 focus-visible:after:ring-2 focus-visible:after:ring-ring focus-visible:after:ring-inset"
          onPointerDown={onPress}
          onClick={onOpen}
        >
          {body}
        </button>
      ) : (
        <div className="flex min-w-0 flex-1 items-center gap-4 py-2.5">{body}</div>
      )}
      {action && (
        <Button
          variant="outline"
          size="sm"
          className="relative"
          aria-label={`${action.label} (${name})`}
          onPointerDown={action.opens ? onPress : undefined}
          onClick={action.run}
        >
          {action.label}
        </Button>
      )}
      {children}
      {state.bar}
    </motion.li>
  );
}

// The first Home of the app's run lets its rows rise in; later ones (and re-renders) stand still.
let opened = false;

export default function Home({
  meetings,
  listFailed,
  progress,
  recording,
  starting,
  importing,
  morphFrom,
  onChanged,
  onOpen,
  onRecord,
  onStart,
  onImport,
  onSetup,
}: {
  meetings: MeetingItem[] | null;
  /** The list could not be read: Home still offers recording. */
  listFailed: boolean;
  progress: Record<string, Progress>;
  /** A recording is running: the main button goes back to it instead of starting another. */
  recording: boolean;
  /** A start was asked and the list does not show the recording yet. */
  starting: boolean;
  /** A recording file is being decoded (REC-12). */
  importing: boolean;
  /** The meeting just left by a click: its title glides back into its row. */
  morphFrom?: string | null;
  onChanged: () => void;
  /** `keyboard`: a key pressed the button, so nothing moves (design-system 5). */
  onOpen: (meeting: MeetingItem, keyboard: boolean) => void;
  /** To the recording screen. */
  onRecord: () => void;
  /** Starts a recording (App.tsx, which also listens for ⌘R). */
  onStart: () => void;
  /** Picks a recording file to import (App.tsx, so it outlasts this screen). */
  onImport: (e: MouseEvent) => void;
  onSetup: (tab: SettingsTab) => void;
}) {
  const [setup, setSetup] = useState<SetupItem[]>([]);
  // The model item: a file is missing (or the check failed; the 전사 tab says why), or the shared
  // download (ipc.ts) is running, with its last `model://download`.
  const [noModel, setNoModel] = useState(false);
  // Where the item's settings link goes: the summary model's tab when only it is missing.
  const [modelTab, setModelTab] = useState<SettingsTab>("transcription");
  const [downloading, setDownloading] = useState(false);
  const [received, setReceived] = useState<ModelDownload | null>(null);
  const [downloadFailed, setDownloadFailed] = useState(() => !!modelDownloadFailure());
  const [error, setError] = useState("");
  const first = useRef(!opened);
  // Rows mounted after the first rise (one moving to another group) just appear.
  const [entering, setEntering] = useState(first.current);
  useEffect(() => {
    opened = true;
  }, []);
  // The one row whose title shares its document's `layoutId`: the one pressed (a key never presses
  // one), or the meeting come back from. Only on an unscrolled page: App scrolls a new screen to
  // its top, and the title would fly from where the row no longer is.
  const [morph, setMorph] = useState(morphFrom ?? null);
  // A Home coming back while still on its way out keeps its state: the meeting come back from takes
  // the row, and the window starts again.
  useEffect(() => {
    if (morphFrom) setMorph(morphFrom);
  }, [morphFrom]);
  useEffect(() => {
    if (!morph) return;
    // ponytail: let go after the glide, so the row never glides again when it moves to another group.
    const t = window.setTimeout(() => setMorph(null), TITLE_GLIDE_MS);
    return () => window.clearTimeout(t);
  }, [morph, morphFrom]);

  const checkModel = () =>
    Promise.all([modelStatus(), modelStatus("transcription")])
      .then(([all, transcription]) => {
        setModelTab(transcription.files.every((f) => f.installed) ? "llm" : "transcription");
        return all.files.some((f) => !f.installed);
      })
      .catch(() => true)
      .then(setNoModel);

  // Checked again once the download settles: its last progress event comes before the hash check.
  const follow = (running: Promise<void>) => {
    setDownloading(true);
    running.then(() => {
      setDownloading(false);
      setReceived(null);
      setDownloadFailed(!!modelDownloadFailure());
      checkModel();
    });
  };

  // Recording works without any of these. The model comes first: nothing is transcribed without
  // it, so its item downloads right away (ONB-01); the 전사 tab has cancel, import and the mirror.
  // Summaries need nothing more (plan 5.8): with this PC as the default the summary model is in that
  // item (`modelStatus()`), and a saved default, local or cloud, is set up already.
  useEffect(() => {
    checkModel();
    const unlisten = onModelDownload(setReceived);
    unlisten.catch((e) => setError(`다운로드 진행률을 받지 못했습니다: ${e}`));
    const running = modelDownload();
    if (running) follow(running);
    getSettings()
      .then(({ settings }) =>
        setSetup(settings.slack_team ? [] : [{ action: "Slack 로그인", tab: "slack" }]),
      )
      // The settings screen shows the error and offers starting over.
      .catch(() => setSetup([{ text: "설정 파일을 읽을 수 없습니다", action: "설정 열기", tab: "general" }]));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  const resume = (id: string) => resumeMeeting(id).then(onChanged, (e) => setError(String(e)));

  // REC-12, on both the first screen and Home's head.
  const importButton = (className: string) => (
    <Button variant="ghost" className={className} disabled={importing} onClick={onImport}>
      <FileAudio aria-hidden />
      {importing ? "가져오는 중…" : "녹음 파일 가져오기"}
    </Button>
  );

  // One short line instead of a card (design-system 6.4): what transcribing, summarizing and sending
  // still need, each item its own way there. The model downloads in place; its tab (cancel, import,
  // the mirror) shows up as a quieter link while that matters.
  const download = () => follow(startModelDownload());
  // Plan 5.2-3: the time for every file left, not the speed.
  const note = received && downloadNote(received, received.all_total - received.all_received);
  const setupLine = (align: string) =>
    (noModel || downloading || setup.length > 0) && (
      <div className={`flex flex-wrap items-center gap-x-2 gap-y-1 text-label ${align}`}>
        <span className="text-ink-2">요약까지 하려면</span>
        <ul aria-label="남은 설정" className="flex flex-wrap items-center gap-x-2 gap-y-1">
          {(noModel || downloading) && (
            <li className={ITEM}>
              {downloading ? (
                <>
                  <span className="tabular-nums">
                    {[
                      `모델 받는 중 ${received?.all_total ? Math.floor((received.all_received / received.all_total) * 100) : 0}%`,
                      note && (note.stage || note.time),
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                  {/* Only when the stage changes: never the time. */}
                  <span role="status" className="sr-only">
                    {note?.stage || "모델 받는 중"}
                  </span>
                </>
              ) : downloadFailed ? (
                <>
                  <span>모델을 받지 못했습니다</span>
                  <Button variant="link" size="xs" onClick={download}>
                    다시 받기
                  </Button>
                </>
              ) : (
                <Button variant="link" size="xs" onClick={download}>
                  모델 받기
                </Button>
              )}
              {(downloading || downloadFailed) && (
                <Button variant="link" size="xs" className="text-ink-2 underline" onClick={() => onSetup(modelTab)}>
                  {modelTab === "llm" ? "요약(LLM) 설정" : "전사 설정"}
                </Button>
              )}
            </li>
          )}
          {setup.map((item) => (
            <li key={item.tab} className={ITEM}>
              {item.text && <span>{item.text}</span>}
              <Button variant="link" size="xs" onClick={() => onSetup(item.tab)}>
                {item.action}
              </Button>
            </li>
          ))}
        </ul>
      </div>
    );
  const errorBanner = error && <Banner kind="error">{error}</Banner>;

  // The first screen (design-system 6.5): no meetings yet. Seen once, so it carries one flourish,
  // the ring around the record button. No tour, no checklist: recording needs no setup.
  if (meetings?.length === 0) {
    return (
      // Centered in the window under the header, as tall as the room there so nothing scrolls. Half
      // of App's dock room (`pb-24`) is given back (`-mb-12`): the dock takes the bottom 80px, so it
      // still clears the content. The gaps follow the window's height (48px from 960 high, 36 at the
      // default 720, 30 at the smallest window, 600) and centering puts the rest above and below, so
      // a resized window never scrolls either.
      <main className="mx-auto -mb-12 box-content flex min-h-[calc(100dvh-6rem)] max-w-home flex-col items-center justify-center gap-[clamp(1rem,5dvh,3rem)] px-8 text-center">
        {errorBanner}
        <div className="space-y-4">
          <h1 tabIndex={-1} className="font-display text-display-l outline-hidden">
            <span className="block">회의가 끝나면,</span> <span className="block">기록은 이미 와 있어요.</span>
          </h1>
          <p className="text-ink-2">녹음만 하면 이 PC에서 전사하고, 요약을 검토해 Slack으로 보냅니다.</p>
        </div>
        <div className="flex flex-col items-center gap-3">
          <button
            className="group flex flex-col items-center gap-5 rounded-float p-3 disabled:opacity-50"
            disabled={starting}
            aria-keyshortcuts={SHORTCUT.start.aria}
            onClick={onStart}
          >
            <span aria-hidden className="relative grid size-24 place-items-center">
              <span className="absolute inset-0 rounded-full border border-ink/30 motion-safe:animate-ring" />
              <span className="absolute inset-0 rounded-full border border-ink/30 [animation-delay:-1.4s] motion-safe:animate-ring" />
              <span className="relative grid size-24 place-items-center rounded-full bg-ink motion-safe:transition-transform motion-safe:duration-200 motion-safe:group-hover:scale-[1.03] motion-safe:group-active:scale-[0.97]">
                <span className="size-7 rounded-full bg-rec" />
              </span>
            </span>
            <span className="flex items-center gap-2 font-semibold">
              첫 회의 녹음하기
              <Kbd>{SHORTCUT.start.label}</Kbd>
            </span>
          </button>
          {importButton("h-9 rounded-full text-ink-2")}
        </div>
        <ol className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-label text-ink-2">
          {["녹음", "이 PC에서 전사", "요약 검토", "Slack으로 보내기"].map((step, i) => (
            <li key={step} className="flex items-center gap-1.5">
              {i > 0 && <ChevronRight aria-hidden className="mr-0.5 size-3.5 text-ink-3" />}
              <span className="font-semibold text-ink tabular-nums">{i + 1}</span>
              {step}
            </li>
          ))}
        </ol>
        {setupLine("justify-center")}
      </main>
    );
  }

  // Until the list arrives there is nothing to show yet, unless it failed: recording still works.
  if (!meetings && !listFailed) return <main aria-busy className="min-h-40" />;

  const today = new Date();
  const actionOf = (m: MeetingItem, p: Progress | undefined) => {
    if (m.schema_version > SCHEMA_VERSION) return null;
    if (m.interrupted) return { label: "처리 계속", run: () => resume(m.id) };
    if (m.status === "recording") return { label: "녹음 화면으로", run: onRecord };
    // In line or running: it needs nothing from the recorder. A retried failure waits too.
    if (phase(m, p)) return null;
    if (m.status === "review") {
      return { label: "검토하기", run: (e: MouseEvent) => onOpen(m, e.detail === 0), opens: true };
    }
    if (m.status === "transcribe_failed") return { label: "다시 시도", run: () => resume(m.id) };
    // 요약 다시 실행 picks a connection (SUM-07), so it opens the meeting's document.
    if (m.status === "summarize_failed") {
      return { label: "요약 다시 실행", run: (e: MouseEvent) => onOpen(m, e.detail === 0), opens: true };
    }
    if (waitOf(m) === "connection_deleted") {
      return { label: "연결 고르기", run: (e: MouseEvent) => onOpen(m, e.detail === 0), opens: true };
    }
    return null;
  };
  const row = (m: MeetingItem, now: boolean) => (
    <Row
      key={m.id}
      meeting={m}
      progress={progress[m.id]}
      day={now ? shortDay(new Date(m.created_at), today) : null}
      action={actionOf(m, progress[m.id])}
      morph={morph === m.id}
      onPress={() => !m.interrupted && !window.scrollY && setMorph(m.id)}
      onOpen={m.interrupted ? undefined : (e) => onOpen(m, e.detail === 0)}
    >
      {m.interrupted && (
        <DeleteButton
          id={m.id}
          size="sm"
          label={`삭제 (${nameOf(m)})`}
          onDeleted={onChanged}
          onError={setError}
        />
      )}
    </Row>
  );

  // Each meeting once: "지금 할 일" first, the rest by day (the list comes newest first).
  const now = meetings?.filter((m) => needsYou(m, progress[m.id])) ?? [];
  const days = new Map<string, MeetingItem[]>();
  for (const m of meetings ?? []) {
    if (needsYou(m, progress[m.id])) continue;
    const day = dayGroup(new Date(m.created_at), today);
    days.set(day, [...(days.get(day) ?? []), m]);
  }
  const groups = [
    ...(now.length ? [{ name: "지금 할 일", rows: now, now: true }] : []),
    ...[...days].map(([name, rows]) => ({ name, rows, now: false })),
  ];

  return (
    <main className="mx-auto box-content max-w-home space-y-10 px-8 pt-10 pb-8">
      <div className="space-y-4">
        <header className="flex flex-wrap items-center justify-between gap-4">
          <h1 tabIndex={-1} className="font-display text-display-m outline-hidden">
            {today.toLocaleDateString("ko-KR", { month: "long", day: "numeric" })}{" "}
            <span className="font-sans text-title font-normal text-ink-2">
              {today.toLocaleDateString("ko-KR", { weekday: "long" })}
            </span>
          </h1>
          <div className="flex items-center gap-2">
            {importButton("h-10 rounded-full px-3.5")}
            <Button
              variant="ink"
              className="h-10 rounded-full pr-4 pl-3.5"
              disabled={starting}
              aria-keyshortcuts={recording ? undefined : SHORTCUT.start.aria}
              onClick={recording ? onRecord : onStart}
            >
              <span aria-hidden className={`size-2 rounded-full bg-rec ${recording ? "motion-safe:animate-breathe" : ""}`} />
              {recording ? "녹음 화면으로" : "녹음 시작"}
              {!recording && <Kbd className="ml-1">{SHORTCUT.start.label}</Kbd>}
            </Button>
          </div>
        </header>
        {setupLine("justify-start")}
        {errorBanner}
      </div>

      {meetings && (
        <motion.div
          variants={listVariants}
          initial={entering ? "hidden" : false}
          animate="show"
          onAnimationComplete={() => setEntering(false)}
          className="space-y-10"
        >
          {groups.map((g) => (
            <section key={g.name} aria-label={g.name}>
              <motion.h2 variants={rowVariants} className="flex items-baseline gap-2 text-title">
                {g.name}
                {g.now && (
                  <>
                    {" "}
                    <span className="font-normal text-ink-2 tabular-nums">{g.rows.length}</span>
                  </>
                )}
              </motion.h2>
              <ul className="-mx-3 mt-2 divide-y divide-line">{g.rows.map((m) => row(m, g.now))}</ul>
            </section>
          ))}
        </motion.div>
      )}
    </main>
  );
}
