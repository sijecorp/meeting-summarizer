import { useState, type ComponentProps } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test, vi } from "vitest";
import {
  cancelModelDownload,
  modelDownload,
  modelDownloadFailure,
  startModelDownload,
  type AudioDevices,
  type LoadedSettings,
  modelFileFilter,
  type ModelFileState,
  type ModelState,
} from "../lib/ipc";
import SettingsScreen from "./Settings";

// Like App: the save bar goes into a slot of the bottom stack.
function Settings(props: ComponentProps<typeof SettingsScreen>) {
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  return (
    <>
      <SettingsScreen {...props} barSlot={slot} />
      <div ref={setSlot} />
    </>
  );
}
// Connections and webhooks are rows that open to edit them.
const expand = async (name: string) =>
  fireEvent.click(await screen.findByRole("button", { name: new RegExp(`^${name}`) }));

// The model download is app-wide (ipc.ts): each test's is settled and awaited here, and a failure
// it left (kept until the next download) is cleared by one that works, so nothing leaks into the
// next test. No clearMocks(): the screen unlistens asynchronously after unmount (see App.test.tsx).
let release = () => {};
afterEach(async () => {
  vi.useRealTimers();
  cleanup();
  release();
  await modelDownload();
  if (modelDownloadFailure()) {
    mockIPC(() => undefined);
    await startModelDownload();
  }
});

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
    webhooks: [],
    last_channel: null,
    glossary: [],
  },
};

// A removed connection's key may only leave the keychain once settings.json no longer lists it.
test("deletes a removed connection's secret only after a successful save", async () => {
  const calls: { cmd: string; args: unknown }[] = [];
  let saveFails = true;
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "save_settings" && saveFails) throw new Error("settings.json 저장 실패");
  });

  render(<Settings onHome={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "요약(LLM)" }));
  await expand("회사 클라우드");
  fireEvent.click(screen.getByRole("button", { name: "연결 삭제" }));
  const dialog = await screen.findByRole("alertdialog", { name: "회사 클라우드 연결을 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(screen.queryByText("회사 클라우드")).toBeNull());

  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText(/settings\.json 저장 실패/)).toBeTruthy();
  expect(calls.map((c) => c.cmd)).not.toContain("delete_secret");

  saveFails = false;
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await waitFor(() =>
    expect(calls.filter((c) => c.cmd === "delete_secret").map((c) => c.args)).toEqual([
      { kind: "llm", name: "회사 클라우드" },
    ]),
  );
});

type Call = { cmd: string; args: Record<string, unknown> };

// One 저장 covers every tab, so a tab switch must keep every kind of unsaved input.
test("keeps unsaved edits across tabs and saves them all, keys included, at once", async () => {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return false;
  });
  render(<Settings onHome={() => {}} />);
  const tab = (name: string) => screen.getByRole("button", { name });
  const recorderName = () =>
    screen.queryByRole("textbox", { name: "녹음자 이름" }) as HTMLInputElement;

  await screen.findByDisplayValue("홍길동");
  fireEvent.change(recorderName(), { target: { value: "김철수" } });

  fireEvent.click(tab("요약(LLM)"));
  expect(tab("요약(LLM)").getAttribute("aria-current")).toBe("page");
  expect(tab("일반").getAttribute("aria-current")).toBeNull();
  await expand("회사 클라우드");
  fireEvent.change(screen.getByLabelText(/^API 키/), { target: { value: "sk-아직-안-넣음" } });

  fireEvent.click(tab("Slack"));
  // Hidden panels stay mounted but out of reach.
  expect(recorderName()).toBeNull();
  fireEvent.change(screen.getByRole("textbox", { name: "새 웹훅 이름" }), {
    target: { value: "#회의록" },
  });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));

  fireEvent.click(tab("일반"));
  expect(recorderName().value).toBe("김철수");
  fireEvent.click(tab("요약(LLM)"));
  expect((screen.getByLabelText(/^API 키/) as HTMLInputElement).value).toBe("sk-아직-안-넣음");
  expect(calls.map((c) => c.cmd)).not.toContain("set_secret");

  // Pressed from the Slack tab: the key typed on the 요약(LLM) tab goes too.
  fireEvent.click(tab("Slack"));
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await waitFor(() =>
    expect(calls.find((c) => c.cmd === "set_secret")?.args).toEqual({
      kind: "llm",
      name: "회사 클라우드",
      value: "sk-아직-안-넣음",
    }),
  );
  expect(calls.find((c) => c.cmd === "save_settings")?.args.value).toMatchObject({
    recorder_name: "김철수",
    webhooks: [{ name: "#회의록" }],
  });
  fireEvent.click(tab("요약(LLM)"));
  expect(screen.getByLabelText("API 키 · 키체인에 저장됨")).toBeTruthy();
  expect((screen.getByLabelText(/^API 키/) as HTMLInputElement).value).toBe("");
});

// Architecture 9.2: a key stored before settings.json lists its entry could be orphaned.
test("stores a new entry's key only after a successful save", async () => {
  const calls: Call[] = [];
  let saveFails = true;
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "save_settings" && saveFails) throw new Error("settings.json 저장 실패");
  });
  render(<Settings tab="slack" onHome={() => {}} />);

  fireEvent.change(await screen.findByRole("textbox", { name: "새 웹훅 이름" }), {
    target: { value: "#회의록" },
  });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));
  const url = screen.getByLabelText(/^웹훅 URL/) as HTMLInputElement;
  expect(url.disabled).toBe(false);
  fireEvent.change(url, { target: { value: " https://hooks.slack.com/services/x " } });

  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText(/settings\.json 저장 실패/)).toBeTruthy();
  expect(calls.map((c) => c.cmd)).not.toContain("set_secret");
  expect(url.value).toBe(" https://hooks.slack.com/services/x ");

  saveFails = false;
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await waitFor(() => expect(screen.getByLabelText("웹훅 URL · 키체인에 저장됨")).toBeTruthy());
  const cmds = calls.map((c) => c.cmd);
  expect(cmds.lastIndexOf("save_settings")).toBeLessThan(cmds.indexOf("set_secret"));
  expect(calls.find((c) => c.cmd === "set_secret")?.args).toEqual({
    kind: "slack",
    name: "#회의록",
    value: "https://hooks.slack.com/services/x",
  });
  expect((screen.getByLabelText(/^웹훅 URL/) as HTMLInputElement).value).toBe("");
});

