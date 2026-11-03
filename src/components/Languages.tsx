import { useEffect, useState } from "react";
import { ChevronDown } from "lucide-react";
import { ro } from "@/components/common";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import {
  modelDownload,
  modelDownloadFailure,
  modelDownloadScope,
  modelStatus,
  startModelDownload,
  transcribeLanguages,
  type Lang,
} from "@/lib/ipc";

// Plan 5.6-20..25 and design-system 8.7: one check list for the recording screen, the settings,
// the meeting's dialog and the import dialog. The languages come from Rust's table, in its order.

/** The language table, null until read: the pickers wait for it. `failure`: why it could not be
 * read, for `retry`. */
export function useLanguages() {
  const [langs, setLangs] = useState<Lang[] | null>(null);
  const [failure, setFailure] = useState("");
  const [tries, setTries] = useState(0);
  useEffect(() => {
    let live = true;
    setFailure("");
    transcribeLanguages().then(
      (l) => live && setLangs(l),
      (e) => live && setFailure(`언어 목록을 읽지 못했습니다: ${e}`),
    );
    return () => {
      live = false;
    };
  }, [tries]);
  return { langs, failure, retry: () => setTries((n) => n + 1) };
}

/** The names of `codes` in table order; none until the table is read. */
export const languageNames = (langs: Lang[] | null, codes: string[]) =>
  langs?.filter((l) => codes.includes(l.code)).map((l) => l.name) ?? [];

/** Plan 5.6-21: "한국어", "한국어·베트남어", "한국어 외 2개". */
export const shortName = (names: string[]) =>
  names.length > 2 ? `${names[0]} 외 ${names.length - 1}개` : names.join("·");

/** Plan 5.6-27: "한국어로 전사", "한국어·베트남어로 전사". */
export const transcribedIn = (names: string[]) => {
  const all = names.join("·");
  return `${all}${ro(all)} 전사`;
};

const same = (a: string[], b: string[]) => a.join() === b.join();

/** Plan 5.6-15: the detection model starts downloading as a second language is first picked, in
 * the app's one download (ipc.ts), after the one running if any. */
const fetchLid = () =>
  modelStatus("lid").then(
    (state) => {
      if (!state || state.files.every((f) => f.installed)) return;
      void (modelDownload() ?? Promise.resolve()).then(() => startModelDownload("lid"));
    },
    () => {},
  );

/** Plan 5.6-27: a meeting of two or more languages waiting in line without the detection model.
 * While `on`, it is fetched if nothing fetches it, and followed to its end. `failure`: the
 * download's own words, for 다시 시도. */
export function useLid(on: boolean) {
  const [lid, setLid] = useState<{ missing: boolean; failure: string } | null>(null);
  const [tries, setTries] = useState(0);
  useEffect(() => {
    if (!on) return;
    let live = true;
    const read = (): Promise<void> =>
      modelStatus("lid").then(
        (state) => {
          if (!live || !state) return;
          const missing = !state.files.every((f) => f.installed);
          let running = modelDownload();
          const failure = missing && !running && modelDownloadScope() === "lid" ? modelDownloadFailure() : "";
          if (missing && !running && !failure) running = startModelDownload("lid");
          setLid({ missing, failure });
          if (missing && running) return running.then(read);
        },
        () => {},
      );
    read();
    return () => {
      live = false;
    };
  }, [on, tries]);
  const retry = () => {
    void startModelDownload("lid");
    setTries((n) => n + 1);
  };
  return on && lid?.missing ? { failure: lid.failure, retry } : null;
}

/** The checks (design-system 8.7): whole rows are labels of 32px or more, the last one left is
 * disabled with its reason, and two or more say transcribing takes longer. */
export function LanguageChecks({
  langs,
  value,
  onChange,
}: {
  langs: Lang[];
  value: string[];
  onChange: (languages: string[]) => void;
}) {
  const pick = (code: string, on: boolean) => {
    // Kept in table order, the order Rust stores.
    const next = langs.map((l) => l.code).filter((c) => (c === code ? on : value.includes(c)));
    if (next.length > 1 && value.length < 2) void fetchLid();
    onChange(next);
  };
  return (
    <fieldset className="space-y-2">
      <legend className="mb-2 text-label text-ink-2">회의에서 쓰는 언어를 모두 고르세요 · 요약은 항상 한국어로 씁니다</legend>
      <div>
        {langs.map((l) => {
          const checked = value.includes(l.code);
          const last = checked && value.length === 1;
          return (
            <label
              key={l.code}
              className="-mx-2 flex min-h-9 items-center gap-2.5 rounded-lg px-2 text-body hover:bg-sunken has-disabled:hover:bg-transparent"
            >
              <input
                type="checkbox"
                className="size-4 shrink-0 accent-primary"
                checked={checked}
                disabled={last}
                onChange={(e) => pick(l.code, e.target.checked)}
              />
              {l.name}
              {last && <span className="ml-auto text-caption text-ink-2">하나는 골라야 합니다</span>}
            </label>
          );
        })}
      </div>
      {value.length > 1 && <p className="text-caption text-ink-2">여러 언어를 고르면 전사가 더 걸립니다</p>}
    </fieldset>
  );
}

