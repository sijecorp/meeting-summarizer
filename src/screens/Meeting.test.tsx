import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { hhmm } from "../components/common";
import type {
  Channel,
  ConnectionItem,
  LlmConnection,
  LoadedSettings,
  MeetingDetail,
  Meta,
  ModelFileState,
  Progress,
  Segment,
  Settings,
  Status,
} from "../lib/ipc";
import Meeting from "./Meeting";
import { parseDoc, SENT_SHOWN_MS } from "./MeetingParts";
import lines from "../../src-tauri/tests/fixtures/summary-lines.json";

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
  languages: ["ko"],
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
    slack_team: "SIJE",
    last_channel: "C1",
    glossary: [],
    transcribe_languages: ["ko"],
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

// `transcribe_languages`, in Rust's table order.
const LANGS = [
  { code: "ko", name: "한국어" },
  { code: "en", name: "영어" },
  { code: "vi", name: "베트남어" },
  { code: "id", name: "인도네시아어" },
];

const CHANNELS: Channel[] = [
  { id: "C1", name: "회의록", private: false },
  { id: "C2", name: "개발", private: false },
  { id: "G1", name: "경영", private: true },
];

/** `get_meeting` answers with `detail` as it is then (no send out unless it says so); `handler`
 * answers the rest (signed in, #회의록 sent to last, unless it says otherwise), and every call is
 * kept. */
const mock = (
  detail: Omit<MeetingDetail, "sending"> & { sending?: boolean },
  handler: (cmd: string, args: Call["args"]) => unknown = () => undefined,
) => {
  const calls: Call[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: args as Call["args"] });
      if (cmd === "get_meeting") return { sending: false, ...detail };
      const fallback: Record<string, unknown> = {
        get_settings: settings(),
        slack_channels: CHANNELS,
        transcribe_languages: LANGS,
      };
      return handler(cmd, args as Call["args"]) ?? fallback[cmd];
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

// Our own line parser (architecture 10.4): the prompt's shapes, and everything else as typed. The
// grounding check's parser (grounding.rs) reads the same example lines, so the two cannot drift.
test("parses every shared example line as the grounding check does", () => {
  expect(lines.length).toBeGreaterThan(20);
  for (const { line, block } of lines) expect(parseDoc(line), JSON.stringify(line)).toEqual([{ line: 0, ...block }]);
  // Each block keeps its line, so a click can put the caret there.
  expect(parseDoc("## 결정사항\n\n- 없음").map((b) => b.line)).toEqual([0, 1, 2]);
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

// Plan 5.6: the grounding check makes 미정 only the names nobody said, so 미정 can sit among names.
test("미정 among an item's owners is on the marker, the names beside it are not", async () => {
  mock({ meta: meta(), summary: "## 액션 아이템\n- [김지훈, 미정] 회의실 예약 (10월 18일)" });
  render(<Meeting id={id} {...props} />);
  // The document's item, not the 미정 list's.
  const heading = await screen.findByRole("heading", { level: 2, name: "액션 아이템" });
  const doc = heading.closest("[class*=whitespace-pre-wrap]") as HTMLElement;
  const item = within(doc).getByRole("listitem");
  const marks = [...item.querySelectorAll("mark")];
  expect(marks.map((m) => m.textContent)).toEqual(["미정"]);
  expect(marks[0].className).toMatch(/bg-marker/);
  const chip = marks[0].parentElement!;
  expect(chip.textContent).toBe("김지훈, 미정");
  expect(chip.className).toMatch(/bg-sunken/);
});

// Plan 5.6, ENG11: a summary the grounding check could not read is not passed as checked.
test("says when the grounding check could not read the action items, and only then", async () => {
  const notice = "액션 아이템 형식을 읽지 못해 근거 검사를 하지 못했습니다";
  for (const grounding_checked of [false, true, undefined]) {
    id = `${id}x`;
    mock({ meta: meta({ grounding_checked }), summary: "## 액션 아이템\n철수: 대시보드" });
    render(<Meeting id={id} {...props} />);
    await screen.findByRole("heading", { level: 2, name: "액션 아이템" });
    expect(!!screen.queryByText(notice), String(grounding_checked)).toBe(grounding_checked === false);
    cleanup();
  }
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
test("no speech: a warning and no editor or send; only 녹음 파일 저장, 전사 언어 바꾸기…, 전송 안 함 and 삭제 remain", async () => {
  mock({ meta: meta({ no_speech: true, llm_connection: null, title: "" }), summary: "" });

  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("음성이 거의 감지되지 않았습니다")).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["홈으로", "전사본", ""]);
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
  expect((await menu()).getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
    "녹음 파일 저장",
    "전사 언어 바꾸기…",
    "전송 안 함",
    "삭제",
  ]);
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
  expect(screen.getByRole("heading", { level: 1, name: "제목 준비 중" })).toBeTruthy();
  expect(screen.getByText("열심히 듣는 중…")).toBeTruthy();
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

// SND-04: the channel sent to last is ready while the recorder is still in it; else they pick one.
test("the send button names the channel sent to last, else waits for a pick", async () => {
  // The particle follows the channel's last syllable: 록 → 으로, 발 (ㄹ) → 로.
  const cases: [Partial<Settings>, string, string][] = [
    [{}, "#회의록", "#회의록으로 보내기"],
    [{ last_channel: "C2" }, "#개발", "#개발로 보내기"],
    [{ last_channel: "C9" }, "채널 고르기", "Slack으로 보내기"],
    [{ last_channel: null }, "채널 고르기", "Slack으로 보내기"],
  ];
  for (const [patch, picked, button] of cases) {
    mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_settings" ? settings(patch) : undefined));
    render(<Meeting id={id} {...props} />);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "보낼 채널" }).textContent).toBe(picked));
    expect((await sendButton(button)).disabled).toBe(picked === "채널 고르기");
    cleanup();
  }
});

test("sends to the channel picked, with the title, body and the start's UTC offset", async () => {
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "send_to_slack") return meta({ status: "sent" });
  });
  render(<Meeting id={id} {...props} />);

  await sendButton("#회의록으로 보내기");
  fireEvent.keyDown(screen.getByRole("combobox", { name: "보낼 채널" }), { key: "Enter" });
  // A private channel says so in words, not by its icon alone.
  expect(await screen.findByRole("option", { name: "#경영, 비공개 채널" })).toBeTruthy();
  fireEvent.click(screen.getByRole("option", { name: "#개발" }));
  fireEvent.click(await sendButton("#개발로 보내기"));

  await waitFor(() => expect(calls.map((c) => c.cmd)).toContain("send_to_slack"));
  expect(calls.find((c) => c.cmd === "send_to_slack")?.args).toEqual({
    id,
    channel: CHANNELS[1],
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

test("signed out: the send is off and the Slack sign-in is one click away", async () => {
  const tabs: string[] = [];
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) =>
    cmd === "get_settings" ? settings({ slack_team: null }) : undefined,
  );
  render(<Meeting id={id} {...props} onSetup={(tab) => tabs.push(tab)} />);

  fireEvent.click(await screen.findByRole("button", { name: "Slack 로그인하기" }));
  expect(tabs).toEqual(["slack"]);
  expect((screen.getByRole("button", { name: "Slack으로 보내기" }) as HTMLButtonElement).disabled).toBe(true);
  expect(calls.map((c) => c.cmd)).not.toContain("slack_channels");
});

