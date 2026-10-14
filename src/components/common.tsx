import type { ReactNode } from "react";
import { ArrowLeft, CircleAlert, Trash2, TriangleAlert, X } from "lucide-react";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  deleteMeeting,
  SCHEMA_VERSION,
  STATUS_LABEL,
  type Meta,
  type Progress,
  type Status,
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
// A meeting waits while it has a place in line, which is how a retried `transcribe_failed` one
// waits too. M4: revisit `transcribed` here and in `phase` once the summarizer queues it (APP-07).
const WAITING: Status[] = ["recorded", "paused"];

/** Running, waiting or neither. The header chip, Home and the processing screen all ask this, so
 * the chip's "처리 중 N · 대기 M" always matches the badges. */
export function phase(meta: Meta, progress: Progress | undefined) {
  // A meeting from a newer app version is read-only and never runs here (like `busy_ids`).
  if (meta.schema_version > SCHEMA_VERSION) return null;
  if (RUNNING.includes(meta.status)) return "running";
  // Finished in M3, whatever an old place in line says.
  if (meta.status === "transcribed") return null;
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

/** A running meeting's bar and line (TRN-01, APP-07). `detailed` adds "구간 2/5"; `name` tells
 * one bar from another in a list. Plain text and no live region, so a screen reader is not told
 * every tick. */
export function TranscribeProgress({
  progress: p,
  detailed,
  name,
}: {
  progress: Progress;
  detailed?: boolean;
  name?: string;
}) {
  // Until the chunk plan exists, `percent` is the planning's own; then it starts again from 0.
  const planning = p.chunks === 0;
  const text = planning
    ? "구간 나누는 중"
    : [
        detailed && `구간 ${Math.min(p.chunk + 1, p.chunks)}/${p.chunks}`,
        `${p.percent}%`,
        p.eta_secs != null && remaining(p.eta_secs),
      ]
        .filter(Boolean)
        .join(" · ");
  return (
    <div className="grid gap-1.5">
      <ProgressBar
        value={p.percent}
        aria-label={`${planning ? "구간 나누기 진행률" : "전사 진행률"}${name ? ` (${name})` : ""}`}
        className="bg-cool200"
      />
      <span className="text-sm text-muted-foreground">{text}</span>
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
