import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { BatteryLow, Check, ChevronRight, CircleAlert, Copy, Cpu, Lock, RotateCcw, Send, X } from "lucide-react";
import { AnimatePresence, motion, useTransform, type Variants } from "motion/react";
import {
  Bars,
  Dots,
  phase,
  Present,
  progressText,
  remaining,
  ro,
  statusLabel,
  stopping as stoppingOf,
  TranscriptList,
  useFollow,
} from "@/components/common";
import { LanguageChecks, transcribedIn, useLanguages, useLid } from "@/components/Languages";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  retranscribe,
  setMeetingLanguage,
  SLACK_MAX,
  type Channel,
  type Lang,
  type Meta,
  type Progress,
  type Segment,
  type SlowReason,
  type Status,
} from "@/lib/ipc";
import { dur, ease, exitDur, revealVariants, sectionVariants, spring } from "@/lib/motion";
import { cn } from "@/lib/utils";

// The meeting document's parts (design-system 6.6). Meeting.tsx holds the state and the IPC.

/** The summary's type (`body-doc`). The editor's textarea and the layer under it wrap alike only
 * when everything that places a glyph is the same: font, size, line height, spacing, padding,
 * width, white-space, word-break, overflow-wrap and tab size. The bottom padding takes a serif's
 * glyphs reaching a pixel or two past the last line, so the textarea has nothing to scroll. */
export const BODY =
  "block w-full p-0 pb-1 font-sans text-body-doc text-ink whitespace-pre-wrap break-keep wrap-break-word [tab-size:4]";
/** What is going on while the summary is still to come (design-system 6.6), in the title's place. */
const WAITING_LINES = {
  transcribing: ["열심히 듣는 중…", "말을 글로 옮기는 중…", "조금만 기다려 주세요…"],
  summarizing: ["열심히 읽는 중…", "중요한 내용을 고르는 중…", "조금만 기다려 주세요…"],
  waiting: ["차례를 기다리는 중…", "조금만 기다려 주세요…"],
};
export type WaitingStage = keyof typeof WAITING_LINES;
/** Each line stays this long. */
export const LINE_MS = 2000;

/** The title's place until there is one: a new line every 2 s, crossfading (opacity only, so
 * Reduce Motion keeps it). A screen reader hears one steady name instead of a change every 2 s. */
export function WaitingTitle({ stage }: { stage: WaitingStage }) {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    setTick(0);
    const timer = window.setInterval(() => setTick((n) => n + 1), LINE_MS);
    return () => window.clearInterval(timer);
  }, [stage]);
  const lines = WAITING_LINES[stage];
  const line = lines[tick % lines.length];
  return (
    <>
      <span className="sr-only">제목 준비 중</span>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={`${stage}:${line}`}
          aria-hidden
          className="inline-block"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: dur.medium }}
        >
          {line}
        </motion.span>
      </AnimatePresence>
    </>
  );
}

/** The title's: serif `display-l`, wrapping like the body. */
export const TITLE =
  "block w-full p-0 pb-1 font-display text-display-l text-ink whitespace-pre-wrap break-keep wrap-break-word";

/** The marker (REV-01). The glyphs keep their weight and box: it reaches past them as a shadow,
 * which moves nothing. */
const MARK = "rounded-[3px] bg-marker shadow-[0_0_0_2px_var(--color-marker)]";

/** "미정" on the marker, as React text nodes only (architecture 10.4). */
function highlight(text: string) {
  return text.split("미정").flatMap((part, i) =>
    i
      ? [
          <mark key={i} className={cn(MARK, "text-inherit")}>
            미정
          </mark>,
          part,
        ]
      : [part],
  );
}

/** A textarea as tall as its text, so the page scrolls and never the field: a mirror in the flow
 * holds the height (WebKit has no `field-sizing`), and the textarea lies over it with a clear
 * background. With `marks` the mirror also paints 미정 under the glyphs. It is `aria-hidden`; caret
 * and selection stay the textarea's. `children` come after the textarea, for a `peer-focus:` mark.
 * Spans, so it can sit in a heading. */
