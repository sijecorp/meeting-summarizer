import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Mirrors src-tauri/src/store.rs, settings.rs, commands.rs, audio/mod.rs, queue.rs, summarize.rs,
// slack.rs and transcribe/model.rs.

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
  /** The connection chosen when summarizing started (or by a retry); null before. */
  llm_connection: string | null;
  summarize_local: boolean;
  /** SUM-10: too little speech, so no LLM was asked: nothing to send. */
  no_speech: boolean;
  error: string | null;
  sent: { channel: string; at: string; edit_count: number } | null;
  /** The grounding check (plan 5.6) could read the action items. Absent: no summary checked. */
  grounding_checked?: boolean;
  /** Why a meeting with its transcript waits in `transcribed` or `paused` instead of summarizing
   * (plan 5.4). Absent once its summary starts or its connection is changed. */
  wait?: Wait;
};

/** `summary_model`: the built-in route needs the summary model (`modelStatus("summary")` tells
 * whether it is downloading). `connection_deleted`: the connection it was pinned to is gone; it is
 * never sent elsewhere unasked (`retrySummary` picks another). */
export type Wait = "summary_model" | "connection_deleted";

/** A listed meeting. `interrupted`: left in `recording` by a crash, offered as 처리 계속 / 삭제 (STO-03). */
export type MeetingItem = Meta & { interrupted: boolean };

/** `summary`: the body (`summary.md` if edited, else the generated one without its title line).
 * `sending`: a send of it is out, maybe from a review screen since closed. */
export type MeetingDetail = { meta: Meta; summary: string; sending: boolean };

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
  /** The summary engine's GPU failed and its CPU worked: automatic mode summarizes on the CPU
   * (plan 5.3). Rust keeps it across saves. */
  summary_gpu_failed?: boolean;
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

/** `rename`: saved connections whose name the built-in route reserves (`builtin`, "이 PC"): out of
 * every route, and `saveSettings` refuses them until they are renamed. */
export type LoadedSettings = { settings: Settings; read_only: boolean; rename: string[] };

/** The built-in route's connection name, in `Meta.llm_connection` and `listConnections`. */
export const BUILTIN = "builtin";
/** A connection's name as the screens show it: "이 PC" for the built-in route, never `builtin`. */
export const connectionLabel = (name: string) => (name === BUILTIN ? "이 PC" : name);
/** `settings::reserved` (d6): names only the built-in route may have. */
export const reserved = (name: string) => [BUILTIN, connectionLabel(BUILTIN)].includes(name.trim());

/** A connection a screen may offer (plan 5.4): the saved ones, then the built-in route, labelled
 * "이 PC". `host`: where it sends a transcript (SUM-07), empty for the built-in route. No secret. */
export type ConnectionItem = { name: string; label: string; host: string; is_local: boolean; builtin: boolean };
/** `getSettings` never holds the built-in route; screens that pick a connection read this. */
export const listConnections = () => invoke<ConnectionItem[]>("list_connections");

export type SecretKind = "llm" | "slack";

export const getSettings = () => invoke<LoadedSettings>("get_settings");
/** Resolves to the settings as stored: the local default is off while nothing local can summarize
 * (SUM-08), and the fields Rust owns (`gpu_failed`, `last_channel`) are its own. */
export const saveSettings = (value: Settings) => invoke<Settings>("save_settings", { value });
/** Backs up an unreadable settings.json and returns the defaults. */
export const resetSettings = () => invoke<LoadedSettings>("reset_settings");

export const setSecret = (kind: SecretKind, name: string, value: string) =>
  invoke<void>("set_secret", { kind, name, value });
export const hasSecret = (kind: SecretKind, name: string) =>
  invoke<boolean>("has_secret", { kind, name });
export const deleteSecret = (kind: SecretKind, name: string) =>
  invoke<void>("delete_secret", { kind, name });

