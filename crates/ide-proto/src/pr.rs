//! Pull requests and agent prompts of the Changes footer (Hermes Desk `main/github.ts`,
//! `shared/merge.ts`, renderer `lib/prStatus.ts`, `lib/prRecheck.ts`, `lib/pullRequestFlow.ts`,
//! `lib/taskPrompts.ts`). Wire types of the `pr.*`, `prompt.*` and `session.send_prompt` RPCs,
//! plus the pure decision helpers Desk runs in its renderer ([`merge_blocker`],
//! [`merge_after_create_step`], [`drift_change`], [`choose_pr_target`],
//! [`effective_exclusions`]) so the app does not re-implement them. See
//! `docs/changes-actions.md`, "Pull requests and agent prompts".

use serde::{Deserialize, Serialize};

use crate::error::ProtoError;

/// State of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    /// Open.
    Open,
    /// Closed without merging.
    Closed,
    /// Merged.
    Merged,
}

/// A pull request as GitHub reports it (Desk `PrStatus`): the checked-out branch's open PR,
/// else its latest one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrStatus {
    /// PR number.
    pub number: u64,
    /// Web page.
    pub url: String,
    /// Title.
    pub title: String,
    /// Open / closed / merged.
    pub state: PrState,
    /// Draft PR.
    pub draft: bool,
    /// Head branch name.
    pub head_ref: String,
    /// Head commit (full sha).
    pub head_sha: String,
    /// Base (target) branch name.
    pub base_ref: String,
    /// `None` while GitHub is still computing it right after a push.
    pub mergeable: Option<bool>,
    /// GitHub's `mergeable_state`: clean, behind, blocked, dirty, unstable, draft, unknown…
    pub mergeable_state: String,
    /// At least one approval and no change request (latest review per reviewer).
    pub approved: bool,
    /// A reviewer's latest review requests changes.
    pub changes_requested: bool,
}

/// State of one status check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrCheckState {
    /// Queued / running / expected.
    Pending,
    /// Succeeded.
    Passed,
    /// Failed, cancelled, timed out, action required…
    Failed,
    /// Neutral / skipped.
    Skipped,
}

/// One status check on a PR's head commit (check run or commit status).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCheck {
    /// Check name (check run name or status context).
    pub name: String,
    /// State.
    pub state: PrCheckState,
    /// Branch protection requires it before merging.
    pub required: bool,
}

/// What stands between a PR and merging (Desk `PrGate`, GraphQL): review decision
/// (`REVIEW_REQUIRED` / `CHANGES_REQUESTED` / `APPROVED`, `None` = no review rule), the head
/// commit's checks and GitHub's merge state (`CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrGate {
    /// Head commit.
    pub head_sha: String,
    /// GitHub's review decision.
    pub review_decision: Option<String>,
    /// GitHub's `mergeStateStatus` (`UNKNOWN` when absent).
    pub merge_state: String,
    /// Checks of the head commit.
    pub checks: Vec<PrCheck>,
}

/// How `pr.merge` merges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrMergeMethod {
    /// Create a merge commit.
    Merge,
    /// Squash and merge (Desk's default in the merge dialog).
    #[default]
    Squash,
    /// Rebase and merge.
    Rebase,
}

impl PrMergeMethod {
    /// GitHub's `merge_method` value.
    pub fn as_str(self) -> &'static str {
        match self {
            PrMergeMethod::Merge => "merge",
            PrMergeMethod::Squash => "squash",
            PrMergeMethod::Rebase => "rebase",
        }
    }
}

/// `pr.merge` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrMerged {
    /// The merge commit GitHub made.
    pub sha: String,
    /// The remote head branch was deleted (asked and allowed).
    pub branch_deleted: bool,
    /// GitHub's message (untranslated, may be empty).
    pub message: String,
}

/// `pr.close` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrClosed {
    /// PR number.
    pub number: u64,
    /// Web page.
    pub url: String,
}

