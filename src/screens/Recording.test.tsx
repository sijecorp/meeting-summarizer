import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { Level, RecordingState, Warning } from "../lib/ipc";
import Recording from "./Recording";

// No clearMocks(): the screen unlistens asynchronously after unmount (see App.test.tsx).
afterEach(cleanup);

const state: RecordingState = {
  id: "2026-10-14T0500-a1b2",
  warnings: [{ kind: "disk", message: "남은 공간으로 최대 약 90분 녹음할 수 있습니다" }],
};

test("shows elapsed time, spectrums and levels from level events", async () => {
  mockIPC((cmd) => (cmd === "recording_state" ? state : undefined), { shouldMockEvents: true });
  render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);
  expect(await screen.findByText("00:00")).toBeTruthy();

  const micBands = Array.from({ length: 24 }, (_, i) => i * 4);
  const level: Level = {
    elapsed_ms: 3_723_000,
    mic: 0.1,
    system: 0,
    mic_bands: micBands,
    system_bands: Array(24).fill(0),
    waiting_permission: true,
  };
  await act(() => emit("recording://level", level));

  // mm:ss under an hour, h:mm:ss past it.
  expect(screen.getByText("1:02:03")).toBeTruthy();
  // The bars are decoration next to each meter; the meter is what screen readers get. Levels go
  // straight to each bar's transform, never through a height: 48 bars, the 24 bands mirrored out
  // from the middle (band 0 at the two center bars, band 23 at both ends).
  const bars = (name: string) =>
    Array.from(screen.getByRole("meter", { name }).previousElementSibling!.children) as HTMLElement[];
  const mirrored = [...[...micBands].reverse(), ...micBands];
  expect(bars("내 마이크").map((b) => b.style.transform)).toEqual(mirrored.map((b) => `scaleY(${b / 100})`));
  expect(bars("시스템 오디오")).toHaveLength(48);
  expect(bars("시스템 오디오").every((b) => b.style.transform === "scaleY(0)")).toBe(true);
  expect(bars("내 마이크")[0].parentElement!.getAttribute("aria-hidden")).toBe("true");
  // 0.1 RMS = -20 dBFS → 40 of 60 dB.
  expect(screen.getByRole("meter", { name: "내 마이크" }).getAttribute("aria-valuenow")).toBe("67");
  expect(screen.getByRole("meter", { name: "시스템 오디오" }).getAttribute("aria-valuenow")).toBe("0");
  expect(screen.getByText(/권한 허용을 기다리는 중/)).toBeTruthy();
});

test("a dismissed warning stays hidden until it is sent again", async () => {
  mockIPC((cmd) => (cmd === "recording_state" ? state : undefined), { shouldMockEvents: true });
  render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);

  // Standing warnings from before the screen opened are shown too.
  expect(await screen.findByText(state.warnings[0].message)).toBeTruthy();
  const mic: Warning = { kind: "mic_permission", message: "마이크 소리가 들어오지 않습니다" };
  await act(() => emit("recording://warning", mic));
  expect(screen.getAllByRole("button", { name: "닫기" })).toHaveLength(2);

  fireEvent.click(screen.getAllByRole("button", { name: "닫기" })[1]);
  expect(screen.queryByText(mic.message)).toBeNull();
  expect(screen.getByText(state.warnings[0].message)).toBeTruthy();

  await act(() => emit("recording://warning", mic));
  expect(screen.getByText(mic.message)).toBeTruthy();
});

// SUM-07: shown while recording, before anything is sent.
test("tells where this meeting's summary will go", async () => {
  const asked: unknown[] = [];
  mockIPC(
    (cmd, args) => {
      if (cmd === "recording_state") return state;
      if (cmd === "summary_route") {
        asked.push(args);
        return { connection: null, local: false, local_available: false, locked: false };
      }
    },
    { shouldMockEvents: true },
  );
  render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);

  expect(await screen.findByText("LLM 연결이 없어 전사까지만 합니다")).toBeTruthy();
  expect(asked).toEqual([{ id: state.id }]);
});

// Design-system 6.3: leaving for Home keeps recording; only the stop button (or ⌘.) stops it.
test("← 홈 leaves the recording running; 녹음 종료 asks to stop", async () => {
  const calls: string[] = [];
  mockIPC(
    (cmd) => {
      calls.push(cmd);
      if (cmd === "recording_state") return state;
    },
    { shouldMockEvents: true },
  );
  let home = 0;
  let stops = 0;
  render(
    <Recording
      startedAt="2026-10-14T05:00:00Z"
      stopping={false}
      onHome={() => home++}
      onStop={() => stops++}
    />,
  );

  expect(await screen.findByText("녹음은 계속돼요")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1 }).textContent).toMatch(/^녹음 중 · \d{2}:\d{2} 시작$/);
  fireEvent.click(screen.getByRole("button", { name: "홈" }));
  expect(home).toBe(1);
  expect(stops).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "녹음 종료" }));
  expect(stops).toBe(1);
  expect(calls).not.toContain("stop_recording");
});

// A start (⌘R) while this screen said no recording was running: the list's new start time asks again.
test("asks for the recording again when a new one starts", async () => {
  let current: RecordingState | null = null;
  mockIPC((cmd) => (cmd === "recording_state" ? current : undefined), { shouldMockEvents: true });
  const { rerender } = render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);
  expect(await screen.findByRole("heading", { level: 1, name: "진행 중인 녹음이 없습니다" })).toBeTruthy();

  current = { id: "새 회의", warnings: [] };
  rerender(<Recording startedAt="2026-10-14T05:00:00Z" stopping={false} onHome={() => {}} onStop={() => {}} />);
  expect(await screen.findByRole("heading", { level: 1, name: /^녹음 중 · \d{2}:\d{2} 시작$/ })).toBeTruthy();
  expect(screen.getByRole("button", { name: "녹음 종료" })).toBeTruthy();
});