/** `id` is cpal's `DeviceId` string, the form `input_device` and `output_device` pin. */
export type AudioDevice = { id: string; name: string };
export type AudioDevices = { inputs: AudioDevice[]; outputs: AudioDevice[] };
export const listAudioDevices = () => invoke<AudioDevices>("list_audio_devices");

// The audio tab's checks (ONB-03, architecture 4.4). They change no setting, run one at a time,
// and fail while recording (also one that starts meanwhile). `device` is what the tab shows, saved
// or not.

/** Plays a 1 kHz tone for a second and listens to the system audio meanwhile; true when heard
 * (above -40 dBFS). `device` is the output device, Windows only (macOS hears every output). */
export const testSystemAudio = (device: string | null) => invoke<boolean>("test_system_audio", { device });
/** Listens to the microphone for 5 s, sending `audio-test://level`. True when sound came in. */
export const testMicrophone = (device: string | null) => invoke<boolean>("test_microphone", { device });
/** 25 a second during `testMicrophone`: 0..100, -60..0 dBFS like the recording meter. */
export const onMicTestLevel = (handler: (level: number) => void) =>
  listen<number>("audio-test://level", (e) => handler(e.payload));

export const listMeetings = () => invoke<MeetingItem[]>("list_meetings");
export const getMeeting = (id: string) => invoke<MeetingDetail>("get_meeting", { id });
/** Times are from the start of the recording (architecture 5.2). */
export type Segment = { start_ms: number; end_ms: number; text: string };
/** `transcript.json` of a transcribed meeting. Plain text: never render it as HTML (10.4). */
export const getTranscript = (id: string) => invoke<Segment[]>("get_transcript", { id });
export const deleteMeeting = (id: string) => invoke<void>("delete_meeting", { id });
/** "처리 계속" for an interrupted recording, or a retry after `transcribe_failed`: queues it. */
export const resumeMeeting = (id: string) => invoke<Meta>("resume_meeting", { id });

// Summaries and review (architecture 6, 10.2).

/** Where a meeting's summary goes (SUM-07) and whether "로컬로 요약" can still change (SUM-08). */
export type SummaryRoute = {
  /** null without connections, when the meeting's choice cannot be met, or when nothing was sent
   * (`no_speech`, or reviewed without a connection). No secret in it. */
  connection: LlmConnection | null;
  /** The meeting's "로컬로 요약". */
  local: boolean;
  /** Some connection is local, so the toggle is offered. */
  local_available: boolean;
  /** Summarizing has started: the route no longer changes. */
  locked: boolean;
  /** Flipping "로컬로 요약" changes where the summary goes or whether it waits: the toggle is
   * offered only then. */
  toggle_matters: boolean;
};
export const summaryRoute = (id: string) => invoke<SummaryRoute>("summary_route", { id });
/** Fails once the route is locked, or when asked for local without a local connection. */
export const setSummarizeLocal = (id: string, local: boolean) =>
  invoke<SummaryRoute>("set_summarize_local", { id, local });
/** REV-01, in `review` only. The title is stored as one line. `summary.generated.md` stays as the
 * grounding check left it. */
export const saveSummary = (id: string, title: string, body: string) =>
  invoke<Meta>("save_summary", { id, title, body });
/** REV-03 "전송 안 함": `review` → `archived`. */
export const archiveMeeting = (id: string) => invoke<Meta>("archive_meeting", { id });
/** SUM-06 "요약 다시 실행" after `summarize_failed`, or another connection for a meeting waiting
 * with its transcript (`transcribed`, `paused`), picked by `listConnections` name: queues it. */
export const retrySummary = (id: string, connection: string) =>
  invoke<Meta>("retry_summary", { id, connection });
/** REV-05: summary and transcript as one `.md` file, through a save dialog. The saved path, or null
 * when cancelled. */
export const exportMarkdown = (id: string) => invoke<string | null>("export_markdown", { id });

// Slack (architecture 7, 10.2).

/** REV-02: Slack's limit for the message block. */
export const SLACK_MAX = 12_000;
/** REV-02: the characters Slack gets for this title and body, header included and footer not.
 * `offsetMinutes`: the local UTC offset at the meeting's start; Rust cannot read it safely. */
