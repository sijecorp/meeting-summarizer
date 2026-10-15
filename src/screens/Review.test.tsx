import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { MeetingDetail, Meta, Segment } from "../lib/ipc";
import Review from "./Review";

// Autosave state outlives the screen (one per meeting id), so each test opens a meeting of its own.
let id = "";
let count = 0;
beforeEach(() => {
  id = `m${++count}`;
});

afterEach(() => {
  cleanup();
  clearMocks();
  vi.useRealTimers();
  // jsdom has no clipboard; a test that adds one takes it away again.
  delete (navigator as { clipboard?: Clipboard }).clipboard;
});

const meta = (patch: Partial<Meta> = {}): Meta => ({
  schema_version: 1,
  id,
  title: "주간 회의",
  status: "review",
  created_at: "2026-10-13T05:00:03Z",
  duration_ms: 0,
  gaps: [],
  backend: null,
  chunks_done: 0,
  llm_connection: "회사 클라우드",
  summarize_local: false,
  no_speech: false,
  error: null,
  sent: null,
  ...patch,
});

type Call = { cmd: string; args: Record<string, unknown> };

/** `get_meeting` answers with `detail`; `handler` answers the rest, and every call is kept. */
const mock = (detail: MeetingDetail, handler: (cmd: string, args: Call["args"]) => unknown = () => undefined) => {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    return cmd === "get_meeting" ? detail : handler(cmd, args as Call["args"]);
  });
  return calls;
};

/** Saves that wait until the test settles them, in the order they were sent. */
const holdSaves = () => {
  const held: { args: Call["args"]; resolve: (meta: Meta) => void; reject: (e: string) => void }[] = [];
  const handler = (cmd: string, args: Call["args"]) =>
    cmd === "save_summary" ? new Promise((resolve, reject) => held.push({ args, resolve, reject })) : undefined;
  return { held, handler };
};

/** Moves fake timers on and lets the IPC replies that follow land. */
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

const props = { onHome: () => {}, onNotice: () => {} };
const textbox = (name: string) => screen.getByRole("textbox", { name }) as HTMLInputElement;

// D6 / NFR-07: LLM output must stay plain text, never become markup, in the 미정 box too.
test("renders HTML in a summary as text", async () => {
  const summary = "## 액션 아이템\n- <img src=x onerror=alert(1)> 담당: 미정";
  mock({ meta: meta({ title: "<b>주간</b> 회의" }), summary });

  const { container } = render(<Review id={id} {...props} />);

  const textarea = (await screen.findByDisplayValue(summary, { normalizer: (s) => s })) as HTMLTextAreaElement;
  expect(textarea.tagName).toBe("TEXTAREA");
  expect(textarea.value).toBe(summary);
  expect(screen.getByDisplayValue("<b>주간</b> 회의")).toBeTruthy();
  expect(within(screen.getByRole("note")).getByText("- <img src=x onerror=alert(1)> 담당: 미정")).toBeTruthy();
  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("b")).toBeNull();
});

