# TODOS

## Distribution

### 파일럿 후 자동 업데이트

**What:** `tauri-plugin-updater`로 앱이 새 버전을 스스로 받아 설치하게 한다.

**Why:** 파일럿 동안은 새 설치 파일을 사내 채널로 나눠 덮어 설치한다. 사용자가 늘면 업데이트를 안 한 사람이 구버전에 남고, 버그 수정이 퍼지지 않는다.

**Context:** eng review D2(2026-10-14)에서 v1 범위에서 뺐다. 다시 시작할 때 필요한 것은 세 가지다.
- 공개 releases 전용 저장소: 비공개 저장소의 Release 자산은 업데이터가 인증 없이 받을 수 없다. 설계 문서 Open Question 2.
- 업데이터 서명 키: `TAURI_SIGNING_PRIVATE_KEY`를 CI 시크릿으로 둔다. `.env`로는 동작하지 않는다.
- `tauri-action`이 `latest.json`을 만들어 releases 저장소에 올리게 한다. `latest.json` 전체가 검증되므로, 플랫폼 항목 하나만 깨져도 모든 플랫폼의 업데이트가 멈춘다.
- 참고: [설계 문서](designs/meeting-summarizer.md) Distribution Plan, [요구사항](requirements.md) "v1에서 제외".

**Effort:** M
**Priority:** P2
**Depends on:** 설계 문서 Open Question 2(공개 releases 저장소) 해결, 파일럿(M7) 결과

### Windows 코드 서명 검토

**What:** Windows 설치 파일에 코드 서명을 할지, 한다면 어떤 방식으로 할지 정한다.

**Why:** 파일럿은 서명 없이 배포한다. 서명이 없으면 설치할 때 SmartScreen 경고가 뜨고, 비기술 동료는 거기서 설치를 포기할 수 있다. 설계의 성공 지표(실제로 녹음되는 회의 비율)는 설치 마찰에 민감하다.

**Context:** 설계 문서 Distribution Plan은 "처음에는 서명 없이 배포한다. 서명은 사용자 반응을 보고 결정한다"고 남겨 두었다(eng review D19에서 TODO로 기록).
- 파일럿(M7) 1주차 설치 지원 기록에서 SmartScreen 경고로 막히거나 문의한 사람 수를 센다.
- 선택지: OV/EV 코드 서명 인증서, 또는 클라우드 서명 서비스. 비용과 발급 대기 기간을 함께 비교한다.
- 서명하기로 하면 GitHub Actions의 Windows 빌드 단계에 서명을 붙인다.

**Effort:** S/M
**Priority:** P3
**Depends on:** 파일럿(M7) 설치 피드백

## Summary

### 실행 중인 Ollama·LM Studio 찾기

**What:** 요약(LLM) 탭에 "설치된 Ollama·LM Studio 찾기" 버튼을 두고, 응답한 서버의 모델을 고르면 연결(컨텍스트 8,192)을 추가해 기본으로 정한다. 찾지 못하면 서버마다 할 일을 보인다.

**Why:** 이미 Ollama·LM Studio를 쓰는 사람이 연결 주소와 컨텍스트를 손으로 맞추지 않아도 된다(사용자 결정 U2).

**Context:** M6.5 최종 관문(2026-10-16) UC2에서 미뤘다. 설계는 `docs/plans/2026-10-16-local-summary.md` 5.7과 DX10(빈 상태, `OLLAMA_CONTEXT_LENGTH=8192 ollama serve` 복사 명령)에 그대로 있다. `no_proxy()` 클라이언트, 이름 "Ollama · <모델>"(같으면 " 2"부터), 고르면 기본.

**Effort:** human M / CC S
**Priority:** P2
**Depends on:** M6.5 내장 경로

### 느린 PC 속도 안내

**What:** 마지막 내장 요약의 `timings` 합계로 "1시간 회의 약 N분"을 요약(LLM) 탭에 보이고, 15분을 넘으면 클라우드 연결 추가를 권한다(그 안내로 연 추가 칸은 "기본으로 쓰기"가 켜짐).

**Why:** CPU만 있는 Windows PC는 1시간 회의에 10분 넘게 걸릴 수 있다. 미리 알면 녹음자가 다른 연결을 고를 수 있다.

**Context:** M6.5 CEO 리뷰 E2, 최종 관문(2026-10-16) UC2에서 미뤘다. 공식은 (22,000 ÷ 프롬프트 속도 + 3,000 ÷ 생성 속도) ÷ 60, 처리 장치가 바뀌면 지운다. M6.5는 `timings`를 로그와 평가용으로 이미 모은다. 7단계 실측 숫자로 15분 기준을 다시 본다.

**Effort:** human S / CC S
**Priority:** P2
**Depends on:** M6.5 7단계 실측

### 요약 줄마다 전사본 시각 연결

**What:** 요약의 결정사항·액션 아이템 줄마다 근거가 된 전사본 구간의 시각을 붙이고, 누르면 전사본 패널의 그 자리로 간다.

**Why:** 로컬 요약 기본화(M6.5) 뒤 요약은 4B 모델이 쓴다. 불참자와 녹음자가 요약 한 줄을 전사본과 바로 맞춰 볼 수 있으면 지어낸 내용을 빨리 찾는다.

