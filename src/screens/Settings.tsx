import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Download, FolderOpen, Plus, RotateCcw, Trash2 } from "lucide-react";
import { Banner, Field, Page, SelectField } from "@/components/common";
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
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
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
  modelStatus,
  NEWER_VERSION,
  onModelDownload,
  resetSettings,
  saveSettings,
  setSecret,
  startModelDownload,
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

export type SettingsTab = "general" | "audio" | "transcription" | "llm" | "slack";

const TABS: { value: SettingsTab; label: string }[] = [
  { value: "general", label: "일반" },
  { value: "audio", label: "오디오" },
  { value: "transcription", label: "전사" },
  { value: "llm", label: "요약(LLM)" },
  { value: "slack", label: "Slack" },
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

// Keychain entries are keyed `kind:name`, one per connection and webhook.
type SecretRef = { kind: SecretKind; name: string };
const secretRefs = (s: SettingsData): SecretRef[] => [
  ...s.llm_connections.map((c) => ({ kind: "llm" as const, name: c.name })),
  ...s.webhooks.map((w) => ({ kind: "slack" as const, name: w.name })),
];
const keyOf = ({ kind, name }: SecretRef) => `${kind}:${name}`;

// One 저장 for every tab, keys included: the panels stay mounted while hidden and typed keys live
// in this component's state, so switching tabs never drops an edit.
export default function Settings({
  tab: initialTab = "general",
  onHome,
}: {
  tab?: SettingsTab;
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
  const [saving, setSaving] = useState(false);
  const [newLlm, setNewLlm] = useState("");
  const [newWebhook, setNewWebhook] = useState("");
  const [devices, setDevices] = useState<AudioDevices | null>(null);
  const [deviceError, setDeviceError] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [models, setModels] = useState<ModelState | null>(null);
  const [received, setReceived] = useState<Record<string, ModelDownload>>({});
  const [modelBusy, setModelBusy] = useState<"download" | "import" | null>(null);
  // Both shown in the 전사 tab, not the shared banner. A status read that works clears its own
  // error but never a download's or import's, which lasts until the next try.
  const [statusError, setStatusError] = useState("");
  const [modelError, setModelError] = useState(modelDownloadFailure);

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

  const readModels = () =>
    modelStatus().then(
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
    const running = modelDownload();
    if (running) follow(running);
    return () => {
      unlisten.then((stop) => stop(), () => {});
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

  const edit = (patch: Partial<SettingsData>) => {
    setDraft({ ...draft, ...patch });
    setNotice("");
  };
  const editLlm = (index: number, patch: Partial<LlmConnection>) =>
    edit({
      llm_connections: draft.llm_connections.map((c, i) => (i === index ? { ...c, ...patch } : c)),
    });

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

  const addLlm = () => {
    const name = newLlm.trim();
    const problem = nameProblem(
      name,
      draft.llm_connections.map((c) => c.name),
      keyHolders("llm", persisted.llm_connections),
    );
    if (problem) return setError(problem);
    edit({
      llm_connections: [
        ...draft.llm_connections,
        { name, base_url: "", model: "", context_tokens: 8192, is_local: false, timeout_secs: cloudTimeout },
      ],
      default_llm: draft.default_llm ?? name,
    });
    setNewLlm("");
    setError("");
  };

  const addWebhook = () => {
    const name = newWebhook.trim();
    const problem = nameProblem(
      name,
      draft.webhooks.map((w) => w.name),
      keyHolders("slack", persisted.webhooks),
    );
    if (problem) return setError(problem);
    edit({ webhooks: [...draft.webhooks, { name }] });
    setNewWebhook("");
    setError("");
  };

  // Only the draft changes; the keychain entry goes once the settings are saved.
  const remove = (kind: SecretKind, name: string) => {
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

  const glossaryCount = glossary.split("\n").filter((t) => t.trim()).length;
  const hasLocal = draft.llm_connections.some((c) => c.is_local);

  const save = async () => {
    // Each check opens the tab that holds the field.
    if (glossaryCount > 100) {
      setTab("transcription");
      return setError("용어집은 100개까지 넣을 수 있습니다");
    }
    if (!draft.model_base_url.trim()) {
      setTab("transcription");
      return setError("모델 다운로드 주소를 입력하세요");
    }
    const invalid = draft.llm_connections.find(
      (c) => !isPositiveInt(c.context_tokens) || !isPositiveInt(c.timeout_secs),
    );
    if (invalid) {
      setTab("llm");
      return setError(`컨텍스트 토큰과 응답 시간 제한은 1 이상의 정수여야 합니다: ${invalid.name}`);
    }
    const value = {
      ...draft,
      // Same rule as settings.rs: only switching the backend to GPU clears it.
      gpu_failed: draft.gpu_failed && !(draft.backend === "gpu" && persisted.backend !== "gpu"),
      recorder_name: draft.recorder_name.trim(),
      // Its switch hides without a local connection, so it cannot stay on unseen.
      summarize_local_by_default: draft.summarize_local_by_default && hasLocal,
      glossary: glossary
        .split("\n")
        .map((term) => term.trim())
        .filter(Boolean),
    };
    const kept = secretRefs(value);
    const keptKeys = new Set(kept.map(keyOf));
    const removed = [...secretRefs(persisted), ...orphans].filter((r) => !keptKeys.has(keyOf(r)));
    // The form is disabled until this ends, so a second 저장 cannot interleave with it.
    setSaving(true);
    try {
      try {
        await saveSettings(value);
      } catch (e) {
        // settings.json still lists the removed entries and not the new ones, so the keychain
        // stays as it is and the typed keys stay in the draft.
        return setError(String(e));
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
      setError(problems.length ? `설정은 저장했지만 ${problems.join(" / ")}` : "");
      setNotice(problems.length ? "" : "저장했습니다");
    } finally {
      setSaving(false);
    }
  };

  const secretProps = (kind: SecretKind, name: string) => {
    const key = keyOf({ kind, name });
    return {
      saved: !!secrets[key],
      value: secretDrafts[key] ?? "",
      onChange: (typed: string) => {
        setSecretDrafts((d) => ({ ...d, [key]: typed }));
        setNotice("");
      },
    };
  };

  // Download and import act on the saved model and address: `model_status` and the backend read
  // settings.json. So an unsaved change blocks both until 저장, rather than acting on the old one.
  const unsaved =
    draft.model !== persisted.model || draft.model_base_url !== persisted.model_base_url;
  const idle = !modelBusy && !models?.downloading && !unsaved;
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
    follow(startModelDownload());
  };

  // ONB-02: a model file got some other way (a colleague, a USB stick).
  const importFile = () =>
    open({ filters: [{ name: "모델 파일", extensions: ["bin"] }] })
      .then((path) => {
        if (!path) return;
        setModelBusy("import");
        return importModel(path)
          .then(setModels)
          .finally(() => setModelBusy(null));
      })
      .then(
        () => setModelError(""),
        (e) => setModelError(String(e)),
      );

  // A pinned device missing from the list still shows, so the select is never blank. Only a
  // list that loaded can tell it is unplugged.
  const deviceOptions = (list: AudioDevice[] | undefined, pinned: string | null) => [
    { value: NO_DEFAULT, label: "시스템 기본값" },
    ...(list ?? []).map((d) => ({ value: d.id, label: d.name })),
    ...(pinned && !list?.some((d) => d.id === pinned)
      ? [{ value: pinned, label: list ? "설정한 장치 (지금 연결되지 않음)" : "설정한 장치" }]
      : []),
  ];

  return (
    <Page title="설정" onHome={onHome} width="wide">
      <div className="flex flex-col gap-6 md:flex-row md:items-start">
        <nav aria-label="설정 메뉴" className="flex flex-wrap gap-1 md:w-48 md:shrink-0 md:flex-col">
          {TABS.map((t) => (
            <button
              key={t.value}
              aria-current={tab === t.value ? "page" : undefined}
              className="rounded-lg px-3 py-2 text-left text-sm font-medium hover:bg-accent aria-[current=page]:bg-accent-tint aria-[current=page]:font-bold"
              onClick={() => setTab(t.value)}
            >
              {t.label}
            </button>
          ))}
        </nav>

        <div className="min-w-0 flex-1 space-y-6">
          {readOnly && <Banner kind="warning">{NEWER_VERSION}. 설정을 바꿀 수 없습니다.</Banner>}
          {error && <Banner kind="error">{error}</Banner>}

          <fieldset disabled={readOnly || saving} className="space-y-6">
            <section hidden={tab !== "general"} className="space-y-4">
              <h2 className="sr-only">일반</h2>
              <Field label="녹음자 이름 (Slack 메시지에 표시됩니다)">
                <Input
                  value={draft.recorder_name}
                  onChange={(e) => edit({ recorder_name: e.target.value })}
                />
              </Field>
              {/* ONB-03: desktop notification permission cannot be read, so this is all we can say. */}
              <p className="text-sm text-muted-foreground">
                알림이 보이지 않으면 OS 알림 설정에서 '회의 요약'을 허용해 주세요
              </p>
            </section>

            <section hidden={tab !== "audio"} className="space-y-4">
              <h2 className="sr-only">오디오</h2>
              {deviceError && <Banner kind="error">{deviceError}</Banner>}
              <SelectField
                label="마이크"
                value={draft.input_device ?? NO_DEFAULT}
                options={deviceOptions(devices?.inputs, draft.input_device)}
                onChange={(v) => edit({ input_device: v === NO_DEFAULT ? null : v })}
              />
              {pinsOutput ? (
                <SelectField
                  label="시스템 오디오 (출력 장치)"
                  value={draft.output_device ?? NO_DEFAULT}
                  options={deviceOptions(devices?.outputs, draft.output_device)}
                  onChange={(v) => edit({ output_device: v === NO_DEFAULT ? null : v })}
                />
              ) : (
                <p className="text-sm text-muted-foreground">
                  시스템 오디오는 모든 출력 장치의 소리를 함께 녹음합니다.
                </p>
              )}
              {/* REC-10. The system audio test comes with M6 (ONB-03). */}
              <p className="text-sm text-muted-foreground">
                헤드폰을 쓰면 상대방 목소리가 마이크로 다시 들어가는 에코가 줄어듭니다.
              </p>
            </section>

            <section hidden={tab !== "transcription"} className="space-y-4">
              <h2 className="sr-only">전사</h2>
              {statusError && <Banner kind="error">{statusError}</Banner>}
              {modelError && <Banner kind="error">{modelError}</Banner>}
              {/* 5.4: only automatic mode falls back; choosing GPU and saving clears it. */}
              {persisted.gpu_failed && persisted.backend === "auto" && (
                <Banner kind="warning">
                  GPU 전사에 실패해 CPU로 처리하고 있습니다. 전사 백엔드를 GPU로 골라 저장하면 다시
                  GPU로 처리합니다.
                </Banner>
              )}
              <SelectField
                label="모델"
                value={draft.model}
                // A hand-set model (like `tiny`) still shows, so the select is never blank.
                options={
                  MODELS.some((m) => m.value === draft.model)
                    ? MODELS
                    : [...MODELS, { value: draft.model, label: draft.model }]
                }
                disabled={!!modelBusy}
                onChange={(model) => edit({ model })}
              />
              <div className="space-y-3 rounded-lg border p-4">
                {models && (
                  <ul className="space-y-1 text-sm">
                    {models.files.map((f, i) => (
                      <li key={f.file} className="flex flex-wrap justify-between gap-x-4">
                        <span>
                          {MODEL_FILES[i]} <span className="text-muted-foreground">{f.file}</span>
                        </span>
                        <span>{fileState(f)}</span>
                      </li>
                    ))}
                  </ul>
                )}
                <div className="flex flex-wrap gap-2">
                  <Button
                    disabled={!idle || !models?.files.some((f) => !f.installed)}
                    onClick={startDownload}
                  >
                    <Download aria-hidden />
                    다운로드
                  </Button>
                  <Button
                    variant="outline"
                    disabled={modelBusy !== "download" && !models?.downloading}
                    onClick={() => cancelModelDownload().catch((e) => setModelError(String(e)))}
                  >
                    취소
                  </Button>
                  <Button variant="outline" disabled={!idle} onClick={importFile}>
                    <FolderOpen aria-hidden />
                    {modelBusy === "import" ? "가져오는 중" : "직접 가져오기"}
                  </Button>
                </div>
                {unsaved && (
                  <p className="text-sm text-muted-foreground">
                    바꾼 모델과 주소는 저장한 뒤에 받거나 가져올 수 있습니다
                  </p>
                )}
              </div>
              <Field label="모델 다운로드 주소 (사내 미러가 있으면 바꿉니다)">
                <Input
                  value={draft.model_base_url}
                  placeholder="https://huggingface.co"
                  onChange={(e) => edit({ model_base_url: e.target.value })}
                />
              </Field>
              <SelectField
                label="전사 백엔드"
                value={draft.backend}
                options={BACKENDS}
                onChange={(backend) => edit({ backend: backend as Backend })}
              />
              <Field label={`용어집: 사람 이름, 제품명을 한 줄에 하나씩 (${glossaryCount}/100)`}>
                <Textarea
                  className="h-32 field-sizing-fixed"
                  aria-invalid={glossaryCount > 100}
                  value={glossary}
                  onChange={(e) => {
                    setGlossary(e.target.value);
                    setNotice("");
                  }}
                />
              </Field>
            </section>

            <section hidden={tab !== "llm"} className="space-y-4">
              <h2 className="sr-only">요약(LLM)</h2>
              {draft.llm_connections.map((c, i) => (
                <Card key={c.name}>
                  <CardHeader>
                    <CardTitle>{c.name}</CardTitle>
                  </CardHeader>
                  <CardContent className="space-y-3">
                    <Field label="Base URL">
                      <Input
                        value={c.base_url}
                        placeholder="https://api.example.com/v1"
                        onChange={(e) => editLlm(i, { base_url: e.target.value })}
                      />
                    </Field>
                    {/* SUM-07 */}
                    {!c.is_local && (
                      <p className="text-sm text-muted-foreground">
                        요약할 때 전사본 전문이 이 주소로 전송됩니다
                      </p>
                    )}
                    <Field label="모델">
                      <Input value={c.model} onChange={(e) => editLlm(i, { model: e.target.value })} />
                    </Field>
                    <div className="grid grid-cols-2 gap-3">
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
                    </div>
                    <label className="flex items-center gap-2 text-sm">
                      <Switch
                        checked={c.is_local}
                        onCheckedChange={(isLocal) => {
                          // Keep a custom timeout; swap only the defaults (300 cloud / 900 local).
                          const usesDefault =
                            c.timeout_secs === (c.is_local ? localTimeout : cloudTimeout);
                          editLlm(i, {
                            is_local: isLocal,
                            timeout_secs: usesDefault
                              ? isLocal
                                ? localTimeout
                                : cloudTimeout
                              : c.timeout_secs,
                          });
                        }}
                      />
                      이 PC에서 도는 로컬 LLM (Ollama, LM Studio)
                    </label>
                    {/* 9.3: a server running a smaller context cuts the input without a word. */}
                    {c.is_local && (
                      <p className="text-sm text-muted-foreground">
                        Ollama는 OLLAMA_CONTEXT_LENGTH, LM Studio는 모델을 불러올 때 Context Length를
                        위 컨텍스트 토큰 이상으로 맞춰 주세요
                      </p>
                    )}
                    <SecretField label="API 키" {...secretProps("llm", c.name)} />
                    <RemoveButton
                      label="연결 삭제"
                      title={`${c.name} 연결을 삭제할까요?`}
                      description="저장하면 키체인의 API 키도 함께 지웁니다."
                      onConfirm={() => remove("llm", c.name)}
                    />
                  </CardContent>
                </Card>
              ))}
              <div className="flex gap-2">
                <Input
                  aria-label="새 LLM 연결 이름"
                  placeholder="새 연결 이름 (예: 회사 클라우드)"
                  value={newLlm}
                  onChange={(e) => setNewLlm(e.target.value)}
                />
                <Button variant="outline" onClick={addLlm}>
                  <Plus aria-hidden />
                  연결 추가
                </Button>
              </div>
              {draft.llm_connections.length > 0 && (
                <SelectField
                  label="기본 LLM"
                  value={draft.default_llm ?? NO_DEFAULT}
                  options={[
                    { value: NO_DEFAULT, label: "선택 안 함" },
                    ...draft.llm_connections.map((c) => ({ value: c.name, label: c.name })),
                  ]}
                  onChange={(v) => edit({ default_llm: v === NO_DEFAULT ? null : v })}
                />
              )}
              {/* SUM-08: each new meeting's "로컬로 요약" starts here. */}
              {hasLocal && (
                <label className="flex w-fit items-center gap-2 text-sm">
                  <Switch
                    checked={draft.summarize_local_by_default}
                    onCheckedChange={(v) => edit({ summarize_local_by_default: v })}
                  />
                  새 회의를 기본으로 로컬에서 요약
                </label>
              )}
            </section>

            <section hidden={tab !== "slack"} className="space-y-4">
              <h2 className="sr-only">Slack</h2>
              {draft.webhooks.map((w) => (
                <Card key={w.name}>
                  <CardHeader>
                    <CardTitle>{w.name}</CardTitle>
                  </CardHeader>
                  <CardContent className="space-y-3">
                    <SecretField label="웹훅 URL" {...secretProps("slack", w.name)} />
                    <RemoveButton
                      label="웹훅 삭제"
                      title={`${w.name} 웹훅을 삭제할까요?`}
                      description="저장하면 키체인의 웹훅 URL도 함께 지웁니다."
                      onConfirm={() => remove("slack", w.name)}
                    />
                  </CardContent>
                </Card>
              ))}
              <div className="flex gap-2">
                <Input
                  aria-label="새 웹훅 이름"
                  placeholder="채널 이름 (예: #회의록)"
                  value={newWebhook}
                  onChange={(e) => setNewWebhook(e.target.value)}
                />
                <Button variant="outline" onClick={addWebhook}>
                  <Plus aria-hidden />
                  웹훅 추가
                </Button>
              </div>
            </section>

            <div className="flex items-center gap-3 border-t pt-6">
              <Button onClick={save}>저장</Button>
              <span className="text-muted-foreground">
                {notice || "모든 탭의 변경 사항을 함께 저장합니다"}
              </span>
            </div>
          </fieldset>
        </div>
      </div>
    </Page>
  );
}

/** Confirmed in a dialog like DeleteButton, so a misclick never drops an entry and its key. */
function RemoveButton({
  label,
  title,
  description,
  onConfirm,
}: {
  label: string;
  title: string;
  description: string;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button variant="outline">
          <Trash2 aria-hidden />
          {label}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>취소</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={onConfirm}>
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
