import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test, vi } from "vitest";
import type { LlmConnection, Meta, Progress, SummaryRoute } from "../lib/ipc";
import { EngineStarting, phase, RouteNotice } from "./common";

afterEach(() => {
  cleanup();
  clearMocks();
  vi.useRealTimers();
});

const cloud: LlmConnection = {
  name: "회사 클라우드",
  base_url: "https://api.example.com/v1",
  model: "m",
  context_tokens: 200000,
  is_local: false,
  timeout_secs: 300,
};
const ollama: LlmConnection = { ...cloud, name: "Ollama", base_url: "http://localhost:11434/v1", is_local: true };
// `settings::builtin`: the built-in route, its address given at run time.
const thisPc: LlmConnection = { ...ollama, name: "builtin", base_url: "", context_tokens: 16_384, timeout_secs: 1_800 };

const route = (patch: Partial<SummaryRoute>): SummaryRoute => ({
  connection: cloud,
  local: false,
  local_available: false,
  locked: false,
  toggle_matters: true,
  ...patch,
});

// SUM-07, SUM-08.
test("says where the summary goes, and offers 로컬로 요약 only with a local connection", async () => {
  const cases: [SummaryRoute, string, boolean][] = [
    [route({}), "요약할 때 전사본이 회사 클라우드(api.example.com)로 전송됩니다", false],
    [
      route({ connection: ollama, local: true, local_available: true }),
      "이 PC의 Ollama로 요약합니다. 전사본은 밖으로 나가지 않습니다",
      true,
    ],
    [
      route({ connection: thisPc, local_available: true }),
      "이 PC에서 요약합니다. 전사본은 밖으로 나가지 않습니다",
      true,
    ],
    // No route: only the summary model can be missing (plan 5.4).
    [route({ connection: null }), "설정 → 요약(LLM)에서 요약 모델을 받아 주세요", false],
    // Its local connection was removed: the switch stays so it can be turned off.
    [route({ connection: null, local: true }), "설정 → 요약(LLM)에서 요약 모델을 받아 주세요", true],
    // Where flipping it changes nothing (a local default, this PC with its model): no switch.
    [route({ connection: thisPc, local_available: true, toggle_matters: false }), "이 PC에서 요약합니다. 전사본은 밖으로 나가지 않습니다", false],
    [route({ connection: ollama, local_available: true, toggle_matters: false }), "이 PC의 Ollama로 요약합니다. 전사본은 밖으로 나가지 않습니다", false],
    // This PC without its model, a saved Ollama beside it: on goes there, off waits.
    [route({ connection: null, local_available: true }), "설정 → 요약(LLM)에서 요약 모델을 받아 주세요", true],
  ];
  for (const [r, text, toggle] of cases) {
    mockIPC((cmd) => (cmd === "summary_route" ? r : undefined));
    render(<RouteNotice id="a" />);
    expect(await screen.findByText(text)).toBeTruthy();
    expect(!!screen.queryByRole("switch", { name: "로컬로 요약" })).toBe(toggle);
    cleanup();
  }
});

// Plan 5.4: a waiting meeting's own reason, even once its route is pinned (locked).
test("says why a pinned meeting waits: its connection deleted, or the summary model", async () => {
  mockIPC((cmd) => {
    if (cmd === "summary_route") return route({ connection: null, locked: true });
    if (cmd === "model_status") {
      const file = { file: "m.gguf", size: 100, installed: false, received: 20, downloading: true };
      return { files: [file], downloading: true };
    }
  });
  render(<RouteNotice id="a" wait="connection_deleted" />);
  expect(await screen.findByText("이 회의에 정한 연결이 삭제됐습니다. 다른 연결을 골라 주세요")).toBeTruthy();
  cleanup();

  render(<RouteNotice id="a" wait="summary_model" />);
  expect(await screen.findByText("요약 모델 받는 중 20% · 다 받으면 자동으로 요약합니다")).toBeTruthy();
  expect(screen.queryByRole("switch")).toBeNull();
});

// Plan 5.3: counted on screen between events, from when the engine began starting.
test("요약 엔진 준비 중 counts its seconds", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-19T05:00:10Z"));
  render(<EngineStarting since={Date.parse("2026-10-19T05:00:00Z") / 1000} />);
  expect(screen.getByText("요약 엔진 준비 중 · 10초")).toBeTruthy();
  await act(() => vi.advanceTimersByTimeAsync(2_000));
  expect(screen.getByText("요약 엔진 준비 중 · 12초")).toBeTruthy();
});

test("the switch sets 로컬로 요약 and shows the route it returns", async () => {
  const calls: { cmd: string; args: unknown }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "summary_route") return route({ local_available: true });
    if (cmd === "set_summarize_local") return route({ connection: ollama, local: true, local_available: true });
  });
  render(<RouteNotice id="a" />);

  fireEvent.click(await screen.findByRole("switch", { name: "로컬로 요약" }));

  expect(await screen.findByText(/^이 PC의 Ollama/)).toBeTruthy();
  expect(calls.find((c) => c.cmd === "set_summarize_local")?.args).toEqual({ id: "a", local: true });
  expect(screen.getByRole("switch", { name: "로컬로 요약" }).getAttribute("aria-checked")).toBe("true");
});

