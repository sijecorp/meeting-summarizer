import { cloneElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode, type Ref } from "react";
import { createPortal } from "react-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, motion, useIsPresent, type Transition } from "motion/react";
import { Check, ChevronDown, CircleAlert, Download, FolderOpen, Plus, RotateCcw, Trash2, TriangleAlert } from "lucide-react";
import { Banner, Field, host, Page, Present } from "@/components/common";
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
import { Progress as ProgressBar } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  cancelModelDownload,
  deleteSecret,
  getSettings,
  hasSecret,
  importModel,
  listAudioDevices,
  modelDownload,
  modelDownloadFailure,
  modelFileFilter,
  modelStatus,
  NEWER_VERSION,
  onMicTestLevel,
  onModelDownload,
  resetSettings,
  saveSettings,
  sendTestMessage,
  setSecret,
  startModelDownload,
  testMicrophone,
  testSystemAudio,
  type AudioDevice,
  type AudioDevices,
  type Backend,
  type LlmConnection,
  type LoadedSettings,
  type ModelDownload,
  type ModelFileState,
  type ModelState,
  type SecretKind,
  type Settings as SettingsData,
} from "@/lib/ipc";
import { dur, ease, spring } from "@/lib/motion";
import { cn } from "@/lib/utils";

export type SettingsTab = "general" | "audio" | "transcription" | "llm" | "slack";

const TABS: { value: SettingsTab; label: string; about: string }[] = [
  { value: "general", label: "일반", about: "회의록에 남는 이름과 알림입니다." },
  { value: "audio", label: "오디오", about: "녹음할 마이크와 시스템 오디오를 고릅니다." },
  { value: "transcription", label: "전사", about: "녹음은 이 PC에서 글로 옮깁니다. 모델은 처음 한 번만 받습니다." },
  {
    value: "llm",
    label: "요약(LLM)",
    about: "전사본을 요약할 LLM 연결입니다. 클라우드 연결은 전사본을 그 주소로 보냅니다.",
  },
  { value: "slack", label: "Slack", about: "요약을 보낼 채널의 웹훅입니다. URL은 OS 키체인에만 저장합니다." },
];

const BACKENDS: { value: Backend; label: string }[] = [
  { value: "auto", label: "자동" },
  { value: "gpu", label: "GPU" },
  { value: "cpu", label: "CPU" },
];

// Architecture 5.1. `tiny` is for development only and never offered.
const MODELS = [
  { value: "large-v3-turbo-q8_0", label: "기본: large-v3-turbo q8_0 (874MB)" },
  { value: "large-v3-turbo", label: "원본: large-v3-turbo (1.6GB)" },
  { value: "medium-q5_0", label: "가벼움: medium q5_0 (539MB)" },
];
// `model_status` lists the Whisper model, then the VAD model.
const MODEL_FILES = ["전사 모델", "음성 구간 감지(VAD) 모델"];
// One decimal below 10 MB, for the 0.9 MB VAD file.
const mb = (bytes: number) => (bytes < 1e7 ? (bytes / 1e6).toFixed(1) : Math.round(bytes / 1e6));

const cloudTimeout = 300;
const localTimeout = 900;
// Radix Select items cannot have an empty value. Names are trimmed and non-empty and device ids
// are `host:id`, so a space is never a connection's name or a device id.
const NO_DEFAULT = " ";

// The macOS global tap records every output device, so only Windows pins one (capture.rs).
const pinsOutput = navigator.userAgent.includes("Windows");

// u32 on the Rust side; an emptied number field arrives as Number("") = 0.
const isPositiveInt = (n: number) => Number.isInteger(n) && n >= 1 && n <= 0xffffffff;
const terms = (glossary: string) =>
  glossary
    .split("\n")
    .map((term) => term.trim())
    .filter(Boolean);

// Keychain entries are keyed `kind:name`, one per connection and webhook.
type SecretRef = { kind: SecretKind; name: string };
const secretRefs = (s: SettingsData): SecretRef[] => [
  ...s.llm_connections.map((c) => ({ kind: "llm" as const, name: c.name })),
  ...s.webhooks.map((w) => ({ kind: "slack" as const, name: w.name })),
];
const keyOf = ({ kind, name }: SecretRef) => `${kind}:${name}`;

/** The settings as 저장 writes them: the name trimmed, and the local default only with a local
 * connection (its switch hides without one, so it cannot stay on unseen). */
const asSaved = (s: SettingsData): SettingsData => ({
  ...s,
  recorder_name: s.recorder_name.trim(),
  summarize_local_by_default: s.summarize_local_by_default && s.llm_connections.some((c) => c.is_local),
});

// What the save bar counts: each field as 저장 would write it, and each connection or webhook
// added, removed or edited once however many of its fields changed.
const FIELDS = [
  "recorder_name",
  "backend",
  "model",
  "model_base_url",
  "input_device",
  "output_device",
  "default_llm",
  "summarize_local_by_default",
] as const;
const entryChanges = <T extends { name: string }>(now: T[], saved: T[]) => {
  const before = new Map(saved.map((e) => [e.name, JSON.stringify(e)]));
  const added = new Set(now.map((e) => e.name));
  return (
    now.filter((e) => before.get(e.name) !== JSON.stringify(e)).length +
    saved.filter((e) => !added.has(e.name)).length
  );
};

/** 저장했습니다 stays in the save bar this long before the bar leaves. */
const SAVED_MS = 1500;
const NOW = { duration: 0 };
// One quiet surface per group: hairlines between rows, no border, no shadow (design-system 1).
const GROUP = "min-w-0 divide-y divide-line rounded-float bg-surface";
const HELP = "text-label font-normal text-ink-2";
// A control at a row's right, under its label on a narrow window.
const CONTROL = "w-full sm:w-72 sm:shrink-0";
// The mic check's level starts empty; events then write it straight to the DOM.
const LEVEL_ZERO = { transform: "scaleX(0)" };

// A check's outcome (ONB-03, ONB-06), in words the recorder can act on.
type Outcome = { ok: boolean; text: string };
type CheckState = "running" | Outcome;

