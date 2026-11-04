import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { Lang, Level, LlmConnection, Meta, ModelFileState, RecordingState, SummaryRoute, Warning } from "../lib/ipc";
import Recording from "./Recording";

// No clearMocks(): the screen unlistens asynchronously after unmount (see App.test.tsx).
afterEach(cleanup);

const state: RecordingState = {
  id: "2026-10-14T0500-a1b2",
  warnings: [{ kind: "disk", message: "남은 공간으로 최대 약 90분 녹음할 수 있습니다" }],
};

test("shows elapsed time, spectrums and levels from level events", async () => {
  mockIPC((cmd) => (cmd === "recording_state" ? state : undefined), { shouldMockEvents: true });
  render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);
  expect(await screen.findByText("00:00")).toBeTruthy();

  const micBands = Array.from({ length: 24 }, (_, i) => i * 4);
  const level: Level = {
    elapsed_ms: 3_723_000,
    mic: 0.1,
    system: 0,
    mic_bands: micBands,
    system_bands: Array(24).fill(0),
    waiting_permission: true,
  };
  await act(() => emit("recording://level", level));

  // mm:ss under an hour, h:mm:ss past it.
  expect(screen.getByText("1:02:03")).toBeTruthy();
  // The bars are decoration next to each meter; the meter is what screen readers get. Levels go
  // straight to each bar's transform, never through a height: 48 bars, the 24 bands mirrored out
  // from the middle (band 0 at the two center bars, band 23 at both ends).
  const bars = (name: string) =>
    Array.from(screen.getByRole("meter", { name }).previousElementSibling!.children) as HTMLElement[];
  const mirrored = [...[...micBands].reverse(), ...micBands];
  expect(bars("내 마이크").map((b) => b.style.transform)).toEqual(mirrored.map((b) => `scaleY(${b / 100})`));
  expect(bars("시스템 오디오")).toHaveLength(48);
  expect(bars("시스템 오디오").every((b) => b.style.transform === "scaleY(0)")).toBe(true);
  expect(bars("내 마이크")[0].parentElement!.getAttribute("aria-hidden")).toBe("true");
  // 0.1 RMS = -20 dBFS → 40 of 60 dB.
  expect(screen.getByRole("meter", { name: "내 마이크" }).getAttribute("aria-valuenow")).toBe("67");
  expect(screen.getByRole("meter", { name: "시스템 오디오" }).getAttribute("aria-valuenow")).toBe("0");
  expect(screen.getByText(/권한 허용을 기다리는 중/)).toBeTruthy();
});

test("a dismissed warning stays hidden until it is sent again", async () => {
  mockIPC((cmd) => (cmd === "recording_state" ? state : undefined), { shouldMockEvents: true });
  render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);

  // Standing warnings from before the screen opened are shown too.
  expect(await screen.findByText(state.warnings[0].message)).toBeTruthy();
  const mic: Warning = { kind: "mic_permission", message: "마이크 소리가 들어오지 않습니다" };
  await act(() => emit("recording://warning", mic));
  expect(screen.getAllByRole("button", { name: "닫기" })).toHaveLength(2);

  fireEvent.click(screen.getAllByRole("button", { name: "닫기" })[1]);
  expect(screen.queryByText(mic.message)).toBeNull();
  expect(screen.getByText(state.warnings[0].message)).toBeTruthy();

  await act(() => emit("recording://warning", mic));
  expect(screen.getByText(mic.message)).toBeTruthy();
});

const GGUF = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
// `settings::builtin`, as `summary_route` names it.
const thisPc: LlmConnection = {
  name: "builtin",
  base_url: "",
  model: GGUF,
  context_tokens: 16_384,
  is_local: true,
  timeout_secs: 1_800,
};

