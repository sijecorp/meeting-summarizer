import { useEffect, useState } from "react";
import { Square } from "lucide-react";
import { Banner, Page } from "@/components/common";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  onRecordingLevel,
  onRecordingWarning,
  recordingState,
  stopRecording,
  type Level,
  type Warning,
} from "@/lib/ipc";

/** -50 dBFS, the silence threshold (REC-05). Louder system audio counts as heard (REC-10). */
const HEARD_RMS = 0.0032;

function formatElapsed(ms: number) {
  const s = Math.floor(ms / 1000);
  return [Math.floor(s / 3600), Math.floor(s / 60) % 60, s % 60]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
}

const SILENT: number[] = Array(24).fill(0);

/** The bands as bars (REC-09). They are decoration: screen readers get the level as a meter. */
function Spectrum({
  label,
  bands,
  rms,
  color,
}: {
  label: string;
  bands: number[];
  rms: number;
  color: string;
}) {
  // RMS on a -60..0 dBFS scale, as a percentage.
  const value = Math.round(Math.min(Math.max((20 * Math.log10(rms || 1e-6) + 60) / 60, 0), 1) * 100);
  return (
    <div className="grid gap-2">
      {/* The meter below carries the name for screen readers. */}
      <span aria-hidden className="text-sm font-medium text-muted-foreground">
        {label}
      </span>
      <div aria-hidden className="flex h-20 items-end gap-1">
        {bands.map((band, i) => (
          <div
            key={i}
            className={`min-h-0.5 flex-1 rounded-t-sm ${color} transition-[height] duration-100 ease-out motion-reduce:transition-none`}
            style={{ height: `${band}%` }}
          />
        ))}
      </div>
      {/* Not a live region: read when the recorder moves to it, never announced per event. */}
      <span
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={value}
        className="sr-only"
      />
    </div>
  );
}

export default function Recording({
  onHome,
  onStopped,
}: {
  onHome: () => void;
  onStopped: (id: string) => void;
}) {
  const [active, setActive] = useState<boolean | null>(null);
  const [level, setLevel] = useState<Level | null>(null);
  const [warnings, setWarnings] = useState<Warning[]>([]);
  const [heardSystem, setHeardSystem] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    const add = (w: Warning) => setWarnings((ws) => [...ws.filter((x) => x.kind !== w.kind), w]);
    const unlisten = [
      onRecordingLevel((l) => {
        setLevel(l);
        if (l.system >= HEARD_RMS) setHeardSystem(true);
      }),
      onRecordingWarning(add),
    ];
    // Listen first, then read the standing warnings, so none is missed in between.
    Promise.all(unlisten)
      .then(recordingState)
      .then((state) => {
        setActive(!!state);
        state?.warnings.forEach(add);
      })
      .catch((e) => setError(String(e)));
    return () => unlisten.forEach((u) => u.then((stop) => stop(), () => {}));
  }, []);

  const stop = () => {
    setStopping(true);
    stopRecording().then(
      (id) => (id ? onStopped(id) : onHome()),
      (e) => {
        setError(String(e));
        setStopping(false);
      },
    );
  };

  return (
    <Page title="녹음" onHome={onHome}>
      {error && <Banner kind="error">{error}</Banner>}
      {/* Dismissed until the same kind is sent again (REC-07, REC-08). */}
      {warnings.map((w) => (
        <Banner
          key={w.kind}
          kind="warning"
          onDismiss={() => setWarnings((ws) => ws.filter((x) => x.kind !== w.kind))}
        >
          {w.message}
        </Banner>
      ))}
      {active === false && <p className="text-muted-foreground">진행 중인 녹음이 없습니다.</p>}
      {active && (
        <div className="flex flex-col gap-6">
          <Card className="gap-6 p-6">
            <div className="flex flex-col items-center gap-2">
              <p className="flex items-center gap-3">
                <span className="size-3.5 rounded-full bg-danger motion-safe:animate-pulse" aria-hidden />
                <span className="text-5xl font-bold tabular-nums">
                  {formatElapsed(level?.elapsed_ms ?? 0)}
                </span>
              </p>
              {level?.waiting_permission && (
                <p className="text-muted-foreground">
                  권한 허용을 기다리는 중입니다. 화면에 뜬 권한 요청에서 허용을 눌러 주세요.
                </p>
              )}
            </div>
            <Spectrum
              label="내 마이크"
              bands={level?.mic_bands ?? SILENT}
              rms={level?.mic ?? 0}
              color="bg-mainBlue"
            />
            <Spectrum
              label="시스템 오디오"
              bands={level?.system_bands ?? SILENT}
              rms={level?.system ?? 0}
              color="bg-mint"
            />
          </Card>
          {heardSystem && (
            <p className="text-sm text-muted-foreground">
              스피커 소리가 마이크에 다시 녹음되면 전사가 부정확해질 수 있습니다. 헤드폰을 쓰는 것을
              권장합니다.
            </p>
          )}
          {/* White on danger is 3.2:1, so this label stays bold and at least 18.66px (NFR-09). */}
          <Button
            className="h-auto self-center bg-danger px-8 py-3 text-xl font-bold text-white hover:bg-dangerDark"
            disabled={stopping}
            onClick={stop}
          >
            <Square aria-hidden className="fill-current" />
            녹음 종료
          </Button>
        </div>
      )}
    </Page>
  );
}
