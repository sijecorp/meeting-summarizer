import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
    { file: WHISPER, size: 874_188_075, installed, received: 0, downloading: false },
    { file: "ggml-silero-v6.2.0.bin", size: 885_098, installed: true, received: 0, downloading: false },
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

const props = {
  listFailed: false,
  recording: false,
  starting: false,
  onChanged: () => {},
  onOpen: () => {},
  onRecord: () => {},
  onStart: () => {},
  onSetup: () => {},
};

const home = (meetings: MeetingItem[], queue: Progress[], onChanged = () => {}) =>
  render(
    <Home
      {...props}
      meetings={meetings}
      progress={Object.fromEntries(queue.map((p) => [p.id, p]))}
      onChanged={onChanged}
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

test("a summarizing row shows 요약 중; a failed summary shows its error, retried on its own screen", () => {
  mock();
  home(
    [meeting("a", "summarizing"), meeting("b", "summarize_failed", "LLM 사용 한도를 넘었습니다(HTTP 429)")],
    [progress("a", { status: "summarizing", chunk: 1, chunks: 3, percent: 33 })],
  );

  expect(screen.getByRole("progressbar", { name: "요약 진행률 (a)" }).getAttribute("aria-valuenow")).toBe("33");
  expect(screen.getByText("요약 중 · 요청 2/3")).toBeTruthy();
  expect(screen.getByText("LLM 사용 한도를 넘었습니다(HTTP 429)")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^다시 시도/ })).toBeNull();
});

// ONB-01: the setup line downloads right away; cancel, import and the mirror stay in the 전사 tab.
test("the setup line's 모델 받기 downloads with progress and checks the files again", async () => {
  let installed = false;
  mock((cmd) => {
    if (cmd === "model_status") return models(installed);
    if (cmd === "download_models") return new Promise<void>((resolve) => (release = resolve));
  });
  let opened = "";
  render(<Home {...props} meetings={[]} progress={{}} onSetup={(tab) => (opened = tab)} />);

  // Before a download there is nothing to cancel: the 전사 tab's link waits.
  expect(screen.queryByRole("button", { name: "전사 설정" })).toBeNull();
  fireEvent.click(await screen.findByRole("button", { name: "모델 받기" }));
  expect(await screen.findByText("모델 받는 중 0%")).toBeTruthy();
  // While it runs, the item shows progress instead of the button.
  expect(screen.queryByRole("button", { name: "모델 받기" })).toBeNull();
  // The percent is the whole chain's: Whisper, VAD, then the summary model (E3).
  const all_total = 874_188_075 + 885_098 + 2_740_937_888;
  await act(() =>
    emit("model://download", {
      file: WHISPER,
      received: 524_512_845,
      total: 874_188_075,
      all_received: 524_512_845,
      all_total,
    }),
  );
  expect(await screen.findByText("모델 받는 중 14%")).toBeTruthy();
  await act(() =>
    emit("model://download", {
      file: "Qwen3.5-4B-Q4_K_M.gguf",
      received: 1_370_468_944,
      total: 2_740_937_888,
      all_received: 874_188_075 + 885_098 + 1_370_468_944,
      all_total,
    }),
  );
  expect(await screen.findByText("모델 받는 중 62%")).toBeTruthy();
  expect(screen.queryByText(/전사 모델 받는 중/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "전사 설정" }));
  expect(opened).toBe("transcription");

  installed = true;
  await act(async () => release());
  await waitFor(() => expect(screen.queryByText(/모델 받/)).toBeNull());
});

test("a failed download says so and 다시 받기 tries again; each link opens its tab", async () => {
  let downloads = 0;
  mock((cmd) => {
    if (cmd === "model_status") return models(false);
    if (cmd === "get_settings") return { settings: { llm_connections: [], webhooks: [] } };
    if (cmd === "download_models") {
      if (++downloads === 1) throw new Error("연결이 끊겼습니다");
      return new Promise<void>((resolve) => (release = resolve));
    }
  });
  const opened: string[] = [];
  render(<Home {...props} meetings={[]} progress={{}} onSetup={(tab) => opened.push(tab)} />);

  const items = within(await screen.findByRole("list", { name: "남은 설정" }));
  fireEvent.click(await items.findByRole("button", { name: "모델 받기" }));
  expect(await items.findByText("모델을 받지 못했습니다")).toBeTruthy();
  fireEvent.click(items.getByRole("button", { name: "전사 설정" }));
  fireEvent.click(items.getByRole("button", { name: "LLM 연결" }));
  fireEvent.click(items.getByRole("button", { name: "Slack 채널 연결" }));
  expect(opened).toEqual(["transcription", "llm", "slack"]);

  fireEvent.click(items.getByRole("button", { name: "다시 받기" }));
  expect(await items.findByText("모델 받는 중 0%")).toBeTruthy();
  expect(downloads).toBe(2);
  expect(items.queryByText("모델을 받지 못했습니다")).toBeNull();
});

