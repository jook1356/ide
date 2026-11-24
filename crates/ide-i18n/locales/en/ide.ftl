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
nav-configurations = Profiles
nav-repos = Repos
nav-widgets = Widgets
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
detail-toggle = Detail area
composer-toggle = Input
composer-show = Show the input box under the terminal
composer-hide = Hide the input box (type in the terminal)
composer-placeholder = Message (Enter to send, Shift+Enter for a new line, ↑ for earlier ones)
composer-send-tip = Send (Enter): paste into the terminal and press Enter
attach-no-folder-remote = A folder cannot be sent to a remote server; drop its files instead.
tab-web = Web
web-back = Back
web-forward = Forward
web-reload = Reload
url-placeholder = Enter a URL
no-session = No session

## Detail window (the detail area in a window of its own, like Hermes Desk's region window)

detail-popout = Open in a new window
detail-window-title = Detail · { $session }
detail-window-title-idle = Detail
detail-window-pin = Always show above the app window
detail-window-unpin = Unpin (no longer shown above the app)
detail-window-merge = Merge into app
detail-window-merge-tip = Merge into the app window
detail-window-hide = Hide the detached detail window
detail-window-show = Show the detail area in its window
window-minimize = Minimize
window-maximize = Maximize
window-restore = Restore Down
rail-settings = Open items settings
rail-show-as = Show open items as
rail-tabs = Vertical tabs
rail-popover = Popover
rail-side = Tab position
rail-left = Left
rail-right = Right
close-all = Close all tabs
close-all-tip = Closes every tab opened from Changes, History and Explore (the web page stays)
close-idle = Close idle items
close-idle-tip = Closes saved files, changes and commits; files with unsaved edits stay
close-others = Close other items
close-many-title = Unsaved changes
close-many-body = These files have unsaved edits. Closing them discards the edits.
close-many-discard = Discard and close
close-many-save = Save and close
open-items-none = None
item-copy-path = Copy path
item-copy-rel = Copy relative path
item-reveal-windows = Show in Windows Explorer
item-reveal-finder = Reveal in Finder
item-reveal-files = Show in the file manager
item-open-vscode = Open in VS Code
item-copy-sha = Copy commit SHA
item-copy-subject = Copy commit subject
item-copy-url = Copy address
item-open-external = Open in external browser
item-copied-path = Path copied
item-copied-rel = Relative path copied
item-copied-sha = Commit SHA copied
item-copied-subject = Commit subject copied
item-copied-url = Address copied
item-reveal-os-failed = Could not open the file explorer: { $error }
item-vscode-failed = Could not open VS Code: { $error }
item-external-failed = Could not open the external browser: { $error }
chg-filter = Filter
explore-settings = File search settings
explore-hide-ignored = Hide gitignored files
explore-hide-ignored-tip = When checked, files and folders matched by .gitignore are left out of the results
explore-more = { $count ->
    [one] 1 more. Narrow the search.
   *[other] { $count } more. Narrow the search.
}
foot-update-branch = Update branch
foot-update-branch-hint = Brings the base branch's new commits into the PR branch on GitHub
foot-branch-updating = Updating the branch
foot-update-branch-failed = Branch update refused: { $error }
badge-checking = Checking
badge-failed = Check failed
badge-none = No changes
badge-none-tip = Nothing to commit, push or merge
badge-dirty = Commit needed
badge-ahead = Push needed
badge-publish = Publish needed
badge-ahead-tip = { $count ->
    [one] 1 commit not on the remote yet
   *[other] { $count } commits not on the remote yet
}
badge-pr = PR open
badge-pr-conflict = PR conflict
badge-unmerged = Has commits
badge-unmerged-tip = { $count ->
    [one] 1 commit the base branch does not have yet · no PR
   *[other] { $count } commits the base branch does not have yet · no PR
}
badge-unmerged-conflict-tip = { $count ->
    [one] Conflicts with the base branch · cannot tell (1 commit) · no PR
   *[other] Conflicts with the base branch · cannot tell ({ $count } commits) · no PR
}
badge-merged = Merged
group-terminal = Terminal
browser-new-tab = New tab
bookmark-add = Add to bookmarks
bookmark-remove = Remove from bookmarks
bookmark-rename = Rename
new-scope-session = This session
new-scope-global = Shared
new-scope-session-hint = Belongs to this session. Closed when the session is deleted.
new-scope-global-hint = Opens without a session. Stays when you switch or delete sessions.
new-web = Web
new-web-open = Open web page
new-widgets = Widgets
widget-devtools = Developer tools
widget-grant-note = Its page can then call them by itself. A widget written by the agent runs with your permissions in ide.
new-web-placeholder = Address or search (e.g. localhost:5173)
uterm-in-home = Home folder
uterm-pick-folder = Choose folder…
uterm-start-in = Start in
uterm-default-shell = Default shell
uterm-make-default = Make default
pick-term-title = Terminal start folder
pick-skill-title = Import a skill folder
new-running-servers = { $count ->
    [one] Running server · 1
   *[other] Running servers · { $count }
}
global-item-tip = Shared (no session · stays when the session is deleted)
common-current = current
common-pin = Pin as a common tab (shown in every session)
common-unpin = Unpin (back to this session)
new-item = New
new-item-tip = Open new (terminal · web page · Alt+click: shared)
uterm-new-title = Terminal
uterm-in-session = Terminal · session folder
uterm-running = Running
uterm-done = Exited
uterm-failed = Exited (code { $code })
uterm-stop = Stop
uterm-rerun = Run again
uterm-remove = Remove from list
uterm-stop-remove = Stop and remove from list
uterm-copy-command = Copy command
err-uterm-no-shell = No shell was found on the server.
err-uterm-not-found = The terminal is gone: { $id }
item-copied-command = Command copied
scripts-title = Scripts
scripts-refresh = Find again
scripts-search = Scripts · file name
scripts-run = Run
scripts-running = Running · show the terminal
scripts-search-toggle = Search
scripts-expand-all = Expand all
scripts-collapse-all = Collapse all
scripts-stack-expand = Expand all of { $label }
scripts-stack-collapse = Collapse all of { $label }
scripts-stack-files = { $count ->
    [one] { $count } file
   *[other] { $count } files
}
scripts-none = No scripts to run were found
scripts-no-match = No matching scripts
rail-pin = Pin the tab list
rail-unpin = Unpin the tab list
rail-empty = No open items
rail-to-tabs = Show as vertical tabs

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
old-server-title = Restart the server
old-server-body = The running server is from another ide build and speaks another protocol, so this app cannot connect to it. Restarting ends that server (its running sessions stop) and starts this app's server. Sessions are kept and can be started again.
old-server-note = Not connected: the running server is from another ide build. Restart it to connect.
old-server-confirm = Restart server
old-server-failed = Could not stop the server: { $error }
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
font-label = FONTS
font-ui = UI
font-code = Code
font-terminal = Terminal
font-default = Default ({ $name })
font-reset = Default
font-placeholder = Type or pick a font
font-restart-note = The UI font applies after the app restarts.
font-system = system
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
err-session-flags-failed = Could not change the session
err-lists-failed = Could not load the lists from the server
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
err-configuration-not-found = Profile “{ $name }” not found.
err-configuration-in-use = { $count ->
    [one] 1 session uses
   *[other] { $count } sessions use
} profile “{ $name }”; delete them first.
err-last-configuration = Cannot delete the last profile “{ $name }”.
err-skill-not-found = Skill “{ $name }” not found.
err-skill-exists = A skill named “{ $name }” exists already.
err-skill-invalid = Not a skill folder (no SKILL.md): { $path }
err-attach-too-large = The attachment is larger than { $max } MB.
err-attach-not-found = The attachment upload “{ $name }” was not found.
err-provider-not-found = The agent “{ $provider }” is not available on the server.
err-provider-no-login = The agent “{ $provider }” has no accounts or login command.
err-invalid-mcp-name = MCP server “{ $name }”: use a unique name of letters, digits, - and _, and a command.
name-rule-empty = must not be empty
name-rule-too-long = must be at most 64 characters
name-rule-invalid-chars = only lowercase letters, digits, '-' and '_' are allowed
name-rule-bad-start = must start with a letter or digit
err-session-not-found = Session { $id } not found.
err-session-preparing = This session's worktrees are still being created. Try again when they are ready.
err-invalid-session-id = Invalid session id “{ $id }”.
err-not-a-git-repo = { $path } is not a git repository: { $detail }
err-git-spawn = Failed to run git: { $detail }
err-sandbox-setup = The agent's sandbox setup did not finish: { $detail }
err-git = git { $args } failed ({ $status }): { $detail }
err-worktree-exists = Worktree path already exists: { $path }
err-program-not-found = “{ $program }” not found on PATH; install it or add it to PATH.