**Context:** M6.5 CEO 리뷰(2026-10-16) E4에서 미뤘다. 프롬프트가 구간 시각을 받아 쓰게 하고(`summarize.rs`), 줄 파서(`MeetingParts.tsx`의 `parseDoc`)가 시각 표기를 알아보며, 전사본 패널이 그 시각으로 스크롤해야 한다. Slack 메시지에는 시각을 넣지 않는다(채널에서는 누를 곳이 없다). 근거 검사(M6.5 C2)가 먼저 들어간다.

**Effort:** human L / CC M
**Priority:** P2
**Depends on:** M6.5, 파일럿(M7)에서 요약 수정 기록

### 요약 출력 형식 강제 (JSON 스키마)

**What:** 내장 `llama-server`의 JSON 스키마(문법) 제약으로 요약을 섹션별 구조로 받고, 앱이 Markdown으로 바꾼다.

**Why:** 작은 모델은 섹션을 빼거나 형식을 어길 수 있다. 문법 제약을 걸면 형식 오류가 구조적으로 없어진다.

**Context:** M6.5 CEO 리뷰(2026-10-16) E6에서 미뤘다. 파일럿에서 섹션 누락·형식 오류가 실제로 보일 때 시작한다. 외부 연결(Ollama, 클라우드)은 지금처럼 Markdown을 받으므로 두 경로가 생긴다는 점을 함께 정한다.

**Effort:** human M / CC S
**Priority:** P3
**Depends on:** M6.5, 파일럿(M7) 형식 오류 기록

### 홈에서 실행 중인 Ollama 제안

**What:** 요약 모델을 받지 않았고 앱을 열 때 `127.0.0.1:11434`가 짧게(200ms) 응답하면, 홈 설정 줄에 "이 PC의 Ollama로 요약하기"를 보이고 누르면 로컬 서버 찾기 결과를 연다.

**Why:** 이미 Ollama를 쓰는 사람은 2.7GB 요약 모델을 받지 않아도 된다.

**Context:** M6.5 CEO 리뷰(2026-10-16) E7에서 받아들였다가 스펙 리뷰 1차에서 미뤘다. "실행 중인 Ollama·LM Studio 찾기"(위 항목, 사용자 결정 U2)와 겹치고, 앱 시작 때 네트워크 탐지와 홈 상태가 하나 더 생긴다. Ollama 경로는 서버 컨텍스트를 8,192 이상으로 맞춰야 해 초보자에게 약하다. 파일럿에서 찾기 버튼 사용과 Ollama 사용자 수를 본 뒤 정한다.

**Effort:** human S / CC S
**Priority:** P3
**Depends on:** 실행 중인 Ollama·LM Studio 찾기, 파일럿(M7)

### 쓰지 않게 된 모델 파일 정리

**What:** 모델 표의 파일 이름이 바뀌어 쓰지 않게 된 옛 모델 파일을 설정에서 크기와 함께 보이고 지울 수 있게 한다.

**Why:** 요약 모델(2.7GB)이나 전사 모델을 바꾸면 옛 파일이 `models` 폴더에 남아 디스크를 차지한다.

**Context:** M6.5 스펙 리뷰 2차(2026-10-16)에서 미뤘다(지금은 표의 파일 이름이 바뀔 일이 드물다). 자동으로 지우지는 않는다(재설치 재사용과 직접 가져오기 파일을 지키려고). 모델을 처음 바꿀 때 함께 한다.

**Effort:** human S / CC S
**Priority:** P3
**Depends on:** 모델 표의 항목이 처음 바뀔 때

## Development

### push마다 테스트 돌리기

**What:** master에 push할 때 `cargo test`와 `bun run test`를 CI에서 돌린다. 먼저 Windows만, 필요하면 macOS도.

**Why:** 지금 테스트는 개발자 Mac에서만 돈다. 사이드카·감독·포트·한글 경로처럼 Windows에서만 깨지는 것은 배포 빌드 때에야 드러난다.

**Context:** M6.5 Eng 리뷰(2026-10-16)에서 미뤘다. M6.5는 기존 빌드 작업(태그·수동 실행)에 테스트 단계만 더한다. CI는 사용자가 요청하거나 배포할 때만 돌린다는 결정 때문이다(macOS 실행 시간은 10배로 청구된다). Windows 작업은 1배 청구라 push마다 돌려도 싸다. 사용자가 원하면 `on: push` 작업을 따로 만든다.

**Effort:** human S / CC S
**Priority:** P3
**Depends on:** 사용자 결정(CI 비용)

### worktree 격리

**What:** 개발 worktree마다 앱 데이터 폴더, 키체인 항목(앱 identifier), 개발 서버 포트(1420)를 따로 쓰게 한다.

**Why:** 지금은 worktree 두 개를 띄우면 같은 회의 폴더·설정·키체인을 함께 쓰고 1420 포트가 겹친다. 코딩 에이전트가 병렬로 시험하면 서로의 데이터를 바꾼다.

**Context:** M6.5 DX 리뷰(2026-10-16)에서 미뤘다. 지금은 README "개발 시작"에 공유 상태를 적는 것으로 둔다. 개발 빌드에서만 identifier 접미와 데이터 폴더를 환경 변수로 바꾸고 Vite 포트를 고르는 방식이 후보다. 릴리스 identifier는 바꾸지 않는다.

**Effort:** human M / CC S
**Priority:** P3
**Depends on:** 병렬 worktree 시험이 실제로 필요해질 때

## Completed
