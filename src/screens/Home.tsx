import { useEffect, useState } from "react";
import { Mic } from "lucide-react";
import {
  Banner,
  DeleteButton,
  Page,
  phase,
  StatusBadge,
  statusLabel,
  TranscribeProgress,
} from "@/components/common";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  getSettings,
  modelDownload,
  modelDownloadFailure,
  modelStatus,
  onModelDownload,
  resumeMeeting,
  startModelDownload,
  startRecording,
  type MeetingItem,
  type ModelDownload,
  type Progress,
} from "@/lib/ipc";
import type { SettingsTab } from "@/screens/Settings";

/** Something transcribing, summarizing or sending needs, and the settings tab that fixes it. */
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
  progress,
  recording,
  onChanged,
  onOpen,
  onRecord,
  onSetup,
}: {
  meetings: MeetingItem[] | null;
  progress: Record<string, Progress>;
  /** A recording is running: the main button goes back to it instead of starting another. */
  recording: boolean;
  onChanged: () => void;
  onOpen: (meeting: MeetingItem) => void;
  onRecord: () => void;
  onSetup: (tab: SettingsTab) => void;
}) {
  const [setup, setSetup] = useState<SetupItem[]>([]);
  // The model row: a file is missing (or the check failed; the 전사 tab says why), or the shared
  // download (ipc.ts) is running, with its last `model://download`.
  const [noModel, setNoModel] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [received, setReceived] = useState<ModelDownload | null>(null);
  const [downloadFailed, setDownloadFailed] = useState(() => !!modelDownloadFailure());
  const [error, setError] = useState("");
  const [starting, setStarting] = useState(false);

  const checkModel = () =>
    modelStatus()
      .then((state) => state.files.some((f) => !f.installed))
      .catch(() => true)
      .then(setNoModel);

  // Checked again once the download settles: its last progress event comes before the hash check.
  const follow = (running: Promise<void>) => {
    setDownloading(true);
    running.then(() => {
      setDownloading(false);
      setReceived(null);
      setDownloadFailed(!!modelDownloadFailure());
      checkModel();
    });
  };

  // Recording works without any of these. The model comes first: nothing is transcribed without
  // it, so its row downloads right away (ONB-01); the 전사 tab has cancel, import and the mirror.
  useEffect(() => {
    checkModel();
    const unlisten = onModelDownload(setReceived);
    unlisten.catch((e) => setError(`다운로드 진행률을 받지 못했습니다: ${e}`));
    const running = modelDownload();
    if (running) follow(running);
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
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  const resume = (id: string) => resumeMeeting(id).then(onChanged, (e) => setError(String(e)));

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

      {(noModel || downloading || setup.length > 0) && (
        <Card className="gap-0 pb-0">
          <CardHeader className="border-b">
            <CardTitle>
              <h2 className="font-bold">설정이 남아 있습니다</h2>
            </CardTitle>
            <CardDescription>
              녹음은 지금 바로 할 수 있습니다. 전사하고 요약해 Slack으로 보내려면 아래 항목이
              필요합니다.
            </CardDescription>
          </CardHeader>
          <CardContent className="px-0">
            <ul className="divide-y">
              {(noModel || downloading) && (
                <li className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 px-4 py-3">
                  <span>
                    {downloading
                      ? `전사 모델을 받는 중 ${received ? Math.floor((received.received / received.total) * 100) : 0}%`
                      : downloadFailed
                        ? "전사 모델을 받지 못했습니다"
                        : "전사 모델이 없습니다"}
                  </span>
                  <span className="flex items-center gap-2">
                    <Button variant="link" onClick={() => onSetup("transcription")}>
                      전사 설정
                    </Button>
                    {!downloading && (
                      <Button variant="outline" onClick={() => follow(startModelDownload())}>
                        모델 받기
                      </Button>
                    )}
                  </span>
                </li>
              )}
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
                    <Button variant="outline" onClick={() => resume(m.id)}>
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
              {listed.map((m) => {
                const p = progress[m.id];
                const now = phase(m, p);
                return (
                  // The button stretches over the whole row (after:inset-0), so the progress and the
                  // error below it open the meeting too, without nesting them in the button.
                  <li key={m.id} className="relative hover:bg-accent">
                    <button
                      className="flex w-full items-center justify-between gap-4 px-4 py-3 text-left outline-none after:absolute after:inset-0 focus-visible:after:ring-3 focus-visible:after:ring-ring focus-visible:after:ring-inset"
                      onClick={() => onOpen(m)}
                    >
                      <Title meeting={m} />
                      <StatusBadge recording={m.status === "recording"}>{statusLabel(m, p)}</StatusBadge>
                    </button>
                    {now === "running" && p && (
                      <div className="px-4 pb-3">
                        <TranscribeProgress progress={p} name={m.title || "제목 없음"} />
                      </div>
                    )}
                    {now === null && m.status === "transcribe_failed" && (
                      <div className="flex items-center justify-between gap-4 px-4 pb-3">
                        {/* The full error is on the processing screen. */}
                        <span className="min-w-0 truncate text-sm text-muted-foreground">
                          {m.error ?? "전사하지 못했습니다"}
                        </span>
                        <Button
                          variant="outline"
                          className="relative"
                          aria-label={`다시 시도 (${m.title || "제목 없음"})`}
                          onClick={() => resume(m.id)}
                        >
                          다시 시도
                        </Button>
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          </Card>
        )}
      </section>
    </Page>
  );
}