// SUM-07: shown while recording, before anything is sent. With this PC as the default and no route
// yet, the summary model in one of three states (plan 5.2, DX6).
test("tells where this meeting's summary will go, and what the summary model still needs", async () => {
  const asked: unknown[] = [];
  let route: SummaryRoute = { connection: null, local: false, local_available: false, locked: false, toggle_matters: false };
  let summary: ModelFileState = { file: GGUF, size: 2_497_281_120, installed: false, received: 0, downloading: false };
  mockIPC(
    (cmd, args) => {
      if (cmd === "recording_state") return state;
      if (cmd === "summary_route") {
        asked.push(args);
        return route;
      }
      if (cmd === "model_status") return { files: [summary], downloading: summary.downloading };
    },
    { shouldMockEvents: true },
  );
  let opened = 0;
  const recording = () => render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} onSetup={() => opened++} />);

  const missing = recording();
  expect(await screen.findByText("설정 → 요약(LLM)에서 요약 모델을 받아 주세요")).toBeTruthy();
  expect(asked).toEqual([{ id: state.id }]);
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM) 설정" }));
  expect(opened).toBe(1);
  missing.unmount();

  summary = { ...summary, received: 1_248_640_560, downloading: true };
  const downloading = recording();
  expect(await screen.findByText("요약 모델 받는 중 50% · 다 받으면 자동으로 요약합니다")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "요약(LLM) 설정" })).toBeNull();
  const progress = { file: GGUF, received: 1_872_960_840, total: 2_497_281_120, all_received: 0, all_total: 0 };
  await act(() => emit("model://download", progress));
  expect(await screen.findByText("요약 모델 받는 중 75% · 다 받으면 자동으로 요약합니다")).toBeTruthy();
  downloading.unmount();

  route = { ...route, connection: thisPc, local_available: true };
  recording();
  expect(await screen.findByText("이 PC에서 요약합니다. 전사본은 밖으로 나가지 않습니다")).toBeTruthy();
  // This PC is the default, its model in: "로컬로 요약" would change nothing.
  expect(screen.queryByRole("switch", { name: "로컬로 요약" })).toBeNull();
});

// Design-system 6.3: leaving for Home keeps recording; only the stop button (or ⌘.) stops it.
test("← 홈 leaves the recording running; 녹음 종료 asks to stop", async () => {
  const calls: string[] = [];
  mockIPC(
    (cmd) => {
      calls.push(cmd);
      if (cmd === "recording_state") return state;
    },
    { shouldMockEvents: true },
  );
  let home = 0;
  let stops = 0;
  render(
    <Recording
      startedAt="2026-10-14T05:00:00Z"
      stopping={false}
      onHome={() => home++}
      onStop={() => stops++}
    />,
  );

  expect(await screen.findByText("녹음은 계속돼요")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1 }).textContent).toMatch(/^녹음 중 · \d{2}:\d{2} 시작$/);
  fireEvent.click(screen.getByRole("button", { name: "홈" }));
  expect(home).toBe(1);
  expect(stops).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "녹음 종료" }));
  expect(stops).toBe(1);
  expect(calls).not.toContain("stop_recording");
});

// A start (⌘R) while this screen said no recording was running: the list's new start time asks again.
test("asks for the recording again when a new one starts", async () => {
  let current: RecordingState | null = null;
  mockIPC((cmd) => (cmd === "recording_state" ? current : undefined), { shouldMockEvents: true });
  const { rerender } = render(<Recording stopping={false} onHome={() => {}} onStop={() => {}} />);
  expect(await screen.findByRole("heading", { level: 1, name: "진행 중인 녹음이 없습니다" })).toBeTruthy();

  current = { id: "새 회의", warnings: [] };
  rerender(<Recording startedAt="2026-10-14T05:00:00Z" stopping={false} onHome={() => {}} onStop={() => {}} />);
  expect(await screen.findByRole("heading", { level: 1, name: /^녹음 중 · \d{2}:\d{2} 시작$/ })).toBeTruthy();
  expect(screen.getByRole("button", { name: "녹음 종료" })).toBeTruthy();
});