/// Outcome kind of `pr.create` / `pr.find_open` (Desk `PullRequestResult.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrResultKind {
    /// The PR was created now.
    Created,
    /// An open PR of the branch already exists (`url`, `number`).
    Exists,
    /// No API available (GitLab, `gh` missing or not logged in): `url` is the prefilled
    /// "new PR" page, `reason` says why.
    Page,
    /// The API refused (`reason`); `url` is that page as an alternative.
    Failed,
}

/// Result of `pr.create` / `pr.find_open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestResult {
    /// What happened.
    pub kind: PrResultKind,
    /// PR page or the prefilled "new PR" page.
    pub url: String,
    /// PR title.
    pub title: String,
    /// PR number (created / exists).
    pub number: Option<u64>,
    /// Why there is no PR (`Page` / `Failed`): a translatable error (`pr-gitlab-page`,
    /// `gh-missing`, `gh-auth`, `pr-api`).
    pub reason: Option<ProtoError>,
    /// An existing PR's target branch on GitHub.
    pub base: Option<String>,
    /// GitHub's mergeability against that target (`pr.find_open`); only `dirty` confirms a
    /// conflict.
    pub mergeable_state: Option<String>,
    /// That target differs from the session's base branch: the session's base (a warning).
    pub session_base: Option<String>,
}

/// A commit in PR lists (short sha + subject).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCommit {
    /// Abbreviated sha.
    pub sha: String,
    /// Subject line.
    pub subject: String,
}

/// An uncommitted path with its two-letter `git status --porcelain` code (trimmed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrWorkFile {
    /// Repo-relative path, `/`-separated (renames: the new path).
    pub path: String,
    /// Porcelain status, trimmed (`M`, `A`, `??`, `R`, `MM`…).
    pub status: String,
}

/// `pr.commit_preview`: PR text built from committed history (Desk `CommitPrPreview`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPrPreview {
    /// Default title (newest subject, `"… 외 N개"` for several commits).
    pub title: String,
    /// Body (commit list + `git diff --stat`), Desk's Korean template.
    pub body: String,
    /// Commits the PR carries, oldest first.
    pub commits: Vec<PrCommit>,
    /// HEAD (full sha).
    pub head: String,
    /// The PR source: HEAD (push) or the fetched `origin/<branch>` (no push).
    pub source_head: String,
    /// The fetched `origin/<base>`.
    pub base_head: String,
    /// Checked-out branch.
    pub branch: String,
}

/// Pins of a commit-based PR (Desk `CommitPrTextOptions`): every `Some` field must still match
/// when the text is written and when the PR is created, else `pr-changed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPrTextOptions {
    /// The selected origin target branch name, literally.
    #[serde(default)]
    pub expected_base: Option<String>,
    /// Push the local commits (`None` = true).
    #[serde(default)]
    pub push: Option<bool>,
    /// The edited title (one line); `None` = the preview's title.
    #[serde(default)]
    pub title: Option<String>,
    /// HEAD at preview time.
    #[serde(default)]
    pub expected_head: Option<String>,
    /// Source head at preview time.
    #[serde(default)]
    pub expected_source_head: Option<String>,
    /// `origin/<base>` at preview time.
    #[serde(default)]
    pub expected_base_head: Option<String>,
    /// Branch at preview time.
    #[serde(default)]
    pub expected_branch: Option<String>,
}

/// A commit since a PR request (`pr.drift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftCommit {
    /// Abbreviated sha.
    pub sha: String,
    /// Subject.
    pub subject: String,
    /// Full sha.
    pub full: String,
    /// A plain (non-merge) commit of only `allowed` paths: the asked-for commit.
    pub expected: bool,
}

