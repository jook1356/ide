# ide UI 문자열: 한국어.
#
# 규칙: 메시지 키와 변수는 en/ide.ftl과 정확히 같아야 해요 (다르면 빌드가 실패해요).
# 버튼·라벨은 짧은 명사형, 안내·오류 문장은 해요체로 씁니다.
# 변수 바로 뒤에 조사(은/는, 을/를, 이/가)가 오지 않게 문장을 짭니다.

language-name = 한국어

## 내비게이션 레일 (왼쪽 끝: 페이지와 서버 상태)

nav-work = 작업
nav-accounts = 계정
nav-configurations = 구성
nav-repos = 레포
sessions-hide = 세션 목록 숨기기
sessions-show = 세션 목록 펼치기
rail-server = 서버: { $target } · { $state }
rail-server-version = 서버: { $target } · { $state } · { $version }
conn-state-connecting = 연결 중
conn-state-connected = 연결됨
conn-state-reconnecting = 다시 연결 중
conn-state-disconnected = 연결 안 됨

## Sidebar

settings = 설정
sessions-label = 세션
new-session = 새 세션
no-sessions = 세션이 아직 없어요. +를 눌러 시작하세요.
session-title-placeholder = 세션 제목
start = 시작
stop = 중지
restart = 다시 시작
rename = 이름 바꾸기
delete = 삭제

## Session status

status-stopped = 중지됨
status-starting = 시작 중
status-running = 실행 중
status-exited = 종료됨
status-exited-code = 종료됨 ({ $code })
status-failed = 실패

## Agent kinds


## Centre

no-session-selected = 선택한 세션이 없어요
process-exited = 프로세스가 종료됐어요
process-exited-code = 프로세스가 종료됐어요 (종료 코드 { $code })
session-start-failed = 세션을 시작하지 못했어요
session-stopped = 세션이 중지됐어요
start-agent = { $agent } 시작

## Detail area

show-detail = 상세 패널 보기
hide-detail = 상세 패널 숨기기
tab-web = 웹
web-back = 뒤로
web-forward = 앞으로
web-reload = 새로고침
url-placeholder = URL 입력
no-session = 세션 없음

## New-session dialog

agent-label = 에이전트
title-label = 제목 (선택)
title-placeholder = 비워 두면 '에이전트 · 저장소'
cancel = 취소
create = 만들기
creating = 만드는 중…

## Delete dialog

delete-session-title = “{ $title }” 세션을 삭제할까요?
delete-session-body = 프로세스를 중지하고 목록에서 세션을 지워요.
delete-remove-worktree = worktree와 브랜치도 삭제 (커밋하지 않은 변경 사항은 사라져요)
deleting = 삭제 중…

## Settings dialog

language-label = 언어
language-system = 시스템 설정
save = 저장
add = 추가
close = 닫기

## Connection and server (sidebar footer, settings)

conn-connecting = 서버에 연결하는 중이에요…
conn-connected = 연결됨 · 서버 { $version }
conn-reconnecting = 연결이 끊겼어요. 다시 연결하는 중이에요…
conn-disconnected = 연결 안 됨: { $detail }
conn-retry = 다시 연결
conn-server-stopped = 서버를 중지했어요. 실행 중인 세션이 없어요.
conn-closed = 연결을 닫았어요.
conn-read-only = 연결되지 않았어요. 터미널은 읽기 전용이에요.
conn-local = 로컬
server-label = 서버
server-note = 창을 닫아도 세션은 백그라운드 서버에서 계속 실행돼요.
stop-server = 서버 중지…
stop-server-confirm = 서버 중지
stop-server-title = 서버를 중지할까요?
stop-server-body = 모든 세션(에이전트와 셸)을 중지하고 백그라운드 서버를 종료해요. worktree는 디스크에 남아요.
stopping = 중지하는 중…

