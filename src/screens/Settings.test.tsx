import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { AudioDevices, LoadedSettings } from "../lib/ipc";
import Settings from "./Settings";

afterEach(() => {
  cleanup();
  clearMocks();
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
