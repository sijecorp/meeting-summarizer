import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, CircleAlert, Info, Trash2, TriangleAlert, X } from "lucide-react";
import { AnimatePresence, motion, useIsPresent, useReducedMotion, useSpring } from "motion/react";
import { Alert, AlertAction, AlertDescription } from "@/components/ui/alert";
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
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  BUILTIN,
  connectionLabel,
  deleteMeeting,
  modelDownload,
  modelStatus,
  onModelDownload,
  SCHEMA_VERSION,
  setSummarizeLocal,
  STATUS_LABEL,
  summaryRoute,
  type Meta,
  type ModelDownload,
  type Progress,
  type Segment,
  type Status,
  type SummaryRoute,
  type Wait,
} from "@/lib/ipc";
import { dur, ease } from "@/lib/motion";
import { josa } from "@/lib/utils";

/** Something on its way out (a screen, a banner, the dock's last shape) stays mounted for its exit:
 * out of the tab order, the accessibility tree and hit-testing meanwhile. */
export function Present({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div inert={!useIsPresent()} className={className}>
      {children}
    </div>
  );
}

/** Body widths (design-system 4): the review document, Home's timeline (and the screens without
 * their own), settings. With `box-content` the side padding sits outside, so text gets all of it. */
export const WIDTH = { doc: "max-w-doc", home: "max-w-home", wide: "max-w-wide" };

/** Every screen's body under the app header: one width, one title style, the back link. */
export function Page({
  title,
  action,
  onHome,
  width = "home",
  children,
}: {
  title: string;
  /** Shown at the right of the title, e.g. Home's 녹음 시작. */
  action?: ReactNode;
  onHome?: () => void;
  width?: keyof typeof WIDTH;
  children: ReactNode;
}) {
  return (
    <main className={`mx-auto box-content space-y-6 px-8 py-8 ${WIDTH[width]}`}>
      <div className="space-y-2">
        {onHome && (
          <Button variant="ghost" size="sm" className="-ml-3 text-ink-2 hover:text-ink" onClick={onHome}>
            <ArrowLeft aria-hidden />
            홈으로
          </Button>
        )}
        <div className="flex flex-wrap items-center justify-between gap-4">
          <h1 tabIndex={-1} className="font-display text-display-m outline-hidden">
            {title}
          </h1>
          {action}
        </div>
      </div>
      {children}
    </main>
  );
}

/** The company logo, in the text color. */
export function SijeLogo({ className }: { className?: string }) {
  return (
    <svg viewBox="98.2 98.1 761.8 271.2" fill="currentColor" role="img" aria-label="SIJE" className={className}>
      <path d="M344.7,113.3v27.5h23.3v185.8h-23.3v27.5c0,8.4,6.8,15.2,15.2,15.2h82.9v-42.7h-24.3v-185.8h24.3v-42.6h-82.9c-8.4,0-15.2,6.8-15.2,15.2Z" />
      <path d="M587.6,98.1h-86.2v42.6h51.5v185.8h-51.5v42.7h86.2c8.7,0,15.8-6.8,15.8-15.2V113.3c0-8.4-7.1-15.2-15.8-15.2Z" />
      <path d="M860,140.7v-42.6h-174.7c-8.4,0-15.2,6.8-15.2,15.2v240.8c0,8.4,6.8,15.2,15.2,15.2h174.7v-42.7h-140.4v-77.7h134.4v-42.1h-134.4v-66.1h140.4Z" />
      <path d="M151.8,206.6v-39.6c0-16.9,9.6-26.2,26.9-26.2h118.1v-42.7h-133.8c-12.6.3-22.4,2.9-30,7.4-8.6,4.6-15.7,11.5-20.7,19.8-6.1,9.4-9.6,21.9-9.6,38.4v44.7h0v40.2h149.4v51.8c0,16.9-9.6,26.2-26.9,26.2h-127v42.7h142.8c12.6-.3,22.4-2.9,30-7.4,8.6-4.6,15.7-11.5,20.7-19.8,6.1-9.4,9.6-21.9,9.6-38.4v-97.1h-149.4Z" />
    </svg>
  );
}