export function Grow({
  value,
  marks,
  className,
  children,
  ...props
}: ComponentProps<"textarea"> & { value: string; marks?: boolean; children?: ReactNode }) {
  return (
    <span className="relative block">
      <span aria-hidden className={cn(className, "text-transparent")}>
        {marks ? highlight(value) : value}
        {/* A last empty line has no box without something on it. */}
        {"\u200b"}
      </span>
      <textarea
        {...props}
        value={value}
        rows={1}
        // ponytail: the bottom padding should leave nothing to scroll; kept in case a webview still
        // scrolls to the caret, which would lift the text off its mirror.
        onScroll={(e) => (e.currentTarget.scrollTop = 0)}
        className={cn(className, "peer absolute inset-0 resize-none overflow-hidden bg-transparent outline-hidden placeholder:text-ink-2")}
      />
      {children}
    </span>
  );
}

/** Where the caret is on screen, read off a `Grow`'s mirror, which holds the same text in the same
 * place: a textarea has no way to say. The box is the character after the caret's (a collapsed
 * range has none in some engines); at the very end the mirror's zero-width space stands in. With a
 * selection, the caret is its moving end: the start when it grows backward (Shift+Up). */
export function caretRect(field: HTMLTextAreaElement) {
  const walk = document.createTreeWalker(field.previousElementSibling!, NodeFilter.SHOW_TEXT);
  let left = field.selectionDirection === "backward" ? field.selectionStart : field.selectionEnd;
  for (let node = walk.nextNode(); node; node = walk.nextNode()) {
    const length = node.textContent!.length;
    if (left < length) {
      const range = document.createRange();
      range.setStart(node, left);
      range.setEnd(node, left + 1);
      return range.getBoundingClientRect();
    }
    left -= length;
  }
}

/** One line of a summary as the document shows it, with its line number in the text, so a click
 * can put the caret there. */
export type Block =
  | { kind: "heading"; line: number; text: string }
  | { kind: "item"; line: number; owner: string | null; text: string; due: string | null }
  | { kind: "none"; line: number }
  | { kind: "text"; line: number; text: string }
  | { kind: "gap"; line: number };

/** Our own line parser, not a Markdown library (architecture 10.4). It knows the prompt's few
 * shapes (architecture 6.2): `#` headings, `- [담당자] 할 일 (기한)` and `- 없음`. Everything else,
 * bold, links and HTML included, stays the text it is. The grounding check (grounding.rs) reads lines
 * the same way: change both with `src-tauri/tests/fixtures/summary-lines.json`, which both test. */
export function parseDoc(text: string): Block[] {
  return text.split("\n").map((raw, line): Block => {
    const heading = /^#{1,3} (.*)/s.exec(raw);
    if (heading) return { kind: "heading", line, text: heading[1] };
    const item = /^[-*•] (.*)/s.exec(raw);
    if (!item) return raw.trim() ? { kind: "text", line, text: raw } : { kind: "gap", line };
    if (item[1].trim() === "없음") return { kind: "none", line };
    // A leading [owner] (but not a link's [text](url)) and a trailing (due). Always matches.
    const [, owner = null, rest, due = null] = /^(?:\[([^\]]+)\](?!\()\s*)?(.*?)(?:\s*\(([^()]+)\))?\s*$/s.exec(
      item[1],
    )!;
    return { kind: "item", line, owner, text: rest, due };
  });
}

function Line({ block: b }: { block: Exclude<Block, { kind: "item" }> }) {
  if (b.kind === "heading") {
    return (
      <h2 data-line={b.line} className="mb-1 text-title">
        {highlight(b.text)}
      </h2>
    );
  }
  if (b.kind === "none") {
    return (
      <p data-line={b.line} className="text-ink-2">
        없음
      </p>
    );
  }
  if (b.kind === "gap") return <div data-line={b.line} aria-hidden className="h-3" />;
  return <p data-line={b.line}>{highlight(b.text)}</p>;
}

/** A list item: a quiet bullet with a hanging indent, the owner as a chip, the due date after it.
 * Nobody named (미정) is on the marker: the whole chip, or the 미정 among names ("민수, 미정"). */
