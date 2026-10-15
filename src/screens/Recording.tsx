import { useEffect, useRef, useState, type RefObject } from "react";
import { ArrowLeft, Square } from "lucide-react";
import { Banner, clock, hhmm, Kbd, RollingTime, RouteNotice, SHORTCUT, sinceStart } from "@/components/common";
import { Button } from "@/components/ui/button";
import { onRecordingLevel, onRecordingWarning, recordingState, type Warning } from "@/lib/ipc";

/** -50 dBFS, the silence threshold (REC-05). Louder system audio counts as heard (REC-10). */
const HEARD_RMS = 0.0032;


/** RMS on a -60..0 dBFS scale, as a percentage. */
const meterValue = (rms: number) =>
  Math.round(Math.min(Math.max((20 * Math.log10(rms || 1e-6) + 60) / 60, 0), 1) * 100);

/** 48 thin bars per track: the 24 bands mirrored out from the middle, low frequencies at the
 * center, so a voice swells from there. */
const MIRROR = [...Array.from({ length: 24 }, (_, i) => 23 - i), ...Array.from({ length: 24 }, (_, i) => i)];
const FLAT = { transform: "scaleY(0)" };

/** One track's bands as bars (REC-09): the mic grows up from the middle line, system audio down.
 * Square ends, so scaleY never warps them. Decoration: screen readers get the level as a meter. */
function Track({
  label,
  up,
  color,
  bars,
  meter,
}: {
  label: string;
  up: boolean;
  color: string;
  bars: RefObject<HTMLDivElement | null>;
  meter: RefObject<HTMLSpanElement | null>;
}) {
  return (
    <div>
      <div ref={bars} aria-hidden className="flex h-16 gap-[3px]">
        {MIRROR.map((_, i) => (
          <div key={i} className={`h-full w-[3px] ${color} ${up ? "origin-bottom" : "origin-top"}`} style={FLAT} />
        ))}
      </div>
      {/* Not a live region: read when the recorder moves to it, never announced per event. */}
      <span
        ref={meter}
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={0}
        className="sr-only"
      />
    </div>
  );
}