/** Three bars, the sign of transcribing. */
export function Bars({ className = "" }: { className?: string }) {
  return (
    <span aria-hidden className={`flex items-end gap-[2px] ${className}`}>
      {[0.55, 1, 0.75].map((height, i) => (
        <span
          key={i}
          className="w-[3px] origin-bottom rounded-full bg-current motion-safe:animate-bar"
          // Out of step, so the three never rise together.
          style={{ height: `${height * 100}%`, animationDelay: `${i * -0.3}s` }}
        />
      ))}
    </span>
  );
}

/** Three dots taking turns: waiting in line. */
export function Dots() {
  return (
    <span aria-hidden className="flex gap-[3px]">
      {[0, 1, 2].map((i) => (
        <span
          key={i}
          className="size-1 rounded-full bg-ink-2 motion-safe:animate-dot"
          style={{ animationDelay: `${i * 0.2}s` }}
        />
      ))}
    </span>
  );
}

// The sweep starts out of sight and only moves without Reduce Motion. It passes under the text
// (`isolate` + `-z-10`), so the words keep their contrast, and never past their box (`overflow-hidden`).
export const SHIMMER =
  "relative isolate overflow-hidden after:pointer-events-none after:absolute after:inset-0 after:-z-10 after:-translate-x-full after:bg-linear-to-r after:from-transparent after:via-surface/70 after:to-transparent motion-safe:after:animate-shimmer";

const RUNNING: Status[] = ["transcribing", "summarizing"];
// A meeting also waits while it has a place in line: a retried `transcribe_failed` or
// `summarize_failed` one, and a `transcribed` one waiting for its summary (APP-07). Without a place,
// `transcribed` is where a meeting stops when its route does not resolve, and so is a `paused` one
// with a `wait` (plan 5.4): neither is in line.
const WAITING: Status[] = ["recorded", "paused"];

/** Running, paused by the recorder (`held`, plan 5.3), waiting or neither. The dock, Home and the
 * meeting document all ask this, so the dock's "처리 중 N" always matches the rows. A held meeting
 * whose run still stops is running: "멈추는 중…" comes from the two together, so it survives a
 * redraw. */
export function phase(meta: Meta, progress: Progress | undefined) {
  // A meeting from a newer app version is read-only and never runs here (like `busy_ids`).
  if (meta.schema_version > SCHEMA_VERSION) return null;
  // An event saying it runs wins over the list: between transcribing and summarizing the list can
  // read `transcribed`, and the dock must not drop the meeting for that moment.
  if (progress && RUNNING.includes(progress.status)) return "running";
  if (meta.held) return "held";
  if (RUNNING.includes(meta.status)) return "running";
  if (progress?.position != null || (WAITING.includes(meta.status) && !meta.wait)) return "waiting";
  return null;
}

/** Plan 5.3: a held meeting whose run has not ended yet. */
export const stopping = (meta: Meta, progress: Progress | undefined) =>
  !!meta.held && phase(meta, progress) === "running";

/** A waiting meeting shows its place in line instead of its stored status. */
export function statusLabel(meta: Meta, progress: Progress | undefined) {
  if (phase(meta, progress) !== "waiting") return STATUS_LABEL[meta.status] ?? meta.status;
  return progress?.position ? `대기 중 · ${progress.position}번째` : "대기 중";
}

/** A spring following `target` that is drawn straight from its MotionValue, never a render per
 * frame. Reduce Motion does not slow a MotionValue by itself, so it jumps then. */
export function useFollow(target: number) {
  const reduce = useReducedMotion();
  const value = useSpring(target, { stiffness: 140, damping: 24 });
  useEffect(() => {
    if (reduce) value.jump(target);
    else value.set(target);
  }, [target, reduce]);
  return value;
}

/** A recording's length: "42분", "1시간 15분", "1분 미만". */
export function lengthText(ms: number) {
  const min = Math.floor(ms / 60_000);
  if (min >= 60) return `${Math.floor(min / 60)}시간${min % 60 ? ` ${min % 60}분` : ""}`;
  return min ? `${min}분` : "1분 미만";
}