/// How a worktree moved since a PR request whose HEAD was `since` (Desk `PrDrift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDrift {
    /// HEAD now (full sha).
    pub head: String,
    /// The request's HEAD is no longer an ancestor of HEAD (rebased / reset).
    pub rewritten: bool,
    /// Commits since the request's HEAD, oldest first (first parent, at most 50).
    pub commits: Vec<DriftCommit>,
    /// Uncommitted paths now.
    pub files: Vec<PrWorkFile>,
}

/// What a PR request would take along beyond origin (Desk `PendingWork`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingWork {
    /// Checked-out branch (`HEAD` when detached).
    pub branch: String,
    /// HEAD's commits no remote has yet, newest first (at most 30 listed).
    pub commits: Vec<PrCommit>,
    /// How many there are in all.
    pub commit_count: u32,
    /// Uncommitted paths.
    pub files: Vec<PrWorkFile>,
    /// origin has this branch: a PR can be made from what is there without pushing.
    pub published: bool,
}

/// How a left-over PR text relates to HEAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeftoverMatch {
    /// HEAD is the commit the text was used for.
    Same,
    /// Commits were added since (`new_commits`).
    Ahead,
    /// That commit is no longer on the branch (rebase / reset).
    Diverged,
    /// The text was never used for an attempt.
    Unknown,
}

/// The agent's PR text left behind: written, never turned into a PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrLeftoverFile {
    /// Server path of the PR file.
    pub path: String,
    /// Title (first line, leading `#` removed; empty if the file has none).
    pub title: String,
    /// Body.
    pub body: String,
    /// File modification time, unix seconds.
    pub written_at: u64,
    /// The commit the app was creating the PR for when it used this text.
    pub used_for: Option<String>,
    /// Why that attempt failed, when recorded.
    pub reason: Option<ProtoError>,
    /// Relation to HEAD.
    pub matches: LeftoverMatch,
    /// Commits since `used_for` (with [`LeftoverMatch::Ahead`]).
    pub new_commits: Vec<PrCommit>,
}

/// `pr.leftover` (Desk `PrLeftover`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrLeftover {
    /// HEAD (full sha).
    pub head: String,
    /// The PR text left behind.
    pub file: Option<PrLeftoverFile>,
    /// Whether HEAD differs from `origin/<base>` in content (`None`: no base asked / not
    /// fetched).
    pub changes: Option<bool>,
    /// Commits the PR would carry (only with changes; at most 30).
    pub commits: Vec<PrCommit>,
}

/// `pr.unmerged` (Desk `UnmergedWork`, the session's "커밋 보유").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmergedWork {
    /// The branch's own commits the base lacks (by patch: squashed ones drop).
    pub commits: u32,
    /// Merging conflicts: whether it would still add anything could not be told.
    pub conflict: bool,
}

/// `pr.remote_branches`: origin's branches (`git ls-remote`), its default branch and the
/// checked-out one (Desk `remoteBranches`, the PR dialog's target list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrTargets {
    /// Branch names on origin, sorted.
    pub branches: Vec<String>,
    /// origin's HEAD branch.
    pub default_branch: Option<String>,
    /// Checked-out branch (`HEAD` when detached).
    pub current: String,
}

/// What the user left out of a PR request (Desk `PrExclusions`; the branch is filled in by the
/// server from the worktree).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrExclusions {
    /// Unpushed commits: the app does not push; the PR is the branch as on origin.
    #[serde(default)]
    pub commits: bool,
    /// Uncommitted files: the agent does not commit them.
    #[serde(default)]
    pub files: bool,
}

// ---- Desk's renderer decisions, as pure functions -------------------------------------------

/// Why the app would not merge a PR now (Desk `mergeBlocker`); `None` = it can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeBlocker {
    /// 이미 머지됐어요.
    AlreadyMerged,
    /// PR이 닫혀 있어요.
    Closed,
    /// 초안 PR이에요.
    Draft,
    /// 변경 요청이 있어요.
    ChangesRequested,
    /// GitHub 브랜치 보호 규칙(필수 검사·리뷰 등)을 아직 통과하지 못했어요.
    Blocked,
    /// 기준 브랜치와 충돌해요.
    Conflict,
    /// 기준 브랜치가 앞서 있어요 (브랜치 업데이트 필요).
    Behind,
    /// GitHub가 머지할 수 없다고 해요.
    NotMergeable,
}

