# 인수인계 (handoff) — 세션 20261120-0935-a3f0 → 새 세션

이 문서는 Hermes Desk(`powered_ide`)를 Rust 네이티브 앱으로 옮기는 `ide` 저장소 작업을 새 세션에서
이어가기 위한 것이에요. 기준 시점: `main` = `984690e` (2026-11-22).

## 1. 프로젝트 한 줄 요약

- `ide` = Hermes Desk(Electron+React)를 **Rust + iced(master) + CEF(OSR)** 로 옮기는 앱.
  클라이언트(`ide-app`, iced 창)와 사용자별 데몬 서버(`ide-server`)로 나뉘고, 둘은 `ide-proto`
  프레임 프로토콜(현재 **PROTO_VERSION 6**)로 통신해요.
- **원칙: Hermes Desk가 명세예요.** 구조·흐름·문구·아이콘을 Desk(`powered_ide`)에서 읽고 그대로
  옮겨요. 임의로 "개선"한 대안을 제안하지 말고, 피할 수 없는 차이는 각 `docs/*.md`의
  "Deviations" 절에 이유와 함께 적어요. (사용자는 Desk와의 충실도로 판단하고, 요청하지 않은
  요소를 싫어해요.)

## 2. 먼저 읽을 것

1. `AGENTS.md` — 빌드·테스트·i18n·스크립트 규칙 (필독)
2. `docs/architecture.md` — 크레이트 구조와 핵심 개념
3. 주제별 설계 문서 (각각 "Desk 동작 → ide 파일" 매핑표 + 차이점):

| 문서 | 내용 |
|---|---|
| `docs/platforms.md` | OS별 동작 (Windows / macOS / Linux), macOS·Linux 할 일 목록과 변경 로그 (Mac 없음: 쌓아만 둠) |
| `docs/server-architecture.md` | 클라이언트/서버 분리, 전송, 핸드셰이크, 데몬 |
| `docs/agent-providers.md` | 에이전트 CLI(claude/codex/shell/선언형), 계정·구성, 프롬프트 전달 |
| `docs/repos-and-workspaces.md` | 레포 등록, 기본 레포, 세션 워크스페이스(워크트리), 기준 브랜치 규칙 |
| `docs/side-panel.md` | 변경 사항 / 히스토리 / 탐색기 탭, 탭 안쪽 동작 |
| `docs/git-history.md` | 히스토리 RPC와 UI |
| `docs/changes-actions.md` | 로컬 git 동작, PR·에이전트 요청, 푸터 UI, GitHub 전송 방식 |
| `docs/detail-window.md` | 상세 영역 새 창 분리 |

## 3. 크레이트

| 크레이트 | 역할 |
|---|---|
| `ide-core` | UI 없는 로직(레포·세션·워크트리·프롬프트 텍스트). 서버만 사용 |
| `ide-proto` | 요청/응답 타입, `PROTO_VERSION`, Desk의 판단 로직 순수 함수(PR 머지 가능 여부 등) |
| `ide-server` | 데몬: PTY, git, 파일, 검색, PR(GitHub), 감사 로그 |
| `ide-client` | 앱이 쓰는 비동기 클라이언트 |
| `ide-agents` | 에이전트 CLI별 코드(플래그·환경변수·프롬프트 입력 방식). **provider id로 `match`는 여기서만** |
| `ide-term` | 서버 바이트로 그리는 터미널 모델/위젯(한글 IME) |
| `ide-web` | CEF 런타임 + OSR 웹뷰 위젯, 창 소유자(pin) |
| `ide-views` | 파일 트리, 코드 뷰, diff 뷰, 변경 목록 위젯 |
| `ide-i18n` | Fluent 메시지(en, ko), 타입 안전 `tr::*` |
| `ide-app` | iced 앱(클라이언트). git/파일/에이전트를 직접 만지지 않고 전부 `ide_client::Client` 경유 |

## 4. 지금까지 끝난 것 (이 세션 후반부)

- 세션 목록: 레포별 → **구성별 묶음 + `전체`**, 묶음 접기, **정렬**(이름/마지막 활동/생성 시각/실행 상태, 우선순위·방향) — `24e64bc`
- 오른쪽 패널: 검색 탭을 **탐색기에 합침**(파일 찾기 + 내용 검색) — `dadfed9`
- **히스토리 탭**(커밋 목록, 원격에 없는 커밋 ↑ 표시·배지, 커밋/파일 diff) — `0b997cf`, `9102d3b`
- **변경 사항·히스토리 Desk 기능 전부**: 체크박스, stash, 변경 취소, 커밋 우클릭 메뉴, `레포 / 브랜치`
  선택기(전환·생성·fetch), 푸터(`커밋 맡기기`, 동기화, `…` 메뉴: PR 올리기/머지/보기/닫기/새 브랜치로
  이어가기), PR 대화상자, PR 상태 줄 — `94bb47d`, `21532fe`
