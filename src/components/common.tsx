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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { deleteMeeting } from "@/lib/ipc";

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
