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

## Navigation rail (left edge: pages and the server status)

nav-work = Work
nav-accounts = Accounts
nav-configurations = Configurations
nav-repos = Repos
sessions-hide = Hide session list
sessions-show = Show session list
rail-server = Server: { $target } · { $state }
rail-server-version = Server: { $target } · { $state } · { $version }
conn-state-connecting = connecting
conn-state-connected = connected
conn-state-reconnecting = reconnecting
conn-state-disconnected = disconnected

## Sidebar

settings = Settings
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

agent-label = AGENT
title-label = TITLE (OPTIONAL)
title-placeholder = Defaults to agent · repo
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
save = Save
add = Add
close = Close

## Connection and server (sidebar footer, settings)

conn-connecting = Connecting to the server…
conn-connected = Connected · server { $version }
conn-reconnecting = Connection lost. Reconnecting…
conn-disconnected = Disconnected: { $detail }
conn-retry = Reconnect
conn-server-stopped = The server was stopped. Sessions are not running.
conn-closed = Connection closed.
conn-read-only = Not connected: the terminal is read-only.
conn-local = Local
server-label = SERVER
server-note = Sessions keep running in the background server when this window closes.
stop-server = Stop server…
stop-server-confirm = Stop server
stop-server-title = Stop the server?
stop-server-body = Every session (agents and shells) is stopped and the background server exits. Worktrees stay on disk.
stopping = Stopping…

conn-target = Server: { $target }
server-ssh = SSH
ssh-host-placeholder = user@host
ssh-command-placeholder = Server command (default: ide-server --stdio)
server-apply = Apply
server-env-override = IDE_REMOTE={ $value } overrides this setting.
err-ssh-host = Enter an SSH host such as user@host.
switch-server-title = Switch server?
switch-server-body = The app reconnects to { $target }. Sessions on the current server keep running there.
switch-server = Switch
on-close-label = WHEN THE WINDOW CLOSES
on-close-ask = Ask
on-close-keep = Keep agents running
on-close-stop = Stop agents and server
close-title = Agents are still running
close-body = { $count ->
    [one] 1 session is
   *[other] { $count } sessions are
} running on the local server.
close-keep = Keep agents running in the background
close-stop = Stop all agents and the server
close-remember = Don't ask again (change it in Settings)

## Errors shown by the app

err-web-view = Web view: { $detail }
err-rename-failed = Rename failed: { $detail }
err-choose-repo = Choose a repository.
err-server-missing = The server program was not found: { $detail }

## Errors from the server and the connection (ide_proto::ProtoError ids)

err-bad-message = The server sent an invalid message: { $detail }
err-bad-request = The server does not support this request: { $method }
err-proto-version = Incompatible server (protocol { $server }; this app speaks { $client }).
err-not-handshaken = The connection was not set up correctly.
err-unauthorized = The server refused access.
err-session-running = Session { $id } is already running.
err-session-not-running = Session { $id } is not running.
err-spawn-failed = Could not start “{ $program }”: { $detail }
err-internal = Internal server error: { $detail }
err-message-too-large = The request is too large.
err-path-denied = Access to { $path } is not allowed.
err-disconnected = Not connected to the server.
err-timeout = The server did not answer in time.
err-connect-failed = Could not reach the server: { $detail }
err-unknown = Server error { $id }: { $details }

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
err-account-not-found = Account “{ $name }” of { $provider } not found.
err-account-in-use = { $count ->
    [one] 1 session uses
   *[other] { $count } sessions use
} account “{ $name }”; delete them first.
err-configuration-not-found = Configuration “{ $name }” not found.
err-configuration-in-use = { $count ->
    [one] 1 session uses
   *[other] { $count } sessions use
} configuration “{ $name }”; delete them first.
err-last-configuration = Cannot delete the last configuration “{ $name }”.
err-provider-not-found = The agent “{ $provider }” is not available on the server.
err-provider-no-login = The agent “{ $provider }” has no accounts or login command.
err-invalid-mcp-name = MCP server “{ $name }”: use a unique name of letters, digits, - and _, and a command.
name-rule-empty = must not be empty
name-rule-too-long = must be at most 64 characters
name-rule-invalid-chars = only lowercase letters, digits, '-' and '_' are allowed
name-rule-bad-start = must start with a letter or digit
err-session-not-found = Session { $id } not found.
err-invalid-session-id = Invalid session id “{ $id }”.
err-not-a-git-repo = { $path } is not a git repository: { $detail }
err-git-spawn = Failed to run git: { $detail }
err-git = git { $args } failed ({ $status }): { $detail }
err-worktree-exists = Worktree path already exists: { $path }
err-program-not-found = “{ $program }” not found on PATH; install it or add it to PATH.