// Plan 5.6-20..22 (design-system 8.7): the recording's languages, named on a trigger by the bottom
// group, picked in a popover of checks drawn from Rust's table.
const LANGS: Lang[] = [
  { code: "ko", name: "한국어" },
  { code: "en", name: "영어" },
  { code: "vi", name: "베트남어" },
  { code: "id", name: "인도네시아어" },
];
const LID = {
  files: [{ file: "ggml-tiny.bin", size: 77_691_713, installed: true, received: 0, downloading: false }],
  downloading: false,
};
type Call = { cmd: string; args: Record<string, unknown> };
/** The recording `state`, the language table, an installed detection model; `handler` first. */
const mockLanguages = (handler: (cmd: string, args: Call["args"]) => unknown = () => undefined) => {
  const calls: Call[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: args as Call["args"] });
      const fallback: Record<string, unknown> = { recording_state: state, transcribe_languages: LANGS, model_status: LID };
      return handler(cmd, args as Call["args"]) ?? fallback[cmd];
    },
    { shouldMockEvents: true },
  );
  return calls;
};
const renderPicker = (languages: string[], onChanged = () => {}) =>
  render(<Recording languages={languages} stopping={false} onHome={() => {}} onStop={() => {}} onChanged={onChanged} />);
/** The trigger once the table is read, opened; returns the popover. */
const openPicker = async (name: string | RegExp) => {
  const trigger = (await screen.findByRole("button", { name })) as HTMLButtonElement;
  await waitFor(() => expect(trigger.disabled).toBe(false));
  fireEvent.click(trigger);
  return within(await screen.findByRole("dialog", { name: "전사 언어" }));
};
const checks = (popover: ReturnType<typeof within>) =>
  (popover.getAllByRole("checkbox") as HTMLInputElement[]).map((c) => c.parentElement!.textContent);

test("the checks are the languages Rust lists, and nothing else", async () => {
  mockLanguages((cmd) => (cmd === "transcribe_languages" ? LANGS.slice(0, 2) : undefined));
  renderPicker(["ko"]);
  const popover = await openPicker("전사 언어: 한국어");
  expect(checks(popover)).toEqual(["한국어하나는 골라야 합니다", "영어"]);
  expect(popover.getByRole("group").textContent).toMatch(/^회의에서 쓰는 언어를 모두 고르세요 · 요약은 항상 한국어로 씁니다/);
});

test("one language is always left: its check is off, and why; two or more say it takes longer", async () => {
  mockLanguages();
  renderPicker(["ko"]);
  const popover = await openPicker("전사 언어: 한국어");
  const box = (name: string) => popover.getByRole("checkbox", { name: new RegExp(`^${name}`) }) as HTMLInputElement;
  expect(box("한국어").disabled).toBe(true);
  expect(box("한국어").parentElement!.textContent).toBe("한국어하나는 골라야 합니다");
  expect(popover.queryByText("여러 언어를 고르면 전사가 더 걸립니다")).toBeNull();

  fireEvent.click(box("베트남어"));
  expect(box("한국어").disabled).toBe(false);
  expect(box("한국어").checked).toBe(true);
  expect(popover.queryByText("하나는 골라야 합니다")).toBeNull();
  expect(popover.getByText("여러 언어를 고르면 전사가 더 걸립니다")).toBeTruthy();

  fireEvent.click(box("한국어"));
  expect(box("베트남어").disabled).toBe(true);
  expect(box("베트남어").parentElement!.textContent).toBe("베트남어하나는 골라야 합니다");
  expect(popover.queryByText("여러 언어를 고르면 전사가 더 걸립니다")).toBeNull();
});

test("Vietnamese or Indonesian, alone or mixed, points the summary to an LLM connection", async () => {
  mockLanguages();
  renderPicker(["ko"]);
  const popover = await openPicker("전사 언어: 한국어");
  const box = (name: string) => popover.getByRole("checkbox", { name: new RegExp(`^${name}`) }) as HTMLInputElement;
  const hint = "베트남어·인도네시아어 회의는 이 PC 요약이 자주 틀립니다. LLM 연결로도 요약해 보세요";
  fireEvent.click(box("영어"));
  expect(popover.queryByText(hint)).toBeNull();
  fireEvent.click(box("인도네시아어"));
  expect(popover.getByText(hint)).toBeTruthy();
  fireEvent.click(box("인도네시아어"));
  fireEvent.click(box("베트남어"));
  expect(popover.getByText(hint)).toBeTruthy();
});