test("keeps a typed key and says so when the keychain write fails", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return false;
    if (cmd === "set_secret") throw new Error("키체인 저장 실패");
  });
  render(<Settings tab="llm" onHome={() => {}} />);

  await expand("회사 클라우드");
  fireEvent.change(screen.getByLabelText(/^API 키/), { target: { value: "sk-새 키" } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));

  expect(
    await screen.findByText(/설정은 저장했지만 키체인에 키를 저장하지 못했습니다: .*키체인 저장 실패/),
  ).toBeTruthy();
  expect(screen.queryByText("저장했습니다")).toBeNull();
  expect((screen.getByLabelText("API 키 · 입력 안 됨") as HTMLInputElement).value).toBe("sk-새 키");
});

// Its old key is deleted only on 저장, so a same-name entry added before that would inherit it.
test("refuses to re-add a removed saved name before saving", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
  });
  render(<Settings tab="llm" onHome={() => {}} />);

  await expand("회사 클라우드");
  fireEvent.click(screen.getByRole("button", { name: "연결 삭제" }));
  const dialog = await screen.findByRole("alertdialog", { name: "회사 클라우드 연결을 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: "연결 삭제" })).toBeNull());

  fireEvent.change(screen.getByRole("textbox", { name: "새 LLM 연결 이름" }), {
    target: { value: "회사 클라우드" },
  });
  fireEvent.click(screen.getByRole("button", { name: "연결 추가" }));

  expect(screen.getByText("저장한 뒤 다시 추가하세요")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "연결 삭제" })).toBeNull();
});

// Plan 5.4: no default is this PC, so adding a connection (here the first, a cloud one) must not
// make it the default.
test("adding a connection never changes the default LLM", async () => {
  const calls: Call[] = [];
  const none = { ...loaded, settings: { ...loaded.settings, llm_connections: [], default_llm: null } };
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return none;
    if (cmd === "has_secret") return false;
  });
  render(<Settings tab="llm" onHome={() => {}} />);
  fireEvent.change(await screen.findByRole("textbox", { name: "새 LLM 연결 이름" }), {
    target: { value: "회사 클라우드" },
  });
  fireEvent.click(screen.getByRole("button", { name: "연결 추가" }));
  expect(screen.getByRole("combobox", { name: "기본 LLM" }).textContent).toBe("선택 안 함");

  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  const saved = () => calls.filter((c) => c.cmd === "save_settings").map((c) => c.args.value);
  await waitFor(() => expect(saved()).toHaveLength(1));
  expect(saved()[0]).toMatchObject({ default_llm: null, llm_connections: [{ name: "회사 클라우드" }] });
});

test("a failed check opens the tab that holds the field", async () => {
  const cmds: string[] = [];
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
  });
  render(<Settings tab="transcription" onHome={() => {}} />);
  const tab = (name: string) => screen.getByRole("button", { name });

  const glossary = await screen.findByRole("textbox", { name: /^용어집/ });
  const terms = Array.from({ length: 101 }, (_, i) => `용어${i}`).join("\n");
  fireEvent.change(glossary, { target: { value: terms } });
  fireEvent.click(tab("일반"));
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(screen.getByText("용어집은 100개까지 넣을 수 있습니다")).toBeTruthy();
  expect(tab("전사").getAttribute("aria-current")).toBe("page");
  // Pressed again with the same problem: a new alert, said again.
  const said = screen.getByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(screen.getByRole("alert")).not.toBe(said);
  expect(screen.getByRole("alert").textContent).toBe("용어집은 100개까지 넣을 수 있습니다");

  fireEvent.change(glossary, { target: { value: "홍길동" } });
  fireEvent.click(tab("요약(LLM)"));
  await expand("회사 클라우드");
  fireEvent.change(screen.getByRole("spinbutton", { name: "컨텍스트 토큰" }), {
    target: { value: "" },
  });
  // Closed again: the failure opens it.
  await expand("회사 클라우드");
  fireEvent.click(tab("일반"));
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(screen.getByText(/컨텍스트 토큰과 응답 시간 제한은 1 이상의 정수여야 합니다/)).toBeTruthy();
  expect(tab("요약(LLM)").getAttribute("aria-current")).toBe("page");
  expect(screen.getByRole("button", { name: /^회사 클라우드/ }).getAttribute("aria-expanded")).toBe("true");
  expect(cmds).not.toContain("save_settings");
});

test("shows a pinned device before the list loads, and as unplugged once it has", async () => {
  const pinned: LoadedSettings = {
    ...loaded,
    settings: { ...loaded.settings, input_device: "coreaudio:사라진-마이크" },
  };
  const devices: AudioDevices = {
    inputs: [{ id: "coreaudio:BuiltInMicrophoneDevice", name: "MacBook 마이크" }],
    outputs: [],
  };
  mockIPC((cmd) => {
    if (cmd === "get_settings") return pinned;
    if (cmd === "has_secret") return true;
    if (cmd === "list_audio_devices") return devices;
  });
  render(<Settings onHome={() => {}} />);

  fireEvent.click(await screen.findByRole("button", { name: "오디오" }));
  const mic = screen.getByRole("combobox", { name: "마이크" });
  // Not blank while the list loads, but not called unplugged before the list can tell.
  expect(mic.textContent).toBe("설정한 장치");

  await waitFor(() => expect(mic.textContent).toContain("설정한 장치 (지금 연결되지 않음)"));
});

