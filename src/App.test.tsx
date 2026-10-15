import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import App from "./App";
import type {
  Level,
  LoadedSettings,
  MeetingItem,
  ModelState,
  Progress,
  RecordingState,
  Status,
} from "./lib/ipc";

// No clearMocks() here: App unlistens asynchronously after unmount and still needs the mocked
// event internals. Vitest gives each test file its own window.
afterEach(cleanup);

const WHISPER = "ggml-large-v3-turbo-q8_0.bin";
const models = (installed: boolean): ModelState => ({
  files: [
    { file: WHISPER, size: 874_188_075, installed, received: 0 },
    { file: "ggml-silero-v6.2.0.bin", size: 885_098, installed: true, received: 0 },
  ],
  downloading: false,
});

// What App asks on every start, unless the handler says otherwise: complete settings, installed
// models and an empty line, so no setup card shows by accident.
const mock = (handler: (cmd: string) => unknown) =>
  mockIPC(
    (cmd) => {
      const defaults: Record<string, unknown> = {
        get_settings: loaded,
        model_status: models(true),
        queue_state: [],
      };
      return handler(cmd) ?? defaults[cmd];
    },
    { shouldMockEvents: true },
  );

const loaded: LoadedSettings = {
  read_only: false,
  settings: {
    schema_version: 1,
    recorder_name: "홍길동",
    backend: "auto",
    gpu_failed: false,
    model: "large-v3-turbo-q8_0",
    model_base_url: "https://huggingface.co",
    input_device: null,
    output_device: null,
    llm_connections: [
      {
        name: "회사 클라우드",
        base_url: "https://api.example.com/v1",
        model: "m",
        context_tokens: 200000,
        is_local: false,
        timeout_secs: 300,
      },
    ],
    default_llm: "회사 클라우드",
    summarize_local_by_default: false,
    webhooks: [{ name: "#회의록" }],
    last_channel: null,
    glossary: [],
  },
};

test("switches from home to settings and back", async () => {
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
  });

  render(<App />);

  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "설정" }));

  expect(await screen.findByDisplayValue("홍길동")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1, name: "설정" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "홈으로" }));

  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1, name: "회의" })).toBeTruthy();
});

test("opens on Home with empty settings; the setup card opens the matching tab", async () => {
  const empty: LoadedSettings = {
    ...loaded,
    settings: { ...loaded.settings, llm_connections: [], default_llm: null, webhooks: [] },
  };
  mock((cmd) => {
    if (cmd === "get_settings") return empty;
    if (cmd === "list_meetings") return [];
  });

  render(<App />);

  expect(await screen.findByText("LLM 연결이 없습니다")).toBeTruthy();
  expect(screen.getByText("Slack 웹훅이 없습니다")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1, name: "회의" })).toBeTruthy();
  expect((screen.getByRole("button", { name: "녹음 시작" }) as HTMLButtonElement).disabled).toBe(false);

  fireEvent.click(screen.getByRole("button", { name: "LLM 연결하기" }));
  expect(await screen.findByRole("textbox", { name: "새 LLM 연결 이름" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "요약(LLM)" }).getAttribute("aria-current")).toBe("page");

  fireEvent.click(screen.getByRole("button", { name: "홈으로" }));
  fireEvent.click(await screen.findByRole("button", { name: "웹훅 추가하기" }));
  expect(await screen.findByRole("textbox", { name: "새 웹훅 이름" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Slack" }).getAttribute("aria-current")).toBe("page");
});

const meeting = (id: string, status: Status, interrupted = false): MeetingItem => ({
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
  error: null,
  sent: null,
  interrupted,
});

test("header chip shows the recording time and returns to the recording screen", async () => {
  const state: RecordingState = { id: "지금 회의", warnings: [] };
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") {
      return [meeting("지금 회의", "recording"), meeting("지난 회의", "recorded")];
    }
    if (cmd === "recording_state") return state;
  });

  render(<App />);

  expect(await screen.findByRole("button", { name: "녹음 중 00:00" })).toBeTruthy();
  // Recording stops all workers (TRN-09), so the processing counts give way.
  expect(screen.queryByText(/대기 \d/)).toBeNull();
  // Home goes back to the running recording instead of failing to start a second one.
  expect(screen.queryByRole("button", { name: "녹음 시작" })).toBeNull();
  expect(screen.getByRole("button", { name: "녹음 화면으로" })).toBeTruthy();
  const level: Level = {
    elapsed_ms: 65_400,
    mic: 0,
    system: 0,
    mic_bands: [],
    system_bands: [],
    waiting_permission: false,
  };
  await act(() => emit("recording://level", level));

  fireEvent.click(screen.getByRole("button", { name: "녹음 중 01:05" }));
  expect(await screen.findByRole("heading", { level: 1, name: "녹음" })).toBeTruthy();
  expect(await screen.findByRole("button", { name: /녹음 종료/ })).toBeTruthy();
});

test("header chip counts meetings being processed and waiting", async () => {
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") {
      return [
        meeting("a", "transcribing"),
        meeting("b", "recorded"),
        // No place in line: it stopped here for want of an LLM connection.
        meeting("c", "transcribed"),
        // In line for its summary (APP-07).
        meeting("j", "transcribed"),
        meeting("d", "paused"),
        meeting("e", "review"),
        // Not in line until 다시 시도.
        meeting("f", "transcribe_failed"),
        // Interrupted: offered on Home, not counted as recording or waiting.
        meeting("g", "recording", true),
        // From a newer app version: read-only, never processed here.
        { ...meeting("h", "transcribing"), schema_version: 2 },
        { ...meeting("i", "recording"), schema_version: 2 },
      ];
    }
    if (cmd === "queue_state") return [progress("j", { status: "transcribed", position: 1 })];
  });

  render(<App />);

  expect(await screen.findByText("처리 중 1 · 대기 3")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^녹음 중/ })).toBeNull();
});

