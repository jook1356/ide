# Session list branch badge

The badge on each session row (변경 없음, 커밋 필요, 푸시 필요 / 게시 필요, PR 열림 / PR 충돌,
커밋 보유, 머지됨; 확인 중 before the first check, 조회 실패 when it failed) is Hermes Desk's
`SessionBranchBadge`. It used to be computed by the app every 30 s, so every app start (and
every server restart) began with 확인 중 everywhere. It is now computed and kept by the server.

| Piece | Where |
|---|---|
| Per repo check: `git.status`, `git.sync_status` (no fetch), `pr.status`, `pr.unmerged` (PR calls skipped without a remote; a failing PR call leaves that part unknown) | `ide-server` `service/branch_badges.rs::branch_repo` (through `handle_blocking`, the same requests the app's footer uses) |
| Repos aggregated (Desk's `aggregateBranches`) | `branch_badges.rs::aggregate` -> `ide_proto::BranchState` |
| Last result per session, kept across restarts | `<data>/branch-badges.json` (`BranchBadge::Known` / `Failed`); badges of deleted sessions are dropped on the next full check |
| Sent to clients | `SessionInfo::branch` (`None` = not checked yet) and `Notify::Branch { session, badge }` when a check changes it |
| When | Only while a client is connected: a full check right after a client connects and every 30 s (`FULL_EVERY`); a session whose files or git state changed (`FsChanged` / `GitChanged`) sooner (at most every 5 s); new sessions as soon as they have repos. Sessions still creating worktrees are skipped. |
| Off switch | `ServerConfig::branch_badges` / `IDE_BRANCH_BADGES=0` (tests that script every GitHub answer in order, e.g. `pr_e2e`) |
| Shown | `ide-app` `branch_badge.rs::branch_badge` (labels, colours, tooltips; `kind` picks the most pressing state) |