test("channels that cannot be read say why, and nothing can be sent", async () => {
  mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "slack_channels") throw "Slack 응답을 받지 못했습니다. 잠시 후 다시 시도해 주세요";
  });
  render(<Meeting id={id} {...props} />);

  expect(
    await screen.findByText("Slack 채널 목록을 읽지 못했습니다: Slack 응답을 받지 못했습니다. 잠시 후 다시 시도해 주세요"),
  ).toBeTruthy();
  // Not still reading.
  expect(screen.getByRole("combobox", { name: "보낼 채널" }).textContent).toBe("채널 없음");
  expect((screen.getByRole("button", { name: "Slack으로 보내기" }) as HTMLButtonElement).disabled).toBe(true);
});

// Rust forgets a sign-in Slack no longer takes; the bar then reads the settings again.
test("an ended sign-in turns the bar back to the sign-in link", async () => {
  let signedIn = true;
  mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "get_settings") return settings({ slack_team: signedIn ? "SIJE" : null });
    if (cmd === "send_to_slack") {
      signedIn = false;
      throw "Slack 로그인이 필요합니다. 설정 → Slack에서 다시 로그인해 주세요";
    }
  });
  render(<Meeting id={id} {...props} />);

  fireEvent.click(await sendButton("#회의록으로 보내기"));
  expect(await screen.findByText("Slack 로그인이 필요합니다. 설정 → Slack에서 다시 로그인해 주세요")).toBeTruthy();
  expect(await screen.findByRole("button", { name: "Slack 로그인하기" })).toBeTruthy();
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
  const cause = "채널이 없거나 보관됐습니다(channel_not_found). 다른 채널을 골라 주세요";
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

// Plan 5.4: the recording as one WAV, from ⋯.
test("녹음 파일 저장 sits after .md로 저장, shows without a summary, and hides while recording or deleting", async () => {
  mock({ meta: meta(), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  await screen.findByRole("button", { name: "고치기" });
  expect((await menu()).getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
    "요약 복사",
    ".md로 저장",
    "녹음 파일 저장",
    "전사 언어 바꾸기…",
    "전송 안 함",
    "삭제",
  ]);
  cleanup();

  for (const status of ["recorded", "transcribe_failed", "recording", "deleting", "unknown"] as Status[]) {
    mock({ meta: meta({ status, llm_connection: null }), summary: "" });
    render(<Meeting id={id} {...props} />);
    await screen.findByRole("button", { name: "더보기" });
    const items = (await menu()).getAllByRole("menuitem").map((i) => i.textContent);
    expect(items.includes("녹음 파일 저장"), status).toBe(!["recording", "deleting", "unknown"].includes(status));
    cleanup();
  }
});

test("녹음 파일 저장 says it is saving, waits, and names where it went; a cancel says nothing", async () => {
  const replies: ((value: unknown) => void)[] = [];
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd) => {
    if (cmd === "export_recording") return new Promise((resolve) => replies.push(resolve));
  });
  render(<Meeting id={id} {...props} />);
  await screen.findByRole("button", { name: "고치기" });

  await pick("녹음 파일 저장");
  expect(await screen.findByText("녹음 파일 저장 중…")).toBeTruthy();
  expect(disabled((await menu()).getByRole("menuitem", { name: "녹음 파일 저장" }))).toBe(true);
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
  await act(async () => replies.shift()!({ path: "/Users/me/주간 회의.wav", mixed: true }));
  const said = screen.getByText("녹음 파일(두 소리 합침)을 저장했습니다: /Users/me/주간 회의.wav");
  expect(said.getAttribute("role")).toBe("status");

  await pick("녹음 파일 저장");
  await waitFor(() => expect(replies).toHaveLength(1));
  await act(async () => replies.shift()!({ path: "/Users/me/가져온 녹음.wav", mixed: false }));
  expect(screen.getByText("녹음 파일을 저장했습니다: /Users/me/가져온 녹음.wav")).toBeTruthy();

  await pick("녹음 파일 저장");
  await waitFor(() => expect(replies).toHaveLength(1));
  await act(async () => replies.shift()!(null));
  expect(screen.queryByText(/^녹음 파일/)).toBeNull();
  expect(calls.filter((c) => c.cmd === "export_recording").map((c) => c.args.id)).toEqual([id, id, id]);
});

test("a recording that cannot be saved says why above the document", async () => {
  for (const cause of [
    "필요 약 115MB, 남은 공간 40MB. 다른 폴더를 골라 주세요",
    "회의가 삭제되어 저장하지 못했습니다",
    "녹음 파일을 쓰지 못했습니다: Permission denied (os error 13)",
  ]) {
    mock({ meta: meta(), summary: "요약" }, (cmd) => {
      if (cmd === "export_recording") throw cause;
    });
    render(<Meeting id={id} {...props} />);
    await screen.findByRole("button", { name: "고치기" });
    await pick("녹음 파일 저장");
    expect(await screen.findByText(cause)).toBeTruthy();
    expect(screen.queryByText("녹음 파일 저장 중…")).toBeNull();
    expect(disabled((await menu()).getByRole("menuitem", { name: "녹음 파일 저장" }))).toBe(false);
    cleanup();
  }
});

// Plan 5.5: more channels for a sent summary, as it is.
const sentMeta = (...channels: [string, string][]) =>
  meta({
    status: "sent",
    sent: { channel: channels[0][0], channel_id: channels[0][1], at: "2026-10-13T05:43:00Z", edit_count: 2 },
    also_sent: channels.slice(1).map(([channel, channel_id], i) => ({
      channel,
      channel_id,
      at: `2026-10-13T06:0${i}:00Z`,
      edit_count: 2,
    })),
  });