test("with only the summary model missing, the item's settings link opens the 요약(LLM) tab", async () => {
  const summary = { file: "Qwen3.5-4B-Q4_K_M.gguf", size: 2_740_937_888, installed: false, received: 0, downloading: false };
  mock((cmd, args) => {
    if (cmd === "model_status") {
      const transcription = models(true);
      const { scope } = args as { scope?: string };
      return scope === "transcription" ? transcription : { ...transcription, files: [...transcription.files, summary] };
    }
    if (cmd === "download_models") throw new Error("연결이 끊겼습니다");
  });
  let opened = "";
  render(<Home {...props} meetings={[]} progress={{}} onSetup={(tab) => (opened = tab)} />);

  fireEvent.click(await screen.findByRole("button", { name: "모델 받기" }));
  expect(await screen.findByText("모델을 받지 못했습니다")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "전사 설정" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM) 설정" }));
  expect(opened).toBe("llm");
});

// Titles come from the summary: until then a row says what it is, never a column of "제목 없음".
test("an untitled row goes by its length, or as the recording running", () => {
  mock();
  const untitled = (id: string, status: Status, duration_ms: number, interrupted = false) => ({
    ...meeting(id, status),
    title: "",
    duration_ms,
    interrupted,
  });
  home(
    [
      untitled("a", "recording", 0),
      untitled("b", "transcribing", 42 * 60_000 + 30_000),
      untitled("c", "recorded", 75 * 60_000),
      untitled("d", "transcribe_failed", 20_000),
      untitled("e", "recording", 0, true),
    ],
    [],
  );

  const names = screen.getAllByRole("listitem").map((li) => li.querySelector(".text-body.truncate")?.textContent);
  expect(names).toEqual(["녹음 중인 회의", "42분 녹음", "1시간 15분 녹음", "1분 미만 녹음", "제목 없는 녹음"]);
  expect(screen.getByRole("button", { name: "다시 시도 (1분 미만 녹음)" })).toBeTruthy();
});

/** `days` before today at `hour`:00 local time. */
const ago = (days: number, hour: number) => {
  const d = new Date();
  d.setDate(d.getDate() - days);
  d.setHours(hour, 0, 0, 0);
  return d.toISOString();
};
const titles = (region: HTMLElement) =>
  within(region)
    .getAllByRole("listitem")
    .map((li) => li.querySelector(".text-body.truncate")?.textContent);

// Design-system 6.4: what needs the recorder on top, the rest by day, each meeting once.
test("groups meetings into 지금 할 일 and days, each once", () => {
  mock();
  const at = (title: string, status: Status, days: number, hour: number, patch: Partial<MeetingItem> = {}) => ({
    ...meeting(title, status),
    created_at: ago(days, hour),
    ...patch,
  });
  // Newest first, as `list_meetings` sends them.
  home(
    [
      at("녹음 회의", "recording", 0, 16),
      at("보낸 회의", "sent", 0, 15, { sent: { channel: "#회의록", at: ago(0, 15), edit_count: 0 } }),
      at("전사 회의", "transcribing", 0, 14),
      at("새 버전 회의", "review", 0, 13, { schema_version: 2 }),
      at("멈춘 회의", "transcribed", 0, 12),
      at("대기 회의", "recorded", 1, 17),
      at("실패 회의", "transcribe_failed", 1, 16, { error: "전사하지 못했습니다" }),
      at("보관 회의", "archived", 1, 15),
      at("검토 회의", "review", 2, 10),
      at("중단 회의", "recording", 3, 9, { interrupted: true }),
      at("옛 회의", "sent", 9, 9),
    ],
    [progress("전사 회의", { chunk: 1, chunks: 3, percent: 42 }), progress("대기 회의", { status: "recorded", position: 1 })],
  );

  const regions = screen.getAllByRole("region");
  expect(regions.map((r) => r.getAttribute("aria-label"))).toEqual([
    "지금 할 일",
    "오늘",
    "어제",
    expect.stringMatching(/^(\d+년 )?\d+월 \d+일 \S+요일$/),
  ]);
  expect(regions.map(titles)).toEqual([
    ["녹음 회의", "전사 회의", "대기 회의", "실패 회의", "검토 회의", "중단 회의"],
    ["보낸 회의", "새 버전 회의", "멈춘 회의"],
    ["보관 회의"],
    ["옛 회의"],
  ]);
  for (const title of regions.flatMap(titles)) expect(screen.getAllByText(title!)).toHaveLength(1);
  // Under 지금 할 일, a meeting from another day says which.
  expect(within(screen.getByText("대기 회의").closest("li")!).getByText("어제")).toBeTruthy();
  expect(within(screen.getByText("보낸 회의").closest("li")!).getByText("#회의록으로 보냄")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 2, name: "지금 할 일 6" })).toBeTruthy();
});

