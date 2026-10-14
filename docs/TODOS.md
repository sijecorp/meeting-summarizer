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

## Completed
