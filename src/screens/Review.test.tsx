import { cleanup, render, screen } from "@testing-library/react";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, expect, test } from "vitest";
import type { MeetingDetail } from "../lib/ipc";
import Review from "./Review";

afterEach(() => {
  cleanup();
  clearMocks();
});

// D6 / NFR-07: LLM output must stay plain text, never become markup.
test("renders HTML in a summary as text", async () => {
  const summary = "# 주간 회의\n<img src=x onerror=alert(1)>\n- 결정: 미정";
  const detail: MeetingDetail = {
    meta: {
      schema_version: 1,
      id: "2026-10-13T0500-a1b2",
      title: "<b>주간</b> 회의",
      status: "review",
      created_at: "2026-10-13T05:00:03Z",
      duration_ms: 0,
      gaps: [],
      backend: null,
      chunks_done: 0,
      llm_connection: null,
      summarize_local: false,
      no_speech: false,
      error: null,
      sent: null,
    },
    summary,
  };
  mockIPC((cmd) => (cmd === "get_meeting" ? detail : undefined));

  const { container } = render(<Review id={detail.meta.id} onHome={() => {}} />);

  const textarea = (await screen.findByDisplayValue(summary, { normalizer: (s) => s })) as HTMLTextAreaElement;
  expect(textarea.tagName).toBe("TEXTAREA");
  expect(textarea.value).toBe(summary);
  expect(screen.getByDisplayValue("<b>주간</b> 회의")).toBeTruthy();
  expect(container.querySelector("img")).toBeNull();
  expect(container.querySelector("b")).toBeNull();
});