// One 저장 for every tab, keys included: the panels stay mounted while hidden and typed keys live
// in this component's state, so switching tabs never drops an edit.
export default function Settings({
  tab: initialTab = "general",
  barSlot,
  recording = false,
  onHome,
}: {
  tab?: SettingsTab;
  /** App's bottom stack: the save bar goes there, above the dock. */
  barSlot?: HTMLElement | null;
  /** A recording is running or starting: the audio checks wait (they open devices too). */
  recording?: boolean;
  onHome: () => void;
}) {
  const [tab, setTab] = useState(initialTab);
  const [draft, setDraft] = useState<SettingsData | null>(null);
  // What settings.json holds: keychain entries follow this, never the unsaved draft.
  const [persisted, setPersisted] = useState<SettingsData | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [glossary, setGlossary] = useState("");
  const [secrets, setSecrets] = useState<Record<string, boolean>>({});
  // Keys typed but not stored yet, by `keyOf`. They reach the keychain only on 저장 (9.2).
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string>>({});
  // Keys of removed entries that the keychain failed to delete: retried on every 저장.
  const [orphans, setOrphans] = useState<SecretRef[]>([]);
  // While saving: the count 저장 was pressed with. The bar holds it until the keychain steps end,
  // though the draft is the saved one before then (9.2).
  const [saving, setSaving] = useState<number | null>(null);
  // A save that wrote everything: the bar says so for a moment, then leaves.
  const [done, setDone] = useState(false);
  const [revertAsked, setRevertAsked] = useState(false);
  const [newLlm, setNewLlm] = useState("");
  const [newWebhook, setNewWebhook] = useState("");
  // Why a name cannot be added, under the row that adds it.
  const [nameError, setNameError] = useState<{ kind: SecretKind; text: string } | null>(null);
  // The connection and the webhook open for editing, one of each.
  const [expanded, setExpanded] = useState<Record<SecretKind, string | null>>({ llm: null, slack: null });
  // A key picked the tab or opened the row: nothing moves (design-system 5).
  const [instant, setInstant] = useState(false);
  const [devices, setDevices] = useState<AudioDevices | null>(null);
  const [deviceError, setDeviceError] = useState("");
  const [error, setError] = useState("");
  // Each time 저장 reports an error, even the same one again: its banner is said and shown anew.
  const [reported, setReported] = useState(0);
  const [models, setModels] = useState<ModelState | null>(null);
  const [received, setReceived] = useState<Record<string, ModelDownload>>({});
  const [modelBusy, setModelBusy] = useState<"download" | "import" | null>(null);
  // Both shown in the 전사 tab, not the shared banner. A status read that works clears its own
  // error but never a download's or import's, which lasts until the next try.
  const [statusError, setStatusError] = useState("");
  const [modelError, setModelError] = useState(modelDownloadFailure);
  // The audio and Slack checks, by "mic", "system" or the webhook's `keyOf`. Never part of the
  // draft. `on`: the device or typed URL it ran with; no `outcome` while it runs.
  const [checks, setChecks] = useState<Record<string, { on: string | null; outcome?: Outcome }>>({});
  const micLevel = useRef<HTMLDivElement>(null);
  const present = useIsPresent();
  const nav = useRef<HTMLElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const banner = useRef<HTMLDivElement>(null);
  const newLlmField = useRef<HTMLInputElement>(null);
  const newWebhookField = useRef<HTMLInputElement>(null);
  // The bar the revert dialog came from is gone once it reverts.
  const reverted = useRef(false);

  const show = async ({ settings, read_only }: LoadedSettings) => {
    setDraft(settings);
    setPersisted(settings);
    setReadOnly(read_only);
    setGlossary(settings.glossary.join("\n"));
    setError("");
    const saved = await Promise.all(
      secretRefs(settings).map(async (r) => [keyOf(r), await hasSecret(r.kind, r.name)] as const),
    );
    setSecrets(Object.fromEntries(saved));
  };

  // The 전사 tab's files only; the summary model belongs to the 요약(LLM) tab (plan 5.8).
  const readModels = () =>
    modelStatus("transcription").then(
      (state) => {
        setModels(state);
        setStatusError("");
      },
      (e) => setStatusError(String(e)),
    );

  // The download lives in ipc.ts, so this also picks up one started before the screen opened.
  const follow = (running: Promise<void>) => {
    setModelBusy("download");
    running.then(() => {
      setModelBusy(null);
      setModelError(modelDownloadFailure());
      readModels();
    });
  };

  useEffect(() => {
    getSettings()
      .then(show)
      .catch((e) => setError(String(e)));
    const unlisten = onModelDownload((d) => setReceived((all) => ({ ...all, [d.file]: d })));
    unlisten.catch((e) => setStatusError(`다운로드 진행률을 받지 못했습니다: ${e}`));
    // 25 times a second, straight to the DOM: no React state (design-system 5).
    const unlistenLevel = onMicTestLevel((level) => {
      const meter = micLevel.current;
      meter?.setAttribute("aria-valuenow", String(level));
      (meter?.firstElementChild as HTMLElement | null)?.style.setProperty("transform", `scaleX(${level / 100})`);
    });
    const running = modelDownload();
    if (running) follow(running);
    return () => {
      for (const u of [unlisten, unlistenLevel]) u.then((stop) => stop(), () => {});
    };
  }, []);

  // Read each time the tab opens, like the audio devices.
  useEffect(() => {
    if (tab === "transcription") readModels();
  }, [tab]);

  // Listed again each time the tab opens, so a headset plugged in meanwhile shows up.
  useEffect(() => {
    if (tab !== "audio") return;
    listAudioDevices().then(
      (list) => {
        setDevices(list);
        setDeviceError("");
      },
      (e) => setDeviceError(`오디오 장치 목록을 읽지 못했습니다: ${e}`),
    );
  }, [tab]);

  useEffect(() => {
    if (!done) return;
    const timer = setTimeout(() => setDone(false), SAVED_MS);
    return () => clearTimeout(timer);
  }, [done]);

  // 저장 is at the bottom of the window and its errors at the top of the tab.
  useEffect(() => {
    if (reported) banner.current?.scrollIntoView({ block: "nearest" });
  }, [reported]);

  if (!draft || !persisted) {
    // Unreadable settings.json: the old file is kept as a backup before starting over.
    return (
      <Page title="설정" onHome={onHome} width="wide">
        {error && (
          <>
            <Banner kind="error">{error}</Banner>
            <p className="text-muted-foreground">
              기본값으로 다시 시작하면 지금 파일은 settings.json.bak-(시각) 이름으로 남겨 둡니다.
            </p>
            <Button
              variant="outline"
              onClick={() =>
                resetSettings()
                  .then(show)
                  .catch((e) => setError(String(e)))
              }
            >
              <RotateCcw aria-hidden />
              기본값으로 다시 시작
            </Button>
          </>
        )}
      </Page>
    );
  }

  const locked = readOnly || saving !== null;
  const move: Transition = instant ? NOW : spring.gentle;

  const edit = (patch: Partial<SettingsData>) => {
    setDraft({ ...draft, ...patch });
    setDone(false);
  };
  const editLlm = (index: number, patch: Partial<LlmConnection>) =>
    edit({
      llm_connections: draft.llm_connections.map((c, i) => (i === index ? { ...c, ...patch } : c)),
    });

  const pick = (next: SettingsTab, keyboard: boolean) => {
    setInstant(keyboard);
    setTab(next);
  };
  const toggle = (kind: SecretKind, name: string, keyboard: boolean) => {
    setInstant(keyboard);
    setExpanded((x) => ({ ...x, [kind]: x[kind] === name ? null : name }));
  };
  // The bar's buttons leave with what they act on: focus goes to the open tab, never the page.
  const focusTab = () => nav.current?.querySelector<HTMLElement>("[aria-current=page]")?.focus();
  const report = (message: string) => {
    setError(message);
    setReported((n) => n + 1);
  };

  // A name keys its keychain entry, so names are fixed once added: remove and re-add to rename.
  // A saved name removed in this draft keeps its key until 저장 deletes it, and so does an orphan
  // until a 저장 manages to, so neither can come back before then: it would inherit the old key.
  const keyHolders = (kind: SecretKind, saved: { name: string }[]) => [
    ...saved.map((e) => e.name),
    ...orphans.filter((o) => o.kind === kind).map((o) => o.name),
  ];
  const nameProblem = (name: string, taken: string[], saved: string[]) =>
    !name
      ? "이름을 입력하세요"
      : taken.includes(name)
        ? "같은 이름이 이미 있습니다"
        : saved.includes(name)
          ? "저장한 뒤 다시 추가하세요"
          : "";

  // A new entry opens, ready for its address and key.
  const addLlm = (keyboard: boolean) => {
    const name = newLlm.trim();
    const problem = nameProblem(
      name,
      draft.llm_connections.map((c) => c.name),
      keyHolders("llm", persisted.llm_connections),
    );
    if (problem) return setNameError({ kind: "llm", text: problem });
    edit({
      llm_connections: [
        ...draft.llm_connections,
        { name, base_url: "", model: "", context_tokens: 8192, is_local: false, timeout_secs: cloudTimeout },
      ],
      default_llm: draft.default_llm ?? name,
    });
    setNewLlm("");
    setNameError(null);
    setInstant(keyboard);
    setExpanded((x) => ({ ...x, llm: name }));
  };

  const addWebhook = (keyboard: boolean) => {
    const name = newWebhook.trim();
    const problem = nameProblem(
      name,
      draft.webhooks.map((w) => w.name),
      keyHolders("slack", persisted.webhooks),
    );
    if (problem) return setNameError({ kind: "slack", text: problem });
    edit({ webhooks: [...draft.webhooks, { name }] });
    setNewWebhook("");
    setNameError(null);
    setInstant(keyboard);
    setExpanded((x) => ({ ...x, slack: name }));
  };

  // Only the draft changes; the keychain entry goes once the settings are saved.
  const remove = (kind: SecretKind, name: string, keyboard: boolean) => {
    setInstant(keyboard);
    setSecretDrafts((d) => ({ ...d, [keyOf({ kind, name })]: "" }));
    if (kind === "llm") {
      edit({
        llm_connections: draft.llm_connections.filter((c) => c.name !== name),
        default_llm: draft.default_llm === name ? null : draft.default_llm,
      });
    } else {
      edit({ webhooks: draft.webhooks.filter((w) => w.name !== name) });
    }
  };

  const glossaryCount = terms(glossary).length;
  const hasLocal = draft.llm_connections.some((c) => c.is_local);

  // Every typed key counts, even blanks (저장 clears them), and so does a key the keychain still
  // has to delete: only 저장 finishes either.
  const now = asSaved(draft);
  const then = asSaved(persisted);
  const changes =
    FIELDS.filter((k) => now[k] !== then[k]).length +
    Number(terms(glossary).join("\n") !== persisted.glossary.join("\n")) +
    entryChanges(draft.llm_connections, persisted.llm_connections) +
    entryChanges(draft.webhooks, persisted.webhooks) +
    Object.values(secretDrafts).filter(Boolean).length +
    orphans.length;
  const typedKeys = Object.values(secretDrafts).some(Boolean);

  // Back to what settings.json holds, typed keys included. A key left to delete stays: it is
  // already gone from the settings.
  const revert = (keyboard: boolean) => {
    setInstant(keyboard);
    setDraft(persisted);
    setGlossary(persisted.glossary.join("\n"));
    setSecretDrafts({});
    setNameError(null);
    setError("");
    if (bar.current?.contains(document.activeElement)) focusTab();
  };

  const save = async () => {
    // Nothing can change then, so the bar never shows; this keeps it so.
    if (readOnly) return;
    // Each check opens the tab, and the row, that holds the field: at once, since that tab was
    // hidden a moment ago.
    const fail = (at: SettingsTab, message: string) => {
      setInstant(true);
      setTab(at);
      report(message);
    };
    if (glossaryCount > 100) return fail("transcription", "용어집은 100개까지 넣을 수 있습니다");
    if (!draft.model_base_url.trim()) return fail("transcription", "모델 다운로드 주소를 입력하세요");
    const invalid = draft.llm_connections.find(
      (c) => !isPositiveInt(c.context_tokens) || !isPositiveInt(c.timeout_secs),
    );
    if (invalid) {
      setExpanded((x) => ({ ...x, llm: invalid.name }));
      return fail("llm", `컨텍스트 토큰과 응답 시간 제한은 1 이상의 정수여야 합니다: ${invalid.name}`);
    }
    const value = {
      ...now,
      // Same rule as settings.rs: only switching the backend to GPU clears it.
      gpu_failed: draft.gpu_failed && !(draft.backend === "gpu" && persisted.backend !== "gpu"),
      glossary: terms(glossary),
    };
    const kept = secretRefs(value);
    const keptKeys = new Set(kept.map(keyOf));
    const removed = [...secretRefs(persisted), ...orphans].filter((r) => !keptKeys.has(keyOf(r)));
    // The form and the bar are locked until this ends, so a second 저장 cannot interleave with it.
    setSaving(changes);
    try {
      try {
        await saveSettings(value);
      } catch (e) {
        // settings.json still lists the removed entries and not the new ones, so the keychain
        // stays as it is and the typed keys stay in the draft.
        return report(String(e));
      }
      setDraft(value);
      setPersisted(value);
      // The model may have changed.
      readModels();
      const problems: string[] = [];
      const stillOrphaned: SecretRef[] = [];
      for (const r of removed) {
        await deleteSecret(r.kind, r.name).then(
          () => setSecrets((s) => ({ ...s, [keyOf(r)]: false })),
          (e) => {
            stillOrphaned.push(r);
            problems.push(`키체인에서 키를 지우지 못했습니다: ${e}`);
          },
        );
      }
      setOrphans(stillOrphaned);
      // Only now: settings.json lists every kept entry, so no stored key can be orphaned.
      for (const r of kept) {
        const key = keyOf(r);
        const typed = secretDrafts[key]?.trim();
        if (!typed) {
          if (secretDrafts[key]) setSecretDrafts((d) => ({ ...d, [key]: "" }));
          continue;
        }
        await setSecret(r.kind, r.name, typed).then(
          () => {
            setSecrets((s) => ({ ...s, [key]: true }));
            setSecretDrafts((d) => ({ ...d, [key]: "" }));
          },
          // The draft stays, so the next 저장 tries again.
          (e) => problems.push(`키체인에 키를 저장하지 못했습니다: ${e}`),
        );
      }
      // A Slack check's result spoke of the URL before this save; the typed one it ran with is
      // gone now, so it would read as the new URL's.
      setChecks((c) =>
        Object.fromEntries(Object.entries(c).filter(([key, check]) => !key.startsWith("slack:") || !check.outcome)),
      );
      if (problems.length) report(`설정은 저장했지만 ${problems.join(" / ")}`);
      else setError("");
      setDone(!problems.length);
      // Nothing is left to save, so 저장 leaves the bar.
      if (!problems.length && bar.current?.contains(document.activeElement)) focusTab();
    } finally {
      setSaving(null);
    }
  };

  const secretProps = (kind: SecretKind, name: string) => {
    const key = keyOf({ kind, name });
    return {
      saved: !!secrets[key],
      value: secretDrafts[key] ?? "",
      onChange: (typed: string) => {
        setSecretDrafts((d) => ({ ...d, [key]: typed }));
        setDone(false);
      },
    };
  };

  // Download and import act on the saved model and address: `model_status` and the backend read
  // settings.json. So an unsaved change blocks both until 저장, rather than acting on the old one.
  const unsaved =
    draft.model !== persisted.model || draft.model_base_url !== persisted.model_base_url;
  const downloading = modelBusy === "download" || !!models?.downloading;
  // While it runs, only 취소 and the progress show.
  const idle = !modelBusy && !models?.downloading && !unsaved;
  const missing = !!models?.files.some((f) => !f.installed);
  // A download started on Home may be past this tab's files: then nothing here to cancel.
  const cancellable = downloading && !!models?.files.some((f) => f.downloading || !f.installed);
  const fileState = (f: ModelFileState) => {
    const d = received[f.file];
    if (f.installed) return "설치됨";
    if (modelBusy === "download" && d) {
      return `받는 중 ${Math.floor((d.received / d.total) * 100)}% (${mb(d.received)}/${mb(d.total)}MB)`;
    }
    return f.received ? `없음 · ${mb(f.received)}/${mb(f.size)}MB 받아 둠` : "없음";
  };

  const startDownload = () => {
    setReceived({});
    setModelError("");
    follow(startModelDownload("transcription"));
  };

  // ONB-02: a model file got some other way (a colleague, a USB stick).
  const importFile = () =>
    open({ filters: [modelFileFilter("transcription")] })
      .then((path) => {
        if (!path) return;
        setModelBusy("import");
        return importModel(path, "transcription")
          .then(setModels)
          .finally(() => setModelBusy(null));
      })
      .then(
        () => setModelError(""),
        (e) => setModelError(String(e)),
      );

  // The checks act on what the tab shows, saved or not, and leave the draft alone.
  const run = (key: string, on: string | null, check: () => Promise<Outcome>) => {
    setChecks((c) => ({ ...c, [key]: { on } }));
    check().then(
      (outcome) => setChecks((c) => ({ ...c, [key]: { on, outcome } })),
      (e) => setChecks((c) => ({ ...c, [key]: { on, outcome: { ok: false, text: String(e) } } })),
    );
  };
  // An outcome speaks only of the device or URL it ran with: picking or typing another hides it,
  // even one that arrives later.
  const checkState = (key: string, on: string | null): CheckState | undefined => {
    const check = checks[key];
    if (!check?.outcome) return check && "running";
    return check.on === on ? check.outcome : undefined;
  };
  // Rust runs one audio check at a time, so neither is offered while either runs.
  const auditing = ["mic", "system"].some((key) => checks[key] && !checks[key].outcome);

  // A pinned device missing from the list still shows, so the select is never blank. Only a
  // list that loaded can tell it is unplugged.
  const deviceOptions = (list: AudioDevice[] | undefined, pinned: string | null) => [
    { value: NO_DEFAULT, label: "시스템 기본값" },
    ...(list ?? []).map((d) => ({ value: d.id, label: d.name })),
    ...(pinned && !list?.some((d) => d.id === pinned)
      ? [{ value: pinned, label: list ? "설정한 장치 (지금 연결되지 않음)" : "설정한 장치" }]
      : []),
  ];

  // Rows move only when a row opens or closes or the list changes: a tab coming into view (its
  // rows measured as hidden a moment ago) must not fly in.
  const llmMoves = [expanded.llm, ...draft.llm_connections.map((c) => c.name)].join("\n");
  const slackMoves = [expanded.slack, ...draft.webhooks.map((w) => w.name)].join("\n");

  return (
    <Page title="설정" onHome={onHome} width="wide">
      <div className="flex flex-col gap-8 md:flex-row md:items-start md:gap-12">
        <nav
          ref={nav}
          aria-label="설정 메뉴"
          className="flex flex-wrap gap-1 md:sticky md:top-16 md:w-40 md:shrink-0 md:flex-col"
        >
          {TABS.map((t) => (
            <button
              key={t.value}
              aria-current={tab === t.value ? "page" : undefined}
              className="relative rounded-control px-3 py-2 text-left font-medium text-ink-2 hover:text-ink aria-[current=page]:text-ink"
              onClick={(e) => pick(t.value, e.detail === 0)}
            >
              {/* The selection slides from tab to tab (design-system 6.7). */}
              {tab === t.value && (
                <motion.span
                  data-indicator
                  layoutId="settings-tab"
                  transition={instant ? NOW : spring.snappy}
                  style={{ borderRadius: 10 }}
                  className="absolute inset-0 bg-sunken"
                />
              )}
              <span className="relative">{t.label}</span>
            </button>
          ))}
        </nav>

        {/* Room under the last row for the save bar over the dock: App keeps room for the dock only. */}
        <div className="min-w-0 flex-1 space-y-6 pb-16">
          {readOnly && <Banner kind="warning">{NEWER_VERSION}. 설정을 바꿀 수 없습니다.</Banner>}
          <div ref={banner} hidden={!error}>
            {error && (
              <Banner key={reported} kind="error">
                {error}
              </Banner>
            )}
          </div>

          <Section tab="general" open={tab}>
            <fieldset disabled={locked} className={GROUP}>
              <Row label="녹음자 이름" help="Slack 메시지에 표시됩니다">
                <Input
                  className={CONTROL}
                  value={draft.recorder_name}
                  onChange={(e) => edit({ recorder_name: e.target.value })}
                />
              </Row>
            </fieldset>
            {/* ONB-03: desktop notification permission cannot be read, so this is all we can say. */}
            <p className={cn(HELP, "px-5")}>알림이 보이지 않으면 OS 알림 설정에서 'meeting-summarizer'를 허용해 주세요</p>
          </Section>

          <Section tab="audio" open={tab}>
            {deviceError && <Banner kind="error">{deviceError}</Banner>}
            <fieldset disabled={locked} className={GROUP}>
              <Row
                label="마이크"
                below={
                  // Opens the mic only when asked: opening it with the tab would raise the OS
                  // permission prompt from the settings.
                  <RunCheck
                    name="마이크 테스트"
                    action="테스트"
                    busyText="테스트 중…"
                    state={checkState("mic", draft.input_device)}
                    blocked={recording || auditing}
                    idle="5초 동안 마이크 소리를 들어 봅니다"
                    hint="마이크에 대고 말해 보세요"
                    running={
                      <div
                        ref={micLevel}
                        role="meter"
                        aria-label="마이크 입력 레벨"
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={0}
                        className="h-1.5 w-32 overflow-hidden rounded-full bg-sunken"
                      >
                        {/* Square ends, so scaleX never warps them (design-system 5). */}
                        <div className="size-full origin-left bg-brand" style={LEVEL_ZERO} />
                      </div>
                    }
                    onRun={() =>
                      run("mic", draft.input_device, () =>
                        testMicrophone(draft.input_device).then((heard) =>
                          heard
                            ? { ok: true, text: "소리가 들어와요" }
                            : {
                                ok: false,
                                text: pinsOutput
                                  ? "소리가 들어오지 않아요. 마이크 권한과 장치를 확인하세요"
                                  : "소리가 들어오지 않아요. 고른 마이크가 맞는지 보고, 시스템 설정 > 개인정보 보호 및 보안 > 마이크에서 'meeting-summarizer'를 허용하세요",
                              },
                        ),
                      )
                    }
                  />
                }
              >
                <Pick
                  disabled={locked}
                  value={draft.input_device ?? NO_DEFAULT}
                  options={deviceOptions(devices?.inputs, draft.input_device)}
                  onChange={(v) => edit({ input_device: v === NO_DEFAULT ? null : v })}
                />
              </Row>
              <Row
                label="시스템 오디오"
                help={pinsOutput ? "녹음할 출력 장치" : "모든 출력 장치의 소리를 함께 녹음합니다."}
                below={
                  <RunCheck
                    name="시스템 오디오 테스트"
                    action="테스트"
                    busyText="테스트 중…"
                    state={checkState("system", draft.output_device)}
                    blocked={recording || auditing}
                    idle="짧은 소리를 재생해 앱에 들리는지 확인합니다"
                    hint="테스트음을 재생하고 있어요"
                    onRun={() =>
                      run("system", draft.output_device, () =>
                        // The loopback most likely hears what plays before the output volume, so
                        // the volume is not offered as a cause.
                        testSystemAudio(draft.output_device).then((heard) =>
                          heard
                            ? { ok: true, text: "시스템 오디오가 잡혀요" }
                            : {
                                ok: false,
                                text: pinsOutput
                                  ? "시스템 오디오가 잡히지 않아요. 고른 출력 장치로 소리가 나는지 확인하세요"
                                  : "시스템 오디오가 잡히지 않아요. 시스템 설정 > 개인정보 보호 및 보안 > 화면 및 시스템 오디오 녹음에서 'meeting-summarizer'를 허용하세요",
                              },
                        ),
                      )
                    }
                  />
                }
              >
                {pinsOutput ? (
                  <Pick
                    disabled={locked}
                    value={draft.output_device ?? NO_DEFAULT}
                    options={deviceOptions(devices?.outputs, draft.output_device)}
                    onChange={(v) => edit({ output_device: v === NO_DEFAULT ? null : v })}
                  />
                ) : undefined}
              </Row>
            </fieldset>
            <div className={cn(HELP, "space-y-1 px-5")}>
              {/* REC-10 */}
              <p>헤드폰을 쓰면 상대방 목소리가 마이크로 다시 들어가는 에코가 줄어듭니다.</p>
              {/* One line for both checks, so a recording starting is said once. */}
              <p role="status">{recording ? "녹음 중에는 테스트할 수 없어요" : ""}</p>
            </div>
          </Section>

          <Section tab="transcription" open={tab}>
            {statusError && <Banner kind="error">{statusError}</Banner>}
            {modelError && <Banner kind="error">{modelError}</Banner>}
            <fieldset disabled={locked} className={GROUP}>
              <Row label="모델">
                <Pick
                  value={draft.model}
                  // A hand-set model (like `tiny`) still shows, so the select is never blank.
                  options={
                    MODELS.some((m) => m.value === draft.model)
                      ? MODELS
                      : [...MODELS, { value: draft.model, label: draft.model }]
                  }
                  disabled={locked || !!modelBusy}
                  onChange={(model) => edit({ model })}
                />
              </Row>
              <div className="space-y-4 px-5 py-4">
                {models && (
                  <ul className="space-y-3">
                    {models.files.map((f, i) => {
                      const d = received[f.file];
                      return (
                        <li key={f.file} className="space-y-2">
                          <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                            <span>
                              {MODEL_FILES[i]} <span className="text-caption text-ink-2">{f.file}</span>
                            </span>
                            <span className="flex items-center gap-1.5 text-label tabular-nums">
                              {f.installed && <Check aria-hidden className="size-4 text-ok-line" />}
                              {fileState(f)}
                            </span>
                          </div>
                          {modelBusy === "download" && d && !f.installed && (
                            <ProgressBar
                              aria-label={`${MODEL_FILES[i]} 받는 중`}
                              value={Math.floor((d.received / d.total) * 100)}
                            />
                          )}
                        </li>
                      );
                    })}
                  </ul>
                )}
                {/* Import stays offered when the status cannot be read (ONB-02). */}
                {(missing || downloading || modelBusy || statusError) && (
                  <div className="flex flex-wrap gap-2">
                    {missing && !downloading && (
                      <Button size="sm" disabled={!idle} onClick={startDownload}>
                        <Download aria-hidden />
                        다운로드
                      </Button>
                    )}
                    {cancellable && (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() =>
                          cancelModelDownload("transcription").then(
                            (done) => {
                              // Nothing of the tab's was left: the list was stale.
                              if (!done) readModels();
                            },
                            (e) => setModelError(String(e)),
                          )
                        }
                      >
                        취소
                      </Button>
                    )}
                    {!downloading && (
                      <Button size="sm" variant="ghost" disabled={!idle} onClick={importFile}>
                        <FolderOpen aria-hidden />
                        {modelBusy === "import" ? "가져오는 중" : "직접 가져오기"}
                      </Button>
                    )}
                  </div>
                )}
                {unsaved && <p className={HELP}>바꾼 모델과 주소는 저장한 뒤에 받거나 가져올 수 있습니다</p>}
              </div>
              <Row label="모델 다운로드 주소" help="사내 미러가 있으면 바꿉니다">
                <Input
                  className={CONTROL}
                  value={draft.model_base_url}
                  placeholder="https://huggingface.co"
                  onChange={(e) => edit({ model_base_url: e.target.value })}
                />
              </Row>
            </fieldset>
            <fieldset disabled={locked} className={GROUP}>
              <Row label="전사 백엔드" help="자동은 GPU로 처리하다 문제가 생기면 CPU로 바꿉니다">
                <Segmented
                  value={draft.backend}
                  options={BACKENDS}
                  onChange={(backend) => edit({ backend })}
                />
              </Row>
              {/* 5.4: only automatic mode falls back; choosing GPU and saving clears it. */}
              {persisted.gpu_failed && persisted.backend === "auto" && (
                <p className="flex gap-2 px-5 py-3 text-label font-normal">
                  <TriangleAlert aria-hidden className="mt-px size-4 shrink-0 text-warn-line" />
                  GPU 전사에 실패해 CPU로 처리하고 있습니다. 전사 백엔드를 GPU로 골라 저장하면 다시 GPU로 처리합니다.
                </p>
              )}
              <Row
                stacked
                label="용어집"
                help="사람 이름, 제품명을 한 줄에 하나씩"
                aside={<span className={cn(HELP, "tabular-nums")}>{glossaryCount}/100</span>}
              >
                <Textarea
                  className="h-32 field-sizing-fixed"
                  aria-invalid={glossaryCount > 100}
                  value={glossary}
                  onChange={(e) => {
                    setGlossary(e.target.value);
                    setDone(false);
                  }}
                />
              </Row>
            </fieldset>
          </Section>

          <Section tab="llm" open={tab}>
            <List moves={llmMoves} move={move}>
              {draft.llm_connections.map((c, i) => (
                <Entry
                  key={c.name}
                  label={[c.name, c.is_local ? "로컬" : "클라우드", host(c.base_url), draft.default_llm === c.name && "기본 LLM"]
                    .filter(Boolean)
                    .join(", ")}
                  open={expanded.llm === c.name}
                  moves={llmMoves}
                  instant={instant}
                  onToggle={(keyboard) => toggle("llm", c.name, keyboard)}
                  head={
                    <>
                      <span className="min-w-0 flex-1">
                        <span className="flex items-baseline gap-2">
                          <span className="truncate font-medium">{c.name}</span>
                          {draft.default_llm === c.name && <span className="shrink-0 text-label text-brand">기본</span>}
                        </span>
                        <span className={cn(HELP, "block truncate")}>{c.base_url ? host(c.base_url) : "주소 없음"}</span>
                      </span>
                      {/* In words, not a color (NFR-09). */}
                      <span className="shrink-0 rounded-full bg-sunken px-2 py-0.5 text-caption text-ink-2">
                        {c.is_local ? "로컬" : "클라우드"}
                      </span>
                    </>
                  }
                >
                  <fieldset disabled={locked} className="grid min-w-0 gap-4 px-5 pt-1 pb-5 sm:grid-cols-2">
                    <Field label="Base URL" className="sm:col-span-2">
                      <Input
                        value={c.base_url}
                        placeholder="https://api.example.com/v1"
                        onChange={(e) => editLlm(i, { base_url: e.target.value })}
                      />
                    </Field>
                    {/* SUM-07 */}
                    {!c.is_local && (
                      <p className={cn(HELP, "-mt-2 sm:col-span-2")}>요약할 때 전사본 전문이 이 주소로 전송됩니다</p>
                    )}
                    <Field label="모델">
                      <Input value={c.model} onChange={(e) => editLlm(i, { model: e.target.value })} />
                    </Field>
                    <SecretField label="API 키" {...secretProps("llm", c.name)} />
                    <Field label="컨텍스트 토큰">
                      <Input
                        type="number"
                        min={1}
                        step={1}
                        required
                        aria-invalid={!isPositiveInt(c.context_tokens)}
                        value={c.context_tokens || ""}
                        onChange={(e) => editLlm(i, { context_tokens: Number(e.target.value) })}
                      />
                    </Field>
                    <Field label="응답 시간 제한(초)">
                      <Input
                        type="number"
                        min={1}
                        step={1}
                        required
                        aria-invalid={!isPositiveInt(c.timeout_secs)}
                        value={c.timeout_secs || ""}
                        onChange={(e) => editLlm(i, { timeout_secs: Number(e.target.value) })}
                      />
                    </Field>
                    <label className="flex w-fit items-center gap-3 sm:col-span-2">
                      <Switch
                        checked={c.is_local}
                        onCheckedChange={(isLocal) => {
                          // Keep a custom timeout; swap only the defaults (300 cloud / 900 local).
                          const usesDefault = c.timeout_secs === (c.is_local ? localTimeout : cloudTimeout);
                          editLlm(i, {
                            is_local: isLocal,
                            timeout_secs: usesDefault ? (isLocal ? localTimeout : cloudTimeout) : c.timeout_secs,
                          });
                        }}
                      />
                      이 PC에서 도는 로컬 LLM (Ollama, LM Studio)
                    </label>
                    {/* 9.3: a server running a smaller context cuts the input without a word. */}
                    {c.is_local && (
                      <p className={cn(HELP, "-mt-2 sm:col-span-2")}>
                        Ollama는 OLLAMA_CONTEXT_LENGTH, LM Studio는 모델을 불러올 때 Context Length를 위
                        컨텍스트 토큰 이상으로 맞춰 주세요
                      </p>
                    )}
                    <div className="flex justify-end sm:col-span-2">
                      <RemoveButton
                        label="연결 삭제"
                        title={`${c.name} 연결을 삭제할까요?`}
                        description="저장하면 키체인의 API 키도 함께 지웁니다."
                        onConfirm={(keyboard) => remove("llm", c.name, keyboard)}
                        focusAfter={() => newLlmField.current?.focus()}
                      />
                    </div>
                  </fieldset>
                </Entry>
              ))}
              <AddRow
                field={newLlmField}
                label="새 LLM 연결 이름"
                placeholder="새 연결 이름 (예: 회사 클라우드)"
                action="연결 추가"
                value={newLlm}
                error={nameError?.kind === "llm" ? nameError.text : ""}
                locked={locked}
                moves={llmMoves}
                move={move}
                onChange={(v) => {
                  setNewLlm(v);
                  setNameError(null);
                }}
                onAdd={addLlm}
              />
            </List>
            {draft.llm_connections.length > 0 && (
              // Below the list, so it moves with the list's rows.
              <motion.fieldset
                layout="position"
                layoutDependency={llmMoves}
                transition={move}
                disabled={locked}
                className={GROUP}
              >
                <Row label="기본 LLM" help="새 회의를 이 연결로 요약합니다">
                  <Pick
                    disabled={locked}
                    value={draft.default_llm ?? NO_DEFAULT}
                    options={[
                      { value: NO_DEFAULT, label: "선택 안 함" },
                      ...draft.llm_connections.map((c) => ({ value: c.name, label: c.name })),
                    ]}
                    onChange={(v) => edit({ default_llm: v === NO_DEFAULT ? null : v })}
                  />
                </Row>
                {/* SUM-08: each new meeting's "로컬로 요약" starts here. */}
                {hasLocal && (
                  <Row label="새 회의를 기본으로 로컬에서 요약" help="회의마다 요약 전에 바꿀 수 있습니다">
                    <Switch
                      checked={draft.summarize_local_by_default}
                      onCheckedChange={(v) => edit({ summarize_local_by_default: v })}
                    />
                  </Row>
                )}
              </motion.fieldset>
            )}
          </Section>

          <Section tab="slack" open={tab}>
            <List moves={slackMoves} move={move}>
              {draft.webhooks.map((w) => {
                const key = keyOf({ kind: "slack", name: w.name });
                const status = secrets[key] ? "키체인에 저장됨" : "URL 입력 안 됨";
                const typed = secretDrafts[key]?.trim() || null;
                return (
                  <Entry
                    key={w.name}
                    label={`${w.name}, ${status}`}
                    open={expanded.slack === w.name}
                    moves={slackMoves}
                    instant={instant}
                    onToggle={(keyboard) => toggle("slack", w.name, keyboard)}
                    head={
                      <>
                        <span className="min-w-0 flex-1 truncate font-medium">{w.name}</span>
                        <span className={cn(HELP, "shrink-0")}>{status}</span>
                      </>
                    }
                  >
                    <fieldset disabled={locked} className="grid min-w-0 gap-4 px-5 pt-1 pb-5">
                      <SecretField label="웹훅 URL" {...secretProps("slack", w.name)} />
                      <div className="flex items-start justify-between gap-3">
                        {/* ONB-06: a URL typed and not saved yet is the one tried. */}
                        <RunCheck
                          action="테스트 메시지 보내기"
                          busyText="보내는 중…"
                          state={checkState(key, typed)}
                          onRun={() =>
                            run(key, typed, () =>
                              sendTestMessage(w.name, typed).then(() => ({
                                ok: true,
                                text: "보냈습니다. Slack 채널을 확인하세요",
                              })),
                            )
                          }
                        />
                        <RemoveButton
                          label="웹훅 삭제"
                          title={`${w.name} 웹훅을 삭제할까요?`}
                          description="저장하면 키체인의 웹훅 URL도 함께 지웁니다."
                          onConfirm={(keyboard) => remove("slack", w.name, keyboard)}
                          focusAfter={() => newWebhookField.current?.focus()}
                        />
                      </div>
                    </fieldset>
                  </Entry>
                );
              })}
              <AddRow
                field={newWebhookField}
                label="새 웹훅 이름"
                placeholder="채널 이름 (예: #회의록)"
                action="웹훅 추가"
                value={newWebhook}
                error={nameError?.kind === "slack" ? nameError.text : ""}
                locked={locked}
                moves={slackMoves}
                move={move}
                onChange={(v) => {
                  setNewWebhook(v);
                  setNameError(null);
                }}
                onAdd={addWebhook}
              />
            </List>
          </Section>
        </div>
      </div>

      <p role="status" className="sr-only">
        {done ? "설정을 저장했습니다" : ""}
      </p>
      {/* Typed keys never come back once dropped, so only then is it asked. */}
      <AlertDialog open={revertAsked} onOpenChange={setRevertAsked}>
        <AlertDialogContent
          onCloseAutoFocus={(e) => {
            if (!reverted.current) return;
            reverted.current = false;
            e.preventDefault();
            focusTab();
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>입력한 키도 되돌릴까요?</AlertDialogTitle>
            <AlertDialogDescription>
              저장하지 않은 API 키와 웹훅 URL을 지우고 마지막으로 저장한 설정으로 돌아갑니다.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>취소</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                reverted.current = true;
                revert(e.detail === 0);
              }}
            >
              되돌리기
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {barSlot &&
        createPortal(
          <SaveBar
            bar={bar}
            shown={present && !readOnly && (changes > 0 || done || saving !== null)}
            count={saving ?? changes}
            saving={saving !== null}
            canRevert={changes > orphans.length}
            onRevert={(keyboard) => (typedKeys ? setRevertAsked(true) : revert(keyboard))}
            onSave={save}
          />,
          barSlot,
        )}
    </Page>
  );
}

