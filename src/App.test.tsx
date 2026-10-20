import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { MotionGlobalConfig } from "motion/react";
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
    { file: WHISPER, size: 874_188_075, installed, received: 0, downloading: false },
    { file: "ggml-silero-v6.2.0.bin", size: 885_098, installed: true, received: 0, downloading: false },
  ],
  downloading: false,
});

// What App asks on every start, unless the handler says otherwise: complete settings, installed
// models and an empty line, so no setup card shows by accident.
const mock = (handler: (cmd: string, args: Record<string, unknown>) => unknown) =>
  mockIPC(
    (cmd, args) => {
      const defaults: Record<string, unknown> = {
        get_settings: loaded,
        model_status: models(true),
        list_connections: [
          { name: "회사 클라우드", label: "회사 클라우드", host: "api.example.com", is_local: false, builtin: false },
          { name: "builtin", label: "이 PC", host: "", is_local: true, builtin: true },
        ],
        queue_state: [],
      };
      return handler(cmd, args as Record<string, unknown>) ?? defaults[cmd];
    },
    { shouldMockEvents: true },
  );

const loaded: LoadedSettings = {
  read_only: false,
  rename: [],
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

// With no meetings at all, Home is the first screen (design-system 6.5).
const FIRST = "회의가 끝나면, 기록은 이미 와 있어요.";
const first = () => screen.findByRole("heading", { level: 1, name: FIRST });
/** Which way the screen holding this heading came in. */
const cameIn = (heading: HTMLElement) => heading.closest("[data-direction]")?.getAttribute("data-direction");

// Design-system 6.1: forward from Home, back to it.
test("switches from home to settings and back, forward and then back", async () => {
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
  });

  render(<App />);

  expect(await first()).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "설정" }));

  expect(await screen.findByDisplayValue("홍길동")).toBeTruthy();
  expect(cameIn(screen.getByRole("heading", { level: 1, name: "설정" }))).toBe("forward");
  fireEvent.click(screen.getByRole("button", { name: "홈으로" }));

  expect(cameIn(await first())).toBe("back");
  // Nothing recording or processing: no dock.
  expect(screen.queryByRole("button", { name: /^(녹음|처리) 중/ })).toBeNull();
});

// A key on a button that leaves with its screen: focus lands on the new screen's title, never the
// page. A button that stays (the header's) keeps it. Opening the app moves nothing, even in
// StrictMode's double run.
test("a screen changed from the keyboard takes the focus its button had", async () => {
  mock((cmd) => {
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
  });
  render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
  await first();
  expect(document.activeElement).toBe(document.body);

  const gear = screen.getByRole("button", { name: "설정" });
  gear.focus();
  fireEvent.click(gear, { detail: 0 });
  const back = await screen.findByRole("button", { name: "홈으로" });
  expect(document.activeElement).toBe(gear);

  back.focus();
  fireEvent.click(back, { detail: 0 });
  const heading = await first();
  await waitFor(() => expect(document.activeElement).toBe(heading));
});

