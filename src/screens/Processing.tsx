import { useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";
import {
  badgeKind,
  Banner,
  DeleteButton,
  host,
  Page,
  phase,
  ProcessProgress,
  RouteNotice,
  SelectField,
  StatusBadge,
  statusLabel,
  TranscriptList,
} from "@/components/common";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  getMeeting,
  getSettings,
  getTranscript,
  NEWER_VERSION,
  resumeMeeting,
  retrySummary,
  SCHEMA_VERSION,
  type LlmConnection,
  type Meta,
  type Progress,
  type Segment,
  type Status,
} from "@/lib/ipc";
import type { SettingsTab } from "@/screens/Settings";

// `transcript.json` exists from `transcribed` on; a `paused` meeting has it only if it stopped
// while summarizing.
const TRANSCRIBED: Status[] = ["transcribed", "summarizing", "summarize_failed", "paused"];

export default function Processing({
  id,
  progress,
  onHome,
  onReview,
  onSetup,
}: {
  id: string;
  /** The meeting's last `meeting://progress`, if it has been in line. */
  progress?: Progress;
  onHome: () => void;
  /** The summary is ready: the review screen takes over. */
  onReview: (id: string) => void;
  onSetup: (tab: SettingsTab) => void;
}) {
  const [meta, setMeta] = useState<Meta | null>(null);
  const [segments, setSegments] = useState<Segment[] | null>(null);
  // SUM-06: the connections a failed summary can run again with, and the one picked.
  const [connections, setConnections] = useState<LlmConnection[] | null>(null);
  const [chosen, setChosen] = useState("");
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

  useEffect(() => {
    if (meta?.status === "review") onReview(id);
  }, [id, meta?.status]);

  // The transcript is the result to read until the summary is ready.
  useEffect(() => {
    const status = meta?.status;
    if (!status || !TRANSCRIBED.includes(status)) return;
    getTranscript(id).then(setSegments, (e) => status !== "paused" && setError(String(e)));
  }, [id, meta?.status]);

  // The meeting's own connection first if it still exists, then the default, then the first.
  const summaryFailed = meta?.status === "summarize_failed";
  useEffect(() => {
    if (!summaryFailed) return;
    getSettings().then(({ settings }) => {
      const names = settings.llm_connections.map((c) => c.name);
      setConnections(settings.llm_connections);
      setChosen(
        [meta.llm_connection, settings.default_llm].find((n) => n && names.includes(n)) ??
          names[0] ??
          "",
      );
    }, (e) => setError(String(e)));
  }, [summaryFailed]);

  const now = meta && phase(meta, progress);
  const newer = !!meta && meta.schema_version > SCHEMA_VERSION;
  // Stopped after transcribing, with no place in line: no LLM connection. The last transcribing
  // event can still be here while the meeting already reads `transcribed`.
  const stopped =
    meta?.status === "transcribed" && now === null && !newer && progress?.status !== "transcribing";
  const failed = summaryFailed && now === null;
  const hint = stopped
    ? "LLM 연결을 추가하면 요약합니다"
    : failed && connections?.length === 0
      ? "LLM 연결을 추가하면 요약을 다시 실행할 수 있습니다"
      : "";

  return (
    <Page title="처리 중" onHome={onHome}>
      {error && <Banner kind="error">{error}</Banner>}
      {meta && (
        <>
          {newer && <Banner kind="warning">{NEWER_VERSION}</Banner>}
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
                  <StatusBadge kind={badgeKind(meta, progress)}>{statusLabel(meta, progress)}</StatusBadge>
                </dd>
                {now === "running" && progress && (
                  <>
                    <dt className="text-muted-foreground">진행</dt>
                    <dd>
                      <ProcessProgress progress={progress} detailed />
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
          {!newer && <RouteNotice id={id} status={meta.status} />}
          {/* Hidden once a retry has it back in line. */}
          {meta.error && now === null && <Banner kind="error">{meta.error}</Banner>}
          {hint && <p>{hint}</p>}
          <div className="flex flex-wrap items-end gap-2">
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
            {failed && !!connections?.length && (
              <>
                <div className="min-w-56">
                  <SelectField
                    label="요약할 LLM 연결"
                    value={chosen}
                    // SUM-07: where each one would send the transcript.
                    options={connections.map((c) => ({
                      value: c.name,
                      label: `${c.name} (${c.is_local ? "이 PC" : `${host(c.base_url)}로 전송`})`,
                    }))}
                    onChange={setChosen}
                  />
                </div>
                <Button
                  variant="outline"
                  onClick={() =>
                    retrySummary(id, chosen).then(
                      (queued) => {
                        setMeta(queued);
                        setError("");
                      },
                      (e) => setError(String(e)),
                    )
                  }
                >
                  <RotateCcw aria-hidden />
                  요약 다시 실행
                </Button>
              </>
            )}
            {/* A missing or refused key (6.4) is fixed there too. */}
            {(stopped || failed) && (
              <Button variant="outline" onClick={() => onSetup("llm")}>
                요약(LLM) 설정
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
                <TranscriptList segments={segments} />
              </CardContent>
            </Card>
          )}
        </>
      )}
    </Page>
  );
}