## File views (ide-views): file tree, code view, diff view, changes list

tree-open = Open
tree-new-file = New file
tree-new-folder = New folder
tree-loading = Loading…
tree-name-placeholder = Name
code-find-placeholder = Find
code-find-none = No results
code-find-prev = Previous match (Shift+Enter)
code-find-next = Next match (Enter)
code-goto-placeholder = Go to line (line:column)
code-modified = Modified
code-revert = Revert
code-save = Save (Ctrl+S)
diff-binary = Binary file not shown.
diff-no-changes = No content changes (only metadata such as permissions changed).
diff-renamed-only = Renamed without content changes: { $from } → { $to }
diff-expand-lines = { $count ->
    [one] Show 1 unchanged line
   *[other] Show { $count } unchanged lines
}
diff-unified = Unified view
diff-split = Side-by-side view
diff-no-newline = No newline at end of file
changes-staged = Staged
changes-unstaged = Changes
changes-untracked = Untracked
changes-empty = No changes.
git-modified = Modified
git-added = Added
git-deleted = Deleted
git-untracked = Untracked
git-renamed = Renamed
git-conflicted = Conflicted

# Detail area (open items, side panel)
open-items = Open items
group-browser = Browser
side-changes = Changes
side-explore = Explore
changes-count = { $count ->
    [one] 1 changed file
   *[other] { $count } changed files
}
kind-file = File
kind-changed = Changed
kind-staged = Staged
kind-compare = Disk vs. unsaved edits
file-binary = Binary file ({ $size }): not shown.
file-truncated = Large file ({ $size }): only the first part is shown, read-only.
file-conflict = The file changed on disk since it was opened. Your edits were not saved.
file-changed-on-disk = The file changed on disk. Your edits are not saved yet.
file-deleted-on-disk = The file was deleted on disk.
bar-reload = Reload from disk (discard my edits)
bar-overwrite = Save my version
bar-compare = Compare with the disk
explore-find-placeholder = Find files
explore-contents = Contents
search-case = Match case
search-word = Whole word
search-regex = Regular expression
search-running = Searching…
search-summary = { $matches ->
    [one] 1 result
   *[other] { $matches } results
} in { $files ->
    [one] 1 file
   *[other] { $files } files
}
search-truncated = (limit reached)
search-none = No results.
quick-open-placeholder = Go to file
quick-open-none = No matching files.
def-pick-title = Definitions of “{ $name }”
delete-entry-title = Delete “{ $name }”?
delete-entry-body = The file is deleted permanently.
delete-folder-body = The folder and everything in it are deleted permanently.
unsaved-title = Save changes to “{ $name }”?
unsaved-body = Your changes are lost if you don't save them.
dont-save = Don't save
browse-up = Parent folder
browse-empty = No subfolders.
browse-git = Git repository
pick-root-title = Add a repository
pick-root-note = Folders on the server. Double-click a folder (or press Enter) to open it.
pick-start = Computer
pick-select = Select this folder
pick-truncated = Too many folders: only the first ones are shown.
remove = Remove

# Phase 2 server errors
err-path-invalid = Invalid path: { $path }
err-fs-not-found = Not found: { $path }
err-fs-exists = Already exists: { $path }
err-fs-permission = Permission denied: { $path }
err-fs-dir-not-empty = The folder is not empty: { $path }
err-fs-not-a-dir = Not a folder: { $path }
err-fs-is-a-dir = { $path } is a folder.
err-fs-io = Could not access { $path }: { $detail }
err-fs-conflict = { $path } changed on disk since it was read.
err-fs-bad-encoding = Unknown text encoding: { $encoding }
err-fs-unencodable = { $path } contains characters the { $encoding } encoding cannot store.
err-watch-failed = Cannot watch the folder for changes: { $detail }
err-search-empty = Enter something to search for.
err-search-bad-regex = Invalid regular expression: { $detail }
err-search-bad-glob = Invalid file pattern { $glob }: { $detail }
err-git-bad-rev = Invalid revision: { $rev }
err-git-no-base = Base branch not found: { $base }
err-def-invalid-position = There is nothing to look up at this position.
err-def-unsupported-file = Go to definition is not available for this file type: { $path }
err-def-no-name = Put the cursor on a name to go to its definition.
err-def-external-module = { $spec } is an external module.
err-def-module-not-found = Module not found: { $spec }
err-def-external-name = Defined in the external module { $spec }.
err-def-not-found = No definition found for “{ $name }”.