// Architecture 9.2: a key whose delete failed stays owned until a later 저장 deletes it.
test("retries a failed key delete on the next save and blocks re-adding the name meanwhile", async () => {
  const deletes: unknown[] = [];
  let deleteFails = true;
  mockIPC((cmd, args) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "delete_secret") {
      deletes.push(args);
      if (deleteFails) throw new Error("키체인 잠김");
    }
  });
  render(<Settings tab="llm" onHome={() => {}} />);
  const add = () => {
    fireEvent.change(screen.getByRole("textbox", { name: "새 LLM 연결 이름" }), {
      target: { value: "회사 클라우드" },
    });
    fireEvent.click(screen.getByRole("button", { name: "연결 추가" }));
  };

  await expand("회사 클라우드");
  fireEvent.click(screen.getByRole("button", { name: "연결 삭제" }));
  const dialog = await screen.findByRole("alertdialog", { name: "회사 클라우드 연결을 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText(/키체인에서 키를 지우지 못했습니다: .*키체인 잠김/)).toBeTruthy();

  add();
  expect(screen.getByText("저장한 뒤 다시 추가하세요")).toBeTruthy();

  deleteFails = false;
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(deletes).toEqual([
    { kind: "llm", name: "회사 클라우드" },
    { kind: "llm", name: "회사 클라우드" },
  ]);
  add();
  expect(screen.getByRole("button", { name: "연결 삭제" })).toBeTruthy();
});

test("never stores a whitespace-only key and clears it", async () => {
  const cmds: string[] = [];
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return false;
  });
  render(<Settings tab="llm" onHome={() => {}} />);

  await expand("회사 클라우드");
  fireEvent.change(screen.getByLabelText(/^API 키/), { target: { value: "   " } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));

  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(cmds).not.toContain("set_secret");
  expect((screen.getByLabelText(/^API 키/) as HTMLInputElement).value).toBe("");
});

test("removing an entry drops the key typed for it", async () => {
  const cmds: string[] = [];
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return false;
  });
  render(<Settings tab="slack" onHome={() => {}} />);
  const add = () => {
    fireEvent.change(screen.getByRole("textbox", { name: "새 웹훅 이름" }), {
      target: { value: "#회의록" },
    });
    fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));
  };

  await screen.findByRole("textbox", { name: "새 웹훅 이름" });
  add();
  fireEvent.change(screen.getByLabelText(/^웹훅 URL/), { target: { value: "https://hooks/x" } });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 삭제" }));
  const dialog = await screen.findByRole("alertdialog", { name: "#회의록 웹훅을 삭제할까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));

  // Never saved, so the same name may come back, without the old typed URL.
  add();
  expect((screen.getByLabelText(/^웹훅 URL/) as HTMLInputElement).value).toBe("");
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(cmds).not.toContain("set_secret");
});

test("read-only settings disable the key inputs and 저장", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return { ...loaded, read_only: true };
    if (cmd === "has_secret") return true;
  });
  render(<Settings tab="llm" onHome={() => {}} />);

  // Disabled through the fieldset, which `.disabled` does not reflect but `:disabled` does. Rows
  // still open, for reading.
  await expand("회사 클라우드");
  expect(screen.getByLabelText(/^API 키/).matches(":disabled")).toBe(true);
  // Even a change forced past the disabled field never brings the save bar.
  fireEvent.change(screen.getByLabelText(/^API 키/), { target: { value: "sk-막힘" } });
  fireEvent.click(screen.getByRole("button", { name: "일반" }));
  fireEvent.change(screen.getByRole("textbox", { name: "녹음자 이름" }), { target: { value: "김철수" } });
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
  expect(screen.queryByText(/^저장 안 한 변경/)).toBeNull();
  expect(screen.queryByRole("button", { name: "저장" })).toBeNull();
  // The tabs stay usable for reading; the selects are locked by their own prop, which Radix reads.
  expect(screen.getByRole("button", { name: "Slack" }).matches(":disabled")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM)" }));
  expect((screen.getByRole("combobox", { name: "기본 LLM" }) as HTMLButtonElement).disabled).toBe(true);
});

// DX1: Rust picks the folder, so the button only asks. Not a setting, so read-only settings keep it.
test("로그 폴더 열기 asks Rust to open the log folder and says beside it when it cannot", async () => {
  const cmds: string[] = [];
  let fails = true;
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") return { ...loaded, read_only: true };
    if (cmd === "has_secret") return true;
    if (cmd === "open_log_dir" && fails) throw "로그 폴더를 열지 못했습니다: 없음";
  });
  render(<Settings onHome={() => {}} />);

  const button = await screen.findByRole("button", {
    name: "로그 폴더 열기",
    description: "문제가 생기면 이 폴더의 app.log를 보내 주세요",
  });
  expect(button.matches(":disabled")).toBe(false);
  fireEvent.click(button);
  expect((await screen.findByRole("alert")).textContent).toBe("로그 폴더를 열지 못했습니다: 없음");
  // A later open that works clears it.
  fails = false;
  fireEvent.click(button);
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  expect(cmds.filter((c) => c === "open_log_dir")).toHaveLength(2);
});

// When settings.json cannot be read is when the log is needed most.
test("the unreadable-settings screen offers 로그 폴더 열기 too", async () => {
  const cmds: string[] = [];
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") throw "settings.json을 읽을 수 없습니다";
  });
  render(<Settings onHome={() => {}} />);

  expect(await screen.findByRole("button", { name: "기본값으로 다시 시작" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "로그 폴더 열기" }));
  await waitFor(() => expect(cmds).toContain("open_log_dir"));
});

// SUM-07 for each cloud address; SUM-08's default for new meetings needs a local connection.
test("요약(LLM) tab: where a transcript goes, and the local default only with a local connection", async () => {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") {
      return { ...loaded, settings: { ...loaded.settings, summarize_local_by_default: true } };
    }
    if (cmd === "has_secret") return true;
  });
  render(<Settings tab="llm" onHome={() => {}} />);
  const localDefault = () => screen.queryByRole("switch", { name: "새 회의를 기본으로 로컬에서 요약" });
  const saved = () => calls.filter((c) => c.cmd === "save_settings").map((c) => c.args.value);

  await expand("회사 클라우드");
  expect(screen.getByText("요약할 때 전사본 전문이 이 주소로 전송됩니다")).toBeTruthy();
  expect(screen.queryByText(/OLLAMA_CONTEXT_LENGTH/)).toBeNull();
  expect(localDefault()).toBeNull();
  // Hidden, so it cannot stay on. (The bar comes with an edit.)
  fireEvent.change(screen.getByRole("textbox", { name: "모델" }), { target: { value: "m2" } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await waitFor(() => expect(saved()).toMatchObject([{ summarize_local_by_default: false }]));

  fireEvent.click(screen.getByRole("switch", { name: "이 PC에서 도는 로컬 LLM (Ollama, LM Studio)" }));
  expect(screen.queryByText("요약할 때 전사본 전문이 이 주소로 전송됩니다")).toBeNull();
  expect(screen.getByText(/OLLAMA_CONTEXT_LENGTH/)).toBeTruthy();
  fireEvent.click(localDefault()!);
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await waitFor(() => expect(saved()).toHaveLength(2));
  expect(saved()[1]).toMatchObject({
    summarize_local_by_default: true,
    llm_connections: [{ name: "회사 클라우드", is_local: true }],
  });
});

