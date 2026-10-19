import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { hhmm } from "../components/common";
import type { LlmConnection, LoadedSettings, MeetingDetail, Meta, Progress, Segment, Settings } from "../lib/ipc";
import Meeting from "./Meeting";
import { parseDoc } from "./MeetingParts";

// Autosave state outlives the screen (one per meeting id), so each test opens a meeting of its own.
let id = "";
let count = 0;
beforeEach(() => {
  id = `m${++count}`;
});

// No clearMocks() here: the screen unlistens asynchronously after unmount and still needs the
// mocked event internals (as in App.test.tsx). Each test mocks IPC anew.
afterEach(() => {
  cleanup();
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

const connection = (name: string): LlmConnection => ({
  name,
  base_url: "https://api.example.com/v1",
  model: "m",
  context_tokens: 200000,
  is_local: name === "로컬",
  timeout_secs: 300,
});

const settings = (patch: Partial<Settings> = {}): LoadedSettings => ({
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
    llm_connections: [],
    default_llm: null,
    summarize_local_by_default: false,
    webhooks: [{ name: "#회의록" }],
    last_channel: null,
    glossary: [],
    ...patch,
  },
});

const progress = (patch: Partial<Progress>): Progress => ({
  id,
  status: "summarizing",
  chunk: 0,
  chunks: 0,
  percent: 0,
  eta_secs: null,
  position: null,
  ...patch,
});

type Call = { cmd: string; args: Record<string, unknown> };

/** `get_meeting` answers with `detail` as it is then (no send out unless it says so); `handler`
 * answers the rest (settings with one webhook unless it says otherwise), and every call is kept. */
const mock = (
  detail: Omit<MeetingDetail, "sending"> & { sending?: boolean },
  handler: (cmd: string, args: Call["args"]) => unknown = () => undefined,
) => {
  const calls: Call[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: args as Call["args"] });
      if (cmd === "get_meeting") return { sending: false, ...detail };
      return handler(cmd, args as Call["args"]) ?? (cmd === "get_settings" ? settings() : undefined);
    },
    { shouldMockEvents: true },
  );
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

// The send bar goes to App's bottom stack; here, the body.
const props = { onHome: () => {}, onNotice: () => {}, onSetup: () => {}, barSlot: document.body };
const textbox = (name: string) => screen.getByRole("textbox", { name }) as HTMLInputElement;
/** Opens ⋯ from the keyboard, as Radix menus open on Enter. */
const menu = async () => {
  fireEvent.keyDown(screen.getByRole("button", { name: "더보기" }), { key: "Enter" });
  return within(await screen.findByRole("menu"));
};
/** Picks an item from ⋯. */
const pick = async (name: string) => fireEvent.click((await menu()).getByRole("menuitem", { name }));
const disabled = (el: HTMLElement) => el.getAttribute("aria-disabled") === "true";
/** Opens the summary's editor with 고치기, as a key would, and returns it. Sync, for fake timers. */
const write = () => {
  fireEvent.click(screen.getByRole("button", { name: "고치기" }));
  return screen.getByRole("textbox", { name: "요약" }) as HTMLTextAreaElement;
};
/** The same once 고치기 can be pressed: the summary has been read, and has come in. */
const openEditor = async () => {
  await waitFor(() => expect((screen.getByRole("button", { name: "고치기" }) as HTMLButtonElement).disabled).toBe(false));
  return write();
};

// D6 / NFR-07: LLM output must stay plain text, never become markup: in the document, the editor
// and the 미정 list.
test("renders HTML in a summary as text", async () => {
  const summary = "## 액션 아이템\n- <img src=x onerror=alert(1)> 담당: 미정";
  mock({ meta: meta({ title: "<b>주간</b> 회의" }), summary });

  render(<Meeting id={id} {...props} />);

  const item = (await screen.findByText(/^<img src=x onerror=alert\(1\)> 담당:$/)).closest("li");
  expect(item?.textContent).toBe("<img src=x onerror=alert(1)> 담당: 미정");
  expect(screen.getByRole("heading", { level: 2, name: "액션 아이템" })).toBeTruthy();
  expect(screen.getByDisplayValue("<b>주간</b> 회의")).toBeTruthy();
  fireEvent.click(screen.getByText("확인할 항목 1개"));
  expect(screen.getByText("- <img src=x onerror=alert(1)> 담당: 미정")).toBeTruthy();
  expect(document.body.querySelector("img")).toBeNull();
  expect(document.body.querySelector("b")).toBeNull();

  const textarea = await openEditor();
  expect(textarea.tagName).toBe("TEXTAREA");
  expect(textarea.value).toBe(summary);
  expect(document.body.querySelector("img")).toBeNull();
});

// Our own line parser (architecture 10.4): the prompt's shapes, and everything else as typed.
test("parses a summary into headings, items with owner and due date, 없음 and literal text", () => {
  expect(
    parseDoc(
      [
        "## 결정사항",
        "- 파일럿은 12명으로 시작한다",
        "* [김지훈] 설치 안내 문서 작성 (10월 18일)",
        "• [미정] 키 발급 요청 (미정)",
        "- 없음",
        "",
        "# 제목 ### 셋",
        "**굵게** 와 [링크](https://example.com)",
        "- [링크](https://example.com) 그대로 (괄호) 뒤",
        "#### 넷",
        "-붙은 줄",
      ].join("\n"),
    ),
  ).toEqual([
    { kind: "heading", line: 0, text: "결정사항" },
    { kind: "item", line: 1, owner: null, text: "파일럿은 12명으로 시작한다", due: null },
    { kind: "item", line: 2, owner: "김지훈", text: "설치 안내 문서 작성", due: "10월 18일" },
    { kind: "item", line: 3, owner: "미정", text: "키 발급 요청", due: "미정" },
    { kind: "none", line: 4 },
    { kind: "gap", line: 5 },
    { kind: "heading", line: 6, text: "제목 ### 셋" },
    { kind: "text", line: 7, text: "**굵게** 와 [링크](https://example.com)" },
    { kind: "item", line: 8, owner: null, text: "[링크](https://example.com) 그대로 (괄호) 뒤", due: null },
    { kind: "text", line: 9, text: "#### 넷" },
    { kind: "text", line: 10, text: "-붙은 줄" },
  ]);
  // A line break from Windows does not stop an item from parsing.
  expect(parseDoc("- [박서연] 웹훅 정리 (미정)\r")[0]).toMatchObject({ owner: "박서연", text: "웹훅 정리" });
});

