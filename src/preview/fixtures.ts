import type { InvokeArgs } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type * as ipc from "@/lib/ipc";
import {
  BUILTIN,
  connectionLabel,
  reserved,
  SCHEMA_VERSION,
  type AudioDevices,
  type ConnectionItem,
  type Level,
  type LlmConnection,
  type MeetingItem,
  type Meta,
  type ModelFileState,
  type ModelState,
  type Progress,
  type Segment,
  type Settings,
  type SlowReason,
  type Status,
  type SummaryRoute,
  type Wait,
} from "@/lib/ipc";

// The dev preview's backend (design-system 7): every command `ipc.ts` calls, over fixtures typed
// with the real IPC shapes, plus the events the Rust side sends. Never in the build.

export const SCENARIOS = {
  empty: "첫 화면",
  many: "여러 상태",
  recording: "녹음 중",
  setup: "설정 없음",
  // Plan 5.2-5.4 (DX6): this PC as the default, its summary model in each of three states.
  summaryDownloading: "요약 모델 받는 중",
  summaryMissing: "요약 모델 없음",
  summaryReady: "이 PC 요약",
  // Plan 5.1-12: a meeting for each slow reason (Home's row names only saver).
  slow: "느린 처리",
  // Plan 5.3, 5.7-15: paused meetings in each step, one still stopping, and "모두 이어서 처리".
  pause: "일시정지",
  // Plan 5.3-12: resumed while recording, it waits for the recording to end.
  pauseRecording: "녹음 중 이어서 처리",
  // Plan 5.7-15 언어: every picker, the dialog's three modes, a mixed transcript, the date notice.
  languages: "전사 언어",
  // The recording screen's trigger in two languages, its first apply failing.
  languagesRecording: "녹음 중 전사 언어",
  // A mixed meeting waits for the detection model while a Korean one runs past it.
  languagesLid: "판별 모델 받는 중",
} as const;
export type Scenario = keyof typeof SCENARIOS;

type Stored = {
  meta: MeetingItem;
  summary: string;
  generated: string;
  sending: boolean;
  transcript: Segment[] | null;
};

const CLOUD: LlmConnection = {
  name: "회사 클라우드",
  base_url: "https://llm.example.co.kr/v1",
  model: "gpt-4.1",
  context_tokens: 128_000,
  is_local: false,
  timeout_secs: 300,
};
const OLLAMA: LlmConnection = {
  name: "Ollama",
  base_url: "http://localhost:11434/v1",
  model: "qwen3:14b",
  context_tokens: 32_768,
  is_local: true,
  timeout_secs: 600,
};

// A second cloud connection, so the settings list has more than one of each.
const GATEWAY: LlmConnection = {
  name: "사내 게이트웨이",
  base_url: "https://llm-gw.example.co.kr/v1",
  model: "qwen3-235b",
  context_tokens: 64_000,
  is_local: false,
  timeout_secs: 300,
};

const fullSettings = (): Settings => ({
  schema_version: 1,
  recorder_name: "홍길동",
  backend: "auto",
  gpu_failed: false,
  model: "large-v3-turbo-q8_0",
  model_base_url: "https://huggingface.co",
  input_device: null,
  output_device: null,
  llm_connections: [CLOUD, OLLAMA, GATEWAY],
  default_llm: CLOUD.name,
  summarize_local_by_default: false,
  slack_team: "SIJE",
  last_channel: "C1",
  glossary: ["Tauri", "Whisper", "파일럿"],
  transcribe_languages: ["ko"],
});

