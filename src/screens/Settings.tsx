import { cloneElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode, type Ref } from "react";
import { createPortal } from "react-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, motion, useIsPresent, type Transition } from "motion/react";
import { Check, ChevronDown, CircleAlert, Download, FolderOpen, Plus, RotateCcw, Trash2, TriangleAlert } from "lucide-react";
import { Banner, downloadNote, Field, host, Page, Present } from "@/components/common";
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
  modelDownloadScope,
  modelFileFilter,
  modelStatus,
  NEWER_VERSION,
  onMicTestLevel,
  onModelDownload,
  openLogDir,
  reserved,
  resetSettings,
  saveSettings,
  setSecret,
  slackCancelSignIn,
  slackSignIn,
  slackSignOut,
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
  type ModelScope,
  type ModelState,
  type SecretKind,
  type Settings as SettingsData,
} from "@/lib/ipc";
import { dur, ease, spring } from "@/lib/motion";
import { cn, josa } from "@/lib/utils";

export type SettingsTab = "general" | "audio" | "transcription" | "llm" | "slack";

const TABS: { value: SettingsTab; label: string; about: string }[] = [
  { value: "general", label: "일반", about: "회의록에 남는 이름, 알림과 로그입니다." },
  { value: "audio", label: "오디오", about: "녹음할 마이크와 시스템 오디오를 고릅니다." },
  { value: "transcription", label: "전사", about: "녹음은 이 PC에서 글로 옮깁니다. 모델은 처음 한 번만 받습니다." },
  {
    value: "llm",
    label: "요약(LLM)",
    about: "기본은 이 PC에서 요약합니다. 다른 연결을 더할 수 있고, 클라우드 연결은 전사본을 그 주소로 보냅니다.",
  },
  {
    value: "slack",
    label: "Slack",
    about: "회사 Slack에 로그인하면 내가 들어가 있는 채널로 요약을 보냅니다. 로그인 정보는 OS 키체인에만 저장합니다.",
  },
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
// `model_status` lists the Whisper model, then the VAD model; `summary` the summary model alone.
const MODEL_FILES = ["전사 모델", "음성 구간 감지(VAD) 모델"];
const SUMMARY_FILES = ["요약 모델"];
type Scope = Exclude<ModelScope, "all">;
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

// Keychain entries are keyed `kind:name`, one per connection.
type SecretRef = { kind: SecretKind; name: string };
const secretRefs = (s: SettingsData): SecretRef[] => s.llm_connections.map((c) => ({ kind: "llm" as const, name: c.name }));
const keyOf = ({ kind, name }: SecretRef) => `${kind}:${name}`;

/** A saved connection can summarize on this PC; one under a reserved name is in no route (d6). */
const savedLocal = (s: SettingsData, rename: string[]) =>
  s.llm_connections.some((c) => c.is_local && !rename.includes(c.name));

/** The settings as 저장 writes them: the name trimmed. Rust turns the local default off while nothing
 * local can summarize (SUM-08) and answers with what it stored. */
const asSaved = (s: SettingsData): SettingsData => ({ ...s, recorder_name: s.recorder_name.trim() });

// What the save bar counts: each field as 저장 would write it, and each connection added, removed
// or edited once however many of its fields changed.
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

// A check's outcome (ONB-03), in words the recorder can act on.
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
  // Why a name cannot be added, under the row that adds it.
  const [nameError, setNameError] = useState<{ kind: SecretKind; text: string } | null>(null);
  // The connection open for editing.
  const [expanded, setExpanded] = useState<Record<SecretKind, string | null>>({ llm: null });
  // A key picked the tab or opened the row: nothing moves (design-system 5).
  const [instant, setInstant] = useState(false);
  const [devices, setDevices] = useState<AudioDevices | null>(null);
  const [deviceError, setDeviceError] = useState("");
  const [error, setError] = useState("");
  // Each time 저장 reports an error, even the same one again: its banner is said and shown anew.
  const [reported, setReported] = useState(0);
  const [models, setModels] = useState<ModelState | null>(null);
  // The 요약(LLM) tab's summary model (plan 5.8), read when that tab opens: null until then.
  const [summary, setSummary] = useState<ModelState | null>(null);
  const summaryRead = useRef(false);
  const [received, setReceived] = useState<Record<string, ModelDownload>>({});
  // The file of the last `model://download`: the one in flight.
  const [latest, setLatest] = useState("");
  const [modelBusy, setModelBusy] = useState<"download" | "import" | null>(null);
  // Both shown in the 전사 tab, not the shared banner. A status read that works clears its own
  // error but never a download's or import's, which lasts until the next try.
  const [statusError, setStatusError] = useState("");
  // A failure stays on the tab whose files the download was started for (ipc.ts keeps the scope).
  const [modelError, setModelError] = useState(() =>
    modelDownloadScope() === "summary" ? "" : modelDownloadFailure(),
  );
  // The same two for the summary model, shown in the 요약(LLM) tab.
  const [summaryStatusError, setSummaryStatusError] = useState("");
  const [summaryError, setSummaryError] = useState(() =>
    modelDownloadScope() === "summary" ? modelDownloadFailure() : "",
  );
  // Saved connections under a name this PC's route reserves (d6): listed, to be renamed.
  const [rename, setRename] = useState<string[]>([]);
  // The audio checks, by "mic" or "system". Never part of the draft. `on`: the device it ran with;
  // no `outcome` while it runs.
  const [checks, setChecks] = useState<Record<string, { on: string | null; outcome?: Outcome }>>({});
  const micLevel = useRef<HTMLDivElement>(null);
  const present = useIsPresent();
  const nav = useRef<HTMLElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const banner = useRef<HTMLDivElement>(null);
  const newLlmField = useRef<HTMLInputElement>(null);
  // The bar the revert dialog came from is gone once it reverts.
  const reverted = useRef(false);

  const show = async ({ settings, read_only, rename }: LoadedSettings) => {
    setDraft(settings);
    setPersisted(settings);
    setReadOnly(read_only);
    setRename(rename ?? []);
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

  const readSummary = () => {
    summaryRead.current = true;
    return modelStatus("summary").then(
      (state) => {
        setSummary(state);
        setSummaryStatusError("");
      },
      (e) => setSummaryStatusError(String(e)),
    );
  };
  const errorOf = (scope: Scope) => (scope === "summary" ? setSummaryError : setModelError);

  // The download lives in ipc.ts, so this also picks up one started before the screen opened. Its
  // failure goes to the tab of its scope: the summary model's, else the 전사 tab (Home's `all`).
  const follow = (running: Promise<void>) => {
    setModelBusy("download");
    running.then(() => {
      setModelBusy(null);
      errorOf(modelDownloadScope() === "summary" ? "summary" : "transcription")(modelDownloadFailure());
      readModels();
      if (summaryRead.current) readSummary();
    });
  };

  useEffect(() => {
    getSettings()
      .then(show)
      .catch((e) => setError(String(e)));
    // A file sending progress is in the download's chain, whatever the last status read said.
    const fetching = (d: ModelDownload) => (state: ModelState | null) =>
      state?.files.some((f) => f.file === d.file && !f.downloading)
        ? { ...state, files: state.files.map((f) => (f.file === d.file ? { ...f, downloading: true } : f)) }
        : state;
    const unlisten = onModelDownload((d) => {
      setReceived((all) => ({ ...all, [d.file]: d }));
      setLatest(d.file);
      setModels(fetching(d));
      setSummary(fetching(d));
    });
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
    if (tab === "llm") readSummary();
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
            <LogFolder />
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

  // A new entry opens, ready for its address and key. It never becomes the default (plan 5.4):
  // no default is this PC, and a cloud connection added must not take its place.
  const addLlm = (keyboard: boolean) => {
    const name = newLlm.trim();
    // d6: Rust's words, as 저장 would refuse it.
    const problem = reserved(name)
      ? `'${name}'은(는) 이 PC 요약이 쓰는 이름이라 연결 이름으로 쓸 수 없습니다. 이름을 바꿔 주세요`
      : nameProblem(name, draft.llm_connections.map((c) => c.name), keyHolders("llm", persisted.llm_connections));
    if (problem) return setNameError({ kind: "llm", text: problem });
    edit({
      llm_connections: [
        ...draft.llm_connections,
        { name, base_url: "", model: "", context_tokens: 8192, is_local: false, timeout_secs: cloudTimeout },
      ],
    });
    setNewLlm("");
    setNameError(null);
    setInstant(keyboard);
    setExpanded((x) => ({ ...x, llm: name }));
  };

  // Only the draft changes; the keychain entry goes once the settings are saved.
  const remove = (kind: SecretKind, name: string, keyboard: boolean) => {
    setInstant(keyboard);
    setSecretDrafts((d) => ({ ...d, [keyOf({ kind, name })]: "" }));
    edit({
      llm_connections: draft.llm_connections.filter((c) => c.name !== name),
      default_llm: draft.default_llm === name ? null : draft.default_llm,
    });
  };

  const glossaryCount = terms(glossary).length;
  // The summary model is in (plan 5.4), as the 요약(LLM) tab read it.
  const builtinReady = !!summary?.files[0]?.installed;
  // The connections a default may name: not one under a reserved name (d6).
  const usable = draft.llm_connections.filter((c) => !rename.includes(c.name));
  // No default, or one gone: this PC summarizes (`Settings::default_connection`).
  const defaultConnection = usable.find((c) => c.name === draft.default_llm);
  const builtinDefault = !defaultConnection;
  // SUM-08: a new meeting's "로컬로 요약" changes its route only with a cloud default and something
  // local to use, or with this PC as the default while its model is missing and a saved local
  // connection can stand in (plan 5.4, as `toggle_matters`).
  const localDefaultMatters = defaultConnection
    ? !defaultConnection.is_local && (builtinReady || savedLocal(draft, rename))
    : !builtinReady && savedLocal(draft, rename);

  // Every typed key counts, even blanks (저장 clears them), and so does a key the keychain still
  // has to delete: only 저장 finishes either.
  const now = asSaved(draft);
  const then = asSaved(persisted);
  const changes =
    FIELDS.filter((k) => now[k] !== then[k]).length +
    Number(terms(glossary).join("\n") !== persisted.glossary.join("\n")) +
    entryChanges(draft.llm_connections, persisted.llm_connections) +
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
      let stored: SettingsData;
      try {
        stored = (await saveSettings(value)) ?? value;
      } catch (e) {
        // settings.json still lists the removed entries and not the new ones, so the keychain
        // stays as it is and the typed keys stay in the draft.
        return report(String(e));
      }
      // As Rust stored it: its own fields and the local default it may have turned off.
      setDraft(stored);
      setPersisted(stored);
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
  // The summary model needs only the address.
  const addressUnsaved = draft.model_base_url !== persisted.model_base_url;
  const unsaved = draft.model !== persisted.model || addressUnsaved;
  // What the engine will use (`queue::first_device`): the saved backend, automatic on the CPU once
  // the summary engine's GPU failed.
  const device = `${
    persisted.backend === "auto" && persisted.summary_gpu_failed
      ? "CPU (GPU에서 멈춘 적이 있어 CPU로 요약합니다)"
      : (BACKENDS.find((b) => b.value === persisted.backend)?.label ?? persisted.backend)
  } · 전사 탭에서 바꾸기`;

  const startDownload = (scope: Scope) => {
    setReceived({});
    errorOf(scope)("");
    follow(startModelDownload(scope));
    // Which of the tab's files the download fetches, for 취소.
    (scope === "summary" ? readSummary : readModels)();
  };

  // ONB-02: a model file got some other way (a colleague, a USB stick).
  const importFile = (scope: Scope) =>
    open({ filters: [modelFileFilter(scope)] })
      .then((path) => {
        if (!path) return;
        setModelBusy("import");
        return importModel(path, scope)
          .then(scope === "summary" ? setSummary : setModels)
          .finally(() => setModelBusy(null));
      })
      .then(
        () => errorOf(scope)(""),
        (e) => errorOf(scope)(String(e)),
      );

  const cancel = (scope: Scope) =>
    cancelModelDownload(scope).then(
      (done) => {
        // Nothing of the tab's was left: the list was stale.
        if (!done) (scope === "summary" ? readSummary : readModels)();
      },
      (e) => errorOf(scope)(String(e)),
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
  // Signing in and out are not settings: both copies take them, so 저장 counts nothing.
  const signedIn = (team: string | null) => {
    const signed = (s: SettingsData | null) =>
      s && { ...s, slack_team: team, last_channel: team === s.slack_team ? s.last_channel : null };
    setDraft(signed);
    setPersisted(signed);
  };

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
            <div className={cn(GROUP, "px-5 py-4")}>
              <LogFolder />
            </div>
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
              <ModelFiles
                labels={MODEL_FILES}
                models={models}
                received={received}
                latest={latest}
                busy={modelBusy}
                blocked={unsaved}
                unreadable={!!statusError}
                onDownload={() => startDownload("transcription")}
                onCancel={() => cancel("transcription")}
                onImport={() => importFile("transcription")}
              >
                {unsaved && <p className={HELP}>바꾼 모델과 주소는 저장한 뒤에 받거나 가져올 수 있습니다</p>}
              </ModelFiles>
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
            {summaryStatusError && <Banner kind="error">{summaryStatusError}</Banner>}
            {summaryError && <Banner kind="error">{summaryError}</Banner>}
            {/* Plan 5.8: the built-in route first, as the default it usually is. */}
            <div className="space-y-2">
              <h3 className="flex items-baseline gap-2 px-5 font-medium">
                이 PC에서 요약{" "}
                {builtinDefault && <span className="text-label text-brand">기본</span>}
              </h3>
              <fieldset disabled={locked} className={GROUP}>
                <ModelFiles
                  labels={SUMMARY_FILES}
                  models={summary}
                  received={received}
                  latest={latest}
                  busy={modelBusy}
                  blocked={addressUnsaved}
                  unreadable={!!summaryStatusError}
                  onDownload={() => startDownload("summary")}
                  onCancel={() => cancel("summary")}
                  onImport={() => importFile("summary")}
                >
                  {addressUnsaved && (
                    <p className={HELP}>바꾼 다운로드 주소는 저장한 뒤에 받거나 가져올 수 있습니다</p>
                  )}
                </ModelFiles>
                {/* One device setting for both (plan 5.3): read here, changed in the 전사 tab. */}
                <Row label="처리 장치" help="전사 탭의 전사 백엔드를 함께 씁니다">
                  {/* Named by what it shows, not by the row's label (WCAG 2.5.3). */}
                  <Button
                    variant="link"
                    size="sm"
                    className="px-0"
                    aria-label={device}
                    onClick={(e) => pick("transcription", e.detail === 0)}
                  >
                    {device}
                  </Button>
                </Row>
              </fieldset>
              {/* D7: q8_0 KV cache, at most about 5.1 GB. */}
              <p className={cn(HELP, "px-5")}>요약하는 동안 메모리를 약 5GB 씁니다</p>
            </div>

            <div className="space-y-2">
              <h3 className="px-5 font-medium">다른 연결</h3>
              <List moves={llmMoves} move={move}>
                {draft.llm_connections.map((c, i) => (
                  <Entry
                    key={c.name}
                    label={[
                      c.name,
                      rename.includes(c.name) && "이름을 바꿔 주세요",
                      c.is_local ? "로컬" : "클라우드",
                      host(c.base_url),
                      !builtinDefault && draft.default_llm === c.name && "기본 LLM",
                    ]
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
                            {!builtinDefault && draft.default_llm === c.name && (
                              <span className="shrink-0 text-label text-brand">기본</span>
                            )}
                          </span>
                          {rename.includes(c.name) ? (
                            <span className={cn(HELP, "flex items-center gap-1.5")}>
                              <TriangleAlert aria-hidden className="size-4 shrink-0 text-warn-line" />
                              이름을 바꿔 주세요
                            </span>
                          ) : (
                            <span className={cn(HELP, "block truncate")}>{c.base_url ? host(c.base_url) : "주소 없음"}</span>
                          )}
                        </span>
                        {/* In words, not a color (NFR-09). */}
                        <span className="shrink-0 rounded-full bg-sunken px-2 py-0.5 text-caption text-ink-2">
                          {c.is_local ? "로컬" : "클라우드"}
                        </span>
                      </>
                    }
                  >
                    <fieldset disabled={locked} className="grid min-w-0 gap-4 px-5 pt-1 pb-5 sm:grid-cols-2">
                      {/* d6: no migration. A name keys its keychain entry, so it is renamed by adding
                          it again; until then no route uses it and 저장 is refused. */}
                      {rename.includes(c.name) && (
                        <p className={cn(HELP, "sm:col-span-2")}>
                          {`'${c.name}'${josa(c.name, "은", "는")} 이 PC 요약이 쓰는 이름이라 이 연결로는 요약하지 않고, 이 연결이 있는 동안 설정을 저장할 수 없습니다. 삭제한 뒤 다른 이름으로 다시 추가해 주세요`}
                        </p>
                      )}
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
            </div>
            {/* Below the list, so it moves with the list's rows. Always there: this PC is a choice. */}
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
                  value={builtinDefault ? NO_DEFAULT : draft.default_llm!}
                  options={[
                    { value: NO_DEFAULT, label: "이 PC" },
                    ...usable.map((c) => ({ value: c.name, label: c.name })),
                  ]}
                  onChange={(v) => edit({ default_llm: v === NO_DEFAULT ? null : v })}
                />
              </Row>
              {/* SUM-08: each new meeting's "로컬로 요약" starts here, offered where it changes a route. */}
              {localDefaultMatters && (
                <Row label="새 회의를 기본으로 로컬에서 요약" help="회의마다 요약 전에 바꿀 수 있습니다">
                  <Switch
                    checked={draft.summarize_local_by_default}
                    onCheckedChange={(v) => edit({ summarize_local_by_default: v })}
                  />
                </Row>
              )}
            </motion.fieldset>
          </Section>

          <Section tab="slack" open={tab}>
            <div className={GROUP}>
              <SlackAccount team={draft.slack_team} locked={readOnly} onTeam={signedIn} />
            </div>
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
              저장하지 않은 API 키를 지우고 마지막으로 저장한 설정으로 돌아갑니다.
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

/** A tab's model files (ONB-01, ONB-02): each one's state and its bar while it downloads, then
 * 다운로드, 취소 and 직접 가져오기. The 전사 tab's and the summary model's (plan 5.8). */
function ModelFiles({
  labels,
  models,
  received,
  latest,
  busy,
  blocked,
  unreadable,
  onDownload,
  onCancel,
  onImport,
  children,
}: {
  /** Each file's name, in `model_status`'s order. */
  labels: string[];
  models: ModelState | null;
  /** The last `model://download` of each file. */
  received: Record<string, ModelDownload>;
  /** The file in flight: the only row with a speed and a time (plan 5.2-3). */
  latest: string;
  /** This screen's download or import: the download itself is the app's one (ipc.ts). */
  busy: "download" | "import" | null;
  /** An unsaved change it depends on: download and import wait for 저장. */
  blocked: boolean;
  /** The status could not be read: import stays offered (ONB-02). */
  unreadable: boolean;
  onDownload: () => void;
  onCancel: () => void;
  onImport: () => void;
  /** Under the buttons: why they wait. */
  children?: ReactNode;
}) {
  const downloading = busy === "download" || !!models?.downloading;
  // While it runs, only 취소 and the progress show.
  const idle = !busy && !models?.downloading && !blocked;
  const missing = !!models?.files.some((f) => !f.installed);
  // Only while the download still has one of these files to fetch: one for another tab's files, or
  // started on Home and past these, has nothing here to cancel.
  const cancellable = downloading && !!models?.files.some((f) => f.downloading);
  const fileState = (f: ModelFileState) => {
    const d = received[f.file];
    if (f.installed) return "설치됨";
    if (busy === "download" && d) {
      const got = `받는 중 ${Math.floor((d.received / d.total) * 100)}% (${mb(d.received)}/${mb(d.total)}MB)`;
      // A file behind keeps its last event as it was.
      if (f.file !== latest) return got;
      const { stage, time } = downloadNote(d, d.total - d.received);
      // All in: only the hash check is left.
      if (d.received >= d.total) return stage;
      return [got, d.bps && `${(d.bps / 1e6).toFixed(1)}MB/s`, stage || (time && `이 파일 ${time}`)]
        .filter(Boolean)
        .join(" · ");
    }
    return f.received ? `없음 · ${mb(f.received)}/${mb(f.size)}MB 받아 둠` : "없음";
  };
  // The stage of this tab's file in flight, never its speed or time.
  const flying = models?.files.findIndex((f) => f.file === latest && !f.installed) ?? -1;
  const stage =
    flying >= 0 && received[latest] ? downloadNote(received[latest], 0).stage || `${labels[flying]} 받는 중` : "";
  return (
    <div className="space-y-4 px-5 py-4">
      {models && (
        <ul className="space-y-3">
          {models.files.map((f, i) => {
            const d = received[f.file];
            return (
              <li key={f.file} className="space-y-2">
                <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                  <span>
                    {labels[i]} <span className="text-caption text-ink-2">{f.file}</span>
                  </span>
                  <span className="flex items-center gap-1.5 text-label tabular-nums">
                    {f.installed && <Check aria-hidden className="size-4 text-ok-line" />}
                    {fileState(f)}
                  </span>
                </div>
                {busy === "download" && d && !f.installed && (
                  <ProgressBar aria-label={`${labels[i]} 받는 중`} value={Math.floor((d.received / d.total) * 100)} />
                )}
              </li>
            );
          })}
        </ul>
      )}
      {busy === "download" && (
        <p role="status" className="sr-only">
          {stage}
        </p>
      )}
      {(missing || downloading || busy || unreadable) && (
        <div className="flex flex-wrap gap-2">
          {missing && !downloading && (
            <Button size="sm" disabled={!idle} onClick={onDownload}>
              <Download aria-hidden />
              다운로드
            </Button>
          )}
          {cancellable && (
            <Button size="sm" variant="outline" onClick={onCancel}>
              취소
            </Button>
          )}
          {!downloading && (
            <Button size="sm" variant="ghost" disabled={!idle} onClick={onImport}>
              <FolderOpen aria-hidden />
              {busy === "import" ? "가져오는 중" : "직접 가져오기"}
            </Button>
          )}
        </div>
      )}
      {children}
    </div>
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

/** ONB-04: the Slack sign-in, in the browser (OAuth with PKCE, slack.rs). Not a setting: signing in
 * and out take effect at once and never count toward 저장. A sign-in waiting for the browser can be
 * cancelled; a new one cancels it too. */
function SlackAccount({
  team,
  locked,
  onTeam,
}: {
  team: string | null;
  /** Settings from a newer app (STO-05): the workspace could not be written. */
  locked: boolean;
  onTeam: (team: string | null) => void;
}) {
  const help = useId();
  const [busy, setBusy] = useState<"in" | "out" | null>(null);
  const [error, setError] = useState("");
  const run = (which: "in" | "out", request: Promise<void>) => {
    setError("");
    setBusy(which);
    request.catch((e) => setError(String(e))).finally(() => setBusy(null));
  };
  return (
    <div className="space-y-3 px-5 py-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-8">
        <div className="min-w-0 flex-1 space-y-0.5">
          <p className="font-medium">{team ?? "Slack 계정"}</p>
          <p id={help} className={HELP}>
            {busy === "in"
              ? "브라우저에서 Slack 로그인을 마쳐 주세요"
              : team
                ? "로그인했습니다. 요약은 내 이름으로, 내가 들어가 있는 채널에 올라갑니다"
                : "로그인하지 않았습니다"}
          </p>
        </div>
        {busy === "in" ? (
          <Button size="sm" variant="outline" aria-describedby={help} onClick={() => slackCancelSignIn().catch(() => {})}>
            취소
          </Button>
        ) : team ? (
          <Button
            size="sm"
            variant="outline"
            disabled={locked || busy === "out"}
            aria-describedby={help}
            onClick={() => run("out", slackSignOut().then(() => onTeam(null)))}
          >
            {busy === "out" ? "로그아웃하는 중…" : "로그아웃"}
          </Button>
        ) : (
          <Button
            size="sm"
            variant="outline"
            disabled={locked}
            aria-describedby={help}
            onClick={() => run("in", slackSignIn().then((signed) => void (signed && onTeam(signed))))}
          >
            <SlackMark />
            Slack으로 로그인
          </Button>
        )}
      </div>
      {error && <Banner kind="error">{error}</Banner>}
    </div>
  );
}

/** Slack's mark, in its own colors, on the sign-in button. */
function SlackMark() {
  return (
    <svg aria-hidden viewBox="0 0 54 54" className="size-4">
      <path
        fill="#36C5F0"
        d="M19.712.133a5.381 5.381 0 0 0-5.376 5.387 5.381 5.381 0 0 0 5.376 5.386h5.376V5.52A5.381 5.381 0 0 0 19.712.133m0 14.365H5.376A5.381 5.381 0 0 0 0 19.884a5.381 5.381 0 0 0 5.376 5.387h14.336a5.381 5.381 0 0 0 5.376-5.387 5.381 5.381 0 0 0-5.376-5.386"
      />
      <path
        fill="#2EB67D"
        d="M53.76 19.884a5.381 5.381 0 0 0-5.376-5.386 5.381 5.381 0 0 0-5.376 5.386v5.387h5.376a5.381 5.381 0 0 0 5.376-5.387m-14.336 0V5.52A5.381 5.381 0 0 0 34.048.133a5.381 5.381 0 0 0-5.376 5.387v14.364a5.381 5.381 0 0 0 5.376 5.387 5.381 5.381 0 0 0 5.376-5.387"
      />
      <path
        fill="#ECB22E"
        d="M34.048 54a5.381 5.381 0 0 0 5.376-5.387 5.381 5.381 0 0 0-5.376-5.386h-5.376v5.386A5.381 5.381 0 0 0 34.048 54m0-14.365h14.336a5.381 5.381 0 0 0 5.376-5.386 5.381 5.381 0 0 0-5.376-5.387H34.048a5.381 5.381 0 0 0-5.376 5.387 5.381 5.381 0 0 0 5.376 5.386"
      />
      <path
        fill="#E01E5A"
        d="M0 34.249a5.381 5.381 0 0 0 5.376 5.386 5.381 5.381 0 0 0 5.376-5.386v-5.387H5.376A5.381 5.381 0 0 0 0 34.25m14.336-.001v14.364A5.381 5.381 0 0 0 19.712 54a5.381 5.381 0 0 0 5.376-5.387V34.25a5.381 5.381 0 0 0-5.376-5.387 5.381 5.381 0 0 0-5.376 5.387"
      />
    </svg>
  );
}

/** DX1 "로그 폴더 열기". Not a setting: it works while read-only or saving and never counts
 * toward 저장, and it is on the unreadable-settings screen too, where logs matter most. A failure
 * stays beside it until a later open works. */
function LogFolder() {
  const help = useId();
  const [error, setError] = useState("");
  return (
    <div className="space-y-3">
      <div className="space-y-0.5">
        <p className="font-medium">로그</p>
        <p id={help} className={HELP}>
          문제가 생기면 이 폴더의 app.log를 보내 주세요
        </p>
      </div>
      <Button
        size="sm"
        variant="outline"
        aria-describedby={help}
        onClick={() => openLogDir().then(() => setError(""), (e) => setError(String(e)))}
      >
        <FolderOpen aria-hidden />
        로그 폴더 열기
      </Button>
      {error && <Banner kind="error">{error}</Banner>}
    </div>
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

/** A check that changes no setting (ONB-03), so it never counts toward 저장. Running or
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

/** The connections' group. Its height follows the rows with `layout`, never CSS. */
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

/** A connection: its summary row, opening in place to edit it. The rows below slide by
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