const WHISPER = "ggml-large-v3-turbo-q8_0.bin";
const VAD = "ggml-silero-v6.2.0.bin";
const models = (whisper: Partial<ModelFileState> = {}): ModelState => ({
  files: [
    { file: WHISPER, size: 874_188_075, installed: false, received: 0, downloading: false, ...whisper },
    { file: VAD, size: 885_098, installed: true, received: 0, downloading: false },
  ],
  downloading: false,
});

test("전사 tab: file states, download progress from events, and a quiet cancel", async () => {
  const calls: Call[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: args as Call["args"] });
      if (cmd === "get_settings") return loaded;
      if (cmd === "has_secret") return true;
      if (cmd === "model_status") return models({ received: 437_094_037 });
      if (cmd === "download_models") {
        return new Promise((_, reject) => (release = () => reject("모델 다운로드를 취소했습니다")));
      }
      if (cmd === "cancel_model_download") {
        release();
        return "stopped";
      }
    },
    { shouldMockEvents: true },
  );
  render(<Settings tab="transcription" onHome={() => {}} />);

  expect(await screen.findByText("없음 · 437/874MB 받아 둠")).toBeTruthy();
  expect(screen.getByText("설치됨")).toBeTruthy();
  // `tiny` is development only.
  expect(screen.getByRole("combobox", { name: "모델" }).textContent).toBe("기본: large-v3-turbo q8_0 (874MB)");

  fireEvent.click(screen.getByRole("button", { name: "다운로드" }));
  await act(() => emit("model://download", { file: WHISPER, received: 524_512_845, total: 874_188_075 }));
  expect(await screen.findByText("받는 중 60% (525/874MB)")).toBeTruthy();
  // While it runs: only 취소 and the progress.
  expect(screen.queryByRole("button", { name: "다운로드" })).toBeNull();
  expect(screen.queryByRole("button", { name: "직접 가져오기" })).toBeNull();
  expect(screen.getByRole("progressbar", { name: "전사 모델 받는 중" })).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "취소" }));
  await waitFor(() =>
    expect((screen.getByRole("button", { name: "다운로드" }) as HTMLButtonElement).disabled).toBe(false),
  );
  // The tab's files only: the summary model belongs to the 요약(LLM) tab.
  const scopes = calls.filter((c) => c.cmd.includes("model")).map((c) => [c.cmd, c.args.scope]);
  expect(scopes).toEqual(
    expect.arrayContaining([
      ["model_status", "transcription"],
      ["download_models", "transcription"],
      ["cancel_model_download", "transcription"],
    ]),
  );
  expect(scopes.every(([, scope]) => scope === "transcription")).toBe(true);
  expect(screen.queryByRole("alert")).toBeNull();
});

test("직접 가져오기 hands the picked file to import_model", async () => {
  const calls: Call[] = [];
  const path = "/Users/me/Downloads/ggml-large-v3-turbo-q8_0.bin";
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models();
    if (cmd === "plugin:dialog|open") return path;
    if (cmd === "import_model") return models({ installed: true });
  });
  render(<Settings tab="transcription" onHome={() => {}} />);

  expect(await screen.findByText("없음")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "직접 가져오기" }));

  await waitFor(() => expect(screen.getAllByText("설치됨")).toHaveLength(2));
  expect(calls.find((c) => c.cmd === "import_model")?.args).toEqual({ path, scope: "transcription" });
  // Transcription files are `.bin`; the summary model's import (scope `summary`) picks `.gguf`.
  expect(calls.find((c) => c.cmd === "plugin:dialog|open")?.args).toMatchObject({
    options: { filters: [{ extensions: ["bin"] }] },
  });
  expect(modelFileFilter("summary").extensions).toEqual(["gguf"]);
});

test("a failed download shows its error in the 전사 tab, not the shared banner", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models();
    if (cmd === "download_models") throw new Error("huggingface.co에 연결할 수 없습니다");
  });
  render(<Settings tab="transcription" onHome={() => {}} />);

  expect(await screen.findByText("없음")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "다운로드" }));

  expect((await screen.findByRole("alert")).textContent).toMatch(/huggingface\.co에 연결할 수 없습니다/);
  fireEvent.click(screen.getByRole("button", { name: "일반" }));
  expect(screen.queryByRole("alert")).toBeNull();
});

// Download and import act on what settings.json holds, so an unsaved change waits for 저장.
test("an unsaved model address blocks download and import until saved", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models();
  });
  render(<Settings tab="transcription" onHome={() => {}} />);
  const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

  expect(await screen.findByText("없음")).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox", { name: /^모델 다운로드 주소/ }), {
    target: { value: "https://mirror.example.com" },
  });
  expect(button("다운로드").disabled).toBe(true);
  expect(button("직접 가져오기").disabled).toBe(true);
  expect(screen.getByText("바꾼 모델과 주소는 저장한 뒤에 받거나 가져올 수 있습니다")).toBeTruthy();

  fireEvent.click(button("저장"));
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(button("다운로드").disabled).toBe(false);
  expect(button("직접 가져오기").disabled).toBe(false);
});