/** A tab's panel: its title and what it is for, over its groups. Hidden, not unmounted. */
function Section({ tab, open, children }: { tab: SettingsTab; open: SettingsTab; children: ReactNode }) {
  const { label, about } = TABS.find((t) => t.value === tab)!;
  return (
    <section hidden={tab !== open} className="space-y-5">
      <header className="space-y-1">
        <h2 className="text-title">{label}</h2>
        <p className="text-ink-2">{about}</p>
      </header>
      {children}
    </section>
  );
}

type Control = ReactElement<{ id?: string; "aria-describedby"?: string }>;

/** A row of a group (design-system 6.7): its label and help on the left and its control on the
 * right, or under them when `stacked`. The label names the control and the help describes it. */
function Row({
  label,
  help,
  aside,
  stacked,
  below,
  children,
}: {
  label: string;
  help?: string;
  /** At the right of the label, like the glossary's count. */
  aside?: ReactNode;
  stacked?: boolean;
  /** Under the label and control, across the row, like an audio check. */
  below?: ReactNode;
  children?: Control;
}) {
  const id = useId();
  return (
    <div className="space-y-3 px-5 py-4">
      <div className={cn("flex flex-col gap-3", !stacked && "sm:flex-row sm:items-center sm:justify-between sm:gap-8")}>
        <div className="min-w-0 flex-1 space-y-0.5">
          <div className="flex items-baseline justify-between gap-3">
            <label id={`${id}-label`} htmlFor={children ? id : undefined} className="font-medium">
              {label}
            </label>
            {aside}
          </div>
          {help && (
            <p id={`${id}-help`} className={HELP}>
              {help}
            </p>
          )}
        </div>
        {children && cloneElement(children, { id, "aria-describedby": help ? `${id}-help` : undefined })}
      </div>
      {below}
    </div>
  );
}

