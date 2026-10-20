# Slack 앱 만들기

회의 요약은 녹음자가 앱에서 Slack에 로그인해, 자기가 들어가 있는 채널에 자기 이름으로 요약을 보낸다(ONB-04, SND-01). 그러려면 회사 워크스페이스에 Slack 앱이 하나 있어야 한다. 처음 한 번만 만들고, 앱의 Client ID를 `src-tauri/src/slack.rs`의 `CLIENT_ID`에 넣어 빌드한다.

- 관련: [요구사항 SND·ONB](requirements.md), [아키텍처 7](architecture.md#7-slack-전송)

## 왜 이 방식인가

- **PKCE**: 데스크톱 앱은 Client Secret을 숨길 수 없다(실행 파일에서 꺼낼 수 있다). Slack은 2026-03부터 PKCE를 지원해 Secret 없이 토큰을 받을 수 있다([Slack 문서](https://docs.slack.dev/authentication/using-pkce)). 대신 데스크톱 리디렉트는 사용자 권한(user scope)만 받을 수 있어 봇이 아니라 녹음자 본인으로 올린다.
- **loopback 리디렉트**: 로그인이 끝나면 Slack이 브라우저를 `http://localhost:47815/slack/callback`으로 돌려보내고, 앱이 그 포트에서 기다리다 코드를 받는다. PKCE를 켠 앱의 `localhost` 리디렉트는 데스크톱 리디렉트로 취급된다.
- **토큰 회전**: PKCE 앱의 토큰은 항상 회전한다. 액세스 토큰은 12시간, 갱신 토큰은 한 번 쓰면 바뀌고 30일 동안 쓰지 않으면 끝난다. 그때는 설정 → Slack에서 다시 로그인한다.
- **권한**: `channels:read`·`groups:read`(내가 들어가 있는 공개·비공개 채널 목록, `users.conversations`), `chat:write`(내 이름으로 보내기). 메시지를 읽는 권한은 없다.

## 만드는 순서

1. https://api.slack.com/apps 에서 **Create New App → From a manifest**를 고르고 회사 워크스페이스를 고른다.
2. 아래 매니페스트를 YAML 칸에 붙여 넣고 만든다.
3. **Basic Information → App Credentials**의 **Client ID**를 `src-tauri/src/slack.rs`의 `CLIENT_ID`에 넣는다. Client Secret은 쓰지 않는다(앱에 넣지 않는다).
4. 워크스페이스가 앱 승인을 요구하면 관리자가 승인한다. 공개 배포(Distribution)는 필요 없다: 한 워크스페이스 안에서만 쓴다.

```yaml
display_information:
  name: 회의 요약
  description: 검토한 회의 요약을 녹음자 이름으로 Slack 채널에 보냅니다
  background_color: "#1f1d1a"
oauth_config:
  redirect_urls:
    - http://localhost:47815/slack/callback
  scopes:
    user:
      - channels:read
      - groups:read
      - chat:write
  pkce_enabled: true
settings:
  token_rotation_enabled: true
  org_deploy_enabled: false
  socket_mode_enabled: false
```

## 주의

- PKCE는 한 번 켜면 Slack 지원팀을 거치지 않고는 끌 수 없다(앱이 공개 클라이언트가 된다). 토큰 회전도 끌 수 없다.
- 리디렉트 포트(47815)는 `slack.rs`의 `PORT`와 매니페스트가 같아야 한다. 다른 프로그램이 그 포트를 쓰고 있으면 로그인이 "포트를 열 수 없습니다"로 실패한다.
- 녹음자가 들어가 있지 않은 채널은 목록에 나오지 않는다. 보낼 채널에 먼저 들어가 있어야 한다.
