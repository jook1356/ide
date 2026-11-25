# 인수인계 (handoff)

이 문서는 Hermes Desk(`powered_ide`)를 Rust 네이티브 앱으로 옮기는 `ide` 저장소 작업을 새 세션에서
이어가기 위한 것이에요. 기준 시점: 2026-11-23, 세션 가족(부모/자식 세션)과 공유 폴더 쓰기 권한까지.

## 1. 프로젝트 한 줄 요약

- `ide` = Hermes Desk(Electron+React)를 **Rust + iced(master) + CEF(OSR)** 로 옮기는 앱.
  클라이언트(`ide-app`, iced 창)와 사용자별 데몬 서버(`ide-server`)로 나뉘고, 둘은 `ide-proto`
  프레임 프로토콜(현재 **PROTO_VERSION 15**)로 통신해요.
- **원칙: Hermes Desk가 명세예요.** 구조·흐름·문구·아이콘을 Desk(`powered_ide`)에서 읽고 그대로
  옮겨요. 임의로 "개선"한 대안을 제안하지 말고, 피할 수 없는 차이는 각 `docs/*.md`의
  "Deviations" 절에 이유와 함께 적어요. 예외: 사용자가 Desk보다 나아가자고 정한 것(세션 가족,
  에이전트 브리지)은 그 설계 문서를 따라요.

## 2. 먼저 읽을 것

1. `AGENTS.md` — 빌드·테스트·i18n·스크립트 규칙 (필독)
2. `docs/architecture.md` — 크레이트 구조와 핵심 개념
3. 주제별 설계 문서:

| 문서 | 내용 |
|---|---|
| `docs/platforms.md` | OS별 동작 (Windows / macOS / Linux), macOS·Linux 할 일 목록과 변경 로그 (Mac 없음: 쌓아만 둠) |
| `docs/server-architecture.md` | 클라이언트/서버 분리, 전송, 핸드셰이크, 데몬, 프로토콜 버전별 추가 |
| `docs/agent-providers.md` | 에이전트 CLI(claude/codex/shell/선언형), 계정·구성, 프롬프트 전달 |
| `docs/agent-bridge.md` | 에이전트 ↔ ide 연결: `ide` MCP 서버, 훅(`ide-server hook`), 화면 판독, 활동 상태 |
| `docs/session-family.md` | 부모/자식 세션(분리·공유), 머지·따라잡기, 가족 도구, 공유 폴더 쓰기 권한 |
| `docs/agent-features-roadmap.md` | 다음 기능 계획: 터미널 조회·입력, 브라우저 조작, 대화 읽기·프롬프트 전달, 미리보기, 모드 |
| `docs/repos-and-workspaces.md` | 레포 등록, 기본 레포, 세션 워크스페이스(워크트리), 기준 브랜치 규칙 |
| `docs/side-panel.md` | 변경 사항 / 히스토리 / 탐색기 탭, 탭 안쪽 동작 |
| `docs/git-history.md` | 히스토리 RPC와 UI |
| `docs/changes-actions.md` | 로컬 git 동작, PR·에이전트 요청, 푸터 UI, GitHub 전송 방식 |
| `docs/detail-window.md` | 상세 영역 새 창 분리 |

## 3. 크레이트

| 크레이트 | 역할 |
|---|---|
| `ide-core` | UI 없는 로직(레포·세션·워크트리·세션 가족·프롬프트 텍스트). 서버만 사용 |
| `ide-proto` | 요청/응답 타입, `PROTO_VERSION`, Desk의 판단 로직 순수 함수(PR 머지 가능 여부 등) |
| `ide-server` | 데몬: PTY, git, 파일, 검색, PR(GitHub), 에이전트 브리지(`bridge.rs`), 감사 로그 |
| `ide-client` | 앱이 쓰는 비동기 클라이언트 |
| `ide-agents` | 에이전트 CLI별 코드(플래그·환경변수·프롬프트 입력·훅·화면 규칙). **provider id로 `match`는 여기서만** |
| `ide-term` | 서버 바이트로 그리는 터미널 모델/위젯(한글 IME) |
| `ide-web` | CEF 런타임 + OSR 웹뷰 위젯, 창 소유자(pin) |
| `ide-views` | 파일 트리, 코드 뷰, diff 뷰, 변경 목록 위젯 |
| `ide-i18n` | Fluent 메시지(en, ko), 타입 안전 `tr::*` |
| `ide-app` | iced 앱(클라이언트). git/파일/에이전트를 직접 만지지 않고 전부 `ide_client::Client` 경유 |