conn-target = 서버: { $target }
server-ssh = SSH
ssh-host-placeholder = user@host
ssh-command-placeholder = 서버 명령 (기본값: ide-server --stdio)
server-apply = 적용
server-env-override = IDE_REMOTE={ $value } 환경 변수가 이 설정보다 우선해요.
err-ssh-host = user@host 형식으로 SSH 호스트를 입력하세요.
switch-server-title = 서버를 바꿀까요?
switch-server-body = { $target } 서버에 다시 연결해요. 지금 서버의 세션은 그 서버에서 계속 실행돼요.
switch-server = 바꾸기
on-close-label = 창을 닫을 때
on-close-ask = 매번 묻기
on-close-keep = 에이전트 계속 실행
on-close-stop = 에이전트와 서버 중지
close-title = 에이전트가 아직 실행 중이에요
close-body = 로컬 서버에서 세션 { $count }개가 실행 중이에요.
close-keep = 백그라운드에서 계속 실행
close-stop = 에이전트와 서버 모두 중지
close-remember = 다시 묻지 않기 (설정에서 바꿀 수 있어요)

## Errors shown by the app

err-web-view = 웹 뷰 오류: { $detail }
err-rename-failed = 이름을 바꾸지 못했어요: { $detail }
err-choose-repo = 레포를 선택하세요.
err-server-missing = 서버 프로그램을 찾지 못했어요: { $detail }

## Errors from the server and the connection (ide_proto::ProtoError ids)

err-bad-message = 서버가 잘못된 메시지를 보냈어요: { $detail }
err-bad-request = 서버가 이 요청을 지원하지 않아요: { $method }
err-proto-version = 서버와 호환되지 않아요 (서버 프로토콜 { $server }, 앱 프로토콜 { $client }).
err-not-handshaken = 연결이 올바르게 설정되지 않았어요.
err-unauthorized = 서버가 접근을 거부했어요.
err-session-running = 세션이 이미 실행 중이에요: { $id }
err-session-not-running = 세션이 실행 중이 아니에요: { $id }
err-spawn-failed = “{ $program }” 프로그램을 시작하지 못했어요: { $detail }
err-internal = 서버 내부 오류가 발생했어요: { $detail }
err-message-too-large = 요청이 너무 커요.
err-path-denied = 이 경로에는 접근할 수 없어요: { $path }
err-disconnected = 서버에 연결되어 있지 않아요.
err-timeout = 서버가 제때 응답하지 않았어요.
err-connect-failed = 서버에 연결하지 못했어요: { $detail }
err-unknown = 서버 오류가 발생했어요 ({ $id }): { $details }

## Errors from ide-core (ide_core::ErrorMessage)

err-io = { $path } { $action } 실패: { $detail }
io-reading = 읽기
io-writing = 쓰기
io-replacing = 바꾸기
io-creating-directory = 폴더 만들기
io-renaming = 이름 바꾸기
io-removing = 삭제
err-json = { $path } 파일의 JSON이 올바르지 않아요: { $detail }
err-no-data-dir = 앱 데이터 폴더를 정할 수 없어요. IDE_DATA_DIR 환경 변수를 설정하세요.
err-invalid-name = 이름이 올바르지 않아요 (“{ $name }”): { $reason }
err-account-not-found = “{ $name }” 계정을 찾을 수 없어요 (에이전트: { $provider }).
err-account-in-use = “{ $name }” 계정을 쓰는 세션이 { $count }개 있어요. 세션을 먼저 삭제하세요.
err-configuration-not-found = “{ $name }” 구성을 찾을 수 없어요.
err-configuration-in-use = “{ $name }” 구성을 쓰는 세션이 { $count }개 있어요. 세션을 먼저 삭제하세요.
err-last-configuration = 마지막 남은 구성은 삭제할 수 없어요: “{ $name }”
err-provider-not-found = 서버에서 “{ $provider }” 에이전트를 쓸 수 없어요.
err-provider-no-login = “{ $provider }” 에이전트에는 계정이나 로그인 명령이 없어요.
err-invalid-mcp-name = MCP 서버 “{ $name }”: 영문자, 숫자, -, _로 된 겹치지 않는 이름과 명령이 필요해요.
name-rule-empty = 비워 둘 수 없어요
name-rule-too-long = 64자 이하여야 해요
name-rule-invalid-chars = 영문 소문자, 숫자, '-', '_'만 쓸 수 있어요
name-rule-bad-start = 영문 소문자나 숫자로 시작해야 해요
err-session-not-found = 세션을 찾을 수 없어요: { $id }
err-invalid-session-id = 세션 ID가 올바르지 않아요: “{ $id }”
err-not-a-git-repo = git 저장소가 아니에요: { $path } ({ $detail })
err-git-spawn = git을 실행하지 못했어요: { $detail }
err-git = git { $args } 실패 ({ $status }): { $detail }
err-worktree-exists = worktree 경로가 이미 있어요: { $path }
err-program-not-found = PATH에서 “{ $program }” 프로그램을 찾을 수 없어요. 설치하거나 PATH에 추가하세요.

