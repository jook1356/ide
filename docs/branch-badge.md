# Session list branch badge

The badge on each session row (변경 없음, 커밋 필요, 푸시 필요 / 게시 필요, PR 열림 / PR 충돌,
커밋 보유, PR 머지됨, 부모에 머지됨, 머지됨; 확인 중 before the first check, 조회 실패 when it
failed) started as Hermes Desk's `SessionBranchBadge`. The server computes and keeps it.

## Rule: git decides what is merged

A session's work is merged where **git** says it is: a target contains it by ancestry
(`git merge-base --is-ancestor`, what `git branch --merged` uses). It does not matter who merged,
how (ide's 머지, an agent's `git merge`, a terminal, GitHub) or into which target.

| How the work moved | git | Badge |
|---|---|---|
| Merge commit (conflicts resolved any way) | merged | 부모에 머지됨 / 머지됨 |
| Fast-forward; a child rebased onto its parent, then fast-forwarded | merged | 부모에 머지됨 / 머지됨 |
| Squash, cherry-pick, `checkout <branch> -- .`, `diff \| apply`, copied files | not merged | 커밋 보유 |
| The target's branch rewritten after the merge (the commits are gone from it) | not merged | 커밋 보유 |
| A pull request merged on GitHub by squash / rebase, or merged but not fetched yet | not merged (yet) | PR 머지됨 (GitHub's fact, shown apart) |

ide does not try to see through what git does not record (no content matching, no guessing
where copied files came from).

## Per repo of a session

- **Targets**, nearest first: the branches of its parents (separate children; a shared child
  works on its parent's branch), then the base of the root workspace session: local
  `<base>` and `origin/<base>`. Original-repo sessions have none (no 커밋 보유 / 머지됨).
- **Unlanded** (`BranchState::unlanded`): `git rev-list --count --no-merges HEAD --not
  <targets>`, the commits no target has (a sync merge from the parent is not counted).
- **Own work**: the newest commit made on the branch, from its reflog (commit, amend,
  cherry-pick, revert, am, and a rebase of such work; not merges, fast-forwards, resets or
  the branch's creation). A child that only synced from its parent has none, so it shows
  변경 없음, not 머지됨. Branch reflogs expire (git's default: 90 days): an older session whose
  reflog is gone shows 변경 없음 once everything is merged.
- **Landed** (`BranchState::landed`): with nothing unlanded, the farthest target that contains
  the own work: `Parent`, `LocalBase` (the local base only), `Base` (`origin/<base>`).
- **Commits to push**: `git.sync_status` (no fetch). A branch not on the remote asks to be
  published (게시 필요) only while it has unlanded work and is a root session: a child lands in
  its parent's local branch.
- **PR** (with a remote): open, conflict (`mergeable_state == "dirty"`); `pr_merged` when the PR
  is merged, git still sees unlanded commits, and nothing was added or edited since.

## Aggregation and display

Repos aggregate (`branch_badges.rs::aggregate`): counts add up, any edit / open PR / conflict
shows, a repo whose PR was merged on GitHub does not count as unlanded, and `landed` is the
nearest place among the repos with work of their own (an idle repo does not count).

The app (`ide-app` `branch_badge.rs::kind`) shows the most pressing state:
PR 충돌 / PR 열림, 커밋 필요, 푸시 필요 / 게시 필요, 커밋 보유 (unlanded), PR 머지됨,
부모에 머지됨 (`Parent`), 머지됨 (`LocalBase` / `Base`; the tooltip says which), else 변경 없음.

## Mechanics

| Piece | Where |
|---|---|
| Per repo check (`git.status`, `git.sync_status`, `pr.status`, targets and [`landing`]) | `ide-server` `service/branch_badges.rs::branch_repo` (through `handle_blocking`, the same requests the app's footer uses) |
| Repos aggregated | `branch_badges.rs::aggregate` -> `ide_proto::BranchState` |
| Last result per session, kept across restarts | `<data>/branch-badges.json` (`BranchBadge::Known` / `Failed`); badges of deleted sessions are dropped on the next full check |
| Sent to clients | `SessionInfo::branch` (`None` = not checked yet) and `Notify::Branch { session, badge }` when a check changes it |
| When | Only while a client is connected: a full check right after a client connects and every 30 s (`FULL_EVERY`); a session whose files or git state changed (`FsChanged` / `GitChanged`) sooner (at most every 5 s), **with its children** (their work may have landed in it); new sessions as soon as they have repos. Sessions still creating worktrees are skipped. |
| Remote base | Never fetched by the badges: a background fetch raced the user's own fetches (기준 브랜치 최신화 failed on the `origin/<base>` ref lock). `origin/<base>` is as fresh as the last fetch (최신화, pull, the branch picker); a PR merged on GitHub shows as PR 머지됨 until then. |
| Off switch | `ServerConfig::branch_badges` / `IDE_BRANCH_BADGES=0` (tests that script every GitHub answer in order, e.g. `pr_e2e`) |
| Tests | `branch_badges.rs` tests: aggregation, own work from the reflog, and `landing` on a real repo (a merge with a resolved conflict, a fast-forward, a squash, a child that only synced) |

[`landing`]: ../crates/ide-server/src/service/branch_badges.rs
