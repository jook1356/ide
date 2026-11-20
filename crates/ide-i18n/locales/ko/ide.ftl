# ide UI 문자열: 한국어.
#
# 규칙: 메시지 키와 변수는 en/ide.ftl과 정확히 같아야 해요 (다르면 빌드가 실패해요).
# 버튼·라벨은 짧은 명사형, 안내·오류 문장은 해요체로 씁니다.
# 변수 바로 뒤에 조사(은/는, 을/를, 이/가)가 오지 않게 문장을 짭니다.

language-name = 한국어

## Sidebar

settings = 설정
profile-label = 프로필
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

agent-claude = Claude
agent-codex = Codex
agent-shell = 셸

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
repo-dialog-title = 저장소 폴더
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
profiles-label = 프로필
profiles-note = 프로필마다 Claude 설정 폴더와 Codex 홈을 따로 써요.
profile-sessions = 세션 { $total }개, 실행 중 { NUMBER($running) }개
profile-name-placeholder = 새 프로필 이름 (a-z, 0-9, -, _)
profile-rename-placeholder = 새 이름
profile-delete-tooltip = 삭제 (디스크의 파일은 남아요)
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
err-profile-in-use = “{ $name }” 프로필을 쓰는 세션이 { $count }개 있어요. 세션을 먼저 삭제하세요.
err-profile-running = “{ $name }” 프로필의 세션 { $count }개가 실행 중이에요. 이름을 바꾸기 전에 중지하세요.

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
name-rule-empty = 비워 둘 수 없어요
name-rule-too-long = 64자 이하여야 해요
name-rule-invalid-chars = 영문 소문자, 숫자, '-', '_'만 쓸 수 있어요
name-rule-bad-start = 영문 소문자나 숫자로 시작해야 해요
err-profile-exists = “{ $name }” 프로필이 이미 있어요.
err-profile-not-found = “{ $name }” 프로필을 찾을 수 없어요.
err-last-profile = 마지막 남은 프로필은 삭제할 수 없어요: “{ $name }”
err-session-not-found = 세션을 찾을 수 없어요: { $id }
err-invalid-session-id = 세션 ID가 올바르지 않아요: “{ $id }”
err-not-a-git-repo = git 저장소가 아니에요: { $path } ({ $detail })
err-git-spawn = git을 실행하지 못했어요: { $detail }
err-git = git { $args } 실패 ({ $status }): { $detail }
err-worktree-exists = worktree 경로가 이미 있어요: { $path }
err-program-not-found = PATH에서 “{ $program }” 프로그램을 찾을 수 없어요. 설치하거나 PATH에 추가하세요.