/** A check that changes no setting (ONB-03, ONB-06), so it never counts toward 저장. Running or
 * unavailable, its button is `aria-disabled`, not `disabled`, so the focus stays (design-system 7,
 * phase 3). What it says is one `role="status"`: what it does, what to do while it runs, then the
 * outcome, which has an icon so color is never all it has. Rust runs one audio check at a time and
 * says so if another is asked for. */
function RunCheck({
  name,
  action,
  busyText,
  state,
  blocked,
  idle,
  hint,
  running,
  onRun,
}: {
  /** The button's name, when `action` alone would be ambiguous. */
  name?: string;
  action: string;
  busyText: string;
  state: CheckState | undefined;
  /** It cannot run now: a recording (said once for the group) or another check running. */
  blocked?: boolean;
  idle?: string;
  hint?: string;
  /** Beside the button while it runs, like the mic's level. */
  running?: ReactNode;
  onRun: () => void;
}) {
  const busy = state === "running";
  const outcome = busy ? null : state;
  const off = busy || blocked;
  const Icon = outcome?.ok ? Check : CircleAlert;
  return (
    <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-2">
      <Button
        size="sm"
        variant="outline"
        // Matches the visible text while it runs (WCAG 2.5.3).
        aria-label={name && (busy ? `${name} 중` : name)}
        aria-disabled={off || undefined}
        className="aria-disabled:opacity-50"
        onClick={off ? undefined : onRun}
      >
        {busy ? busyText : action}
      </Button>
      {busy && running}
      <p role="status" className={cn(HELP, "flex gap-1.5")}>
        {busy ? (
          hint
        ) : outcome ? (
          <>
            <Icon aria-hidden className={cn("mt-px size-4 shrink-0", outcome.ok ? "text-ok-line" : "text-danger-line")} />
            {outcome.text}
          </>
        ) : (
          idle
        )}
      </p>
    </div>
  );
}