// Design-system 6.6: a formatted document, not Markdown source.
test("the summary reads as a document: headings, owner chips, due dates, 없음 and 미정 on the marker", async () => {
  const summary = [
    "## 결정사항",
    "- 파일럿은 12명으로 시작한다",
    "## 액션 아이템",
    "- [김지훈] 설치 안내 문서 작성 (10월 18일)",
    "- [미정] 키 발급 요청 (미정)",
    "## 미해결 질문",
    "- 없음",
  ].join("\n");
  mock({ meta: meta(), summary });
  render(<Meeting id={id} {...props} />);

  const heading = await screen.findByRole("heading", { level: 2, name: "액션 아이템" });
  expect(heading.className).toMatch(/text-title/);
  expect(heading.className).not.toMatch(/font-display/);
  const doc = heading.closest("[class*=whitespace-pre-wrap]") as HTMLElement;
  expect(doc.textContent).not.toMatch(/##|- |\[|\(/);
  const [plain, owned, undecided] = within(doc).getAllByRole("listitem");
  expect(plain.textContent).toBe("파일럿은 12명으로 시작한다");
  expect(within(owned).getByText("김지훈").className).toMatch(/bg-sunken/);
  expect(within(owned).getByText("10월 18일").className).toMatch(/text-ink-2/);
  expect(within(undecided).getAllByText("미정").map((el) => /bg-marker/.test(el.className))).toEqual([true, true]);
  expect(within(doc).getByText("없음").className).toMatch(/text-ink-2/);
  // Owners and items as one list per section.
  expect(within(doc).getAllByRole("list")).toHaveLength(2);
});

// Design-system 6.6: a click on a line puts the caret at its start; Esc goes back to the document.
test("a click on a line opens the editor there, and Esc goes back to the document and to 고치기", async () => {
  const summary = "## 결정사항\n- 파일럿은 12명으로 시작한다\n## 액션 아이템\n- [김지훈] 설치 안내 문서 작성 (10월 18일)";
  mock({ meta: meta(), summary });
  render(<Meeting id={id} {...props} />);

  fireEvent.click(await screen.findByText("김지훈"));
  const field = screen.getByRole("textbox", { name: "요약" }) as HTMLTextAreaElement;
  expect(document.activeElement).toBe(field);
  expect([field.selectionStart, field.selectionEnd]).toEqual(Array(2).fill(summary.indexOf("- [김지훈]")));

  fireEvent.keyDown(field, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull());
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "고치기" }));

  fireEvent.click(screen.getByRole("heading", { level: 2, name: "액션 아이템" }));
  expect(textbox("요약").selectionStart).toBe(summary.indexOf("## 액션 아이템"));
});

// Moving up, a browser brings the caret only to the window's top edge: under App's sticky header.
test("the caret moving up is kept clear of the sticky header", async () => {
  mock({ meta: meta(), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  const field = await openEditor();
  const scrollBy = vi.spyOn(window, "scrollBy").mockImplementation(() => {});
  const caret = vi.spyOn(Range.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 20, 1, 20));

  fireEvent.select(field);
  // 16px below the 48px header.
  await waitFor(() => expect(scrollBy).toHaveBeenCalledWith(0, 20 - 48 - 16));
  caret.mockRestore();
  scrollBy.mockRestore();
});

test("leaving the editor goes back to the document, but not for the send bar or another app", async () => {
  mock({ meta: meta(), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  const field = await openEditor();

  // Another app took the window: the field is still the focused one, and the editor stays.
  fireEvent.blur(field);
  expect(textbox("요약")).toBe(field);
  // A press on the send bar keeps it, for a send that fails.
  const send = await sendButton("#회의록으로 보내기");
  fireEvent.pointerDown(send);
  send.focus();
  expect(textbox("요약")).toBe(field);

  field.focus();
  screen.getByRole("button", { name: "홈으로" }).focus();
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull());
  expect(screen.getByText("요약")).toBeTruthy();
});

// REV-01: the editor coming and going changes nothing about saving.
test("editing then leaving the editor saves once, with the latest text", async () => {
  vi.useFakeTimers();
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "save_summary" ? meta() : undefined));
  const saves = () => calls.filter((c) => c.cmd === "save_summary").map((c) => c.args.body);
  const { unmount } = render(<Meeting id={id} {...props} />);
  await tick();

  const field = write();
  fireEvent.change(field, { target: { value: "고친" } });
  fireEvent.change(field, { target: { value: "고친 요약" } });
  fireEvent.keyDown(field, { key: "Escape" });
  await tick(100);
  expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull();
  expect(screen.getByText("고친 요약")).toBeTruthy();
  expect(saves()).toEqual([]);

  await tick(800);
  expect(saves()).toEqual(["고친 요약"]);
  unmount();
  await tick(800);
  expect(saves()).toEqual(["고친 요약"]);
});

