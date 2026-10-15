import { useEffect, useRef, useState } from "react";
import { Archive, Copy, FileDown, Send, TriangleAlert } from "lucide-react";
import {
  Banner,
  DeleteButton,
  Field,
  mmss,
  NotYet,
  Page,
  TranscriptList,
} from "@/components/common";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  archiveMeeting,
  exportMarkdown,
  getMeeting,
  getTranscript,
  NEWER_VERSION,
  saveSummary,
  SCHEMA_VERSION,
  type MeetingDetail,
  type Segment,
} from "@/lib/ipc";

/** Autosave waits this long after the last keystroke. */
const SAVE_DELAY_MS = 800;

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

// LLM output is untrusted: title and summary are plain text in form fields, never HTML (D6, NFR-07).
export default function Review({
  id,
  onHome,
  onNotice,
}: {
  id: string;
  onHome: () => void;
  /** The app's banner: a save that fails after the screen has closed is told there. */
  onNotice: (message: string) => void;
}) {
  const s = savesOf(id);
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [saveState, setSaveState] = useState("");
  const [saveError, setSaveError] = useState("");
  const [segments, setSegments] = useState<Segment[] | null>(null);
  // A copy's result shows briefly next to its button; the saved `.md` path stays.
  const [copied, setCopied] = useState<{ at: "summary" | "transcript"; text: string } | null>(null);
  const [exported, setExported] = useState("");
  const [error, setError] = useState("");
  const timer = useRef(0);

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

  // Read after the last save of an earlier visit has landed. An edit that visit could not save
  // comes back into the fields and goes again.
  useEffect(() => {
    let live = true;
    s.last
      .catch(() => {})
      .then(() => getMeeting(id))
      .then(
        (d) => {
          if (!live) return;
          setDetail(d);
          const unsaved = s.pending;
          setTitle(unsaved?.title ?? d.meta.title);
          setBody(unsaved?.body ?? d.summary);
          if (unsaved) {
            setSaveState("저장 중…");
            flush().catch(() => {});
          }
        },
        (e) => live && setError(String(e)),
      );
    return () => {
      live = false;
    };
  }, [id]);

  // Leaving (홈으로, the header) saves a pending edit right away. The screen is gone by the time
  // it fails, so the app says so; reopening the meeting sends the edit again.
  useEffect(
    () => () =>
      void flush().catch((e) => onNotice(`검토 화면에서 고친 내용을 저장하지 못했습니다: ${e}`)),
    [],
  );

  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(null), 3000);
    return () => window.clearTimeout(t);
  }, [copied]);

  // Both act on what is on disk, so a pending edit goes first, and a failed save stops them.
  const archive = () => {
    setError("");
    flush().then(
      () => archiveMeeting(id).then(onHome, (e) => setError(String(e))),
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
  const copyText = (at: "summary" | "transcript", text: string) => {
    setError("");
    copy(text).then((note) => setCopied({ at, text: note }));
  };

  const meta = detail?.meta;
  const newer = !!meta && meta.schema_version > SCHEMA_VERSION;
  // `sent`, `archived` and a newer app's meeting stay read-only.
  const editable = meta?.status === "review" && !newer;
  // REV-01: the prompt writes "미정" for an owner or a date nobody named (architecture 6.2).
  const undecided = body
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.includes("미정"));

  return (
    <Page title="검토" onHome={onHome}>
      {error && <Banner kind="error">{error}</Banner>}
      {saveError && <Banner kind="error">{saveError}</Banner>}
      {meta && (
        <>
          {newer && <Banner kind="warning">{NEWER_VERSION}</Banner>}
          {meta.no_speech ? (
            // SUM-10 (6.5): no LLM was asked, so there is nothing to edit, copy or send.
            <Banner kind="warning">음성이 거의 감지되지 않았습니다</Banner>
          ) : (
            <>
              {/* SUM-07 */}
              {meta.llm_connection && (
                <p className="text-sm text-muted-foreground">
                  {meta.summarize_local
                    ? `이 PC의 ${meta.llm_connection}(으)로 요약했습니다`
                    : `전사본을 ${meta.llm_connection}(으)로 보내 요약했습니다`}
                </p>
              )}
              <Field label="제목">
                <Input
                  value={title}
                  readOnly={!editable}
                  onChange={(e) => edit({ title: e.target.value, body })}
                />
              </Field>
              <Field label="요약">
                <Textarea
                  className="h-96 field-sizing-fixed font-mono"
                  value={body}
                  readOnly={!editable}
                  onChange={(e) => edit({ title, body: e.target.value })}
                />
              </Field>
              <div className="flex justify-between gap-4 text-sm text-muted-foreground">
                <span>{saveState}</span>
                <span>{Array.from(body).length.toLocaleString("ko-KR")}자</span>
              </div>
              {/* Updated while typing, so a note rather than an alert that would interrupt. */}
              {undecided.length > 0 && (
                <Alert variant="warning" role="note">
                  <TriangleAlert aria-hidden />
                  <AlertTitle>{`담당자·기한이 미정인 항목 ${undecided.length}개`}</AlertTitle>
                  <AlertDescription>
                    {/* As written: the lines carry their own "- ". */}
                    <ul className="space-y-0.5">
                      {undecided.map((line, i) => (
                        <li key={i}>{line}</li>
                      ))}
                    </ul>
                  </AlertDescription>
                </Alert>
              )}
              <div className="flex flex-wrap items-center gap-2">
                <Button variant="outline" onClick={() => copyText("summary", `# ${title}\n\n${body}`)}>
                  <Copy aria-hidden />
                  요약 복사
                </Button>
                <Button variant="outline" onClick={exportFile}>
                  <FileDown aria-hidden />
                  .md로 저장
                </Button>
                {/* Two regions, so the path is not read out again when a copy result goes. */}
                <span role="status" className="text-sm text-muted-foreground">
                  {copied?.at === "summary" ? copied.text : ""}
                </span>
                <span role="status" className="text-sm text-muted-foreground">
                  {exported}
                </span>
              </div>
            </>
          )}
          {/* REV-01: always there to read, even with no speech. Read the first time it opens. */}
          <details
            className="rounded-lg border bg-card"
            onToggle={(event) => {
              if (event.currentTarget.open && !segments) {
                getTranscript(id).then(setSegments, (e) => setError(String(e)));
              }
            }}
          >
            <summary className="cursor-pointer rounded-lg px-4 py-3 font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring">
              전사본 보기
            </summary>
            <div className="space-y-4 border-t px-4 py-3">
              {segments && <TranscriptList segments={segments} />}
              {!!segments?.length && (
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    variant="outline"
                    onClick={() =>
                      copyText(
                        "transcript",
                        segments.map((seg) => `[${mmss(seg.start_ms)}] ${seg.text.trim()}`).join("\n"),
                      )
                    }
                  >
                    <Copy aria-hidden />
                    전사본 복사
                  </Button>
                  <span role="status" className="text-sm text-muted-foreground">
                    {copied?.at === "transcript" ? copied.text : ""}
                  </span>
                </div>
              )}
            </div>
          </details>
          <div className="flex flex-wrap items-center gap-2">
            {/* REV-02's exact count and the send itself come with slack.rs in M5. */}
            <NotYet hint="Slack 전송은 다음 단계에서 추가됩니다">
              <Button disabled>
                <Send aria-hidden />
                Slack으로 전송
              </Button>
            </NotYet>
            {meta.status === "review" && !newer && (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="outline">
                    <Archive aria-hidden />
                    전송 안 함
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>전송하지 않을까요?</AlertDialogTitle>
                    <AlertDialogDescription>
                      전송 안 함으로 두면 이 회의는 나중에 Slack으로 보낼 수 없습니다. 요약과
                      전사본은 남습니다.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>취소</AlertDialogCancel>
                    <AlertDialogAction onClick={archive}>전송 안 함</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            )}
            <DeleteButton
              id={id}
              onDeleted={() => {
                // Nothing is left to save, and leaving must not say otherwise.
                saves.delete(id);
                s.pending = null;
                s.last = Promise.resolve();
                onHome();
              }}
              onError={setError}
            />
          </div>
        </>
      )}
    </Page>
  );
}