function Item({ block: b }: { block: Extract<Block, { kind: "item" }> }) {
  return (
    <li data-line={b.line} className="relative pl-5">
      <span aria-hidden className="absolute top-3 left-1.5 size-1 rounded-full bg-ink-3" />
      {b.owner && (
        <span
          className={cn(
            "mr-1.5 rounded-full px-2 py-0.5 text-label whitespace-nowrap",
            b.owner === "미정" ? "bg-marker" : "bg-sunken",
          )}
        >
          {b.owner === "미정" ? b.owner : highlight(b.owner)}
        </span>
      )}
      {highlight(b.text)}
      {b.due && (
        <span
          className={cn("ml-1.5 text-caption whitespace-nowrap", b.due === "미정" ? cn(MARK, "text-ink") : "text-ink-2")}
        >
          {b.due}
        </span>
      )}
    </li>
  );
}

/** One heading's part of the document; a run of items is one list. */
function Section({ blocks }: { blocks: Block[] }) {
  const parts: ReactNode[] = [];
  for (let i = 0; i < blocks.length; ) {
    const items: Extract<Block, { kind: "item" }>[] = [];
    for (let b = blocks[i]; b?.kind === "item"; b = blocks[++i]) items.push(b);
    if (items.length) {
      parts.push(
        <ul key={items[0].line} className="space-y-1">
          {items.map((b) => (
            <Item key={b.line} block={b} />
          ))}
        </ul>,
      );
    } else {
      const b = blocks[i++] as Exclude<Block, { kind: "item" }>;
      parts.push(<Line key={b.line} block={b} />);
    }
  }
  return parts;
}

/** The summary as a document to read (design-system 6.6), from `parseDoc`: React elements and text
 * nodes only, never HTML (NFR-07). `reveal` brings it in section by section, once. With `onEdit`, a
 * click on a line (not a selection being made) asks for the editor there. */
export function DocView({
  text,
  reveal,
  onRevealed,
  onEdit,
}: {
  text: string;
  reveal?: boolean;
  onRevealed?: () => void;
  /** The line clicked, and where its top is on screen. */
  onEdit?: (line: number, top?: number) => void;
}) {
  const sections: Block[][] = [];
  for (const b of parseDoc(text)) {
    if (b.kind === "heading" || !sections.length) sections.push([]);
    sections[sections.length - 1].push(b);
  }
  return (
    <motion.div
      variants={revealVariants}
      initial={reveal ? "hidden" : false}
      animate="show"
      onAnimationComplete={onRevealed}
      onClick={
        onEdit &&
        ((e) => {
          if (!getSelection()?.isCollapsed) return;
          const line = (e.target as Element).closest<HTMLElement>("[data-line]");
          onEdit(Number(line?.dataset.line ?? 0), line?.getBoundingClientRect().top);
        })
      }
      className="font-sans text-body-doc text-ink whitespace-pre-wrap break-keep wrap-break-word"
    >
      {sections.map((blocks, i) => (
        <motion.div key={i} variants={sectionVariants} className="not-first:mt-7">
          <Section blocks={blocks} />
        </motion.div>
      ))}
    </motion.div>
  );
}

const STEPS = ["녹음", "전사", "요약", "검토"];
/** Plan 5.3-15: what the recorder can do about a slow PC itself. */
const LATER = " · 일시정지하고 나중에 이어서 처리할 수 있습니다";
/** Plan 5.1-12: why it is slow, and what may help. */
const SLOW: Record<SlowReason, string> = {
  saver: "에너지 절약 모드라 처리가 느릴 수 있습니다. 전원을 연결하면 빨라질 수 있습니다",
  cpu: "GPU를 쓰지 못해 CPU로 처리하고 있어 느립니다. 그래픽 드라이버를 업데이트하면 나아질 수 있습니다",
  cpu_nogpu: `이 PC에는 쓸 수 있는 GPU가 없어 CPU로 처리합니다${LATER}`,
  slow: `이 PC에서는 전사가 회의 길이보다 오래 걸립니다${LATER}`,
};
/** Plan 5.3: the processing area's one button, and what it says once it changed. */
export const PAUSE = {
  pause: "일시정지",
  stopping: "멈추는 중…",
  resume: "이어서 처리",
  held: "일시정지됨",
  paused: "일시정지했습니다",
  resumed: "이어서 처리합니다",
};
// Summarizing is next or under way. A `paused` meeting with a connection stopped while summarizing.
const SUMMARY_STEP: Status[] = ["transcribed", "summarizing", "summarize_failed"];

