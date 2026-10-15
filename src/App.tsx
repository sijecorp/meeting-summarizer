import { useEffect, useRef, useState } from "react";
import { Settings as SettingsIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Banner, Bars, phase, Present, WIDTH, withModifier } from "@/components/common";
import Dock from "@/components/Dock";
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
import { bannerVariants, screenVariants, type ScreenMove } from "@/lib/motion";
import {
  confirmQuit,
  getMeeting,
  listMeetings,
  onGpuFailed,
  onMeetingProgress,
  onOpenMeeting,
  onQuitRequested,
  onRecordingStopped,
  onSlackFinished,
  queueState,
  requestQuit,
  SCHEMA_VERSION,
  startRecording,
  stopRecording,
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
// Where a shortcut never fires: typing, or a dialog asking something.
const TYPING = "input, textarea, select, [contenteditable]";
const DIALOG = '[role="dialog"], [role="alertdialog"], [role="listbox"], [role="menu"]';

export default function App() {
  const [screen, setScreen] = useState<Screen>({ name: "home" });
  // The next screen change comes from a shortcut, so it does not move (design-system 5). Whichever
  // change comes first takes it: a stop's own reply or its `recording://stopped`.
  const byKey = useRef(false);
  const [instant, setInstant] = useState(false);
  const go = (next: Screen | ((s: Screen) => Screen)) => {
    setInstant(byKey.current);
    byKey.current = false;
    setScreen(next);
  };
  // Shared by the dock and Home, so both show the same list.
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
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  // Also in a ref: a second click on a stop button on its way out still holds the old state.
  const stopAsked = useRef(false);
  // The recording `recording://stopped` ended: until the list shows it, the dock bridges it like a
  // stop that was asked, instead of offering 종료 for a recording already over.
  const [ended, setEnded] = useState<string | null>(null);
  const stopDone = () => {
    stopAsked.current = false;
    setStopping(false);
  };
  const home = () => go({ name: "home" });
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
      // A start the list never showed (unreadable, or stopped by itself first) is over now.
      setStarting(false);
      setEnded(id);
      refresh();
      go((s) => (s.name === "recording" ? { name: "processing", id } : s));
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
        ({ meta }) => go(screenOf(meta)),
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
  const live = current.find((m) => m.status === "recording" && !m.interrupted);
  const recording = !!live;
  const ending = stopping || (!!live && live.id === ended);
  const running = current.filter((m) => phase(m, progress[m.id]) === "running");
  // By place in line; one without a place yet (just stopped, not yet in an event) last.
  const place = (m: MeetingItem) => progress[m.id]?.position ?? Number.MAX_SAFE_INTEGER;
  const waiting = current
    .filter((m) => phase(m, progress[m.id]) === "waiting")
    .sort((a, b) => place(a) - place(b));

  // Starting and stopping last until the list shows it, so the dock and Home never flick back.
  useEffect(() => {
    setStarting(false);
    stopDone();
  }, [recording]);
  const start = () => {
    setStarting(true);
    startRecording().then(
      () => {
        refresh();
        // Settings stays put: leaving would drop its unsaved draft, and the dock shows the recording.
        go((s) => (s.name === "settings" ? s : { name: "recording" }));
      },
      (e) => {
        byKey.current = false;
        setStarting(false);
        setNotice(`녹음을 시작하지 못했습니다: ${e}`);
      },
    );
  };
  // From the recording screen it goes on to the meeting; from the dock it stays where it is.
  const stop = () => {
    if (stopAsked.current) return;
    stopAsked.current = true;
    setStopping(true);
    stopRecording().then(
      (id) => {
        refresh();
        // A start the list never showed is over too.
        setStarting(false);
        // Nothing for the list to show: the recording had already ended.
        if (!id || !recording) stopDone();
        go((s) => (s.name === "recording" ? (id ? { name: "processing", id } : { name: "home" }) : s));
      },
      (e) => {
        byKey.current = false;
        stopDone();
        setNotice(`녹음을 끝내지 못했습니다: ${e}`);
      },
    );
  };

  // ⌘R / ⌘. (Ctrl on Windows) start and stop a recording from any screen while the window has
  // focus, never while typing or in a dialog. The webview's own reload never gets ⌘R.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!withModifier(e) || e.altKey || e.shiftKey || (e.code !== "KeyR" && e.code !== "Period")) return;
      e.preventDefault();
      const typing = e.target instanceof Element && e.target.closest(TYPING);
      if (e.repeat || typing || document.querySelector(DIALOG)) return;
      if (e.code === "KeyR" ? recording || starting : !recording || ending) return;
      byKey.current = true;
      if (e.code === "KeyR") start();
      else stop();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [recording, starting, ending]);

  const banners = [quitError, listError].filter(Boolean);
  const width = WIDTH[screen.name === "settings" ? "wide" : screen.name === "review" ? "doc" : "home"];
  // Forward from Home rises, back to Home sinks (design-system 6.1). Back always goes Home.
  const move: ScreenMove = { dir: screen.name === "home" ? -1 : 1, instant };

  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-10 flex h-12 items-center justify-between gap-4 border-b border-line bg-paper/92 px-6 backdrop-blur-md">
        <button className="flex items-center gap-2 rounded-md font-display text-title" onClick={home}>
          {/* Still unless something is being transcribed or summarized. */}
          <Bars live={running.length > 0} className="h-3.5 text-brand" />
          회의 요약
        </button>
        <Button variant="ghost" size="icon" aria-label="설정" onClick={() => go({ name: "settings" })}>
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

      {/* The leaving screen pops out of the flow too: the two cross without a layout shift. The
          bottom keeps clear of the dock. */}
      <div className="relative pb-24">
        <AnimatePresence initial={false} mode="popLayout" custom={move}>
          <motion.div
            key={"id" in screen ? `${screen.name}:${screen.id}` : screen.name}
            layout="position"
            custom={move}
            variants={screenVariants}
            initial="enter"
            animate="show"
            exit="exit"
            data-direction={move.dir > 0 ? "forward" : "back"}
          >
            <Present>
              {screen.name === "home" && (
                <Home
                  meetings={meetings}
                  listFailed={!!listError}
                  progress={progress}
                  recording={recording}
                  starting={starting}
                  onChanged={refresh}
                  onOpen={(m) => go(screenOf(m))}
                  onRecord={() => go({ name: "recording" })}
                  onStart={start}
                  onSetup={(tab) => go({ name: "settings", tab })}
                />
              )}
              {screen.name === "recording" && (
                <Recording startedAt={live?.created_at} stopping={ending} onHome={home} onStop={stop} />
              )}
              {screen.name === "processing" && (
                <Processing
                  key={screen.id}
                  id={screen.id}
                  progress={progress[screen.id]}
                  onHome={home}
                  // Only from its own screen: one on its way out may still finish a read.
                  onReview={(id) =>
                    go((s) => (s.name === "processing" && s.id === id ? { name: "review", id } : s))
                  }
                  onSetup={(tab) => go({ name: "settings", tab })}
                />
              )}
              {screen.name === "review" && (
                <Review
                  key={screen.id}
                  id={screen.id}
                  onHome={home}
                  onNotice={setNotice}
                  onSetup={(tab) => go({ name: "settings", tab })}
                />
              )}
              {screen.name === "settings" && <Settings tab={screen.tab} onHome={home} />}
            </Present>
          </motion.div>
        </AnimatePresence>
      </div>

      {/* From the first list on: the dock only says what changed after that. */}
      {meetings && (
        <Dock
          live={live}
          stopping={ending}
          queue={[...running, ...waiting]}
          progress={progress}
          hidden={screen.name === "recording"}
          onRecording={() => go({ name: "recording" })}
          onStop={stop}
          onOpen={(m) => go(screenOf(m))}
        />
      )}

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
