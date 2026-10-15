import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { LoadedSettings, MeetingDetail, Meta, Progress, Segment } from "../lib/ipc";
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

const handlers = { onHome: () => {}, onReview: () => {}, onSetup: () => {} };

const progress = (patch: Partial<Progress>): Progress => ({
  id: "a",
  status: "summarizing",
  chunk: 0,
  chunks: 0,
  percent: 0,
  eta_secs: null,
  position: null,
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

  const { container } = render(<Processing id="2026-10-14T0500-a1b2" {...handlers} />);

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

  render(<Processing id="a" progress={running} {...handlers} />);

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
  const { rerender } = render(<Processing id="a" {...handlers} />);

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
  rerender(<Processing id="a" progress={queued} {...handlers} />);
  expect(await screen.findByText("대기 중 · 1번째")).toBeTruthy();
  expect(screen.queryByText(error)).toBeNull();
  expect(screen.queryByRole("button", { name: "다시 시도" })).toBeNull();
});

// Summarizing starts again at `chunks` 0, without any chunk planning.
test("a summarizing meeting shows 요약 중, with the request count and bar past one request", async () => {
  mockIPC((cmd) => (cmd === "get_meeting" ? { meta: meta({ status: "summarizing" }), summary: "" } : undefined));
  const { rerender } = render(<Processing id="a" progress={progress({})} {...handlers} />);
  const shown = () => screen.getByText("진행").nextElementSibling?.textContent;

  await waitFor(() => expect(shown()).toBe("요약 중"));
  // Nothing to count yet, so no empty bar.
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText("구간 나누는 중")).toBeNull();

  rerender(<Processing id="a" progress={progress({ chunk: 1, chunks: 3, percent: 33 })} {...handlers} />);
  expect(await screen.findByText("요약 중 · 요청 2/3")).toBeTruthy();
  expect(screen.getByRole("progressbar", { name: "요약 진행률" }).getAttribute("aria-valuenow")).toBe("33");

  // The last event comes before the meeting is read again: it is no chunk of a transcription.
  rerender(
    <Processing id="a" progress={progress({ status: "summarize_failed", chunk: 1, chunks: 1, percent: 100 })} {...handlers} />,
  );
  await waitFor(() => expect(screen.queryByRole("progressbar")).toBeNull());
  expect(screen.queryByText(/구간 \d/)).toBeNull();
});

test("switches to the review screen once the summary is ready", async () => {
  let status: Meta["status"] = "summarizing";
  mockIPC((cmd) => (cmd === "get_meeting" ? { meta: meta({ status }), summary: "" } : undefined));
  const reviewed: string[] = [];
  const props = { ...handlers, onReview: (id: string) => reviewed.push(id) };
  const { rerender } = render(<Processing id="a" progress={progress({})} {...props} />);
  expect(await screen.findByText("진행")).toBeTruthy();
  expect(reviewed).toEqual([]);

  status = "review";
  rerender(<Processing id="a" progress={progress({ status: "review" })} {...props} />);
  await waitFor(() => expect(reviewed).toEqual(["a"]));
});

const settings = (names: string[], defaultLlm: string | null) =>
  ({
    read_only: false,
    settings: {
      llm_connections: names.map((name) => ({
        name,
        base_url: "https://api.example.com/v1",
        is_local: name === "로컬",
      })),
      default_llm: defaultLlm,
    },
  }) as unknown as LoadedSettings;

// SUM-06: the meeting's own connection is gone, so the default is offered first; another can be
// picked. Each says where it sends the transcript (SUM-07).
test("요약 다시 실행 queues the failed summary with the picked connection", async () => {
  const error = "LLM 응답 시간 초과입니다. 연결을 확인하고 다시 실행해 주세요";
  const failed = meta({ status: "summarize_failed", error, llm_connection: "지운 연결" });
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "get_meeting") return { meta: failed, summary: "" };
    if (cmd === "get_settings") return settings(["회사 클라우드", "로컬"], "로컬");
    if (cmd === "retry_summary") return { ...failed, llm_connection: "회사 클라우드" };
  });
  render(<Processing id="a" {...handlers} />);

  expect(await screen.findByText(error)).toBeTruthy();
  const select = await screen.findByRole("combobox", { name: "요약할 LLM 연결" });
  await waitFor(() => expect(select.textContent).toBe("로컬 (이 PC)"));
  fireEvent.keyDown(select, { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: "회사 클라우드 (api.example.com로 전송)" }));
  await waitFor(() => expect(select.textContent).toBe("회사 클라우드 (api.example.com로 전송)"));
  // A key problem is fixed in settings, so that stays in reach too.
  expect(screen.getByRole("button", { name: "요약(LLM) 설정" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "요약 다시 실행" }));

  await waitFor(() =>
    expect(calls.find((c) => c.cmd === "retry_summary")?.args).toEqual({ id: "a", connection: "회사 클라우드" }),
  );
});

test("without a connection, the hint opens the 요약(LLM) tab", async () => {
  const opened: string[] = [];
  let status: Meta["status"] = "transcribed";
  mockIPC((cmd) => {
    if (cmd === "get_meeting") return { meta: meta({ status }), summary: "" };
    if (cmd === "get_settings") return settings([], null);
    if (cmd === "get_transcript") return [];
  });
  const props = { ...handlers, onSetup: (tab: string) => opened.push(tab) };
  // Read as `transcribed` while the last transcribing event is still here: summarizing comes next.
  const between = render(<Processing id="a" progress={progress({ status: "transcribing" })} {...props} />);
  await screen.findByText("전사 완료");
  expect(screen.queryByText("LLM 연결을 추가하면 요약합니다")).toBeNull();
  between.unmount();

  const first = render(<Processing id="a" {...props} />);
  expect(await screen.findByText("LLM 연결을 추가하면 요약합니다")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM) 설정" }));
  expect(opened).toEqual(["llm"]);
  first.unmount();

  // A failed summary whose connections were all removed.
  status = "summarize_failed";
  render(<Processing id="a" {...props} />);
  expect(await screen.findByText("LLM 연결을 추가하면 요약을 다시 실행할 수 있습니다")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "요약 다시 실행" })).toBeNull();
});