/// Desk `mergeBlocker` (the "머지하기" item's hint).
pub fn merge_blocker(pr: &PrStatus) -> Option<MergeBlocker> {
    Some(match pr.state {
        PrState::Merged => MergeBlocker::AlreadyMerged,
        PrState::Closed => MergeBlocker::Closed,
        PrState::Open if pr.draft => MergeBlocker::Draft,
        PrState::Open if pr.changes_requested => MergeBlocker::ChangesRequested,
        PrState::Open => match pr.mergeable_state.as_str() {
            "blocked" => MergeBlocker::Blocked,
            "dirty" => MergeBlocker::Conflict,
            "behind" => MergeBlocker::Behind,
            _ if pr.mergeable == Some(false) => MergeBlocker::NotMergeable,
            _ => return None,
        },
    })
}

/// Why "PR 올린 뒤 바로 머지" stops (Desk `mergeAfterCreateStep` stop reasons).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeStop {
    /// PR 대상 브랜치가 바뀌었어요 (`from` → `to`). 머지하지 않았어요.
    TargetChanged {
        /// The PR's base now.
        from: String,
        /// The requested target.
        to: String,
    },
    /// 이미 머지됐어요.
    AlreadyMerged,
    /// 초안 PR은 머지하지 않아요.
    Draft,
    /// 변경 요청이 있어요.
    ChangesRequested,
    /// 리뷰 승인이 필요해요.
    ReviewRequired,
    /// 필수 검사가 실패했어요: names.
    ChecksFailed(Vec<String>),
    /// 브랜치 보호 규칙(리뷰 승인이나 필수 검사)을 통과해야 머지할 수 있어요.
    Protected,
    /// Another [`merge_blocker`] reason.
    Blocker(MergeBlocker),
}

/// One step of the merge-after-create wait (Desk `MergeStep`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeStep {
    /// Wait (poll again in [`MERGE_POLL_SECS`]); `checks` = (done, total, pending names).
    Wait {
        /// Progress of the watched checks, when known.
        checks: Option<(u32, u32, Vec<String>)>,
    },
    /// Stop without merging.
    Stop(MergeStop),
    /// Merge at this head sha.
    Merge {
        /// Head the merge is pinned to.
        sha: String,
    },
}

/// Desk: required checks may take a moment to register; a `blocked` PR with nothing pending is
/// waited out this long (seconds) before giving up.
pub const UNEXPLAINED_BLOCK_SECS: u64 = 180;
/// Desk: poll interval while waiting for GitHub / checks (seconds).
pub const MERGE_POLL_SECS: u64 = 10;
/// Desk: the longest wait for checks (seconds).
pub const MERGE_MAX_WAIT_SECS: u64 = 2 * 60 * 60;