// REV-01: the marker sits behind the text in a layer of text nodes the textarea lies over.
test("미정 is marked on a hidden mirror of the text, without touching the field", async () => {
  const summary = "- [미정] 회의실 예약\n- [김철수] 자료 (미정)\n";
  mock({ meta: meta(), summary }, (cmd) => (cmd === "save_summary" ? meta() : undefined));

  render(<Meeting id={id} {...props} />);

  const field = await openEditor();
  const mirror = field.previousElementSibling as HTMLElement;
  expect(mirror.getAttribute("aria-hidden")).toBe("true");
  expect(mirror.textContent).toBe(`${summary}\u200b`);
  expect([...mirror.querySelectorAll("mark")].map((m) => m.textContent)).toEqual(["미정", "미정"]);
  expect(mirror.querySelector("mark")?.className).not.toMatch(/font-/);
  // Same box: what places a glyph is the same on both.
  expect(mirror.className.replace(" text-transparent", "")).toBe(
    field.className.replace(/ peer .*$/, "").replace(" text-ink", ""),
  );
  expect(field.value).toBe(summary);

  fireEvent.change(field, { target: { value: "미정 없음" } });
  expect(mirror.querySelectorAll("mark")).toHaveLength(1);
  expect(mirror.textContent).toBe("미정 없음\u200b");
});

// SUM-10, eng review D8. REV-01: the transcript can still be read.
test("no speech: a warning and no editor or send; only 전송 안 함 and 삭제 remain", async () => {
  mock({ meta: meta({ no_speech: true, llm_connection: null, title: "" }), summary: "" });

  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("음성이 거의 감지되지 않았습니다")).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["홈으로", "전사본", ""]);
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
  expect((await menu()).getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["전송 안 함", "삭제"]);
});

test("the 미정 list holds only the lines with 미정, and follows edits", async () => {
  const summary = [
    "## 액션 아이템",
    "- [미정] 회의실 예약 (10/20)",
    "- [김철수] 자료 준비 (미정)",
    "- [이영희] 보고서 (10/22)",
    "## 미해결 질문",
    "- 없음",
  ].join("\n");
  mock({ meta: meta(), summary }, (cmd) => (cmd === "save_summary" ? meta() : undefined));

  render(<Meeting id={id} {...props} />);

  const list = (await screen.findByText("확인할 항목 2개")).closest("details") as HTMLDetailsElement;
  expect(list.open).toBe(false);
  fireEvent.click(screen.getByText("확인할 항목 2개"));
  expect(list.open).toBe(true);
  expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
    "- [미정] 회의실 예약 (10/20)",
    "- [김철수] 자료 준비 (미정)",
  ]);

  fireEvent.change(await openEditor(), { target: { value: "- [이영희] 보고서 (10/22)" } });
  expect(screen.queryByText(/확인할 항목/)).toBeNull();
});

// REV-01: one save per pause, with what is typed last; leaving saves at once.
test("autosaves once, 800 ms after typing stops, and saves a pending edit on leaving", async () => {
  vi.useFakeTimers();
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd, args) =>
    cmd === "save_summary" ? meta({ title: args.title as string }) : undefined,
  );
  const saves = () => calls.filter((c) => c.cmd === "save_summary").map((c) => c.args);
  const { unmount } = render(<Meeting id={id} {...props} />);
  await tick();

  fireEvent.change(write(), { target: { value: "첫 줄" } });
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
  render(<Meeting id={id} {...props} />);
  await tick();

  fireEvent.change(write(), { target: { value: "하나" } });
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
  const first = render(<Meeting id={id} {...props} onNotice={(m) => notices.push(m)} />);
  await tick();

  fireEvent.change(write(), { target: { value: "고친 요약" } });
  await tick(800);
  held[0].reject("디스크가 가득 찼습니다");
  await tick();
  expect(screen.getByText("저장하지 못했습니다: 디스크가 가득 찼습니다")).toBeTruthy();
  expect(textbox("요약").value).toBe("고친 요약");

  first.unmount();
  await tick();
  expect(held.map((h) => h.args.body)).toEqual(["고친 요약", "고친 요약"]);

  // Reopened while that save runs: the meeting is read only after it ends.
  render(<Meeting id={id} {...props} onNotice={(m) => notices.push(m)} />);
  await tick();
  expect(reads()).toBe(1);
  held[1].reject("디스크가 가득 찼습니다");
  await tick();
  expect(notices).toEqual(["회의 문서에서 고친 내용을 저장하지 못했습니다: 디스크가 가득 찼습니다"]);
  expect(reads()).toBe(2);

  // The unsaved edit is back, in the document and the editor, and goes again.
  expect(screen.getByText("고친 요약")).toBeTruthy();
  expect((write()).value).toBe("고친 요약");
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
  render(<Meeting id={id} {...props} />);
  await tick();

  fireEvent.change(textbox("제목"), { target: { value: "주간 " } });
  await tick(800);
  expect(screen.getByText("저장됨")).toBeTruthy();
  expect(textbox("제목").value).toBe("주간 ");

  fireEvent.change(textbox("제목"), { target: { value: "주간\t\t회의" } });
  await tick(800);
  expect(textbox("제목").value).toBe("주간 회의");
  // Enter never breaks the title; a pasted break turns into a space, and the caret stays put.
  expect(fireEvent.keyDown(textbox("제목"), { key: "Enter" })).toBe(false);
  fireEvent.change(textbox("제목"), { target: { value: "주간\n회의", selectionStart: 3, selectionEnd: 3 } });
  expect(textbox("제목").value).toBe("주간 회의");
  expect(textbox("제목").selectionStart).toBe(3);
});