## File views (ide-views): file tree, code view, diff view, changes list

tree-new-file = New file
tree-new-folder = New folder
tree-attach = Attach to the input
tree-attach-many = Attach to the input ({ NUMBER($count) })
tree-cut = Cut
tree-copy = Copy
tree-paste = Paste
tree-copy-remote-url = Copy remote file URL
explore-cut-done = { $count ->
    [one] Cut 1 item
   *[other] Cut { $count } items
}
explore-copied-done = { $count ->
    [one] Copied 1 item
   *[other] Copied { $count } items
}
explore-reveal-remote = The files are on the remote server: they cannot be shown in this machine's file manager.
explore-paste-remote = Files from this machine cannot be pasted on a remote server yet.
explore-paste-other-repo = Cut or copied in another repository: paste it there.
explore-remote-url-copied = Copied the remote file URL
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
side-history = History
kind-commit = Commit
history-empty = No commits.
history-more = Show more
history-unpushed-count = { $count ->
    [one] 1 commit not pushed to a remote
   *[other] { $count } commits not pushed to a remote
}
history-unpushed-mark = Not pushed to a remote yet
history-show-files = Show changed files
history-hide-files = Collapse file list
history-load-failed = Couldn't load the commit list: { $error }
time-just-now = just now
time-minutes = { $count ->
    [one] 1 minute ago
   *[other] { $count } minutes ago
}
time-hours = { $count ->
    [one] 1 hour ago
   *[other] { $count } hours ago
}
time-yesterday = yesterday
time-days = { $count ->
    [one] 1 day ago
   *[other] { $count } days ago
}
time-last-month = last month
time-months = { $count ->
    [one] 1 month ago
   *[other] { $count } months ago
}
time-last-year = last year
time-years = { $count ->
    [one] 1 year ago
   *[other] { $count } years ago
}
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
delete-entry-body = The file goes to the recycle bin.
delete-folder-body = The folder and everything in it go to the recycle bin.
delete-many-title = { $count ->
    [one] Delete 1 item?
   *[other] Delete { $count } items?
}
delete-many-body = The selected items go to the recycle bin.
unsaved-title = Save changes to “{ $name }”?
unsaved-body = Your changes are lost if you don't save them.
dont-save = Don't save
browse-up = Parent folder
browse-empty = No subfolders.
browse-git = Git repository
pick-root-title = Add a repository
pick-root-note = Folders on the server. Double-click a folder (or press Enter) to open it.
pick-path-placeholder = Folder path on the server (Enter to open)
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