/** A select named by its row's label. */
function Pick({
  value,
  options,
  disabled,
  onChange,
  ...trigger
}: {
  value: string;
  options: { value: string; label: string }[];
  disabled?: boolean;
  onChange: (value: string) => void;
  id?: string;
  "aria-describedby"?: string;
}) {
  return (
    <Select value={value} disabled={disabled} onValueChange={onChange}>
      <SelectTrigger className={CONTROL} {...trigger}>
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
  );
}

/** A few choices side by side, as native radios: arrow keys pick, and the group's fieldset locks
 * them. The selection slides from choice to choice, but not when a key moved it. A `<label for>`
 * cannot name a group, so its row's label does by id (`Row`). */
function Segmented<T extends string>({
  id,
  value,
  options,
  onChange,
  ...described
}: {
  id?: string;
  "aria-describedby"?: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  const name = useId();
  const [instant, setInstant] = useState(false);
  return (
    <div
      id={id}
      role="radiogroup"
      aria-labelledby={`${id}-label`}
      {...described}
      className="flex w-fit rounded-full bg-sunken p-0.5"
      onPointerDown={() => setInstant(false)}
      onKeyDown={() => setInstant(true)}
    >
      {options.map((o) => (
        <label
          key={o.value}
          className="relative grid h-8 min-w-16 cursor-pointer place-items-center rounded-full px-3 text-label text-ink-2 has-checked:text-ink has-disabled:cursor-not-allowed has-disabled:opacity-50 has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-ring"
        >
          <input
            type="radio"
            name={name}
            className="sr-only"
            checked={value === o.value}
            onChange={() => onChange(o.value)}
          />
          {value === o.value && (
            <motion.span
              layoutId={`${name}-thumb`}
              transition={instant ? NOW : spring.snappy}
              style={{ borderRadius: 9999 }}
              className="absolute inset-0 bg-surface shadow-sm"
            />
          )}
          <span className="relative">{o.label}</span>
        </label>
      ))}
    </div>
  );
}

/** The connections' or webhooks' group. Its height follows the rows with `layout`, never CSS. */
function List({ moves, move, children }: { moves: string; move: Transition; children: ReactNode }) {
  return (
    <motion.div
      layout
      layoutDependency={moves}
      transition={move}
      style={{ borderRadius: 16 }}
      className="divide-y divide-line bg-surface"
    >
      {children}
    </motion.div>
  );
}

/** A connection or webhook: its summary row, opening in place to edit it. The rows below slide by
 * `layout` (design-system 5); a key opens it at once. */
function Entry({
  label,
  head,
  open,
  moves,
  instant,
  onToggle,
  children,
}: {
  /** The row's name for screen readers: its name first, then what the row shows. */
  label: string;
  head: ReactNode;
  open: boolean;
  moves: string;
  instant: boolean;
  onToggle: (keyboard: boolean) => void;
  children: ReactNode;
}) {
  const id = useId();
  return (
    // Opaque, so the rows sliding down cover the opening fields instead of showing through them.
    <motion.div
      layout="position"
      layoutDependency={moves}
      transition={instant ? NOW : spring.gentle}
      className="group/entry bg-surface first:rounded-t-float"
    >
      <button
        aria-label={label}
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        className="flex min-h-14 w-full items-center gap-3 px-5 py-3 text-left hover:bg-sunken/60 group-first/entry:rounded-t-float"
        onClick={(e) => onToggle(e.detail === 0)}
      >
        {head}
        <motion.span
          aria-hidden
          initial={false}
          animate={{ rotate: open ? 180 : 0 }}
          transition={instant ? NOW : spring.snappy}
          className="text-ink-2"
        >
          <ChevronDown className="size-4" />
        </motion.span>
      </button>
      {open && (
        <motion.div
          id={id}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={instant ? NOW : { duration: dur.small, ease: ease.out }}
        >
          {children}
        </motion.div>
      )}
    </motion.div>
  );
}

/** The last row of a list: a name and its add button. Enter adds too. */
function AddRow({
  field,
  label,
  placeholder,
  action,
  value,
  error,
  locked,
  moves,
  move,
  onChange,
  onAdd,
}: {
  field: Ref<HTMLInputElement>;
  label: string;
  placeholder: string;
  action: string;
  value: string;
  error: string;
  locked: boolean;
  moves: string;
  move: Transition;
  onChange: (value: string) => void;
  onAdd: (keyboard: boolean) => void;
}) {
  return (
    <motion.div
      layout="position"
      layoutDependency={moves}
      transition={move}
      className="rounded-b-float bg-surface first:rounded-t-float"
    >
      <fieldset disabled={locked} className="flex min-w-0 gap-2 px-5 py-3">
        <Input
          ref={field}
          aria-label={label}
          aria-invalid={!!error || undefined}
          placeholder={placeholder}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && onAdd(true)}
        />
        <Button variant="outline" onClick={(e) => onAdd(e.detail === 0)}>
          <Plus aria-hidden />
          {action}
        </Button>
      </fieldset>
      {error && (
        <p role="alert" className="flex gap-1.5 px-5 pb-3 text-label">
          <CircleAlert aria-hidden className="mt-px size-4 shrink-0 text-danger-line" />
          {error}
        </p>
      )}
    </motion.div>
  );
}