export default function Recording({
  startedAt,
  stopping,
  onHome,
  onStop,
}: {
  /** The recording meeting's `created_at`, when the list has it. */
  startedAt?: string;
  /** A stop was asked (App.tsx stops, as ⌘. and the dock do). */
  stopping: boolean;
  onHome: () => void;
  onStop: () => void;
}) {
  // The recording's meeting id; null when none is running, undefined until asked.
  const [id, setId] = useState<string | null>();
  const [seconds, setSeconds] = useState(() => sinceStart(startedAt));
  const [waitingPermission, setWaitingPermission] = useState(false);
  const [warnings, setWarnings] = useState<Warning[]>([]);
  const [heardSystem, setHeardSystem] = useState(false);
  const [error, setError] = useState("");
  const micBars = useRef<HTMLDivElement>(null);
  const micMeter = useRef<HTMLSpanElement>(null);
  const systemBars = useRef<HTMLDivElement>(null);
  const systemMeter = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    // 25 times a second, straight to the DOM: no React state, no CSS transition (design-system 5).
    const paint = (bars: HTMLDivElement | null, meter: HTMLSpanElement | null, bands: number[], rms: number) => {
      bars?.childNodes.forEach((bar, i) => {
        (bar as HTMLElement).style.transform = `scaleY(${(bands[MIRROR[i]] ?? 0) / 100})`;
      });
      meter?.setAttribute("aria-valuenow", String(meterValue(rms)));
    };
    const add = (w: Warning) => setWarnings((ws) => [...ws.filter((x) => x.kind !== w.kind), w]);
    const unlisten = [
      onRecordingLevel((l) => {
        paint(micBars.current, micMeter.current, l.mic_bands, l.mic);
        paint(systemBars.current, systemMeter.current, l.system_bands, l.system);
        // Equal values let React skip the event: these change about once a second at most.
        setSeconds(Math.floor(l.elapsed_ms / 1000));
        setWaitingPermission(l.waiting_permission);
        if (l.system >= HEARD_RMS) setHeardSystem(true);
      }),
      onRecordingWarning(add),
    ];
    // Listen first, then read the standing warnings, so none is missed in between.
    Promise.all(unlisten)
      .then(recordingState)
      .then((state) => {
        setId(state?.id ?? null);
        state?.warnings.forEach(add);
      })
      .catch((e) => setError(String(e)));
    return () => unlisten.forEach((u) => u.then((stop) => stop(), () => {}));
    // Asked again when the list shows a new recording: one started (⌘R) while this screen said
    // none was running.
  }, [startedAt]);

  return (
    // Centered in the window under the header, clear of the dock's room (App.tsx `pb-24`).
    <main className="mx-auto box-content flex min-h-[calc(100dvh-9rem)] max-w-home flex-col px-8 py-6">
      <div className="flex items-center gap-3">
        <Button variant="ghost" size="sm" className="-ml-3 text-ink-2 hover:text-ink" onClick={onHome}>
          <ArrowLeft aria-hidden />홈
        </Button>
        {id && <span className="text-caption text-ink-2">녹음은 계속돼요</span>}
      </div>
      <div className="flex flex-1 flex-col items-center justify-center gap-10 py-8 text-center">
        {error && (
          <div className="self-stretch text-left">
            <Banner kind="error">{error}</Banner>
          </div>
        )}
        {id === null && <h1 className="font-display text-display-m">진행 중인 녹음이 없습니다</h1>}
        {id && (
          <>
            <div className="flex flex-col items-center gap-2">
              <h1 className="flex items-center gap-2 text-label text-ink-2">
                <span aria-hidden className="size-2 rounded-full bg-rec motion-safe:animate-breathe" />
                녹음 중{startedAt && ` · ${hhmm(startedAt)} 시작`}
              </h1>
              <RollingTime text={clock(seconds)} className="text-display-xl" />
              {waitingPermission && (
                <p className="text-ink-2">
                  권한 허용을 기다리는 중입니다. 화면에 뜬 권한 요청에서 허용을 눌러 주세요.
                </p>
              )}
            </div>
            {/* Two tracks mirrored on one line, never mixed into one bar. */}
            <div className="flex flex-col items-center gap-4">
              <div className="w-fit">
                <Track label="내 마이크" up color="bg-mic" bars={micBars} meter={micMeter} />
                <div className="h-px bg-line" />
                <Track label="시스템 오디오" up={false} color="bg-system" bars={systemBars} meter={systemMeter} />
              </div>
              {/* The meters carry these names for screen readers. */}
              <div aria-hidden className="flex items-center gap-4 text-caption text-ink-2">
                <span className="flex items-center gap-1.5">
                  <span className="size-2 bg-mic" />내 마이크
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="size-2 bg-system" />
                  시스템 오디오
                </span>
              </div>
            </div>
            <div className="flex w-full max-w-doc flex-col items-center gap-3">
              <RouteNotice id={id} center />
              {heardSystem && (
                <p className="text-sm text-ink-2">
                  스피커 소리가 마이크에 다시 녹음되면 전사가 부정확해질 수 있습니다. 헤드폰을 쓰는 것을
                  권장합니다.
                </p>
              )}
              {/* Dismissed until the same kind is sent again (REC-07, REC-08). */}
              {warnings.map((w) => (
                <div key={w.kind} className="self-stretch text-left">
                  <Banner kind="warning" onDismiss={() => setWarnings((ws) => ws.filter((x) => x.kind !== w.kind))}>
                    {w.message}
                  </Banner>
                </div>
              ))}
            </div>
            {/* White on ink with a red square (design-system 3): no small-text contrast exception. */}
            <Button
              variant="ink"
              className="h-12 rounded-full px-7"
              disabled={stopping}
              aria-keyshortcuts={SHORTCUT.stop.aria}
              onClick={onStop}
            >
              <Square aria-hidden className="fill-current text-rec" />
              녹음 종료
              <Kbd className="ml-1">{SHORTCUT.stop.label}</Kbd>
            </Button>
          </>
        )}
      </div>
    </main>
  );
}