- **GitHub 전송**: Desk 방식(`git credential fill` 토큰 + REST 직접 호출)이 기본, 토큰이 없을 때나
  401일 때만 `gh` 폴백 — `32f4f69`
- **상세 영역 새 창 분리**(새 창으로 분리 / 앱으로 합치기 / 고정, 상태 기억) — `984690e`

이전 커밋들(네비게이션 레일, 계정·구성, 레포 모델, 기준 브랜치 규칙, 파일/검색/diff, CEF 등)은
`git log`와 위 문서 참고.

## 5. 남은 일 / 확인 필요

- **사용자 수동 확인 대기**
  - 분리된 상세 창의 주소창·웹 페이지에서 한글 IME 입력
  - 상세 창 고정 상태에서 앱 창 최소화/복원(상세 창이 최대화돼 있던 경우 포함)
  - 다른 DPI 모니터 간 이동, 모니터 배치가 바뀐 뒤 저장 위치(화면 밖에 뜰 수 있음)
- **알려진 차이 (각 문서의 Deviations 참고)**: 에이전트 "답변 중" 신호 없음(Desk의 커밋 맡기기 차단
  없음), AI PR 작업은 PR 파일을 2초마다 확인, PR 페이지는 앱 안 웹 탭.
- **가져오지 않은 Desk 기능**: 채팅 첨부·커밋 드래그(채팅 입력창 없음), 브라우저
  탭·공통 탭의 재시작 후 유지, 에이전트 프로세스 목록, 모드, 미리보기 탭.
- **AGENTS.md 갱신 누락**: 앱 e2e 스크립트의 `IDE_APP_SCRIPT_ONLY=pop`(상세 창 단계만) 설명을
  추가하려던 편집이 승인 시간 초과로 들어가지 않았어요. 필요하면 추가.
- **저장소 설정**: GitHub 기본 브랜치가 아직 `hermes/20261120-0935-a3f0`일 수 있어요(사용자가
  `main`으로 바꾸기로 함). 이 PC의 `gh`는 `jook1356a` 계정이라 `jook1356/ide`에 API 접근이
  안 돼요(404). push는 git 저장 인증으로 돼요.
- 일부 토스트는 만든 시점 언어로 고정(언어 전환 후 다시 번역되지 않음).

## 6. 빌드 환경 (새 세션에서 다시 갖춰야 함)

세션 가드 때문에 툴체인·빌드 산출물은 **세션 워크스페이스 안**에 있어야 해요. 이 세션의 툴체인은
`C:/Users/사용자/hermes-workspaces/20261120-0935-a3f0/spikes/.toolchain`에 있지만, 새 세션은 다른
세션 폴더에 쓸 수 없어서(cargo가 `CARGO_HOME`에 써야 함) **새 워크스페이스에 다시 설치**해야 해요.
절차는 Hermes 스킬 `rust-desktop-windows`(software-development)에 있어요:

1. Rust를 워크스페이스 로컬로 설치(`rustup-init.exe`, `RUSTUP_HOME`/`CARGO_HOME`을 워크스페이스로),
   `rustup component add clippy rustfmt`.
2. MSVC: VS 2022 Build Tools(`C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`)의
   `vcvars64.bat`를 워크스페이스 안 `vcenv.cmd`로 불러 `env.sh`에서 PATH 등을 가져와요(스킬의
   `templates/env.sh`). 이 세션의 `spikes/env.sh`, `spikes/.toolchain/vcenv.cmd`를 복사해서 경로만
   바꿔 써도 돼요(읽기는 가능).
3. **CMake 4.x 포터블**(VS 번들 3.31은 비ASCII 경로 `사용자`에서 크래시)과 Ninja.
4. **CEF 156**: `CEF_PATH`에 export된 CEF 디렉터리(약 430 MB). 이 세션의
   `spikes/.toolchain/cef`를 새 워크스페이스로 **복사**(`cp -r`)하면 다시 받을 필요가 없어요.
   `libcef.dll`이 PATH에 있어야 실행돼요.
5. `CARGO_TARGET_DIR`은 짧게(예: `<ws>/t1`) — CEF 래퍼 빌드가 경로 길이 제한에 걸려요.
   릴리스 빌드 하나가 수 GB라 다 쓴 타깃 폴더는 지워요.

빌드/검사(`run_task`로 실행):