// The download lives in ipc.ts, not in the screen: leaving and coming back must not lose it, and a
// failure while the screen was closed shows on the next visit.
test("reopened mid-download, the tab follows the same download and later shows its failure", async () => {
  let fail = (_: string) => {};
  mockIPC(
    (cmd) => {
      if (cmd === "get_settings") return loaded;
      if (cmd === "has_secret") return true;
      if (cmd === "model_status") return models();
      if (cmd === "download_models") {
        return new Promise((resolve, reject) => {
          release = () => resolve(undefined);
          fail = reject;
        });
      }
    },
    { shouldMockEvents: true },
  );
  const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;
  const first = render(<Settings tab="transcription" onHome={() => {}} />);
  expect(await screen.findByText("없음")).toBeTruthy();
  fireEvent.click(button("다운로드"));
  first.unmount();

  const second = render(<Settings tab="transcription" onHome={() => {}} />);
  await waitFor(() => expect(button("취소").disabled).toBe(false));
  expect(screen.queryByRole("button", { name: "다운로드" })).toBeNull();
  // One decimal below 10 MB.
  await act(() => emit("model://download", { file: WHISPER, received: 4_370_940, total: 874_188_075 }));
  expect(await screen.findByText("받는 중 0% (4.4/874MB)")).toBeTruthy();
  second.unmount();

  await act(async () => fail("huggingface.co에 연결할 수 없습니다"));
  render(<Settings tab="transcription" onHome={() => {}} />);
  expect((await screen.findByRole("alert")).textContent).toMatch(/huggingface\.co에 연결할 수 없습니다/);
  expect(button("다운로드").disabled).toBe(false);
});

// A chain started on Home goes on to the summary model after the tab's two files.
test("the 전사 tab offers 취소 only while one of its files is still to come", async () => {
  let past = false;
  mockIPC(
    (cmd) => {
      if (cmd === "get_settings") return loaded;
      if (cmd === "has_secret") return true;
      if (cmd === "model_status") {
        return { ...models(past ? { installed: true } : { downloading: true }), downloading: true };
      }
      if (cmd === "download_models") return new Promise((resolve) => (release = () => resolve(undefined)));
      if (cmd === "cancel_model_download") {
        past = true;
        return null;
      }
    },
    { shouldMockEvents: true },
  );
  void startModelDownload();
  const first = render(<Settings tab="transcription" onHome={() => {}} />);
  // Read before the chain moved on. Answered null, the tab reads again and lets the button go.
  fireEvent.click(await screen.findByRole("button", { name: "취소" }));
  await waitFor(() => expect(screen.getAllByText("설치됨")).toHaveLength(2));
  expect(screen.queryByRole("button", { name: "취소" })).toBeNull();
  first.unmount();

  render(<Settings tab="transcription" onHome={() => {}} />);
  await waitFor(() => expect(screen.getAllByText("설치됨")).toHaveLength(2));
  expect(screen.queryByRole("button", { name: "취소" })).toBeNull();
});

test("once a cancel stopped the download, a later answer cannot turn its end into a failure", async () => {
  const answers = ["stopped", null];
  mockIPC((cmd) => {
    if (cmd === "download_models") {
      return new Promise((_, reject) => (release = () => reject("모델 다운로드를 취소했습니다")));
    }
    if (cmd === "cancel_model_download") return answers.shift();
  });
  const running = startModelDownload();
  expect(await cancelModelDownload()).toBe("stopped");
  expect(await cancelModelDownload("summary")).toBeNull();
  release();
  await running;
  expect(modelDownloadFailure()).toBe("");
});

test("the 전사 tab explains a GPU fallback in automatic mode", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return { ...loaded, settings: { ...loaded.settings, gpu_failed: true } };
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models();
  });
  render(<Settings tab="transcription" onHome={() => {}} />);

  expect(
    await screen.findByText(
      "GPU 전사에 실패해 CPU로 처리하고 있습니다. 전사 백엔드를 GPU로 골라 저장하면 다시 GPU로 처리합니다.",
    ),
  ).toBeTruthy();
});

test("the selected tab carries the sliding indicator", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models();
  });
  render(<Settings onHome={() => {}} />);
  const tab = (name: string) => screen.getByRole("button", { name });
  const marked = () =>
    ["일반", "오디오", "전사", "요약(LLM)", "Slack"].filter((name) => tab(name).querySelector("[data-indicator]"));

  await screen.findByRole("heading", { name: "일반" });
  expect(marked()).toEqual(["일반"]);
  fireEvent.click(tab("전사"));
  expect(marked()).toEqual(["전사"]);
  expect(tab("전사").getAttribute("aria-current")).toBe("page");
  expect(screen.getByRole("heading", { name: "전사" })).toBeTruthy();
  expect(screen.queryByRole("heading", { name: "일반" })).toBeNull();
});

test("the backend is a segmented choice, and choosing GPU clears the fallback", async () => {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return { ...loaded, settings: { ...loaded.settings, gpu_failed: true } };
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") return models();
  });
  render(<Settings tab="transcription" onHome={() => {}} />);
  const choice = (name: string) => screen.getByRole("radio", { name }) as HTMLInputElement;

  // Named by its row's label and described by its help, as every row's control.
  const group = await screen.findByRole("radiogroup", { name: "전사 백엔드" });
  const help = document.getElementById(group.getAttribute("aria-describedby")!);
  expect(help?.textContent).toBe("자동은 GPU로 처리하다 문제가 생기면 CPU로 바꿉니다");
  const label = screen.getByText("전사 백엔드", { selector: "label" }) as HTMLLabelElement;
  expect(document.getElementById(label.htmlFor)).toBe(group);
  expect(within(group).getAllByRole("radio").map((r) => r.closest("label")?.textContent)).toEqual(["자동", "GPU", "CPU"]);
  expect(choice("자동").checked).toBe(true);
  fireEvent.click(choice("GPU"));
  expect(choice("GPU").checked).toBe(true);
  expect(choice("자동").checked).toBe(false);

  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await waitFor(() =>
    expect(calls.find((c) => c.cmd === "save_settings")?.args.value).toMatchObject({ backend: "gpu", gpu_failed: false }),
  );
  expect(screen.queryByText(/GPU 전사에 실패해/)).toBeNull();
});

