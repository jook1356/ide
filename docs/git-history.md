# Git history (History tab)

Port of the backend behind Hermes Desk's Changes panel **History** tab (`powered_ide`:
`src/main/changes.ts` `commitLog` / `commitFiles` / `commitDiff`, `src/main/github.ts`
`historyLocalCommits`, `src/renderer/src/lib/repoPanelCache.ts` `loadRepositoryHistory`,
`src/renderer/src/components/ChangesPanel.tsx`, `DiffContent.tsx`). Protocol version 5.

Every RPC names `(session, repo)` like the other git RPCs and runs in that repo's worktree of
the session (`repo-not-attached`, `session-not-found` as usual). All git calls go through
`ide_core::git_raw` (`core.quotepath=false`, no prompts, `GIT_OPTIONAL_LOCKS=0`).

## Hermes Desk behaviour -> ide crate/file

| Hermes Desk | ide |
|---|---|
| `changes:log` -> `commitLog(dir, skip, limit=200)`: `git log --format=%H%x1f%h%x1f%s%x1f%an%x1f%at%x1f%ae%x1e --skip=N -n L HEAD` | `git.log` (`Request::GitLog{session, repo, skip, limit}` -> `Response::GitLog(GitLog)`), `ide-server/src/git.rs::log` / `parse_log`; same format string. `limit` 0 = `GIT_LOG_PAGE` (200, Desk's `LOG_PAGE`). |
| Renderer `CommitInfo {sha, short, subject, author, email, date}` (`date` ms) | `ide_proto::CommitInfo` (`files.rs`), `date` = unix **seconds** (`%at`). |
| "more" row: `hasMore = page.length === LOG_PAGE` | `GitLog.has_more`: the server asks for `limit + 1` commits and reports whether the extra one exists (exact, no empty last page). |
| `github:historyLocal` -> `historyLocalCommits`: upstream -> `rev-list upstream..HEAD`; upstream gone -> none; no upstream / detached -> `rev-list HEAD --not --remotes`; `--max-count=500` | `git.unpushed` (`Request::GitUnpushed` -> `Response::GitUnpushed(Vec<String>)`), `git.rs::unpushed`; same branch / `for-each-ref %(upstream)|%(upstream:track)` logic, cap `GIT_UNPUSHED_MAX` = 500 (UI badge shows `500+` at the cap, ↑ mark per commit). |
| `changes:commitFiles` -> `commitFiles`: `git diff-tree --no-commit-id -r -M --root --name-status -z <sha>` | `git.commit_files` (`Request::GitCommitFiles` -> `Response::GitChanges(Vec<GitChange>)`), `git.rs::commit_files` / `parse_name_status`; `diff-tree -r -M --name-status -z <first parent> <sha>`, root commit `--root <sha>`. |
| Click a commit: `changes:show` -> `commitDiff`: `git show --no-color --format= --find-renames <sha>`, parsed in the renderer | `git.commit_diff` with `path: None` (`Request::GitCommitDiff` -> `Response::Diff(Vec<DiffFile>)`, the same structured diff as `git.diff`), `git.rs::commit_diff`: `diff-tree -r -p -M <first parent> <sha>` (root: `--root`), parsed by the existing `parse_diff`. |
| Click a commit's file: the whole commit diff, filtered to that path in the renderer (`DiffContent` `commitPath`) | `git.commit_diff` with `path: Some(p)`: the pathspec is limited server-side (a renamed file's other side is added so rename detection still pairs them); a renamed file matches by its new or old path. |
| `commitDetail` sha check `/^[0-9a-f]{4,64}$/i` | `sha` of `git.commit_files` / `git.commit_diff` must be 4-64 hex digits naming an existing commit, else `git-bad-rev` (`rev`). |
| `changes:commitDetail` (full message, committer, parents for the diff header) | Not ported (not part of this contract). |

Client (`ide-client`): `git_log`, `git_unpushed`, `git_commit_files`, `git_commit_diff`;
`CommitInfo`, `GitLog`, `GIT_LOG_PAGE`, `GIT_UNPUSHED_MAX` are re-exported.

## Deviations (with reasons)

- **Unpushed = Desk's History marks (`historyLocalCommits`), not `github:unpushed`.** The History
  tab's ↑ marks and badge come from `github:historyLocal`; `github:unpushed`
  (`HEAD --not --remotes`) is used elsewhere in Desk. The RPC keeps the contract's name
  `git.unpushed`.
- **Empty without a remote** (contract): Desk's `historyLocalCommits` would mark every commit of a
  repo that has no remote (`HEAD --not --remotes`); Desk's sync status treats such a repo as
  "no remote" instead of counting unpushed commits, which `git.unpushed` follows.
- **First parent for merge commits.** Desk's `git diff-tree <sha>` lists nothing for a merge
  commit and `git show` prints a combined diff; here both files and diff compare a merge with its
  first parent (what the branch gained), so a merge commit shows its merged-in changes.
- **`date` in seconds** (the wire convention for times, e.g. `SessionInfo::created_at`); Desk's
  renderer uses ms.
- **Exact `has_more`** via one extra commit instead of "page was full".
- **`git.log` errors:** Desk swallows every `git log` failure into an empty list; here a repo
  without commits returns an empty page, other git failures are reported (`git`). `limit` is
  capped at 10 000 per page to bound the reply size.
- **Kinds:** `T` -> `TypeChanged`, `C` -> `Copied` (Desk folds them into modified / renamed).
- **`path` of `git.commit_diff`** is only syntax-checked (`normalize_rel`), not resolved in the
  worktree: the file may no longer exist there.

## UI (ide-app)

The History tab itself (rows, refresh triggers, commit items, deviations) is described in
`docs/side-panel.md`, "History". In short: `detail.rs` loads `git.log` (refresh: every shown
commit, at least `GIT_LOG_PAGE`) and `git.unpushed` when the repo is first shown, after a
`GitChanged` notification and when the tab is opened; "more" pages use `skip`; unfolding a commit
calls `git.commit_files`; a commit row opens `ItemKey::Commit { path: None }` and a file row
`ItemKey::Commit { path: Some(..) }`, both loaded with `git.commit_diff` into the existing diff
view. `date` (seconds) is shown as Desk's relative time and a local `YYYY-MM-DD HH:MM` tooltip.

## Errors

No new error ids: `git-bad-rev` (`rev`) for a malformed or unknown commit id, `path-invalid` /
`path-denied` for a bad `path`, `git` / `git-spawn` for git failures.

## Tests

`ide-server/src/git.rs` (`history_log_files_diff_unpushed`, `log_and_name_status_parsing`):
paging and `has_more`, root commit files, rename in commit files and diff (by new and old path),
merge vs first parent, one-path and whole-commit diffs, invalid ids, unpushed with a local bare
`origin` (upstream, untracked branch, gone upstream, no remote), empty repo.
`ide-server/tests/files_e2e.rs` (`files_scenario`, memory and child transports): the four RPCs
through `ide_client::Client`. `ide-proto` round-trip test covers the new messages.