// SUM-10, eng review D8. REV-01: the transcript can still be read.
test("no speech: a warning and no editor; send stays disabled, only 전송 안 함 and 삭제 remain", async () => {
  mock({ meta: meta({ no_speech: true, llm_connection: null }), summary: "" });

  render(<Review id={id} {...props} />);

  expect(await screen.findByText("음성이 거의 감지되지 않았습니다")).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect((screen.getByRole("button", { name: "Slack으로 전송" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual([
    "홈으로",
    "Slack으로 전송",
    "전송 안 함",
    "삭제",
  ]);
  expect(screen.getByText("전사본 보기")).toBeTruthy();
});

test("the 미정 box lists only the lines with 미정, and follows edits", async () => {
  const summary = [
    "## 액션 아이템",
    "- [미정] 회의실 예약 (10/20)",
    "- [김철수] 자료 준비 (미정)",
    "- [이영희] 보고서 (10/22)",
    "## 미해결 질문",
    "- 없음",
  ].join("\n");
  mock({ meta: meta(), summary }, (cmd) => (cmd === "save_summary" ? meta() : undefined));

  render(<Review id={id} {...props} />);

  expect(await screen.findByText("담당자·기한이 미정인 항목 2개")).toBeTruthy();
  const items = () => within(screen.getByRole("note")).getAllByRole("listitem").map((li) => li.textContent);
  expect(items()).toEqual(["- [미정] 회의실 예약 (10/20)", "- [김철수] 자료 준비 (미정)"]);

  fireEvent.change(textbox("요약"), { target: { value: "- [이영희] 보고서 (10/22)" } });
  expect(screen.queryByRole("note")).toBeNull();
});

// REV-01: one save per pause, with what is typed last; leaving saves at once.
test("autosaves once, 800 ms after typing stops, and saves a pending edit on leaving", async () => {
  vi.useFakeTimers();
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd, args) =>
    cmd === "save_summary" ? meta({ title: args.title as string }) : undefined,
  );
  const saves = () => calls.filter((c) => c.cmd === "save_summary").map((c) => c.args);
  const { unmount } = render(<Review id={id} {...props} />);
  await tick();

  fireEvent.change(textbox("요약"), { target: { value: "첫 줄" } });
  await tick(500);
  fireEvent.change(textbox("요약"), { target: { value: "첫 줄\n둘째 줄" } });
  expect(screen.getByText("저장 중…")).toBeTruthy();
  await tick(799);
  expect(saves()).toEqual([]);
  await tick(1);
  expect(saves()).toEqual([{ id, title: "주간 회의", body: "첫 줄\n둘째 줄" }]);
  expect(screen.getByText("저장됨")).toBeTruthy();

  fireEvent.change(textbox("제목"), { target: { value: "새 제목" } });
  unmount();
  await tick();
  expect(saves()).toEqual([
    { id, title: "주간 회의", body: "첫 줄\n둘째 줄" },
    { id, title: "새 제목", body: "첫 줄\n둘째 줄" },
  ]);
  // Sent on leaving, so the debounce has nothing left to send.
  await tick(800);
  expect(saves()).toHaveLength(2);
});

test("saves go one at a time and in order, and only the latest reply shows", async () => {
  vi.useFakeTimers();
  const { held, handler } = holdSaves();
  mock({ meta: meta(), summary: "요약" }, handler);
  render(<Review id={id} {...props} />);
  await tick();

  fireEvent.change(textbox("요약"), { target: { value: "하나" } });
  await tick(800);
  fireEvent.change(textbox("요약"), { target: { value: "둘" } });
  await tick(800);
  // The second waits for the first, so "하나" can never land after "둘".
  expect(held.map((h) => h.args.body)).toEqual(["하나"]);

  // A stale reply: its title and its 저장됨 must not show.
  held[0].resolve(meta({ title: "옛 제목" }));
  await tick();
  expect(held.map((h) => h.args.body)).toEqual(["하나", "둘"]);
  expect(textbox("제목").value).toBe("주간 회의");
  expect(screen.queryByText("저장됨")).toBeNull();

  held[1].resolve(meta());
  await tick();
  expect(screen.getByText("저장됨")).toBeTruthy();
});

test("a failed edit goes again on leaving, the app says it failed, and reopening brings it back", async () => {
  vi.useFakeTimers();
  const { held, handler } = holdSaves();
  const calls = mock({ meta: meta(), summary: "요약" }, handler);
  const reads = () => calls.filter((c) => c.cmd === "get_meeting").length;
  const notices: string[] = [];
  const first = render(<Review id={id} onHome={() => {}} onNotice={(m) => notices.push(m)} />);
  await tick();

  fireEvent.change(textbox("요약"), { target: { value: "고친 요약" } });
  await tick(800);
  held[0].reject("디스크가 가득 찼습니다");
  await tick();
  expect(screen.getByText("저장하지 못했습니다: 디스크가 가득 찼습니다")).toBeTruthy();
  expect(textbox("요약").value).toBe("고친 요약");

  first.unmount();
  await tick();
  expect(held.map((h) => h.args.body)).toEqual(["고친 요약", "고친 요약"]);

  // Reopened while that save runs: the meeting is read only after it ends.
  render(<Review id={id} onHome={() => {}} onNotice={(m) => notices.push(m)} />);
  await tick();
  expect(reads()).toBe(1);
  held[1].reject("디스크가 가득 찼습니다");
  await tick();
  expect(notices).toEqual(["검토 화면에서 고친 내용을 저장하지 못했습니다: 디스크가 가득 찼습니다"]);
  expect(reads()).toBe(2);

  // The unsaved edit is back in the field, and goes again.
  expect(textbox("요약").value).toBe("고친 요약");
  expect(held.map((h) => h.args.body)).toEqual(["고친 요약", "고친 요약", "고친 요약"]);
  held[2].resolve(meta());
  await tick();
  expect(screen.getByText("저장됨")).toBeTruthy();
});

// The backend stores the title as one line (store.rs `save_summary`).
test("shows the stored one-line title, keeping a space typed at the end", async () => {
  vi.useFakeTimers();
  mock({ meta: meta(), summary: "" }, (cmd, args) =>
    cmd === "save_summary"
      ? meta({ title: (args.title as string).split(/\s+/).filter(Boolean).join(" ") })
      : undefined,
  );
  render(<Review id={id} {...props} />);
  await tick();

  fireEvent.change(textbox("제목"), { target: { value: "주간 " } });
  await tick(800);
  expect(screen.getByText("저장됨")).toBeTruthy();
  expect(textbox("제목").value).toBe("주간 ");

  fireEvent.change(textbox("제목"), { target: { value: "주간\t\t회의" } });
  await tick(800);
  expect(textbox("제목").value).toBe("주간 회의");
});

test("a failed save keeps the text, says so, and stops the export", async () => {
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "save_summary") throw new Error("summary.md 저장 실패");
  });
  render(<Review id={id} {...props} />);

  const body = (await screen.findByRole("textbox", { name: "요약" })) as HTMLTextAreaElement;
  fireEvent.change(body, { target: { value: "고친 요약" } });
  fireEvent.click(screen.getByRole("button", { name: ".md로 저장" }));

  expect(await screen.findByText(/저장하지 못했습니다: .*summary\.md 저장 실패/)).toBeTruthy();
  expect(body.value).toBe("고친 요약");
  expect(calls.map((c) => c.cmd)).not.toContain("export_markdown");
});

