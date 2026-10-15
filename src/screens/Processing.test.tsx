import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { MeetingDetail, Meta, Progress, Segment } from "../lib/ipc";
import Processing from "./Processing";

afterEach(() => {
  cleanup();
  clearMocks();
});

const meta = (patch: Partial<Meta>): Meta => ({
  schema_version: 1,
  id: "2026-10-14T0500-a1b2",
  title: "주간 회의",
  status: "transcribed",
  created_at: "2026-10-14T05:00:03Z",
  duration_ms: 0,
  gaps: [],
  backend: "gpu",
  chunks_done: 2,
  llm_connection: null,
  summarize_local: false,
  no_speech: false,
  error: null,
  sent: null,
  ...patch,
});

// Architecture 10.4: a transcript is outside text and must stay plain text.
test("shows a transcribed meeting's transcript with times, HTML as text", async () => {
  const segments: Segment[] = [
    { start_ms: 3_000, end_ms: 9_000, text: "안녕하세요" },
    { start_ms: 4_515_000, end_ms: 4_520_000, text: "<img src=x onerror=alert(1)> <b>다음</b> 안건" },
  ];
  mockIPC((cmd, args) => {
    if (cmd === "get_meeting") return { meta: meta({}), summary: "" } satisfies MeetingDetail;
    if (cmd === "get_transcript" && (args as { id: string }).id === "2026-10-14T0500-a1b2") {
      return segments;
    }
  });

  const { container } = render(<Processing id="2026-10-14T0500-a1b2" onHome={() => {}} />);

  expect(await screen.findByText("<img src=x onerror=alert(1)> <b>다음</b> 안건")).toBeTruthy();
  expect(screen.getByText("안녕하세요")).toBeTruthy();
  expect(screen.getByText("00:03")).toBeTruthy();
  expect(screen.getByText("75:15")).toBeTruthy();
  expect(screen.getByText("GPU")).toBeTruthy();
  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("b")).toBeNull();
});

test("a running meeting shows its chunk, percent and time left", async () => {
  const running: Progress = {
    id: "a",
    status: "transcribing",
    chunk: 1,
    chunks: 5,
    percent: 42,
    eta_secs: 3_900,
    position: null,
  };
  mockIPC((cmd) => (cmd === "get_meeting" ? { meta: meta({ status: "transcribing" }), summary: "" } : undefined));

  render(<Processing id="a" progress={running} onHome={() => {}} />);

  expect(await screen.findByText("구간 2/5 · 42% · 약 1시간 5분 남음")).toBeTruthy();
  expect(screen.getByRole("progressbar", { name: "전사 진행률" }).getAttribute("aria-valuenow")).toBe("42");
});

test("다시 시도 puts a failed meeting back in line", async () => {
  const error = "메모리가 부족해 전사가 중단되었습니다";
  const failed = meta({ status: "transcribe_failed", error });
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "get_meeting") return { meta: failed, summary: "" };
    if (cmd === "resume_meeting") return failed;
  });
  const { rerender } = render(<Processing id="a" onHome={() => {}} />);

  expect(await screen.findByText(error)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
  await waitFor(() => expect(calls.find((c) => c.cmd === "resume_meeting")?.args).toEqual({ id: "a" }));

  // The queue's event gives it a place: no error and no second 다시 시도 while it waits.
  const queued: Progress = {
    id: "a",
    status: "transcribe_failed",
    chunk: 0,
    chunks: 0,
    percent: 0,
    eta_secs: null,
    position: 1,
  };
  rerender(<Processing id="a" progress={queued} onHome={() => {}} />);
  expect(await screen.findByText("대기 중 · 1번째")).toBeTruthy();
  expect(screen.queryByText(error)).toBeNull();
  expect(screen.queryByRole("button", { name: "다시 시도" })).toBeNull();
});