test("the trigger waits for the table, shows one, two or the first and a count, and is named by all", async () => {
  let table: (langs: Lang[]) => void = () => {};
  mockLanguages((cmd) => (cmd === "transcribe_languages" ? new Promise((resolve) => (table = resolve)) : undefined));
  const { unmount } = renderPicker(["ko"]);
  const waiting = (await screen.findByRole("button", { name: "전사 언어:" })) as HTMLButtonElement;
  expect(waiting.disabled).toBe(true);
  await act(async () => table(LANGS));
  const one = screen.getByRole("button", { name: "전사 언어: 한국어" }) as HTMLButtonElement;
  expect(one.disabled).toBe(false);
  expect(one.textContent).toBe("한국어");
  unmount();

  mockLanguages();
  const cases = [
    [["ko", "vi"], "한국어·베트남어", "전사 언어: 한국어, 베트남어"],
    // In the table's order, whatever order it came in.
    [["vi", "id", "ko"], "한국어 외 2개", "전사 언어: 한국어, 베트남어, 인도네시아어"],
  ] as const;
  for (const [languages, text, name] of cases) {
    renderPicker([...languages]);
    expect((await screen.findByRole("button", { name })).textContent).toBe(text);
    cleanup();
  }
});

test("적용 sets the recording's languages in table order; 취소 and an unchanged list change nothing", async () => {
  const calls = mockLanguages((cmd, args) =>
    cmd === "set_meeting_language" ? ({ id: state.id, languages: args.languages } as Partial<Meta>) : undefined,
  );
  let changed = 0;
  renderPicker(["ko"], () => changed++);
  const sets = () => calls.filter((c) => c.cmd === "set_meeting_language").map((c) => c.args);

  let popover = await openPicker("전사 언어: 한국어");
  fireEvent.click(popover.getByRole("checkbox", { name: "베트남어" }));
  fireEvent.click(popover.getByRole("button", { name: "취소" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  // Opened again, the draft is gone.
  popover = await openPicker("전사 언어: 한국어");
  expect((popover.getByRole("checkbox", { name: "베트남어" }) as HTMLInputElement).checked).toBe(false);
  fireEvent.click(popover.getByRole("button", { name: "적용" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(sets()).toEqual([]);

  popover = await openPicker("전사 언어: 한국어");
  fireEvent.click(popover.getByRole("checkbox", { name: "인도네시아어" }));
  fireEvent.click(popover.getByRole("checkbox", { name: "영어" }));
  fireEvent.click(popover.getByRole("button", { name: "적용" }));
  const trigger = await screen.findByRole("button", { name: "전사 언어: 한국어, 영어, 인도네시아어" });
  expect(sets()).toEqual([{ id: state.id, languages: ["ko", "en", "id"] }]);
  expect(changed).toBe(1);
  expect(screen.queryByRole("dialog")).toBeNull();
  await waitFor(() => expect(document.activeElement).toBe(trigger));
});

test("a refused 적용 says why inside and becomes 다시 시도; Esc closes it back to the trigger", async () => {
  const calls = mockLanguages((cmd) => {
    if (cmd === "set_meeting_language") throw "녹음이 이미 끝났습니다";
  });
  renderPicker(["ko"]);
  const popover = await openPicker("전사 언어: 한국어");
  fireEvent.click(popover.getByRole("checkbox", { name: "베트남어" }));
  fireEvent.click(popover.getByRole("button", { name: "적용" }));

  expect(await popover.findByText("녹음이 이미 끝났습니다")).toBeTruthy();
  fireEvent.click(popover.getByRole("button", { name: "다시 시도" }));
  await waitFor(() => expect(calls.filter((c) => c.cmd === "set_meeting_language")).toHaveLength(2));
  expect(await popover.findByRole("button", { name: "다시 시도" })).toBeTruthy();
  // Nothing changed meanwhile.
  expect(screen.getByRole("button", { name: "전사 언어: 한국어" })).toBeTruthy();

  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "전사 언어: 한국어" }));
});
