# Write guard

Status: implemented. An agent writes freely only inside its own session; writing anywhere else
needs the user's approval, per folder, through ide's approval system (`docs/approvals.md`).
Code: `ide-core/src/write_guard.rs` (what a tool call writes, what leaves the session),
`ide-server/src/service/write_guard.rs` (scope, asking), hooks in `ide-agents` (Claude, Codex).

## Why

Sessions used to read *and write* anything: an agent asked to "merge into main" would `cd` into
the registered repository's own checkout, merge there and push. ide then could not follow the
session's work (its branch badge went from 커밋 있음 to 변경 없음 instead of 머지됨, the merge
happened outside every session). Reading stays free; writing outside the session goes through
the user.

## The session's own area (free)

- The session folder (`Session::root`: the workspace folder; original-repo sessions: the
  repository) and legacy worktrees outside it.
- Git writes in the session's worktrees **on the session's branch** (`ide/<id>`); an
  original-repo session on the repository's **current** branch.
- The session's data folder (`<data>/sessions/<id>`: scratch files, attachments), the agents'
  account folders (`<data>/accounts`; without an account, the CLI's default config home that
  its provider names, `AgentProvider::default_state_dirs`: memory, plans, settings) and the OS
  temp folder.

## What needs approval

| Write | Folder asked for |
|---|---|
| A file edit (Write / Edit / apply_patch) outside the area | the file's repository top level, else the nearest existing folder |
| A shell command that writes there: redirects (`>`, `>>`), `rm` / `mv` / `cp` / `mkdir` / `touch` / `tee` / `sed -i`, PowerShell `Remove-Item` / `Set-Content` / `Copy-Item` / `New-Item` ... | same |
| A git command that changes a repository outside the area (`cd <repo> && git merge`, `git -C <repo> commit`, ...) | that repository's top level |
| A git command in the session's own worktree that writes **another branch**: `git push origin HEAD:main`, `git push origin main`, `git branch -f/-D/-m`, `git checkout -b`, `git switch other`, `git checkout <existing branch>`, `git update-ref`, `git worktree add`, or any write while the worktree is not on the session's branch | the registered repository's folder |

Git reads are never asked for (`status`, `log`, `diff`, `show`, `fetch`, `branch` listing,
`worktree list`, `config <key>`, ...).

One grant covers a folder and everything below it, so allowing the repository once also covers
"push to main" from the worktree. Answers are the approval system's: 거절, 이번만, 이 세션
동안, 항상 (`approvals.json`); Settings -> 권한 lists folder grants as `폴더에 쓰기` with the
folder as target.

## How

1. The CLI's `PreToolUse` hook (`ide-server hook`) sends edits (`PreEdit { paths }`) and shell
   commands (`PreShell { command, cwd }`) to the daemon. Claude: every tool (Bash and PowerShell
   are shells); Codex: `apply_patch|Edit|Write|Bash|exec_command|shell|PowerShell`.
2. The daemon (`UserCtx::write_refusal`, off the connection loop) turns the call into writes
   (`write_guard::shell_writes`: follows `cd` / `pushd` / `Set-Location`, `git -C`, `bash -c` /
   `powershell -Command` / `cmd /c`, skips heredoc bodies and quoted text, maps Git Bash
   `/c/...` to `C:/...`), keeps those outside the scope (`write_guard::needs`, one per folder),
   and checks the grants (`Grants::allows_folder`: a grant on the folder or one above).
3. Not granted: the user is asked (`folder.write`, target = the folder). Refused, closed, timed
   out or no ide window open: the hook denies the tool call with a reason telling the agent not
   to work around it and to ask the user instead.
4. The hook waits as long as the question can take (`PreToolUse` hook timeout 330 s, the hook
   program 320 s once connected; other hooks stay at 10 s / 5 s).

## Limits

A guard, not a sandbox: a shell can always write in ways a command line does not show
(variables, scripts, programs writing files themselves, `git` aliases). Unknown paths (a
variable, a relative path without a cwd) are not asked for. Sessions without hooks (the plain
shell provider, a Codex account without ide's hooks installed) are not guarded. ide's own git
actions (footer commit / PR / merge, child merges) run in the daemon and are not affected.