test("connection and webhook rows open in place and close again", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
  });
  render(<Settings tab="llm" onHome={() => {}} />);
  const row = (name: string) => screen.getByRole("button", { name: new RegExp(`^${name}`) });

  // Its summary: name, where it sends, a tag in words, and the default marker.
  expect((await screen.findByRole("button", { name: /^회사 클라우드/ })).getAttribute("aria-label")).toBe(
    "회사 클라우드, 클라우드, api.example.com, 기본 LLM",
  );
  expect(row("회사 클라우드").getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("textbox", { name: "Base URL" })).toBeNull();
  fireEvent.click(row("회사 클라우드"));
  expect(row("회사 클라우드").getAttribute("aria-expanded")).toBe("true");
  expect((screen.getByRole("textbox", { name: "Base URL" }) as HTMLInputElement).value).toBe("https://api.example.com/v1");
  fireEvent.click(row("회사 클라우드"));
  expect(screen.queryByRole("textbox", { name: "Base URL" })).toBeNull();

  // A new webhook opens for its URL; removed, focus goes to the add row, not the page.
  fireEvent.click(screen.getByRole("button", { name: "Slack" }));
  fireEvent.change(screen.getByRole("textbox", { name: "새 웹훅 이름" }), { target: { value: "#회의록" } });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));
  expect(row("#회의록").getAttribute("aria-expanded")).toBe("true");
  expect(row("#회의록").getAttribute("aria-label")).toBe("#회의록, URL 입력 안 됨");
  fireEvent.click(row("#회의록"));
  expect(screen.queryByLabelText(/^웹훅 URL/)).toBeNull();
  fireEvent.click(row("#회의록"));
  fireEvent.click(screen.getByRole("button", { name: "웹훅 삭제" }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "새 웹훅 이름" })));
});

test("the save bar shows only with unsaved changes, and counts them", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return false;
    if (cmd === "model_status") return models();
  });
  render(<Settings onHome={() => {}} />);
  const count = () => screen.queryByText(/^저장 안 한 변경/)?.textContent ?? null;
  const name = await screen.findByRole("textbox", { name: "녹음자 이름" });

  expect(screen.queryByRole("button", { name: "저장" })).toBeNull();
  expect(count()).toBeNull();
  // 저장 trims the name, so spaces alone change nothing.
  fireEvent.change(name, { target: { value: "홍길동 " } });
  expect(count()).toBeNull();
  fireEvent.change(name, { target: { value: "김철수" } });
  expect(count()).toBe("저장 안 한 변경 1개");

  fireEvent.click(screen.getByRole("button", { name: "전사" }));
  fireEvent.click(await screen.findByRole("radio", { name: "CPU" }));
  // Blank lines and spaces around terms are not a change: 저장 drops them.
  fireEvent.change(screen.getByRole("textbox", { name: /^용어집/ }), { target: { value: "\n  \n" } });
  expect(count()).toBe("저장 안 한 변경 2개");

  // An added webhook and its typed URL are two; an edited connection is one however many fields.
  fireEvent.click(screen.getByRole("button", { name: "Slack" }));
  fireEvent.change(screen.getByRole("textbox", { name: "새 웹훅 이름" }), { target: { value: "#회의록" } });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));
  fireEvent.change(screen.getByLabelText(/^웹훅 URL/), { target: { value: "https://hooks/x" } });
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM)" }));
  await expand("회사 클라우드");
  fireEvent.change(screen.getByRole("textbox", { name: "모델" }), { target: { value: "m2" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "컨텍스트 토큰" }), { target: { value: "1000" } });
  expect(count()).toBe("저장 안 한 변경 5개");

  // Typing it back is no change at all.
  fireEvent.click(screen.getByRole("button", { name: "일반" }));
  fireEvent.change(name, { target: { value: "홍길동" } });
  expect(count()).toBe("저장 안 한 변경 4개");
});

test("되돌리기 puts back what was saved, typed keys too after asking", async () => {
  const cmds: string[] = [];
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return false;
  });
  render(<Settings onHome={() => {}} />);
  const name = (await screen.findByRole("textbox", { name: "녹음자 이름" })) as HTMLInputElement;

  // Nothing typed into a key: no question.
  fireEvent.change(name, { target: { value: "김철수" } });
  fireEvent.click(screen.getByRole("button", { name: "되돌리기" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(name.value).toBe("홍길동");
  await waitFor(() => expect(screen.queryByRole("button", { name: "저장" })).toBeNull());

  fireEvent.change(name, { target: { value: "김철수" } });
  fireEvent.click(screen.getByRole("button", { name: "Slack" }));
  fireEvent.change(screen.getByRole("textbox", { name: "새 웹훅 이름" }), { target: { value: "#회의록" } });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));
  fireEvent.change(screen.getByLabelText(/^웹훅 URL/), { target: { value: "https://hooks/x" } });
  fireEvent.click(screen.getByRole("button", { name: "되돌리기" }));
  const dialog = await screen.findByRole("alertdialog", { name: "입력한 키도 되돌릴까요?" });
  fireEvent.click(within(dialog).getByRole("button", { name: "되돌리기" }));

  await waitFor(() => expect(screen.queryByRole("button", { name: "저장" })).toBeNull());
  expect(screen.queryByRole("button", { name: /^#회의록/ })).toBeNull();
  expect(name.value).toBe("홍길동");
  // Added again, it comes back without the URL typed before.
  fireEvent.change(screen.getByRole("textbox", { name: "새 웹훅 이름" }), { target: { value: "#회의록" } });
  fireEvent.click(screen.getByRole("button", { name: "웹훅 추가" }));
  expect((screen.getByLabelText(/^웹훅 URL/) as HTMLInputElement).value).toBe("");
  expect(cmds).not.toContain("save_settings");
  expect(cmds).not.toContain("set_secret");
});

test("after a save the bar says so for a moment, then leaves", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
  });
  render(<Settings onHome={() => {}} />);

  fireEvent.change(await screen.findByRole("textbox", { name: "녹음자 이름" }), { target: { value: "김철수" } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(screen.getByRole("status").textContent).toBe("설정을 저장했습니다");
  expect(screen.queryByRole("button", { name: "저장" })).toBeNull();

  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.queryByText("저장했습니다")).toBeNull();
  expect(screen.getByRole("status").textContent).toBe("");
});

