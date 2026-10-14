import { useEffect, useState } from "react";
import { KeyRound, Plus, RotateCcw, Trash2 } from "lucide-react";
import { Banner, Field, Header } from "@/components/common";
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
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  deleteSecret,
  getSettings,
  hasSecret,
  NEWER_VERSION,
  resetSettings,
  saveSettings,
  setSecret,
  type Backend,
  type LlmConnection,
  type LoadedSettings,
  type SecretKind,
  type Settings as SettingsData,
} from "@/lib/ipc";

const BACKENDS: { value: Backend; label: string }[] = [
  { value: "auto", label: "자동" },
  { value: "gpu", label: "GPU" },
  { value: "cpu", label: "CPU" },
];

const cloudTimeout = 300;
const localTimeout = 900;
// Radix Select items cannot have an empty value. Names are trimmed and non-empty, so a space
// can never be a connection's name.
const NO_DEFAULT = " ";

// u32 on the Rust side; an emptied number field arrives as Number("") = 0.
const isPositiveInt = (n: number) => Number.isInteger(n) && n >= 1 && n <= 0xffffffff;

// Keychain entries are keyed `kind:name`, one per connection and webhook.
type SecretRef = { kind: SecretKind; name: string };
const secretRefs = (s: SettingsData): SecretRef[] => [
  ...s.llm_connections.map((c) => ({ kind: "llm" as const, name: c.name })),
  ...s.webhooks.map((w) => ({ kind: "slack" as const, name: w.name })),
];
const keyOf = ({ kind, name }: SecretRef) => `${kind}:${name}`;

