import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { LoadedSettings, MeetingDetail, Meta, Segment, Settings } from "../lib/ipc";
import Review from "./Review";

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

const settings = (patch: Partial<Settings> = {}): LoadedSettings => ({
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
    llm_connections: [],
    default_llm: null,
    summarize_local_by_default: false,
    webhooks: [{ name: "#회의록" }],
    last_channel: null,
    glossary: [],
    ...patch,
  },
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

const props = { onHome: () => {}, onNotice: () => {}, onSetup: () => {} };
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
  const send = (await screen.findByRole("button", { name: "#회의록으로 전송" })) as HTMLButtonElement;
  expect(send.disabled).toBe(true);
  expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual([
    "홈으로",
    "#회의록으로 전송",
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
  const first = render(<Review id={id} {...props} onNotice={(m) => notices.push(m)} />);
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
  render(<Review id={id} {...props} onNotice={(m) => notices.push(m)} />);
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
  render(<Review id={id} {...props} onHome={() => home++} />);

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
  const { unmount } = render(
    <Review id={id} {...props} onHome={() => home++} onNotice={(n) => notices.push(n)} />,
  );

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
    [{}, "전사본을 회사 클라우드로 보내 요약했습니다"],
    [{ llm_connection: "Ollama", summarize_local: true }, "이 PC의 Ollama로 요약했습니다"],
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

// Slack (SND-01..06, REV-02).

const offset = -new Date("2026-10-13T05:00:03Z").getTimezoneOffset();
const sendButton = (name: string) => screen.findByRole("button", { name }) as Promise<HTMLButtonElement>;

// SND-04, NFR-04: with one webhook the button is ready, no choice to make.
test("the send button names the default channel: the only one, the last one sent to, else the first", async () => {
  // The particle follows the channel's last syllable: 록 → 으로, 발 (ㄹ) → 로.
  const cases: [Partial<Settings>, string, string, boolean][] = [
    [{}, "#회의록", "#회의록으로 전송", false],
    [{ webhooks: [{ name: "#회의록" }, { name: "#개발" }], last_channel: "#개발" }, "#개발", "#개발로 전송", true],
    [
      { webhooks: [{ name: "#회의록" }, { name: "#개발" }], last_channel: "#지운 채널" },
      "#회의록",
      "#회의록으로 전송",
      true,
    ],
  ];
  for (const [patch, channel, button, choice] of cases) {
    mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_settings" ? settings(patch) : undefined));
    render(<Review id={id} {...props} />);
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
  render(<Review id={id} {...props} />);

  fireEvent.keyDown(await screen.findByRole("combobox", { name: "보낼 채널" }), { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: "#개발" }));
  fireEvent.click(await sendButton("#개발로 전송"));

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
  render(<Review id={id} {...props} />);

  expect(await screen.findByText("12,345 / 12,000자 (Slack 전송 기준)")).toBeTruthy();
  expect(screen.getByText("Slack 한도 12,000자를 넘었습니다. 345자를 줄여 주세요")).toBeTruthy();
  expect((await sendButton("#회의록으로 전송")).disabled).toBe(true);
  expect(calls.find((c) => c.cmd === "slack_length")?.args).toEqual({
    id,
    title: "주간 회의",
    body: "요약",
    offsetMinutes: offset,
  });

  length = 12_000;
  fireEvent.change(screen.getByRole("textbox", { name: "요약" }), { target: { value: "줄인 요약" } });
  expect(await screen.findByText("12,000 / 12,000자 (Slack 전송 기준)")).toBeTruthy();
  expect(screen.queryByText(/Slack 한도/)).toBeNull();
  expect((await sendButton("#회의록으로 전송")).disabled).toBe(false);
  const counted = calls.filter((c) => c.cmd === "slack_length");
  expect(counted[counted.length - 1].args.body).toBe("줄인 요약");
});

test("no webhook: the send is off and the Slack settings are one click away", async () => {
  const tabs: string[] = [];
  mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "get_settings" ? settings({ webhooks: [] }) : undefined));
  render(<Review id={id} {...props} onSetup={(tab) => tabs.push(tab)} />);

  fireEvent.click(await screen.findByRole("button", { name: "웹훅 추가하기" }));
  expect(tabs).toEqual(["slack"]);
  expect((screen.getByRole("button", { name: "Slack으로 전송" }) as HTMLButtonElement).disabled).toBe(true);
});

test("a meeting out of review is never sent from here", async () => {
  mock({ meta: meta({ status: "archived" }), summary: "요약" });
  render(<Review id={id} {...props} />);
  expect((await sendButton("#회의록으로 전송")).disabled).toBe(true);
});

test("the pending edit is saved before the send, and a failed save stops it", async () => {
  const { held, handler } = holdSaves();
  const calls = mock({ meta: meta(), summary: "요약" }, (cmd, args) =>
    cmd === "send_to_slack" ? meta({ status: "sent" }) : handler(cmd, args),
  );
  render(<Review id={id} {...props} />);

  fireEvent.change(await screen.findByRole("textbox", { name: "요약" }), { target: { value: "고친 요약" } });
  fireEvent.click(await sendButton("#회의록으로 전송"));
  await waitFor(() => expect(held).toHaveLength(1));
  expect(held[0].args.body).toBe("고친 요약");
  expect(calls.map((c) => c.cmd)).not.toContain("send_to_slack");
  // Nothing more is typed while it goes, and the meeting cannot be deleted under it.
  expect(screen.getByRole("textbox", { name: "요약" }).hasAttribute("readonly")).toBe(true);
  expect((screen.getByRole("button", { name: "삭제" }) as HTMLButtonElement).disabled).toBe(true);

  held[0].reject("디스크가 가득 찼습니다");
  expect(await screen.findByText("저장하지 못했습니다: 디스크가 가득 찼습니다")).toBeTruthy();
  expect(calls.map((c) => c.cmd)).not.toContain("send_to_slack");

  fireEvent.click(await sendButton("#회의록으로 전송"));
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
    return meta({ status: "sent", sent: { channel: "#회의록", at: "2026-10-15T05:03:00Z", edit_count: 0 } });
  });
  render(<Review id={id} {...props} />);

  fireEvent.click(await sendButton("#회의록으로 전송"));
  expect(await screen.findByText(cause)).toBeTruthy();
  expect(screen.getByRole("button", { name: "요약 복사" })).toBeTruthy();
  // Still in review, so still editable.
  expect(screen.getByRole("textbox", { name: "요약" }).hasAttribute("readonly")).toBe(false);

  fail = false;
  fireEvent.click(await sendButton("다시 보내기"));
  expect(await screen.findByText(/^#회의록으로 전송함/)).toBeTruthy();
  expect(screen.queryByText(cause)).toBeNull();
});

// Sent from an earlier visit, which went home: nothing may change under the send, and its end
// (`slack://finished`) is read again here.
test("a send still out from an earlier visit keeps the screen read-only until it ends", async () => {
  const detail = { meta: meta(), summary: "요약", sending: true };
  const calls = mock(detail);
  render(<Review id={id} {...props} />);

  expect((await sendButton("보내는 중…")).disabled).toBe(true);
  expect(textbox("제목").hasAttribute("readonly")).toBe(true);
  expect(textbox("요약").hasAttribute("readonly")).toBe(true);
  expect((screen.getByRole("button", { name: "전송 안 함" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "삭제" }) as HTMLButtonElement).disabled).toBe(true);

  Object.assign(detail, {
    meta: meta({ status: "sent", sent: { channel: "#회의록", at: "2026-10-15T05:03:00Z", edit_count: 0 } }),
    sending: false,
  });
  // Another meeting's send is not this one's.
  await act(() => emit("slack://finished", "다른 회의"));
  expect(calls.filter((c) => c.cmd === "get_meeting")).toHaveLength(1);
  await act(() => emit("slack://finished", id));
  expect(await screen.findByText(/^#회의록으로 전송함/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "보내는 중…" })).toBeNull();
});

test("a send that failed before is told again when the meeting opens", async () => {
  mock({ meta: meta({ error: "Slack 응답 시간 초과입니다" }), summary: "요약" });
  render(<Review id={id} {...props} />);
  expect(await screen.findByText("Slack 응답 시간 초과입니다")).toBeTruthy();
  expect((await sendButton("다시 보내기")).disabled).toBe(false);
});

// Stays here, read-only, so the recorder sees where it went and how much they changed.
test("a sent meeting says where, when and how many edits, and is read-only", async () => {
  const at = "2026-10-15T05:03:00Z";
  const sent = meta({ status: "sent", sent: { channel: "#회의록", at, edit_count: 2 } });
  mock({ meta: meta(), summary: "요약" }, (cmd) => (cmd === "send_to_slack" ? sent : undefined));
  render(<Review id={id} {...props} />);

  fireEvent.click(await sendButton("#회의록으로 전송"));
  const line = `#회의록으로 전송함 · ${new Date(at).toLocaleString("ko-KR")} · 수정 2곳`;
  expect(await screen.findByText(line)).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "요약" }).hasAttribute("readonly")).toBe(true);
  expect(screen.queryByRole("button", { name: /전송$/ })).toBeNull();
  expect(screen.queryByRole("button", { name: "전송 안 함" })).toBeNull();
});