/** A percent that rolls up from the last one, drawn from its MotionValue. */
function Ticker({ value }: { value: number }) {
  const text = useTransform(useFollow(value), (v) => `${Math.round(v)}%`);
  return <motion.span className="tabular-nums">{text}</motion.span>;
}

/** Where a meeting stands before review: 녹음 ✓ › 전사 42% › 요약 › 검토, then a line of detail.
 * Plain text, never a live region; a screen reader gets the value from a hidden progress bar. With
 * `onPause` the line ends in plan 5.3's button: 일시정지, 멈추는 중… until the run ends, 이어서
 * 처리. One button whose words change, so the focus stays on it. */
export function Steps({
  meta,
  progress: p,
  failed,
  recording = false,
  named = [],
  onPause,
  onResume,
}: {
  meta: Meta;
  progress?: Progress;
  failed: boolean;
  /** The names of its languages, in table order (plan 5.6-27). */
  named?: string[];
  /** A recording runs: nothing starts until it ends (TRN-09). */
  recording?: boolean;
  onPause?: () => void;
  onResume?: () => void;
}) {
  const now = phase(meta, p);
  const held = now === "held";
  const stopping = stoppingOf(meta, p);
  // An event saying it runs wins over the stored status, as in `phase`: read as `transcribed` while
  // the last event is still transcribing, it is still at 전사.
  const running = p?.status === "transcribing" || p?.status === "summarizing" ? p.status : null;
  const total = meta.chunks_total ?? 0;
  // Paused after its last chunk, before its summary started: its transcript is done.
  const transcribed = total > 0 && meta.chunks_done >= total;
  const summary = running
    ? running === "summarizing"
    : SUMMARY_STEP.includes(meta.status) || (meta.status === "paused" && (!!meta.llm_connection || transcribed));
  const at = summary ? 2 : 1;
  const transcribing = now === "running" && at === 1;
  const counted = transcribing && p?.status === "transcribing" && p.chunks > 0;
  const line = now === "running" && p ? progressText(p) : null;
  // Plan 5.1-12: only saver goes on into the summary.
  const slow = now === "running" && p?.slow && (!summary || p.slow === "saver") ? p.slow : null;
  // Plan 5.6-27: a mixed meeting waits in line for the detection model.
  const lid = useLid(now === "waiting" && meta.languages.length > 1);
  const chunking = transcribing && p?.status === "transcribing";
  // Plan 5.3-12: where it stopped, from `meta.json`, so a restart draws it too.
  const stoppedIn = summary
    ? "요약 중 멈춤 · 끝난 부분부터 이어서 합니다"
    : total > 0
      ? `구간 ${Math.min(meta.chunks_done + 1, total)}/${total}에서 멈춤 · 이어서 처리하면 이 구간부터 다시 합니다`
      : "구간을 나누기 전에 멈춤 · 이어서 처리하면 처음부터 합니다";
  const note = held
    ? stoppedIn
    : [
        now === "waiting" && statusLabel(meta, p),
        now === "waiting" && recording && "녹음이 끝나면 처리합니다",
        counted && `구간 ${Math.min(p.chunk + 1, p.chunks)}/${p.chunks}`,
        counted && p.eta_secs != null && remaining(p.eta_secs),
        chunking && !counted && "구간 나누는 중",
        // Every meeting's first chunk says its languages, so a wrong pick shows early.
        chunking && p.chunk === 0 && named.length > 0 && transcribedIn(named),
        chunking && p.detecting && "언어를 가리는 중",
        lid && (lid.failure || "언어 판별 모델을 받는 중"),
        // The reason says the CPU already.
        slow !== "cpu" && slow !== "cpu_nogpu" && meta.backend && `처리 장치 ${meta.backend.toUpperCase()}`,
      ]
        .filter(Boolean)
        .join(" · ");
  // The transcription's value where it stopped, for the hidden progress bar.
  const stoppedAt = held && !summary && total > 0 ? Math.round((meta.chunks_done / total) * 100) : null;
  const action = held ? PAUSE.resume : stopping ? PAUSE.stopping : now ? PAUSE.pause : null;
  // Said once when it changes, politely; never on the first render.
  const [said, setSaid] = useState("");
  const was = useRef(held);
  useEffect(() => {
    if (was.current !== held) setSaid(held ? PAUSE.paused : PAUSE.resumed);
    was.current = held;
  }, [held]);
  const mark = failed ? (
    <CircleAlert aria-hidden className="size-3.5 text-danger-line" />
  ) : now === "waiting" ? (
    <Dots />
  ) : transcribing ? (
    <Bars className="h-3 text-brand" />
  ) : null;
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-3">
        <ol aria-label="처리 단계" className="flex flex-wrap items-center gap-x-2 gap-y-1 text-label text-ink-2">
          {STEPS.map((step, i) => (
            <li
              key={step}
              aria-current={i === at ? "step" : undefined}
              className={`flex items-center gap-1.5 ${i === at ? "text-ink" : ""}`}
            >
              {i > 0 && <ChevronRight aria-hidden className="mr-0.5 size-3.5 text-ink-3" />}
              {i === at && mark}
              {step}
              {i < at && (
                <>
                  <Check aria-hidden className="size-3.5" />
                  <span className="sr-only">완료</span>
                </>
              )}
              {i === at && counted && <Ticker value={p.percent} />}
              {i === at && failed && <span>실패</span>}
              {i === at && held && <span>{PAUSE.held}</span>}
            </li>
          ))}
        </ol>
        {onPause && action && (
          <Button
            variant="ghost"
            size="sm"
            aria-disabled={stopping || undefined}
            className="-my-1 aria-disabled:opacity-50"
            onClick={() => {
              if (stopping) return;
              if (held) onResume?.();
              else onPause();
            }}
          >
            {action}
          </Button>
        )}
      </div>
      {note && <p className="text-caption text-ink-2 tabular-nums">{note}</p>}
      {lid?.failure && (
        <Button variant="outline" size="sm" onClick={lid.retry}>
          <RotateCcw aria-hidden />
          다시 시도
        </Button>
      )}
      <AnimatePresence initial={false} mode="wait">
        {slow && (
          <motion.p
            key={slow}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: dur.small }}
            className="flex items-start gap-1.5 text-caption text-ink-2"
          >
            {slow === "saver" ? (
              <BatteryLow aria-hidden className="mt-px size-3.5 shrink-0" />
            ) : (
              <Cpu aria-hidden className="mt-px size-3.5 shrink-0" />
            )}
            {SLOW[slow]}
          </motion.p>
        )}
      </AnimatePresence>
      {p && line?.bar && (
        <span
          role="progressbar"
          aria-label={line.label}
          aria-valuenow={p.percent}
          aria-valuemin={0}
          aria-valuemax={100}
          className="sr-only"
        />
      )}
      {stoppedAt != null && (
        <span
          role="progressbar"
          aria-label="전사 진행률"
          aria-valuenow={stoppedAt}
          aria-valuemin={0}
          aria-valuemax={100}
          className="sr-only"
        />
      )}
      <span role="status" className="sr-only">
        {said}
      </span>
    </div>
  );
}

