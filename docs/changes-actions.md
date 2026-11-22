# Changes / History actions

Port of the backend behind Hermes Desk's Changes panel footer and History context menu
(`powered_ide`: `src/main/changes.ts`, `commitActions.ts`, `branches.ts`, `branchRefresh.ts`,
`github.ts` `syncStatus` / `pushBranch` / `pullBranch`, IPC in `src/main/index.ts`; renderer
`ChangesPanel.tsx`, `StashSection.tsx`, `SyncButton.tsx`, `CommitContextMenu.tsx`,
`BranchSelect.tsx`, `BranchRefreshPanel.tsx`). Protocol version 6.

## Local git actions

Every RPC names `(session, repo)` like `git.status` and runs in that repo's worktree of the
session (`session-not-found`, `repo-not-attached` as usual); paths are worktree-relative and
jailed (`path-invalid`, `path-denied`). Server code: `ide-server/src/git_actions.rs` (pure
functions on the worktree root + `UserCtx::handle_git_actions`); wire types:
`ide-proto/src/git_actions.rs`; client methods `Client::git_*` (re-exported types in
`ide-client`). Git runs through `ide_core::git_raw` / an own timed runner with Desk's
`NO_PROMPT` (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`), `core.quotepath=false`; network
commands are killed after Desk's timeouts (`git-timeout`). Mutations are audited
(`git.discard`, `git.stash_push`, `git.stash_action`, `git.commit_action`, `git.push`,
`git.pull`, `git.switch_branch`, `git.create_branch`, `git.refresh_branch`; target
`<session> <repo> <detail>`). The worktree watcher reports the resulting `FsChanged` /
`GitChanged`; `git.refresh_branch` also sends `SessionsChanged` (the worktree's recorded branch).

### Hermes Desk handler/component -> ide RPC/file

| Hermes Desk | ide |
|---|---|
| `changes:discard` -> `discardChange(dir, file)`; ChangesPanel 변경 취소 modal (loops over the checked files) | `git.discard {session, repo, file: GitChange}` -> `Unit` (`Client::git_discard`). `Untracked` deleted (link removed, not followed; already gone is fine), `Added`/`Copied` `git rm -f`, `Renamed` `restore --source=HEAD --staged --worktree <orig>` + `rm -f <path>`, anything else restored from HEAD. One file per call like Desk. |
| `git:listStashes` -> `listStashes` (`useStashes`, StashSection) | `git.stash_list` -> `Response::GitStashes(Vec<StashEntry>)` (`git_stash_list`): `stash list --format=%H%x1f%ct%x1f%s`, subject parsed (`On <b>: msg` / `WIP on <b>: …`), `auto` = message starts with `hermes-desk:` (message then empty), files from `stash show --name-status --include-untracked -z` with the 3rd parent's paths as `Untracked`. Git failure = empty list. |
| `git:stashPush(dir, message, paths?)` -> `stashPush` (ChangesPanel 임시 보관: checked files, both sides of a rename, or all) | `git.stash_push {message, paths: Option<Vec<String>>}` -> `Unit`: `[--literal-pathspecs] stash push --include-untracked [-m msg.trim()] [-- paths]`; nothing new on `refs/stash` = `git-stash-empty`. Message > 10 000 bytes or `Some(vec![])` = `bad-request`. |
| `git:stashAction(dir, sha, 'pop'|'apply'|'drop')` -> `stashAction` (StashSection buttons) | `git.stash_action {sha, action: StashAction}` -> `Response::GitStashResult(StashActionResult{conflicts})`: `stash@{n}` looked up by sha right before acting; a conflicting pop/apply returns the conflicted paths (stash kept). Not a 40/64-hex id = `git-bad-rev`; not in the list = `git-stash-gone`. |
| `git:commitAction(dir, {action, sha, branch?, confirmed})` -> `runCommitAction` (CommitContextMenu: 이 커밋 체크아웃 / 브랜치 만들기 / 되돌리기 / 체리픽) | `git.commit_action {sha, action: CommitAction::{Checkout, Branch{name}, Revert, CherryPick}}` -> `Unit`. Same checks and order: full sha (`git-bad-rev`), branch name pre-check + `check-ref-format refs/heads/<n>` (`git-bad-branch`), one action per worktree (`git-busy`), no MERGE/REBASE/CHERRY_PICK/REVERT_HEAD, rebase-*, sequencer (`git-in-progress`), clean tree incl. untracked (`git-dirty`), `cat-file -t` = commit (`git-bad-rev`), no merge commit for revert/cherry-pick (`git-merge-commit`). Commands: `switch --detach`, `switch --no-track -c <n>`, `revert --no-edit`, `cherry-pick` (`GIT_EDITOR=true`, 60 s). |
| `github:syncStatus(dir, fetch)` -> `syncStatus(dir, fetch, mergedPrHead)` (SyncButton numbers; fetch on open / every 5 min) | `git.sync_status {fetch, merged_head: Option<String>}` -> `Response::GitSyncStatus(SyncStatus{ahead, behind, no_remote, upstream, unpublished, gone})` (`git_sync_status`). No remote -> `no_remote`; no upstream or a gone one -> `unpublished` (+`gone`), `ahead` = Desk's `undeliveredCommits` (`upstream..HEAD`, else `HEAD --not --remotes`, minus what `merged_head` reaches, cap 500); else `upstream` + for-each-ref `ahead/behind`, or with `fetch` a quiet `fetch <remote> <branch>` (failures ignored) then `rev-list --left-right --count`. |
| `github:push(dir, force, expected?)` -> `pushBranch` (SyncButton push / publish / force push) | `git.push {force, expected: Option<PushExpected{head_sha, branch}>}` -> `Response::GitBranchName(branch)`: `push -u [--force-with-lease] origin HEAD`; with `expected` (Desk's PR flow): HEAD must still be that commit on that branch with a clean tree (`git-push-changed`), pushes `<sha>:refs/heads/<branch>`. Detached = `git-detached`. 180 s. |
| `github:pull(dir, mode)` -> `pullBranch` (SyncButton pull / rebase / 원격 상태로 덮어쓰기) | `git.pull {mode: PullMode::{Merge, Rebase, Reset}}` -> `GitBranchName`: `pull --no-rebase|--rebase --no-edit --autostash` (`GIT_MERGE_AUTOEDIT=no`, 180 s); `Reset` fetches the upstream (120 s) and `reset --hard` to it, no upstream = `git-no-upstream`. Detached = `git-detached`. |
| `git:branches` -> `branchList` + `git:remoteTrackingBranches` -> `remoteBranchList` + `git:branchBases` -> `branchBases` (BranchSelect list, its 원격 group, new-branch base list) | `git.branches` -> `Response::GitBranches(GitBranches{current, local: Vec<LocalBranch{name, checkout}>, remote: Vec<RemoteBranch{remote, branch, short_ref, has_local}>})` (`git_branches`). `checkout`: `Here`, `MainRepo(path)` (only the registered repo folder: switchable), `Worktree(path)` (another session: disabled), `None` — Desk's `branchHolders` ranking. Remotes split by the longest configured remote name. Local refs only. |
| `git:fetchRemotes` -> `fetchRemotes` (BranchSelect opening) | `git.fetch_remotes` -> `Response::GitFetchStatus(RemoteFetchStatus{ok, fetched, error})`: `fetch --prune --quiet <remote>` each (30 s), at most every 2 min per repository (shared git dir: all its worktrees), 20 s after a failure; a concurrent call waits and reuses the result (`fetched: false`). Never an error reply. |
| `git:switch(dir, branch, 'carry'|'stash')` -> `switchBranch`, `git:switchRemote(dir, remote, branch, mode)` -> `switchRemoteBranch` | `git.switch_branch {branch, remote: Option<String>, mode: SwitchMode}` -> `Unit`. Local: must exist (`git-bad-rev`), not held by another worktree (`git-branch-checked-out` `branch`,`path`), `switch --ignore-other-worktrees`. `Stash` = `stash push --include-untracked -m hermes-desk:<from>` first, popped back if the switch fails. Remote: configured remote only (`git-unknown-remote`), existing local branch -> local switch, else explicit-refspec fetch, `branch --no-track` + `branch.<b>.remote/merge`, switch (branch deleted again on failure). |
| `git:createBranch(dir, name, base, mode)` -> `createBranch` (BranchSelect 새 브랜치) | `git.create_branch {name, base, mode}` -> `Unit`: `check-ref-format --branch` (`git-bad-branch`), `base` must be a commit-ish (`git-bad-rev`), `switch --no-track -c <name> <base>`, `Stash` as above. |
| `git:branchRefreshInfo(dir, open?)` -> `branchRefreshInfo(path, base, fetch)` (BranchRefreshPanel; opened from the Changes "…" menu and the merged-PR card) | `git.branch_refresh_info {fetch}` -> `Response::GitRefreshInfo(BranchRefreshInfo{branch, base, suggested, commits: Vec<BranchCommit{full, sha, subject, in_base}>, more_commits, files: Vec<StatusEntry{path, status}>, fetch_failed})`. `base` = the session's base for the repo (`Session::base_of`, what `git.status` uses); `fetch` = `fetch origin +refs/heads/<base>:refs/remotes/origin/<base>` (failure -> `fetch_failed`). Commits `log --no-merges -100 HEAD ^origin/<base>` oldest first; `in_base` = newest commit whose `merge-tree` into the base leaves the base tree, and all before it (40 checked). |
| `git:branchRefreshCheck(dir, commits, moveFiles)` -> `checkBranchRefresh` | `git.branch_refresh_check {commits, move_files}` -> `Response::GitRefreshCheck(BranchRefreshCheck{conflict: Option<RefreshConflict{sha, subject, files}>, skipped, files_conflict})`: in-memory cherry-picks (`merge-tree --write-tree --merge-base=<c^>`), uncommitted changes via `stash create` + `merge-tree` and untracked paths in the new tree. |
| `git:refreshBranch(dir, {name?, commits, moveFiles, closePr?})` -> `refreshBranch` + `workspace.setWorktreeBranch` | `git.refresh_branch {options: BranchRefreshOptions{name, commits, move_files}}` -> `Response::GitRefreshResult(BranchRefreshResult{branch, previous, previous_head, moved_commits, skipped_commits, left_commits, moved_files, stashed_files, files_left_in_stash})`: fetch base (errors reported), plan, `commit-tree` with the original author/date/message (committer of the moved commits when no identity is configured), stash, `switch --no-track -c`, pop when asked (conflict -> reset/clean, left in the stash). The session worktree's recorded branch becomes the new one (saved, instructions regenerated, `SessionsChanged`). |

### What the UI does per Desk control

| Desk control | Call |
|---|---|
| 변경 취소 (discard modal, N checked files) | `git_discard(session, repo, file)` per file, collect failures like Desk, then refresh `git.status`. `file` = the row's `GitChange` (untracked rows: `kind: Untracked`, `path`). |
| 임시 보관 button + 메모 | `git_stash_push(session, repo, memo.trim(), None)` when every file is checked, else `Some(paths)` with `orig_path` added for renames. `git-stash-empty` -> Desk's "임시 보관할 변경이 없어요". |
| 임시 보관 section (this branch's / all) | `git_stash_list` on show / after changes; filter `entry.branch == status.branch` (all on request); title = `message`, else `auto` ? 브랜치 전환 때 임시 보관 : 메모 없음; `date` is seconds. |
| 꺼내기 / 적용 / 버리기 | `git_stash_action(sha, Pop|Apply|Drop)`; non-empty `conflicts` -> Desk's conflict notice (stash kept). |
| History row menu: 체크아웃 / 여기서 브랜치 만들기 / 되돌리기 / 체리픽 (after the confirm dialog) | `git_commit_action(sha_full, Checkout | Branch{name} | Revert | CherryPick)`. Copy sha stays client-side. |
| SyncButton numbers | `git_sync_status(fetch, merged_head)`: `fetch: true` on open (unless fetched < 60 s ago) and every 5 min, `false` on tree changes; `merged_head` from the PR backend's merged PR head if any, else `None`. Label: `no_remote` -> hidden/disabled, `unpublished` -> 게시 (publish), `behind > 0` -> pull, `ahead > 0` -> push, else fetch. |
| push / publish / 강제 push | `git_push(force, None)`; returns the branch for the toast. |
| pull / rebase / 원격으로 덮어쓰기 | `git_pull(Merge | Rebase | Reset)`. |
| 가져오기 (fetch) | `git_sync_status(fetch: true, ..)`. |
| BranchSelect list (opening) | `git_branches` now, `git_fetch_remotes` in the background, `git_branches` again if `fetched`; show `error` when `!ok`. Disable local rows with `checkout = Worktree(_)`; the remote group = `remote` with `!has_local`. |
| BranchSelect pick (carry / stash dialog) | `git_switch_branch(branch, None, mode)`; a remote row: `git_switch_branch(row.branch, Some(row.remote), mode)`. |
| BranchSelect 새 브랜치 (base list) | bases = `local[].name` + `remote[].short_ref`; `git_create_branch(name, base, mode)`. |
| 새 브랜치로 이어가기 dialog | `git_branch_refresh_info(fetch: false)` to decide whether to offer it, `fetch: true` when the dialog opens; checks default to `!in_base`; `git_branch_refresh_check(picks, move_files)` on every pick change; run = `git_refresh_branch({name, commits, move_files})`, then (if the user asked to close the old PR) the PR backend's close call with `previous` / `previous_head`. |

### Error ids

New (server-side `ProtoError`, no `ide-core` variants): `git-bad-branch` (`name`),
`git-detached`, `git-no-upstream`, `git-push-changed`, `git-busy`, `git-in-progress`,
`git-dirty`, `git-merge-commit`, `git-branch-checked-out` (`branch`, `path`),
`git-unknown-remote` (`remote`), `git-branch-exists` (`name`), `git-stash-empty`,
`git-stash-gone`, `git-refresh-moved`, `git-refresh-conflict` (`sha` short, `subject`,
`files` = first three joined by `, `, `more` = how many more), `git-merge-tree` (`detail`;
git older than 2.40). Reused: `git-bad-rev` (`rev`), `git-no-base` (`base`), `git` /
`git-spawn` / `git-timeout` (`command`, `secs`), `path-invalid`, `path-denied`, `fs-*`,
`bad-request` (`method`), `session-not-found`, `repo-not-attached`.

### Deviations (with reasons)

- **One RPC per Desk picker trio**: `git.branches` returns what `branchList`,
  `remoteTrackingBranches` and `branchBases` return (remote rows carry `has_local` instead of
  two lists); holder paths are absolute server paths for display only.
- **`git:switch` + `git:switchRemote` merged** into `git.switch_branch` (`remote: Option`).
- **Stash-mode switch only pops what it stashed**: Desk runs `stash pop` after a failed switch
  even when `stash push` had nothing to save (it would pop an unrelated stash); here the pop
  happens only if `refs/stash` moved.
- **Local switch requires an existing local branch** (`git-bad-rev`) and a valid name;
  Desk passed the name to `git switch` unchecked.
- **`merged_head` is a request field**: Desk reads it from its PR records
  (`mergedPrHead`); here the UI passes it from the PR backend (or `None`).
- **Not ported**: Desk's `stashesFor` / `resolveStash` (unused by the renderer; the stash list
  carries `branch` + `auto` instead); `openPr` of `branchRefreshInfo` and `closePr` of
  `refreshBranch` (GitHub API: the PR backend's calls); `confirmed: true` of commit actions (the
  UI's confirm dialog; the protocol has no implicit confirmation flag).
- **Times in seconds** (`StashEntry::date`), like the other wire types.
- **`fetch_remotes` in-flight sharing** is a per-repository lock: a second caller waits for the
  running fetch and gets its result with `fetched: false`.
- **Picks**: `git.branch_refresh_check` / `git.refresh_branch` ignore commit ids that are not
  40-64 hex digits (as Desk) and refuse names not listed (`git-refresh-moved`).
- **`git-refresh-conflict` carries data, not text**: the UI builds Desk's
  "`<sha> <subject>`을(를) 옮기면 충돌해요 (files 외 N개)" message from the args.

### Tests

`ide-server/src/git_actions.rs` unit tests (scratch repos under `IDE_TEST_TMP`, local bare
`origin`s): `discard_each_kind`, `stash_list_push_actions` (partial / all / untracked, apply,
pop by sha, conflicting pop, gone / bad ids), `stash_subjects`, `commit_actions` (validation,
dirty, revert, cherry-pick, branch, detached checkout, merge commit, in-progress, busy slot
released), `sync_push_pull` (no remote, ahead/behind with and without fetch, merge / rebase
with autostash / reset pulls, unpublished, merged head, gone upstream, verified push, force
with lease, detached), `branches_switch_create_fetch` (holders across linked worktrees,
throttled fetch shared by worktrees, carry / stash switches, refused switch keeps the changes,
remote branch checkout and cleanup, create from remote / local base), `suggest_names`,
`branch_refresh` (in-base detection, dry runs with commit and file conflicts, refused runs,
moved / skipped / left commits with carried changes, conflicting changes left in the stash,
detached). `ide-server/tests/git_actions_e2e.rs` (memory and real child transports): the whole
flow through `ide_client::Client`, including the session's recorded branch after
`git.refresh_branch`. `ide-proto` round trip covers the new messages.

## Pull requests and agent prompts

Port of Desk's PR backend (`src/main/github.ts`: `pullRequestStatus`, `pullRequestGate`,
`findOpenPullRequest`, `createPullRequest`, `mergePullRequest`, `closePullRequest`,
`updatePullRequestBranch`, `pullRequestLeftover`, `prDrift`, `pendingWork`, `unmergedWork`,
`remoteBranches`, `commitPrPreview`, `writeCommitPrText`, `prFilePath`, `resetPrFile`; IPC
`github:*` in `src/main/index.ts`), of the prompt texts (`renderer/src/lib/taskPrompts.ts`) and
of the renderer's PR decisions (`lib/prStatus.ts` `mergeBlocker`, `lib/prRecheck.ts`
`driftChange`, `lib/pullRequestFlow.ts` `mergeAfterCreateStep`, `lib/prTarget.ts`,
`lib/prPending.ts`). Server: `ide-server/src/pr.rs` (+ `pr/tests.rs`), dispatched from
`UserCtx::handle_files`; wire types and pure helpers: `ide-proto/src/pr.rs`; prompt texts and
the PR file path: `ide-core/src/prompts.rs`; prompt delivery: `ide-agents/src/prompt.rs`
(`AgentProvider::prompt_input`); client: `Client::pr_*`, `prompt_*`, `send_prompt`.
Capabilities `pr`, `send-prompt`.

**GitHub transport: Desk's way first, `gh` as the fallback** (`ide-server/src/pr/github.rs`,
`GithubTransport`, behind the `GhApi` seam the PR functions use).

- **Primary (Desk `credentialToken` / `fillCredential` / `githubApi` / `githubHeaders`).** The
  token is the password git's credential store has for the host: `git credential fill` with
  `protocol=https\nhost=<host>\n\n` on stdin, `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`,
  no console window (`CREATE_NO_WINDOW`), 20 s; the `password=` line, nothing on any failure.
  Cached per host (lower-cased) 10 min, "no token" 1 min; concurrent callers wait for the lookup
  in flight. Calls go over HTTPS (`ureq`, rustls) to `https://api.github.com` for github.com,
  `https://<host>/api/v3` otherwise (GraphQL: `/graphql`, Enterprise `/api/graphql`), with
  `Authorization: Bearer <token>`, `Accept: application/vnd.github+json`,
  `X-GitHub-Api-Version: 2022-11-28`, `Content-Type: application/json` for bodies, GET parameters
  form-encoded in the query, 20 s per request. Every HTTP status is passed on as is (403 / 429
  rate limits included, like Desk). The token is never logged, put in an error or audited.
- **401 with the credential token:** the token is forgotten (Desk `forgetToken`: the next call
  asks git again) and the same call is made once through `gh` if `gh` is found; without `gh`
  -> `github-auth` (`host`).
- **Fallback (`gh`), only when git yields no token** (no credential, helper failed, timeout):
  `gh api --hostname <host> --include -X <METHOD> <path>` (GET parameters as `-f k=v`, JSON
  bodies on stdin with `--input -`). `gh` is `IDE_GH` / `ServerConfig::gh` if set (missing file
  -> not found), else `gh` on the server's `PATH`, else `%ProgramFiles%\GitHub CLI\gh.exe`,
  `%LOCALAPPDATA%\Programs\GitHub CLI\gh.exe` (`/opt/homebrew/bin`, `/usr/local/bin`,
  `/usr/bin` elsewhere). No token and no `gh` -> `github-no-credentials` (`host`); `gh` exit 4
  (no login) or HTTP 401 -> `gh-auth` (`host`); no HTTP answer -> `gh-failed` (`detail`, also a
  30 s timeout). The process gets `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1`, `NO_COLOR=1`.
- **No HTTP answer with a token** (network, TLS, timeout) -> `github-failed` (`detail`); no `gh`
  retry (Desk: the `fetch` error).
- `pr.create` treats `gh-auth`, `gh-missing`, `github-auth` and `github-no-credentials` as "no
  API" and returns the prefilled page (Desk: no token -> page).
- Test hooks: `IDE_GITHUB_API` / `ServerConfig::github_api` replaces the API root of every host
  (and disables proxies), `ServerConfig::git_credential_env` adds environment to
  `git credential fill` (tests pass `GIT_CONFIG_GLOBAL=<scratch config with a test
  credential.helper>`, `GIT_CONFIG_NOSYSTEM=1`). `origin` is parsed like Desk (`git remote get-url origin`, https/ssh, credentials
dropped); when that URL is not a web URL (an `insteadOf` rewrite) the configured
`remote.origin.url` is parsed instead. GitLab: lookups/merge/close refused (`pr-gitlab`), create
returns the prefilled merge-request page.

### Hermes Desk handler -> ide RPC

| Hermes Desk | ide (`Request` -> `Response`, client method) |
|---|---|
| `github:prStatus` -> `pullRequestStatus` | `pr.status {session, repo}` -> `PrStatus(Option<PrStatus>)`, `pr_status`. List `pulls?state=all&head=<owner>:<branch>&per_page=20&sort=created&direction=desc`, same-repo heads only, open first; detail GET (one retry after 1.5 s while `mergeable` is null); latest review per reviewer. Detached -> `pr-detached`. |
| `github:prGate` -> `pullRequestGate` | `pr.gate {number}` -> `PrGate(PrGate{head_sha, review_decision, merge_state, checks})`, `pr_gate`. Desk's GraphQL query and `checkState` mapping. |
| `github:findOpenPr` -> `findOpenPullRequest` + `sessionBase` | `pr.find_open` -> `PrFound(Option<PullRequestResult>)` (`kind: Exists`, `base`, `mergeable_state`, `session_base` when the PR's target is not the session's base), `pr_find_open`. |
| `github:remoteBranches` | `pr.remote_branches` -> `PrTargets(PrTargets{branches, default_branch, current})` (`ls-remote --symref origin HEAD refs/heads/*`, 60 s), `pr_remote_branches`. |
| `github:pendingWork` | `pr.pending_work` -> `PrPendingWork(PendingWork{branch, commits (≤30), commit_count, files, published})`, `pr_pending_work`. |
| `github:prLeftover` | `pr.leftover {base: Option}` -> `PrLeftover(PrLeftover{head, file: Option<PrLeftoverFile{path, title, body, written_at, used_for, reason, matches, new_commits}>, changes, commits})`, `pr_leftover`. |
| `github:prFilePath` + `github:resetPrFile` | `pr.reset_file` -> `PrFile{path}`, `pr_reset_file`: removes `<file>` and `<file>.meta.json`, creates the folder. PR file = `<session folder>/.ide/pr-<repo name, [^A-Za-z0-9_.-]+ -> _>.md` (Desk: `.hermes-desk/`). Workspace sessions only (`pr-needs-workspace`). |
| `github:prDrift` | `pr.drift {since: Option, allowed}` -> `PrDrift(PrDrift{head, rewritten, commits: Vec<DriftCommit{sha, subject, full, expected}>, files})`, `pr_drift`. |
| `github:unmerged` -> `unmergedWork` | `pr.unmerged` -> `PrUnmerged(Option<UnmergedWork{commits, conflict}>)` against the session's base (`Session::base_of`), `pr_unmerged`. Not cached. |
| `github:commitPrPreview` | `pr.commit_preview {base, push}` -> `PrCommitPreview(CommitPrPreview{title, body, commits, head, source_head, base_head, branch})`, `pr_commit_preview` (fetches `origin/<base>`, and `origin/<branch>` when `push` is false). |
| `github:commitPrText` | `pr.commit_text {base, options: CommitPrTextOptions}` -> `PrTitle(String)`, `pr_commit_text`. |
| `github:createPr` -> `createPullRequest` | `pr.create {base, draft, push, push_sha, options}` -> `PrCreated(PullRequestResult)`, `pr_create`. The PR file comes from `(session, repo)`, never from the client. Same order and gates as Desk: pins revalidated (`pr-changed`), detached / same branch / no origin branch without push, open-PR lookup first (an existing PR settles the text: file removed, `Exists`), PR file read (`pr-no-text`, `pr-empty-title`; title taken literally for commit-based text), `push_sha` must be on the branch (`pr-bad-sha`, `pr-not-on-branch`), meta written (`head`, `branch`, `base`, `attemptAt`), push (`push -u origin HEAD`, or `<sha>:refs/heads/<branch>` + set upstream; a failure is recorded as the meta `reason` `pr-push-failed` and returned), then `POST pulls {title, body, head, base, draft}`. No API (`gh-missing`, `gh-auth`, GitLab `pr-gitlab-page`) -> `Page` with the compare URL; refusal -> one reconciling lookup, else `Failed` (`reason` `pr-api`, also stored in the meta); lost answer -> lookup, else `pr-create-unconfirmed`. Audited (`pr.create`). |
| `github:mergePr` -> `mergePullRequest` | `pr.merge {number, sha, method: PrMergeMethod, delete_branch}` -> `PrMerged(PrMerged{sha, branch_deleted, message})`, `pr_merge`. GET first: merged -> `pr-already-merged`, not open -> `pr-not-open`, head not starting with `sha` -> `pr-head-moved` (`number`, `expected`, `actual`); `PUT …/merge {sha: <head>, merge_method}` (refusal `pr-merge-refused`); fetch of the base; same-repo head branch deleted on request (204/422 = deleted). Audited. |
| `github:closePr` -> `closePullRequest` | `pr.close {number}` -> `PrClosed(PrClosed{number, url})`, `pr_close`: open check, `PATCH {state: closed}` (`pr-close-failed`). Audited. |
| `github:updatePrBranch` | `pr.update_branch {number, sha}` -> `PrBranchUpdated{message: Option}`, `pr_update_branch`: `PUT …/update-branch` with `expected_head_sha` for a 40-hex `sha` (`pr-update-refused`). Audited. |
| `taskPrompts.renderCommitPrompt` (Workspace `commitPrompt`, 커밋 맡기기) | `prompt.commit {files, all}` -> `Prompt(String)`, `prompt_commit`. Desk's default template + `worktreeSafety` + allowlist, repo = registered name, folder = the worktree path. |
| `taskPrompts.renderPrPrompt` (`requestPullRequest`) | `prompt.pr {target, exclude: PrExclusions}` -> `Prompt(String)`, `prompt_pr`: Desk's PR template with the PR file path and `exclusionLines` (branch = the worktree's branch). `target` checked like a branch (`pr-bad-branch`). Workspace sessions only. |
| `askAgent` / `sendPromptTracked` (chat send) | `session.send_prompt {session, text}` -> `Unit`, `send_prompt`. See below. |