/** The one 저장 for every tab (ONB-07): a white pill in App's bottom stack, only while something is
 * unsaved (design-system 6.7), then a moment of 저장했습니다. Saving locks it without `disabled`,
 * which would drop the focus. */
function SaveBar({
  bar,
  shown,
  count,
  saving,
  canRevert,
  onRevert,
  onSave,
}: {
  bar: Ref<HTMLDivElement>;
  shown: boolean;
  count: number;
  saving: boolean;
  canRevert: boolean;
  onRevert: (keyboard: boolean) => void;
  onSave: () => void;
}) {
  const locked = saving || undefined;
  return (
    <AnimatePresence>
      {shown && (
        <motion.div
          key="save"
          ref={bar}
          layout
          initial={{ y: 24, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: 24, opacity: 0 }}
          transition={{ ...spring.dock, opacity: { duration: dur.small } }}
          // In `style`, not a class: Motion corrects the radius while the size morphs.
          style={{ borderRadius: 28 }}
          className="pointer-events-auto flex h-14 max-w-full items-center bg-surface p-2 shadow-float"
        >
          <Present className="contents">
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.div
                key={count ? "unsaved" : "saved"}
                layout="position"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: dur.small, layout: spring.dock }}
                className="flex items-center gap-1"
              >
                <Present className="contents">
                  {count ? (
                    <>
                      <span className="px-3 text-label text-ink-2 tabular-nums">{`저장 안 한 변경 ${count}개`}</span>
                      {canRevert && (
                        <Button
                          variant="ghost"
                          aria-disabled={locked}
                          className="h-10 rounded-full px-4 aria-disabled:opacity-50"
                          onClick={saving ? undefined : (e) => onRevert(e.detail === 0)}
                        >
                          되돌리기
                        </Button>
                      )}
                      <Button
                        aria-disabled={locked}
                        className="h-10 rounded-full px-5 aria-disabled:opacity-50"
                        onClick={saving ? undefined : onSave}
                      >
                        {saving ? "저장 중…" : "저장"}
                      </Button>
                    </>
                  ) : (
                    <span className="flex items-center gap-2 px-4 text-label">
                      <Check aria-hidden className="size-4 text-ok-line" />
                      저장했습니다
                    </span>
                  )}
                </Present>
              </motion.div>
            </AnimatePresence>
          </Present>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** Confirmed in a dialog like DeleteButton, so a misclick never drops an entry and its key. The
 * button leaves with its row, so focus goes to `focusAfter` instead of the page. */
function RemoveButton({
  label,
  title,
  description,
  onConfirm,
  focusAfter,
}: {
  label: string;
  title: string;
  description: string;
  onConfirm: (keyboard: boolean) => void;
  focusAfter: () => void;
}) {
  const removed = useRef(false);
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="outline" size="sm">
          <Trash2 aria-hidden />
          {label}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent
        onCloseAutoFocus={(e) => {
          if (!removed.current) return;
          e.preventDefault();
          focusAfter();
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>취소</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={(e) => {
              removed.current = true;
              onConfirm(e.detail === 0);
            }}
          >
            삭제
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** OS keychain only (ONB-08). A stored key never comes back to the UI, only whether one exists;
 * a typed value replaces it on 저장. */
function SecretField({
  label,
  saved,
  value,
  onChange,
}: {
  label: string;
  saved: boolean;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <Field label={`${label} · ${saved ? "키체인에 저장됨" : "입력 안 됨"}`}>
      <Input
        type="password"
        autoComplete="off"
        placeholder={saved ? "바꾸려면 새로 입력하고 저장" : ""}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </Field>
  );
}