```bash
source <ws>/env.sh && export CARGO_TARGET_DIR=<ws>/t1 IDE_TEST_TMP=<ws>/t1-tmp
cargo clippy -p ide-app --all-targets          # 바꾼 크레이트만
cargo test -p <crate>
cargo build --release -p ide-app -p ide-server # 앱과 서버는 항상 같이
```

- `ide-app`과 일부 크레이트는 rustfmt 기준으로 깨끗하지 않아요: `cargo fmt --all` 금지, 필요하면 바꾼
  크레이트만 `-p`로.
- 저장소 파일은 CRLF 작업본(autocrlf=true)이에요. 스크립트로 쓸 때 줄끝이 섞이지 않게 주의.

## 7. 테스트 방법과 안전 규칙

- **앱 e2e 스크립트**(앱 안에서 iced 이벤트로 조작, OS 입력 없음): `IDE_APP_SCRIPT=1` + 전용
  `IDE_DATA_DIR`/`IDE_APP_TEST_REPO`/`IDE_APP_TEST_REPO2`/`IDE_APP_EVIDENCE` (AGENTS.md 참고).
  마지막 실행: 436 passed, 0 failed. 이 세션의 실행 래퍼 `run-pop.sh`(워크스페이스 루트)를 복사해
  경로만 바꿔 쓰면 돼요. 이 래퍼는 아래 안전 환경을 이미 설정해요:
  - `IDE_GH=<repo>/crates/ide-app/tests/fake-gh/gh.cmd` (가짜 gh, 상태는 `IDE_FAKE_GH_STATE`),
    `IDE_FAKE_GH_ORIGIN=<data>/script-origin.git`
  - `IDE_GITHUB_API=http://127.0.0.1:9` (도달 불가 주소), 임시 `GIT_CONFIG_GLOBAL`(credential helper가
    `quit=1`), `GIT_CONFIG_NOSYSTEM=1`, 빈 `GH_TOKEN`/`GITHUB_TOKEN`
- **절대 하지 말 것**
  - 테스트에서 `claude`/`codex`에 프롬프트 보내기(회사 구독 사용량). 열기·스크린샷·종료만.
  - 실제 GitHub 호출, 사용자의 실제 git 인증 정보 읽기.
  - OS 입력(SendInput, 마우스, computer_use 클릭/입력). computer_use는 pid+window_id로 **읽기 전용
    캡처**만.
  - `taskkill /IM ide.exe|ide-server.exe` — 사용자의 앱/데몬까지 죽어요. 자기 테스트 프로세스만 PID로.
- iced `window::screenshot`은 `text_input`/`text_editor` 값을 안 그려요 → 입력값 확인은 실제 창 캡처.

## 8. 사용자가 지금 쓰고 있는 앱

- 실행 중: `C:/Users/사용자/hermes-workspaces/20261120-0935-a3f0/ti-pop/release/ide.exe`,
  데이터 폴더 `.../20261120-0935-a3f0/ide-data`(이 세션 워크스페이스 안). 데몬은
  `ti-ui-b/release/ide-server.exe --daemon`(PID는 `ide-data/ide-server.pid`).
- 새 세션은 이 폴더에 쓸 수 없어요. 새 빌드로 바꿔 띄울 때는 **새 세션 워크스페이스에 데이터 폴더를
  새로 두고**(세션 목록은 새로 시작) 사용자에게 알리거나, 기존 데이터를 쓰려면 이 폴더를 공용 데이터
  폴더로 지정해 달라고 요청해요.
- 앱만 교체: 앱 창 PID와 그 `--stdio` 자식만 종료(`/T` 금지 — 데몬까지 죽음). 프로토콜 버전이 바뀌면
  데몬도 바꿔야 하고 실행 중인 세션이 중지되니 **먼저 사용자에게 알리고** 진행해요.

## 9. 작업 방식 (사용자 선호)

- 한국어로 소통, 짧고 직접적으로.
- UI 변경은 Desk 소스를 먼저 읽고 그대로. 요청보다 좁게 해석, 요청하지 않은 요소 추가 금지.
- 큰 작업은 서브에이전트 병렬(백엔드 proto/server/client ↔ UI app), 공유 파일은 블록 위치를 약속하고
  각자 새 모듈에 작성. 끝나면 부모가 clippy/test/e2e 확인 후 주제별로 커밋.
- 커밋: `git -c user.name='DongJuKim-Carlo' -c user.email='147456232+jook1356a@users.noreply.github.com' commit`
  (설정 파일에 쓰지 않음), 명령형 메시지, 주제 하나당 커밋 하나.
- 새 빌드는 띄우기 전에 세션이 중지되는지 알려주고 확인받기.