test("saving locks the form and the bar until it ends", async () => {
  let finish = () => {};
  const cmds: string[] = [];
  mockIPC((cmd) => {
    cmds.push(cmd);
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "save_settings") return new Promise<void>((resolve) => (finish = resolve));
  });
  render(<Settings onHome={() => {}} />);
  const name = await screen.findByRole("textbox", { name: "녹음자 이름" });

  fireEvent.change(name, { target: { value: "김철수" } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  const saving = await screen.findByRole("button", { name: "저장 중…" });
  expect(saving.getAttribute("aria-disabled")).toBe("true");
  expect(screen.getByRole("button", { name: "되돌리기" }).getAttribute("aria-disabled")).toBe("true");
  expect(name.matches(":disabled")).toBe(true);
  // Pressed again meanwhile: still one save, and nothing reverted.
  fireEvent.click(saving);
  fireEvent.click(screen.getByRole("button", { name: "되돌리기" }));
  expect(cmds.filter((c) => c === "save_settings")).toHaveLength(1);
  expect((name as HTMLInputElement).value).toBe("김철수");
  // Radix opens a select on pointerdown whatever its fieldset says: it is locked by its own prop.
  fireEvent.click(screen.getByRole("button", { name: "요약(LLM)" }));
  expect((screen.getByRole("combobox", { name: "기본 LLM" }) as HTMLButtonElement).disabled).toBe(true);

  await act(async () => finish());
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(name.matches(":disabled")).toBe(false);
});

// Architecture 9.2's keychain steps run after settings.json is written: the bar holds through them.
test("the save bar stays through the keychain steps, then hands focus to the tab", async () => {
  let finishDelete = () => {};
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "delete_secret") return new Promise<void>((resolve) => (finishDelete = resolve));
  });
  render(<Settings tab="llm" onHome={() => {}} />);
  await expand("회사 클라우드");
  fireEvent.click(screen.getByRole("button", { name: "연결 삭제" }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "삭제" }));
  await waitFor(() => expect(screen.queryByRole("button", { name: /^회사 클라우드/ })).toBeNull());

  const save = screen.getByRole("button", { name: "저장" });
  save.focus();
  fireEvent.click(save, { detail: 0 });
  // settings.json is written and the key's delete is still out: the count stays as pressed.
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "저장 중…" }));
  expect(screen.getByText("저장 안 한 변경 2개")).toBeTruthy();

  await act(async () => finishDelete());
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "요약(LLM)" }));
});

// ONB-02: import is the way out when the model state cannot be read.
test("직접 가져오기 stays offered when the model status cannot be read", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "model_status") throw new Error("모델 폴더를 읽지 못했습니다");
  });
  render(<Settings tab="transcription" onHome={() => {}} />);

  expect((await screen.findByRole("alert")).textContent).toMatch(/모델 폴더를 읽지 못했습니다/);
  expect((screen.getByRole("button", { name: "직접 가져오기" }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByRole("button", { name: "다운로드" })).toBeNull();
});

// ONB-03 and ONB-06: checks change no setting, so the save bar never counts them.
const unsaved = () => screen.queryByText(/^저장 안 한 변경/)?.textContent ?? null;
const twoMics: AudioDevices = {
  inputs: [
    { id: "coreaudio:BuiltInMicrophoneDevice", name: "MacBook 마이크" },
    { id: "coreaudio:usb", name: "USB 마이크" },
  ],
  outputs: [],
};

test("the mic check listens to the mic shown, saved or not, and draws its level", async () => {
  const calls: Call[] = [];
  let finish = (_heard: boolean) => {};
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: args as Call["args"] });
      if (cmd === "get_settings") return loaded;
      if (cmd === "has_secret") return true;
      if (cmd === "list_audio_devices") return twoMics;
      if (cmd === "test_microphone") return new Promise<boolean>((resolve) => (finish = resolve));
    },
    { shouldMockEvents: true },
  );
  render(<Settings tab="audio" onHome={() => {}} />);
  const tests = () => calls.filter((c) => c.cmd === "test_microphone").map((c) => c.args);

  const check = await screen.findByRole("button", { name: "마이크 테스트" });
  expect(screen.getByText("5초 동안 마이크 소리를 들어 봅니다")).toBeTruthy();
  fireEvent.keyDown(screen.getByRole("combobox", { name: "마이크" }), { key: "Enter" });
  fireEvent.click(await screen.findByRole("option", { name: "USB 마이크" }));
  expect(unsaved()).toBe("저장 안 한 변경 1개");
  // The tab never opens the mic by itself: that would raise the OS permission prompt.
  expect(tests()).toEqual([]);

  fireEvent.click(check);
  expect(tests()).toEqual([{ device: "coreaudio:usb" }]);
  expect(check.textContent).toBe("테스트 중…");
  // Its name says so too, like what it shows.
  expect(check.getAttribute("aria-label")).toBe("마이크 테스트 중");
  expect(check.getAttribute("aria-disabled")).toBe("true");
  expect(screen.getByText("마이크에 대고 말해 보세요")).toBeTruthy();
  fireEvent.click(check);
  expect(tests()).toHaveLength(1);
  // Rust runs one audio check at a time, so the other is not offered meanwhile.
  const system = screen.getByRole("button", { name: "시스템 오디오 테스트" });
  expect(system.getAttribute("aria-disabled")).toBe("true");
  fireEvent.click(system);
  expect(calls.map((c) => c.cmd)).not.toContain("test_system_audio");

  await act(() => emit("audio-test://level", 64));
  const meter = screen.getByRole("meter", { name: "마이크 입력 레벨" });
  expect(meter.getAttribute("aria-valuenow")).toBe("64");
  expect((meter.firstElementChild as HTMLElement).style.transform).toBe("scaleX(0.64)");

  await act(async () => finish(false));
  expect(system.getAttribute("aria-disabled")).toBeNull();
  // Said once, through a status line. macOS lists the app by its bundle name.
  const outcome = await screen.findByText(/^소리가 들어오지 않아요\./);
  expect(outcome.getAttribute("role")).toBe("status");
  expect(outcome.textContent).toContain("개인정보 보호 및 보안 > 마이크에서 'meeting-summarizer'를 허용하세요");
  expect(screen.queryByRole("meter")).toBeNull();
  expect(check.getAttribute("aria-disabled")).toBeNull();
  expect(check.getAttribute("aria-label")).toBe("마이크 테스트");

  fireEvent.click(check);
  await act(async () => finish(true));
  expect(await screen.findByText("소리가 들어와요")).toBeTruthy();
  expect(unsaved()).toBe("저장 안 한 변경 1개");

  // An outcome speaks of the mic it ran on, even one that arrives after another was picked.
  const pickMic = async (name: string) => {
    fireEvent.keyDown(screen.getByRole("combobox", { name: "마이크" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("option", { name }));
  };
  await pickMic("MacBook 마이크");
  expect(screen.queryByText("소리가 들어와요")).toBeNull();
  fireEvent.click(check);
  await pickMic("USB 마이크");
  await act(async () => finish(true));
  expect(screen.queryByText("소리가 들어와요")).toBeNull();
  expect(screen.getByText("5초 동안 마이크 소리를 들어 봅니다")).toBeTruthy();
});

