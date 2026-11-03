import { useEffect, useRef, useState } from "react";
import { Square } from "lucide-react";
import { AnimatePresence, motion, useTransform } from "motion/react";
import {
  clock,
  engineStarting,
  hhmm,
  Kbd,
  phase,
  Present,
  RollingTime,
  SHORTCUT,
  sinceStart,
  useFollow,
} from "@/components/common";
import { onRecordingLevel, type ImportProgress, type MeetingItem, type Progress } from "@/lib/ipc";
import { dur, spring } from "@/lib/motion";

// The dock (design-system 6.2): the app's one live element, an ink pill at the bottom. It records,
// or it processes, or it is gone. White focus rings: the brand one is 3.2:1 on ink.
const PART = "flex h-10 items-center rounded-full focus-visible:outline-white";

/** 12 bars, the low bands in the middle: a voice swells from the center out. */
const MIRROR = [5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5];
const QUIET = { transform: "scaleY(0.15)" };

/** Recording: the breathing dot, a mini waveform, the time and the stop button. */
function Recording({
  startedAt,
  onOpen,
  onStop,
}: {
  startedAt: string;
  /** `keyboard`: a key pressed it, so the screen change does not move (design-system 5). */
  onOpen: (keyboard: boolean) => void;
  onStop: () => void;
}) {
  const bars = useRef<HTMLSpanElement>(null);
  // Seeded from the meeting, so coming back mid-recording does not roll every digit up from 0.
  const [s, setSeconds] = useState(() => sinceStart(startedAt));
  useEffect(() => {
    const unlisten = onRecordingLevel((l) => {
      // Whole seconds: an equal value lets React skip the other 24 events of each second.
      setSeconds(Math.floor(l.elapsed_ms / 1000));
      // 25 times a second, straight to the DOM: no React state, no CSS transition (design-system 5).
      // Square ends, so scaleY never warps them.
      const loud = (j: number) =>
        Math.max(0, ...[0, 1, 2, 3].flatMap((k) => [l.mic_bands[4 * j + k] ?? 0, l.system_bands[4 * j + k] ?? 0]));
      bars.current?.childNodes.forEach((bar, i) => {
        (bar as HTMLElement).style.transform = `scaleY(${0.15 + 0.0085 * loud(MIRROR[i])})`;
      });
    });
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);
  const time = clock(s);
  return (
    <>
      <button
        aria-label={`녹음 중 ${time}`}
        className={`${PART} gap-3 pr-3 pl-4 hover:bg-white/10`}
        onClick={(e) => onOpen(e.detail === 0)}
      >
        <span aria-hidden className="size-2.5 rounded-full bg-rec motion-safe:animate-breathe" />
        <span ref={bars} aria-hidden className="flex h-5 items-center gap-[2px]">
          {MIRROR.map((_, i) => (
            <span key={i} className="h-full w-[2px] bg-rec" style={QUIET} />
          ))}
        </span>
        <RollingTime text={time} className="text-label" />
      </button>
      <button
        aria-label="녹음 종료"
        aria-keyshortcuts={SHORTCUT.stop.aria}
        className={`${PART} gap-2 bg-white/10 px-4 text-label hover:bg-white/15`}
        onClick={onStop}
      >
        <Square aria-hidden className="size-3 fill-current text-rec" />
        종료
        <Kbd>{SHORTCUT.stop.label}</Kbd>
      </button>
    </>
  );
}

/** What the meeting at the head of the line is doing, e.g. "주간 회의 · 요약 2/3". `percent` is set
 * when there is a value to show; `shown` when the words carry it too ("전사 42%"). */
function jobOf(m: MeetingItem, p: Progress | undefined) {
  // Titles come from the summary: until then the meeting goes by its start.
  const name = m.title || `${hhmm(m.created_at)} 회의`;
  if (phase(m, p) === "waiting") {
    return { name, step: "대기 중", percent: null, shown: false, running: false };
  }
  // Events come before the list is read again: a summary's first one can meet a list that still
  // says transcribing.
  if ((p?.status === "summarizing" ? p.status : m.status) === "summarizing") {
    const n = p?.status === "summarizing" && p.chunks > 1 ? p : null;
    const step = engineStarting(p)
      ? "요약 엔진 준비 중"
      : n
        ? `요약 ${Math.min(n.chunk + 1, n.chunks)}/${n.chunks}`
        : "요약 중";
    return { name, step, percent: n?.percent ?? null, shown: false, running: true };
  }
  // Until the chunk plan exists, `percent` is the planning's own.
  const counted = p?.status === "transcribing" && p.chunks > 0;
  const step = counted ? "전사" : p?.status === "transcribing" ? "구간 나누는 중" : "전사 중";
  return { name, step, percent: counted ? p.percent : null, shown: counted, running: true };
}

const ARC = { cx: 12, cy: 12, r: 9, fill: "none", strokeWidth: 2.5 };

/** Processing: a ring and "처리 중 N" (`title`) over the head of the line, or an import's file. One
 * spring drives the ring and the percent, rendered from the MotionValue without a re-render per
 * frame. Keyed by meeting or file, so a new one starts from its own value. */
function Processing({ title, job }: { title: string; job: ReturnType<typeof jobOf> }) {
  const value = useFollow(job.percent ?? 0);
  const length = useTransform(value, (v) => v / 100);
  const text = useTransform(value, (v) => `${Math.round(v)}%`);
  const turning = job.running && job.percent == null;
  return (
    <>
      <svg aria-hidden viewBox="0 0 24 24" className={`size-6 shrink-0 -rotate-90 ${turning ? "motion-safe:animate-spin" : ""}`}>
        <circle {...ARC} className="stroke-white/20" />
        {job.percent != null ? (
          <motion.circle {...ARC} className="stroke-white" style={{ pathLength: length }} />
        ) : (
          // Nothing to count yet: a quarter arc turning. Waiting shows the empty ring.
          turning && <circle {...ARC} pathLength={1} strokeDasharray="0.25 1" className="stroke-white" />
        )}
      </svg>
      <span className="grid min-w-0 text-left">
        <span className="text-label">{title}</span>
        <span className="max-w-64 truncate text-caption text-dock-2">
          {job.name} · {job.step}
          {job.shown && (
            <>
              {" "}
              <motion.span className="tabular-nums">{text}</motion.span>
            </>
          )}
        </span>
      </span>
    </>
  );
}

type Mode = "recording" | "importing" | "processing" | "idle";
/** Said once per change (politely), never per tick. */
function announcement(was: Mode, now: Mode) {
  if (now === "recording") return "녹음을 시작했습니다";
  if (now === "importing") return "녹음 파일을 가져오고 있습니다";
  if (was === "importing" && now === "idle") return "녹음 파일 가져오기를 마쳤습니다";
  if (was === "recording") return now === "processing" ? "녹음을 마쳤습니다. 처리를 시작합니다" : "녹음을 마쳤습니다";
  return now === "processing" ? "처리를 시작했습니다" : "처리를 마쳤습니다";
}

export default function Dock({
  live,
  stopping,
  queue,
  held = false,
  progress,
  imported,
  hidden,
  onRecording,
  onStop,
  onOpen,
}: {
  /** The recording running, if any. */
  live?: MeetingItem;
  /** Its stop was asked and the list does not show it yet. */
  stopping: boolean;
  /** Meetings being processed first, then the ones waiting, by place in line. */
  queue: MeetingItem[];
  /** A meeting paused by the recorder is left (plan 5.3-14): processing then ends without a word. */
  held?: boolean;
  progress: Record<string, Progress>;
  /** A recording file being imported (REC-12): shown over processing, which it is about to join. */
  imported?: ImportProgress | null;
  /** The recording screen shows all of this itself. */
  hidden: boolean;
  /** `keyboard`: a key pressed it, so the screen change does not move (design-system 5). */
  onRecording: (keyboard: boolean) => void;
  onStop: () => void;
  onOpen: (meeting: MeetingItem, keyboard: boolean) => void;
}) {
  // Recording stops every worker (TRN-09), so processing waits behind it.
  const mode: Mode = live ? "recording" : imported ? "importing" : queue.length ? "processing" : "idle";
  const [said, setSaid] = useState("");
  const last = useRef(mode);
  useEffect(() => {
    const quiet = held && last.current === "processing" && mode === "idle";
    if (last.current !== mode) setSaid(quiet ? "" : announcement(last.current, mode));
    last.current = mode;
  }, [mode]);

  // Focus moves to the pill before 종료 swaps the shape, so it never falls back to the page.
  const pill = useRef<HTMLDivElement>(null);
  const stop = () => {
    pill.current?.focus();
    onStop();
  };
  const head = queue[0];
  const job = head && jobOf(head, progress[head.id]);
  // Each shape's content, keyed so the pill morphs between them.
  const content = live
    ? stopping
      ? { key: "stopping", body: <span className="px-4 text-label">전사를 시작합니다</span> }
      : {
          key: "recording",
          body: <Recording startedAt={live.created_at} onOpen={onRecording} onStop={stop} />,
        }
    : imported
      ? {
          key: "importing",
          body: (
            <div className={`${PART} min-w-0 gap-3 pr-5 pl-3`}>
              <Processing
                key={imported.name}
                title="녹음 파일 가져오기"
                job={{ name: imported.name, step: "가져오는 중", percent: imported.percent, shown: true, running: true }}
              />
            </div>
          ),
        }
      : job && {
          key: "processing",
          body: (
            <button
              aria-label={[`처리 중 ${queue.length}`, job.name, job.shown ? `${job.step} ${job.percent}%` : job.step].join(" · ")}
              className={`${PART} min-w-0 gap-3 pr-5 pl-3 hover:bg-white/10`}
              onClick={(e) => onOpen(head, e.detail === 0)}
            >
              <Processing key={head.id} title={`처리 중 ${queue.length}`} job={job} />
            </button>
          ),
        };

  // In App's bottom stack, under a meeting's send bar.
  return (
    <>
      <AnimatePresence>
        {content && !hidden && (
          <motion.div
            key="dock"
            ref={pill}
            tabIndex={-1}
            layout
            initial={{ y: 96, opacity: 0 }}
            animate={{ y: 0, opacity: 1 }}
            exit={{ y: 96, opacity: 0 }}
            transition={spring.dock}
            // In `style`, not a class: Motion corrects the radius while the size morphs.
            style={{ borderRadius: 28 }}
            className="pointer-events-auto relative flex h-14 max-w-full items-center gap-1 bg-dock p-2 text-white shadow-float focus-visible:outline-white"
          >
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.div
                key={content.key}
                layout="position"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                // Labels slide with the pill's own spring; only their fade is timed.
                transition={{ duration: dur.small, layout: spring.dock }}
                className="flex min-w-0 items-center gap-1"
              >
                {/* The last shape stays a moment for its exit: never clicked twice meanwhile. */}
                <Present className="contents">{content.body}</Present>
              </motion.div>
            </AnimatePresence>
          </motion.div>
        )}
      </AnimatePresence>
      <p role="status" className="sr-only">
        {said}
      </p>
    </>
  );
}
