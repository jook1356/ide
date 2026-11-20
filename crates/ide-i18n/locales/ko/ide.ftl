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

## Errors shown by the app

err-sessions-unreadable = sessions.json 파일을 읽지 못했어요: { $detail }
err-start-process = 프로세스를 시작하지 못했어요: { $detail }
err-worktree-missing = worktree가 없어요: { $path }. 이 세션을 삭제하고 새로 만드세요.
err-profile-gone = “{ $name }” 프로필이 더 이상 없어요.
err-web-view = 웹 뷰 오류: { $detail }
err-rename-failed = 이름을 바꾸지 못했어요: { $detail }
err-delete-cancelled = 삭제가 취소됐어요.
err-choose-repo = 저장소 폴더를 선택하세요.
err-create-cancelled = 세션 만들기가 취소됐어요.
err-session-save = 세션을 저장하지 못했어요: { $detail }
err-profile-in-use = “{ $name }” 프로필을 쓰는 세션이 { $count }개 있어요. 세션을 먼저 삭제하세요.
err-profile-running = “{ $name }” 프로필의 세션 { $count }개가 실행 중이에요. 이름을 바꾸기 전에 중지하세요.

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