configuration-label = PROFILE
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
configuration-name-placeholder = Profile name
configuration-name-label = Name
profile-add = Add profile
profiles-hide = Hide the profile list
profiles-show = Show the profile list
profiles-none = No profiles yet.
profile-row-detail = { $sessions ->
    [one] 1 session
   *[other] { $sessions } sessions
} · { $skills ->
    [one] 1 skill
   *[other] { $skills } skills
}
profile-unsaved = Unsaved
profile-revert = Revert
profile-delete-question = Delete this profile?
profile-section-basic = Basics
profile-instructions-note = Added after the system (developer) prompt of agents started with this profile.
profile-mcp-note = Local (stdio) MCP servers added to the agent: name, command, arguments, KEY=VALUE environment variables.
mcp-none = No MCP servers.
configuration-delete-tooltip = Delete the profile
configuration-instructions = Instructions
configuration-skills = Skills
skills-count = { $total ->
    [one] 1 skill
   *[other] { $total } skills
}, { NUMBER($on) } on
skills-none = No skills yet.
skills-note = Changes apply right away, to new sessions. Each skill is a folder with a SKILL.md, kept with the profile.
skill-new = New skill
skill-import = Import folder
skill-name-placeholder = Name (e.g. pdf-tools)
skill-description-placeholder = What it does and when to use it
skill-new-note = Creates a SKILL.md you can edit afterwards.
skill-create = Create
skill-edit = Edit
skill-edit-tip = Edit SKILL.md
skill-delete-tip = Delete the skill
skill-edit-title = Edit skill
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
repos-hide = Hide the repository list
repos-show = Show the repository list
repo-add = Add repo
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
configuration-default-repos-note = Pre-checked when a session is created with this profile; each session can change them.
workspace-label = WORKSPACE
use-worktrees = Use worktrees
use-worktrees-note = The session gets its own folder with a worktree of each repository (branch ide/<session-id>), separate from the originals.
worktree-repos-label = Repositories to create worktrees for
new-repos-search = Search repositories (name or path)
new-repos-search-none = No repositories match.
new-repos-selected = { $count ->
    [one] 1 selected
   *[other] { $count } selected
}
new-repo-chip-remove = Uncheck
no-repos-empty-workspace = No repositories registered: the session starts with an empty workspace.
empty-workspace-note = No repository checked: the session starts with an empty workspace.
worktree-repos-note = The first selection is the profile's default repositories; repositories can be added later from the session's ⚙.
original-repo-warning = The agent edits and commits in the repository's own folder and current branch. Other sessions and your own work in that repository can get mixed up; clean up uncommitted changes first.
original-repo-label = Repository to work in
no-repos-register-first = No repositories registered. Register one on the Repos page first.
session-name-label = Session name
session-repos-label = Repositories · branch ← base
session-repo-detach = Remove this worktree from the session (its branch is kept)
session-repos-add-label = Add repositories
session-repos-note = Added repositories get a worktree in the session folder when you save; a running agent sees them in its guidance after a restart.
session-repos-no-workspace = This session works in a repository directly and has no workspace, so repositories cannot be added.
session-repos-open = Session repositories
activity-working = Working
activity-needs-input = Needs you
activity-new-answer = New answer
activity-preparing = Creating worktrees
child-create-menu = Add a child session
child-dialog-title = Child session of “{ $parent }”
child-title-placeholder = Title (empty: derived)
child-separate = Own worktrees
child-separate-hint = Branched from the parent’s branches; merge it back later.
child-shared = Shared folder
session-own-worktrees = Works in its own worktrees (branches)
session-unguarded = Writes outside this session are not guarded: its account has no current ide integration. Install it from the account's settings (Accounts), then approve the hooks once in the CLI.
child-shared-hint = Works in the parent’s folder and worktrees.
child-repos-label = Repositories (the parent’s)
child-create = Create
child-merge = Merge into the parent
child-sync = Get the parent’s changes
family-fold = Fold child sessions
family-unfold = Unfold child sessions
merge-nothing = Nothing to merge.
merge-merged = { $name }: merged
merge-up-to-date = { $name }: already up to date
merge-conflicts = { $count ->
    [one] { $name }: 1 file in conflict; resolve it in Changes and commit
   *[other] { $name }: { $count } files in conflict; resolve them in Changes and commit
}
merge-failed = { $name }: failed, nothing changed ({ $error })
integration-checking = Checking the ide integration…
integration-missing = ide integration: not installed
integration-installed = ide integration: installed
integration-outdated = ide integration: outdated
integration-install = Install
integration-update = Update
integration-uninstall = Remove
integration-tooltip = Adds ide’s hooks to this account’s CLI settings (only ide’s entries; your own hooks stay), so ide sees what the agent does and can refuse edits in shared folders. If the CLI needs its sandbox set up to write, that is done once too (Windows: an administrator prompt).
integration-approve = Approve ide’s hooks once in the { $provider } CLI with { $command }.
update-button = Build ide and restart
update-ready = New build ready · click to restart
update-confirm-title = Build ide and restart
update-confirm-body = Pulls the repository (if it cannot, builds the code as it is) and builds a new ide. You can keep using this ide while it builds; when it is done a restart icon appears in the footer. Clicking it closes ide and opens the new build. The server and its sessions stay (if the server protocol changed, restart the server yourself). If the new ide does not come up, the previous one comes back.
update-start = Build and restart
update-step-pulling = Pulling the repository (1/2)
update-step-building = Building (2/2)
update-step-swapping = Restarting
update-done = ide updated ({ $from } → { $to })
update-done-same = ide rebuilt
update-done-later = Build done: the next start uses it
update-failed = ide update failed: { $detail }
update-rolled-back = The new build did not come up: the previous ide is back
update-start-failed = Could not start the update: { $error }
session-pin = Pin
session-unpin = Unpin
session-archive = Move to archive
session-unarchive = Take out of archive
session-edit-start = Edit sessions
session-edit-end = Finish editing
session-edit-check-all = Select all
session-edit-none = Select sessions
session-edit-count = { $count ->
    [one] 1 selected
   *[other] { $count } selected
}
session-edit-more = More
session-edit-done = Done
session-edit-delete-title = { $count ->
    [one] Delete 1 session
   *[other] Delete { $count } sessions
}
session-edit-delete-body = The selected sessions are stopped and removed from the list. Child sessions are deleted before their parents.
session-edit-delete-warning = Uncommitted changes and unpushed commits are lost with the worktrees. To see what is left first, delete sessions one at a time.
session-edit-delete-confirm = { $count ->
    [one] Delete 1
   *[other] Delete { $count }
}
archive-label = Archive
archive-fold = Fold archive
archive-unfold = Unfold archive
session-open-folder = Open working folder
session-copy-folder = Copy working folder path
session-copy-id = Copy session ID
session-settings = Session settings
session-copied-folder = Working folder path copied
session-copied-id = Session ID copied
session-open-folder-failed = Could not open the working folder: { $error }
no-repos-in-session = This session has no repositories. Add one with ⚙.
err-some-worktrees-failed = Some worktrees could not be created
err-repo-not-found = Repository { $id } is not registered.
err-repo-in-use = { $count ->
    [one] 1 session uses
   *[other] { $count } sessions use
} repository “{ $name }”; delete them first.
err-repo-not-attached = Repository “{ $name }” is not attached to this session.
err-not-a-workspace-session = Session { $id } works in a repository directly (no workspace).
err-family-needs-workspace = Session { $id } works in a repository directly: it cannot have child sessions.
err-family-too-deep = Child sessions go at most two levels deep.
err-repo-not-in-parent = The parent session has no repository “{ $name }”.
err-widget = Widget: { $detail }
err-widget-changed = The draft of the “{ $widget }” widget changed since you were asked. Ask the agent to register it again.
err-widget-not-allowed = The widget may not use { $tool } (not declared or not allowed).
err-approval-not-found = That approval is gone already.
err-session-has-children = { $count ->
    [one] This session has a child session; delete it first.
   *[other] This session has { $count } child sessions; delete them first.
}
err-repo-used-by-children = { $count ->
    [one] A child session works in
   *[other] { $count } child sessions work in
} repository “{ $name }”.
err-shared-child-repos = A shared child session uses its parent's repositories: change them in the parent.

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
base-changed-by-rule = Base branches changed by their rule
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
session-sort-help = Criteria apply from the top; use the arrows to change priority. Profile groups stay, equal values keep their previous order and items without a value come last.
sort-key-name = Name
sort-key-updated = Last activity
sort-key-created = Created
sort-key-activity = Run state
sort-asc = Ascending
sort-desc = Descending
sort-move-up = Raise priority
sort-move-down = Lower priority
sort-remove = Remove criterion