Pure helpers (ide-proto, re-exported by ide-client): `merge_blocker(&PrStatus) ->
Option<MergeBlocker>`, `merge_after_create_step(pr, number, gate, blocked_for_secs, target) ->
MergeStep` (+ `MERGE_POLL_SECS`, `MERGE_MAX_WAIT_SECS`, `UNEXPLAINED_BLOCK_SECS`),
`drift_change(&PrDrift, before_head, before_files, files_included) -> Option<PrRecheckChange>`,
`choose_pr_target(branches, current, preferred, worktree_base, default) -> Option<String>`,
`effective_exclusions(Option<&PendingWork>, PrExclusions) -> Option<PrExclusions>`.

### Delivering a prompt (`session.send_prompt`)

The provider says how its CLI takes input (`AgentProvider::prompt_input() -> PromptInput`,
`ide_agents::prompt_writes`); the server writes to the running PTY (`session-not-running`
otherwise). The text is cleaned first: CRLF/CR -> LF, every control character except LF and
tab removed (so it can neither end a paste nor start an escape sequence), trimmed; nothing left
-> `prompt-empty`. Audited (`session.send_prompt`, length only).

| Provider | Behaviour |
|---|---|
| Claude Code, Codex | `Paste { submit_delay_ms: 300 }`: only while the server-side terminal model has bracketed paste on (DECSET 2004, i.e. the TUI shows its input), else `prompt-not-ready` and nothing is written. Writes `ESC[200~` + text (LF -> CR, the app's own paste format) + `ESC[201~`, waits 300 ms, writes CR (Enter). |
| Shell (and declarative providers by default) | `Lines`: text with LF -> CR plus a final CR, i.e. every line typed and entered. |
| Declarative | `[prompt] mode = "paste"` (+ `submit_delay_ms`) or `mode = "lines"`. |

Verified: the byte sequences (unit tests), the shell path end to end (the echoed marker appears
in the terminal), the not-ready refusal with a paste-mode declarative provider whose program
never enables bracketed paste. **Not verified against the real CLIs** (tests never send prompts
to `claude` / `codex`): that Claude Code / Codex accept the paste and submit on the delayed CR.
Both enable bracketed paste in their input (this is what the app's own paste relies on); a
TUI that queues input while busy may submit later.

### What the UI does per Desk control

The app keeps Desk's flows; the server has no PR job / notice state.

| Desk control | Calls |
|---|---|
| 커밋 맡기기 (checked files; tooltip, disabled while `ai.blocked` / nothing checked) | `prompt_commit(files, all = every row checked)` then `send_prompt(session, text)`; show `prompt-not-ready` / `session-not-running` as Desk's "지금은 요청을 보낼 수 없어요". There is no chat turn tracking: "에이전트가 답변 중" blocking is not available (no busy signal from the PTY). |
| PR status (open PR turns "PR 올리기" into "머지하기") | `pr_status` when the repo is shown and when the commit list / unpushed set changes (Desk: on `commits`/`unpushed` change and after sync); `pr.state == Open` -> open PR. A `github-no-credentials` / `github-auth` / `github-failed` / `gh-missing` / `gh-auth` / `gh-failed` error means "unknown", not "no PR". For the sibling's sync status `merged_head`: `pr.head_sha` when `pr.state == Merged`. |
| `...` -> 머지하기 · #N (hint) | Disabled with Desk's hint when uncommitted changes / unpushed commits, else `merge_blocker(&pr)` (translate `MergeBlocker`). Merge dialog (Squash default, 원격 브랜치 삭제 on) -> `pr_merge(number, pr.head_sha, method, delete)`; then `pr_status` again. |
| `...` -> PR 보기 | open `pr.url` (client-side). |
| `...` -> PR 닫기 · #N (confirm) | `pr_close(number)`, then `pr_status`. |
| `...` -> PR 올리기 (disabled: not a workspace session, `sync_status.no_remote`, nothing to push) | Dialog: `pr_pending_work` ("PR 올리기 전에 확인", skip when nothing), `pr_remote_branches` + `choose_pr_target` (session base first), `pr_leftover(target)`, `pr_commit_preview(target, push)` for the default "커밋 기록으로 작성" mode, `pr_find_open` before generating (an existing PR is shown instead; a failed lookup is an error, never "no PR"). Submit: see the job below. |
| PR job, AI mode | `pr_reset_file`, `pr_drift(None, [])` (snapshot: `head`, files), `prompt_pr(target, exclude)` + `send_prompt`. Desk resumes on the chat turn's end; here the UI waits for the user ("PR 만들기" on the job notice) or polls `pr_leftover` until `file` appears, then `pr_drift(Some(before.head), allowed)` + `drift_change(...)`: a change -> the recheck dialog (decision -> `pr_create(.., push_sha = keep_up_to)` / commit prompt for the files / `pr_create(.., push_sha = drift.head)`), none -> `pr_create(target, draft, push, None, None)`. 재요청 = reset + send again; 취소 = stop waiting (the agent cannot be interrupted except by the user in the terminal). |
| PR job, commit mode | `pr_commit_text(target, options with the preview pins and edited title)` then `pr_create(target, draft, push, Some(preview.head) when pushing, Some(options))`. |
| PR result | `Created` / `Exists` -> open `url`, `pr_status` (dirty -> conflict notice); `Page` -> warning with `reason` + open the prefilled `url`; `Failed` -> error with `reason`, page as alternative. |
| PR 올린 뒤 바로 머지 | poll every `MERGE_POLL_SECS` up to `MERGE_MAX_WAIT_SECS`: `pr_status`, `pr_gate` (stop using it after one failure), `merge_after_create_step(..)`; `Merge{sha}` -> `pr_merge`. |
| 올리지 않은 PR 내용이 있어요 strip | `pr_leftover(None)` when commits / unpushed / PR number or state change; show when `file.is_some()` and no open PR and no recheck pending; click opens the PR dialog (reuse = `pr_create` without asking the agent). |
| PR 올리기 전 확인이 필요해요 strip / PR 확인 필요 item | client-side recheck state (Desk's `prRecheck` store) built from `drift_change`. |
| 커밋 보유 badge | `pr_unmerged` (`commits > 0`). |
| Update branch (PR card) | `pr_update_branch(number, pr.head_sha)`; `message` `None` -> Desk's "브랜치를 업데이트하고 있어요". |

### Error ids

New (`ProtoError`, args in parentheses): `github-no-credentials` (`host`; no credential token
and no `gh`), `github-auth` (`host`; the credential token got 401 and there is no `gh` to retry
with), `github-failed` (`detail`; no HTTP answer with the credential token), `gh-missing`
(`program`; the server no longer returns it — without a token the transport reports
`github-no-credentials` — kept for the `GhApi` seam), `gh-auth`
(`host`), `gh-failed` (`detail`), `pr-no-origin`, `pr-origin-unparsable`
(`url`), `pr-gitlab`, `pr-gitlab-page` (a `Page` reason only), `pr-detached`, `pr-same-branch`
(`branch`), `pr-no-remote-branch` (`branch`), `pr-bad-branch` (`branch`), `pr-bad-number`,
`pr-bad-sha` (`sha`), `pr-not-on-branch` (`sha`), `pr-changed`, `pr-no-text`,
`pr-empty-title`, `pr-bad-title`, `pr-no-commits` (`base`), `pr-needs-workspace`, `pr-api`
(`status`, `detail`), `pr-merge-refused` (`status`, `detail`), `pr-close-failed` (`status`,
`detail`), `pr-update-refused` (`status`, `detail`), `pr-already-merged` (`number`),
`pr-not-open` (`number`), `pr-head-moved` (`number`, `expected`, `actual`: 7-char shas),
`pr-push-failed` (`detail`; meta `reason` only, the push error itself is returned),
`pr-create-unconfirmed` (`detail`), `prompt-empty`, `prompt-not-ready` (`id`). Reused:
`session-not-running` (`id`), `git` / `git-spawn` / `git-timeout`, `session-not-found`,
`repo-not-attached`, `bad-request`.

### Deviations (with reasons)

- **`gh` fallback** (Desk has none): hosts git has no token for still work when `gh` is logged
  in, and a token GitHub refuses (401) is retried once through `gh` (Desk only forgets it).
- **`User-Agent: ide-server`** (Desk: `hermes-desk`).
- **Server-chosen PR file**: `pr.create` / `pr.commit_text` take no path (Desk passes the path
  from the renderer); folder `.ide/` instead of `.hermes-desk/`.
- **Meta `reason` is a `ProtoError`** (translatable) instead of Desk's Korean sentence;
  `written_at` / `attemptAt` in seconds.
- **Commit-based PR body** ends with `_커밋 기록으로 작성했어요 (ide)._` (Desk: `(Hermes Desk)`).
- **No PR records / events** (`github:prEvents`, `prSighting`, `mergedPrHead`, focus recheck):
  ide has no chat to show "PR #N이 올라왔어요"; `pending_work` therefore does not subtract a
  merged PR's commits (pass the merged head to the sibling's `git.sync_status` from `pr_status`).
- **No per-repo prompt templates** (`repo.taskPrompts` / the global customization): Desk's
  defaults only.
- **No turn tracking**: `session.send_prompt` returns when the bytes are written; the UI cannot
  know when the agent finished (see the PR job above).
- **Not ported**: `closeReplacedPullRequest` (새 브랜치로 이어가기's "close the old PR" with a
  comment; the UI can use `pr_close` after checking containment itself, but the comment and the
  `previous_head` containment check are not done server-side), `openPullRequestOf`,
  `recheckPullRequest`, the `unmergedWork` cache.
- **`pr.unmerged` uses the session's base** (Desk: repo rule / setting / worktree base through
  `resolveBaseBranch`, which `Session::base_of` ports).

### Tests

`ide-server/src/pr/tests.rs` (local bare `origin` behind an `insteadOf` GitHub URL, in-process
fake `GhApi` recording calls; the transport tests run `git credential fill` against a scratch
config only — `GIT_CONFIG_GLOBAL` with a test `credential.helper`, `GIT_CONFIG_NOSYSTEM=1` —
and a local fake GitHub over HTTP; never the user's credential store or real GitHub):
`api_roots_like_desk`, `transport_sends_the_credential_token_over_https` (Bearer token, query
encoding, GraphQL body, gh untouched, cache), `transport_falls_back_to_gh_without_a_token`
(gh called, HTTP untouched; no `gh` -> `github-no-credentials`),
`transport_forgets_a_refused_token` (401 -> forgotten + one `gh` retry; no `gh` ->
`github-auth`; no answer -> `github-failed`), `origins_urls_and_gh_output` (origin parsing, compare URLs,
`gh --include` parsing, argument lists, `IDE_GH` resolution, porcelain), `pr_file_reading`,
`status_gate_and_open_lookup` (filtering, reviews, gate mapping, session base, 401 / gh missing
/ refusal / retry / detached), `create_pushes_then_opens_the_pr` (no text, push + upstream,
POST body, existing PR, same branch, no remote branch), `create_without_api_or_refused_leaves_the_text`
(`Page` for gh-auth / gh-missing / github-no-credentials, `Failed` + reconciling lookup + meta reason, leftover
same/ahead/diverged), `commit_based_pr_is_pinned`, `merge_close_update`,
`pending_drift_unmerged_targets`. `ide-server/tests/pr_e2e.rs`: a fake `gh` script (`.cmd` /
`sh`; records arguments and stdin, prints canned `--include` output) through the client over
memory with a no-token credential config (prompts, PR file, local state, status / close / refused merge, `send_prompt` into a
shell, `prompt-not-ready` for a paste-mode provider, original-repo session) and a real server
child with `IDE_GH`, `IDE_GITHUB_API` and a scratch `GIT_CONFIG_GLOBAL`
(`credential_token_first_in_a_server_child`: token -> fake GitHub over HTTP with
`Bearer`, gh not run; no token -> fake `gh`; neither -> `github-no-credentials`). `ide-core` `prompts` tests compare the full commit
prompt with Desk's output; `ide-agents` `prompt` tests the byte sequences and the declarative
`[prompt]` table; `ide-proto` `pr` tests the pure helpers and `pr_messages_round_trip`.

### Footer UI (ide-app `footer.rs`, `footer_view.rs`)

The Changes / History footer is a port of Desk's `AiActionBar` + `SyncButton` +
`PullRequestModal` / `MergePrModal` / close confirm / `BranchRefreshModal` / `PrRecheckModal`
and the `pr-leftover-strip` strips, using the calls in the tables above.

| Desk | ide |
|---|---|
| `AiActionBar` under Changes and History, hidden on Explore, kept mounted | `footer::shown_on(side)`; drawn by `detail_view.rs` under the tab content; state per session (`Detail::footer`) and per repo (`RepoFooter`), so it survives tab switches |
| 커밋 맡기기 (violet round button, sparkles, count badge when only some rows are checked; tooltip) | `FooterMsg::Commit`: `Detail::checked_changes()` (sibling API) -> `prompt_commit(files, all)` + `send_prompt(selected session)`; `prompt-not-ready` / `session-not-running` shown as a toast |
| `SyncButton` (primary op with ↑/↓ counts, publish / push / pull / fetch; menu: 가져오기 (fetch), 풀 (병합), 풀 (리베이스), 브랜치 게시 / 푸시, 주의가 필요한 작업: 강제 푸시, 원격 상태로 덮어쓰기 with confirms; 원격 없음 when `no_remote`; periodic fetch `FETCH_EVERY_MS`, `FRESH_ON_OPEN_MS`) | `sync_button` / `sync_menu`; `git_sync_status` (fetch on open when older than 60 s and every 5 min while shown), `git_push` / `git_pull`; force push and reset go through `SyncConfirmForm`; merged-PR hints from `pr_status` |
| `…` round icon menu with item descriptions: PR 올리기, 머지하기 · #N, PR 보기, PR 닫기 · #N, PR 확인 필요, 새 브랜치로 이어가기 | `App::footer_more(sid) -> Vec<MoreEntry>` (kind, enabled, hint; the script checks it per state); 머지하기 disabled with Desk's hint while changes / unpushed commits exist |
| PR 보기 / opening a created PR | `footer_open_url`: the session's web detail tab (the ide's in-app browser; Desk uses the OS browser via `openExternal`) |
| `PullRequestModal` (PR 올리기 전에 확인 step, 머지 대상 브랜치, 커밋 기록으로 작성 + body preview, PR 제목, leftover file, 초안, PR을 올린 뒤 바로 머지, 기존 PR 확인) | `Modal::Footer(FooterModal::Pr)`; PR job notice card above the footer (AI mode: waits for the PR file by polling `pr_leftover` every 2 s, or 'Create PR' / 'Ask again' / 'Cancel') |
| `MergePrModal` (method, 머지 후 원격 브랜치 삭제), close confirm | `FooterModal::Merge` / `FooterModal::ClosePr` |
| `BranchRefreshModal` (새 브랜치 이름, 가져갈 커밋 picks, 변경 옮기기, 열린 PR #N 닫기, pre-check, result lines) | `FooterModal::Continue`: `git_branch_refresh_info` / `_check` / `_run`, then `pr_close` when the box is on |
| `pr-leftover-strip`, PR 확인 필요 strip, `PrRecheckModal` | strips above the footer; `FooterModal::Recheck` built from `drift_change` |
| Server errors | `i18n::proto_error` routes `gh-*`, `github-*`, `pr-*`, `prompt-*` to `footer::proto_error_text` (`err-*` messages) |

Deviations / omissions:

- **No "agent busy" signal**: Desk's `ai.blocked` hints (에이전트가 답변 중) are omitted; the
  buttons only need a connection and checked files.
- **AI PR job end**: Desk resumes on the chat turn's end; the ide polls for the PR file and offers
  'Create PR' on the notice.
- **PR pages open in the in-app web tab**, not the OS browser.
- **Job notice texts** are formatted when the event happens (they do not re-translate on a
  language switch).
- **브랜치 업데이트** (`pr_update_branch`) is a "…" menu entry while the open PR is behind its
  base (Desk: a button on the Merge page). The 커밋 보유 / branch state badge is on the session
  list (`branch_badge.rs`: `git.status`, `git.sync_status`, `pr.status`, `pr.unmerged` per repo,
  aggregated like Desk's `aggregateBranches`, every 30 s).
- **Closing the old PR on 새 브랜치로 이어가기** uses `pr_close` without Desk's comment.

Script (`script_footer.rs`, spliced after the base-branch pass; run with `IDE_GH` = the fake `gh`
`crates/ide-app/tests/fake-gh/gh.cmd` (Node `gh.js`: `pulls` list / create / get / close / merge,
reviews, GraphQL gate, ref delete; PRs in `$IDE_FAKE_GH_STATE/state.json`, commits from and squash
merges into the bare origin `$IDE_FAKE_GH_ORIGIN`), `IDE_GITHUB_API` = an unreachable local address, `GIT_CONFIG_GLOBAL` = a scratch config
whose credential helper answers `quit=1`, `GIT_CONFIG_NOSYSTEM=1`; the test repo's origin is
`https://github.com/ide-test/footer.git` rewritten by `url.<bare origin>.insteadOf`, so git
pushes to a local bare repo and the PR backend falls back to the fake `gh`): footer shown on
Changes / History, hidden on Explore; 커밋 맡기기 into a shell session (prompt line in the
terminal); publish, push (↑1), fetch + pull (↓1) counts; `…` entries per state (no PR, open PR,
uncommitted change disables 머지하기); PR 올리기 (commit mode) -> open PR shown in the web tab
-> 머지하기 -> merged; pending step + PR 닫기; AI mode (prompt in the terminal, PR file written
by the script) -> created; 새 브랜치로 이어가기 (closes the PR, worktree moves to `-2`).
Screenshots `foot_01`..`foot_16`, `ko_foot_01_footer`, `ko_foot_02_sync_menu`,
`ko_foot_03_more_menu`, `ko_foot_04_pr_dialog`, `ko_foot_05_merge_dialog`,
`ko_foot_06_branch_continue`.