test("a refused switch keeps the route and says why, until a read works", async () => {
  mockIPC((cmd) => {
    if (cmd === "summary_route") return route({ local_available: true });
    if (cmd === "set_summarize_local") throw new Error("요약이 이미 시작돼 바꿀 수 없습니다");
  });
  const { rerender } = render(<RouteNotice id="a" status="recorded" />);

  fireEvent.click(await screen.findByRole("switch", { name: "로컬로 요약" }));

  expect(await screen.findByText(/요약이 이미 시작돼 바꿀 수 없습니다/)).toBeTruthy();
  expect(screen.getByRole("switch", { name: "로컬로 요약" }).getAttribute("aria-checked")).toBe("false");
  rerender(<RouteNotice id="a" status="transcribing" />);
  await waitFor(() => expect(screen.queryByText(/요약이 이미 시작돼 바꿀 수 없습니다/)).toBeNull());
});

// A read sent before the switch must not undo it when its reply comes last.
test("only the newest reply sets the route", async () => {
  let reply: (r: SummaryRoute) => void = () => {};
  let reads = 0;
  mockIPC((cmd) => {
    if (cmd === "summary_route") {
      if (++reads === 1) return route({ local_available: true });
      return new Promise((resolve) => (reply = resolve));
    }
    if (cmd === "set_summarize_local") return route({ connection: ollama, local: true, local_available: true });
  });
  const { rerender } = render(<RouteNotice id="a" status="recorded" />);
  const toggle = await screen.findByRole("switch", { name: "로컬로 요약" });

  rerender(<RouteNotice id="a" status="transcribing" />);
  await waitFor(() => expect(reads).toBe(2));
  fireEvent.click(toggle);
  expect(await screen.findByText(/^이 PC의 Ollama/)).toBeTruthy();

  await act(async () => reply(route({ local_available: true })));
  expect(screen.getByText(/^이 PC의 Ollama/)).toBeTruthy();
  expect(toggle.getAttribute("aria-checked")).toBe("true");
});

// SUM-07 while the transcript is being sent; the review screen says it afterwards.
test("once locked: where it is being summarized while summarizing, never the switch, then nothing", async () => {
  let current = route({ local_available: true });
  mockIPC((cmd) => (cmd === "summary_route" ? current : undefined));
  const { container, rerender } = render(<RouteNotice id="a" status="transcribing" />);
  expect(await screen.findByRole("switch", { name: "로컬로 요약" })).toBeTruthy();

  current = route({ local_available: true, locked: true });
  rerender(<RouteNotice id="a" status="summarizing" />);
  expect(
    await screen.findByText("전사본을 회사 클라우드(api.example.com)로 보내 요약하고 있습니다"),
  ).toBeTruthy();
  expect(screen.queryByRole("switch")).toBeNull();

  current = route({ connection: ollama, local: true, local_available: true, locked: true });
  rerender(<RouteNotice id="b" status="summarizing" />);
  expect(await screen.findByText("이 PC의 Ollama로 요약하고 있습니다")).toBeTruthy();
  expect(screen.queryByRole("switch")).toBeNull();

  current = route({ connection: thisPc, local_available: true, locked: true });
  rerender(<RouteNotice id="c" status="summarizing" />);
  expect(await screen.findByText("이 PC에서 요약하고 있습니다")).toBeTruthy();
  expect(container.textContent).not.toContain("builtin");

  rerender(<RouteNotice id="b" status="summarize_failed" />);
  await waitFor(() => expect(container.textContent).toBe(""));
});

// Between transcribing and summarizing the list can read `transcribed` while the last event still
// says transcribing: the dock must not drop the meeting for that moment (APP-07).
test("phase: an event saying it runs wins over the list's status", () => {
  const meta = { schema_version: 1, status: "transcribed" } as Meta;
  const event = (patch: Partial<Progress>): Progress => ({
    id: "a",
    status: "transcribing",
    chunk: 4,
    chunks: 5,
    percent: 98,
    eta_secs: null,
    position: null,
    ...patch,
  });
  // No place in line and no event: it stopped, waiting for its route.
  expect(phase(meta, undefined)).toBeNull();
  // A paused one waiting for its route is out of line too (plan 5.4); without a reason it waits.
  const paused = { ...meta, status: "paused" } as Meta;
  expect(phase({ ...paused, wait: "connection_deleted" }, undefined)).toBeNull();
  expect(phase(paused, undefined)).toBe("waiting");
  expect(phase({ ...paused, wait: "summary_model" }, event({ status: "paused", position: 2 }))).toBe("waiting");
  expect(phase(meta, event({}))).toBe("running");
  expect(phase(meta, event({ status: "summarizing", chunk: 0, chunks: 0, percent: 0 }))).toBe("running");
  // Its last event is the end: nothing runs.
  expect(phase(meta, event({ status: "transcribed" }))).toBeNull();
  expect(phase(meta, event({ status: "transcribed", position: 1 }))).toBe("waiting");
  // A newer app's meeting never runs here.
  expect(phase({ ...meta, schema_version: 2 }, event({}))).toBeNull();
});
