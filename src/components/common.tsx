import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, CircleAlert, Info, Trash2, TriangleAlert, X } from "lucide-react";
import { AnimatePresence, motion, useIsPresent } from "motion/react";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress as ProgressBar } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  deleteMeeting,
  SCHEMA_VERSION,
  setSummarizeLocal,
  STATUS_LABEL,
  summaryRoute,
  type Meta,
  type Progress,
  type Segment,
  type Status,
  type SummaryRoute,
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
          <h1 className="font-display text-display-m">{title}</h1>
          {action}
        </div>
      </div>
      {children}
    </main>
  );
}

/** The logo's three bars, also the sign of transcribing. They move only while `live`; the
 * attribute says which, for tests. */
export function Bars({ live, className = "" }: { live: boolean; className?: string }) {
  return (
    <span aria-hidden data-live={live} className={`flex items-end gap-[2px] ${className}`}>
      {[0.55, 1, 0.75].map((height, i) => (
        <span
          key={i}
          className={`w-[3px] origin-bottom rounded-full bg-current ${live ? "motion-safe:animate-bar" : ""}`}
          // Out of step, so the three never rise together.
          style={{ height: `${height * 100}%`, animationDelay: `${i * -0.3}s` }}
        />
      ))}
    </span>
  );
}

/** What a badge shows moving (design-system 6.4). */
export type BadgeKind = "recording" | "transcribing" | "summarizing" | "waiting";

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

/** One look for a status in the meeting list, on the processing screen and in the app header. A
 * state that is happening moves, and its words always say it too: never color or motion alone. */
export function StatusBadge({ kind, children }: { kind?: BadgeKind; children: ReactNode }) {
  return (
    <Badge variant="secondary" className={kind === "summarizing" ? SHIMMER : undefined}>
      {kind === "recording" && <span aria-hidden className="size-2 rounded-full bg-rec motion-safe:animate-breathe" />}
      {kind === "transcribing" && <Bars live className="h-3 text-brand" />}
      {kind === "waiting" && <Dots />}
      {children}
    </Badge>
  );
}

const RUNNING: Status[] = ["transcribing", "summarizing"];
// A meeting also waits while it has a place in line: a retried `transcribe_failed` or
// `summarize_failed` one, and a `transcribed` one waiting for its summary (APP-07). Without a place,
// `transcribed` is where a meeting stops when there is no LLM connection.
const WAITING: Status[] = ["recorded", "paused"];

/** Running, waiting or neither. The header chip, Home and the processing screen all ask this, so
 * the chip's "처리 중 N · 대기 M" always matches the badges. */
export function phase(meta: Meta, progress: Progress | undefined) {
  // A meeting from a newer app version is read-only and never runs here (like `busy_ids`).
  if (meta.schema_version > SCHEMA_VERSION) return null;
  if (RUNNING.includes(meta.status)) return "running";
  if (progress?.position != null || WAITING.includes(meta.status)) return "waiting";
  return null;
}

/** A meeting's badge, by the same reading as `phase`. */
export function badgeKind(meta: Meta, progress: Progress | undefined): BadgeKind | undefined {
  const now = phase(meta, progress);
  return meta.status === "recording"
    ? "recording"
    : now === "waiting"
      ? "waiting"
      : now === "running"
        ? meta.status === "summarizing"
          ? "summarizing"
          : "transcribing"
        : undefined;
}

/** A waiting meeting shows its place in line instead of its stored status. */
export function statusLabel(meta: Meta, progress: Progress | undefined) {
  if (phase(meta, progress) !== "waiting") return STATUS_LABEL[meta.status] ?? meta.status;
  return progress?.position ? `대기 중 · ${progress.position}번째` : "대기 중";
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

const MAC = navigator.userAgent.includes("Mac");
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

function remaining(secs: number) {
  const min = Math.max(1, Math.round(secs / 60));
  const parts = [min >= 60 && `${Math.floor(min / 60)}시간`, min % 60 && `${min % 60}분`];
  return `약 ${parts.filter(Boolean).join(" ")} 남음`;
}

/** A running meeting's words and bar (TRN-01, APP-07), or null when its last event (its final
 * status, with the last step's counts) came before the meeting was read again: nothing runs then.
 * `detailed` adds "구간 2/5" while transcribing; a summary of more than one LLM request shows
 * "요청 2/5". `bar` is false when there is no value to show. */
export function progressText(p: Progress, detailed?: boolean) {
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
      : [
          detailed && `구간 ${Math.min(p.chunk + 1, p.chunks)}/${p.chunks}`,
          `${p.percent}%`,
          p.eta_secs != null && remaining(p.eta_secs),
        ]
          .filter(Boolean)
          .join(" · ");
  const label = summarizing ? "요약 진행률" : planning ? "구간 나누기 진행률" : "전사 진행률";
  return { text, label, bar: !summarizing || p.chunks > 1 };
}

/** A running meeting's line with its bar. Plain text and no live region, so a screen reader is not
 * told every tick. */
export function ProcessProgress({ progress: p, detailed }: { progress: Progress; detailed?: boolean }) {
  const line = progressText(p, detailed);
  if (!line) return null;
  return (
    <div className="grid gap-1.5">
      {line.bar && <ProgressBar value={p.percent} aria-label={line.label} />}
      <span className="text-sm text-muted-foreground tabular-nums">{line.text}</span>
    </div>
  );
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

/** SUM-07 where the transcript goes for its summary, and SUM-08 "로컬로 요약" until summarizing
 * starts (`locked`). Read again when `status` changes. Once locked it shows only while
 * `summarizing`; then the review screen says where the transcript went. */
export function RouteNotice({ id, status, center }: { id: string; status?: Status; center?: boolean }) {
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

  useEffect(() => {
    settle(summaryRoute(id));
  }, [id, status]);

  const toggle = (local: boolean) => {
    setBusy(true);
    settle(setSummarizeLocal(id, local)).finally(() => setBusy(false));
  };

  if (!route) return error ? <Banner kind="error">{error}</Banner> : null;
  const c = route.connection;
  const text = route.locked
    ? status === "summarizing" && c
      ? c.is_local
        ? `이 PC의 ${c.name}${ro(c.name)} 요약하고 있습니다`
        : `전사본을 ${c.name}(${host(c.base_url)})${ro(c.name)} 보내 요약하고 있습니다`
      : ""
    : c
      ? c.is_local
        ? `이 PC의 ${c.name}${ro(c.name)} 요약합니다. 전사본은 밖으로 나가지 않습니다`
        : `요약할 때 전사본이 ${c.name}(${host(c.base_url)})${ro(c.name)} 전송됩니다`
      : route.local
        ? "로컬 LLM 연결이 없습니다"
        : "LLM 연결이 없어 전사까지만 합니다";
  if (!text) return null;
  return (
    <div className={center ? "flex flex-col items-center gap-2 text-center" : "space-y-2"}>
      <p className="flex gap-2 text-sm text-muted-foreground">
        <Info aria-hidden className="mt-0.5 size-4 shrink-0" />
        {text}
      </p>
      {/* Also shown when "로컬로 요약" is on but its connection is gone, so it can be turned off. */}
      {!route.locked && (route.local_available || route.local) && (
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
export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="grid gap-1.5 text-sm">
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
  disabled,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="font-medium text-muted-foreground">{label}</span>
      <Select value={value} disabled={disabled} onValueChange={onChange}>
        <SelectTrigger aria-label={label} className="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map((o) => (
            <SelectItem key={o.value} value={o.value}>
              {o.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
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
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="outline" size={size} disabled={disabled} aria-label={label}>
          <Trash2 aria-hidden />
          삭제
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
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
