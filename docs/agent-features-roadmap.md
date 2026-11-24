# 에이전트 기능 로드맵 (브리지 위에 올릴 것)

상태: 위젯(5), 대화 읽기 + 프롬프트 전달(3), 터미널 조회·입력(1)은 구현됨(`docs/widgets.md`,
`docs/session-family.md`, `docs/terminals.md`), 나머지 둘은 **계획만**. 승인 체계는 공통으로 구현됨(`docs/approvals.md`): 새 기능은 위험 등급과 capability만 정하면 돼요. 기반(에이전트 브리지, 훅, 화면 판독,
세션 가족)은 끝났으니, 이 문서의 방향대로 하나씩 올리면 돼요. 먼저 `docs/agent-bridge.md`와
`docs/session-family.md`를 읽으세요.

## 0. 이미 있는 기반 (모든 기능이 공통으로 씀)

| 기반 | 어디 | 쓰는 법 |
|---|---|---|
| 에이전트 → ide 도구 | `ide-server/src/bridge.rs` (`tools()`, `call()`), `service.rs` (`family_tool`) | 도구 하나 = `AgentTool`(이름·설명·JSON 스키마) + 처리 함수 |
| 데몬이 답하는 도구 | `UserCtx::family_tool` 패턴 (`spawn_blocking`) | git·파일·터미널처럼 서버가 가진 것 |
| 앱이 답하는 도구 | `Notify::AgentRequest` → 앱 `agent_bridge.rs` → `agent.reply` (60초 제한, 앱 없으면 즉시 오류) | 화면·CEF·창처럼 앱만 가진 것 |
| 세션 활동 상태 | `SessionInfo.activity` (훅 + 화면 판독) | "답변 중이면 기다렸다가" 같은 판단 |
| 메시지 대기열 | `BridgeState::enqueue` / `UserCtx::deliver_pending` (`send_prompt` 규칙) | 에이전트에게 글 넣기 |
| 훅 입력 | `ide-server hook` → `AgentHookEvent` | 세션 식별자·기록 파일 경로 같은 CLI 정보 받기 |
| provider별 코드 | `ide-agents` (trait 메서드 추가, 기본값 = 지원 안 함) | CLI마다 다른 것(기록 파일 위치 등)은 여기만 |

공통 규칙:
- 도구 이름은 `ide_` 접두사, MCP 서버 이름은 `ide-bridge`(`ide`는 Claude Code의 IDE 연동과 충돌).
- 위험한 동작(남의 터미널 입력, 브라우저 조작, 사이트 접근)은 **사용자 승인**을 거쳐요(`docs/approvals.md`의
  `UserCtx::approve(requester, capability, target, detail)`; 아래 옛 설명 대신 이것을 써요): 앱이
  답하는 도구로 만들고 앱이 확인 대화상자를 띄운 뒤 `agent.reply`로 결과를 돌려줘요.
- 모든 도구 호출은 이미 감사 로그(`agent.call`)에 남아요.
- 테스트는 실제 CLI에 프롬프트를 보내지 않고, 에이전트 연결(메모리 전송)이나 실제 `ide-server mcp`
  바이너리로 도구를 직접 불러서 해요(`ide-server/tests/bridge_e2e.rs`, `family_e2e.rs` 참고).
- 새 요청·응답은 프로토콜 버전을 올리고 `docs/server-architecture.md` 표에 한 줄 추가.

## 1. 터미널 조회·입력 — 구현됨 (`docs/terminals.md`)

구현: 아래 설계대로, 다만 출력은 링 버퍼 대신 서버 `Term`의 스크롤백(사용자 터미널만 2000줄)에서 읽어요
(커서 이동·덮어쓰기가 이미 적용된 글). 승인은 `terminal.input`(대상 = 터미널 id).

Desk: `terminal_inspect`(사용자 터미널 읽기), `terminal_input`(텍스트·키 입력).