test(".md로 저장 saves a pending edit first, then shows the path; a cancel shows nothing", async () => {
  let path: string | null = null;
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "save_summary") return meta();
    if (cmd === "export_markdown") return path;
  });
  render(<Review id={id} {...props} />);

  fireEvent.click(await screen.findByRole("button", { name: ".md로 저장" }));
  await waitFor(() => expect(calls.map((c) => c.cmd)).toContain("export_markdown"));
  expect(screen.queryByText(/^저장했습니다/)).toBeNull();

  path = "/Users/me/주간 회의.md";
  fireEvent.change(screen.getByRole("textbox", { name: "요약" }), { target: { value: "고친 요약" } });
  fireEvent.click(screen.getByRole("button", { name: ".md로 저장" }));
  expect(await screen.findByText("저장했습니다: /Users/me/주간 회의.md")).toBeTruthy();
  expect(calls.map((c) => c.cmd).filter((c) => c !== "get_meeting")).toEqual([
    "export_markdown",
    "save_summary",
    "export_markdown",
  ]);
});

// REV-03: an archived meeting can never be sent, so it is confirmed first, and it takes the edit.
test("전송 안 함 asks first, waits for a pending edit's save, then archives and goes home", async () => {
  const { held, handler } = holdSaves();
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd, args) =>
    cmd === "archive_meeting" ? meta({ status: "archived" }) : handler(cmd, args),
  );
  let home = 0;
  render(<Review id={id} onHome={() => home++} onNotice={() => {}} />);

  fireEvent.change(await screen.findByRole("textbox", { name: "요약" }), { target: { value: "고친 요약" } });
  fireEvent.click(screen.getByRole("button", { name: "전송 안 함" }));
  const dialog = await screen.findByRole("alertdialog", { name: "전송하지 않을까요?" });
  expect(calls.map((c) => c.cmd)).not.toContain("archive_meeting");
  fireEvent.click(within(dialog).getByRole("button", { name: "전송 안 함" }));

  await waitFor(() => expect(held).toHaveLength(1));
  expect(held[0].args.body).toBe("고친 요약");
  expect(calls.map((c) => c.cmd)).not.toContain("archive_meeting");
  held[0].resolve(meta());

  await waitFor(() => expect(home).toBe(1));
  expect(calls.find((c) => c.cmd === "archive_meeting")?.args).toEqual({ id });
});

