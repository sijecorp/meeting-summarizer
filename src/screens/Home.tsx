import { useEffect, useState } from "react";
import { Mic, Settings as SettingsIcon } from "lucide-react";
import { Banner, DeleteButton, Header } from "@/components/common";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  listMeetings,
  onRecordingStopped,
  resumeMeeting,
  startRecording,
  STATUS_LABEL,
  type MeetingItem,
} from "@/lib/ipc";

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
  onOpen,
  onRecord,
  onSettings,
}: {
  onOpen: (meeting: MeetingItem) => void;
  onRecord: () => void;
  onSettings: () => void;
}) {
  const [meetings, setMeetings] = useState<MeetingItem[] | null>(null);
  const [error, setError] = useState("");
  const [starting, setStarting] = useState(false);

  const load = () =>
    listMeetings()
      .then(setMeetings)
      .catch((e) => setError(String(e)));
  useEffect(() => {
    load();
    // A recording that ends while home is shown is no longer "녹음 중".
    const unlisten = onRecordingStopped(load);
    unlisten.catch((e) => setError(String(e)));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
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
    <>
      <Header title="회의 요약">
        <Button variant="outline" onClick={onSettings}>
          <SettingsIcon aria-hidden />
          설정
        </Button>
      </Header>
      <main className="mx-auto max-w-2xl space-y-6 p-6">
        {error && <Banner kind="error">{error}</Banner>}

        <Button size="lg" className="h-11 px-6 text-base" disabled={starting} onClick={start}>
          <Mic aria-hidden />
          녹음 시작
        </Button>

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
                      <Button onClick={() => resumeMeeting(m.id).then(load, (e) => setError(String(e)))}>
                        처리 계속
                      </Button>
                      <DeleteButton id={m.id} onDeleted={load} onError={setError} />
                    </span>
                  </li>
                ))}
              </ul>
            </Card>
          </section>
        )}

        <section className="space-y-2">
          <h2 className="font-bold">회의 목록</h2>
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
                      <Badge variant="secondary">{STATUS_LABEL[m.status] ?? m.status}</Badge>
                    </button>
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </section>
      </main>
    </>
  );
}