export const slackLength = (id: string, title: string, body: string, offsetMinutes: number) =>
  invoke<number>("slack_length", { id, title, body, offsetMinutes });
/** SND-01..06, on the send button only: saves the title and body, then posts them to the webhook
 * named `webhook`. Resolves `sent`; on failure the meeting stays in `review` with the cause in
 * `error`. Never retried by itself: a resend could post twice. */
export const sendToSlack = (
  id: string,
  webhook: string,
  title: string,
  body: string,
  offsetMinutes: number,
) => invoke<Meta>("send_to_slack", { id, webhook, title, body, offsetMinutes });
/** ONB-06: a fixed message to the webhook `webhook`. `url`: one typed but not saved yet, sent
 * instead of the keychain's. Rejects with the same messages as `sendToSlack`. */
export const sendTestMessage = (webhook: string, url: string | null) =>
  invoke<void>("send_test_message", { webhook, url });
/** A send of this meeting ended, sent or not, whichever screen started it. */
export const onSlackFinished = (handler: (id: string) => void) =>
  listen<string>("slack://finished", (e) => handler(e.payload));

// Recording (architecture 4, 10.2).

/** A newer warning of the same kind replaces the older one. */
export type Warning = { kind: string; message: string };
/** The recording in progress and the warnings still standing. */
export type RecordingState = { id: string; warnings: Warning[] };
/** Every 40 ms. `mic` and `system` are RMS, 0..1. */
export type Level = {
  elapsed_ms: number;
  mic: number;
  system: number;
  /** The last 64 ms in 24 log-spaced bands, 80 Hz–8 kHz: -80..-20 dBFS as 0..100. */
  mic_bands: number[];
  system_bands: number[];
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
/** Opens the folder of `app.log` in the file manager. Rust picks the path. */
export const openLogDir = () => invoke<void>("open_log_dir");
/** Quit was requested while work is running; the UI must ask before `confirmQuit`. */
export const onQuitRequested = (handler: () => void) =>
  listen("app://quit-requested", handler);
/** APP-03: an OS notification about this meeting was clicked; the window is already shown. */
export const onOpenMeeting = (handler: (id: string) => void) =>
  listen<string>("app://open-meeting", (e) => handler(e.payload));

// Processing queue and models (architecture 5, 6, 10.2).

/**
 * `meeting://progress`. Sent when a meeting joins the line, its place changes, it starts, a chunk
 * or LLM request moves, and when it ends (with its final status: `review`, `transcribed` when there
 * is no LLM connection, `transcribe_failed`, `summarize_failed`, `paused`).
 */
export type Progress = {
  id: string;
  status: Status;
  /** Chunks finished, of `chunks`. `chunks` is 0 until the chunk plan exists. While `summarizing`:
   * LLM requests done, of the ones known so far (a long meeting adds some). */
  chunk: number;
  chunks: number;
  /** While `chunks` is 0, the chunk planning's progress; then 0..100 over the whole recording.
   * While `summarizing`, from `chunk` / `chunks`. */
  percent: number;
  /** Remaining seconds, once this run has done enough to tell. Always null while `summarizing`. */
  eta_secs: number | null;
  /** 1-based place in line while waiting; null while running. */
  position: number | null;
  /** Unix seconds since the built-in engine began starting for this summary, until it is ready or
   * fails (plan 5.3): "요약 엔진 준비 중 · N초" counts from it. */
  engine_since?: number | null;
};
/** Queued and running meetings, for the first render before progress events arrive. */
export const queueState = () => invoke<Progress[]>("queue_state");
export const onMeetingProgress = (handler: (progress: Progress) => void) =>
  listen<Progress>("meeting://progress", (e) => handler(e.payload));
/** The GPU crashed twice on one chunk; automatic mode now uses the CPU. */
export const onGpuFailed = (handler: (notice: { message: string }) => void) =>
  listen<{ message: string }>("transcribe://gpu-failed", (e) => handler(e.payload));

/** What the model commands act on (DX12). `all`, the default: the chosen Whisper model and the VAD
 * model, then the summary model while the built-in LLM is the default (`default_llm` null).
 * `transcription`: the Whisper and VAD models only. `summary`: the summary model only. */
export type ModelScope = "all" | "transcription" | "summary";
/** `received`: bytes in the `.part` file kept for resuming. `downloading`: the running download
 * still has this file to fetch, now or later. */
export type ModelFileState = {
  file: string;
  size: number;
  installed: boolean;
  received: number;
  downloading: boolean;
};
/** `downloading`: some download or import runs. */
export type ModelState = { files: ModelFileState[]; downloading: boolean };
/** `model://download`, about once per percent of `file`. `all_*`: every file of the download
 * together, the ones already in place counted whole. */
export type ModelDownload = {
  file: string;
  received: number;
  total: number;
  all_received: number;
  all_total: number;
};
export const modelStatus = (scope?: ModelScope) => invoke<ModelState>("model_status", { scope });

// One download for the whole app: it outlives the screen that started it (874 MB takes a while),
// so Home's setup card and the settings' 전사 tab both follow the same one.
let download: Promise<void> | null = null;
// The scope of the running download, or the last one: its failure belongs to the tab of that scope.
let downloadScope: ModelScope = "all";
let cancelled = false;
let failure = "";
// A cancel answered `stopped` for this download: later answers cannot take it back.
let stopped = false;

/**
 * ONB-01: starts `download_models` for `asked`, or joins the one running, whatever its scope.
 * Settles when every file has been tried (one failing does not stop the next), or when it is
 * cancelled. Never rejects: the reasons stay in `modelDownloadFailure()` until the next download
 * or a successful import, so a screen opened later still shows them. A cancel leaves no reason.
 */
export const startModelDownload = (asked?: ModelScope) => {
  if (!download) {
    downloadScope = asked ?? "all";
    failure = "";
    // A cancel clicked as the last download settled must not hide this one's failure.
    cancelled = stopped = false;
    download = invoke<void>("download_models", { scope: asked })
      .catch((e) => {
        if (!cancelled) failure = String(e);
      })
      .finally(() => {
        download = null;
        cancelled = stopped = false;
      });
  }
  return download;
};
/** The download running, if any. */
export const modelDownload = () => download;
/** What the running download, or the last one, was started for. */
export const modelDownloadScope = () => downloadScope;
export const modelDownloadFailure = () => failure;
/** What a cancel did: `stopped` the download and its chain, `dropped` the scope's files still
 * ahead from the chain (the file in flight goes on), or null with nothing of the scope to cancel. */
export type ModelCancel = "stopped" | "dropped" | null;
/** Stops the download and the rest of its chain; what arrived stays for resuming. A narrower scope
 * cancels only its files (`ModelCancel`). */
export const cancelModelDownload = (scope?: ModelScope) => {
  // Set first: the download can reject before this answer arrives. Kept only once one answer
  // said stopped: after a drop or nothing, the download still runs and may still fail.
  cancelled = true;
  return invoke<ModelCancel>("cancel_model_download", { scope }).then(
    (done) => {
      stopped ||= done === "stopped";
      cancelled = stopped;
      return done;
    },
    (e) => {
      cancelled = stopped;
      throw e;
    },
  );
};
/** The import picker's file type (ONB-02): `.bin` for transcription, `.gguf` for the summary model. */
export const modelFileFilter = (scope?: ModelScope) => ({
  name: "모델 파일",
  extensions: scope === "summary" ? ["gguf"] : ["bin"],
});
/** Installs a picked model file of `scope` if its size and sha256 match (ONB-02). */
export const importModel = (path: string, scope?: ModelScope) =>
  invoke<ModelState>("import_model", { path, scope }).then((state) => {
    failure = "";
    return state;
  });
export const onModelDownload = (handler: (progress: ModelDownload) => void) =>
  listen<ModelDownload>("model://download", (e) => handler(e.payload));