test("a delete drops a pending edit: nothing is saved afterwards and nothing is said", async () => {
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "delete_meeting" ? null : undefined));
  let home = 0;
  const notices: string[] = [];
  const { unmount } = render(<Review id={id} onHome={() => home++} onNotice={(n) => notices.push(n)} />);

  fireEvent.change(await screen.findByRole("textbox", { name: "요약" }), { target: { value: "지울 회의" } });
  fireEvent.click(screen.getByRole("button", { name: "삭제" }));
  const dialog = await screen.findByRole("alertdialog", { name: "회의를 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(home).toBe(1));
  unmount();
  // Past the autosave debounce.
  await new Promise((r) => setTimeout(r, 900));

  expect(calls.map((c) => c.cmd)).not.toContain("save_summary");
  expect(notices).toEqual([]);
});

// SUM-07: only a meeting whose transcript went to an LLM says where.
test("says where the transcript went only when it went somewhere", async () => {
  const cases: [Partial<Meta>, string | null][] = [
    [{}, "전사본을 회사 클라우드(으)로 보내 요약했습니다"],
    [{ llm_connection: "Ollama", summarize_local: true }, "이 PC의 Ollama(으)로 요약했습니다"],
    [{ llm_connection: null }, null],
    [{ no_speech: true }, null],
  ];
  for (const [patch, line] of cases) {
    mock({ meta: meta(patch), summary: "요약" });
    render(<Review id={id} {...props} />);
    await screen.findByRole("button", { name: "삭제" });
    expect(screen.queryByText(/요약했습니다$/)?.textContent ?? null).toBe(line);
    cleanup();
  }
});

// REV-05. The webview's clipboard can be missing or refuse.
test("copies the summary as Markdown, and says so when the clipboard fails", async () => {
  mock({ meta: meta(), summary: "## 결정사항\n- 없음" });
  render(<Review id={id} {...props} />);

  fireEvent.click(await screen.findByRole("button", { name: "요약 복사" }));
  expect(await screen.findByText("클립보드에 복사하지 못했습니다")).toBeTruthy();

  const writeText = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  fireEvent.click(screen.getByRole("button", { name: "요약 복사" }));
  expect(await screen.findByText("복사했습니다")).toBeTruthy();
  expect(writeText).toHaveBeenCalledWith("# 주간 회의\n\n## 결정사항\n- 없음");
});

test("전사본 보기 reads the transcript once, as text, and copies it with times", async () => {
  const segments: Segment[] = [
    { start_ms: 3_000, end_ms: 9_000, text: " 안녕하세요" },
    { start_ms: 4_515_000, end_ms: 4_520_000, text: "<b>다음</b> 안건" },
  ];
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_transcript" ? segments : undefined));
  const writeText = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  const { container } = render(<Review id={id} {...props} />);

  const summary = await screen.findByText("전사본 보기");
  expect(calls.map((c) => c.cmd)).not.toContain("get_transcript");
  fireEvent.click(summary);
  expect(await screen.findByText("<b>다음</b> 안건")).toBeTruthy();
  expect(screen.getByText("75:15")).toBeTruthy();
  expect(container.querySelector("b")).toBeNull();

  fireEvent.click(summary);
  fireEvent.click(summary);
  await waitFor(() => expect((container.querySelector("details") as HTMLDetailsElement).open).toBe(true));
  expect(calls.filter((c) => c.cmd === "get_transcript")).toHaveLength(1);

  fireEvent.click(screen.getByRole("button", { name: "전사본 복사" }));
  expect(await screen.findByText("복사했습니다")).toBeTruthy();
  expect(writeText).toHaveBeenCalledWith("[00:03] 안녕하세요\n[75:15] <b>다음</b> 안건");
});
