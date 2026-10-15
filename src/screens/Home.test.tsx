import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import {
  modelDownload,
  type LoadedSettings,
  type MeetingItem,
  type ModelState,
  type Progress,
  type Status,
} from "../lib/ipc";
import Home from "./Home";

// The model download is app-wide (ipc.ts): a test that leaves one pending would hand it to the
// next. Settled here, then awaited. No clearMocks(): Home unlistens asynchronously after unmount.
let release = () => {};
afterEach(async () => {
  cleanup();
  release();
  await modelDownload();
});

const WHISPER = "ggml-large-v3-turbo-q8_0.bin";
const models = (installed: boolean): ModelState => ({
  files: [
    { file: WHISPER, size: 874_188_075, installed, received: 0 },
    { file: "ggml-silero-v6.2.0.bin", size: 885_098, installed: true, received: 0 },
  ],
  downloading: false,
});
// Home reads only whether these lists are empty.
const complete = { settings: { llm_connections: [{}], webhooks: [{}] } } as unknown as LoadedSettings;

// Complete settings and installed models unless the handler says otherwise, so no setup card shows
// by accident.
const mock = (handler: (cmd: string, args: unknown) => unknown = () => undefined) =>
  mockIPC(
    (cmd, args) => {
      const defaults: Record<string, unknown> = { get_settings: complete, model_status: models(true) };
      return handler(cmd, args) ?? defaults[cmd];
    },
    { shouldMockEvents: true },
  );

const meeting = (id: string, status: Status, error: string | null = null): MeetingItem => ({
  schema_version: 1,
  id,
  title: id,
  status,
  created_at: "2026-10-14T05:00:00Z",
  duration_ms: 0,
  gaps: [],
  backend: null,
  chunks_done: 0,
  llm_connection: null,
  summarize_local: false,
  no_speech: false,
  error,
  sent: null,
  interrupted: false,
});

const progress = (id: string, patch: Partial<Progress>): Progress => ({
  id,
  status: "transcribing",
  chunk: 0,
  chunks: 0,
  percent: 0,
  eta_secs: null,
  position: null,
  ...patch,
});

const home = (meetings: MeetingItem[], queue: Progress[], onChanged = () => {}) =>
  render(
    <Home
      meetings={meetings}
      progress={Object.fromEntries(queue.map((p) => [p.id, p]))}
      recording={false}
      onChanged={onChanged}
      onOpen={() => {}}
      onRecord={() => {}}
      onSetup={() => {}}
    />,
  );

test("rows show the running bar and time left, chunk planning, and places in line", () => {
  mock();
  home(
    [
      meeting("a", "transcribing"),
      meeting("b", "transcribing"),
      meeting("c", "recorded"),
      meeting("d", "transcribe_failed"),
      meeting("e", "transcribed"),
      meeting("f", "transcribed"),
    ],
    [
      progress("a", { chunk: 1, chunks: 3, percent: 42, eta_secs: 170 }),
      // Planning: its percent is not the meeting's.
      progress("b", { percent: 30 }),
      progress("c", { status: "recorded", position: 1 }),
      // A retried failure waits in line instead of offering 다시 시도 again.
      progress("d", { status: "transcribe_failed", position: 2 }),
      // In line for its summary.
      progress("e", { status: "transcribed", position: 3 }),
      // No place in line: stopped here for want of an LLM connection.
      progress("f", { status: "transcribed" }),
    ],
  );

  const bars = screen.getAllByRole("progressbar");
  expect(bars.map((b) => [b.getAttribute("aria-label"), b.getAttribute("aria-valuenow")])).toEqual([
    ["전사 진행률 (a)", "42"],
    ["구간 나누기 진행률 (b)", "30"],
  ]);
  expect(screen.getByText("42% · 약 3분 남음")).toBeTruthy();
  expect(screen.getByText("구간 나누는 중")).toBeTruthy();
  expect(screen.getByText("대기 중 · 1번째")).toBeTruthy();
  expect(screen.getByText("대기 중 · 2번째")).toBeTruthy();
  expect(screen.getByText("대기 중 · 3번째")).toBeTruthy();
  expect(screen.getByText("전사 완료")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^다시 시도/ })).toBeNull();
});

test("a failed row shows the error and 다시 시도 queues it", async () => {
  const calls: { cmd: string; args: unknown }[] = [];
  mock((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "resume_meeting") return meeting("a", "transcribe_failed");
  });
  let changed = 0;
  home([meeting("a", "transcribe_failed", "메모리가 부족해 전사가 중단되었습니다")], [], () => changed++);

  expect(screen.getByText("메모리가 부족해 전사가 중단되었습니다")).toBeTruthy();
  // Named after its meeting: a list can have several.
  fireEvent.click(screen.getByRole("button", { name: "다시 시도 (a)" }));

  await waitFor(() => expect(changed).toBe(1));
  expect(calls.find((c) => c.cmd === "resume_meeting")?.args).toEqual({ id: "a" });
});

// ONB-01: the setup card downloads right away; cancel, import and the mirror stay in the 전사 tab.
test("the setup card's 모델 받기 downloads with progress and checks the files again", async () => {
  let installed = false;
  mock((cmd) => {
    if (cmd === "model_status") return models(installed);
    if (cmd === "download_models") return new Promise<void>((resolve) => (release = resolve));
  });
  let opened = "";
  render(
    <Home
      meetings={[]}
      progress={{}}
      recording={false}
      onChanged={() => {}}
      onOpen={() => {}}
      onRecord={() => {}}
      onSetup={(tab) => (opened = tab)}
    />,
  );

  fireEvent.click(await screen.findByRole("button", { name: "모델 받기" }));
  expect(await screen.findByText("전사 모델을 받는 중 0%")).toBeTruthy();
  // While it runs, the row shows progress instead of the button.
  expect(screen.queryByRole("button", { name: "모델 받기" })).toBeNull();
  await act(() => emit("model://download", { file: WHISPER, received: 524_512_845, total: 874_188_075 }));
  expect(await screen.findByText("전사 모델을 받는 중 60%")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "전사 설정" }));
  expect(opened).toBe("transcription");

  installed = true;
  await act(async () => release());
  await waitFor(() => expect(screen.queryByText(/전사 모델/)).toBeNull());
});