test("a failed save keeps the text, says so, and stops the export", async () => {
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "save_summary") throw new Error("summary.md 저장 실패");
  });
  render(<Meeting id={id} {...props} />);

  fireEvent.change(await openEditor(), { target: { value: "고친 요약" } });
  await pick(".md로 저장");

  expect(await screen.findByText(/저장하지 못했습니다: .*summary\.md 저장 실패/)).toBeTruthy();
  // The menu took focus, so the document is back, with the text kept.
  expect(screen.getByText("고친 요약")).toBeTruthy();
  expect((await openEditor()).value).toBe("고친 요약");
  expect(calls.map((c) => c.cmd)).not.toContain("export_markdown");
});

test(".md로 저장 saves a pending edit first, then shows the path; a cancel shows nothing", async () => {
  let path: string | null = null;
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "save_summary") return meta();
    if (cmd === "export_markdown") return path;
  });
  render(<Meeting id={id} {...props} />);

  await screen.findByRole("button", { name: "고치기" });
  await pick(".md로 저장");
  await waitFor(() => expect(calls.map((c) => c.cmd)).toContain("export_markdown"));
  expect(screen.queryByText(/^저장했습니다/)).toBeNull();

  path = "/Users/me/주간 회의.md";
  fireEvent.change(await openEditor(), { target: { value: "고친 요약" } });
  await pick(".md로 저장");
  expect(await screen.findByText("저장했습니다: /Users/me/주간 회의.md")).toBeTruthy();
  expect(calls.map((c) => c.cmd).filter((c) => c === "save_summary" || c === "export_markdown")).toEqual([
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
  render(<Meeting id={id} {...props} onHome={() => home++} />);

  fireEvent.change(await openEditor(), { target: { value: "고친 요약" } });
  await pick("전송 안 함");
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
  const { unmount } = render(
    <Meeting id={id} {...props} onHome={() => home++} onNotice={(n) => notices.push(n)} />,
  );

  fireEvent.change(await openEditor(), { target: { value: "지울 회의" } });
  await pick("삭제");
  const dialog = await screen.findByRole("alertdialog", { name: "회의를 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(home).toBe(1));
  unmount();
  // Past the autosave debounce.
  await new Promise((r) => setTimeout(r, 900));

  expect(calls.map((c) => c.cmd)).not.toContain("save_summary");
  expect(notices).toEqual([]);
});

// SUM-07: only a meeting whose transcript went to an LLM says where, in the meta line.
test("says where the transcript went only when it went somewhere", async () => {
  const cases: [Partial<Meta>, string | null][] = [
    [{}, "회사 클라우드로 보내 요약함"],
    [{ llm_connection: "Ollama", summarize_local: true }, "이 PC에서 Ollama로 요약함"],
    // The built-in route is this PC itself, never its internal name.
    [{ llm_connection: "builtin", summarize_local: true }, "이 PC에서 요약함"],
    [{ llm_connection: null }, null],
    [{ no_speech: true }, null],
  ];
  for (const [patch, line] of cases) {
    mock({ meta: meta(patch), summary: "요약" });
    render(<Meeting id={id} {...props} />);
    await screen.findByText("녹음 홍길동");
    expect(screen.queryByText(/요약함$/)?.textContent ?? null).toBe(line);
    cleanup();
  }
});

// Design-system 6.6: the same header in every state, before the meeting is even read.
test("the header says when, how long and who recorded; a title waits for the summary", async () => {
  mock({ meta: meta({ status: "transcribing", title: "", duration_ms: 42 * 60_000 }), summary: "" });
  const start = new Date("2026-10-13T05:00:03Z");
  const when = `${start.getMonth() + 1}월 ${start.getDate()}일 ${"일월화수목금토"[start.getDay()]} ${hhmm(start.toISOString())}`;

  render(<Meeting id={id} {...props} listed={{ ...meta({ status: "transcribing", title: "" }), interrupted: false }} />);

  // From the list at once, so the title can glide in from its row.
  expect(screen.getByRole("heading", { level: 1, name: "제목은 요약 뒤에 붙어요" })).toBeTruthy();
  expect(screen.getByText(when)).toBeTruthy();
  expect(await screen.findByText("42분")).toBeTruthy();
  expect(await screen.findByText("녹음 홍길동")).toBeTruthy();
  // Nothing to read yet, and nothing to send.
  expect(screen.queryByRole("button", { name: "전사본" })).toBeNull();
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
});

// REV-05. The webview's clipboard can be missing or refuse.
test("copies the summary as Markdown, and says so when the clipboard fails", async () => {
  mock({ meta: meta(), summary: "## 결정사항\n- 없음" });
  render(<Meeting id={id} {...props} />);

  await screen.findByRole("button", { name: "고치기" });
  await pick("요약 복사");
  expect(await screen.findByText("클립보드에 복사하지 못했습니다")).toBeTruthy();

  const writeText = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  await pick("요약 복사");
  expect(await screen.findByText("복사했습니다")).toBeTruthy();
  expect(writeText).toHaveBeenCalledWith("# 주간 회의\n\n## 결정사항\n- 없음");
});

test("전사본 opens a panel that reads the transcript once, as text, copies it with times, and closes on Esc", async () => {
  const segments: Segment[] = [
    { start_ms: 3_000, end_ms: 9_000, text: " 안녕하세요" },
    { start_ms: 4_515_000, end_ms: 4_520_000, text: "<b>다음</b> 안건" },
  ];
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_transcript" ? segments : undefined));
  const writeText = vi.fn(() => Promise.resolve());
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  render(<Meeting id={id} {...props} />);

  const toggle = await screen.findByRole("button", { name: "전사본" });
  expect(calls.map((c) => c.cmd)).not.toContain("get_transcript");
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(toggle);
  const panel = within(await screen.findByRole("complementary", { name: "전사본" }));
  expect(await panel.findByText("<b>다음</b> 안건")).toBeTruthy();
  expect(panel.getByText("75:15")).toBeTruthy();
  expect(document.body.querySelector("b")).toBeNull();
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  // Focus moves in.
  expect(document.activeElement).toBe(panel.getByRole("heading", { name: "전사본" }));

  fireEvent.click(panel.getByRole("button", { name: "전사본 복사" }));
  expect(await panel.findByText("복사했습니다")).toBeTruthy();
  expect(writeText).toHaveBeenCalledWith("[00:03] 안녕하세요\n[75:15] <b>다음</b> 안건");

  // Esc closes it and focus goes back to its button.
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("complementary")).toBeNull());
  expect(document.activeElement).toBe(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("false");

  fireEvent.click(toggle);
  fireEvent.click(await screen.findByRole("button", { name: "닫기" }));
  await waitFor(() => expect(screen.queryByRole("complementary")).toBeNull());
  expect(calls.filter((c) => c.cmd === "get_transcript")).toHaveLength(1);
});

// An Esc a menu, dialog or list took is marked by Radix before it closes, and may be gone by the
// time the window hears it (no exit animation with Reduce Motion). The editor's Esc is its own too.
test("an Esc taken by a menu or by the editor leaves the transcript panel open", async () => {
  mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_transcript" ? [] : undefined));
  render(<Meeting id={id} {...props} />);
  fireEvent.click(await screen.findByRole("button", { name: "전사본" }));
  await screen.findByRole("complementary", { name: "전사본" });

  const taken = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
  taken.preventDefault();
  window.dispatchEvent(taken);
  expect(screen.getByRole("complementary", { name: "전사본" })).toBeTruthy();

  fireEvent.keyDown(await openEditor(), { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull());
  expect(screen.getByRole("complementary", { name: "전사본" })).toBeTruthy();

  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("complementary")).toBeNull());
});