test("opens on Home with empty settings; the setup line opens the matching tab", async () => {
  const empty: LoadedSettings = {
    ...loaded,
    settings: { ...loaded.settings, llm_connections: [], default_llm: null, webhooks: [] },
  };
  mock((cmd) => {
    if (cmd === "get_settings") return empty;
    if (cmd === "list_meetings") return [];
  });

  render(<App />);

  const items = within(await screen.findByRole("list", { name: "남은 설정" }));
  expect(await items.findByRole("button", { name: "Slack 채널 연결" })).toBeTruthy();
  // This PC summarizes with no connection (plan 5.8): its model is the model item's, here installed.
  expect(items.queryByRole("button", { name: "LLM 연결" })).toBeNull();
  expect(screen.getByRole("heading", { level: 1, name: FIRST })).toBeTruthy();
  // Recording needs no setup: no tour, no gate.
  expect((screen.getByRole("button", { name: "첫 회의 녹음하기" }) as HTMLButtonElement).disabled).toBe(false);

  fireEvent.click(screen.getByRole("button", { name: "Slack 채널 연결" }));
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

/** A recording running now: its start is the dock's first time. */
const live = (id: string): MeetingItem => ({ ...meeting(id, "recording"), created_at: new Date().toISOString() });
const RECORDING_HEADING = /^녹음 중 · \d{2}:\d{2} 시작$/;
// The dock's first second, give or take one for a slow run.
const LIVE = /^녹음 중 00:0\d$/;

// Design-system 6.2: the dock records, processes, or is gone.
test("the dock shows the recording time and opens the recording screen, where it hides", async () => {
  const state: RecordingState = { id: "지금 회의", warnings: [] };
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [live("지금 회의"), meeting("지난 회의", "recorded")];
    if (cmd === "recording_state") return state;
  });

  render(<App />);

  expect(await screen.findByRole("button", { name: LIVE })).toBeTruthy();
  // Recording stops all workers (TRN-09), so processing gives way.
  expect(screen.queryByRole("button", { name: /^처리 중/ })).toBeNull();
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
  expect(await screen.findByRole("heading", { level: 1, name: RECORDING_HEADING })).toBeTruthy();
  expect(await screen.findByRole("button", { name: /녹음 종료/ })).toBeTruthy();
  // The recording screen shows all of it itself: one stop button, no dock.
  await waitFor(() => expect(screen.queryByRole("button", { name: /^녹음 중/ })).toBeNull());
  expect(screen.getAllByRole("button", { name: /녹음 종료/ })).toHaveLength(1);
});

test("the dock's 종료 stops the recording where the recorder is, then turns to processing", async () => {
  const calls: string[] = [];
  let list = [live("a"), meeting("지난 회의", "sent")];
  mock((cmd) => {
    calls.push(cmd);
    if (cmd === "list_meetings") return list;
    if (cmd === "stop_recording") {
      list = [{ ...list[0], status: "recorded" }, list[1]];
      return "a";
    }
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "녹음 종료" }));

  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 대기 중" })).toBeTruthy();
  expect(calls).toContain("stop_recording");
  // Still Home: stopping from the dock does not leave the screen.
  expect(screen.getByRole("button", { name: "녹음 시작" })).toBeTruthy();
});

test("the dock counts the line: meetings being processed and waiting, nothing else", async () => {
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

  // One running (a) and three waiting (j, b, d), the one with a place first.
  expect(await screen.findByRole("button", { name: "처리 중 4 · a · 전사 중" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^녹음 중/ })).toBeNull();
});

test("only waiting: the dock names the head of the line and opens it", async () => {
  mock((cmd, args) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [meeting("a", "recorded")];
    if (cmd === "get_meeting") return { meta: meeting(args.id as string, "recorded"), summary: "", sending: false };
  });

  render(<App />);

  fireEvent.click(await screen.findByRole("button", { name: "처리 중 1 · a · 대기 중" }));
  // Its document, still being written.
  expect(await screen.findByRole("heading", { level: 1, name: "a" })).toBeTruthy();
  expect(await screen.findByText("대기 중")).toBeTruthy();
});

test("a recording stop turns the dock from recording to processing, said once", async () => {
  let list = [live("a")];
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return list;
  });

  render(<App />);
  expect(await screen.findByRole("button", { name: LIVE })).toBeTruthy();
  const said = screen.getAllByRole("status").find((s) => s.closest(".fixed"))!;
  // Not when the app opens on it: nothing changed.
  expect(said.textContent).toBe("");

  list = [{ ...list[0], status: "recorded" }];
  await act(() => emit("recording://stopped", { id: "a", message: null }));

  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 대기 중" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^녹음 중/ })).toBeNull();
  expect(said.textContent).toBe("녹음을 마쳤습니다. 처리를 시작합니다");
});

