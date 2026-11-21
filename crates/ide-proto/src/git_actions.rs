//! Wire types of the local git actions of the Changes / History tabs (port of Hermes Desk's
//! discard, 임시 보관 (stash), commit context menu, sync button, branch picker and
//! 새 브랜치로 이어가기). See `docs/changes-actions.md`.

use serde::{Deserialize, Serialize};

use crate::GitChange;

/// Message tag of the stashes the app makes when switching / creating a branch with
/// [`SwitchMode::Stash`] or continuing on a new branch (Hermes Desk's `STASH_TAG`, kept so
/// stashes made by Desk are recognised): the message is `hermes-desk:<branch left>`.
pub const STASH_TAG: &str = "hermes-desk:";

/// Uncommitted changes when switching to / creating a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchMode {
    /// Bring them along (git refuses on a conflict).
    Carry,
    /// Stash them first, tagged with the branch they were left on ([`STASH_TAG`]).
    Stash,
}

/// Who has a local branch checked out (git allows only one worktree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchCheckout {
    /// Nobody.
    None,
    /// This session's worktree (the current branch).
    Here,
    /// Only the registered repository's own folder (absolute server path, display only): a
    /// worktree may still switch to it (`git switch --ignore-other-worktrees`).
    MainRepo(String),
    /// Another linked worktree (a session's; absolute server path, display only): switching
    /// to it is refused.
    Worktree(String),
}

/// A local branch of `git.branches`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBranch {
    /// Short name (`feature/x`).
    pub name: String,
    /// Where it is checked out.
    pub checkout: BranchCheckout,
}

/// A remote-tracking branch of `git.branches` (each remote's `HEAD` left out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteBranch {
    /// Remote name (`origin`; the longest configured remote that prefixes the ref).
    pub remote: String,
    /// Branch name on the remote (the local branch it would become).
    pub branch: String,
    /// Short ref (`origin/feature-x`), also a valid `base` of `git.create_branch`.
    pub short_ref: String,
    /// A local branch of the same name exists (Desk's picker hides these; its new-branch base
    /// list shows them).
    pub has_local: bool,
}

/// `git.branches`: Hermes Desk's `branchList` + `remoteTrackingBranches` + `branchBases`, from
/// local refs only (no network).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranches {
    /// The checked-out branch (`None` when detached).
    pub current: Option<String>,
    /// Local branches (`refs/heads`, git's order).
    pub local: Vec<LocalBranch>,
    /// Remote-tracking branches (`refs/remotes`, git's order).
    pub remote: Vec<RemoteBranch>,
}

/// `git.fetch_remotes`: outcome of the picker's background `git fetch --prune`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFetchStatus {
    /// Every remote fetched (or none configured).
    pub ok: bool,
    /// `false` when skipped because the repository was fetched recently (the lists are current).
    pub fetched: bool,
    /// `remote: git message` lines of the failed remotes (git's own text, not translated).
    pub error: Option<String>,
}

/// History tab commit context menu actions (`git.commit_action`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommitAction {
    /// `git switch --detach <sha>`.
    Checkout,
    /// `git switch --no-track -c <name> <sha>`.
    Branch {
        /// New branch name.
        name: String,
    },
    /// `git revert --no-edit <sha>` (not for merge commits).
    Revert,
    /// `git cherry-pick <sha>` (not for merge commits).
    CherryPick,
}

/// `git.sync_status`: the sync (push / pull) button's numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    /// Commits to push. For an unpublished branch: its commits no remote has, minus those its
    /// merged PR already delivered (`merged_head` of the request).
    pub ahead: u32,
    /// Commits to pull.
    pub behind: u32,
    /// A local-only repository (no remote): nothing to push, pull or open a PR on.
    pub no_remote: bool,
    /// The branch's upstream (`origin/x`) when it has a live one; `None` = not published.
    pub upstream: Option<String>,
    /// The remote has no branch for it: "Publish branch", not "push N".
    pub unpublished: bool,
    /// It had an upstream that is gone (deleted on the remote, e.g. after its PR was merged).
    pub gone: bool,
}

/// How `git.pull` brings the upstream in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PullMode {
    /// `git pull --no-rebase --no-edit --autostash`.
    Merge,
    /// `git pull --rebase --no-edit --autostash`.
    Rebase,
    /// Fetch the upstream and `git reset --hard` to it (drops local commits and edits).
    Reset,
}

/// `git.push` of a verified commit (Desk's PR flow): push exactly `head_sha` to `branch` and
/// only if HEAD is still that commit on that branch and the tree is clean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushExpected {
    /// Full commit id (40 or 64 hex digits).
    pub head_sha: String,
    /// The branch that was checked out.
    pub branch: String,
}

