# ide UI strings: English (reference and fallback locale).
#
# Rules (see AGENTS.md):
# - Every message here must exist in every other locale with the same variables;
#   the build fails otherwise.
# - A variable is an i64 parameter of the generated tr:: function when it is used as a
#   plural selector or in NUMBER(); otherwise it is &str.
# - No message references ({ other-message }); keep messages self-contained.

# The language's name in itself (reserved; shown in the language picker).
language-name = English

## Sidebar

settings = Settings
profile-label = PROFILE
sessions-label = SESSIONS
new-session = New session
no-sessions = No sessions yet. Click + to start one.
session-title-placeholder = Session title
start = Start
stop = Stop
restart = Restart
rename = Rename
delete = Delete

## Session status

status-stopped = stopped
status-starting = starting
status-running = running
status-exited = exited
status-exited-code = exited ({ $code })
status-failed = failed

## Agent kinds

agent-claude = Claude
agent-codex = Codex
agent-shell = Shell

## Centre

no-session-selected = No session selected
process-exited = Process exited
process-exited-code = Process exited with code { $code }
session-start-failed = The session could not be started
session-stopped = Session stopped
start-agent = Start { $agent }

## Detail area

show-detail = Show detail
hide-detail = Hide detail
tab-web = Web
web-back = Back
web-forward = Forward
web-reload = Reload
url-placeholder = Enter a URL
no-session = No session

## New-session dialog

repo-folder-label = REPOSITORY FOLDER
repo-placeholder = C:/src/my-repo
repo-dialog-title = Repository folder
browse = Browse…
agent-label = AGENT
title-label = TITLE (OPTIONAL)
title-placeholder = Defaults to agent · repo
base-ref-label = BASE REF (OPTIONAL)
base-ref-placeholder = HEAD
worktree-note = A new worktree on branch ide/<session-id> is created for the session.
cancel = Cancel
create = Create
creating = Creating…

## Delete dialog

delete-session-title = Delete session “{ $title }”?
delete-session-body = The process is stopped and the session is removed from the list.
delete-remove-worktree = Also remove its worktree and branch (uncommitted changes are lost)
deleting = Deleting…

## Settings dialog

language-label = LANGUAGE
language-system = System
profiles-label = PROFILES
profiles-note = Each profile has its own Claude config dir and Codex home.
profile-sessions = { $total ->
    [one] 1 session
   *[other] { $total } sessions
}, { NUMBER($running) } running
profile-name-placeholder = new-profile-name
profile-rename-placeholder = new-name
profile-delete-tooltip = Delete (files are kept on disk)
save = Save
add = Add
close = Close

## Errors shown by the app

err-sessions-unreadable = sessions.json could not be read: { $detail }
err-start-process = Could not start the process: { $detail }
err-worktree-missing = Worktree { $path } is missing; delete this session and create a new one.
err-profile-gone = Profile “{ $name }” no longer exists.
err-web-view = Web view: { $detail }
err-rename-failed = Rename failed: { $detail }
err-delete-cancelled = Delete was cancelled.
err-choose-repo = Choose a repository folder.
err-create-cancelled = Session creation was cancelled.
err-session-save = Saving the session failed: { $detail }
err-profile-in-use = { $count ->
    [one] 1 session uses
   *[other] { $count } sessions use
} profile “{ $name }”; delete them first.
err-profile-running = { $count ->
    [one] 1 session of profile “{ $name }” is running
   *[other] { $count } sessions of profile “{ $name }” are running
}; stop them before renaming.

## Errors from ide-core (ide_core::ErrorMessage)

err-io = { $action } { $path } failed: { $detail }
io-reading = Reading
io-writing = Writing
io-replacing = Replacing
io-creating-directory = Creating directory
io-renaming = Renaming
io-removing = Removing
err-json = Invalid JSON in { $path }: { $detail }
err-no-data-dir = Cannot determine the app data directory (set IDE_DATA_DIR).
err-invalid-name = Invalid name “{ $name }”: { $reason }
name-rule-empty = must not be empty
name-rule-too-long = must be at most 64 characters
name-rule-invalid-chars = only lowercase letters, digits, '-' and '_' are allowed
name-rule-bad-start = must start with a letter or digit
err-profile-exists = Profile “{ $name }” already exists.
err-profile-not-found = Profile “{ $name }” not found.
err-last-profile = Cannot delete the last profile “{ $name }”.
err-session-not-found = Session { $id } not found.
err-invalid-session-id = Invalid session id “{ $id }”.
err-not-a-git-repo = { $path } is not a git repository: { $detail }
err-git-spawn = Failed to run git: { $detail }
err-git = git { $args } failed ({ $status }): { $detail }
err-worktree-exists = Worktree path already exists: { $path }
err-program-not-found = “{ $program }” not found on PATH; install it or add it to PATH.