// A send may end after its review screen closed: the list must not keep it unsent.
test("a finished send refreshes the list", async () => {
  let list = [meeting("a", "review")];
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return list;
  });

  render(<App />);
  expect(await screen.findByText("검토 대기")).toBeTruthy();

  list = [meeting("a", "sent")];
  await act(() => emit("slack://finished", "a"));

  expect(await screen.findByText("전송 완료")).toBeTruthy();
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
  expect(await first()).toBeTruthy();
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

test("the dock follows progress events and opens the meeting at the head of the line", async () => {
  const waiting = { status: "recorded" as const, chunk: 0, chunks: 0, percent: 0, eta_secs: null };
  let list = [meeting("a", "transcribing"), meeting("b", "recorded"), meeting("c", "recorded")];
  mock((cmd, args) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return list;
    if (cmd === "queue_state") {
      return [progress("a"), progress("b", { ...waiting, position: 1 }), progress("c", { ...waiting, position: 2 })];
    }
    if (cmd === "get_meeting") return { meta: list.find((m) => m.id === args.id), summary: "", sending: false };
  });

  render(<App />);
  expect(await screen.findByRole("button", { name: "처리 중 3 · a · 전사 42%" })).toBeTruthy();
  expect(screen.getByText("대기 중 · 2번째")).toBeTruthy();

  // b starts: the status changed, so the list is read again.
  list = [meeting("a", "transcribing"), meeting("b", "transcribing"), meeting("c", "recorded")];
  await act(() => emit("meeting://progress", progress("b", { chunks: 4, percent: 17 })));
  await act(() => emit("meeting://progress", progress("c", { ...waiting, position: 1 })));
  await act(() => emit("meeting://progress", progress("a", { percent: 57 })));
  expect(await screen.findByRole("button", { name: "처리 중 3 · a · 전사 57%" })).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "설정" }));
  expect(await screen.findByRole("heading", { level: 1, name: "설정" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "처리 중 3 · a · 전사 57%" }));
  expect(await screen.findByRole("heading", { level: 1, name: "a" })).toBeTruthy();
  expect(await screen.findByRole("progressbar", { name: "전사 진행률" })).toBeTruthy();
});

// While summarizing, `percent` is requests done; one request has nothing to count.
test("the dock counts a summary as running, with its requests only past one", async () => {
  const summarizing = { status: "summarizing" as const, eta_secs: null };
  mock((cmd) => {
    if (cmd === "list_meetings") return [meeting("a", "summarizing")];
    if (cmd === "queue_state") return [progress("a", { ...summarizing, chunk: 0, chunks: 1, percent: 0 })];
  });

  render(<App />);
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 요약 중" })).toBeTruthy();

  // Plan 5.3: the built-in engine loading first.
  const since = Math.floor(Date.now() / 1000);
  await act(() => emit("meeting://progress", progress("a", { ...summarizing, chunk: 0, chunks: 0, percent: 0, engine_since: since })));
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 요약 엔진 준비 중" })).toBeTruthy();

  await act(() => emit("meeting://progress", progress("a", { ...summarizing, chunk: 1, chunks: 3, percent: 33 })));
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 요약 2/3" })).toBeTruthy();
});

