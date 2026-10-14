# 아키텍처

- 상태: v1 기준선 (2026-10-13). 스파이크 S1~S5 결과에 따라 바뀔 수 있는 부분은 **[S#]**로 표시했다.
- 근거: [설계 문서](designs/meeting-summarizer.md) · [요구사항](requirements.md)
- 관련 문서: [로드맵](roadmap.md)

## 1. 전체 구조

```
┌──────────────────────── Tauri 앱 (단일 프로세스) ────────────────────────┐
│                                                                          │
│  React (WebView)                    Rust 백엔드                          │
│  ┌──────────────┐   invoke    ┌──────────────────────────────────────┐   │
│  │ 화면 6개      │ ─────────▶ │ commands.rs                          │   │
│  │ (상태로 전환) │ ◀───────── │                                      │   │
│  └──────────────┘   events    │  audio/ ──▶ meetings/{id}/*.wav      │   │
│                               │     │                                │   │
│                               │     ▼ (녹음 종료)                    │   │
│                               │  queue ──▶ transcribe/ ──▶ summarize/│   │
│                               │                         │            │   │
│                               │  slack.rs ◀── (검토 후 전송 버튼)    │   │
│                               │  store.rs (meta.json)  settings.rs   │   │
│                               └──────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────┘
        │ 모델 다운로드          │ LLM 요청              │ Slack 웹훅
        ▼                       ▼                       ▼
   Hugging Face          클라우드 API 또는          hooks.slack.com
   (또는 사내 미러)      로컬 Ollama/LM Studio
```

- 무거운 일(녹음, 전사, 요약, 전송)은 모두 Rust에서 한다. React는 화면만 그린다.
- React → Rust는 `invoke`(명령), Rust → React는 이벤트(진행률, 레벨, 상태 변경)로 통신한다.
- 데이터는 모두 녹음자 PC의 앱 데이터 폴더에 있다. DB와 서버는 없다.

## 2. 기술 스택

| 영역 | 선택 | 비고 |
|---|---|---|
| 앱 셸 | Tauri 2 | 기존 템플릿. Tauri 3는 알파라 쓰지 않는다 |
| 프론트엔드 | React 19, TypeScript 6, Vite 8, bun | 기존 템플릿 |
| 스타일 | Tailwind CSS v4 (`@tailwindcss/vite`) | 회사 색상 토큰을 `@theme`으로 이식 (10.3) |
| UI 컴포넌트 | shadcn/ui (CLI 4.21.x, base: Radix, 프리셋 `radix-nova`) + `lucide-react` 아이콘 + `tw-animate-css`. CLI가 `radix-ui`, `class-variance-authority`, `cn`(shadcn의 `cn` 패키지)을 함께 설치한다 | 사용자 결정(2026-10-14). 컴포넌트 소스는 `src/components/ui/`에 복사돼 우리 코드가 된다. shadcn 테마 변수에 회사 색상을 연결한다 (10.3) |
| 오디오 캡처 | `cpal` >= 0.18.2, macOS는 `objc2-core-audio` 0.3 | 마이크는 양쪽 OS 모두 cpal. 시스템 오디오는 Windows가 cpal WASAPI 루프백, macOS가 직접 만든 Core Audio tap(4.3). 0.18.0에서 `name()`이 제거됐고, 0.18.2부터 `DeviceId`가 재시작 후에도 유지된다 |
| 링 버퍼 | `rtrb` | 오디오 콜백 → 기록 스레드. 락·할당 없는 SPSC |
| WAV 기록 | `hound` | |
| 리샘플 | `rubato` 5 | 장치 샘플레이트 → 16kHz. 안티에일리어싱 FFT 리샘플러(`Fft`) |
| 전사 | `whisper-rs` (whisper.cpp 바인딩) | 버전은 **[S3]**로 고정. features: macOS `metal`, Windows `vulkan` |
| HTTP | `reqwest` | LLM, Slack, 모델 다운로드 |
| 해시 | `sha2` | 모델 검증 |
| 비밀값 | `keyring` 4.x | `tauri-plugin-stronghold`는 지원 중단돼 쓰지 않는다 |
| 절전 방지 | `keepawake` | |
| Tauri 플러그인 | `tauri-plugin-notification` >= 2.6.0(데스크톱 알림 클릭 `onAction`이 이 버전부터 동작), `tauri-plugin-dialog`(파일 선택), `tauri-plugin-single-instance`(두 번째 실행 차단, eng review D14), 내장 tray-icon | 자동 업데이트(`tauri-plugin-updater`)는 파일럿 뒤로 미뤘다(eng review D2) |

## 3. 디렉터리 구조 (제안)

```
src-tauri/src/
  main.rs            진입점 (기존). `--transcribe-worker <meeting> <chunk> <backend>` 인자로 실행되면 전사 워커 모드로 동작 (5.3)
  lib.rs             Builder, 플러그인, 트레이, 명령 등록
  commands.rs        React에서 부르는 명령 모음
  settings.rs        settings.json 읽기·쓰기 + keyring 래퍼
  audio/
    mod.rs           Recorder(녹음 시작·종료·상태), 이벤트, 디스크 여유 공간
    capture.rs       캡처 스레드: cpal 스트림 열기·재연결, 기본 장치 1초 폴링
    macos_tap.rs     macOS 시스템 오디오용 전역 tap + 비공개 aggregate 장치 (4.3)
    writer.rs        기록 스레드: 다운믹스 → 리샘플 → 정렬 → WAV, 레벨·무음·권한·최대 길이·디스크·절전 검사
    clock.rs         OS 시계 읽기 + 무음 채우기·넘친 샘플·절전 판정 (판정은 순수 로직)
    mix.rs           게이팅 + 합산 (순수 로직)
  transcribe/
    model.rs         모델 다운로드·가져오기·검증
    engine.rs        whisper-rs 실행, 백엔드 선택, 중단 콜백, VAD 기반 10분 구간 나누기
    filter.rs        환각 필터, 비정상 결과 검사 (순수 로직)
  summarize.rs       OpenAI 호환 클라이언트, 프롬프트, 구간 요약
  slack.rs           페이로드 생성, 글자 수 계산, 전송
  store.rs           폴더 구조, meta.json, 상태 전이, 앱 시작 시 복구
  queue.rs           FIFO 처리 워커, 일시정지·중단
  resources/
    hallucinations.ko.txt   환각 문구 목록 (한 줄에 하나)
src/
  main.tsx, App.tsx  화면 전환 (라우터 라이브러리 없이 상태 하나로)
  screens/           Onboarding, Home, Recording, Processing, Review, Settings
  components/ui/     shadcn이 생성한 컴포넌트(button, input, textarea, card, alert, alert-dialog, select, switch, badge, tooltip)
  components/        앱 전용 컴포넌트(common.tsx: Header, Banner, Field, DeleteButton 등. 이후 LevelMeter, ChannelSelect)
  lib/ipc.ts         invoke·이벤트 타입 래퍼
  lib/utils.ts       shadcn `cn()` (`export { cn } from "cn"`, CLI 4.21 기본)
  test-setup.ts      Vitest jsdom 보완 코드(Radix용)
  styles/theme.css   Tailwind 진입점 + 회사 색상 토큰 + shadcn 테마 변수 연결 (10.3)
components.json     shadcn 설정 (style, base radix, alias @/)
```

화면은 6개뿐이라 라우터 라이브러리를 쓰지 않는다. `App.tsx`가 현재 화면과 선택된 회의 id를 상태로 들고 있다.

## 4. 녹음

### 4.1 스레드 구조

```
캡처 스레드 (스트림 열기, 1초 폴링)
  cpal 콜백 (마이크)        ── 링 버퍼 ──┐
  cpal 콜백 (시스템 오디오) ── 링 버퍼 ──┴▶ 기록 스레드 (20ms마다) ──▶ mic.wav / system.wav
                                               └─▶ 레벨 이벤트 (100ms마다 React로)
```

- 오디오 콜백은 장치 샘플(인터리브 f32)을 트랙별 `rtrb` 링 버퍼(2초 분량)에 넣기만 한다. 콜백 안에서 할당, 락, 파일 I/O를 하지 않는다. 버퍼가 차면(기록 스레드가 멈춘 경우) 나머지를 버리고, 빈 구간은 4.2의 무음 채우기가 메운다.
- 스트림을 다시 열면 새 링 버퍼를 만들고, 받는 쪽을 채널로 기록 스레드에 넘긴다. 기록 스레드는 이전 버퍼에 남은 샘플을 먼저 쓴 뒤 바꾼다.
- 기록 스레드가 트랙마다 다음을 한다: 채널 다운믹스(모노) → 16kHz 리샘플 → 시간축 정렬 → 16-bit PCM으로 `hound`에 기록. 리샘플러의 시작 지연만큼 앞부분을 잘라 소리가 늦게 밀리지 않게 한다.
- `hound::WavWriter::flush()`를 5초마다 불러 헤더의 길이 필드를 갱신하고, 같은 파일의 두 번째 핸들로 `sync_data()`를 불러 디스크에 내린다(REC-02, NFR-05). 그래서 전원 차단에도 손실은 최대 5초다. sync 실패는 쓰기 오류와 같이 처리한다(4.6).
- 녹음 종료는 언제나 기록 스레드가 마무리한다. 두 WAV를 finalize하고, `meta.json`에 길이·`gaps`와 `recorded`를 쓰고, `recording://stopped`를 보낸다. 사용자 종료, 4시간, 절전, 디스크 부족, 쓰기 오류가 모두 같은 길이다.
- 절전 방지(`keepawake`, REC-06)는 기록 스레드가 녹음 동안 쥐고 있다. Windows `SetThreadExecutionState`가 스레드 단위라서다.

### 4.2 시간축 정렬 (REC-03)

- 기준 시계는 `clock.rs`의 **절전을 세지 않는 단조 시계**다. macOS는 `CLOCK_UPTIME_RAW`(= `mach_absolute_time`), Windows는 `QueryUnbiasedInterruptTime`을 쓴다. 둘 다 OS가 "절전·최대 절전 시간을 포함하지 않는다"고 문서화한 시계다.
- **절전 감지는 오디오 데이터가 아니라 OS 시계로 한다** (eng review outside voice #2).
  - 절전을 포함하는 시계(macOS `CLOCK_MONOTONIC_RAW` = `mach_continuous_time`, Windows `QueryInterruptTime`)와 위 시계의 증가분 차이를 녹음 시작부터 잰다. 차이가 생긴 만큼이 절전 길이다. 두 쌍 모두 같은 원천에서 나온 시계라 깨어 있는 동안에는 차이가 변하지 않는다. macOS `CLOCK_MONOTONIC`은 쓰지 않는다. NTP가 속도를 조정해(실측 약 -3ppm) 깨어 있어도 차이가 조금씩 벌어진다. 두 시계를 차례로 읽는 오차가 있으므로 1초 미만의 차이는 절전으로 보지 않는다.
  - 그래서 마이크가 빠지고 시스템 오디오가 무음인 공백은 길이와 상관없이 절전으로 오판하지 않는다. 데이터 공백은 항상 장치 문제로 보고 아래 무음 채우기로 처리한다.
  - 기준 시계가 절전을 세지 않으므로, 절전 구간에는 어느 트랙에도 무음이 들어가지 않는다.
- 트랙마다 `기대 샘플 수 = 시계 경과 × 16000`을 계산한다.
  - 기록한 샘플이 기대보다 100ms(1,600샘플) 넘게 적으면, 차이만큼 0을 채운다. Windows 루프백의 무음 구간과 마이크 끊김이 이걸로 처리된다.
  - 10초마다 확인해서 기대보다 100ms 넘게 많으면, 넘친 만큼 버린다. 장치 클럭이 빠른 경우다.
- 절전 처리(REC-06): 위에서 잰 절전 길이가 5분 이하면 계속 녹음하고, 그 구간을 `meta.json`의 `gaps`에 남긴다. 5분을 넘으면 절전 시점에서 녹음을 닫고 `recorded`로 바꾼 뒤 대기열에 넣는다.

### 4.3 장치 관리 (REC-07)

**macOS 시스템 오디오는 cpal 루프백을 쓰지 않는다 (결정, 2026-10-14).** cpal 0.18.2는 출력 장치에 입력 채널이 **없을 때만** Core Audio tap을 연다. 출력 장치가 입력도 가진 경우(USB·블루투스 헤드셋 대부분)에는 오류 없이 그 장치의 **마이크**를 연다(cpal `macos/device.rs`에서 확인). 앱이 헤드폰을 권장하므로(REC-10) 이 경우가 흔하고, 그러면 "시스템 오디오" 트랙에 헤드셋 마이크가 녹음된다. 그래서 `macos_tap.rs`가 tap을 직접 만든다.

- `objc2-core-audio`로 **전역 모노 tap**을 만든다(`CATapDescription` `initMonoGlobalTapButExcludeProcesses`, 제외 프로세스 없음, `Unmuted`, `private`). 특정 출력 장치가 아니라 모든 프로세스의 출력을 받는다.
- 그 tap 하나만 담은 **비공개 aggregate 장치**를 만든다(드리프트 보정 켬). 사전 구성은 cpal `loopback.rs`를 따랐다.
- aggregate는 만든 프로세스에게 보이는 입력 장치라서, cpal `device_by_id`로 찾아 보통 입력 스트림으로 연다(실측: 모노 48kHz f32로 열림).
- 닫는 순서: 스트림 → aggregate 장치 → tap. 기본 출력 장치가 바뀌면 tap과 aggregate를 다시 만든다.
- macOS 14.2 이상이 필요하다(앱 최소 버전 14.6). 권한 문구는 `NSAudioCaptureUsageDescription`(11.1).
- macOS에서는 설정의 출력 장치 고정을 쓰지 않는다. 전역 tap은 모든 출력 장치의 소리를 받기 때문이다.
- Windows는 cpal WASAPI 루프백(기본 출력 장치에 입력 스트림)을 쓴다. 재생 중인 소리가 없으면 데이터가 오지 않고, 4.2의 무음 채우기가 메운다.

- `capture.rs`의 장치 감시 루프가 1초마다 `host.default_input_device()`와 `host.default_output_device()`의 `id()`(`DeviceId`)를 확인한다. cpal에는 기본 장치 변경을 구독하는 API가 없어 폴링한다. 스트림 오류 콜백도 플래그만 세우고, 다음 폴링(1초 이내)에서 다시 연다.
- 같은 장치가 3번 연속 열리지 않으면 그 장치는 5초마다만 다시 시도한다(macOS에서는 시도마다 tap을 만들고 지우기 때문). 처음 두 번은 1초 폴링을 따르므로 REC-07의 2초 안에 든다. 다른 장치가 기본 장치가 되면 바로 시도한다.
- 녹음이 끝난 뒤에는 캡처 스레드가 새 스트림이나 tap을 만들지 않고 경고도 보내지 않는다. 권한 팝업에 막혀 늦게 돌아온 경우도 같다. 앱 종료 때 이 스레드는 기다리지 않는다.
- 고정 장치는 `settings.json`의 `input_device`·`output_device`에 `DeviceId` 문자열(`coreaudio:<UID>` 등)로 둔다.
- 다시 연결하는 경우:
  - 스트림 오류 콜백이 왔다 (장치 분리).
  - 장치를 고정하지 않았는데 기본 장치 식별자가 바뀌었다 (헤드셋 연결 등).
  - 고정한 장치가 목록에서 사라졌다 → 기본 장치로 바꾼다.
- 다시 연결하는 동안 빠진 구간은 4.2의 무음 채우기 규칙으로 메운다.
- **대체할 장치가 없을 때**(예: 마이크가 하나뿐인 데스크톱에서 마이크를 뽑음): 녹음을 멈추지 않는다. 그 트랙은 무음으로 채우며 계속 녹음하고 "마이크를 찾을 수 없습니다" 배너를 띄운다. 장치가 다시 나타나면 1초 폴링이 붙인다. React에 `recording://warning` 이벤트를 보내 배너를 띄운다.

### 4.4 권한 감지 (REC-08)

- **첫 실행 권한 팝업은 스트림 생성을 멈춘다** (S1 도구에서 확인): 마이크 권한을 아직 묻지 않은 상태에서 cpal `build_input_stream`을 부르면, 사용자가 팝업에 답할 때까지 macOS HAL 안에서 호출이 돌아오지 않는다. 그래서 스트림은 캡처 스레드에서만 열고, UI와 IPC 스레드에서 열지 않는다. `start_recording`은 스트림을 기다리지 않고 바로 돌아온다. 장치 열기(macOS tap 생성부터 스트림 시작까지)가 1초를 넘으면 레벨 이벤트의 `waiting_permission`이 켜지고 화면에 "권한 허용을 기다리는 중"을 보여 준다. 그동안 두 트랙은 무음으로 채워져 시간축이 유지된다. "권한이 없으면 0 버퍼"는 사용자가 이미 거부한 뒤의 동작으로 보이며, S1에서 확인한다.

- 마이크: 마이크 장치에서 처음 받은 3초 분량의 샘플이 모두 정확히 0이면 `recording://warning { kind: "mic_permission" }`을 보낸다. 무음 채우기로 넣은 0은 세지 않는다.
- 시스템 오디오: 녹음 중에는 검사하지 않는다. 온보딩과 설정의 "시스템 오디오 테스트"가 1kHz 사인파를 1초 재생하고, 같은 시간 루프백 RMS가 -40 dBFS를 넘으면 통과로 본다.
- macOS에서는 권한이 없으면 오류 없이 0 버퍼만 온다. 그래서 위 방식으로만 판정한다.

### 4.5 합산과 게이팅 (TRN-02)

전사 워커가 **구간마다** `mic.wav`와 `system.wav`에서 그 구간의 샘플을 읽어 `mix.rs`로 메모리에서 합친다. 합친 결과는 파일로 저장하지 않는다(eng review outside voice #5). 10분 구간은 f32로 약 38MB다.

```
for 각 20ms 프레임:
    sys_rms = RMS(system 프레임)
    target_gain = sys_rms > -45 dBFS ? -20 dB : 0 dB
    mic_gain = 스무딩(target_gain, attack 5ms, release 300ms)
    out = mic × mic_gain + system
    out = 소프트 리미터(out)   # 클리핑 방지
```

임계값(-45 dBFS), 감쇠(-20dB), attack·release 값은 **[S4]**에서 조정한다.

4시간 녹음을 f32로 한꺼번에 메모리에 올리면 약 920MB이므로, 항상 창이나 구간 단위로만 읽는다. 처리 단계는 회의 폴더에 오디오를 새로 쓰지 않으므로, 디스크 부족으로 녹음이 끝난 직후에도 처리가 디스크를 더 쓰지 않는다(전사·요약 결과는 수십 KB).

### 4.6 디스크 공간 (REC-11, eng review D7)

- 필요량: 4시간 회의 기준 두 트랙 약 920MB. 처리 단계는 오디오를 새로 쓰지 않는다(4.5). 두 트랙 녹음 속도는 64KB/s다(16kHz × 2B × 2트랙).
- **시작 전**: 여유 공간이 1.5GB 미만이면 "남은 공간으로 최대 약 N분 녹음할 수 있습니다"(N = 여유 공간 ÷ 64KB/s) 경고를 띄운다. 녹음은 시작할 수 있다.
- **녹음 중**: 60초마다 여유 공간을 확인한다. 200MB 미만이 되거나 WAV 쓰기 오류가 나면 녹음을 안전하게 끝낸다. 마지막 flush, 상태 `recorded`, 처리 대기열에 넣기, "디스크 공간이 부족해 녹음을 종료했습니다" 알림 순서다.
- 단위 테스트: 쓰기 오류를 주입하면 상태가 `recorded`가 되고 WAV가 유효한지 확인한다.

## 5. 전사

### 5.1 모델 파일

출처와 해시는 2026-10-13에 Hugging Face API로 확인한 값이다.

| 용도 | 파일 | 크기 | sha256 |
|---|---|---|---|
| 기본 모델 | `ggerganov/whisper.cpp` / `ggml-large-v3-turbo-q8_0.bin` | 874,188,075 B | `317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1` |
| 원본(선택) | `ggerganov/whisper.cpp` / `ggml-large-v3-turbo.bin` | 1,624,555,275 B | `1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69` |
| CPU 폴백 후보 **[S2]** | `ggerganov/whisper.cpp` / `ggml-medium-q5_0.bin` | 539,212,467 B | `19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f` |
| VAD | `ggml-org/whisper-vad` / `ggml-silero-v6.2.0.bin` | 885,098 B | `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987` |

- **다운로드 URL**: `{base}/{repo}/resolve/main/{file}`. 기본 `base`는 `https://huggingface.co`이고, 설정에서 사내 미러로 바꿀 수 있다(ONB-02).
- **저장 위치**: 앱 데이터 폴더의 `models/`.
- **다운로드 절차**: `.part` 파일로 받는다. 연결 10초, 데이터가 30초 동안 한 바이트도 오지 않으면 중단하고, 다음 시도 때 `Range` 헤더로 이어받는다. 끝나면 sha256을 검증하고, 맞으면 이름을 바꾼다. 해시가 틀리면 `.part`를 지우고 실패로 보고한다.
- **직접 가져오기**: 녹음자가 고른 파일을 복사하고 같은 방식으로 해시를 검증한다.
- **S3 실패 시 변화**: 내장 VAD가 없는 `whisper-rs` 버전으로 고정하면, VAD를 Silero ONNX 모델과 `ort` 크레이트로 따로 돌린다. 그러면 위 표의 VAD 항목이 ONNX 파일로 바뀌고 의존성에 `ort`가 추가된다.

### 5.2 구간 나누기와 실행

- **녹음 길이 (M2 → M3 인계)**: 녹음 길이는 `meta.json`의 `duration_ms`가 아니라 두 WAV 중 짧은 쪽의 길이(`min(len)`)로 정한다. 쓰기 오류로 끝난 녹음은 WAV가 마지막 5초 flush까지만 유효해 `duration_ms`보다 짧고, 중단된 녹음은 `duration_ms`가 0이다.

1. 워커를 `--plan-chunks {id}` 모드로 한 번 띄운다. 워커는 두 트랙을 10분 창(앞 창과 30초 겹침) 단위로 읽어 메모리에서 합치고 `WhisperVadContext`로 음성 구간 목록을 얻은 뒤, 구간 경계를 JSON으로 출력한다. whisper/ggml 네이티브 코드는 모두 워커 안에서만 돈다. 이 API는 샘플 슬라이스 전체를 한 번에 받으므로, 4시간 녹음을 통째로 넘기면 f32로 약 920MB가 된다. 그래서 창 단위로 나눈다. 겹친 30초에서 나온 구간은 시작 시각 기준으로 중복을 제거한다.
2. 음성 구간을 앞에서부터 모아, 누적 길이가 10분을 넘기 직전의 무음 지점에서 자른다.
3. 구간마다 `whisper_full`을 실행한다.
   - 파라미터: `language="ko"`, VAD 켬, `no_context=true`, `suppress_nst=true`, `temperature` 폴백 기본값 유지, 스레드 수 = 물리 코어 수(S2에서 측정해 조정).
   - 용어집을 initial prompt에 넣을지는 **[S2]**로 정한다.
4. 구간 결과를 `transcript.partial.jsonl`에 한 줄로 추가한다. 형식: `{"chunk":3,"start_ms":...,"end_ms":...,"segments":[{"start_ms","end_ms","text"}]}`.
5. 마지막 구간이 끝나면 합쳐서 `transcript.json`을 쓰고 상태를 `transcribed`로 바꾼다.

**재개 단위는 구간이다.** 재개할 때는 `transcript.partial.jsonl`의 마지막 구간 다음부터 처리한다.

### 5.3 전사 워커 프로세스와 중단 (TRN-06, TRN-09)

전사는 **같은 실행 파일을 자식 프로세스로 띄워** 돌린다(eng review D5). whisper.cpp가 GPU 드라이버 문제 등으로 비정상 종료해도 앱, 녹음, UI는 살아 있다.

```
queue.rs ──spawn── 앱 실행 파일 --transcribe-worker {id} {chunk} {gpu|cpu}
   ▲                    │ stdout: JSON 한 줄씩
   │                    │   {"type":"progress","pct":42}
   │                    │   {"type":"result","chunk":3,"segments":[...]}
   └──── 종료 코드 ─────┘ 0 = 정상, 그 외 = 실패/크래시
```

- 워커는 구간 하나를 처리하고 끝난다. 모델 로딩(수 초)은 구간마다 반복되지만, 구간이 10분 분량이라 비중이 작다.
- 결과는 부모(`queue.rs`)가 받아 `transcript.partial.jsonl`에 쓴다. 워커는 회의 폴더의 오디오를 읽기만 한다.
- **중단**: 녹음이 시작되면 부모가 워커 프로세스를 kill한다. 그 구간의 결과는 버리고, 녹음이 끝나면 그 구간부터 다시 처리한다. abort 콜백에 기대지 않는다.
- 워커의 stderr는 회의 폴더의 `worker.log`에 남긴다(크래시 원인 확인용).
- **종료 코드 계약**: `0` 정상, `2` 입력·파일 오류(오디오 없음, 인자 오류), `3` 모델 로드 실패(파일 손상·경로 오류), 그 외 코드나 시그널 종료는 **크래시**. 5.4의 GPU 실패 판정은 크래시만 센다. `2`·`3`은 GPU와 무관하므로 바로 `transcribe_failed`로 바꾸고 원인을 보여준다(`3`이면 "모델 다시 받기"를 제안한다).

### 5.4 백엔드 선택과 크래시 판정 (TRN-05, TRN-06)

- **빌드**: macOS는 `metal`, Windows는 `vulkan` feature로 빌드한다. CPU 사용은 런타임에 `WhisperContextParameters::use_gpu(false)`로 고른다.
- **Vulkan 로더 동봉**: Vulkan 빌드는 `vulkan-1.dll`에 로드 타임 의존성이 있다. 이 DLL이 없는 PC에서는 앱이 아예 시작되지 않는다. 그래서 LunarG Vulkan Loader(Apache-2.0)의 `vulkan-1.dll`을 실행 파일 옆에 함께 설치한다. 설치 경로는 **[S3]**에서 확인한다.
- **자동 선택 순서**:
  1. `settings.json`에 `gpu_failed=true`가 있으면 CPU를 쓴다.
  2. GPU 컨텍스트 생성을 시도하고, 실패하면 CPU로 바꾼다.
- **강제 설정**: 설정에서 백엔드를 "GPU"로 바꾸면 `gpu_failed`를 `false`로 되돌린다. "CPU"로 바꾸면 항상 CPU를 쓴다.
- **GPU 워커 크래시** (5.3 종료 코드 계약의 "크래시", 또는 kill하지 않았는데 결과 없이 끝남):
  - 같은 구간을 GPU 워커로 **한 번** 즉시 다시 실행한다.
  - 다시 비정상 종료하면 `settings.json`에 `gpu_failed=true`를 쓰고, 녹음자에게 "GPU 처리가 불안정해 CPU로 전환했습니다. 처리 시간이 길어집니다" 알림을 띄운 뒤, 같은 구간을 CPU 워커로 처리한다.
  - CPU 워커까지 비정상 종료하면 회의를 `transcribe_failed`로 바꾸고 원인(`worker.log` 마지막 줄)을 보여준다.
  - 앱 재시작은 필요 없다. 크래시 횟수는 처리 중 메모리에만 둔다.
- **비정상 결과 검사** (TRN-07): 구간 결과가 아래 중 하나면 그 구간을 CPU로 다시 처리한다.
  - VAD상 음성이 30초 넘게 있는데 전사 텍스트가 비어 있다.
  - 같은 토큰이 8번 넘게 연속으로 반복된다.

### 5.5 환각 필터 (TRN-08)

- 세그먼트 텍스트를 정규화한다: 앞뒤 공백 제거, 문장부호 제거, 연속 공백을 하나로.
- 정규화한 텍스트가 `resources/hallucinations.ko.txt`의 한 줄과 **정확히 같을 때만** 그 세그먼트를 버린다.
- 초기 목록 예: `시청해주셔서 감사합니다`, `구독과 좋아요 부탁드립니다`, `다음 영상에서 만나요`. 파일럿에서 발견되는 대로 추가한다.

## 6. 요약

### 6.1 요청

- `POST {base_url}/chat/completions`, `Authorization: Bearer {api_key}`, `temperature: 0.2`.
- **시간 제한** (eng review D12): 연결 10초, 응답 전체 300초(클라우드) / 900초(로컬). 연결마다 `timeout_secs`로 바꿀 수 있다. 넘으면 6.4의 "시간 초과"로 분류한다.
- 연결 정보는 여러 개 저장할 수 있다(예: "회사 클라우드", "내 Ollama"). 각 연결은 `settings.json`에 `name`, `base_url`, `model`, `context_tokens`, `is_local`, `timeout_secs`를 두고, `api_key`는 키체인에 둔다. `context_tokens`는 새 연결의 기본값이 8,192(보수적)이고, 쓰는 모델의 실제 컨텍스트 길이로 바꿔 입력한다. 작게 잡으면 구간 요약(6.3)이 불필요하게 늘어날 뿐 실패하지는 않는다.
- "로컬로 요약"을 켰을 때 쓰는 연결: 기본 연결(`default_llm`)이 로컬이면 그것을, 아니면 목록에서 첫 번째 `is_local=true` 연결을 쓴다. 로컬 연결이 하나도 없으면 토글을 보여주지 않는다(SUM-08).
- `is_local=true`인 연결로 요약 중일 때만 녹음 시작 시 요청을 취소한다(SUM-09). `reqwest` 요청 future를 drop하는 방식이다.

### 6.2 프롬프트

시스템 프롬프트(요지):

```
너는 회의록 작성자다. 아래 한국어 회의 전사본을 읽고, 회의에 없던 사람이 읽을 요약을 쓴다.
출력은 아래 Markdown 형식만 쓴다.

# {회의 제목: 20자 이내}
## 결정사항
- ...
## 액션 아이템
- [담당자] 할 일 (기한)
## 주요 논의
- ...
## 미해결 질문
- ...

규칙:
- 전사본에 명시되지 않은 담당자나 기한은 "미정"으로 쓴다. 추정하지 않는다.
- 해당 내용이 없는 섹션은 "- 없음"으로 쓴다.
- 용어집의 표기를 따른다: {용어집}
- 전사 오류로 보이는 단어는 문맥상 확실할 때만 고친다.
```

### 6.3 긴 회의 (SUM-04)

- 전사본 길이를 대략 `글자 수 ÷ 2`로 토큰 추정한다(한국어 기준 보수적 추정).
- 추정치가 연결의 `context_tokens`의 60%를 넘으면 구간별로 나눈다. 각 구간에 같은 형식의 부분 요약을 만들고, 부분 요약들을 합쳐 최종 요약을 만든다.
- **모든 요청에 예산을 둔다** (eng review outside voice #8). 요청마다 `입력 추정 + max_tokens ≤ context_tokens × 0.9`를 지킨다. 부분 요약의 `max_tokens`는 1,500으로 둔다.
- 부분 요약들을 합친 길이가 다시 `context_tokens`의 60%를 넘으면, 몇 개씩 묶어 한 번 더 요약하는 단계를 한도 안에 들어올 때까지 반복한다(재귀 축약).
- 부분 요약 중 하나라도 실패하면 요약 전체를 실패로 처리한다(SUM-06).

### 6.4 실패 분류 (SUM-06)

| 상황 | 판정 | 화면 메시지 |
|---|---|---|
| 클라우드 연결인데 키체인에 키가 없음 (요청 전에 확인) | 키 미설정 | 설정에서 API 키 입력 |
| 연결 실패, 시간 초과 | 네트워크 | 인터넷 연결 확인 |
| `is_local`이고 연결 거부 | 로컬 LLM 미실행 | Ollama·LM Studio 실행 확인 |
| 401, 403 | 키 문제 | 설정에서 API 키 확인 |
| 429 | 할당량 | 잠시 후 다시 시도 |
| 그 외 4xx, 5xx | 공급자 오류 | 응답 본문 일부 표시 |
| 긴 회의의 부분 요약 중 일부 실패 (6.3) | 위 분류 중 해당 원인 | 원인 메시지 + "구간 n/m에서 실패" |

자동 재시도는 네트워크 오류(시간 초과 포함)에 한해 1번만 한다. 시간 제한 값은 6.1, 7.1, 5.1에 있다. 응답하지 않는 로컬 스텁 서버로 각 시간 제한이 "시간 초과"로 분류되는지 단위 테스트한다.

### 6.5 빈 전사본 (SUM-10, eng review D8)

- 환각 필터를 거친 전사본이 30자 미만이면 LLM을 부르지 않는다. 빈 입력에서 LLM이 회의 요약을 지어내는 것을 막고 API 비용도 아낀다.
- 상태는 `review`로 두고 검토 화면에 "음성이 거의 감지되지 않았습니다"를 보여준다. 전송 버튼은 비활성화하고, 삭제와 "전송 안 함"만 남긴다.
- 단위 테스트: 빈 전사본과 짧은 전사본에서 LLM 요청이 나가지 않고 안내가 표시되는지 확인한다.

## 7. Slack 전송

### 7.1 페이로드

```json
{
  "text": "{제목} 회의 요약",
  "blocks": [
    { "type": "markdown", "text": "**{제목}**\n{일시} · {길이} · 녹음: {녹음자}\n\n{요약 본문}" },
    { "type": "context", "elements": [
      { "type": "mrkdwn", "text": "AI 요약 · 녹음자 검토 완료 (수정 {N}곳)" }
    ]}
  ]
}
```

- `text`는 알림 미리보기용 대체 텍스트다.
- 요청 시간 제한은 15초다(eng review D12).
- `markdown` 블록의 `text`(머리말 + 본문) 길이가 12,000자를 넘으면 전송하지 않는다(REV-02). 검토 화면의 글자 수는 이 값을 그대로 센다. 꼬리말은 별도 `context` 블록이라 세지 않는다.
- **[S5]** 웹훅이 `markdown` 블록을 거부하면 `section` 블록과 mrkdwn으로 바꾼다. 이때는 변환이 필요하다: `**굵게**` → `*굵게*`, 제목(`#`) → 굵은 줄, 링크 → `<url|text>`, `&` `<` `>` 이스케이프. 또 3,000자마다 section 블록을 나눈다.

### 7.2 수정 수 계산 (SND-03)

- `summary.generated.md`와 전송본을 줄 단위 LCS로 비교한다(직접 구현, 수십 줄).
- 추가·삭제·변경된 줄 수를 N으로 한다. 변경은 삭제와 추가가 붙어 있는 경우를 1로 센다.
- 제목이 바뀌었으면 1을 더한다. 결과는 `meta.json`의 `edit_count`에 기록한다.

## 8. 저장과 상태

### 8.1 폴더 구조

- `settings.json`은 앱 데이터 폴더(Tauri `app_data_dir`, Windows는 `%APPDATA%`)에 둔다.
- 용량이 큰 `models/`와 `meetings/`는 **로컬 앱 데이터 폴더**(Tauri `app_local_data_dir`, Windows는 `%LOCALAPPDATA%`)에 둔다. Windows Roaming 프로필 동기화에 녹음 파일과 GB 단위 모델이 섞이지 않게 하려는 것이다(M1 코드 리뷰). macOS는 두 경로가 같다.

```
{앱 데이터}/
  settings.json            비밀값 없는 설정
{로컬 앱 데이터}/
  models/                  5.1의 모델 파일
  meetings/
    2026-10-13T1400-a1b2/
      meta.json
      mic.wav
      system.wav
      transcript.partial.jsonl
      transcript.json
      summary.generated.md
      summary.md
```

### 8.2 meta.json

```json
{
  "schema_version": 1,
  "id": "2026-10-13T1400-a1b2",
  "title": "주간 개발 회의",
  "status": "review",
  "created_at": "2026-10-13T05:00:03Z",
  "duration_ms": 3512000,
  "gaps": [{ "at_ms": 1200000, "len_ms": 45000, "reason": "sleep" }],
  "backend": "gpu",
  "chunks_done": 6,
  "llm_connection": "회사 클라우드",
  "summarize_local": false,
  "error": null,
  "sent": { "channel": "#회의록", "at": "...", "edit_count": 3 }
}
```

- 시각은 UTC로 저장한다(`created_at`, id의 시각 부분). 화면에는 로컬 시각으로 보여 준다.
- 쓰기는 항상 임시 파일에 쓴 뒤 이름 바꾸기로 한다. 쓰는 도중 크래시가 나도 파일이 깨지지 않는다.
- **단일 쓰기 경로**: 기록 스레드, 처리 대기열, UI 명령이 모두 `meta.json`을 고친다. 그래서 모든 변경은 `store.rs`의 회의별 잠금(`Mutex`) 안에서 읽고-고치고-쓴다. 원자적 이름 바꾸기는 파일 손상을 막을 뿐, 서로의 변경을 덮어쓰는 것은 막지 못하기 때문이다.
- **스키마 버전** (STO-05, eng review D9): `meta.json`과 `settings.json`에 `schema_version`을 둔다. 앱이 아는 버전보다 높으면 그 데이터를 읽기 전용으로 열고 "새 버전 앱에서 만든 데이터입니다"를 보여준다. 낮으면 읽을 때 현재 버전으로 옮긴다.

### 8.3 상태 전이

```
recording ─▶ recorded ─▶ transcribing ─▶ transcribed ─▶ summarizing ─▶ review ─┬▶ sent
                              │  ▲                           │  ▲              └▶ archived
                              ▼  │                           ▼  │
                            paused / transcribe_failed     paused / summarize_failed
(어느 상태에서든) ─▶ 삭제
```

- 앱이 모르는 상태값이나 더 높은 `schema_version`의 `meta.json`은 `unknown` 상태의 읽기 전용 회의로 보여 준다(목록에서 사라지지 않게). 폴더 이름이 회의 id의 기준이다(`meta.json` 안의 `id`를 믿지 않는다).
- 실패 상태는 `transcribe_failed`와 `summarize_failed` 두 가지다. 설계 문서의 "각 단계에는 실패 상태가 있다"를 이렇게 구체화했다.
  - 녹음 단계의 문제(장치 끊김, 절전)는 실패가 아니라 `recorded`로 넘어간다.
  - 전송 실패는 상태가 아니라 `review`에 붙은 `error`로 기록한다. 다시 보낼 수 있어야 하기 때문이다.
- `transcribe_failed`, `summarize_failed`에서는 각각 `resume_meeting`, `retry_summary`(10.2)로 다시 시도한다.
- **삭제 절차** (REV-04, eng review outside voice #4):
  1. `meta.json` 상태를 `deleting`으로 기록한다.
  2. 진행 중인 작업을 취소한다. 녹음 중이면 녹음을 끝내 파일을 닫고, 워커가 돌고 있으면 kill하고, LLM 요청이 진행 중이면 취소한다.
  3. 그 작업들이 실제로 멈출 때까지 기다린다.
  4. 회의 폴더를 지운다. 폴더 안의 다른 파일을 먼저 지우고 `meta.json`을 마지막에 지운다. 중간에 실패해도 `deleting` 상태의 `meta.json`이 남아 다음 실행 때 복구가 삭제를 마저 한다(M1 코드 리뷰).
  - 대기열과 `store.rs`는 `deleting`이거나 이미 없는 회의에 대한 늦은 결과(워커 출력, LLM 응답, 기록 스레드 flush)를 모두 버린다. 쓰기 전에 회의 존재와 상태를 잠금 안에서 확인한다.
  - 앱이 4단계 전에 꺼지면, 다음 실행 때 복구 단계가 `deleting` 회의의 폴더를 마저 지운다.

### 8.4 처리 대기열과 시작 시 복구 (queue.rs, store.rs, lib.rs)

- 워커 스레드 하나가 대기열에서 회의를 꺼내 처리한다. 상태에 따라 전사 → 요약 순서로 진행한다.
- 녹음이 시작되면 진행 중인 전사 워커를 kill하고(5.3), 로컬 LLM 요약이면 요청도 취소한다. 녹음이 끝나면 같은 회의의 같은 구간부터 다시 진행한다.
- **단일 실행** (APP-06, eng review D14): `tauri-plugin-single-instance`를 빌더에 가장 먼저 등록한다. 두 번째 실행은 즉시 끝나고, 실행 중인 인스턴스가 메인 창을 보여 주고 앞으로 가져온다. 그래서 아래 복구는 항상 회의 폴더를 소유한 인스턴스 하나에서만 돈다.
- 앱 시작 시 `store.rs`의 복구 함수가 모든 `meta.json`을 읽어 다음을 한다.
  - `recording` → 디스크의 상태는 그대로 두고, 메모리에서 "중단된 녹음"으로 표시한다. 녹음자가 "처리 계속"을 고르면 `recorded`로 바꿔 대기열에 넣는다. 종료 확인(8.5)의 "진행 중인 작업"에는 세지 않는다. 실행 중에 녹음을 끝냈는데 `recorded`를 쓰지 못한 회의(디스크가 가득 참 등)도 "중단된 녹음"이 된다. 공간을 비운 뒤 "처리 계속"이 다시 쓴다.
  - `deleting` → 폴더 삭제를 마저 한다.
  - `transcribing`, `summarizing` → 앱 자체가 중간에 꺼진 경우다(워커 크래시와 무관). 디스크에 `paused`로 고쳐 쓴 뒤 대기열에 넣는다. 실제로 돌지 않는 작업을 디스크가 "진행 중"이라고 주장하지 않게 하려는 것이다.
  - `paused`, `recorded`, `transcribed` → 대기열에 넣는다.

### 8.5 앱 종료 처리 (APP-02, TRN-06)

Tauri 2는 종료 경로에 따라 보내는 이벤트가 다르다(tauri #9198, #14558).

| 종료 경로 | 이벤트 | 처리 |
|---|---|---|
| 트레이 메뉴 "종료", 직접 만든 앱 메뉴 "종료"(`CmdOrCtrl+Q` 단축키, `app.exit(0)` 호출) | `ExitRequested` → `Exit` | 진행 중인 녹음·처리가 있으면 `prevent_exit()` 후 확인을 받는다. 확인하면 `paused`를 기록하고 종료한다 |
| macOS Dock "종료", OS 종료·로그아웃 | `Exit`만 | 확인을 받을 수 없다. `Exit` 안에서 녹음 중이면 먼저 녹음을 끝내 두 WAV를 닫고 `recorded`를 쓴다(기록 스레드 한 주기, 수십 ms). 그다음 진행 중인 회의의 `meta.json`을 동기로 `paused`로 기록한다. 빠르게 끝나야 하므로 회의마다 파일 하나 쓰기만 한다 |
| 크래시, 강제 종료, 전원 차단 | 없음 | 다음 실행 때 8.4 복구와 5.4 비정상 종료 판정이 처리한다. 녹음은 5초 flush 덕분에 최대 5초만 잃는다 |

- macOS 기본 앱 메뉴의 "Quit" 항목은 `ExitRequested`를 보내지 않으므로, 기본 메뉴 대신 직접 만든 메뉴를 쓴다.
- 직접 만든 앱 메뉴(종료 `CmdOrCtrl+Q` + "편집" 하위 메뉴)는 **macOS에만** 둔다. 편집 메뉴가 없으면 macOS 입력란에서 복사·붙여넣기 단축키가 동작하지 않는다.
- Windows는 메뉴 막대를 두지 않는다. 대신 프론트엔드가 Ctrl+Q를 받아 `request_quit` 명령을 부르고, 이 명령이 `app.exit(0)`을 호출해 같은 `ExitRequested` 경로를 탄다.
- 종료 확인 뒤 다시 `ExitRequested`가 와도 막지 않도록 "종료 확정" 플래그를 둔다. 상태를 고칠 수 없는 회의(새 버전 스키마, 디스크 오류)가 있어도 종료가 무한 반복되지 않게 하려는 것이다(M1 코드 리뷰 P1).

## 9. 설정과 비밀값

### 9.1 settings.json (비밀값 없음)

```json
{
  "schema_version": 1,
  "recorder_name": "홍길동",
  "backend": "auto",
  "gpu_failed": false,
  "model": "large-v3-turbo-q8_0",
  "model_base_url": "https://huggingface.co",
  "input_device": null,
  "output_device": null,
  "llm_connections": [
    { "name": "회사 클라우드", "base_url": "https://api.example.com/v1", "model": "...", "context_tokens": 200000, "is_local": false, "timeout_secs": 300 }
  (`timeout_secs`가 없으면 읽을 때 클라우드 300, 로컬 900으로 채운다. 0이나 `context_tokens` 0은 저장할 때 거부한다)
  ],
  "default_llm": "회사 클라우드",
  "summarize_local_by_default": false,
  "webhooks": [{ "name": "#회의록" }],
  "last_channel": "#회의록",
  "glossary": ["{제품명}", "{고객사명}", "{사람 이름}"]
}
```

### 9.2 키체인

- 서비스 이름: `com.sije.meeting-summarizer`
- 비밀값 존재 확인(`has_secret`)은 값을 복호화하지 않는다. macOS에서는 `keyring-core`의 속성 전용 검색을 써서 키체인 접근 팝업을 피한다(`keyring-core`는 `keyring`과 같은 버전을 유지해야 한다).
- 키체인에 주인 없는 항목이 남지 않게 한다. 설정 화면에서 연결·웹훅을 지우면 확인을 받고, 그 키체인 항목은 "저장"이 성공한 뒤에만 지운다. 아직 저장되지 않은 연결·웹훅은 키 입력란이 비활성화되며 "설정을 저장한 뒤 입력하세요"를 보여 준다.
- 계정 이름: `llm:{연결 이름}`, `slack:{웹훅 이름}`
- 값: API 키, 웹훅 URL

### 9.3 연결 설정 입력 (ONB-04)

- 녹음자가 온보딩과 설정 화면에서 직접 입력한다: LLM 연결(`name`, `base_url`, `model`, API 키)과 Slack 웹훅(이름, URL).
- API 키와 웹훅 URL은 입력 즉시 키체인에 저장하고, 나머지는 `settings.json`에 저장한다.
- 설정 파일 가져오기는 v1에서 뺐다(eng review D3). 평문 비밀값 파일을 만들지 않기 위해서다. 키와 URL을 녹음자에게 어떻게 전달할지는 설계 문서 Open Question 3이다.

## 10. 프론트엔드

### 10.1 화면 전환

| 화면 | 들어오는 경로 |
|---|---|
| 온보딩 | 모델이나 연결 설정이 없을 때 |
| 홈 | 앱을 그냥 열 때, 다른 화면에서 돌아올 때 |
| 녹음 중 | 녹음 시작 |
| 처리 중 | 녹음 종료 직후, 홈에서 처리 중인 회의 선택 |
| 검토·전송 | 처리 완료 알림 클릭(해당 회의), 홈에서 회의 선택 |
| 설정 | 홈의 설정 버튼 |

### 10.2 IPC

| 명령 (React → Rust) | 이벤트 (Rust → React) |
|---|---|
| `start_recording` → `{ id, warnings }` (장치는 캡처 스레드에서 열므로 바로 돌아온다), `stop_recording` → id (두 WAV를 닫고 `recorded`가 된 뒤 돌아온다), `recording_state` → 진행 중인 녹음과 남아 있는 경고 | `recording://level` `{ elapsed_ms, mic, system, waiting_permission }` (100ms마다, 트랙별 RMS 0~1) |
| `list_meetings` (항목마다 `interrupted`: 중단된 녹음), `get_meeting`, `delete_meeting` (녹음 중이면 먼저 녹음을 끝낸다) | `recording://warning` `{ kind, message }` (`disk`, `mic_permission`, `mic_device`, `system_device`, `silence`, `length`, `sleep`. 같은 `kind`는 앞의 경고를 바꾼다) |
| `resume_meeting(id)`: 중단된 녹음의 "처리 계속", `transcribe_failed` 재시도(M3) | `recording://stopped` `{ id, message }` (녹음 종료. 스스로 끝난 경우 `message`에 이유) |
| `set_summarize_local(id, bool)` | `meeting://status` (상태 변경) |
| `save_summary(id, title, body)`, `archive_meeting(id)` | `meeting://progress` (구간 진행률, 대기열 위치) |
| `send_to_slack(id, webhook)` | `model://download` (진행률) |
| `retry_summary(id, connection)` | |
| `get_settings`, `save_settings`, `set_secret`, `has_secret`, `delete_secret` (비밀값 자체는 프론트엔드로 돌려주지 않는다) | `app://quit-requested` (진행 중 작업이 있을 때 종료 확인 요청, 8.5) |
| `confirm_quit` (종료 확인 후 진행 중 작업을 `paused`로 기록하고 종료), `request_quit` (Windows Ctrl+Q 경로) | |
| `reset_settings` (`settings.json`이 손상됐을 때만: 원본을 `settings.json.bak-<시각>`으로 옮기고 기본값으로 시작) | |
| `download_models`, `import_model(path)`, `test_system_audio`, `send_test_message` | |

### 10.3 색상 토큰

회사 서비스의 Tailwind 설정(monolis 공용 preset과 fni 확장)에서 앱이 쓰는 토큰만 **같은 키 이름으로** 복사한다. 원본에는 형식이 잘못된 값(`greenDark: "##1BA75C"`, `#`이 빠진 `warning900`·`success50`)이 있다. 이 토큰들은 지금 쓰지 않아 복사하지 않고, 나중에 쓰게 되면 고친 값으로 가져온다.

원본은 `pulse` 키프레임을 덮어써서 Tailwind 기본 `animate-pulse`의 동작까지 바꾼다. 이 앱에서는 키프레임 이름을 `pulse-faster`로 따로 두어 기본값을 건드리지 않는다.

`src/styles/theme.css`:

```css
@import "tailwindcss";

@theme {
  /* 회사 색상 토큰 (monolis 공용 preset + fni 확장에서 이식) */
  --color-mainBlue: #5550ff;
  --color-blue50: #f6f6ff;
  --color-blue100: #eeedff;
  --color-blue900: #4440cc;
  --color-danger: #f65959;
  --color-dangerDark: #dd5050;
  --color-danger100: #feeeee;
  --color-warning: #ff9b3f;
  --color-warning100: #fff5ec;
  --color-green: #1eba66;
  --color-green50: #f4fcf7;
  --color-mint: #5ec9e0;
  --color-black: #24232e;
  --color-white: #ffffff;
  --color-blueGray: #f9fafb;
  --color-cool200: #e3e9ef;
  --color-cool500: #a1aab2;
  --color-cool600: #838c94;
  --color-cool700: #69727a;

  --animate-pulse-faster: pulse-faster 1s cubic-bezier(0.4, 0, 0.6, 1) infinite;
  @keyframes pulse-faster {
    0%, 100% { opacity: 1; }
    50% { opacity: 0; }
  }
}
```

**shadcn 테마 변수 연결** (사용자 결정 2026-10-14: UI는 shadcn + Tailwind v4)

shadcn 컴포넌트는 `--primary` 같은 의미 변수를 쓴다. `theme.css`의 `:root`에서 이 변수들에 회사 색상 값을 넣고, shadcn 기본 `@theme inline` 매핑을 둔다(chart·sidebar 매핑은 쓰지 않아 뺀다). 회사 토큰 클래스(`bg-mainBlue` 등)도 함께 쓸 수 있다.

- **라이트 모드만**: `.dark` 변수 블록을 두지 않고, `@custom-variant dark (&:where(.dark, .dark *));`를 정의한다. 앱은 `.dark` 클래스를 쓰지 않으므로 생성된 컴포넌트의 `dark:` 클래스는 절대 적용되지 않는다. Tailwind 기본 `dark` 변형은 OS 다크 모드를 따르므로, 이 정의가 없으면 OS 다크 모드에서 화면이 반쯤 바뀐다. 이 방식이면 `shadcn add`로 컴포넌트를 추가할 때 손댈 것이 없다.
- **스캔 범위**: `@import "tailwindcss" source("../");`로 `src/`만 스캔한다. 그러지 않으면 `docs/`에 적힌 클래스명까지 CSS에 들어간다.
- **버튼 눌림·hover**: 주요 버튼은 `hover:bg-blue900`을 쓴다(흰 글자 7.4:1). shadcn 기본 `hover:bg-primary/80`은 3.7:1이라 쓰지 않는다.
- **포커스 표시**: 모든 조작 요소는 `ring`(mainBlue) 기반의 보이는 포커스 링을 둔다(WCAG 2.4.7).

| shadcn 변수 | 회사 토큰 |
|---|---|
| `--background` | `blueGray` #F9FAFB |
| `--foreground`, `--card-foreground`, `--popover-foreground` | `black` #24232E |
| `--card`, `--popover` | `white` |
| `--primary` / `--primary-foreground` | `mainBlue` / `white` |
| `--secondary`, `--accent` / 글자 | `blue50` / `black` |
| `--muted` / `--muted-foreground` | `blueGray` / `cool700` |
| `--border` (카드·구분선 등 장식용 경계) | `cool200` |
| `--input` (입력란·셀렉트·스위치처럼 조작 대상을 알아보는 데 필요한 경계) | `cool600` #838C94 (흰 배경 3.4:1, WCAG 1.4.11의 3:1 충족) |
| `--ring` | `mainBlue` |
| `--destructive` | `dangerDark` (아래 변형 규칙과 함께) |

- **`destructive` 버튼 변형은 바꾼다.** shadcn 기본(빨간 배경 + 작은 흰 글자)은 대비가 3.2:1이라 NFR-09에 어긋난다. 그래서 `danger100` 배경 + `dangerDark` 테두리·아이콘 + `black` 글자로 바꾼다. 녹음 종료 버튼만 아래 표의 예외(흰 글자, 굵게 18.66px 이상)를 쓴다.
- 외부 폰트(Google Fonts 등)를 불러오지 않는다. CSP가 `default-src 'self'`라 막히기 때문이다. 시스템 폰트를 쓰고, 필요하면 폰트 파일을 앱에 번들한다.
- Radix는 위치 계산에 인라인 style을 쓴다. CSP의 `style-src 'unsafe-inline'`로 허용된다.

**쓰임새와 대비 규칙** (NFR-09). 대비는 흰 배경 기준으로 계산했다.

| 용도 | 토큰 | 규칙 |
|---|---|---|
| 본문 글자 | `black` (15.5:1) | `danger100`·`warning100`·`green50` 배경 위에서도 13.7:1 이상 |
| 보조 글자 | `cool700` (4.9:1) | `blueGray` 배경 위에서도 4.7:1 |
| 비활성 글자 | `cool500` | 비활성 요소라 대비 기준 예외 |
| 주요 버튼 | 배경 `mainBlue`, 글자 `white` (5.3:1), 눌림 `blue900` | |
| 선택 상태 | 배경 `blue50` / `blue100` | |
| 녹음 중 표시 | `danger` 점 + `animate-pulse-faster` | 점에만 색을 쓴다 |
| 녹음 종료 버튼 | 배경 `danger`, 글자 `white` (3.2:1) | 큰 글자 기준(3:1)만 충족하므로 **굵게 14pt(18.66px) 이상** |
| 에러 배너 | 배경 `danger100`, 테두리·아이콘 `dangerDark`, 글자 `black` | `dangerDark` 글자는 3.9:1이라 쓰지 않는다 |
| 경고 배너, "미정" | 배경 `warning100`, 테두리·아이콘 `warning`, 글자 `black` | `warning` 글자는 2.1:1이라 쓰지 않는다 |
| 전송 완료 | 아이콘 `green`, 배경 `green50`, 글자 `black` | `green` 글자는 2.5:1이라 쓰지 않는다 |
| 레벨 미터 | 마이크 `mainBlue`, 시스템 오디오 `mint` | |
| 경계선 / 앱 배경 | `cool200` / `blueGray` | |

기존 템플릿의 `src/App.css`는 지우고 이 파일로 바꾼다.

### 10.4 LLM 출력 표시 안전 규칙 (eng review D6)

LLM 요약과 전사본은 외부에서 온 텍스트다. 웹뷰는 `send_to_slack`, `set_secret` 같은 명령을 부를 수 있으므로, 이 텍스트가 HTML이나 스크립트로 실행되지 않게 두 겹으로 막는다.

1. **HTML을 그리지 않는다.** 제목과 요약은 `<textarea>` 일반 텍스트로 편집한다. 미리보기가 필요하면 raw HTML을 허용하지 않는 Markdown 렌더러를 쓰고, `dangerouslySetInnerHTML`은 쓰지 않는다. 링크는 opener 플러그인으로만 연다.
2. **CSP를 켠다.** `src-tauri/tauri.conf.json`의 `app.security.csp`를 템플릿 기본값 `null`에서 아래로 바꾼다. 데스크톱 개발 모드(`tauri dev`)는 `devUrl`을 직접 불러와 CSP를 주입하지 않으므로, CSP 동작은 빌드된 앱에서 확인한다(`devCsp`는 데스크톱에서 효과가 없다).

```
default-src 'self'; script-src 'self'; connect-src ipc: http://ipc.localhost; img-src 'self' data:; style-src 'self' 'unsafe-inline'
```

- 단위 테스트: `<img src=x onerror=...>`가 든 요약이 텍스트로 표시되는지 확인한다.
- 릴리스 점검: 빌드된 설정에 CSP가 들어 있는지 확인한다.

## 11. 플랫폼별 설정

### 11.1 macOS

- **최소 버전**: `tauri.conf.json`의 `bundle.macOS.minimumSystemVersion`을 `"14.6"`으로 둔다.
- **`src-tauri/Info.plist`** (M2에서 추가. Tauri가 번들에 병합하고, `tauri dev`에서는 실행 파일에 내장한다):
  - `NSMicrophoneUsageDescription`: "회의를 녹음하기 위해 마이크를 사용합니다."
  - `NSAudioCaptureUsageDescription`: "화상 회의 상대방의 목소리를 녹음하기 위해 시스템 오디오를 사용합니다."
- **엔타이틀먼트** (Hardened Runtime): `com.apple.security.device.audio-input`.
- **개발 중 주의**: `tauri dev`에서는 권한 요청이 터미널 앱 이름으로 뜰 수 있다. 권한을 초기화하려면 `tccutil reset Microphone com.sije.meeting-summarizer`를 실행한다.
- **서명**: ad-hoc 서명은 빌드마다 권한을 다시 묻는다. 개발 중에도 Apple Development 인증서처럼 고정된 서명을 쓴다.

### 11.2 Windows

- **설치 파일**: NSIS, `installMode: "currentUser"`. 관리자 권한 없이 사용자 단위로 설치한다(NFR-06). 파일럿 동안 새 버전은 같은 설치 파일로 덮어 설치한다. MSI는 쓰지 않는다.
- **WebView2**: Tauri 기본값(`downloadBootstrapper`)을 그대로 쓴다. Windows 10/11 대부분에는 WebView2가 이미 설치돼 있다. 없는 PC에서는 WebView2 설치에 관리자 권한을 요구할 수 있다(NFR-06의 예외).
- **Vulkan 로더**: `vulkan-1.dll`을 동봉한다(5.4).
- **CPU**: AVX2가 없는 CPU는 전사가 4~10배 느리다(Meetily #306). 첫 실행 때 감지해서 "이 PC는 처리가 오래 걸릴 수 있습니다"를 안내한다.

## 12. 테스트 전략

### 12.1 자동 테스트

- **Rust**: `cargo test`. 하드웨어 없이 돌아가는 로직이 대상이다. 핵심 로직은 모두 여기서 검증한다.
- **프론트엔드**: Vitest + @testing-library/react + jsdom(eng review D10). `vite.config.ts`를 재사용하고 `bun run test`로 실행한다. 승인된 화면 증명(D6, D8), 검토 화면 보조 함수, 녹음 화면에만 쓴다. Radix 컴포넌트(Dialog, Select)를 jsdom에서 돌리기 위해 `src/test-setup.ts`에 `ResizeObserver`, `hasPointerCapture`, `releasePointerCapture`, `scrollIntoView` 보완 코드를 둔다.

| 대상 | 확인하는 것 |
|---|---|
| `clock.rs` 정렬 | 가짜 시계 값으로 무음 채우기, 넘친 샘플 버리기(경계값 포함), 절전 5분 이하 = 계속 / 초과 = 종료, 데이터 공백은 절전이 아님, 0.1% 빠른 장치와 느린 장치를 4시간 돌려도 두 트랙 차이 0.21초 이하 |
| `mix.rs` | 시스템 소리가 있을 때 마이크 감쇠, 클리핑 없음 |
| `writer.rs` 기록 경로 | 쓰기 오류 주입 시 상태 `recorded` + 마지막 flush 길이만큼 읽히는 WAV (D7), `recorded`를 못 쓰면 중단된 녹음, 5분 넘는 절전은 절전 시점에서 종료, 3시간 50분 경고와 4시간 종료, 마이크 3초 0버퍼 경고, 48k 스테레오·44.1k 모노 → 16kHz 길이와 주파수 |
| `filter.rs` | 정확히 일치할 때만 제거, 비슷한 문장은 남김, 반복 토큰·빈 결과 검사 |
| `queue.rs` + 가짜 워커 | 크래시 → GPU 1회 재시도 → `gpu_failed` → CPU (D5), 종료 코드 3 → `gpu_failed` 아님 (5.3), 녹음 시작 → kill → 같은 구간 재처리, 마지막 완료 구간 다음부터 재개. 가짜 워커는 종료 코드만 내는 작은 실행 파일이고, `queue.rs`는 실행할 명령을 생성자로 받는다 |
| `slack.rs` | 페이로드 형식, 12,000자 경계, mrkdwn 변환 |
| 수정 수 계산 | 줄 추가·삭제·변경, 제목 변경 |
| `store.rs` | 상태 전이, 원자적 쓰기, 동시 갱신 두 개가 모두 남음 (8.2), 복구 분류 (8.4), 중단된 녹음의 처리 계속·삭제 (STO-03), 높은 `schema_version` → 읽기 전용 (D9) |
| `summarize.rs` | 토큰 추정과 구간 나누기, 실패 분류 (6.4), 30자 미만 → LLM 요청 없음 (D8) |
| `model.rs` | 해시 불일치 시 `.part` 삭제 |
| 검토 화면 (Vitest) | `<img src=x onerror=...>`가 든 요약이 텍스트로 표시됨 (D6), 빈 녹음 안내와 전송 버튼 비활성화 (D8), "미정" 강조 |
| 녹음 화면 (Vitest) | 레벨 이벤트로 경과 시간·레벨 미터 표시, 경고 배너 닫기와 같은 종류가 다시 오면 다시 표시 |

### 12.2 수동 점검 (릴리스마다)

| 시나리오 | macOS | Windows |
|---|---|---|
| 대면 회의 10분 녹음 → 전송 | ✓ | ✓ |
| 화상 회의(스피커) 10분 녹음, 중복 문장 확인 | ✓ | ✓ |
| 녹음 중 헤드셋 연결·분리 | ✓ | ✓ |
| 녹음 중 덮개 닫기 1분 / 6분 | ✓ | ✓ |
| 녹음 중 앱 강제 종료 → 재실행 복구 | ✓ | ✓ |
| 처리 중 새 녹음 시작 → 전사 즉시 중단 확인 | ✓ | ✓ |
| **서명·공증된 배포 빌드**에서 마이크·시스템 오디오가 0이 아닌지 | ✓ | - |
| Vulkan 런타임이 없는 PC에서 실행 | - | ✓ |
| 녹음 중 앱을 한 번 더 실행 → 인스턴스 하나, 녹음 영향 없음 (D14) | ✓ | ✓ |

### 12.3 요약 프롬프트 평가 (eng review D11)

- 고정 전사본 3개를 `src-tauri/tests/fixtures/`에 둔다. M0 녹음에서 만들고, 사람 이름·회사 정보는 익명화한다.
  - 담당자가 언급되지 않은 액션 아이템이 있는 회의
  - 용어집 단어가 나오는 회의
  - 약 1시간 길이의 회의(구간 요약 경로 확인)
- `cargo test -- --ignored summary_eval`로 실행한다. 설정된 LLM을 실제로 호출하므로 평소 테스트에서는 빠진다.
- 형식 검사: 다섯 섹션이 모두 있음, 제목 20자 이하, 담당자가 언급되지 않은 항목은 "미정", 전사본과 용어집에 없는 사람 이름이 나오지 않음.
- **내용 검사** (eng review D17): 전사본마다 `expected.json`을 손으로 만든다.
  - 꼭 들어가야 할 결정과 할 일: 키워드 묶음으로 적고, 해당 섹션에 모두 나와야 한다.
  - 들어가면 안 되는 것: 기각된 제안 하나와 부정문("~하지 않기로") 하나 이상이 "결정사항"에 나오면 실패다.
  - 회의 중에 뒤집힌 결정 하나: 최종 상태만 나와야 하고, 뒤집히기 전 내용이 결정으로 나오면 실패다.
  - 1시간 전사본에는 구간 경계에 걸친 결정을 하나 넣어 구간 요약과 재귀 축약(6.3)을 검사한다.
  - 키워드 검사라 표현이 크게 바뀌면 거짓 실패가 날 수 있다. 실패하면 사람이 출력을 읽고 키워드와 요약 중 어느 쪽을 고칠지 판단한다.
- 실행 시점: M4 끝, 그리고 프롬프트나 LLM 모델을 바꿀 때마다. 실행마다 API 비용이 몇 센트 든다.

## 13. 차용 코드

- 차용 대상: Meetily(`Zackriya-Solutions/meetily`), Handy(`cjpais/Handy`). 둘 다 MIT 라이선스다.
- 차용한 파일 맨 위에 출처 URL, 커밋 해시, 라이선스를 주석으로 남긴다.
- 루트에 `THIRD_PARTY_NOTICES.md`를 두고 함께 배포하는 서드파티의 라이선스 전문을 모은다.
  - MIT: Meetily, Handy, whisper.cpp, Silero VAD
  - Apache-2.0: Vulkan Loader(`vulkan-1.dll`)
  - 그 외 Rust·npm 의존성 라이선스는 빌드 시 도구로 생성한다(예: `cargo about`).
