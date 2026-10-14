import { useEffect, useState } from "react";
import { Mic } from "lucide-react";
import { Banner, DeleteButton, Page, StatusBadge } from "@/components/common";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  getSettings,
  resumeMeeting,
  startRecording,
  STATUS_LABEL,
  type MeetingItem,
} from "@/lib/ipc";
import type { SettingsTab } from "@/screens/Settings";

/** Something summarizing or sending needs, and the settings tab that fixes it. */
type SetupItem = { text: string; action: string; tab: SettingsTab };

function Title({ meeting }: { meeting: MeetingItem }) {
  return (
    <span>
      <span className="block font-medium">{meeting.title || "제목 없음"}</span>
      <span className="text-muted-foreground">
        {new Date(meeting.created_at).toLocaleString("ko-KR")}
      </span>
    </span>
  );
}

export default function Home({
  meetings,
  recording,
  onChanged,
  onOpen,
  onRecord,
  onSetup,
}: {
  meetings: MeetingItem[] | null;
  /** A recording is running: the main button goes back to it instead of starting another. */
  recording: boolean;
  onChanged: () => void;
  onOpen: (meeting: MeetingItem) => void;
  onRecord: () => void;
  onSetup: (tab: SettingsTab) => void;
}) {
  const [setup, setSetup] = useState<SetupItem[]>([]);
  const [error, setError] = useState("");
  const [starting, setStarting] = useState(false);

  // Recording works without any of these. M3 adds the Whisper model download here.
  useEffect(() => {
    getSettings()
      .then(({ settings }) =>
        setSetup(
          [
            !settings.llm_connections.length && {
              text: "LLM 연결이 없습니다",
              action: "LLM 연결하기",
              tab: "llm" as const,
            },
            !settings.webhooks.length && {
              text: "Slack 웹훅이 없습니다",
              action: "웹훅 추가하기",
              tab: "slack" as const,
            },
          ].filter((item) => item !== false),
        ),
      )
      // The settings screen shows the error and offers starting over.
      .catch(() => setSetup([{ text: "설정 파일을 읽을 수 없습니다", action: "설정 열기", tab: "general" }]));
  }, []);

  const start = () => {
    setStarting(true);
    startRecording().then(onRecord, (e) => {
      setError(String(e));
      setStarting(false);
    });
  };

  const interrupted = meetings?.filter((m) => m.interrupted) ?? [];
  const listed = meetings?.filter((m) => !m.interrupted);

  return (
    <Page
      title="회의"
      action={
        <Button
          size="lg"
          className="h-11 px-6 text-base"
          disabled={starting}
          onClick={recording ? onRecord : start}
        >
          <Mic aria-hidden className="size-5" />
          {recording ? "녹음 화면으로" : "녹음 시작"}
        </Button>
      }
    >
      {error && <Banner kind="error">{error}</Banner>}

      {setup.length > 0 && (
        <Card className="gap-0 pb-0">
          <CardHeader className="border-b">
            <CardTitle>
              <h2 className="font-bold">설정이 남아 있습니다</h2>
            </CardTitle>
            <CardDescription>
              녹음은 지금 바로 할 수 있습니다. 요약하고 Slack으로 보내려면 아래 항목이 필요합니다.
            </CardDescription>
          </CardHeader>
          <CardContent className="px-0">
            <ul className="divide-y">
              {setup.map((item) => (
                <li key={item.tab} className="flex items-center justify-between gap-4 px-4 py-3">
                  <span>{item.text}</span>
                  <Button variant="outline" onClick={() => onSetup(item.tab)}>
                    {item.action}
                  </Button>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      {/* STO-03: left in `recording` by a crash. The WAV files hold what the last 5 s flush wrote. */}
      {interrupted.length > 0 && (
        <section className="space-y-2">
          <h2 className="font-bold">중단된 녹음</h2>
          <p className="text-muted-foreground">
            녹음 중에 앱이 꺼졌습니다. 마지막으로 저장된 곳까지 처리하거나 삭제할 수 있습니다.
          </p>
          <Card className="gap-0 py-0">
            <ul className="divide-y">
              {interrupted.map((m) => (
                <li key={m.id} className="flex items-center justify-between gap-4 px-4 py-3">
                  <Title meeting={m} />
                  <span className="flex gap-2">
                    <Button
                      variant="outline"
                      onClick={() => resumeMeeting(m.id).then(onChanged, (e) => setError(String(e)))}
                    >
                      처리 계속
                    </Button>
                    <DeleteButton id={m.id} onDeleted={onChanged} onError={setError} />
                  </span>
                </li>
              ))}
            </ul>
          </Card>
        </section>
      )}

      <section className="space-y-2">
        <h2 className="font-bold">최근 회의</h2>
        {listed?.length === 0 && <p className="text-muted-foreground">아직 회의가 없습니다</p>}
        {!!listed?.length && (
          <Card className="gap-0 py-0">
            <ul className="divide-y">
              {listed.map((m) => (
                <li key={m.id}>
                  <button
                    className="flex w-full items-center justify-between gap-4 px-4 py-3 text-left outline-none hover:bg-accent focus-visible:ring-3 focus-visible:ring-ring focus-visible:ring-inset"
                    onClick={() => onOpen(m)}
                  >
                    <Title meeting={m} />
                    <StatusBadge recording={m.status === "recording"}>
                      {STATUS_LABEL[m.status] ?? m.status}
                    </StatusBadge>
                  </button>
                </li>
              ))}
            </ul>
          </Card>
        )}
      </section>
    </Page>
  );
}