/** A trigger showing the short name, named by the whole list, opening the checks with 적용 and
 * 취소 (plan 5.6-21). `onApply` rejects with the reason, shown inside with 다시 시도. */
export function LanguagePicker({
  label,
  value,
  small,
  side,
  className,
  onApply,
  ...rest
}: {
  /** What the list is for: the start of the trigger's name and the popover's. */
  label: string;
  value: string[];
  /** The recording screen's caption-sized trigger; else a field like a select. */
  small?: boolean;
  side?: "top" | "bottom";
  className?: string;
  onApply: (languages: string[]) => Promise<unknown>;
  /** A settings row's label and help point at the trigger. */
  id?: string;
  "aria-describedby"?: string;
}) {
  const { langs } = useLanguages();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(value);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const names = languageNames(langs, value);
  const show = (next: boolean) => {
    setOpen(next);
    if (!next) return;
    setDraft(value);
    setError("");
  };
  const apply = () => {
    if (same(draft, value)) return setOpen(false);
    setBusy(true);
    setError("");
    onApply(draft)
      .then(
        () => setOpen(false),
        (e) => setError(String(e)),
      )
      .finally(() => setBusy(false));
  };
  return (
    <Popover open={open} onOpenChange={show}>
      <PopoverTrigger asChild>
        <Button
          {...rest}
          variant={small ? "ghost" : "outline"}
          size={small ? "sm" : "default"}
          disabled={!langs}
          aria-label={`${label}: ${names.join(", ")}`}
          className={cn(small ? "text-caption text-ink" : "justify-between font-normal", className)}
        >
          {shortName(names)}
          <ChevronDown aria-hidden className="text-ink-2" />
        </Button>
      </PopoverTrigger>
      <PopoverContent side={side} aria-label={label} className="space-y-3">
        {langs && <LanguageChecks langs={langs} value={draft} onChange={setDraft} />}
        {error && <p className="text-caption text-danger-line">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={() => setOpen(false)}>
            취소
          </Button>
          <Button size="sm" disabled={busy} onClick={apply}>
            {error ? "다시 시도" : "적용"}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** Plan 5.6-25 (d10a): asked before a picked or dropped recording file is processed, the settings'
 * languages picked already. Cancelled, nothing is imported. */
export function ImportLanguages({
  initial,
  onImport,
  onCancel,
}: {
  /** The settings' languages while it asks; null when closed. */
  initial: string[] | null;
  onImport: (languages: string[]) => void;
  onCancel: () => void;
}) {
  return (
    <AlertDialog open={!!initial} onOpenChange={(open) => !open && onCancel()}>
      <AlertDialogContent aria-describedby={undefined}>
        {initial && <ImportChoice initial={initial} onImport={onImport} />}
      </AlertDialogContent>
    </AlertDialog>
  );
}

function ImportChoice({ initial, onImport }: { initial: string[]; onImport: (languages: string[]) => void }) {
  const { langs, failure, retry } = useLanguages();
  const [draft, setDraft] = useState(initial);
  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>녹음 파일 가져오기</AlertDialogTitle>
      </AlertDialogHeader>
      {langs && <LanguageChecks langs={langs} value={draft} onChange={setDraft} />}
      {failure && <p className="text-caption text-danger-line">{failure}</p>}
      <AlertDialogFooter>
        <AlertDialogCancel>취소</AlertDialogCancel>
        {/* One button: 다시 시도 turns back into 가져와서 전사 and keeps the focus (no `disabled`). */}
        <Button
          aria-disabled={(!langs && !failure) || undefined}
          className="aria-disabled:opacity-50"
          onClick={() => (failure ? retry() : langs && onImport(draft))}
        >
          {failure ? "다시 시도" : "가져와서 전사"}
        </Button>
      </AlertDialogFooter>
    </>
  );
}