test("the system audio check says what it heard, and no check runs while recording", async () => {
  const calls: Call[] = [];
  const answers = [true, false];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return loaded;
    if (cmd === "has_secret") return true;
    if (cmd === "list_audio_devices") return twoMics;
    if (cmd === "test_system_audio") return answers.shift();
    if (cmd === "test_microphone") throw "마이크를 열 수 없습니다: 장치가 사용 중입니다";
  });
  const { rerender } = render(<Settings tab="audio" onHome={() => {}} />);
  const tests = () => calls.filter((c) => c.cmd.startsWith("test_"));

  const system = await screen.findByRole("button", { name: "시스템 오디오 테스트" });
  expect(screen.getByText("짧은 소리를 재생해 앱에 들리는지 확인합니다")).toBeTruthy();
  fireEvent.click(system);
  expect(await screen.findByText("시스템 오디오가 잡혀요")).toBeTruthy();
  expect(tests().map((c) => c.args)).toEqual([{ device: null }]);
  fireEvent.click(system);
  // The loopback hears before the output volume, so only the permission is offered as the cause.
  // macOS lists the app by its bundle name.
  expect((await screen.findByText(/^시스템 오디오가 잡히지 않아요\./)).textContent).toBe(
    "시스템 오디오가 잡히지 않아요. 시스템 설정 > 개인정보 보호 및 보안 > 화면 및 시스템 오디오 녹음에서 'meeting-summarizer'를 허용하세요",
  );
  // A device that cannot be opened says so in Rust's words.
  fireEvent.click(screen.getByRole("button", { name: "마이크 테스트" }));
  expect(await screen.findByText("마이크를 열 수 없습니다: 장치가 사용 중입니다")).toBeTruthy();
  expect(unsaved()).toBeNull();

  rerender(<Settings tab="audio" recording onHome={() => {}} />);
  for (const name of ["마이크 테스트", "시스템 오디오 테스트"]) {
    const button = screen.getByRole("button", { name });
    expect(button.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(button);
  }
  // Said once for both.
  expect(screen.getByText("녹음 중에는 테스트할 수 없어요").getAttribute("role")).toBe("status");
  expect(tests()).toHaveLength(3);
});

test("a test message goes with the URL typed, else the saved one, and leaves the draft alone", async () => {
  const calls: Call[] = [];
  const withWebhook: LoadedSettings = {
    ...loaded,
    settings: { ...loaded.settings, webhooks: [{ name: "#회의록" }] },
  };
  let finish = () => {};
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Call["args"] });
    if (cmd === "get_settings") return withWebhook;
    if (cmd === "has_secret") return true;
    if (cmd === "send_test_message") {
      if (!(args as Call["args"]).url) return new Promise<void>((resolve) => (finish = resolve));
      throw "Slack에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요: connection refused";
    }
  });
  render(<Settings tab="slack" onHome={() => {}} />);
  const sends = () => calls.filter((c) => c.cmd === "send_test_message").map((c) => c.args);
  await expand("#회의록");

  const send = screen.getByRole("button", { name: "테스트 메시지 보내기" });
  fireEvent.click(send);
  expect(sends()).toEqual([{ webhook: "#회의록", url: null }]);
  expect(send.textContent).toBe("보내는 중…");
  expect(send.getAttribute("aria-disabled")).toBe("true");
  await act(async () => finish());
  expect(await screen.findByText("보냈습니다. Slack 채널을 확인하세요")).toBeTruthy();
  expect(unsaved()).toBeNull();

  fireEvent.change(screen.getByLabelText(/^웹훅 URL/), {
    target: { value: " https://hooks.slack.com/services/new " },
  });
  // It went to the saved URL, not this one.
  expect(screen.queryByText("보냈습니다. Slack 채널을 확인하세요")).toBeNull();
  fireEvent.click(send);
  expect(sends()[1]).toEqual({ webhook: "#회의록", url: "https://hooks.slack.com/services/new" });
  expect(await screen.findByText(/^Slack에 연결하지 못했습니다/)).toBeTruthy();
  // Only the typed URL counts; the test sent nothing to save.
  expect(unsaved()).toBe("저장 안 한 변경 1개");
  expect(calls.map((c) => c.cmd)).not.toContain("set_secret");
});

// The typed URL is saved and its field empties: the result for the old keychain URL must not return.
test("a test result for the saved URL goes once a new URL is saved", async () => {
  mockIPC((cmd) => {
    if (cmd === "get_settings") return { ...loaded, settings: { ...loaded.settings, webhooks: [{ name: "#회의록" }] } };
    if (cmd === "has_secret") return true;
  });
  render(<Settings tab="slack" onHome={() => {}} />);
  await expand("#회의록");
  fireEvent.click(screen.getByRole("button", { name: "테스트 메시지 보내기" }));
  expect(await screen.findByText("보냈습니다. Slack 채널을 확인하세요")).toBeTruthy();

  fireEvent.change(screen.getByLabelText(/^웹훅 URL/), { target: { value: "https://hooks.slack.com/services/b" } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(await screen.findByText("저장했습니다")).toBeTruthy();
  expect((screen.getByLabelText(/^웹훅 URL/) as HTMLInputElement).value).toBe("");
  expect(screen.queryByText("보냈습니다. Slack 채널을 확인하세요")).toBeNull();
});