## File views (ide-views): file tree, code view, diff view, changes list

tree-open = 열기
tree-new-file = 새 파일
tree-new-folder = 새 폴더
tree-loading = 불러오는 중…
tree-name-placeholder = 이름
code-find-placeholder = 찾기
code-find-none = 결과 없음
code-find-prev = 이전 결과 (Shift+Enter)
code-find-next = 다음 결과 (Enter)
code-goto-placeholder = 줄로 이동 (줄:열)
code-modified = 수정됨
code-revert = 되돌리기
code-save = 저장 (Ctrl+S)
diff-binary = 바이너리 파일은 표시하지 않아요.
diff-no-changes = 내용 변경이 없어요 (권한 등 메타데이터만 바뀌었어요).
diff-renamed-only = 내용 변경 없이 이름만 바뀌었어요: { $from } → { $to }
diff-expand-lines = 변경 없는 줄 { $count }개 보기
diff-unified = 한 열로 보기
diff-split = 나란히 보기
diff-no-newline = 파일 끝에 줄바꿈 없음
changes-staged = 스테이징됨
changes-unstaged = 변경
changes-untracked = 추적 안 됨
changes-empty = 변경 사항이 없어요.
git-modified = 수정
git-added = 추가
git-deleted = 삭제
git-untracked = 새 파일
git-renamed = 이름 변경
git-conflicted = 충돌