// A read clears only a read's failure: here the one banner that says why the send is off.
test("a settings failure is still said after the meeting is read", async () => {
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "get_settings") throw "설정 파일이 손상됐습니다";
  });
  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("설정을 읽지 못했습니다: 설정 파일이 손상됐습니다")).toBeTruthy();
  await screen.findByRole("button", { name: "고치기" });
  expect(calls.filter((c) => c.cmd === "get_meeting")).toHaveLength(1);
  expect(screen.getByText("설정을 읽지 못했습니다: 설정 파일이 손상됐습니다")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Slack으로 보내기" }) as HTMLButtonElement).disabled).toBe(true);
});

// A save still out when the meeting is deleted: when it fails, nothing goes again and nothing is said.
test("a save that fails after its meeting was deleted is dropped", async () => {
  const { held, handler } = holdSaves();
  mock({ meta: meta(), summary: "요약" }, (cmd, args) => (cmd === "delete_meeting" ? null : handler(cmd, args)));
  let home = 0;
  const notices: string[] = [];
  const { unmount } = render(
    <Meeting id={id} {...props} onHome={() => home++} onNotice={(n) => notices.push(n)} />,
  );

  fireEvent.change(await openEditor(), { target: { value: "지울 회의" } });
  await waitFor(() => expect(held).toHaveLength(1), { timeout: 2000 });
  await pick("삭제");
  const dialog = await screen.findByRole("alertdialog", { name: "회의를 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(home).toBe(1));

  held[0].reject("디스크가 가득 찼습니다");
  await act(() => Promise.resolve());
  expect(screen.queryByText(/저장하지 못했습니다/)).toBeNull();
  unmount();
  await new Promise((r) => setTimeout(r, 50));
  expect(held).toHaveLength(1);
  expect(notices).toEqual([]);
});

// Slack (SND-01..06, REV-02).

const offset = -new Date("2026-10-13T05:00:03Z").getTimezoneOffset();
const sendButton = (name: string) => screen.findByRole("button", { name }) as Promise<HTMLButtonElement>;

// SND-04, NFR-04: with one webhook the button is ready, no choice to make.
test("the send button names the default channel: the only one, the last one sent to, else the first", async () => {
  // The particle follows the channel's last syllable: 록 → 으로, 발 (ㄹ) → 로.
  const cases: [Partial<Settings>, string, string, boolean][] = [
    [{}, "#회의록", "#회의록으로 보내기", false],
    [{ webhooks: [{ name: "#회의록" }, { name: "#개발" }], last_channel: "#개발" }, "#개발", "#개발로 보내기", true],
    [
      { webhooks: [{ name: "#회의록" }, { name: "#개발" }], last_channel: "#지운 채널" },
      "#회의록",
      "#회의록으로 보내기",
      true,
    ],
  ];
  for (const [patch, channel, button, choice] of cases) {
    mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_settings" ? settings(patch) : undefined));
    render(<Meeting id={id} {...props} />);
    expect((await sendButton(button)).disabled).toBe(false);
    const select = screen.queryByRole("combobox", { name: "보낼 채널" });
    expect(select?.textContent ?? null).toBe(choice ? channel : null);
    cleanup();
  }
});

test("sends to the channel picked, with the title, body and the start's UTC offset", async () => {
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "get_settings") return settings({ webhooks: [{ name: "#회의록" }, { name: "#개발" }] });
    if (cmd === "send_to_slack") return meta({ status: "sent" });
  });
  render(<Meeting id={id} {...props} />);

  fireEvent.keyDown(await screen.findByRole("combobox", { name: "보낼 채널" }), { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: "#개발" }));
  fireEvent.click(await sendButton("#개발로 보내기"));

  await waitFor(() => expect(calls.map((c) => c.cmd)).toContain("send_to_slack"));
  expect(calls.find((c) => c.cmd === "send_to_slack")?.args).toEqual({
    id,
    webhook: "#개발",
    title: "주간 회의",
    body: "요약",
    offsetMinutes: offset,
  });
});

