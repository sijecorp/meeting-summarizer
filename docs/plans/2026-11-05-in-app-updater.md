<!-- /autoplan restore point: "/Users/kangjae4real/sije/meeting-summarizer/.gstack/tmp/autoplan/master-autoplan-restore-20261105-114054.md" -->
## Implementation plan
# 계획: 앱 안 업데이터와 버전 표시 (1.2.0)

- 상태: 승인 (2026-11-06, /autoplan 두 번째 관문), 구현 전. 구현 첫 커밋은 10·11장을 본문에 접는 일이다
- 관련: [TODOS](../TODOS.md) "파일럿 후 자동 업데이트"·"Windows 코드 서명 검토", [요구사항](../requirements.md) "v1에서 제외"와 APP-05, [아키텍처 8.5·11](../architecture.md), [설치 안내](../install.md), [설계 문서](../designs/meeting-summarizer.md) Distribution Plan·Open Question 1·2

## 1. 왜

지금은 새 버전이 나오면 설치 파일을 Slack 채널과 OneDrive로 나누고, 동료가 직접 받아 덮어 설치한다(install.md "새 버전으로 업데이트"). 1.0.0(10/20)부터 1.1.1(11/04)까지 2주 남짓 동안 릴리스가 여섯 번 나왔다. 손으로 업데이트하지 않은 사람은 고친 버그를 받지 못하고, Mac은 업데이트마다 "그래도 열기"와 권한 재허용을 다시 한다. 앱이 새 버전을 스스로 받아 설치하게 하고, 설정에서 지금 버전을 보이게 한다.

## 2. 사용자 요청과 결정 (2026-11-05)

| # | 요청 | 메모 |
|---|---|---|
| 1 | 앱 안 업데이터(In-app Updater) | |
| 2 | 설정 화면에 현재 앱 버전 | |
| 3 | 서명·공증(Apple Developer, Windows Authenticode)을 조사하고 어떻게 풀지 정해 구현 | |

| 결정 | 내용 |
|---|---|
| U1 업데이트 위치 | 공개 releases 전용 저장소 `sijecorp/meeting-summarizer-releases`. 소스 저장소는 비공개 그대로 둔다. 조직은 Team 요금제이고 관리자 권한, 공개 저장소 생성 권한이 있다(2026-11-05 `gh api` 확인). 설계 문서 Open Question 2를 닫는다 |
| U2 서명 비용 | 돈을 들이지 않는다. Apple Developer Program과 Windows 코드 서명 인증서를 쓰지 않는다. 첫 설치의 "그래도 열기"(Mac)와 "Windows의 PC 보호"(Windows)는 받아들이고, 그 뒤 버전은 앱 안 업데이터로 받는다 |
| U3 기록 | 이 계획은 eng review D2(2026-10-14, 자동 업데이트를 파일럿 뒤로)를 사용자 요청(2026-11-05)으로 뒤집는다. 근거: 2주 남짓 동안 6회 릴리스, 업데이트마다 Mac 권한 재허용. 설계 문서 전제 5(Developer ID 필요)는 U2로 대체한다. M7 파일럿 진행 여부는 로드맵에 기록이 없어 확인이 필요하다. 파일럿 1주차의 SmartScreen·백신 차단 수 세기(TODOS)는 그대로 둔다 |
| U2 파생 | Mac은 무료로 서명 정체성을 고정하는 자체 서명 코드 서명 인증서를 쓴다(3.2). 실측(6장 2단계)이 실패하면 ad-hoc 서명으로 두고 업데이트 뒤 권한 재허용 안내를 넣는다 |

## 3. 조사 요약 (2026-11-05)

### 3.1 서명은 두 가지다

- **OS 코드 서명**: macOS Developer ID + 공증, Windows Authenticode. OS가 앱을 믿게 하는 것이고 돈이 든다.
- **업데이터 서명**: Tauri 업데이터는 업데이트 파일마다 우리가 만든 키(`tauri signer generate`)의 서명을 확인한다. 끌 수 없고 무료다. 개인 키를 잃으면 이미 설치된 앱에 더는 업데이트를 보낼 수 없어서 모두 손으로 다시 설치해야 한다(Tauri 문서).
- 업데이터는 OS 코드 서명 없이도 돈다. OS 코드 서명은 첫 설치 경고, 백신 오탐, macOS 권한 유지에 영향을 준다.

### 3.2 macOS

- **Developer ID + 공증(쓰지 않음, U2)**: Apple Developer Program 조직 회원 $99/년, D-U-N-S 번호 필요, 등록까지 2~3주(보고). "Developer ID Application" 인증서로 서명하고 `notarytool`로 공증한다. Tauri는 `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`(또는 `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`)를 읽는다([Tauri macOS 서명](https://v2.tauri.app/distribute/sign/macos/)). 나중에 계정이 생기면 워크플로의 주석 처리된 `APPLE_*`를 채우면 된다.
- **ad-hoc 서명은 업데이트마다 권한을 잃는다**: 이 Mac에 설치된 1.1.1의 지정 요구사항(designated requirement)은 `cdhash H"1345aa4c…"`다(`codesign -d -r-`, 2026-11-05). 빌드마다 바뀌는 해시라 macOS는 새 버전을 다른 앱으로 본다. Apple DTS는 ad-hoc 코드는 안정된 지정 요구사항이 없어 업데이트 뒤 화면 기록 권한을 잃는다고 답했다([forums 795739](https://developer.apple.com/forums/thread/795739), TN3127). 마이크, "화면 및 시스템 오디오 녹음", 키체인 항목 허용이 모두 같다. install.md의 "Mac에서 업데이트 뒤 소리가 녹음되지 않을 때"가 이 증상이다. 자동 업데이트로 릴리스가 잦아지면 상대방 목소리 없이 녹음되는 회의가 매번 생길 수 있다.
- **자체 서명 인증서는 지정 요구사항을 고정한다(실측)**: openssl로 만든 자체 서명 인증서(`extendedKeyUsage=codeSigning`, 신뢰 설정 없음, `CSSMERR_TP_NOT_TRUSTED`)로 서로 다른 두 빌드를 `codesign --options runtime`으로 서명했다. 둘 다 `identifier "com.sije.selfsign-test" and certificate root = H"ceb9d571…"`였고 `codesign --verify --strict`를 통과했다(2026-11-05, scratchpad 실험). 같은 인증서로 서명하는 한 macOS는 업데이트 전후를 같은 앱으로 볼 근거가 있다. TCC가 실제로 권한을 유지하는지는 6장 2단계에서 확인한다(yabai 같은 도구가 같은 이유로 자체 서명을 안내한다, 보고).
- **Tauri에서 자체 서명하는 법**: `APPLE_CERTIFICATE`로 넘기면 Tauri가 임시 키체인에 넣은 뒤 이름이 "Developer ID Application:" 같은 Apple 접두어로 시작하고 OU가 있는 인증서만 찾는다(`tauri-macos-sign` `keychain/identity.rs` `list`). 자체 서명 인증서는 거기서 잡히지 않는다. 대신 CI가 직접 임시 키체인을 만들어 넣고(`security create-keychain`, `import -T /usr/bin/codesign`, `set-key-partition-list`, `list-keychains -s`) `APPLE_SIGNING_IDENTITY`에 인증서 이름만 주면 Tauri는 `codesign --force -s <이름> --options runtime`을 부른다(`with_signing_identity`). `--timestamp`를 붙이지 않으므로 Apple 타임스탬프 서버도 필요 없다. 사이드카(`llama-server`)도 같은 신원으로 서명된다(지금 ad-hoc도 그렇다, build.yml 검사).
- **격리 속성**: Slack에서 받은 1.1.1에는 `com.apple.quarantine: …;Slack;`이 붙어 있다(확인). 첫 설치의 "그래도 열기"는 이 속성 때문이다. 업데이터는 앱 안의 HTTP 클라이언트로 받고 직접 푸므로 이 속성이 붙지 않아 업데이트 뒤에는 Gatekeeper가 다시 묻지 않을 것으로 본다(추정, 6장 2단계에서 `xattr`로 확인).
- **업데이터의 macOS 설치**: `.app.tar.gz`와 `.sig`를 받아 번들을 바꾼다. plugin 2.13.2부터 설치가 실패해도 기존 앱을 지우지 않는다(릴리스 노트). macOS 13부터의 App Management 보호가 Team ID 없는 앱의 자기 번들 교체를 막거나 묻는지는 찾지 못했다(6장 2단계에서 확인). 교체는 같은 볼륨 안의 rename으로 한다(plugin `updater.rs` macOS `install_inner`).
- **사용자 계정이 관리자가 아닐 때**: `/Applications`에 쓰지 못하면 교체가 실패한다. 실패하면 오류와 릴리스 페이지 링크를 보인다(5.4).

### 3.3 Windows

- **Authenticode 선택지(쓰지 않음, U2)**: Microsoft Artifact Signing(옛 Trusted Signing)은 2026-10-08 문서 기준 한국 조직도 대상이고 Basic 약 $9.99/월, 유료 Azure 구독과 본인 확인 1~20 영업일이 필요하다([quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart)). CA OV 인증서는 2023년부터 키를 HSM에 두어야 해서 클라우드 HSM(SSL.com eSigner, Certum 등, 3년 약 $350~870, 보고)을 쓰고, 2026-03-01부터 발급 인증서 유효기간이 최대 460일이다(CA/B CSC-31, 보고). 2024년부터 EV도 SmartScreen 평판을 바로 주지 않는다([Tauri Windows 서명](https://v2.tauri.app/distribute/sign/windows/)). 결정이 바뀌면 Tauri `bundle.windows.signCommand`로 붙이고 `vulkan-1.dll`은 따로 서명한다. 이 조사는 TODOS "Windows 코드 서명 검토"에 옮긴다.
- **업데이터의 Windows 설치**: `createUpdaterArtifacts: true`면 NSIS `-setup.exe`와 `.sig`를 그대로 쓴다(`.nsis.zip`은 v1 호환 전용). `installMode`는 `passive`(진행 막대만, 기본값). 설치 단계에서 앱은 스스로 종료된다(Tauri 문서). 우리 설치는 `currentUser`라 관리자 권한이 없어도 된다.
- **사이드카가 파일을 잡고 있으면 설치가 실패한다(보고)**: llama-server를 `RunEvent::Exit`에서야 끄면 NSIS 복사와 경합해 실패한 사례([dormouse#119](https://github.com/diffplug/dormouse/pull/119), [tauri#15134](https://github.com/tauri-apps/tauri/issues/15134)). 우리 앱은 llama-server와 전사 워커(같은 `meeting-summarizer.exe`를 `--transcribe-worker`로 띄움)를 둘 다 띄운다. `Sidecar::stop`은 끄기를 스레드에 맡기고 바로 돌아오며(`sidecar.rs:647-677`), `Job::halt`는 `child.kill()`만 하고 기다리지 않는다(`queue.rs:294-303`). 그래서 설치 전에 둘이 끝날 때까지 기다려야 한다(5.2).
- **Mark-of-the-Web**: 앱이 HTTP로 받아 쓴 파일에는 `Zone.Identifier`가 붙지 않아 업데이트 때 SmartScreen이 뜨지 않을 것으로 본다(추정, 근거 문서 없음). Windows 실기기에서 확인한다.

### 3.4 Tauri 업데이터

- 버전: `tauri-plugin-updater` 2.14.1(Rust 1.90 이상). 기본 기능은 `rustls-tls`, `system-proxy`, `zip`이고 `native-tls`를 고를 수 있다. 앱의 reqwest는 사내 CA 때문에 `native-tls` + `system-proxy`다(Cargo.toml). 업데이터도 `default-features = false`, `features = ["native-tls", "system-proxy"]`로 맞춘다(`zip`은 v1 호환 zip을 풀 때만 쓴다. 빌드로 확인).
- 설정: `bundle.createUpdaterArtifacts: true`, `plugins.updater.pubkey`(공개 키 내용), `plugins.updater.endpoints`, `plugins.updater.windows.installMode`.
- Rust API: `app.updater_builder().timeout(..).build()?.check().await?` → `Option<Update>`, `update.download(on_chunk, on_done)` → 바이트, `update.install(bytes)`, 그 뒤 `app.restart()`. 모두 Rust에서 하면 웹뷰의 CSP(`connect-src ipc:`)를 바꿀 필요가 없고, JS 플러그인(`@tauri-apps/plugin-updater`)과 `tauri-plugin-process`, capability 권한도 필요 없다.
- `latest.json`: `version`, `notes`, `pub_date`(RFC 3339), `platforms.{darwin-aarch64,windows-x86_64}.{url,signature}`. `signature`는 `.sig` 파일의 내용이다. 버전을 비교하기 전에 파일 전체를 검증하므로 플랫폼 항목 하나만 깨져도 모든 플랫폼의 업데이트가 멈춘다.
- 서명 키는 CI의 `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 환경 변수로 준다(`.env`는 읽지 않는다).
- 버전 표시는 `@tauri-apps/api/app`의 `getVersion()`(`core:default`에 포함)이나 Rust `app.package_info().version`이다.

### 3.5 업데이트 피드

- 비공개 저장소의 Release 자산은 인증 없이 받을 수 없다. 앱 안에 읽기 토큰을 넣으면 토큰을 꺼내 소스 전체를 읽을 수 있으므로 쓰지 않는다.
- 공개 저장소면 `https://github.com/sijecorp/meeting-summarizer-releases/releases/latest/download/latest.json`이 가장 최근 Release의 `latest.json`으로 넘어간다. `latest`는 초안과 사전 릴리스를 건너뛰므로, 자산을 다 올린 뒤 초안을 공개해야 클라이언트가 반쯤 올라간 Release를 보지 않는다.
- 다른 저장소에 쓰려면 기본 `GITHUB_TOKEN`으로는 안 되고, 공개 저장소에만 Contents 쓰기 권한을 준 fine-grained PAT(또는 GitHub App 토큰)이 필요하다.
- 공개 저장소에 올라가는 것: 두 설치 파일, `.app.tar.gz`, `.sig` 두 개(`.app.tar.gz.sig`, `-setup.exe.sig`), `latest.json`, 릴리스 노트. 앱 안에 비밀값은 없다(Slack 로그인은 PKCE라 `CLIENT_ID`만 있다, `slack.rs:407`).

## 4. 범위

**포함**

- E0. 실측: 코드 없이 자체 서명 덮어 설치로 권한·키체인을 확인하고(6장 0단계), 1단계 코드로 업데이터의 격리·App Management·다시 뜨기를 확인한다(6장 2단계).
- E1. 설정 > 일반에 지금 버전.
- E2. Rust 업데이터: 확인(시작 30초 뒤, 6시간마다, 설정의 "업데이트 확인"), 찾으면 백그라운드로 받기, 받은 뒤 "다시 시작해 업데이트". 진행 중인 녹음·처리는 종료 확인(아키텍처 8.5)과 같은 흐름으로 멈추고 다음 실행에서 이어 한다.
- E3. UI: 설정 일반의 "앱 버전" 묶음(준비되면 릴리스 노트), 업데이트가 준비되면 홈의 한 줄과 트레이 메뉴 항목, 업데이트 뒤 첫 실행의 결과 1회 표시. 시작할 때 `app.log`에 버전 한 줄.
- E4. CI: 업데이터 서명, macOS 자체 서명, 공개 저장소 Release와 `latest.json`.
- E5. 문서: install.md, 아키텍처, 요구사항(APP-05 다시 씀), TODOS, 설계 문서 Open Question 2, 디자인 시스템 6.7, 릴리스 노트.

**범위 밖**

- 유료 OS 서명(U2). 결정이 바뀌면 3.2·3.3대로 붙인다.
- 베타 채널, 단계적 배포, 강제 업데이트(최소 버전), 델타 업데이트, 업데이트 끄기 설정.
- 다운그레이드. 업데이터는 더 높은 버전만 받는다. 잘못된 릴리스는 고친 다음 버전으로 덮는다.
- 녹음·처리 중 자동 설치. 설치는 사용자가 누를 때만 한다.
- 종료할 때 자동 설치(TODOS로 미룸: 앱은 트레이에 살아 사용자가 종료하는 일이 드물고, OS 종료는 `ExitRequested`를 거치지 않는다. Windows는 `restart_after_install(false)`로 다시 띄우지 않게 할 수 있다), 업데이트 준비 데스크톱 알림(TODOS로 미룸: 홈 줄과 트레이로 충분한지 파일럿에서 본다).

## 5. 설계

### 5.1 상태

```
           시작 30초 뒤 / 6시간마다 / "업데이트 확인"
idle ──────────────────────────────────────────────▶ checking
                                                        │
             ┌──────────────── 새 버전 없음 ◀───────────┤
             ▼                                          │ 새 버전 있음
         up_to_date                                     ▼
                                                   downloading (받은 바이트/전체)
                                                        │
                          실패(네트워크, 서명) ◀─────────┤ 서명 확인 끝
                                 │                      ▼
                                 ▼                   ready {version, notes}
                              failed {message}          │ "다시 시작해 업데이트"
                                                        ▼
                                  진행 중 작업 있음 ─▶ 확인 대화상자 ─▶ installing
                                                        │ (작업 정리 → 설치 → 다시 시작)
                                                        ▼
                                                   새 버전으로 실행
```

- `failed`는 다음 확인 때 다시 `checking`으로 간다. 자동 확인의 실패는 로그만 남기고 홈에 띄우지 않는다. 사용자가 누른 확인의 실패만 오류 줄로 보인다. 마지막으로 성공한 확인 시각(`last_check_ok`)을 `updated_to`와 같은 곳에 적고, 7일을 넘으면 설정 버전 행에 "7일 넘게 업데이트를 확인하지 못했습니다"를 보인다(5.4 표).
- 자동 확인(시작 30초 뒤, 6시간마다)은 `idle`·`up_to_date`·`failed`에서만 돈다. `checking`·`downloading`·`ready`·`installing`에서는 건너뛴다. `ready` 동안 더 새 버전이 나오면 다음 실행에서 받는다. 수동 확인 버튼은 `ready`에서 "다시 시작해 업데이트"로 바뀌므로 겹치지 않는다.
- `installing`에서 설치가 실패하면(macOS 교체 실패, Windows 설치 프로그램 실행 실패) 로그를 남기고 `app.restart()`로 지금 버전을 다시 띄운다. 정리 단계에서 큐가 닫혔으므로(`queue.rs:1323`) 같은 프로세스로 계속 돌지 않는다. 멈춘 작업은 다음 실행에서 이어 한다.
- `failed`는 `{message, manual}`이다. `manual`은 사용자가 누른 확인과 그 뒤 이어진 받기에서 난 실패만 참이다. 화면은 `manual`일 때만 오류와 링크를 보이고, 아니면 `idle`처럼 "업데이트 확인" 버튼만 보인다.
- 받은 업데이트(바이트)는 메모리에 둔다. 앱을 끄면 버리고 다음 실행에서 다시 받는다.

### 5.2 Rust (`update.rs`)

- `Updater` 상태(Mutex)와 `app://update` 이벤트. 명령 셋: `update_state`, `check_update`, `install_update`. `update_state`는 지금 버전, 상태(5.1), `just_updated: {version, ok} | null`(5.4)을 돌려준다.
- `install_update`: `ready`가 아니면 거절. 녹음 중이면 `Err("recording")`로 거절한다(녹음은 다시 할 수 없다). 정리 전에 `latest.json`을 다시 확인한다(30초): 받은 버전과 다르면(철회됐거나 더 새 버전) 받은 것을 버리고 확인 흐름으로 돌아가고, 네트워크로 실패하면 받은 것(서명 확인 끝)으로 계속한다. 그다음 작업을 정리한다: `QUIT_CONFIRMED`, `queue.shutdown()`, `recorder.stop(None)`, `store.pause_busy()`, `exports.cancel_all()`. 그 뒤 엔진과 워커가 끝날 때까지 기다린다(최대 10초. `Queue::engine_running()`과 새 `Queue::workers_running()`(워커 자식의 `try_wait`)으로 판정). 설정에 `updated_to`를 적는다. 10초가 지나도 남아 있으면 설치하지 않고 로그를 남긴 뒤 `app.restart()`로 지금 버전을 다시 띄운다(다음 시작에 "업데이트하지 못했습니다", 반쯤 덮인 설치를 만들지 않는다). 다 끝났으면 `update.install(bytes)`. macOS는 성공하면 `app.restart()`. Windows는 `install`이 설치 프로그램을 띄운 뒤 `std::process::exit(0)`으로 끝나고(`RunEvent::Exit`가 오지 않는다), 설치 프로그램이 앱을 다시 띄운다(`restart_after_install` 기본값 true, plugin `updater.rs` `updater_parameters`).
- 정리는 한 함수(`shutdown_work`)로 묶어 `confirm_quit`, `RunEvent::Exit`, `install_update`가 같이 쓴다. 지금 `confirm_quit`에는 내보내기 취소가 없고(`commands.rs:345-359`) `RunEvent::Exit`에만 있다(`lib.rs:476-490`). Windows 설치는 `RunEvent::Exit`를 거치지 않으므로 이 함수가 그 일을 다 해야 한다.
- 진행 중 작업 판정은 `store.busy_ids()`(종료 확인과 같다). 녹음 중에는 설치 버튼을 막고 "녹음이 끝나면 업데이트할 수 있습니다"를 보이며 트레이 항목도 숨긴다. 그 밖의 진행 중 작업(전사·요약)은 확인 뒤 멈춘다. `install_update(confirmed: bool)`은 `confirmed`가 거짓이고 진행 중 작업이 있으면 아무것도 하지 않고 `Err("busy")`를 돌려준다. 화면은 그때 대화상자를 띄우고, "업데이트"를 누르면 `confirmed: true`로 다시 부른다.
- 업데이터는 `native-tls` + `system-proxy`(`zip` 기능 없음. macOS의 tar·flate2는 기능과 상관없이 들어간다). 확인 요청은 30초 타임아웃. 빌더 타임아웃은 받기 요청에도 걸리므로(plugin `updater.rs:849-850`) 받기 전에 `update.timeout = None`으로 풀고, 60초 동안 새 조각이 오지 않으면 받기를 끊고 `failed`로 간다(`on_chunk` 시각과 `tokio::select`).
- 로그: 확인 결과, 받은 크기와 시간, 설치 시작, 실패 원인을 `app.log`에 남긴다.

### 5.3 설정 파일

- `tauri.conf.json`: `plugins.updater.pubkey`, `plugins.updater.endpoints: ["https://github.com/sijecorp/meeting-summarizer-releases/releases/latest/download/latest.json"]`, `plugins.updater.windows.installMode: "passive"`.
- `bundle.createUpdaterArtifacts: true`는 CI만 쓰는 `src-tauri/tauri.updater.conf.json`에 두고 tauri-action `args`에 `--config`로 더한다. 이 값이 켜져 있으면 서명 키 없이 `tauri build`가 실패하므로, README의 로컬 설치 파일 빌드가 깨지지 않게 한다.
- capability는 그대로 둔다(JS에서 업데이터를 부르지 않는다).
- 업데이터는 빌드할 때 `MS_UPDATER=1`일 때만 켜진다(`option_env!`, 기존 `MS_LLAMA_SERVER`와 같은 이름 규칙). CI의 태그·수동 빌드와 업데이터 시험 빌드만 이 값을 준다. 그 밖의 빌드(`tauri dev`, README의 로컬 설치 파일)는 확인하지 않는다. 로컬에서 만든 앱이 공개 버전으로 자기를 바꾸지 않게 한다.

### 5.4 화면

- 설정 > 일반 맨 아래 "앱 정보" 묶음(디자인 시스템 6.7의 행) 하나에 두 행: 위 "버전", 아래 지금의 로그 폴더 행(`LogFolder`, `Settings.tsx:1431-1456`). 이 묶음은 `fieldset disabled={locked}` 밖에 둔다(LogFolder처럼). 읽기 전용 설정(새 버전 앱이 쓴 설정)이어도 확인과 설치가 된다. 설정을 읽지 못한 화면(`Settings.tsx`의 오류 반환)에도 버전 행을 보인다. 일반 탭 설명(`TABS`)은 "회의록에 남는 이름, 알림, 로그와 앱 버전입니다."로 바꾼다.
- 버전 행: 왼쪽 첫 줄 "버전 1.2.1"(숫자 `tabular-nums`), 둘째 줄 상태 줄(`text-label`, `ink-2`). 오른쪽 버튼 하나: 평소 outline sm, `ready`일 때만 brand sm. 버튼은 상태가 바뀌어도 그 자리에 남고(못 누를 때는 `aria-disabled`, `Settings.tsx:1504` 방식) 초점을 잃지 않는다. 창 너비가 `sm` 미만이면 버튼이 글 아래로 내려간다.
- 버전 숫자 뒤에는 조사를 바로 붙이지 않고 "버전"을 끼운다("1.2.2 버전을", "1.2.2 버전으로"). `josa()`는 숫자를 읽지 못한다.

| 상태 | 상태 줄 | 버튼 |
|---|---|---|
| 업데이터가 꺼진 빌드(`MS_UPDATER` 없음, 5.3) | "이 빌드는 업데이트를 확인하지 않습니다" | 없음 |
| idle(확인한 적 없음) | 없음 | "업데이트 확인" |
| up_to_date | 수동 확인 직후 3초 "방금 확인했습니다", 그 뒤 "최신 버전입니다" | "업데이트 확인" |
| 마지막 성공 확인이 7일 넘음(자동 실패 포함) | "7일 넘게 업데이트를 확인하지 못했습니다" | "업데이트 확인" |
| checking | "확인하는 중…" | "업데이트 확인"(막힘) |
| downloading | "1.2.2 버전을 받는 중 42%". 전체 크기를 모르면 "1.2.2 버전을 받는 중 12MB". 15초 넘게 새 조각이 없으면 "연결을 기다리는 중…" | "업데이트 확인"(막힘) |
| ready | "1.2.2 버전을 받았습니다" | "다시 시작해 업데이트"(brand) |
| ready, 녹음 중 | "녹음이 끝나면 업데이트할 수 있습니다" | "다시 시작해 업데이트"(막힘) |
| ready, 가져오기나 내보내기 중 | "파일 작업이 끝나면 업데이트할 수 있습니다" | "다시 시작해 업데이트"(막힘) |
| ready, 저장하지 않은 설정 있음 | "저장하지 않은 설정이 있습니다. 저장하거나 되돌린 뒤 업데이트하세요" | "다시 시작해 업데이트"(막힘) |
| installing | "업데이트를 확인하는 중…" → "작업을 마무리하는 중…" → "업데이트하는 중…"(Windows는 "업데이트하는 중… 창이 닫혔다가 다시 열립니다") | "다시 시작해 업데이트"(막힘) |
| 재확인에서 철회됨 | "이 업데이트는 취소되었습니다" 뒤 up_to_date | "업데이트 확인" |
| 재확인에서 더 새 버전 | "더 새 버전이 나와 다시 받습니다" 뒤 downloading | "업데이트 확인"(막힘) |
| failed(수동) | 오류 분류 문구(5.4 오류 문구) | "다시 시도", 옆에 "릴리스 페이지 열기" 링크 |
| 그 버전 설치 실패 기록 있음 | "1.2.2 버전을 앱에서 설치하지 못했습니다. 릴리스 페이지에서 받아 설치하세요" | "업데이트 확인", 옆에 "릴리스 페이지 열기" 링크 |
| 상태를 불러오지 못함(`update_state` 실패) | "업데이트 상태를 불러오지 못했습니다" | "다시 시도" |

- 설정의 이 묶음은 저장 막대에 세지 않는다(로그 폴더 열기처럼 바로 일어나는 일).
- `ready`면 버전 행 아래에 "바뀐 것 보기" 접기 단추(`aria-expanded`, 키보드로 열고 닫음)를 둔다. 펼치면 노트를 줄마다 목록으로 보인다(최대 12줄 높이, 넘치면 안에서 스크롤, 긴 줄과 주소는 줄바꿈). 노트는 `scripts/latest-json.ts`가 릴리스 노트의 `## 바뀐 것` 절만 뽑아 마크다운 기호(목록 기호, 강조, 코드 따옴표, 링크 괄호)를 지운 평문 줄로 넣는다. 화면은 해석하지 않고 React 글자 노드로 줄만 나눈다(NFR-07). 노트가 비면 접기 단추를 두지 않는다.
- 홈(6.4)과 첫 화면(6.5)의 설정 줄 자리에는 업데이트 줄이 많아야 하나다(설정 줄과 같은 모양: `text-label`, `ink-2` 글, 끝에 link xs 버튼). `ready`면 "1.2.2 버전을 받았습니다 · 다시 시작해 업데이트"이고 닫기가 없다. 업데이트 결과 줄(아래)은 `ready`가 아닐 때만 보인다. 녹음 화면에는 보이지 않는다. 줄 순서는 위에서부터 오디오 확인 줄(5.7), 설정 줄, 업데이트 줄이다(녹음 준비가 먼저, 업데이트는 미뤄도 되는 일). 줄이 길면 글이 줄바꿈되고 버튼은 글 끝에 붙는다. 첫 화면은 줄이 넘치면 세로 스크롤을 허용한다.
- `ready`면 트레이 메뉴 맨 위에 "다시 시작해 업데이트"와 구분선을 더한다(순서: 다시 시작해 업데이트, 구분선, 열기, 종료. 설정·홈과 같은 문구). 메뉴는 `ready`가 되거나 풀릴 때 `set_menu`로 다시 만든다(지금은 시작 때 한 번 만든다, `lib.rs:366-379`). 누르면 창을 앞으로 가져오고 `app://update-install-requested`를 보낸다(종료의 `app://quit-requested`와 같은 길). 화면은 진행 중 작업이 있으면 대화상자를, 없으면 바로 설치를 부른다.
- 업데이트 뒤 첫 실행: 설치 직전에 설정에 `updated_to`(새 버전)를 적는다. Rust가 시작할 때 그 값을 읽고 지우면서 `just_updated: {version, ok}`를 그 세션 메모리에만 둔다(`ok`는 지금 버전 >= `updated_to`. 실패 뒤 링크로 더 새 버전을 손 설치한 경우도 성공으로 본다). `update_state`가 이것을 돌려준다. 화면은 `ok`면 "1.2.2 버전으로 업데이트했습니다 · 바뀐 것"(그 버전의 공개 Release 페이지), 아니면 "업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다 · 릴리스 페이지 열기"를 홈과 첫 화면의 업데이트 줄로 보인다. 닫기(×, 32×32, `aria-label="닫기"`)를 누르거나 앱을 다시 시작하면 사라진다. 실패는 설정 버전 행에 남는다(설치 실패 기록, 5.4 표).
- 실측이 ad-hoc 대체로 끝난 경우에만: macOS에서 업데이트 뒤 첫 실행의 "업데이트했습니다" 줄에 "녹음 화면에서 상대방 소리 막대가 움직이지 않으면 설정 > 오디오의 시스템 오디오 '테스트'로 확인하세요"를 붙이고 install.md의 권한 절로 잇는다.
- 진행 중인 처리(전사·요약)가 있으면 종료 확인과 같은 대화상자: 제목 "진행 중인 작업이 있습니다", 설명 "지금 업데이트하면 진행 중인 전사와 요약을 멈추고, 다시 시작한 뒤 이어서 처리합니다", 버튼 "취소"·"업데이트". 녹음, 가져오기, 내보내기 중에는 대화상자 대신 버튼이 막힌다(5.2, 5.4 표). 대화상자는 열릴 때 "취소"에 초점을 두고, 닫히면 연 버튼으로(트레이에서 열었으면 창 본문으로) 초점을 돌린다.
- 미리보기(`preview.html`)와 fixture에 상태별 장면을 더한다.

### 5.5 CI (`build.yml`)

- macOS 빌드: tauri-action 앞에 "자체 서명 인증서 넣기" 단계. 시크릿 `MACOS_SELF_SIGNED_P12`(base64), `MACOS_SELF_SIGNED_P12_PASSWORD`로 임시 키체인을 만들고, `APPLE_SIGNING_IDENTITY`를 인증서 이름으로 내보낸다. 시크릿이 없으면 수동·예약 실행은 지금처럼 ad-hoc(`"-"`)으로 빌드하고 경고를 남기며, 태그 빌드는 실패한다.
- macOS 검사: 기존 `codesign --verify`에 더해 지정 요구사항이 `certificate root = H"<고정 해시>"`인지 본다(태그 빌드에서는 다르면 실패). 실측이 ad-hoc 대체로 끝나면 자체 서명 단계와 이 검사는 넣지 않는다.
- 두 OS: tauri-action에 `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. 아티팩트에 `.app.tar.gz`, `.sig`를 더한다.
- `release` 작업은 지금처럼 비공개 저장소 Release만 만든다. 그 뒤 새 `publish` 작업(`needs: release`)이 공개 저장소에 같은 태그의 초안 Release를 만들어 설치 파일·업데이트 파일·`.sig`를 올리고, `latest.json`을 만들어 올린 뒤 `gh release edit --draft=false --latest`로 공개한다. 토큰은 조직 소유 GitHub App(공개 저장소에만 Contents 쓰기)의 설치 토큰을 `actions/create-github-app-token`으로 만든다(시크릿 `RELEASES_APP_ID`, `RELEASES_APP_KEY`). 개인 PAT와 달리 만료와 퇴사에 묶이지 않는다. `slack`은 `needs: [release, publish]`와 `if: always() && needs.release.result == 'success' && vars.SLACK_AUTO != 'false'`로 바꾼다. 공개 게시가 실패해도 설치 파일은 Slack에 올라간다(긴급 배포 경로). 노트 링크는 `publish`가 성공했을 때만 공개 저장소 주소로 붙인다. 공개 저장소에 그 태그의 Release가 이미 공개돼 있으면 `publish`를 실패시킨다(공개된 자산과 `.sig`를 바꾸면 받는 중인 앱이 서명 불일치를 본다). 초안이면 덮어쓰고 이어 간다.
- `slack-release.yml`의 릴리스 노트 링크(56행, 지금 비공개 저장소 주소)를 공개 저장소 주소로 바꾼다. 동료는 비공개 저장소를 열 수 없다.
- 릴리스 노트는 공개 글이다(`docs/releases/<태그>.md`가 그대로 공개된다). 사내 사람·팀 이름과 회의 내용을 쓰지 않는다.
- `latest.json`은 `scripts/latest-json.ts`(bun)가 만든다: 태그, 노트(`docs/releases/<태그>.md`), 두 `.sig`, 공개 저장소 자산 URL. 두 플랫폼이 다 있고 서명이 비어 있지 않고 버전이 태그와 같을 때만 쓴다. 노트 파일이 없으면 `notes`는 빈 문자열이고 경고를 남긴다. 테스트를 둔다.
- 수동 실행(`workflow_dispatch`)은 공개 저장소에 올리지 않는다.
- 공개된 태그는 바뀌지 않는다: 태그 빌드는 맨 앞에서 공개 저장소에 그 태그의 Release가 공개돼 있는지 보고, 있으면 빌드 전에 실패한다("공개된 태그는 다시 빌드하지 않습니다. 버전을 올려 새 태그를 만드세요"). 비공개 Release의 `--clobber` 덮어쓰기는 공개 전 재시도에만 쓰인다. `publish`만 실패했으면 "실패한 작업만 다시 실행"으로 같은 실행의 산출물을 다시 쓴다.
- 공개 저장소는 README 커밋 하나로 초기화해 둔다. 빈 저장소에는 태그를 만들 커밋이 없어 `gh release create --repo`가 실패한다. 공개 저장소에는 `--verify-tag` 대신 `--target main`을 쓴다.

### 5.6 키와 인증서 보관

- 업데이터 키 쌍(1단계 시작 때)과 자체 서명 인증서(유효 20년, 0단계에서 만들어 2단계와 CI가 그대로 쓴다)는 1회 만든다. 개인 키와 비밀번호는 GitHub 시크릿과 회사 비밀번호 관리자 두 곳에 둔다. 업데이터 키를 잃으면 모든 사람이 손으로 다시 설치해야 하고, 인증서를 잃으면 다음 업데이트에서 Mac 권한을 한 번 다시 받는다.
- 공개 키는 `tauri.conf.json`에, 인증서의 해시는 build.yml 검사에 넣는다.

### 5.7 1.1.1에서 1.2.0으로

- 1.1.1에는 업데이터가 없으므로 동료는 한 번 더 손으로 설치한다. 그 버전은 실제 피드로 업데이트가 검증된 1.2.1이다(6장 4단계). 1.2.0은 시험 기기에만 설치한다.
- Mac은 서명이 ad-hoc에서 자체 서명으로 바뀌므로 1.2.x 첫 실행에서 마이크·시스템 오디오 권한과 키체인 "항상 허용"을 한 번 더 받는다. 마이크는 OS가 다시 묻지만 시스템 오디오는 오류 없이 조용히 0이 된다. 그래서 앱이 안내한다: 이전 설치의 흔적(설정 파일이나 회의 폴더)이 있는데 `last_run_version`이 없거나 1.2.0 미만이면(1.1.1은 이 값을 쓰지 않는다) macOS에서 `audio_check_needed`를 켠다. 켜져 있으면 홈과 첫 화면 맨 위에 "Mac 권한이 바뀌어 오디오를 다시 확인해야 합니다 · 오디오 확인"(설정 > 오디오로 이동) 줄을 보이고, 설정 > 오디오의 시스템 오디오 테스트가 통과하면 끈다. 녹음은 막지 않는다(곧 시작할 회의가 있을 수 있다). 시작할 때마다 `last_run_version`을 지금 버전으로 적는다. 릴리스 노트와 install.md에도 쓴다.
- 1.2.0부터는 앱 안에서 업데이트한다. 실측이 실패해 ad-hoc으로 돌아가면 업데이트마다 권한 재허용 안내를 앱에 넣는다(U2 파생).

### 5.8 실측 실패별 대응

| 실패 | 대응 |
|---|---|
| 업데이트 뒤 마이크·시스템 오디오 권한이나 키체인 허용이 풀림 | 자체 서명의 이득이 없으므로 ad-hoc으로 돌아가고 5.4 마지막 줄의 안내를 넣는다. 자체 서명 단계와 검사는 넣지 않는다. 그리고 macOS 업데이트 뒤 다음 녹음 전에 시스템 오디오 테스트(ONB-03)를 먼저 하게 한다: 설정에 남는 표시로 통과할 때까지 홈과 녹음 시작에서 안내한다 |
| App Management 창이 한 번 뜨고 허용하면 다음 업데이트부터 안 뜸 | 자체 서명을 유지하고 install.md와 첫 업데이트 안내에 "허용"을 적는다 |
| App Management가 교체를 막음(허용해도 실패) | macOS는 알림만: 준비 상태 대신 "새 버전이 있습니다"와 릴리스 페이지 링크(손 설치). Windows는 그대로 |
| 업데이트 뒤 Gatekeeper 창이 뜨거나 격리 속성이 남음 | 업데이터는 유지한다(받기는 앱이 한다). install.md에 업데이트 뒤 "그래도 열기" 절차를 적고 원인을 TODOS에 남긴다 |
| b가 스스로 다시 뜨지 않음 | 릴리스 전에 고친다(버그). 고치기 전에는 릴리스하지 않는다 |
| 첫 설치의 "그래도 열기"가 ad-hoc과 다름(더 막힘) | install.md 첫 설치 절차를 그 화면대로 고친다. 막혀서 열 수 없으면 ad-hoc으로 돌아간다 |

## 6. 단계

| 단계 | 내용 | 끝나는 조건 |
|---|---|---|
| 0. 권한 실측 (macOS, 코드 없이, 몇 시간) | 지금 코드를 자체 서명 인증서로 버전만 바꿔 두 번 로컬 빌드(a, b). a.dmg에 격리 속성을 붙여 설치("그래도 열기"), 마이크·시스템 오디오 허용, Slack 로그인, 녹음. b를 응용 프로그램 폴더에 끌어다 덮어 설치한 뒤 다시 녹음·보내기 | 권한·키체인이 유지되면 Mac 방향(자체 서명) 확정. 아니면 5.8 첫 줄로 1단계 전에 계획을 고친다. 2단계는 업데이터에만 해당하는 확인(격리, App Management, 다시 뜨기)에 집중한다 |
| 1. 앱 | 먼저 업데이터 키 쌍을 만들어 공개 키를 `tauri.conf.json`에 넣고 개인 키·비밀번호를 두 곳에 둔다(5.6). 그다음 E1, E2, E3, 테스트, 미리보기 | `bun run test`, `cargo test`, `cargo clippy -D warnings` 통과 |
| 2. 실측 (macOS, 이 Mac) | 자체 서명 인증서를 만들고, 1단계 코드와 키로 시험 빌드 두 개(1.2.0-a, 1.2.0-b)를 로컬에서 서명해 만든다. 시험용 `--config`로 끝점을 로컬 서버(`http://127.0.0.1`)로 돌리고 `plugins.updater.dangerousInsecureTransportProtocol: true`를 준다(업데이터는 https가 아니면 거절한다, plugin `updater.rs:827`). a.dmg에 내려받은 파일처럼 격리 속성을 붙이고(`xattr -w com.apple.quarantine "0081;<시각>;Safari;"`, 또는 Slack으로 받아) 설치해 "그래도 열기"가 실제로 뜨고 통과되는지 본다. 마이크·시스템 오디오 허용, Slack 로그인(키체인 항목), 10초 녹음. 앱에서 b로 업데이트한 뒤 다시 녹음. 시험 하네스: 로컬 빌드는 `--config src-tauri/tauri.updater.conf.json`과 시험 `--config`를 함께 주고 `TAURI_SIGNING_PRIVATE_KEY`·`_PASSWORD`를 환경 변수로 둔다. `latest-json.ts`는 두 플랫폼을 요구하므로 시험용 `latest.json`(darwin-aarch64 하나)은 손으로 쓴다. Windows는 이 단계에서 시험하지 않는다(11장 d4) | 첫 설치의 "그래도 열기"가 자체 서명 앱에서도 ad-hoc과 같은 절차, 업데이트 뒤 Gatekeeper 창 없음, `xattr`에 `com.apple.quarantine` 없음, 두 트랙이 0이 아님, 키체인 창 없음, App Management 창 없음, b가 스스로 다시 뜨고 "b로 업데이트했습니다"가 한 번 보임. 실패하면 5.8의 표대로 고친다 |
| 3. 배포 | 공개 저장소 생성(README 커밋), 시크릿 등록(사용자: `TAURI_SIGNING_PRIVATE_KEY`·`_PASSWORD`, `MACOS_SELF_SIGNED_P12`·`_PASSWORD`, GitHub App `RELEASES_APP_ID`·`RELEASES_APP_KEY`, 저장소 변수 `SLACK_AUTO=false`). 업데이터 키와 인증서의 두 번째 보관자를 정한다. 그 뒤 E4를 master에 | 수동 실행 빌드 통과, 태그 없이 공개 저장소에 쓰지 않음 |
| 4. 릴리스(조용한 시작) | `SLACK_AUTO=false`인 채로 1.2.0 태그. 이 Mac과 회사 관리 Windows PC 1대(표준 보안 프로그램이 켜진 기준 기기)에만 1.2.0을 손으로 설치. 곧바로 1.2.1 태그를 붙여 실제 공개 피드로 1.2.0 → 1.2.1을 업데이트하고 녹음·전사·요약·Slack 보내기까지 한다. 통과하면 `SLACK_AUTO`를 지우고 slack-release를 손으로 돌려 1.2.1을 마지막 손 설치 버전으로 알린다 | 두 기기 모두 통과하기 전에는 업데이터가 든 빌드를 Slack에 알리지 않는다. 두 OS에서 앱 안 업데이트 성공, 설치 뒤 앱이 스스로 다시 뜸, "1.2.1로 업데이트했습니다" 1회, Windows에서 SmartScreen 창 없음, 설치 뒤 llama-server가 새 것 |
| 5. 문서 | E5 | |

## 7. 테스트

- Rust: 상태 전이(확인 → 없음/받기 → 준비 → 설치, 실패 → 다시 확인), `ready`가 아닐 때 `install_update` 거절, 정리 함수가 `confirm_quit`과 같은 순서로 부르는지, 엔진·워커 대기가 시간 제한에서 끝나는지.
- 화면: 설정 일반의 상태별 문구와 버튼, 준비됐을 때 릴리스 노트(마크다운 기호가 글자 그대로), 진행 중 작업이 있을 때 대화상자, 홈의 준비 줄과 업데이트 뒤 1회 표시(성공·실패), `app://update-install-requested`에 같은 흐름, 저장 막대에 세지 않음.
- Rust: 시작할 때 `updated_to`를 읽고 지우는지(두 번째 시작에는 `just_updated`가 없음). 트레이 항목 목록을 만드는 순수 함수가 `ready`일 때만 그 항목을 넣는지(모의 런타임에는 트레이가 없다, `lib.rs:194`). 자동 확인이 `downloading`·`ready`·`installing`에서 건너뛰는지. 받기 감시가 60초 무진행에서 끊는지. `just_updated.ok`가 지금 버전 >= `updated_to`일 때 참인지(같음, 더 높음, 더 낮음). 자동 확인 실패는 `manual: false`, 수동 확인과 이어진 받기의 실패는 `manual: true`인지. `install_update(false)`가 진행 중 작업이 있으면 정리 없이 `Err("busy")`, 없으면 진행하는지. 녹음 중이면 `confirmed`와 상관없이 `Err("recording")`인지. 설치 직전 재확인이 다른 버전을 보면 받은 것을 버리는지. 10초 대기를 넘으면 설치하지 않고 다시 시작하는지.
- 측정(파일럿): 릴리스마다 공개 저장소 업데이트 파일의 `download_count`(받은 앱 수)와 동료 명단(수십 명)을 대조해 3영업일 뒤 최신 버전 비율을 본다. 80% 미만이 두 번 이어지면 X6·X7을 앞당긴다(TODOS).
- `scripts/latest-json.ts`: 정상, 플랫폼 하나 빠짐, 빈 서명, 태그와 버전 불일치.
- 수동: 2단계와 4단계.

## 8. 위험

| 위험 | 대응 |
|---|---|
| 자체 서명으로도 TCC가 권한을 유지하지 않음 | 2단계에서 릴리스 전에 확인. 실패하면 ad-hoc + 재허용 안내 |
| App Management가 자기 번들 교체를 막음 | 2단계에서 확인. 막히면 오류와 릴리스 페이지 링크로 손 설치 |
| Windows에서 엔진·워커가 파일을 잡아 설치 실패 | 설치 전 대기(5.2). 실패하면 다음 실행에서 "업데이트하지 못했습니다"와 링크(5.4) |
| 회사 방화벽이 GitHub 다운로드 주소(`objects.githubusercontent.com`)를 막음 | 오류와 릴리스 페이지 링크, 손 설치는 그대로 된다 |
| 업데이터 키 분실 | 두 곳 보관(5.6) |
| `latest.json` 하나가 깨져 모든 업데이트가 멈춤 | 스크립트 검사와 테스트, 초안으로 올린 뒤 공개 |
| GitHub App 키 분실이나 권한 변경 | `publish`가 실패로 드러나고 Slack은 링크 없이 설치 파일을 올린다(비공개 Release는 그대로). 앱 키를 새로 만들어 시크릿을 바꾸고 `publish`부터 다시 돌린다 |
| 잘못된 릴리스가 나감 | `gh release edit <이전 태그> --repo sijecorp/meeting-summarizer-releases --latest`로 이전 릴리스를 최신으로 되돌리거나 잘못된 릴리스를 초안으로 돌린다. 자산은 지우지 않는다(받는 중인 앱이 깨진다). 이미 받아 둔 앱은 설치 직전 재확인(5.2)에서 버린다. 앱이 열리지 않는 버그는 Slack·OneDrive 손 설치로 회복하고, 그 버전은 데이터 호환(`schema_version`, NEWER_VERSION 읽기 전용)을 지킨다 |
| Windows 실기기에서 한 번도 돌려 보지 않음 | 2단계에서는 시험하지 않는다(11장 d4). 4단계의 조용한 시작에서 회사 관리 Windows PC로 실제 피드 업데이트를 반드시 통과한 뒤에만 알린다. 백신·EDR이 업데이터가 띄운 설치 프로그램을 막는지도 이때 본다 |

## 9. 문서 변경

- install.md: "새 버전으로 업데이트"를 앱 안 업데이트로 다시 쓴다. 1.2.0까지는 손 설치, Mac의 권한 재허용 1회. 받기 링크를 공개 저장소로.
- 아키텍처: 11장에 업데이터·서명·피드, 8.5에 업데이트 설치 경로.
- 요구사항: "v1에서 제외"의 자동 업데이트 줄을 지우고 APP-05를 업데이트로 채운다.
- TODOS: "파일럿 후 자동 업데이트"를 지우고, "Windows 코드 서명 검토"에 3.3을 옮긴다. "종료할 때 자동 설치"(X6)와 "업데이트 준비 데스크톱 알림"(X7)을 더한다.
- README: 다운로드 링크를 공개 저장소로. 설치 파일 빌드는 그대로 된다(업데이트 파일은 CI만 만든다).
- `.github/workflows/slack-release.yml`: 릴리스 노트 링크(5.5).
- 설계 문서: Open Question 2를 닫는다. Distribution Plan의 업데이터 문장을 고친다.
- 디자인 시스템 6.7: "앱 버전" 묶음.
- `docs/releases/v1.2.0.md`.

## 10. 리뷰에서 수락한 요구사항

아래 줄은 계획 리뷰가 수락한 요구사항이다(리뷰 도구가 붙인다). 위에서 아래로 시간순이고, 같은 주제의 줄이 여럿이면 가장 나중 줄이 이긴다(본문보다도). "(… 대체)" 표시가 없어도 같다. 구현을 시작하기 전에 이 장을 본문에 접어 넣어 본문을 최종 사양으로 만들고, 이 장은 결정 기록으로 남긴다(DX 리뷰).

<!-- autoplan-accepted:ceo -->
- 6장 1단계 앞에 0단계 실측을 둔다. 자체 서명 빌드 a→b 앱 안 업데이트 뒤 Gatekeeper 창 없음, `com.apple.quarantine` 없음, 마이크·시스템 오디오 두 트랙이 0이 아님, 키체인 창 없음, App Management 창 없음을 확인한다. 하나라도 실패하면 기록하고 ad-hoc + 업데이트 뒤 권한 재허용 안내로 계획을 고친다.
- 설정 > 일반 "앱 버전" 묶음: 지금 버전, 상태 줄, 버튼 하나(5.4 표). `ready`면 `latest.json`의 `notes`를 접어 보인다. 저장 막대에 세지 않는다.
- `ready`면 홈 머리 아래 한 줄("새 버전 X를 받았습니다" + "다시 시작해 업데이트")을 보인다. 헤더 설정 버튼의 점은 두지 않는다. 녹음 화면에는 보이지 않는다.
- `ready`면 트레이 메뉴에 "업데이트하고 다시 시작"이 있고, 누르면 창을 앞으로 가져와 같은 설치 흐름(진행 중 작업이 있으면 대화상자)을 탄다. `ready`가 아니면 그 항목이 없다.
- 설치 직전 설정에 `updated_to`를 적고, 새 버전 첫 실행에서 지금 버전과 같으면 홈에 "X로 업데이트했습니다"를 한 번 보이고 지운다. 다르면 표시 없이 지운다. Rust 테스트로 확인한다.
- 시작할 때 `app.log`에 앱 버전 한 줄을 남긴다.
- 공개 저장소는 README 커밋으로 초기화하고, 공개 저장소 Release는 `--target main`으로 만든다(초안 → 자산 → `latest.json` → 공개).
- 종료할 때 자동 설치와 업데이트 준비 데스크톱 알림은 범위 밖이며 TODOS에 미룬다.
- 1장 수치는 "1.0.0(10/20)부터 1.1.1(11/04)까지 2주 남짓 동안 6회"다.
- (스펙 리뷰 1 대체) 첫 줄의 "6장 1단계 앞에 0단계 실측"은 "1단계 코드로 하는 6장 2단계 실측, 4단계 릴리스 전"으로 바꾼다. 확인 항목과 실패 시 대체는 그대로다. 시험 `--config`는 로컬 http 끝점과 `dangerousInsecureTransportProtocol: true`를 쓴다.
- (스펙 리뷰 1 대체) `updated_to` 줄의 "다르면 표시 없이 지운다"는 "다르면 '업데이트하지 못했습니다'와 릴리스 페이지 링크를 한 번 보이고 지운다"로 바꾼다.
- 노트는 마크다운을 해석하지 않고 글자 그대로 줄바꿈만 살린다(NFR-07).
- 트레이 메뉴는 `ready`가 되거나 풀릴 때 `set_menu`로 다시 만들고, 항목을 누르면 `app://update-install-requested`로 화면이 같은 흐름을 탄다. 항목 목록은 순수 함수로 만들고 테스트한다.
- 정리는 `shutdown_work` 한 함수(`QUIT_CONFIRMED`, `queue.shutdown`, `recorder.stop`, `pause_busy`, `exports.cancel_all`)로 `confirm_quit`·`RunEvent::Exit`·`install_update`가 같이 쓴다. 설치 전 엔진·워커 종료를 최대 10초 기다리고(`workers_running` 추가), 넘으면 경고 후 설치를 계속한다.
- 설치가 실패하면 로그를 남기고 `app.restart()`로 지금 버전을 다시 띄운다.
- 자동 확인은 `idle`·`up_to_date`·`failed`에서만 돈다. 확인은 30초 타임아웃, 받기는 `update.timeout = None` + 60초 무진행 감시.
- 업데이터 기능은 `native-tls`·`system-proxy`(`zip` 없음). `createUpdaterArtifacts`는 CI 전용 `src-tauri/tauri.updater.conf.json`으로만 켠다.
- 시크릿은 E4를 master에 넣기 전에 등록한다. 태그 빌드는 자체 서명 시크릿이 없으면 실패하고, 수동·예약 실행은 ad-hoc + 경고다. 실측이 ad-hoc 대체로 끝나면 자체 서명 단계와 지정 요구사항 검사는 넣지 않는다.
- 공개 저장소에 같은 태그의 Release가 이미 공개돼 있으면 release 작업을 실패시킨다. 초안이면 덮어쓰고 이어 간다. 노트 파일이 없으면 `notes`는 빈 문자열 + 경고.
- `slack-release.yml`의 릴리스 노트 링크를 공개 저장소로 바꾼다.
- `RELEASES_TOKEN` 만료를 위험 표에 두고 만료 30일 전 알림을 둔다.
- 4단계 끝나는 조건에 "설치 뒤 앱이 스스로 다시 뜸"과 "X로 업데이트했습니다" 1회를 넣는다.
- 실측이 ad-hoc 대체로 끝난 경우에만 macOS 업데이트 뒤 첫 실행 줄에 시스템 오디오 테스트 안내를 붙인다(5.4 마지막 줄).
- TODOS에 X6(종료할 때 자동 설치)과 X7(업데이트 준비 데스크톱 알림)을 더한다.
- (스펙 리뷰 2 대체) `updated_to` 성공 판정은 "지금 버전과 같으면"이 아니라 "지금 버전 >= `updated_to`"다. Rust가 시작할 때 읽고 지우며 `just_updated: {version, ok}`를 그 세션에만 두고 `update_state`로 돌려준다. 화면은 홈과 첫 화면의 설정 줄 자리, 설정 줄 위에 한 줄로 보이고 닫기(×)나 다시 시작으로 사라진다. `ready` 줄과 함께면 `ready` 줄이 위다.
- (스펙 리뷰 2 대체) 공개 저장소 게시는 `release` 작업이 아니라 새 `publish` 작업(`needs: release`)이 하고, `slack`은 `needs: publish`다. 초안 공개는 `gh release edit --draft=false --latest`. PAT가 만료되면 `publish`가 실패하고 Slack도 보내지 않는다(비공개 Release는 그대로).
- `failed`는 `{message, manual}`이고, 수동 확인과 그 뒤 이어진 받기의 실패만 `manual: true`로 화면에 보인다.
- 업데이터 키 쌍은 1단계 시작 때 만들어 공개 키를 `tauri.conf.json`에 넣고 개인 키를 두 곳에 둔다. 자체 서명 인증서는 2단계에서 만든다.
- 2단계 실측은 a.dmg에 격리 속성을 붙여(또는 Slack으로 받아) 실제 첫 설치의 "그래도 열기"를 거치고, a에서 Slack에 로그인해 둔 뒤 업데이트한다.
- 3.5의 공개 자산 `.sig`는 두 개(`.app.tar.gz.sig`, `-setup.exe.sig`)다.
- (스펙 리뷰 3 대체) 실측 실패 시 "모두 ad-hoc + 안내"가 아니라 5.8의 실패별 표를 따른다. 권한·키체인 유지 실패만 ad-hoc + 안내다.
- `install_update(confirmed: bool)`: 거짓이고 진행 중 작업이 있으면 정리 없이 `Err("busy")`, 화면은 대화상자 뒤 `true`로 다시 부른다. 테스트한다.
- 준비 줄과 업데이트 결과 줄은 홈과 첫 화면 모두 설정 줄 자리에 있고, 순서는 준비 줄, 결과 줄, 설정 줄이다.
- 2단계 실측 하네스: 로컬 빌드는 `tauri.updater.conf.json` + 시험 `--config` + 업데이터 키 환경 변수, 시험 `latest.json`은 darwin 하나로 손으로 쓴다. 통과 조건에 "b가 스스로 다시 뜨고 결과 줄 1회"를 더한다.
- Windows PC가 있으면 2단계에서 Windows a→b도 시험한다. 없으면 4단계가 첫 시험이고, 깨지면 지금처럼 손 설치로 회복한다(위험 표).
- (CEO 두 목소리 대체) 동료가 마지막으로 손 설치하는 버전은 1.2.0이 아니라 1.2.1이다. 1.2.0은 `SLACK_AUTO=false`로 알리지 않고 이 Mac과 회사 관리 Windows PC(표준 보안 프로그램) 두 대에만 설치해 실제 공개 피드로 1.2.0 → 1.2.1을 검증한다. 두 기기 모두 통과하기 전에는 업데이터가 든 빌드를 알리지 않는다.
- (CEO 두 목소리 대체) 녹음 중에는 설치하지 않는다(`Err("recording")`, 버튼 막힘, 트레이 항목 숨김). 확인 대화상자는 처리(전사·요약)에만 쓴다.
- (CEO 두 목소리 대체) 10초 대기를 넘으면 설치하지 않고 지금 버전으로 다시 시작한다("업데이트 계속" 규칙을 바꾼다).
- 설치 직전 `latest.json`을 다시 확인해 받은 버전과 다르면 버리고 확인 흐름으로 돌아간다. 네트워크 실패면 받은 것으로 계속한다.
- (CEO 두 목소리 대체) `slack`은 `publish` 실패와 상관없이 설치 파일을 올린다(`needs: [release, publish]`, `if: always() && release 성공 && SLACK_AUTO != false`). 노트 링크만 `publish` 성공 때 붙인다.
- (CEO 두 목소리 대체) 공개 저장소 쓰기 토큰은 개인 PAT가 아니라 조직 소유 GitHub App 설치 토큰(`RELEASES_APP_ID`, `RELEASES_APP_KEY`)이다. PAT 만료 위험 행은 GitHub App 키 행으로 바뀐다.
- 잘못된 릴리스 대응 절차(이전 태그를 `--latest`로, 자산은 지우지 않음)를 위험 표, 아키텍처 11장, install.md 관리자 절에 둔다.
- 업데이터 키와 인증서의 두 번째 보관자를 정한다(사용자).
- 릴리스 노트는 공개 글이다. 사내 사람·팀 이름과 회의 내용을 쓰지 않는다.
- 6장 0단계(코드 없는 권한 실측)를 1단계 전에 한다. 결과로 Mac 방향을 1단계 전에 정한다.
- ad-hoc 대체 경로에서는 macOS 업데이트 뒤 다음 녹음 전에 시스템 오디오 테스트를 하게 하는 남는 표시를 둔다.
- 측정: 릴리스마다 `download_count`와 명단으로 3영업일 뒤 최신 버전 비율을 보고, 80% 미만이 두 번 이어지면 X6·X7을 앞당긴다.
- U3: D2를 뒤집는다는 기록, 전제 5 대체, M7 상태 확인 필요, 파일럿 1주차 SmartScreen·백신 차단 수 세기 유지.
- `updated_to` 쓰기가 실패하면(읽기 전용 설정) 로그만 남기고 설치는 계속한다.
- 상태 잠금은 두 순서(수동 확인 중 타이머, 설치 재확인 중 타이머)로 시험하고, 시간 상수(30초·6시간·60초·10초)는 주입 가능한 시계나 짧은 상수로 시험한다.
- 4단계에서 Windows NSIS 로그 위치를 확인해 install.md 관리자 절에 적는다.
<!-- /autoplan-accepted:ceo -->

<!-- autoplan-accepted:design -->
- (디자인, CEO 줄 대체) 설정의 버전은 일반 탭 맨 아래 "앱 정보" 묶음의 "버전" 행이다. 로그 폴더 행과 한 묶음이고, `fieldset disabled={locked}` 밖에 두며, 설정을 읽지 못한 화면에도 보인다. 일반 탭 설명은 "회의록에 남는 이름, 알림, 로그와 앱 버전입니다."
- 버전 행: "버전 1.2.1"(`tabular-nums`)과 상태 줄(`text-label`, `ink-2`), 오른쪽 버튼 하나(평소 outline sm, `ready`만 brand sm). 버튼은 모든 상태에서 같은 자리에 남고 못 누를 때는 `aria-disabled`로 막아 초점을 지킨다. `sm` 미만 너비에서는 버튼이 글 아래로 내려간다.
- 상태 줄과 버튼은 5.4 표를 따른다: 개발 빌드, 확인한 적 없음, 방금 확인(3초), 최신, 7일 넘게 확인 못 함, 확인 중, 받는 중(퍼센트, 크기를 모르면 MB, 15초 무진행이면 "연결을 기다리는 중…"), 준비, 준비+녹음 중, 준비+가져오기·내보내기 중, 준비+저장하지 않은 설정, 설치 단계 3개(Windows는 창이 닫혔다 열린다는 말 포함), 재확인 철회, 재확인 더 새 버전, 수동 실패, 설치 실패 기록, 상태 불러오기 실패.
- 버전 숫자 뒤에는 "버전"을 끼워 조사를 붙인다("1.2.2 버전을", "1.2.2 버전으로"). 모든 업데이트 문구에 적용한다.
- (디자인, CEO 줄 대체) 버튼 문구는 설정, 홈, 첫 화면, 트레이 모두 "다시 시작해 업데이트" 하나다. 트레이는 `ready`일 때 맨 위에 그 항목과 구분선(순서: 다시 시작해 업데이트, 구분선, 열기, 종료).
- (디자인, CEO 줄 대체) 홈과 첫 화면에는 업데이트 줄이 많아야 하나다. 설정 줄과 같은 모양(`text-label`, `ink-2`, 끝에 link xs 버튼)이다. `ready` 줄은 닫기가 없고, 결과 줄은 `ready`가 아닐 때만 보인다. 순서는 오디오 확인 줄, 설정 줄, 업데이트 줄이다. 긴 줄은 줄바꿈하고 버튼은 글 끝에 붙는다. 첫 화면은 줄이 넘치면 세로 스크롤을 허용한다. 녹음 화면에는 보이지 않는다.
- 결과 줄: 성공 "1.2.2 버전으로 업데이트했습니다 · 바뀐 것"(그 버전의 공개 Release 페이지), 실패 "업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다 · 릴리스 페이지 열기". 닫기(×)는 32×32, `aria-label="닫기"`. 실패는 설정 버전 행에 설치 실패 기록으로 남는다.
- 노트: `scripts/latest-json.ts`가 릴리스 노트의 `## 바뀐 것` 절만 뽑아 마크다운 기호를 지운 평문 줄로 `notes`에 넣는다(절이 없으면 빈 문자열). 화면은 `ready`일 때 "바뀐 것 보기" 접기 단추(`aria-expanded`, 키보드로 열고 닫음)를 두고, 펼치면 줄마다 목록, 최대 12줄 높이에 안에서 스크롤, 긴 줄과 주소는 줄바꿈. 노트가 비면 단추를 두지 않는다.
- 설치 준비: "다시 시작해 업데이트"를 누르면 곧바로 앱 전체가 `installing`이 된다. 창 본문에 `inert`를 걸고, 단계 문구("업데이트를 확인하는 중…", "작업을 마무리하는 중…", "업데이트하는 중…")를 버전 행·업데이트 줄과 앱 알림 띠(`App.tsx`의 `banners` 자리)에 보인다. 어느 화면에서 눌렀든 보인다. 재확인이 철회나 더 새 버전으로 끝나 정리 전에 돌아오면 `inert`를 풀고 5.4 표의 해당 문구를 보인다.
- Rust는 `installing`에서 녹음 시작, 가져오기, 두 번째 `install_update`를 거절한다. 녹음 시작과 설치 판정은 같은 잠금 안에서 한다(녹음이 막 시작되는 중이면 설치가 `Err("recording")`). 가져오기·내보내기 중이면 `install_update`는 `Err("file_work")`이고 화면은 대화상자 대신 버튼을 막는다.
- 저장하지 않은 작업: 설치 전에 화면은 모든 회의의 편집 대기분(`Meeting.tsx`의 `saves`)을 바로 저장하고 끝까지 기다린다. 저장이 실패하면 설치를 시작하지 않고 "회의 문서에서 고친 내용을 저장하지 못해 업데이트를 멈췄습니다"를 보인다. 설정의 저장하지 않은 변경은 자동으로 저장하거나 버리지 않고 설치 버튼을 막는다(5.4 표 문구). 트레이와 홈 경로도 같은 판정을 거친다.
- 오류 문구: 원문 오류는 `app.log`에만 남기고 화면은 분류별 문구를 보인다. 연결 실패·시간 초과·60초 무진행은 "업데이트 서버에 연결하지 못했습니다. 인터넷 연결을 확인하고 다시 시도하세요". 서명 불일치·손상은 "받은 업데이트 파일이 올바르지 않아 버렸습니다. 다시 시도하세요". 피드 형식 오류나 이 플랫폼 항목 없음은 "지금은 이 PC용 업데이트를 받을 수 없습니다. 릴리스 페이지에서 받으세요". 그 밖은 "업데이트하지 못했습니다. 릴리스 페이지에서 받으세요". 분류는 순수 함수로 둔다.
- 설치 실패 기록: 설치 실패(교체 실패, 설치 프로그램 실행 실패, 10초 대기 초과, 다음 시작에 `just_updated.ok`가 거짓)가 나면 `install_failed: {version, count}`를 `updated_to`와 같은 곳에 적는다. 그 버전은 다시 받지 않고 버전 행에 "앱에서 설치하지 못했습니다" 행과 릴리스 페이지 링크를 보인다. 더 새 버전을 찾으면 기록을 지우고 평소대로 받는다.
- 마지막 성공 확인 시각(`last_check_ok`)을 같은 곳에 적는다. 7일을 넘으면 설정 버전 행에만 "7일 넘게 업데이트를 확인하지 못했습니다". 자동 확인 실패는 홈에 띄우지 않는다.
- 녹음 중에는 자동 확인을 건너뛴다(다음 주기에 다시 본다). 개발 빌드는 확인하지 않는다.
- 오디오 확인(5.7): 이전 설치의 흔적(설정 파일이나 회의 폴더)이 있는데 `last_run_version`이 없거나 1.2.0 미만이면 macOS에서 `audio_check_needed`를 켠다. 켜져 있으면 홈과 첫 화면 맨 위에 "Mac 권한이 바뀌어 오디오를 다시 확인해야 합니다 · 오디오 확인"(설정 > 오디오로 이동). 설정 > 오디오의 시스템 오디오 테스트가 통과하면 끈다. 녹음은 막지 않는다. 시작할 때마다 `last_run_version`을 지금 버전으로 적는다. 0단계 실측이 ad-hoc 대체로 끝나면 macOS 업데이트 뒤마다 같은 표시를 켠다(5.4 마지막 줄의 안내와 5.8 첫 줄의 "홈과 녹음 시작에서 안내"를 이 장치로 한다).
- 접근성: 상태 줄은 단계가 바뀔 때만 sr-only `role=status`로 알린다(퍼센트마다 알리지 않는다, `Home.tsx:455` 방식). 색과 초점 고리는 기존 토큰을 쓴다. 대화상자는 "취소"에 첫 초점, 닫히면 연 버튼으로(트레이에서 열었으면 창 본문으로) 초점을 돌린다.
- 공개 저장소 README에 동료용 설치 안내를 둔다(첫 설치 "그래도 열기", Windows SmartScreen "추가 정보 → 실행", 업데이트 뒤 오디오 확인). 동료는 비공개 install.md를 열 수 없다. 공개 Release 본문은 지금 노트처럼 "## 설치 파일" 표가 맨 위다.
- 디자인 시스템 6.7에 "앱 정보" 묶음과 버전 행, 6.4·6.5에 업데이트 줄과 오디오 확인 줄의 순서를 적는다.
- 미리보기(`preview.html`)와 fixture: 5.4 표의 모든 행, 홈에 오디오 확인 줄+설정 줄+업데이트 줄이 함께 있는 장면, 결과 줄 성공·실패, 노트 펼침(긴 노트), 설치 중 알림 띠, 처리 중 대화상자. 각 장면을 520×600과 1040×720에서 본다.
- 화면 테스트: 5.4 표의 행별 문구와 버튼(막힘 이유별), 상태가 바뀌어도 버튼 초점 유지, 홈 줄 순서와 업데이트 줄 하나만(`ready`가 결과 줄을 가림), 노트가 비면 단추 없음, 저장하지 않은 설정이면 설치 막힘, 회의 편집 저장 실패면 설치 안 함, `installing`에서 `inert`와 알림 띠, 트레이 경로가 같은 판정을 거침.
- Rust 테스트: `installing`에서 녹음 시작·가져오기·두 번째 설치 거절, 가져오기·내보내기 중 `Err("file_work")`, 설치 실패 기록이 있으면 그 버전을 받지 않고 더 새 버전이 기록을 지움, `audio_check_needed` 판정(흔적 있고 `last_run_version` 없음 → 켬, 1.1.1 → 켬, 1.2.0 → 끔, 흔적 없음 → 끔, Windows → 끔), 개발 빌드는 확인 안 함, 녹음 중 자동 확인 건너뜀, 오류 분류 함수(원문 → 4가지 문구), 7일 판정.
- `scripts/latest-json.ts` 테스트: `## 바뀐 것` 절만 뽑음, 마크다운 기호 제거, 절이 없으면 빈 노트.
- (디자인 재리뷰) 카드 위치: 창 너비 760px 이상이면 `fixed left-6 bottom-6`, 폭 320px(`max-w-80`), 아래 스택과 같은 z-20(전사본 패널 z-30, 대화상자 z-50 아래). 760px 미만이거나 세로 창이면 아래 스택(`App.tsx:528`의 독·보내기 막대·저장 막대) 바로 위에 왼쪽 정렬로 올린다. 카드가 보이는 동안 화면 아래 여백(`pb-24`), 회의 문서의 캐럿 보호, 좁은 창 전사본 패널(`MeetingParts.tsx:641`)의 아래 경계를 카드 높이만큼 늘린다. 카드는 저장 막대, 보내기 막대, 독, 전사본 패널, 설정 버전 행을 가리지 않는다.
- (디자인 재리뷰) 카드 모양: `surface` 바탕, `rounded-float`, `shadow-float`, 안쪽 여백 16px. 제목 `body` 500 `ink`, 보조 글 `label` `ink-2`. 버튼 outline sm, 링크 link xs, 닫기 × 32×32 `aria-label="업데이트 알림 닫기"`. 경고색과 반복 애니메이션은 쓰지 않는다. 나타남·사라짐은 `dur.small`, 움직임 줄이기 설정에서는 불투명도만.
- (디자인 재리뷰) 카드 접근성: 이름 있는 비모달 영역(`role="region" aria-label="앱 업데이트"`), 나타날 때 자동 초점 없음. 처음 나타날 때와 단계가 바뀔 때만 sr-only `role=status`로 한 번 알리고, 설정 버전 행과 겹쳐 읽지 않는다. DOM 순서는 화면 본문 뒤, 아래 스택 앞. ×로 닫으면 초점을 화면 본문(screenBox)으로 돌린다.
- (디자인 재리뷰) 닫기 수명: "이번 실행"은 앱 프로세스 수명이다(창 숨김, 다시 열기, 화면 이동으로는 다시 보이지 않는다). 닫기는 (종류, 버전)으로 기억한다: 준비, 결과, 14일 알림은 따로이고 다른 버전이 준비되면 다시 보인다. 사용자가 시작한 설치 흐름(카드, 설정, 트레이)의 막힘 이유, 준비·설치 단계, 재확인 결과, 저장 실패, 사전 점검 실패는 닫았어도 카드에 다시 보인다. 설치 중에는 ×가 없다.
- (디자인 재리뷰) (디자인·Eng 줄 대체) 설치 중 피드백: 단계 문구는 카드 하나에 보인다(설정 화면이면 버전 행에도). 앱 알림 띠에는 보이지 않는다. 카드는 `inert` 범위 밖이고, 준비가 시작되면 초점을 카드 상태 글(`tabIndex=-1`)로 옮긴다. `inert`는 화면 본문, 헤더, 독, 아래 스택, `document.body`에 붙는 전사본 포털에 걸고, 전역 단축키(`App.tsx:355` 녹음 단축키 등)는 준비·설치 중 무시한다. `needs_confirm`이면 잠금을 풀고 대화상자의 "취소"로, 실패·철회·막힘이면 눌렀던 버튼(없으면 카드 버튼)으로 초점을 돌린다. 문구: 준비 "업데이트를 준비하는 중…", 재확인 "마지막으로 확인하는 중…", 마무리 "작업을 마무리하는 중…", 설치 "업데이트하는 중…".
- (디자인 재리뷰) 재확인 결과: 다른 버전이 나오면 카드는 사라지지 않고 "다른 버전을 받는 중 42%"를 보이다가 준비 카드로 돌아간다(다시 누르는 것은 사용자). 철회 문구 "이 업데이트는 취소되었습니다"는 ×를 누르거나 다음 상태가 될 때까지 남는다.
- (디자인 재리뷰) 상태 표: 5.4 표를 한 표로 다시 쓴다. 열은 상태, 설정 상태 줄, 설정 버튼, 카드 글, 카드 버튼, 트레이, 닫기 가능. 빠진 행을 더한다: Slack 보내기 중 막힘, macOS 위치 사전 점검 실패(카드 버튼은 "릴리스 페이지 열기"), 설치 실패 재시도(`count < 2`면 "다시 시도", 두 번째 실패부터 링크만), 준비 단계, 회의 편집 저장 실패, 14일 확인 실패. "더 새 버전이 나와"는 "다른 버전이 나와"로. 업데이트 줄 시대의 문장은 10장 접기(DX)에서 지운다.
- (디자인 재리뷰) 확인 대화상자의 확인 버튼은 "다시 시작해 업데이트"다(앱이 다시 시작된다는 것을 다시 알린다).
- (디자인 재리뷰) Windows 설치 창: `bundle.windows.nsis.languages: ["Korean"]`(첫 설치에도). 설치 단계 문구는 Windows "설치 창이 잠깐 보인 뒤 앱이 다시 열립니다", macOS "앱이 잠시 닫혔다가 다시 열립니다".
- (디자인 재리뷰) (디자인 줄 대체) 결과 카드의 "바뀐 것": 설치 직전 `update-state.json`에 `notes`를 함께 적고, 결과 카드 안에서 같은 노트 목록 컴포넌트로 펼친다(최대 높이, 안에서 스크롤). 공개 Release 페이지(맨 위가 설치 파일 표)로 보내지 않는다. 노트가 비면 링크가 없다.
- (디자인 재리뷰) 디자인 시스템에 "업데이트 카드" 절을 새로 두고, 6.2의 "앱에서 유일하게 살아 있는 요소" 문장을 고친다(카드가 두 번째 떠 있는 요소). 9장 문서 변경에 더한다.
- (디자인 재리뷰) 미리보기: 520×880 회의 문서(보내기 막대와 카드), 전사본 패널을 연 상태와 카드, 설정 다른 탭의 저장 막대와 카드, 520×600 첫 화면과 카드, 1040×720 넓은 창 왼쪽 아래. 카드 상태별 장면: 준비, 막힘 이유별, 준비 단계, 설치 단계, 다른 버전 받는 중, 철회, 성공, 실패, 14일 알림, 닫은 뒤. 녹음 중과 설정 일반 탭에서 숨김.
- (디자인 재리뷰) 화면 테스트: 닫기 키(종류, 버전), 사용자가 시작한 흐름은 닫힘을 무시, 설치 중 × 없음, 초점 이동(준비 시작, 대화상자 취소, 실패), 준비·설치 중 전역 단축키 무시, 녹음 중과 일반 탭에서 숨김, 좁은 창에서 카드가 아래 스택 위로 올라가고 여백이 늘어남, 14일 판정.
<!-- /autoplan-accepted:design -->

<!-- autoplan-accepted:dx -->
- (DX) 10장 우선 규칙: 시간순, 같은 주제는 가장 나중 줄이 본문보다도 이긴다. 구현 첫 작업으로 10장을 본문(5~9장)에 접어 넣고 10장을 결정 기록으로 바꾼다. 상태 표는 5.4 하나만 둔다.
- (DX, CEO·디자인 줄 대체) IPC 계약: Rust는 serde 태그 열거형, 화면은 `src/lib/ipc.ts`의 같은 모양 판별 유니온을 쓴다. `update_state` → `{ version, state, blocked, just_updated, failed_install, stale }`. `state`는 `idle | checking | up_to_date | downloading{version, received, total?, waiting} | ready{version, notes} | installing{stage: recheck | finishing | installing} | failed{kind, manual}`. `blocked`는 `recording | file_work | null`(Rust가 판정, 저장하지 않은 설정과 회의 편집은 화면이 판정). `app://update` 이벤트의 내용은 `update_state`와 같다. 화면은 이벤트를 먼저 구독한 뒤 `update_state`를 한 번 부른다.
- (DX, CEO·디자인 줄 대체) `install_update(confirmed)`는 오류 문자열 대신 결과를 돌려준다: `started | needs_confirm | blocked{reason} | not_ready`. `needs_confirm`이면 상태는 `ready` 그대로 대화상자를 띄우고, 취소하면 아무것도 바뀌지 않는다. `installing`은 `started` 뒤부터다. 화면은 회의 편집 저장(디자인 줄)을 `install_update`를 부르기 전에 끝낸다. 실제 실패만 `Err`다.
- (DX, 디자인 줄 대체) 오류 분류: Rust의 순수 함수가 원문 오류를 `kind`(`network | signature | feed | other`)로 나누고, 4가지 한국어 문구는 화면(TS)이 `kind`로 고른다. 사용자 문구는 화면 한 곳에만 둔다. `app.log`에는 `kind`, 단계, 대상 버전, 원문을 한 줄로 남긴다.
- (DX, CEO·디자인 줄 대체) 업데이터 상태는 설정이 아니라 앱 데이터 폴더의 `update-state.json`(Rust만 쓴다, 저장 막대와 읽기 전용 설정과 무관)에 둔다: `{ schema_version, updated_to, install_failed{version, count}, last_check_ok, last_run_version, audio_check_needed }`. 모르는 필드는 무시하고, 파일이 깨지면 기본값으로 읽고 로그를 남긴다. 쓰기가 실패하면 로그만 남기고 설치는 계속한다(결과 줄과 실패 기록은 잃을 수 있다).
- 오디오 확인 판정: macOS에서 이전 설치의 흔적(설정 파일이나 회의 폴더)이 있는데 `update-state.json`이 없거나, `last_run_version` < `AUDIO_RECHECK_BELOW`(이름 붙은 상수, 처음 값 1.2.0)이면 `audio_check_needed`를 켠다. 자체 서명 인증서를 바꾸면 이 상수를 그 버전으로 올린다(관리자 문서와 build.yml 해시 검사 옆 주석에 적는다).
- 빌드별 업데이터: `MS_UPDATER` 없는 빌드(`tauri dev`, 미리보기, README 로컬 설치 파일)는 확인하지 않는다. CI 태그 빌드는 공개 피드, CI 수동 빌드는 공개 피드를 확인하지만 공개 저장소에 올리지 않는다. 업데이터 시험 빌드는 시험 `--config`의 로컬 끝점을 쓴다. build.yml은 배포용 설정 파일에 `dangerousInsecureTransportProtocol`이 있으면 실패한다.
- 관리자 문서(install.md "관리자용: 설치 파일 만들기"에 둔다. README와 아키텍처 11장이 링크한다). 복사해 그대로 실행하는 명령으로 쓴다: `bun tauri signer generate -w ~/.tauri/meeting-summarizer.key`, openssl로 자체 서명 코드 서명 인증서 만들기(`extendedKeyUsage=codeSigning`, 20년)와 p12 내보내기, `base64 -i cert.p12 | gh secret set MACOS_SELF_SIGNED_P12`, `gh secret set TAURI_SIGNING_PRIVATE_KEY < key`, `gh variable set SLACK_AUTO --body false`, 공개 저장소 만들기(README 커밋), GitHub App 만들기(공개 저장소 하나에만 설치, Contents 읽기·쓰기)와 키 등록, `codesign -d -r- <앱>`으로 `certificate root` 해시 읽기. 각 값의 형식, 등록 위치, 성공 확인을 함께 적는다.
- 관리자 문서의 복구 절: 업데이터 키, 인증서, GitHub App 키 각각에 대해 "백업에서 되살리기"와 "영구 분실"을 나눈다. 업데이터 키 영구 분실은 모든 사람이 손으로 다시 설치, 인증서 영구 분실은 새 인증서 + `AUDIO_RECHECK_BELOW` 올리기 + 해시 바꾸기, App 키 분실은 새 키 등록 후 `publish`만 다시 실행. 6장 3단계는 두 번째 보관자가 백업한 키로 시험 파일에 서명·검증해 본 뒤에 끝난다.
- build.yml에 왜 `APPLE_CERTIFICATE`가 아닌지 주석을 둔다(Tauri는 Apple 접두어가 없는 인증서를 찾지 않는다, `tauri-macos-sign` `identity.rs`).
- 업데이터 시험 하네스: `scripts/updater-test.ts`(bun)가 시험 빌드의 `.app.tar.gz`와 `.sig`로 한 플랫폼짜리 `latest.json`을 만들고 `127.0.0.1:8765`에서 내준다. 관리자 문서에 0·2단계 절차를 순서대로 둔다: 지금 앱 종료와 앱 데이터·키체인 항목 백업, 두 버전 빌드(`MS_UPDATER=1 APPLE_SIGNING_IDENTITY=… TAURI_SIGNING_PRIVATE_KEY=… bun run tauri build --config src-tauri/tauri.updater.conf.json --config <시험 설정>`), 하네스 실행, 격리 속성 붙여 설치, 확인 목록, 원상 복구. 인증서는 0단계에서 만든 것을 쓴다.
- 태그 배포 체크리스트(관리자 문서): `package.json`·`src-tauri/Cargo.toml`·`src-tauri/tauri.conf.json` 버전 올리기와 lockfile, `docs/releases/v<버전>.md`, 커밋, 태그. build.yml의 태그 검사는 세 파일 모두를 태그와 비교한다(지금은 `tauri.conf.json`만).
- `scripts/latest-json.ts` 사용법: `bun scripts/latest-json.ts --tag vX.Y.Z --notes docs/releases/vX.Y.Z.md --assets <폴더> --repo sijecorp/meeting-summarizer-releases --out latest.json`. 성공 0, 실패 1이고 빠진 플랫폼·파일·필드와 고칠 방법을 한 줄씩 출력한다.
- 릴리스 노트 계약과 틀(`docs/releases/TEMPLATE.md`): 맨 위 `## 설치 파일` 표, 그다음 `## 바뀐 것`(앱이 보이는 부분, 업데이트 전에 꼭 읽어야 할 말도 여기), `## 알려진 제한`. 설치 안내 링크는 공개 저장소 README로. 태그 빌드에서 노트 파일이 없거나 `## 바뀐 것`이 없으면 `publish`가 실패한다. 공개 저장소에는 `--generate-notes`를 쓰지 않는다(PR 제목과 작성자가 공개된다). install.md의 동료용 설치 절차는 공개 README를 가리키고 두 벌을 두지 않는다. 공개 README와 1.1.1 → 1.2.1 손 설치 안내는 6장 4단계의 알림 전에 끝낸다.
- CI 오류 문구는 문제, 원인, 고칠 방법을 `::error::` 한 줄에 쓴다(build.yml:55 방식). 자체 서명 시크릿 없음("태그 빌드는 자체 서명이 필요합니다. 관리자 문서의 인증서 절로 MACOS_SELF_SIGNED_P12를 등록하세요"), 해시 불일치(기대값과 실제값, "인증서를 바꿨다면 해시와 AUDIO_RECHECK_BELOW를 올리세요"), 공개된 태그("공개된 태그는 다시 빌드하지 않습니다. 버전을 올려 새 태그를 만드세요"), 노트 계약 위반, `latest-json` 검증 실패, GitHub App 토큰 실패("RELEASES_APP_ID·RELEASES_APP_KEY와 App 설치 저장소를 확인하세요"), 버전 불일치(세 파일 값과 태그).
- 잘못된 릴리스 절차: 모든 `gh` 명령에 `--repo sijecorp/meeting-summarizer-releases`를 붙이고, 되돌린 뒤 `curl -sL https://github.com/sijecorp/meeting-summarizer-releases/releases/latest/download/latest.json | jq -r .version`으로 확인한다. 제한도 적는다: 이미 설치한 앱은 내려가지 않는다, 설치 직전 재확인이 네트워크 오류로 끝난 앱은 받아 둔 버전을 설치할 수 있다.
- 측정 명령(관리자 문서): `gh api repos/sijecorp/meeting-summarizer-releases/releases/tags/vX.Y.Z --jq '.assets[]|select(.name|test("tar.gz$|setup.exe$"))|"\(.name) \(.download_count)"'`. 재시도와 끊긴 받기도 세므로 실제보다 많다고 적는다.
- 테스트: `latest-json.ts`의 인자 누락·플랫폼 누락·`## 바뀐 것` 없음 실패 문구와 종료 코드, `update-state.json` 읽기(없음·깨짐·모르는 필드)와 쓰기 실패, `AUDIO_RECHECK_BELOW` 판정, `MS_UPDATER` 없는 빌드의 상태, `install_update` 결과 4가지와 `needs_confirm` 뒤 취소 시 상태 그대로, Rust 상태 열거형의 JSON 모양 스냅샷(TS 유니온과 맞는지), build.yml의 세 파일 버전 검사와 시험 설정 검사.
<!-- /autoplan-accepted:dx -->

<!-- autoplan-accepted:eng -->
- (Eng, CEO 줄 대체) 공개 피드 승격: `publish` 작업은 `concurrency: {group: publish-releases, cancel-in-progress: false}`로 한 번에 하나만 돈다. 그 안에서 공개 저장소의 지금 latest와 이 태그를 semver로 비교해 더 새로울 때만 `--latest`, 아니면 `--latest=false`로 공개한다. 공개 뒤 `curl -sfL …/latest/download/latest.json | jq -r .version`이 가장 높은 공개 태그와 같은지 확인하고 다르면 실패한다. "latest로 할지" 판정은 순수 함수와 테스트로 둔다. 롤백 절차는 진행 중인 `publish`가 끝난 뒤에 한다(관리자 문서).
- (Eng) release·publish 조건은 `github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')`다(지금 `build.yml:292`는 태그만 본다. 태그를 고른 수동 실행이 공개하지 않게 한다).
- (Eng, CEO 줄 대체) 설치 대기 시간은 상수 `INSTALL_WAIT = Limits::reap + 5초`(지금 15초)이고 정리 단계 전체(녹음 마무리, 내보내기 취소, 엔진·워커 종료)에 걸린다. `recorder.stop`과 `exports.cancel_all`처럼 막히는 정리는 메인 스레드 밖에서 돈다. 넘으면 설치하지 않고 지금 버전으로 다시 시작한다. 테스트: `INSTALL_WAIT > reap`.
- (Eng) 엔진 판정은 띄우는 중인 사이드카도 센다: `sidecar.rs`의 세대 확인과 자식 등록 사이를 생명주기 잠금 안의 "띄우는 중" 수로 막고, 설치 대기는 띄우는 중·큐 작업·자식·정리 중이 모두 0일 때 끝난다. 테스트: 세대 확인과 등록 사이에 종료.
- (Eng, 디자인 줄 대체) 설치 실패 기록은 다시 시도할 수 있다: 실패 행에서 사용자가 "업데이트 확인"을 누르면 기록을 한 번 지우고 다시 받는다(`count < 2`). 두 번 실패하면 그 버전은 링크만 보인다. 자동 확인은 기록이 있는 버전을 받지 않는다.
- (Eng, 디자인·DX 줄 대체) 설치 수락 판정은 잠금 둘을 엇갈려 잡지 않는다: 전역 `INSTALLING: AtomicBool`(SeqCst, `QUIT_CONFIRMED`와 같은 방식). 설치는 깃발을 먼저 세운 뒤 녹음·파일 작업·Slack 보내기 수를 보고, 하나라도 있으면 깃발을 내리고 `blocked{reason}`을 돌려준다. 녹음 시작, 파일 작업, Slack 보내기는 자기 수를 먼저 올린 뒤 깃발을 보고 서 있으면 수를 내리고 물러난다. 테스트: 두 순서 모두.
- (Eng) 파일 작업 보호: 회의 마크다운 내보내기, 녹음 내보내기, 오디오 가져오기, 모델 가져오기는 명령에 들어오자마자(저장 대화상자와 작업 시작 전에) 보호 객체(RAII 수)를 잡고 마지막 쓰기나 취소까지 쥔다(`commands.rs:257`, `:767`, `:842`). `Exports`의 실행 여부는 공개 접근자로 읽는다.
- (Eng) Slack 보내기 보호: 보내기는 같은 방식의 수를 잡고 전송과 `Sent`·`also_sent` 저장이 끝날 때까지 쥔다(`slack.rs:944` 이후). 그동안 설치는 `blocked{reason: sending}`이고 화면 문구는 "Slack 보내기가 끝나면 업데이트할 수 있습니다".
- (Eng, DX 줄 대체) 화면 준비 단계: "다시 시작해 업데이트"를 누르면 화면이 먼저 앱 전체 "준비 중"(`inert`, 편집·이동 막음)이 되고, 모든 회의의 저장 대기분을 끝까지 비운 뒤 `install_update`를 부른다. `needs_confirm`이면 준비를 풀고 대화상자, 확인하면 다시 준비·비우기 뒤 `true`로 부른다. `blocked`·오류면 준비를 풀고 이유를 보인다. 설정은 저장하지 않은 변경이 있거나 저장 중(`saving`, 키체인 정리 포함)이면 막고, 이 상태는 App이 알 수 있게 올린다.
- (Eng) 종료 중재: `INSTALLING`이 서 있으면 트레이·앱 메뉴의 "종료"(`lib.rs:398` `app.exit(0)`)와 `ExitRequested`는 미뤄지고, 업데이터가 하는 다시 시작만 나갈 수 있다. 테스트: 설치 중 메뉴 종료, 설치 성공 뒤 다시 시작.
- (Eng) 다시 시작과 스레드: `install_update`는 비동기 명령(`#[tauri::command(async)]`)이고 설치·다시 시작은 메인 스레드 밖에서 부른다(macOS 관리자 암호 대체 경로가 메인 스레드를 기다린다). 다시 시작은 `app.request_restart()`로 해 `RunEvent::Exit`가 돌게 한다(단일 실행 소켓 정리). `shutdown_work`는 두 번 불려도 안전하다. 테스트: 두 번 부르기.
- (Eng) macOS 사전 점검: `started` 전에 앱 위치가 `/AppTranslocation/`이나 `/Volumes/` 아래인지, 임시 폴더와 같은 볼륨인지, 앱 폴더에 쓸 수 있는지 본다. 하나라도 걸리면 작업을 멈추지 않고 "앱에서 설치할 수 없는 위치입니다. 응용 프로그램 폴더로 옮기거나 릴리스 페이지에서 받으세요"와 링크를 보인다. 쓸 수 없을 때 플러그인이 띄우는 관리자 암호 창은 install.md에 적는다.
- (Eng) 배포 파일 검사: macOS 작업은 `.app.tar.gz`를 임시 폴더에 풀어 `codesign --verify --deep --strict`와 `certificate root` 해시를 본다. `publish`는 두 `.sig`를 `tauri.conf.json`의 공개 키로 검증하고(minisign), 공개 뒤 `latest.json`의 두 URL에 HEAD 요청을 보낸다.
- (Eng) `plugins.updater.requireSignedVersion: true`(사내 TLS 검사 프록시가 `latest.json`을 바꿔 옛 서명 파일을 새 버전으로 속이는 다운그레이드를 막는다). 2단계에서 확인한다.
- (Eng, CEO 줄 대체) 5.8 첫 줄을 나눈다: 업데이트 뒤 마이크·시스템 오디오(TCC) 권한이 풀리면 ad-hoc + 안내. 키체인 창만 뜨면 자체 서명을 유지하고 업데이트 뒤 줄에 "항상 허용"을 누르라는 말을 붙인다(install.md:65).
- (Eng, DX 줄 대체) macOS에서 `MS_UPDATER=1` 빌드는 자체 서명 인증서가 있어야 한다. 인증서 시크릿이 없는 수동 빌드는 `MS_UPDATER` 없이 빌드한다(ad-hoc 앱이 업데이터로 서명을 바꿔 오디오 확인을 건너뛰지 않게).
- (Eng) CI 임시 키체인: `security set-keychain-settings -lut 21600 "$KC"`, 서명 직전 다시 `unlock-keychain`, `if: always()` 단계에서 지운다(20~50분 컴파일 동안 잠기지 않게).
- (Eng, 계획 사실 고침) plugin 2.14.1은 받기에 이미 `timeout: None`을 쓴다(`updater.rs:735`). `update.timeout = None` 줄은 빼고 60초 무진행 감시는 둔다. `latest.json`은 JSON이 깨질 때만 모든 플랫폼이 멈추고, 플랫폼 하나가 빠지면 그 플랫폼만 멈춘다(3.4 고침). `ReleaseNotFound`(공개 Release 없음, 롤백 중)는 `feed`로 나누고 자동 확인에서는 조용히 넘어간다. 재확인 결과 문구는 "다른 버전이 나와 다시 받습니다"(낮은 버전으로 되돌린 경우도 맞게).
- (Eng, DX 줄 대체) `update_state`에서 `blocked`를 뺀다(녹음·파일 작업은 화면이 이미 알고, 막힘은 `install_update`의 `blocked{reason}`이 판정한다). 트레이 항목은 기존 `refresh_tray`(`lib.rs:193`) 안에서 다시 만든다(녹음 중 숨김이 그대로 된다).
- (Eng) 자동 확인 주기: 30분마다 깨어나 마지막 시도 시각(`update-state.json`)과의 벽시계 차이가 6시간 이상이면 확인한다(잠자기 동안 tokio 타이머가 멈추기 때문). 설치 직전 재확인 시간 제한은 10초로 줄인다.
- (Eng) `update.rs`는 `unwrap`·패닉 없이 쓰고 기존 독 허용 `lock()`을 쓴다(릴리스 빌드는 패닉에 중단된다).
- (Eng) 6장 0단계 전에 `tccutil reset All com.sije.meeting-summarizer`를 하고, 시험 동안 `tauri dev`를 띄우지 않는다.
- (Eng) 위험 표에 더한다: 업데이터 서명 키가 tauri-action 단계 환경에 있어 크레이트 빌드 스크립트·bun postinstall·llama.cpp CMake가 읽을 수 있다(받아들임, 의존성 갱신은 리뷰). Windows 반쯤 끝난 NSIS 설치(새 exe, 옛 사이드카)는 `just_updated.ok`로 못 잡는다(4단계 확인으로 받아들임).
- (Eng) `slack-release` 수동 실행은 노트 링크를 공개 저장소 주소로 기본 설정한다. 1.2.1 이후 Slack 문구는 "앱에서 업데이트됩니다, 처음 설치만 이 파일로"로 바꾼다.
- (Eng) 테스트 추가: latest 판정 함수, CI `.sig` 검증과 풀린 앱의 codesign·해시 검사, `INSTALL_WAIT > reap`, `shutdown_work` 두 번, 설치·녹음·파일 작업·보내기 깃발 두 순서, 띄우는 중 사이드카, `ReleaseNotFound` 분류, 벽시계 주기, 설치 실패 재시도(`count`), 설치 중 메뉴 종료 미룸, macOS 사전 점검 판정(경로 문자열 입력), 화면 준비 단계(비우는 동안 입력이 막힘, 취소 시 풀림, 설정 저장 중 막힘).
- (Eng 재리뷰, Eng 줄 대체) 준비 예약: 화면은 준비(잠금)에 들어가기 전에 `prepare_update`를 부른다. Rust는 이때 `INSTALLING`을 세우고 녹음·파일 작업·Slack 보내기 수를 본다. 하나라도 있으면 깃발을 내리고 `blocked{reason}`을 돌려주며, 화면은 잠그지 않고 이유를 보인다(비우기를 하지 않는다). 막힘이 없으면 화면이 잠그고 회의 편집을 비운 뒤 `install_update(confirmed)`를 부른다. `install_update`는 이 준비가 세운 깃발이 없으면 `not_ready`다. `needs_confirm`, 저장 실패, 오류면 `cancel_update`로 깃발을 내린다. 깃발이 서 있는 동안 트레이·앱 메뉴의 종료와 `ExitRequested`는 미뤄지고, 준비가 설치 없이 끝나면 미룬 종료를 그때 한다. 화면이 60초 안에 `install_update`나 `cancel_update`를 부르지 않으면 Rust가 깃발을 내린다(화면이 멈춰도 종료를 막지 않게).
- (Eng 재리뷰, Eng 줄 대체) 정리는 한 번만 돈다: `shutdown_work`는 처음 부른 쪽만 실제로 정리하고, 나중에 부른 쪽(설치 시간 초과 뒤 `request_restart`가 부르는 `RunEvent::Exit` 등)은 처음 정리가 끝나기를 남은 마감 시각까지만 기다리고 돌아온다. 설치는 시작할 때 마감 시각 하나(지금 + `INSTALL_WAIT`)를 정하고 정리 전체와 그 뒤의 `Exit`가 함께 쓴다. 마감을 넘기면 막힌 정리를 기다리지 않고 지금 버전으로 다시 시작한다. 녹음 수는 녹음 마무리(`Recorder::stop`의 join, `audio/mod.rs:286`)가 끝날 때까지 쥔다(마무리 중인 녹음은 `blocked{recording}`). 독립 리뷰어가 제안한 Windows 대기·중단 줄이기는 하지 않는다(승인된 결정 유지, 아래 리뷰 기록).
- (Eng 재리뷰) `update_state` 스냅샷과 `app://update` 이벤트에 단조 증가 `rev`를 붙이고, 화면은 가진 것보다 작은 `rev`를 버린다(늦게 온 `ready` 스냅샷이 `installing`을 덮지 않게. `App.tsx:137`의 큐 초기화와 같은 문제).
- (Eng 재리뷰, DX 줄 대체) IPC 모양을 카드에 맞춘다: `downloading{version, received, total?, waiting, origin}`의 `origin`은 `background | manual | replacement`(재확인에서 다른 버전)다. 철회는 `update_state.notice: withdrawn{version} | null`로 싣고 다음 상태가 되거나 화면이 ×로 지울 때까지 둔다. `just_updated`는 `{version, ok, notes}`다. `update-state.json`에 `pending_notes`(설치 직전 적고 결과를 보인 뒤 지움), `first_seen_at`, `attempt{version, id}`를 더한다.
- (Eng 재리뷰, Eng 줄 대체) 설치 실패 재시도: `install_failed{version, count}`는 더 새 버전을 찾을 때만 지운다. 실패 행의 "다시 시도"는 `count < 2`일 때만 보이고, 누르면 기록은 그대로 둔 채 그 버전을 한 번 다시 받는다. 설치 직전 `attempt`를 적고 실패는 그 시도에서 한 번만 센다: 설치 단계에서 본 실패(시간 초과, 교체 실패)를 적으면 `attempt`를 지우고, 다음 시작의 `just_updated.ok == false`는 `attempt`가 남아 있을 때만 센다.
- (Eng 재리뷰, 디자인 줄 대체) 카드 배치: 좁은 창(760px 미만이거나 세로 창)에서는 카드를 아래 스택(`App.tsx:528`의 `flex-col`) 안 첫 자식으로 넣고 왼쪽 정렬한다(스택이 쌓으므로 겹치지 않는다). 넓은 창에서는 `fixed left-6 bottom-6`이고, 카드와 아래 스택 안 막대(저장 막대, 보내기 막대, 독)의 측정한 사각형이 겹치면 좁은 창 배치로 바꾼다(800px 창의 설정 저장 막대, `Settings.tsx:1821`). 화면 아래 여백, 캐럿 보호, 전사본 패널의 아래 경계는 화면마다 따로 고치지 않고, 아래 스택(카드 포함)과 왼쪽 카드의 측정 높이를 `ResizeObserver`로 CSS 변수 하나(`--bottom-stack`)에 담아 쓴다.
- (Eng 재리뷰) 설정 탭 소유: `Settings`는 탭의 처음 값만 받는다(`Settings.tsx:173`). 탭이 바뀔 때 `onTab`으로 App에 알리고, 카드의 "바뀐 것 보기"는 App이 `notesIntent`(일반 탭으로 바꾸고 노트 펼치기)로 Settings에 보낸다. Settings는 다시 마운트하지 않는다(저장하지 않은 입력을 잃는다). 일반 탭 숨김(D13)과 잠긴 동안의 예외(D16)는 App이 아는 지금 탭으로 판정한다.
- (Eng 재리뷰) 준비·설치 중 바깥 진입: 알림 클릭(`App.tsx:202`), 파일 끌어놓기(`App.tsx:314`), 트레이의 화면 이동, 준비 전에 시작해 늦게 끝난 콜백은 `inert`와 단축키 차단을 거치지 않는다. 화면 이동(`go`)과 대화상자 열기 앞에 준비·설치 판정 하나를 두고 그동안 온 요청은 버린다(잠금이 풀린 뒤에 다시 하지 않는다).
- (Eng 재리뷰, 디자인 줄 대체) 오래 확인 못 함 판정: 14일 카드(D15)와 설정의 7일 문구는 이번 실행에서 확인 시도가 실패한 뒤에만 판정한다(시작하자마자 판정하지 않는다. 휴가 뒤 첫 실행의 거짓 경고를 막는다). 서버에 닿은 확인(`ReleaseNotFound` 포함)은 성공으로 센다. 기준 시각은 `last_check_ok`, 없으면 `first_seen_at`(이 기기에서 `update-state.json`을 처음 만든 시각, 깨진 파일에서 되살리면 그 시각)이다. 시계가 뒤로 가 차이가 음수면 0으로 본다.
- (Eng 재리뷰, Eng 줄 대체) 공개 피드 승격 판정은 "지금 latest"가 아니라 이미 공개된 모든 태그와 비교한다: 이 태그가 공개된 어떤 태그보다 높을 때만 `--latest`, 아니면 `--latest=false`. 롤백으로 latest를 낮춰도 늦게 도는 `publish`가 롤백한 릴리스를 다시 올리지 않는다. 공개 뒤 확인은 "가장 높은 공개 태그"가 아니라 이 판정이 기대한 latest(올렸으면 이 태그, 아니면 공개 전 latest)와 `latest.json`의 `version`을 비교한다.
- (Eng 재리뷰) 10·11장 접기(DX 줄)는 1단계의 첫 커밋으로 하고, 접은 뒤 본문에 대체된 옛 문장이 남지 않았는지 찾아 지운다: 5.2의 `Err("busy")`·`Err("recording")`·`update.timeout = None`·"최대 10초", 5.4의 홈·첫 화면 업데이트 줄과 앱 알림 띠, 3.4의 "플랫폼 항목 하나만 깨져도 모든 플랫폼", 10장 첫 CEO 줄의 "넘으면 경고 후 설치를 계속한다". 구현은 접은 본문만 보고 한다.
- (Eng 재리뷰) 깨진 릴리스 막기: (a) 관리자 태그 체크리스트에 "업데이터 경로(`update.rs`, `lib.rs`의 종료·다시 시작, `tauri.conf.json`의 `plugins.updater`, build.yml 서명 단계)가 바뀐 릴리스는 태그 전에 `scripts/updater-test.ts`로 지금 공개 버전에서 새 빌드로 업데이트를 이 Mac에서 한 번 돌린다"를 넣는다. (b) CI 시작 확인: 두 OS 빌드 작업이 만든 앱을 띄워 60초 안에 `app.log`에 버전 줄이 찍히는지 보고 끈다. 찍히지 않으면 태그 빌드는 공개하지 않는다(시작하자마자 죽는 버전은 앱 안에서 고칠 수 없다).
- (Eng 재리뷰, DX 줄 대체) `update-state.json`은 설정 파일 옆(`app_data_dir`, `lib.rs:332`, Windows는 로밍)이 아니라 회의 폴더처럼 `app_local_data_dir`(`lib.rs:325`)에 둔다. 값이 모두 이 기기의 설치에 대한 것이다(로밍 프로필이 다른 PC에 가짜 "업데이트하지 못했습니다"를 띄우지 않게). 쓰기는 `store::write_atomic`(`store.rs:259`).
- (Eng 재리뷰) 키 불일치: CLI는 서명 키와 공개 키가 맞지 않아도 경고만 남긴다(tauri-cli 2.12.2 `bundle.rs`). `publish`는 초안을 공개하기 전에 두 `.sig`를 `tauri.conf.json`의 공개 키로 검증하고 실패하면 공개하지 않는다. `latest-json.ts`는 `.sig`의 trusted comment에 든 `version:`이 태그와 같은지도 본다(`requireSignedVersion` 때문에 다르면 아무도 받지 못한다).
- (Eng 재리뷰, 디자인 줄 대체) Windows 설치가 실패하면 앱이 꺼진 채로 남는다(플러그인은 설치 프로그램을 띄운 뒤 바로 끝난다). Windows 설치 단계 문구는 "설치 창이 잠깐 보인 뒤 앱이 다시 열립니다. 1분이 지나도 열리지 않으면 시작 메뉴에서 여세요"이고 install.md에도 적는다. 마지막 로그는 플러그인의 `on_before_exit`에서 비운다.
- (Eng 재리뷰) 위험 표에 더한다: 0·2단계는 이 Mac 한 대(한 macOS 버전)에서만 본다. 최소 지원 14.6(`tauri.conf.json:45`)부터의 TCC·App Management·Gatekeeper 차이는 4단계와 파일럿에서 본다(받아들임). 2단계 확인 목록에 `xattr -l`의 `com.apple.provenance`를 더한다. 자체 서명에는 Team ID가 없어 키체인 "항상 허용" 창이 업데이트마다 뜰 수 있다: 0단계에서 보고, 뜨면 5.8의 키체인 줄(자체 서명 유지 + 안내)이 기본 경로다.
- (Eng 재리뷰) 모델 내려받기(`download_models`, `commands.rs:454`)도 파일 작업 보호에 넣는다(설치가 2.5GB 받기를 끊지 않게). 막힘 문구는 "파일 작업이 끝나면 업데이트할 수 있습니다"와 같다.
- (Eng 재리뷰) 받는 중 이벤트는 250ms나 1% 단위로 줄여 보낸다. 15초 무진행("연결을 기다리는 중…")은 조각이 오지 않는 것을 봐야 하므로 Rust 타이머로 판정해 `waiting`을 보낸다.
- (Eng 재리뷰, 사실 고침) NSIS 한국어는 이미 `tauri.windows.conf.json:9`(`"languages": ["Korean"]`)에 있어 바꿀 것이 없다(디자인 재리뷰 줄). Release 자산은 이제 `release-assets.githubusercontent.com`으로 넘어간다(2026-11-06 curl 확인). 위험 표와 install.md의 `objects.githubusercontent.com`을 고치고, 방화벽 허용 목록은 `github.com`과 이 주소다.
- (Eng 재리뷰, Eng 줄 대체) macOS 사전 점검은 `/AppTranslocation/`과 `/Volumes/` 두 가지만 본다. 임시 폴더와 같은 볼륨인지는 플러그인이 옮기기 전에 거절하고(`TempDirNotOnSameMountPoint`), 이 오류는 같은 위치 문구로 보인다. 앱 폴더에 쓸 수 없으면 막지 않고 플러그인의 관리자 암호 창에 맡기며 install.md에 적는다(쓸 수 없으면 막는다는 줄과 암호 창 안내가 서로 어긋났다).
- (Eng 재리뷰) 종료 중재는 `ExitRequested`의 `code`가 `tauri::RESTART_EXIT_CODE`(`tauri` 2.12.3 `app.rs:77`)이면 통과시킨다(`request_restart`만 나간다). `update.install(bytes)`는 tar 풀기가 동기라 `spawn_blocking`에서 부른다.
- (Eng 재리뷰, Eng 줄 대체) 자동 확인 주기: 6시간 간격은 마지막 성공(서버에 닿은 확인) 시각으로 잰다. 실패한 시도(네트워크)는 다음 30분 깨움에 다시 한다(잠자기에서 깬 직후의 실패가 6시간을 기다리지 않게).
- (Eng 재리뷰) 관리자 문서: `publish` 동시성 그룹은 대기 작업을 하나만 둔다(세 번째가 오면 앞의 대기 작업이 취소된다). 취소된 `publish`는 "실패한 작업만 다시 실행"으로 다시 돌린다.
- (Eng 재리뷰) 보안: 릴리스 페이지 링크는 피드 값이 아니라 상수(공개 저장소 Releases 주소와 버전)로 만든다. `notes`는 64KB와 200줄에서 자른다. `actions/create-github-app-token`에는 `repositories: meeting-summarizer-releases`로 범위를 준다. 공개되는 바이너리에 든 Slack `CLIENT_ID`는 PKCE라 괜찮지만, Slack 앱이 우리 워크스페이스에만 설치되는지 3단계에서 확인한다.
- (Eng 재리뷰) 테스트 추가: 준비 예약(막히면 잠그지 않음, 저장 대기 중 메뉴 종료 미룸과 준비 취소 뒤 실행, 60초 만료), 정리 한 번(동시 호출, 멈춘 정리 뒤 `Exit`가 마감 안에 끝남), `rev`로 늦은 스냅샷 버림, IPC 모양(Rust와 TS가 같은 JSON fixture를 읽음), 재시도 횟수(실패, 다시 시작, 다시 시도, 실패, 다시 시작이면 `count` 2이고 링크만, 한 시도를 두 번 세지 않음), 카드 배치(760px·800px 가로 창의 저장 막대), 탭 소유(다른 탭에 입력이 있을 때 "바뀐 것 보기"가 일반 탭을 열고 입력을 지킴), 준비 중 알림 클릭·끌어놓기가 화면을 바꾸지 않음, 오래 확인 못 함(휴가 뒤 첫 확인 성공이면 경고 없음, 처음 설치, 시계 되돌림, 13·14·15일), 승격 판정(롤백 뒤 낮은 태그와 높은 태그), `.sig`의 키·버전 불일치, `update-state.json` 위치, CI 시작 확인, 진행 이벤트 줄이기, D16(일반 탭에서 잠긴 동안만 카드와 초점, 잠금이 풀리면 버전 행과 버튼).
<!-- /autoplan-accepted:eng -->

## 11. 최종 관문 결정 (2026-11-05, 사용자)

10장의 같은 주제 줄보다 이 장이 이긴다.

- d1 업데이트 알림은 창 왼쪽 아래에 떠 있는 카드로 보인다. 잠깐 나왔다 사라지는 토스트가 아니라, 닫거나 업데이트할 때까지 남는다. 위치, 겹침, 모양, 상태, 닫기, 초점의 세부는 10장의 "(디자인 재리뷰)" 줄을 따른다(이 장과 같은 결정이다).
  - 녹음 화면과 설정의 일반 탭에서는 숨긴다(D13). 녹음 중에는 어느 화면에서든 준비 카드를 숨긴다(D12, 트레이 항목과 같다). 그 밖의 화면(홈, 첫 화면, 회의 문서, 설정의 다른 탭)에 보인다. 다만 사용자가 업데이트를 눌러 앱이 잠긴 동안(준비·설치 단계)에는 설정 일반 탭에서도 카드를 보이고 초점을 카드 상태 글로 옮긴다(D16). 잠금이 풀린 뒤의 막힘·실패 문구는 일반 탭이면 버전 행에 보이고 초점은 누른 버튼으로 돌아간다.
  - 준비 카드: "1.2.2 버전이 준비되었습니다"(D9, v 없이), 그 아래 "다시 시작해 업데이트" 테두리 버튼(outline sm, D10). 노트가 있으면 "바뀐 것 보기" 링크(D14): 설정 일반의 버전 행으로 가서 노트를 펼친다.
  - 창이 좁으면(760px 미만이거나 세로 창) 카드는 아래쪽 막대(독, 보내기 막대, 저장 막대) 바로 위에 왼쪽 정렬로 올라간다(D11). 넓은 창에서는 왼쪽 아래다.
  - 닫기(×)가 있다. 닫으면 이번 실행(앱 프로세스) 동안 숨기고, 앱을 다시 켜면 다시 보인다. 닫아도 설정의 버전 행과 트레이 메뉴에서 업데이트할 수 있다.
  - 업데이트 뒤 결과도 같은 자리, 같은 카드로 보인다: 성공 "1.2.2 버전으로 업데이트했습니다"와 "바뀐 것", 실패 "업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다"와 "릴리스 페이지 열기". 카드 자리는 하나이고 준비 카드가 먼저다.
  - 14일 넘게 성공한 확인이 없으면 실행마다 한 번, 닫을 수 있는 카드로 "업데이트를 확인하지 못하고 있습니다"와 "릴리스 페이지 열기"를 보인다(D15).
  - 디자인 리뷰의 홈·첫 화면 "업데이트 줄"(준비 줄과 결과 줄)은 모두 이 카드로 바뀐다. 홈과 첫 화면의 줄은 오디오 확인 줄, 설정 줄 순서다(d9).
  - 막힘 이유(녹음, 파일 작업, Slack 보내기, 저장하지 않은 설정)와 설치 단계 문구는 5.4 표와 10장의 문구를 카드에 쓴다.
  - 미리보기와 화면 테스트의 "업데이트 줄" 장면은 카드 장면으로 바꾼다(10장 "(디자인 재리뷰)" 줄의 목록).
- d4 Windows 업데이트는 2단계에서 시험하지 않고 4단계(조용한 시작, 회사 관리 Windows PC)가 첫 시험이다. 두 기기 모두 통과하기 전에는 알리지 않는다는 조건은 그대로다.
- d2, d3, d5, d6, d7, d8, d9는 추천안대로. 계획 전체는 승인(plan1a).
- 두 번째 관문(2026-11-06, 디자인·Eng 재리뷰 뒤): d10 Windows 설치 전 대기는 지금대로 유지(a). 계획 전체 다시 승인(plan1a).

## Review record

### CEO 리뷰 Step 0 (2026-11-05, /autoplan, SELECTIVE EXPANSION)

**시스템 점검**: 기준 브랜치 master, 차이·스태시 없음, 소스에 TODO/FIXME 표시 없음. 최근 30일 가장 많이 바뀐 파일은 `docs/architecture.md`(42), `src/App.test.tsx`·`src/lib/ipc.ts`·`src/App.tsx`·`src-tauri/src/lib.rs`(각 23~24)다. 이 계획이 건드리는 파일과 겹친다. 업데이터 관련 커밋은 아직 없다(`git log origin/master --grep update`, 2026-11-05). 지난 리뷰: 자동 업데이트는 eng review D2(2026-10-14)에서 미뤘고, 이 계획은 사용자 요청(2026-11-05)으로 그 결정을 뒤집는다. Windows 빌드는 실기기에서 돌려 본 적이 없다(메모리 ci-deferred).

**참고할 코드**: 좋은 예는 `confirm_quit`(`commands.rs:343-357`)과 `RunEvent::Exit`(`lib.rs:476-490`)의 정리 순서, 설정의 바로 일어나는 동작 `LogFolder`(`Settings.tsx:1431-1456`), 버전마다 한 번 하는 일 `reset_gpu_failed_once`(`settings.rs:295-304`), 받기 진행 문구(디자인 시스템 8.10). 피할 예는 1a37b3a에서 되돌린 Slack 릴리스 카드다. CI 게시 단계는 최소 권한과 단순한 흐름으로 둔다.

**조사(Landscape)**: 1층(검증된 방식)은 서명된 피드와 OS 코드 서명을 갖춘 Sparkle·Squirrel·Tauri 업데이터다. 2층(지금 자료)은 다음과 같다. Tauri 업데이터 서명은 끌 수 없지만 OS 서명과 무관하다. Artifact Signing은 한국 법인이 대상이다. EV는 SmartScreen 평판을 바로 주지 않는다. 3층(첫 원리)으로 보면, 앱 안 업데이터는 격리 속성을 거치지 않으므로 macOS에서 서명이 하는 일은 권한 연속성만 남는다. 그 일에는 Apple의 신뢰가 아니라 안정된 지정 요구사항만 있으면 된다. 자체 서명 인증서가 그것을 무료로 준다(3.2 실측). EUREKA로 기록했다.

**이전 학습 적용**: cpal-loopback-macos(6/10, 2026-10-13)에 따르면 macOS 탭은 권한이 없으면 오류 없이 0 버퍼를 준다. 무음 경고(REC-05, `writer.rs:453-467`)는 두 트랙이 모두 15분 동안 조용해야 뜬다. 그래서 시스템 오디오 권한만 풀리면 경고 없이 상대방 목소리가 빠진다. 0단계 실측은 꼭 거쳐야 한다.

#### 0A. 전제 점검

진짜 문제: 새 버전이 사람들에게 닿지 않고, Mac은 버전마다 "그래도 열기"와 권한 재허용을 반복한다. 아무것도 안 하면 릴리스마다 Slack·OneDrive로 다시 나누고, 손으로 업데이트하지 않은 사람은 고친 버그를 받지 못한다. 이 계획은 프록시가 아니라 그 고통을 바로 푼다.

| 전제 | 주장 | 근거 | 판정 |
|---|---|---|---|
| P1 | 손 업데이트 마찰 때문에 구버전에 남는다 | install.md:57-77 절차, 릴리스 6회(10/20~11/04) [measured: git tags] | 구버전 사용자 수는 unverified(앱에 원격 측정이 없다) |
| P2 | 비공개 저장소 자산은 인증 없이 받을 수 없다 | TODOS.md:12, `gh repo view` PRIVATE | verified |
| P3 | ad-hoc의 지정 요구사항은 cdhash라 업데이트마다 권한이 풀린다 | `codesign -d -r-` 실측, Apple DTS forums 795739 | verified |
| P4 | 자체 서명은 지정 요구사항을 고정한다 | scratchpad 두 빌드 실측 | 요구사항은 verified, TCC 유지는 unverified(0단계) |
| P5 | 업데이터가 받은 파일에는 격리 속성이 붙지 않는다 | 추정 | unverified(0단계) |
| P6 | Windows 앱 안 업데이트에는 SmartScreen이 뜨지 않는다 | 추정 | unverified(3단계) |
| P7 | 업데이터는 OS 서명 없이 돈다 | Tauri 업데이터 문서 | verified(문서) |
| P8 | 엔진·워커가 파일을 잡으면 NSIS가 실패한다 | `sidecar.rs:647-677`, `queue.rs:294-303`, tauri#15134 | verified(코드), 실패는 reported |
| P9 | 공개 설치 파일에 비밀값이 없다 | `slack.rs:407` CLIENT_ID만, PKCE 테스트 `slack.rs:1788` | verified |

unverified=4 (P1 수, P4 TCC, P5, P6). P4·P5는 0단계, P6은 3단계의 끝나는 조건이다.

#### 0B. 기존 코드 재사용

| 하위 문제 | 기존 코드 | 쓰는 법 |
|---|---|---|
| 지금 버전 | `app.package_info().version`(`lib.rs:333`, `queue.rs:1478`) | 그대로 |
| 설치 전 정리 | `confirm_quit`(`commands.rs:343-357`) | 정리를 한 함수로 빼서 둘이 같이 쓴다 |
| 진행 중 작업 확인 | `busy_ids`(`store.rs:795`), 종료 대화상자(`App.tsx:581-595`) | 같은 판정, 같은 대화상자 모양 |
| 다음 실행에서 이어 하기 | `pause_busy`(`store.rs:812`), 시작 때 `enqueue_pending` | 그대로(업데이트도 종료처럼 다룬다) |
| HTTP | reqwest `native-tls`·`system-proxy` | 업데이터 기능도 같게 |
| 설정의 바로 일어나는 동작 | `LogFolder`(`Settings.tsx:1431`) | 같은 모양 |
| 버전마다 한 번 | `gpu_reset_version`(`settings.rs:295`) | "업데이트했습니다" 1회 표시에 같은 패턴 |
| 릴리스 | build.yml `release` 작업, `docs/releases/<태그>.md` | 공개 저장소 게시를 같은 작업에 더한다 |
| 트레이 메뉴 | `lib.rs:366-379` | 준비되면 항목 하나 |

새로 만드는 것: `update.rs`(상태와 명령), `scripts/latest-json.ts`, CI 자체 서명 단계. 업데이터 자체는 플러그인을 쓴다.

#### 0C. 12개월 이상향

```
  지금                          이 계획                          12개월 이상향
  손 설치, 버전마다 Mac 권한  ->  첫 설치만 손으로, 이후 앱 안  ->  설치 1회, 업데이트는 조용히,
  재허용, 버전 표시 없음          업데이트, 자체 서명으로 권한      문제 버전은 빨리 덮음, 서명 정책은
                                  유지, 설정에 버전                 필요할 때 시크릿만 바꿔 유료로 전환
```

이 계획은 이상향 쪽으로 간다. 유료 서명으로 바꿀 때는 Mac은 `APPLE_*` 시크릿, Windows는 `signCommand`만 더하면 된다(3.2, 3.3).

#### 0E·0F·0G. 모드와 범위

모드는 SELECTIVE EXPANSION(autoplan 규칙). 바꿀 파일은 코드 약 14개와 문서 약 9개로 추정한다 [estimated]. 15개가 넘지만 모드는 규칙대로 둔다. 문서는 이미 있는 장을 고치는 일이다.

HOLD 점검:
1. 복잡도: 새 모듈은 `update.rs` 하나와 스크립트 하나다. 더 줄일 곳으로 tauri-action이 다른 저장소에 직접 Release와 `latest.json`을 올리게 하는 길이 있다. 그러면 두 OS 빌드와 검사가 다 통과한 뒤에만 Release를 만든다는 지금의 불변식(build.yml `release` 작업)이 깨진다. 그래서 쓰지 않는다(기계적, P5).
2. 최소 변경: 비공개 저장소 Release는 그대로 두고 공개 저장소 게시를 더한다. Slack 업로드(`slack-release.yml`)가 비공개 Release를 읽기 때문이다(P3).
3. 불변식: 녹음·처리 중에는 설치하지 않는다. 종료 확인과 같은 정리를 거친다. 업데이터 서명 검증은 끄지 않는다.

확장 후보와 결정(자동 결정, 감사 기록에 남김):

| # | 후보 | 크기 | 결정 | 이유 |
|---|---|---|---|---|
| X1 | 준비된 업데이트의 릴리스 노트를 설정에서 보이기(`latest.json` `notes`) | S | 수락 | 받을 버전에서 무엇이 바뀌는지 알아야 누른다. 영향 범위 안(P2) |
| X2 | 업데이트 뒤 첫 실행에 "1.2.1로 업데이트했습니다"를 한 번 보이기 | S | 수락 | 업데이트가 됐는지 확인할 수 있다. `gpu_reset_version` 패턴을 재사용한다(P2, P4) |
| X3 | 업데이트가 준비되면 홈에 한 줄과 "다시 시작해 업데이트" 버튼(6.4 설정 줄 패턴) | S | 수락(TASTE) | 앱은 트레이에 살아서 설정을 잘 열지 않는다. 헤더 점보다 할 일이 바로 보인다. 헤더 점은 빼고 홈 줄로 대신한다 |
| X4 | 업데이트가 준비되면 트레이 메뉴에 "업데이트하고 다시 시작" | S | 수락 | 창을 닫아 둔 사람의 경로다. 영향 범위 안(`lib.rs` 트레이 메뉴) |
| X5 | 시작할 때 `app.log`에 버전 한 줄 | S | 수락 | 지원할 때 버전부터 묻는다. 한 줄이다 |
| X6 | 종료할 때 준비된 업데이트를 자동 설치 | M | 미룸(TODOS) | Windows NSIS 업데이트 모드가 설치 뒤 앱을 다시 띄우는지 확인되지 않았다. 다시 띄우면 "종료"가 거짓말이 된다 |
| X7 | 업데이트가 준비되면 데스크톱 알림 | S | 미룸(TODOS) | X3·X4로 충분한지 파일럿에서 본다. 알림 남발 위험이 있다 |
| X8 | 버전 옆에 빌드 커밋 | S | 건너뜀 | 태그마다 버전이 다르다. 쓸 사람이 없다 |

<!-- autoplan-baseline-edits:ceo {"sourceSha256":"6f7d30a8a536a9a6b3f830708ff2a0677a885e9119ca701eb8547467faaf5b86","replacements":[{"oldText":"- 상태: 초안 (2026-11-05), 계획 리뷰 전","newText":"- 상태: 초안 (2026-11-05), /autoplan 계획 리뷰 중"},{"oldText":"1.0.0 뒤 3주 동안 릴리스가 여섯 번 나왔다.","newText":"1.0.0(10/20)부터 1.1.1(11/04)까지 2주 남짓 동안 릴리스가 여섯 번 나왔다."},{"oldText":"0단계 실측이 실패하면 ad-hoc 서명으로 두고","newText":"실측(6장 2단계)이 실패하면 ad-hoc 서명으로 두고"},{"oldText":"TCC가 실제로 권한을 유지하는지는 0단계에서 확인한다","newText":"TCC가 실제로 권한을 유지하는지는 6장 2단계에서 확인한다"},{"oldText":"(추정, 0단계에서 `xattr`로 확인)","newText":"(추정, 6장 2단계에서 `xattr`로 확인)"},{"oldText":"막거나 묻는지는 찾지 못했다(0단계에서 확인).","newText":"막거나 묻는지는 찾지 못했다(6장 2단계에서 확인). 교체는 같은 볼륨 안의 rename으로 한다(plugin `updater.rs` macOS `install_inner`)."},{"oldText":"- E0. 실측: 자체 서명 + 업데이터로 macOS에서 권한·키체인·격리·App Management를 확인한다(6장 0단계).","newText":"- E0. 실측: 자체 서명 + 업데이터로 macOS에서 권한·키체인·격리·App Management를 확인한다(6장 2단계, 1단계 코드로)."},{"oldText":"- E3. UI: 설정 일반의 \"앱 버전\" 묶음, 업데이트가 준비되면 헤더 설정 버튼의 점.","newText":"- E3. UI: 설정 일반의 \"앱 버전\" 묶음(준비되면 릴리스 노트), 업데이트가 준비되면 홈의 한 줄과 트레이 메뉴 항목, 업데이트 뒤 첫 실행의 결과 1회 표시. 시작할 때 `app.log`에 버전 한 줄."},{"oldText":"- 녹음·처리 중 자동 설치. 설치는 사용자가 누를 때만 한다.","newText":"- 녹음·처리 중 자동 설치. 설치는 사용자가 누를 때만 한다.\n- 종료할 때 자동 설치(TODOS로 미룸: 앱은 트레이에 살아 사용자가 종료하는 일이 드물고, OS 종료는 `ExitRequested`를 거치지 않는다. Windows는 `restart_after_install(false)`로 다시 띄우지 않게 할 수 있다), 업데이트 준비 데스크톱 알림(TODOS로 미룸: 홈 줄과 트레이로 충분한지 파일럿에서 본다)."},{"oldText":"- `failed`는 다음 확인 때 다시 `checking`으로 간다. 자동 확인의 실패는 로그만 남기고 화면에 띄우지 않는다. 사용자가 누른 확인의 실패만 보인다.","newText":"- `failed`는 다음 확인 때 다시 `checking`으로 간다. 자동 확인의 실패는 로그만 남기고 화면에 띄우지 않는다. 사용자가 누른 확인의 실패만 보인다.\n- 자동 확인(시작 30초 뒤, 6시간마다)은 `idle`·`up_to_date`·`failed`에서만 돈다. `checking`·`downloading`·`ready`·`installing`에서는 건너뛴다. `ready` 동안 더 새 버전이 나오면 다음 실행에서 받는다. 수동 확인 버튼은 `ready`에서 \"다시 시작해 업데이트\"로 바뀌므로 겹치지 않는다.\n- `installing`에서 설치가 실패하면(macOS 교체 실패, Windows 설치 프로그램 실행 실패) 로그를 남기고 `app.restart()`로 지금 버전을 다시 띄운다. 정리 단계에서 큐가 닫혔으므로(`queue.rs:1323`) 같은 프로세스로 계속 돌지 않는다. 멈춘 작업은 다음 실행에서 이어 한다.\n- `failed`는 `{message, manual}`이다. `manual`은 사용자가 누른 확인과 그 뒤 이어진 받기에서 난 실패만 참이다. 화면은 `manual`일 때만 오류와 링크를 보이고, 아니면 `idle`처럼 \"업데이트 확인\" 버튼만 보인다."},{"oldText":"아니면 `confirm_quit`과 같은 순서로 정리한다. `QUIT_CONFIRMED`, `queue.shutdown()`, `recorder.stop(None)`, `store.pause_busy()`, 내보내기 취소. 그 뒤 엔진과 워커가 끝날 때까지 기다린다(최대 10초, `engine_running()`과 워커 수). 그다음 `update.install(bytes)`, 성공하면 `app.restart()`. Windows에서는 `install`이 설치 프로그램을 띄우고 앱을 끝낸다.","newText":"녹음 중이면 `Err(\"recording\")`로 거절한다(녹음은 다시 할 수 없다). 정리 전에 `latest.json`을 다시 확인한다(30초): 받은 버전과 다르면(철회됐거나 더 새 버전) 받은 것을 버리고 확인 흐름으로 돌아가고, 네트워크로 실패하면 받은 것(서명 확인 끝)으로 계속한다. 그다음 작업을 정리한다: `QUIT_CONFIRMED`, `queue.shutdown()`, `recorder.stop(None)`, `store.pause_busy()`, `exports.cancel_all()`. 그 뒤 엔진과 워커가 끝날 때까지 기다린다(최대 10초. `Queue::engine_running()`과 새 `Queue::workers_running()`(워커 자식의 `try_wait`)으로 판정). 설정에 `updated_to`를 적는다. 10초가 지나도 남아 있으면 설치하지 않고 로그를 남긴 뒤 `app.restart()`로 지금 버전을 다시 띄운다(다음 시작에 \"업데이트하지 못했습니다\", 반쯤 덮인 설치를 만들지 않는다). 다 끝났으면 `update.install(bytes)`. macOS는 성공하면 `app.restart()`. Windows는 `install`이 설치 프로그램을 띄운 뒤 `std::process::exit(0)`으로 끝나고(`RunEvent::Exit`가 오지 않는다), 설치 프로그램이 앱을 다시 띄운다(`restart_after_install` 기본값 true, plugin `updater.rs` `updater_parameters`)."},{"oldText":"- 정리를 `confirm_quit`과 한 함수로 묶어 둘이 갈라지지 않게 한다.","newText":"- 정리는 한 함수(`shutdown_work`)로 묶어 `confirm_quit`, `RunEvent::Exit`, `install_update`가 같이 쓴다. 지금 `confirm_quit`에는 내보내기 취소가 없고(`commands.rs:345-359`) `RunEvent::Exit`에만 있다(`lib.rs:476-490`). Windows 설치는 `RunEvent::Exit`를 거치지 않으므로 이 함수가 그 일을 다 해야 한다."},{"oldText":"- 업데이터는 `native-tls` + `system-proxy`, 타임아웃은 확인 30초, 받기는 진행이 있는 한 계속.","newText":"- 업데이터는 `native-tls` + `system-proxy`(`zip` 기능 없음. macOS의 tar·flate2는 기능과 상관없이 들어간다). 확인 요청은 30초 타임아웃. 빌더 타임아웃은 받기 요청에도 걸리므로(plugin `updater.rs:849-850`) 받기 전에 `update.timeout = None`으로 풀고, 60초 동안 새 조각이 오지 않으면 받기를 끊고 `failed`로 간다(`on_chunk` 시각과 `tokio::select`)."},{"oldText":"- `tauri.conf.json`: `bundle.createUpdaterArtifacts: true`, `plugins.updater.pubkey`, `plugins.updater.endpoints: [\"https://github.com/sijecorp/meeting-summarizer-releases/releases/latest/download/latest.json\"]`, `plugins.updater.windows.installMode: \"passive\"`.","newText":"- `tauri.conf.json`: `plugins.updater.pubkey`, `plugins.updater.endpoints: [\"https://github.com/sijecorp/meeting-summarizer-releases/releases/latest/download/latest.json\"]`, `plugins.updater.windows.installMode: \"passive\"`.\n- `bundle.createUpdaterArtifacts: true`는 CI만 쓰는 `src-tauri/tauri.updater.conf.json`에 두고 tauri-action `args`에 `--config`로 더한다. 이 값이 켜져 있으면 서명 키 없이 `tauri build`가 실패하므로, README의 로컬 설치 파일 빌드가 깨지지 않게 한다."},{"oldText":"- `ready`면 헤더의 설정 버튼에 작은 brand 점과 접근성 이름 \"설정, 업데이트 준비됨\".","newText":"- `ready`면 설정 묶음에 릴리스 노트(`latest.json`의 `notes`)를 접어 보인다. 마크다운을 해석하지 않고 글자 그대로 줄바꿈만 살린다(NFR-07, React 글자 노드).\n- `ready`면 홈(6.4)과 첫 화면(6.5)의 설정 줄 자리, 설정 줄 위에 한 줄(같은 모양): \"새 버전 1.2.1을 받았습니다\"와 \"다시 시작해 업데이트\". 녹음 화면에는 보이지 않는다. 줄 순서는 위에서부터 준비 줄, 업데이트 결과 줄, 설정 줄이다.\n- `ready`면 트레이 메뉴에 \"업데이트하고 다시 시작\"을 더한다. 메뉴는 `ready`가 되거나 풀릴 때 `set_menu`로 다시 만든다(지금은 시작 때 한 번 만든다, `lib.rs:366-379`). 누르면 창을 앞으로 가져오고 `app://update-install-requested`를 보낸다(종료의 `app://quit-requested`와 같은 길). 화면은 진행 중 작업이 있으면 대화상자를, 없으면 바로 설치를 부른다.\n- 업데이트 뒤 첫 실행: 설치 직전에 설정에 `updated_to`(새 버전)를 적는다. Rust가 시작할 때 그 값을 읽고 지우면서 `just_updated: {version, ok}`를 그 세션 메모리에만 둔다(`ok`는 지금 버전 >= `updated_to`. 실패 뒤 링크로 더 새 버전을 손 설치한 경우도 성공으로 본다). `update_state`가 이것을 돌려준다. 화면은 `ok`면 \"1.2.1로 업데이트했습니다\", 아니면 \"업데이트하지 못했습니다\"와 \"릴리스 페이지 열기\"를 홈(6.4)과 첫 화면(6.5)의 설정 줄 자리, 설정 줄 위에 한 줄로 보인다. 닫기(×)를 누르거나 앱을 다시 시작하면 사라진다. `ready` 줄과 함께 있으면 `ready` 줄이 위다.\n- 실측이 ad-hoc 대체로 끝난 경우에만: macOS에서 업데이트 뒤 첫 실행의 \"업데이트했습니다\" 줄에 \"녹음 화면에서 상대방 소리 막대가 움직이지 않으면 설정 > 오디오의 시스템 오디오 '테스트'로 확인하세요\"를 붙이고 install.md의 권한 절로 잇는다."},{"oldText":"시크릿이 없으면 지금처럼 ad-hoc(`\"-\"`)으로 빌드하고 경고를 남긴다.","newText":"시크릿이 없으면 수동·예약 실행은 지금처럼 ad-hoc(`\"-\"`)으로 빌드하고 경고를 남기며, 태그 빌드는 실패한다."},{"oldText":"(태그 빌드에서는 다르면 실패).","newText":"(태그 빌드에서는 다르면 실패). 실측이 ad-hoc 대체로 끝나면 자체 서명 단계와 이 검사는 넣지 않는다."},{"oldText":"두 플랫폼이 다 있고 서명이 비어 있지 않고 버전이 태그와 같을 때만 쓴다. 테스트를 둔다.","newText":"두 플랫폼이 다 있고 서명이 비어 있지 않고 버전이 태그와 같을 때만 쓴다. 노트 파일이 없으면 `notes`는 빈 문자열이고 경고를 남긴다. 테스트를 둔다."},{"oldText":"- 수동 실행(`workflow_dispatch`)은 공개 저장소에 올리지 않는다.","newText":"- 수동 실행(`workflow_dispatch`)은 공개 저장소에 올리지 않는다.\n- 공개 저장소는 README 커밋 하나로 초기화해 둔다. 빈 저장소에는 태그를 만들 커밋이 없어 `gh release create --repo`가 실패한다. 공개 저장소에는 `--verify-tag` 대신 `--target main`을 쓴다."},{"oldText":"| 0. 실측 (macOS, 이 Mac) | 자체 서명 인증서와 업데이터 키를 만들고, 업데이터를 켠 시험 빌드 두 개(1.2.0-a, 1.2.0-b)를 로컬에서 서명해 만든다. 시험용 설정(`--config`)으로 끝점을 로컬 서버로 돌린다. a를 dmg로 설치해 \"그래도 열기\", 마이크·시스템 오디오 허용, 10초 녹음. 앱에서 b로 업데이트한 뒤 다시 녹음 | 업데이트 뒤 Gatekeeper 창 없음, `xattr`에 `com.apple.quarantine` 없음, 두 트랙이 0이 아님, 키체인 창 없음, App Management 창 없음. 하나라도 실패하면 기록하고 U2 파생(ad-hoc + 안내)으로 계획을 고친다 |\n| 1. 앱 | E1, E2, E3, 테스트, 미리보기 | `bun run test`, `cargo test`, `cargo clippy -D warnings` 통과 |\n| 2. 배포 | 공개 저장소 생성, 시크릿 등록(사용자), E4 | 수동 실행 빌드 통과, 태그 없이 공개 저장소에 쓰지 않음 |\n| 3. 릴리스 | 1.2.0 태그(수동 설치), 이어서 1.2.1로 실제 업데이트 확인. Windows 실기기 1대에서 1.2.0 → 1.2.1 | 두 OS에서 앱 안 업데이트 성공, Windows에서 SmartScreen 창 없음, 설치 뒤 llama-server 버전이 새 것 |\n| 4. 문서 | E5 | |","newText":"| 0. 권한 실측 (macOS, 코드 없이, 몇 시간) | 지금 코드를 자체 서명 인증서로 버전만 바꿔 두 번 로컬 빌드(a, b). a.dmg에 격리 속성을 붙여 설치(\"그래도 열기\"), 마이크·시스템 오디오 허용, Slack 로그인, 녹음. b를 응용 프로그램 폴더에 끌어다 덮어 설치한 뒤 다시 녹음·보내기 | 권한·키체인이 유지되면 Mac 방향(자체 서명) 확정. 아니면 5.8 첫 줄로 1단계 전에 계획을 고친다. 2단계는 업데이터에만 해당하는 확인(격리, App Management, 다시 뜨기)에 집중한다 |\n| 1. 앱 | 먼저 업데이터 키 쌍을 만들어 공개 키를 `tauri.conf.json`에 넣고 개인 키·비밀번호를 두 곳에 둔다(5.6). 그다음 E1, E2, E3, 테스트, 미리보기 | `bun run test`, `cargo test`, `cargo clippy -D warnings` 통과 |\n| 2. 실측 (macOS, 이 Mac) | 자체 서명 인증서를 만들고, 1단계 코드와 키로 시험 빌드 두 개(1.2.0-a, 1.2.0-b)를 로컬에서 서명해 만든다. 시험용 `--config`로 끝점을 로컬 서버(`http://127.0.0.1`)로 돌리고 `plugins.updater.dangerousInsecureTransportProtocol: true`를 준다(업데이터는 https가 아니면 거절한다, plugin `updater.rs:827`). a.dmg에 내려받은 파일처럼 격리 속성을 붙이고(`xattr -w com.apple.quarantine \"0081;<시각>;Safari;\"`, 또는 Slack으로 받아) 설치해 \"그래도 열기\"가 실제로 뜨고 통과되는지 본다. 마이크·시스템 오디오 허용, Slack 로그인(키체인 항목), 10초 녹음. 앱에서 b로 업데이트한 뒤 다시 녹음. 시험 하네스: 로컬 빌드는 `--config src-tauri/tauri.updater.conf.json`과 시험 `--config`를 함께 주고 `TAURI_SIGNING_PRIVATE_KEY`·`_PASSWORD`를 환경 변수로 둔다. `latest-json.ts`는 두 플랫폼을 요구하므로 시험용 `latest.json`(darwin-aarch64 하나)은 손으로 쓴다. Windows PC가 있으면 같은 a→b 시험을 Windows에서도 한다(수동 실행 빌드 + 시험 `--config`, 로컬 http) | 첫 설치의 \"그래도 열기\"가 자체 서명 앱에서도 ad-hoc과 같은 절차, 업데이트 뒤 Gatekeeper 창 없음, `xattr`에 `com.apple.quarantine` 없음, 두 트랙이 0이 아님, 키체인 창 없음, App Management 창 없음, b가 스스로 다시 뜨고 \"b로 업데이트했습니다\"가 한 번 보임. 실패하면 5.8의 표대로 고친다 |\n| 3. 배포 | 공개 저장소 생성(README 커밋), 시크릿 등록(사용자: `TAURI_SIGNING_PRIVATE_KEY`·`_PASSWORD`, `MACOS_SELF_SIGNED_P12`·`_PASSWORD`, GitHub App `RELEASES_APP_ID`·`RELEASES_APP_KEY`, 저장소 변수 `SLACK_AUTO=false`). 업데이터 키와 인증서의 두 번째 보관자를 정한다. 그 뒤 E4를 master에 | 수동 실행 빌드 통과, 태그 없이 공개 저장소에 쓰지 않음 |\n| 4. 릴리스(조용한 시작) | `SLACK_AUTO=false`인 채로 1.2.0 태그. 이 Mac과 회사 관리 Windows PC 1대(표준 보안 프로그램이 켜진 기준 기기)에만 1.2.0을 손으로 설치. 곧바로 1.2.1 태그를 붙여 실제 공개 피드로 1.2.0 → 1.2.1을 업데이트하고 녹음·전사·요약·Slack 보내기까지 한다. 통과하면 `SLACK_AUTO`를 지우고 slack-release를 손으로 돌려 1.2.1을 마지막 손 설치 버전으로 알린다 | 두 기기 모두 통과하기 전에는 업데이터가 든 빌드를 Slack에 알리지 않는다. 두 OS에서 앱 안 업데이트 성공, 설치 뒤 앱이 스스로 다시 뜸, \"1.2.1로 업데이트했습니다\" 1회, Windows에서 SmartScreen 창 없음, 설치 뒤 llama-server가 새 것 |\n| 5. 문서 | E5 | |"},{"oldText":"- 화면: 설정 일반의 상태별 문구와 버튼, 진행 중 작업이 있을 때 대화상자, 헤더 점과 접근성 이름, 저장 막대에 세지 않음.","newText":"- 화면: 설정 일반의 상태별 문구와 버튼, 준비됐을 때 릴리스 노트(마크다운 기호가 글자 그대로), 진행 중 작업이 있을 때 대화상자, 홈의 준비 줄과 업데이트 뒤 1회 표시(성공·실패), `app://update-install-requested`에 같은 흐름, 저장 막대에 세지 않음.\n- Rust: 시작할 때 `updated_to`를 읽고 지우는지(두 번째 시작에는 `just_updated`가 없음). 트레이 항목 목록을 만드는 순수 함수가 `ready`일 때만 그 항목을 넣는지(모의 런타임에는 트레이가 없다, `lib.rs:194`). 자동 확인이 `downloading`·`ready`·`installing`에서 건너뛰는지. 받기 감시가 60초 무진행에서 끊는지. `just_updated.ok`가 지금 버전 >= `updated_to`일 때 참인지(같음, 더 높음, 더 낮음). 자동 확인 실패는 `manual: false`, 수동 확인과 이어진 받기의 실패는 `manual: true`인지. `install_update(false)`가 진행 중 작업이 있으면 정리 없이 `Err(\"busy\")`, 없으면 진행하는지. 녹음 중이면 `confirmed`와 상관없이 `Err(\"recording\")`인지. 설치 직전 재확인이 다른 버전을 보면 받은 것을 버리는지. 10초 대기를 넘으면 설치하지 않고 다시 시작하는지.\n- 측정(파일럿): 릴리스마다 공개 저장소 업데이트 파일의 `download_count`(받은 앱 수)와 동료 명단(수십 명)을 대조해 3영업일 뒤 최신 버전 비율을 본다. 80% 미만이 두 번 이어지면 X6·X7을 앞당긴다(TODOS)."},{"oldText":"- 수동: 0단계와 3단계.","newText":"- 수동: 2단계와 4단계."},{"oldText":"| 자체 서명으로도 TCC가 권한을 유지하지 않음 | 0단계에서 먼저 확인.","newText":"| 자체 서명으로도 TCC가 권한을 유지하지 않음 | 2단계에서 릴리스 전에 확인."},{"oldText":"| App Management가 자기 번들 교체를 막음 | 0단계에서 확인.","newText":"| App Management가 자기 번들 교체를 막음 | 2단계에서 확인."},{"oldText":"| Windows에서 엔진·워커가 파일을 잡아 설치 실패 | 설치 전 대기(5.2). 실패하면 다음 실행에서 오류와 링크 |","newText":"| Windows에서 엔진·워커가 파일을 잡아 설치 실패 | 설치 전 대기(5.2). 실패하면 다음 실행에서 \"업데이트하지 못했습니다\"와 링크(5.4) |"},{"oldText":"| `latest.json` 하나가 깨져 모든 업데이트가 멈춤 | 스크립트 검사와 테스트, 초안으로 올린 뒤 공개 |","newText":"| `latest.json` 하나가 깨져 모든 업데이트가 멈춤 | 스크립트 검사와 테스트, 초안으로 올린 뒤 공개 |\n| GitHub App 키 분실이나 권한 변경 | `publish`가 실패로 드러나고 Slack은 링크 없이 설치 파일을 올린다(비공개 Release는 그대로). 앱 키를 새로 만들어 시크릿을 바꾸고 `publish`부터 다시 돌린다 |\n| 잘못된 릴리스가 나감 | 공개 저장소에서 `gh release edit <이전 태그> --latest`로 이전 릴리스를 최신으로 되돌리거나 잘못된 릴리스를 초안으로 돌린다. 자산은 지우지 않는다(받는 중인 앱이 깨진다). 이미 받아 둔 앱은 설치 직전 재확인(5.2)에서 버린다. 앱이 열리지 않는 버그는 Slack·OneDrive 손 설치로 회복하고, 그 버전은 데이터 호환(`schema_version`, NEWER_VERSION 읽기 전용)을 지킨다 |"},{"oldText":"| Windows 실기기에서 한 번도 돌려 보지 않음 | 3단계에서 실기기 1대 확인을 끝나는 조건으로 둔다 |","newText":"| Windows 실기기에서 한 번도 돌려 보지 않음 | Windows PC가 있으면 2단계에서 a→b를 먼저 시험한다. 4단계의 조용한 시작에서 회사 관리 Windows PC로 실제 피드 업데이트를 반드시 통과한 뒤에만 알린다. 백신·EDR이 업데이터가 띄운 설치 프로그램을 막는지도 이때 본다 |"},{"oldText":"- TODOS: \"파일럿 후 자동 업데이트\"를 지우고, \"Windows 코드 서명 검토\"에 3.3을 옮긴다.","newText":"- TODOS: \"파일럿 후 자동 업데이트\"를 지우고, \"Windows 코드 서명 검토\"에 3.3을 옮긴다. \"종료할 때 자동 설치\"(X6)와 \"업데이트 준비 데스크톱 알림\"(X7)을 더한다.\n- README: 다운로드 링크를 공개 저장소로. 설치 파일 빌드는 그대로 된다(업데이트 파일은 CI만 만든다).\n- `.github/workflows/slack-release.yml`: 릴리스 노트 링크(5.5)."},{"oldText":"- `docs/releases/v1.2.0.md`.","newText":"- `docs/releases/v1.2.0.md`.\n\n## 10. 리뷰에서 수락한 요구사항\n\n아래 줄은 계획 리뷰가 수락한 요구사항이다(리뷰 도구가 붙인다). 본문과 다르면 아래가 이긴다."},{"oldText":"- `Updater` 상태(Mutex)와 `app://update` 이벤트. 명령 셋: `update_state`, `check_update`, `install_update`.","newText":"- `Updater` 상태(Mutex)와 `app://update` 이벤트. 명령 셋: `update_state`, `check_update`, `install_update`. `update_state`는 지금 버전, 상태(5.1), `just_updated: {version, ok} | null`(5.4)을 돌려준다."},{"oldText":"`.sig` 세 개","newText":"`.sig` 두 개(`.app.tar.gz.sig`, `-setup.exe.sig`)"},{"oldText":"- release 작업: 지금처럼 비공개 저장소 Release를 만든 뒤, 공개 저장소에 같은 태그의 초안 Release를 만들어 설치 파일·업데이트 파일·`.sig`를 올리고, `latest.json`을 만들어 올린 뒤 초안을 공개한다. 토큰은 시크릿 `RELEASES_TOKEN`.","newText":"- `release` 작업은 지금처럼 비공개 저장소 Release만 만든다. 그 뒤 새 `publish` 작업(`needs: release`)이 공개 저장소에 같은 태그의 초안 Release를 만들어 설치 파일·업데이트 파일·`.sig`를 올리고, `latest.json`을 만들어 올린 뒤 `gh release edit --draft=false --latest`로 공개한다. 토큰은 조직 소유 GitHub App(공개 저장소에만 Contents 쓰기)의 설치 토큰을 `actions/create-github-app-token`으로 만든다(시크릿 `RELEASES_APP_ID`, `RELEASES_APP_KEY`). 개인 PAT와 달리 만료와 퇴사에 묶이지 않는다. `slack`은 `needs: [release, publish]`와 `if: always() && needs.release.result == 'success' && vars.SLACK_AUTO != 'false'`로 바꾼다. 공개 게시가 실패해도 설치 파일은 Slack에 올라간다(긴급 배포 경로). 노트 링크는 `publish`가 성공했을 때만 공개 저장소 주소로 붙인다. 공개 저장소에 그 태그의 Release가 이미 공개돼 있으면 `publish`를 실패시킨다(공개된 자산과 `.sig`를 바꾸면 받는 중인 앱이 서명 불일치를 본다). 초안이면 덮어쓰고 이어 간다.\n- `slack-release.yml`의 릴리스 노트 링크(56행, 지금 비공개 저장소 주소)를 공개 저장소 주소로 바꾼다. 동료는 비공개 저장소를 열 수 없다.\n- 릴리스 노트는 공개 글이다(`docs/releases/<태그>.md`가 그대로 공개된다). 사내 사람·팀 이름과 회의 내용을 쓰지 않는다."},{"oldText":"- 업데이터 키 쌍과 자체 서명 인증서(유효 20년)는 1회 만든다.","newText":"- 업데이터 키 쌍(1단계 시작 때)과 자체 서명 인증서(유효 20년, 2단계)는 1회 만든다."},{"oldText":"- 진행 중 작업이 있는지는 `store.busy_ids()`(종료 확인과 같다). 화면이 묻고, 확인하면 `install_update`를 부른다.","newText":"- 진행 중 작업 판정은 `store.busy_ids()`(종료 확인과 같다). 녹음 중에는 설치 버튼을 막고 \"녹음이 끝나면 업데이트할 수 있습니다\"를 보이며 트레이 항목도 숨긴다. 그 밖의 진행 중 작업(전사·요약)은 확인 뒤 멈춘다. `install_update(confirmed: bool)`은 `confirmed`가 거짓이고 진행 중 작업이 있으면 아무것도 하지 않고 `Err(\"busy\")`를 돌려준다. 화면은 그때 대화상자를 띄우고, \"업데이트\"를 누르면 `confirmed: true`로 다시 부른다."},{"oldText":"- 1.2.0부터는 앱 안에서 업데이트한다. 실측이 실패해 ad-hoc으로 돌아가면 업데이트마다 권한 재허용 안내를 앱에 넣는다(U2 파생).","newText":"- 1.2.0부터는 앱 안에서 업데이트한다. 실측이 실패해 ad-hoc으로 돌아가면 업데이트마다 권한 재허용 안내를 앱에 넣는다(U2 파생).\n\n### 5.8 실측 실패별 대응\n\n| 실패 | 대응 |\n|---|---|\n| 업데이트 뒤 마이크·시스템 오디오 권한이나 키체인 허용이 풀림 | 자체 서명의 이득이 없으므로 ad-hoc으로 돌아가고 5.4 마지막 줄의 안내를 넣는다. 자체 서명 단계와 검사는 넣지 않는다. 그리고 macOS 업데이트 뒤 다음 녹음 전에 시스템 오디오 테스트(ONB-03)를 먼저 하게 한다: 설정에 남는 표시로 통과할 때까지 홈과 녹음 시작에서 안내한다 |\n| App Management 창이 한 번 뜨고 허용하면 다음 업데이트부터 안 뜸 | 자체 서명을 유지하고 install.md와 첫 업데이트 안내에 \"허용\"을 적는다 |\n| App Management가 교체를 막음(허용해도 실패) | macOS는 알림만: 준비 상태 대신 \"새 버전이 있습니다\"와 릴리스 페이지 링크(손 설치). Windows는 그대로 |\n| 업데이트 뒤 Gatekeeper 창이 뜨거나 격리 속성이 남음 | 업데이터는 유지한다(받기는 앱이 한다). install.md에 업데이트 뒤 \"그래도 열기\" 절차를 적고 원인을 TODOS에 남긴다 |\n| b가 스스로 다시 뜨지 않음 | 릴리스 전에 고친다(버그). 고치기 전에는 릴리스하지 않는다 |\n| 첫 설치의 \"그래도 열기\"가 ad-hoc과 다름(더 막힘) | install.md 첫 설치 절차를 그 화면대로 고친다. 막혀서 열 수 없으면 ad-hoc으로 돌아간다 |"},{"oldText":"| U2 파생 | Mac은 무료로 서명 정체성을 고정하는 자체 서명 코드 서명 인증서를 쓴다(3.2). ","newText":"| U3 기록 | 이 계획은 eng review D2(2026-10-14, 자동 업데이트를 파일럿 뒤로)를 사용자 요청(2026-11-05)으로 뒤집는다. 근거: 2주 남짓 동안 6회 릴리스, 업데이트마다 Mac 권한 재허용. 설계 문서 전제 5(Developer ID 필요)는 U2로 대체한다. M7 파일럿 진행 여부는 로드맵에 기록이 없어 확인이 필요하다. 파일럿 1주차의 SmartScreen·백신 차단 수 세기(TODOS)는 그대로 둔다 |\n| U2 파생 | Mac은 무료로 서명 정체성을 고정하는 자체 서명 코드 서명 인증서를 쓴다(3.2). "},{"oldText":"- 진행 중 작업이 있으면 종료 확인과 같은 대화상자: 제목 \"진행 중인 작업이 있습니다\", 설명 \"지금 업데이트하면 진행 중인 녹음과 처리를 멈추고, 다시 시작한 뒤 이어서 처리합니다\", 버튼 \"취소\"·\"업데이트\".","newText":"- 진행 중인 처리(전사·요약)가 있으면 종료 확인과 같은 대화상자: 제목 \"진행 중인 작업이 있습니다\", 설명 \"지금 업데이트하면 진행 중인 처리를 멈추고, 다시 시작한 뒤 이어서 처리합니다\", 버튼 \"취소\"·\"업데이트\". 녹음 중에는 대화상자 대신 버튼이 막힌다(5.2)."},{"oldText":"- 1.1.1에는 업데이터가 없으므로 1.2.0은 마지막으로 손으로 설치한다(Slack·OneDrive, 공개 저장소 링크).","newText":"- 1.1.1에는 업데이터가 없으므로 동료는 한 번 더 손으로 설치한다. 그 버전은 실제 피드로 업데이트가 검증된 1.2.1이다(6장 4단계). 1.2.0은 시험 기기에만 설치한다."}]} -->

<!-- autoplan-accepted:ceo -->
- 6장 1단계 앞에 0단계 실측을 둔다. 자체 서명 빌드 a→b 앱 안 업데이트 뒤 Gatekeeper 창 없음, `com.apple.quarantine` 없음, 마이크·시스템 오디오 두 트랙이 0이 아님, 키체인 창 없음, App Management 창 없음을 확인한다. 하나라도 실패하면 기록하고 ad-hoc + 업데이트 뒤 권한 재허용 안내로 계획을 고친다.
- 설정 > 일반 "앱 버전" 묶음: 지금 버전, 상태 줄, 버튼 하나(5.4 표). `ready`면 `latest.json`의 `notes`를 접어 보인다. 저장 막대에 세지 않는다.
- `ready`면 홈 머리 아래 한 줄("새 버전 X를 받았습니다" + "다시 시작해 업데이트")을 보인다. 헤더 설정 버튼의 점은 두지 않는다. 녹음 화면에는 보이지 않는다.
- `ready`면 트레이 메뉴에 "업데이트하고 다시 시작"이 있고, 누르면 창을 앞으로 가져와 같은 설치 흐름(진행 중 작업이 있으면 대화상자)을 탄다. `ready`가 아니면 그 항목이 없다.
- 설치 직전 설정에 `updated_to`를 적고, 새 버전 첫 실행에서 지금 버전과 같으면 홈에 "X로 업데이트했습니다"를 한 번 보이고 지운다. 다르면 표시 없이 지운다. Rust 테스트로 확인한다.
- 시작할 때 `app.log`에 앱 버전 한 줄을 남긴다.
- 공개 저장소는 README 커밋으로 초기화하고, 공개 저장소 Release는 `--target main`으로 만든다(초안 → 자산 → `latest.json` → 공개).
- 종료할 때 자동 설치와 업데이트 준비 데스크톱 알림은 범위 밖이며 TODOS에 미룬다.
- 1장 수치는 "1.0.0(10/20)부터 1.1.1(11/04)까지 2주 남짓 동안 6회"다.
- (스펙 리뷰 1 대체) 첫 줄의 "6장 1단계 앞에 0단계 실측"은 "1단계 코드로 하는 6장 2단계 실측, 4단계 릴리스 전"으로 바꾼다. 확인 항목과 실패 시 대체는 그대로다. 시험 `--config`는 로컬 http 끝점과 `dangerousInsecureTransportProtocol: true`를 쓴다.
- (스펙 리뷰 1 대체) `updated_to` 줄의 "다르면 표시 없이 지운다"는 "다르면 '업데이트하지 못했습니다'와 릴리스 페이지 링크를 한 번 보이고 지운다"로 바꾼다.
- 노트는 마크다운을 해석하지 않고 글자 그대로 줄바꿈만 살린다(NFR-07).
- 트레이 메뉴는 `ready`가 되거나 풀릴 때 `set_menu`로 다시 만들고, 항목을 누르면 `app://update-install-requested`로 화면이 같은 흐름을 탄다. 항목 목록은 순수 함수로 만들고 테스트한다.
- 정리는 `shutdown_work` 한 함수(`QUIT_CONFIRMED`, `queue.shutdown`, `recorder.stop`, `pause_busy`, `exports.cancel_all`)로 `confirm_quit`·`RunEvent::Exit`·`install_update`가 같이 쓴다. 설치 전 엔진·워커 종료를 최대 10초 기다리고(`workers_running` 추가), 넘으면 경고 후 설치를 계속한다.
- 설치가 실패하면 로그를 남기고 `app.restart()`로 지금 버전을 다시 띄운다.
- 자동 확인은 `idle`·`up_to_date`·`failed`에서만 돈다. 확인은 30초 타임아웃, 받기는 `update.timeout = None` + 60초 무진행 감시.
- 업데이터 기능은 `native-tls`·`system-proxy`(`zip` 없음). `createUpdaterArtifacts`는 CI 전용 `src-tauri/tauri.updater.conf.json`으로만 켠다.
- 시크릿은 E4를 master에 넣기 전에 등록한다. 태그 빌드는 자체 서명 시크릿이 없으면 실패하고, 수동·예약 실행은 ad-hoc + 경고다. 실측이 ad-hoc 대체로 끝나면 자체 서명 단계와 지정 요구사항 검사는 넣지 않는다.
- 공개 저장소에 같은 태그의 Release가 이미 공개돼 있으면 release 작업을 실패시킨다. 초안이면 덮어쓰고 이어 간다. 노트 파일이 없으면 `notes`는 빈 문자열 + 경고.
- `slack-release.yml`의 릴리스 노트 링크를 공개 저장소로 바꾼다.
- `RELEASES_TOKEN` 만료를 위험 표에 두고 만료 30일 전 알림을 둔다.
- 4단계 끝나는 조건에 "설치 뒤 앱이 스스로 다시 뜸"과 "X로 업데이트했습니다" 1회를 넣는다.
- 실측이 ad-hoc 대체로 끝난 경우에만 macOS 업데이트 뒤 첫 실행 줄에 시스템 오디오 테스트 안내를 붙인다(5.4 마지막 줄).
- TODOS에 X6(종료할 때 자동 설치)과 X7(업데이트 준비 데스크톱 알림)을 더한다.
- (스펙 리뷰 2 대체) `updated_to` 성공 판정은 "지금 버전과 같으면"이 아니라 "지금 버전 >= `updated_to`"다. Rust가 시작할 때 읽고 지우며 `just_updated: {version, ok}`를 그 세션에만 두고 `update_state`로 돌려준다. 화면은 홈과 첫 화면의 설정 줄 자리, 설정 줄 위에 한 줄로 보이고 닫기(×)나 다시 시작으로 사라진다. `ready` 줄과 함께면 `ready` 줄이 위다.
- (스펙 리뷰 2 대체) 공개 저장소 게시는 `release` 작업이 아니라 새 `publish` 작업(`needs: release`)이 하고, `slack`은 `needs: publish`다. 초안 공개는 `gh release edit --draft=false --latest`. PAT가 만료되면 `publish`가 실패하고 Slack도 보내지 않는다(비공개 Release는 그대로).
- `failed`는 `{message, manual}`이고, 수동 확인과 그 뒤 이어진 받기의 실패만 `manual: true`로 화면에 보인다.
- 업데이터 키 쌍은 1단계 시작 때 만들어 공개 키를 `tauri.conf.json`에 넣고 개인 키를 두 곳에 둔다. 자체 서명 인증서는 2단계에서 만든다.
- 2단계 실측은 a.dmg에 격리 속성을 붙여(또는 Slack으로 받아) 실제 첫 설치의 "그래도 열기"를 거치고, a에서 Slack에 로그인해 둔 뒤 업데이트한다.
- 3.5의 공개 자산 `.sig`는 두 개(`.app.tar.gz.sig`, `-setup.exe.sig`)다.
- (스펙 리뷰 3 대체) 실측 실패 시 "모두 ad-hoc + 안내"가 아니라 5.8의 실패별 표를 따른다. 권한·키체인 유지 실패만 ad-hoc + 안내다.
- `install_update(confirmed: bool)`: 거짓이고 진행 중 작업이 있으면 정리 없이 `Err("busy")`, 화면은 대화상자 뒤 `true`로 다시 부른다. 테스트한다.
- 준비 줄과 업데이트 결과 줄은 홈과 첫 화면 모두 설정 줄 자리에 있고, 순서는 준비 줄, 결과 줄, 설정 줄이다.
- 2단계 실측 하네스: 로컬 빌드는 `tauri.updater.conf.json` + 시험 `--config` + 업데이터 키 환경 변수, 시험 `latest.json`은 darwin 하나로 손으로 쓴다. 통과 조건에 "b가 스스로 다시 뜨고 결과 줄 1회"를 더한다.
- Windows PC가 있으면 2단계에서 Windows a→b도 시험한다. 없으면 4단계가 첫 시험이고, 깨지면 지금처럼 손 설치로 회복한다(위험 표).
- (CEO 두 목소리 대체) 동료가 마지막으로 손 설치하는 버전은 1.2.0이 아니라 1.2.1이다. 1.2.0은 `SLACK_AUTO=false`로 알리지 않고 이 Mac과 회사 관리 Windows PC(표준 보안 프로그램) 두 대에만 설치해 실제 공개 피드로 1.2.0 → 1.2.1을 검증한다. 두 기기 모두 통과하기 전에는 업데이터가 든 빌드를 알리지 않는다.
- (CEO 두 목소리 대체) 녹음 중에는 설치하지 않는다(`Err("recording")`, 버튼 막힘, 트레이 항목 숨김). 확인 대화상자는 처리(전사·요약)에만 쓴다.
- (CEO 두 목소리 대체) 10초 대기를 넘으면 설치하지 않고 지금 버전으로 다시 시작한다("업데이트 계속" 규칙을 바꾼다).
- 설치 직전 `latest.json`을 다시 확인해 받은 버전과 다르면 버리고 확인 흐름으로 돌아간다. 네트워크 실패면 받은 것으로 계속한다.
- (CEO 두 목소리 대체) `slack`은 `publish` 실패와 상관없이 설치 파일을 올린다(`needs: [release, publish]`, `if: always() && release 성공 && SLACK_AUTO != false`). 노트 링크만 `publish` 성공 때 붙인다.
- (CEO 두 목소리 대체) 공개 저장소 쓰기 토큰은 개인 PAT가 아니라 조직 소유 GitHub App 설치 토큰(`RELEASES_APP_ID`, `RELEASES_APP_KEY`)이다. PAT 만료 위험 행은 GitHub App 키 행으로 바뀐다.
- 잘못된 릴리스 대응 절차(이전 태그를 `--latest`로, 자산은 지우지 않음)를 위험 표, 아키텍처 11장, install.md 관리자 절에 둔다.
- 업데이터 키와 인증서의 두 번째 보관자를 정한다(사용자).
- 릴리스 노트는 공개 글이다. 사내 사람·팀 이름과 회의 내용을 쓰지 않는다.
- 6장 0단계(코드 없는 권한 실측)를 1단계 전에 한다. 결과로 Mac 방향을 1단계 전에 정한다.
- ad-hoc 대체 경로에서는 macOS 업데이트 뒤 다음 녹음 전에 시스템 오디오 테스트를 하게 하는 남는 표시를 둔다.
- 측정: 릴리스마다 `download_count`와 명단으로 3영업일 뒤 최신 버전 비율을 보고, 80% 미만이 두 번 이어지면 X6·X7을 앞당긴다.
- U3: D2를 뒤집는다는 기록, 전제 5 대체, M7 상태 확인 필요, 파일럿 1주차 SmartScreen·백신 차단 수 세기 유지.
- `updated_to` 쓰기가 실패하면(읽기 전용 설정) 로그만 남기고 설치는 계속한다.
- 상태 잠금은 두 순서(수동 확인 중 타이머, 설치 재확인 중 타이머)로 시험하고, 시간 상수(30초·6시간·60초·10초)는 주입 가능한 시계나 짧은 상수로 시험한다.
- 4단계에서 Windows NSIS 로그 위치를 확인해 install.md 관리자 절에 적는다.
<!-- /autoplan-accepted:ceo -->

#### CEO 결정 장부

| ID·담당 | 계약과 근거 | 현재 | 제안 | 상태 | 승인과 범위 |
|---|---|---|---|---|---|
| U1 사용자 | 업데이트 피드 위치 | 공개 releases 저장소 | - | approved | 사용자 D1 답(2026-11-05) |
| U2 사용자 | 유료 서명 없음, 첫 설치 수동 | 자체 서명(Mac), 무서명(Win) | - | approved | 사용자 D2·D3 답(2026-11-05) "비용을 들이지 않고 싶어" |
| C1 CEO 0B | 공개 Release 생성 방식 | 우리 release 작업 + 스크립트 | tauri-action 직접 게시 | approved | autoplan 자동 결정(기계적, P5): 빌드·검사 통과 뒤 게시 불변식 유지 |
| C2 CEO 0B | 비공개 Release 유지 | 유지 | 공개로 일원화 | approved | autoplan 자동 결정(P3): Slack 업로드가 비공개 Release를 읽음 |
| X1~X5 CEO 0G | 확장 수락 | 수락 | - | approved | autoplan 자동 결정(P2, 영향 범위 안, S). X3은 TASTE |
| X6·X7 CEO 0G | 확장 미룸 | TODOS | - | deferred | autoplan 자동 결정(P3) |
| X8 CEO 0G | 빌드 커밋 표시 | 건너뜀 | - | declined | autoplan 자동 결정(P4·P5) |
| R1 CEO reality | 공개 저장소 초기화 | README 커밋 + `--target main` | - | approved | 기계적(surface-check: 빈 저장소엔 태그를 만들 커밋이 없음) |

<!-- AUTONOMOUS DECISION LOG -->
## Decision Audit Trail

| # | Phase | Decision | Classification | Principle | Rationale | Rejected |
|---|-------|----------|----------------|-----------|-----------|----------|
| 1 | CEO | 모드 SELECTIVE EXPANSION | Mechanical | autoplan 규칙 | 기능 추가 계획, autoplan 기본 | SCOPE REDUCTION(파일 수 규칙) |
| 2 | CEO | 공개 Release는 우리 release 작업이 만든다 | Mechanical | P5 | 두 OS 빌드·검사 통과 뒤에만 게시하는 불변식 유지 | tauri-action 직접 게시 |
| 3 | CEO | 비공개 Release 유지, 공개 게시를 더함 | Mechanical | P3 | slack-release.yml이 비공개 Release를 읽음 | 공개로 일원화 |
| 4 | CEO | X1 릴리스 노트 표시 수락 | Mechanical | P2 | 영향 범위 안, S | - |
| 5 | CEO | X2 업데이트 뒤 1회 표시 수락 | Mechanical | P2, P4 | `gpu_reset_version` 패턴 재사용 | - |
| 6 | CEO | X3 홈 한 줄로 알림(헤더 점 대신) | Taste | P1, P5 | 트레이 앱이라 설정을 잘 열지 않음, 6.4 설정 줄 패턴 | 헤더 설정 버튼 점 |
| 7 | CEO | X4 트레이 메뉴 항목 수락 | Mechanical | P2 | 창을 닫아 둔 사람의 경로 | - |
| 8 | CEO | X5 시작 로그에 버전 | Mechanical | P2 | 한 줄, 지원에 필요 | - |
| 9 | CEO | X6 종료 때 자동 설치 미룸 | Mechanical | P3 | NSIS 재실행 동작 미확인 | 수락 |
| 10 | CEO | X7 데스크톱 알림 미룸 | Mechanical | P3 | 알림 남발 위험, X3·X4 먼저 | 수락 |
| 11 | CEO | X8 빌드 커밋 표시 건너뜀 | Mechanical | P4 | 태그마다 버전이 다름 | 수락 |
| 12 | CEO | 공개 저장소 README 초기화 + `--target main` | Mechanical | P1 | 빈 저장소에서 release create 실패 | `--verify-tag` |
| 13 | CEO spec 1 | 설치 실패 시 `app.restart()`로 지금 버전 재실행 | Mechanical | P1 | 정리 뒤 큐가 닫혀(`queue.rs:1323`) 같은 프로세스로 못 돈다 | 오류만 보이고 계속 실행 |
| 14 | CEO spec 1 | 자동 확인은 idle·up_to_date·failed에서만 | Mechanical | P5 | 받는 중·준비 상태와 겹치지 않게 | 언제나 확인 |
| 15 | CEO spec 1 | 공개 Release가 이미 공개돼 있으면 release 작업 실패 | Mechanical | P1 | 공개 자산·`.sig` 비원자 교체는 서명 불일치 | `--clobber` |
| 16 | CEO spec 1 | `slack-release.yml` 노트 링크를 공개 저장소로 | Mechanical | P1 | 동료는 비공개 저장소를 못 연다(56행) | 그대로 |
| 17 | CEO spec 1 | PAT 만료 위험 행 | Mechanical | P1 | 조용히 멈추지 않게 | - |
| 18 | CEO spec 1 | `updated_to` 불일치 = "업데이트하지 못했습니다" 1회 | Mechanical | P1 | 8장 Windows 행과 일치 | 조용히 지우기 |
| 19 | CEO spec 1 | `shutdown_work` 한 함수(내보내기 취소 포함) | Mechanical | P4 | Windows 설치는 `RunEvent::Exit`를 안 거친다 | `confirm_quit` 복제 |
| 20 | CEO spec 1 | Windows 설치 뒤 재실행을 끝나는 조건에 | Mechanical | P1 | `restart_after_install` 기본 true(소스 확인) | - |
| 21 | CEO spec 1 | 수락 요구사항을 10장 제목 아래로 | Mechanical | P5 | 9장 문서 변경과 섞이지 않게 | - |
| 22 | CEO spec 1 | 10초 대기 뒤 경고하고 설치 계속, `workers_running` 추가 | Mechanical | P5 | 실패해도 5.1 실패 경로가 받는다 | 설치 중단 |
| 23 | CEO spec 1 | 받기 타임아웃 해제 + 60초 무진행 감시 | Mechanical | P1 | 빌더 타임아웃이 받기에도 걸림(소스 `updater.rs:849`) | 30초 그대로 |
| 24 | CEO spec 1 | 실측을 1단계 뒤 2단계로 | Taste | P5 | 버리는 시험 코드 없이 실제 코드로 실측 | 1단계 앞 + 임시 훅 |
| 25 | CEO spec 1 | ad-hoc 대체 시 안내 문구·위치 정의 | Mechanical | P1 | 대체 경로가 바로 구현 가능해야 | 나중에 정함 |
| 26 | CEO spec 1 | 트레이는 `set_menu` 재구성 + 이벤트, 순수 함수 테스트 | Mechanical | P5 | 모의 런타임엔 트레이 없음 | - |
| 27 | CEO spec 1 | 노트는 글자 그대로, 노트 파일 없으면 빈 문자열 | Mechanical | P5 | NFR-07, 지금 release 흐름과 같은 관대함 | 마크다운 렌더 |
| 28 | CEO spec 1 | `createUpdaterArtifacts`는 CI 전용 설정 파일로 | Taste | P5 | 로컬 `tauri build`가 키 없이 깨지지 않게 | tauri.conf.json에 직접 |
| 29 | CEO spec 1 | 태그 빌드는 자체 서명 시크릿 필수 | Mechanical | P1 | 검사와 대체 규칙의 충돌 해소 | 언제나 ad-hoc 대체 |
| 30 | CEO spec 2 | 실측에서 a.dmg에 격리 속성 + Slack 로그인 | Mechanical | P1 | 실제 첫 설치와 키체인 항목을 시험 | 로컬 dmg 그대로 |
| 31 | CEO spec 2 | `updated_to` 성공 = 지금 버전 >= 기록 | Mechanical | P1 | 실패 뒤 손 설치한 새 버전이 실패로 보이지 않게 | 같을 때만 |
| 32 | CEO spec 2 | 공개 게시를 `publish` 작업으로 분리, Slack은 그 뒤 | Mechanical | P1 | Slack 링크가 공개 Release를 가리킴, 위험 표와 일치 | release 작업 안 |
| 33 | CEO spec 2 | `.sig` 두 개로 고침 | Mechanical | P5 | 번들 대상 두 개 | 세 개 |
| 34 | CEO spec 2 | `failed {message, manual}` | Mechanical | P5 | 자동·수동 실패 구분을 표현 | 이전 상태로 되돌리기 |
| 35 | CEO spec 2 | `just_updated`를 `update_state`로, 표시 자리·닫기·순서 정의 | Mechanical | P5 | UI가 읽을 인터페이스 필요 | Rust 전용 필드 |
| 36 | CEO spec 2 | 업데이터 키는 1단계 시작 때 | Mechanical | P1 | `pubkey`가 1단계 설정에 들어감 | 2단계 |
| 37 | CEO spec 2 | 초안 공개 때 `--latest` 명시 | Mechanical | P5 | 자동 판정에 기대지 않음 | 생략 |
| 38 | CEO spec 3 | `install_update(confirmed)` + `Err("busy")` | Mechanical | P5 | 화면이 진행 중 작업을 알 길이 없었다 | `update_state`에 busy |
| 39 | CEO spec 3 | 실측 실패별 대응 표(5.8) | Mechanical | P1 | ad-hoc은 App Management·Gatekeeper 실패를 못 고친다 | 모두 ad-hoc |
| 40 | CEO spec 3 | 준비 줄도 첫 화면에, 줄 순서 정의 | Mechanical | P5 | 첫 화면 사용자도 업데이트를 본다 | 홈만 |
| 41 | CEO spec 3 | 실측 하네스와 "스스로 다시 뜸" 통과 조건 | Mechanical | P1 | macOS 재실행 경로를 릴리스 전에 확인 | 4단계에서만 |
| 42 | CEO spec 3 | Windows PC가 있으면 2단계에서 Windows a→b | Taste | P1 | 릴리스 전 Windows 설치 경로 확인. 없으면 회복 비용은 지금과 같다 | 4단계 첫 시험 |
| 43 | CEO 0H | CEO 범위 문서와 수정된 계획 승인(A) | Mechanical | P6 | 두 문서가 결정 장부와 일치, 스펙 리뷰 3회 반영(29개 중 24개 리뷰어 확인, 5개 본문 확인) | B 수정, C 멈춤 |

#### 0H. 스펙 리뷰 결과

3회(상한). 점수 6 → 7 → 8. 지적 29개(17 + 7 + 5), 리뷰어가 다음 회차에서 다시 꺼내지 않은 것 24개, 마지막 회차 5개는 반영 뒤 본문에서 확인했다(issues verified: 5 of 5, 리뷰어 재확인 없음). 지표는 `analytics/spec-review.jsonl`에 기록했다. CEO 범위 문서: `~/.gstack/projects/sijecorp-meeting-summarizer/ceo-plans/2026-11-05-in-app-updater.md`.

#### 0I. 구현 시간대별 질문

```
  1시간째(기초)   : 업데이터 키를 먼저 만들고 공개 키를 넣는다. `tauri-plugin-updater` 기능 플래그(native-tls, system-proxy, zip 없음)가
                    Cargo.lock에 rustls를 새로 끌고 오지 않는지 `cargo tree -i rustls`로 본다. 플러그인 등록 위치(setup의 #[cfg(desktop)]).
  2~3시간째(핵심) : `shutdown_work`를 빼내며 `confirm_quit`·`RunEvent::Exit` 동작이 바뀌지 않는지(기존 App.test·queue 테스트).
                    `workers_running`이 halt 뒤에도 자식 핸들을 보는지(`Job.child`가 halt 뒤 남는지 확인).
                    받기 감시: `download`의 on_chunk 콜백은 동기 클로저라 시각을 공유 상태(AtomicU64)로 남기고 select로 본다.
  4~5시간째(통합) : 트레이 메뉴 재구성(`set_menu`)이 macOS 앱 메뉴와 섞이지 않는지. `app.restart()`가 single-instance 잠금과
                    겹치지 않는지(스펙 리뷰 3: Exit에서 잠금이 풀린다). 홈 줄 세 개(준비, 결과, 설정)의 자리.
  6시간째+(마무리): 미리보기 장면, 문서 9곳, CI의 publish 작업과 latest-json.ts 테스트, 2단계 실측(사람이 함께).
```

막는 요소(feasibility blocker): 없음. 2단계 실측은 사람(권한 허용, "그래도 열기")이 필요하고, 3단계 시크릿 등록과 공개 저장소 생성은 사용자가 한다.
노력: 사람 팀 약 4~5일 / CC 약 3~4시간(코드·테스트) + 실측 1시간(사람 동반) + CI 시행착오.
| 44 | CEO voices | 조용한 시작: 1.2.0은 시험 기기만, 1.2.1을 알림 | Mechanical | P1 | 두 목소리 일치(Claude 1·8, Codex 3). 업데이터 결함이 곧 전원 손 재설치가 되는 것을 막음 | 1.2.0을 바로 알림 |
| 45 | CEO voices | 녹음 중 설치 금지 | Mechanical | P1 | Codex 2: 녹음은 되돌릴 수 없다 | 확인 뒤 녹음 중단 |
| 46 | CEO voices | 대기 초과 시 설치 중단 후 재시작 | Mechanical | P1 | Codex 2: 반쯤 덮인 설치를 만들지 않음 | 경고 후 계속 |
| 47 | CEO voices | 설치 직전 재확인 + 철회 절차 | Mechanical | P1 | 두 목소리 일치(Claude 9, Codex 4) | 없음 |
| 48 | CEO voices | Slack은 publish 실패와 무관하게 업로드 | Mechanical | P1 | Codex 5: 긴급 배포 경로 유지. 스펙 2의 `needs: publish`를 바꿈 | `needs: publish` |
| 49 | CEO voices | GitHub App 토큰 | Mechanical | P1 | Claude 10: 만료·퇴사 단일 실패점 제거 | fine-grained PAT |
| 50 | CEO voices | 0단계 권한 실측(코드 없이) | Mechanical | P1, P3 | Claude 2: 몇 시간으로 Mac 방향을 1단계 전에 정함 | 2단계에서만 |
| 51 | CEO voices | 1.1.2로 자체 서명만 먼저 배포 | Taste | P5 | Claude 2 선택 사항. 손 설치가 두 번(1.1.2, 1.2.1)으로 늘어 기본은 하지 않음 | 1.1.2 배포 |
| 52 | CEO voices | 1.2.0 범위 줄이기(트레이·노트 뒤로) | Taste | P1 | Claude 4. 그 코드는 업데이트 경로 밖이고 트레이 앱의 주 통로라 유지. 60초 감시는 느린 회선에서 총 시간 제한보다 정확해 유지 | 1.2.1로 미룸 |
| 53 | CEO voices | 끝난 자리(보낸 뒤)에도 준비 줄, Slack 꼬리말에 버전 | Taste | P3 | Claude 5. 측정(download_count)으로 먼저 보고 X6·X7 승격 조건에 묶음 | 지금 추가 |
| 54 | CEO voices | 측정과 X6·X7 승격 조건 | Mechanical | P1 | 두 목소리 일치(Claude 5, Codex 6) | 없음 |
| 55 | CEO voices | 공개 저장소 대신 비공개 URL 버킷 재검토 | Taste | P6 | Claude 3. 사용자 D1에서 이미 정함. 릴리스 노트 공개 글 규칙만 더함 | R2·S3 |
| 56 | CEO voices | D-U-N-S 조회 시작 | Mechanical | P6 | Claude 7. 사용자가 유료 서명을 하지 않기로 함(U2). 전제 5 대체만 기록 | 지금 시작 |
| 57 | CEO voices | ad-hoc 대체 시 녹음 전 테스트 표시 | Mechanical | P1 | Codex 1 | 사라지는 안내만 |
| 58 | CEO voices | D2 뒤집기 기록(U3) | Mechanical | P5 | Claude 6 | 없음 |
| 59 | CEO 2절 | `updated_to` 쓰기 실패는 로그만, 설치는 계속 | Mechanical | P1 | 읽기 전용 설정(새 버전 것)에서도 업데이트가 막히지 않게 | 설치 중단 |
| 60 | CEO 4절 | 상태 잠금 두 순서 시험(수동 확인 중 타이머, 설치 재확인 중 타이머) | Mechanical | P1 | 공유 상태 순서 불변식의 회귀 증명 | 단일 순서 시험 |
| 61 | CEO 5절 | `install_update`를 revalidate·shutdown_work·wait_children·install_and_restart로 나눔 | Mechanical | P5 | 분기 5개 초과 | 한 함수 |
| 62 | CEO 6절 | 시간 상수는 주입 가능한 시계·짧은 상수로 시험 | Mechanical | P1 | 깨지기 쉬운 시간 의존 시험 방지 | 실제 대기 |
| 63 | CEO 8절 | 4단계에서 NSIS 로그 위치를 확인해 install.md 관리자 절에 적음 | Mechanical | P1 | Windows 설치 실패 원인이 앱 로그에 없다 | 없음 |
| 64 | Design H1·C10 | 버전 행을 `fieldset disabled={locked}` 밖에 두고 설정 읽기 실패 화면에도 보임 | Mechanical | P1 | 새 버전이 쓴 설정(읽기 전용)에서 업데이트가 막히면 회복 길이 없다 | 일반 섹션 안 |
| 65 | Design H2 | 로그 폴더와 한 "앱 정보" 묶음, 탭 설명에 앱 버전 | Mechanical | P5 | 즉시 동작 행끼리 모으고 찾을 수 있게 | 별도 묶음 |
| 66 | Design H3 | 버튼 문구를 "다시 시작해 업데이트" 하나로(트레이 포함, CEO 줄 대체) | Mechanical | P5 | 같은 행동에 두 이름이면 다른 일로 읽힌다 | "업데이트하고 다시 시작" |
| 67 | Design H4·C6 | 홈은 link xs·닫기 없음, 설정은 `ready`만 brand sm | Mechanical | P5 | 녹음 버튼이 홈의 기준점으로 남는다 | 홈에 강한 버튼 |
| 68 | Design H5·C6 | 홈·첫 화면 줄 순서: 오디오 확인 → 설정 → 업데이트, 업데이트 줄은 하나 | TASTE | P1 | 녹음 준비가 미뤄도 되는 업데이트보다 먼저(Codex). CEO 단계 순서(준비 줄 먼저)를 뒤집으므로 관문에 올린다 | 준비 줄, 결과 줄, 설정 줄 |
| 69 | Design M1·M8·C3 | 준비+녹음 중, 준비+가져오기·내보내기 중 행. 대화상자는 전사·요약만 | Mechanical | P1 | 내보내기가 말없이 취소되지 않게 | 내보내기도 대화상자로 취소 |
| 70 | Design M2·C2 | 누르는 즉시 앱 전체 `installing`, `inert`, 알림 띠 단계 문구, Rust가 같은 잠금으로 녹음 시작 거절 | Mechanical | P1 | 재확인·정리 40초 동안 녹음 시작과 경합 | 설정 버튼만 막음 |
| 71 | Design M3·C5 | 설치 실패 기록(`install_failed`), 그 버전 다시 받지 않음, 더 새 버전이 지움 | Mechanical | P1 | 실패한 설치를 반복하는 고리 방지, 실패가 다시 시작해도 남는다 | 세션 한 번 표시 |
| 72 | Design M4·C9 | 재확인 결과 문구 "이 업데이트는 취소되었습니다", "더 새 버전이 나와 다시 받습니다" | Mechanical | P1 | 버튼이 말없이 바뀌지 않게 | 문구 없음 |
| 73 | Design M5·C5 | 오류 4분류 한국어 문구, 원문은 `app.log`, 분류는 순수 함수 | Mechanical | P1 | 원문 영어 오류는 행동을 알려 주지 않는다 | 오류 원문 한 줄 |
| 74 | Design M6·C5 | `last_check_ok` 7일 넘으면 설정에만 안내 | Mechanical | P3 | 자동 실패가 계속되면 아무도 모른다. 홈은 조용히 | 홈에도 표시 |
| 75 | Design M7·C8·C9 | 버튼 자리 유지(`aria-disabled`), 크기 모르면 MB, 15초 무진행 "연결을 기다리는 중…" | Mechanical | P1 | 초점 잃음과 멈춘 진행률 방지 | 버튼 숨김 |
| 76 | Design M9·C9 | 노트 비면 접기 없음, 개발 빌드는 확인 안 함, 상태 불러오기 실패 행 | Mechanical | P1 | 빈 상태 명시 | 정하지 않음 |
| 77 | Design C9 | 서명 확인 중 상태는 두지 않음 | Mechanical | P5 | 마지막 조각 뒤 몇 밀리초, 문구가 깜빡이기만 한다 | "업데이트 파일을 확인하는 중…" |
| 78 | Design J1·C4 | `last_run_version`과 `audio_check_needed`, 홈 맨 위 오디오 확인 줄, 테스트 통과로 끔, 녹음은 막지 않음 | Mechanical | P1 | 1.1.1 → 1.2.x에서 시스템 오디오가 조용히 0이 된다. 막으면 곧 시작할 회의를 놓친다 | 릴리스 노트만, 또는 녹음 차단 |
| 79 | Design J2 | 성공 줄에 "바뀐 것"(공개 Release 페이지) | Mechanical | P1 | 무엇이 바뀌었는지 볼 곳 | 없음 |
| 80 | Design J3 | 녹음 중 자동 확인 건너뜀 | Mechanical | P3 | 회의 중 대역폭과 받기 실패 방지, 비용 없음 | 항상 확인 |
| 81 | Design J4 | Windows 설치 문구에 "창이 닫혔다가 다시 열립니다" | Mechanical | P1 | 창이 사라지면 꺼진 줄 안다 | 없음 |
| 82 | Design S1 | 버전 숫자 뒤 "버전"을 끼워 조사("1.2.2 버전을") | Mechanical | P5 | `josa()`는 숫자를 못 읽는다 | 숫자 뒤 조사 |
| 83 | Design S2 | CI가 `## 바뀐 것`만 평문으로, 화면 이름 "바뀐 것 보기" | Mechanical | P5 | 릴리스 노트 제목과 같은 말. 설치 파일 표가 노트에 섞이지 않는다 | 노트 전체, "변경 사항"(Codex) |
| 84 | Design S3 | 공개 저장소 README에 동료용 설치 안내 | Mechanical | P1 | 동료는 비공개 install.md를 못 연다 | install.md 링크 |
| 85 | Design S4 | 트레이 순서: 다시 시작해 업데이트, 구분선, 열기, 종료 | Mechanical | P5 | 새 항목이 눈에 띄고 종료와 떨어진다 | 열기 다음 |
| 86 | Design S5 | 수동 확인 최신이면 3초 "방금 확인했습니다" | Mechanical | P3 | 눌렀는데 그대로면 안 된 줄 안다 | 바로 "최신 버전입니다" |
| 87 | Design C1 | 설치 전 회의 편집 대기분 저장, 실패면 멈춤. 설정 초안이면 막음(자동 저장·폐기 안 함) | Mechanical | P1 | 다시 시작이 고친 내용을 잃는다(Codex만, 데이터 손실이라 수용) | 그대로 다시 시작 |
| 88 | Design C7·H5 | `sm` 미만 버튼 아래로, 줄바꿈, 첫 화면 세로 스크롤 허용, 520×600·1040×720 미리보기 | Mechanical | P1 | 최소 창에서 버튼이 밀려나지 않게 | 정하지 않음 |
| 89 | Design C8 | 단계 바뀔 때만 `role=status`, 대화상자 첫 초점 "취소"와 초점 복귀, 닫기 32×32 `aria-label` | Mechanical | P1 | 기존 패턴(`Home.tsx:455`, `Settings.tsx:1504`)과 같게 | 퍼센트마다 알림 |
| 90 | Design S6 | ad-hoc 대체의 권한 안내도 같은 `audio_check_needed`로 | Mechanical | P4 | 장치 하나로 두 경우 | 별도 문구 줄 |
| 91 | DX C1·X10 | 10장 우선 규칙을 시간순(나중 줄이 이김)으로, 구현 첫 작업으로 본문에 접기 | Mechanical | P5 | 옛 줄(10초 뒤 설치 계속 등)이 본문을 이기는 규칙 | 지금 규칙 유지, 리뷰 중 전면 재작성 |
| 92 | DX C6·X5 | `install_update` 결과 열거형(`started`, `needs_confirm`, `blocked`, `not_ready`), 실제 실패만 `Err` | Mechanical | P5 | 기존 명령은 오류 채널에 한국어 문구를 쓴다. 확인 필요는 오류가 아니다 | `Err("busy")` 문자열 |
| 93 | DX C6·X5 | 상태는 serde 태그 열거형 + TS 판별 유니온, 이벤트 내용 = `update_state`, 구독 뒤 조회 | Mechanical | P1 | 구현자가 모양을 추측하지 않게 | 정하지 않음 |
| 94 | DX C6 | 오류 `kind`는 Rust, 한국어 문구는 화면 한 곳 | Mechanical | P4 | 사용자 문구가 두 곳에 갈라지지 않게 | Rust가 문구까지 |
| 95 | DX C8·X6 | 업데이터 상태는 `update-state.json`(Rust 전용 파일) | Mechanical | P1 | 설정은 읽기 전용·저장 막대·덮어쓰기 경합(`settings.rs:254`)이 있다 | 설정 파일에 필드 추가 |
| 96 | DX C3 | `AUDIO_RECHECK_BELOW` 상수, 인증서를 바꾸면 올림 | Mechanical | P1 | 인증서 교체가 조용한 무음 녹음을 되살린다 | 1.2.0 고정 비교 |
| 97 | DX C9·X7 | 빌드 시 `MS_UPDATER=1`일 때만 업데이터, 런타임 끝점 변수는 두지 않음 | Mechanical | P5 | 로컬 설치 파일이 공개 버전으로 자기를 바꾸지 않게. 기존 `MS_*` 규칙 | `tauri::is_dev()`만, 런타임 `MS_UPDATE_ENDPOINT` |
| 98 | DX C5·X4 | 배포 설정에 `dangerousInsecureTransportProtocol`이 있으면 CI 실패 | Mechanical | P1 | 시험 설정이 배포에 섞이는 사고 방지 | 검사 없음 |
| 99 | DX C2·X3 | 관리자 문서는 install.md 관리자 절 하나, 복사 명령과 성공 확인 | Mechanical | P4 | 이미 있는 관리자 절(112행)을 쓴다 | 새 문서 파일 |
| 100 | DX C2·X3 | 키·인증서·App 키별 복구 절, 두 번째 보관자 서명 시험이 3단계 완료 조건 | Mechanical | P1 | 복구는 처음 하는 사람이 한다 | 시크릿 이름 목록만 |
| 101 | DX C2 | build.yml에 `APPLE_CERTIFICATE`를 쓰지 않는 이유 주석 | Mechanical | P5 | 누군가 "단순화"해 되돌리지 않게 | 없음 |
| 102 | DX C5·X4 | `scripts/updater-test.ts`와 0·2단계 절차(백업·복구 포함), 인증서는 0단계 1회 | Mechanical | P1 | 업데이터를 고칠 때마다 다시 필요하다. 개발 앱과 데이터 공유(`README.md:173`) | 계획 문장만 |
| 103 | DX X1 | 공개 태그 불변: 태그 빌드 맨 앞에서 공개 Release가 있으면 실패, `publish`만 재실행 | Mechanical | P1 | 재실행이 비공개 설치 파일을 바꾸고 Slack에 다른 바이너리가 간다(`build.yml:309`) | 공개 단계에서만 검사 |
| 104 | DX X2 | 롤백 명령에 `--repo`, `latest.json` 확인 명령, 제한 명시 | Mechanical | P1 | 소스 체크아웃에서 실행하면 엉뚱한 저장소를 바꾼다 | 저장소 생략 |
| 105 | DX X8 | 세 파일 버전을 태그와 비교, 태그 체크리스트 | Mechanical | P1 | 지금은 `tauri.conf.json`만 본다(`build.yml:53`) | 한 파일 검사 |
| 106 | DX X8 | `latest-json.ts` CLI 인자, 종료 코드, 실패 문구 | Mechanical | P5 | 실패가 무엇을 고칠지 말해야 한다 | 정하지 않음 |
| 107 | DX C4·X9 | 노트 틀과 계약, 노트 없거나 `## 바뀐 것` 없으면 `publish` 실패, 공개에 `--generate-notes` 안 씀, 링크는 공개 README | Mechanical | P1 | 비공개 링크와 PR 제목·작성자가 공개된다(`v1.1.1.md:10`, `build.yml:313`) | 관례로만 |
| 108 | DX C7 | CI `::error::` 문구 7종(문제·원인·고칠 방법) | Mechanical | P1 | 새 실패 지점마다 다음 행동이 필요하다 | 기본 실패 출력 |
| 109 | DX C10 | `download_count` 명령과 과대 집계 주의 | Mechanical | P1 | 측정 줄에 방법이 없다 | 없음 |
| 110 | Eng H1·C7 | `publish` 직렬화 + semver latest 판정 + 공개 뒤 확인 | Mechanical | P1 | 태그 실행이 병렬이라 늦게 끝난 옛 태그가 latest가 된다(`build.yml:19`) | 무조건 `--latest` |
| 111 | Eng C8 | release·publish는 push 이벤트 + 태그일 때만 | Mechanical | P1 | 태그를 고른 수동 실행이 공개한다(`build.yml:292`) | 태그 조건만 |
| 112 | Eng H2·C10 | `INSTALL_WAIT = reap + 5초`, 정리 단계 전체에 걸림, 막히는 정리는 메인 스레드 밖 | Mechanical | P1 | 대기 10초 = 엔진 정리 10초(`sidecar.rs:67`) | 고정 10초 |
| 113 | Eng C1 | 띄우는 중 사이드카를 생명주기 잠금 안에서 셈 | Mechanical | P1 | 세대 확인과 등록 사이에 `engine_running()`이 거짓 | 자식만 폴링 |
| 114 | Eng H2 | 설치 실패 기록은 수동 확인으로 1회 재시도(`count < 2`) | Mechanical | P1 | 한 번 실패가 그 버전을 영구히 막는다 | 영구 차단, `count` 미사용 |
| 115 | Eng M5·C5 | `INSTALLING` 원자 깃발 손잡기, 파일 작업 RAII 수는 명령 입구에서 | Mechanical | P5 | 잠금 둘 엇갈림 교착, 저장 대화상자 동안 빠짐 | 같은 잠금(미정) |
| 116 | Eng C2 | Slack 보내기 수가 저장까지 설치를 막음(`blocked: sending`) | Mechanical | P1 | 보냈는데 저장 전 종료되면 중복 전송 | 막지 않음 |
| 117 | Eng C3·C4 | 화면 준비 단계(inert, 비우기, 확인 뒤 다시 비우기), 설정 저장 중도 막음 | Mechanical | P1 | 비우는 동안 입력이 빠져나가고 `saving`이 남는다 | `started` 뒤에만 inert |
| 118 | Eng C6 | `INSTALLING`이면 메뉴 종료·ExitRequested 미룸 | Mechanical | P1 | 메뉴 종료가 `app.exit(0)`로 설치를 끊는다(`lib.rs:398`) | 중재 없음 |
| 119 | Eng M4 | 비동기 명령, `request_restart`, `shutdown_work` 두 번 안전 | Mechanical | P1 | 메인 스레드 재시작은 `RunEvent::Exit`를 건너뛰고 관리자 암호 경로가 교착 | 메인 스레드 `restart()` |
| 120 | Eng M7 | macOS 사전 점검(번역 위치, /Volumes, 같은 볼륨, 쓰기 권한)은 작업을 멈추기 전에 | Mechanical | P1 | 알 수 있는 실패가 전사를 멈춘 뒤에야 드러난다 | 설치 시 실패 |
| 121 | Eng M1 | 배포 파일 검사: 풀린 앱 codesign·해시, `.sig` 공개 키 검증, URL HEAD | Mechanical | P1 | 잘못된 키나 서명이 모든 앱에 거절되는 릴리스를 만든다 | 번들 폴더 앱만 검사 |
| 122 | Eng M2 | `requireSignedVersion: true` | Mechanical | P1 | 사내 TLS 프록시가 피드를 바꿔 다운그레이드할 수 있다. CLI가 이미 버전을 서명한다 | 끔 |
| 123 | Eng M3 | 5.8 첫 줄 분리: TCC는 ad-hoc, 키체인만이면 자체 서명 유지 + 안내 | Mechanical | P1 | 키체인 창 하나로 큰 이득(조용한 무음 방지)을 버리지 않게 | 한 줄로 묶음 |
| 124 | Eng C9 | macOS `MS_UPDATER` 빌드는 인증서 필수 | Mechanical | P1 | ad-hoc 수동 빌드가 오디오 확인 없이 서명을 바꾼다 | 서명 세대 추적(더 복잡) |
| 125 | Eng M8 | CI 키체인 `-lut 21600`, 서명 직전 unlock, always 정리 | Mechanical | P1 | 긴 컴파일 동안 임시 키체인이 잠긴다 | 기본 설정 |
| 126 | Eng LOW | 플러그인 사실 고침(timeout, 플랫폼 하나, ReleaseNotFound=feed, 중립 문구) | Mechanical | P5 | 플러그인 2.14.1 소스와 다르다 | 계획 문장 그대로 |
| 127 | Eng LOW | `update_state`에서 `blocked` 제거, 트레이는 `refresh_tray` | Mechanical | P5 | 모든 녹음·파일 변화가 이벤트를 다시 쏘는 결합을 피한다 | 상태에 blocked |
| 128 | Eng LOW | 30분 깨움 + 벽시계 6시간 | Mechanical | P1 | 잠자기 동안 tokio 타이머가 멈춘다 | 6시간 타이머 |
| 129 | Eng LOW | 재확인은 유지하고 10초로 | Mechanical | P3 | CEO 안전장치 유지, 클릭 뒤 대기 축소 | 제거(독립 리뷰어 제안), 30초 |
| 130 | Eng LOW | update.rs 패닉 없음, 독 허용 lock | Mechanical | P1 | 릴리스 빌드는 패닉에 중단 | unwrap |
| 131 | Eng LOW | 0단계 전 `tccutil reset`, 시험 중 `tauri dev` 금지 | Mechanical | P1 | 기존 권한 항목이 결과를 흐린다 | 그대로 시험 |
| 132 | Eng LOW | 서명 키 빌드 스크립트 노출과 Windows 반쯤 설치는 위험 표로 받아들임 | Mechanical | P3 | 옮기는 비용이 크고 4단계가 확인한다 | 서명 단계 이동 |
| 133 | Eng LOW | slack-release 수동 실행 기본 링크는 공개 저장소, 1.2.1 뒤 문구 변경 | Mechanical | P5 | 수동 실행에 publish 결과 입력이 없다 | 그대로 |
| 134 | Eng Test | 테스트 13종 추가(수락 줄 목록) | Mechanical | P1 | 새 경로마다 회귀를 잡는 테스트 | 수동만 |
| 135 | Eng Voices | 합의 5/6, 보안은 Claude만이고 수용 | Mechanical | P6 | Codex가 반대하지 않았다 | 없음 |
| 136 | Gate d1 | 업데이트 알림을 왼쪽 아래 떠 있는 카드로(녹음 화면 제외, 닫기 있음·다시 켜면 다시 보임, 결과도 같은 카드, "v1.2.2 버전이 준비되었습니다") | User | - | 사용자 결정(관문 답과 D3~D6) | 홈 한 줄 |
| 137 | Gate d4 | Windows 업데이트 첫 시험은 4단계 | User | - | 사용자 결정 | 2단계에서 Windows도 시험 |
| 138 | Design2 D9 | 카드 문구 "1.2.2 버전이 준비되었습니다"(v 없이) | User | - | 사용자 결정. 앱 전체 표기와 같게 | v1.2.2 버전 |
| 139 | Design2 D10 | 카드 버튼 outline sm | User | - | 사용자 결정(Claude brand sm, Codex outline sm으로 갈림) | brand sm |
| 140 | Design2 D11 | 좁은 창은 아래 스택 위, 왼쪽 정렬 | User | - | 사용자 결정. 보내기 막대를 가리지 않게 | 가운데 정렬 |
| 141 | Design2 D12 | 녹음 중에는 모든 화면에서 준비 카드 숨김 | User | - | 사용자 결정. 트레이와 같은 규칙 | 녹음 화면만 |
| 142 | Design2 D13 | 설정 일반 탭에서 카드 숨김 | User | - | 사용자 결정. 버전 행과 중복 | 모든 탭 |
| 143 | Design2 D14 | 준비 카드에 "바뀐 것 보기" 링크 | User | - | 사용자 결정. 재시작 전에 노트를 본다 | 없음 |
| 144 | Design2 D15 | 14일 확인 실패 카드 | User | - | 사용자 결정. 방화벽에 막힌 동료를 찾는다 | 설정 7일 문구만 |
| 145 | Design2 C1·Cx1 | 760px 기준 위치, z-20, 여백·캐럿·전사본 경계 늘림 | Mechanical | P1 | 520px 창에서 보내기 막대와 겹친다 | 정하지 않음 |
| 146 | Design2 H1·Cx6 | 카드 모양 토큰과 비모달 영역 접근성, 디자인 시스템 새 절 | Mechanical | P5 | 기존 토큰으로 정해야 구현자가 경고색을 고르지 않는다 | 정하지 않음 |
| 147 | Design2 H2·Cx4 | 닫기 수명(프로세스, 종류·버전), 사용자 시작 흐름은 닫힘 무시, 설치 중 × 없음 | Mechanical | P1 | 닫은 뒤 트레이 설치가 아무 반응 없이 끝나는 막다른 길 | 닫으면 모두 숨김 |
| 148 | Design2 H3·Cx3 | 5.4 한 표에 카드 열과 빠진 행 6개 | Mechanical | P1 | 카드 상태가 정해지지 않았다 | 카드는 문구 재사용만 |
| 149 | Design2 H4·Cx2 | 단계 문구는 카드 하나, 카드는 inert 밖, 포털·전역 단축키 차단, 초점 규칙 | Mechanical | P1 | inert가 초점을 잃게 하고 문구가 세 곳에서 중복 | 알림 띠 + 줄 + 버전 행 |
| 150 | Design2 H5 | 재확인 뒤 카드 유지, 철회 문구 유지, "마지막으로 확인하는 중…" | Mechanical | P1 | 누른 동작의 결과가 사라진다 | 카드 제거 |
| 151 | Design2 M2 | 결과 카드 노트는 카드 안에서 펼침 | Mechanical | P1 | 공개 Release 페이지 맨 위가 설치 파일 표 | Release 페이지 링크 |
| 152 | Design2 M6 | NSIS 한국어, OS별 설치 단계 문구 | Mechanical | P1 | passive 설치 창이 영어로 뜬다 | 기본값 |
| 153 | Design2 M7 | 확인 대화상자 버튼 "다시 시작해 업데이트" | Mechanical | P5 | 다시 시작된다는 것을 다시 알린다 | "업데이트" |
| 154 | Eng2 D16 | 설정 일반 탭에서 누르면 잠긴 동안만 카드와 초점, 풀리면 버전 행 | User | - | 사용자 결정. D13을 설치하는 몇 초만 예외로 | 끝까지 숨기고 버전 행만 잠금에서 빼기 |
| 155 | Eng2 C1 | 준비 예약 `prepare_update`·`cancel_update`·60초 만료, 그동안 종료 미룸 | Mechanical | P1 | 깃발 전 저장 중 메뉴 종료가 편집을 잃는다(`lib.rs:398`) | 깃발은 `install_update`에서만 |
| 156 | Eng2 C3 | `shutdown_work` 한 번, 마감 시각 하나를 `Exit`와 함께 씀 | Mechanical | P1 | 시간 초과 뒤 `Exit`가 막힌 정리를 다시 기다린다 | 두 번 불려도 안전(순차만) |
| 157 | Eng2 C4 | 스냅샷·이벤트에 `rev` | Mechanical | P5 | 늦은 `ready` 스냅샷이 `installing`을 덮는다(`App.tsx:137`과 같은 문제) | 구독 순서만 |
| 158 | Eng2 C5 | `origin`, `notice`, `just_updated.notes`, 상태 파일 새 필드 | Mechanical | P5 | 카드의 "다른 버전을 받는 중"·철회 유지·결과 노트를 IPC가 담지 못함 | 화면 기억에 맡김 |
| 159 | Eng2 C6 | `count` 유지, `attempt`로 한 번만 셈 | Mechanical | P1 | 기록을 지우면 두 번 실패 끝이 오지 않는다 | 재시도 때 기록 지움 |
| 160 | Eng2 C7·M10 | 좁은 창은 스택 안, 넓은 창은 측정 겹침 시 전환, `--bottom-stack` | Mechanical | P5 | 800px에서 저장 막대와 겹친다(`Settings.tsx:1821`) | 760px 기준과 화면별 여백 |
| 161 | Eng2 C8 | `onTab`과 `notesIntent`, 다시 마운트 없음 | Mechanical | P1 | Settings는 탭 처음 값만 받는다(`Settings.tsx:173`) | 탭 키로 다시 마운트 |
| 162 | Eng2 C9 | `go`·대화상자 앞 준비 판정 하나 | Mechanical | P1 | 알림 클릭·끌어놓기가 `inert`를 우회(`App.tsx:202`, `:314`) | 화면별 막기 |
| 163 | Eng2 C10·M2 | 실패한 시도 뒤에만 판정, `first_seen_at` 기준, 음수는 0 | Mechanical | P1 | 휴가 뒤 거짓 경고, 처음 설치의 기준 없음 | 시작 때 판정 |
| 164 | Eng2 C11 | 승격은 공개된 모든 태그와 비교, 공개 뒤 확인은 기대 latest | Mechanical | P1 | 롤백 뒤 늦은 publish가 실패하거나 나쁜 릴리스를 다시 올린다 | 지금 latest와 비교, 가장 높은 태그 확인 |
| 165 | Eng2 H1 | 접기를 1단계 첫 커밋으로, 지울 옛 문장 목록 | Mechanical | P5 | 구현자가 대체된 문장을 구현할 위험 | 지금 본문을 다시 씀 |
| 166 | Eng2 H2 | 태그 전 업데이트 시험(업데이터 경로가 바뀔 때), 두 OS CI 시작 확인 | Mechanical | P1 | 시작 직후 죽는 릴리스는 앱 안에서 고칠 수 없다 | 없음 |
| 167 | Eng2 M3 | `update-state.json`은 `app_local_data_dir` | Mechanical | P1 | 설정 폴더는 Windows 로밍(`lib.rs:332`) | 설정 옆 |
| 168 | Eng2 M5 | 공개 전 `.sig` 검증, trusted comment 버전 = 태그 | Mechanical | P1 | CLI는 키 불일치를 경고만 한다 | 공개 뒤 검증 |
| 169 | Eng2 M6 | Windows 단계 문구에 1분 안내, `on_before_exit` 로그 | Mechanical | P1 | 설치가 막히면 앱이 꺼진 채 남는다 | 문구 없음 |
| 170 | Eng2 M7 | macOS 버전 범위 위험 표, provenance 확인, 키체인 창 기본 경로 | Mechanical | P3 | 시험 기기가 한 대 | 언급 없음 |
| 171 | Eng2 M8 | 모델 내려받기도 파일 작업 보호 | Mechanical | P1 | 설치가 2.5GB 받기를 끊는다(`commands.rs:454`) | 받아들임 |
| 172 | Eng2 M9 | 진행 이벤트 250ms·1%, 15초는 Rust 타이머 | Mechanical | P3 | 조각마다 emit하면 수백 번 다시 그린다 | 조각마다 |
| 173 | Eng2 LOW 1·3 | NSIS 한국어 이미 있음(사실 고침), `release-assets.githubusercontent.com` | Mechanical | P5 | 설정 확인, curl 확인 | 바꾸기, 옛 주소 |
| 174 | Eng2 LOW 4 | 사전 점검은 Translocation과 `/Volumes`만, 쓰기 불가는 관리자 암호 창 | Mechanical | P5 | 같은 볼륨은 플러그인이 검사, 쓰기 줄과 암호 창 안내가 모순 | 네 가지 검사 |
| 175 | Eng2 LOW 5·6 | `RESTART_EXIT_CODE`만 통과, `install`은 `spawn_blocking` | Mechanical | P1 | 종료 중재가 다시 시작을 막거나 동기 tar 풀기가 스레드를 잡는다 | 깃발만 봄 |
| 176 | Eng2 LOW 7 | 6시간은 마지막 성공부터, 실패는 30분 뒤 | Mechanical | P1 | 잠자기에서 깬 직후 실패가 6시간을 기다린다 | 마지막 시도부터 6시간 |
| 177 | Eng2 LOW 2 | 관리자 문서에 publish 대기 교체와 다시 실행 | Mechanical | P3 | 세 번째 publish가 앞의 대기를 취소한다 | 언급 없음 |
| 178 | Eng2 LOW 9 | 링크 상수, 노트 64KB·200줄, App 토큰 `repositories`, Slack 앱 범위 확인 | Mechanical | P1 | 피드 값으로 링크를 만들지 않고 토큰 범위를 줄인다 | 피드 URL, 넓은 토큰 |
| 179 | Eng2 M1 | Windows 대기·중단 줄이기를 받지 않음 | Mechanical | P1 | 승인된 결정이고 한 목소리만 제안, Windows는 4단계 전 시험 없음 | 대기·중단 빼기 |
| 180 | Eng2 M4 | 서명 단계 분리를 받지 않음 | Mechanical | P3 | 악성 빌드 의존성은 키 없이도 바이너리를 바꾼다, 손 묶기 단계가 는다 | `tauri signer sign` 분리 |
| 181 | Eng2 YAGNI | 재확인·`count`·두 기준 유지 | Mechanical | P1 | 앞 단계와 관문에서 승인된 동작 | 빼기 |
| 182 | Eng2 H2 대안 | 초안 + 관리자 손 공개를 받지 않음 | Mechanical | P3 | 릴리스마다 사람 단계, 태그 전 시험과 시작 확인으로 대신 | 손 공개 |
| 183 | Eng2 M7 대안 | 4단계에 다른 macOS Mac 더하기를 받지 않음 | Mechanical | P3 | 승인된 조용한 시작 기기 구성을 바꾸지 않음, 위험 표로 | 기기 추가 |

### CEO 두 목소리 (Step 0.5)

- Claude 독립 리뷰어: 완료(INPUT `ceo 0f03833c…` 일치). 지적 10개(HIGH 3, MEDIUM-HIGH 2, MEDIUM 5).
- Codex(gpt-6-astra, plan-review): 완료(`OUTSIDE_STATUS: completed`). 지적 6개(High 5, Medium 1). 권고: "구현 전에 고쳐라. 녹음 연속성, 잘못된 릴리스 봉쇄, 업데이트 마찰 감소의 측정 없이 배포만 빨라진다."

```
CEO DUAL VOICES — CONSENSUS TABLE:
  Dimension                             Claude              Codex               Consensus
  1. Premises valid?                    부분(U1 공개·D2·전제5) 부분(릴리스 수 ≠ 사용자 영향) CONFIRMED: 전제를 측정으로 뒷받침해야
  2. Right problem to solve?            예                  예(결과를 "녹음 준비 유지"로) CONFIRMED
  3. Scope calibration correct?         1.2.0을 줄여라       안전장치를 더하라      DISAGREE → TASTE(감사 52)
  4. Alternatives sufficiently explored? 아니오(R2·S3, D-U-N-S) 언급 없음          Claude만, 사용자 결정으로 유지(감사 55·56)
  5. Competitive/market risks covered?  무료 노트 도구로 이탈  언급 없음            Claude만(정보)
  6. 6-month trajectory sound?          철회 절차 없음        철회·회복 경로 없음     CONFIRMED gap → 위험 표에 절차(감사 47)
```

두 모델이 모두 사용자의 방향(U1 공개 저장소, U2 무료 서명)을 바꾸자고 한 것은 없다. Codex는 U1·U2를 유지하라고 했고, Claude는 U1·U2를 다시 따졌지만 혼자였다. 그래서 User Challenge는 없다. Claude의 공개 범위 지적은 최종 관문에 정보로만 올린다.

### CEO 리뷰 1~11절

**지금 범위**: 모드 SELECTIVE EXPANSION(autoplan 규칙, 감사 1). 수락: U1, U2, X1~X5, R1, 스펙 리뷰 반영(감사 13~42), 두 목소리 반영(감사 44~50, 54, 57, 58). 미룸: X6, X7(TODOS). 건너뜀: X8. TASTE: X3(감사 6), 실측 순서(24), CI 전용 설정(28), Windows 사전 시험(42), 1.1.2(51), 범위 줄이기(52), 끝난 자리 표시(53), 공개 범위(55).

#### 1절. 아키텍처

```
  ┌──────────────── 비공개 sijecorp/meeting-summarizer ─────────────────┐
  │ build(macOS, Windows) ──▶ release(비공개 Release) ──▶ publish ──┐   │
  │   자체 서명(macOS)        gh release create             GitHub App 토큰 │
  │   업데이터 서명(두 OS)                                  latest-json.ts │
  │                              └──────▶ slack(설치 파일 업로드, 링크는 publish 성공 시)
  └───────────────────────────────────────────────────────────────┼───┘
                                                                  ▼
                       공개 sijecorp/meeting-summarizer-releases (Release: 설치 파일, .app.tar.gz, .sig×2, latest.json)
                                                                  │ https GET releases/latest/download/latest.json
  ┌────────────────────────────── 앱 (Tauri) ──────────────────────┼──────────────┐
  │ update.rs: Updater{state, bytes, just_updated} ── tauri-plugin-updater(native-tls)│
  │   ├─ check_update / update_state / install_update(confirmed)  (commands)        │
  │   ├─ 시작 30초 뒤·6시간 타이머                                                   │
  │   └─ shutdown_work() ◀── confirm_quit, RunEvent::Exit 도 같이 씀                 │
  │        ├─ Queue::shutdown, workers_running, engine_running                       │
  │        ├─ Recorder::stop   ├─ Store::pause_busy   └─ Exports::cancel_all         │
  │ settings.rs: updated_to   lib.rs: 트레이 항목(set_menu), 시작 로그 버전          │
  │ 웹뷰: Settings(앱 버전 묶음) · Home/첫 화면(준비 줄, 결과 줄) · App(대화상자, 이벤트) │
  └──────────────────────────────────────────────────────────────────────────────┘
```

- 새 결합: `update.rs`가 큐·녹음기·저장소·내보내기를 직접 알게 된다. `shutdown_work` 한 함수로 모아 종료 경로와 같은 순서를 쓰므로 결합은 정당하다. 큐·녹음기 내부에는 손대지 않고 `workers_running` 하나만 더한다.
- 규모: 사용자 수십 명, 6시간마다 한 번 GET. GitHub가 받는다. 10배, 100배에서도 문제없다.
- 단일 실패점: 공개 저장소(GitHub 장애 시 업데이트만 미뤄진다), 업데이터 개인 키(잃으면 전원 손 재설치, 두 곳 보관 + 두 번째 보관자), GitHub App 키(바꿀 수 있다).
- 보안 경계: 웹뷰는 `install_update`로 "이미 서명 확인이 끝난 업데이트"만 설치시킬 수 있다. 3절.
- 운영 실패 예: GitHub 다운로드 주소 차단(사내 방화벽) → 확인 실패, 손 설치는 Slack으로 그대로. 다룬다.
- 되돌리기: 잘못된 릴리스는 이전 태그를 `--latest`로(몇 분). 앱 자체 변경은 다음 버전으로 덮는다.
- 산출물 가치 점검: `latest.json`은 업데이터가 쓴다. 측정(`download_count`)은 X6·X7 승격 결정에 쓰고, 파일럿이 끝나면 접는다.
- 결과: 새 지적 없음(스펙 리뷰와 두 목소리에서 모두 반영).

#### 2절. 오류와 구조 지도

```
  METHOD/CODEPATH            | WHAT CAN GO WRONG                         | ERROR CLASS
  ---------------------------|-------------------------------------------|---------------------------------
  Updater::check             | 네트워크·프록시·TLS 실패, 30초 초과       | updater::Error::Reqwest
                             | 첫 공개 Release 전 404                    | 404 응답 → Error
                             | latest.json 형식 오류·플랫폼 누락          | Error::Serialization / TargetNotFound
                             | 지금보다 낮거나 같은 버전                 | Ok(None) (오류 아님)
  Updater::download          | 60초 무진행(잠자기, 끊긴 회선)             | 우리 감시 → Stalled
                             | 서명 불일치(.sig와 파일 다름)              | Error::Minisign
                             | 크기·연결 끊김                            | Error::Reqwest
  install_update             | ready 아님                                | Err("not ready")
                             | 녹음 중                                   | Err("recording")
                             | 진행 중 처리 + 미확인                     | Err("busy")
                             | 재확인이 다른 버전                         | 버리고 checking
                             | 엔진·워커 10초 내 안 끝남                 | 설치 중단 → restart
                             | updated_to 쓰기 실패(읽기 전용 설정)       | settings Err
  Update::install (macOS)    | /Applications 쓰기 권한 없음, rename 실패 | Error::Io / TempDirNotOnSameMountPoint
                             | App Management 차단                       | Error::Io(PermissionDenied)
  Update::install (Windows)  | ShellExecute 실패                         | Error::Io
                             | NSIS 복사 실패(exit 뒤)                   | 앱 밖(다음 시작에 판정)
  publish(CI)                | App 토큰 실패, 이미 공개됨, sig 누락       | job 실패
```

```
  ERROR                       | RESCUED? | RESCUE ACTION                                   | USER SEES
  ----------------------------|----------|-------------------------------------------------|-------------------------------
  check 네트워크/404/형식     | Y        | 자동: 로그만, 다음 6시간. 수동: failed{manual}   | 수동일 때만 오류 한 줄 + 릴리스 링크
  download 무진행/서명 불일치 | Y        | 받은 것 버림, failed, 다음 확인에 다시          | 수동이면 오류 + 링크, 자동이면 없음
  install not ready/recording | Y        | 거절                                            | 버튼 막힘 / "녹음이 끝나면…"
  install busy                | Y        | 대화상자                                        | "진행 중인 작업이 있습니다"
  재확인 다른 버전            | Y        | 버리고 확인 흐름                                | 준비 줄이 사라지거나 새 버전 받는 중
  10초 초과                   | Y        | 설치 안 함, updated_to 남기고 restart           | 다음 시작 "업데이트하지 못했습니다" + 링크
  updated_to 쓰기 실패        | Y        | 로그만, 설치는 계속                              | 결과 줄이 없다(설치는 된다)
  macOS 교체 실패             | Y        | plugin이 원래 앱 복구, 로그, restart            | "업데이트하지 못했습니다" + 링크
  Windows ShellExecute 실패   | Y        | 로그, restart                                   | 같음
  Windows NSIS 실패(앱 밖)    | Y(다음 시작) | 다음 시작에서 지금 버전 < updated_to 판정    | 같음
  publish 실패(CI)            | Y        | job 실패로 드러남, Slack은 링크 없이 업로드     | 동료에게는 Slack 설치 파일
```

- GAP 1개를 찾아 수락했다: `updated_to` 쓰기 실패(설정이 새 버전 것이라 읽기 전용일 때)는 로그만 남기고 설치를 막지 않는다(감사 59). 나머지는 모두 구조 동작과 사용자 표시가 있다.

#### 3절. 보안과 위협

| 위협 | 가능성 | 영향 | 대응 |
|---|---|---|---|
| 피드를 바꿔 악성 업데이트 | 낮음 | 높음 | minisign 서명 검증(끌 수 없음, 공개 키는 앱 안). 피드와 서명 키를 둘 다 가져야 한다 |
| 업데이터 개인 키 유출 | 낮음 | 높음 | GitHub 시크릿 + 비밀번호 관리자. 유출 시 새 키로 바꾼 버전을 옛 키로 서명해 내보내 회전(3.4) |
| 자체 서명 인증서 키 유출 | 낮음 | 중간 | 같은 지정 요구사항의 앱을 만들어 마이크 권한을 물려받을 수 있다. 그러려면 사용자가 그 앱을 같은 자리에 설치해야 한다. 시크릿에만 둔다 |
| GitHub App 키 유출 | 낮음 | 중간 | 공개 저장소에 Release를 올릴 수 있지만 서명 없이는 앱이 설치하지 않는다. 키를 바꾼다 |
| http로 내려받기 | 낮음 | 높음 | plugin이 https만 허용(`updater.rs:827`). 시험 설정만 예외이고 커밋하지 않는다 |
| 웹뷰에서 install_update 남용 | 낮음 | 낮음 | 이미 검증된 업데이트만 설치. 녹음 중 거절 |
| 공개 설치 파일의 비밀값 | 낮음 | 중간 | 비밀값 없음(`slack.rs:407` CLIENT_ID만, PKCE). 노트는 공개 글 규칙 |
| 새 의존성 | 낮음 | 중간 | `tauri-plugin-updater`(공식), `actions/create-github-app-token`(공식, SHA 고정) |

감사 기록: 설치 시작·실패·버전이 `app.log`에 남는다. 새 문제 없음.

#### 4절. 데이터 흐름과 상호작용 경계

```
  latest.json ─▶ 파싱·플랫폼 선택 ─▶ 버전 비교 ─▶ 받기(조각) ─▶ 서명 확인 ─▶ 메모리 bytes ─▶ install
     │404/형식오류     │플랫폼 누락        │같거나 낮음    │60초 무진행    │불일치          │앱 종료 시 버림  │재확인 다름→버림
     ▼                 ▼                  ▼              ▼               ▼                                ▼
   failed(자동은 조용) failed            up_to_date     failed          failed                         checking
```

- 공유 상태 순서: 상태는 Mutex 하나다. 자동 확인은 `idle`·`up_to_date`·`failed`에서만 들어가고, 수동 확인은 `checking`·`downloading`이면 버튼이 막혀 있다. 그래서 두 확인이 겹치지 않는다. 설치는 `ready`에서만 들어가고, 들어가면 `installing`으로 바꾼 뒤 재확인을 한다. 그동안 자동 확인은 건너뛴다. 막는 장치는 상태 검사와 전이를 한 잠금 안에서 하는 것이다. 테스트는 정지·재개 지점을 둔 두 순서(수동 확인 중 타이머, 설치 재확인 중 타이머)로 확인한다(감사 60).

| 상호작용 | 경계 | 처리? | 어떻게 |
|---|---|---|---|
| "다시 시작해 업데이트" 두 번 | 두 번째 클릭 | 예 | `installing`이면 거절, 버튼 막힘 |
| 트레이 항목 + 설정 버튼 동시 | 두 요청 | 예 | 같은 상태 잠금 |
| 받는 중 앱 종료 | bytes 버림 | 예 | 다음 시작 30초 뒤 다시 |
| 받는 중 잠자기 | 연결 끊김 | 예 | 60초 감시 → failed → 다음 확인 |
| 준비 중 녹음 시작 | 설치 요청 | 예 | 거절, 준비 줄은 녹음 화면에 안 보임 |
| 결과 줄 닫기 뒤 다시 시작 | 다시 보임? | 예 | 시작할 때 `updated_to`를 지웠으므로 안 보임 |
| 오프라인 시작 | 확인 실패 | 예 | 조용히 다음 확인 |

#### 5절. 코드 품질

- `install_update`는 분기가 5개를 넘는다(ready, recording, busy, 재확인, 대기, 설치 결과). 구현은 `revalidate`, `shutdown_work`, `wait_children`, `install_and_restart`로 나눈다(감사 61, 구현 지침).
- 중복: 정리 순서가 `confirm_quit`과 `RunEvent::Exit`에 이미 두 벌 있다(`commands.rs:345-359`, `lib.rs:476-490`). `shutdown_work`로 합치면 줄어든다.
- 과설계 점검: 상태 6개는 화면 표와 1:1이라 줄일 곳이 없다. 트레이·노트는 TASTE(감사 52)로 유지.

#### 6절. 테스트

```
  새 코드경로                          종류        행복 경로                 실패 경로                       경계
  Updater 상태 전이                    unit(Rust)  idle→checking→ready       check 실패→failed{manual}       ready 중 자동 확인 건너뜀
  install_update 가드                  unit(Rust)  ready·한가함→설치 진행    recording/busy/not ready 거절   confirmed=true 재호출
  설치 직전 재확인                     unit(Rust)  같은 버전→계속            다른 버전→버림, 네트워크 실패→계속
  wait_children 10초                   unit(Rust)  즉시 끝남                 초과→설치 안 함, restart
  받기 감시                            unit(Rust)  조각 계속                 60초 무진행→failed
  updated_to/just_updated              unit(Rust)  같음·높음→ok             낮음→실패, 쓰기 실패→계속       두 번째 시작에 없음
  트레이 항목 순수 함수                unit(Rust)  ready→항목 있음           녹음 중→없음
  shutdown_work 순서                   unit(Rust)  confirm_quit과 같은 순서  내보내기 취소 포함
  상태 잠금 두 순서                    unit(Rust)  수동 확인 중 타이머       설치 재확인 중 타이머
  설정 앱 버전 묶음                    vitest      상태별 문구·버튼          failed{manual}만 오류           노트 마크다운 그대로
  홈·첫 화면 줄                        vitest      준비·결과·설정 순서       결과 줄 닫기                    녹음 화면에 없음
  업데이트 대화상자·이벤트             vitest      busy→대화상자→true 재호출 recording→버튼 막힘             트레이 이벤트 같은 흐름
  latest-json.ts                       vitest(bun) 두 플랫폼                 하나 빠짐·빈 서명·버전 불일치    노트 파일 없음→빈 문자열
  macOS 실측(0·2단계)                  수동        권한 유지·다시 뜸         5.8 표
  실제 피드 1.2.0→1.2.1(4단계)         수동 E2E    두 기기 통과              통과 전 알리지 않음
```

- 금요일 새벽 2시 시험: 4단계 조용한 시작(실제 피드, 두 기기). 적대적 QA: 받는 중 잠자기, 준비 중 녹음, 업데이트 직후 철회. 혼돈 시험: `latest.json`에서 플랫폼 하나 지우기(스크립트가 거절해야 한다).
- 깨지기 쉬운 시험: 시간(30초·6시간·60초·10초)은 주입 가능한 시계·짧은 상수로 시험한다(감사 62).
- `no_claim`: Rust·vitest는 실제 GitHub 피드, NSIS, macOS rename·TCC를 증명하지 않는다(모의). 0·2·4단계 수동이 맡는다.

#### 7절. 성능

- 메모리: 받은 업데이트를 메모리에 둔다. 1.1.1 기준 dmg 18MB, setup.exe 24MB(`gh release view`, 2026-11-05)라 `.app.tar.gz`도 수십 MB다. 요약 모델(약 5GB 최대 메모리)에 비하면 작다.
- 대역폭: 6시간마다 작은 JSON 하나, 새 버전일 때 수십 MB 한 번. 회의 중이어도 부담이 작다.
- 느린 경로: 설치 대기 최대 10초(엔진·워커 종료), 재확인 최대 30초. 사용자가 누른 뒤의 일이고 "업데이트하는 중…"이 보인다.
- 새 문제 없음.

#### 8절. 관측과 디버깅

- 로그 줄: 시작 버전, 확인 결과(버전·소요), 받기(크기·시간), 재확인 결과, 설치 시작, 대기 초과, 설치 실패 원인, 다음 시작의 `just_updated`. 3주 뒤 보고가 와도 `app.log`만으로 다시 그릴 수 있다.
- 지표: 공개 저장소 `download_count`(파일럿 측정). 대시보드·알림은 이 규모에서 두지 않는다.
- 운영 절차: 잘못된 릴리스(위험 표), GitHub App 키 교체, 업데이터 키 회전(3.4).
- 새 문제 없음.

#### 9절. 배포와 롤아웃

```
  0단계 권한 실측 ─▶ 1단계 앱 ─▶ 2단계 업데이터 실측 ─▶ 3단계 시크릿·공개 저장소·CI ─▶ 4단계 조용한 시작
                                                                                       1.2.0(시험 기기만) → 1.2.1(실제 피드 검증)
                                                                                       ─▶ 통과: SLACK_AUTO 해제, 1.2.1 알림
                                                                                       ─▶ 실패: 고치고 1.2.2로 다시(알리지 않음)
```

- 이전 버전과 함께 돌 때: 1.1.1은 `updated_to`를 모르고 무시한다(`Settings`는 `#[serde(default)]`, `deny_unknown_fields` 없음, `settings.rs:61-62`). 데이터 형식은 바뀌지 않는다.
- 기능 플래그: 따로 두지 않는다. 업데이터는 피드가 없으면 조용히 실패하고, 알림은 `SLACK_AUTO`로 막는다.
- 배포 뒤 확인: 공개 Release에 자산 6개, `latest.json`의 두 플랫폼, 시험 기기의 결과 줄.
- 새 문제 없음(조용한 시작은 감사 44에서 수락).

```
  되돌리기 흐름
  잘못된 릴리스 발견 ─▶ 아직 설치 전? ─예─▶ gh release edit <이전 태그> --latest (또는 잘못된 것을 초안으로)
                          │                   ─▶ 받아 둔 앱은 설치 직전 재확인에서 버림
                          아니오
                          ▼
                     앱이 열리나? ─예─▶ 고친 다음 버전을 내보냄(앱 안 업데이트)
                          │
                          아니오 ─▶ Slack·OneDrive로 고친 설치 파일 손 설치(데이터 호환 유지)
```

#### 10절. 장기 궤적

- 기술 부채: CI의 자체 서명 단계와 지정 요구사항 검사(유료 서명으로 바꾸면 지운다), `latest-json.ts`.
- 경로 의존: 업데이터 공개 키는 한 번 퍼지면 회전이 번거롭다. 공개 저장소는 되돌릴 수 없다(공개된 파일은 미러될 수 있다).
- 되돌리기 쉬움: 3/5. 앱 코드는 쉽게 바꾸지만, 공개 저장소와 키는 한 방향 문이다.
- 1년 뒤 새 사람: 아키텍처 11장, install.md 관리자 절, 이 계획으로 충분하다.
- 다음 단계: X6·X7(측정에 묶음), 유료 서명(사용자가 원할 때 시크릿만), 단계적 배포(필요해지면 피드를 두 개로).
- 지적 없음.

#### 11절. 디자인과 UX

```
  [트레이 앱, 창 닫힘] ──(받기 끝)──▶ 트레이 메뉴에 "업데이트하고 다시 시작"
        │창 열기
        ▼
  [홈/첫 화면] 준비 줄 "새 버전 1.2.1을 받았습니다 · 다시 시작해 업데이트"
        │누름                     │녹음 시작
        ▼                         ▼
  처리 중? ─예─▶ 대화상자 ─취소─▶ 그대로      [녹음 화면] 준비 줄 없음, 설치 불가
        │아니오        │업데이트
        ▼              ▼
  [업데이트하는 중…] ─▶ 앱이 다시 뜸 ─▶ [홈] 결과 줄 "1.2.1로 업데이트했습니다 ×"
                              └실패─▶ [홈] "업데이트하지 못했습니다 · 릴리스 페이지 열기 ×"
  [설정 > 일반] 앱 버전 묶음: 버전 · 상태 줄 · 버튼(상태표 5.4) · 노트(준비 때)
```

| 기능 | 로딩 | 빈 | 오류 | 성공 | 부분 |
|---|---|---|---|---|---|
| 설정 앱 버전 | "확인하는 중…" | 확인 전: 상태 줄 없음 | 수동 실패만 오류 + 링크 | "최신 버전입니다" | "받는 중 42%" |
| 홈 준비 줄 | 없음 | ready 아님: 없음 | - | 준비 줄 | - |
| 결과 줄 | - | 업데이트 안 함: 없음 | "업데이트하지 못했습니다" | "업데이트했습니다" | - |

- 정보 순서: 홈에서 준비 줄 → 결과 줄 → 설정 줄. 설정은 버전이 먼저다.
- AI 슬롭 위험: 낮다. 기존 행·줄·버튼 패턴(6.4, 6.7)만 쓴다. 문구에 줄표(em dash)를 쓰지 않는다(메모리 규칙).
- 접근성: 상태 줄은 단계가 바뀔 때만 `role=status`(8.10과 같다), 받는 중 퍼센트는 알리지 않는다. 닫기(×)에 이름 "알림 닫기". 키보드로 모든 버튼에 닿는다.
- 반응형: 좁은 창(520px)에서 줄이 버튼 아래로 접히는지는 디자인 리뷰(2단계)에서 본다.
- 2단계 디자인 리뷰가 이어서 돈다(autoplan Phase 2).

### CEO 필수 산출물

**NOT in scope**

| 항목 | 처리 | 이유 |
|---|---|---|
| X6 종료할 때 자동 설치 | 미룸(TODOS) | 트레이 앱이라 종료가 드물다. 측정에서 80% 미만 2회면 앞당긴다 |
| X7 업데이트 준비 데스크톱 알림 | 미룸(TODOS) | 같은 측정 조건 |
| 끝난 자리(보낸 뒤) 준비 줄, Slack 꼬리말의 버전 | 미룸(감사 53) | 측정을 먼저 본다 |
| 1.1.2로 자체 서명만 먼저 | 건너뜀(TASTE 51) | 손 설치가 두 번으로 늘어난다 |
| 유료 OS 서명, D-U-N-S | 건너뜀(U2) | 사용자 결정 |
| 베타 채널, 단계적 배포, 강제 업데이트, 델타, 다운그레이드, 업데이트 끄기 | 건너뜀 | 이 규모에서 필요 없음 |
| X8 버전 옆 빌드 커밋 | 건너뜀 | 쓸 사람이 없다 |

**What already exists**: 0B 표와 같다. 재사용: `package_info().version`, `confirm_quit`·`RunEvent::Exit`의 정리(→ `shutdown_work`), `busy_ids`·`pause_busy`, 종료 대화상자 모양, `LogFolder` 행 모양, 6.4 설정 줄 자리, `gpu_reset_version` 패턴, reqwest 기능 구성, build.yml `release` 작업과 `docs/releases/`, slack-release 워크플로.

**Dream state delta**: 이 계획 뒤에는 첫 설치 한 번과 1.2.1 손 설치 한 번만 남고, 그 뒤 버그 수정은 다음 실행에 닿는다. Mac 권한은(0단계가 통과하면) 업데이트로 풀리지 않는다. 12개월 이상향과의 차이는 세 가지다. 첫 설치의 "그래도 열기"(유료 서명이 있어야 없어진다), 사용자가 눌러야 설치되는 점(X6·X7), 단계적 배포가 없는 점이다.

**Failure Modes Registry**

```
  CODEPATH              | FAILURE MODE                    | RESCUED? | TEST? | USER SEES?                        | LOGGED?
  ----------------------|---------------------------------|----------|-------|-----------------------------------|--------
  check                 | 오프라인·404·형식               | Y        | Y     | 자동: 없음 / 수동: 오류+링크       | Y
  download              | 무진행·서명 불일치               | Y        | Y     | 수동만 오류                        | Y
  install 가드          | 녹음 중·처리 중·ready 아님       | Y        | Y     | 막힌 버튼 / 대화상자               | Y
  install 재확인        | 철회된 버전                     | Y        | Y     | 준비 줄 사라짐                     | Y
  wait_children         | 10초 초과                       | Y        | Y     | 다음 시작 "업데이트하지 못했습니다" | Y
  macOS install         | rename·권한·App Management       | Y        | 수동  | 같음                               | Y
  Windows install       | 파일 잠금·백신(앱 밖)           | Y        | 수동  | 같음(다음 시작)                    | 부분(NSIS 로그 없음)
  TCC(자체 서명)        | 권한이 풀림                     | Y(5.8)   | 수동  | 대체 경로의 남는 안내              | -
  publish(CI)           | 토큰·중복·sig 누락              | Y        | Y(스크립트) | 동료: Slack 설치 파일        | Y(Actions)
```

CRITICAL GAP 없음(RESCUED=N이면서 TEST=N이고 조용한 행이 없다). Windows NSIS 실패는 앱 로그에 원인이 남지 않는다(다음 시작의 판정만). 4단계에서 NSIS 로그 위치를 확인해 install.md 관리자 절에 적는다(감사 63).

**Stale diagram audit**: 이 계획이 건드리는 문서의 ASCII 그림은 아키텍처의 상태 그림(회의 상태)과 디자인 시스템 6.4 홈 그림이다. 회의 상태 그림은 바뀌지 않는다. 6.4 홈 그림에는 준비 줄·결과 줄이 없으므로 디자인 시스템 갱신(9장)에서 같이 고친다.

**Reality rows (CEO)**

REALITY: premise-table finding 9개 전제 중 unverified=4(P1 구버전 사용자 수, P4 TCC 유지, P5 격리 속성, P6 MOTW). P4·P5는 0·2단계, P6은 4단계 끝나는 조건 docs/plans/2026-11-05-in-app-updater.md:190
REALITY: already-done pass 업데이터 관련 커밋 없음(git log origin/master --grep update, --since 2026-11-04 대상 파일) src-tauri/tauri.conf.json:1
REALITY: surface-check finding 빈 공개 저장소에는 태그를 만들 커밋이 없어 gh release create --repo가 실패한다(README 초기화 + --target main으로 수락). gh release create --draft·--latest·-R, gh release edit --draft=false, tauri signer generate는 --help로 확인 .github/workflows/build.yml:290
REALITY: binding-decisions finding eng review D2(자동 업데이트 파일럿 뒤로, 2026-10-14)를 사용자 요청으로 뒤집는다. U3로 기록 docs/TODOS.md:5
REALITY: numbers pass 릴리스 6회 10/20~11/04 [measured: git tag v1.0.0..v1.1.1, 427893d], dmg 18MB [measured: gh release view v1.1.1, 427893d], setup.exe 24MB [measured: gh release view v1.1.1, 427893d], $99·$9.99 [estimated: vendor docs] docs/plans/2026-11-05-in-app-updater.md:8
REALITY: deferred-asks pass 요청 3(서명·공증 조사와 해결)은 조사 3.2·3.3, 무료 해결(자체 서명, 무서명)로 처리. 유료 서명은 U2로 하지 않음. X6·X7은 TODOS docs/plans/2026-11-05-in-app-updater.md:16
REALITY: pr-count pass ship-policy 파일이 없어 기본(한 묶음에 PR 하나)이고, 이 저장소는 PR 없이 master에 바로 커밋한다(최근 커밋 d2828c9·427893d). 이 계획은 단계별 커밋으로 나눠도 사용자 대상 참조(install.md·README 링크)는 3단계 공개 저장소 생성 뒤 한 커밋에서 바꾼다 docs/install.md:7

#### Implementation Tasks (CEO)
Synthesized from this review's findings. Each task derives from a specific finding above. Run with Claude Code or Codex; checkbox as you ship.

- [ ] **T1 (P1, human: ~30m / CC: ~5m)** — 키 — 업데이터 키 쌍을 만들고 공개 키를 `tauri.conf.json`에, 개인 키·비밀번호를 두 곳에
  - Surfaced by: 스펙 리뷰 2 (업데이터 키를 1단계 시작 때)
  - Files: src-tauri/tauri.conf.json
  - Verify: `cargo build`가 플러그인 설정을 읽는다, 비밀번호 관리자에 항목이 있다
- [ ] **T2 (P1, human: ~3h / CC: ~30m + 사람)** — 실측 — 0단계 코드 없는 권한 실측(자체 서명 a→b 끌어다 덮기)
  - Surfaced by: CEO 두 목소리 Claude 2
  - Files: (로컬 빌드만)
  - Verify: 덮어 설치 뒤 두 트랙이 0이 아니고 키체인 창이 없다
- [ ] **T3 (P1, human: ~2d / CC: ~1.5h)** — Rust — `update.rs` 상태·명령·타이머·받기 감시·재확인·`shutdown_work`·`workers_running`·`updated_to`/`just_updated`·시작 로그 버전
  - Surfaced by: 5.1, 5.2, 스펙 리뷰 1~3, 두 목소리(녹음 금지, 대기 초과 중단, 재확인)
  - Files: src-tauri/src/update.rs, src-tauri/src/lib.rs, src-tauri/src/commands.rs, src-tauri/src/queue.rs, src-tauri/src/settings.rs, src-tauri/Cargo.toml
  - Verify: `cargo test`(7장 Rust 항목), `cargo clippy --all-targets -- -D warnings`
- [ ] **T4 (P1, human: ~3h / CC: ~15m)** — 트레이 — 준비 때만 "업데이트하고 다시 시작"(`set_menu`, 순수 함수, `app://update-install-requested`), 녹음 중 숨김
  - Surfaced by: X4, 스펙 리뷰 1
  - Files: src-tauri/src/lib.rs
  - Verify: 순수 함수 테스트
- [ ] **T5 (P1, human: ~1.5d / CC: ~1h)** — 화면 — 설정 앱 버전 묶음, 홈·첫 화면 준비 줄과 결과 줄, 대화상자, ipc, 미리보기 장면, 테스트
  - Surfaced by: 5.4, X1~X3, 스펙 리뷰 2·3
  - Files: src/screens/Settings.tsx, src/screens/Home.tsx, src/App.tsx, src/lib/ipc.ts, src/preview/fixtures.ts, src/screens/Settings.test.tsx, src/screens/Home.test.tsx, src/App.test.tsx
  - Verify: `bun run test`, preview.html의 상태별 장면
- [ ] **T6 (P1, human: ~1d / CC: ~45m)** — CI — `tauri.updater.conf.json`, 자체 서명 키체인 단계와 지정 요구사항 검사, 업데이터 서명 환경 변수, 아티팩트, `publish` 작업(GitHub App), `slack` 조건, `latest-json.ts`와 테스트
  - Surfaced by: 5.5, 스펙 리뷰 1·2, 두 목소리(Slack 독립, GitHub App)
  - Files: .github/workflows/build.yml, .github/workflows/slack-release.yml, src-tauri/tauri.updater.conf.json, scripts/latest-json.ts, scripts/latest-json.test.ts
  - Verify: `bun run test`(스크립트 4경우), 수동 실행 빌드 통과
- [ ] **T7 (P1, human: ~2h / CC: ~15m + 사람)** — 실측 — 2단계 업데이터 실측(이 Mac, Windows PC가 있으면 Windows도)
  - Surfaced by: 6장 2단계, 스펙 리뷰 3
  - Files: (로컬 빌드, 시험 config)
  - Verify: 6장 2단계 끝나는 조건 전부
- [ ] **T8 (P1, human: ~1h)** — 배포 준비(사용자) — 공개 저장소(README), GitHub App, 시크릿, `SLACK_AUTO=false`, 두 번째 보관자
  - Surfaced by: 6장 3단계, 두 목소리 Claude 10
  - Files: (GitHub 설정)
  - Verify: 수동 실행 빌드가 공개 저장소에 쓰지 않는다
- [ ] **T9 (P1, human: ~2h)** — 릴리스 — 4단계 조용한 시작(1.2.0 시험 기기 → 실제 피드로 1.2.1), 통과 뒤 알림
  - Surfaced by: 두 목소리 Claude 1·8, Codex 3
  - Files: docs/releases/v1.2.0.md, docs/releases/v1.2.1.md
  - Verify: 6장 4단계 끝나는 조건
- [ ] **T10 (P2, human: ~4h / CC: ~20m)** — 문서 — install.md(1.2.1 손 설치, 업데이트, 관리자 절의 철회 절차·NSIS 로그), 아키텍처 8.5·11, 요구사항 APP-05, TODOS(X6·X7, Windows 서명 조사, 자동 업데이트 항목 삭제), 설계 문서(OQ2 닫기, 전제 5 대체), 디자인 시스템 6.4·6.7, README
  - Surfaced by: 9장, U3, 두 목소리 Claude 6·9
  - Files: docs/install.md, docs/architecture.md, docs/requirements.md, docs/TODOS.md, docs/designs/meeting-summarizer.md, docs/design-system.md, README.md
  - Verify: 문서 안 링크와 절 번호가 맞는다, 줄표(em dash) 없음

#### Completion Summary (CEO)

```
  +====================================================================+
  |            MEGA PLAN REVIEW — COMPLETION SUMMARY                   |
  +====================================================================+
  | Mode selected        | SELECTIVE EXPANSION                         |
  | System Audit         | 업데이터 커밋 없음, D2 뒤집기, Windows 실기기 미검증 |
  | Step 0               | 전제 9(unverified 4), 확장 8(수락 5·미룸 2·건너뜀 1) |
  | Section 1  (Arch)    | 0 issues found (앞 단계에서 반영)            |
  | Section 2  (Errors)  | 23 error paths mapped, 1 GAPS (수락: 감사 59) |
  | Section 3  (Security)| 0 issues found, 0 High severity             |
  | Section 4  (Data/UX) | 7 edge cases mapped, 0 unhandled (+순서 시험 60) |
  | Section 5  (Quality) | 1 issues found (감사 61)                     |
  | Section 6  (Tests)   | Diagram produced, 1 gaps (감사 62)           |
  | Section 7  (Perf)    | 0 issues found                              |
  | Section 8  (Observ)  | 1 gaps found (감사 63)                       |
  | Section 9  (Deploy)  | 0 risks flagged (조용한 시작 수락)           |
  | Section 10 (Future)  | Reversibility: 3/5, debt items: 2           |
  | Section 11 (Design)  | 0 issues (디자인 리뷰로 넘김)                |
  +--------------------------------------------------------------------+
  | NOT in scope         | written (7 items)                           |
  | What already exists  | written                                     |
  | Dream state delta    | written                                     |
  | Error/rescue registry| 11 rows, 0 CRITICAL GAPS                    |
  | Failure modes        | 9 total, 0 CRITICAL GAPS                    |
  | TODOS.md updates     | 2 items proposed (X6, X7, 문서 단계에서 씀)  |
  | Scope proposals      | 8 proposed, 5 accepted (EXP + SEL)          |
  | CEO plan             | written                                     |
  | Outside voice        | codex + completed                           |
  | Lake Score           | N/A (점수 붙은 질문 없음)                     |
  | Diagrams produced    | 7 (아키텍처, 데이터 흐름, 상태, 오류, 배포, 되돌리기, 사용자 흐름) |
  | Stale diagrams found | 1 (디자인 시스템 6.4 홈 그림)                |
  | Unresolved decisions | 0 (TASTE 9개는 최종 관문)                    |
  | Reality rows         | 7/7 rows (PLAN_REALITY complete), 4 unverified |
  +====================================================================+
```

Approval readiness: PASS. 확인한 행: U1(사용자 D1), U2(사용자 D2·D3), C1·C2·X1~X8·R1(autoplan 자동 결정, 감사 1~12), 스펙 반영(감사 13~42), 0H(감사 43), 두 목소리(감사 44~58), 1~11절(감사 59~63). TASTE 행(6, 24, 28, 42, 51, 52, 53, 55)은 자동 결정의 기본값으로 적용했고 최종 관문에서 다시 묻는다.

### 디자인 리뷰 (Phase 2, 2026-11-05)

디자인 1차 기준 편집 기록(관문 d1 뒤 디자인 재실행 전에 보관, 원래 source SHA 그대로):

```text
<!-- autoplan-baseline-edits:design {"sourceSha256":"f8c1b1194255c6f8264b00cd52152703da21ccb8616d6b9a0711577e402161a1","replacements":[{"oldText":"자동 확인의 실패는 로그만 남기고 화면에 띄우지 않는다. 사용자가 누른 확인의 실패만 보인다.","newText":"자동 확인의 실패는 로그만 남기고 홈에 띄우지 않는다. 사용자가 누른 확인의 실패만 오류 줄로 보인다. 마지막으로 성공한 확인 시각(`last_check_ok`)을 `updated_to`와 같은 곳에 적고, 7일을 넘으면 설정 버전 행에 \"7일 넘게 업데이트를 확인하지 못했습니다\"를 보인다(5.4 표)."},{"oldText":"- 설정 > 일반, 로그 묶음 위에 \"앱 버전\" 묶음(디자인 시스템 6.7의 행). 왼쪽 \"버전 1.1.1\"과 상태 한 줄, 오른쪽 작은 outline 버튼 하나.\n\n| 상태 | 상태 줄 | 버튼 |\n|---|---|---|\n| idle, up_to_date | \"최신 버전입니다\"(확인한 적이 있을 때만) | \"업데이트 확인\" |\n| checking | \"확인하는 중…\" | 막힘 |\n| downloading | \"새 버전 1.2.1을 받는 중 42%\" | 없음 |\n| ready | \"새 버전 1.2.1을 받았습니다\" | \"다시 시작해 업데이트\"(brand) |\n| installing | \"업데이트하는 중…\" | 막힘 |\n| failed(수동 확인) | 오류 한 줄과 \"릴리스 페이지 열기\" 링크 | \"다시 시도\" |","newText":"- 설정 > 일반 맨 아래 \"앱 정보\" 묶음(디자인 시스템 6.7의 행) 하나에 두 행: 위 \"버전\", 아래 지금의 로그 폴더 행(`LogFolder`, `Settings.tsx:1431-1456`). 이 묶음은 `fieldset disabled={locked}` 밖에 둔다(LogFolder처럼). 읽기 전용 설정(새 버전 앱이 쓴 설정)이어도 확인과 설치가 된다. 설정을 읽지 못한 화면(`Settings.tsx`의 오류 반환)에도 버전 행을 보인다. 일반 탭 설명(`TABS`)은 \"회의록에 남는 이름, 알림, 로그와 앱 버전입니다.\"로 바꾼다.\n- 버전 행: 왼쪽 첫 줄 \"버전 1.2.1\"(숫자 `tabular-nums`), 둘째 줄 상태 줄(`text-label`, `ink-2`). 오른쪽 버튼 하나: 평소 outline sm, `ready`일 때만 brand sm. 버튼은 상태가 바뀌어도 그 자리에 남고(못 누를 때는 `aria-disabled`, `Settings.tsx:1504` 방식) 초점을 잃지 않는다. 창 너비가 `sm` 미만이면 버튼이 글 아래로 내려간다.\n- 버전 숫자 뒤에는 조사를 바로 붙이지 않고 \"버전\"을 끼운다(\"1.2.2 버전을\", \"1.2.2 버전으로\"). `josa()`는 숫자를 읽지 못한다.\n\n| 상태 | 상태 줄 | 버튼 |\n|---|---|---|\n| 개발 빌드(`tauri dev`) | \"개발 빌드는 업데이트를 확인하지 않습니다\" | 없음 |\n| idle(확인한 적 없음) | 없음 | \"업데이트 확인\" |\n| up_to_date | 수동 확인 직후 3초 \"방금 확인했습니다\", 그 뒤 \"최신 버전입니다\" | \"업데이트 확인\" |\n| 마지막 성공 확인이 7일 넘음(자동 실패 포함) | \"7일 넘게 업데이트를 확인하지 못했습니다\" | \"업데이트 확인\" |\n| checking | \"확인하는 중…\" | \"업데이트 확인\"(막힘) |\n| downloading | \"1.2.2 버전을 받는 중 42%\". 전체 크기를 모르면 \"1.2.2 버전을 받는 중 12MB\". 15초 넘게 새 조각이 없으면 \"연결을 기다리는 중…\" | \"업데이트 확인\"(막힘) |\n| ready | \"1.2.2 버전을 받았습니다\" | \"다시 시작해 업데이트\"(brand) |\n| ready, 녹음 중 | \"녹음이 끝나면 업데이트할 수 있습니다\" | \"다시 시작해 업데이트\"(막힘) |\n| ready, 가져오기나 내보내기 중 | \"파일 작업이 끝나면 업데이트할 수 있습니다\" | \"다시 시작해 업데이트\"(막힘) |\n| ready, 저장하지 않은 설정 있음 | \"저장하지 않은 설정이 있습니다. 저장하거나 되돌린 뒤 업데이트하세요\" | \"다시 시작해 업데이트\"(막힘) |\n| installing | \"업데이트를 확인하는 중…\" → \"작업을 마무리하는 중…\" → \"업데이트하는 중…\"(Windows는 \"업데이트하는 중… 창이 닫혔다가 다시 열립니다\") | \"다시 시작해 업데이트\"(막힘) |\n| 재확인에서 철회됨 | \"이 업데이트는 취소되었습니다\" 뒤 up_to_date | \"업데이트 확인\" |\n| 재확인에서 더 새 버전 | \"더 새 버전이 나와 다시 받습니다\" 뒤 downloading | \"업데이트 확인\"(막힘) |\n| failed(수동) | 오류 분류 문구(5.4 오류 문구) | \"다시 시도\", 옆에 \"릴리스 페이지 열기\" 링크 |\n| 그 버전 설치 실패 기록 있음 | \"1.2.2 버전을 앱에서 설치하지 못했습니다. 릴리스 페이지에서 받아 설치하세요\" | \"업데이트 확인\", 옆에 \"릴리스 페이지 열기\" 링크 |\n| 상태를 불러오지 못함(`update_state` 실패) | \"업데이트 상태를 불러오지 못했습니다\" | \"다시 시도\" |"},{"oldText":"- `ready`면 설정 묶음에 릴리스 노트(`latest.json`의 `notes`)를 접어 보인다. 마크다운을 해석하지 않고 글자 그대로 줄바꿈만 살린다(NFR-07, React 글자 노드).","newText":"- `ready`면 버전 행 아래에 \"바뀐 것 보기\" 접기 단추(`aria-expanded`, 키보드로 열고 닫음)를 둔다. 펼치면 노트를 줄마다 목록으로 보인다(최대 12줄 높이, 넘치면 안에서 스크롤, 긴 줄과 주소는 줄바꿈). 노트는 `scripts/latest-json.ts`가 릴리스 노트의 `## 바뀐 것` 절만 뽑아 마크다운 기호(목록 기호, 강조, 코드 따옴표, 링크 괄호)를 지운 평문 줄로 넣는다. 화면은 해석하지 않고 React 글자 노드로 줄만 나눈다(NFR-07). 노트가 비면 접기 단추를 두지 않는다."},{"oldText":"- `ready`면 홈(6.4)과 첫 화면(6.5)의 설정 줄 자리, 설정 줄 위에 한 줄(같은 모양): \"새 버전 1.2.1을 받았습니다\"와 \"다시 시작해 업데이트\". 녹음 화면에는 보이지 않는다. 줄 순서는 위에서부터 준비 줄, 업데이트 결과 줄, 설정 줄이다.","newText":"- 홈(6.4)과 첫 화면(6.5)의 설정 줄 자리에는 업데이트 줄이 많아야 하나다(설정 줄과 같은 모양: `text-label`, `ink-2` 글, 끝에 link xs 버튼). `ready`면 \"1.2.2 버전을 받았습니다 · 다시 시작해 업데이트\"이고 닫기가 없다. 업데이트 결과 줄(아래)은 `ready`가 아닐 때만 보인다. 녹음 화면에는 보이지 않는다. 줄 순서는 위에서부터 오디오 확인 줄(5.7), 설정 줄, 업데이트 줄이다(녹음 준비가 먼저, 업데이트는 미뤄도 되는 일). 줄이 길면 글이 줄바꿈되고 버튼은 글 끝에 붙는다. 첫 화면은 줄이 넘치면 세로 스크롤을 허용한다."},{"oldText":"트레이 메뉴에 \"업데이트하고 다시 시작\"을 더한다.","newText":"트레이 메뉴 맨 위에 \"다시 시작해 업데이트\"와 구분선을 더한다(순서: 다시 시작해 업데이트, 구분선, 열기, 종료. 설정·홈과 같은 문구)."},{"oldText":"화면은 `ok`면 \"1.2.1로 업데이트했습니다\", 아니면 \"업데이트하지 못했습니다\"와 \"릴리스 페이지 열기\"를 홈(6.4)과 첫 화면(6.5)의 설정 줄 자리, 설정 줄 위에 한 줄로 보인다. 닫기(×)를 누르거나 앱을 다시 시작하면 사라진다. `ready` 줄과 함께 있으면 `ready` 줄이 위다.","newText":"화면은 `ok`면 \"1.2.2 버전으로 업데이트했습니다 · 바뀐 것\"(그 버전의 공개 Release 페이지), 아니면 \"업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다 · 릴리스 페이지 열기\"를 홈과 첫 화면의 업데이트 줄로 보인다. 닫기(×, 32×32, `aria-label=\"닫기\"`)를 누르거나 앱을 다시 시작하면 사라진다. 실패는 설정 버전 행에 남는다(설치 실패 기록, 5.4 표)."},{"oldText":"- 진행 중인 처리(전사·요약)가 있으면 종료 확인과 같은 대화상자: 제목 \"진행 중인 작업이 있습니다\", 설명 \"지금 업데이트하면 진행 중인 처리를 멈추고, 다시 시작한 뒤 이어서 처리합니다\", 버튼 \"취소\"·\"업데이트\". 녹음 중에는 대화상자 대신 버튼이 막힌다(5.2).","newText":"- 진행 중인 처리(전사·요약)가 있으면 종료 확인과 같은 대화상자: 제목 \"진행 중인 작업이 있습니다\", 설명 \"지금 업데이트하면 진행 중인 전사와 요약을 멈추고, 다시 시작한 뒤 이어서 처리합니다\", 버튼 \"취소\"·\"업데이트\". 녹음, 가져오기, 내보내기 중에는 대화상자 대신 버튼이 막힌다(5.2, 5.4 표). 대화상자는 열릴 때 \"취소\"에 초점을 두고, 닫히면 연 버튼으로(트레이에서 열었으면 창 본문으로) 초점을 돌린다."},{"oldText":"Mac은 서명이 ad-hoc에서 자체 서명으로 바뀌므로 1.2.0에서 마이크·시스템 오디오 권한과 키체인 \"항상 허용\"을 한 번 더 받는다. 릴리스 노트와 install.md에 쓴다.","newText":"Mac은 서명이 ad-hoc에서 자체 서명으로 바뀌므로 1.2.x 첫 실행에서 마이크·시스템 오디오 권한과 키체인 \"항상 허용\"을 한 번 더 받는다. 마이크는 OS가 다시 묻지만 시스템 오디오는 오류 없이 조용히 0이 된다. 그래서 앱이 안내한다: 이전 설치의 흔적(설정 파일이나 회의 폴더)이 있는데 `last_run_version`이 없거나 1.2.0 미만이면(1.1.1은 이 값을 쓰지 않는다) macOS에서 `audio_check_needed`를 켠다. 켜져 있으면 홈과 첫 화면 맨 위에 \"Mac 권한이 바뀌어 오디오를 다시 확인해야 합니다 · 오디오 확인\"(설정 > 오디오로 이동) 줄을 보이고, 설정 > 오디오의 시스템 오디오 테스트가 통과하면 끈다. 녹음은 막지 않는다(곧 시작할 회의가 있을 수 있다). 시작할 때마다 `last_run_version`을 지금 버전으로 적는다. 릴리스 노트와 install.md에도 쓴다."}]} -->
```

<!-- autoplan-accepted:design -->
- (디자인, CEO 줄 대체) 설정의 버전은 일반 탭 맨 아래 "앱 정보" 묶음의 "버전" 행이다. 로그 폴더 행과 한 묶음이고, `fieldset disabled={locked}` 밖에 두며, 설정을 읽지 못한 화면에도 보인다. 일반 탭 설명은 "회의록에 남는 이름, 알림, 로그와 앱 버전입니다."
- 버전 행: "버전 1.2.1"(`tabular-nums`)과 상태 줄(`text-label`, `ink-2`), 오른쪽 버튼 하나(평소 outline sm, `ready`만 brand sm). 버튼은 모든 상태에서 같은 자리에 남고 못 누를 때는 `aria-disabled`로 막아 초점을 지킨다. `sm` 미만 너비에서는 버튼이 글 아래로 내려간다.
- 상태 줄과 버튼은 5.4 표를 따른다: 개발 빌드, 확인한 적 없음, 방금 확인(3초), 최신, 7일 넘게 확인 못 함, 확인 중, 받는 중(퍼센트, 크기를 모르면 MB, 15초 무진행이면 "연결을 기다리는 중…"), 준비, 준비+녹음 중, 준비+가져오기·내보내기 중, 준비+저장하지 않은 설정, 설치 단계 3개(Windows는 창이 닫혔다 열린다는 말 포함), 재확인 철회, 재확인 더 새 버전, 수동 실패, 설치 실패 기록, 상태 불러오기 실패.
- 버전 숫자 뒤에는 "버전"을 끼워 조사를 붙인다("1.2.2 버전을", "1.2.2 버전으로"). 모든 업데이트 문구에 적용한다.
- (디자인, CEO 줄 대체) 버튼 문구는 설정, 홈, 첫 화면, 트레이 모두 "다시 시작해 업데이트" 하나다. 트레이는 `ready`일 때 맨 위에 그 항목과 구분선(순서: 다시 시작해 업데이트, 구분선, 열기, 종료).
- (디자인, CEO 줄 대체) 홈과 첫 화면에는 업데이트 줄이 많아야 하나다. 설정 줄과 같은 모양(`text-label`, `ink-2`, 끝에 link xs 버튼)이다. `ready` 줄은 닫기가 없고, 결과 줄은 `ready`가 아닐 때만 보인다. 순서는 오디오 확인 줄, 설정 줄, 업데이트 줄이다. 긴 줄은 줄바꿈하고 버튼은 글 끝에 붙는다. 첫 화면은 줄이 넘치면 세로 스크롤을 허용한다. 녹음 화면에는 보이지 않는다.
- 결과 줄: 성공 "1.2.2 버전으로 업데이트했습니다 · 바뀐 것"(그 버전의 공개 Release 페이지), 실패 "업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다 · 릴리스 페이지 열기". 닫기(×)는 32×32, `aria-label="닫기"`. 실패는 설정 버전 행에 설치 실패 기록으로 남는다.
- 노트: `scripts/latest-json.ts`가 릴리스 노트의 `## 바뀐 것` 절만 뽑아 마크다운 기호를 지운 평문 줄로 `notes`에 넣는다(절이 없으면 빈 문자열). 화면은 `ready`일 때 "바뀐 것 보기" 접기 단추(`aria-expanded`, 키보드로 열고 닫음)를 두고, 펼치면 줄마다 목록, 최대 12줄 높이에 안에서 스크롤, 긴 줄과 주소는 줄바꿈. 노트가 비면 단추를 두지 않는다.
- 설치 준비: "다시 시작해 업데이트"를 누르면 곧바로 앱 전체가 `installing`이 된다. 창 본문에 `inert`를 걸고, 단계 문구("업데이트를 확인하는 중…", "작업을 마무리하는 중…", "업데이트하는 중…")를 버전 행·업데이트 줄과 앱 알림 띠(`App.tsx`의 `banners` 자리)에 보인다. 어느 화면에서 눌렀든 보인다. 재확인이 철회나 더 새 버전으로 끝나 정리 전에 돌아오면 `inert`를 풀고 5.4 표의 해당 문구를 보인다.
- Rust는 `installing`에서 녹음 시작, 가져오기, 두 번째 `install_update`를 거절한다. 녹음 시작과 설치 판정은 같은 잠금 안에서 한다(녹음이 막 시작되는 중이면 설치가 `Err("recording")`). 가져오기·내보내기 중이면 `install_update`는 `Err("file_work")`이고 화면은 대화상자 대신 버튼을 막는다.
- 저장하지 않은 작업: 설치 전에 화면은 모든 회의의 편집 대기분(`Meeting.tsx`의 `saves`)을 바로 저장하고 끝까지 기다린다. 저장이 실패하면 설치를 시작하지 않고 "회의 문서에서 고친 내용을 저장하지 못해 업데이트를 멈췄습니다"를 보인다. 설정의 저장하지 않은 변경은 자동으로 저장하거나 버리지 않고 설치 버튼을 막는다(5.4 표 문구). 트레이와 홈 경로도 같은 판정을 거친다.
- 오류 문구: 원문 오류는 `app.log`에만 남기고 화면은 분류별 문구를 보인다. 연결 실패·시간 초과·60초 무진행은 "업데이트 서버에 연결하지 못했습니다. 인터넷 연결을 확인하고 다시 시도하세요". 서명 불일치·손상은 "받은 업데이트 파일이 올바르지 않아 버렸습니다. 다시 시도하세요". 피드 형식 오류나 이 플랫폼 항목 없음은 "지금은 이 PC용 업데이트를 받을 수 없습니다. 릴리스 페이지에서 받으세요". 그 밖은 "업데이트하지 못했습니다. 릴리스 페이지에서 받으세요". 분류는 순수 함수로 둔다.
- 설치 실패 기록: 설치 실패(교체 실패, 설치 프로그램 실행 실패, 10초 대기 초과, 다음 시작에 `just_updated.ok`가 거짓)가 나면 `install_failed: {version, count}`를 `updated_to`와 같은 곳에 적는다. 그 버전은 다시 받지 않고 버전 행에 "앱에서 설치하지 못했습니다" 행과 릴리스 페이지 링크를 보인다. 더 새 버전을 찾으면 기록을 지우고 평소대로 받는다.
- 마지막 성공 확인 시각(`last_check_ok`)을 같은 곳에 적는다. 7일을 넘으면 설정 버전 행에만 "7일 넘게 업데이트를 확인하지 못했습니다". 자동 확인 실패는 홈에 띄우지 않는다.
- 녹음 중에는 자동 확인을 건너뛴다(다음 주기에 다시 본다). 개발 빌드는 확인하지 않는다.
- 오디오 확인(5.7): 이전 설치의 흔적(설정 파일이나 회의 폴더)이 있는데 `last_run_version`이 없거나 1.2.0 미만이면 macOS에서 `audio_check_needed`를 켠다. 켜져 있으면 홈과 첫 화면 맨 위에 "Mac 권한이 바뀌어 오디오를 다시 확인해야 합니다 · 오디오 확인"(설정 > 오디오로 이동). 설정 > 오디오의 시스템 오디오 테스트가 통과하면 끈다. 녹음은 막지 않는다. 시작할 때마다 `last_run_version`을 지금 버전으로 적는다. 0단계 실측이 ad-hoc 대체로 끝나면 macOS 업데이트 뒤마다 같은 표시를 켠다(5.4 마지막 줄의 안내와 5.8 첫 줄의 "홈과 녹음 시작에서 안내"를 이 장치로 한다).
- 접근성: 상태 줄은 단계가 바뀔 때만 sr-only `role=status`로 알린다(퍼센트마다 알리지 않는다, `Home.tsx:455` 방식). 색과 초점 고리는 기존 토큰을 쓴다. 대화상자는 "취소"에 첫 초점, 닫히면 연 버튼으로(트레이에서 열었으면 창 본문으로) 초점을 돌린다.
- 공개 저장소 README에 동료용 설치 안내를 둔다(첫 설치 "그래도 열기", Windows SmartScreen "추가 정보 → 실행", 업데이트 뒤 오디오 확인). 동료는 비공개 install.md를 열 수 없다. 공개 Release 본문은 지금 노트처럼 "## 설치 파일" 표가 맨 위다.
- 디자인 시스템 6.7에 "앱 정보" 묶음과 버전 행, 6.4·6.5에 업데이트 줄과 오디오 확인 줄의 순서를 적는다.
- 미리보기(`preview.html`)와 fixture: 5.4 표의 모든 행, 홈에 오디오 확인 줄+설정 줄+업데이트 줄이 함께 있는 장면, 결과 줄 성공·실패, 노트 펼침(긴 노트), 설치 중 알림 띠, 처리 중 대화상자. 각 장면을 520×600과 1040×720에서 본다.
- 화면 테스트: 5.4 표의 행별 문구와 버튼(막힘 이유별), 상태가 바뀌어도 버튼 초점 유지, 홈 줄 순서와 업데이트 줄 하나만(`ready`가 결과 줄을 가림), 노트가 비면 단추 없음, 저장하지 않은 설정이면 설치 막힘, 회의 편집 저장 실패면 설치 안 함, `installing`에서 `inert`와 알림 띠, 트레이 경로가 같은 판정을 거침.
- Rust 테스트: `installing`에서 녹음 시작·가져오기·두 번째 설치 거절, 가져오기·내보내기 중 `Err("file_work")`, 설치 실패 기록이 있으면 그 버전을 받지 않고 더 새 버전이 기록을 지움, `audio_check_needed` 판정(흔적 있고 `last_run_version` 없음 → 켬, 1.1.1 → 켬, 1.2.0 → 끔, 흔적 없음 → 끔, Windows → 끔), 개발 빌드는 확인 안 함, 녹음 중 자동 확인 건너뜀, 오류 분류 함수(원문 → 4가지 문구), 7일 판정.
- `scripts/latest-json.ts` 테스트: `## 바뀐 것` 절만 뽑음, 마크다운 기호 제거, 절이 없으면 빈 노트.
- (디자인 재리뷰) 카드 위치: 창 너비 760px 이상이면 `fixed left-6 bottom-6`, 폭 320px(`max-w-80`), 아래 스택과 같은 z-20(전사본 패널 z-30, 대화상자 z-50 아래). 760px 미만이거나 세로 창이면 아래 스택(`App.tsx:528`의 독·보내기 막대·저장 막대) 바로 위에 왼쪽 정렬로 올린다. 카드가 보이는 동안 화면 아래 여백(`pb-24`), 회의 문서의 캐럿 보호, 좁은 창 전사본 패널(`MeetingParts.tsx:641`)의 아래 경계를 카드 높이만큼 늘린다. 카드는 저장 막대, 보내기 막대, 독, 전사본 패널, 설정 버전 행을 가리지 않는다.
- (디자인 재리뷰) 카드 모양: `surface` 바탕, `rounded-float`, `shadow-float`, 안쪽 여백 16px. 제목 `body` 500 `ink`, 보조 글 `label` `ink-2`. 버튼 outline sm, 링크 link xs, 닫기 × 32×32 `aria-label="업데이트 알림 닫기"`. 경고색과 반복 애니메이션은 쓰지 않는다. 나타남·사라짐은 `dur.small`, 움직임 줄이기 설정에서는 불투명도만.
- (디자인 재리뷰) 카드 접근성: 이름 있는 비모달 영역(`role="region" aria-label="앱 업데이트"`), 나타날 때 자동 초점 없음. 처음 나타날 때와 단계가 바뀔 때만 sr-only `role=status`로 한 번 알리고, 설정 버전 행과 겹쳐 읽지 않는다. DOM 순서는 화면 본문 뒤, 아래 스택 앞. ×로 닫으면 초점을 화면 본문(screenBox)으로 돌린다.
- (디자인 재리뷰) 닫기 수명: "이번 실행"은 앱 프로세스 수명이다(창 숨김, 다시 열기, 화면 이동으로는 다시 보이지 않는다). 닫기는 (종류, 버전)으로 기억한다: 준비, 결과, 14일 알림은 따로이고 다른 버전이 준비되면 다시 보인다. 사용자가 시작한 설치 흐름(카드, 설정, 트레이)의 막힘 이유, 준비·설치 단계, 재확인 결과, 저장 실패, 사전 점검 실패는 닫았어도 카드에 다시 보인다. 설치 중에는 ×가 없다.
- (디자인 재리뷰) (디자인·Eng 줄 대체) 설치 중 피드백: 단계 문구는 카드 하나에 보인다(설정 화면이면 버전 행에도). 앱 알림 띠에는 보이지 않는다. 카드는 `inert` 범위 밖이고, 준비가 시작되면 초점을 카드 상태 글(`tabIndex=-1`)로 옮긴다. `inert`는 화면 본문, 헤더, 독, 아래 스택, `document.body`에 붙는 전사본 포털에 걸고, 전역 단축키(`App.tsx:355` 녹음 단축키 등)는 준비·설치 중 무시한다. `needs_confirm`이면 잠금을 풀고 대화상자의 "취소"로, 실패·철회·막힘이면 눌렀던 버튼(없으면 카드 버튼)으로 초점을 돌린다. 문구: 준비 "업데이트를 준비하는 중…", 재확인 "마지막으로 확인하는 중…", 마무리 "작업을 마무리하는 중…", 설치 "업데이트하는 중…".
- (디자인 재리뷰) 재확인 결과: 다른 버전이 나오면 카드는 사라지지 않고 "다른 버전을 받는 중 42%"를 보이다가 준비 카드로 돌아간다(다시 누르는 것은 사용자). 철회 문구 "이 업데이트는 취소되었습니다"는 ×를 누르거나 다음 상태가 될 때까지 남는다.
- (디자인 재리뷰) 상태 표: 5.4 표를 한 표로 다시 쓴다. 열은 상태, 설정 상태 줄, 설정 버튼, 카드 글, 카드 버튼, 트레이, 닫기 가능. 빠진 행을 더한다: Slack 보내기 중 막힘, macOS 위치 사전 점검 실패(카드 버튼은 "릴리스 페이지 열기"), 설치 실패 재시도(`count < 2`면 "다시 시도", 두 번째 실패부터 링크만), 준비 단계, 회의 편집 저장 실패, 14일 확인 실패. "더 새 버전이 나와"는 "다른 버전이 나와"로. 업데이트 줄 시대의 문장은 10장 접기(DX)에서 지운다.
- (디자인 재리뷰) 확인 대화상자의 확인 버튼은 "다시 시작해 업데이트"다(앱이 다시 시작된다는 것을 다시 알린다).
- (디자인 재리뷰) Windows 설치 창: `bundle.windows.nsis.languages: ["Korean"]`(첫 설치에도). 설치 단계 문구는 Windows "설치 창이 잠깐 보인 뒤 앱이 다시 열립니다", macOS "앱이 잠시 닫혔다가 다시 열립니다".
- (디자인 재리뷰) (디자인 줄 대체) 결과 카드의 "바뀐 것": 설치 직전 `update-state.json`에 `notes`를 함께 적고, 결과 카드 안에서 같은 노트 목록 컴포넌트로 펼친다(최대 높이, 안에서 스크롤). 공개 Release 페이지(맨 위가 설치 파일 표)로 보내지 않는다. 노트가 비면 링크가 없다.
- (디자인 재리뷰) 디자인 시스템에 "업데이트 카드" 절을 새로 두고, 6.2의 "앱에서 유일하게 살아 있는 요소" 문장을 고친다(카드가 두 번째 떠 있는 요소). 9장 문서 변경에 더한다.
- (디자인 재리뷰) 미리보기: 520×880 회의 문서(보내기 막대와 카드), 전사본 패널을 연 상태와 카드, 설정 다른 탭의 저장 막대와 카드, 520×600 첫 화면과 카드, 1040×720 넓은 창 왼쪽 아래. 카드 상태별 장면: 준비, 막힘 이유별, 준비 단계, 설치 단계, 다른 버전 받는 중, 철회, 성공, 실패, 14일 알림, 닫은 뒤. 녹음 중과 설정 일반 탭에서 숨김.
- (디자인 재리뷰) 화면 테스트: 닫기 키(종류, 버전), 사용자가 시작한 흐름은 닫힘을 무시, 설치 중 × 없음, 초점 이동(준비 시작, 대화상자 취소, 실패), 준비·설치 중 전역 단축키 무시, 녹음 중과 일반 탭에서 숨김, 좁은 창에서 카드가 아래 스택 위로 올라가고 여백이 늘어남, 14일 판정.
<!-- /autoplan-accepted:design -->

#### Step 0

- 디자인 완성도 6/10. 10이면: 기존 부품(6.7 행, 설정 줄의 link xs 버튼, 종료 대화상자)에 1:1로 대응, 상태마다 정확한 한국어 문구, 노트 표시 방식, 설치 중 화면, 좁은 창, 초점·알림 규칙이 모두 정해져 있다. 디자인 기준: `docs/design-system.md`(DESIGN.md 대신). 목업은 만들지 않는다(사용자 메모리: 앱에 바로 구현하고 preview.html로 보인다).
- 기존 패턴: 홈 설정 줄 `Home.tsx:436-470`(`text-label`, `ink-2` 머리말, link xs 버튼, 단계 바뀔 때만 sr-only `role=status`), 설정 `LogFolder`(`Settings.tsx:1431-1456`), 종료 대화상자(`App.tsx:581-595`), 앱 알림 띠(`App.tsx:400`의 `banners`), 회의 편집 자동 저장(`Meeting.tsx:169-178`의 `saves`, `flush`).

#### 두 목소리 (Step 0.5)

- Claude 독립 디자인 리뷰어: 완료(INPUT `design 903bdcc6…` 일치). 높음 8, 중간 13.
  - H1 버전 묶음이 `fieldset disabled={locked}` 안에 들어가면 읽기 전용(새 버전 설정)일 때 막힌다. LogFolder처럼 밖에 두고, 설정을 못 읽는 화면에도 버전을 보인다.
  - H2 LogFolder와 한 묶음, 두 행. 탭 설명 문구에 버전. H3 버튼 문구 하나로 통일. H4 홈은 link xs, 설정은 brand sm, 준비 줄은 × 없음. H5 첫 화면은 업데이트 줄 하나만(준비 우선), 520×600 미리보기 장면.
  - M1 "준비 + 녹음 중" 행. M2 설치 중 창 전체 `inert` + 한 줄 문구, Rust가 녹음 시작·가져오기를 거절. M3 같은 버전 설치가 한 번 실패하면 앱 안 설치를 멈춤(`{version, failures}` 저장). M4 재확인 결과 문구. M5 오류를 4~5가지 한국어 문구로, 원문은 로그만. M6 마지막 성공 확인 시각, 7일 넘으면 설정에만 안내. M7 받는 중에도 버튼 자리 유지, 초점, 크기 모르면 MB. M8 가져오기 중 설치 막기, 내보내기도. M9 노트 비면 접기 없음, 개발 빌드.
  - J1 1.1.1 → 1.2.x 손 설치에서 서명이 바뀌어 Mac 사용자 모두 시스템 오디오 권한을 한 번 조용히 잃는다. `last_run_version`과 홈 한 줄. J2 결과 줄에 "바뀐 점" 링크. J3 녹음 중 자동 확인 건너뜀. J4 Windows 창이 닫혔다 열린다는 안내.
  - S1 "1.2.2을" 조사 오류 → "1.2.2 버전을". S2 노트는 `## 바뀐 것` 절만, 기호 제거, "바뀐 것 보기". S3 공개 Release 본문의 설치 파일 표, 공개 README 설치 안내. S4 트레이 순서. S5 "방금 확인했습니다" 3초. S6 대체 경로 안내 위치.
- Codex(gpt-6-astra, plan-review): 완료(`OUTSIDE_STATUS: completed`, `FINDINGS: P1`). 6/10, 지적 10개(High 5, Medium 5). 권고: "구현 전에 고쳐라. 다시 시작 안전, 권한 회복, 실패 회복, 좁은 창 접근성은 구현자가 지어낼 일이 아니다."
  - C1(High) 다시 시작이 저장하지 않은 회의 편집(`Meeting.tsx`의 자동 저장 대기)과 설정 초안을 잃는다. C2(High) 설치를 누른 뒤 재확인·정리 대기 동안 앱 전체 상태가 없고 녹음 시작과 경합한다. C3(High) 정리가 내보내기를 취소하는데 문구는 처리만 말한다. C4(High) 권한 회복 표시가 상호작용 설계 없이 요구사항만 있다. C5(High) 실패 처리가 회복보다 진단 위주, 자동 실패는 설정에서도 안 보이고 설치 실패 알림이 다시 시작하면 사라진다.
  - C6(Medium) 홈이 녹음 준비보다 업데이트를 위에 둔다. C7 좁은 창(520×600) 동작이 정해지지 않았다. C8 접근성(초점, 알림, 대화상자 첫 초점, 닫기 크기)이 일부만. C9 상태 표가 불러오기 실패, 크기 모름, 멈춘 받기, 서명 확인 중, 철회 설명, 빈 노트를 빠뜨림. C10 설정 읽기 전용·읽기 실패와 무관하게 동작해야 한다(H1과 같음).

```
DESIGN OUTSIDE VOICES — LITMUS SCORECARD:
═══════════════════════════════════════════════════════════════
  Check                                    Claude  Codex  Consensus
  ─────────────────────────────────────── ─────── ─────── ─────────
  1. Brand unmistakable in first screen?   N/A     N/A    N/A (기존 화면에 줄만 더함)
  2. One strong visual anchor?             YES*    NO     DISAGREE → 1패스(감사 68)
  3. Scannable by headlines only?          YES     —      NOT SPEC'D(Codex 언급 없음)
  4. Each section has one job?             NO      NO     CONFIRMED 실패 → 1패스 수정
  5. Cards actually necessary?             N/A     N/A    카드 없음
  6. Motion improves hierarchy?            —       —      NOT SPEC'D(움직임 없음, 필요 없음)
  7. Premium without decorative shadows?   N/A     N/A    그림자 없음
  ─────────────────────────────────────── ─────── ─────── ─────────
  Hard rejections triggered:               없음    없음   CONFIRMED 없음
═══════════════════════════════════════════════════════════════
```

\* Claude는 홈의 기준점(녹음 버튼)이 link xs 줄로 지켜진다고 봤고(H4), Codex는 줄 순서가 업데이트를 녹음 준비보다 위에 둔다고 봤다(C6). 4번: 두 목소리 모두 업데이트 정보가 여러 줄·여러 묶음으로 흩어졌다고 봤다(H2·H5, C6).

지적 대응 표(같은 문제를 두 목소리가 따로 찾은 것):

| 주제 | Claude | Codex | 합의 |
|---|---|---|---|
| 설정 잠금·읽기 실패와 무관 | H1 | C10 | CONFIRMED |
| 홈 업데이트 줄 하나, 약한 버튼 | H4·H5 | C6 | CONFIRMED(순서는 DISAGREE) |
| 설치 중 앱 전체 상태 | M2 | C2 | CONFIRMED |
| 파일 작업(가져오기·내보내기) 중 설치 | M8 | C3 | CONFIRMED |
| 권한 회복 표시 | J1·S6 | C4 | CONFIRMED |
| 실패 분류·회복·지속 | M3·M5·M6 | C5 | CONFIRMED |
| 상태 표 빈칸 | M1·M4·M7·M9 | C9 | CONFIRMED |
| 좁은 창 | H5 | C7 | CONFIRMED |
| 접근성 | M7 | C8 | CONFIRMED |
| 저장하지 않은 작업 | 없음 | C1 | Codex만, 데이터 손실이라 수용 |
| 조사·문구·노트·트레이 | S1·S2·S4·S5·J2·J4·H3 | 노트 이름 "변경 사항"(C9) | Claude만 대부분, 노트 이름만 다름 |

#### 1~7 패스

| 패스 | 처음 | 고친 뒤 | 무엇이 바뀌었나 |
|---|---|---|---|
| 1. 정보 구조 | 5 | 8 | 설정은 "앱 정보" 한 묶음(버전·로그), 잠금 밖. 홈과 첫 화면은 업데이트 줄 하나, 순서는 오디오 확인 → 설정 → 업데이트(감사 68, TASTE). 버튼 문구 하나. 남은 틈: 순서는 관문에서 사용자가 정한다 |
| 2. 상태 | 4 | 9 | 5.4 표가 20행: 개발 빌드, 첫 확인 전, 방금 확인, 7일 넘음, 받는 중(크기 모름, 멈춤), 준비+막힘 이유 3가지, 설치 단계, 재확인 결과 2가지, 실패 분류, 설치 실패 기록, 불러오기 실패. 서명 확인 중 상태는 두지 않는다(마지막 조각 뒤 몇 밀리초, 감사 77) |
| 3. 사용자 여정 | 4 | 8 | 아래 여정 표. 조용한 권한 손실(J1), 저장하지 않은 편집(C1), 같은 실패 반복(M3), 녹음 중 자동 확인(J3)을 막는다. 남은 틈: 실제 권한 유지 여부는 0단계 실측이 정한다 |
| 4. 흔한 AI 티 | 8 | 9 | 앱 화면(OPERATE). 카드·장식 없음, 기존 부품만. 하드 리젝트 없음. 조사 오류와 "업데이트하고 다시 시작"/"다시 시작해 업데이트" 혼용을 고쳤다 |
| 5. 디자인 시스템 | 6 | 8 | 6.7 행, `text-label`·`ink-2`, link xs·outline sm·brand sm, `tabular-nums`, 알림 띠 재사용. 디자인 시스템 6.4·6.5·6.7에 적는다. 남은 틈: DESIGN.md 형식 토큰 경로는 없다(기존 문서 방식 유지) |
| 6. 좁은 창·접근성 | 4 | 8 | `sm` 미만에서 버튼이 아래로, 줄바꿈, 첫 화면 세로 스크롤 허용, 520×600·1040×720 미리보기. `aria-disabled`로 초점 유지, 단계 바뀔 때만 `role=status`, 접기 단추 `aria-expanded`, 대화상자 첫 초점 "취소"와 초점 복귀, 닫기 32×32. 남은 틈: 125% 글자 크기는 미리보기에서 손으로 본다 |
| 7. 남은 결정 | 결정 26개 자동 처리, 1개 관문(감사 68), 0개 미룸 | | |

여정 표(3패스):

| 단계 | 사용자가 하는 일 | 느낌 | 계획이 정한 것 |
|---|---|---|---|
| 1 | 1.2.1을 손으로 설치하고 첫 실행 | "또 설치네" | 홈 맨 위 "Mac 권한이 바뀌어 오디오를 다시 확인해야 합니다 · 오디오 확인"(5.7) |
| 2 | 오디오 테스트 통과 | 안심 | 표시가 꺼지고 줄이 사라진다 |
| 3 | 며칠 뒤 홈에 "1.2.2 버전을 받았습니다" | 방해받지 않음 | link xs 한 줄, 녹음 버튼보다 약하다, 녹음 화면에는 없다 |
| 4 | 회의 문서를 고치다 트레이에서 "다시 시작해 업데이트" | 잃을까 걱정 | 편집 대기분을 먼저 저장, 실패면 멈춤(C1). 처리 중이면 대화상자 |
| 5 | 설치 중 | 기다림 | 앱 전체 `inert`, 알림 띠에 단계 문구, Windows는 창이 닫혔다 열린다고 미리 말한다 |
| 6 | 새 버전으로 열림 | 확인 | "1.2.2 버전으로 업데이트했습니다 · 바뀐 것" |
| 6' | 설치 실패 | 불안 | "지금 버전으로 계속 쓸 수 있습니다", 같은 버전 재시도 안 함, 설정에 기록 |

#### NOT in scope (디자인)

- 업데이트 준비 데스크톱 알림: CEO X7로 이미 미룸. 홈 줄과 트레이로 충분한지 파일럿에서 본다.
- 서명 확인 중 상태: 마지막 조각 뒤 몇 밀리초라 문구가 깜빡이기만 한다.
- 노트 마크다운 렌더링: NFR-07(글자 노드만). CI가 평문으로 만든다.
- 목업: 사용자 메모리(앱에 바로 구현하고 preview.html로 본다).

#### What already exists

- `docs/design-system.md` 6.4(홈), 6.5(첫 화면), 6.7(설정 행). 홈 설정 줄(`Home.tsx:436-470`), `LogFolder`(`Settings.tsx:1431-1456`), 종료 대화상자(`App.tsx:581-595`), 알림 띠(`App.tsx:400`), `aria-disabled` 패턴(`Settings.tsx:1504`), 회의 편집 `saves`·`flush`(`Meeting.tsx`), `josa()`(`src/lib/utils.ts:21`, 숫자는 못 읽는다), 트레이 메뉴(`lib.rs:366-372`, 열기·종료).

#### TODOS.md (디자인)

- 새 TODO 없음. 디자인 지적은 모두 이 계획 안에서 고친다. X7(데스크톱 알림)은 CEO 단계 TODO 그대로.

#### Implementation Tasks (Design)

- [ ] **D1 (P1, human: ~1d / CC: ~40min)** — 설정 — "앱 정보" 묶음과 버전 행, 5.4 표 20행, "바뀐 것 보기"
  - Surfaced by: H1·H2·M1·M4·M7·M9, C9·C10
  - Files: `src/screens/Settings.tsx`, 새 `src/components/AppVersion.tsx`, `src/ipc.ts`
  - Verify: 화면 테스트(행별 문구·버튼·초점), 미리보기 520×600
- [ ] **D2 (P1, human: ~4h / CC: ~20min)** — 홈·첫 화면 — 업데이트 줄 하나와 오디오 확인 줄, 순서
  - Surfaced by: H4·H5·J1·J2, C4·C6
  - Files: `src/screens/Home.tsx`, 첫 화면 컴포넌트
  - Verify: 화면 테스트(순서, `ready`가 결과 줄을 가림)
- [ ] **D3 (P1, human: ~1d / CC: ~40min)** — 설치 흐름 — 앱 전체 `installing`, `inert`, 알림 띠, Rust 거절(녹음·가져오기·두 번째 설치, `file_work`)
  - Surfaced by: M2·M8·J4, C2·C3
  - Files: `src/App.tsx`, `src-tauri/src/update.rs`, `src-tauri/src/commands.rs`
  - Verify: Rust 테스트(같은 잠금), 화면 테스트(`inert`)
- [ ] **D4 (P1, human: ~4h / CC: ~20min)** — 저장하지 않은 작업 — 회의 편집 대기분 저장 후 설치, 설정 초안이면 막힘
  - Surfaced by: C1
  - Files: `src/screens/Meeting.tsx`, `src/screens/Settings.tsx`, `src/App.tsx`
  - Verify: 화면 테스트(저장 실패면 설치 안 함)
- [ ] **D5 (P1, human: ~6h / CC: ~30min)** — 실패 — 오류 분류 함수, 설치 실패 기록, `last_check_ok` 7일, 개발 빌드, 녹음 중 자동 확인 건너뜀
  - Surfaced by: M3·M5·M6·J3, C5
  - Files: `src-tauri/src/update.rs`, `src-tauri/src/settings.rs`
  - Verify: Rust 테스트
- [ ] **D6 (P1, human: ~4h / CC: ~20min)** — 권한 — `last_run_version`, `audio_check_needed`, 시스템 오디오 테스트 통과로 끔
  - Surfaced by: J1·S6, C4
  - Files: `src-tauri/src/settings.rs`, `src-tauri/src/lib.rs`, `src/screens/Settings.tsx`(오디오 테스트)
  - Verify: Rust 판정 테스트 5가지
- [ ] **D7 (P2, human: ~2h / CC: ~10min)** — CI — 노트에서 `## 바뀐 것`만 뽑아 평문으로
  - Surfaced by: S2
  - Files: `scripts/latest-json.ts`와 테스트
  - Verify: `bun run test`
- [ ] **D8 (P2, human: ~1h / CC: ~5min)** — 트레이 — "다시 시작해 업데이트", 구분선, 순서
  - Surfaced by: H3·S4
  - Files: `src-tauri/src/lib.rs`
  - Verify: 트레이 항목 순수 함수 테스트
- [ ] **D9 (P2, human: ~3h / CC: ~15min)** — 미리보기 — 5.4 표 행, 홈 세 줄, 결과 줄, 노트, 알림 띠, 대화상자, 두 크기
  - Surfaced by: H5, C7
  - Files: `src/preview/`, fixture
  - Verify: preview.html을 520×600·1040×720에서 본다
- [ ] **D10 (P2, human: ~2h / CC: ~10min)** — 문서 — 디자인 시스템 6.4·6.5·6.7, 공개 저장소 README 설치 안내
  - Surfaced by: S3, Pass 5
  - Files: `docs/design-system.md`, 공개 저장소 README
  - Verify: 읽기

#### Completion Summary (Design)

```
  +====================================================================+
  |         DESIGN PLAN REVIEW — COMPLETION SUMMARY                    |
  +====================================================================+
  | System Audit         | docs/design-system.md(DESIGN.md 없음), 설정·홈·첫 화면·트레이 |
  | Step 0               | 6/10, 상태·여정·좁은 창 중심                |
  | Pass 1  (Info Arch)  | 5/10 → 8/10 after fixes                     |
  | Pass 2  (States)     | 4/10 → 9/10 after fixes                     |
  | Pass 3  (Journey)    | 4/10 → 8/10 after fixes                     |
  | Pass 4  (AI Slop)    | 8/10 → 9/10 after fixes                     |
  | Pass 5  (Design Sys) | 6/10 → 8/10 after fixes                     |
  | Pass 6  (Responsive) | 4/10 → 8/10 after fixes                     |
  | Pass 7  (Decisions)  | 26 resolved, 0 deferred, 1 → gate           |
  +--------------------------------------------------------------------+
  | NOT in scope         | written (4 items)                           |
  | What already exists  | written                                     |
  | TODOS.md updates     | 0 items proposed                            |
  | Approved Mockups     | 0 generated, 0 approved(사용자 메모리)       |
  | Decisions made       | 26 added to plan(자동 결정, 감사 64~90)      |
  | Decisions deferred   | 0                                           |
  | Overall design score | 4/10 → 8/10                                 |
  +====================================================================+
```

- 모든 패스가 8 이상이다. 구현 뒤 `/design-review`로 화면을 본다.
- Unresolved Decisions: 홈 줄 순서(감사 68)는 자동으로 Codex 쪽(녹음 준비가 위)을 골랐고 최종 관문에서 사용자가 확인한다. 그 밖에 묻지 않은 결정은 없다.

### DX 리뷰 (Phase 2.5, 2026-11-05, DX POLISH)

<!-- autoplan-baseline-edits:dx {"sourceSha256":"b1c039671b74ec7c1f42180c25808f5e48d2d4b4031b01370b4e31dc0ad9bb21","replacements":[{"oldText":"아래 줄은 계획 리뷰가 수락한 요구사항이다(리뷰 도구가 붙인다). 본문과 다르면 아래가 이긴다.","newText":"아래 줄은 계획 리뷰가 수락한 요구사항이다(리뷰 도구가 붙인다). 위에서 아래로 시간순이고, 같은 주제의 줄이 여럿이면 가장 나중 줄이 이긴다(본문보다도). \"(… 대체)\" 표시가 없어도 같다. 구현을 시작하기 전에 이 장을 본문에 접어 넣어 본문을 최종 사양으로 만들고, 이 장은 결정 기록으로 남긴다(DX 리뷰)."},{"oldText":"- E0. 실측: 자체 서명 + 업데이터로 macOS에서 권한·키체인·격리·App Management를 확인한다(6장 2단계, 1단계 코드로).","newText":"- E0. 실측: 코드 없이 자체 서명 덮어 설치로 권한·키체인을 확인하고(6장 0단계), 1단계 코드로 업데이터의 격리·App Management·다시 뜨기를 확인한다(6장 2단계)."},{"oldText":"업데이터 키 쌍(1단계 시작 때)과 자체 서명 인증서(유효 20년, 2단계)는 1회 만든다.","newText":"업데이터 키 쌍(1단계 시작 때)과 자체 서명 인증서(유효 20년, 0단계에서 만들어 2단계와 CI가 그대로 쓴다)는 1회 만든다."},{"oldText":"공개 저장소에서 `gh release edit <이전 태그> --latest`로","newText":"`gh release edit <이전 태그> --repo sijecorp/meeting-summarizer-releases --latest`로"},{"oldText":"- capability는 그대로 둔다(JS에서 업데이터를 부르지 않는다).","newText":"- capability는 그대로 둔다(JS에서 업데이터를 부르지 않는다).\n- 업데이터는 빌드할 때 `MS_UPDATER=1`일 때만 켜진다(`option_env!`, 기존 `MS_LLAMA_SERVER`와 같은 이름 규칙). CI의 태그·수동 빌드와 업데이터 시험 빌드만 이 값을 준다. 그 밖의 빌드(`tauri dev`, README의 로컬 설치 파일)는 확인하지 않는다. 로컬에서 만든 앱이 공개 버전으로 자기를 바꾸지 않게 한다."},{"oldText":"| 개발 빌드(`tauri dev`) | \"개발 빌드는 업데이트를 확인하지 않습니다\" | 없음 |","newText":"| 업데이터가 꺼진 빌드(`MS_UPDATER` 없음, 5.3) | \"이 빌드는 업데이트를 확인하지 않습니다\" | 없음 |"},{"oldText":"- 수동 실행(`workflow_dispatch`)은 공개 저장소에 올리지 않는다.","newText":"- 수동 실행(`workflow_dispatch`)은 공개 저장소에 올리지 않는다.\n- 공개된 태그는 바뀌지 않는다: 태그 빌드는 맨 앞에서 공개 저장소에 그 태그의 Release가 공개돼 있는지 보고, 있으면 빌드 전에 실패한다(\"공개된 태그는 다시 빌드하지 않습니다. 버전을 올려 새 태그를 만드세요\"). 비공개 Release의 `--clobber` 덮어쓰기는 공개 전 재시도에만 쓰인다. `publish`만 실패했으면 \"실패한 작업만 다시 실행\"으로 같은 실행의 산출물을 다시 쓴다."}]} -->

<!-- autoplan-accepted:dx -->
- (DX) 10장 우선 규칙: 시간순, 같은 주제는 가장 나중 줄이 본문보다도 이긴다. 구현 첫 작업으로 10장을 본문(5~9장)에 접어 넣고 10장을 결정 기록으로 바꾼다. 상태 표는 5.4 하나만 둔다.
- (DX, CEO·디자인 줄 대체) IPC 계약: Rust는 serde 태그 열거형, 화면은 `src/lib/ipc.ts`의 같은 모양 판별 유니온을 쓴다. `update_state` → `{ version, state, blocked, just_updated, failed_install, stale }`. `state`는 `idle | checking | up_to_date | downloading{version, received, total?, waiting} | ready{version, notes} | installing{stage: recheck | finishing | installing} | failed{kind, manual}`. `blocked`는 `recording | file_work | null`(Rust가 판정, 저장하지 않은 설정과 회의 편집은 화면이 판정). `app://update` 이벤트의 내용은 `update_state`와 같다. 화면은 이벤트를 먼저 구독한 뒤 `update_state`를 한 번 부른다.
- (DX, CEO·디자인 줄 대체) `install_update(confirmed)`는 오류 문자열 대신 결과를 돌려준다: `started | needs_confirm | blocked{reason} | not_ready`. `needs_confirm`이면 상태는 `ready` 그대로 대화상자를 띄우고, 취소하면 아무것도 바뀌지 않는다. `installing`은 `started` 뒤부터다. 화면은 회의 편집 저장(디자인 줄)을 `install_update`를 부르기 전에 끝낸다. 실제 실패만 `Err`다.
- (DX, 디자인 줄 대체) 오류 분류: Rust의 순수 함수가 원문 오류를 `kind`(`network | signature | feed | other`)로 나누고, 4가지 한국어 문구는 화면(TS)이 `kind`로 고른다. 사용자 문구는 화면 한 곳에만 둔다. `app.log`에는 `kind`, 단계, 대상 버전, 원문을 한 줄로 남긴다.
- (DX, CEO·디자인 줄 대체) 업데이터 상태는 설정이 아니라 앱 데이터 폴더의 `update-state.json`(Rust만 쓴다, 저장 막대와 읽기 전용 설정과 무관)에 둔다: `{ schema_version, updated_to, install_failed{version, count}, last_check_ok, last_run_version, audio_check_needed }`. 모르는 필드는 무시하고, 파일이 깨지면 기본값으로 읽고 로그를 남긴다. 쓰기가 실패하면 로그만 남기고 설치는 계속한다(결과 줄과 실패 기록은 잃을 수 있다).
- 오디오 확인 판정: macOS에서 이전 설치의 흔적(설정 파일이나 회의 폴더)이 있는데 `update-state.json`이 없거나, `last_run_version` < `AUDIO_RECHECK_BELOW`(이름 붙은 상수, 처음 값 1.2.0)이면 `audio_check_needed`를 켠다. 자체 서명 인증서를 바꾸면 이 상수를 그 버전으로 올린다(관리자 문서와 build.yml 해시 검사 옆 주석에 적는다).
- 빌드별 업데이터: `MS_UPDATER` 없는 빌드(`tauri dev`, 미리보기, README 로컬 설치 파일)는 확인하지 않는다. CI 태그 빌드는 공개 피드, CI 수동 빌드는 공개 피드를 확인하지만 공개 저장소에 올리지 않는다. 업데이터 시험 빌드는 시험 `--config`의 로컬 끝점을 쓴다. build.yml은 배포용 설정 파일에 `dangerousInsecureTransportProtocol`이 있으면 실패한다.
- 관리자 문서(install.md "관리자용: 설치 파일 만들기"에 둔다. README와 아키텍처 11장이 링크한다). 복사해 그대로 실행하는 명령으로 쓴다: `bun tauri signer generate -w ~/.tauri/meeting-summarizer.key`, openssl로 자체 서명 코드 서명 인증서 만들기(`extendedKeyUsage=codeSigning`, 20년)와 p12 내보내기, `base64 -i cert.p12 | gh secret set MACOS_SELF_SIGNED_P12`, `gh secret set TAURI_SIGNING_PRIVATE_KEY < key`, `gh variable set SLACK_AUTO --body false`, 공개 저장소 만들기(README 커밋), GitHub App 만들기(공개 저장소 하나에만 설치, Contents 읽기·쓰기)와 키 등록, `codesign -d -r- <앱>`으로 `certificate root` 해시 읽기. 각 값의 형식, 등록 위치, 성공 확인을 함께 적는다.
- 관리자 문서의 복구 절: 업데이터 키, 인증서, GitHub App 키 각각에 대해 "백업에서 되살리기"와 "영구 분실"을 나눈다. 업데이터 키 영구 분실은 모든 사람이 손으로 다시 설치, 인증서 영구 분실은 새 인증서 + `AUDIO_RECHECK_BELOW` 올리기 + 해시 바꾸기, App 키 분실은 새 키 등록 후 `publish`만 다시 실행. 6장 3단계는 두 번째 보관자가 백업한 키로 시험 파일에 서명·검증해 본 뒤에 끝난다.
- build.yml에 왜 `APPLE_CERTIFICATE`가 아닌지 주석을 둔다(Tauri는 Apple 접두어가 없는 인증서를 찾지 않는다, `tauri-macos-sign` `identity.rs`).
- 업데이터 시험 하네스: `scripts/updater-test.ts`(bun)가 시험 빌드의 `.app.tar.gz`와 `.sig`로 한 플랫폼짜리 `latest.json`을 만들고 `127.0.0.1:8765`에서 내준다. 관리자 문서에 0·2단계 절차를 순서대로 둔다: 지금 앱 종료와 앱 데이터·키체인 항목 백업, 두 버전 빌드(`MS_UPDATER=1 APPLE_SIGNING_IDENTITY=… TAURI_SIGNING_PRIVATE_KEY=… bun run tauri build --config src-tauri/tauri.updater.conf.json --config <시험 설정>`), 하네스 실행, 격리 속성 붙여 설치, 확인 목록, 원상 복구. 인증서는 0단계에서 만든 것을 쓴다.
- 태그 배포 체크리스트(관리자 문서): `package.json`·`src-tauri/Cargo.toml`·`src-tauri/tauri.conf.json` 버전 올리기와 lockfile, `docs/releases/v<버전>.md`, 커밋, 태그. build.yml의 태그 검사는 세 파일 모두를 태그와 비교한다(지금은 `tauri.conf.json`만).
- `scripts/latest-json.ts` 사용법: `bun scripts/latest-json.ts --tag vX.Y.Z --notes docs/releases/vX.Y.Z.md --assets <폴더> --repo sijecorp/meeting-summarizer-releases --out latest.json`. 성공 0, 실패 1이고 빠진 플랫폼·파일·필드와 고칠 방법을 한 줄씩 출력한다.
- 릴리스 노트 계약과 틀(`docs/releases/TEMPLATE.md`): 맨 위 `## 설치 파일` 표, 그다음 `## 바뀐 것`(앱이 보이는 부분, 업데이트 전에 꼭 읽어야 할 말도 여기), `## 알려진 제한`. 설치 안내 링크는 공개 저장소 README로. 태그 빌드에서 노트 파일이 없거나 `## 바뀐 것`이 없으면 `publish`가 실패한다. 공개 저장소에는 `--generate-notes`를 쓰지 않는다(PR 제목과 작성자가 공개된다). install.md의 동료용 설치 절차는 공개 README를 가리키고 두 벌을 두지 않는다. 공개 README와 1.1.1 → 1.2.1 손 설치 안내는 6장 4단계의 알림 전에 끝낸다.
- CI 오류 문구는 문제, 원인, 고칠 방법을 `::error::` 한 줄에 쓴다(build.yml:55 방식). 자체 서명 시크릿 없음("태그 빌드는 자체 서명이 필요합니다. 관리자 문서의 인증서 절로 MACOS_SELF_SIGNED_P12를 등록하세요"), 해시 불일치(기대값과 실제값, "인증서를 바꿨다면 해시와 AUDIO_RECHECK_BELOW를 올리세요"), 공개된 태그("공개된 태그는 다시 빌드하지 않습니다. 버전을 올려 새 태그를 만드세요"), 노트 계약 위반, `latest-json` 검증 실패, GitHub App 토큰 실패("RELEASES_APP_ID·RELEASES_APP_KEY와 App 설치 저장소를 확인하세요"), 버전 불일치(세 파일 값과 태그).
- 잘못된 릴리스 절차: 모든 `gh` 명령에 `--repo sijecorp/meeting-summarizer-releases`를 붙이고, 되돌린 뒤 `curl -sL https://github.com/sijecorp/meeting-summarizer-releases/releases/latest/download/latest.json | jq -r .version`으로 확인한다. 제한도 적는다: 이미 설치한 앱은 내려가지 않는다, 설치 직전 재확인이 네트워크 오류로 끝난 앱은 받아 둔 버전을 설치할 수 있다.
- 측정 명령(관리자 문서): `gh api repos/sijecorp/meeting-summarizer-releases/releases/tags/vX.Y.Z --jq '.assets[]|select(.name|test("tar.gz$|setup.exe$"))|"\(.name) \(.download_count)"'`. 재시도와 끊긴 받기도 세므로 실제보다 많다고 적는다.
- 테스트: `latest-json.ts`의 인자 누락·플랫폼 누락·`## 바뀐 것` 없음 실패 문구와 종료 코드, `update-state.json` 읽기(없음·깨짐·모르는 필드)와 쓰기 실패, `AUDIO_RECHECK_BELOW` 판정, `MS_UPDATER` 없는 빌드의 상태, `install_update` 결과 4가지와 `needs_confirm` 뒤 취소 시 상태 그대로, Rust 상태 열거형의 JSON 모양 스냅샷(TS 유니온과 맞는지), build.yml의 세 파일 버전 검사와 시험 설정 검사.
<!-- /autoplan-accepted:dx -->

#### Step 0

- 제품 유형: 사내 배포 데스크톱 앱의 **배포·업데이트 플랫폼**(CI 워크플로, 서명 키, 공개 피드, 시험 하네스)과 구현자가 만드는 Rust/TS 인터페이스. 동료(최종 사용자)는 디자인 리뷰가 다뤘다.
- 모드: DX POLISH(자동 결정, autoplan 규칙). 범위를 넓히지 않고 접점을 단단하게 한다.

```
TARGET DEVELOPER PERSONA
========================
Who:       저장소 관리자(혼자 태그로 릴리스) + 키가 사라졌을 때 혼자 복구해야 하는 두 번째 보관자
Context:   처음 한 번의 배포 설정, 매 릴리스, 드물게 키 분실·잘못된 릴리스 복구
Tolerance: 설정은 반나절도 참지만 복구는 문서만 보고 1시간 안에 끝나야 한다(동료가 기다린다)
Expects:   복사해 실행하는 명령, 실패하면 무엇을 고칠지 말해 주는 CI, 다시 만들 수 있는 시험 절차
```

- 공감 서술(예측, 실측 아님): "1.2.0을 내려고 계획을 연다. 키 쌍, 자체 서명 인증서, 공개 저장소, GitHub App, 시크릿 여섯 개와 변수 하나가 필요하다는 말은 있는데 명령은 없다. openssl 옵션은 '실험'이라고만 적혀 있어 검색부터 한다. 10장에서 10초 대기를 넘으면 설치를 계속한다는 줄과 하지 않는다는 줄을 둘 다 보고 어느 쪽이 맞는지 리뷰 기록을 뒤진다. 태그를 붙였는데 `publish`가 실패했다. 다시 돌리면 비공개 Release의 설치 파일이 새 빌드로 바뀌고 Slack에도 그게 올라가는데, 공개 저장소에는 옛것이 남는다. 1년 뒤 내가 없을 때 두 번째 보관자는 이 계획 문서를 열어 볼까?"
- 경쟁 비교(웹 조사 안 함, 알려진 지식 기준 추정):

| 도구 | 시작 → 결과 | 시간(근거) | DX 선택 |
|---|---|---|---|
| Tauri 공식 업데이터 문서 | 키 생성 → 첫 서명 업데이트 | 약 1~2시간(추정) | `tauri signer generate`, 환경 변수, `createUpdaterArtifacts` 예제 |
| electron-builder + GitHub provider | 설정 → 자동 게시 | 약 1시간(추정) | `publish: github` 한 줄, 서명은 별도 |
| 이 계획(리뷰 전) | 아무것도 없음 → 공개 피드로 첫 업데이트 | 반나절 이상(추정, 17단계에 명령 없음) | 단계 목록만 |
| 이 계획(리뷰 후 목표) | 같음 | 약 90분(목표) | 관리자 문서의 복사 명령, CI 오류 문구, 시험 하네스 |

- 목표(자동 결정): 관리자 첫 배포 설정 90분, 두 번째 보관자 복구 60분. 사내 도구라 "Competitive" 등급에 맞춘다.
- 마법의 순간: 관리자가 태그를 붙이면 20분 안에 동료 앱의 홈에 "1.2.2 버전을 받았습니다"가 뜨고 Slack에 파일을 올릴 필요가 없다. 전달 수단은 이미 계획에 있는 태그 워크플로와 공개 피드(새 기능 없음).

개발자 여정(0F):

```
STAGE           | DEVELOPER DOES                         | FRICTION POINTS                         | STATUS
----------------|----------------------------------------|-----------------------------------------|--------
1. Discover     | 계획·README·install.md에서 절차를 찾는다 | 10장 우선 규칙이 옛 줄을 이기게 함        | fixed(시간순 규칙 + 본문 접기)
2. Install      | 키·인증서·저장소·App·시크릿 설정         | 명령 없음, 인증서를 두 단계에서 만듦       | fixed(관리자 문서, 0단계 1회)
3. Hello World  | 0·2단계 시험, 첫 태그                   | 하네스 재현 불가, 로컬 빌드가 공개 피드 확인 | fixed(하네스 스크립트, MS_UPDATER)
4. Real Usage   | 매 릴리스: 버전, 노트, 태그              | 버전 세 곳, 노트가 비공개 링크·자동 생성   | fixed(체크리스트, 노트 계약, 세 파일 검사)
5. Debug        | CI 실패, 앱 오류                         | 새 CI 실패 문구 없음, 오류 채널 혼합      | fixed(::error:: 문구, 결과 열거형, kind)
6. Upgrade      | 재실행, 롤백, 인증서 교체, 키 분실         | 태그가 바뀜, `--repo` 없음, 인증서 교체가 조용한 무음 회귀 | fixed(태그 불변, --repo, AUDIO_RECHECK_BELOW, 복구 절)
```

첫 사용자 혼란 보고(0G, 두 번째 보관자가 1년 뒤 인증서 분실을 복구):

```
T+0:00  install.md 관리자 절을 연다(리뷰 전: 이 절에 업데이터 내용 없음 → 리뷰 후: 복구 절이 있다)
T+0:10  새 인증서를 만든다(리뷰 전: openssl 옵션 검색 → 리뷰 후: 복사 명령)
T+0:25  태그 빌드가 해시 불일치로 실패(리뷰 전: 문구 없음 → 리뷰 후: 기대값·실제값과 "해시와 AUDIO_RECHECK_BELOW를 올리세요")
T+0:35  상수와 해시를 올리고 다시 태그(리뷰 전: 상수를 몰라 Mac 동료 전원이 조용히 상대 소리를 잃음)
T+0:50  끝. 동료 앱에 오디오 확인 줄이 뜬다
```

#### 두 목소리 (Step 0.5)

- Claude 독립 DX 리뷰어: 완료(INPUT `dx d3391466…` 일치). 10개(치명 1, 높음 3, 중간 6). 요지: 1 10장 우선 규칙이 옛 줄을 이기게 한다(10초 대기, release/publish, PAT 알림, Slack, 문구, 홈 순서 세 벌). 2 일회성 설정에 명령이 없다. 3 인증서 교체가 조용한 무음 회귀를 되살린다(`AUDIO_RECHECK_BELOW`). 4 공개 노트에 강제 검사가 없고 비공개 링크·`--generate-notes`. 5 시험 하네스가 커밋되지 않는다. 6 IPC 오류 채널에 한국어 문구와 기계 코드가 섞인다. 7 새 CI 실패 문구가 없다. 8 새 상태의 저장 위치가 모호하고 사용자 설정에 묶인다. 9 개발·시험 빌드의 탈출구가 없다. 10 측정 명령이 없다.
- Codex(gpt-6-astra, plan-review): 완료(`OUTSIDE_STATUS: completed`, `FINDINGS: P1`). 6/10, 11개(High 7, Medium 4). 요지: 1 같은 태그가 다른 바이너리를 가리킬 수 있다(`build.yml:309` `--clobber`). 2 롤백 명령에 `--repo`가 없다. 3 두 번째 보관자가 실행할 절차가 없다. 4 시험 하네스가 재현 가능하지 않고 인증서를 두 번 만든다, 개발 앱과 데이터 공유. 5 Rust/TS 계약이 없다. 6 새 영속 필드의 소유권과 덮어쓰기 경합(`settings.rs:254`). 7 로컬 설치 파일 빌드가 공개 피드로 자기를 바꿀 수 있다. 8 버전이 세 파일, `latest-json` CLI 계약 없음. 9 공개 노트 틀 없음. 10 최종 사양을 찾으려면 리뷰 이력을 해석해야 한다. 11 README 5분 경로(화면 미리보기만 현실적).

```
DX DUAL VOICES — CONSENSUS TABLE:
  Dimension                           Claude  Codex  Consensus
  1. Getting started < 5 min?          NO      NO     CONFIRMED 실패(관리자 설정 반나절+, 명령 없음)
  2. API/CLI naming guessable?         부분    부분   CONFIRMED(이름은 괜찮고 계약·결과형이 없다)
  3. Error messages actionable?        NO      NO     CONFIRMED(새 CI 실패와 latest-json 문구 없음)
  4. Docs findable & complete?         NO      NO     CONFIRMED(10장 우선 규칙, 관리자 문서 없음)
  5. Upgrade path safe?                NO      NO     CONFIRMED(인증서 교체 회귀 / 태그 불변·--repo)
  6. Dev environment friction-free?    NO      NO     CONFIRMED(하네스, 로컬 빌드의 업데이트)
CONFIRMED = native + outside agree. 불일치 0.
```

단일 목소리 지적: Claude만 3(인증서 교체), 10(측정 명령). Codex만 1(태그 불변), 2(`--repo`), 8(버전 세 곳), 11(README 5분). 모두 근거를 확인했고(`build.yml:309`, `build.yml:53`, `install.md:112`, `MS_LLAMA_SERVER`) 11만 범위 밖으로 둔다.

#### 1~8 패스

| 패스 | 처음 | 고친 뒤 | 무엇이 바뀌었나 |
|---|---|---|---|
| 1. 시작하기(관리자 첫 설정) | 3 | 8 | 관리자 문서의 복사 명령, 인증서는 0단계 1회, 두 번째 보관자 서명 시험이 3단계 완료 조건 |
| 2. 인터페이스 | 5 | 8 | `update_state`·`app://update`·`install_update` 결과 열거형, TS 판별 유니온, `latest-json` CLI 인자와 종료 코드 |
| 3. 오류 | 5 | 8 | CI `::error::` 문구 7종(문제·원인·고칠 방법), 앱 오류는 `kind` + 화면 문구, 로그 한 줄 형식 |
| 4. 문서 | 4 | 8 | 10장 시간순 규칙과 구현 전 본문 접기, install.md 관리자 절 하나, 노트 틀, 공개 README가 동료 안내의 유일한 곳 |
| 5. 업그레이드 | 5 | 8 | 공개 태그 불변, 롤백 `--repo`와 확인 명령, `AUDIO_RECHECK_BELOW`, 키별 복구 절, 세 파일 버전 검사 |
| 6. 개발 환경 | 4 | 8 | `MS_UPDATER` 빌드 정책, `scripts/updater-test.ts`, 시험 전후 백업·복구, 배포 설정의 insecure 검사 |
| 7. 공동체 | 6 | 7 | 사내 도구라 공동체는 Slack 채널이다. 공개 저장소 README가 동료 안내의 정본이 된다. 남은 틈: 공개 저장소에 이슈 창구를 두지 않는다(사내 Slack이 창구) |
| 8. 측정 | 5 | 8 | `download_count` 명령과 과대 집계 주의, 관리자 첫 설정 90분·복구 60분을 4단계와 3단계에서 실제로 잰다 |

#### DX 스코어카드

```
+====================================================================+
|              DX PLAN REVIEW — SCORECARD                             |
+====================================================================+
| Dimension            | Score  | Prior  | Trend  |
|----------------------|--------|--------|--------|
| Getting Started      | 8/10   | 3/10   | ↑      |
| API/CLI/SDK          | 8/10   | 5/10   | ↑      |
| Error Messages       | 8/10   | 5/10   | ↑      |
| Documentation        | 8/10   | 4/10   | ↑      |
| Upgrade Path         | 8/10   | 5/10   | ↑      |
| Dev Environment      | 8/10   | 4/10   | ↑      |
| Community            | 7/10   | 6/10   | ↑      |
| DX Measurement       | 8/10   | 5/10   | ↑      |
+--------------------------------------------------------------------+
| TTHW                 | 90 min | 240+ min(추정) | ↑ |
| Competitive Rank     | Competitive(사내 도구 기준)                   |
| Magical Moment       | designed via 태그 워크플로 + 공개 피드          |
| Product Type         | Platform(배포 파이프라인) + 내부 인터페이스      |
| Mode                 | POLISH                                       |
| Overall DX           | 8/10   | 5/10   | ↑      |
+====================================================================+
| DX PRINCIPLE COVERAGE                                               |
| Zero Friction      | covered(복사 명령, 90분 목표)                    |
| Learn by Doing     | covered(시험 하네스)                             |
| Fight Uncertainty  | covered(CI 문구, 결과 열거형)                     |
| Opinionated + Escape Hatches | covered(MS_UPDATER, 시험 설정)          |
| Code in Context    | covered(실제 저장소 이름이 든 명령)               |
| Magical Moments    | covered(태그 → 동료 앱)                          |
+====================================================================+
```

TTHW: 관리자 첫 설정은 지금 반나절 이상(추정)이고 목표는 90분이다. 4단계에서 실제로 잰다. 기여자의 화면 미리보기 경로(`bun install`, `bun run dev`)는 바뀌지 않는다.

```
DX IMPLEMENTATION CHECKLIST
============================
[ ] 관리자 첫 설정 90분 안(4단계에서 잰다), 두 번째 보관자 복구 60분 안(3단계 서명 시험)
[ ] 설치(기여자): 바뀌지 않음. README 로컬 빌드는 키 없이 된다
[ ] 첫 실행: MS_UPDATER 없는 빌드는 "이 빌드는 업데이트를 확인하지 않습니다"
[ ] 마법의 순간: 태그 → 동료 앱의 준비 줄(4단계에서 확인)
[ ] 모든 CI 실패 문구: 문제 + 원인 + 고칠 방법(+ 관리자 문서 절)
[ ] 인터페이스: 결과 열거형과 TS 유니온, latest-json CLI 사용법
[ ] 기본값: 업데이터는 CI 빌드에서만 켜진다
[ ] 문서의 명령은 그대로 붙여 넣어 돈다(3단계에서 두 번째 보관자가 확인)
[ ] 실제 사용 예: 롤백, 키 분실, 인증서 교체 절
[ ] 업그레이드: 공개 태그 불변, AUDIO_RECHECK_BELOW
[ ] TypeScript 타입: update 상태 유니온
[ ] CI: 세 파일 버전 검사, insecure 설정 검사
[ ] 변경 기록: docs/releases 노트 계약과 틀
```

#### NOT in scope (DX)

- README 기여자 "5분 안에 화면" 재구성(Codex 11): 이 계획은 기여자 시작 경로를 바꾸지 않는다. 화면 미리보기 경로는 이미 두 명령이다. 따로 다룰 일.
- 공개 저장소의 이슈 창구: 사내 Slack 채널이 창구다.
- 실행 중 끝점 바꾸기(`MS_UPDATE_ENDPOINT` 같은 런타임 변수): 시험 `--config`와 `MS_UPDATER`로 충분하다(Claude 9의 런타임 변수 제안은 빌드 시 값으로 대체).

#### What already exists

- install.md "관리자용: 설치 파일 만들기"(112행), build.yml의 `::error::` 태그 검사(53-55행), `MS_LLAMA_SERVER` 이름 규칙, `src/lib/ipc.ts`의 명령 타입 패턴, `docs/releases/*.md`의 `## 설치 파일`·`## 바뀐 것` 구성, `settings.rs`의 Rust 소유 필드 보존(254행, 업데이터 상태는 별도 파일이라 쓰지 않는다).

#### TODOS.md (DX)

- 새 TODO 없음. 지적은 모두 이 계획 안에서 고친다. README 기여자 경로는 범위 밖으로만 적는다.

#### Implementation Tasks (DX)

- [ ] **X1 (P1, human: ~3h / CC: ~20min)** — 계획 — 10장을 본문에 접어 넣고 10장을 결정 기록으로
  - Surfaced by: Claude 1, Codex 10
  - Files: `docs/plans/2026-11-05-in-app-updater.md`
  - Verify: 같은 주제가 본문 한 곳에만 있다
- [ ] **X2 (P1, human: ~4h / CC: ~20min)** — IPC — 상태·결과 열거형, TS 유니온, `kind`, 구독 순서
  - Surfaced by: Claude 6, Codex 5
  - Files: `src-tauri/src/update.rs`, `src/lib/ipc.ts`
  - Verify: JSON 모양 스냅샷 테스트
- [ ] **X3 (P1, human: ~3h / CC: ~15min)** — 상태 파일 — `update-state.json`, `AUDIO_RECHECK_BELOW`
  - Surfaced by: Claude 3·8, Codex 6
  - Files: `src-tauri/src/update.rs`
  - Verify: Rust 테스트(없음·깨짐·쓰기 실패·판정)
- [ ] **X4 (P1, human: ~1h / CC: ~10min)** — 빌드 정책 — `MS_UPDATER`, insecure 설정 검사
  - Surfaced by: Claude 9, Codex 7
  - Files: `src-tauri/src/update.rs`, `.github/workflows/build.yml`
  - Verify: 로컬 빌드가 확인하지 않음
- [ ] **X5 (P1, human: ~1d / CC: ~40min)** — 관리자 문서 — 설정 명령, 복구 절, 태그 체크리스트, 롤백, 측정
  - Surfaced by: Claude 2·10, Codex 2·3·8
  - Files: `docs/install.md`, `README.md`, `docs/architecture.md`
  - Verify: 두 번째 보관자가 명령만으로 서명 시험(3단계)
- [ ] **X6 (P1, human: ~4h / CC: ~20min)** — CI — 공개 태그 불변 검사, 세 파일 버전 검사, 노트 계약 검사, `::error::` 문구, `APPLE_CERTIFICATE` 주석
  - Surfaced by: Claude 4·7, Codex 1·8·9
  - Files: `.github/workflows/build.yml`
  - Verify: 수동 실행에서 각 실패 경로 문구
- [ ] **X7 (P2, human: ~3h / CC: ~15min)** — 시험 하네스 — `scripts/updater-test.ts`와 0·2단계 절차
  - Surfaced by: Claude 5, Codex 4
  - Files: `scripts/updater-test.ts`, `docs/install.md`
  - Verify: 2단계를 절차대로 한 번 돈다
- [ ] **X8 (P2, human: ~2h / CC: ~10min)** — 노트 — `docs/releases/TEMPLATE.md`, 공개 README 링크, `latest-json` CLI 문구
  - Surfaced by: Claude 4, Codex 8·9
  - Files: `docs/releases/TEMPLATE.md`, `scripts/latest-json.ts`
  - Verify: `bun run test`

#### Completion Summary (DX)

- 두 목소리 모두 완료, 6개 차원 모두 CONFIRMED, 불일치 0. 결정 19개 자동(감사 91~109), TASTE 0, User Challenge 0.
- 전체 DX 5/10 → 8/10. 모든 패스 7 이상, 공동체만 7(사내 도구).
- Unresolved Decisions: 없음.

### Eng 리뷰 (Phase 3, 2026-11-05)

<!-- autoplan-accepted:eng -->
- (Eng, CEO 줄 대체) 공개 피드 승격: `publish` 작업은 `concurrency: {group: publish-releases, cancel-in-progress: false}`로 한 번에 하나만 돈다. 그 안에서 공개 저장소의 지금 latest와 이 태그를 semver로 비교해 더 새로울 때만 `--latest`, 아니면 `--latest=false`로 공개한다. 공개 뒤 `curl -sfL …/latest/download/latest.json | jq -r .version`이 가장 높은 공개 태그와 같은지 확인하고 다르면 실패한다. "latest로 할지" 판정은 순수 함수와 테스트로 둔다. 롤백 절차는 진행 중인 `publish`가 끝난 뒤에 한다(관리자 문서).
- (Eng) release·publish 조건은 `github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')`다(지금 `build.yml:292`는 태그만 본다. 태그를 고른 수동 실행이 공개하지 않게 한다).
- (Eng, CEO 줄 대체) 설치 대기 시간은 상수 `INSTALL_WAIT = Limits::reap + 5초`(지금 15초)이고 정리 단계 전체(녹음 마무리, 내보내기 취소, 엔진·워커 종료)에 걸린다. `recorder.stop`과 `exports.cancel_all`처럼 막히는 정리는 메인 스레드 밖에서 돈다. 넘으면 설치하지 않고 지금 버전으로 다시 시작한다. 테스트: `INSTALL_WAIT > reap`.
- (Eng) 엔진 판정은 띄우는 중인 사이드카도 센다: `sidecar.rs`의 세대 확인과 자식 등록 사이를 생명주기 잠금 안의 "띄우는 중" 수로 막고, 설치 대기는 띄우는 중·큐 작업·자식·정리 중이 모두 0일 때 끝난다. 테스트: 세대 확인과 등록 사이에 종료.
- (Eng, 디자인 줄 대체) 설치 실패 기록은 다시 시도할 수 있다: 실패 행에서 사용자가 "업데이트 확인"을 누르면 기록을 한 번 지우고 다시 받는다(`count < 2`). 두 번 실패하면 그 버전은 링크만 보인다. 자동 확인은 기록이 있는 버전을 받지 않는다.
- (Eng, 디자인·DX 줄 대체) 설치 수락 판정은 잠금 둘을 엇갈려 잡지 않는다: 전역 `INSTALLING: AtomicBool`(SeqCst, `QUIT_CONFIRMED`와 같은 방식). 설치는 깃발을 먼저 세운 뒤 녹음·파일 작업·Slack 보내기 수를 보고, 하나라도 있으면 깃발을 내리고 `blocked{reason}`을 돌려준다. 녹음 시작, 파일 작업, Slack 보내기는 자기 수를 먼저 올린 뒤 깃발을 보고 서 있으면 수를 내리고 물러난다. 테스트: 두 순서 모두.
- (Eng) 파일 작업 보호: 회의 마크다운 내보내기, 녹음 내보내기, 오디오 가져오기, 모델 가져오기는 명령에 들어오자마자(저장 대화상자와 작업 시작 전에) 보호 객체(RAII 수)를 잡고 마지막 쓰기나 취소까지 쥔다(`commands.rs:257`, `:767`, `:842`). `Exports`의 실행 여부는 공개 접근자로 읽는다.
- (Eng) Slack 보내기 보호: 보내기는 같은 방식의 수를 잡고 전송과 `Sent`·`also_sent` 저장이 끝날 때까지 쥔다(`slack.rs:944` 이후). 그동안 설치는 `blocked{reason: sending}`이고 화면 문구는 "Slack 보내기가 끝나면 업데이트할 수 있습니다".
- (Eng, DX 줄 대체) 화면 준비 단계: "다시 시작해 업데이트"를 누르면 화면이 먼저 앱 전체 "준비 중"(`inert`, 편집·이동 막음)이 되고, 모든 회의의 저장 대기분을 끝까지 비운 뒤 `install_update`를 부른다. `needs_confirm`이면 준비를 풀고 대화상자, 확인하면 다시 준비·비우기 뒤 `true`로 부른다. `blocked`·오류면 준비를 풀고 이유를 보인다. 설정은 저장하지 않은 변경이 있거나 저장 중(`saving`, 키체인 정리 포함)이면 막고, 이 상태는 App이 알 수 있게 올린다.
- (Eng) 종료 중재: `INSTALLING`이 서 있으면 트레이·앱 메뉴의 "종료"(`lib.rs:398` `app.exit(0)`)와 `ExitRequested`는 미뤄지고, 업데이터가 하는 다시 시작만 나갈 수 있다. 테스트: 설치 중 메뉴 종료, 설치 성공 뒤 다시 시작.
- (Eng) 다시 시작과 스레드: `install_update`는 비동기 명령(`#[tauri::command(async)]`)이고 설치·다시 시작은 메인 스레드 밖에서 부른다(macOS 관리자 암호 대체 경로가 메인 스레드를 기다린다). 다시 시작은 `app.request_restart()`로 해 `RunEvent::Exit`가 돌게 한다(단일 실행 소켓 정리). `shutdown_work`는 두 번 불려도 안전하다. 테스트: 두 번 부르기.
- (Eng) macOS 사전 점검: `started` 전에 앱 위치가 `/AppTranslocation/`이나 `/Volumes/` 아래인지, 임시 폴더와 같은 볼륨인지, 앱 폴더에 쓸 수 있는지 본다. 하나라도 걸리면 작업을 멈추지 않고 "앱에서 설치할 수 없는 위치입니다. 응용 프로그램 폴더로 옮기거나 릴리스 페이지에서 받으세요"와 링크를 보인다. 쓸 수 없을 때 플러그인이 띄우는 관리자 암호 창은 install.md에 적는다.
- (Eng) 배포 파일 검사: macOS 작업은 `.app.tar.gz`를 임시 폴더에 풀어 `codesign --verify --deep --strict`와 `certificate root` 해시를 본다. `publish`는 두 `.sig`를 `tauri.conf.json`의 공개 키로 검증하고(minisign), 공개 뒤 `latest.json`의 두 URL에 HEAD 요청을 보낸다.
- (Eng) `plugins.updater.requireSignedVersion: true`(사내 TLS 검사 프록시가 `latest.json`을 바꿔 옛 서명 파일을 새 버전으로 속이는 다운그레이드를 막는다). 2단계에서 확인한다.
- (Eng, CEO 줄 대체) 5.8 첫 줄을 나눈다: 업데이트 뒤 마이크·시스템 오디오(TCC) 권한이 풀리면 ad-hoc + 안내. 키체인 창만 뜨면 자체 서명을 유지하고 업데이트 뒤 줄에 "항상 허용"을 누르라는 말을 붙인다(install.md:65).
- (Eng, DX 줄 대체) macOS에서 `MS_UPDATER=1` 빌드는 자체 서명 인증서가 있어야 한다. 인증서 시크릿이 없는 수동 빌드는 `MS_UPDATER` 없이 빌드한다(ad-hoc 앱이 업데이터로 서명을 바꿔 오디오 확인을 건너뛰지 않게).
- (Eng) CI 임시 키체인: `security set-keychain-settings -lut 21600 "$KC"`, 서명 직전 다시 `unlock-keychain`, `if: always()` 단계에서 지운다(20~50분 컴파일 동안 잠기지 않게).
- (Eng, 계획 사실 고침) plugin 2.14.1은 받기에 이미 `timeout: None`을 쓴다(`updater.rs:735`). `update.timeout = None` 줄은 빼고 60초 무진행 감시는 둔다. `latest.json`은 JSON이 깨질 때만 모든 플랫폼이 멈추고, 플랫폼 하나가 빠지면 그 플랫폼만 멈춘다(3.4 고침). `ReleaseNotFound`(공개 Release 없음, 롤백 중)는 `feed`로 나누고 자동 확인에서는 조용히 넘어간다. 재확인 결과 문구는 "다른 버전이 나와 다시 받습니다"(낮은 버전으로 되돌린 경우도 맞게).
- (Eng, DX 줄 대체) `update_state`에서 `blocked`를 뺀다(녹음·파일 작업은 화면이 이미 알고, 막힘은 `install_update`의 `blocked{reason}`이 판정한다). 트레이 항목은 기존 `refresh_tray`(`lib.rs:193`) 안에서 다시 만든다(녹음 중 숨김이 그대로 된다).
- (Eng) 자동 확인 주기: 30분마다 깨어나 마지막 시도 시각(`update-state.json`)과의 벽시계 차이가 6시간 이상이면 확인한다(잠자기 동안 tokio 타이머가 멈추기 때문). 설치 직전 재확인 시간 제한은 10초로 줄인다.
- (Eng) `update.rs`는 `unwrap`·패닉 없이 쓰고 기존 독 허용 `lock()`을 쓴다(릴리스 빌드는 패닉에 중단된다).
- (Eng) 6장 0단계 전에 `tccutil reset All com.sije.meeting-summarizer`를 하고, 시험 동안 `tauri dev`를 띄우지 않는다.
- (Eng) 위험 표에 더한다: 업데이터 서명 키가 tauri-action 단계 환경에 있어 크레이트 빌드 스크립트·bun postinstall·llama.cpp CMake가 읽을 수 있다(받아들임, 의존성 갱신은 리뷰). Windows 반쯤 끝난 NSIS 설치(새 exe, 옛 사이드카)는 `just_updated.ok`로 못 잡는다(4단계 확인으로 받아들임).
- (Eng) `slack-release` 수동 실행은 노트 링크를 공개 저장소 주소로 기본 설정한다. 1.2.1 이후 Slack 문구는 "앱에서 업데이트됩니다, 처음 설치만 이 파일로"로 바꾼다.
- (Eng) 테스트 추가: latest 판정 함수, CI `.sig` 검증과 풀린 앱의 codesign·해시 검사, `INSTALL_WAIT > reap`, `shutdown_work` 두 번, 설치·녹음·파일 작업·보내기 깃발 두 순서, 띄우는 중 사이드카, `ReleaseNotFound` 분류, 벽시계 주기, 설치 실패 재시도(`count`), 설치 중 메뉴 종료 미룸, macOS 사전 점검 판정(경로 문자열 입력), 화면 준비 단계(비우는 동안 입력이 막힘, 취소 시 풀림, 설정 저장 중 막힘).
- (Eng 재리뷰, Eng 줄 대체) 준비 예약: 화면은 준비(잠금)에 들어가기 전에 `prepare_update`를 부른다. Rust는 이때 `INSTALLING`을 세우고 녹음·파일 작업·Slack 보내기 수를 본다. 하나라도 있으면 깃발을 내리고 `blocked{reason}`을 돌려주며, 화면은 잠그지 않고 이유를 보인다(비우기를 하지 않는다). 막힘이 없으면 화면이 잠그고 회의 편집을 비운 뒤 `install_update(confirmed)`를 부른다. `install_update`는 이 준비가 세운 깃발이 없으면 `not_ready`다. `needs_confirm`, 저장 실패, 오류면 `cancel_update`로 깃발을 내린다. 깃발이 서 있는 동안 트레이·앱 메뉴의 종료와 `ExitRequested`는 미뤄지고, 준비가 설치 없이 끝나면 미룬 종료를 그때 한다. 화면이 60초 안에 `install_update`나 `cancel_update`를 부르지 않으면 Rust가 깃발을 내린다(화면이 멈춰도 종료를 막지 않게).
- (Eng 재리뷰, Eng 줄 대체) 정리는 한 번만 돈다: `shutdown_work`는 처음 부른 쪽만 실제로 정리하고, 나중에 부른 쪽(설치 시간 초과 뒤 `request_restart`가 부르는 `RunEvent::Exit` 등)은 처음 정리가 끝나기를 남은 마감 시각까지만 기다리고 돌아온다. 설치는 시작할 때 마감 시각 하나(지금 + `INSTALL_WAIT`)를 정하고 정리 전체와 그 뒤의 `Exit`가 함께 쓴다. 마감을 넘기면 막힌 정리를 기다리지 않고 지금 버전으로 다시 시작한다. 녹음 수는 녹음 마무리(`Recorder::stop`의 join, `audio/mod.rs:286`)가 끝날 때까지 쥔다(마무리 중인 녹음은 `blocked{recording}`). 독립 리뷰어가 제안한 Windows 대기·중단 줄이기는 하지 않는다(승인된 결정 유지, 아래 리뷰 기록).
- (Eng 재리뷰) `update_state` 스냅샷과 `app://update` 이벤트에 단조 증가 `rev`를 붙이고, 화면은 가진 것보다 작은 `rev`를 버린다(늦게 온 `ready` 스냅샷이 `installing`을 덮지 않게. `App.tsx:137`의 큐 초기화와 같은 문제).
- (Eng 재리뷰, DX 줄 대체) IPC 모양을 카드에 맞춘다: `downloading{version, received, total?, waiting, origin}`의 `origin`은 `background | manual | replacement`(재확인에서 다른 버전)다. 철회는 `update_state.notice: withdrawn{version} | null`로 싣고 다음 상태가 되거나 화면이 ×로 지울 때까지 둔다. `just_updated`는 `{version, ok, notes}`다. `update-state.json`에 `pending_notes`(설치 직전 적고 결과를 보인 뒤 지움), `first_seen_at`, `attempt{version, id}`를 더한다.
- (Eng 재리뷰, Eng 줄 대체) 설치 실패 재시도: `install_failed{version, count}`는 더 새 버전을 찾을 때만 지운다. 실패 행의 "다시 시도"는 `count < 2`일 때만 보이고, 누르면 기록은 그대로 둔 채 그 버전을 한 번 다시 받는다. 설치 직전 `attempt`를 적고 실패는 그 시도에서 한 번만 센다: 설치 단계에서 본 실패(시간 초과, 교체 실패)를 적으면 `attempt`를 지우고, 다음 시작의 `just_updated.ok == false`는 `attempt`가 남아 있을 때만 센다.
- (Eng 재리뷰, 디자인 줄 대체) 카드 배치: 좁은 창(760px 미만이거나 세로 창)에서는 카드를 아래 스택(`App.tsx:528`의 `flex-col`) 안 첫 자식으로 넣고 왼쪽 정렬한다(스택이 쌓으므로 겹치지 않는다). 넓은 창에서는 `fixed left-6 bottom-6`이고, 카드와 아래 스택 안 막대(저장 막대, 보내기 막대, 독)의 측정한 사각형이 겹치면 좁은 창 배치로 바꾼다(800px 창의 설정 저장 막대, `Settings.tsx:1821`). 화면 아래 여백, 캐럿 보호, 전사본 패널의 아래 경계는 화면마다 따로 고치지 않고, 아래 스택(카드 포함)과 왼쪽 카드의 측정 높이를 `ResizeObserver`로 CSS 변수 하나(`--bottom-stack`)에 담아 쓴다.
- (Eng 재리뷰) 설정 탭 소유: `Settings`는 탭의 처음 값만 받는다(`Settings.tsx:173`). 탭이 바뀔 때 `onTab`으로 App에 알리고, 카드의 "바뀐 것 보기"는 App이 `notesIntent`(일반 탭으로 바꾸고 노트 펼치기)로 Settings에 보낸다. Settings는 다시 마운트하지 않는다(저장하지 않은 입력을 잃는다). 일반 탭 숨김(D13)과 잠긴 동안의 예외(D16)는 App이 아는 지금 탭으로 판정한다.
- (Eng 재리뷰) 준비·설치 중 바깥 진입: 알림 클릭(`App.tsx:202`), 파일 끌어놓기(`App.tsx:314`), 트레이의 화면 이동, 준비 전에 시작해 늦게 끝난 콜백은 `inert`와 단축키 차단을 거치지 않는다. 화면 이동(`go`)과 대화상자 열기 앞에 준비·설치 판정 하나를 두고 그동안 온 요청은 버린다(잠금이 풀린 뒤에 다시 하지 않는다).
- (Eng 재리뷰, 디자인 줄 대체) 오래 확인 못 함 판정: 14일 카드(D15)와 설정의 7일 문구는 이번 실행에서 확인 시도가 실패한 뒤에만 판정한다(시작하자마자 판정하지 않는다. 휴가 뒤 첫 실행의 거짓 경고를 막는다). 서버에 닿은 확인(`ReleaseNotFound` 포함)은 성공으로 센다. 기준 시각은 `last_check_ok`, 없으면 `first_seen_at`(이 기기에서 `update-state.json`을 처음 만든 시각, 깨진 파일에서 되살리면 그 시각)이다. 시계가 뒤로 가 차이가 음수면 0으로 본다.
- (Eng 재리뷰, Eng 줄 대체) 공개 피드 승격 판정은 "지금 latest"가 아니라 이미 공개된 모든 태그와 비교한다: 이 태그가 공개된 어떤 태그보다 높을 때만 `--latest`, 아니면 `--latest=false`. 롤백으로 latest를 낮춰도 늦게 도는 `publish`가 롤백한 릴리스를 다시 올리지 않는다. 공개 뒤 확인은 "가장 높은 공개 태그"가 아니라 이 판정이 기대한 latest(올렸으면 이 태그, 아니면 공개 전 latest)와 `latest.json`의 `version`을 비교한다.
- (Eng 재리뷰) 10·11장 접기(DX 줄)는 1단계의 첫 커밋으로 하고, 접은 뒤 본문에 대체된 옛 문장이 남지 않았는지 찾아 지운다: 5.2의 `Err("busy")`·`Err("recording")`·`update.timeout = None`·"최대 10초", 5.4의 홈·첫 화면 업데이트 줄과 앱 알림 띠, 3.4의 "플랫폼 항목 하나만 깨져도 모든 플랫폼", 10장 첫 CEO 줄의 "넘으면 경고 후 설치를 계속한다". 구현은 접은 본문만 보고 한다.
- (Eng 재리뷰) 깨진 릴리스 막기: (a) 관리자 태그 체크리스트에 "업데이터 경로(`update.rs`, `lib.rs`의 종료·다시 시작, `tauri.conf.json`의 `plugins.updater`, build.yml 서명 단계)가 바뀐 릴리스는 태그 전에 `scripts/updater-test.ts`로 지금 공개 버전에서 새 빌드로 업데이트를 이 Mac에서 한 번 돌린다"를 넣는다. (b) CI 시작 확인: 두 OS 빌드 작업이 만든 앱을 띄워 60초 안에 `app.log`에 버전 줄이 찍히는지 보고 끈다. 찍히지 않으면 태그 빌드는 공개하지 않는다(시작하자마자 죽는 버전은 앱 안에서 고칠 수 없다).
- (Eng 재리뷰, DX 줄 대체) `update-state.json`은 설정 파일 옆(`app_data_dir`, `lib.rs:332`, Windows는 로밍)이 아니라 회의 폴더처럼 `app_local_data_dir`(`lib.rs:325`)에 둔다. 값이 모두 이 기기의 설치에 대한 것이다(로밍 프로필이 다른 PC에 가짜 "업데이트하지 못했습니다"를 띄우지 않게). 쓰기는 `store::write_atomic`(`store.rs:259`).
- (Eng 재리뷰) 키 불일치: CLI는 서명 키와 공개 키가 맞지 않아도 경고만 남긴다(tauri-cli 2.12.2 `bundle.rs`). `publish`는 초안을 공개하기 전에 두 `.sig`를 `tauri.conf.json`의 공개 키로 검증하고 실패하면 공개하지 않는다. `latest-json.ts`는 `.sig`의 trusted comment에 든 `version:`이 태그와 같은지도 본다(`requireSignedVersion` 때문에 다르면 아무도 받지 못한다).
- (Eng 재리뷰, 디자인 줄 대체) Windows 설치가 실패하면 앱이 꺼진 채로 남는다(플러그인은 설치 프로그램을 띄운 뒤 바로 끝난다). Windows 설치 단계 문구는 "설치 창이 잠깐 보인 뒤 앱이 다시 열립니다. 1분이 지나도 열리지 않으면 시작 메뉴에서 여세요"이고 install.md에도 적는다. 마지막 로그는 플러그인의 `on_before_exit`에서 비운다.
- (Eng 재리뷰) 위험 표에 더한다: 0·2단계는 이 Mac 한 대(한 macOS 버전)에서만 본다. 최소 지원 14.6(`tauri.conf.json:45`)부터의 TCC·App Management·Gatekeeper 차이는 4단계와 파일럿에서 본다(받아들임). 2단계 확인 목록에 `xattr -l`의 `com.apple.provenance`를 더한다. 자체 서명에는 Team ID가 없어 키체인 "항상 허용" 창이 업데이트마다 뜰 수 있다: 0단계에서 보고, 뜨면 5.8의 키체인 줄(자체 서명 유지 + 안내)이 기본 경로다.
- (Eng 재리뷰) 모델 내려받기(`download_models`, `commands.rs:454`)도 파일 작업 보호에 넣는다(설치가 2.5GB 받기를 끊지 않게). 막힘 문구는 "파일 작업이 끝나면 업데이트할 수 있습니다"와 같다.
- (Eng 재리뷰) 받는 중 이벤트는 250ms나 1% 단위로 줄여 보낸다. 15초 무진행("연결을 기다리는 중…")은 조각이 오지 않는 것을 봐야 하므로 Rust 타이머로 판정해 `waiting`을 보낸다.
- (Eng 재리뷰, 사실 고침) NSIS 한국어는 이미 `tauri.windows.conf.json:9`(`"languages": ["Korean"]`)에 있어 바꿀 것이 없다(디자인 재리뷰 줄). Release 자산은 이제 `release-assets.githubusercontent.com`으로 넘어간다(2026-11-06 curl 확인). 위험 표와 install.md의 `objects.githubusercontent.com`을 고치고, 방화벽 허용 목록은 `github.com`과 이 주소다.
- (Eng 재리뷰, Eng 줄 대체) macOS 사전 점검은 `/AppTranslocation/`과 `/Volumes/` 두 가지만 본다. 임시 폴더와 같은 볼륨인지는 플러그인이 옮기기 전에 거절하고(`TempDirNotOnSameMountPoint`), 이 오류는 같은 위치 문구로 보인다. 앱 폴더에 쓸 수 없으면 막지 않고 플러그인의 관리자 암호 창에 맡기며 install.md에 적는다(쓸 수 없으면 막는다는 줄과 암호 창 안내가 서로 어긋났다).
- (Eng 재리뷰) 종료 중재는 `ExitRequested`의 `code`가 `tauri::RESTART_EXIT_CODE`(`tauri` 2.12.3 `app.rs:77`)이면 통과시킨다(`request_restart`만 나간다). `update.install(bytes)`는 tar 풀기가 동기라 `spawn_blocking`에서 부른다.
- (Eng 재리뷰, Eng 줄 대체) 자동 확인 주기: 6시간 간격은 마지막 성공(서버에 닿은 확인) 시각으로 잰다. 실패한 시도(네트워크)는 다음 30분 깨움에 다시 한다(잠자기에서 깬 직후의 실패가 6시간을 기다리지 않게).
- (Eng 재리뷰) 관리자 문서: `publish` 동시성 그룹은 대기 작업을 하나만 둔다(세 번째가 오면 앞의 대기 작업이 취소된다). 취소된 `publish`는 "실패한 작업만 다시 실행"으로 다시 돌린다.
- (Eng 재리뷰) 보안: 릴리스 페이지 링크는 피드 값이 아니라 상수(공개 저장소 Releases 주소와 버전)로 만든다. `notes`는 64KB와 200줄에서 자른다. `actions/create-github-app-token`에는 `repositories: meeting-summarizer-releases`로 범위를 준다. 공개되는 바이너리에 든 Slack `CLIENT_ID`는 PKCE라 괜찮지만, Slack 앱이 우리 워크스페이스에만 설치되는지 3단계에서 확인한다.
- (Eng 재리뷰) 테스트 추가: 준비 예약(막히면 잠그지 않음, 저장 대기 중 메뉴 종료 미룸과 준비 취소 뒤 실행, 60초 만료), 정리 한 번(동시 호출, 멈춘 정리 뒤 `Exit`가 마감 안에 끝남), `rev`로 늦은 스냅샷 버림, IPC 모양(Rust와 TS가 같은 JSON fixture를 읽음), 재시도 횟수(실패, 다시 시작, 다시 시도, 실패, 다시 시작이면 `count` 2이고 링크만, 한 시도를 두 번 세지 않음), 카드 배치(760px·800px 가로 창의 저장 막대), 탭 소유(다른 탭에 입력이 있을 때 "바뀐 것 보기"가 일반 탭을 열고 입력을 지킴), 준비 중 알림 클릭·끌어놓기가 화면을 바꾸지 않음, 오래 확인 못 함(휴가 뒤 첫 확인 성공이면 경고 없음, 처음 설치, 시계 되돌림, 13·14·15일), 승격 판정(롤백 뒤 낮은 태그와 높은 태그), `.sig`의 키·버전 불일치, `update-state.json` 위치, CI 시작 확인, 진행 이벤트 줄이기, D16(일반 탭에서 잠긴 동안만 카드와 초점, 잠금이 풀리면 버전 행과 버튼).
<!-- /autoplan-accepted:eng -->

#### Step 0: Scope Challenge

- 이미 있는 것: 종료 확인 흐름(`commands.rs:345-359`, `lib.rs:466-490`, `QUIT_CONFIRMED`), `refresh_tray`(`lib.rs:193`), `Limits::reap`(`sidecar.rs:67`), `busy_ids`(`store.rs:795`), `Exports`, 회의 편집 `saves`·`flush`(`Meeting.tsx:169-178`, `:279`), 앱 알림 띠(`App.tsx:400`), build.yml의 태그 검사(`:49-55`)와 릴리스 작업(`:290-316`), `tauri-plugin-updater` 2.14.1(Rust API).
- 복잡도: 바뀌는 파일 약 20개(Rust 6, 화면 6, CI·스크립트 4, 문서 5), 새 모듈 `update.rs`와 스크립트 2개. 8개 이상이라 복잡도 관문에 걸린다. autoplan 규칙(P2, 범위를 줄이지 않는다)으로 범위는 그대로 두고, 배치는 원래대로(모듈 하나 `update.rs`에 상태·명령·검사를 모으고 나머지는 기존 파일에 붙인다) 자동 결정했다. Scope Challenge 결과: scope accepted as-is.
- 검색: 웹 조사 안 함. 플러그인 소스(2.14.1), tauri 2.12.3, single-instance 2.5.3, CLI 2.12.2를 독립 리뷰어가 직접 읽었다 [Layer 1].
- TODOS: X6(종료할 때 자동 설치), X7(업데이트 준비 데스크톱 알림)은 CEO 단계 그대로. 디자인·DX는 새 TODO 없음. Eng 새 TODO 없음(서명 키 노출과 Windows 반쯤 끝난 설치는 위험 표로 받아들임). TODOS.md 반영은 9장대로 5단계에서 한다.

```
REALITY: premise-table finding 엔진 정리 한도 10초가 설치 대기 10초와 같다(설치가 늘 시간 초과할 수 있음) src-tauri/src/sidecar.rs:67; unverified=4는 CEO 단계 그대로(TCC 유지, 격리, MOTW, 키체인-cdhash 묶임은 0·2단계)
REALITY: already-done pass 업데이터 관련 코드 없음, HEAD d2828c9 이후 커밋 없음 src-tauri/tauri.conf.json:1
REALITY: surface-check pass gh release edit --latest(불 플래그, --latest=false 가능), tccutil reset SERVICE BUNDLE_ID, security set-keychain-settings -lut 확인. requireSignedVersion은 plugin config.rs(독립 리뷰어 확인) .github/workflows/build.yml:292
REALITY: binding-decisions pass U1·U2·U3와 CEO·디자인·DX 수락 줄을 유지하고 Eng 줄은 "(… 대체)"로 시간순 규칙을 따른다 docs/plans/2026-11-05-in-app-updater.md:261
REALITY: numbers pass INSTALL_WAIT 15초 = reap 10초 + 5초 [measured: sidecar.rs:67], 자동 확인 30분 깨움·6시간 간격, 재확인 10초 [estimated] src-tauri/src/sidecar.rs:67
REALITY: deferred-asks pass 요청 3개 모두 처리, X6·X7은 TODOS docs/plans/2026-11-05-in-app-updater.md:27
REALITY: pr-count pass PR 없이 master에 단계별 커밋(기본 정책), 사용자 대상 링크는 3단계 뒤 한 커밋 docs/install.md:7
```

#### 두 목소리 (Step 0.5)

- Claude 독립 Eng 리뷰어: 완료(INPUT `eng fd292bbc…` 일치). 높음 2, 중간 8, 낮음 다수. 플러그인 2.14.1·tauri 2.12.3 소스로 확인. H1 연속 태그가 공개 피드를 옛 버전으로 되돌린다. H2 설치 대기 10초 = 엔진 정리 10초, 실패 기록이 영구. M1 배포 파일 검사 없음. M2 `requireSignedVersion`. M3 키체인과 TCC 실패를 나눔. M4 다시 시작 스레드와 `request_restart`. M5 잠금 둘 교착 대신 원자 깃발. M6 10장 접기. M7 macOS 사전 점검. M8 CI 키체인 자동 잠금.
- Codex(gpt-6-astra, plan-review): 완료(`OUTSIDE_STATUS: completed`, `FINDINGS: P1`). 6/10, 10개(High 7, Medium 3). C1 띄우는 중 사이드카를 못 셈. C2 Slack 보내기와 저장 사이 설치. C3 비우기 동안 입력이 빠져나감. C4 설정 저장 중. C5 파일 작업 보호가 명령 입구에 없음. C6 메뉴 종료가 설치를 끊음. C7 피드 승격 경합(H1과 같음). C8 태그 수동 실행이 공개함. C9 수동 ad-hoc 빌드가 오디오 확인을 건너뜀. C10 대기 시간이 막히는 정리 뒤에 시작.

```
ENG DUAL VOICES — CONSENSUS TABLE:
  Dimension                           Claude  Codex  Consensus
  1. Architecture sound?               예(수명주기 틈) 예(수명주기 틈) CONFIRMED: 구조는 맞고 설치 수명주기를 단단히
  2. Test coverage sufficient?         NO      NO     CONFIRMED(테스트 13종 추가)
  3. Performance risks addressed?      부분    부분   CONFIRMED(잠자기 타이머 / 막히는 정리)
  4. Security threats covered?         NO      언급 없음 Claude만(requireSignedVersion, 배포 파일 검사, 키 노출) → 수용
  5. Error paths handled?              NO      NO     CONFIRMED(영구 실패 기록 / 메뉴 종료·Slack·비우기 틈)
  6. Deployment risk manageable?       NO      NO     CONFIRMED(latest 경합, 태그 수동 실행)
5/6 CONFIRMED, 불일치 0. 단일 목소리 치명 지적 없음.
```

#### 1절. 아키텍처

```
        [화면 App.tsx]──준비(inert, 비우기)──▶ install_update(confirmed) ─┐
          │  ▲ app://update (update_state)                                │
          │  └─────────────────────────────────────────────┐              ▼
   [Settings 버전 행] [Home 업데이트 줄] [트레이 refresh_tray] │      [update.rs]
                                                            │   상태 Mutex · INSTALLING(AtomicBool)
   녹음 시작 / 파일 작업 / Slack 보내기 ── 수 올림 → 깃발 확인 ──┤   check(30분 깨움, 6시간 벽시계)
                                                            │   download(60초 감시) · 재확인(10초)
   update-state.json ◀── 읽기·쓰기 (Rust만) ─────────────────┤   macOS 사전 점검
                                                            │   shutdown_work(두 번 안전)
   종료 중재(ExitRequested) ◀── INSTALLING이면 미룸 ──────────┤   대기 INSTALL_WAIT=reap+5초
                                                            │   install → request_restart
   tauri-plugin-updater 2.14.1 ── HTTPS ──▶ 공개 저장소 latest.json ◀── publish(직렬, semver)
```

- 결합: 업데이터는 녹음기·내보내기·Slack을 직접 잠그지 않고 원자 깃발과 수만 본다(교착 없음).
- 실제 장애 하나씩: 피드 경합(직렬 승격), 엔진이 늦게 꺼짐(reap+5초), 앱이 DMG에서 실행(사전 점검), 프록시 다운그레이드(서명된 버전), 메뉴 종료(중재), 잘못된 서명 키(CI `.sig` 검증).

#### 2절. 코드 품질

- `update.rs` 하나에 상태·명령·판정 순수 함수(latest 판정은 스크립트 쪽, 오류 분류, 오디오 확인, 사전 점검, 벽시계 주기)를 둔다. 순수 함수는 입력·출력만 받아 테스트한다.
- 5.2의 `update.timeout = None`은 이미 플러그인이 한다(사실 고침). 10장 접기(X1)가 5.1 `failed{message}`, 5.2 `Err("busy")`, "설정에 `updated_to`" 같은 옛 문장을 없앤다.
- 공유 코드: `shutdown_work`가 `confirm_quit`·`RunEvent::Exit`·설치의 정리를 하나로 묶는다(CEO 수락). 새 추출은 없다.

#### 3절. 테스트

```
CODE PATHS                                              USER FLOWS
[+] update.rs                                           [+] 설정에서 업데이트
  ├── check (자동/수동, 30분 깨움, 벽시계)                  ├── [GAP] 확인→받기→준비→다시 시작 [→수동 2·4단계]
  │   ├── [GAP] 최신 / 새 버전 / ReleaseNotFound(feed)     ├── [GAP] 처리 중 → needs_confirm → 취소 그대로
  │   └── [GAP] 녹음 중 건너뜀, 실패 기록 버전 건너뜀       └── [GAP] 저장 안 한 설정·저장 중 → 막힘
  ├── download (60초 감시, 크기 모름)  [GAP]              [+] 홈·트레이
  ├── install_update                                       ├── [GAP] 트레이 → 준비 → 비우기 중 입력 막힘
  │   ├── [GAP] not_ready / needs_confirm / blocked{recording|file_work|sending} / started
  │   ├── [GAP] 깃발 두 순서(설치 먼저 / 녹음 먼저)          └── [GAP] 설치 중 메뉴 종료 미룸
  │   ├── [GAP] 재확인: 같음 / 다름 / 네트워크 실패          [+] 업데이트 뒤 첫 실행
  │   ├── [GAP] macOS 사전 점검 4가지                       ├── [GAP] 성공 줄 1회, 실패 줄 + 설정 기록
  │   ├── [GAP] shutdown_work 두 번, 띄우는 중 사이드카      └── [GAP] 오디오 확인 줄(흔적·상수 판정)
  │   └── [GAP] INSTALL_WAIT 넘음 → 설치 안 함 → 다시 시작
  ├── 오류 분류 kind  [GAP]                              [+] CI
  ├── update-state.json 없음/깨짐/모르는 필드/쓰기 실패 [GAP] ├── [GAP] latest 판정(semver)
  └── 설치 실패 기록 재시도(count<2)  [GAP]                 ├── [GAP] .sig 검증, 풀린 앱 codesign·해시
[+] scripts/latest-json.ts  [GAP] 정상/플랫폼 빠짐/빈 서명/  ├── [GAP] 세 파일 버전, insecure 설정, 노트 계약
    버전 불일치/## 바뀐 것만/기호 제거                        └── [GAP] 태그 수동 실행이 공개 안 함
COVERAGE: 0/34 (새 코드라 모두 GAP) | 단위 26, 화면 5, CI 검사 6, 수동 2·4단계 [→E2E]
```

- 모든 GAP는 수락 줄의 테스트 목록(CEO·디자인·DX·Eng)에 들어 있다. 실제 설치·권한·Gatekeeper·SmartScreen은 자동화할 수 없어 0·2·4단계 수동 확인이 E2E다.
- 회귀 위험(기존 동작): 종료 확인(`confirm_quit`)과 `RunEvent::Exit`를 `shutdown_work`로 바꾸면서 내보내기 취소가 종료 확인에도 들어간다. 기존 동작 보존: 종료 대화상자 문구와 진행 중 작업 판정은 그대로, 바뀌는 것은 종료할 때 내보내기도 취소된다는 것 하나. 테스트: `shutdown_work`가 순서대로 부르는지(CEO 수락 줄)와 종료 확인 흐름의 기존 테스트 유지.
- 테스트 계획 파일: `~/.gstack/projects/sijecorp-meeting-summarizer/kangjae4real-master-eng-review-test-plan-20261105.md`.

#### 4절. 성능

- 자동 확인은 30분마다 깨어 시각만 비교한다(네트워크 없음). 받기는 수십 MB 한 번, 메모리에 둔다(dmg 18MB, setup.exe 24MB [measured: 1.1.1]). 녹음 중 자동 확인을 건너뛴다. 막히는 정리는 메인 스레드 밖. 문제 없음.

#### 실패 모드

| 경로 | 실제 장애 | 처리 | 사용자에게 |
|---|---|---|---|
| 피드 승격 | 늦게 끝난 옛 태그가 latest가 됨 | 직렬 + semver + 공개 뒤 확인 | 없음(CI 실패) |
| 설치 대기 | 엔진이 10초 넘게 안 꺼짐 | reap+5초, 넘으면 설치 안 함 | "업데이트하지 못했습니다", 재시도 1회 |
| 비우기 | 비우는 중 새 입력 | 준비 단계가 입력을 막음 | 저장 실패면 멈춤 문구 |
| Slack | 보냈는데 저장 전 종료 | 보내기 수가 설치를 막음 | "Slack 보내기가 끝나면…" |
| 메뉴 종료 | 설치 중 종료 | 종료 미룸 | 없음 |
| 위치 | DMG·다운로드 폴더에서 실행 | 사전 점검 | "앱에서 설치할 수 없는 위치입니다…" |
| 프록시 | latest.json 바꿔치기 | requireSignedVersion | 서명 오류 문구 |
| 서명 키 | 잘못된 키로 릴리스 | CI `.sig` 검증 | 없음(CI 실패) |
| 키체인 | 업데이트마다 키체인 창 | 5.8 나눔, "항상 허용" 안내 | 안내 줄 |

치명 틈(테스트 없음 + 처리 없음 + 조용한 실패): 0개. 남는 조용한 위험은 Windows 반쯤 끝난 설치(4단계 확인으로 받아들임).

#### NOT in scope (Eng)

- 서명을 `publish` 작업으로 옮겨 빌드 스크립트에서 키를 숨기기: 산출물 실행 권한이 사라져 어렵다. 위험 표로 받아들인다.
- 설치 직전 재확인 없애기(독립 리뷰어 제안): CEO 단계가 넣은 안전장치라 유지하고 시간만 10초로 줄인다.
- Windows 반쯤 끝난 NSIS 설치 감지: 4단계 확인으로 받아들인다.

#### What already exists (Eng)

- 종료 확인 흐름과 `QUIT_CONFIRMED`(`commands.rs:345-359`), `RunEvent` 처리(`lib.rs:466-490`), `refresh_tray`(`lib.rs:193`), `Limits::reap`(`sidecar.rs:67`), `busy_ids`(`store.rs:795`), `Exports`, 독 허용 `lock()`, 회의 `saves`·`flush`, 앱 알림 띠, build.yml 태그 검사. 플러그인은 macOS 교체 실패 시 백업 복구, Windows 설치 전 `cleanup_before_exit`를 이미 한다.

#### Worktree parallelization strategy

| Step | Modules touched | Depends on |
|---|---|---|
| 0단계 실측 | 없음(로컬 빌드) | — |
| A. Rust 업데이터 | `src-tauri/src/` | 0단계 |
| B. 화면 | `src/` | A의 IPC 타입 |
| C. CI·스크립트 | `.github/`, `scripts/` | — |
| D. 문서 | `docs/`, README | C의 명령 |

Lane A: A → B(공유 IPC 타입) / Lane C: C → D. 실행 순서: 0단계 끝난 뒤 A와 C를 함께, A 다음 B, C 다음 D. 충돌: `src/lib/ipc.ts`(A·B), `docs/install.md`(D만).

#### Implementation Tasks (Eng)

- [ ] **E1 (P1, human: ~3h / CC: ~15min)** — CI — `publish` 직렬화, semver latest 판정, 공개 뒤 확인, push 이벤트 조건
  - Surfaced by: H1, C7, C8
  - Files: `.github/workflows/build.yml`, `scripts/latest-json.ts`
  - Verify: latest 판정 단위 테스트, 태그 수동 실행이 공개 안 함
- [ ] **E2 (P1, human: ~1d / CC: ~40min)** — Rust — `INSTALLING` 깃발 손잡기, 파일 작업·Slack 보내기 RAII 수, 띄우는 중 사이드카, 종료 중재
  - Surfaced by: M5, C1, C2, C5, C6
  - Files: `src-tauri/src/update.rs`, `commands.rs`, `slack.rs`, `sidecar.rs`, `lib.rs`
  - Verify: 두 순서 테스트, 설치 중 메뉴 종료 테스트
- [ ] **E3 (P1, human: ~4h / CC: ~20min)** — Rust — `INSTALL_WAIT`, 막히는 정리 분리, `request_restart`, 비동기 명령, `shutdown_work` 두 번, 실패 재시도
  - Surfaced by: H2, M4, C10
  - Files: `src-tauri/src/update.rs`, `commands.rs`, `lib.rs`
  - Verify: Rust 테스트
- [ ] **E4 (P1, human: ~4h / CC: ~20min)** — 화면 — 준비 단계(inert, 비우기, 다시 비우기), 설정 저장 중 막힘
  - Surfaced by: C3, C4
  - Files: `src/App.tsx`, `src/screens/Meeting.tsx`, `src/screens/Settings.tsx`
  - Verify: 화면 테스트
- [ ] **E5 (P1, human: ~3h / CC: ~15min)** — 보안·검사 — `requireSignedVersion`, `.sig` 검증, 풀린 앱 codesign·해시, URL HEAD, MS_UPDATER는 인증서 필수, CI 키체인 잠금 설정
  - Surfaced by: M1, M2, M8, C9
  - Files: `src-tauri/tauri.conf.json`, `.github/workflows/build.yml`
  - Verify: 수동 실행 빌드, 2단계
- [ ] **E6 (P2, human: ~3h / CC: ~15min)** — Rust — macOS 사전 점검, 벽시계 주기, `ReleaseNotFound` 분류, 재확인 10초, 트레이는 `refresh_tray`, 패닉 없음
  - Surfaced by: M7, LOW
  - Files: `src-tauri/src/update.rs`, `lib.rs`
  - Verify: 순수 함수 테스트
- [ ] **E7 (P2, human: ~1h / CC: ~5min)** — 계획·문서 — 5.8 첫 줄 나누기, 0단계 `tccutil reset`, 위험 표 두 줄, slack-release 기본 링크·문구
  - Surfaced by: M3, LOW
  - Files: `docs/plans/2026-11-05-in-app-updater.md`, `docs/install.md`, `.github/workflows/slack-release.yml`
  - Verify: 읽기

#### Completion Summary (Eng)

- Step 0: Scope Challenge — scope accepted as-is(복잡도 관문은 autoplan P2로 원래 배치)
- Reality rows: 7/7, unverified 4(CEO 단계와 같음, 0·2·4단계가 확인)
- Architecture Review: 6 issues(H1, M4, M5, C1, C6, C7)
- Code Quality Review: 2 issues(사실 고침, 10장 접기)
- Test Review: 다이어그램 작성, 34 gaps(모두 수락 줄의 테스트로 계획됨)
- Performance Review: 1 issue(잠자기 타이머)
- NOT in scope: written / What already exists: written
- TODOS.md updates: 0 새 항목(X6·X7은 CEO 단계, 5단계에서 반영)
- Failure modes: 0 critical gaps
- Unresolved decisions: 0
- Outside voice: codex completed
- Parallelization: 2 lanes, 2 parallel / 2 sequential
- Lake Score: N/A(질문 없이 자동 결정)
- 결정 26개 자동(감사 110~135), TASTE 0, User Challenge 0.

### 디자인 재리뷰 (Phase 2 재실행, 관문 d1 뒤, 2026-11-05)

<!-- autoplan-baseline-edits:design {"sourceSha256":"5c53f2c439f0e059f71ac33ccf0d94dc6b72aa5a9b5f52ff74b56f9c591738b0","replacements":[{"oldText":"- d1 업데이트 알림은 창 왼쪽 아래에 떠 있는 카드로 보인다. 잠깐 나왔다 사라지는 토스트가 아니라, 닫거나 업데이트할 때까지 남는다.\n  - 녹음 화면을 뺀 모든 화면(홈, 첫 화면, 회의 문서, 설정)에 보인다. 녹음 화면에서는 숨긴다.\n  - 준비 카드: \"v1.2.2 버전이 준비되었습니다\", 그 아래 \"다시 시작해 업데이트\" 버튼. 버전 숫자 뒤에는 \"버전\"을 끼운다.\n  - 닫기(×)가 있다. 닫으면 이번 실행 동안 숨기고, 앱을 다시 켜면 다시 보인다. 닫아도 설정의 버전 행과 트레이 메뉴에서 업데이트할 수 있다.\n  - 업데이트 뒤 결과도 같은 자리, 같은 카드로 보인다: 성공 \"v1.2.2 버전으로 업데이트했습니다\"와 \"바뀐 것\", 실패 \"업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다\"와 \"릴리스 페이지 열기\". 준비 카드가 있으면 준비 카드가 먼저다.\n  - 디자인 리뷰의 홈·첫 화면 \"업데이트 줄\"(준비 줄과 결과 줄)은 모두 이 카드로 바뀐다. 홈과 첫 화면의 줄은 오디오 확인 줄, 설정 줄 순서다(d9).\n  - 막힘 이유(녹음, 파일 작업, Slack 보내기, 저장하지 않은 설정)와 설치 단계 문구는 5.4 표와 10장의 문구를 카드에 그대로 쓴다.\n  - 미리보기와 화면 테스트의 \"업데이트 줄\" 장면은 카드 장면(준비, 막힘, 설치 중, 성공, 실패, 닫은 뒤, 회의 문서 화면 위)으로 바꾼다.","newText":"- d1 업데이트 알림은 창 왼쪽 아래에 떠 있는 카드로 보인다. 잠깐 나왔다 사라지는 토스트가 아니라, 닫거나 업데이트할 때까지 남는다. 위치, 겹침, 모양, 상태, 닫기, 초점의 세부는 10장의 \"(디자인 재리뷰)\" 줄을 따른다(이 장과 같은 결정이다).\n  - 녹음 화면과 설정의 일반 탭에서는 숨긴다(D13). 녹음 중에는 어느 화면에서든 준비 카드를 숨긴다(D12, 트레이 항목과 같다). 그 밖의 화면(홈, 첫 화면, 회의 문서, 설정의 다른 탭)에 보인다.\n  - 준비 카드: \"1.2.2 버전이 준비되었습니다\"(D9, v 없이), 그 아래 \"다시 시작해 업데이트\" 테두리 버튼(outline sm, D10). 노트가 있으면 \"바뀐 것 보기\" 링크(D14): 설정 일반의 버전 행으로 가서 노트를 펼친다.\n  - 창이 좁으면(760px 미만이거나 세로 창) 카드는 아래쪽 막대(독, 보내기 막대, 저장 막대) 바로 위에 왼쪽 정렬로 올라간다(D11). 넓은 창에서는 왼쪽 아래다.\n  - 닫기(×)가 있다. 닫으면 이번 실행(앱 프로세스) 동안 숨기고, 앱을 다시 켜면 다시 보인다. 닫아도 설정의 버전 행과 트레이 메뉴에서 업데이트할 수 있다.\n  - 업데이트 뒤 결과도 같은 자리, 같은 카드로 보인다: 성공 \"1.2.2 버전으로 업데이트했습니다\"와 \"바뀐 것\", 실패 \"업데이트하지 못했습니다. 지금 버전으로 계속 쓸 수 있습니다\"와 \"릴리스 페이지 열기\". 카드 자리는 하나이고 준비 카드가 먼저다.\n  - 14일 넘게 성공한 확인이 없으면 실행마다 한 번, 닫을 수 있는 카드로 \"업데이트를 확인하지 못하고 있습니다\"와 \"릴리스 페이지 열기\"를 보인다(D15).\n  - 디자인 리뷰의 홈·첫 화면 \"업데이트 줄\"(준비 줄과 결과 줄)은 모두 이 카드로 바뀐다. 홈과 첫 화면의 줄은 오디오 확인 줄, 설정 줄 순서다(d9).\n  - 막힘 이유(녹음, 파일 작업, Slack 보내기, 저장하지 않은 설정)와 설치 단계 문구는 5.4 표와 10장의 문구를 카드에 쓴다.\n  - 미리보기와 화면 테스트의 \"업데이트 줄\" 장면은 카드 장면으로 바꾼다(10장 \"(디자인 재리뷰)\" 줄의 목록)."}]} -->

- 왜: 최종 관문에서 사용자가 d1을 바꿨다(홈 한 줄 → 왼쪽 아래 떠 있는 카드). autoplan 규칙대로 디자인 → Eng를 다시 돈다(사용자 D7). 1차 디자인 기준 편집 기록은 위 디자인 리뷰 절의 보관 블록으로 옮겼다.
- Claude 독립 디자인 리뷰어: 완료(INPUT `design bc58db48…` 일치). 치명 1, 높음 5, 중간 7. C1 좁은 창에서 카드가 아래 스택(독·보내기·저장 막대)과 전사본 패널, 본문 끝을 덮는다. H1 시각·접근성 사양 없음. H2 닫기 뒤 트레이로 누르면 막힘 이유가 안 보이는 막다른 길. H3 5.4 표에 카드 열과 행이 없다. H4 설치 중 문구가 세 곳, `inert`가 초점을 잃게 한다. H5 재확인 뒤 카드가 사라진다. M1 "v…버전" 겹침. M2 결과 카드 "바뀐 것"이 설치 파일 표로 간다. M3 녹음 중 다른 화면의 카드. M4 설정 일반 탭 중복. M5 14일 확인 실패. M6 NSIS 설치 창이 영어. M7 대화상자 버튼 문구.
- Codex(gpt-6-astra): 처음 시도는 사용 한도로 실패(사용자 D8로 16:14 재설정까지 기다림), 다시 실행해 완료(`OUTSIDE_STATUS: completed`). 7/10, 6개(High 3, Medium 3): 1 하단 UI와 충돌, 2 `inert`·초점·포털·전역 단축키, 3 카드 상태 계약, 4 닫기 수명, 5 노트 링크, 6 시각 사양(버튼 outline sm 권고).

```
DESIGN OUTSIDE VOICES — LITMUS SCORECARD (재리뷰):
  Check                                    Claude  Codex  Consensus
  1. Brand unmistakable in first screen?   N/A     N/A    N/A(기존 화면에 카드만 더함)
  2. One strong visual anchor?             NO      NO     CONFIRMED 위험: 카드가 보내기·녹음과 경쟁 → outline sm(D10), 좁은 창 배치(D11)
  3. Scannable by headlines only?          YES     —      NOT SPEC'D(Codex 언급 없음)
  4. Each section has one job?             NO      NO     CONFIRMED: 설정 일반 탭 중복(D13), 단계 문구 세 곳 → 카드 하나
  5. Cards actually necessary?             YES     YES    CONFIRMED: 사용자 결정(d1), 떠 있는 카드 하나
  6. Motion improves hierarchy?            —       —      dur.small 한 번, 반복 애니메이션 없음
  7. Premium without decorative shadows?   YES     YES    shadow-float 기존 토큰만
  Hard rejections triggered:               없음    없음   CONFIRMED 없음
```

| 패스 | 처음 | 고친 뒤 | 무엇이 바뀌었나 |
|---|---|---|---|
| 1. 정보 구조 | 5 | 8 | 카드 자리 하나, 준비 먼저, 녹음 중·일반 탭 숨김, 좁은 창은 아래 스택 위 |
| 2. 상태 | 5 | 8 | 카드 열이 있는 한 표, 빠진 행 6개, 재확인 연속성, 철회 문구 유지 |
| 3. 여정 | 6 | 8 | 닫기 뒤 막다른 길 제거, 노트 링크, 결과 노트는 카드 안, 14일 알림 |
| 4. AI 티 | 7 | 8 | 경고색·반복 애니메이션 없음, 기존 토큰 |
| 5. 디자인 시스템 | 4 | 8 | surface·rounded-float·shadow-float·outline sm, 6.2 문장 고침과 새 절 |
| 6. 좁은 창·접근성 | 3 | 8 | 760px 기준 배치, 여백, 비모달 영역, 초점 규칙, 전역 단축키 차단 |
| 7. 남은 결정 | 사용자 결정 7개(D9~D15), 자동 결정 13개, 미룸 0 | | |

- 두 목소리가 같이 본 것: 하단 충돌, 설치 중 초점·잠금, 카드 상태 계약, 닫기 수명, 노트 접근, 시각 사양(5/7 litmus 일치, 버튼 강조만 갈림 → 사용자 D10). Claude만: 녹음 중 숨김(D12), 일반 탭(D13), 14일 알림(D15), v 표기(D9), NSIS 한국어, 대화상자 버튼, 결과 노트.
- NOT in scope: 카드 위치를 사용자가 옮기는 기능, 데스크톱 알림(X7 그대로).
- What already exists: 아래 스택(`App.tsx:528`), 알림 띠(`App.tsx:400`), `Banner` 닫기, 전사본 패널, `rounded-float`·`shadow-float`·`dur.small` 토큰.
- TODOS: 새 항목 없음.
- Completion: 전체 3/10 → 8/10(가장 낮은 패스 기준). Unresolved Decisions: 없음.

#### Implementation Tasks (Design 재리뷰)

- [ ] **R1 (P1, human: ~1d / CC: ~40min)** — 카드 — 위치(760px 기준), 모양, 접근성, 닫기 수명, 녹음 중·일반 탭 숨김
  - Files: `src/App.tsx`, 새 `src/components/UpdateCard.tsx`
- [ ] **R2 (P1, human: ~4h / CC: ~20min)** — 설치 중 — 카드만 단계 문구, inert 범위·포털·전역 단축키, 초점 이동
  - Files: `src/App.tsx`, `src/screens/MeetingParts.tsx`
- [ ] **R3 (P1, human: ~3h / CC: ~15min)** — 상태 표 — 5.4 한 표(카드 열, 빠진 행 6개)
  - Files: `docs/plans/2026-11-05-in-app-updater.md`
- [ ] **R4 (P2, human: ~2h / CC: ~10min)** — 노트 — 준비 카드 링크, 결과 카드 안 펼침, `update-state.json` notes
  - Files: `src/components/UpdateCard.tsx`, `src-tauri/src/update.rs`
- [ ] **R5 (P2, human: ~1h / CC: ~5min)** — Windows·문구 — NSIS Korean, 대화상자 버튼, 단계 문구
  - Files: `src-tauri/tauri.conf.json`, `src/App.tsx`
- [ ] **R6 (P2, human: ~3h / CC: ~15min)** — 미리보기·디자인 시스템 — 카드 장면과 새 절, 6.2 고침
  - Files: `src/preview/`, `docs/design-system.md`

### Eng 재리뷰 (Phase 3 재실행, 디자인 재리뷰 뒤, 2026-11-06)

<!-- autoplan-baseline-edits:eng {"sourceSha256":"67d5386f16915db0621f8d837f69b79d442e75b622776785f9c9a8a8812f763d","replacements":[{"oldText":"  - 녹음 화면과 설정의 일반 탭에서는 숨긴다(D13). 녹음 중에는 어느 화면에서든 준비 카드를 숨긴다(D12, 트레이 항목과 같다). 그 밖의 화면(홈, 첫 화면, 회의 문서, 설정의 다른 탭)에 보인다.","newText":"  - 녹음 화면과 설정의 일반 탭에서는 숨긴다(D13). 녹음 중에는 어느 화면에서든 준비 카드를 숨긴다(D12, 트레이 항목과 같다). 그 밖의 화면(홈, 첫 화면, 회의 문서, 설정의 다른 탭)에 보인다. 다만 사용자가 업데이트를 눌러 앱이 잠긴 동안(준비·설치 단계)에는 설정 일반 탭에서도 카드를 보이고 초점을 카드 상태 글로 옮긴다(D16). 잠금이 풀린 뒤의 막힘·실패 문구는 일반 탭이면 버전 행에 보이고 초점은 누른 버튼으로 돌아간다."}]} -->

- 왜: 최종 관문에서 d1이 떠 있는 카드로 바뀌었고 디자인 재리뷰가 카드 줄 13개를 더했다. 사용자 D7대로 디자인 다음 Eng를 다시 돈다. 1차 Eng 수락 줄은 그대로 두고, 바뀐 줄은 "(Eng 재리뷰, … 대체)"로 적는다.
- 사용자 결정: D16(설정 일반 탭에서 업데이트를 누르면 잠긴 동안만 카드를 보인다)을 11장 d1에 더했다(위 기준 편집 기록).

#### Step 0: Scope Challenge

- 이미 있는 것: 1차 Eng의 목록에 더해, 큐 초기화의 이벤트 우선 처리(`App.tsx:137`), 아래 스택 `flex-col`(`App.tsx:528`), `store::write_atomic`(`store.rs:259`), 회의 폴더용 `app_local_data_dir`(`lib.rs:325`), Windows Job Object(`sidecar.rs:306-324`), NSIS 한국어(`tauri.windows.conf.json:9`), `tauri::RESTART_EXIT_CODE`(`tauri` 2.12.3 `app.rs:77`).
- 복잡도: 바뀌는 파일은 1차와 같은 약 20개에 새 `src/components/UpdateCard.tsx` 하나. 8개 이상이라 복잡도 관문에 걸리고, autoplan 규칙(P2, 범위를 줄이지 않는다)으로 범위와 배치(`update.rs` 하나에 모으고 나머지는 기존 파일에 붙임)를 그대로 둔다. Scope Challenge 결과: scope accepted as-is.
- 검색: 웹 조사 안 함. 독립 리뷰어가 crates.io의 tauri-plugin-updater 2.14.1, tauri-cli 2.12.2, tauri-bundler 2.10.2와 로컬 tauri 2.12.3 소스를 읽었다 [Layer 1]. Release 자산 주소는 curl로 확인했다.
- TODOS: 새 항목 없음. X6·X7은 CEO 단계 그대로.

```
REALITY: premise-table finding 디자인 재리뷰의 "NSIS Korean으로 바꾼다"는 이미 있는 값(사실 고침) src-tauri/tauri.windows.conf.json:9; 설정 탭은 처음 값만 받는다 src/screens/Settings.tsx:173; 설정은 로밍 폴더 src-tauri/src/lib.rs:332; unverified=6(CEO 단계 4 + NSIS Restart Manager가 같은 exe를 닫는다는 것(독립 리뷰어가 bundler utils.nsh:35-47을 읽음, 내가 다시 읽지 않음) + 키체인 창이 업데이트마다 뜨는지(0단계))
REALITY: already-done pass origin/master에 2026-11-05 이후 커밋 없음(d2828c9), 업데이터 코드 없음 src-tauri/tauri.conf.json:1
REALITY: surface-check pass tauri::RESTART_EXIT_CODE 확인 ~/.cargo/registry tauri-2.12.3/src/app.rs:77; TempDirNotOnSameMountPoint·on_before_exit·tauri signer sign --app-version은 독립 리뷰어가 2.14.1·2.12.2 소스로 확인; Release 자산은 release-assets.githubusercontent.com으로 넘어감(curl 2026-11-06) .github/workflows/build.yml:292
REALITY: binding-decisions pass 사용자 D9~D16과 U1~U3 유지. 승인된 Windows 설치 대기·중단(CEO 두 목소리 대체 줄)을 줄이자는 독립 리뷰어 제안은 한 목소리뿐이라 받지 않았다 docs/plans/2026-11-05-in-app-updater.md:300
REALITY: numbers pass 넓은 창 카드 x=24~344px [measured: left-6 + max-w-80] src/App.tsx:528; 최소 macOS 14.6 [measured] src-tauri/tauri.conf.json:45; 준비 예약 60초 [estimated] docs/plans/2026-11-05-in-app-updater.md:392; 시작 확인 60초 [estimated] docs/plans/2026-11-05-in-app-updater.md:403; 진행 이벤트 250ms·1% [estimated] docs/plans/2026-11-05-in-app-updater.md:409; 노트 64KB·200줄 [estimated] docs/plans/2026-11-05-in-app-updater.md:415
REALITY: deferred-asks pass 사용자 요청 3개와 관문 결정(d1~d9, D9~D16) 모두 반영, 빠진 것 없음 docs/plans/2026-11-05-in-app-updater.md:401
REALITY: pr-count pass PR 없이 master에 단계별 커밋(기본 정책), 바뀐 것 없음 docs/install.md:7
```

#### 두 목소리 (Step 0.5)

- Claude 독립 Eng 리뷰어: 완료(INPUT `eng d2c81606…` 일치). 높음 2, 중간 10, 낮음 10, 뺄 것 4. H1 본문·10장·11장이 서로 덮어써 옛 문장이 남는다. H2 시작하자마자 죽는 릴리스 하나가 모두를 손 설치로 몬다. M1 Windows 설치 전 대기·중단은 NSIS Restart Manager와 Job Object가 대신하니 줄이자(승인된 결정을 뒤집음). M2 휴가 뒤 거짓 경고. M3 `update-state.json`이 로밍 폴더. M4 서명 단계를 빌드에서 떼기. M5 키 불일치를 CLI가 경고만 함. M6 Windows 설치 실패 시 앱이 꺼진 채. M7 macOS 한 버전만 시험. M8 모델 내려받기가 보호 밖. M9 진행 이벤트 폭주. M10 카드 배치를 화면마다 고치면 깨지기 쉽다.
- Codex(gpt-6-astra, plan-review): 완료(`OUTSIDE_STATUS: completed`, `FINDINGS: P1`). 11개(High 2, Medium 9). C1 준비 중(깃발 전) 메뉴 종료가 저장 전에 앱을 닫는다. C2 일반 탭에서 카드를 숨기면 설치 진행을 보일 곳이 없다. C3 시간 초과 뒤 `Exit`가 막힌 정리를 다시 기다린다. C4 스냅샷과 이벤트 순서. C5 IPC가 카드 상태를 담지 못한다. C6 재시도 횟수가 지워져 끝나지 않는다. C7 800px에서 카드가 저장 막대와 겹친다. C8 설정 탭을 App이 모른다. C9 알림 클릭·끌어놓기가 잠금을 우회한다. C10 14일 판정의 기준 시각이 없다. C11 공개 뒤 확인이 롤백과 어긋난다.

```
ENG DUAL VOICES — CONSENSUS TABLE (재리뷰):
  Dimension                           Claude  Codex  Consensus
  1. Architecture sound?               NO      NO     CONFIRMED: 수명주기 틈(준비 예약, 정리 한 번, rev, IPC 모양)
  2. Test coverage sufficient?         NO      NO     CONFIRMED: 테스트 16종 추가
  3. Performance risks addressed?      NO      언급 없음 Claude만(진행 이벤트 폭주) → 수용
  4. Security threats covered?         NO      언급 없음 Claude만(키 불일치, 링크 상수, 토큰 범위) → 수용, 서명 분리는 받지 않음
  5. Error paths handled?              NO      NO     CONFIRMED: 재시도 횟수, Windows 설치 실패, 오래 확인 못 함
  6. Deployment risk manageable?       NO      NO     CONFIRMED: 깨진 릴리스(H2) / 승격 확인과 롤백(C11)
4/6 CONFIRMED, 불일치 0. 단일 목소리 치명 지적 없음(H2는 Claude만이지만 C11과 같은 배포 차원).
```

#### 1절. 아키텍처

```
  [카드 UpdateCard] [설정 버전 행] [트레이 refresh_tray]
        │ 누름            │               │ app://update-install-requested
        └───────┬─────────┴───────────────┘
                ▼
   prepare_update ──▶ [update.rs] INSTALLING 세움, 녹음·파일 작업·보내기 수 확인
        │ blocked{reason} → 잠그지 않고 이유(일반 탭이면 버전 행)
        ▼ 통과
   화면 잠금(inert, 바깥 진입 판정, 단축키 무시) → 카드에 단계 문구(일반 탭도, D16)
        │ 회의 편집 비우기 (실패 → cancel_update)
        ▼
   install_update(confirmed) ── needs_confirm → cancel_update → 대화상자
        │ started
        ▼
   재확인(10초) → shutdown_work(한 번, 마감 = 지금 + INSTALL_WAIT)
        │ 마감 넘음 → request_restart(Exit도 같은 마감)
        ▼
   spawn_blocking(install) → request_restart (RESTART_EXIT_CODE만 통과)

   update_state / app://update ── {rev, state(origin), notice, just_updated{notes}} ──▶ 화면(작은 rev 버림)
   update-state.json (app_local_data_dir) ◀── Rust만: pending_notes, attempt, first_seen_at, install_failed
   publish(직렬) ── 승격: 공개된 모든 태그보다 높을 때만 latest ── 공개 전 .sig 검증 ── CI 시작 확인
```

- 결합: 카드·설정·트레이가 모두 `prepare_update` 하나로 들어가고, 종료 중재는 `INSTALLING` 하나만 본다. 화면 바깥 진입은 `go`와 대화상자 앞의 판정 하나가 막는다.
- 실제 장애 하나씩: 저장 중 메뉴 종료(준비 예약), 멈춘 정리(마감 하나), 늦은 스냅샷(rev), 로밍 프로필(로컬 폴더), 롤백 뒤 늦은 publish(공개 태그 비교), 시작 직후 죽는 릴리스(CI 시작 확인과 태그 전 시험).

#### 2절. 코드 품질

- 카드 배치는 화면마다 여백을 고치지 않고 측정 높이 하나(`--bottom-stack`)로 모은다(M10·C7). 설정 탭 상태는 Settings가 갖고 App에 알리기만 한다(다시 마운트 없음).
- 10장이 길어 옛 문장이 남는 위험(H1)은 DX 줄의 접기를 1단계 첫 커밋으로 못 박고 찾아 지울 문장 목록을 적어 막는다.
- 공유 코드: 새 추출 없음. `shutdown_work` 하나와 준비 예약 하나로 진입점을 모은다.

#### 3절. 테스트

```
CODE PATHS (재리뷰에서 더한 것)                         USER FLOWS
[+] update.rs                                          [+] 카드에서 업데이트
  ├── prepare_update                                     ├── [GAP] 막힘 → 잠그지 않고 이유
  │   ├── [GAP] blocked(녹음·파일·보내기·모델 받기)        ├── [GAP] 저장 중 메뉴 종료 → 미룸 → 취소 뒤 종료
  │   ├── [GAP] cancel_update / 60초 만료                  └── [GAP] 준비 중 알림 클릭·끌어놓기 무시
  │   └── [GAP] 준비 없이 install_update → not_ready     [+] 설정 일반 탭에서 업데이트(D16)
  ├── shutdown_work 한 번 [GAP] 동시 호출, 멈춘 정리 + Exit ├── [GAP] 잠긴 동안 카드와 초점
  ├── rev [GAP] 늦은 스냅샷 버림                           └── [GAP] 풀린 뒤 버전 행과 버튼 초점
  ├── 재시도 attempt/count [GAP] 두 번 실패 → 링크만      [+] 다른 탭에서 "바뀐 것 보기"
  ├── 오래 확인 못 함 [GAP] 휴가 뒤, 처음 설치, 시계 되돌림  └── [GAP] 일반 탭 열림, 노트 펼침, 입력 남음
  ├── 자동 확인 실패 → 30분 뒤 재시도 [GAP]              [+] 좁은·넓은 창
  └── 진행 이벤트 줄이기, 15초 waiting 타이머 [GAP]          └── [GAP] 760·800px 저장 막대와 카드
[+] latest-json.ts [GAP] .sig 키·버전 불일치             [+] 업데이트 뒤 첫 실행
[+] publish 승격 [GAP] 롤백 뒤 낮은·높은 태그              └── [GAP] 결과 카드 노트(pending_notes)
[+] CI 시작 확인 [GAP] 60초 안 버전 줄
[+] IPC fixture [GAP] Rust와 TS가 같은 JSON
COVERAGE: 0/22 (새 코드라 모두 GAP) | 단위 13, 화면 7, CI 2
```

- 모든 GAP는 "(Eng 재리뷰) 테스트 추가" 줄에 있다. 1차 Eng의 34개 GAP와 겹치지 않는다.
- 회귀 위험: 카드를 아래 스택 안에 넣고 여백을 측정 변수로 바꾸면 기존 회의 문서의 캐럿 보호와 전사본 패널 경계가 바뀐다. 지킬 동작: 카드가 없을 때의 여백과 경계는 지금과 같다. 테스트: 카드가 없을 때 `--bottom-stack`이 지금 스택 높이와 같고 기존 화면 테스트가 그대로 통과.
- 테스트 계획 파일: `~/.gstack/projects/sijecorp-meeting-summarizer/kangjae4real-master-eng-review-test-plan-20261106-093529.md`(1차 파일에 재리뷰 항목을 더한 새 파일).

#### 4절. 성능

- 받는 중 이벤트: 수십 MB를 64KB 안팎 조각으로 받으면 수백 번 emit하고 그만큼 화면을 다시 그린다 [estimated]. 250ms나 1% 단위로 줄인다(M9). 그 밖에 반복 경로 없음.

#### 실패 모드 (재리뷰에서 더한 것)

| 경로 | 실제 장애 | 처리 | 사용자에게 |
|---|---|---|---|
| 준비 | 저장 중 메뉴 종료 | 준비 예약이 종료를 미룸 | 없음(저장 뒤 종료) |
| 준비 | 화면이 멈춤 | 60초 만료로 깃발 내림 | 종료가 다시 된다 |
| 정리 | 정리가 멈춤 | 마감 하나, `Exit`도 같은 마감 | "업데이트하지 못했습니다" |
| 상태 | 늦은 스냅샷 | `rev` | 없음 |
| 재시도 | 같은 버전 계속 실패 | `count` 유지, 두 번째부터 링크 | 링크 |
| 로밍 | 다른 PC의 기록 | 로컬 폴더 | 없음 |
| 피드 | 롤백 뒤 늦은 publish | 공개 태그 비교 | 없음 |
| 릴리스 | 시작 직후 죽는 버전 | CI 시작 확인, 태그 전 시험 | 없음(공개 안 됨) |
| Windows | 백신이 설치를 막음 | 단계 문구에 1분 안내 | "1분이 지나도 열리지 않으면…" |
| 키 | 다른 키로 서명 | 공개 전 `.sig` 검증 | 없음(공개 안 됨) |

치명 틈(테스트 없음 + 처리 없음 + 조용한 실패): 0개. Windows 설치가 백신에 막히면 앱이 꺼진 채라 조용하지만, 단계 문구가 미리 알리고 4단계에서 본다.

#### 받지 않은 제안

- Windows 설치 전 대기·중단 줄이기(독립 리뷰어 M1): CEO 두 목소리 대체 줄로 승인된 결정이고 한 목소리만 제안했다. Windows는 4단계 전에는 시험하지 않으므로(d4) 안전장치를 둔다.
- 서명 단계를 빌드에서 떼기(M4): 빌드 스크립트가 악성이면 키 없이도 바이너리 자체를 바꿀 수 있어 얻는 것이 적고, macOS `.app.tar.gz`를 손으로 묶는 단계가 는다. 1차 Eng의 "받아들임"을 유지한다.
- 설치 직전 재확인·`count`·두 기준(7일, 14일) 빼기(YAGNI): 모두 앞 단계와 사용자 관문에서 승인된 동작이라 유지한다(`count`는 C6대로 고친다).
- 초안으로 두고 관리자가 손으로 공개(H2 대안): 릴리스마다 사람 단계가 는다. 태그 전 시험과 CI 시작 확인으로 대신한다.
- 4단계에 다른 macOS 버전 Mac 더하기(M7): 승인된 조용한 시작의 기기 구성을 바꾸지 않고 위험 표에 둔다.

#### NOT in scope (Eng 재리뷰)

- 카드 위치를 사용자가 옮기는 기능, 데스크톱 알림(X7 그대로).
- Windows 반쯤 끝난 NSIS 설치 감지(1차 Eng대로 4단계 확인).

#### What already exists (Eng 재리뷰)

- 큐 초기화의 이벤트 우선 처리(`App.tsx:137`), 아래 스택(`App.tsx:528`), `store::write_atomic`, `app_local_data_dir`, Windows Job Object, NSIS 한국어 설정, `RESTART_EXIT_CODE`, 플러그인의 같은 볼륨 검사(`TempDirNotOnSameMountPoint`)와 관리자 암호 경로, `on_before_exit` 훅.

#### Worktree parallelization strategy

| Step | Modules touched | Depends on |
|---|---|---|
| A. Rust 업데이터(준비 예약, 정리, IPC, 상태 파일) | `src-tauri/src/` | 0단계 |
| B. 화면(카드, 배치, 탭, 바깥 진입) | `src/` | A의 IPC 타입 |
| C. CI·스크립트(승격, `.sig`, 시작 확인) | `.github/`, `scripts/` | — |
| D. 문서 | `docs/`, README | C의 명령 |

Lane A: A → B(공유 IPC 타입 `src/lib/ipc.ts`) / Lane C: C → D. 1차와 같다. 충돌: `src/lib/ipc.ts`(A·B).

#### Implementation Tasks (Eng 재리뷰)

- [ ] **F1 (P1, human: ~6h / CC: ~30min)** — Rust — 준비 예약(`prepare_update`·`cancel_update`·60초 만료), 정리 한 번과 마감 하나, `RESTART_EXIT_CODE`, `spawn_blocking`, 녹음 마무리까지 녹음 수
  - Surfaced by: Codex C1, C3, 독립 리뷰어 LOW 5·6
  - Files: `src-tauri/src/update.rs`, `lib.rs`, `commands.rs`, `audio/mod.rs`
  - Verify: Rust 테스트(동시 호출, 멈춘 정리, 저장 중 종료 미룸)
- [ ] **F2 (P1, human: ~6h / CC: ~30min)** — IPC·상태 — `rev`, `origin`, `notice`, `just_updated.notes`, `update-state.json` 로컬 폴더와 새 필드, 재시도 `attempt`·`count`, 오래 확인 못 함 판정, 실패 뒤 30분 재시도
  - Surfaced by: Codex C4, C5, C6, C10, 독립 리뷰어 M2, M3, LOW 7
  - Files: `src-tauri/src/update.rs`, `src/lib/ipc.ts`, 공유 JSON fixture
  - Verify: Rust·TS 테스트가 같은 fixture를 읽음
- [ ] **F3 (P1, human: ~1d / CC: ~40min)** — 화면 — 카드 배치(스택 안·측정·`--bottom-stack`), 설정 탭 소유와 `notesIntent`, 바깥 진입 판정, D16
  - Surfaced by: Codex C2, C7, C8, C9, 독립 리뷰어 M10, 사용자 D16
  - Files: `src/App.tsx`, `src/screens/Settings.tsx`, `src/components/UpdateCard.tsx`, `src/screens/MeetingParts.tsx`
  - Verify: 화면 테스트, 미리보기 760·800px
- [ ] **F4 (P1, human: ~4h / CC: ~20min)** — CI — 승격 판정(공개 태그 비교)과 공개 뒤 확인, 공개 전 `.sig` 검증과 버전, 두 OS 시작 확인, App 토큰 범위
  - Surfaced by: Codex C11, 독립 리뷰어 H2, M5, LOW 9
  - Files: `.github/workflows/build.yml`, `scripts/latest-json.ts`
  - Verify: 판정 단위 테스트, 수동 실행 빌드의 시작 확인
- [ ] **F5 (P2, human: ~2h / CC: ~10min)** — Rust — 모델 내려받기 보호, 진행 이벤트 줄이기와 15초 타이머
  - Surfaced by: 독립 리뷰어 M8, M9
  - Files: `src-tauri/src/commands.rs`, `update.rs`
  - Verify: Rust 테스트
- [ ] **F6 (P2, human: ~2h / CC: ~10min)** — 계획·문서 — 접기 뒤 옛 문장 찾기, 태그 전 업데이트 시험, Windows 1분 문구, 다운로드 주소, publish 대기 교체, macOS 범위와 provenance, Slack 앱 범위, 사전 점검 축소, NSIS 줄 사실 고침
  - Surfaced by: 독립 리뷰어 H1, H2, M6, M7, LOW 1·2·3·4
  - Files: `docs/plans/2026-11-05-in-app-updater.md`, `docs/install.md`, 관리자 문서
  - Verify: 읽기, `grep`으로 옛 문장 없음

#### Completion Summary (Eng 재리뷰)

- Step 0: Scope Challenge — scope accepted as-is(복잡도 관문은 autoplan P2로 원래 배치)
- Reality rows: 7/7, unverified 6(CEO 단계 4 + NSIS Restart Manager, 키체인 창)
- Architecture Review: 7 issues(C1, C3, C4, C5, C11, H2, M3)
- Code Quality Review: 3 issues(H1 접기, M10·C7 배치, C8 탭 소유)
- Test Review: 다이어그램 작성, 22 gaps(모두 "(Eng 재리뷰) 테스트 추가" 줄)
- Performance Review: 1 issue(M9 진행 이벤트)
- NOT in scope: written / What already exists: written
- TODOS.md updates: 0 새 항목
- Failure modes: 0 critical gaps
- Unresolved decisions: 0
- Outside voice: codex completed
- Parallelization: 2 lanes, 2 parallel / 2 sequential
- Lake Score: N/A(D16은 종류가 다른 선택이라 완전성 점수가 없다)
- 사용자 결정 1개(D16), 자동 결정 29개(수용 24, 받지 않음 5, 감사 154~183), TASTE 0, User Challenge 0.