test("the setup line's model item comes first; after a failed download 전사 설정 opens the 전사 tab", async () => {
  mock((cmd) => {
    if (cmd === "get_settings") return { ...loaded, settings: { ...loaded.settings, webhooks: [] } };
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models(false);
    if (cmd === "download_models") throw new Error("연결이 끊겼습니다");
  });

  render(<App />);

  const items = within(await screen.findByRole("list", { name: "남은 설정" }));
  await items.findByRole("button", { name: "Slack 채널 연결" });
  await items.findByRole("button", { name: "모델 받기" });
  const rows = items.getAllByRole("listitem").map((li) => li.firstChild?.textContent);
  expect(rows).toEqual(["모델 받기", "Slack 채널 연결"]);
  fireEvent.click(items.getByRole("button", { name: "모델 받기" }));
  expect(await items.findByText("모델을 받지 못했습니다")).toBeTruthy();
  fireEvent.click(items.getByRole("button", { name: "전사 설정" }));

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
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 전사 중" })).toBeTruthy();
  await act(() => emit("meeting://progress", progress("a", { percent: 57 })));
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 전사 57%" })).toBeTruthy();

  await waitFor(() => expect(snapshot).not.toBeNull());
  await act(async () => snapshot?.([progress("a", { percent: 40 })]));
  expect(screen.getByRole("button", { name: "처리 중 1 · a · 전사 57%" })).toBeTruthy();
});

test("a GPU fallback shows a dismissible warning", async () => {
  const message = "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다";
  mock((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "list_meetings") return [];
  });

  render(<App />);
  expect(await first()).toBeTruthy();
  await act(() => emit("transcribe://gpu-failed", { message }));

  expect(await screen.findByText(message)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "닫기" }));
  // Gone once its exit has run (instant in tests).
  await waitFor(() => expect(screen.queryByText(message)).toBeNull());
});

// APP-03: a notification click opens its meeting's document, as it is now: it may have moved on.
test("a notification click opens its meeting: the summary when ready, the retry when it failed", async () => {
  mock((cmd, args) => {
    if (cmd === "list_meetings") return [];
    if (cmd !== "get_meeting") return;
    if (args.id === "a") return { meta: meeting("a", "review"), summary: "요약" };
    if (args.id === "b") return { meta: meeting("b", "summarize_failed"), summary: "" };
    throw new Error(`회의를 찾을 수 없습니다: ${args.id}`);
  });

  render(<App />);
  expect(await first()).toBeTruthy();

  await act(() => emit("app://open-meeting", "a"));
  // The summary, as a document ready to edit.
  expect(await screen.findByRole("button", { name: "고치기" })).toBeTruthy();
  expect(screen.getByText("요약")).toBeTruthy();

  await act(() => emit("app://open-meeting", "b"));
  expect(await screen.findByRole("heading", { level: 1, name: "b" })).toBeTruthy();
  expect(await screen.findByRole("button", { name: "요약 다시 실행" })).toBeTruthy();

  await act(() => emit("app://open-meeting", "gone"));
  expect(await screen.findByText("회의를 열지 못했습니다: Error: 회의를 찾을 수 없습니다: gone")).toBeTruthy();
});

// A leaving screen stays mounted for its exit. The document's read that lands then (here as
// `review`) must not move the recorder from Home.
test("a screen on its way out cannot move the app", async () => {
  MotionGlobalConfig.skipAnimations = false;
  try {
    let status: Status = "summarizing";
    let hold = false;
    const held: ((detail: unknown) => void)[] = [];
    mock((cmd) => {
      if (cmd === "list_meetings") return [meeting("느린 회의", status)];
      if (cmd === "get_meeting") {
        if (hold) return new Promise((resolve) => held.push(resolve));
        return { meta: meeting("느린 회의", status), summary: "", sending: false };
      }
      if (cmd === "summary_route") return { connection: null, local: false, local_available: false, locked: true };
      if (cmd === "get_transcript") return [];
    });

    render(<App />);
    fireEvent.click(await screen.findByText("느린 회의"));
    expect(await screen.findByRole("heading", { level: 1, name: "느린 회의" })).toBeTruthy();
    hold = true;
    await act(() => emit("meeting://progress", progress("느린 회의", { status: "summarizing", eta_secs: null })));
    await waitFor(() => expect(held.length).toBe(1));

    fireEvent.click(screen.getByRole("button", { name: "홈으로" }));
    status = "review";
    await act(async () => held.forEach((resolve) => resolve({ meta: meeting("느린 회의", "review"), summary: "" })));
    // Past the exit (208 ms).
    await act(() => new Promise((resolve) => setTimeout(resolve, 400)));

    // Home alone, under today's date.
    const headings = screen.getAllByRole("heading", { level: 1 });
    expect(headings).toHaveLength(1);
    expect(headings[0].textContent).toMatch(/^\d+월 \d+일 \S+요일$/);
  } finally {
    MotionGlobalConfig.skipAnimations = true;
  }
});

