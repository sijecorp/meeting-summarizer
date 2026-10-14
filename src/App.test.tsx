import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import App from "./App";
import type { LoadedSettings } from "./lib/ipc";

// No clearMocks() here: App unlistens asynchronously after unmount and still needs the mocked
// event internals. Vitest gives each test file its own window.
afterEach(cleanup);

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
    webhooks: [{ name: "#회의록" }],
    last_channel: null,
    glossary: [],
  },
};

test("switches from home to settings and back", async () => {
  mockIPC(
    (cmd) => {
      if (cmd === "get_settings") return loaded;
      if (cmd === "list_meetings") return [];
      if (cmd === "has_secret") return true;
    },
    { shouldMockEvents: true },
  );

  render(<App />);

  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "설정" }));

  expect(await screen.findByDisplayValue("홍길동")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1, name: "설정" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "홈으로" }));

  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  expect(screen.getByRole("heading", { level: 1, name: "회의 요약" })).toBeTruthy();
});

test("quit dialog's 종료 button confirms the quit", async () => {
  const calls: string[] = [];
  mockIPC(
    (cmd) => {
      calls.push(cmd);
      if (cmd === "get_settings") return loaded;
      if (cmd === "list_meetings") return [];
    },
    { shouldMockEvents: true },
  );

  render(<App />);
  expect(await screen.findByText("아직 회의가 없습니다")).toBeTruthy();
  await act(() => emit("app://quit-requested"));

  const dialog = await screen.findByRole("alertdialog", { name: "진행 중인 작업이 있습니다" });
  expect(calls).not.toContain("confirm_quit");
  fireEvent.click(within(dialog).getByRole("button", { name: "종료" }));

  await waitFor(() => expect(calls).toContain("confirm_quit"));
});
