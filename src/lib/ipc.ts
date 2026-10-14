import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Mirrors src-tauri/src/store.rs, settings.rs, commands.rs and audio/mod.rs.

export const SCHEMA_VERSION = 1;
export const NEWER_VERSION = "새 버전 앱에서 만든 데이터입니다";

export type Status =
  | "recording"
  | "recorded"
  | "transcribing"
  | "transcribed"
  | "summarizing"
  | "review"
  | "sent"
  | "archived"
  | "paused"
  | "transcribe_failed"
  | "summarize_failed"
  | "deleting"
  /** A status from a newer app version; such meetings are read-only. */
  | "unknown";

export const STATUS_LABEL: Record<Status, string> = {
  recording: "녹음 중",
  recorded: "녹음 완료",
  transcribing: "전사 중",
  transcribed: "전사 완료",
  summarizing: "요약 중",
  review: "검토 대기",
  sent: "전송 완료",
  archived: "전송 안 함",
  paused: "일시정지",
  transcribe_failed: "전사 실패",
  summarize_failed: "요약 실패",
  deleting: "삭제 중",
  unknown: "알 수 없음",
};

export type Meta = {
  schema_version: number;
  id: string;
  title: string;
  status: Status;
  created_at: string;
  duration_ms: number;
  gaps: { at_ms: number; len_ms: number; reason: string }[];
  backend: string | null;
  chunks_done: number;
  llm_connection: string | null;
  summarize_local: boolean;
  error: string | null;
  sent: { channel: string; at: string; edit_count: number } | null;
};

/** A listed meeting. `interrupted`: left in `recording` by a crash, offered as 처리 계속 / 삭제 (STO-03). */
export type MeetingItem = Meta & { interrupted: boolean };

export type MeetingDetail = { meta: Meta; summary: string };

export type Backend = "auto" | "gpu" | "cpu";

export type LlmConnection = {
  name: string;
  base_url: string;
  model: string;
  context_tokens: number;
  is_local: boolean;
  timeout_secs: number;
};

export type Webhook = { name: string };

export type Settings = {
  schema_version: number;
  recorder_name: string;
  backend: Backend;
  gpu_failed: boolean;
  model: string;
  model_base_url: string;
  input_device: string | null;
  output_device: string | null;
  llm_connections: LlmConnection[];
  default_llm: string | null;
  summarize_local_by_default: boolean;
  webhooks: Webhook[];
  last_channel: string | null;
  glossary: string[];
};

export type LoadedSettings = { settings: Settings; read_only: boolean };

export type SecretKind = "llm" | "slack";

export const getSettings = () => invoke<LoadedSettings>("get_settings");
export const saveSettings = (value: Settings) => invoke<void>("save_settings", { value });
/** Backs up an unreadable settings.json and returns the defaults. */
export const resetSettings = () => invoke<LoadedSettings>("reset_settings");

export const setSecret = (kind: SecretKind, name: string, value: string) =>
  invoke<void>("set_secret", { kind, name, value });
export const hasSecret = (kind: SecretKind, name: string) =>
  invoke<boolean>("has_secret", { kind, name });
export const deleteSecret = (kind: SecretKind, name: string) =>
  invoke<void>("delete_secret", { kind, name });

export const listMeetings = () => invoke<MeetingItem[]>("list_meetings");
export const getMeeting = (id: string) => invoke<MeetingDetail>("get_meeting", { id });
export const deleteMeeting = (id: string) => invoke<void>("delete_meeting", { id });
/** "처리 계속" for an interrupted recording. */
export const resumeMeeting = (id: string) => invoke<Meta>("resume_meeting", { id });

// Recording (architecture 4, 10.2).

/** A newer warning of the same kind replaces the older one. */
export type Warning = { kind: string; message: string };
/** The recording in progress and the warnings still standing. */
export type RecordingState = { id: string; warnings: Warning[] };
/** Every 100 ms. `mic` and `system` are RMS, 0..1. */
export type Level = {
  elapsed_ms: number;
  mic: number;
  system: number;
  /** A device has been opening for over a second: likely the first-run permission prompt. */
  waiting_permission: boolean;
};
/** `message` says why when the recording ended by itself (disk, 4 hours, sleep). */
export type Stopped = { id: string; message: string | null };

export const startRecording = () => invoke<RecordingState>("start_recording");
/** Resolves once both WAV files are closed; returns the meeting id. */
export const stopRecording = () => invoke<string | null>("stop_recording");
export const recordingState = () => invoke<RecordingState | null>("recording_state");
export const onRecordingLevel = (handler: (level: Level) => void) =>
  listen<Level>("recording://level", (e) => handler(e.payload));
export const onRecordingWarning = (handler: (warning: Warning) => void) =>
  listen<Warning>("recording://warning", (e) => handler(e.payload));
export const onRecordingStopped = (handler: (stopped: Stopped) => void) =>
  listen<Stopped>("recording://stopped", (e) => handler(e.payload));

export const confirmQuit = () => invoke<void>("confirm_quit");
/** Windows Ctrl+Q: same exit path as the tray "종료" (architecture 8.5). */
export const requestQuit = () => invoke<void>("request_quit");
/** Quit was requested while work is running; the UI must ask before `confirmQuit`. */
export const onQuitRequested = (handler: () => void) =>
  listen("app://quit-requested", handler);