test("the meta line names two channels, and three or more open a list from the keyboard", async () => {
  mock({ meta: sentMeta(["#회의록", "C1"], ["#개발", "C2"]), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText(`#회의록, #개발로 보냄 · ${hhmm("2026-10-13T05:43:00Z")} · 수정 2곳`)).toBeTruthy();
  cleanup();

  mock({ meta: sentMeta(["#회의록", "C1"], ["#개발", "C2"], ["#경영", "G1"]), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  const more = await screen.findByRole("button", { name: "#회의록 외 2곳에 보냄" });
  expect(more.getAttribute("title")).toBeNull();
  more.focus();
  fireEvent.keyDown(more, { key: "Enter" });
  fireEvent.click(more);
  const list = within(await screen.findByRole("dialog", { name: "보낸 채널" }));
  expect(list.getAllByRole("listitem").map((li) => li.textContent)).toEqual([
    `#회의록${hhmm("2026-10-13T05:43:00Z")}`,
    `#개발${hhmm("2026-10-13T06:00:00Z")}`,
    `#경영${hhmm("2026-10-13T06:01:00Z")}`,
  ]);
});

test("a sent meeting's bar offers the other channels, says the summary is fixed while one is picked, and comes back after sending", async () => {
  const at = "2026-10-13T06:10:00Z";
  const calls = mock({ meta: sentMeta(["#회의록", "C1"]), summary: "요약" }, (cmd) => {
    if (cmd === "send_to_slack")
      return { ...sentMeta(["#회의록", "C1"]), also_sent: [{ channel: "#개발", channel_id: "C2", at, edit_count: 2 }] };
  });
  render(<Meeting id={id} {...props} />);

  const picker = await screen.findByRole("combobox", { name: "보낼 채널" });
  await waitFor(() => expect(picker.textContent).toBe("다른 채널 고르기"));
  expect((await sendButton("보내기")).disabled).toBe(true);
  expect(screen.queryByText(/^보낸 요약은 고칠 수 없습니다/)).toBeNull();
  fireEvent.keyDown(picker, { key: "Enter" });
  expect((await screen.findByRole("option", { name: "#회의록, 보냄" })).getAttribute("aria-disabled")).toBe("true");
  fireEvent.click(screen.getByRole("option", { name: "#개발" }));
  expect(screen.getByText("보낸 요약은 고칠 수 없습니다. 다른 채널에도 같은 내용이 갑니다")).toBeTruthy();

  vi.useFakeTimers({ shouldAdvanceTime: true });
  const send = await sendButton("보내기");
  send.focus();
  fireEvent.click(send);
  expect(await screen.findByRole("button", { name: "#개발로 보냄" })).toBeTruthy();
  expect(screen.getByText("#개발로 보냈습니다")).toBeTruthy();
  expect(screen.queryByText(/^보낸 요약은 고칠 수 없습니다/)).toBeNull();
  expect(calls.find((c) => c.cmd === "send_to_slack")?.args.channel).toEqual(CHANNELS[1]);
  expect(await screen.findByText(`#회의록, #개발로 보냄 · ${hhmm("2026-10-13T05:43:00Z")} · 수정 2곳`)).toBeTruthy();

  await tick(SENT_SHOWN_MS);
  const back = await screen.findByRole("combobox", { name: "보낼 채널" });
  expect(back.textContent).toBe("다른 채널 고르기");
  expect(document.activeElement).toBe(back);
  expect(screen.queryByText("#개발로 보냈습니다")).toBeNull();
});

// After the last other channel, neither the picker nor the button is left: the focus goes to the
// words saying so, not to the page.
test("sending to the last other channel leaves the focus on the words saying none is left", async () => {
  const at = "2026-10-13T06:10:00Z";
  mock({ meta: sentMeta(["#회의록", "C1"], ["#개발", "C2"]), summary: "요약" }, (cmd) => {
    if (cmd === "send_to_slack")
      return {
        ...sentMeta(["#회의록", "C1"], ["#개발", "C2"]),
        also_sent: [
          { channel: "#개발", channel_id: "C2", at, edit_count: 2 },
          { channel: "#경영", channel_id: "G1", at, edit_count: 2 },
        ],
      };
  });
  render(<Meeting id={id} {...props} />);
  const picker = await screen.findByRole("combobox", { name: "보낼 채널" });
  await waitFor(() => expect(picker.textContent).toBe("다른 채널 고르기"));
  fireEvent.keyDown(picker, { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: "#경영, 비공개 채널" }));

  vi.useFakeTimers({ shouldAdvanceTime: true });
  const send = await sendButton("보내기");
  send.focus();
  fireEvent.click(send);
  expect(await screen.findByRole("button", { name: "#경영으로 보냄" })).toBeTruthy();
  await tick(SENT_SHOWN_MS);
  const none = await screen.findByText("보낼 수 있는 다른 채널이 없습니다");
  expect(document.activeElement).toBe(none);
});

test("a sent meeting with no other channel says so, and offers no button", async () => {
  mock({ meta: sentMeta(["#회의록", "C1"], ["#개발", "C2"], ["#경영", "G1"]), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("보낼 수 있는 다른 채널이 없습니다")).toBeTruthy();
  expect(screen.queryByRole("combobox", { name: "보낼 채널" })).toBeNull();
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
});

test("a failed send to another channel keeps the channel for 다시 보내기, with the cause alone in the banner", async () => {
  const cause = "채널이 없거나 보관됐습니다(channel_not_found). 다른 채널을 골라 주세요";
  mock({ meta: sentMeta(["#회의록", "C1"]), summary: "요약" }, (cmd) => {
    if (cmd === "send_to_slack") throw cause;
  });
  render(<Meeting id={id} {...props} />);

  fireEvent.keyDown(await screen.findByRole("combobox", { name: "보낼 채널" }), { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: "#개발" }));
  fireEvent.click(await sendButton("보내기"));
  expect(await screen.findByText(cause)).toBeTruthy();
  expect((await sendButton("다시 보내기")).disabled).toBe(false);
  expect(screen.getByRole("combobox", { name: "보낼 채널" }).textContent).toBe("#개발");
  // A read after the send (its `slack://finished`) does not take the cause away.
  await act(() => emit("slack://finished", id));
  expect(screen.getByText(cause)).toBeTruthy();
});

test("a sent meeting's bar says when the channels cannot be read and reads them again", async () => {
  let fail = true;
  const calls = mock({ meta: sentMeta(["#회의록", "C1"]), summary: "요약" }, (cmd) => {
    if (cmd === "slack_channels" && fail) throw "Slack 응답을 받지 못했습니다";
  });
  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("Slack 채널 목록을 읽지 못했습니다")).toBeTruthy();
  // Not in the banner: that is a failed send's alone.
  expect(screen.queryByText(/Slack 응답을 받지 못했습니다/)).toBeNull();
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
  await waitFor(() => expect(screen.getByRole("combobox", { name: "보낼 채널" }).textContent).toBe("다른 채널 고르기"));
  expect(calls.filter((c) => c.cmd === "slack_channels")).toHaveLength(2);
});

test("a sent meeting signed out of Slack offers the sign-in in its bar", async () => {
  mock({ meta: sentMeta(["#회의록", "C1"]), summary: "요약" }, (cmd) =>
    cmd === "get_settings" ? settings({ slack_team: null }) : undefined,
  );
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByRole("button", { name: "Slack 로그인하기" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /보내기$/ })).toBeNull();
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

// Plan 5.1-12, 5.3-15: one reason under the memo, with its icon; a CPU reason takes the device off
// the memo.
test("a slow meeting says why under its memo", async () => {
  const transcribing = (slow: Progress["slow"]) =>
    progress({ status: "transcribing", chunk: 1, chunks: 5, percent: 42, eta_secs: 3_900, slow });
  mock({ meta: meta({ status: "transcribing", title: "", backend: "cpu" }), summary: "" });
  const { container, rerender } = render(<Meeting id={id} {...props} progress={transcribing(null)} />);
  expect(await screen.findByText("구간 2/5 · 약 1시간 5분 남음 · 처리 장치 CPU")).toBeTruthy();

  const cases = [
    ["saver", "에너지 절약 모드라 처리가 느릴 수 있습니다. 전원을 연결하면 빨라질 수 있습니다", "battery-low", true],
    ["cpu", "GPU를 쓰지 못해 CPU로 처리하고 있어 느립니다. 그래픽 드라이버를 업데이트하면 나아질 수 있습니다", "cpu", false],
    // Plan 5.3-15: what the recorder can do about it, in 1.1.0.
    ["cpu_nogpu", "이 PC에는 쓸 수 있는 GPU가 없어 CPU로 처리합니다 · 일시정지하고 나중에 이어서 처리할 수 있습니다", "cpu", false],
    ["slow", "이 PC에서는 전사가 회의 길이보다 오래 걸립니다 · 일시정지하고 나중에 이어서 처리할 수 있습니다", "cpu", true],
  ] as const;
  for (const [slow, copy, icon, device] of cases) {
    rerender(<Meeting id={id} {...props} progress={transcribing(slow)} />);
    const line = await screen.findByText(copy);
    expect(line.querySelector(`svg.lucide-${icon}`)).toBeTruthy();
    // Not a live region: it changes with the pace.
    expect(line.closest("[role=status], [aria-live]")).toBeNull();
    const memo = device ? "구간 2/5 · 약 1시간 5분 남음 · 처리 장치 CPU" : "구간 2/5 · 약 1시간 5분 남음";
    expect(screen.getByText(memo)).toBeTruthy();
  }
  expect(container.querySelectorAll("svg.lucide-battery-low, svg.lucide-cpu")).toHaveLength(1);
});

// Plan 5.1-12: only saver goes on into the summary.
test("a summary drops the transcription's slow reasons but not saver", async () => {
  const cpu = "GPU를 쓰지 못해 CPU로 처리하고 있어 느립니다. 그래픽 드라이버를 업데이트하면 나아질 수 있습니다";
  const saver = "에너지 절약 모드라 처리가 느릴 수 있습니다. 전원을 연결하면 빨라질 수 있습니다";
  mock({ meta: meta({ status: "summarizing", title: "", backend: "cpu" }), summary: "" });
  const { rerender } = render(<Meeting id={id} {...props} progress={progress({ slow: "cpu" })} />);

  expect(await screen.findByText("처리 장치 CPU")).toBeTruthy();
  expect(screen.queryByText(cpu)).toBeNull();
  rerender(<Meeting id={id} {...props} progress={progress({ slow: "slow" })} />);
  expect(screen.queryByText("이 PC에서는 전사가 회의 길이보다 오래 걸립니다")).toBeNull();
  rerender(<Meeting id={id} {...props} progress={progress({ slow: "saver" })} />);
  expect(await screen.findByText(saver)).toBeTruthy();
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
  expect(screen.getByRole("heading", { level: 1, name: "제목 준비 중" }).className).toMatch(/text-ink-2/);
  expect(screen.getByText("열심히 읽는 중…")).toBeTruthy();

  Object.assign(detail, { meta: meta({ status: "review" }), summary: "## 결정사항\n- 미정" });
  rerender(<Meeting id={id} {...props} progress={progress({ status: "review" })} />);
  expect(await screen.findByRole("heading", { level: 2, name: "결정사항" })).toBeTruthy();
  expect(screen.queryByRole("textbox", { name: "요약" })).toBeNull();
  expect((await openEditor()).value).toBe("## 결정사항\n- 미정");
  expect(textbox("제목").value).toBe("주간 회의");
  expect(screen.queryByText("요약을 쓰고 있어요")).toBeNull();
  expect(await sendButton("#회의록으로 보내기")).toBeTruthy();
});

// What `list_connections` offers: the saved connections, then this PC (plan 5.4).
const item = (name: string, is_local = false, host = "api.example.com"): ConnectionItem => ({
  name,
  label: name,
  host,
  is_local,
  builtin: false,
});
const THIS_PC: ConnectionItem = { name: "builtin", label: "이 PC", host: "", is_local: true, builtin: true };
const GGUF = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
const summaryModel = (patch: Partial<ModelFileState> = {}) => ({
  files: [{ file: GGUF, size: 2_497_281_120, installed: false, received: 0, downloading: false, ...patch }],
  downloading: !!patch.downloading,
});
const picker = () => screen.findByRole("combobox", { name: "요약할 LLM 연결" });
const choose = async (option: string) => {
  fireEvent.keyDown(await picker(), { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: option }));
};

// SUM-06: the meeting's own connection is gone, so a local default is offered first; another can
// be picked. Each says where it sends the transcript (SUM-07), this PC by its name alone.
test("요약 다시 실행 queues the failed summary with the picked connection", async () => {
  const error = "LLM 응답 시간 초과입니다. 연결을 확인하고 다시 실행해 주세요";
  const failed = meta({ status: "summarize_failed", error, llm_connection: "지운 연결" });
  const calls = mock({ meta: failed, summary: "" }, (cmd) => {
    if (cmd === "get_settings") {
      return settings({ llm_connections: [connection("회사 클라우드"), connection("로컬")], default_llm: "로컬" });
    }
    if (cmd === "list_connections") return [item("회사 클라우드"), item("로컬", true), THIS_PC];
    if (cmd === "retry_summary") return { ...failed, llm_connection: "회사 클라우드" };
  });
  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText(error)).toBeTruthy();
  const select = await picker();
  await waitFor(() => expect(select.textContent).toBe("로컬 (이 PC)"));
  fireEvent.keyDown(select, { key: "Enter" });
  expect(await screen.findByRole("option", { name: "이 PC (요약 모델 없음)" })).toBeTruthy();
  fireEvent.click(await screen.findByRole("option", { name: "회사 클라우드 (api.example.com로 전송)" }));
  await waitFor(() => expect(select.textContent).toBe("회사 클라우드 (api.example.com로 전송)"));
  // A key problem is fixed in settings, so that stays in reach too.
  expect(screen.getByRole("button", { name: "요약(LLM) 설정" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "요약 다시 실행" }));

  await waitFor(() =>
    expect(calls.find((c) => c.cmd === "retry_summary")?.args).toEqual({ id, connection: "회사 클라우드" }),
  );
});