# 상세 영역 (열린 항목, 사이드 패널)
open-items = 열린 항목
group-browser = 브라우저
side-changes = 변경 사항
side-explore = 탐색기
changes-count = 변경된 파일 { $count }개
kind-file = 파일
kind-changed = 변경됨
kind-staged = 스테이징됨
kind-compare = 디스크와 편집 내용 비교
side-history = 히스토리
kind-commit = 커밋
history-empty = 커밋이 없습니다.
history-more = 더 보기
history-unpushed-count = 원격에 올라가지 않은 커밋 { $count }개
history-unpushed-mark = 아직 원격에 올라가지 않은 커밋
history-show-files = 변경된 파일 보기
history-hide-files = 파일 목록 접기
history-load-failed = 커밋 목록을 불러오지 못했습니다: { $error }
time-just-now = 방금
time-minutes = { $count }분 전
time-hours = { $count }시간 전
time-yesterday = 어제
time-days = { $count }일 전
time-last-month = 지난달
time-months = { $count }개월 전
time-last-year = 작년
time-years = { $count }년 전
file-binary = 바이너리 파일이라 표시하지 않아요 ({ $size }).
file-truncated = 큰 파일이라 앞부분만 읽기 전용으로 보여 줘요 (전체 { $size }).
file-conflict = 파일을 연 뒤 디스크에서 바뀌었어요. 편집한 내용은 저장하지 않았어요.
file-changed-on-disk = 디스크에서 파일이 바뀌었어요. 편집한 내용은 아직 저장되지 않았어요.
file-deleted-on-disk = 디스크에서 파일이 삭제됐어요.
bar-reload = 디스크에서 다시 불러오기 (편집 내용 버림)
bar-overwrite = 내 내용으로 저장
bar-compare = 디스크와 비교
explore-find-placeholder = 파일 찾기
explore-contents = 내용
search-case = 대소문자 구분
search-word = 단어 단위
search-regex = 정규식
search-running = 검색 중…
search-summary = 파일 { $files }개에서 결과 { $matches }개
search-truncated = (최대 개수에 도달)
search-none = 결과가 없어요.
quick-open-placeholder = 파일로 이동
quick-open-none = 맞는 파일이 없어요.
def-pick-title = “{ $name }” 정의
delete-entry-title = “{ $name }” 항목을 삭제할까요?
delete-entry-body = 파일을 완전히 삭제해요. 되돌릴 수 없어요.
delete-folder-body = 폴더와 그 안의 모든 내용을 완전히 삭제해요. 되돌릴 수 없어요.
unsaved-title = “{ $name }” 파일의 변경 내용을 저장할까요?
unsaved-body = 저장하지 않으면 변경 내용이 사라져요.
dont-save = 저장 안 함
browse-up = 상위 폴더
browse-empty = 하위 폴더가 없어요.
browse-git = Git 저장소
pick-root-title = 레포 추가
pick-root-note = 서버에 있는 폴더예요. 폴더를 두 번 클릭하거나 Enter를 누르면 열려요.
pick-start = 내 컴퓨터
pick-select = 이 폴더 선택
pick-truncated = 폴더가 너무 많아서 앞부분만 보여요.
remove = 제거

# 2단계 서버 오류
err-path-invalid = 올바르지 않은 경로예요: { $path }
err-fs-not-found = 찾을 수 없어요: { $path }
err-fs-exists = 이미 있어요: { $path }
err-fs-permission = 권한이 없어요: { $path }
err-fs-dir-not-empty = 폴더가 비어 있지 않아요: { $path }
err-fs-not-a-dir = 폴더가 아니에요: { $path }
err-fs-is-a-dir = 파일이 아니라 폴더예요: { $path }
err-fs-io = 파일에 접근하지 못했어요: { $path } ({ $detail })
err-fs-conflict = 읽은 뒤 디스크에서 파일이 바뀌었어요: { $path }
err-fs-bad-encoding = 알 수 없는 텍스트 인코딩이에요: { $encoding }
err-fs-unencodable = 파일 인코딩으로 저장할 수 없는 문자가 있어요: { $path } ({ $encoding })
err-watch-failed = 폴더 변경을 감시하지 못해요: { $detail }
err-search-empty = 검색어를 입력해 주세요.
err-search-bad-regex = 정규식이 올바르지 않아요: { $detail }
err-search-bad-glob = 파일 패턴이 올바르지 않아요: { $glob } ({ $detail })
err-git-bad-rev = 올바르지 않은 리비전이에요: { $rev }
err-git-no-base = 기준 브랜치를 찾을 수 없어요: { $base }
err-def-invalid-position = 이 위치에서는 찾을 이름이 없어요.
err-def-unsupported-file = 이 파일 형식은 정의로 이동을 지원하지 않아요: { $path }
err-def-no-name = 정의로 이동하려면 이름 위에 커서를 놓아 주세요.
err-def-external-module = 외부 모듈이에요: { $spec }
err-def-module-not-found = 모듈을 찾을 수 없어요: { $spec }
err-def-external-name = 외부 모듈에 정의돼 있어요: { $spec }
err-def-not-found = 정의를 찾지 못했어요: { $name }

## 계정과 구성 (설정, 새 세션 대화상자)