// Design-system 6.2: one stack at the bottom, the send bar over the dock.
test("a meeting's send bar sits in the bottom stack above the dock, and leaves with its document", async () => {
  mock((cmd, args) => {
    if (cmd === "list_meetings") return [live("지금 회의"), meeting("a", "review")];
    if (cmd === "get_meeting") return { meta: meeting(args.id as string, "review"), summary: "요약", sending: false };
  });

  render(<App />);
  fireEvent.click(await screen.findByText("a"));

  const send = await screen.findByRole("button", { name: "#회의록으로 보내기" });
  const dock = screen.getByRole("button", { name: LIVE });
  expect(send.closest(".fixed")).toBe(dock.closest(".fixed"));
  expect(send.compareDocumentPosition(dock) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "홈으로" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull());
  expect(screen.getByRole("button", { name: LIVE })).toBeTruthy();
});

// ⌘R / ⌘. on macOS, Ctrl elsewhere: both modifiers are set so the test holds on either.
const press = (target: Element, code: "KeyR" | "Period") =>
  fireEvent.keyDown(target, { code, key: code === "KeyR" ? "r" : ".", ctrlKey: true, metaKey: true });

test("⌘R starts a recording from any screen, never while typing, in a dialog or recording; ⌘. stops it", async () => {
  const calls: string[] = [];
  let list: MeetingItem[] = [meeting("지난 회의", "sent")];
  mock((cmd, args) => {
    calls.push(cmd);
    if (cmd === "list_meetings") return list;
    if (cmd === "has_secret") return true;
    if (cmd === "start_recording") {
      list = [live("새 회의"), ...list];
      return { id: "새 회의", warnings: [] } satisfies RecordingState;
    }
    if (cmd === "recording_state") return { id: "새 회의", warnings: [] } satisfies RecordingState;
    if (cmd === "stop_recording") {
      list = [{ ...list[0], status: "recorded" }, ...list.slice(1)];
      return "새 회의";
    }
    if (cmd === "get_meeting") return { meta: list.find((m) => m.id === args.id), summary: "", sending: false };
  });
  const starts = () => calls.filter((c) => c === "start_recording").length;

  render(<App />);
  expect((await screen.findByRole("button", { name: "녹음 시작" })).getAttribute("aria-keyshortcuts")).toMatch(
    /^(Meta|Control)\+R$/,
  );
  fireEvent.click(screen.getByRole("button", { name: "설정" }));

  // Typing: the webview still never reloads, and nothing starts.
  expect(press(await screen.findByDisplayValue("홍길동"), "KeyR")).toBe(false);
  expect(starts()).toBe(0);
  await act(() => emit("app://quit-requested"));
  press(await screen.findByRole("alertdialog"), "KeyR");
  expect(starts()).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "취소" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

  expect(press(document.body, "KeyR")).toBe(false);
  // Settings keeps its unsaved draft: the recording shows in the dock there.
  fireEvent.click(await screen.findByRole("button", { name: LIVE }));
  expect(starts()).toBe(1);
  expect(await screen.findByRole("heading", { level: 1, name: RECORDING_HEADING })).toBeTruthy();
  // Already recording (the heading has its start from the list): a second start would only fail.
  press(document.body, "KeyR");
  expect(starts()).toBe(1);

  expect(press(document.body, "Period")).toBe(false);
  expect(await screen.findByRole("heading", { level: 1, name: "새 회의" })).toBeTruthy();
  expect(calls.filter((c) => c === "stop_recording")).toHaveLength(1);
});