/// One stash of `git.stash_list` (the list belongs to the repository: every worktree, so every
/// session, sees the same entries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashEntry {
    /// Stash commit id: the stable id for `git.stash_action` (never `stash@{n}`, which shifts).
    pub sha: String,
    /// The user's message; empty for git's default WIP message and for app-made stashes.
    pub message: String,
    /// Branch the stash was made on (empty when detached).
    pub branch: String,
    /// Made by the app when switching / creating a branch (message tagged with [`STASH_TAG`]).
    pub auto: bool,
    /// Creation time, unix seconds.
    pub date: i64,
    /// Files in the stash: `Modified`, `Added` (also a copy, with `orig_path`), `Deleted`,
    /// `Renamed` (with `orig_path`) or `Untracked`.
    pub files: Vec<GitChange>,
}

/// `git.stash_action` actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StashAction {
    /// Apply and drop.
    Pop,
    /// Apply, keep the stash.
    Apply,
    /// Delete without applying.
    Drop,
}

/// `git.stash_action` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashActionResult {
    /// Paths left conflicted by pop/apply. When non-empty git kept the stash (even for pop):
    /// nothing is lost, the user resolves the files in Changes.
    pub conflicts: Vec<String>,
}

/// One commit of the branch that `origin/<base>` does not have (새 브랜치로 이어가기).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchCommit {
    /// Full id (what the options name).
    pub full: String,
    /// Abbreviated id.
    pub sha: String,
    /// Subject line.
    pub subject: String,
    /// `origin/<base>` already has its content (and that of every commit before it), e.g. it
    /// was squash-merged: left unchecked by default.
    pub in_base: bool,
}

/// An uncommitted file (`git status --porcelain`, untracked included).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusEntry {
    /// Path (the new path of a rename).
    pub path: String,
    /// Porcelain XY letters, trimmed (`M`, `MM`, `A`, `R`, `??`, ...).
    pub status: String,
}

/// `git.branch_refresh_info`: what the 새 브랜치로 이어가기 dialog shows before anything is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRefreshInfo {
    /// The checked-out branch (empty when detached).
    pub branch: String,
    /// The session's base branch for this repo (`main`); the new branch starts at
    /// `origin/<base>`.
    pub base: String,
    /// Free name used when the name is left empty (`<branch without -N>-2`, `-3`, ...).
    pub suggested: String,
    /// `git log origin/<base>..HEAD`, oldest first, merges left out (the newest 100).
    pub commits: Vec<BranchCommit>,
    /// Commits beyond `commits` (too many to list): they stay on the old branch.
    pub more_commits: u32,
    /// Uncommitted changes.
    pub files: Vec<StatusEntry>,
    /// `origin/<base>` could not be fetched (`fetch: true`): the local copy is used.
    pub fetch_failed: bool,
}

/// A picked commit that does not apply on `origin/<base>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshConflict {
    /// Full id.
    pub sha: String,
    /// Subject line.
    pub subject: String,
    /// Conflicting paths.
    pub files: Vec<String>,
}

/// `git.branch_refresh_check`: dry run of the picked commits on `origin/<base>` (nothing
/// written but git objects).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRefreshCheck {
    /// The first picked commit that does not apply: the run would be refused.
    pub conflict: Option<RefreshConflict>,
    /// Picked commits that change nothing on top of the others (already in the base).
    pub skipped: Vec<String>,
    /// Uncommitted changes that would conflict on the new branch (they would stay in a stash).
    pub files_conflict: Vec<String>,
}

/// `git.refresh_branch` options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRefreshOptions {
    /// Branch to create; `None` / blank uses the suggested name.
    pub name: Option<String>,
    /// Full ids of the listed commits to take along (applied oldest first, as listed).
    pub commits: Vec<String>,
    /// Bring the uncommitted changes along (else they stay stashed, tagged with the old branch).
    pub move_files: bool,
}

/// `git.refresh_branch` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchRefreshResult {
    /// The branch now checked out (also the session worktree's recorded branch).
    pub branch: String,
    /// The branch left behind (kept locally).
    pub previous: String,
    /// The old branch's HEAD before the switch.
    pub previous_head: String,
    /// Commits re-made on the new branch.
    pub moved_commits: u32,
    /// Picked commits that changed nothing (already in the base).
    pub skipped_commits: u32,
    /// Listed commits not picked: they stay on `previous` only.
    pub left_commits: u32,
    /// Uncommitted changes moved along.
    pub moved_files: u32,
    /// Uncommitted changes left in a stash on `previous`.
    pub stashed_files: u32,
    /// `move_files` was asked but re-applying the changes conflicted: they stay in the stash
    /// on `previous` (`stashed_files` counts them).
    pub files_left_in_stash: bool,
}