## Changes tab actions (Hermes Desk ChangesPanel / StashSection / CommitContextMenu / BranchSelect)
chg-select-all = Select all
chg-stash-all-tip = Stash changes (stash)
chg-stash-checked-tip = { $count ->
    [one] Stash 1 checked file (stash)
   *[other] Stash { $count } checked files (stash)
}
chg-stash-checked-body = { $count ->
    [one] Stash the checked file
   *[other] Stash the { $count } checked files
}
chg-stash-all-body = { $count ->
    [one] Stash the 1 change
   *[other] Stash all { $count } changes
}
chg-stash-memo = Note (optional)
chg-stash-untracked = New (untracked) files are stashed too
chg-stash-confirm = Stash
chg-stash-done = { $count ->
    [one] Stashed 1 change
   *[other] Stashed { $count } changes
}
chg-stash-failed = Stash failed: { $error }
chg-discard-tip = Discard changes (back to the last commit)
chg-reveal = Show in Explore
chg-discard = Discard changes
chg-discard-checked = { $count ->
    [one] Discard all checked files (1)
   *[other] Discard all checked files ({ $count })
}
chg-discard-body = The uncommitted changes of these files go back to the last commit. Discarded changes cannot be recovered.
chg-discard-new = New files will be deleted.
chg-discard-done = { $count ->
    [one] Discarded the changes of 1 file
   *[other] Discarded the changes of { $count } files
}
chg-discard-failed = Some files could not be reverted:
    { $error }
chg-stash-title = Stash
chg-stash-auto = Stashed when switching branches
chg-stash-no-memo = No note
chg-stash-meta = { $count ->
    [one] 1 file · { $time }
   *[other] { $count } files · { $time }
}
chg-no-branch = no branch
chg-stash-pop-tip = Pop: restore the changes and remove them from the stash
chg-stash-apply-tip = Apply: restore the changes and keep them in the stash
chg-stash-drop-tip = Drop
chg-stash-popped = Popped the stash
chg-stash-applied = Applied the stash
chg-stash-dropped = Dropped the stash
chg-stash-pop-failed = Pop failed: { $error }
chg-stash-apply-failed = Apply failed: { $error }
chg-stash-drop-failed = Drop failed: { $error }
chg-stash-conflicts = { $count ->
    [one] 1 file conflicted. Resolve it in Changes (the stash was kept).
   *[other] { $count } files conflicted. Resolve them in Changes (the stash was kept).
}
chg-stash-others = { $count ->
    [one] Show 1 stash of other branches
   *[other] Show { $count } stashes of other branches
}
chg-stash-mine = Show this branch's only
chg-stash-drop-title = Drop stash
chg-stash-drop-body = { $count ->
    [one] Drops “{ $title }” with its change. This cannot be undone.
   *[other] Drops “{ $title }” with its { $count } changes. This cannot be undone.
}
chg-stash-drop = Drop
chg-copy-sha = Copy SHA
chg-copy-subject = Copy commit subject
chg-copied-sha = Copied the commit SHA
chg-copied-subject = Copied the commit subject
chg-copy-failed = Copy failed
chg-commit-checkout = Check out this commit (detached HEAD)
chg-commit-branch = Create a branch from this commit
chg-commit-revert = Revert this commit (revert)
chg-commit-cherry-pick = Apply to the current branch (cherry-pick)
chg-commit-checkout-desc = Switches to a detached HEAD that belongs to no branch. Create a branch first for new work.
chg-commit-branch-desc = Creates a new local branch at the selected commit and switches to it. No remote tracking branch is set.
chg-commit-revert-desc = Adds a new commit to the current branch that undoes the selected commit. Existing history is not rewritten.
chg-commit-cherry-pick-desc = Applies the selected commit's changes to the current branch as a new commit.
chg-commit-target = Target: { $path }
chg-commit-branch-name = New branch name
chg-commit-warning = Nothing runs while there are uncommitted or untracked files or a merge / rebase / cherry-pick / revert in progress. Reverting or cherry-picking merge commits is not supported. If a conflict happens, resolve or abort the git operation outside and try again.
chg-commit-run = Confirm and run
chg-commit-done = Git operation completed
chg-refresh = Refresh repository state
chg-picker-repos = Repos
chg-picker-branches = Branches
chg-new-branch = New branch
chg-branch-search = Search branches
chg-local = Local
chg-remote = Remote
chg-more-rows = { $count ->
    [one] 1 more — narrow it down with the search
   *[other] { $count } more — narrow it down with the search
}
chg-fetch-failed = Couldn't fetch the remote list (check authentication)
chg-in-use-main = Also checked out in the main repo ({ $path }) — you can switch to it
chg-in-use-session = In use by session { $session }, so you can't switch to it
chg-in-use-path = In use at { $path }, so you can't switch to it
chg-remote-tip = Remote branch — fetched, then switched to
chg-switch-title = Switch branch
chg-switch-body = Switches this session's worktree: { $from } → { $to }
chg-switch-remote-body = Fetches the remote branch { $remote }/{ $branch } as a local branch of the same name and switches this session's worktree: { $from } → { $branch }
chg-dirty = { $count ->
    [one] 1 uncommitted change
   *[other] { $count } uncommitted changes
}
chg-mode-carry = Bring them along
chg-mode-stash = Stash on the current branch
chg-mode-carry-note = Moves the changes as they are (no switch on a conflict)
chg-mode-stash-note = Stashes them on the current branch (restore them when you come back)
chg-switch-agent-note = If the agent is working, the files it works on change. Switch while the conversation is idle.
chg-switch-fetching = Fetching and switching… ({ $remote }/{ $branch })
chg-switch = Switch
chg-switched = Switched branch: { $branch }
chg-switched-stash = Stashed the changes and switched branch: { $from } → { $to }
chg-switched-remote = Fetched the remote branch and switched: { $remote }/{ $branch }
chg-switch-failed = Switching branches failed: { $error }
chg-switch-remote-failed = Switching to the remote branch failed: { $error }
chg-branch-name = Branch name
chg-branch-taken = This branch already exists
chg-base-branch = Base branch
chg-created = Created a new branch and switched to it: { $base } → { $name }
chg-create-failed = Creating the branch failed: { $error }
err-git-bad-branch = Not a valid branch name: { $name }
err-git-branch-exists = Branch { $name } already exists.
err-git-branch-checked-out = Branch { $branch } is checked out in another worktree: { $path }
err-git-unknown-remote = No such remote: { $remote }
err-git-detached = Not on a branch (detached HEAD).
err-git-no-upstream = The branch has no upstream.
err-git-push-changed = The branch changed after it was checked; nothing was pushed.
err-git-busy = Another git action is running in this worktree.
err-git-in-progress = A merge, rebase, cherry-pick or revert is in progress.
err-git-dirty = There are uncommitted or untracked files.
err-git-merge-commit = Reverting or cherry-picking a merge commit is not supported.
err-git-stash-empty = Nothing to stash.
err-git-stash-gone = The stash no longer exists.
err-git-refresh-moved = The branch changed meanwhile; open the dialog again.
err-git-refresh-conflict = Moving { $sha } { $subject } would conflict ({ $files }{ $more ->
    [0] {""}
   *[other] {" "}and { $more } more
})
err-git-merge-tree = git 2.40 or newer is needed (merge-tree): { $detail }

## Changes footer and pull requests