// Plan 5.4: a failed summary on this PC is offered on this PC again once its model is in, never
// moved to the cloud default unasked; without the model nothing is picked for the recorder.
test("a failed summary on this PC offers this PC once its model is in, and nothing without it", async () => {
  const refused = "이 PC 요약 모델이 아직 없습니다. 설정 → 요약(LLM)에서 요약 모델을 먼저 받아 주세요";
  const failed = meta({ status: "summarize_failed", error: "요약이 길이 제한에서 끊겼습니다", llm_connection: "builtin" });
  let installed = true;
  const calls = mock({ meta: failed, summary: "" }, (cmd) => {
    if (cmd === "get_settings") return settings({ llm_connections: [connection("회사 클라우드")], default_llm: "회사 클라우드" });
    if (cmd === "list_connections") return [item("회사 클라우드"), THIS_PC];
    if (cmd === "model_status") return summaryModel({ installed });
    if (cmd === "retry_summary") throw refused;
  });
  const ready = render(<Meeting id={id} {...props} />);

  await waitFor(async () => expect((await picker()).textContent).toBe("이 PC"));
  fireEvent.click(screen.getByRole("button", { name: "요약 다시 실행" }));
  // Rust has the last word (the model may have gone meanwhile), and its words show.
  expect(await screen.findByText(refused)).toBeTruthy();
  expect(calls.find((c) => c.cmd === "retry_summary")?.args).toEqual({ id, connection: "builtin" });
  ready.unmount();

  installed = false;
  render(<Meeting id={id} {...props} />);
  await waitFor(async () => expect((await picker()).textContent).toBe("연결을 골라 주세요"));
  expect((screen.getByRole("button", { name: "요약 다시 실행" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.keyDown(await picker(), { key: "Enter" });
  const thisPc = await screen.findByRole("option", { name: "이 PC (요약 모델 없음)" });
  expect(thisPc.getAttribute("aria-disabled")).toBe("true");
});

// Plan 5.2, DX6, ENG15: waiting for the summary model, the document says how far it is or where to
// get it, and another connection can be picked instead; then the notice follows the new route.
test("a meeting waiting for the summary model says so, and can take another connection", async () => {
  const opened: string[] = [];
  let model = summaryModel({ received: 999_000_000, downloading: true });
  const waiting = meta({ status: "transcribed", llm_connection: null, wait: "summary_model" });
  const waitingRoute = { connection: null, local: false, local_available: false, locked: false, toggle_matters: false };
  let route: unknown = waitingRoute;
  const calls = mock({ meta: waiting, summary: "" }, (cmd) => {
    if (cmd === "get_settings") return settings({ llm_connections: [connection("회사 클라우드")] });
    if (cmd === "list_connections") return [item("회사 클라우드"), THIS_PC];
    if (cmd === "summary_route") return route;
    if (cmd === "model_status") return model;
    if (cmd === "get_transcript") return [];
    if (cmd === "retry_summary") {
      route = { ...waitingRoute, connection: connection("회사 클라우드"), locked: true };
      return { ...waiting, llm_connection: "회사 클라우드", wait: undefined };
    }
  });
  const downloading = render(<Meeting id={id} {...props} onSetup={(tab) => opened.push(tab)} />);

  const notice = "요약 모델 받는 중 40% · 다 받으면 자동으로 요약합니다";
  expect(await screen.findByText(notice)).toBeTruthy();
  // Nothing to set up while it downloads, and this PC is not offered before its model is in.
  expect(screen.queryByRole("button", { name: "요약(LLM) 설정" })).toBeNull();
  await waitFor(async () => expect((await picker()).textContent).toBe("연결을 골라 주세요"));
  expect((screen.getByRole("button", { name: "이 연결로 요약" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.keyDown(await picker(), { key: "Enter" });
  expect((await screen.findByRole("option", { name: "이 PC (요약 모델 받는 중)" })).getAttribute("aria-disabled")).toBe(
    "true",
  );
  fireEvent.click(await screen.findByRole("option", { name: "회사 클라우드 (api.example.com로 전송)" }));
  fireEvent.click(screen.getByRole("button", { name: "이 연결로 요약" }));
  await waitFor(() =>
    expect(calls.find((c) => c.cmd === "retry_summary")?.args).toEqual({ id, connection: "회사 클라우드" }),
  );
  // Same status, new pin: the route is read again and no longer waits for the model.
  await waitFor(() => expect(screen.queryByText(notice)).toBeNull());
  downloading.unmount();

  model = summaryModel();
  route = waitingRoute;
  render(<Meeting id={id} {...props} onSetup={(tab) => opened.push(tab)} />);
  expect(await screen.findByText("설정 → 요약(LLM)에서 요약 모델을 받아 주세요")).toBeTruthy();
  // One way there, beside the words.
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM) 설정" }));
  expect(opened).toEqual(["llm"]);
  expect(screen.getByRole("button", { name: "이 연결로 요약" })).toBeTruthy();
});

// ENG15: pinned to a connection since deleted, it waits and asks for another; never sent elsewhere
// by itself. Rust checks the pick and its refusal shows. Paused, it is out of line all the same.
test("a meeting whose connection was deleted asks for another, and a refused pick says why", async () => {
  const refused = "LLM 연결 '회사 클라우드'을(를) 찾을 수 없습니다. 다른 연결을 골라 주세요";
  const waiting = meta({ status: "paused", llm_connection: "지운 연결", wait: "connection_deleted" });
  const calls = mock({ meta: waiting, summary: "" }, (cmd) => {
    if (cmd === "get_settings") return settings({ llm_connections: [connection("회사 클라우드")], default_llm: "회사 클라우드" });
    if (cmd === "list_connections") return [item("회사 클라우드"), THIS_PC];
    if (cmd === "summary_route") return { connection: null, local: false, local_available: false, locked: true, toggle_matters: false };
    if (cmd === "model_status") return summaryModel({ installed: true });
    if (cmd === "get_transcript") return [];
    if (cmd === "retry_summary") throw refused;
  });
  render(<Meeting id={id} {...props} />);

  expect(await screen.findByText("이 회의에 정한 연결이 삭제됐습니다. 다른 연결을 골라 주세요")).toBeTruthy();
  // Another connection may still have to be added.
  expect(screen.getByRole("button", { name: "요약(LLM) 설정" })).toBeTruthy();
  expect(screen.queryByText(/대기 중/)).toBeNull();
  // Not the cloud default: this PC, its model in, until another is picked.
  await waitFor(async () => expect((await picker()).textContent).toBe("이 PC"));
  await choose("회사 클라우드 (api.example.com로 전송)");
  fireEvent.click(screen.getByRole("button", { name: "이 연결로 요약" }));
  expect(await screen.findByText(refused)).toBeTruthy();
  expect(calls.find((c) => c.cmd === "retry_summary")?.args).toEqual({ id, connection: "회사 클라우드" });
});

// Plan 5.3: while the engine loads, the document says so with its seconds, not a stuck 0/0.
test("a summary on this PC says while its engine starts", async () => {
  mock({ meta: meta({ status: "summarizing", title: "", llm_connection: "builtin" }), summary: "" });
  const since = Math.floor(Date.now() / 1000) - 5;
  const { rerender } = render(<Meeting id={id} {...props} progress={progress({ engine_since: since })} />);

  expect(await screen.findByText(/^요약 엔진 준비 중 · [5-7]초$/)).toBeTruthy();
  expect(screen.queryByText("요약을 쓰고 있어요")).toBeNull();
  rerender(<Meeting id={id} {...props} progress={progress({ engine_since: null })} />);
  expect(await screen.findByText("요약을 쓰고 있어요")).toBeTruthy();
});

// Design-system 6.6: the title's place says what is going on, a new line every 2 s, per stage.
test("until the title comes, its place turns over a line every 2 seconds, and screen readers hear one name", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  mock({ meta: meta({ status: "transcribing", title: "" }), summary: "" });
  const { rerender } = render(<Meeting id={id} {...props} progress={progress({ status: "transcribing" })} />);
  const heading = await screen.findByRole("heading", { level: 1, name: "제목 준비 중" });
  // The line on screen, once the one it replaced has faded out.
  const shows = (line: string) =>
    waitFor(() =>
      expect([...heading.querySelectorAll("[aria-hidden]")].map((s) => s.textContent)).toEqual([line]),
    );

  await shows("열심히 듣는 중…");
  await act(() => vi.advanceTimersByTimeAsync(2000));
  await shows("말을 글로 옮기는 중…");
  await act(() => vi.advanceTimersByTimeAsync(4000));
  // Around again.
  await shows("열심히 듣는 중…");

  // The summary started: its own lines, from the first.
  rerender(<Meeting id={id} {...props} progress={progress({ status: "summarizing" })} />);
  await shows("열심히 읽는 중…");
  expect(screen.getByRole("heading", { level: 1, name: "제목 준비 중" })).toBe(heading);

  // In line behind another meeting.
  cleanup();
  mock({ meta: meta({ status: "recorded", title: "" }), summary: "" });
  render(<Meeting id={id} {...props} progress={progress({ status: "recorded", position: 2 })} />);
  const waiting = await screen.findByRole("heading", { level: 1, name: "제목 준비 중" });
  expect(waiting.querySelector("[aria-hidden]")?.textContent).toBe("차례를 기다리는 중…");
});

// Plan 5.3-11: one button at the end of the step line whose words change, so the focus stays:
// 일시정지, 멈추는 중… until the run ends, then 이어서 처리. Each change is said once.
test("the processing area's button goes from 일시정지 to 멈추는 중… to 이어서 처리, keeping the focus", async () => {
  const running = progress({ status: "transcribing", chunk: 2, chunks: 7, percent: 30, eta_secs: null });
  const detail = {
    meta: meta({ status: "transcribing", title: "", chunks_done: 2, chunks_total: 7, llm_connection: null }),
    summary: "",
  };
  const calls = mock(detail, (cmd) => {
    if (cmd === "pause_meeting") {
      detail.meta = { ...detail.meta, held: true };
      return "pausing";
    }
    if (cmd === "resume_meeting") {
      detail.meta = { ...detail.meta, held: false };
      return detail.meta;
    }
  });
  const { rerender } = render(<Meeting id={id} {...props} progress={running} />);

  const button = await screen.findByRole("button", { name: "일시정지" });
  // At the end of the step line, above the memo.
  const steps = screen.getByRole("list", { name: "처리 단계" });
  expect(steps.parentElement?.contains(button)).toBe(true);
  expect(steps.parentElement?.nextElementSibling?.textContent).toBe("구간 3/7");
  button.focus();
  fireEvent.click(button);

  await waitFor(() => expect(button.textContent).toBe("멈추는 중…"));
  expect(disabled(button)).toBe(true);
  expect(document.activeElement).toBe(button);
  fireEvent.click(button);
  expect(calls.filter((c) => c.cmd === "pause_meeting")).toEqual([{ cmd: "pause_meeting", args: { id } }]);

  // The run ends `paused`.
  detail.meta = { ...detail.meta, status: "paused" };
  rerender(<Meeting id={id} {...props} progress={progress({ status: "paused" })} />);
  await waitFor(() => expect(button.textContent).toBe("이어서 처리"));
  expect(disabled(button)).toBe(false);
  expect(document.activeElement).toBe(button);
  expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
    "녹음완료",
    "전사일시정지됨",
    "요약",
    "검토",
  ]);
  expect(screen.getByText("구간 3/7에서 멈춤 · 이어서 처리하면 이 구간부터 다시 합니다")).toBeTruthy();
  // The bar stays where it stopped.
  expect(screen.getByRole("progressbar", { name: "전사 진행률" }).getAttribute("aria-valuenow")).toBe("29");
  expect((await screen.findByText("일시정지했습니다")).getAttribute("role")).toBe("status");

  fireEvent.click(button);
  await waitFor(() => expect(button.textContent).toBe("일시정지"));
  expect(calls.filter((c) => c.cmd === "resume_meeting")).toEqual([{ cmd: "resume_meeting", args: { id } }]);
  expect(document.activeElement).toBe(button);
  expect((await screen.findByText("이어서 처리합니다")).getAttribute("role")).toBe("status");
});

// Plan 5.3-10: 멈추는 중… comes from `held` and the running event, never from this screen's state.
test("a meeting still stopping says so after a redraw", async () => {
  const running = progress({ status: "summarizing", chunk: 1, chunks: 3, percent: 33 });
  mock({ meta: meta({ status: "summarizing", title: "", held: true }), summary: "" });
  render(<Meeting id={id} {...props} progress={running} />);
  expect(disabled(await screen.findByRole("button", { name: "멈추는 중…" }))).toBe(true);
  cleanup();
  render(<Meeting id={id} {...props} progress={running} />);
  expect(disabled(await screen.findByRole("button", { name: "멈추는 중…" }))).toBe(true);
});

// Plan 5.3-11: a pause the run beat to the end is no error; the meeting is just read again.
test("a pause that finds the meeting finished redraws it without a banner", async () => {
  const detail = { meta: meta({ status: "recorded", title: "" }), summary: "" };
  const calls = mock(detail, (cmd) => {
    if (cmd === "pause_meeting") {
      detail.meta = meta({ status: "review", title: "주간 회의" });
      detail.summary = "## 결정사항\n- 목요일에 배포한다";
      return "already_done";
    }
  });
  render(<Meeting id={id} {...props} progress={progress({ status: "recorded", position: 1 })} />);
  fireEvent.click(await screen.findByRole("button", { name: "일시정지" }));
  expect(await screen.findByText("목요일에 배포한다")).toBeTruthy();
  expect(calls.filter((c) => c.cmd === "get_meeting").length).toBeGreaterThan(1);
  expect(screen.queryByRole("list", { name: "처리 단계" })).toBeNull();
  expect(document.body.textContent).not.toContain("already_done");
  // No error banner (role=alert) for it either.
  expect(screen.queryByRole("alert")).toBeNull();
});

// Plan 5.3-12: the memo follows the step it stopped in, before the chunk plan too; 녹음 중에 이어서
// 처리하면 it waits for the recording.
test("a paused meeting's memo says where it stopped and what 이어서 처리 does", async () => {
  const cases = [
    [{ chunks_done: 5, chunks_total: 5, llm_connection: "회사 클라우드" }, "요약일시정지됨", "요약 중 멈춤 · 끝난 부분부터 이어서 합니다"],
    [{ chunks_done: 0, chunks_total: 0, llm_connection: null }, "전사일시정지됨", "구간을 나누기 전에 멈춤 · 이어서 처리하면 처음부터 합니다"],
    // A plan saved by 1.0.3: no total, yet it resumes in the chunk it stopped in.
    [{ chunks_done: 2, chunks_total: 0, llm_connection: null }, "전사일시정지됨", "구간 3에서 멈춤 · 이어서 처리하면 이 구간부터 다시 합니다"],
  ] as const;
  for (const [patch, step, memo] of cases) {
    mock({ meta: meta({ status: "paused", title: "", held: true, ...patch }), summary: "" });
    render(<Meeting id={id} {...props} />);
    expect(await screen.findByText(memo)).toBeTruthy();
    expect(screen.getByRole("listitem", { current: "step" }).textContent).toBe(step);
    expect(screen.getByRole("button", { name: "이어서 처리" })).toBeTruthy();
    cleanup();
  }

  mock({ meta: meta({ status: "paused", title: "", llm_connection: null }), summary: "" });
  render(<Meeting id={id} {...props} recording progress={progress({ status: "paused", position: 1 })} />);
  expect(await screen.findByText("대기 중 · 1번째 · 녹음이 끝나면 처리합니다")).toBeTruthy();
  expect(screen.getByRole("button", { name: "일시정지" })).toBeTruthy();
});

// Plan 5.3-11: no button where pausing means nothing: a route wait, a failure, review.
test("the pause button is hidden while it waits for a route, failed or in review", async () => {
  const cases = [
    meta({ status: "transcribed", title: "", wait: "summary_model", llm_connection: null }),
    meta({ status: "transcribe_failed", title: "", error: "전사 중 오류가 났습니다" }),
  ];
  for (const m of cases) {
    mock({ meta: m, summary: "" });
    render(<Meeting id={id} {...props} />);
    expect(await screen.findByRole("list", { name: "처리 단계" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /일시정지|이어서 처리|멈추는 중/ })).toBeNull();
    cleanup();
  }
  mock({ meta: meta(), summary: "## 결정사항\n- 목요일에 배포한다" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("목요일에 배포한다")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /일시정지|이어서 처리|멈추는 중/ })).toBeNull();
});

// Plan 5.6-24 (design-system 8.7): a meeting's languages change from ⋯ until it is sent; the
// recording screen changes a recording's, and a deleting or newer meeting is not this app's to change.
test("전사 언어 바꾸기… is in ⋯ until the meeting is sent, never while recording, deleting or unknown", async () => {
  const shown: Status[] = ["recorded", "transcribing", "transcribed", "paused", "review", "archived", "transcribe_failed"];
  const hidden: Status[] = ["sent", "recording", "deleting", "unknown"];
  const cases = [
    ...shown.map((status) => [meta({ status, title: "" }), true] as const),
    ...hidden.map((status) => [meta({ status, title: "" }), false] as const),
    [meta({ schema_version: 99 }), false] as const,
  ];
  for (const [m, offered] of cases) {
    mock({ meta: m, summary: "" });
    render(<Meeting id={id} {...props} />);
    await screen.findByRole("button", { name: "더보기" });
    const items = (await menu()).getAllByRole("menuitem").map((i) => i.textContent);
    expect([m.status, items.includes("전사 언어 바꾸기…")]).toEqual([m.status, offered]);
    cleanup();
  }
});

const languageDialog = async () => {
  await pick("전사 언어 바꾸기…");
  return within(await screen.findByRole("alertdialog", { name: "전사 언어 바꾸기" }));
};

// Plan 5.6-24: before any chunk is done the list just changes; nothing to warn of.
test("before transcription, 바꾸기 sets the languages once the list changed, and focus goes back to ⋯", async () => {
  const calls = mock({ meta: meta({ status: "recorded", title: "", llm_connection: null }), summary: "" }, (cmd, args) =>
    cmd === "set_meeting_language" ? meta({ status: "recorded", title: "", languages: args.languages as string[] }) : undefined,
  );
  render(<Meeting id={id} {...props} />);
  await screen.findByRole("button", { name: "더보기" });

  const dialog = await languageDialog();
  expect(dialog.queryByText(/지워집니다|바꿀 수 없습니다/)).toBeNull();
  const confirm = dialog.getByRole("button", { name: "바꾸기" }) as HTMLButtonElement;
  await dialog.findByRole("checkbox", { name: "베트남어" });
  expect(disabled(confirm)).toBe(true);
  fireEvent.click(confirm);
  expect(calls.map((c) => c.cmd)).not.toContain("set_meeting_language");
  fireEvent.click(dialog.getByRole("checkbox", { name: "베트남어" }));
  expect(disabled(confirm)).toBe(false);
  fireEvent.click(dialog.getByRole("checkbox", { name: "베트남어" }));
  expect(disabled(confirm)).toBe(true);
  fireEvent.click(dialog.getByRole("checkbox", { name: "베트남어" }));
  fireEvent.click(confirm);

  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(calls.filter((c) => c.cmd === "set_meeting_language").map((c) => c.args)).toEqual([
    { id, languages: ["ko", "vi"] },
  ]);
  expect(calls.map((c) => c.cmd)).not.toContain("retranscribe");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "더보기" }));
  // The meta line says them now.
  expect(await screen.findByText("한국어·베트남어로 전사")).toBeTruthy();

  // 취소 too.
  const again = await languageDialog();
  fireEvent.click(again.getByRole("button", { name: "취소" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "더보기" }));
});

