# 회의 요약 (meeting-summarizer)

회의를 녹음하고 전사·요약해서 Slack으로 보내는 데스크톱 앱이다(Tauri 2, React, Rust). 동료용 설치 안내는 [docs/install.md](docs/install.md), 설계는 [docs/architecture.md](docs/architecture.md)에 있다.

## 다운로드

**[최신 버전 받기 (GitHub Releases)](https://github.com/sijecorp/meeting-summarizer/releases/latest)**

| 컴퓨터 | 파일 |
|---|---|
| Mac (Apple 실리콘: M1 이후, macOS 14.6 이상) | `meeting-summarizer_<버전>_aarch64.dmg` |
| Windows 10/11 (64비트) | `meeting-summarizer_<버전>_x64-setup.exe` |

설치와 첫 실행(Mac의 "그래도 열기", Slack 로그인)은 [설치 안내](docs/install.md)를 따른다. 저장소가 비공개라 접근 권한이 있어야 받을 수 있다. 지난 버전은 [모든 릴리스](https://github.com/sijecorp/meeting-summarizer/releases)에 있다.

## 개발 시작

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