configuration-label = 구성
account-label = 계정
account-default = 기본 로그인 (CLI 자체 설정)
accounts-note = 계정마다 에이전트 CLI 로그인 하나와 설정 폴더를 따로 써요. 추가하면 터미널에서 로그인을 열어요.
account-name-placeholder = 계정 이름
account-add = 추가 후 로그인
account-check = 상태
account-login = 로그인
account-status-checking = 확인 중…
account-logged-in = 로그인됨
account-logged-out = 로그인 안 됨
account-status-unknown = 알 수 없음
account-delete-tooltip = 계정과 로그인 파일 삭제
configurations-note = 어떤 에이전트와 계정에도 붙일 수 있는 스킬, MCP 서버, 지침이에요.
configuration-name-placeholder = 구성 이름
configuration-name-label = 이름
configuration-edit = 편집
configuration-delete-tooltip = 구성 삭제
configuration-instructions = 지침
configuration-skills = 스킬 (SKILL.md가 있는 폴더)
configuration-skill-placeholder = 서버의 스킬 폴더
configuration-mcp = MCP 서버 (stdio)
configuration-sessions = 세션 { $total }개, 실행 중 { NUMBER($running) }개
mcp-name-placeholder = 이름
mcp-command-placeholder = 명령
mcp-args-placeholder = 인수
mcp-env-placeholder = KEY=값

# 레포 (Hermes Desk의 레포 등록, 레포 페이지, 세션 워크스페이스)
group-login = 로그인 터미널
no-repos = 레포 없음
repos-title = 레포 관리
repos-none = 등록된 레포가 없습니다.
repos-list-label = 레포
repo-add = 레포 추가
repo-path-placeholder = 서버의 레포 경로 (Enter로 추가)
repos-pick-note = 왼쪽 레포 목록에서 레포를 추가하거나 선택하세요.
repos-note = 세션마다 자기 폴더에 레포별 워크트리를 만들어요. 등록된 git 레포만 쓸 수 있어요.
repo-no-sessions = 이 레포를 쓰는 세션이 없어요.
repo-unregister = 등록 해제
repo-unregister-keep-files = 등록 해제 (파일 유지)
repo-base-label = 기준 브랜치
repo-base-unknown = (알 수 없음)
repo-sessions-label = 세션 { $count }개
provider-support-modes = { $provider }: 워크스페이스 { $workspace } · 원본 레포 { $original } (스킬, MCP, 지침)
configuration-default-repos = 기본 레포
configuration-default-repos-note = 새 세션을 만들 때 미리 선택되는 레포입니다. 세션마다 바꿀 수 있습니다.
workspace-label = 워크스페이스
use-worktrees = 워크트리 사용
use-worktrees-note = 세션마다 레포의 워크트리(브랜치 ide/<세션 id>)를 만들어 원본과 분리해서 작업합니다.
worktree-repos-label = 워크트리를 만들 레포
no-repos-empty-workspace = 등록된 레포가 없습니다. 레포 없이 빈 워크스페이스로 시작합니다.
empty-workspace-note = 레포를 고르지 않으면 빈 워크스페이스로 시작해요.
worktree-repos-note = 고른 레포마다 워크트리를 만듭니다. 처음 선택은 구성의 기본 레포이고, 레포는 나중에 세션의 ⚙에서 추가할 수 있습니다.
original-repo-warning = 원본 레포에서 바로 작업해요. 에이전트가 고른 레포의 원본 폴더와 지금 체크아웃된 브랜치에서 그대로 파일을 고치고 커밋합니다. 같은 레포를 쓰는 다른 세션이나 직접 하던 작업과 변경이 섞일 수 있어요. 커밋하지 않은 변경이 있으면 먼저 정리해 두세요.
original-repo-label = 작업할 레포
no-repos-register-first = 등록된 레포가 없습니다. 먼저 레포를 등록하세요.
session-repos-title = “{ $title }” 레포
session-repos-label = 레포 · 브랜치 ← 기준 브랜치
session-repo-detach = 이 워크트리를 세션에서 제거 (브랜치는 유지)
session-repos-add-label = 레포 추가
session-repos-note = 추가한 레포는 저장하면 세션 폴더에 워크트리가 생기고, 이미 실행 중인 에이전트는 다시 시작할 때 작업 규칙에 반영돼요.
session-repos-no-workspace = 이 세션은 워크스페이스가 없어 레포를 추가할 수 없습니다.
session-repos-open = 세션 레포
no-repos-in-session = 이 세션에는 레포가 없어요. ⚙에서 추가하세요.
err-some-worktrees-failed = 일부 레포의 워크트리를 만들지 못했습니다:
    { $detail }