// Plan 5.6-24 (E3): once transcribed, changing them deletes the results and transcribes again.
test("after transcription, 지우고 다시 전사 warns what goes and transcribes again", async () => {
  const calls = mock({ meta: meta(), summary: "## 결정사항\n- 목요일에 배포한다" }, (cmd, args) =>
    cmd === "retranscribe" ? meta({ status: "recorded", title: "", languages: args.languages as string[] }) : undefined,
  );
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("목요일에 배포한다")).toBeTruthy();

  const dialog = await languageDialog();
  expect(dialog.getByText("전사본과 요약, 검토에서 고친 내용이 지워집니다")).toBeTruthy();
  const confirm = dialog.getByRole("button", { name: "지우고 다시 전사" }) as HTMLButtonElement;
  expect(disabled(confirm)).toBe(true);
  fireEvent.click(await dialog.findByRole("checkbox", { name: "영어" }));
  fireEvent.click(confirm);

  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(calls.filter((c) => c.cmd === "retranscribe").map((c) => c.args)).toEqual([{ id, languages: ["ko", "en"] }]);
  expect(calls.map((c) => c.cmd)).not.toContain("set_meeting_language");
  // Its summary is gone from the screen.
  expect(screen.queryByText("목요일에 배포한다")).toBeNull();
});

