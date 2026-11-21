# Repositories and session workspaces

This mirrors Hermes Desk (`powered_ide`, read at commit 2028c9d). Each row names the Desk source,
what it does, and where ide does the same. Deviations are listed at the end.

## Desk behaviour -> ide

| Hermes Desk | Behaviour | ide |
|---|---|---|
| `src/main/workspace.ts` `Workspace` state (`workspace.json`: `repos`, `worktrees`, `sessions`) | One registry of repositories plus session workspaces and their worktrees | `ide-core::registry::RepoStore` (`<data>/repos.json`) + per-session `Session::worktrees` / `Session::root` (`sessions.json`) |
| `Repo { id (8 hex), name (folder name), path (top level), baseBranch? }` | Registered repo | `ide_core::RegisteredRepo { id, name, path, base_branch, created_at }` (`RepoId` = 8 hex) |
| `addRepo(path)`: `git rev-parse --show-toplevel`; not a git repo -> error (or `git init` on request); an already registered top level returns the existing entry | Register | `RepoStore::register` (same rules; `git init` offer not ported) -> RPC `repo.register { path }` |
| `removeRepo(id)` (레포 page "등록 해제 (파일 유지)") | Unregister, files kept | `RepoStore::unregister` -> `repo.unregister { id }` (see deviations: refused while sessions use it) |
| `mainBranch(repo)`: `main`, else `master`, else origin's default (`origin/HEAD`), else the current branch; start from `origin/<name>` when the local branch is missing or only behind it | Automatic base branch | `ide_core::registry::detect_base` (same order and start-point rule) |
| `repo.baseBranch` set on the 레포 page | Fixed base for new worktrees | `RegisteredRepo::base_branch` (stored; the 레포 page shows the effective base, editing not ported) |
| `createSession(repoIds, ...)`: key, folder `<root>/<key>/`, one worktree per picked repo at `<folder>/<repo name>` (`-2`, `-3` on name clash, `freeName`), branch `hermes/<key>` from the base, `git worktree add --no-track -b`; a repo failure is reported, not fatal; then `HERMES.md` | New session workspace | `ide_core::workspace::create_session`: folder `<data>/workspaces/<session-id>/`, worktree `<folder>/<repo name>` (same suffix rule), branch `ide/<session-id>`, failures per repo returned with the session (`SessionCreated.failures`, shown as Desk's "일부 레포의 워크트리를 만들지 못했습니다"), then the session guidance |
| `worktreeAdd` cleanup on failure (half-written folder, registration, new branch) | No leftovers | `workspace::add_worktree` removes the folder, prunes and deletes the new branch on failure |
| `sessionInstructions()` -> `<folder>/HERMES.md`, loaded by Hermes as project context because the folder is the cwd | Session guidance: worktree folder, repo, branch, base; rules (work only here, originals are read-only, commit to the listed branches, read each repo's AGENTS.md) | `ide_core::workspace::session_guidance()`; written by the session's provider where its CLI loads project context from the cwd (Claude `CLAUDE.md`, Codex `AGENTS.md` in the session folder; other providers get it through their instructions mechanism). Never inside a worktree |
| Session cwd = the workspace folder | Agent root | `Session::root` = the workspace folder (agent cwd); `--add-dir <repo>/.git` for Claude/Codex so commits in the worktrees can write the shared git dir |
| `NewSessionModal.tsx`: repos pre-checked from `defaultReposFor(agent, repoIds)` (agent's saved list, every repo when none is saved), user adds/removes any, zero repos = empty workspace ("레포를 고르지 않으면 빈 워크스페이스로 시작해요") | New-session dialog | New-session modal: configuration picker -> its `default_repos` pre-checked; check/uncheck any registered repo; zero allowed |
| `lib/agentRepos.ts` (per agent/profile, renderer localStorage) + `AgentsPage.tsx` MultiSelect "기본 레포" ("새 세션을 만들 때 미리 선택되는 레포입니다") | Default repos | `Configuration::default_repos: Option<Vec<RepoId>>` (`None` = every registered repo, as Desk), edited on the 구성 page as a checklist |
| `NewSessionModal` "워크트리 사용" unchecked -> `direct` session: runs in the registered repo's own checkout, no workspace, no worktree | Original-repo mode | `NewSession.original_repo` (`SessionMode::OriginalRepo`): `Session::root` = the repo path, one `SessionWorktree` pointing at the checkout; nothing is written into the repo (providers inject only through flags/scratch files) |
| `SessionSettingsModal.tsx` + `addRepoToSessions(repoId, [key])` (+ the agent's `desk_repo attach`) | Attach a repo to a running session; guidance rewritten | RPC `session.attach_repo { id, repo }` from the session repos dialog (⚙ next to the Changes/Explore repo picker; Desk's "세션 설정" repo table: add = check + 저장); guidance regenerated at once (`refresh_root`) |
| `removeSession(key, deleteBranches)` -> `removeWorktree` each (`git worktree remove --force`, `rm` fallback inside managed folders, `worktree prune`, `branch -D`), then `rm -r` of the folder | Delete cleanup | `ide_core::workspace::remove_session_files` (same steps; folder removed only inside `<data>/workspaces`) |
| `ChangesPanel.tsx` + `BranchSelect.tsx` `repo / branch` button; popover "레포" lists the session's worktrees (`dirOptions` in `pages/Workspace.tsx`), picking one switches the panel; Changes / History / Explore share it | Repo selector | Detail side panel: a `repo / branch` pick list above Changes and Explore (one selection per session, like Desk's per-tab folder), ⚙ opens the session repos dialog. Items (files, diffs) are keyed `<repo id>:<path>` so tabs of several repos coexist |
| `ReposPage.tsx` + `RepoList.tsx` ("레포", "레포 추가", name + path rows) | 레포 page | Rail -> 레포 관리: repo list left (name, session count, 레포 추가 = server folder picker, typed path + Enter), selected repo right (name, path, 기준 브랜치, sessions using it, "등록 해제 (파일 유지)" -> confirm "등록 해제" / 취소 like Desk's `RepoActions`) |
| `writeGuard()` + `isManagedPath()` | Agents blocked from originals; only managed folders are deleted | File/git/search RPCs resolve `(session, repo)` to that session's worktree only (`RootJail` per worktree); deletes only inside `<data>/workspaces` |

## Data layout
```text
<data>/repos.json                         registered repos
<data>/workspaces/<session-id>/           session folder = agent cwd
    <repo name>/                          worktree on ide/<session-id>
    CLAUDE.md | AGENTS.md                 session guidance (by provider)
    .claude/skills/ | .agents/skills/     configuration skills (by provider)
<data>/sessions/<session-id>/agent/       scratch files (MCP JSON, instructions file, plugin)
<data>/server.json browse_roots           read once for the migration, no longer a jail
```

## Migration (server start, `ide_core::workspace::migrate_workspaces`)
- Runs while `repos.json` does not exist: registers every repo used by an existing session and
  every browse root (`server.json`) that is itself a git top level.
- Every session without `worktrees` (single-repo layout): keeps its worktree where it is
  (`<data>/worktrees/<slug>/<id>`, files are not moved), records it as the one attached repo, and
  gets a session folder `<data>/workspaces/<id>/` as its new cwd. The guidance lists the worktree by
  absolute path and the provider passes it with `--add-dir`. Deleting such a session removes the old
  worktree and branch as before.
- `server.json` is left in place; the browse-roots RPCs and settings UI are removed.

## Deviations (no Desk equivalent or deliberately different)
- Branch `ide/<session-id>` instead of `hermes/<key>` (existing prefix guard `BRANCH_PREFIX`).
- Session folder under the data dir (`<data>/workspaces`) instead of `~/hermes-workspaces`.
- Default repos live in the server-side configuration instead of the renderer's localStorage
  per agent (server-authoritative).
- Unregistering a repo used by sessions is refused (`repo-in-use`); Desk unregisters and leaves
  the worktrees. Refusal keeps cleanup possible (deletes need the repo).
- Detach (remove one repo's worktree from a live session, branch kept) has no Desk UI equivalent:
  Desk's session settings only add repos ("repos can be added, not removed").
- Guidance is not a fixed `HERMES.md`; each provider puts the same text where its CLI reads project
  context.
- Text search (Search tab) searches every attached repo and groups results under a repo header;
  Desk has no repo-wide text search panel to mirror, so this follows the task spec. Quick open
  (Ctrl+P) also covers every repo.
- Agent guidance per provider: Claude `CLAUDE.md`, Codex `AGENTS.md` in the session folder (the
  file Desk calls `HERMES.md`); original-repo sessions get the same text through the
  instructions flags (`--append-system-prompt-file` / `developer_instructions`). See
  `agent-providers.md` for what was verified.
- The new-session dialog uses one checkbox per registered repo instead of Desk's add/remove table
  (same result: pre-checked defaults, any subset, zero = empty workspace).
- Not ported: base-branch rules / per-session base overrides / per-repo branch names in the new
  dialog, 기준 브랜치 최신화 (fetch), orphan workspace reuse, child sessions, `git init` of a plain
  folder, task prompts per repo, the agent-side `desk_repo` tool.