## 4. 최근에 끝난 것 (2026-11-23)

- 세션 목록 Desk화: 우클릭 메뉴(Desk `rowMenu`), 고정/보관함(서버 저장, 프로토콜 11), 세션 설정에서
  이름 변경, 호버 버튼(하위 세션 추가·고정·삭제), 목록 너비 드래그 — `5f9856c`, `f24fb19`
- 스크립트 섹션(Desk `ScriptsSection`, 사이드 패널 맨 아래) — `8517e47`
- 에이전트 브리지: 모든 세션에 `ide` MCP 서버 + 기본 지침 + 세션 토큰(프로토콜 12) — `a2df533`
- 에이전트 활동: Claude 훅(`--settings`), 화면 판독(herdr 방식, `screens/*.toml`), 답변 중 / 응답
  필요 / 새 답변 표시(프로토콜 13) — `e2e5724`
- 세션 가족: 분리/공유 하위 세션(최대 2단계), 머지·따라잡기, 가족 도구 5개, 트리 UI(프로토콜 14) —
  `ba21292`
- 공유 폴더 쓰기 권한(훅으로 편집 거부, `ide_request_write_access` / `ide_release_write_access`)
- Codex 계정 통합 설치(계정 화면 설치/갱신/제거, `CODEX_HOME/hooks.json`에 ide 항목만, Codex에서
  `/hooks`로 한 번 승인, 프로토콜 15)

## 5. 남은 일 / 확인 필요

