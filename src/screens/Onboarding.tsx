import { useEffect, useState } from "react";
import { Banner, Field, Header } from "@/components/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { getSettings, saveSettings, type LoadedSettings } from "@/lib/ipc";

export default function Onboarding({ onDone }: { onDone: (next: "settings" | "home") => void }) {
  const [loaded, setLoaded] = useState<LoadedSettings | null>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    getSettings()
      .then((l) => {
        setLoaded(l);
        setName(l.settings.recorder_name);
      })
      .catch((e) => setError(String(e)));
  }, []);

  const finish = async (next: "settings" | "home") => {
    const recorder_name = name.trim();
    if (loaded && !loaded.read_only && recorder_name !== loaded.settings.recorder_name) {
      try {
        await saveSettings({ ...loaded.settings, recorder_name });
      } catch (e) {
        return setError(String(e));
      }
    }
    onDone(next);
  };

  return (
    <>
      <Header title="회의 요약 시작하기" />
      <main className="mx-auto max-w-xl space-y-6 p-6">
        {error && <Banner kind="error">{error}</Banner>}
        <p>회의를 녹음하고, 이 PC에서 전사한 뒤, LLM으로 요약해 검토를 거쳐 Slack으로 보냅니다.</p>
        <Field label="녹음자 이름 (Slack 메시지에 표시됩니다)">
          <Input value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <p className="text-muted-foreground">
          요약에 쓸 LLM 연결과 보낼 Slack 웹훅은 설정에서 입력합니다.
        </p>
        <div className="flex gap-2">
          <Button onClick={() => finish("settings")}>연결 설정하기</Button>
          <Button variant="outline" onClick={() => finish("home")}>
            나중에 하기
          </Button>
        </div>
      </main>
    </>
  );
}