export default function Settings({ onHome }: { onHome: () => void }) {
  const [draft, setDraft] = useState<SettingsData | null>(null);
  // What settings.json holds: keychain entries follow this, never the unsaved draft.
  const [persisted, setPersisted] = useState<SettingsData | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [glossary, setGlossary] = useState("");
  const [secrets, setSecrets] = useState<Record<string, boolean>>({});
  const [newLlm, setNewLlm] = useState("");
  const [newWebhook, setNewWebhook] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

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

  useEffect(() => {
    getSettings()
      .then(show)
      .catch((e) => setError(String(e)));
  }, []);

  const homeButton = (
    <Button variant="outline" onClick={onHome}>
      홈으로
    </Button>
  );

  if (!draft || !persisted) {
    // Unreadable settings.json: the old file is kept as a backup before starting over.
    return (
      <>
        <Header title="설정">{homeButton}</Header>
        <main className="mx-auto max-w-2xl space-y-4 p-6">
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
        </main>
      </>
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
  const nameProblem = (name: string, taken: string[]) =>
    !name ? "이름을 입력하세요" : taken.includes(name) ? "같은 이름이 이미 있습니다" : "";

  const addLlm = () => {
    const name = newLlm.trim();
    const problem = nameProblem(name, draft.llm_connections.map((c) => c.name));
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
    const problem = nameProblem(name, draft.webhooks.map((w) => w.name));
    if (problem) return setError(problem);
    edit({ webhooks: [...draft.webhooks, { name }] });
    setNewWebhook("");
    setError("");
  };

  // Only the draft changes; the keychain entry goes once the settings are saved.
  const remove = (kind: SecretKind, name: string) => {
    if (kind === "llm") {
      edit({
        llm_connections: draft.llm_connections.filter((c) => c.name !== name),
        default_llm: draft.default_llm === name ? null : draft.default_llm,
      });
    } else {
      edit({ webhooks: draft.webhooks.filter((w) => w.name !== name) });
    }
  };

  const save = async () => {
    const invalid = draft.llm_connections.find(
      (c) => !isPositiveInt(c.context_tokens) || !isPositiveInt(c.timeout_secs),
    );
    if (invalid) {
      return setError(`컨텍스트 토큰과 응답 시간 제한은 1 이상의 정수여야 합니다: ${invalid.name}`);
    }
    const value = {
      ...draft,
      recorder_name: draft.recorder_name.trim(),
      glossary: glossary
        .split("\n")
        .map((term) => term.trim())
        .filter(Boolean),
    };
    const kept = new Set(secretRefs(value).map(keyOf));
    const removed = secretRefs(persisted).filter((r) => !kept.has(keyOf(r)));
    try {
      await saveSettings(value);
    } catch (e) {
      // settings.json still lists the removed entries, so their secrets stay.
      return setError(String(e));
    }
    setDraft(value);
    setPersisted(value);
    setError("");
    setNotice("저장했습니다");
    for (const r of removed) {
      await deleteSecret(r.kind, r.name)
        .then(() => setSecrets((s) => ({ ...s, [keyOf(r)]: false })))
        .catch((e) => {
          setNotice("");
          setError(`설정은 저장했지만 키체인에서 키를 지우지 못했습니다: ${e}`);
        });
    }
  };

  const persistedKeys = new Set(secretRefs(persisted).map(keyOf));

  const secretProps = (kind: SecretKind, name: string) => ({
    kind,
    name,
    saved: !!secrets[keyOf({ kind, name })],
    // A key stored for an unsaved entry would be orphaned if the entry is never saved.
    unsaved: !persistedKeys.has(keyOf({ kind, name })),
    onSaved: () => setSecrets((s) => ({ ...s, [keyOf({ kind, name })]: true })),
    onError: setError,
  });

  const glossaryCount = glossary.split("\n").filter((t) => t.trim()).length;

  return (
    <>
      <Header title="설정">{homeButton}</Header>
      <main className="mx-auto max-w-2xl space-y-6 p-6">
        {readOnly && <Banner kind="warning">{NEWER_VERSION}. 설정을 바꿀 수 없습니다.</Banner>}
        {error && <Banner kind="error">{error}</Banner>}

        <fieldset disabled={readOnly} className="space-y-8">
          <section className="space-y-3">
            <h2 className="font-bold">녹음자</h2>
            <Field label="녹음자 이름 (Slack 메시지에 표시됩니다)">
              <Input
                value={draft.recorder_name}
                onChange={(e) => edit({ recorder_name: e.target.value })}
              />
            </Field>
          </section>

          <section className="space-y-3">
            <h2 className="font-bold">전사 백엔드</h2>
            <Select
              value={draft.backend}
              onValueChange={(backend) => edit({ backend: backend as Backend })}
            >
              <SelectTrigger aria-label="전사 백엔드" className="w-40">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {BACKENDS.map((b) => (
                  <SelectItem key={b.value} value={b.value}>
                    {b.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </section>

          <section className="space-y-3">
            <h2 className="font-bold">LLM 연결</h2>
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
              <div className="grid gap-1.5 text-sm">
                <span className="font-medium text-muted-foreground">기본 LLM</span>
                <Select
                  value={draft.default_llm ?? NO_DEFAULT}
                  onValueChange={(v) => edit({ default_llm: v === NO_DEFAULT ? null : v })}
                >
                  <SelectTrigger aria-label="기본 LLM" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={NO_DEFAULT}>선택 안 함</SelectItem>
                    {draft.llm_connections.map((c) => (
                      <SelectItem key={c.name} value={c.name}>
                        {c.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
          </section>

          <section className="space-y-3">
            <h2 className="font-bold">Slack 웹훅</h2>
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

          <section className="space-y-3">
            <h2 className="font-bold">용어집</h2>
            <Field label={`사람 이름, 제품명을 한 줄에 하나씩 (${glossaryCount}/100)`}>
              <Textarea
                className="h-32 field-sizing-fixed"
                value={glossary}
                onChange={(e) => {
                  setGlossary(e.target.value);
                  setNotice("");
                }}
              />
            </Field>
          </section>

          <div className="flex items-center gap-3">
            <Button onClick={save} disabled={glossaryCount > 100}>
              저장
            </Button>
            {notice && <span className="text-muted-foreground">{notice}</span>}
          </div>
        </fieldset>
      </main>
    </>
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

/** Secrets go straight to the OS keychain (ONB-08); the UI only learns whether one is stored. */
function SecretField({
  label,
  kind,
  name,
  saved,
  unsaved,
  onSaved,
  onError,
}: {
  label: string;
  kind: SecretKind;
  name: string;
  saved: boolean;
  unsaved: boolean;
  onSaved: () => void;
  onError: (message: string) => void;
}) {
  const [value, setValue] = useState("");
  const status = unsaved ? "설정을 저장한 뒤 입력하세요" : saved ? "키체인에 저장됨" : "입력 안 됨";
  return (
    <div className="flex items-end gap-2">
      <div className="flex-1">
        <Field label={`${label} · ${status}`}>
          <Input
            type="password"
            autoComplete="off"
            disabled={unsaved}
            placeholder={saved ? "바꾸려면 새로 입력" : ""}
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
        </Field>
      </div>
      <Button
        variant="outline"
        disabled={unsaved || !value}
        onClick={() =>
          setSecret(kind, name, value)
            .then(() => {
              setValue("");
              onSaved();
            })
            .catch((e) => onError(String(e)))
        }
      >
        <KeyRound aria-hidden />
        키체인에 저장
      </Button>
    </div>
  );
}