err-repo-not-found = 레포 { $id }가 등록되어 있지 않아요.
err-repo-in-use = “{ $name }” 레포를 쓰는 세션이 { $count }개 있어요. 세션을 먼저 삭제하세요.
err-repo-not-attached = 이 세션에는 “{ $name }” 레포가 없어요.
err-not-a-workspace-session = { $id } 세션은 원본 레포에서 바로 작업해서 워크스페이스가 없어요.

## Base branches, 기준 브랜치 최신화, git init of a plain folder (Hermes Desk)
base-mode-fixed = 고정 브랜치
base-mode-rule = 규칙으로 자동 선택
base-rule-active-warning = 지금은 규칙으로 자동 선택 중이에요. 여기서 저장하면 규칙이 해제되고 고정 브랜치를 사용해요.
base-fixed-note = 새 세션의 기준 브랜치 기본값이에요. 세션마다 새 세션 창이나 세션 레포 설정에서 다른 브랜치로 바꿀 수 있고, 세션의 상태 표시는 그 세션의 기준 브랜치를 따라요. 바꿔도 기존 세션의 기준 브랜치는 그대로예요.
base-fixed-pick-note = 로컬 및 원격 추적 브랜치 중 선택해요. 저장할 때 원격에서 가져오지 않아요.
base-auto = 자동 (기본 브랜치)
base-loading = 브랜치 불러오는 중…
base-missing = { $branch } (로컬에 없음)
base-missing-error = 현재 설정된 브랜치가 없어요. 다른 브랜치를 선택하거나 자동으로 초기화하세요.
base-auto-note = 자동: main → master → origin 기본 브랜치 → 현재 브랜치 순서로 선택해요. 원격 추적 브랜치는 이 컴퓨터에 저장된 상태를 써요.
base-reset-auto = 자동 기본값으로 초기화
base-saved = 저장했어요.
base-rule-note = 새 워크트리를 만들 때마다 원격 브랜치 목록을 확인해 규칙에 맞는 브랜치 중 버전이 가장 높은 브랜치에서 시작해요. 원격에 연결할 수 없거나 일치하는 브랜치가 없으면 이전 브랜치로 대신 만들지 않고 실패해요.
base-rule-remote = 원격
base-rule-pattern = 후보 패턴
base-rule-pattern-note = * 는 / 를 포함한 모든 문자
base-rule-match = 허용 형식 (정규식, 선택)
base-rule-match-placeholder = 비우면 후보 패턴에 맞는 브랜치 전부
base-rule-version-note = 버전 숫자 순으로 비교해요 (2026.9 < 2026.19 < 2026.20). -rc, -hotfix 같은 접미사 브랜치를 제외하려면 허용 형식을 지정하세요.
base-rule-current = 현재 적용될 브랜치
base-rule-recheck = 원격 다시 확인
base-rule-no-pick = 일치하는 브랜치 없음 → 세션 생성 실패
base-rule-loading = 원격 브랜치 불러오는 중…
base-rule-excluded = 허용 형식과 맞지 않아 제외
base-rule-clear = 규칙 해제
base-rule-last-pick = 마지막 세션: { $last } · 다음 세션부터: { $pick }
new-refresh-base = 기준 브랜치 최신화
new-refresh-base-note = 원격 최신 커밋으로 생성해요. 로컬 기준 브랜치는 변경하지 않으며, 가져오기 실패 시 해당 레포는 생성하지 않아요.
new-base-note = 레포마다 기준 브랜치에서 새 브랜치를 만들고, 상태 표시도 그 기준 브랜치를 기준으로 해요. 기본값은 레포 설정의 기준 브랜치예요.
base-pick-label = 기준 브랜치
base-no-origin-branches = origin 브랜치가 없어요
base-changed-by-rule = 기준 브랜치가 규칙에 따라 바뀌었어요:
    { $detail }