// One action per row, by state (design-system 6.4); the row itself opens the meeting.
test("each row offers the one action its state needs", async () => {
  const calls: { cmd: string; args: unknown }[] = [];
  mock((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "resume_meeting") return meeting("b", "recorded");
  });
  const opened: string[] = [];
  let recorded = 0;
  render(
    <Home
      {...props}
      meetings={[
        meeting("a", "recording"),
        { ...meeting("b", "recording"), interrupted: true },
        meeting("c", "review"),
        meeting("d", "transcribe_failed"),
        meeting("e", "summarize_failed"),
        meeting("f", "transcribing"),
        meeting("g", "recorded"),
        // Retried: it waits in line instead of offering 다시 시도 again.
        meeting("h", "transcribe_failed"),
        meeting("i", "sent"),
        meeting("j", "archived"),
        meeting("k", "transcribed"),
        { ...meeting("l", "review"), schema_version: 2 },
      ]}
      progress={{
        f: progress("f", { chunk: 1, chunks: 3, percent: 42 }),
        g: progress("g", { status: "recorded", position: 1 }),
        h: progress("h", { status: "transcribe_failed", position: 2 }),
      }}
      onOpen={(m) => opened.push(m.id)}
      onRecord={() => recorded++}
    />,
  );

  const actions = (title: string) => {
    const name = screen.getByText(title);
    return within(name.closest("li")!)
      .getAllByRole("button")
      .filter((b) => !b.contains(name))
      .map((b) => b.getAttribute("aria-label"));
  };
  expect(Object.fromEntries("abcdefghijkl".split("").map((t) => [t, actions(t)]))).toEqual({
    a: ["녹음 화면으로 (a)"],
    b: ["처리 계속 (b)", "삭제 (b)"],
    c: ["검토하기 (c)"],
    d: ["다시 시도 (d)"],
    e: ["요약 다시 실행 (e)"],
    f: [],
    g: [],
    h: [],
    i: [],
    j: [],
    k: [],
    l: [],
  });

  fireEvent.click(screen.getByRole("button", { name: "녹음 화면으로 (a)" }));
  expect(recorded).toBe(1);
  fireEvent.click(screen.getByRole("button", { name: "검토하기 (c)" }));
  // 요약 다시 실행 picks its connection on the meeting's own screen.
  fireEvent.click(screen.getByRole("button", { name: "요약 다시 실행 (e)" }));
  fireEvent.click(screen.getByText("f"));
  expect(opened).toEqual(["c", "e", "f"]);
  // An interrupted recording has no screen to open; 처리 계속 queues it.
  expect(screen.getByText("b").closest("button")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "처리 계속 (b)" }));
  await waitFor(() => expect(calls.find((c) => c.cmd === "resume_meeting")?.args).toEqual({ id: "b" }));
});

// Design-system 6.5: the first screen is Home with no meetings at all, and only then.
test("the first screen shows only while there are no meetings", () => {
  mock();
  let started = 0;
  const view = (meetings: MeetingItem[] | null, listFailed = false) => (
    <Home {...props} meetings={meetings} listFailed={listFailed} progress={{}} onStart={() => started++} />
  );
  const { rerender } = render(view([]));

  const record = screen.getByRole("button", { name: "첫 회의 녹음하기" });
  expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("회의가 끝나면, 기록은 이미 와 있어요.");
  expect(record.getAttribute("aria-keyshortcuts")).toMatch(/^(Meta|Control)\+R$/);
  fireEvent.click(record);
  expect(started).toBe(1);
  expect(screen.queryByRole("button", { name: "녹음 시작" })).toBeNull();

  rerender(view([meeting("a", "sent")]));
  expect(screen.queryByRole("button", { name: "첫 회의 녹음하기" })).toBeNull();
  expect(screen.getByRole("heading", { level: 1 }).textContent).toMatch(/^\d+월 \d+일 \S+요일$/);
  fireEvent.click(screen.getByRole("button", { name: "녹음 시작" }));
  expect(started).toBe(2);

  // Not known yet: neither, so the first screen never flashes before a full Home.
  rerender(view(null));
  expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
  // Unreadable: Home still offers recording.
  rerender(view(null, true));
  expect(screen.getByRole("button", { name: "녹음 시작" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "첫 회의 녹음하기" })).toBeNull();
});
