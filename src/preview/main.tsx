import React, { useState } from "react";
import ReactDOM from "react-dom/client";
import { MotionConfig } from "motion/react";
import App from "@/App";
import { modelDownload, startModelDownload } from "@/lib/ipc";
import { install, reset, SCENARIOS, simulate, type Scenario } from "./fixtures";
import "./preview.css";

// The dev preview (`bun run dev` → /preview.html): the real App over a mocked backend, so every
// screen and its motion can be seen without recording. Not part of the build.
install("many");

const PILL = "rounded-full px-2.5 py-1 hover:bg-white/10 aria-pressed:bg-white aria-pressed:text-ink";

function Preview() {
  const [scenario, setScenario] = useState<Scenario>("many");
  // Picking a scenario, even the same one, starts it over with a fresh App.
  const [run, setRun] = useState(0);
  const pick = async (next: Scenario) => {
    reset(next);
    // ipc.ts keeps one download for the app: the last one settles first. 설정 없음 is mid-download,
    // started before the App so Home and the 전사 tab follow it.
    await modelDownload();
    if (next === "setup") void startModelDownload();
    setScenario(next);
    setRun((n) => n + 1);
  };

  return (
    <>
      <App key={run} />
      <section
        aria-label="미리보기 시나리오"
        className="fixed bottom-4 left-4 z-[100] grid gap-2 rounded-float bg-ink p-3 text-label text-white shadow-float"
      >
        <span className="px-1 text-caption text-dock-2">미리보기</span>
        <div className="flex flex-wrap gap-1">
          {(Object.keys(SCENARIOS) as Scenario[]).map((s) => (
            <button key={s} aria-pressed={s === scenario} className={PILL} onClick={() => pick(s)}>
              {SCENARIOS[s]}
            </button>
          ))}
        </div>
        <div className="flex flex-wrap gap-1 border-t border-white/15 pt-2">
          <button className={PILL} onClick={simulate.gpuFailed}>
            GPU 경고
          </button>
          <button className={PILL} onClick={simulate.quitRequested}>
            종료 확인
          </button>
        </div>
      </section>
    </>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <MotionConfig reducedMotion="user">
      <Preview />
    </MotionConfig>
  </React.StrictMode>,
);
