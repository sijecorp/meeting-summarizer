import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import {
  modelDownload,
  modelDownloadFailure,
  startModelDownload,
  type AudioDevices,
  type LoadedSettings,
  type ModelFileState,
  type ModelState,
} from "../lib/ipc";
import Settings from "./Settings";

// The model download is app-wide (ipc.ts): each test's is settled and awaited here, and a failure
// it left (kept until the next download) is cleared by one that works, so nothing leaks into the
// next test. No clearMocks(): the screen unlistens asynchronously after unmount (see App.test.tsx).
let release = () => {};
afterEach(async () => {
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
    screen.queryByRole("textbox", { name: "녹음자 이름 (Slack 메시지에 표시됩니다)" }) as HTMLInputElement;

  await screen.findByDisplayValue("홍길동");
  fireEvent.change(recorderName(), { target: { value: "김철수" } });

  fireEvent.click(tab("요약(LLM)"));
  expect(tab("요약(LLM)").getAttribute("aria-current")).toBe("page");
  expect(tab("일반").getAttribute("aria-current")).toBeNull();
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

  fireEvent.change(await screen.findByLabelText(/^API 키/), { target: { value: "sk-새 키" } });
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

  fireEvent.click(await screen.findByRole("button", { name: "연결 삭제" }));
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

  fireEvent.change(glossary, { target: { value: "홍길동" } });
  fireEvent.click(tab("요약(LLM)"));
  fireEvent.change(screen.getByRole("spinbutton", { name: "컨텍스트 토큰" }), {
    target: { value: "" },
  });
  fireEvent.click(tab("일반"));
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  expect(screen.getByText(/컨텍스트 토큰과 응답 시간 제한은 1 이상의 정수여야 합니다/)).toBeTruthy();
  expect(tab("요약(LLM)").getAttribute("aria-current")).toBe("page");
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

  fireEvent.click(await screen.findByRole("button", { name: "연결 삭제" }));
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

  fireEvent.change(await screen.findByLabelText(/^API 키/), { target: { value: "   " } });
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

  // Disabled through the fieldset, which `.disabled` does not reflect but `:disabled` does.
  expect((await screen.findByLabelText(/^API 키/)).matches(":disabled")).toBe(true);
  expect(screen.getByRole("button", { name: "저장" }).matches(":disabled")).toBe(true);
  // The tabs stay usable for reading.
  expect(screen.getByRole("button", { name: "Slack" }).matches(":disabled")).toBe(false);
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

  expect(await screen.findByText("요약할 때 전사본 전문이 이 주소로 전송됩니다")).toBeTruthy();
  expect(screen.queryByText(/OLLAMA_CONTEXT_LENGTH/)).toBeNull();
  expect(localDefault()).toBeNull();
  // Hidden, so it cannot stay on.
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
    { file: WHISPER, size: 874_188_075, installed: false, received: 0, ...whisper },
    { file: VAD, size: 885_098, installed: true, received: 0 },
  ],
  downloading: false,
});

test("전사 tab: file states, download progress from events, and a quiet cancel", async () => {
  const cmds: string[] = [];
  mockIPC(
    (cmd) => {
      cmds.push(cmd);
      if (cmd === "get_settings") return loaded;
      if (cmd === "has_secret") return true;
      if (cmd === "model_status") return models({ received: 437_094_037 });
      if (cmd === "download_models") {
        return new Promise((_, reject) => (release = () => reject("모델 다운로드를 취소했습니다")));
      }
      if (cmd === "cancel_model_download") release();
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
  expect((screen.getByRole("button", { name: "다운로드" }) as HTMLButtonElement).disabled).toBe(true);

  fireEvent.click(screen.getByRole("button", { name: "취소" }));
  await waitFor(() =>
    expect((screen.getByRole("button", { name: "다운로드" }) as HTMLButtonElement).disabled).toBe(false),
  );
  expect(cmds).toContain("cancel_model_download");
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
  expect(calls.find((c) => c.cmd === "import_model")?.args).toEqual({ path });
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
  expect(button("다운로드").disabled).toBe(true);
  // One decimal below 10 MB.
  await act(() => emit("model://download", { file: WHISPER, received: 4_370_940, total: 874_188_075 }));
  expect(await screen.findByText("받는 중 0% (4.4/874MB)")).toBeTruthy();
  second.unmount();

  await act(async () => fail("huggingface.co에 연결할 수 없습니다"));
  render(<Settings tab="transcription" onHome={() => {}} />);
  expect((await screen.findByRole("alert")).textContent).toMatch(/huggingface\.co에 연결할 수 없습니다/);
  expect(button("다운로드").disabled).toBe(false);
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