foot-commit = Ask to commit
foot-commit-pick = Pick the files to commit
foot-commit-partial = { $count ->
    [one] The agent commits the 1 selected file
   *[other] The agent commits the { $count } selected files
}
foot-commit-all = The agent looks at the changes, writes a commit message and commits
foot-prompt-failed = Could not send the request: { $error }
foot-more = More
foot-no-remote = No remote
foot-no-remote-tip = A local repository without a remote. Push and pull work once you add a remote (origin).
foot-sync-publish = Publish branch
foot-sync-pull = Pull
foot-sync-push = Push
foot-sync-fetch = Fetch
foot-sync-options = Sync options
foot-menu-fetch = Fetch
foot-menu-pull = Pull (merge)
foot-menu-rebase = Pull (rebase)
foot-menu-caution = Needs care
foot-menu-force-push = Force push
foot-menu-reset = Overwrite with the remote
foot-hint-fetch = Checks the remote's latest state
foot-hint-merged = PR #{ $number } was merged. Nothing left to push · click to check the remote's latest state
foot-hint-merged-gone = PR #{ $number } was merged. Nothing left to push (remote branch deleted after the merge) · click to check the remote's latest state
foot-hint-publish = { $count ->
    [0] Not on the remote yet. Publishing pushes it to the remote (origin)
    [one] Not on the remote yet. Publishing pushes it to the remote (origin) · 1 commit
   *[other] Not on the remote yet. Publishing pushes it to the remote (origin) · { $count } commits
}
foot-hint-publish-gone = { $count ->
    [0] The remote branch was deleted (e.g. after a merge). Publishing pushes this branch again · nothing new to push
   *[other] The remote branch was deleted (e.g. after a merge). Publishing pushes this branch again · { $count } commits not on the remote
}
foot-hint-publish-pr = { $count ->
    [0] PR #{ $number } is open for this branch. Publishing adds to PR #{ $number }; afterwards it shows as push
   *[other] PR #{ $number } is open for this branch. Publishing adds { $count } commits to PR #{ $number }; afterwards it shows as push
}
foot-hint-pull = { $behind ->
    [one] 1 new commit on the remote
   *[other] { $behind } new commits on the remote
}{ $ahead ->
    [0] {""}
   *[other] {" "}({ $ahead } only local)
}
foot-hint-push = { $count ->
    [one] Pushes 1 commit to { $upstream }
   *[other] Pushes { $count } commits to { $upstream }
}
foot-hint-push-pr = { $count ->
    [one] Pushes 1 commit to { $upstream } · added to the open PR #{ $number }
   *[other] Pushes { $count } commits to { $upstream } · added to the open PR #{ $number }
}
foot-published = Published { $branch } to the remote
foot-pushed = Pushed { $branch }
foot-force-pushed = Force-pushed { $branch }
foot-pulled = Pulled the remote changes into { $branch }
foot-rebased = Rebased { $branch } on the remote changes
foot-reset-done = Overwrote { $branch } with the remote state
foot-sync-failed = { $op } failed: { $error }
foot-op-fetch = Fetch
foot-op-push = Push
foot-op-publish = Publish branch
foot-op-force-push = Force push
foot-op-pull = Pull
foot-op-rebase = Pull (rebase)
foot-op-reset = Overwrite with the remote
foot-remote-branch = the remote branch
foot-reset-title = Overwrite with the remote state?
foot-reset-body = { $count ->
    [0] Makes the local branch identical to { $upstream }. Local-only commits and uncommitted changes are lost and cannot be restored.
   *[other] Makes the local branch identical to { $upstream }. The { $count } local-only commits and uncommitted changes are lost and cannot be restored.
}
foot-reset-confirm = Overwrite
foot-force-title = Force push?
foot-force-body = { $count ->
    [0] Overwrites the remote branch with the local history. Refused for safety if someone pushed since the last fetch (--force-with-lease).
   *[other] Overwrites the remote branch with the local history ({ $count } remote-only commits are lost). Refused for safety if someone pushed since the last fetch (--force-with-lease).
}
foot-merge-item = Merge · #{ $number }
foot-merge-hint = The app merges this PR into the base branch
foot-merge-uncommitted = There are uncommitted changes
foot-merge-unpushed = There are commits not pushed yet
foot-view-pr = View PR
foot-close-item = Close PR · #{ $number }
foot-close-hint = Closes it without merging. The branch and commits stay
foot-recheck-item = PR needs a check
foot-recheck-hint = Check what changed after the request before the PR goes up
foot-create-pr = Open PR
foot-pr-hint = Pick the target branch; the agent writes the PR title and body, the app pushes and opens the PR
foot-pr-needs-workspace = PR requests work in workspace sessions
foot-pr-no-remote = A local repository without a remote cannot open a PR
foot-pr-nothing = No commits to put up
foot-continue = Continue on a new branch
foot-continue-hint = Makes a new branch from the latest base branch and moves the commits and changes you pick
foot-blocker-merged = Already merged
foot-blocker-closed = The PR is closed
foot-blocker-draft = It is a draft PR
foot-blocker-changes-requested = Changes were requested
foot-blocker-blocked = GitHub branch protection (required checks, reviews, …) has not passed yet
foot-blocker-conflict = It conflicts with the base branch
foot-blocker-behind = The base branch is ahead (update the branch)
foot-blocker-not-mergeable = GitHub says it cannot be merged
foot-recheck-strip = The PR needs a check before it goes up
foot-recheck-strip-action = Check
foot-recheck-strip-tip = New commits or changes appeared after the PR request; it waits before going up
foot-leftover-strip = There is PR text that was not put up
foot-leftover-strip-action = Open
foot-leftover-strip-tip = In the Open PR dialog you can put it up as is or have it written again
foot-merge-title = Merge PR #{ $number }
foot-merge-method = Merge method
foot-merge-delete-after = Delete the remote branch after merging
foot-merge-delete-branch = Delete the remote branch after merging
foot-merge-guard = Not merged if anything was pushed after the checked commit { $sha }. GitHub branch protection still applies.
foot-merge = Merge
foot-merging = Merging…
foot-merged = Merged PR #{ $number } · { $title }
foot-merged-branch-deleted = {" "}· remote branch deleted
foot-merge-failed = Could not merge PR #{ $number }: { $error }
foot-close-title = Close PR #{ $number }?
foot-close-body = Closes it without merging. The branch and commits stay, and it can be reopened on GitHub.
foot-close-confirm = Close PR
foot-closed = Closed PR #{ $number }
foot-close-failed = Could not close the PR: { $error }
foot-pending-title = Check before opening the PR
foot-pending-loading = Checking unpushed commits and changes…
foot-pending-commit-mode-note = Writing from the commit history does not commit uncommitted files; including files applies to the AI mode only.
foot-pending-intro = { $repo } · { $branch } · Unchecked items stay out of this PR and remain local.
foot-pending-commits = { $count ->
    [one] Include 1 unpushed commit
   *[other] Include { $count } unpushed commits
}
foot-pending-commits-unpublished = { $count ->
    [one] Include 1 commit to publish (branch not on the remote)
   *[other] Include { $count } commits to publish (branch not on the remote)
}
foot-pending-commits-unpublished-note = This branch is not on the remote yet: without them there is nothing to open a PR from.
foot-pending-commits-on = Pushed together with the PR.
foot-pending-commits-off = Left out: not pushed, kept local. The PR is made from origin/{ $branch } on the remote.
foot-pending-files = { $count ->
    [one] Include 1 uncommitted file
   *[other] Include { $count } uncommitted files
}
foot-pending-files-on = The AI commits them before writing the PR.
foot-pending-files-off = Left out: not committed, left as they are.
foot-pending-files-off-commits = Leaving the commits out leaves these out too (a new commit would not go up).
foot-pending-continue = Continue
foot-pending-continue-some = Continue without some
foot-more-count = { $count ->
    [one] and 1 more
   *[other] and { $count } more
}
foot-excluded = Left out: { $parts } — they stay local
foot-excluded-commits = { $count ->
    [one] 1 unpushed commit
   *[other] { $count } unpushed commits
}
foot-excluded-files = { $count ->
    [one] 1 uncommitted file
   *[other] { $count } uncommitted files
}
foot-pr-title = Open PR · { $repo }
foot-pr-request-error = Could not check the PR request: { $error } · check for an existing PR instead of a new AI request.
foot-pr-existing = PR #{ $number } is already open
foot-pr-open-existing = Open the existing PR
foot-pr-none-open = No open PR for this repository and branch.
foot-pr-same-as = Same content as { $target }
foot-pr-same-as-body = Nothing to put up. Commits not on the base may show, but their content is already in { $target } (e.g. left after a squash merge).
foot-pr-targets-failed = Could not load the remote branches: { $error }
foot-pr-targets-loading = Loading the remote branches…
foot-pr-target = Target branch
foot-pr-target-note = The session's base branch is picked first (picking another one here does not change the session's base).
foot-pr-base-missing = origin has no { $base } branch, so { $target } was picked.
foot-pr-from-commits = Write from the commit history
foot-pr-from-commits-note = Fills the title and body from the included commits. Does not call the AI.
foot-pr-uncommitted-stay = Uncommitted files are not committed automatically and stay local.
foot-pr-preview-failed = Could not load the commit preview: { $error }
foot-pr-preview-loading = Loading the commit preview…
foot-pr-no-commits = No commits to include.
foot-pr-title-label = PR title
foot-pr-title-note = Starts with the latest commit's subject. Edit it freely.
foot-pr-body-preview = { $count ->
    [one] Body preview · 1 commit
   *[other] Body preview · { $count } commits
}
foot-pr-body-show = Show body
foot-pr-body-hide = Fold body
foot-pr-draft = Open as a draft PR
foot-pr-merge-now = Merge right after opening the PR
foot-pr-merge-now-note = Waits for the required checks and merges when they pass. Does not merge (and says so) when a review approval is needed.
foot-pr-merge-now-draft = A draft PR cannot be merged
foot-pr-what-commits-push = The app pushes the commits and opens the PR.
foot-pr-what-commits-remote = The app opens the PR from the commits on the remote.
foot-pr-what-ai = When the agent has prepared the commits and the PR title and body, the app pushes and opens the PR.
foot-pr-what-ai-no-files = When the agent has prepared the PR title and body, the app pushes and opens the PR.
foot-pr-what-ai-remote = When the agent has prepared the PR title and body, the app opens the PR from the commits on the remote (no push).
foot-pr-check-existing = Check for an existing PR
foot-pr-check-existing-tip = Only looks up an open PR for this branch, without the AI or a push
foot-pr-reuse-diverged-confirm = The commit history changed since it was written, so it may not match. Click again to put it up with this text.
foot-pr-rewrite = Have the AI write it again
foot-pr-reuse = Put it up with this text
foot-pr-reuse-anyway = Put it up with this text anyway
foot-pr-submit-merge = Open PR and merge
foot-pr-submit-ai = Request PR (AI)
foot-pr-submit-ai-merge = Request PR and merge (AI)
foot-left-title = There is PR text that was written but not put up
foot-left-written = Written { $when }
foot-left-written-reason = Written { $when } · last attempt: { $reason }
foot-left-same = Matches the commits · no commit changed since it was written
foot-left-ahead = { $count ->
    [one] 1 more commit since it was written · not covered by the text
   *[other] { $count } more commits since it was written · not covered by the text
}
foot-left-diverged = The commits it was written for are no longer on the branch (rebase / reset) · it may not match
foot-left-unknown = No record of the commit it was written for, so it cannot be checked
foot-left-pr-title = PR title
foot-left-empty = (empty)
foot-left-show = Show text
foot-left-fold = Fold
foot-job-title = { $repo } PR request
foot-job-preparing = Making the PR title and body from the commit history…
foot-job-writing = The session commits and writes the PR title and body… (ide cannot see when the agent's turn ends: it goes on once the PR file is written, or when you click Create PR)
foot-job-file = PR file: { $path }
foot-job-retry = Ask again
foot-job-create-now = Create PR
foot-job-open = Open
foot-job-cancelled = { $repo } PR request cancelled
foot-job-merge-cancelled = { $repo } merge wait cancelled
foot-job-merge-cancelled-body = The PR stays open.
foot-job-creating-title = Opening the { $repo } PR
foot-job-creating-push = Pushing and opening the PR on GitHub…
foot-job-creating-remote = Opening the PR on GitHub from the commits on the remote…
foot-job-prepare-failed = Could not make the { $repo } PR text
foot-job-send-failed = Could not send the { $repo } PR request
foot-job-check-failed = The { $repo } pre-PR check failed
foot-job-commit-send-failed = Could not send the { $repo } commit request
foot-job-create-failed = Could not open the { $repo } PR
foot-job-page-title = The { $repo } PR has to be opened on its page
foot-job-page-hint = { $reason } · Open shows the PR creation page
foot-job-page-hint-no-merge = { $reason } · Open shows the PR creation page (not merged)
foot-job-created = Opened { $repo } PR #{ $number }
foot-job-exists = { $repo } already has the open PR #{ $number }
foot-job-mergeable-unknown = Conflicts are not known yet. Refresh to check.
foot-job-exists-no-merge = An existing PR is not merged automatically.
foot-job-conflict-title = { $repo } PR #{ $number } · conflicts with the base branch
foot-job-conflict-body = The PR is open. Resolve the conflicts with { $base } to merge it.
foot-job-recheck-title = { $repo } PR needs a check before it goes up
foot-job-recheck-body = After the request: { $parts }. Nothing goes up before you check.
foot-drift-rewritten = the branch history changed (rebase etc.)
foot-drift-commits = { $count ->
    [one] 1 new commit
   *[other] { $count } new commits
}
foot-drift-files = { $count ->
    [one] 1 changed file
   *[other] { $count } changed files
}
foot-job-committing = { $count ->
    [one] The session commits 1 file… checked once more when it is done
   *[other] The session commits { $count } files… checked once more when they are done
}
foot-job-merge-wait-title = Waiting to merge { $repo } PR #{ $number }
foot-job-merge-wait-first = Waiting while GitHub checks whether it can be merged…
foot-job-merge-wait = GitHub is checking whether it can be merged · { $minutes } min waited
foot-job-merge-wait-checks = Checks { $progress } · waiting for { $pending } · { $minutes } min waited. Merges when all pass.
foot-job-merge-timeout = The checks did not finish in time. Merge with Merge in the Changes footer when they are done.
foot-job-merging = Merging…
foot-job-merged = Merged { $repo } PR #{ $number }
foot-job-merged-deleted = The remote branch was deleted too
foot-job-not-merged = { $repo } PR #{ $number } was not merged
foot-stop-target-changed = The PR's target branch changed ({ $from } → { $to }). Not merged.
foot-stop-draft = A draft PR is not merged
foot-stop-review-required = A review approval is needed. Merge with Merge in the Changes footer once approved.
foot-stop-checks-failed = Required checks failed: { $names }
foot-stop-protected = Branch protection (review approval or required checks) has to pass before it can be merged
foot-recheck-intro = { $repo } · These appeared after the PR request. Unchecked items stay out of this PR and remain local.
foot-recheck-rewritten = The branch history changed after the request (rebase, reset, …). It can only go up as it is now, and a push is refused if it differs from the remote.
foot-recheck-commits = { $count ->
    [one] Include 1 commit made after the request
   *[other] Include { $count } commits made after the request
}
foot-recheck-commits-on = Pushed together with the PR.
foot-recheck-commits-on-pushoff = Pushed together with the PR, along with the unpushed commits left out at the request.
foot-recheck-commits-off = Left out: not pushed. The PR is made up to the commits checked at the request.
foot-recheck-commits-off-pushoff = Left out: not pushed. The PR is made from the commits on the remote.
foot-recheck-files-on = The session commits them first, then this check runs once more.
foot-recheck-merge = Merge right after it goes up (set at the request)
foot-recheck-cancel = Cancel the PR request
foot-recheck-commit-go = Commit and continue
foot-later = Later
foot-continue-loading = Fetching the latest base branch…
foot-continue-load-failed = Could not read the branch state: { $error }
foot-continue-intro = Makes a new branch from the latest origin/{ $base } and takes only what you pick. { $branch } stays as it is locally.
foot-continue-fetch-failed = Could not fetch origin/{ $base }; showing the last local state.
foot-continue-name = New branch name
foot-continue-name-placeholder = Empty: { $suggested }
foot-continue-bad-name = This branch name cannot be used
foot-continue-commits = Commits to take
foot-continue-all = Select all
foot-continue-none = Clear all
foot-continue-no-commits = There are no commits that origin/{ $base } does not have.
foot-continue-no-change = No change
foot-continue-in-base = Same content in { $base }
foot-continue-more = { $count ->
    [one] 1 older commit is not listed and is not moved.
   *[other] { $count } older commits are not listed and are not moved.
}
foot-continue-order = Moved oldest first. The picks are tried in advance; nothing runs if they conflict.
foot-continue-conflict = Moving { $sha } { $subject } conflicts. Also pick the earlier commit it relies on, or leave this one out.
foot-continue-conflict-files = { $more ->
    [0] Moving { $sha } { $subject } conflicts ({ $files }). Also pick the earlier commit it relies on, or leave this one out.
   *[other] Moving { $sha } { $subject } conflicts ({ $files } and { $more } more). Also pick the earlier commit it relies on, or leave this one out.
}
foot-continue-check-failed = Could not try the picks: { $error }
foot-continue-files = { $count ->
    [one] Take the uncommitted changes · 1 file
   *[other] Take the uncommitted changes · { $count } files
}
foot-continue-files-off = Not taken. Stashed on { $branch } to take out later.
foot-continue-files-conflict = { $count ->
    [one] 1 file conflicts with the new branch. If it cannot be taken it stays in the stash of { $branch }.
   *[other] { $count } files conflict with the new branch. If they cannot be taken they stay in the stash of { $branch }.
}
foot-continue-files-list-more = { $names } and { NUMBER($count) } more
foot-continue-close-pr = Close the open PR #{ $number }
foot-continue-close-pr-note = { $title } · closed without a comment (ide)
foot-continue-run = Create new branch
foot-continue-running = Creating…
foot-continue-failed = Could not move to a new branch: { $error }
foot-continue-result-branch = Work continues on { $branch }
foot-continue-result-moved = { $count ->
    [one] Moved 1 commit
   *[other] Moved { $count } commits
}
foot-continue-result-skipped = { $count ->
    [one] Skipped 1 commit that changes nothing (already in)
   *[other] Skipped { $count } commits that change nothing (already in)
}
foot-continue-result-left = { $count ->
    [one] 1 commit not picked stays on { $previous }
   *[other] { $count } commits not picked stay on { $previous }
}
foot-continue-result-closed = Closed PR #{ $number }. Open the PR again from the new branch
foot-continue-result-not-closed = The old PR was not closed: { $error }
foot-continue-result-files-stash = { $count ->
    [one] 1 change conflicted on the new branch and stays in the stash of { $previous }
   *[other] { $count } changes conflicted on the new branch and stay in the stash of { $previous }
}
foot-continue-result-files-moved = { $count ->
    [one] Moved 1 change
   *[other] Moved { $count } changes
}
foot-continue-result-files-stashed = { $count ->
    [one] Stashed 1 change on { $previous }
   *[other] Stashed { $count } changes on { $previous }
}
err-gh-missing = The GitHub CLI (gh) was not found: { $program }
err-github-no-credentials = No GitHub login for { $host }: git has no credential for it and the GitHub CLI (gh) is not installed
err-github-auth = GitHub rejected git's credential for { $host } (401) and there is no GitHub CLI (gh) to retry
err-github-failed = No answer from GitHub: { $detail }
err-gh-auth = gh is not logged in to { $host } (run gh auth login)
err-gh-failed = gh failed: { $detail }
err-pr-no-origin = The repository has no origin remote
err-pr-origin-unparsable = Cannot tell the GitHub repository from origin ({ $url })
err-pr-gitlab = GitLab merge requests are not supported here
err-pr-gitlab-page = GitLab: open the merge request page
err-pr-detached = HEAD is detached (no branch)
err-pr-same-branch = The PR's branch and target are the same ({ $branch })
err-pr-no-remote-branch = origin has no { $branch } branch
err-pr-bad-branch = Not a valid branch name: { $branch }
err-pr-bad-number = Not a valid PR number
err-pr-bad-sha = Not a valid commit id: { $sha }
err-pr-not-on-branch = Commit { $sha } is not on this branch
err-pr-changed = The branch changed after the preview; open the dialog again
err-pr-no-text = The PR file was not written
err-pr-empty-title = The PR title is empty
err-pr-bad-title = The PR title is not valid
err-pr-no-commits = No commits that { $base } does not have
err-pr-needs-workspace = PR requests work in workspace sessions
err-pr-api = GitHub refused the request ({ $status }): { $detail }
err-pr-merge-refused = GitHub refused the merge ({ $status }): { $detail }
err-pr-close-failed = GitHub did not close the PR ({ $status }): { $detail }
err-pr-update-refused = GitHub refused to update the branch ({ $status }): { $detail }
err-pr-already-merged = PR #{ $number } is already merged
err-pr-not-open = PR #{ $number } is not open
err-pr-head-moved = PR #{ $number } moved on ({ $expected } → { $actual }); check it again before merging
err-pr-push-failed = Push failed: { $detail }
err-pr-create-unconfirmed = Not sure whether the PR was created: { $detail }
err-prompt-empty = The request is empty
err-prompt-not-ready = The agent is not at its input right now (session { $id }); try again when it waits for input

# Notification centre (notice_view.rs)
notice-bell = Notifications
notice-bell-running = Notifications · { $count ->
    [one] 1 task running
   *[other] { $count } tasks running
}
notice-title = Notifications
notice-running = { NUMBER($count) } running
notice-read-all = Mark all as read
notice-clear = Clear finished notifications
notice-empty = No notifications.
merge-notice-title = Merge result
notice-restart = Restart
notice-attach-failed = Could not attach to the session's terminal
notice-pr-status-failed = Could not check the PR status
notice-conn-back = Connected to the server again
notice-needs-input = Waiting for you
notice-answered = The answer is done
notice-bell-rang = The terminal rang its bell
notice-load-failed = A page could not be loaded
notice-sync-status-failed = Could not check the branch against its remote

## Widgets page, launcher and registration (widgets_page.rs, widgets_ui.rs)
widgets-hide = Hide the widget list
widgets-show = Show the widget list
widgets-none = No widgets yet.
widgets-note = Small apps that agents make. Registered once, every session can open them as a tab next to the terminal.
widgets-hint = Ask an agent, for example: “make a to-do list widget”.
widgets-pick = Pick a widget in the list.
widget-gone = There is no widget “{ $widget }” any more.
widget-group-family = Per session family
widget-group-session = Per session
widget-group-global = Shared data
widget-scope-global = Shared: every session sees the same data, in one shared tab
widget-scope-family = Per session family: a parent session and its children share the data
widget-scope-session = Per session: each session keeps its own data
widget-version = v{ $version }
widget-label-description = Description
widget-label-data = Data
widget-label-tools = ide features
widget-label-created-by = Registered by
widget-label-version = Version
widget-label-updated = Updated
widget-label-open-in = Open in
widget-tools-none = None
widget-session-gone = Deleted session
widget-open-none = Not open anywhere
widget-open-common = The shared tab of every session
widget-open-here = Open in this session
widget-close-here = Close here
widget-delete = Delete widget
widget-delete-note = Its tabs close and its shared data is removed. Agents can register it again.
widget-manage = Manage
widget-launcher-no-session = Pick a session to open widgets in.
widget-launcher-common = Shared
widget-launcher-session = This session
widget-state-open = Open
widget-state-closed = Closed
widget-register-title = Register widget
widget-register-new = The agent of “{ $session }” wants to register the widget “{ $name }”. Every session can then open it.
widget-register-other = The agent of “{ $session }” wants to update the widget “{ $name }”, registered by “{ $by }”.
widget-register-tools = The agent of “{ $session }” wants to change the ide features the widget “{ $name }” uses.
widget-register-uses = It uses these ide features:
widget-register-no-tools = It uses no ide features.
widget-register-allow = Allow
widget-register-refuse = Refuse

## Approvals (approvals_ui.rs)
approval-title = Approval
approval-steer = The agent of “{ $who }” wants to type a message into “{ $whom }” now, while its agent is working.
approval-interrupt = The agent of “{ $who }” wants to stop the current work of “{ $whom }” and send it a message.
approval-other = The agent of “{ $who }” asks for “{ $what }” on “{ $whom }”.
approval-folder-write = The agent of “{ $who }” wants to write outside its session, in the folder { $folder } (another checkout, another branch or another folder). Allowing it covers this folder and everything below it.
approval-note = ide carries this out only after you allow it. Allowed for this session, it lasts until the ide server restarts.
approval-refuse = Refuse
approval-once = Allow once
approval-session = Allow for this session
approval-always = Always allow
approval-cap-steer = message during work
approval-cap-interrupt = interrupt and message
approval-cap-folder-write = write in the folder
approvals-label = PERMISSIONS
approvals-note = What you allowed agents to do beyond their own session.
approvals-none = Nothing allowed yet.
approvals-row = { $who } → { $whom }: { $what }
approvals-always = Always
approvals-until-restart = Until the ide server restarts
approvals-revoke = Revoke
approvals-wait-label = APPROVAL WAIT
approvals-wait-note = A question not answered within this time is refused and its dialog closes. The agent waits meanwhile. Without a limit, it is still refused when every ide window closes or after a day.
approvals-wait-1m = 1 minute
approvals-wait-5m = 5 minutes
approvals-wait-15m = 15 minutes
approvals-wait-1h = 1 hour
approvals-wait-none = No limit
approval-timed-out = An approval question was not answered in time and was refused. Change the wait in Settings → Permissions.

## Auto answers (auto_answers_ui.rs)
auto-answers-label = AUTO ANSWERS
auto-answers-note = Dialogs of an agent that ide answers by itself. Folder questions are answered only for the folders ide set up for the session.
auto-answer-trust-workspace = Trust the session's folder
auto-answer-trust-workspace-note = The agent asks whether to trust the folder when a session starts in a new one.
auto-answer-trust-added-dir = Trust the session's added folders
auto-answer-trust-added-dir-note = The agent asks before it uses a folder added to the session, e.g. a repository's git folder.

## Previews (docs/previews.md)
preview-tip = A preview from the session's agent
preview-devtools = Developer tools
err-preview = Preview: { $detail }

## Terminals (agent terminal tools: approvals_ui.rs)
approval-terminal-input = The agent of “{ $who }” wants to type into the terminal “{ $whom }”.
approval-cap-terminal-input = type into a terminal

## Browser control (browser_agent.rs, approvals_ui.rs)
approval-browser-act = The agent of “{ $who }” wants to use the site { $whom } in the ide browser (open, click, type, run scripts).
approval-browser-read = The agent of “{ $who }” wants to read the site { $whom } in the ide browser (its text, console or a screenshot).
approval-cap-browser-act = use the site in the browser
approval-cap-browser-read = read the site in the browser