// Heading and line widths (%) of five sections, roughly a summary's (architecture 6.2).
const SKELETON = [
  [16, 92, 74],
  [20, 88, 80, 58],
  [14, 95, 84, 66],
  [18, 72],
  [22, 86],
];
// A sweep on each bar, never over the paper between them. Only without Reduce Motion.
const SWEEP =
  "relative overflow-hidden after:pointer-events-none after:absolute after:inset-0 after:-translate-x-full after:bg-linear-to-r after:from-transparent after:via-surface/80 after:to-transparent motion-safe:after:animate-shimmer";

/** Where the summary will be: five faint sections. A sweep passes over them while it is written. */
export function Skeleton({ writing }: { writing: boolean }) {
  const bar = `rounded-full bg-sunken ${writing ? SWEEP : ""}`;
  return (
    <div aria-hidden className={`space-y-8 ${writing ? "" : "opacity-60"}`}>
      {SKELETON.map(([head, ...lines], i) => (
        <div key={i} className="space-y-3">
          <div className={`h-4 ${bar}`} style={{ width: `${head}%` }} />
          {lines.map((width, j) => (
            <div key={j} className={`h-3 ${bar}`} style={{ width: `${width}%` }} />
          ))}
        </div>
      ))}
    </div>
  );
}

const instantly = { duration: 0 };
// `custom` is whether a key opened or closed it: keyboard actions do not move (design-system 5).
const PANEL: Variants = {
  out: (instant: boolean) => ({
    x: 24,
    opacity: 0,
    transition: instant ? instantly : { duration: exitDur(0.24), ease: ease.in },
  }),
  in: (instant: boolean) => ({ x: 0, opacity: 1, transition: instant ? instantly : { duration: 0.24, ease: ease.out } }),
};