const pad = (n: number) => String(n).padStart(2, "0");
/** mm:ss; past an hour it reads 75:30. */
export const mmss = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
};
/** A running time: 12:43 under an hour, then 1:02:03. */
export const clock = (s: number) =>
  s < 3600
    ? `${pad(Math.floor(s / 60))}:${pad(s % 60)}`
    : `${Math.floor(s / 3600)}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}`;

/** The local wall-clock time of a stored timestamp, "14:05". */
export const hhmm = (iso: string) => {
  const d = new Date(iso);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

/** Seconds since a recording started, read from its meeting until the first level event says. */
export const sinceStart = (iso?: string) =>
  iso ? Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 1000)) : 0;

/** A time whose digits roll as they change (design-system 6.3): 200ms, `ease.out`. Screen readers
 * get the plain text, never the digits on their way out. Pretendard's fixed-width digits, so nothing
 * beside it shifts. */
export function RollingTime({ text, className = "" }: { text: string; className?: string }) {
  return (
    <span className={`tabular-nums ${className}`}>
      <span className="sr-only">{text}</span>
      <span aria-hidden className="inline-flex">
        {[...text].map((c, i) =>
          /\d/.test(c) ? (
            <span key={i} className="relative inline-block overflow-hidden">
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={c}
                  className="inline-block"
                  initial={{ y: "-60%", opacity: 0 }}
                  animate={{ y: 0, opacity: 1 }}
                  exit={{ y: "60%", opacity: 0 }}
                  transition={{ duration: dur.small, ease: ease.out }}
                >
                  {c}
                </motion.span>
              </AnimatePresence>
            </span>
          ) : (
            <span key={i}>{c}</span>
          ),
        )}
      </span>
    </span>
  );
}

export const MAC = navigator.userAgent.includes("Mac");
export const WINDOWS = navigator.userAgent.includes("Windows");
/** The modifier shortcuts use here: ⌘ on macOS, Ctrl elsewhere. */
export const withModifier = (e: KeyboardEvent) => (MAC ? e.metaKey : e.ctrlKey);
const shortcut = (key: string) => ({
  label: MAC ? `⌘${key}` : `Ctrl+${key}`,
  aria: `${MAC ? "Meta" : "Control"}+${key}`,
});
/** Recording's two shortcuts (App.tsx listens), as each button shows and names them. */
export const SHORTCUT = { start: shortcut("R"), stop: shortcut(".") };

/** A shortcut hint on a button. Hidden from screen readers: the button's `aria-keyshortcuts` says it. */
export function Kbd({ children, className = "" }: { children: string; className?: string }) {
  return (
    <kbd aria-hidden className={`font-sans text-caption tracking-normal opacity-70 ${className}`}>
      {children}
    </kbd>
  );
}

/** "약 3분 남음", "약 1시간 5분 남음". */
export function remaining(secs: number) {
  const min = Math.max(1, Math.round(secs / 60));
  const parts = [min >= 60 && `${Math.floor(min / 60)}시간`, min % 60 && `${min % 60}분`];
  return `약 ${parts.filter(Boolean).join(" ")} 남음`;
}

/** Plan 5.2-3 (E4): what follows a download's percent. `stage`: "확인 중…" while a file whose bytes
 * are all in has its hash checked, "연결 기다리는 중" after 10 s without a byte; the only part a
 * `role="status"` says. Otherwise `time`, for the `left` bytes at the last 10 s' speed, none in the
 * download's first 10 s. */
export function downloadNote(d: ModelDownload, left: number) {
  const stage = d.received >= d.total ? "확인 중…" : d.bps === 0 ? "연결 기다리는 중" : "";
  return { stage, time: !stage && d.bps ? remaining(left / d.bps) : "" };
}

/** A running meeting's words and bar (TRN-01, APP-07), or null when its last event (its final
 * status, with the last step's counts) came before the meeting was read again: nothing runs then.
 * A summary of more than one LLM request shows "요청 2/5". `bar` is false when there is no value
 * to show. */