test("header chip leaves out a zero count", async () => {
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [meeting("a", "recorded")];
  });

  render(<App />);

  expect(await screen.findByText("대기 1")).toBeTruthy();
});

test("a recording stop refreshes the chip", async () => {
  let list = [meeting("a", "recording")];
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return list;
  });

  render(<App />);
  expect(await screen.findByRole("button", { name: "녹음 중 00:00" })).toBeTruthy();

  list = [meeting("a", "recorded")];
  await act(() => emit("recording://stopped", { id: "a", message: null }));

  expect(await screen.findByText("대기 1")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^녹음 중/ })).toBeNull();
});

test("unreadable settings: the setup card leads to the error and the reset", async () => {
  let broken = true;
  mock((cmd) => {
    if (cmd === "get_settings") {
      if (broken) throw new Error("settings.json이 손상됐습니다");
      return loaded;
    }
    if (cmd === "reset_settings") {
      broken = false;
      return loaded;
    }
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
  });

  render(<App />);

  expect(await screen.findByText("설정 파일을 읽을 수 없습니다")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "설정 열기" }));
  expect(await screen.findByText(/settings\.json이 손상됐습니다/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "기본값으로 다시 시작" }));

  expect(await screen.findByDisplayValue("홍길동")).toBeTruthy();
  expect(screen.getByRole("button", { name: "일반" }).getAttribute("aria-current")).toBe("page");
});

test("quit dialog's 종료 button confirms the quit", async () => {
  const calls: string[] = [];
  mock((cmd) => {
    calls.push(cmd);
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [];
  });

  render(<App />);
  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  await act(() => emit("app://quit-requested"));

  const dialog = await screen.findByRole("alertdialog", { name: "진행 중인 작업이 있습니다" });
  expect(calls).not.toContain("confirm_quit");
  fireEvent.click(within(dialog).getByRole("button", { name: "종료" }));

  await waitFor(() => expect(calls).toContain("confirm_quit"));
});

const progress = (id: string, patch: Partial<Progress> = {}): Progress => ({
  id,
  status: "transcribing",
  chunk: 1,
  chunks: 3,
  percent: 42,
  eta_secs: 180,
  position: null,
  ...patch,
});

test("header chip adds the running percent, follows progress events and goes home", async () => {
  const waiting = { status: "recorded" as const, chunk: 0, chunks: 0, percent: 0, eta_secs: null };
  let list = [meeting("a", "transcribing"), meeting("b", "recorded"), meeting("c", "recorded")];
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return list;
    if (cmd === "queue_state") {
      return [progress("a"), progress("b", { ...waiting, position: 1 }), progress("c", { ...waiting, position: 2 })];
    }
  });

  render(<App />);
  expect(await screen.findByText("처리 중 1 (42%) · 대기 2")).toBeTruthy();
  expect(screen.getByText("대기 중 · 2번째")).toBeTruthy();

  // b starts: the status changed, so the list is read again.
  list = [meeting("a", "transcribing"), meeting("b", "transcribing"), meeting("c", "recorded")];
  await act(() => emit("meeting://progress", progress("b", { chunks: 4, percent: 17 })));
  await act(() => emit("meeting://progress", progress("c", { ...waiting, position: 1 })));
  expect(await screen.findByText("처리 중 2 (42%, 17%) · 대기 1")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "설정" }));
  expect(await screen.findByRole("heading", { level: 1, name: "설정" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "처리 중 2 (42%, 17%) · 대기 1" }));
  expect(await screen.findByRole("heading", { level: 1, name: "회의" })).toBeTruthy();
});

test("the setup card's model row comes first; 전사 설정 opens the 전사 tab", async () => {
  mock((cmd) => {
    if (cmd === "get_settings") return { ...loaded, settings: { ...loaded.settings, webhooks: [] } };
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models(false);
  });

  render(<App />);

  expect(await screen.findByText("전사 모델이 없습니다")).toBeTruthy();
  await screen.findByText("Slack 웹훅이 없습니다");
  const rows = screen.getAllByRole("listitem").map((li) => li.firstChild?.textContent);
  expect(rows).toEqual(["전사 모델이 없습니다", "Slack 웹훅이 없습니다"]);
  fireEvent.click(screen.getByRole("button", { name: "전사 설정" }));

  expect(await screen.findByText(WHISPER)).toBeTruthy();
  expect(screen.getByRole("button", { name: "전사" }).getAttribute("aria-current")).toBe("page");
});

// The snapshot can be older than an event that arrived first; a stale entry could stick forever.
test("progress events win over an older queue_state snapshot", async () => {
  let snapshot: ((list: Progress[]) => void) | null = null;
  mock((cmd) => {
    if (cmd === "list_meetings") return [meeting("a", "transcribing")];
    if (cmd === "queue_state") return new Promise((resolve) => (snapshot = resolve));
  });

  render(<App />);
  expect(await screen.findByText("처리 중 1")).toBeTruthy();
  await act(() => emit("meeting://progress", progress("a", { percent: 57 })));
  expect(await screen.findByText("처리 중 1 (57%)")).toBeTruthy();

  await waitFor(() => expect(snapshot).not.toBeNull());
  await act(async () => snapshot?.([progress("a", { percent: 40 })]));
  expect(screen.getByText("처리 중 1 (57%)")).toBeTruthy();
});

test("a GPU fallback shows a dismissible warning", async () => {
  const message = "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다";
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [];
  });

  render(<App />);
  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  await act(() => emit("transcribe://gpu-failed", { message }));

  expect(await screen.findByText(message)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "닫기" }));
  expect(screen.queryByText(message)).toBeNull();
});
