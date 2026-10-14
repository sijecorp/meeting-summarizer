import { useEffect, useState } from "react";
import { Banner } from "@/components/common";
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
import {
  confirmQuit,
  getSettings,
  onQuitRequested,
  onRecordingStopped,
  requestQuit,
  type Status,
} from "@/lib/ipc";
import Home from "@/screens/Home";
import Onboarding from "@/screens/Onboarding";
import Processing from "@/screens/Processing";
import Recording from "@/screens/Recording";
import Review from "@/screens/Review";
import Settings from "@/screens/Settings";

// Six screens, no router: one piece of state (architecture 3, 10.1).
type Screen =
  | { name: "loading" | "onboarding" | "home" | "recording" | "settings" }
  | { name: "processing" | "review"; id: string };

const REVIEWABLE: Status[] = ["review", "sent", "archived"];

export default function App() {
  const [screen, setScreen] = useState<Screen>({ name: "loading" });
  const [quitAsked, setQuitAsked] = useState(false);
  const [quitError, setQuitError] = useState("");
  // Why a recording ended by itself (disk, 4 hours, sleep). OS notifications come in M5.
  const [notice, setNotice] = useState("");
  const home = () => setScreen({ name: "home" });
  const quitFailed = (e: unknown) => setQuitError(`종료하지 못했습니다: ${e}`);

  useEffect(() => {
    getSettings()
      .then(({ settings }) =>
        setScreen({
          name: settings.llm_connections.length && settings.webhooks.length ? "home" : "onboarding",
        }),
      )
      // The settings screen shows the load error.
      .catch(() => setScreen({ name: "settings" }));
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

  return (
    <div className="min-h-screen">
      {quitError && (
        <div className="p-4">
          <Banner kind="error">{quitError}</Banner>
        </div>
      )}
      {notice && (
        <div className="p-4">
          <Banner kind="warning" onDismiss={() => setNotice("")}>
            {notice}
          </Banner>
        </div>
      )}
      {screen.name === "onboarding" && <Onboarding onDone={(next) => setScreen({ name: next })} />}
      {screen.name === "home" && (
        <Home
          onOpen={(m) =>
            setScreen(
              m.status === "recording"
                ? { name: "recording" }
                : { name: REVIEWABLE.includes(m.status) ? "review" : "processing", id: m.id },
            )
          }
          onRecord={() => setScreen({ name: "recording" })}
          onSettings={() => setScreen({ name: "settings" })}
        />
      )}
      {screen.name === "recording" && (
        <Recording onHome={home} onStopped={(id) => setScreen({ name: "processing", id })} />
      )}
      {screen.name === "processing" && <Processing key={screen.id} id={screen.id} onHome={home} />}
      {screen.name === "review" && <Review key={screen.id} id={screen.id} onHome={home} />}
      {screen.name === "settings" && <Settings onHome={home} />}

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
