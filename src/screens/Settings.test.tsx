import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { LoadedSettings } from "../lib/ipc";
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
  fireEvent.click(await screen.findByRole("button", { name: "연결 삭제" }));
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
