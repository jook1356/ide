# Session families (parent / child sessions)

Status: implemented (protocol 14): model and rules (`ide-core/src/family.rs`), server
(`session.create_child`, `session.merge_child`, `session.sync_parent`), bridge tools, app
(tree, dialog, row menu). Tests: `ide-core` `family::tests`, `ide-server/tests/family_e2e.rs`.
Goes beyond Hermes Desk, whose child sessions only share the parent's
folder ("공유 작업 폴더 · 독립 대화"): here a child either shares the parent's workspace or gets
its own worktrees branched from the parent's, and the parent merges them back.

## Model

- A session may have a `parent` (another session). Depth: a root, its children, their
  children (two levels below a root). A child of a grandchild is refused (`family-too-deep`).
- Only workspace sessions have children (`family-needs-workspace`): the parent's worktrees are
  the **integration worktrees** (branch `ide/<parent>`, base: the repo's base as usual). The
  parent is an ordinary workspace session: repos picked at creation (configuration defaults),
  more attached later in 세션 설정.
- **Separate child** (`shared = false`): its own session folder with one worktree per picked
  repo, **only repos the parent has** (`repo-not-in-parent`); each starts at the parent's
  branch tip and its base is the parent's branch (`ide/<parent>`, a local branch: status,
  diffs and badges resolve it in the worktree, which shares the repository's refs). Repos
  attached later must also be the parent's; base again the parent's branch.
- **Shared child** (`shared = true`): the parent's folder and worktrees, as they are (no disk
  changes). It cannot attach or detach repos (`shared-child-repos`: change the parent). Its
  provider files are not written into the shared folder (`SessionContext::shared_root`): the
  guidance goes into its instructions instead.
- The child runs the parent's agent (provider, account, configuration) unless the request
  names others.

## Rules

- A session with children cannot be deleted (`session-has-children`).
- A parent cannot detach a repo its separate children use (`repo-used-by-children`).
- Deleting a separate child removes its worktrees like any session (with the dialog's choice);
  deleting a shared child never touches the shared folder or worktrees.

## Integration

- **Merge a child** (parent): per repo of the child, in the parent's worktree,
  `git merge --no-ff ide/<child>` (or `--squash` + commit). Result per repo: merged, already
  up to date, conflicts (files; the merge is left in progress for the parent to resolve in its
  Changes panel), failed (e.g. local changes in the way).
- **Sync from the parent** (child): per repo, in the child's worktree, `git merge
  ide/<parent>`; same results.
- Uncommitted work is never part of either: only commits on the branches.

## Agent bridge tools

| Tool | Who | What |
|---|---|---|
| `ide_list_related_sessions` | any | parent, siblings, children: id, title, mode, activity, repos / branches |
| `ide_create_child_session` | depth allows | `{title?, shared?, repos?, prompt?}`: create, start, send the first prompt |
| `ide_send_to_session` | related | `{session, message}`: delivered at once when the target is idle, else queued and delivered when its turn ends; prefixed with the sender |
| `ide_merge_child` | parent | `{child, squash?}` -> per repo result |
| `ide_sync_from_parent` | separate child | per repo result |

Related = parent, children and siblings. Messages go through the target's terminal (its
provider's prompt input), never through tests (no prompts to real CLIs).

## App

- Session list: children under their parent (indented per level, the parent folds them with a
  count), as Desk's tree.
- Row menu: 하위 세션 만들기 (title, 분리 / 공유, the parent's repos for 분리).
- Errors above appear as the usual banner.
- **The detail area is per folder**: the sessions of one folder (a parent and its shared
  children) share one detail area — open items (files, diffs, commits), browser tabs, the web
  page and the side panel (tab, repo, filters, scripts). The app keeps it in the folder owner's
  state (`App::workspace_owner`: up the parents while `shared`; `detail()` / `detail_mut()` /
  `persist_detail` / the views use it). Terminals stay each session's: the terminal lists show
  the selected session's (`App::term_live`), another session's terminal that is the folder's
  active item is not shown (the page instead), terminal ids name their session
  (`<session>:t<n>`) for lookups. Separate children have their own folder, so their own detail
  area. 공통 tabs stay global.

## Protocol (14)

| Request | Payload |
|---|---|
| `session.create_child` | `{parent, shared, repos, title, start, cols, rows}` -> `SessionCreated` |
| `session.merge_child` | `{id (child), squash}` -> `Merges([{repo, name, result}])` |
| `session.sync_parent` | `{id (child)}` -> `Merges` |

`SessionInfo.parent` / `.shared`; `MergeResult` = `Merged` / `UpToDate` / `Conflicts{files}` /
`Failed{error}`. Messages queued by `ide_send_to_session` (and a child's first prompt) are typed
in with `session.send_prompt`'s input rules when the target is not working, not waiting for the
user and its TUI is at its input (retried every screen tick, 0.7 s).

## Write lease of a shared folder

A folder shared by a parent and its shared children has one writer at a time (the folder's
owner, i.e. the parent, by default; `SessionInfo.writer`, ✎ in the list).

- Enforced by hooks: a `PreToolUse` file edit (`AgentHookEvent::PreEdit`) by a member that does
  not hold the access, of a path inside the folder (or without a path), is refused with a reason
  that names the holder and the ways out. Edits outside the folder and separate children are not
  guarded. Shell commands are not guarded (hooks cannot tell what a command writes); the
  guidance asks members not to write through the shell without the access.
- `ide_request_write_access`: granted at once when the holder is not working (not Working /
  NeedsInput, or not running), else refused; `ide_release_write_access`: back to the owner. A
  deleted holder hands it back too. In memory: a daemon restart gives it back to the owners.
- Codex: its hooks are not registered yet (`docs/agent-bridge.md`), so its edits are not refused;
  it gets the same guidance.

## Later
- Reading a related session's conversation (provider transcripts).
