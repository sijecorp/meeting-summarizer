# 회의 요약 (meeting-summarizer)

회의를 녹음하고 전사·요약해서 Slack으로 보내는 데스크톱 앱이다(Tauri 2, React, Rust). 동료용 설치 안내는 [docs/install.md](docs/install.md), 설계는 [docs/architecture.md](docs/architecture.md)에 있다.

## 다운로드

**[최신 버전 받기 (GitHub Releases)](https://github.com/sijecorp/meeting-summarizer/releases/latest)**

| 컴퓨터 | 파일 |
|---|---|
| Mac (Apple 실리콘: M1 이후, macOS 14.6 이상) | `meeting-summarizer_<버전>_aarch64.dmg` |
| Windows 10(1809 이상)/11 (64비트) | `meeting-summarizer_<버전>_x64-setup.exe` |

설치와 첫 실행(Mac의 "그래도 열기", Slack 로그인)은 [설치 안내](docs/install.md)를 따른다. 저장소가 비공개라 접근 권한이 있어야 받을 수 있다. 지난 버전은 [모든 릴리스](https://github.com/sijecorp/meeting-summarizer/releases)에 있다.

## 느릴 때 보낼 것

처리가 느리거나 실패한다는 동료에게는 두 파일을 받는다.

- `app.log`: 앱의 설정 > 일반 > "로그 폴더 열기"가 여는 폴더에 있다. 전사 구간마다 처리 장치, GPU 이름, 스레드·beam·flash attention, 걸린 시간, 실시간 배수(`rtf`), 전원 상태가 `chunk device=… rtf=… power=…` 한 줄로 남고, 회의의 전사가 끝나면 `meeting id=…` 한 줄, 모델을 받으면 `download file=… bps=…` 한 줄이 남는다.
- `settings.json`: macOS `~/Library/Application Support/com.sije.meeting-summarizer/settings.json`, Windows `%APPDATA%\com.sije.meeting-summarizer\settings.json`. 전사 백엔드, GPU 실패로 CPU에 묶였는지(`gpu_failed`), flash attention(`transcribe_flash_attn`)이 여기 있다. API 키와 Slack 로그인은 키체인에 있어 이 파일에 없다.

## 전사 실측

설치한 앱의 실행 파일로 그 PC의 전사 속도와 정확도를 잰다(계획 5.1-14). 개발 환경은 필요 없다. 처리를 마친 회의 하나의 앞 10분을 설정 조합마다 전사하고, 조합 하나가 끝날 때마다 그 회의 폴더의 `bench.tsv`에 한 줄을 쓴다. 중간에 끊어도 끝난 줄은 남는다. 녹음은 임시 폴더에 복사해서 돌고, 회의 폴더에는 `bench.tsv`만 쓴다.

1. 앱을 끄고 시작한다(메뉴 막대나 트레이의 앱 아이콘 > 종료). 앱이 전사하는 동안 재면 둘이 CPU·GPU를 나눠 써서 숫자가 틀어진다.
2. 잴 회의의 폴더를 찾는다. 폴더 이름이 회의 id(녹음을 시작한 UTC 시각)다.
   - macOS: `~/Library/Application Support/com.sije.meeting-summarizer/meetings/<id>`
   - Windows: `%LOCALAPPDATA%\com.sije.meeting-summarizer\meetings\<id>`
3. 먼저 `--smoke`(제품 기본 조합 하나)로 도는지 본 뒤, `--smoke`를 빼고 모든 조합을 돈다. 시작할 때 예상 시간을 `bench.tsv` 첫 줄에 쓴다. GPU 없는 노트북에서는 모든 조합이 몇 시간 걸릴 수 있다.

macOS:

```sh
dir="$HOME/Library/Application Support/com.sije.meeting-summarizer/meetings/<id>"
/Applications/meeting-summarizer.app/Contents/MacOS/meeting-summarizer --transcribe-bench "$dir" --smoke
```

Windows(PowerShell). 설치한 앱은 콘솔이 없는 프로그램이라 `Start-Process -Wait`로 끝날 때까지 기다리고, 진행과 오류 메시지는 `bench.log`로 받는다.

```powershell
$dir = "$env:LOCALAPPDATA\com.sije.meeting-summarizer\meetings\<id>"
Start-Process -Wait -FilePath "$env:LOCALAPPDATA\meeting-summarizer\meeting-summarizer.exe" -ArgumentList "--transcribe-bench `"$dir`" --smoke" -RedirectStandardError "$env:TEMP\bench.log"
Get-Content "$env:TEMP\bench.log" -Tail 3
```

- 기준 전사본이 있으면 `--reference <파일>`을 더해 CER(글자 오류율)도 잰다.
- 다른 구간은 `--range 600000-1200000`(ms), 일부 조합만은 `--only <조합,...>`(조합 이름은 `bench.tsv` 첫 열), 한국어가 아닌 회의는 `--lang en`처럼 준다.
- 모델은 설치한 앱의 모델 폴더에서 `large-v3-turbo-q8_0`과 `medium-q5_0` 중 받아 둔 것을 쓴다. 다른 파일은 `--model <파일> --vad <파일>`로 준다.

**기준 전사본 만들기**: 같은 구간(기본은 앞 10분)을 사람이 들으며 고친 UTF-8 텍스트 파일이다. 회의 문서의 전사본에서 "전사본 복사"를 눌러 메모장 같은 편집기에 붙이고, 녹음을 들으며 틀린 글자를 고친 뒤 `[10:00]` 뒤의 줄을 지운다. 줄 앞의 `[분:초]`, 줄바꿈, 띄어쓰기, 문장부호는 CER에 세지 않으므로 그대로 둬도 된다. 파일은 회의 폴더 밖에 저장한다.

**`bench.tsv` 읽기**: 탭으로 나눈 글 파일이라 스프레드시트에 붙여 넣으면 열로 나뉜다. 실행할 때마다 `#` 머리 줄(앱 버전, 잰 구간, 조합 수, 예상 시간)과 열 이름 줄이 새로 붙는다. 열 이름 다음 행이 제품 기본값이고, 그 아래로 설정 하나만 바꾼 조합, 둘을 바꾼 조합 순이다. 전원을 연결했을 때와 배터리일 때는 따로 돌려 따로 비교한다.

| 열 | 뜻 |
|---|---|
| `combo` | `<장치>/<모델>/b<beam>/fa<0·1>/<half·cores>`. `--only`에 그대로 쓴다. half는 논리 CPU의 절반(지금 제품 값), cores는 물리 코어 수다 |
| `device`, `gpu` | 실제로 돈 장치와 찾은 GPU 이름. GPU를 쓰지 못하면 gpu 조합도 `cpu`다 |
| `threads`, `beam`, `fa` | 그 조합의 스레드 수, beam 크기(1은 greedy), flash attention |
| `audio_s`, `speech_s` | 잰 구간의 길이와 그중 음성(초) |
| `run_s`, `rtf` | 전사에만 걸린 시간과 그것을 구간 길이로 나눈 값. 작을수록 빠르고, 1이면 회의 길이만큼 걸린다 |
| `load_s`, `wall_s` | 모델을 불러오는 데 걸린 시간, 워커를 띄워 끝날 때까지 걸린 전체 시간 |
| `cer_pct` | 기준 전사본과 다른 글자의 비율(%). 기준 전사본이 없거나 실패한 조합은 빈칸 |
| `power`, `saver`, `ecoqos` | 전원(`ac`·`battery`), 절전 모드(`1`·`0`), Windows 효율 모드를 끈 결과. macOS는 `?`와 `-` |
| `status` | `ok`, 아니면 종료 코드와 워커의 마지막 오류 줄 |

## 개발 시작

화면만 보려면 Bun만 있으면 된다(아래 "가장 빠른 길").

### 준비물

모든 OS에 [Bun](https://bun.sh), [Rust](https://rustup.rs)(stable), Git, CMake가 필요하다.

macOS (Apple 실리콘):

```sh
xcode-select --install
brew install cmake
```

Windows x64:

```powershell
winget install Git.Git Kitware.CMake Ninja-build.Ninja KhronosGroup.VulkanSDK Microsoft.PowerShell
winget install Microsoft.VisualStudio.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"
```

Windows에서는 아래 명령을 모두 x64 MSVC 개발자 환경에서 돌린다("x64 Native Tools Command Prompt for VS"를 열고 `pwsh`). CI와 같은 값도 먼저 준다(아키텍처 11.3). Visual Studio 생성기로는 ggml-vulkan의 하위 빌드가 컴파일러를 찾지 못하고, `src-tauri\target` 아래에서는 경로가 MSVC의 260자 한계를 넘는다. `vulkan-1.dll`은 `tauri.windows.conf.json`이 설치 파일에 넣는 파일이라 개발 빌드에도 있어야 한다.

```powershell
$env:CMAKE_GENERATOR_x86_64_pc_windows_msvc = 'Ninja'
$env:CARGO_TARGET_DIR = 'C:\t'
Copy-Item C:\Windows\System32\vulkan-1.dll src-tauri\vulkan-1.dll
```

### 가장 빠른 길: 화면만 보기

```sh
bun install
bun run dev
```

http://localhost:1420/preview.html 을 연다. IPC를 가짜로 바꾼 미리보기라 Rust를 빌드하지 않고 모든 화면과 상태를 본다(디자인 시스템 7).

### 앱 실행

```sh
bun run app:dev
```

요약 엔진(`llama-server` 사이드카)이 없거나, 빈 자리표시 파일이거나, 도장이 지금 커밋·옵션과 다르면 먼저 빌드한 뒤 `tauri dev`를 띄운다. 첫 빌드는 몇 분 걸리고, 그 뒤에는 캐시에서 복사만 한다. `bun run app:dev --check`는 사이드카 상태만 알려 준다.

### Slack 앱

Slack 로그인에는 회사 워크스페이스의 Slack 앱이 필요하다. 만드는 법과 매니페스트는 [docs/slack-app.md](docs/slack-app.md)에 있고, 그 Client ID를 `src-tauri/src/slack.rs`의 `CLIENT_ID`에 넣는다. 비어 있으면 앱은 "이 빌드에는 Slack 앱이 설정되지 않았습니다"로 로그인을 거부한다(나머지는 그대로 된다).

### 요약 엔진 사이드카

llama.cpp의 `llama-server`를 정적 링크로 빌드해 `src-tauri/binaries/llama-server-<target-triple>`에 둔다. git에는 넣지 않는다. CI도 같은 스크립트를 부른다.

```sh
scripts/build-llama-server.sh
```

```powershell
pwsh -File scripts\build-llama-server.ps1
```

- llama.cpp 커밋은 `scripts/llama.cpp.rev`, CMake 옵션은 `scripts/llama-server.flags` 한 곳에만 있다.
- 결과는 저장소 밖 캐시에 커밋·대상·옵션별로 남는다(macOS `~/Library/Caches/meeting-summarizer/llama-server`, Windows `%LOCALAPPDATA%\meeting-summarizer-dev\llama-server`, 앱 설치 폴더와 겹치지 않게). 다른 worktree는 복사만 한다. 캐시 폴더는 `MS_LLAMA_SERVER_CACHE`로 바꾼다.
- Windows는 MSVC의 260자 한계 때문에 `C:\t\lls`에서 빌드하고, 다 되면 그 폴더를 지운다. 다른 드라이브를 쓰려면 `MS_LLAMA_SERVER_WORK`에 상위 폴더를 준다(그 아래 `lls`만 쓰고 지운다).
- 바이너리 옆의 도장(`llama-server-<target-triple>.stamp`)에 커밋과 옵션 해시가 있다. 사이드카가 없으면 디버그 빌드(`cargo check`, `cargo test`, rust-analyzer)는 빈 자리표시 파일을 만들고 경고만 한다. 릴리스 빌드는 자리표시이거나 도장이 없거나 다르면 멈춘다.

### 검사

```sh
bun run test
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

### 요약 평가

`src-tauri/tests/fixtures`의 전사본을 실제 LLM으로 요약해 채점한다(아키텍처 12.3). 이 PC 요약 엔진으로 돌리려면 사이드카와 요약 모델 파일(`Qwen3-4B-Instruct-2507-Q4_K_M.gguf`)이 있어야 한다. 아래 경로는 앱이 받아 둔 모델이다.

```sh
SUMMARY_EVAL_BUILTIN=1 SUMMARY_EVAL_MODEL_PATH="$HOME/Library/Application Support/com.sije.meeting-summarizer/models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf" cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored --exact summarize::tests::summary_eval --nocapture
```

```powershell
$env:SUMMARY_EVAL_BUILTIN = "1"; $env:SUMMARY_EVAL_MODEL_PATH = "$env:LOCALAPPDATA\com.sije.meeting-summarizer\models\Qwen3-4B-Instruct-2507-Q4_K_M.gguf"; cargo test --manifest-path src-tauri/Cargo.toml --lib -- --ignored --exact summarize::tests::summary_eval --nocapture
Remove-Item Env:SUMMARY_EVAL_BUILTIN, Env:SUMMARY_EVAL_MODEL_PATH
```

PowerShell의 `$env:` 값은 창을 닫을 때까지 남으므로 마지막 줄로 지운다.

- 컨텍스트는 제품과 같은 16,384다. `SUMMARY_EVAL_CONTEXT_TOKENS`로 바꾸면 서버의 `-c`도 같이 바뀐다. `SUMMARY_EVAL_DEVICE=cpu`면 CPU로 돈다.
- 전사본마다 엔진 준비·요약 시간, 입력·출력 초당 토큰, 최대 메모리(macOS만, Windows는 "측정 안 함"), 생각 모드가 꺼졌는지, 근거 검사 결과를 찍는다. 서버 출력은 임시 폴더의 `summary-eval-logs/llama-server.log`에 남는다.
- 클라우드나 Ollama 같은 연결로 돌리는 법은 `src-tauri/tests/fixtures/README.md`에 있다.

### 설치 파일 빌드

```sh
bun run tauri build
```

사이드카가 먼저 있어야 한다. 동료에게 줄 설치 파일은 GitHub Actions의 build 워크플로(수동 실행이나 `v*` 태그)가 macOS dmg와 Windows setup.exe로 만든다(아키텍처 11.3).

### worktree를 여러 개 쓸 때

worktree들은 같은 앱으로 실행된다(identifier `com.sije.meeting-summarizer`). 그래서 아래를 함께 쓴다.

- 앱 데이터 폴더: 회의·모델·설정(macOS `~/Library/Application Support/com.sije.meeting-summarizer`, Windows 회의·모델은 `%LOCALAPPDATA%`, 설정은 `%APPDATA%` 아래 같은 이름)
- 키체인 항목(LLM 키, Slack 로그인 토큰)
- Vite 포트 1420: 두 번째 `bun run dev`·`app:dev`는 포트가 겹쳐 시작하지 않는다
- Slack 로그인 포트 47815: 로그인을 기다리는 앱이 둘이면 나중 것이 포트를 열지 못한다
- 단일 실행 검사: 앱이 이미 떠 있으면 두 번째 앱은 첫 창을 앞으로 가져오고 꺼진다

따로 쓰게 하는 일은 `docs/TODOS.md`의 "worktree 격리"에 있다.
