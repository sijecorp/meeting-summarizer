import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, CircleAlert, Info, Trash2, TriangleAlert, X } from "lucide-react";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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

/** Every screen's body under the app header: one width, one title style, the back link. */
export function Page({
  title,
  action,
  onHome,
  wide,
  children,
}: {
  title: string;
  /** Shown at the right of the title, e.g. Home's 녹음 시작. */
  action?: ReactNode;
  onHome?: () => void;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <main className={`mx-auto w-full space-y-6 px-6 py-8 ${wide ? "max-w-5xl" : "max-w-3xl"}`}>
      <div className="space-y-1">
        {onHome && (
          <Button variant="link" className="px-0" onClick={onHome}>
            <ArrowLeft aria-hidden />
            홈으로
          </Button>
        )}
        <div className="flex flex-wrap items-center justify-between gap-4">
          <h1 className="text-2xl font-bold">{title}</h1>
          {action}
        </div>
      </div>
      {children}
    </main>
  );
}

/** One look for a status in the meeting list, on the processing screen and in the app header. */
export function StatusBadge({ recording, children }: { recording?: boolean; children: ReactNode }) {
  return (
    <Badge variant="secondary">
      {recording && <span className="size-2 rounded-full bg-danger motion-safe:animate-pulse-faster" aria-hidden />}
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

function remaining(secs: number) {
  const min = Math.max(1, Math.round(secs / 60));
  const parts = [min >= 60 && `${Math.floor(min / 60)}시간`, min % 60 && `${min % 60}분`];
  return `약 ${parts.filter(Boolean).join(" ")} 남음`;
}

/** A running meeting's line, with its bar when there is a value to show (TRN-01, APP-07).
 * `detailed` adds "구간 2/5" while transcribing; a summary of more than one LLM request shows
 * "요청 2/5". `name` tells one bar from another in a list. Plain text and no live region, so a
 * screen reader is not told every tick. */
export function ProcessProgress({
  progress: p,
  detailed,
  name,
}: {
  progress: Progress;
  detailed?: boolean;
  name?: string;
}) {
  // A meeting's last event (its final status, with the last step's counts) can come before the
  // meeting is read again: there is nothing running to show for it.
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
  return (
    <div className="grid gap-1.5">
      {(!summarizing || p.chunks > 1) && (
        <ProgressBar
          value={p.percent}
          aria-label={`${label}${name ? ` (${name})` : ""}`}
          className="bg-cool200"
        />
      )}
      <span className="text-sm text-muted-foreground">{text}</span>
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

/** The host a connection sends to, for SUM-07 notices. */
export const host = (url: string) => (URL.canParse(url) ? new URL(url).host : url);

/** SUM-07 where the transcript goes for its summary, and SUM-08 "로컬로 요약" until summarizing
 * starts (`locked`). Read again when `status` changes. Once locked it shows only while
 * `summarizing`; then the review screen says where the transcript went. */
export function RouteNotice({ id, status }: { id: string; status?: Status }) {
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
        ? `이 PC의 ${c.name}(으)로 요약하고 있습니다`
        : `전사본을 ${c.name}(${host(c.base_url)})(으)로 보내 요약하고 있습니다`
      : ""
    : c
      ? c.is_local
        ? `이 PC의 ${c.name}(으)로 요약합니다. 전사본은 밖으로 나가지 않습니다`
        : `요약할 때 전사본이 ${c.name}(${host(c.base_url)})(으)로 전송됩니다`
      : route.local
        ? "로컬 LLM 연결이 없습니다"
        : "LLM 연결이 없어 전사까지만 합니다";
  if (!text) return null;
  return (
    <div className="space-y-2">
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

/** Error and warning banners (architecture 10.3): colored border and icon, black text. */
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

/** A disabled control for a feature that is not built yet, with a hint on hover or focus. */
export function NotYet({ hint, children }: { hint: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} className="inline-block">
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent>{hint}</TooltipContent>
    </Tooltip>
  );
}

/** Confirmed in a dialog so a single misclick never removes a meeting (REV-04). */
export function DeleteButton({
  id,
  onDeleted,
  onError,
}: {
  id: string;
  onDeleted: () => void;
  onError: (message: string) => void;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="outline">
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
