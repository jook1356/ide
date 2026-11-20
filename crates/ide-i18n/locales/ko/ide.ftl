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

repo-folder-label = 저장소 폴더
repo-placeholder = C:/src/my-repo
browse = 찾아보기…
agent-label = 에이전트
title-label = 제목 (선택)
title-placeholder = 비워 두면 '에이전트 · 저장소'
base-ref-label = 기준 ref (선택)
base-ref-placeholder = HEAD
worktree-note = 세션마다 ide/<session-id> 브랜치로 새 worktree를 만들어요.
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
err-choose-repo = 저장소 폴더를 선택하세요.
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
side-search = 검색
changes-count = 변경된 파일 { $count }개
kind-file = 파일
kind-changed = 변경됨
kind-staged = 스테이징됨
kind-compare = 디스크와 편집 내용 비교
file-binary = 바이너리 파일이라 표시하지 않아요 ({ $size }).
file-truncated = 큰 파일이라 앞부분만 읽기 전용으로 보여 줘요 (전체 { $size }).
file-conflict = 파일을 연 뒤 디스크에서 바뀌었어요. 편집한 내용은 저장하지 않았어요.
file-changed-on-disk = 디스크에서 파일이 바뀌었어요. 편집한 내용은 아직 저장되지 않았어요.
file-deleted-on-disk = 디스크에서 파일이 삭제됐어요.
bar-reload = 디스크에서 다시 불러오기 (편집 내용 버림)
bar-overwrite = 내 내용으로 저장
bar-compare = 디스크와 비교
search-placeholder = 파일에서 검색
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
browse-roots-label = 저장소 폴더
browse-roots-note = 새 세션 대화상자에서 서버의 이 폴더들을 둘러볼 수 있어요. 홈 폴더는 항상 포함돼요.
browse-root-placeholder = 서버의 절대 폴더 경로
browse-home = 홈
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
provider-support = { $provider }: 스킬 { $skills } · MCP { $mcp } · 지침 { $instructions }