// REV-02: the count is the message block's, from slack.rs; over 12,000 nothing goes.
test("over 12,000 characters the send is off and says by how much", async () => {
  let length = 12_345;
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "slack_length" ? length : undefined));
  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("12,345 / 12,000자 (Slack 전송 기준)")).toBeTruthy();
  expect(screen.getByText("345자를 줄여 주세요")).toBeTruthy();
  expect((await sendButton("#회의록으로 보내기")).disabled).toBe(true);
  expect(calls.find((c) => c.cmd === "slack_length")?.args).toEqual({
    id,
    title: "주간 회의",
    body: "요약",
    offsetMinutes: offset,
  });

  length = 12_000;
  fireEvent.change(await openEditor(), { target: { value: "줄인 요약" } });
  expect(await screen.findByText("12,000 / 12,000자 (Slack 전송 기준)")).toBeTruthy();
  expect(screen.queryByText(/줄여 주세요/)).toBeNull();
  expect((await sendButton("#회의록으로 보내기")).disabled).toBe(false);
  const counted = calls.filter((c) => c.cmd === "slack_length");
  expect(counted[counted.length - 1].args.body).toBe("줄인 요약");
});

test("the length ring turns warn-line from 90% and danger-line past the limit", async () => {
  let length = 10_799;
  mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "slack_length" ? length : undefined));
  render(<Meeting id={id} {...props} />);
  const ring = async (text: string) =>
    (await screen.findByText(text)).previousElementSibling?.querySelector("circle:last-child")?.getAttribute("class");

  expect(await ring("10,799 / 12,000자 (Slack 전송 기준)")).toBe("stroke-brand");
  expect(screen.queryByText(/자 남음$/)).toBeNull();
  length = 10_800;
  fireEvent.change(await openEditor(), { target: { value: "긴 요약" } });
  expect(await ring("10,800 / 12,000자 (Slack 전송 기준)")).toBe("stroke-warn-line");
  // Not the color alone (NFR-09).
  expect(screen.getByText("1,200자 남음")).toBeTruthy();
  length = 12_001;
  fireEvent.change(screen.getByRole("textbox", { name: "요약" }), { target: { value: "더 긴 요약" } });
  expect(await ring("12,001 / 12,000자 (Slack 전송 기준)")).toBe("stroke-danger-line");
  expect(screen.queryByText(/자 남음$/)).toBeNull();
  expect(screen.getByText("1자를 줄여 주세요")).toBeTruthy();
});

test("no webhook: the send is off and the Slack settings are one click away", async () => {
  const tabs: string[] = [];
  mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_settings" ? settings({ webhooks: [] }) : undefined));
  render(<Meeting id={id} {...props} onSetup={(tab) => tabs.push(tab)} />);

  fireEvent.click(await screen.findByRole("button", { name: "웹훅 추가하기" }));
  expect(tabs).toEqual(["slack"]);
  expect((screen.getByRole("button", { name: "Slack으로 보내기" }) as HTMLButtonElement).disabled).toBe(true);
});

test("a meeting out of review is never sent from here, and reads as text", async () => {
  mock({ meta: meta({ status: "archived" }), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("전송 안 함")).toBeTruthy();
  expect(screen.getByText("요약")).toBeTruthy();
  // Nothing to edit: no 고치기, and a click on the text opens nothing.
  expect(screen.queryByRole("button", { name: "고치기" })).toBeNull();
  fireEvent.click(screen.getByText("요약"));
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
});

test("the pending edit is saved before the send, and a failed save stops it", async () => {
  const { held, handler } = holdSaves();
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd, args) =>
    cmd === "send_to_slack" ? meta({ status: "sent" }) : handler(cmd, args),
  );
  render(<Meeting id={id} {...props} />);

  fireEvent.change(await openEditor(), { target: { value: "고친 요약" } });
  fireEvent.click(await sendButton("#회의록으로 보내기"));
  await waitFor(() => expect(held).toHaveLength(1));
  expect(held[0].args.body).toBe("고친 요약");
  expect(calls.map((c) => c.cmd)).not.toContain("send_to_slack");
  // Nothing more is typed while it goes, and the meeting cannot be deleted under it.
  expect(screen.getByRole("textbox", { name: "요약" }).hasAttribute("readonly")).toBe(true);
  expect(disabled((await menu()).getByRole("menuitem", { name: "삭제" }))).toBe(true);
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });

  held[0].reject("디스크가 가득 찼습니다");
  expect(await screen.findByText("저장하지 못했습니다: 디스크가 가득 찼습니다")).toBeTruthy();
  expect(calls.map((c) => c.cmd)).not.toContain("send_to_slack");

  fireEvent.click(await sendButton("#회의록으로 보내기"));
  await waitFor(() => expect(held).toHaveLength(2));
  held[1].resolve(meta());
  await waitFor(() => expect(calls.map((c) => c.cmd)).toContain("send_to_slack"));
  expect(calls.find((c) => c.cmd === "send_to_slack")?.args.body).toBe("고친 요약");
});

// SND-06.
test("a failed send shows the cause and becomes 다시 보내기, with 요약 복사 still there", async () => {
  const cause = "웹훅이 없거나 삭제됐습니다(HTTP 404: no_service). 설정 → Slack에서 웹훅 주소를 확인해 주세요";
  let fail = true;
  mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd !== "send_to_slack") return;
    if (fail) throw cause;
    return meta({ status: "sent", sent: { channel: "#회의록", at: "2026-10-13T05:43:00Z", edit_count: 0 } });
  });
  render(<Meeting id={id} {...props} />);

  fireEvent.click(await sendButton("#회의록으로 보내기"));
  expect(await screen.findByText(cause)).toBeTruthy();
  expect((await menu()).getByRole("menuitem", { name: "요약 복사" })).toBeTruthy();
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
  // Still in review, so still editable.
  expect((await openEditor()).hasAttribute("readonly")).toBe(false);

  fail = false;
  fireEvent.click(await sendButton("다시 보내기"));
  expect(await screen.findByText(/^#회의록으로 보냄 · /)).toBeTruthy();
  expect(screen.queryByText(cause)).toBeNull();
});