export function progressText(p: Progress) {
  if (p.status !== "transcribing" && p.status !== "summarizing") return null;
  // Until the chunk plan exists, `percent` is the planning's own; then it starts again from 0. A
  // summary plans nothing: with a single LLM request there is nothing to count, so no bar.
  const summarizing = p.status === "summarizing";
  const planning = !summarizing && p.chunks === 0;
  const text = summarizing
    ? ["요약 중", p.chunks > 1 && `요청 ${Math.min(p.chunk + 1, p.chunks)}/${p.chunks}`]
        .filter(Boolean)
        .join(" · ")
    : planning
      ? "구간 나누는 중"
      : [`${p.percent}%`, p.eta_secs != null && remaining(p.eta_secs)].filter(Boolean).join(" · ");
  const label = summarizing ? "요약 진행률" : planning ? "구간 나누기 진행률" : "전사 진행률";
  return { text, label, bar: !summarizing || p.chunks > 1 };
}

/** A transcript with `mm:ss` times. Plain text nodes only (architecture 10.4). */
export function TranscriptList({ segments }: { segments: Segment[] }) {
  if (segments.length === 0) return <p className="text-muted-foreground">전사된 말이 없습니다</p>;
  return (
    <ol className="space-y-2 text-base">
      {segments.map((s, i) => (
        <li key={i} className="grid grid-cols-[auto_1fr] gap-x-4">
          <span className="text-muted-foreground tabular-nums">{mmss(s.start_ms)}</span>
          <span>{s.text}</span>
        </li>
      ))}
    </ol>
  );
}

/** "으로" or "로" after a name: the particle follows the name, never a "(host)" after it. */
export const ro = (word: string) => josa(word, "으로", "로");

/** The host a connection sends to, for SUM-07 notices. */
export const host = (url: string) => (URL.canParse(url) ? new URL(url).host : url);

/** The built-in engine is starting for this summary (plan 5.3): Home, the dock and the meeting
 * document say so instead of a summary that looks stuck. */
export const engineStarting = (p?: Progress): p is Progress & { engine_since: number } =>
  p?.status === "summarizing" && p.engine_since != null;

/** Plan 5.3, DX6: the built-in engine loads its model before the first request (up to 120 s),
 * so a summary is not left looking stuck at 0/0. The seconds count here, between events. */
