import { useEffect, useRef, useState } from "react";
import { Settings as SettingsIcon } from "lucide-react";
import { Banner, mmss, phase, StatusBadge } from "@/components/common";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  confirmQuit,
  listMeetings,
  onGpuFailed,
  onMeetingProgress,
  onQuitRequested,
  onRecordingLevel,
  onRecordingStopped,
  queueState,
  requestQuit,
  SCHEMA_VERSION,
  type MeetingItem,
  type Progress,
  type Status,
} from "@/lib/ipc";
import Home from "@/screens/Home";
import Processing from "@/screens/Processing";
import Recording from "@/screens/Recording";
import Review from "@/screens/Review";
import Settings, { type SettingsTab } from "@/screens/Settings";

// Five screens, no router: one piece of state (architecture 3, 10.1). Always opens on Home.
type Screen =
  | { name: "home" | "recording" }
  | { name: "settings"; tab?: SettingsTab }
  | { name: "processing" | "review"; id: string };

const REVIEWABLE: Status[] = ["review", "sent", "archived"];
// h-6: a 24px target around the 20px badge (WCAG 2.5.8).
const CHIP =
  "flex h-6 items-center rounded-4xl outline-none hover:opacity-80 focus-visible:ring-3 focus-visible:ring-ring";

/** Listens to levels itself and keeps whole seconds, so only the chip re-renders, once a second. */
function RecordingChip({ onClick }: { onClick: () => void }) {
  const [s, setSeconds] = useState(0);
  useEffect(() => {
    // An equal value lets React skip the other 24 level events of each second.
    const unlisten = onRecordingLevel((l) => setSeconds(Math.floor(l.elapsed_ms / 1000)));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);
  return (
    <button className={CHIP} onClick={onClick}>
      <StatusBadge recording>{`녹음 중 ${mmss(s * 1000)}`}</StatusBadge>
    </button>
  );
}

export default function App() {
  const [screen, setScreen] = useState<Screen>({ name: "home" });
  // Shared by the header chip and Home, so both show the same list.
  const [meetings, setMeetings] = useState<MeetingItem[] | null>(null);
  // The last `meeting://progress` of each meeting (TRN-01, APP-07).
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [listError, setListError] = useState("");
  const [quitAsked, setQuitAsked] = useState(false);
  const [quitError, setQuitError] = useState("");
  // Why a recording ended by itself (disk, 4 hours, sleep), the GPU fallback, or a progress
  // listener that would not start. OS notifications come in M5.
  const [notice, setNotice] = useState("");
  const home = () => setScreen({ name: "home" });
  const quitFailed = (e: unknown) => setQuitError(`종료하지 못했습니다: ${e}`);

  // Only the latest request may set the list: an older one can resolve after a newer one.
  const latest = useRef(0);
  const refresh = () => {
    const request = ++latest.current;
    listMeetings().then(
      (list) => {
        if (request !== latest.current) return;
        setMeetings(list);
        setListError("");
      },
      (e) => request === latest.current && setListError(`회의 목록을 읽지 못했습니다: ${e}`),
    );
  };
  // Refreshed on screen changes, recording stops and status changes in progress events.
  useEffect(refresh, [screen]);

  // Listening comes first and the `queue_state` snapshot second, so nothing falls between them.
  useEffect(() => {
    // The last status seen per meeting: a new one means the list is stale.
    const seen = new Map<string, Status>();
    const unlisten = onMeetingProgress((p) => {
      if (seen.get(p.id) !== p.status) refresh();
      seen.set(p.id, p.status);
      setProgress((all) => ({ ...all, [p.id]: p }));
    });
    unlisten
      .then(queueState)
      // Events win: the snapshot may be older, and a running entry it brings back would never be
      // corrected after that meeting's last event.
      .then((list) => setProgress((all) => ({ ...Object.fromEntries(list.map((p) => [p.id, p])), ...all })))
      .catch((e) => setNotice(`처리 진행률을 읽지 못했습니다: ${e}`));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  useEffect(() => {
    const unlisten = onGpuFailed(({ message }) => setNotice(message));
    unlisten.catch((e) => setNotice(`GPU 전환 알림을 준비하지 못했습니다: ${e}`));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  useEffect(() => {
    const unlisten = onQuitRequested(() => setQuitAsked(true));
    unlisten.catch((e) => setQuitError(`종료 확인 창을 준비하지 못했습니다: ${e}`));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  // Listened here, not on the recording screen, so an automatic stop is shown wherever the
  // recorder is.
  useEffect(() => {
    const unlisten = onRecordingStopped(({ id, message }) => {
      if (message) setNotice(message);
      refresh();
      setScreen((s) => (s.name === "recording" ? { name: "processing", id } : s));
    });
    unlisten.catch((e) => setQuitError(`녹음 종료 알림을 준비하지 못했습니다: ${e}`));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  // Windows has no menu bar, so Ctrl+Q arrives here instead (architecture 8.5).
  useEffect(() => {
    if (!navigator.userAgent.includes("Windows")) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.code === "KeyQ") {
        e.preventDefault();
        requestQuit().catch(quitFailed);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // A meeting from a newer app version is read-only and never runs here (like `busy_ids`).
  const current = meetings?.filter((m) => m.schema_version <= SCHEMA_VERSION) ?? [];
  const recording = current.some((m) => m.status === "recording" && !m.interrupted);
  const running = current.filter((m) => phase(m, progress[m.id]) === "running");
  const waiting = current.filter((m) => phase(m, progress[m.id]) === "waiting").length;
  // "(42%)", or "(42%, 17%)" with two running. Only once a meeting's chunk plan exists: before
  // that, `percent` is the planning's own.
  const percents = running.flatMap((m) => {
    const p = progress[m.id];
    return p?.chunks ? [`${p.percent}%`] : [];
  });
  const banners = [quitError, listError].filter(Boolean);

  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-10 flex h-14 items-center justify-between gap-4 border-b bg-card px-6">
        <div className="flex min-w-0 items-center gap-3">
          <button
            className="rounded-md text-base font-bold outline-none focus-visible:ring-3 focus-visible:ring-ring"
            onClick={home}
          >
            회의 요약
          </button>
          {recording ? (
            <RecordingChip onClick={() => setScreen({ name: "recording" })} />
          ) : (
            running.length + waiting > 0 && (
              <button className={CHIP} onClick={home}>
                <StatusBadge>
                  {[
                    running.length &&
                      `처리 중 ${running.length}${percents.length ? ` (${percents.join(", ")})` : ""}`,
                    waiting && `대기 ${waiting}`,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </StatusBadge>
              </button>
            )
          )}
        </div>
        <Button
          variant="ghost"
          size="icon-lg"
          className="hover:bg-accent"
          aria-label="설정"
          onClick={() => setScreen({ name: "settings" })}
        >
          <SettingsIcon aria-hidden className="size-5" />
        </Button>
      </header>

      {(banners.length > 0 || notice) && (
        <div
          className={`mx-auto space-y-3 px-6 pt-6 ${screen.name === "settings" ? "max-w-5xl" : "max-w-3xl"}`}
        >
          {banners.map((b) => (
            <Banner key={b} kind="error">
              {b}
            </Banner>
          ))}
          {notice && (
            <Banner kind="warning" onDismiss={() => setNotice("")}>
              {notice}
            </Banner>
          )}
        </div>
      )}

      {screen.name === "home" && (
        <Home
          meetings={meetings}
          progress={progress}
          recording={recording}
          onChanged={refresh}
          onOpen={(m) =>
            setScreen(
              m.status === "recording"
                ? { name: "recording" }
                : { name: REVIEWABLE.includes(m.status) ? "review" : "processing", id: m.id },
            )
          }
          onRecord={() => setScreen({ name: "recording" })}
          onSetup={(tab) => setScreen({ name: "settings", tab })}
        />
      )}
      {screen.name === "recording" && (
        <Recording onHome={home} onStopped={(id) => setScreen({ name: "processing", id })} />
      )}
      {screen.name === "processing" && (
        <Processing key={screen.id} id={screen.id} progress={progress[screen.id]} onHome={home} />
      )}
      {screen.name === "review" && <Review key={screen.id} id={screen.id} onHome={home} />}
      {screen.name === "settings" && <Settings tab={screen.tab} onHome={home} />}

      <AlertDialog open={quitAsked} onOpenChange={setQuitAsked}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>진행 중인 작업이 있습니다</AlertDialogTitle>
            <AlertDialogDescription>
              지금 종료하면 진행 중인 녹음과 처리를 멈추고, 다음에 앱을 열 때 이어서 처리합니다.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>취소</AlertDialogCancel>
            <AlertDialogAction onClick={() => confirmQuit().catch(quitFailed)}>종료</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