// A recording that stops by itself before the list ever shows it must not leave 녹음 시작 off.
test("a start the list never showed ends with its recording://stopped; recording can start again", async () => {
  const calls: string[] = [];
  mock((cmd) => {
    calls.push(cmd);
    if (cmd === "list_meetings") return [meeting("지난 회의", "sent")];
    if (cmd === "start_recording") return { id: "짧은 회의", warnings: [] } satisfies RecordingState;
    if (cmd === "recording_state") return null;
    if (cmd === "get_meeting") return { meta: meeting("짧은 회의", "recorded"), summary: "", sending: false };
  });
  const starts = () => calls.filter((c) => c === "start_recording").length;

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "녹음 시작" }));
  expect(await screen.findByRole("heading", { level: 1, name: "진행 중인 녹음이 없습니다" })).toBeTruthy();
  // Still starting as far as the app knows: a second start waits.
  press(document.body, "KeyR");
  expect(starts()).toBe(1);

  await act(() => emit("recording://stopped", { id: "짧은 회의", message: "디스크 공간이 부족해 녹음을 멈췄습니다" }));
  expect(await screen.findByRole("heading", { level: 1, name: "짧은 회의" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "홈으로" }));
  expect(((await screen.findByRole("button", { name: "녹음 시작" })) as HTMLButtonElement).disabled).toBe(false);
  press(document.body, "KeyR");
  await waitFor(() => expect(starts()).toBe(2));
});

test("the dock bridges a stop with 전사를 시작합니다, keeps focus, and a second click stops nothing", async () => {
  const calls: string[] = [];
  let list = [live("a")];
  let finish = (_: string) => {};
  mock((cmd) => {
    calls.push(cmd);
    if (cmd === "list_meetings") return list;
    if (cmd === "stop_recording") return new Promise<string>((resolve) => (finish = resolve));
  });

  render(<App />);
  const button = await screen.findByRole("button", { name: "녹음 종료" });
  fireEvent.click(button);
  fireEvent.click(button);

  const bridge = await screen.findByText("전사를 시작합니다");
  expect(screen.queryByRole("button", { name: "녹음 종료" })).toBeNull();
  expect(calls.filter((c) => c === "stop_recording")).toHaveLength(1);
  // The pill keeps focus while its shape changes under it.
  expect(document.activeElement).toBe(bridge.closest("[tabindex]"));

  list = [{ ...list[0], status: "recorded" }];
  await act(async () => finish("a"));
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 대기 중" })).toBeTruthy();
});

test("a failed start says why and lets the recorder try again", async () => {
  mock((cmd) => {
    if (cmd === "list_meetings") return [meeting("지난 회의", "sent")];
    if (cmd === "start_recording") throw new Error("마이크를 열 수 없습니다");
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "녹음 시작" }));

  expect(await screen.findByText("녹음을 시작하지 못했습니다: Error: 마이크를 열 수 없습니다")).toBeTruthy();
  expect((screen.getByRole("button", { name: "녹음 시작" }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByRole("heading", { level: 1, name: RECORDING_HEADING })).toBeNull();
});

test("the dock's head of the line goes by place in line, not list order", async () => {
  const waiting = { status: "recorded" as const, chunk: 0, chunks: 0, percent: 0, eta_secs: null };
  mock((cmd) => {
    if (cmd === "list_meetings") return [meeting("b", "recorded"), meeting("c", "recorded")];
    if (cmd === "queue_state") return [progress("b", { ...waiting, position: 2 }), progress("c", { ...waiting, position: 1 })];
  });

  render(<App />);

  expect(await screen.findByRole("button", { name: "처리 중 2 · c · 대기 중" })).toBeTruthy();
});

test("an automatic stop on Home says why; the dock bridges it until the list shows it", async () => {
  let list = [live("a")];
  let hold = false;
  const held: (() => void)[] = [];
  mock((cmd) => {
    if (cmd !== "list_meetings") return;
    if (hold) return new Promise((resolve) => held.push(() => resolve(list)));
    return list;
  });

  render(<App />);
  expect(await screen.findByRole("button", { name: LIVE })).toBeTruthy();
  hold = true;
  list = [{ ...list[0], status: "recorded" }];
  await act(() => emit("recording://stopped", { id: "a", message: "디스크 공간이 부족해 녹음을 멈췄습니다" }));

  expect(await screen.findByText("디스크 공간이 부족해 녹음을 멈췄습니다")).toBeTruthy();
  // No 종료 for a recording already over, while the list catches up.
  expect(screen.getByText("전사를 시작합니다")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "녹음 종료" })).toBeNull();
  await act(async () => held.forEach((resolve) => resolve()));
  expect(await screen.findByRole("button", { name: "처리 중 1 · a · 대기 중" })).toBeTruthy();
});

