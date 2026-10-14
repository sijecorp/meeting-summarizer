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

test("shows elapsed time and levels from level events", async () => {
  mockIPC((cmd) => (cmd === "recording_state" ? state : undefined), { shouldMockEvents: true });
  render(<Recording onHome={() => {}} onStopped={() => {}} />);
  expect(await screen.findByText("00:00:00")).toBeTruthy();

  const level: Level = { elapsed_ms: 3_723_000, mic: 0.1, system: 0, waiting_permission: true };
  await act(() => emit("recording://level", level));

  expect(screen.getByText("01:02:03")).toBeTruthy();
  // 0.1 RMS = -20 dBFS → 40 of 60 dB.
  expect(screen.getByRole("meter", { name: "마이크" }).getAttribute("aria-valuenow")).toBe("67");
  expect(screen.getByRole("meter", { name: "시스템 오디오" }).getAttribute("aria-valuenow")).toBe("0");
  expect(screen.getByText(/권한 허용을 기다리는 중/)).toBeTruthy();
});

test("a dismissed warning stays hidden until it is sent again", async () => {
  mockIPC((cmd) => (cmd === "recording_state" ? state : undefined), { shouldMockEvents: true });
  render(<Recording onHome={() => {}} onStopped={() => {}} />);

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