/// Desk `mergeAfterCreateStep`: what to do with a freshly created PR given GitHub's latest view
/// (and its gate when available). `blocked_for_secs`: how long it has been `blocked` with a gate
/// but nothing to wait for.
pub fn merge_after_create_step(
    pr: Option<&PrStatus>,
    expected_number: u64,
    gate: Option<&PrGate>,
    blocked_for_secs: u64,
    target: Option<&str>,
) -> MergeStep {
    let Some(pr) = pr.filter(|p| p.number == expected_number) else {
        return MergeStep::Wait { checks: None };
    };
    if let Some(t) = target
        && pr.base_ref != t
    {
        return MergeStep::Stop(MergeStop::TargetChanged {
            from: pr.base_ref.clone(),
            to: t.to_owned(),
        });
    }
    if pr.state == PrState::Merged {
        return MergeStep::Stop(MergeStop::AlreadyMerged);
    }
    if pr.draft {
        return MergeStep::Stop(MergeStop::Draft);
    }
    match gate.and_then(|g| g.review_decision.as_deref()) {
        Some("CHANGES_REQUESTED") => return MergeStep::Stop(MergeStop::ChangesRequested),
        Some("REVIEW_REQUIRED") => return MergeStep::Stop(MergeStop::ReviewRequired),
        _ => {}
    }
    if let Some(g) = gate {
        // A check counts when branch protection requires it; with none marked, every check is
        // waited for (a failure of a non-required one does not stop it).
        let required: Vec<&PrCheck> = g.checks.iter().filter(|c| c.required).collect();
        let watched: Vec<&PrCheck> = if required.is_empty() {
            g.checks.iter().collect()
        } else {
            required.clone()
        };
        let failed: Vec<String> = required
            .iter()
            .filter(|c| c.state == PrCheckState::Failed)
            .map(|c| c.name.clone())
            .collect();
        if !failed.is_empty() {
            return MergeStep::Stop(MergeStop::ChecksFailed(failed));
        }
        let pending: Vec<String> = watched
            .iter()
            .filter(|c| c.state == PrCheckState::Pending)
            .map(|c| c.name.clone())
            .collect();
        if !pending.is_empty() {
            let total = watched.len() as u32;
            return MergeStep::Wait {
                checks: Some((total - pending.len() as u32, total, pending)),
            };
        }
    }
    if pr.mergeable.is_none() || pr.mergeable_state == "unknown" {
        return MergeStep::Wait { checks: None };
    }
    if pr.mergeable_state == "blocked" {
        if gate.is_some() && blocked_for_secs < UNEXPLAINED_BLOCK_SECS {
            return MergeStep::Wait { checks: None };
        }
        return MergeStep::Stop(MergeStop::Protected);
    }
    let approved = PrStatus {
        approved: true,
        ..pr.clone()
    };
    if let Some(b) = merge_blocker(&approved) {
        return MergeStep::Stop(MergeStop::Blocker(b));
    }
    MergeStep::Merge {
        sha: pr.head_sha.clone(),
    }
}

/// What changed since a PR request (Desk `PrRecheckChange`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRecheckChange {
    /// The request's HEAD is gone from the branch: nothing can be left out.
    pub rewritten: bool,
    /// From the first commit nobody asked for on (oldest first).
    pub commits: Vec<PrCommit>,
    /// The last commit before them (the request's HEAD or the asked-for commit): pushed instead
    /// when the user leaves them out.
    pub keep_up_to: String,
    /// Uncommitted files that were not there (or were to be committed) at request time.
    pub files: Vec<PrWorkFile>,
}

/// Desk `driftChange`: `None` = nothing new, go ahead. `before_files`: uncommitted paths at
/// request time; `files_included`: they were to be committed for the PR (any left is news).
pub fn drift_change(
    drift: &PrDrift,
    before_head: &str,
    before_files: &[String],
    files_included: bool,
) -> Option<PrRecheckChange> {
    let files: Vec<PrWorkFile> = drift
        .files
        .iter()
        .filter(|f| files_included || !before_files.contains(&f.path))
        .cloned()
        .collect();
    let first = drift.commits.iter().position(|c| !c.expected);
    let commits: Vec<PrCommit> = first.map_or_else(Vec::new, |i| {
        drift.commits[i..]
            .iter()
            .map(|c| PrCommit {
                sha: c.sha.clone(),
                subject: c.subject.clone(),
            })
            .collect()
    });
    // Desk: `first <= 0` (none found counts as -1) keeps the request's HEAD.
    let keep_up_to = match first {
        Some(i) if i > 0 => drift.commits[i - 1].full.clone(),
        _ => before_head.to_owned(),
    };
    if !drift.rewritten && commits.is_empty() && files.is_empty() {
        return None;
    }
    Some(PrRecheckChange {
        rewritten: drift.rewritten,
        commits,
        keep_up_to,
        files,
    })
}

