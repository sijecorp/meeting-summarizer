import { useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";
import {
  Banner,
  DeleteButton,
  mmss,
  Page,
  phase,
  StatusBadge,
  statusLabel,
  TranscribeProgress,
} from "@/components/common";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  getMeeting,
  getTranscript,
  NEWER_VERSION,
  resumeMeeting,
  SCHEMA_VERSION,
  type Meta,
  type Progress,
  type Segment,
} from "@/lib/ipc";

export default function Processing({
  id,
  progress,
  onHome,
}: {
  id: string;
  /** The meeting's last `meeting://progress`, if it has been in line. */
  progress?: Progress;
  onHome: () => void;
}) {
  const [meta, setMeta] = useState<Meta | null>(null);
  const [segments, setSegments] = useState<Segment[] | null>(null);
  const [error, setError] = useState("");

  // Read again when the status or the finished chunks change (`backend`, `chunks_done`). Only the
  // latest reply may land: an older one can resolve after a newer one. A fresh read clears an
  // older error.
  useEffect(() => {
    let latest = true;
    getMeeting(id).then(
      (detail) => {
        if (!latest) return;
        setMeta(detail.meta);
        setError("");
      },
      (e) => latest && setError(String(e)),
    );
    return () => {
      latest = false;
    };
  }, [id, progress?.status, progress?.chunk]);

  // Summarizing is M4: until then the transcript is where the recorder sees the result.
  useEffect(() => {
    if (meta?.status !== "transcribed") return;
    getTranscript(id).then(setSegments, (e) => setError(String(e)));
  }, [id, meta?.status]);

  const now = meta && phase(meta, progress);

  return (
    <Page title="처리 중" onHome={onHome}>
      {error && <Banner kind="error">{error}</Banner>}
      {meta && (
        <>
          {meta.schema_version > SCHEMA_VERSION && <Banner kind="warning">{NEWER_VERSION}</Banner>}
          <Card>
            <CardHeader>
              <CardTitle>
                <h2 className="text-xl font-bold">{meta.title || "제목 없음"}</h2>
              </CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2 text-base">
                <dt className="text-muted-foreground">상태</dt>
                <dd>
                  <StatusBadge>{statusLabel(meta, progress)}</StatusBadge>
                </dd>
                {now === "running" && progress && (
                  <>
                    <dt className="text-muted-foreground">진행</dt>
                    <dd>
                      <TranscribeProgress progress={progress} detailed />
                    </dd>
                  </>
                )}
                <dt className="text-muted-foreground">녹음 시각</dt>
                <dd>{new Date(meta.created_at).toLocaleString("ko-KR")}</dd>
                {meta.backend && (
                  <>
                    <dt className="text-muted-foreground">처리 장치</dt>
                    <dd>{meta.backend.toUpperCase()}</dd>
                  </>
                )}
                <dt className="text-muted-foreground">전사한 구간</dt>
                <dd>{meta.chunks_done}개</dd>
              </dl>
            </CardContent>
          </Card>
          {/* Hidden once a retry has it back in line. */}
          {meta.error && now === null && <Banner kind="error">{meta.error}</Banner>}
          <div className="flex gap-2">
            {meta.status === "transcribe_failed" && now === null && (
              <Button
                variant="outline"
                onClick={() =>
                  resumeMeeting(id).then(
                    (queued) => {
                      setMeta(queued);
                      setError("");
                    },
                    (e) => setError(String(e)),
                  )
                }
              >
                <RotateCcw aria-hidden />
                다시 시도
              </Button>
            )}
            <DeleteButton id={id} onDeleted={onHome} onError={setError} />
          </div>
          {segments && (
            <Card>
              <CardHeader>
                <CardTitle>
                  <h2 className="text-xl font-bold">전사본</h2>
                </CardTitle>
              </CardHeader>
              <CardContent>
                {segments.length === 0 ? (
                  <p className="text-muted-foreground">전사된 말이 없습니다</p>
                ) : (
                  // Plain text nodes only (architecture 10.4).
                  <ol className="space-y-2 text-base">
                    {segments.map((s, i) => (
                      <li key={i} className="grid grid-cols-[auto_1fr] gap-x-4">
                        <span className="text-muted-foreground tabular-nums">{mmss(s.start_ms)}</span>
                        <span>{s.text}</span>
                      </li>
                    ))}
                  </ol>
                )}
              </CardContent>
            </Card>
          )}
        </>
      )}
    </Page>
  );
}