export function EngineStarting({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return <>{`요약 엔진 준비 중 · ${Math.max(0, Math.floor(now / 1000 - since))}초`}</>;
}

/** The summary model as the route notices tell it (plan 5.2, DX6), or null until read. Read when
 * `on`, then followed through `model://download` and the app's one download (ipc.ts). A read that
 * fails reads as not downloaded: the 요약(LLM) tab says why. `stage` and `time` (`downloadNote`) are
 * the whole download's, every file left. */
export function useSummaryModel(on: boolean) {
  const [model, setModel] = useState<{
    file: string;
    installed: boolean;
    downloading: boolean;
    percent: number;
    stage?: string;
    time?: string;
  } | null>(null);
  useEffect(() => {
    if (!on) return;
    let live = true;
    let followed: Promise<void> | null = null;
    const read = () =>
      modelStatus("summary").then(
        (state) => {
          const f = state?.files?.[0];
          const percent = f ? Math.floor((f.received / f.size) * 100) : 0;
          if (live) setModel({ file: f?.file ?? "", installed: !!f?.installed, downloading: !!f?.downloading, percent });
        },
        () => live && setModel({ file: "", installed: false, downloading: false, percent: 0 }),
      );
    // A download started from another screen later on reads it again, and so does its end. One
    // running already is in the first read.
    const follow = (started: boolean) => {
      const running = modelDownload();
      if (!running || running === followed) return;
      followed = running;
      if (started) read();
      running.then(read);
    };
    read();
    follow(false);
    const unlisten = onModelDownload((d) => {
      follow(true);
      setModel(
        (m) =>
          m && {
            ...m,
            ...(m.file === d.file ? { downloading: true, percent: Math.floor((d.received / d.total) * 100) } : {}),
            ...downloadNote(d, d.all_total - d.all_received),
          },
      );
    });
    return () => {
      live = false;
      unlisten.then((stop) => stop()).catch(() => {});
    };
  }, [on]);
  return model;
}

/** SUM-07 where the transcript goes for its summary, and SUM-08 "로컬로 요약" until summarizing
 * starts (`locked`). Read again when `status` changes. Once locked it shows only while
 * `summarizing`; then the meeting document's meta line says where the transcript went. With no
 * route yet, it says why (plan 5.4): `wait`, the meeting's reason when known, else the summary model
 * in one of three states (DX6), with `onSetup` opening the 요약(LLM) tab. */
export function RouteNotice({
  id,
  status,
  wait,
  pinned,
  center,
  onSetup,
}: {
  id: string;
  status?: Status;
  wait?: Wait;
  /** The meeting's `llm_connection`: a connection picked for it changes the route (ENG15). */
  pinned?: string | null;
  center?: boolean;
  onSetup?: () => void;
}) {
  const [route, setRoute] = useState<SummaryRoute | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // The read and the switch share one count, so only the newest reply sets the route.
  const latest = useRef(0);
  const settle = (reply: Promise<SummaryRoute>) => {
    const request = ++latest.current;
    return reply.then(
      (r) => {
        if (request !== latest.current) return;
        setRoute(r);
        setError("");
      },
      (e) => request === latest.current && setError(String(e)),
    );
  };

  // A meeting given another connection while it waits keeps its status: its reason and pin change.
  useEffect(() => {
    settle(summaryRoute(id));
  }, [id, status, wait, pinned]);

  const c = route?.connection;
  // Only the summary model can be missing from a route that is not pinned (`summarize::route`).
  const waitsForModel =
    !!route && !c && wait !== "connection_deleted" && (wait === "summary_model" || !route.locked);
  const model = useSummaryModel(waitsForModel);

  const toggle = (local: boolean) => {
    setBusy(true);
    settle(setSummarizeLocal(id, local)).finally(() => setBusy(false));
  };

  if (!route) return error ? <Banner kind="error">{error}</Banner> : null;
  // The built-in route is this PC itself: "이 PC에서", not "이 PC의 이 PC로".
  const local = c && (c.name === BUILTIN ? `${connectionLabel(c.name)}에서` : `이 PC의 ${c.name}${ro(c.name)}`);
  const missing = waitsForModel && !!model && !model.installed && !model.downloading;
  const text =
    wait === "connection_deleted" && !c
      ? "이 회의에 정한 연결이 삭제됐습니다. 다른 연결을 골라 주세요"
      : waitsForModel
        ? !model
          ? ""
          : model.installed
            ? "이 PC에서 요약합니다. 전사본은 밖으로 나가지 않습니다"
            : model.downloading
              ? [`요약 모델 받는 중 ${model.percent}%`, model.stage || model.time, "다 받으면 자동으로 요약합니다"]
                  .filter(Boolean)
                  .join(" · ")
              : "설정 → 요약(LLM)에서 요약 모델을 받아 주세요"
        : route.locked
          ? status === "summarizing" && c
            ? c.is_local
              ? `${local} 요약하고 있습니다`
              : `전사본을 ${c.name}(${host(c.base_url)})${ro(c.name)} 보내 요약하고 있습니다`
            : ""
          : c?.is_local
            ? `${local} 요약합니다. 전사본은 밖으로 나가지 않습니다`
            : c
              ? `요약할 때 전사본이 ${c.name}(${host(c.base_url)})${ro(c.name)} 전송됩니다`
              : "";
  if (!text) return null;
  return (
    <div className={center ? "flex flex-col items-center gap-2 text-center" : "space-y-2"}>
      <p className="flex gap-2 text-sm text-muted-foreground">
        <Info aria-hidden className="mt-0.5 size-4 shrink-0" />
        {text}
      </p>
      {/* Plan 5.2-3: only when the download's stage changes, never its time. */}
      {waitsForModel && model?.downloading && !model.installed && (
        <span role="status" className="sr-only">
          {model.stage || "요약 모델 받는 중"}
        </span>
      )}
      {missing && onSetup && (
        <Button variant="outline" size="sm" onClick={onSetup}>
          요약(LLM) 설정
        </Button>
      )}
      {/* Only where it changes the route (plan 5.4). Also shown when "로컬로 요약" is on but its
          connection is gone, so it can be turned off. */}
      {!route.locked && route.toggle_matters && (route.local_available || route.local) && (
        <label className="flex w-fit items-center gap-2 text-sm">
          <Switch checked={route.local} disabled={busy} onCheckedChange={toggle} />
          로컬로 요약
        </label>
      )}
      {error && <Banner kind="error">{error}</Banner>}
    </div>
  );
}

/** Error and warning banners (design-system 3): a *-line border and icon, ink text. */
export function Banner({
  kind,
  children,
  onDismiss,
}: {
  kind: "error" | "warning";
  children: ReactNode;
  onDismiss?: () => void;
}) {
  const Icon = kind === "error" ? CircleAlert : TriangleAlert;
  return (
    <Alert
      variant={kind === "error" ? "destructive" : "warning"}
      role={kind === "error" ? "alert" : "status"}
    >
      <Icon aria-hidden />
      <AlertDescription>{children}</AlertDescription>
      {onDismiss && (
        <AlertAction>
          <Button variant="ghost" size="icon-xs" aria-label="닫기" onClick={onDismiss}>
            <X aria-hidden />
          </Button>
        </AlertAction>
      )}
    </Alert>
  );
}

/** A native wrapping label: the control stays text-selectable and needs no id. */
export function Field({ label, className, children }: { label: string; className?: string; children: ReactNode }) {
  return (
    <label className={`grid gap-1.5 text-sm ${className ?? ""}`}>
      <span className="font-medium text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

/** A labelled Radix select; Radix renders a button, so the label goes on it as aria-label. */
export function SelectField({
  label,
  value,
  options,
  placeholder,
  disabled,
  onChange,
}: {
  label: string;
  /** Empty: nothing picked yet, and `placeholder` shows. */
  value: string;
  options: { value: string; label: string; disabled?: boolean }[];
  placeholder?: string;
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="font-medium text-muted-foreground">{label}</span>
      <Select value={value} disabled={disabled} onValueChange={onChange}>
        <SelectTrigger aria-label={label} className="w-full">
          <SelectValue placeholder={placeholder} />
        </SelectTrigger>
        <SelectContent>
          {options.map((o) => (
            <SelectItem key={o.value} value={o.value} disabled={o.disabled}>
              {o.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

/** REV-04's confirmation, opened by its `children` (a trigger) or by `open`, e.g. from a menu. */
export function DeleteDialog({
  id,
  open,
  onOpenChange,
  onCloseAutoFocus,
  onDeleted,
  onError,
  children,
}: {
  id: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Where focus goes back when it was opened without a trigger. */
  onCloseAutoFocus?: (event: Event) => void;
  onDeleted: () => void;
  onError: (message: string) => void;
  children?: ReactNode;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      {children && <AlertDialogTrigger asChild>{children}</AlertDialogTrigger>}
      <AlertDialogContent onCloseAutoFocus={onCloseAutoFocus}>
        <AlertDialogHeader>
          <AlertDialogTitle>회의를 삭제할까요?</AlertDialogTitle>
          <AlertDialogDescription>녹음과 결과를 모두 지웁니다. 되돌릴 수 없습니다.</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>취소</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={() =>
              deleteMeeting(id)
                .then(onDeleted)
                .catch((e) => onError(String(e)))
            }
          >
            삭제
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** Confirmed in a dialog so a single misclick never removes a meeting (REV-04). */
export function DeleteButton({
  id,
  disabled,
  size,
  label,
  onDeleted,
  onError,
}: {
  id: string;
  disabled?: boolean;
  size?: "sm";
  /** The name when "삭제" alone would not say which meeting. */
  label?: string;
  onDeleted: () => void;
  onError: (message: string) => void;
}) {
  return (
    <DeleteDialog id={id} onDeleted={onDeleted} onError={onError}>
      <Button variant="outline" size={size} disabled={disabled} aria-label={label}>
        <Trash2 aria-hidden />
        삭제
      </Button>
    </DeleteDialog>
  );
}