// Sent from an earlier visit, which went home: nothing may change under the send, and its end
// (`slack://finished`) is read again here.
test("a send still out from an earlier visit keeps the screen read-only until it ends", async () => {
  const detail = { meta: meta(), summary: "요약", sending: true };
  const calls = mock(detail);
  render(<Meeting id={id} {...props} />);

  expect((await sendButton("보내는 중…")).disabled).toBe(true);
  expect(textbox("제목").hasAttribute("readonly")).toBe(true);
  // The summary cannot be opened to edit, from 고치기 or by a click.
  expect((screen.getByRole("button", { name: "고치기" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByText("요약"));
  expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull();
  const items = await menu();
  expect(disabled(items.getByRole("menuitem", { name: "전송 안 함" }))).toBe(true);
  expect(disabled(items.getByRole("menuitem", { name: "삭제" }))).toBe(true);
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });

  Object.assign(detail, {
    meta: meta({ status: "sent", sent: { channel: "#회의록", at: "2026-10-13T05:43:00Z", edit_count: 0 } }),
    sending: false,
  });
  // Another meeting's send is not this one's.
  await act(() => emit("slack://finished", "다른 회의"));
  expect(calls.filter((c) => c.cmd === "get_meeting")).toHaveLength(1);
  await act(() => emit("slack://finished", id));
  expect(await screen.findByText(/^#회의록으로 보냄 · /)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "보내는 중…" })).toBeNull();
  // The bar stays to say where it went.
  expect(screen.getByRole("button", { name: "#회의록으로 보냄" })).toBeTruthy();
});

test("a send that failed before is told again when the meeting opens", async () => {
  mock({ meta: meta({ error: "Slack 응답 시간 초과입니다" }), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("Slack 응답 시간 초과입니다")).toBeTruthy();
  expect((await sendButton("다시 보내기")).disabled).toBe(false);
});

// Stays here, read-only, so the recorder sees where it went and how much they changed.
test("a sent meeting says where, when and how many edits, and is read-only", async () => {
  const at = "2026-10-13T05:43:00Z";
  const sent = meta({ status: "sent", sent: { channel: "#회의록", at, edit_count: 2 } });
  mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "send_to_slack" ? sent : undefined));
  render(<Meeting id={id} {...props} />);

  fireEvent.click(await sendButton("#회의록으로 보내기"));
  // The time alone: it went the day the meeting was.
  expect(await screen.findByText(`#회의록으로 보냄 · ${hhmm(at)} · 수정 2곳`)).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button", { name: "고치기" })).toBeNull();
  fireEvent.click(screen.getByText("요약"));
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
  // The button turned into where it went, and that is said once.
  expect(screen.getByRole("button", { name: "#회의록으로 보냄" }).getAttribute("aria-disabled")).toBe("true");
  expect(screen.getByText("#회의록으로 보냈습니다")).toBeTruthy();
  expect((await menu()).queryByRole("menuitem", { name: "전송 안 함" })).toBeNull();
});

// Before review (architecture 10.1).

// Architecture 10.4: a transcript is outside text and must stay plain text.
test("a transcribed meeting's transcript reads with times, HTML as text, and says its device", async () => {
  const segments: Segment[] = [
    { start_ms: 3_000, end_ms: 9_000, text: "안녕하세요" },
    { start_ms: 4_515_000, end_ms: 4_520_000, text: "<img src=x onerror=alert(1)> <b>다음</b> 안건" },
  ];
  mock({ meta: meta({ status: "transcribed", backend: "gpu", llm_connection: null }), summary: "" }, (cmd, args) =>
    cmd === "get_transcript" && args.id === id ? segments : undefined,
  );

  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("처리 장치 GPU")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "전사본" }));
  expect(await screen.findByText("<img src=x onerror=alert(1)> <b>다음</b> 안건")).toBeTruthy();
  expect(screen.getByText("안녕하세요")).toBeTruthy();
  expect(screen.getByText("00:03")).toBeTruthy();
  expect(screen.getByText("75:15")).toBeTruthy();
  expect(document.body.querySelector("img")).toBeNull();
  expect(document.body.querySelector("b")).toBeNull();
});

test("a running meeting shows its step, chunk, percent and time left", async () => {
  const running = progress({ status: "transcribing", chunk: 1, chunks: 5, percent: 42, eta_secs: 3_900 });
  mock({ meta: meta({ status: "transcribing", title: "", backend: "gpu" }), summary: "" });

  render(<Meeting id={id} {...props} progress={running} />);

  expect(await screen.findByText("구간 2/5 · 약 1시간 5분 남음 · 처리 장치 GPU")).toBeTruthy();
  const steps = screen.getByRole("list", { name: "처리 단계" });
  expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
    "녹음완료",
    "전사42%",
    "요약",
    "검토",
  ]);
  expect(within(steps).getByText("42%").closest("li")?.getAttribute("aria-current")).toBe("step");
  expect(screen.getByRole("progressbar", { name: "전사 진행률" }).getAttribute("aria-valuenow")).toBe("42");
});

