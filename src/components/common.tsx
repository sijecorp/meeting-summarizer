import type { ReactNode } from "react";
import { CircleAlert, Trash2, TriangleAlert, X } from "lucide-react";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { deleteMeeting } from "@/lib/ipc";

export function Header({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <header className="flex items-center justify-between border-b bg-card px-6 py-4">
      <h1 className="text-lg font-bold">{title}</h1>
      {children}
    </header>
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