// Plan 5.7-6: a chunk finished while the dialog was open: Rust refuses, and the dialog becomes E3.
test("a change refused for a chunk done meanwhile redraws as E3, without an error; another error shows inside", async () => {
  let refusal = "이미 전사한 구간이 있어 다시 전사해야 바꿀 수 있습니다";
  const calls = mock({ meta: meta({ status: "paused", title: "", llm_connection: null }), summary: "" }, (cmd) => {
    if (cmd === "set_meeting_language") throw refusal;
    if (cmd === "retranscribe") throw "보내는 중입니다";
  });
  render(<Meeting id={id} {...props} />);
  await screen.findByRole("button", { name: "더보기" });

  let dialog = await languageDialog();
  fireEvent.click(await dialog.findByRole("checkbox", { name: "베트남어" }));
  fireEvent.click(dialog.getByRole("button", { name: "바꾸기" }));
  expect(await dialog.findByText("전사본과 요약, 검토에서 고친 내용이 지워집니다")).toBeTruthy();
  expect(screen.queryByText(refusal)).toBeNull();
  // The pick stays.
  expect((dialog.getByRole("checkbox", { name: "베트남어" }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(dialog.getByRole("button", { name: "지우고 다시 전사" }));
  expect(await dialog.findByText("보내는 중입니다")).toBeTruthy();
  expect(calls.filter((c) => c.cmd === "retranscribe").map((c) => c.args)).toEqual([{ id, languages: ["ko", "vi"] }]);
  fireEvent.click(dialog.getByRole("button", { name: "취소" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

  // Any other refusal is one line inside, the mode unchanged.
  refusal = "보내는 중입니다";
  dialog = await languageDialog();
  fireEvent.click(await dialog.findByRole("checkbox", { name: "영어" }));
  fireEvent.click(dialog.getByRole("button", { name: "바꾸기" }));
  expect(await dialog.findByText("보내는 중입니다")).toBeTruthy();
  expect(dialog.getByRole("button", { name: "바꾸기" })).toBeTruthy();
  expect(dialog.queryByText(/지워집니다/)).toBeNull();
});

// Plan 5.6-24: while it runs the item still opens, to say why and offer 일시정지; paused, it asks.
test("while it runs, the dialog only offers 일시정지, and redraws with the checks once paused", async () => {
  const running = progress({ status: "transcribing", chunk: 2, chunks: 7, percent: 30 });
  const detail = {
    meta: meta({ status: "transcribing", title: "", chunks_done: 2, chunks_total: 7, llm_connection: null }),
    summary: "",
  };
  const calls = mock(detail, (cmd) => {
    if (cmd === "pause_meeting") {
      detail.meta = { ...detail.meta, held: true };
      return "pausing";
    }
  });
  const { rerender } = render(<Meeting id={id} {...props} progress={running} />);
  const more = await screen.findByRole("button", { name: "더보기" });
  more.focus();
  const items = await menu();
  const item = items.getByRole("menuitem", { name: "전사 언어 바꾸기…" });
  expect(disabled(item)).toBe(false);
  fireEvent.keyDown(item, { key: "ArrowDown" });
  fireEvent.keyDown(items.getByRole("menuitem", { name: "삭제" }), { key: "ArrowUp" });
  await waitFor(() => expect(document.activeElement).toBe(item));
  fireEvent.keyDown(item, { key: "Enter" });

  const dialog = within(await screen.findByRole("alertdialog", { name: "전사 언어 바꾸기" }));
  expect(dialog.getByText("처리 중에는 바꿀 수 없습니다")).toBeTruthy();
  expect(dialog.queryByRole("checkbox")).toBeNull();
  expect(dialog.queryByRole("button", { name: /바꾸기|다시 전사/ })).toBeNull();
  const pause = dialog.getByRole("button", { name: "일시정지" });
  pause.focus();
  fireEvent.click(pause);
  await waitFor(() => expect(calls.filter((c) => c.cmd === "pause_meeting")).toEqual([{ cmd: "pause_meeting", args: { id } }]));
  expect(disabled(await dialog.findByRole("button", { name: "멈추는 중…" }))).toBe(true);

  // The run ends `paused`: a chunk is done, so it is E3.
  detail.meta = { ...detail.meta, status: "paused" };
  rerender(<Meeting id={id} {...props} progress={progress({ status: "paused" })} />);
  expect(await dialog.findByText("전사본과 요약, 검토에서 고친 내용이 지워집니다")).toBeTruthy();
  expect(dialog.getAllByRole("checkbox")).toHaveLength(4);
  // The same button, unpressable until the list changes, keeps the focus: a browser drops it from
  // a `disabled` one (jsdom does not).
  const confirm = dialog.getByRole("button", { name: "지우고 다시 전사" });
  expect(disabled(confirm)).toBe(true);
  expect(confirm.hasAttribute("disabled")).toBe(false);
  expect(document.activeElement).toBe(confirm);
});

// Plan 5.6-27: the meta line names the languages unless it is Korean alone.
test("the meta line says the languages it is transcribed in, unless only Korean", async () => {
  const cases = [
    [["ko", "vi"], "한국어·베트남어로 전사"],
    [["en"], "영어로 전사"],
  ] as const;
  for (const [languages, line] of cases) {
    mock({ meta: meta({ languages: [...languages] }), summary: "요약" });
    render(<Meeting id={id} {...props} />);
    expect(await screen.findByText(line)).toBeTruthy();
    cleanup();
  }
  const calls = mock({ meta: meta(), summary: "요약" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("녹음 홍길동")).toBeTruthy();
  await waitFor(() => expect(calls.map((c) => c.cmd)).toContain("transcribe_languages"));
  await act(async () => {});
  expect(screen.queryByText(/로 전사$/)).toBeNull();
});

// Plan 5.6-18: dates are read in Korean only, so another language leaves them unchecked.
test("a meeting in another language says its dates were not checked, with no 미정 line too", async () => {
  mock({ meta: meta({ languages: ["ko", "vi"] }), summary: "## 결정사항\n- 목요일에 배포한다" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("목요일에 배포한다")).toBeTruthy();
  expect(screen.getByText("이 회의의 날짜는 확인하지 않았습니다")).toBeTruthy();
  cleanup();

  mock({ meta: meta(), summary: "## 결정사항\n- 목요일에 배포한다" });
  render(<Meeting id={id} {...props} />);
  expect(await screen.findByText("목요일에 배포한다")).toBeTruthy();
  expect(screen.queryByText("이 회의의 날짜는 확인하지 않았습니다")).toBeNull();
});

// Plan 5.6-28 (d11a): a mixed transcript names its language on the first line and wherever it
// changes; every line carries `lang` for screen readers.
test("a mixed transcript names the language where it changes; a single one names none", async () => {
  const segments: Segment[] = [
    { start_ms: 3_000, end_ms: 4_000, text: "안녕하세요", lang: "ko" },
    { start_ms: 5_000, end_ms: 6_000, text: "시작합시다", lang: "ko" },
    { start_ms: 7_000, end_ms: 8_000, text: "Xin chào", lang: "vi" },
    { start_ms: 9_000, end_ms: 10_000, text: "Cảm ơn", lang: "vi" },
    { start_ms: 11_000, end_ms: 12_000, text: "네", lang: "ko" },
  ];
  const open = async (languages: string[], segs: Segment[]) => {
    mock({ meta: meta({ languages }), summary: "요약" }, (cmd) => (cmd === "get_transcript" ? segs : undefined));
    render(<Meeting id={id} {...props} />);
    await screen.findByText("녹음 홍길동");
    fireEvent.click(await screen.findByRole("button", { name: "전사본" }));
    const panel = within(await screen.findByRole("complementary", { name: "전사본" }));
    await panel.findByText(segs[0].text);
    return panel;
  };

  let panel = await open(["ko", "vi"], segments);
  await waitFor(() =>
    expect(panel.getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "00:03한국어안녕하세요",
      "00:05시작합시다",
      "00:07베트남어Xin chào",
      "00:09Cảm ơn",
      "00:11한국어네",
    ]),
  );
  expect(segments.map((s) => panel.getByText(s.text).getAttribute("lang"))).toEqual(["ko", "ko", "vi", "vi", "ko"]);
  cleanup();

  const single = [
    { start_ms: 3_000, end_ms: 4_000, text: "Hello" },
    { start_ms: 5_000, end_ms: 6_000, text: "Let's start" },
  ];
  panel = await open(["en"], single);
  expect(panel.getAllByRole("listitem").map((li) => li.textContent)).toEqual(["00:03Hello", "00:05Let's start"]);
  expect(single.map((s) => panel.getByText(s.text).getAttribute("lang"))).toEqual(["en", "en"]);
});

// Plan 5.6-27: the first chunk says the languages, so a wrong pick shows early; a mixed chunk
// being told apart says so.
test("the first chunk's memo says its languages, and a mixed chunk says it is being told apart", async () => {
  const korean = meta({ status: "transcribing", title: "" });
  mock({ meta: korean, summary: "" });
  const first = progress({ status: "transcribing", chunk: 0, chunks: 5, percent: 3 });
  const { rerender } = render(<Meeting id={id} {...props} progress={first} />);
  expect(await screen.findByText("구간 1/5 · 한국어로 전사")).toBeTruthy();
  rerender(<Meeting id={id} {...props} progress={{ ...first, chunk: 1, percent: 21 }} />);
  expect(await screen.findByText("구간 2/5")).toBeTruthy();
  expect(screen.queryByText(/한국어로 전사/)).toBeNull();
  cleanup();

  mock({ meta: meta({ status: "transcribing", title: "", languages: ["ko", "vi"] }), summary: "" });
  render(<Meeting id={id} {...props} progress={{ ...first, detecting: true }} />);
  expect(await screen.findByText("구간 1/5 · 한국어·베트남어로 전사 · 언어를 가리는 중")).toBeTruthy();
});

// Plan 5.6-27: a mixed meeting in line without the detection model fetches it, and says so.
test("a mixed meeting waiting without the detection model says it is being fetched", async () => {
  let installed = false;
  let finish = () => {};
  const calls = mock({ meta: meta({ status: "recorded", title: "", llm_connection: null, languages: ["ko", "vi"] }), summary: "" }, (cmd, args) => {
    if (cmd === "model_status" && args.scope === "lid") {
      return {
        files: [{ file: "ggml-tiny.bin", size: 77_691_713, installed, received: 0, downloading: false }],
        downloading: false,
      };
    }
    if (cmd === "download_models") return new Promise<void>((resolve) => (finish = resolve));
  });
  render(<Meeting id={id} {...props} progress={progress({ status: "recorded", position: 1 })} />);

  expect(await screen.findByText(/언어 판별 모델을 받는 중/)).toBeTruthy();
  expect(calls.filter((c) => c.cmd === "download_models").map((c) => c.args)).toEqual([{ scope: "lid" }]);
  installed = true;
  await act(async () => finish());
  await waitFor(() => expect(screen.queryByText(/언어 판별 모델을 받는 중/)).toBeNull());
});