function PanelBody({
  segments,
  languages,
  langs,
  error,
  copied,
  onCopy,
  onClose,
}: {
  segments: Segment[] | null;
  /** The meeting's, and the table that names them (plan 5.6-28). */
  languages: string[];
  langs: Lang[] | null;
  error: string;
  copied: string;
  onCopy: () => void;
  onClose: () => void;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  // Focus moves in when it opens; Meeting puts it back on 전사본 when it closes.
  useEffect(() => heading.current?.focus(), []);
  return (
    <>
      <div className="flex items-center justify-between gap-2 border-b border-line py-2 pr-2 pl-5">
        <h2 ref={heading} tabIndex={-1} className="text-body font-semibold outline-hidden">
          전사본
        </h2>
        <div className="flex items-center gap-1">
          <span role="status" className="text-caption text-ink-2">
            {copied}
          </span>
          {!!segments?.length && (
            <Button variant="ghost" size="sm" onClick={onCopy}>
              <Copy aria-hidden />
              전사본 복사
            </Button>
          )}
          <Button variant="ghost" size="icon-sm" aria-label="닫기" onClick={onClose}>
            <X aria-hidden />
          </Button>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {error ? (
          <p className="text-ink-2">{error}</p>
        ) : segments ? (
          <TranscriptList segments={segments} languages={languages} langs={langs} />
        ) : (
          <p aria-busy className="text-ink-2">
            읽는 중…
          </p>
        )}
      </div>
    </>
  );
}

/** The transcript beside the document (design-system 6.6): one mixed track, so lines, not a
 * 나/상대 chat. On the body, since a screen moving under it would carry a `fixed` panel along. */
export function TranscriptPanel({
  open,
  instant,
  ...body
}: ComponentProps<typeof PanelBody> & { open: boolean; instant: boolean }) {
  return createPortal(
    <AnimatePresence custom={instant}>
      {open && (
        <motion.aside
          key="transcript"
          aria-label="전사본"
          custom={instant}
          variants={PANEL}
          initial="out"
          animate="in"
          exit="out"
          className="fixed top-14 right-3 bottom-3 z-30 flex w-100 max-w-[calc(100vw-1.5rem)] flex-col rounded-float bg-surface shadow-float"
        >
          <Present className="flex min-h-0 flex-1 flex-col">
            <PanelBody {...body} />
          </Present>
        </motion.aside>
      )}
    </AnimatePresence>,
    document.body,
  );
}

const ARC = { cx: 12, cy: 12, r: 9, fill: "none", strokeWidth: 2.5 };

/** REV-02 as a ring filling toward 12,000: `warn-line` from 90%, `danger-line` past it. The count
 * is in words for screen readers and on hover. */
function Gauge({ length }: { length: number }) {
  const ratio = Math.min(1, length / SLACK_MAX);
  const path = useTransform(useFollow(ratio * 100), (v) => v / 100);
  const tone = length > SLACK_MAX ? "stroke-danger-line" : ratio >= 0.9 ? "stroke-warn-line" : "stroke-brand";
  const text = `${length.toLocaleString("ko-KR")} / ${SLACK_MAX.toLocaleString("ko-KR")}자 (Slack 전송 기준)`;
  return (
    <span title={text} className="grid size-8 shrink-0 place-items-center">
      <svg aria-hidden viewBox="0 0 24 24" className="size-6 -rotate-90">
        <circle {...ARC} className="stroke-line" />
        <motion.circle {...ARC} className={tone} style={{ pathLength: path }} />
      </svg>
      <span className="sr-only">{text}</span>
    </span>
  );
}

/** The send bar (design-system 6.6): a white pill where the dock sits, above it when both show. */
export function SendBar({
  shown,
  signedIn,
  channels,
  channel,
  length,
  inFlight,
  failed,
  canSend,
  sentTo,
  onChannel,
  onSend,
  onSetup,
}: {
  shown: boolean;
  /** Signed in to Slack (ONB-04); else the bar leads to the sign-in. */
  signedIn: boolean;
  /** The recorder's channels (SND-04), null until read. */
  channels: Channel[] | null;
  channel: Channel | null;
  length: number | null;
  inFlight: boolean;
  /** The last send failed (SND-06): the button sends again. */
  failed: boolean;
  canSend: boolean;
  /** Sent while open: the button turns into where it went. */
  sentTo?: string;
  onChannel: (id: string) => void;
  onSend: () => void;
  onSetup: () => void;
}) {
  const over = Math.max(0, (length ?? 0) - SLACK_MAX);
  const label = sentTo
    ? `${sentTo}${ro(sentTo)} 보냄`
    : inFlight
      ? "보내는 중…"
      : failed
        ? "다시 보내기"
        : channel
          ? `#${channel.name}${ro(channel.name)} 보내기`
          : "Slack으로 보내기";
  // Sent from the keyboard: `disabled` dropped the focus meanwhile, so it comes back when the send
  // ends, sent or failed.
  const button = useRef<HTMLButtonElement>(null);
  const refocus = useRef(false);
  useEffect(() => {
    if (inFlight || !refocus.current) return;
    refocus.current = false;
    button.current?.focus();
  }, [inFlight]);
  return (
    <AnimatePresence>
      {shown && (
        <motion.div
          key="send"
          // Slides up and down with the dock coming and going under it.
          layout="position"
          initial={{ y: 24, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: 24, opacity: 0 }}
          transition={{ ...spring.dock, opacity: { duration: dur.small } }}
          style={{ borderRadius: 28 }}
          className="pointer-events-auto flex h-14 max-w-full items-center gap-2 bg-surface p-2 shadow-float"
        >
          <Present className="contents">
            {/* SND-04: the recorder's channels; none is picked for them the first time. */}
            {!sentTo && signedIn && (
              <Select value={channel?.id ?? ""} disabled={inFlight || !channels} onValueChange={onChannel}>
                <SelectTrigger
                  aria-label="보낼 채널"
                  className="h-10 max-w-56 rounded-full border-transparent bg-transparent pl-4 hover:bg-sunken"
                >
                  <SelectValue
                    placeholder={!channels ? "채널 읽는 중…" : channels.length ? "채널 고르기" : "채널 없음"}
                  />
                </SelectTrigger>
                <SelectContent position="popper" side="top" className="max-h-80">
                  {channels?.map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {`#${c.name}`}
                      {c.private && (
                        <>
                          <Lock aria-hidden className="size-3.5 text-ink-2" />
                          <span className="sr-only">, 비공개 채널</span>
                        </>
                      )}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            {!sentTo && !signedIn && (
              <Button variant="link" size="sm" onClick={onSetup}>
                Slack 로그인하기
              </Button>
            )}
            {!sentTo && length != null && <Gauge length={length} />}
            {/* The warning in words too, from 90%, not only the ring's color (NFR-09). */}
            {!sentTo && length != null && length / SLACK_MAX >= 0.9 && over === 0 && (
              <span className="text-label text-ink-2 tabular-nums">{`${(SLACK_MAX - length).toLocaleString("ko-KR")}자 남음`}</span>
            )}
            {!sentTo && over > 0 && (
              <span className="text-label">{`${over.toLocaleString("ko-KR")}자를 줄여 주세요`}</span>
            )}
            {/* Named by `aria-label`: while the label swaps, both labels are in the button. */}
            <Button
              ref={button}
              aria-label={label}
              className={cn("relative h-10 rounded-full px-4", sentTo && "bg-transparent text-ink hover:bg-transparent")}
              disabled={!sentTo && !canSend}
              aria-disabled={sentTo ? true : undefined}
              onClick={
                sentTo
                  ? undefined
                  : (e) => {
                      refocus.current = document.activeElement === e.currentTarget;
                      onSend();
                    }
              }
            >
              {/* On success the plane flies off up and right (300ms), then the check comes in. */}
              <span className="relative size-4">
                <motion.span
                  className="absolute inset-0"
                  initial={false}
                  animate={sentTo ? { x: 14, y: -14, opacity: 0 } : { x: 0, y: 0, opacity: 1 }}
                  transition={{ duration: 0.3, ease: ease.in }}
                >
                  <Send aria-hidden />
                </motion.span>
                <motion.span
                  className="absolute inset-0"
                  initial={false}
                  animate={{ opacity: sentTo ? 1 : 0 }}
                  transition={{ delay: sentTo ? 0.15 : 0, duration: dur.small }}
                >
                  <Check aria-hidden className="text-ok-line" />
                </motion.span>
              </span>
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={sentTo ? "sent" : "send"}
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: dur.micro }}
                >
                  {label}
                </motion.span>
              </AnimatePresence>
            </Button>
            <span role="status" className="sr-only">
              {sentTo ? `${sentTo}${ro(sentTo)} 보냈습니다` : ""}
            </span>
          </Present>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

const same = (a: string[], b: string[]) => a.join() === b.join();
// queue::TRANSCRIBED: a chunk finished while the dialog was open (plan 5.7-6).
const TRANSCRIBED_REFUSAL = "이미 전사한 구간이 있어 다시 전사해야 바꿀 수 있습니다";

/** Plan 5.6-24 (design-system 8.7): "전사 언어 바꾸기". While it runs it only says why, with
 * 일시정지, and is drawn again once it stops. Before any chunk is done it just changes the list;
 * after, it deletes the results and transcribes again (E3). Unchanged, neither can be pressed. */
export function LanguageDialog({
  meta,
  open,
  running,
  stopping,
  transcribed,
  onOpenChange,
  onCloseAutoFocus,
  onPause,
  onChanged,
}: {
  meta: Meta;
  open: boolean;
  running: boolean;
  /** Paused, its run not ended yet. */
  stopping: boolean;
  /** A chunk is done or the transcript exists. */
  transcribed: boolean;
  onOpenChange: (open: boolean) => void;
  onCloseAutoFocus: (event: Event) => void;
  onPause: () => void;
  onChanged: (meta: Meta) => void;
}) {
  const langs = useLanguages();
  const [draft, setDraft] = useState(meta.languages);
  // Rust found a chunk done since it opened: redrawn as E3 instead of an error.
  const [late, setLate] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    if (!open) return;
    setDraft(meta.languages);
    setLate(false);
    setError("");
  }, [open]);
  const redo = transcribed || late;
  const confirm = () => {
    setBusy(true);
    setError("");
    (redo ? retranscribe : setMeetingLanguage)(meta.id, draft)
      .then(
        (next) => {
          onOpenChange(false);
          onChanged(next);
        },
        (e) => (!redo && String(e).includes(TRANSCRIBED_REFUSAL) ? setLate(true) : setError(String(e))),
      )
      .finally(() => setBusy(false));
  };
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent
        onCloseAutoFocus={onCloseAutoFocus}
        // Before any chunk is done it has nothing to warn of.
        {...(running || redo ? {} : { "aria-describedby": undefined })}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>전사 언어 바꾸기</AlertDialogTitle>
          {(running || redo) && (
            <AlertDialogDescription>
              {running ? "처리 중에는 바꿀 수 없습니다" : "전사본과 요약, 검토에서 고친 내용이 지워집니다"}
            </AlertDialogDescription>
          )}
        </AlertDialogHeader>
        {!running && langs && <LanguageChecks langs={langs} value={draft} onChange={setDraft} />}
        {error && <p className="text-caption text-danger-line">{error}</p>}
        <AlertDialogFooter>
          <AlertDialogCancel>취소</AlertDialogCancel>
          {running ? (
            <Button
              aria-disabled={stopping || undefined}
              className="aria-disabled:opacity-50"
              onClick={() => !stopping && onPause()}
            >
              {stopping ? PAUSE.stopping : PAUSE.pause}
            </Button>
          ) : (
            <Button
              variant={redo ? "destructive" : "default"}
              disabled={busy || same(draft, meta.languages)}
              onClick={confirm}
            >
              {redo ? "지우고 다시 전사" : "바꾸기"}
            </Button>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