## Accounts and configurations (settings, new-session dialog)

configuration-label = CONFIGURATION
account-label = ACCOUNT
account-default = Default login (the CLI's own settings)
accounts-note = An account is one login of an agent CLI with its own config folder. Adding one opens its login in a terminal.
account-name-placeholder = Account name
account-add = Add and log in
account-check = Status
account-login = Log in
account-status-checking = checking…
account-logged-in = logged in
account-logged-out = not logged in
account-status-unknown = unknown
account-delete-tooltip = Delete the account and its login files
configurations-note = Skills, MCP servers and instructions, added to any agent and account.
configuration-name-placeholder = Configuration name
configuration-name-label = Name
configuration-edit = Edit
configuration-delete-tooltip = Delete the configuration
configuration-instructions = Instructions
configuration-skills = Skills (folders with a SKILL.md)
configuration-skill-placeholder = Skill folder on the server
configuration-mcp = MCP servers (stdio)
configuration-sessions = { $total ->
    [one] 1 session
   *[other] { $total } sessions
}, { NUMBER($running) } running
mcp-name-placeholder = name
mcp-command-placeholder = command
mcp-args-placeholder = arguments
mcp-env-placeholder = KEY=value

# Repositories (Hermes Desk's repo registry, 레포 page, session workspaces)
group-login = Login terminals
no-repos = No repositories
repos-title = Repositories
repos-none = No repositories registered.
repos-list-label = REPOSITORIES
repo-add = Add repo
repo-path-placeholder = Repository path on the server (Enter to add)
repos-pick-note = Add a repository or pick one on the left.
repos-note = Sessions get one worktree per repository in their own folder. Only registered git repositories can be used.
repo-no-sessions = No session uses this repository.
repo-unregister = Unregister
repo-unregister-keep-files = Unregister (keep files)
repo-base-label = BASE BRANCH
repo-base-unknown = (could not be determined)
repo-sessions-label = { $count ->
    [one] 1 SESSION
   *[other] { $count } SESSIONS
}
provider-support-modes = { $provider }: workspace { $workspace } · original repo { $original } (skills, MCP, instructions)
configuration-default-repos = Default repositories
configuration-default-repos-note = Pre-checked when a session is created with this configuration; each session can change them.
workspace-label = WORKSPACE
use-worktrees = Use worktrees
use-worktrees-note = The session gets its own folder with a worktree of each repository (branch ide/<session-id>), separate from the originals.
worktree-repos-label = Repositories to create worktrees for
no-repos-empty-workspace = No repositories registered: the session starts with an empty workspace.
empty-workspace-note = No repository checked: the session starts with an empty workspace.
worktree-repos-note = The first selection is the configuration's default repositories; repositories can be added later from the session's ⚙.
original-repo-warning = The agent edits and commits in the repository's own folder and current branch. Other sessions and your own work in that repository can get mixed up; clean up uncommitted changes first.
original-repo-label = Repository to work in
no-repos-register-first = No repositories registered. Register one on the Repos page first.
session-repos-title = Repositories of “{ $title }”
session-repos-label = Repositories · branch ← base
session-repo-detach = Remove this worktree from the session (its branch is kept)
session-repos-add-label = Add repositories
session-repos-note = Added repositories get a worktree in the session folder when you save; a running agent sees them in its guidance after a restart.
session-repos-no-workspace = This session works in a repository directly and has no workspace, so repositories cannot be added.
session-repos-open = Session repositories
no-repos-in-session = This session has no repositories. Add one with ⚙.
err-some-worktrees-failed = Some worktrees could not be created:
    { $detail }
err-repo-not-found = Repository { $id } is not registered.
err-repo-in-use = { $count ->
    [one] 1 session uses
   *[other] { $count } sessions use
} repository “{ $name }”; delete them first.
err-repo-not-attached = Repository “{ $name }” is not attached to this session.
err-not-a-workspace-session = Session { $id } works in a repository directly (no workspace).

## Base branches, 기준 브랜치 최신화, git init of a plain folder (Hermes Desk)
base-mode-fixed = Fixed branch
base-mode-rule = Pick by rule
base-rule-active-warning = A rule picks the base branch right now. Saving here removes the rule and uses a fixed branch.
base-fixed-note = The default base branch of new sessions. Each session can use another branch (new-session dialog, session repositories); a session's status follows its own base branch. Changing it keeps the base branch of existing sessions.
base-fixed-pick-note = Local and remote-tracking branches. Saving does not fetch from the remote.
base-auto = Automatic (default branch)
base-loading = Loading branches…
base-missing = { $branch } (not present locally)
base-missing-error = The configured branch does not exist. Pick another branch or reset to automatic.
base-auto-note = Automatic: main → master → origin's default branch → the current branch. Remote-tracking branches are used as last fetched on this computer.
base-reset-auto = Reset to automatic
base-saved = Saved.
base-rule-note = Every new worktree checks the remote's branch list and starts from the highest version among the branches matching the rule. If the remote cannot be reached or nothing matches, creating fails instead of using the previous branch.
base-rule-remote = Remote
base-rule-pattern = Candidate pattern
base-rule-pattern-note = * matches anything, including /
base-rule-match = Allowed format (regex, optional)
base-rule-match-placeholder = Empty: every branch matching the pattern
base-rule-version-note = Compared by version numbers (2026.9 < 2026.19 < 2026.20). To exclude suffixed branches such as -rc or -hotfix, set an allowed format.
base-rule-current = Branch that applies now
base-rule-recheck = Check remote again
base-rule-no-pick = No matching branch → creating sessions fails
base-rule-loading = Loading remote branches…
base-rule-excluded = excluded by the allowed format
base-rule-clear = Remove rule
base-rule-last-pick = Last session: { $last } · from the next session: { $pick }
new-refresh-base = Refresh base branch
new-refresh-base-note = Creates from the remote's latest commit. The local base branch is not changed; if fetching fails, that repository is not created.
new-base-note = Each repository gets a new branch from its base branch, and the session's status is measured against that base. The default is the repository's base branch.
base-pick-label = Base branch
base-no-origin-branches = No origin branches
base-changed-by-rule = Base branches changed by their rule:
    { $detail }
session-repos-base-note = Per repository, what the session's status is measured against. Changing it does not recreate or rebase the session branch.
init-repo-title = Not a Git repository
init-repo-message = This folder is not a Git repository. Initialise it and add it?
init-repo-detail = Runs git init in this folder, makes a first commit of the current files and adds it as a repository. The folder gets a .git folder; every file not in .gitignore goes into the first commit.
init-repo-no-remote = Without a remote, push and pull requests are not available (they work once a remote is added).
init-repo-confirm = Initialise and add
err-not-a-git-folder = { $path } is not a Git repository.
err-folder-not-found = Folder not found: { $path }
err-corrupt-git-dir = A broken .git is in the way, so the folder was not initialised: { $path }
err-git-timeout = git { $command } was stopped after { $secs } s.
err-base-refresh-failed = Refreshing the base branch failed: { $detail }
err-base-no-remote = The base branch has no remote. Turn off refreshing the base branch or set an upstream.
err-base-not-branch = Choose a base branch to refresh.
err-base-fetch-failed = Could not fetch the base branch: { $detail }
err-base-rule-no-match = No remote branch matches the base branch rule.
err-invalid-base-rule-remote = The remote name is not valid.
err-invalid-base-rule-pattern = Enter a candidate pattern.
err-invalid-base-rule-match = The regex is not valid.
err-remote-not-found = There is no remote “{ $remote }”.
err-remote-list-failed = Could not list the remote branches: { $detail }
err-invalid-base-branch = The base branch is not valid or not present locally: { $branch }
err-origin-branch-missing = origin has no branch { $branch } (turn on refreshing the base branch).
err-origin-branch-unknown = origin has no branch { $branch } (fetch and try again).

## Session list: configuration filter (all) and sorting

configurations-all = All
group-fold = Collapse “{ $name }”
group-unfold = Expand “{ $name }”
session-sort = Sort sessions
session-sort-tooltip = Sort sessions: { $summary }
session-sort-default = Default order
session-sort-reset = Default order
session-sort-default-note = Default order. Add a criterion to sort automatically.
session-sort-add = Add sort criterion
session-sort-help = Criteria apply from the top; use the arrows to change priority. Configuration groups stay, equal values keep their previous order and items without a value come last.
sort-key-name = Name
sort-key-updated = Last activity
sort-key-created = Created
sort-key-activity = Run state
sort-asc = Ascending
sort-desc = Descending
sort-move-up = Raise priority
sort-move-down = Lower priority
sort-remove = Remove criterion