session-repos-base-note = 레포마다 세션 상태 표시의 기준이에요. 바꿔도 세션 브랜치를 다시 만들거나 rebase하지는 않아요.
init-repo-title = Git 저장소가 아니에요
init-repo-message = 폴더가 Git 저장소가 아니에요. 초기화해서 추가할까요?
init-repo-detail = 이 폴더에서 git init을 하고 지금 파일들로 첫 커밋을 만든 뒤 레포로 추가해요. 폴더에 .git이 생기고, .gitignore에 없는 파일은 모두 첫 커밋에 들어가요.
init-repo-no-remote = 원격이 없어서 push와 PR은 쓸 수 없어요 (나중에 원격을 추가하면 쓸 수 있어요).
init-repo-confirm = 초기화하고 추가
err-not-a-git-folder = Git 저장소가 아닌 폴더예요: { $path }
err-folder-not-found = 폴더가 없어요: { $path }
err-corrupt-git-dir = 손상된 .git이 있어 초기화하지 않았어요: { $path }
err-git-timeout = 시간 초과({ $secs }초)로 git { $command } 명령을 중단했어요.
err-base-refresh-failed = 기준 브랜치 최신화 실패: { $detail }
err-base-no-remote = 기준 브랜치에 원격 저장소가 없어요. 최신화를 해제하거나 upstream을 설정하세요.
err-base-not-branch = 최신화할 기준 브랜치를 선택하세요.
err-base-fetch-failed = 기준 브랜치를 가져오지 못했어요: { $detail }
err-base-rule-no-match = 기준 브랜치 규칙과 일치하는 원격 브랜치가 없어요.
err-invalid-base-rule-remote = 원격 이름이 올바르지 않아요.
err-invalid-base-rule-pattern = 후보 패턴을 입력하세요.
err-invalid-base-rule-match = 정규식이 올바르지 않아요.
err-remote-not-found = 원격 저장소가 없어요: { $remote }
err-remote-list-failed = 원격 브랜치 목록을 가져오지 못했어요: { $detail }
err-invalid-base-branch = 기준 브랜치가 유효하지 않거나 로컬에 없어요: { $branch }
err-origin-branch-missing = origin에 { $branch } 브랜치가 없어요 (기준 브랜치 최신화를 켜 보세요).
err-origin-branch-unknown = origin에 { $branch } 브랜치가 없어요 (fetch 후 다시 시도하세요).

## 세션 목록: 구성 필터(전체)와 정렬

configurations-all = 전체
group-fold = “{ $name }” 접기
group-unfold = “{ $name }” 펼치기
session-sort = 세션 정렬
session-sort-tooltip = 세션 정렬: { $summary }
session-sort-default = 기본 순서
session-sort-reset = 기본 순서로
session-sort-default-note = 기본 순서입니다. 기준을 추가하면 자동 정렬로 바뀌어요.
session-sort-add = 정렬 기준 추가
session-sort-help = 위 기준부터 적용하고, 화살표로 우선순위를 바꿔요. 구성 그룹은 유지되며, 같은 값은 기존 순서로, 정보가 없는 항목은 마지막에 표시돼요.
sort-key-name = 이름
sort-key-updated = 마지막 활동
sort-key-created = 생성 시각
sort-key-activity = 실행 상태
sort-asc = 오름차순
sort-desc = 내림차순
sort-move-up = 우선순위 올리기
sort-move-down = 우선순위 내리기
sort-remove = 정렬 기준 제거