/// Desk `choosePrTarget`: the repo's preferred base, then the worktree's base, then origin's
/// default, else the first branch (never the current one). Refs may be full
/// (`refs/heads/x`, `refs/remotes/origin/x`) or short.
pub fn choose_pr_target(
    branches: &[String],
    current: &str,
    preferred_base: Option<&str>,
    worktree_base: Option<&str>,
    default_branch: Option<&str>,
) -> Option<String> {
    let available: Vec<&String> = branches.iter().filter(|b| *b != current).collect();
    let name = |r: &str| -> String {
        if let Some(n) = r.strip_prefix("refs/heads/") {
            return n.to_owned();
        }
        if let Some(n) = r.strip_prefix("refs/remotes/") {
            return n.split_once('/').map_or(n, |(_, b)| b).to_owned();
        }
        if branches.iter().any(|b| b == r) {
            return r.to_owned();
        }
        r.split_once('/').map_or(r, |(_, b)| b).to_owned()
    };
    for r in [preferred_base, worktree_base].into_iter().flatten() {
        let n = name(r);
        if available.iter().any(|b| **b == n) {
            return Some(n);
        }
    }
    if let Some(d) = default_branch
        && available.iter().any(|b| *b == d)
    {
        return Some(d.to_owned());
    }
    available.first().map(|b| (*b).clone())
}