test("다시 시도 puts a failed meeting back in line", async () => {
  const error = "메모리가 부족해 전사가 중단되었습니다";
  const failed = meta({ status: "transcribe_failed", error });
  const calls = mock({ meta: failed, summary: "" }, (cmd) => (cmd === "resume_meeting" ? failed : undefined));
  const { rerender } = render(<Meeting id={id} {...props} />);

  expect(await screen.findByText(error)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
  await waitFor(() => expect(calls.find((c) => c.cmd === "resume_meeting")?.args).toEqual({ id }));

  // The queue's event gives it a place: no error and no second 다시 시도 while it waits.
  rerender(<Meeting id={id} {...props} progress={progress({ status: "transcribe_failed", position: 1 })} />);
  expect(await screen.findByText("대기 중 · 1번째")).toBeTruthy();
  expect(screen.queryByText(error)).toBeNull();
  expect(screen.queryByRole("button", { name: "다시 시도" })).toBeNull();
});

// Summarizing starts again at `chunks` 0, without any chunk planning.
test("a summarizing meeting writes under a sweep, with the request count and value past one request", async () => {
  mock({ meta: meta({ status: "summarizing", title: "" }), summary: "" });
  const { rerender } = render(<Meeting id={id} {...props} progress={progress({})} />);

  expect(await screen.findByText("요약을 쓰고 있어요")).toBeTruthy();
  // Nothing to count yet, so no value.
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(screen.queryByText("구간 나누는 중")).toBeNull();

  rerender(<Meeting id={id} {...props} progress={progress({ chunk: 1, chunks: 3, percent: 33 })} />);
  expect(await screen.findByText("요약을 쓰고 있어요 · 2/3")).toBeTruthy();
  expect(screen.getByRole("progressbar", { name: "요약 진행률" }).getAttribute("aria-valuenow")).toBe("33");

  // The last event comes before the meeting is read again: it is no chunk of a transcription.
  rerender(
    <Meeting id={id} {...props} progress={progress({ status: "summarize_failed", chunk: 1, chunks: 1, percent: 100 })} />,
  );
  await waitFor(() => expect(screen.queryByRole("progressbar")).toBeNull());
  expect(screen.queryByText(/구간 \d/)).toBeNull();
});

// Design-system 6.6: no other screen; the summary takes the skeleton's place.
test("the summary arrives in place once ready, as a document to edit", async () => {
  const detail = { meta: meta({ status: "summarizing", title: "" }), summary: "" };
  mock(detail);
  const { rerender } = render(<Meeting id={id} {...props} progress={progress({})} />);
  expect(await screen.findByText("요약을 쓰고 있어요")).toBeTruthy();
  // The place for the title says it is not there yet, in ink-2.
  expect(screen.getByRole("heading", { level: 1, name: "제목은 요약 뒤에 붙어요" }).className).toMatch(/text-ink-2/);

  Object.assign(detail, { meta: meta({ status: "review" }), summary: "## 결정사항\n- 미정" });
  rerender(<Meeting id={id} {...props} progress={progress({ status: "review" })} />);
  expect(await screen.findByRole("heading", { level: 2, name: "결정사항" })).toBeTruthy();
  expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull();
  expect((await openEditor()).value).toBe("## 결정사항\n- 미정");
  expect(textbox("제목").value).toBe("주간 회의");
  expect(screen.queryByText("요약을 쓰고 있어요")).toBeNull();
  expect(await sendButton("#회의록으로 보내기")).toBeTruthy();
});

// SUM-06: the meeting's own connection is gone, so the default is offered first; another can be
// picked. Each says where it sends the transcript (SUM-07).
test("요약 다시 실행 queues the failed summary with the picked connection", async () => {
  const error = "LLM 응답 시간 초과입니다. 연결을 확인하고 다시 실행해 주세요";
  const failed = meta({ status: "summarize_failed", error, llm_connection: "지운 연결" });
  const calls = mock({ meta: failed, summary: "" }, (cmd) => {
    if (cmd === "get_settings") {
      return settings({ llm_connections: [connection("회사 클라우드"), connection("로컬")], default_llm: "로컬" });
    }
    if (cmd === "retry_summary") return { ...failed, llm_connection: "회사 클라우드" };
  });
  render(<Meeting id={id} {...props} />);

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
    expect(calls.find((c) => c.cmd === "retry_summary")?.args).toEqual({ id, connection: "회사 클라우드" }),
  );
});

test("without a connection, the hint opens the 요약(LLM) tab", async () => {
  const opened: string[] = [];
  const detail = { meta: meta({ status: "transcribed", llm_connection: null }), summary: "" };
  mock(detail, (cmd) => (cmd === "get_transcript" ? [] : undefined));
  const onSetup = (tab: string) => opened.push(tab);
  // Read as `transcribed` while the last transcribing event is still here: summarizing comes next.
  const between = render(<Meeting id={id} {...props} onSetup={onSetup} progress={progress({ status: "transcribing" })} />);
  const steps = await screen.findByRole("list", { name: "처리 단계" });
  expect(screen.queryByText("LLM 연결을 추가하면 요약합니다")).toBeNull();
  // Still at 전사, as the event says, not at 요약 with no mark.
  expect(within(steps).getAllByRole("listitem").find((li) => li.getAttribute("aria-current"))?.textContent).toBe(
    "전사",
  );
  between.unmount();

  const first = render(<Meeting id={id} {...props} onSetup={onSetup} />);
  expect(await screen.findByText("LLM 연결을 추가하면 요약합니다")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM) 설정" }));
  expect(opened).toEqual(["llm"]);
  first.unmount();

  // A failed summary whose connections were all removed.
  detail.meta = meta({ status: "summarize_failed" });
  render(<Meeting id={id} {...props} onSetup={onSetup} />);
  expect(await screen.findByText("LLM 연결을 추가하면 요약을 다시 실행할 수 있습니다")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "요약 다시 실행" })).toBeNull();
});
