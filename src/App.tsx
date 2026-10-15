import { useEffect, useRef, useState, type ReactNode } from "react";
import { Settings as SettingsIcon } from "lucide-react";
import { AnimatePresence, motion, useIsPresent } from "motion/react";
import { Banner, Bars, mmss, phase, StatusBadge, WIDTH } from "@/components/common";
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
import { bannerVariants, screenVariants } from "@/lib/motion";
import {
  confirmQuit,
  getMeeting,
  listMeetings,
  onGpuFailed,
  onMeetingProgress,
  onOpenMeeting,
  onQuitRequested,
  onRecordingLevel,
  onRecordingStopped,
  onSlackFinished,
  queueState,
  requestQuit,
  SCHEMA_VERSION,
  type MeetingItem,
  type Meta,
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
/** A meeting's own screen, from where it is now. */
const screenOf = (m: Meta): Screen =>
  m.status === "recording"
    ? { name: "recording" }
    : { name: REVIEWABLE.includes(m.status) ? "review" : "processing", id: m.id };
// Every app banner enters, shows and leaves alike.
const BANNER = { variants: bannerVariants, initial: "enter", animate: "show", exit: "exit" };
// h-6: a 24px target around the badge (WCAG 2.5.8).
const CHIP = "flex h-6 items-center rounded-full hover:opacity-80";

/** A screen or banner on its way out stays mounted for its exit: out of the tab order, the
 * accessibility tree and hit-testing meanwhile. */
function Present({ children }: { children: ReactNode }) {
  return <div inert={!useIsPresent()}>{children}</div>;
}

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
      <StatusBadge kind="recording">{`녹음 중 ${mmss(s * 1000)}`}</StatusBadge>
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
  // Why a recording ended by itself (disk, 4 hours, sleep), the GPU fallback, a progress listener
  // that would not start, or a review edit that failed to save after its screen closed. The first
  // is an OS notification too while the window is hidden or not focused (APP-03).
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

  // A send that ended, maybe after its screen closed: a sent meeting is no longer unsent.
  useEffect(() => {
    const unlisten = onSlackFinished(() => refresh());
    unlisten.catch((e) => setNotice(`전송 결과 알림을 준비하지 못했습니다: ${e}`));
    return () => {
      unlisten.then((stop) => stop(), () => {});
    };
  }, []);

  // APP-03: a notification click opens its meeting where it is now: done or failed when told, it
  // may have been sent or retried since.
  useEffect(() => {
    const unlisten = onOpenMeeting((id) =>
      getMeeting(id).then(
        ({ meta }) => setScreen(screenOf(meta)),
        (e) => setNotice(`회의를 열지 못했습니다: ${e}`),
      ),
    );
    unlisten.catch((e) => setNotice(`알림 클릭을 준비하지 못했습니다: ${e}`));
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
  // that, `percent` is the planning's own. A summary shows one only past a single LLM request, and
  // a meeting's last event (before the list is read again) none.
  const percents = running.flatMap((m) => {
    const p = progress[m.id];
    const counted = p?.status === "transcribing" ? p.chunks > 0 : p?.status === "summarizing" && p.chunks > 1;
    return counted ? [`${p.percent}%`] : [];
  });
  const banners = [quitError, listError].filter(Boolean);
  const width = WIDTH[screen.name === "settings" ? "wide" : screen.name === "review" ? "doc" : "home"];
  // Forward from Home rises, back to Home sinks (design-system 6.1).
  const direction = screen.name === "home" ? -1 : 1;

  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-10 flex h-12 items-center justify-between gap-4 border-b border-line bg-paper/92 px-6 backdrop-blur-md">
        <div className="flex min-w-0 items-center gap-3">
          <button className="flex items-center gap-2 rounded-md font-display text-title" onClick={home}>
            {/* Still unless something is being transcribed or summarized. */}
            <Bars live={running.length > 0} className="h-3.5 text-brand" />
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
        <Button variant="ghost" size="icon" aria-label="설정" onClick={() => setScreen({ name: "settings" })}>
          <SettingsIcon aria-hidden className="size-5" />
        </Button>
      </header>

      {/* A leaving banner pops out of the flow at once, so the screen's `layout` slides it up in
          the same render instead of jumping when the fade ends. */}
      <div className={`relative mx-auto box-content px-8 ${width}`}>
        <AnimatePresence initial={false} mode="popLayout">
          {banners.map((b) => (
            <motion.div key={b} layout="position" className="pt-4" {...BANNER}>
              <Present>
                <Banner kind="error">{b}</Banner>
              </Present>
            </motion.div>
          ))}
          {notice && (
            <motion.div key="notice" layout="position" className="pt-4" {...BANNER}>
              <Present>
                <Banner kind="warning" onDismiss={() => setNotice("")}>
                  {notice}
                </Banner>
              </Present>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      {/* The leaving screen pops out of the flow too: the two cross without a layout shift. */}
      <div className="relative">
        <AnimatePresence initial={false} mode="popLayout" custom={direction}>
          <motion.div
            key={"id" in screen ? `${screen.name}:${screen.id}` : screen.name}
            layout="position"
            custom={direction}
            variants={screenVariants}
            initial="enter"
            animate="show"
            exit="exit"
          >
            <Present>
              {screen.name === "home" && (
                <Home
                  meetings={meetings}
                  progress={progress}
                  recording={recording}
                  onChanged={refresh}
                  onOpen={(m) => setScreen(screenOf(m))}
                  onRecord={() => setScreen({ name: "recording" })}
                  onSetup={(tab) => setScreen({ name: "settings", tab })}
                />
              )}
              {screen.name === "recording" && (
                <Recording onHome={home} onStopped={(id) => setScreen({ name: "processing", id })} />
              )}
              {screen.name === "processing" && (
                <Processing
                  key={screen.id}
                  id={screen.id}
                  progress={progress[screen.id]}
                  onHome={home}
                  // Only from its own screen: one on its way out may still finish a read.
                  onReview={(id) =>
                    setScreen((s) => (s.name === "processing" && s.id === id ? { name: "review", id } : s))
                  }
                  onSetup={(tab) => setScreen({ name: "settings", tab })}
                />
              )}
              {screen.name === "review" && (
                <Review
                  key={screen.id}
                  id={screen.id}
                  onHome={home}
                  onNotice={setNotice}
                  onSetup={(tab) => setScreen({ name: "settings", tab })}
                />
              )}
              {screen.name === "settings" && <Settings tab={screen.tab} onHome={home} />}
            </Present>
          </motion.div>
        </AnimatePresence>
      </div>

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