- **실제 CLI로 확인 대기** (테스트에서 프롬프트를 보낼 수 없어서 자동 확인 못 함)
  - Claude: `--settings` 훅이 실제로 불리는지(세션을 열면 활동이 Idle이 되는지, 프롬프트에 "답변
    중"), 편집 거부 메시지가 Claude에 어떻게 보이는지
  - 화면 판독 규칙(`crates/ide-agents/screens/*.toml`)이 실제 Claude/Codex 화면과 맞는지
  - `ide` MCP 도구(가족 도구 포함)를 에이전트가 실제로 쓰는지
- **Codex 통합 설치 실제 확인**: 설치 후 Codex `/hooks` 승인 화면, 훅이 실제로 불리는지(Windows에서
  어떤 셸로 실행되는지 포함), 편집 거부가 동작하는지.
- **사용자 수동 확인 대기**: 분리된 상세 창의 한글 IME 입력, 상세 창 고정 상태에서 최소화/복원, 다른
  DPI 모니터 간 이동과 저장 위치.
- **저장소 설정**: GitHub 기본 브랜치가 아직 `hermes/20261120-0935-a3f0`일 수 있어요. 이 PC의 `gh`는
  `jook1356a` 계정이라 `jook1356/ide` API 접근이 안 돼요(404) — 사용자가 GitHub에서 직접 바꿔야 해요.
  push는 git 저장 인증으로 돼요.
- **브리지 위 다음 기능**: 방향과 순서는 `docs/agent-features-roadmap.md`(터미널 조회·입력, 브라우저
  조작, 관련 세션 대화 읽기·프롬프트 전달, 미리보기, 모드). 작업 실행 대기열(`run_task`)은 그 다음.
- **알림 센터**(`docs/notifications.md`): 세션 목록 아래 오류 배너는 없어지고 알림이 됐어요. 레일의 벨에서 보고
  지울 수 있어요. 새 알림은 언어 전환 후 다시 번역돼요. 기존 토스트 상당수는 아직 만든 시점 언어로 고정돼 있어요
  (`Text::Plain`).

## 6. 빌드 환경 (이 PC, 2026-11-23부터 시스템 전역)

- Rust: rustup(winget, `%USERPROFILE%\.cargo\bin`), CMake 4.4(winget), Ninja(winget), MSVC는
  VS 2022 Build Tools(cmake 크레이트가 `cl.exe`를 스스로 찾음, vcvars 불필요).
- 사용자 환경변수: `CEF_PATH=C:\dev\cef\156`(PATH에도 있음), `CARGO_TARGET_DIR=D:/t/ide`,
  `WGPU_BACKEND=dx12`. 이 설정 전에 열린 셸은 PATH를 레지스트리에서 다시 읽어야 해요.
- 빌드: `cargo app`(= `cargo build --release -p ide-app -p ide-server`), 실행 파일은
  `C:\t\ide\release\ide.exe`.
- 검사: `cargo clippy --workspace --all-targets`, `cargo test --workspace` +
  `IDE_TEST_TMP=C:\t\test-tmp`(**git 저장소 밖**: 저장소 안이면 일반 폴더 테스트 3개가 실패).
- 예전 세션의 워크스페이스 전용 툴체인(`hermes-workspaces/20261120-0935-a3f0/spikes/.toolchain`)과
  `ti-*` 빌드 폴더는 남아 있어요(지워도 됨).
- `ide-app`과 일부 크레이트는 rustfmt 기준으로 깨끗하지 않아요: `cargo fmt --all` 금지.
- 줄끝: 작업본이 CRLF/LF 섞일 수 있어요(커밋 때 정규화). 스크립트로 쓸 때 한 파일 안에서 섞이지 않게.
- 이 worktree(`D:\repository\ide`, `main`)를 다른 세션도 같이 써요: 커밋은 자기 변경만 골라서.

## 7. 테스트 방법과 안전 규칙

- **앱 e2e 스크립트**(앱 안에서 iced 이벤트로 조작, OS 입력 없음): `IDE_APP_SCRIPT=1` + 전용
  `IDE_DATA_DIR`/`IDE_APP_TEST_REPO`/`IDE_APP_TEST_REPO2`/`IDE_APP_EVIDENCE` (AGENTS.md 참고).
  안전 환경(가짜 gh, 도달 불가 GitHub API, 임시 `GIT_CONFIG_GLOBAL`, 빈 토큰)은 AGENTS.md 그대로.
- **서버 e2e**: `ide-server/tests/*`(브리지: `bridge_e2e.rs`, 가족: `family_e2e.rs` — 실제 CLI 없이
  셸 세션·실제 git·실제 `ide-server mcp`/`hook` 바이너리로 확인).
- **절대 하지 말 것**
  - 테스트에서 `claude`/`codex`에 프롬프트 보내기(회사 구독 사용량). 열기·스크린샷·종료만.
  - 실제 GitHub 호출, 사용자의 실제 git 인증 정보 읽기.
  - OS 입력(SendInput, 마우스, computer_use 클릭/입력).
  - `taskkill /IM ide.exe|ide-server.exe` — 다른 데이터 폴더의 데몬까지 죽어요. PID로만.
- iced `window::screenshot`은 `text_input`/`text_editor` 값을 안 그려요 → 입력값 확인은 실제 창 캡처.

## 8. 사용자가 지금 쓰고 있는 앱

- `C:\t\ide\release\ide.exe`(기본 데이터 폴더). 데몬은 앱이 띄운 `C:\t\ide\release\ide-server.exe
  --daemon`. 예전 데이터 폴더(`hermes-workspaces/20261120-0935-a3f0/ide-data`)용 데몬도 따로 돌 수
  있어요(건드리지 않음).
- 앱만 교체: 앱 창 PID와 그 `--stdio` 자식만 종료(`/T` 금지). 실행 중인 `ide.exe`가 파일을 잡고 있어
  빌드 전에 앱을 먼저 닫아야 해요. 프로토콜 버전이 바뀌면 데몬도 바꿔야 하고 실행 중인 세션이 멈추니
  **먼저 사용자에게 알리고** 진행해요.

## 9. 작업 방식 (사용자 선호)

- 한국어로 소통, 짧고 직접적으로.
- UI 변경은 Desk 소스를 먼저 읽고 그대로. 요청보다 좁게 해석, 요청하지 않은 요소 추가 금지.
- 단순 스타일 변경은 사용자가 직접 검증해요: 검증에 시간을 많이 쓰지 말 것.
- 큰 작업은 서브에이전트 병렬(백엔드 proto/server/client ↔ UI app)도 가능. 끝나면 clippy/test 확인 후
  주제별로 커밋.
- 커밋: `git -c user.name='DongJuKim-Carlo' -c user.email='147456232+jook1356a@users.noreply.github.com' commit`
  (설정 파일에 쓰지 않음), 명령형 메시지, 주제 하나당 커밋 하나.
- 새 빌드는 띄우기 전에 세션이 중지되는지 알려주고 확인받기.