test("a shortcut waits while a select is open", async () => {
  const calls: string[] = [];
  mock((cmd) => {
    calls.push(cmd);
    if (cmd === "list_meetings") return [];
    if (cmd === "has_secret") return true;
    if (cmd === "list_audio_devices") return { inputs: [{ id: "mic", name: "마이크 1" }], outputs: [] };
  });

  render(<App />);
  await first();
  fireEvent.click(screen.getByRole("button", { name: "설정" }));
  fireEvent.click(await screen.findByRole("button", { name: "오디오" }));
  fireEvent.keyDown(screen.getByRole("combobox", { name: "마이크" }), { key: "ArrowDown" });
  expect(await screen.findByRole("listbox")).toBeTruthy();

  expect(press(document.body, "KeyR")).toBe(false);
  expect(calls).not.toContain("start_recording");
});

test("REC-12: a picked recording file becomes a meeting, and its document opens", async () => {
  const path = "/Users/hong/Downloads/주간 회의.m4a";
  const calls: { cmd: string; args: unknown }[] = [];
  let list = [meeting("지난 회의", "sent")];
  mock((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "list_meetings") return list;
    if (cmd === "plugin:dialog|open") return path;
    if (cmd === "import_recording") {
      list = [meeting("가져온 회의", "recorded"), ...list];
      return list[0];
    }
    if (cmd === "get_meeting") return { meta: list[0], summary: "", sending: false };
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "녹음 파일 가져오기" }));

  expect(await screen.findByRole("heading", { level: 1, name: "가져온 회의" })).toBeTruthy();
  expect(calls.find((c) => c.cmd === "import_recording")?.args).toEqual({ path });
  expect(calls.find((c) => c.cmd === "plugin:dialog|open")?.args).toMatchObject({
    options: { filters: [{ extensions: expect.arrayContaining(["mp3", "m4a", "wav"]) }] },
  });
});

test("REC-12: a cancelled pick does nothing; a file that cannot be read says why, wherever the recorder is", async () => {
  let picked: string | null = null;
  const calls: string[] = [];
  mock((cmd) => {
    calls.push(cmd);
    if (cmd === "list_meetings") return [meeting("지난 회의", "sent")];
    if (cmd === "plugin:dialog|open") return picked;
    if (cmd === "import_recording") throw "지원하지 않는 오디오 형식입니다";
  });

  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "녹음 파일 가져오기" }));
  await waitFor(() => expect(calls).toContain("plugin:dialog|open"));
  expect(calls).not.toContain("import_recording");

  picked = "/Users/hong/notes.mp3";
  fireEvent.click(screen.getByRole("button", { name: "녹음 파일 가져오기" }));
  // Off to settings meanwhile: the failure follows.
  fireEvent.click(screen.getByRole("button", { name: "설정" }));
  expect(await screen.findByText("녹음 파일을 가져오지 못했습니다: 지원하지 않는 오디오 형식입니다")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1, name: "설정" })).toBeTruthy();
});