- **재료**: 세션의 사용자 터미널은 서버에 있어요(`uterm.rs`, `TermHost`). `TermHost::screen_text()`가
  보이는 화면을 글자로 주고, 링 버퍼(`TermHost::read`)에 지난 출력이 있어요.
- **도구** (데몬이 답함):
  - `ide_terminal_list` — 이 세션의 터미널: id, 제목, 명령, 실행 여부, 포트.
  - `ide_terminal_read {terminal, lines?}` — 화면 + 최근 출력 N줄(ANSI 제거). 링 버퍼 끝에서 읽어
    줄 단위로 자르기.
  - `ide_terminal_input {terminal, text?, keys?}` — 글자 또는 키(`enter`, `ctrl+c`, `up`…)를 입력.
    키 이름 → 바이트 변환표는 서버 한 곳에.
- **범위**: 자기 세션의 터미널만(가족 세션 터미널은 관련 세션일 때 읽기만). 입력은 처음 한 번
  사용자 승인(세션·터미널 단위로 기억) — 앱이 답하는 확인 단계를 앞에 두기.
- **주의**: 실행 중인 명령에 Ctrl+C를 보내는 것도 입력이에요. 작업 대기열(아래 없음, 별도)과는 별개.

## 2. 브라우저(CEF) 조작

Desk: `browser_inspect`(목록·콘솔·컴포넌트·선택), `browser_act`(이동·클릭·입력·키·스크롤·대기·
스크린샷·강조·선택·evaluate·cdp·녹화), `browser_diagnose`. 사이트마다 사용자 승인.

- **재료**: CEF는 앱에 있어요(`ide-web`, 상세 영역의 웹 탭·브라우저 탭). 그래서 전부 **앱이 답하는
  도구**예요.
- **방식**: CEF의 DevTools 프로토콜(`cef` 크레이트의 devtools 메서드 실행)을 앱 안에서 써요.
  `evaluate`는 `Runtime.evaluate`, 클릭·입력은 `Input.dispatchMouseEvent` / `dispatchKeyEvent`,
  스크린샷은 `Page.captureScreenshot`(MCP 결과에 이미지 콘텐츠 `{"type":"image","data":...}` —
  `AgentToolResult`에 이미지 필드 추가 필요), 콘솔은 `Runtime.consoleAPICalled` 버퍼.
- **도구**: `ide_browser_list`, `ide_browser_open {url}`, `ide_browser_act {tab, action, ...}`,
  `ide_browser_read {tab, what: text|console|dom}`, `ide_browser_screenshot {tab}`.
- **승인**: 처음 보는 사이트(origin)에 대한 조작은 앱이 "이 세션이 <site>를 조작하려고 해요" 확인.
  허용 목록은 `ui.json` 또는 서버 설정에 사이트 단위로.
- **주의**: OSR 웹뷰라 실제 OS 입력이 아니어도 돼요(AGENTS.md: OS 입력 금지). 상세 창이 분리돼 있으면
  그 창의 웹뷰를 대상으로.

## 3. 관련 세션 대화 읽기 + 프롬프트 전달 — 구현됨 (Claude Code 대화만; Codex 대화 읽기는 남음)

Desk: `read_session`(관련 세션의 저장된 대화), `send_to_session`(`queue` / `steer`),
`session.interrupt` / `redirect`.

- **대화 읽기 — 기록 파일은 provider마다 달라요 → `ide-agents`**:
  - trait 메서드 예: `fn transcript(&self, file: &Path, limit) -> Result<Vec<TranscriptEntry>>`
    (`TranscriptEntry { role: user|assistant|tool, text, at }`), 기본값 = 지원 안 함.
  - **파일 위치는 훅이 알려줘요**: Claude와 Codex 훅 입력 모두 `transcript_path`(그리고
    `session_id`)를 줘요(`docs/agent-bridge.md`의 훅 절). `SessionStart` 훅에서 이 경로를
    `BridgeState`에 세션별로 저장(재시작 시 다시 받음) → 추측으로 폴더를 뒤지지 않아도 돼요.
    훅이 없는 경우의 대안: Claude는 계정 폴더 `projects/<cwd를 바꾼 이름>/<id>.jsonl`, Codex는
    `CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`에서 cwd·시각으로 찾기(확인 필요).
  - 도구 `ide_read_session {session, limit?, since?}` — 관련 세션만, 최근 N개 대화를 글로.
    큰 도구 출력은 줄이고 "생략" 표시.