/// Desk `effectiveExclusions`: what a "PR 올리기 전에 확인" choice really leaves out (`None` =
/// nothing). Leaving commits out needs a published branch and also leaves the files out.
pub fn effective_exclusions(
    pending: Option<&PendingWork>,
    exclude: PrExclusions,
) -> Option<PrExclusions> {
    let p = pending?;
    let commits = exclude.commits && p.commit_count > 0 && p.published;
    let files = (commits || exclude.files) && !p.files.is_empty();
    (commits || files).then_some(PrExclusions { commits, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PrStatus {
        PrStatus {
            number: 7,
            url: "u".into(),
            title: "t".into(),
            state: PrState::Open,
            draft: false,
            head_ref: "feature".into(),
            head_sha: "a".repeat(40),
            base_ref: "main".into(),
            mergeable: Some(true),
            mergeable_state: "clean".into(),
            approved: false,
            changes_requested: false,
        }
    }

    #[test]
    fn blockers_follow_desk_order() {
        assert_eq!(merge_blocker(&pr()), None);
        let mut p = pr();
        p.mergeable_state = "behind".into();
        assert_eq!(merge_blocker(&p), Some(MergeBlocker::Behind));
        p.draft = true;
        assert_eq!(merge_blocker(&p), Some(MergeBlocker::Draft));
        p.state = PrState::Merged;
        assert_eq!(merge_blocker(&p), Some(MergeBlocker::AlreadyMerged));
        let mut p = pr();
        p.mergeable = Some(false);
        p.mergeable_state = "unstable".into();
        assert_eq!(merge_blocker(&p), Some(MergeBlocker::NotMergeable));
    }

    #[test]
    fn merge_steps() {
        let p = pr();
        assert_eq!(
            merge_after_create_step(None, 7, None, 0, None),
            MergeStep::Wait { checks: None }
        );
        assert_eq!(
            merge_after_create_step(Some(&p), 7, None, 0, Some("main")),
            MergeStep::Merge {
                sha: p.head_sha.clone()
            }
        );
        assert!(matches!(
            merge_after_create_step(Some(&p), 7, None, 0, Some("dev")),
            MergeStep::Stop(MergeStop::TargetChanged { .. })
        ));
        let check = |name: &str, state, required| PrCheck {
            name: name.into(),
            state,
            required,
        };
        let gate = PrGate {
            head_sha: p.head_sha.clone(),
            review_decision: None,
            merge_state: "BLOCKED".into(),
            checks: vec![
                check("ci", PrCheckState::Pending, true),
                check("lint", PrCheckState::Passed, true),
                check("extra", PrCheckState::Failed, false),
            ],
        };
        assert_eq!(
            merge_after_create_step(Some(&p), 7, Some(&gate), 0, None),
            MergeStep::Wait {
                checks: Some((1, 2, vec!["ci".into()]))
            }
        );
        let mut g2 = gate.clone();
        g2.checks[0].state = PrCheckState::Failed;
        assert_eq!(
            merge_after_create_step(Some(&p), 7, Some(&g2), 0, None),
            MergeStep::Stop(MergeStop::ChecksFailed(vec!["ci".into()]))
        );
        g2.review_decision = Some("REVIEW_REQUIRED".into());
        assert_eq!(
            merge_after_create_step(Some(&p), 7, Some(&g2), 0, None),
            MergeStep::Stop(MergeStop::ReviewRequired)
        );
        let mut blocked = p.clone();
        blocked.mergeable_state = "blocked".into();
        let mut g3 = gate.clone();
        g3.checks.clear();
        assert_eq!(
            merge_after_create_step(Some(&blocked), 7, Some(&g3), 10, None),
            MergeStep::Wait { checks: None }
        );
        assert_eq!(
            merge_after_create_step(Some(&blocked), 7, Some(&g3), UNEXPLAINED_BLOCK_SECS, None),
            MergeStep::Stop(MergeStop::Protected)
        );
        assert_eq!(
            merge_after_create_step(Some(&blocked), 7, None, 0, None),
            MergeStep::Stop(MergeStop::Protected)
        );
    }

    #[test]
    fn drift_and_targets() {
        let c = |s: &str, expected| DriftCommit {
            sha: s[..7].into(),
            subject: s.into(),
            full: s.into(),
            expected,
        };
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let file = |p: &str| PrWorkFile {
            path: p.into(),
            status: "M".into(),
        };
        let drift = PrDrift {
            head: b.clone(),
            rewritten: false,
            commits: vec![c(&a, true), c(&b, false)],
            files: vec![file("old.txt"), file("new.txt")],
        };
        let ch = drift_change(&drift, "h", &["old.txt".into()], false).unwrap();
        assert_eq!(ch.keep_up_to, a);
        assert_eq!(ch.commits.len(), 1);
        assert_eq!(ch.files, vec![file("new.txt")]);
        let quiet = PrDrift {
            head: a.clone(),
            rewritten: false,
            commits: vec![c(&a, true)],
            files: vec![],
        };
        assert_eq!(drift_change(&quiet, "h", &[], true), None);

        let br: Vec<String> = ["dev", "main", "release/1", "feature"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            choose_pr_target(
                &br,
                "feature",
                Some("refs/remotes/origin/release/1"),
                None,
                None
            ),
            Some("release/1".into())
        );
        assert_eq!(
            choose_pr_target(
                &br,
                "feature",
                Some("origin/gone"),
                Some("dev"),
                Some("main")
            ),
            Some("dev".into())
        );
        assert_eq!(
            choose_pr_target(&br, "feature", None, None, Some("main")),
            Some("main".into())
        );
        assert_eq!(
            choose_pr_target(&br, "dev", None, None, None),
            Some("main".into())
        );

        let pending = PendingWork {
            branch: "feature".into(),
            commits: vec![],
            commit_count: 2,
            files: vec![file("x")],
            published: false,
        };
        let all = PrExclusions {
            commits: true,
            files: false,
        };
        // Unpublished: commits can't be left out.
        assert_eq!(effective_exclusions(Some(&pending), all), None);
        let published = PendingWork {
            published: true,
            ..pending.clone()
        };
        assert_eq!(
            effective_exclusions(Some(&published), all),
            Some(PrExclusions {
                commits: true,
                files: true
            })
        );
    }
}