const MODEL_FILES: Record<string, { file: string; size: number }> = {
  "large-v3-turbo-q8_0": { file: "ggml-large-v3-turbo-q8_0.bin", size: 874_188_075 },
  "large-v3-turbo": { file: "ggml-large-v3-turbo.bin", size: 1_624_555_275 },
  "medium-q5_0": { file: "ggml-medium-q5_0.bin", size: 539_212_467 },
};
const VAD = { file: "ggml-silero-v6.2.0.bin", size: 885_098 };
const SUMMARY_MODEL = { file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf", size: 2_497_281_120 };
const LID = { file: "ggml-tiny.bin", size: 77_691_713 };

// lang.rs's table, in its order (plan 5.6-1).
const LANGS: ipc.Lang[] = [
  { code: "ko", name: "한국어" },
  { code: "en", name: "영어" },
  { code: "vi", name: "베트남어" },
  { code: "id", name: "인도네시아어" },
];

const DEVICES: AudioDevices = {
  inputs: [
    { id: "coreaudio:BuiltInMicrophoneDevice", name: "MacBook Pro 마이크" },
    { id: "coreaudio:AirPodsPro-input", name: "AirPods Pro" },
  ],
  outputs: [
    { id: "coreaudio:BuiltInSpeakerDevice", name: "MacBook Pro 스피커" },
    { id: "coreaudio:AirPodsPro-output", name: "AirPods Pro" },
  ],
};

const TRANSCRIPT: Segment[] = (
  [
    [0, "네, 그럼 시작하겠습니다. 오늘은 파일럿 범위랑 일정부터 보죠."],
    [6_400, "지난주에 말씀드린 대로 녹음, 전사, 요약, 슬랙 전송까지를 10월 파일럿 범위로 잡으려고 합니다."],
    [14_900, "범위는 그걸로 확정하고요, 파일럿은 개발팀 열두 명으로 시작하는 걸로 하겠습니다."],
    [23_200, "설치 안내 문서는 김지훈 님이 18일까지 정리해 주시면 될 것 같아요."],
    [31_800, "웹훅 채널 정리는 박서연 님이 맡아 주시는데, 기한은 아직 못 정했습니다."],
    [40_500, "사내 LLM 키 발급 요청은 누가 할지 정해야 하는데, 20일까지는 나가야 해요."],
    [52_300, "두 시간 넘는 회의는 구간별로 요약해서 합치는 쪽이 품질이 더 좋았습니다."],
    [61_700, "윈도우 노트북에서 GPU 전사가 멈춘 경우가 두 번 있었어요. 그건 CPU로 넘어가게 해 뒀고요."],
    [72_000, "녹음 파일을 며칠 보관할지는 다음 주에 다시 이야기하죠."],
  ] as const
).map(([start_ms, text]) => ({ start_ms, end_ms: start_ms + 6_000, text }));

// A Korean and Vietnamese meeting (plan 5.6-28): each line's `lang`, switching a few times.
const MIXED: Segment[] = (
  [
    [0, "ko", "그럼 시작할게요. 오늘은 호치민 공장 라인 증설 일정부터 보겠습니다."],
    [7_200, "ko", "투안 씨, 지난주에 말씀하신 설비 반입 날짜 한 번 더 확인 부탁드려요."],
    [14_800, "vi", "Dạ, thiết bị sẽ được giao vào ngày 15 tháng 11, sớm hơn kế hoạch ba ngày."],
    [22_400, "vi", "Nhưng đội lắp đặt chỉ có thể bắt đầu từ ngày 18 vì còn phải chờ giấy phép."],
    [30_100, "ko", "그러면 시운전은 원래대로 25일에 하는 걸로 두죠."],
    [37_600, "ko", "현지 인력은 열 명 더 뽑는 거 맞죠?"],
    [44_900, "vi", "Vâng, chúng tôi đã phỏng vấn mười hai người và chọn được mười người."],
    [52_300, "vi", "Tuần sau họ sẽ bắt đầu đào tạo an toàn."],
    [59_800, "ko", "좋습니다. 안전 교육 자료는 본사에서 베트남어로 번역해서 보내 드릴게요."],
    [68_000, "vi", "Cảm ơn anh. Em sẽ gửi danh sách nhân viên mới qua email trong hôm nay."],
    [75_400, "ko", "네, 그럼 오늘은 여기까지 하겠습니다."],
  ] as const
).map(([start_ms, lang, text]) => ({ start_ms, end_ms: start_ms + 6_000, text, lang }));

// Its summary has no "미정": the date notice stands alone (plan 5.6-18).
const MIXED_SUMMARY = `## 결정사항
- 호치민 공장 설비는 11월 15일에 들어오고, 설치는 허가가 나오는 18일부터 시작한다
- 시운전은 예정대로 11월 25일에 한다
- 현지 인력 10명을 새로 뽑았고, 다음 주부터 안전 교육을 시작한다
## 액션 아이템
- [본사 교육팀] 안전 교육 자료 베트남어 번역본 발송 (11월 14일)
- [투안] 신규 인력 명단 이메일로 공유 (11월 3일)
## 주요 논의
- 설비 반입은 계획보다 사흘 빨라졌지만 설치는 허가 때문에 18일까지 기다려야 한다
## 미해결 질문
- 설치 허가가 18일보다 늦어지면 시운전 날짜를 어떻게 할지 정하지 못했다`;

// The prompt's format (architecture 6.2), without the `# 제목` line that became `meta.title`.
const SUMMARY = `## 결정사항
- 10월 파일럿 범위는 녹음, 전사, 요약, Slack 전송까지로 한다
- 파일럿은 개발팀 12명으로 시작한다
## 액션 아이템
- [김지훈] 설치 안내 문서 작성 (10월 18일)
- [박서연] 파일럿 채널 웹훅 정리 (미정)
- [미정] 사내 LLM 연결 키 발급 요청 (10월 20일)
## 주요 논의
- 두 시간이 넘는 회의는 구간별로 요약한 뒤 합치는 편이 품질이 낫다
- Windows 노트북에서 GPU 전사가 멈춘 사례가 두 건 있었고, CPU로 넘어가게 해 두었다
## 미해결 질문
- 녹음 파일을 며칠 동안 보관할지 정하지 못했다`;

// A long review: about 95% of Slack's 12,000, so the length ring shows its warning; a little more
// typing takes it past the limit.
const TOPICS = ["온보딩 흐름", "녹음 권한 안내", "전사 속도", "요약 품질", "Slack 메시지 형식", "설정 화면", "오류 문구"];
const POINTS = [
  "사용자 인터뷰에서 가장 자주 나온 불편이었다",
  "다음 스프린트에서 먼저 손보기로 했다",
  "지표를 먼저 모은 뒤에 판단하기로 했다",
  "시안 두 가지를 나란히 놓고 비교해 보기로 했다",
  "지금 방식을 유지하되 문구만 다듬기로 했다",
];
const LONG_SUMMARY = (() => {
  const lines = SUMMARY.split("\n");
  const at = lines.indexOf("## 미해결 질문");
  const more: string[] = [];
  for (let i = 0; [...[...lines, ...more].join("\n")].length < 11_300; i++) {
    more.push(`- ${TOPICS[i % TOPICS.length]}: ${POINTS[i % POINTS.length]} (${Math.floor(i / 7) + 1}차 논의)`);
  }
  return [...lines.slice(0, at), ...more, ...lines.slice(at)].join("\n");
})();

const pad = (n: number) => String(n).padStart(2, "0");
/** Today minus `daysAgo`, at hh:mm local time. */
const at = (daysAgo: number, hour: number, minute: number) => {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  d.setHours(hour, minute, 0, 0);
  return d;
};
let serial = 0;
/** store.rs's id: the start in UTC and four hex digits. */
const idOf = (d: Date) =>
  `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}T${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}-${(0xa1b2 + ++serial).toString(16)}`;

/** Titles come from the summary's `# 제목`: until then a meeting has none (store.rs `create("")`). */
function stored(
  status: Status,
  when: Date,
  patch: Partial<MeetingItem> = {},
  extra: Partial<Stored> = {},
): Stored {
  const meta: MeetingItem = {
    schema_version: SCHEMA_VERSION,
    id: idOf(when),
    title: "",
    status,
    created_at: when.toISOString(),
    duration_ms: 42 * 60_000,
    gaps: [],
    backend: null,
    chunks_done: 0,
    llm_connection: null,
    summarize_local: false,
    no_speech: false,
    error: null,
    sent: null,
    languages: ["ko"],
    interrupted: false,
    ...patch,
  };
  return { meta, summary: "", generated: "", sending: false, transcript: null, ...extra };
}
const summarized = (title: string, status: Status, when: Date, patch: Partial<MeetingItem> = {}) =>
  stored(
    status,
    when,
    { title, backend: "gpu", chunks_done: 5, llm_connection: CLOUD.name, ...patch },
    { summary: SUMMARY, generated: SUMMARY, transcript: TRANSCRIPT },
  );
const mixedSummarized = (title: string, status: Status, when: Date, patch: Partial<MeetingItem> = {}): Stored => ({
  ...summarized(title, status, when, { languages: ["ko", "vi"], ...patch }),
  summary: MIXED_SUMMARY,
  generated: MIXED_SUMMARY,
  transcript: MIXED,
});

const progressOf = (id: string, patch: Partial<Progress>): Progress => ({
  id,
  status: "recorded",
  chunk: 0,
  chunks: 0,
  percent: 0,
  eta_secs: null,
  position: null,
  ...patch,
});

// The world the handlers read and change. `reset` builds it again for a scenario.
let meetings: Stored[] = [];
/** The processing line: running entries first, then the waiting ones by `position`. */
let line: Progress[] = [];
let settings: Settings = fullSettings();
let secrets = new Set<string>();
// The sign-in waiting for its pretend browser (`slack_sign_in`).
let signingIn = () => {};
const CHANNELS: ipc.Channel[] = [
  { id: "C2", name: "개발팀", private: false },
  { id: "G1", name: "경영회의", private: true },
  { id: "C1", name: "회의록", private: false },
];
let models: ModelState = { files: [], downloading: false };
let recording: { id: string; startedAt: number } | null = null;
let timers: ReturnType<typeof setInterval>[] = [];
let downloading: { cancelled: boolean; done: () => void; chain: ModelFileState[] } | null = null;
/** An audio check is running: like check.rs, a second one is refused. */
let checking = false;
/** The meeting whose next `set_meeting_language` fails once (design-system 8.7 적용 실패). */
let flaky: string | null = null;

const find = (id: unknown) => {
  const m = meetings.find((x) => x.meta.id === id);
  if (!m) throw `회의를 찾을 수 없습니다: ${id}`;
  return m;
};
const clone = <T>(value: T): T => structuredClone(value);
const send = (event: string, payload?: unknown) => void emit(event, payload);
const RUNNING: Status[] = ["transcribing", "summarizing"];
const isRunning = (p: Progress) => RUNNING.includes(p.status);

/** Every model file, the summary model and then the detection model last; `model_status` shows a
 * scope of them. */
function modelState(
  installed: boolean,
  received = 0,
  summary = { installed, received: 0 },
  lid = { installed, received: 0 },
): ModelState {
  const whisper = MODEL_FILES[settings.model] ?? MODEL_FILES["large-v3-turbo-q8_0"];
  return {
    files: [
      { ...whisper, installed, received, downloading: false },
      { ...VAD, installed, received: 0, downloading: false },
      { ...SUMMARY_MODEL, ...summary, downloading: false },
      { ...LID, ...lid, downloading: false },
    ],
    downloading: false,
  };
}

/** Like `scoped` in commands.rs: `all` has the summary model while the built-in LLM is the default
 * (`defaultConnection`), and never the detection model (plan 5.6-14). */
function scoped(scope: unknown) {
  const [whisper, vad, summary, lid] = models.files;
  if (scope === "summary") return [summary];
  if (scope === "lid") return [lid];
  return scope === "transcription" || defaultConnection() ? [whisper, vad] : [whisper, vad, summary];
}

/** `downloading`: the running download still has the file to fetch. */
const modelStatus = (scope: unknown): ModelState =>
  clone({
    files: scoped(scope).map((f) => ({ ...f, downloading: !f.installed && !!downloading?.chain.includes(f) })),
    downloading: downloading != null,
  });

export function reset(scenario: Scenario) {
  timers.forEach(clearInterval);
  timers = [];
  // ipc.ts keeps one download for the app: let it settle, so the next screens read the new state.
  downloading?.done();
  downloading = null;
  checking = false;
  recording = null;
  flaky = null;
  serial = done = 0;
  settings = fullSettings();
  secrets = new Set([`llm:${CLOUD.name}`, `llm:${GATEWAY.name}`]);
  models = modelState(true);
  meetings = [];
  line = [];

  // The first run: no meetings and nothing set up yet, so the first screen shows its setup line.
  if (scenario === "empty") {
    settings = { ...settings, llm_connections: [], default_llm: null, slack_team: null, last_channel: null };
    secrets = new Set();
    models = modelState(false);
  }
  // 지금 할 일 (running, waiting, review, failed, interrupted) over 오늘, 어제 and older days.
  if (scenario === "many") {
    const chunked = stored("transcribing", at(0, 14, 0), {
      duration_ms: 47 * 60_000,
      backend: "gpu",
      chunks_done: 2,
    });
    const planning = stored("transcribing", at(0, 13, 10), { duration_ms: 25 * 60_000 });
    const summarizing = stored(
      "summarizing",
      at(0, 11, 0),
      { duration_ms: 62 * 60_000, backend: "gpu", chunks_done: 7, llm_connection: CLOUD.name },
      { transcript: TRANSCRIPT },
    );
    const queued = stored("recorded", at(0, 15, 20), { duration_ms: 18 * 60_000 });
    // Stopped by a quit mid-transcription; it waits for its turn again (architecture 8.5).
    const paused = stored("paused", at(0, 9, 30), {
      duration_ms: 33 * 60_000,
      backend: "gpu",
      chunks_done: 1,
    });
    // From a newer app version: read-only here.
    const newer = summarized("새 버전에서 만든 회의", "review", at(3, 15, 0));
    newer.meta.schema_version = SCHEMA_VERSION + 1;
    meetings = [
      chunked,
      planning,
      summarizing,
      queued,
      paused,
      summarized("주간 제품 회의", "review", at(0, 10, 0)),
      {
        ...summarized("분기 계획 회의", "review", at(0, 12, 30), { duration_ms: 95 * 60_000 }),
        summary: LONG_SUMMARY,
        generated: LONG_SUMMARY,
      },
      summarized("채용 인터뷰 디브리프", "sent", at(0, 8, 30), {
        duration_ms: 24 * 60_000,
        sent: { channel: "#채용", at: at(0, 9, 5).toISOString(), edit_count: 1 },
      }),
      // SUM-10: no LLM was asked, so no title and nothing sent anywhere.
      stored(
        "review",
        at(1, 17, 40),
        { duration_ms: 2 * 60_000, backend: "gpu", chunks_done: 1, no_speech: true },
        {
          transcript: [{ start_ms: 1_200, end_ms: 2_400, text: "음…" }],
        },
      ),
      summarized("디자인 리뷰", "sent", at(1, 16, 0), {
        llm_connection: OLLAMA.name,
        summarize_local: true,
        sent: { channel: "#회의록", at: at(1, 16, 52).toISOString(), edit_count: 3 },
      }),
      summarized("1:1 미팅", "archived", at(2, 11, 0), { duration_ms: 28 * 60_000 }),
      // ponytail: stopped for want of an LLM connection, though this scenario has two; the real app
      // would queue it at start (architecture 8.4). Kept so its document can be seen.
      stored("transcribed", at(1, 9, 0), { backend: "cpu", chunks_done: 3 }, { transcript: TRANSCRIPT }),
      stored("transcribe_failed", at(1, 14, 0), {
        duration_ms: 36 * 60_000,
        backend: "cpu",
        chunks_done: 2,
        error: "전사 워커가 같은 구간에서 세 번 멈췄습니다 (구간 3/5)",
      }),
      stored(
        "summarize_failed",
        at(1, 10, 30),
        {
          duration_ms: 51 * 60_000,
          backend: "gpu",
          chunks_done: 5,
          llm_connection: CLOUD.name,
          error: "LLM이 300초 안에 답하지 않았습니다",
        },
        { transcript: TRANSCRIPT },
      ),
      // STO-03: left in `recording` by a crash.
      stored("recording", at(3, 9, 0), { duration_ms: 0, interrupted: true }),
      newer,
    ];
    // ponytail: three running at once so every running state shows on one screen; queue.rs runs two.
    line = [
      progressOf(chunked.meta.id, {
        status: "transcribing",
        chunk: 2,
        chunks: 5,
        percent: 42,
        eta_secs: 180,
      }),
      progressOf(planning.meta.id, { status: "transcribing", percent: 30 }),
      progressOf(summarizing.meta.id, { status: "summarizing", chunk: 1, chunks: 3, percent: 33 }),
      progressOf(queued.meta.id, { status: "recorded", position: 1 }),
      progressOf(paused.meta.id, { status: "paused", position: 2 }),
    ];
  }
  if (scenario === "recording") {
    const started = Date.now() - (12 * 60 + 34) * 1000;
    const live = stored("recording", new Date(started), { duration_ms: 0 });
    recording = { id: live.meta.id, startedAt: started };
    meetings = [
      live,
      summarized("주간 제품 회의", "review", at(0, 10, 0)),
      summarized("채용 인터뷰 디브리프", "sent", at(0, 8, 30), {
        duration_ms: 24 * 60_000,
        sent: { channel: "#채용", at: at(0, 9, 5).toISOString(), edit_count: 1 },
      }),
      summarized("디자인 리뷰", "sent", at(1, 16, 0), {
        sent: { channel: "#회의록", at: at(1, 16, 52).toISOString(), edit_count: 0 },
      }),
      summarized("1:1 미팅", "archived", at(2, 11, 0), { duration_ms: 28 * 60_000 }),
    ];
  }
  // Home with its setup line: a meeting transcribed, waiting for the summary model (plan 5.4).
  if (scenario === "setup") {
    settings = { ...settings, llm_connections: [], default_llm: null, slack_team: null, last_channel: null };
    secrets = new Set();
    // A download stopped part way: 300 MB kept, resumed as the scenario starts (main.tsx).
    models = modelState(false, 300_000_000);
    meetings = [
      stored(
        "transcribed",
        at(0, 11, 0),
        { backend: "cpu", chunks_done: 3, wait: "summary_model" },
        { transcript: TRANSCRIPT },
      ),
      summarized("킥오프 미팅", "sent", at(3, 14, 0), {
        sent: { channel: "#회의록", at: at(3, 14, 50).toISOString(), edit_count: 0 },
      }),
    ];
  }

  // This PC summarizes by default (`default_llm` null) with a cloud connection beside it. Waiting
  // meetings show each reason (Home: 요약 대기, 연결 고르기 in 지금 할 일; the picker leaves this PC
  // out until its model is in). Without the model, a saved Ollama makes "로컬로 요약" matter. Ready:
  // a summary starts its engine (Home, the dock and its document count), the engine once failed on
  // the GPU (the 처리 장치 line says CPU), an old connection named "이 PC" asks to be renamed (d6).
  if (scenario === "summaryDownloading" || scenario === "summaryMissing" || scenario === "summaryReady") {
    const ready = scenario === "summaryReady";
    settings = {
      ...settings,
      llm_connections: scenario === "summaryMissing" ? [CLOUD, OLLAMA] : [CLOUD],
      default_llm: null,
      summary_gpu_failed: ready,
    };
    // 900 MB kept: the download (main.tsx starts it for 요약 모델 받는 중) resumes from there.
    models = modelState(true, 0, { installed: ready, received: ready ? 0 : 900_000_000 });
    const waiting = (wait: Wait, patch: Partial<MeetingItem>, when: Date) =>
      stored("transcribed", when, { backend: "gpu", chunks_done: 4, wait, ...patch }, { transcript: TRANSCRIPT });
    meetings = [
      waiting("summary_model", {}, at(0, 10, 0)),
      waiting("connection_deleted", { llm_connection: "지운 연결", duration_ms: 31 * 60_000 }, at(0, 9, 0)),
      summarized("주간 제품 회의", "sent", at(1, 16, 0), {
        llm_connection: BUILTIN,
        summarize_local: true,
        sent: { channel: "#회의록", at: at(1, 16, 52).toISOString(), edit_count: 2 },
      }),
    ];
    if (ready) {
      settings.llm_connections = [CLOUD, { ...OLLAMA, name: "이 PC" }];
      const starting = stored(
        "summarizing",
        at(0, 11, 30),
        { backend: "gpu", chunks_done: 5, llm_connection: BUILTIN, summarize_local: true },
        { transcript: TRANSCRIPT },
      );
      meetings = [starting, ...meetings.slice(1)];
      line = [progressOf(starting.meta.id, { status: "summarizing", engine_since: Math.floor(Date.now() / 1000) - 4 })];
    }
  }

  if (scenario === "slow") {
    const reasons: [SlowReason, string][] = [
      ["saver", "gpu"],
      ["cpu", "cpu"],
      ["cpu_nogpu", "cpu"],
      ["slow", "cpu"],
    ];
    meetings = reasons.map(([, backend], i) =>
      stored("transcribing", at(0, 14 - i, 0), { duration_ms: 64 * 60_000, backend, chunks_done: 2 }),
    );
    line = reasons.map(([slow], i) =>
      progressOf(meetings[i].meta.id, {
        status: "transcribing",
        chunk: 2,
        chunks: 5,
        percent: 42,
        eta_secs: 1_800,
        slow,
      }),
    );
  }

  if (scenario === "pause") {
    // Its run still stops: 멈추는 중… on Home, in the dock and in its document.
    const winding = stored("transcribing", at(0, 15, 0), {
      duration_ms: 38 * 60_000,
      backend: "gpu",
      chunks_done: 1,
      chunks_total: 4,
      held: true,
    });
    const inChunk = stored("paused", at(0, 14, 0), {
      duration_ms: 70 * 60_000,
      backend: "gpu",
      chunks_done: 2,
      chunks_total: 7,
      held: true,
    });
    const inSummary = stored(
      "paused",
      at(0, 11, 0),
      { duration_ms: 52 * 60_000, backend: "gpu", chunks_done: 5, chunks_total: 5, llm_connection: CLOUD.name, held: true },
      { transcript: TRANSCRIPT },
    );
    const unplanned = stored("paused", at(0, 10, 0), { duration_ms: 20 * 60_000, held: true });
    meetings = [winding, inChunk, inSummary, unplanned, summarized("주간 제품 회의", "review", at(1, 10, 0))];
    line = [progressOf(winding.meta.id, { status: "transcribing", chunk: 1, chunks: 4, percent: 30, eta_secs: 900 })];
  }

  if (scenario === "pauseRecording") {
    const started = Date.now() - 5 * 60_000;
    const live = stored("recording", new Date(started), { duration_ms: 0 });
    recording = { id: live.meta.id, startedAt: started };
    const resumed = stored("paused", at(0, 11, 0), {
      duration_ms: 40 * 60_000,
      backend: "gpu",
      chunks_done: 3,
      chunks_total: 6,
    });
    meetings = [live, resumed];
    line = [progressOf(resumed.meta.id, { status: "paused", position: 1 })];
  }

  // 설정 and the import dialog start from all four ("한국어 외 3개").
  if (scenario === "languages" || scenario === "languagesRecording")
    settings.transcribe_languages = LANGS.map((l) => l.code);

  // Its dialog in each mode: blocked while it runs, before any chunk (one language: the last check
  // is blocked), and E3 once transcribed. The sent one's meta line has its languages and channel.
  if (scenario === "languages") {
    const detecting = stored("transcribing", at(0, 14, 0), {
      duration_ms: 48 * 60_000,
      backend: "gpu",
      languages: ["ko", "vi"],
    });
    // "영어로 전사" on its first chunk.
    const english = stored("transcribing", at(0, 13, 0), { duration_ms: 35 * 60_000, backend: "gpu", languages: ["en"] });
    // Two run, so it keeps waiting a while.
    const waiting = stored("recorded", at(0, 15, 10), { duration_ms: 22 * 60_000 });
    meetings = [
      detecting,
      english,
      waiting,
      mixedSummarized("호치민 공장 주간 회의", "review", at(0, 10, 0), { duration_ms: 46 * 60_000 }),
      mixedSummarized("호치민 공장 주간 회의", "sent", at(7, 10, 0), {
        duration_ms: 51 * 60_000,
        sent: { channel: "#베트남법인", at: at(7, 11, 5).toISOString(), edit_count: 2 },
      }),
      summarized("주간 제품 회의", "review", at(1, 10, 0)),
    ];
    line = [
      progressOf(detecting.meta.id, { status: "transcribing", chunk: 0, chunks: 4, percent: 3, detecting: true }),
      progressOf(english.meta.id, { status: "transcribing", chunk: 0, chunks: 3, percent: 12, eta_secs: 600 }),
      progressOf(waiting.meta.id, { status: "recorded", position: 1 }),
    ];
  }

  if (scenario === "languagesRecording") {
    const started = Date.now() - 8 * 60_000;
    const live = stored("recording", new Date(started), { duration_ms: 0, languages: ["ko", "vi"] });
    recording = { id: live.meta.id, startedAt: started };
    flaky = live.meta.id;
    meetings = [live, mixedSummarized("호치민 공장 주간 회의", "review", at(1, 10, 0))];
  }

  // Without the detection model (78 MB, 15 MB kept; main.tsx starts it), the mixed meeting waits
  // in line and the Korean one behind it runs. Once the model is in, its first chunk tells the
  // languages apart.
  if (scenario === "languagesLid") {
    settings.transcribe_languages = ["ko", "vi"];
    models = modelState(true, 0, undefined, { installed: false, received: 15_000_000 });
    const mixed = stored("recorded", at(0, 14, 0), { duration_ms: 40 * 60_000, languages: ["ko", "vi"] });
    const korean = stored("recorded", at(0, 15, 0), { duration_ms: 18 * 60_000 });
    meetings = [
      mixed,
      korean,
      summarized("주간 제품 회의", "sent", at(1, 10, 0), {
        sent: { channel: "#회의록", at: at(1, 10, 50).toISOString(), edit_count: 0 },
      }),
    ];
    line = [
      progressOf(mixed.meta.id, { status: "recorded", position: 1 }),
      progressOf(korean.meta.id, { status: "recorded", position: 2 }),
    ];
  }

  timers.push(setInterval(tick, 700), setInterval(levels, 40));
}

// ----- the processing line (architecture 8.4, simplified) -----

const TITLES = ["주간 개발 회의", "고객사 정기 미팅", "분기 계획 회의"];
let done = 0;

/** Moves running meetings on, then starts waiting ones while fewer than two run. */
function tick() {
  // TRN-09: a recording holds the line.
  if (recording) return;
  for (const p of line.filter(isRunning)) advance(p);
  while (line.filter(isRunning).length < 2) {
    const next = line.find((p) => p.position != null && canStart(find(p.id)));
    if (!next) break;
    start(next);
  }
}

const lidReady = () => models.files.some((f) => f.file === LID.file && f.installed);
/** queue.rs `can_start`: a mixed meeting still to transcribe waits for the detection model, out of
 * the way of the ones behind it (plan 5.6-14). */
const canStart = (m: Stored) => !!m.transcript || m.meta.languages.length < 2 || lidReady();

function advance(p: Progress) {
  const m = find(p.id);
  // Paused: its run stops when `pause_meeting` says, and never in the scenario.
  if (m.meta.held) return;
  if (p.status === "transcribing" && p.chunks === 0) {
    // Chunk planning (`--plan-chunks`): its own percent, then the plan.
    p.percent = Math.min(100, p.percent + 4);
    if (p.percent === 100) Object.assign(p, { chunks: 5, chunk: 0, percent: 0 });
  } else if (p.status === "transcribing") {
    p.percent = Math.min(100, p.percent + 1);
    p.chunk = Math.min(p.chunks, Math.floor(p.percent / (100 / p.chunks)));
    p.eta_secs = p.percent > 3 ? (100 - p.percent) * 4 : null;
    // Plan 5.6-27: "언어를 가리는 중" through a mixed meeting's first chunk.
    if (m.meta.languages.length > 1) p.detecting = p.chunk === 0;
    m.meta.chunks_done = p.chunk;
    m.meta.backend ??= "gpu";
    if (p.percent === 100) {
      m.transcript = m.meta.languages.length > 1 ? MIXED : TRANSCRIPT;
      const to = route(m.meta);
      if (!to) {
        // No route yet: it waits here, out of line, saying why (plan 5.4).
        m.meta.status = p.status = "transcribed";
        m.meta.wait = waitOf(m.meta);
        return finish(p);
      }
      m.meta.llm_connection = to.name;
      Object.assign(p, {
        status: "summarizing",
        chunk: 0,
        chunks: 3,
        percent: 0,
        eta_secs: null,
        engine_since: engineSince(m.meta),
      });
    }
  } else {
    if (p.engine_since != null) {
      // Plan 5.3: the built-in engine loads its model first, here for 15 s; the screens count.
      if (Date.now() / 1000 - p.engine_since < 15) return;
      p.engine_since = null;
    }
    // Quick, so a summary arriving in its open document is seen within seconds.
    p.percent = Math.min(100, p.percent + 5);
    p.chunk = Math.floor((p.percent / 100) * p.chunks);
    if (p.percent === 100) {
      const summary = m.meta.languages.length > 1 ? MIXED_SUMMARY : SUMMARY;
      Object.assign(m, { summary, generated: summary });
      m.meta.title = TITLES[done++ % TITLES.length];
      m.meta.status = p.status = "review";
      return finish(p);
    }
  }
  m.meta.status = p.status;
  send("meeting://progress", clone(p));
}

/** Its last event carries the final status and no place in line. */
function finish(p: Progress) {
  line = line.filter((x) => x !== p);
  send("meeting://progress", { ...p, position: null });
}

/** A paused run's end (queue.rs `end`): `paused`, back in line last if it was resumed meanwhile. */
function stopRun(m: Stored, p: Progress) {
  m.meta.status = "paused";
  line = line.filter((x) => x !== p);
  send("meeting://progress", { ...p, status: "paused", position: null });
  if (!m.meta.held) enqueue(m);
  renumber();
}

function start(p: Progress) {
  const m = find(p.id);
  m.meta.error = null;
  Object.assign(
    p,
    m.transcript
      ? { status: "summarizing", chunk: 0, chunks: 3, percent: 0 }
      : { status: "transcribing", chunk: 0, chunks: 0, percent: 0 },
    { position: null, eta_secs: null },
  );
  if (p.status === "summarizing") {
    m.meta.llm_connection ??= route(m.meta)?.name ?? null;
    p.engine_since = engineSince(m.meta);
  }
  m.meta.status = p.status;
  renumber();
}

/** The built-in route starts its engine first (plan 5.3). */
const engineSince = (meta: Meta) => (meta.llm_connection === BUILTIN ? Math.floor(Date.now() / 1000) : null);

/** `enqueue_pending`: a meeting waiting for its route goes in line once it resolves (the summary
 * model in, settings saved, its connection picked). */
function enqueuePending() {
  for (const m of meetings) {
    if (!m.meta.wait || !route(m.meta)) continue;
    delete m.meta.wait;
    enqueue(m);
  }
}

/** Waiting entries count from 1 in order; every entry is sent again. */
function renumber() {
  line = [...line.filter(isRunning), ...line.filter((p) => !isRunning(p))];
  line.filter((p) => !isRunning(p)).forEach((p, i) => (p.position = i + 1));
  line.forEach((p) => send("meeting://progress", clone(p)));
}

function enqueue(m: Stored) {
  line.push(progressOf(m.meta.id, { status: m.meta.status, position: line.length + 1 }));
  renumber();
}

// ----- summary route (summarize.rs `route`, `SummaryRoute::of`) -----

const summaryStarted = (meta: Meta) =>
  meta.llm_connection != null ||
  !["recording", "recorded", "transcribing", "transcribed", "paused", "transcribe_failed"].includes(
    meta.status,
  );


/** The built-in route as `Settings::connections` has it (settings.rs `builtin`). */
const BUILTIN_CONNECTION: LlmConnection = {
  name: BUILTIN,
  base_url: "",
  model: SUMMARY_MODEL.file,
  context_tokens: 16_384,
  is_local: true,
  timeout_secs: 1_800,
};

/** `Settings::default_connection`: `undefined` is the built-in route. */
const defaultConnection = () =>
  settings.llm_connections.find((c) => c.name === settings.default_llm && !reserved(c.name));

/** The summary model is in: the built-in route can summarize. */
const builtinReady = () => models.files.some((f) => f.file === SUMMARY_MODEL.file && f.installed);

/** `summarize::route`: the pin; for "로컬로 요약" a local default, else this PC once its model is
 * in, else the first saved local one; else the default, this PC when none is saved. No route (a
 * pin that is gone, this PC without its model) is `null`: the meeting waits. */
function route(meta: Meta): LlmConnection | null {
  const all = [...settings.llm_connections.filter((c) => !reserved(c.name)), BUILTIN_CONNECTION];
  const usable = (c?: LlmConnection) => (c && (c.name !== BUILTIN || builtinReady()) ? c : undefined);
  if (meta.llm_connection) return usable(all.find((c) => c.name === meta.llm_connection)) ?? null;
  const fallback = defaultConnection() ?? BUILTIN_CONNECTION;
  if (meta.summarize_local)
    return (
      usable(fallback.is_local ? fallback : undefined) ??
      usable(BUILTIN_CONNECTION) ??
      all.find((c) => c.is_local && c.name !== BUILTIN) ??
      null
    );
  return usable(fallback) ?? null;
}

/** Why a meeting has no route (`summarize::route`'s `Wait`). */
const waitOf = (meta: Meta): Wait =>
  meta.llm_connection && !connections().some((c) => c.name === meta.llm_connection)
    ? "connection_deleted"
    : "summary_model";

/** `Settings::connections` as `list_connections` gives it. */
const connections = (): ConnectionItem[] => [
  ...settings.llm_connections
    .filter((c) => !reserved(c.name))
    .map((c) => ({
      name: c.name,
      label: c.name,
      host: URL.canParse(c.base_url) ? new URL(c.base_url).host : c.base_url,
      is_local: c.is_local,
      builtin: false,
    })),
  { name: BUILTIN, label: connectionLabel(BUILTIN), host: "", is_local: true, builtin: true },
];

const loaded = () => ({
  settings: clone(settings),
  read_only: false,
  rename: settings.llm_connections.map((c) => c.name).filter(reserved),
});

/** Where a meeting's summary goes, or why it waits. */
const routeKey = (meta: Meta) => route(meta)?.name ?? waitOf(meta);

function summaryRoute(meta: Meta): SummaryRoute {
  const sentNothing =
    meta.no_speech || (meta.llm_connection == null && ["review", "sent", "archived"].includes(meta.status));
  return {
    connection: sentNothing ? null : route(meta),
    local: meta.summarize_local,
    local_available: builtinReady() || settings.llm_connections.some((c) => c.is_local && !reserved(c.name)),
    locked: summaryStarted(meta),
    toggle_matters: routeKey({ ...meta, summarize_local: !meta.summarize_local }) !== routeKey(meta),
  };
}

// ----- recording levels (architecture 4.1) -----

/** Smooth noise for 24 bands: voices sit low, so the energy falls toward 8 kHz. */
const bands = (t: number, loud: number, seed: number) =>
  Array.from({ length: 24 }, (_, i) => {
    const wobble =
      0.5 + 0.25 * Math.sin(t * 3.1 + i * 0.9 + seed) + 0.25 * Math.sin(t * 7.3 + i * 2.1 + seed);
    return Math.round(Math.min(100, Math.max(0, loud * (1 - i / 28) * wobble * 110)));
  });

/** A voice at `t` seconds, 0..1: syllables about four a second, inside phrases that come and go. */
const speech = (t: number) =>
  Math.min(1, Math.max(0, Math.sin(t * 0.9) + 0.35) * (0.55 + 0.45 * Math.abs(Math.sin(t * 6.3))));

function levels() {
  if (!recording) return;
  const elapsed = Date.now() - recording.startedAt;
  const t = elapsed / 1000;
  const voice = speech(t);
  const system = Math.max(0, Math.sin(t * 0.37 + 2)) * 0.6;
  const level: Level = {
    elapsed_ms: elapsed,
    mic: 0.002 + voice * 0.08,
    system: 0.0005 + system * 0.03,
    mic_bands: bands(t, voice, 0),
    system_bands: bands(t, system, 7),
    waiting_permission: false,
  };
  send("recording://level", level);
}

// ----- commands -----

const later = <T>(ms: number, value: () => T) =>
  new Promise<T>((resolve) => {
    const timer = setTimeout(() => resolve(value()), ms);
    timers.push(timer);
  });
const oneLine = (title: string) => title.replace(/\p{Cc}+/gu, " ");
const minutes = (ms: number) => {
  const m = Math.floor(ms / 60_000);
  return m >= 60 ? `${Math.floor(m / 60)}시간${m % 60 ? ` ${m % 60}분` : ""}` : m ? `${m}분` : "1분 미만";
};

type Args = Record<string, unknown>;

/** Every command the app sends, by the `ipc.ts` function that sends it. A handler must resolve to
 * what that function promises, so the preview cannot drift from the real IPC shapes. */
type Commands = {
  get_settings: typeof ipc.getSettings;
  save_settings: typeof ipc.saveSettings;
  reset_settings: typeof ipc.resetSettings;
  set_secret: typeof ipc.setSecret;
  has_secret: typeof ipc.hasSecret;
  delete_secret: typeof ipc.deleteSecret;
  list_audio_devices: typeof ipc.listAudioDevices;
  test_system_audio: typeof ipc.testSystemAudio;
  test_microphone: typeof ipc.testMicrophone;
  list_meetings: typeof ipc.listMeetings;
  get_meeting: typeof ipc.getMeeting;
  get_transcript: typeof ipc.getTranscript;
  delete_meeting: typeof ipc.deleteMeeting;
  resume_meeting: typeof ipc.resumeMeeting;
  pause_meeting: typeof ipc.pauseMeeting;
  transcribe_languages: typeof ipc.transcribeLanguages;
  set_meeting_language: typeof ipc.setMeetingLanguage;
  retranscribe: typeof ipc.retranscribe;
  summary_route: typeof ipc.summaryRoute;
  set_summarize_local: typeof ipc.setSummarizeLocal;
  save_summary: typeof ipc.saveSummary;
  archive_meeting: typeof ipc.archiveMeeting;
  retry_summary: typeof ipc.retrySummary;
  list_connections: typeof ipc.listConnections;
  export_markdown: typeof ipc.exportMarkdown;
  slack_length: typeof ipc.slackLength;
  send_to_slack: typeof ipc.sendToSlack;
  slack_sign_in: typeof ipc.slackSignIn;
  slack_cancel_sign_in: typeof ipc.slackCancelSignIn;
  slack_sign_out: typeof ipc.slackSignOut;
  slack_channels: typeof ipc.slackChannels;
  start_recording: typeof ipc.startRecording;
  stop_recording: typeof ipc.stopRecording;
  recording_state: typeof ipc.recordingState;
  confirm_quit: typeof ipc.confirmQuit;
  request_quit: typeof ipc.requestQuit;
  open_log_dir: typeof ipc.openLogDir;
  queue_state: typeof ipc.queueState;
  model_status: typeof ipc.modelStatus;
  download_models: typeof ipc.startModelDownload;
  cancel_model_download: typeof ipc.cancelModelDownload;
  import_model: typeof ipc.importModel;
  import_recording: typeof ipc.importRecording;
  /** Settings' 직접 가져오기 and Home's recording file picker (`@tauri-apps/plugin-dialog`). */
  "plugin:dialog|open": () => Promise<string | null>;
};
type Handlers = {
  [C in keyof Commands]: (args: Args) => ReturnType<Commands[C]> | Awaited<ReturnType<Commands[C]>>;
};

const handlers: Handlers = {
  get_settings: loaded,
  save_settings: (a) => {
    const value = a.value as Settings;
    const taken = value.llm_connections.find((c) => reserved(c.name));
    if (taken) throw `'${taken.name}'은(는) 이 PC 요약이 쓰는 이름이라 연결 이름으로 쓸 수 없습니다. 이름을 바꿔 주세요`;
    // `local_default_kept`: off while nothing local can summarize.
    const local = builtinReady() || value.llm_connections.some((c) => c.is_local);
    settings = clone({ ...value, summarize_local_by_default: value.summarize_local_by_default && local });
    enqueuePending();
    return clone(settings);
  },
  reset_settings: () => {
    settings = fullSettings();
    return loaded();
  },
  set_secret: (a) => {
    secrets.add(`${a.kind}:${a.name}`);
  },
  has_secret: (a) => secrets.has(`${a.kind}:${a.name}`),
  delete_secret: (a) => {
    secrets.delete(`${a.kind}:${a.name}`);
  },
  list_audio_devices: () => clone(DEVICES),
  // The tone plays for a second, then the loopback heard it.
  test_system_audio: () => {
    if (checking) throw "이미 테스트 중입니다";
    if (recording) throw "녹음 중에는 테스트할 수 없습니다";
    checking = true;
    return later(1000, () => {
      checking = false;
      return true;
    });
  },
  // Five seconds of someone saying a few words, 25 levels a second.
  test_microphone: () => {
    if (checking) throw "이미 테스트 중입니다";
    if (recording) throw "녹음 중에는 테스트할 수 없습니다";
    checking = true;
    const started = Date.now();
    return new Promise<boolean>((resolve) => {
      const timer = setInterval(() => {
        const t = (Date.now() - started) / 1000;
        if (t < 5) return send("audio-test://level", Math.round(10 + speech(t + 1) * 55));
        clearInterval(timer);
        checking = false;
        send("audio-test://level", 0);
        resolve(true);
      }, 40);
      timers.push(timer);
    });
  },

  list_meetings: () =>
    clone(meetings.map((m) => m.meta).sort((a, b) => b.created_at.localeCompare(a.created_at))),
  get_meeting: (a) => {
    const m = find(a.id);
    return { meta: clone(m.meta), summary: m.summary, sending: m.sending };
  },
  get_transcript: (a) => {
    const m = find(a.id);
    if (!m.transcript) throw "transcript.json을 읽지 못했습니다: 파일이 없습니다";
    return clone(m.transcript);
  },
  delete_meeting: (a) => {
    if (recording?.id === a.id) recording = null;
    meetings = meetings.filter((m) => m.meta.id !== a.id);
    line = line.filter((p) => p.id !== a.id);
    renumber();
  },
  resume_meeting: (a) => {
    const m = find(a.id);
    if (m.meta.interrupted)
      Object.assign(m.meta, { status: "recorded", interrupted: false, duration_ms: 31 * 60_000 });
    m.meta.held = false;
    // Still stopping: its end puts it back in line.
    if (!line.some((p) => p.id === m.meta.id)) enqueue(m);
    return clone(m.meta);
  },
  // Plan 5.3-4: one in line is paused at once; a running one stops a moment later.
  pause_meeting: (a) => {
    const m = find(a.id);
    const p = line.find((x) => x.id === m.meta.id);
    if (!p) return "already_done";
    m.meta.held = true;
    if (!isRunning(p)) {
      stopRun(m, p);
      return "paused";
    }
    timers.push(setTimeout(() => stopRun(m, p), 1500));
    return "pausing";
  },

  transcribe_languages: () => clone(LANGS),
  // Plan 5.6-4 (queue.rs `set_language`): not while it runs, nor once a chunk is done.
  set_meeting_language: (a) => {
    const m = find(a.id);
    if (line.some((p) => p.id === m.meta.id && isRunning(p))) throw "처리 중에는 바꿀 수 없습니다";
    if (m.meta.chunks_done > 0 || m.transcript) throw "이미 전사한 구간이 있어 다시 전사해야 바꿀 수 있습니다";
    if (flaky === m.meta.id) {
      flaky = null;
      throw "meta.json 저장 실패: 다른 프로세스가 파일을 사용 중이기 때문에 프로세스가 액세스할 수 없습니다. (os error 32)";
    }
    m.meta.languages = a.languages as string[];
    return clone(m.meta);
  },
  // Plan 5.6-6 (E3, queue.rs `retranscribe`): its results gone, `recorded` and back in line.
  retranscribe: (a) => {
    const m = find(a.id);
    if (line.some((p) => p.id === m.meta.id && isRunning(p))) throw "처리 중입니다. 먼저 일시정지해 주세요";
    if (m.sending) throw "보내는 중입니다";
    const from: Status[] = ["transcribed", "review", "summarize_failed", "archived", "transcribe_failed", "paused"];
    if (!from.includes(m.meta.status)) throw `다시 전사할 수 없는 상태입니다: ${m.meta.status}`;
    line = line.filter((p) => p.id !== m.meta.id);
    Object.assign(m.meta, {
      status: "recorded",
      languages: a.languages as string[],
      chunks_done: 0,
      backend: null,
      error: null,
      llm_connection: null,
      no_speech: false,
      held: false,
    });
    delete m.meta.wait;
    Object.assign(m, { summary: "", generated: "", transcript: null });
    enqueue(m);
    return clone(m.meta);
  },

  summary_route: (a) => summaryRoute(find(a.id).meta),
  set_summarize_local: (a) => {
    const m = find(a.id);
    if (a.local && !summaryRoute(m.meta).local_available) throw "로컬 LLM 연결이 없습니다";
    if (summaryStarted(m.meta)) throw "요약이 이미 시작돼 바꿀 수 없습니다";
    m.meta.summarize_local = a.local as boolean;
    enqueuePending();
    return summaryRoute(m.meta);
  },
  save_summary: (a) => {
    const m = find(a.id);
    if (m.meta.status !== "review") throw "검토 대기인 회의만 저장할 수 있습니다";
    m.meta.title = oneLine(a.title as string);
    m.summary = a.body as string;
    return clone(m.meta);
  },
  archive_meeting: (a) => {
    const m = find(a.id);
    m.meta.status = "archived";
    return clone(m.meta);
  },
  retry_summary: (a) => {
    const m = find(a.id);
    const saved = settings.llm_connections.filter((c) => !reserved(c.name));
    const connection = [...saved, BUILTIN_CONNECTION].find((c) => c.name === a.connection);
    if (!connection) throw `LLM 연결 '${a.connection}'을(를) 찾을 수 없습니다. 다른 연결을 골라 주세요`;
    if (connection.name === BUILTIN && !builtinReady())
      throw "이 PC 요약 모델이 아직 없습니다. 설정 → 요약(LLM)에서 요약 모델을 먼저 받아 주세요";
    const waits = m.meta.status === "transcribed" || (m.meta.status === "paused" && !!m.transcript);
    if (m.meta.status !== "summarize_failed" && !waits) throw "요약을 기다리거나 요약에 실패한 회의가 아닙니다";
    Object.assign(m.meta, { llm_connection: connection.name, summarize_local: connection.is_local });
    delete m.meta.wait;
    enqueue(m);
    return clone(m.meta);
  },
  list_connections: connections,
  export_markdown: (a) => `/Users/hong/Documents/${find(a.id).meta.title || a.id}.md`,

  slack_length: (a) => {
    const meta = find(a.id).meta;
    const start = new Date(meta.created_at);
    const head = `${start.getFullYear()}-${pad(start.getMonth() + 1)}-${pad(start.getDate())} ${pad(start.getHours())}:${pad(start.getMinutes())}`;
    const text = `**${a.title}**\n${head} · ${minutes(meta.duration_ms)} · 녹음: ${settings.recorder_name}\n\n${a.body}`;
    return [...text].length;
  },
  send_to_slack: (a) => {
    const m = find(a.id);
    if (m.sending) throw "이 회의를 이미 보내고 있습니다";
    m.meta.title = oneLine(a.title as string);
    m.summary = a.body as string;
    m.sending = true;
    return later(1000, () => {
      const lines = new Set(m.generated.split("\n"));
      Object.assign(m.meta, {
        status: "sent",
        error: null,
        sent: {
          channel: `#${(a.channel as ipc.Channel).name}`,
          at: new Date().toISOString(),
          edit_count: m.summary.split("\n").filter((l) => !lines.has(l)).length,
        },
      });
      m.sending = false;
      settings.last_channel = (a.channel as ipc.Channel).id;
      send("slack://finished", a.id);
      return clone(m.meta);
    });
  },

  // ONB-04: the browser comes back signed in after two seconds, unless cancelled first.
  slack_sign_in: () =>
    new Promise<string | null>((resolve) => {
      const timer = setTimeout(() => {
        settings.slack_team = "SIJE";
        resolve("SIJE");
      }, 2000);
      signingIn = () => {
        clearTimeout(timer);
        resolve(null);
      };
    }),
  slack_cancel_sign_in: () => signingIn(),
  slack_sign_out: () => {
    settings.slack_team = settings.last_channel = null;
  },
  slack_channels: () => {
    if (!settings.slack_team) throw "Slack 로그인이 필요합니다. 설정 → Slack에서 다시 로그인해 주세요";
    return later(400, () => clone(CHANNELS));
  },

  start_recording: () => {
    if (recording) throw "이미 녹음하고 있습니다";
    // TRN-09: running work stops and waits for the recording to end.
    for (const p of line.filter(isRunning)) find(p.id).meta.status = p.status = "paused";
    const m = stored("recording", new Date(), {
      duration_ms: 0,
      summarize_local: settings.summarize_local_by_default,
      languages: [...settings.transcribe_languages],
    });
    meetings.push(m);
    recording = { id: m.meta.id, startedAt: Date.now() };
    renumber();
    return { id: m.meta.id, warnings: [] };
  },
  stop_recording: () => {
    if (!recording) return null;
    const m = find(recording.id);
    Object.assign(m.meta, { status: "recorded", duration_ms: Date.now() - recording.startedAt });
    recording = null;
    enqueue(m);
    send("recording://stopped", { id: m.meta.id, message: null });
    return m.meta.id;
  },
  recording_state: () => (recording ? { id: recording.id, warnings: [] } : null),

  queue_state: () => clone(line),
  model_status: (args) => modelStatus(args.scope),
  download_models: (args) =>
    new Promise<void>((resolve, reject) => {
      const chain = scoped(args.scope);
      const started = Date.now();
      const timer = setInterval(() => {
        const file = chain.find((f) => !f.installed);
        if (downloading?.cancelled || !file) {
          clearInterval(timer);
          const cancelled = downloading?.cancelled;
          downloading = null;
          return cancelled ? reject("다운로드를 취소했습니다") : resolve();
        }
        // About 32 MB/s: slow enough to watch the progress in the settings and on Home. The 78 MB
        // detection model at 2 MB/s, or its states would be gone before they are seen.
        const step = file.file === LID.file ? 500_000 : 8_000_000;
        file.received = Math.min(file.size, file.received + step);
        const all = chain.reduce((sum, f) => sum + f.size, 0);
        const done = chain.reduce((sum, f) => sum + (f.installed ? f.size : f.received), 0);
        send("model://download", {
          file: file.file,
          received: file.received,
          total: file.size,
          all_received: done,
          all_total: all,
          // Like the app: no speed for the download's first 10 s (plan 5.2-3).
          bps: Date.now() - started < 10_000 ? null : step * 4,
        });
        if (file.received === file.size) {
          Object.assign(file, { installed: true, received: 0 });
          if (file.file === SUMMARY_MODEL.file) enqueuePending();
        }
      }, 250);
      timers.push(timer);
      downloading = { cancelled: false, done: resolve, chain };
    }),
  cancel_model_download: () => {
    if (!downloading) return null;
    downloading.cancelled = true;
    return "stopped";
  },
  import_model: (args) => {
    scoped(args.scope).forEach((f) => Object.assign(f, { installed: true, received: 0 }));
    enqueuePending();
    return modelStatus(args.scope);
  },
  "plugin:dialog|open": (a) => {
    const filters = JSON.stringify(a);
    if (filters.includes("mp3")) return "/Users/hong/Downloads/주간 회의.m4a";
    return `/Users/hong/Downloads/${filters.includes("gguf") ? SUMMARY_MODEL.file : models.files[0].file}`;
  },
  // REC-12: a 37-minute file that ended a minute ago, decoded in two seconds, 5% at a time.
  import_recording: (a) =>
    new Promise((resolve) => {
      const name = String(a.path).split("/").pop();
      let percent = 0;
      const timer = setInterval(() => {
        send("import://progress", { name, percent });
        percent += 5;
        if (percent <= 100) return;
        clearInterval(timer);
        const m = stored("recorded", new Date(Date.now() - 38 * 60_000), {
          duration_ms: 37 * 60_000,
          summarize_local: settings.summarize_local_by_default,
          languages: a.languages as string[],
        });
        meetings.push(m);
        enqueue(m);
        resolve(clone(m.meta));
      }, 100);
      timers.push(timer);
    }),

  confirm_quit: () => {
    console.info("[미리보기] 종료 확인: 실제 앱이라면 진행 중인 작업을 멈추고 종료합니다");
  },
  request_quit: () => send("app://quit-requested"),
  open_log_dir: () => {
    console.info("[미리보기] 로그 폴더 열기: 실제 앱이라면 app.log가 있는 폴더를 엽니다");
  },
};

function handle(cmd: string, args: Args) {
  if (!(cmd in handlers)) throw `미리보기에 없는 명령입니다: ${cmd}`;
  return handlers[cmd as keyof Handlers](args);
}

/** Puts the mock backend in place, once, before the app's first call. */
export function install(scenario: Scenario) {
  reset(scenario);
  mockIPC((cmd, args?: InvokeArgs) => handle(cmd, (args ?? {}) as Args), { shouldMockEvents: true });
  mockWindows("main");
  // The mock's unlisten reads `id`, but the API (2.12) sends `eventId`: without this, a listener
  // StrictMode removed stays and every later emit warns about its missing callback.
  const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (...a: unknown[]) => unknown } })
    .__TAURI_INTERNALS__;
  const invoke = internals.invoke;
  internals.invoke = (cmd, args, options) =>
    invoke(
      cmd,
      cmd === "plugin:event|unlisten" ? { ...(args as Args), id: (args as Args).eventId } : args,
      options,
    );
}

/** Events the Rust side sends by itself, to try banners and dialogs. */
export const simulate = {
  gpuFailed: () =>
    send("transcribe://gpu-failed", {
      message: "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다",
    }),
  quitRequested: () => send("app://quit-requested"),
};