- **프롬프트 전달 — 지금 있는 것 위에 확장**:
  - 이미 있음: `ide_send_to_session`(상대가 쉬면 바로, 아니면 대기열), 첫 프롬프트.
  - 추가: `mode: queue | steer | interrupt`. `interrupt`는 provider별 키(Esc 등 →
    `ide-agents`의 `interrupt_keys()`), `steer`는 답변 중에도 바로 입력(CLI가 받아주는 경우 —
    provider caps로 표시). 대기열 확인·취소 도구 `ide_sent_messages`(Desk의 `sent_messages`).
  - 앱 표시: 대기 중인 메시지 수를 세션 행에 작게(선택).

## 4. 미리보기 (도구 방식)

Desk: 채팅 답변 속 ` ```tsx preview ` 블록을 렌더러 샌드박스에서 그려요(`previewGuide.ts`의
`PREVIEW_GUIDE`, 세션 폴더 `.hermes-desk/preview-guide.md`). ide 세션은 채팅이 아니라 터미널 TUI라
블록을 읽을 곳이 없어서 **도구로** 해요.

- **도구** (앱이 답함): `ide_preview {title, kind: html|markdown|url|file, content?, path?, id?}`.
  - 데몬이 `content`를 `<data>/sessions/<id>/previews/<preview id>.html`에 쓰고(같은 `id`면 덮어써서
    갱신), 앱이 상세 영역에 미리보기 탭을 열거나 새로고침.
  - `file`은 세션 워크트리 안의 파일(HTML/이미지/마크다운)을 그대로 보여주기(`RootJail`로 범위 제한).
  - React/TSX: 앱에 작은 런타임 페이지(번들된 React + 트랜스파일러)를 두고 `content`를 넣어 그리는
    방식이 Desk에 가장 가까워요. 처음엔 HTML만으로 시작해도 충분.
- **보안**: 미리보기는 별도 origin(커스텀 스킴, 예: `ide-preview://`)에서, 앱 내부 API 접근 없이.
- **지침**: Desk처럼 긴 안내서를 세션 폴더 `.ide/preview-guide.md`에 두고 기본 지침에는 한 줄만.

## 5. 위젯 (Desk의 mods) — 구현됨

`docs/widgets.md`. Desk처럼 한 번 등록하면 모든 세션에서 쓰고(위젯 페이지, 상태 바 런처), 범위(세션 / 가족 / 공용)는
data와 탭 배치를 정해요. "모드"는 CLI의 plan mode 등과, "mod"는 Claude Code 자체의 mod와 헷갈려서
위젯이라고 불러요. React 강제 없이 HTML/JS(프레임워크 자유), `ide-widget://` 스킴으로 앱이 서빙,
`window.ide.call`로 브리지의 `ide_*` 도구를 사용자 승인 후 호출, `ide.data`로 범위별 저장.

## 권장 순서

1. ~~대화 읽기 + 프롬프트 전달 확장~~ — 구현됨(승인 체계와 함께). 세션 가족을 실제로 쓰려면 가장 먼저 필요하고, 훅이 이미
   `transcript_path`를 주니 작업이 작아요.
2. ~~터미널 조회·입력~~ — 구현됨(`docs/terminals.md`).
3. 미리보기 — HTML부터.
4. 브라우저 조작 — 이미지 결과, 사이트 승인, DevTools 연동이 필요해 커요.
5. ~~모드~~ 위젯 — 구현됨. 미리보기·브라우저가 이 스킴(`ide_web::set_scheme_handler`)을 재사용.
