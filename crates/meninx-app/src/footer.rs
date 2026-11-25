//! The Changes / History footer (Hermes Desk `ChangesPanel.tsx` `AiActionBar`): 커밋 맡기기
//! (asks the session's agent to commit), the sync button (`SyncButton.tsx`: push / pull /
//! publish / fetch with ahead / behind counts and its menu), the "…" menu (PR 올리기 /
//! 머지하기 / PR 보기 / PR 닫기 / PR 확인 필요 / 새 브랜치로 이어가기), the PR strips above it
//! and the dialogs they open (`PullRequestModal.tsx`, `MergePrModal.tsx`, the close confirm,
//! `BranchRefreshPanel.tsx`, `PrRecheckModal.tsx`), plus the PR job of
//! `lib/pullRequestFlow.ts` (shown as a notice card above the footer).
//!
//! State lives per session ([`crate::detail::Detail::footer`]) and per repo ([`RepoFooter`]);
//! everything goes through the server (`git.sync_status`, `git.push`, `git.pull`, `pr.*`,
//! `prompt.*`, `session.send_prompt`, `git.branch_refresh_*`). The view is in
//! `footer_view.rs`. See `docs/changes-actions.md` for the Desk control -> call table.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iced::Task;
use meninx_client::{
    BranchRefreshCheck, BranchRefreshInfo, BranchRefreshOptions, BranchRefreshResult,
    CommitPrPreview, CommitPrTextOptions, MergeStep, MergeStop, PendingWork, PrClosed, PrDrift,
    PrExclusions, PrGate, PrLeftover, PrMergeMethod, PrMerged, PrRecheckChange, PrResultKind,
    PrState, PrStatus, PrTargets, ProtoError, PullMode, PullRequestResult, SessionMode, SyncStatus,
};

use crate::app::{App, Message, Modal};
use crate::detail::{DetailMsg, SideTab};
use crate::i18n::{self, tr};

/// Desk `FETCH_EVERY_MS`: the sync button fetches this often while shown.
const FETCH_EVERY: Duration = Duration::from_secs(5 * 60);
/// Desk `FRESH_ON_OPEN_MS`: a fetch this recent is reused when the footer appears.
const FRESH_ON_OPEN: Duration = Duration::from_secs(60);
/// How often a waiting PR job looks for the agent's PR text (Desk waits for the chat turn's
/// end; Meninx has no turn tracking).
pub const JOB_POLL: Duration = Duration::from_secs(2);

/// A sync operation (Desk `SyncButton` `Op`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOp {
    Fetch,
    Push,
    Publish,
    ForcePush,
    Pull,
    Rebase,
    Reset,
}

/// The footer's open menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterMenu {
    Sync,
    More,
}

/// Footer state of one session.
#[derive(Default)]
pub struct Footer {
    pub menu: Option<FooterMenu>,
    pub repos: HashMap<String, RepoFooter>,
}

/// Footer state of one repo of a session.
#[derive(Default)]
pub struct RepoFooter {
    /// `git.sync_status` (Desk `SyncButton`'s `sync`); `None` until it arrived or after a failure.
    pub sync: Option<SyncStatus>,
    sync_seq: u64,
    sync_requested: bool,
    /// A sync operation in flight (the button spins).
    pub running: Option<SyncOp>,
    fetched_at: Option<Instant>,
    /// The branch's PR (`pr.status`); `None` = none or unknown.
    pub pr: Option<PrStatus>,
    /// `pr.status` failed (gh missing / not logged in): unknown, not "no PR".
    pub pr_error: Option<String>,
    /// A `pr.status` answer arrived (the PR state is known).
    pub pr_loaded: bool,
    pr_seq: u64,
    /// PR text the agent wrote that never became a PR (`pr.leftover`).
    pub leftover: bool,
    /// What the PR status / leftover were last asked for: (commits, newest commit, unpushed).
    key: Option<(usize, String, usize)>,
    /// The PR request in progress or its last outcome (Desk's job notice).
    pub job: Option<PrJob>,
}

impl RepoFooter {
    pub fn no_remote(&self) -> bool {
        self.sync.as_ref().is_some_and(|s| s.no_remote)
    }

    /// An open PR turns "PR 올리기" into "머지하기".
    pub fn open_pr(&self) -> Option<&PrStatus> {
        self.pr.as_ref().filter(|p| p.state == PrState::Open)
    }

    /// The merged PR's number when it was merged and nothing is left to push (Desk `mergedPr`).
    pub fn merged_pr(&self) -> Option<u64> {
        let pr = self.pr.as_ref().filter(|p| p.state == PrState::Merged)?;
        self.sync
            .as_ref()
            .is_none_or(|s| s.ahead == 0)
            .then_some(pr.number)
    }

    /// The PR request waiting for the user's decision (Desk `usePrRecheckFor`).
    pub fn recheck(&self) -> Option<&PrRecheckChange> {
        match &self.job {
            Some(PrJob {
                stage: JobStage::Reviewing(c),
                ..
            }) => Some(c),
            _ => None,
        }
    }
}

/// The sync button's main action (Desk `primary`): publish a branch origin does not have,
/// else pull when behind, push when ahead, otherwise fetch. `merged`: its PR was merged and
/// nothing is left (publishing would only bring back the deleted branch).
pub fn primary_op(sync: Option<&SyncStatus>, merged: bool) -> SyncOp {
    let (ahead, behind, unpublished) =
        sync.map_or((0, 0, false), |s| (s.ahead, s.behind, s.unpublished));
    if unpublished && !merged {
        SyncOp::Publish
    } else if behind > 0 {
        SyncOp::Pull
    } else if ahead > 0 && !merged {
        SyncOp::Push
    } else {
        SyncOp::Fetch
    }
}

/// Desk `branchNameProblem`: an empty name is fine (the suggestion is used).
pub fn branch_name_ok(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return true;
    }
    !(n.starts_with('-')
        || n.starts_with('/')
        || n.ends_with('/')
        || n.ends_with('.')
        || n.ends_with(".lock")
        || n.contains("..")
        || n.contains("//")
        || n.contains("@{")
        || n == "@"
        || n.chars()
            .any(|c| c.is_whitespace() || "~^:?*[\\\x7f".contains(c) || (c as u32) < 0x20)
        || n.split('/').any(|p| p.starts_with('.')))
}

/// An item of the "…" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoreKind {
    Merge,
    /// The PR is behind its base: GitHub's "Update branch" (Desk's 브랜치 업데이트).
    UpdateBranch,
    View,
    Close,
    Recheck,
    CreatePr,
    Continue,
}

/// One row of the "…" menu: what it is, whether it can be clicked and its description.
#[derive(Debug, Clone)]
pub struct MoreEntry {
    pub kind: MoreKind,
    pub enabled: bool,
    pub title: String,
    pub hint: Option<String>,
}

// ---------------------------------------------------------------------------- PR job

/// Notice tone (Desk notice `tone`): the notification centre's.
pub use crate::notices::Tone;

/// A button of the job notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobAction {
    /// 재요청: ask the agent again.
    Retry,
    /// 취소: stop waiting (the agent itself is not interrupted).
    Cancel,
    /// PR 만들기: the agent is done, go on (Meninx has no turn-end signal).
    CreateNow,
    /// 확인: open the recheck dialog.
    Recheck,
    /// Open this page in the session's web tab.
    Open(String),
}

/// Where a PR job is (Desk `stage`).
#[derive(Debug, Clone)]
pub enum JobStage {
    /// The agent writes the PR text (or commits files a recheck included).
    Writing,
    /// Changed since the request: waits for the user (recheck dialog).
    Reviewing(PrRecheckChange),
    Creating,
    /// "PR 올린 뒤 바로 머지": waiting for GitHub / checks.
    Merging,
    Over,
}

/// "PR 올린 뒤 바로 머지" wait (Desk `mergeCreatedPullRequest`).
#[derive(Debug, Clone)]
pub struct MergeWait {
    pub number: u64,
    pub method: PrMergeMethod,
    pub delete: bool,
    started: Instant,
    last_poll: Option<Instant>,
    /// `pr.gate` failed once: GitHub's REST view only from then on.
    gate_off: bool,
    blocked_since: Option<Instant>,
    in_flight: bool,
}

/// A PR request (Desk `startPullRequestJob`), with the notice that follows it.
#[derive(Debug, Clone)]
pub struct PrJob {
    /// Incremented by 재요청 / 취소: late answers of an older attempt are dropped.
    pub jgen: u64,
    pub stage: JobStage,
    pub target: String,
    pub draft: bool,
    /// Push the commits (false when the request left them out).
    pub push: bool,
    pub merge: Option<(PrMergeMethod, bool)>,
    /// The commit-based text ("커밋 기록으로 작성"); `None` = the agent writes it.
    pub commit_text: Option<CommitPrTextOptions>,
    pub exclude: PrExclusions,
    /// Create from the PR text left behind, without asking the agent.
    pub reuse: bool,
    /// The worktree at the first ask (HEAD, uncommitted paths).
    pub before: Option<(String, Vec<String>)>,
    pub allowed: Vec<String>,
    pub ignored: Vec<String>,
    pub files_included: bool,
    /// Unix seconds of the last ask: PR text written before it is not this request's.
    pub asked_at: u64,
    /// The PR file the agent writes (from `pr.reset_file`).
    pub pr_file: Option<String>,
    /// `written_at` of the PR text seen at the last poll (go on once it stays the same).
    seen: Option<u64>,
    /// Files a recheck asked the agent to commit (waits until they are committed).
    committing: Option<Vec<String>>,
    pub waiting: Option<MergeWait>,
    /// A poll / call in flight.
    busy: bool,
    pub tone: Tone,
    pub title: String,
    pub message: String,
    pub actions: Vec<JobAction>,
}

impl PrJob {
    pub fn running(&self) -> bool {
        !matches!(self.stage, JobStage::Over)
    }

    fn set(&mut self, tone: Tone, title: String, message: String, actions: Vec<JobAction>) {
        self.tone = tone;
        self.title = title;
        self.message = message;
        self.actions = actions;
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------------- dialogs

/// Something that loads.
#[derive(Debug, Clone, Default)]
pub enum Load<T> {
    #[default]
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }
}

fn load<T>(r: Result<T, ProtoError>) -> Load<T> {
    match r {
        Ok(t) => Load::Ready(t),
        Err(e) => Load::Failed(i18n::proto_error(&e)),
    }
}

/// PR 올리기 (Desk `PullRequestModal`).
#[derive(Debug, Clone)]
pub struct PrForm {
    pub session: String,
    pub repo: String,
    pub repo_name: String,
    /// The session's base for this repo (the preferred target).
    pub session_base: String,
    /// "PR 올리기 전에 확인해 주세요": `Ready(None)` = nothing pending (or the lookup failed).
    pub pending: Load<Option<PendingWork>>,
    pub exclude: PrExclusions,
    pub confirmed: bool,
    pub targets: Load<PrTargets>,
    pub target: Option<String>,
    pub from_commits: bool,
    pub preview: Option<Load<CommitPrPreview>>,
    preview_seq: u64,
    pub title: String,
    pub show_body: bool,
    pub draft: bool,
    pub merge_now: bool,
    pub method: PrMergeMethod,
    pub delete_branch: bool,
    pub left: Option<PrLeftover>,
    left_seq: u64,
    pub show_left_body: bool,
    pub confirm_reuse: bool,
    pub existing: Option<PullRequestResult>,
    /// The lookup ran and found nothing.
    pub checked: bool,
    pub busy: bool,
    pub request_error: Option<String>,
    epoch: u64,
}

impl PrForm {
    pub fn asking(&self) -> bool {
        match &self.pending {
            Load::Loading => true,
            Load::Ready(Some(_)) => !self.confirmed,
            _ => false,
        }
    }

    pub fn pending_work(&self) -> Option<&PendingWork> {
        self.pending.ready().and_then(Option::as_ref)
    }

    /// What the choice really leaves out (Desk `effectiveExclusions`).
    pub fn excluded(&self) -> Option<PrExclusions> {
        meninx_client::effective_exclusions(self.pending_work(), self.exclude)
    }

    pub fn push_commits(&self) -> bool {
        !self.excluded().is_some_and(|e| e.commits)
    }

    pub fn preview_loading(&self) -> bool {
        self.from_commits && !matches!(self.preview, Some(Load::Ready(_)) | Some(Load::Failed(_)))
    }

    /// Desk `commitBlocked`.
    pub fn commit_blocked(&self) -> bool {
        if self.preview_loading() {
            return false;
        }
        match &self.preview {
            Some(Load::Ready(p)) => self.title.trim().is_empty() || p.commits.is_empty(),
            _ => true,
        }
    }

    pub fn no_changes(&self) -> bool {
        self.left.as_ref().is_some_and(|l| l.changes == Some(false))
    }
}

/// PR #N 머지 (Desk `MergePrModal`).
#[derive(Debug, Clone)]
pub struct MergeForm {
    pub session: String,
    pub repo: String,
    pub pr: PrStatus,
    pub method: PrMergeMethod,
    pub delete_branch: bool,
    pub busy: bool,
    pub error: Option<String>,
}

/// PR #N을 닫을까요? (Desk's close confirm).
#[derive(Debug, Clone)]
pub struct ClosePrForm {
    pub session: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub busy: bool,
    pub error: Option<String>,
}

/// 강제 푸시 / 원격 상태로 덮어쓰기 confirm (Desk `SyncButton` modal).
#[derive(Debug, Clone)]
pub struct SyncConfirmForm {
    pub session: String,
    pub repo: String,
    pub op: SyncOp,
}

/// 새 브랜치로 이어가기 (Desk `BranchRefreshModal` + `BranchRefreshForm`).
#[derive(Debug, Clone)]
pub struct ContinueForm {
    pub session: String,
    pub repo: String,
    pub info: Load<BranchRefreshInfo>,
    pub name: String,
    /// Picked commits (full ids).
    pub picked: Vec<String>,
    pub move_files: bool,
    /// The open PR (number, title) and whether to close it afterwards.
    pub open_pr: Option<(u64, String)>,
    pub close_pr: bool,
    pub check: Option<Load<BranchRefreshCheck>>,
    check_seq: u64,
    pub running: bool,
    /// Result lines once it ran (Desk `resultLines`).
    pub result: Option<Vec<String>>,
}

impl ContinueForm {
    /// The picks in branch order (oldest first).
    pub fn picks(&self) -> Vec<String> {
        self.info.ready().map_or_else(Vec::new, |i| {
            i.commits
                .iter()
                .filter(|c| self.picked.contains(&c.full))
                .map(|c| c.full.clone())
                .collect()
        })
    }

    pub fn with_files(&self) -> bool {
        self.move_files && self.info.ready().is_some_and(|i| !i.files.is_empty())
    }

    pub fn can_run(&self) -> bool {
        branch_name_ok(&self.name)
            && !self.running
            && matches!(&self.check, Some(Load::Ready(c)) if c.conflict.is_none())
    }
}

/// PR 올리기 전에 확인해 주세요 (Desk `PrRecheckModal`).
#[derive(Debug, Clone)]
pub struct RecheckForm {
    pub session: String,
    pub repo: String,
    pub change: PrRecheckChange,
    pub commits_on: bool,
    pub files_on: bool,
    /// The request had "바로 머지" (`Some`): keep it?
    pub merge: Option<bool>,
    pub push_off: bool,
}

impl RecheckForm {
    pub fn commits_in(&self) -> bool {
        self.change.rewritten || self.commits_on
    }

    pub fn files_in(&self) -> bool {
        self.commits_in() && self.files_on
    }
}

/// The footer's dialogs ([`Modal::Footer`], boxed there).
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum FooterModal {
    Pr(PrForm),
    Merge(MergeForm),
    ClosePr(ClosePrForm),
    SyncConfirm(SyncConfirmForm),
    Continue(ContinueForm),
    Recheck(RecheckForm),
}

impl FooterModal {
    /// A request in flight: the dialog stays open.
    pub fn busy(&self) -> bool {
        match self {
            FooterModal::Pr(f) => f.busy,
            FooterModal::Merge(f) => f.busy,
            FooterModal::ClosePr(f) => f.busy,
            FooterModal::Continue(f) => f.running,
            FooterModal::SyncConfirm(_) | FooterModal::Recheck(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------- messages

/// Messages of the footer of session `.0` of [`Message::Footer`] (`""` = a timer tick).
#[derive(Debug, Clone)]
pub enum FooterMsg {
    Tick,
    Synced(String, u64, bool, Result<SyncStatus, ProtoError>),
    PrLoaded(String, u64, Result<Option<PrStatus>, ProtoError>),
    LeftoverLoaded(String, Result<PrLeftover, ProtoError>),
    Menu(FooterMenu),
    CloseMenu,
    /// 커밋 맡기기.
    Commit,
    PromptSent(Result<(), ProtoError>),
    Sync(SyncOp),
    /// 강제 푸시 / 덮어쓰기 confirmed.
    SyncConfirmed,
    SyncDone(String, SyncOp, Result<String, ProtoError>),
    ViewPr,
    OpenMerge,
    MergeMethod(PrMergeMethod),
    MergeDelete(bool),
    MergeConfirm,
    Merged(Result<PrMerged, ProtoError>),
    OpenClosePr,
    ClosePrConfirm,
    PrClosed(Result<PrClosed, ProtoError>),
    /// "브랜치 업데이트" (the open PR is behind its base), and GitHub's answer for `repo`.
    UpdateBranch,
    BranchUpdated(String, Result<Option<String>, ProtoError>),
    // PR dialog
    OpenPr,
    /// The "올리지 않은 PR 내용" strip: the dialog in AI mode, where the text can be reused.
    OpenLeftover,
    PrPending(Result<PendingWork, ProtoError>),
    PrExcludeCommits(bool),
    PrExcludeFiles(bool),
    PrContinue,
    PrTargets(Result<PrTargets, ProtoError>),
    PrTarget(String),
    PrFromCommits(bool),
    PrPreview(u64, Result<CommitPrPreview, ProtoError>),
    PrTitle(String),
    PrToggleBody,
    PrDraft(bool),
    PrMergeNow(bool),
    PrMergeNowMethod(PrMergeMethod),
    PrMergeNowDelete(bool),
    PrLeftover(u64, Result<PrLeftover, ProtoError>),
    PrToggleLeftoverBody,
    /// 기존 PR 확인 (lookup only).
    PrCheckExisting,
    /// Submit (`true` = reuse the PR text left behind).
    PrSubmit(bool),
    PrLookedUp(
        u64,
        bool,
        bool,
        Result<Option<PullRequestResult>, ProtoError>,
    ),
    PrOpenExisting,
    // PR job
    JobAction(String, JobAction),
    JobDismiss(String),
    /// A job call answered: (repo, generation, what).
    Job(String, u64, JobEvent),
    // recheck dialog
    RecheckCommits(bool),
    RecheckFiles(bool),
    RecheckMerge(bool),
    RecheckResolve,
    RecheckCancel,
    // 새 브랜치로 이어가기
    OpenContinue,
    ContinueInfo(Result<BranchRefreshInfo, ProtoError>),
    ContinueName(String),
    ContinuePick(String),
    ContinueAll,
    ContinueMoveFiles(bool),
    ContinueClosePr(bool),
    ContinueChecked(u64, Result<BranchRefreshCheck, ProtoError>),
    ContinueRun,
    ContinueDone(
        Result<BranchRefreshResult, ProtoError>,
        Option<Result<PrClosed, ProtoError>>,
    ),
}

/// HEAD and the uncommitted paths when a PR was first requested.
pub type Snapshot = (String, Vec<String>);

/// Answers of the PR job's calls.
#[derive(Debug, Clone)]
pub enum JobEvent {
    /// `pr.reset_file` + the snapshot (`pr.drift`) + `prompt.pr` + `send_prompt`.
    Asked(Result<(String, Option<Snapshot>), ProtoError>),
    /// The commit-based text (`pr.commit_text`).
    Prepared(Result<String, ProtoError>),
    Leftover(Result<PrLeftover, ProtoError>),
    Drift(Result<PrDrift, ProtoError>),
    /// Committing included files: their state now.
    CommitDrift(Result<PrDrift, ProtoError>),
    Created(Result<PullRequestResult, ProtoError>),
    /// `pr.status` after creating (conflict check).
    CreatedStatus(PullRequestResult, Result<Option<PrStatus>, ProtoError>),
    /// Merge wait: status + gate.
    MergePoll(
        Result<Option<PrStatus>, ProtoError>,
        Option<Result<PrGate, ProtoError>>,
    ),
    MergeDone(Result<PrMerged, ProtoError>),
    /// The recheck's "commit the files" prompt went out (HEAD before it).
    CommitAsked(Result<String, ProtoError>),
}

fn perform<T: Send + 'static>(
    sid: &str,
    fut: impl Future<Output = T> + Send + 'static,
    f: impl FnOnce(T) -> FooterMsg + Send + 'static,
) -> Task<Message> {
    let sid = sid.to_owned();
    Task::perform(fut, move |r| Message::Footer(sid, f(r)))
}

/// What a detail message means for the footer (checked before it is handled).
pub enum DetailHook {
    None,
    /// Git status of a repo arrived (the tree / HEAD may have changed).
    Status(String),
    /// History / tab / repo changed: the PR state may be outdated.
    Refresh,
}

pub fn detail_hook(m: &DetailMsg) -> DetailHook {
    match m {
        DetailMsg::Status(repo, Ok(_)) => DetailHook::Status(repo.clone()),
        DetailMsg::HistoryLoaded(..)
        | DetailMsg::Unpushed(..)
        | DetailMsg::PickRepo(_)
        | DetailMsg::Side(_) => DetailHook::Refresh,
        _ => DetailHook::None,
    }
}

/// The footer is shown under Changes and History, not under Explore (Desk keeps it mounted).
pub fn shown_on(side: SideTab) -> bool {
    side != SideTab::Explore
}

// ---------------------------------------------------------------------------- logic

impl App {
    pub fn footer(&self, sid: &str, repo: &str) -> Option<&RepoFooter> {
        self.detail(sid)?.footer.repos.get(repo)
    }

    fn footer_mut(&mut self, sid: &str, repo: &str) -> Option<&mut RepoFooter> {
        Some(
            self.detail_mut(sid)?
                .footer
                .repos
                .entry(repo.to_owned())
                .or_default(),
        )
    }

    /// The repo shown by the session's Changes / History (`None` without repos).
    fn footer_repo(&self, sid: &str) -> Option<String> {
        self.detail(sid)
            .map(|d| d.repo.clone())
            .filter(|r| !r.is_empty())
    }

    fn footer_repo_name(&self, sid: &str, repo: &str) -> String {
        self.live(sid)
            .and_then(|l| l.info.repos.iter().find(|r| r.repo == repo))
            .map(|r| r.name.clone())
            .unwrap_or_else(|| repo.to_owned())
    }

    fn repo_base(&self, sid: &str, repo: &str) -> String {
        self.live(sid)
            .and_then(|l| l.info.repos.iter().find(|r| r.repo == repo))
            .map(|r| r.base.clone())
            .unwrap_or_default()
    }

    /// Workspace sessions can ask for a PR (Desk: `onPullRequest` only for workspace sessions).
    pub fn footer_workspace(&self, sid: &str) -> bool {
        self.live(sid)
            .is_some_and(|l| l.info.mode == SessionMode::Workspace)
    }

    /// The files 커밋 맡기기 commits (the checked rows of the Changes list, Desk's
    /// `checkedFiles`) and how many changed files there are.
    pub fn footer_checked(&self, sid: &str) -> (Vec<String>, usize) {
        let Some(d) = self.detail(sid) else {
            return (Vec::new(), 0);
        };
        let files = d.checked_changes().into_iter().map(|c| c.path).collect();
        (files, d.changed_count())
    }

    /// (commits listed in History, commits not on a remote, changed files) of the shown repo.
    pub fn footer_counts(&self, sid: &str) -> (usize, usize, usize) {
        self.detail(sid).map_or((0, 0, 0), |d| {
            (
                d.history.commits.len(),
                d.history.unpushed.len(),
                d.changed_count(),
            )
        })
    }

    /// The "…" menu's rows (Desk `AiActionBar`'s `Menu.Dropdown`).
    pub fn footer_more(&self, sid: &str) -> Vec<MoreEntry> {
        let Some(repo) = self.footer_repo(sid) else {
            return Vec::new();
        };
        let f = self.footer(sid, &repo);
        let (commits, unpushed, changed) = self.footer_counts(sid);
        let no_remote = f.is_some_and(RepoFooter::no_remote);
        let mut out = Vec::new();
        if let Some(pr) = f.and_then(RepoFooter::open_pr) {
            let n = pr.number.to_string();
            let local = if changed > 0 {
                Some(tr::foot_merge_uncommitted())
            } else if unpushed > 0 {
                Some(tr::foot_merge_unpushed())
            } else {
                None
            };
            let blocker = meninx_client::merge_blocker(pr);
            let hint = local.or_else(|| blocker.map(blocker_text));
            out.push(MoreEntry {
                kind: MoreKind::Merge,
                enabled: hint.is_none(),
                title: tr::foot_merge_item(&n),
                hint: Some(hint.unwrap_or_else(tr::foot_merge_hint)),
            });
            if blocker == Some(meninx_client::MergeBlocker::Behind) {
                out.push(MoreEntry {
                    kind: MoreKind::UpdateBranch,
                    enabled: f.is_some_and(|f| f.running.is_none()),
                    title: tr::foot_update_branch(),
                    hint: Some(tr::foot_update_branch_hint()),
                });
            }
            out.push(MoreEntry {
                kind: MoreKind::View,
                enabled: true,
                title: tr::foot_view_pr(),
                hint: None,
            });
            out.push(MoreEntry {
                kind: MoreKind::Close,
                enabled: true,
                title: tr::foot_close_item(&n),
                hint: Some(tr::foot_close_hint()),
            });
        } else if f.and_then(RepoFooter::recheck).is_some() {
            out.push(MoreEntry {
                kind: MoreKind::Recheck,
                enabled: true,
                title: tr::foot_recheck_item(),
                hint: Some(tr::foot_recheck_hint()),
            });
        } else {
            let workspace = self.footer_workspace(sid);
            let nothing = commits == 0 && changed == 0;
            let hint = if !workspace {
                tr::foot_pr_needs_workspace()
            } else if no_remote {
                tr::foot_pr_no_remote()
            } else if nothing {
                tr::foot_pr_nothing()
            } else {
                tr::foot_pr_hint()
            };
            out.push(MoreEntry {
                kind: MoreKind::CreatePr,
                enabled: workspace && !no_remote && !nothing,
                title: tr::foot_create_pr(),
                hint: Some(hint),
            });
        }
        out.push(MoreEntry {
            kind: MoreKind::Continue,
            enabled: !no_remote,
            title: tr::foot_continue(),
            hint: Some(tr::foot_continue_hint()),
        });
        out
    }

    /// After a detail message: the footer's own refreshes (Desk's effects on `version`,
    /// `commits` and `unpushed`).
    pub fn footer_after_detail(&mut self, sid: &str, hook: DetailHook) -> Task<Message> {
        let Some(repo) = self.footer_repo(sid) else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        let first = !self.footer(sid, &repo).is_some_and(|f| f.sync_requested);
        match hook {
            DetailHook::None => {}
            DetailHook::Status(r) if r == repo && !first => {
                tasks.push(self.footer_sync(sid, &repo, false));
            }
            DetailHook::Status(_) | DetailHook::Refresh => {}
        }
        if first {
            // Desk: fetch on open unless a fetch ran within a minute.
            let fresh = self
                .footer(sid, &repo)
                .and_then(|f| f.fetched_at)
                .is_some_and(|t| t.elapsed() < FRESH_ON_OPEN);
            tasks.push(self.footer_sync(sid, &repo, !fresh));
        }
        tasks.push(self.footer_pr_if_changed(sid, &repo));
        Task::batch(tasks)
    }

    /// `pr.status` + `pr.leftover` when the branch's commits changed (Desk: on `commits` /
    /// `unpushed` change, not on every edit).
    fn footer_pr_if_changed(&mut self, sid: &str, repo: &str) -> Task<Message> {
        let Some(d) = self.detail(sid) else {
            return Task::none();
        };
        if !d.history.loaded {
            return Task::none();
        }
        let key = (
            d.history.commits.len(),
            d.history
                .commits
                .first()
                .map(|c| c.sha.clone())
                .unwrap_or_default(),
            d.history.unpushed.len(),
        );
        let f = self.footer_mut(sid, repo).expect("session");
        if f.key.as_ref() == Some(&key) {
            return Task::none();
        }
        f.key = Some(key);
        self.footer_pr(sid, repo)
    }

    /// Ask for the PR status and the leftover PR text again.
    pub fn footer_pr(&mut self, sid: &str, repo: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(f) = self.footer_mut(sid, repo) else {
            return Task::none();
        };
        f.pr_seq += 1;
        let seq = f.pr_seq;
        let (s, r) = (sid.to_owned(), repo.to_owned());
        let (c2, s2, r2) = (c.clone(), s.clone(), r.clone());
        let repo1 = repo.to_owned();
        let repo2 = repo.to_owned();
        Task::batch([
            perform(sid, async move { c.pr_status(&s, &r).await }, move |res| {
                FooterMsg::PrLoaded(repo1, seq, res)
            }),
            perform(
                sid,
                async move { c2.pr_leftover(&s2, &r2, None).await },
                move |res| FooterMsg::LeftoverLoaded(repo2, res),
            ),
        ])
    }

    /// `git.sync_status` (Desk `SyncButton.refresh`).
    fn footer_sync(&mut self, sid: &str, repo: &str, fetch: bool) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(f) = self.footer_mut(sid, repo) else {
            return Task::none();
        };
        f.sync_requested = true;
        f.sync_seq += 1;
        if fetch {
            f.fetched_at = Some(Instant::now());
        }
        let seq = f.sync_seq;
        let merged_head =
            f.pr.as_ref()
                .filter(|p| p.state == PrState::Merged)
                .map(|p| p.head_sha.clone())
                .filter(|s| !s.is_empty());
        let (s, r, repo) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
        perform(
            sid,
            async move { c.git_sync_status(&s, &r, fetch, merged_head).await },
            move |res| FooterMsg::Synced(repo, seq, fetch, res),
        )
    }

    fn footer_toast(&mut self, sid: &str, tone: crate::notices::Tone, msg: String) {
        crate::script::log(&format!("footer toast: {msg}"));
        if let Some(d) = self.detail_mut(sid) {
            d.toast(tone, msg);
        }
    }

    fn footer_modal(&mut self) -> Option<&mut FooterModal> {
        match &mut self.modal {
            Some(Modal::Footer(f)) => Some(f),
            _ => None,
        }
    }

    /// Open `url` in the session's web tab (Desk opens PR pages in the OS browser).
    pub fn footer_open_url(&mut self, sid: &str, url: &str) -> Task<Message> {
        crate::script::log(&format!("footer: open {url}"));
        // The page belongs to the folder's detail area.
        let owner = self.workspace_owner(sid);
        if let Some(l) = self.live_mut(&owner) {
            l.url_input = url.to_owned();
            if let Some(w) = &l.web {
                w.navigate(url);
            }
        }
        self.activate(sid, crate::detail::ItemKey::Web)
    }

    /// The footer timer: periodic fetch of the shown repo, PR job polling.
    pub fn footer_subscription(&self) -> iced::Subscription<Message> {
        let jobs = self.sessions.iter().any(|l| {
            l.detail
                .footer
                .repos
                .values()
                .any(|f| f.job.as_ref().is_some_and(PrJob::running))
        });
        let every = if jobs {
            JOB_POLL
        } else {
            Duration::from_secs(30)
        };
        iced::time::every(every).map(|_| Message::Footer(String::new(), FooterMsg::Tick))
    }

    pub fn on_footer(&mut self, sid: String, m: FooterMsg) -> Task<Message> {
        if matches!(m, FooterMsg::Tick) {
            return self.footer_tick();
        }
        let sid = sid.as_str();
        if self.live(sid).is_none() {
            return Task::none();
        }
        let repo_now = self.footer_repo(sid).unwrap_or_default();
        match m {
            FooterMsg::Tick => Task::none(),
            FooterMsg::Synced(repo, seq, fetch, res) => {
                let Some(f) = self.footer_mut(sid, &repo) else {
                    return Task::none();
                };
                if seq != f.sync_seq {
                    return Task::none();
                }
                match res {
                    Ok(s) => {
                        crate::script::log(&format!("footer sync {repo} (fetch {fetch}): {s:?}"));
                        f.sync = Some(s);
                    }
                    Err(e) => {
                        crate::script::log(&format!("footer sync {repo} failed: {e:?}"));
                        f.sync = None;
                        if e.id != "disconnected" {
                            // History only (it repeats with every check): one per repo.
                            self.notify(
                                crate::notices::Notice::warning(crate::notices::Text::tr(
                                    tr::notice_sync_status_failed,
                                ))
                                .body(i18n::proto_error(&e))
                                .session(self.workspace_owner(sid))
                                .repo(repo.clone())
                                .key(crate::notices::NoticeKey::Other(format!("sync:{sid}:{repo}")))
                                .quiet(),
                            );
                        }
                    }
                }
                Task::none()
            }
            FooterMsg::PrLoaded(repo, seq, res) => {
                let Some(f) = self.footer_mut(sid, &repo) else {
                    return Task::none();
                };
                if seq != f.pr_seq {
                    return Task::none();
                }
                match res {
                    Ok(pr) => {
                        crate::script::log(&format!(
                            "footer pr {repo}: {:?}",
                            pr.as_ref().map(|p| (p.number, p.state))
                        ));
                        let merged_changed = pr.as_ref().map(|p| (p.state, p.head_sha.clone()))
                            != f.pr.as_ref().map(|p| (p.state, p.head_sha.clone()));
                        f.pr = pr;
                        f.pr_error = None;
                        f.pr_loaded = true;
                        // A merged PR's head is subtracted from "ahead" (Desk `mergedPrHead`).
                        if merged_changed {
                            return self.footer_sync(sid, &repo, false);
                        }
                    }
                    Err(e) => {
                        crate::script::log(&format!("footer pr {repo} failed: {e:?}"));
                        f.pr_error = Some(i18n::proto_error(&e));
                    }
                }
                Task::none()
            }
            FooterMsg::LeftoverLoaded(repo, res) => {
                if let Some(f) = self.footer_mut(sid, &repo) {
                    f.leftover = res.is_ok_and(|l| l.file.is_some());
                }
                Task::none()
            }
            FooterMsg::Menu(menu) => {
                let d = self.detail_mut(sid).expect("session");
                d.footer.menu = (d.footer.menu != Some(menu)).then_some(menu);
                Task::none()
            }
            FooterMsg::CloseMenu => {
                if let Some(d) = self.detail_mut(sid) {
                    d.footer.menu = None;
                }
                Task::none()
            }
            FooterMsg::Commit => {
                let (files, total) = self.footer_checked(sid);
                if files.is_empty() || repo_now.is_empty() {
                    return Task::none();
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let all = files.len() == total;
                crate::script::log(&format!("footer: 커밋 맡기기 {files:?} all={all}"));
                let (s, r) = (sid.to_owned(), repo_now.clone());
                perform(
                    sid,
                    async move {
                        let text = c.prompt_commit(&s, &r, files, all).await?;
                        c.send_prompt(&s, &text).await
                    },
                    FooterMsg::PromptSent,
                )
            }
            FooterMsg::PromptSent(res) => {
                match res {
                    Ok(()) => crate::script::log("footer: prompt sent"),
                    Err(e) => {
                        let msg = tr::foot_prompt_failed(&i18n::proto_error(&e));
                        self.footer_toast(sid, crate::notices::Tone::Error, msg);
                    }
                }
                Task::none()
            }
            FooterMsg::Sync(op) => {
                if let Some(d) = self.detail_mut(sid) {
                    d.footer.menu = None;
                }
                if matches!(op, SyncOp::ForcePush | SyncOp::Reset) {
                    self.modal = Some(Modal::Footer(Box::new(FooterModal::SyncConfirm(
                        SyncConfirmForm {
                            session: sid.to_owned(),
                            repo: repo_now,
                            op,
                        },
                    ))));
                    return Task::none();
                }
                self.footer_run_sync(sid, &repo_now, op)
            }
            FooterMsg::SyncConfirmed => {
                let Some(FooterModal::SyncConfirm(f)) = self.footer_modal().cloned() else {
                    return Task::none();
                };
                self.modal = None;
                self.footer_run_sync(&f.session, &f.repo, f.op)
            }
            FooterMsg::SyncDone(repo, op, res) => {
                if let Some(f) = self.footer_mut(sid, &repo) {
                    f.running = None;
                }
                match res {
                    Ok(branch) => {
                        let msg = match op {
                            SyncOp::Fetch => None,
                            SyncOp::Publish => Some(tr::foot_published(&branch)),
                            SyncOp::Push => Some(tr::foot_pushed(&branch)),
                            SyncOp::ForcePush => Some(tr::foot_force_pushed(&branch)),
                            SyncOp::Pull => Some(tr::foot_pulled(&branch)),
                            SyncOp::Rebase => Some(tr::foot_rebased(&branch)),
                            SyncOp::Reset => Some(tr::foot_reset_done(&branch)),
                        };
                        if let Some(m) = msg {
                            self.footer_toast(sid, crate::notices::Tone::Success, m);
                        }
                    }
                    Err(e) => {
                        let m = tr::foot_sync_failed(&op_label(op), &i18n::proto_error(&e));
                        self.footer_toast(sid, crate::notices::Tone::Error, m);
                    }
                }
                // Desk: recount, then the PR's head / mergeability may have changed.
                let pr = self.footer_pr(sid, &repo);
                Task::batch([self.footer_sync(sid, &repo, false), pr])
            }
            FooterMsg::ViewPr => {
                if let Some(d) = self.detail_mut(sid) {
                    d.footer.menu = None;
                }
                let url = self
                    .footer(sid, &repo_now)
                    .and_then(RepoFooter::open_pr)
                    .map(|p| p.url.clone());
                match url {
                    Some(u) => self.footer_open_url(sid, &u),
                    None => Task::none(),
                }
            }
            FooterMsg::OpenMerge => {
                if let Some(d) = self.detail_mut(sid) {
                    d.footer.menu = None;
                }
                let Some(pr) = self
                    .footer(sid, &repo_now)
                    .and_then(RepoFooter::open_pr)
                    .cloned()
                else {
                    return Task::none();
                };
                self.modal = Some(Modal::Footer(Box::new(FooterModal::Merge(MergeForm {
                    session: sid.to_owned(),
                    repo: repo_now,
                    pr,
                    method: PrMergeMethod::Squash,
                    delete_branch: true,
                    busy: false,
                    error: None,
                }))));
                Task::none()
            }
            FooterMsg::MergeMethod(m) => {
                if let Some(FooterModal::Merge(f)) = self.footer_modal() {
                    f.method = m;
                }
                Task::none()
            }
            FooterMsg::MergeDelete(v) => {
                if let Some(FooterModal::Merge(f)) = self.footer_modal() {
                    f.delete_branch = v;
                }
                Task::none()
            }
            FooterMsg::MergeConfirm => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(FooterModal::Merge(f)) = self.footer_modal() else {
                    return Task::none();
                };
                if f.busy {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let (s, r, n, sha, m, del) = (
                    f.session.clone(),
                    f.repo.clone(),
                    f.pr.number,
                    f.pr.head_sha.clone(),
                    f.method,
                    f.delete_branch,
                );
                perform(
                    sid,
                    async move { c.pr_merge(&s, &r, n, &sha, m, del).await },
                    FooterMsg::Merged,
                )
            }
            FooterMsg::Merged(res) => {
                let Some(FooterModal::Merge(f)) = self.footer_modal() else {
                    return Task::none();
                };
                f.busy = false;
                let (repo, n, title) = (f.repo.clone(), f.pr.number, f.pr.title.clone());
                match res {
                    Ok(m) => {
                        self.modal = None;
                        let extra = if m.branch_deleted {
                            tr::foot_merged_branch_deleted()
                        } else {
                            String::new()
                        };
                        let msg = tr::foot_merged(&n.to_string(), &format!("{title}{extra}"));
                        self.footer_toast(sid, crate::notices::Tone::Success, msg);
                        let pr = self.footer_pr(sid, &repo);
                        Task::batch([pr, self.footer_sync(sid, &repo, true)])
                    }
                    Err(e) => {
                        f.error = Some(tr::foot_merge_failed(
                            &n.to_string(),
                            &i18n::proto_error(&e),
                        ));
                        Task::none()
                    }
                }
            }
            FooterMsg::OpenClosePr => {
                if let Some(d) = self.detail_mut(sid) {
                    d.footer.menu = None;
                }
                let Some(pr) = self.footer(sid, &repo_now).and_then(RepoFooter::open_pr) else {
                    return Task::none();
                };
                self.modal = Some(Modal::Footer(Box::new(FooterModal::ClosePr(ClosePrForm {
                    session: sid.to_owned(),
                    repo: repo_now.clone(),
                    number: pr.number,
                    title: pr.title.clone(),
                    busy: false,
                    error: None,
                }))));
                Task::none()
            }
            FooterMsg::ClosePrConfirm => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(FooterModal::ClosePr(f)) = self.footer_modal() else {
                    return Task::none();
                };
                if f.busy {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let (s, r, n) = (f.session.clone(), f.repo.clone(), f.number);
                perform(
                    sid,
                    async move { c.pr_close(&s, &r, n).await },
                    FooterMsg::PrClosed,
                )
            }
            FooterMsg::PrClosed(res) => {
                let Some(FooterModal::ClosePr(f)) = self.footer_modal() else {
                    return Task::none();
                };
                f.busy = false;
                let (repo, n) = (f.repo.clone(), f.number);
                match res {
                    Ok(_) => {
                        self.modal = None;
                        self.footer_toast(sid, crate::notices::Tone::Success, tr::foot_closed(&n.to_string()));
                        self.footer_pr(sid, &repo)
                    }
                    Err(e) => {
                        f.error = Some(tr::foot_close_failed(&i18n::proto_error(&e)));
                        Task::none()
                    }
                }
            }
            FooterMsg::UpdateBranch => {
                if let Some(d) = self.detail_mut(sid) {
                    d.footer.menu = None;
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(pr) = self.footer(sid, &repo_now).and_then(RepoFooter::open_pr) else {
                    return Task::none();
                };
                let (s, r, n, sha) = (sid.to_owned(), repo_now.clone(), pr.number, pr.head_sha.clone());
                let repo = repo_now.clone();
                perform(
                    sid,
                    async move { c.pr_update_branch(&s, &r, n, &sha).await },
                    move |res| FooterMsg::BranchUpdated(repo, res),
                )
            }
            FooterMsg::BranchUpdated(repo, res) => {
                let (tone, msg) = match res {
                    Ok(Some(m)) if !m.trim().is_empty() => (crate::notices::Tone::Info, m),
                    Ok(_) => (crate::notices::Tone::Info, tr::foot_branch_updating()),
                    Err(e) => (crate::notices::Tone::Error, tr::foot_update_branch_failed(&i18n::proto_error(&e))),
                };
                self.footer_toast(sid, tone, msg);
                self.footer_pr(sid, &repo)
            }
            FooterMsg::OpenPr => self.footer_open_pr(sid, &repo_now, true),
            FooterMsg::OpenLeftover => self.footer_open_pr(sid, &repo_now, false),
            FooterMsg::OpenContinue => self.footer_open_continue(sid, &repo_now),
            FooterMsg::JobAction(repo, a) => self.footer_job_action(sid, &repo, a),
            FooterMsg::JobDismiss(repo) => {
                if let Some(f) = self.footer_mut(sid, &repo)
                    && f.job.as_ref().is_some_and(|j| !j.running())
                {
                    f.job = None;
                }
                Task::none()
            }
            FooterMsg::Job(repo, jgen, ev) => self.footer_job_event(sid, &repo, jgen, ev),
            m @ (FooterMsg::RecheckCommits(_)
            | FooterMsg::RecheckFiles(_)
            | FooterMsg::RecheckMerge(_)
            | FooterMsg::RecheckResolve
            | FooterMsg::RecheckCancel) => self.footer_recheck_msg(sid, m),
            m @ (FooterMsg::ContinueInfo(_)
            | FooterMsg::ContinueName(_)
            | FooterMsg::ContinuePick(_)
            | FooterMsg::ContinueAll
            | FooterMsg::ContinueMoveFiles(_)
            | FooterMsg::ContinueClosePr(_)
            | FooterMsg::ContinueChecked(..)
            | FooterMsg::ContinueRun
            | FooterMsg::ContinueDone(..)) => self.footer_continue_msg(sid, m),
            m => self.footer_pr_msg(sid, m),
        }
    }

    fn footer_run_sync(&mut self, sid: &str, repo: &str, op: SyncOp) -> Task<Message> {
        if repo.is_empty() {
            return Task::none();
        }
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(f) = self.footer_mut(sid, repo) else {
            return Task::none();
        };
        if f.running.is_some() {
            return Task::none();
        }
        f.running = Some(op);
        crate::script::log(&format!("footer: sync {op:?} on {repo}"));
        if op == SyncOp::Fetch {
            f.fetched_at = Some(Instant::now());
        }
        let merged_head =
            f.pr.as_ref()
                .filter(|p| p.state == PrState::Merged)
                .map(|p| p.head_sha.clone());
        let (s, r, repo) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
        perform(
            sid,
            async move {
                match op {
                    SyncOp::Fetch => c
                        .git_sync_status(&s, &r, true, merged_head)
                        .await
                        .map(|_| String::new()),
                    SyncOp::Push | SyncOp::Publish => c.git_push(&s, &r, false, None).await,
                    SyncOp::ForcePush => c.git_push(&s, &r, true, None).await,
                    SyncOp::Pull => c.git_pull(&s, &r, PullMode::Merge).await,
                    SyncOp::Rebase => c.git_pull(&s, &r, PullMode::Rebase).await,
                    SyncOp::Reset => c.git_pull(&s, &r, PullMode::Reset).await,
                }
            },
            move |res| FooterMsg::SyncDone(repo, op, res),
        )
    }

    fn footer_tick(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        // Periodic fetch of the shown repo (Desk: every 5 minutes while shown).
        if let Some(sid) = self.shown_detail()
            && let Some(repo) = self.footer_repo(&sid)
            && self.detail(&sid).is_some_and(|d| shown_on(d.side))
            && self
                .footer(&sid, &repo)
                .is_some_and(|f| f.sync_requested && f.running.is_none())
            && self
                .footer(&sid, &repo)
                .and_then(|f| f.fetched_at)
                .is_none_or(|t| t.elapsed() >= FETCH_EVERY)
        {
            tasks.push(self.footer_sync(&sid, &repo, true));
        }
        let jobs: Vec<(String, String)> = self
            .sessions
            .iter()
            .flat_map(|l| {
                l.detail
                    .footer
                    .repos
                    .iter()
                    .filter(|(_, f)| f.job.as_ref().is_some_and(PrJob::running))
                    .map(|(r, _)| (l.info.id.clone(), r.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (sid, repo) in jobs {
            tasks.push(self.footer_job_poll(&sid, &repo));
        }
        Task::batch(tasks)
    }
}

/// Desk `OP_LABEL`.
pub fn op_label(op: SyncOp) -> String {
    match op {
        SyncOp::Fetch => tr::foot_op_fetch(),
        SyncOp::Push => tr::foot_op_push(),
        SyncOp::Publish => tr::foot_op_publish(),
        SyncOp::ForcePush => tr::foot_op_force_push(),
        SyncOp::Pull => tr::foot_op_pull(),
        SyncOp::Rebase => tr::foot_op_rebase(),
        SyncOp::Reset => tr::foot_op_reset(),
    }
}

/// Desk `mergeBlocker` texts.
pub fn blocker_text(b: meninx_client::MergeBlocker) -> String {
    use meninx_client::MergeBlocker as B;
    match b {
        B::AlreadyMerged => tr::foot_blocker_merged(),
        B::Closed => tr::foot_blocker_closed(),
        B::Draft => tr::foot_blocker_draft(),
        B::ChangesRequested => tr::foot_blocker_changes_requested(),
        B::Blocked => tr::foot_blocker_blocked(),
        B::Conflict => tr::foot_blocker_conflict(),
        B::Behind => tr::foot_blocker_behind(),
        B::NotMergeable => tr::foot_blocker_not_mergeable(),
    }
}

fn stop_text(s: &MergeStop) -> String {
    match s {
        MergeStop::TargetChanged { from, to } => tr::foot_stop_target_changed(from, to),
        MergeStop::AlreadyMerged => tr::foot_blocker_merged(),
        MergeStop::Draft => tr::foot_stop_draft(),
        MergeStop::ChangesRequested => tr::foot_blocker_changes_requested(),
        MergeStop::ReviewRequired => tr::foot_stop_review_required(),
        MergeStop::ChecksFailed(names) => tr::foot_stop_checks_failed(&names.join(", ")),
        MergeStop::Protected => tr::foot_stop_protected(),
        MergeStop::Blocker(b) => blocker_text(*b),
    }
}

// ---------------------------------------------------------------------------- PR dialog

impl App {
    /// PR 올리기 (`from_commits`: Desk's default "커밋 기록으로 작성"; the leftover strip opens
    /// the AI mode, where the text left behind can be reused).
    fn footer_open_pr(&mut self, sid: &str, repo: &str, from_commits: bool) -> Task<Message> {
        if let Some(d) = self.detail_mut(sid) {
            d.footer.menu = None;
        }
        if repo.is_empty() || !self.footer_workspace(sid) {
            return Task::none();
        }
        let Some(c) = self.online() else {
            return Task::none();
        };
        let form = PrForm {
            session: sid.to_owned(),
            repo: repo.to_owned(),
            repo_name: self.footer_repo_name(sid, repo),
            session_base: self.repo_base(sid, repo),
            pending: Load::Loading,
            exclude: PrExclusions::default(),
            confirmed: false,
            targets: Load::Loading,
            target: None,
            from_commits,
            preview: None,
            preview_seq: 0,
            title: String::new(),
            show_body: true,
            draft: false,
            merge_now: false,
            method: PrMergeMethod::Squash,
            delete_branch: true,
            left: None,
            left_seq: 0,
            show_left_body: false,
            confirm_reuse: false,
            existing: None,
            checked: false,
            busy: false,
            request_error: None,
            epoch: 0,
        };
        crate::script::log(&format!("footer: PR dialog for {repo}"));
        self.modal = Some(Modal::Footer(Box::new(FooterModal::Pr(form))));
        let (s, r) = (sid.to_owned(), repo.to_owned());
        let (c2, s2, r2) = (c.clone(), s.clone(), r.clone());
        Task::batch([
            perform(
                sid,
                async move { c.pr_pending_work(&s, &r).await },
                FooterMsg::PrPending,
            ),
            perform(
                sid,
                async move { c2.pr_remote_branches(&s2, &r2).await },
                FooterMsg::PrTargets,
            ),
        ])
    }

    fn pr_form(&mut self) -> Option<&mut PrForm> {
        match self.footer_modal() {
            Some(FooterModal::Pr(f)) => Some(f),
            _ => None,
        }
    }

    /// Reload what depends on the target / mode (preview, leftover) and drop lookups.
    fn pr_reload(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(f) = self.pr_form() else {
            return Task::none();
        };
        f.epoch += 1;
        f.busy = false;
        f.existing = None;
        f.checked = false;
        f.request_error = None;
        let Some(target) = f.target.clone() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        f.left_seq += 1;
        f.left = None;
        f.show_left_body = false;
        f.confirm_reuse = false;
        let (s, r, t, seq) = (
            f.session.clone(),
            f.repo.clone(),
            target.clone(),
            f.left_seq,
        );
        let c2 = c.clone();
        tasks.push(perform(
            sid,
            async move { c2.pr_leftover(&s, &r, Some(&t)).await },
            move |res| FooterMsg::PrLeftover(seq, res),
        ));
        f.preview_seq += 1;
        f.preview = None;
        f.title.clear();
        f.show_body = true;
        if f.from_commits && !f.asking() {
            f.preview = Some(Load::Loading);
            let push = f.push_commits();
            let (s, r, seq) = (f.session.clone(), f.repo.clone(), f.preview_seq);
            tasks.push(perform(
                sid,
                async move { c.pr_commit_preview(&s, &r, &target, push).await },
                move |res| FooterMsg::PrPreview(seq, res),
            ));
        }
        Task::batch(tasks)
    }

    fn footer_pr_msg(&mut self, sid: &str, m: FooterMsg) -> Task<Message> {
        let Some(f) = self.pr_form() else {
            return Task::none();
        };
        match m {
            FooterMsg::PrPending(res) => {
                f.pending = Load::Ready(
                    res.ok()
                        .filter(|p| p.commit_count > 0 || !p.files.is_empty()),
                );
                if !f.asking() {
                    return self.pr_reload(sid);
                }
                Task::none()
            }
            FooterMsg::PrExcludeCommits(on) => {
                f.exclude.commits = !on;
                Task::none()
            }
            FooterMsg::PrExcludeFiles(on) => {
                f.exclude.files = !on;
                Task::none()
            }
            FooterMsg::PrContinue => {
                f.confirmed = true;
                self.pr_reload(sid)
            }
            FooterMsg::PrTargets(res) => {
                match res {
                    Ok(t) => {
                        let list: Vec<String> = t
                            .branches
                            .iter()
                            .filter(|b| **b != t.current)
                            .cloned()
                            .collect();
                        let base = (!f.session_base.is_empty()).then_some(f.session_base.as_str());
                        f.target = if base.is_some_and(|b| list.iter().any(|x| x == b)) {
                            base.map(str::to_owned)
                        } else {
                            meninx_client::choose_pr_target(
                                &list,
                                &t.current,
                                None,
                                base,
                                t.default_branch.as_deref(),
                            )
                        };
                        f.targets = Load::Ready(PrTargets {
                            branches: list,
                            ..t
                        });
                    }
                    Err(e) => f.targets = Load::Failed(i18n::proto_error(&e)),
                }
                self.pr_reload(sid)
            }
            FooterMsg::PrTarget(t) => {
                f.target = Some(t);
                self.pr_reload(sid)
            }
            FooterMsg::PrFromCommits(v) => {
                f.from_commits = v;
                self.pr_reload(sid)
            }
            FooterMsg::PrPreview(seq, res) => {
                if seq == f.preview_seq {
                    if let Ok(p) = &res {
                        f.title = p.title.clone();
                    }
                    f.preview = Some(load(res));
                }
                Task::none()
            }
            FooterMsg::PrTitle(t) => {
                f.title = t;
                Task::none()
            }
            FooterMsg::PrToggleBody => {
                f.show_body = !f.show_body;
                Task::none()
            }
            FooterMsg::PrDraft(v) => {
                f.draft = v;
                if v {
                    f.merge_now = false;
                }
                Task::none()
            }
            FooterMsg::PrMergeNow(v) => {
                f.merge_now = v && !f.draft;
                Task::none()
            }
            FooterMsg::PrMergeNowMethod(m) => {
                f.method = m;
                Task::none()
            }
            FooterMsg::PrMergeNowDelete(v) => {
                f.delete_branch = v;
                Task::none()
            }
            FooterMsg::PrLeftover(seq, res) => {
                if seq == f.left_seq {
                    f.left = res.ok();
                }
                Task::none()
            }
            FooterMsg::PrToggleLeftoverBody => {
                f.show_left_body = !f.show_left_body;
                Task::none()
            }
            FooterMsg::PrCheckExisting => self.pr_request(sid, false, false),
            FooterMsg::PrSubmit(reuse) => {
                let diverged = f
                    .left
                    .as_ref()
                    .and_then(|l| l.file.as_ref())
                    .is_some_and(|x| x.matches == meninx_client::LeftoverMatch::Diverged);
                if reuse && diverged && !f.confirm_reuse {
                    f.confirm_reuse = true;
                    return Task::none();
                }
                self.pr_request(sid, true, reuse)
            }
            FooterMsg::PrLookedUp(epoch, generate, reuse, res) => {
                if epoch != f.epoch {
                    return Task::none();
                }
                f.busy = false;
                match res {
                    Err(e) => {
                        f.request_error = Some(i18n::proto_error(&e));
                        Task::none()
                    }
                    Ok(Some(existing)) => {
                        f.existing = Some(existing);
                        f.checked = true;
                        Task::none()
                    }
                    Ok(None) if !generate => {
                        f.checked = true;
                        Task::none()
                    }
                    Ok(None) => {
                        let f = f.clone();
                        self.modal = None;
                        self.footer_start_job(sid, &f, reuse)
                    }
                }
            }
            FooterMsg::PrOpenExisting => {
                let url = f.existing.as_ref().map(|e| e.url.clone());
                self.modal = None;
                match url {
                    Some(u) => self.footer_open_url(sid, &u),
                    None => Task::none(),
                }
            }
            _ => Task::none(),
        }
    }

    /// Desk `request(generate, reuse)`: the open-PR lookup first (never "no PR" on a failed
    /// lookup), then the job.
    fn pr_request(&mut self, sid: &str, generate: bool, reuse: bool) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(f) = self.pr_form() else {
            return Task::none();
        };
        if f.busy || (generate && f.target.is_none()) {
            return Task::none();
        }
        if generate && f.from_commits && f.commit_blocked() {
            return Task::none();
        }
        f.busy = true;
        f.request_error = None;
        f.existing = None;
        f.checked = false;
        let (s, r, epoch) = (f.session.clone(), f.repo.clone(), f.epoch);
        perform(
            sid,
            async move { c.pr_find_open(&s, &r).await },
            move |res| FooterMsg::PrLookedUp(epoch, generate, reuse, res),
        )
    }

    /// Start the PR job from the dialog's choices (Desk `requestPullRequest` +
    /// `startPullRequestJob`).
    fn footer_start_job(&mut self, sid: &str, f: &PrForm, reuse: bool) -> Task<Message> {
        let Some(target) = f.target.clone() else {
            return Task::none();
        };
        let excluded = f.excluded();
        let push = f.push_commits();
        let commit_text = f.from_commits.then(|| {
            let mut o = CommitPrTextOptions {
                push: Some(push),
                ..Default::default()
            };
            if let Some(Load::Ready(p)) = &f.preview {
                o.title = Some(f.title.trim().to_owned());
                o.expected_head = Some(p.head.clone());
                o.expected_source_head = Some(p.source_head.clone());
                o.expected_base_head = Some(p.base_head.clone());
                o.expected_branch = Some(p.branch.clone());
            }
            o
        });
        let exclude = if f.from_commits {
            PrExclusions {
                files: true,
                ..excluded.unwrap_or_default()
            }
        } else {
            excluded.unwrap_or_default()
        };
        let repo_name = f.repo_name.clone();
        let job = PrJob {
            jgen: 0,
            stage: JobStage::Writing,
            target,
            draft: f.draft,
            push,
            merge: (f.merge_now && !f.draft).then_some((f.method, f.delete_branch)),
            commit_text,
            exclude,
            reuse: !f.from_commits && reuse,
            before: None,
            allowed: Vec::new(),
            ignored: Vec::new(),
            files_included: !exclude.files,
            asked_at: 0,
            pr_file: None,
            seen: None,
            committing: None,
            waiting: None,
            busy: false,
            tone: Tone::Info,
            title: tr::foot_job_title(&repo_name),
            message: String::new(),
            actions: Vec::new(),
        };
        crate::script::log(&format!(
            "footer: PR job {} -> {} (commit text {}, reuse {})",
            f.repo,
            job.target,
            job.commit_text.is_some(),
            job.reuse
        ));
        let repo = f.repo.clone();
        if let Some(rf) = self.footer_mut(sid, &repo) {
            rf.job = Some(job);
        }
        self.job_ask(sid, &repo)
    }

    fn job_mut(&mut self, sid: &str, repo: &str) -> Option<&mut PrJob> {
        self.footer_mut(sid, repo)?.job.as_mut()
    }

    /// Desk `ask`: (re)start the job.
    fn job_ask(&mut self, sid: &str, repo: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let name = self.footer_repo_name(sid, repo);
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        j.jgen += 1;
        j.stage = JobStage::Writing;
        j.busy = true;
        j.seen = None;
        j.committing = None;
        j.waiting = None;
        let jgen = j.jgen;
        let (s, r) = (sid.to_owned(), repo.to_owned());
        let repo_s = repo.to_owned();
        if let Some(opts) = j.commit_text.clone() {
            j.set(
                Tone::Info,
                tr::foot_job_title(&name),
                tr::foot_job_preparing(),
                Vec::new(),
            );
            let target = j.target.clone();
            return perform(
                sid,
                async move { c.pr_commit_text(&s, &r, &target, opts).await },
                move |res| FooterMsg::Job(repo_s, jgen, JobEvent::Prepared(res)),
            );
        }
        if j.reuse {
            // The text left behind: create without asking the agent.
            j.busy = false;
            return self.job_create(sid, repo, None, None);
        }
        j.set(
            Tone::Info,
            tr::foot_job_title(&name),
            tr::foot_job_writing(),
            vec![JobAction::CreateNow, JobAction::Retry, JobAction::Cancel],
        );
        j.asked_at = now_secs();
        let (target, exclude) = (j.target.clone(), j.exclude);
        let snapshot = j.before.is_none();
        perform(
            sid,
            async move {
                let path = c.pr_reset_file(&s, &r).await?;
                // The first ask fixes what the request was about (no snapshot on failure).
                let before = if snapshot {
                    c.pr_drift(&s, &r, None, Vec::new())
                        .await
                        .ok()
                        .map(|d| (d.head, d.files.into_iter().map(|f| f.path).collect()))
                } else {
                    None
                };
                let text = c.prompt_pr(&s, &r, &target, exclude).await?;
                c.send_prompt(&s, &text).await?;
                Ok((path, before))
            },
            move |res| FooterMsg::Job(repo_s, jgen, JobEvent::Asked(res)),
        )
    }

    /// Desk `end`.
    fn job_end(
        &mut self,
        sid: &str,
        repo: &str,
        tone: Tone,
        title: String,
        message: String,
        actions: Vec<JobAction>,
    ) {
        crate::script::log(&format!(
            "footer: PR job ended {tone:?}: {title} · {message}"
        ));
        if let Some(j) = self.job_mut(sid, repo) {
            j.stage = JobStage::Over;
            j.busy = false;
            j.waiting = None;
            j.set(tone, title, message, actions);
        }
        if let Some(Modal::Footer(m)) = &self.modal
            && let FooterModal::Recheck(r) = m.as_ref()
            && r.session == sid
            && r.repo == repo
        {
            self.modal = None;
        }
    }

    /// Desk `create`.
    fn job_create(
        &mut self,
        sid: &str,
        repo: &str,
        push: Option<bool>,
        push_sha: Option<String>,
    ) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let name = self.footer_repo_name(sid, repo);
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        j.stage = JobStage::Creating;
        j.busy = true;
        let push = push.unwrap_or(j.push);
        j.set(
            Tone::Info,
            tr::foot_job_creating_title(&name),
            if push {
                tr::foot_job_creating_push()
            } else {
                tr::foot_job_creating_remote()
            },
            Vec::new(),
        );
        let sha = if push {
            push_sha.or_else(|| j.commit_text.as_ref().and_then(|o| o.expected_head.clone()))
        } else {
            None
        };
        let (target, draft, opts, jgen) =
            (j.target.clone(), j.draft, j.commit_text.clone(), j.jgen);
        let (s, r, repo_s) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
        perform(
            sid,
            async move {
                c.pr_create(&s, &r, &target, draft, push, sha.as_deref(), opts)
                    .await
            },
            move |res| FooterMsg::Job(repo_s, jgen, JobEvent::Created(res)),
        )
    }

    /// Desk `verify`: compare the worktree with the request before pushing.
    fn job_verify(&mut self, sid: &str, repo: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        let Some((head, _)) = j.before.clone() else {
            return self.job_create(sid, repo, None, None);
        };
        j.busy = true;
        let (allowed, jgen) = (j.allowed.clone(), j.jgen);
        let (s, r, repo_s) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
        perform(
            sid,
            async move { c.pr_drift(&s, &r, Some(&head), allowed).await },
            move |res| FooterMsg::Job(repo_s, jgen, JobEvent::Drift(res)),
        )
    }

    /// Poll a running job: the agent's PR text, the files a recheck asked to commit, the merge
    /// wait.
    fn footer_job_poll(&mut self, sid: &str, repo: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        if j.busy {
            return Task::none();
        }
        let (jgen, repo_s) = (j.jgen, repo.to_owned());
        let (s, r) = (sid.to_owned(), repo.to_owned());
        match &j.stage {
            JobStage::Writing if j.committing.is_some() => {
                j.busy = true;
                perform(
                    sid,
                    async move { c.pr_drift(&s, &r, None, Vec::new()).await },
                    move |res| FooterMsg::Job(repo_s, jgen, JobEvent::CommitDrift(res)),
                )
            }
            JobStage::Writing if j.commit_text.is_none() && !j.reuse && j.pr_file.is_some() => {
                j.busy = true;
                let target = j.target.clone();
                perform(
                    sid,
                    async move { c.pr_leftover(&s, &r, Some(&target)).await },
                    move |res| FooterMsg::Job(repo_s, jgen, JobEvent::Leftover(res)),
                )
            }
            JobStage::Merging => {
                let Some(w) = j.waiting.as_mut() else {
                    return Task::none();
                };
                if w.in_flight
                    || w.last_poll.is_some_and(|t| {
                        t.elapsed() < Duration::from_secs(meninx_client::MERGE_POLL_SECS)
                    })
                {
                    return Task::none();
                }
                w.in_flight = true;
                w.last_poll = Some(Instant::now());
                let (n, gate) = (w.number, !w.gate_off);
                perform(
                    sid,
                    async move {
                        let pr = c.pr_status(&s, &r).await;
                        let g = if gate {
                            Some(c.pr_gate(&s, &r, n).await)
                        } else {
                            None
                        };
                        (pr, g)
                    },
                    move |(pr, g)| FooterMsg::Job(repo_s, jgen, JobEvent::MergePoll(pr, g)),
                )
            }
            _ => Task::none(),
        }
    }

    fn footer_job_action(&mut self, sid: &str, repo: &str, a: JobAction) -> Task<Message> {
        let name = self.footer_repo_name(sid, repo);
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        crate::script::log(&format!("footer: job action {a:?}"));
        match a {
            JobAction::Retry => {
                j.reuse = false;
                self.job_ask(sid, repo)
            }
            JobAction::Cancel => {
                let was_merging = matches!(j.stage, JobStage::Merging);
                j.jgen += 1;
                if was_merging {
                    self.job_end(
                        sid,
                        repo,
                        Tone::Info,
                        tr::foot_job_merge_cancelled(&name),
                        tr::foot_job_merge_cancelled_body(),
                        Vec::new(),
                    );
                } else {
                    self.job_end(
                        sid,
                        repo,
                        Tone::Info,
                        tr::foot_job_cancelled(&name),
                        String::new(),
                        vec![JobAction::Retry],
                    );
                }
                Task::none()
            }
            JobAction::CreateNow => {
                if !matches!(j.stage, JobStage::Writing) {
                    return Task::none();
                }
                j.jgen += 1;
                j.busy = false;
                j.committing = None;
                self.job_verify(sid, repo)
            }
            JobAction::Recheck => self.footer_open_recheck(sid, repo),
            JobAction::Open(url) => self.footer_open_url(sid, &url),
        }
    }

    fn footer_job_event(
        &mut self,
        sid: &str,
        repo: &str,
        jgen: u64,
        ev: JobEvent,
    ) -> Task<Message> {
        let name = self.footer_repo_name(sid, repo);
        let client = self.online();
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        if jgen != j.jgen {
            return Task::none();
        }
        j.busy = false;
        match ev {
            JobEvent::Prepared(res) => match res {
                Ok(_) => self.job_create(sid, repo, None, None),
                Err(e) => {
                    self.job_end(
                        sid,
                        repo,
                        Tone::Error,
                        tr::foot_job_prepare_failed(&name),
                        i18n::proto_error(&e),
                        vec![JobAction::Retry],
                    );
                    Task::none()
                }
            },
            JobEvent::Asked(res) => match res {
                Ok((path, before)) => {
                    crate::script::log(&format!("footer: PR prompt sent, file {path}"));
                    j.pr_file = Some(path);
                    if j.before.is_none() {
                        j.before = before;
                        j.allowed = match (&j.before, j.files_included) {
                            (Some((_, files)), true) => files.clone(),
                            _ => Vec::new(),
                        };
                    }
                    Task::none()
                }
                Err(e) => {
                    self.job_end(
                        sid,
                        repo,
                        Tone::Error,
                        tr::foot_job_send_failed(&name),
                        i18n::proto_error(&e),
                        vec![JobAction::Retry],
                    );
                    Task::none()
                }
            },
            JobEvent::Leftover(res) => {
                let file = res
                    .ok()
                    .and_then(|l| l.file)
                    .filter(|f| f.written_at + 1 >= j.asked_at);
                match file {
                    // Written: go on once it stopped changing (one more poll).
                    Some(f) if j.seen == Some(f.written_at) => self.job_verify(sid, repo),
                    Some(f) => {
                        j.seen = Some(f.written_at);
                        Task::none()
                    }
                    None => Task::none(),
                }
            }
            JobEvent::Drift(res) => {
                let drift = match res {
                    Ok(d) => d,
                    Err(e) => {
                        self.job_end(
                            sid,
                            repo,
                            Tone::Error,
                            tr::foot_job_check_failed(&name),
                            i18n::proto_error(&e),
                            vec![JobAction::Retry],
                        );
                        return Task::none();
                    }
                };
                let (head, mut files) = j.before.clone().unwrap_or_default();
                files.extend(j.ignored.iter().cloned());
                match meninx_client::drift_change(&drift, &head, &files, j.files_included) {
                    None => self.job_create(sid, repo, None, None),
                    Some(change) => {
                        let mut parts = Vec::new();
                        if change.rewritten {
                            parts.push(tr::foot_drift_rewritten());
                        }
                        if !change.commits.is_empty() {
                            parts.push(tr::foot_drift_commits(i18n::count(change.commits.len())));
                        }
                        if !change.files.is_empty() {
                            parts.push(tr::foot_drift_files(i18n::count(change.files.len())));
                        }
                        crate::script::log(&format!("footer: PR job needs a recheck: {change:?}"));
                        j.stage = JobStage::Reviewing(change);
                        j.allowed.clear();
                        j.set(
                            Tone::Warning,
                            tr::foot_job_recheck_title(&name),
                            tr::foot_job_recheck_body(&parts.join(" · ")),
                            vec![JobAction::Recheck, JobAction::Cancel],
                        );
                        // The checked head is pushed when the user takes everything.
                        j.before =
                            Some((drift.head, j.before.take().map(|b| b.1).unwrap_or_default()));
                        Task::none()
                    }
                }
            }
            JobEvent::CommitAsked(res) => match res {
                Ok(head) => {
                    if let Some(b) = j.before.as_mut() {
                        b.0 = head;
                    }
                    Task::none()
                }
                Err(e) => {
                    self.job_end(
                        sid,
                        repo,
                        Tone::Error,
                        tr::foot_job_commit_send_failed(&name),
                        i18n::proto_error(&e),
                        Vec::new(),
                    );
                    Task::none()
                }
            },
            JobEvent::CommitDrift(res) => {
                let Ok(d) = res else {
                    return Task::none();
                };
                let left = j
                    .committing
                    .as_ref()
                    .is_some_and(|want| d.files.iter().any(|f| want.contains(&f.path)));
                if left {
                    return Task::none();
                }
                j.committing = None;
                self.job_verify(sid, repo)
            }
            JobEvent::Created(res) => {
                let res = match res {
                    Ok(r) => r,
                    Err(e) => {
                        self.job_end(
                            sid,
                            repo,
                            Tone::Error,
                            tr::foot_job_create_failed(&name),
                            i18n::proto_error(&e),
                            vec![JobAction::Retry],
                        );
                        return Task::none();
                    }
                };
                crate::script::log(&format!("footer: PR created {res:?}"));
                let merge = j.merge;
                match res.kind {
                    PrResultKind::Page | PrResultKind::Failed => {
                        let reason = res
                            .reason
                            .as_ref()
                            .map(i18n::proto_error)
                            .unwrap_or_default();
                        let page = res.kind == PrResultKind::Page;
                        let msg = if merge.is_some() {
                            tr::foot_job_page_hint_no_merge(&reason)
                        } else {
                            tr::foot_job_page_hint(&reason)
                        };
                        self.job_end(
                            sid,
                            repo,
                            if page { Tone::Warning } else { Tone::Error },
                            if page {
                                tr::foot_job_page_title(&name)
                            } else {
                                tr::foot_job_create_failed(&name)
                            },
                            msg,
                            vec![JobAction::Open(res.url.clone())],
                        );
                        Task::none()
                    }
                    PrResultKind::Created | PrResultKind::Exists => {
                        let open = self.footer_open_url(sid, &res.url);
                        if let (Some((method, delete)), PrResultKind::Created, Some(n)) =
                            (merge, res.kind, res.number)
                        {
                            let j = self.job_mut(sid, repo).expect("job");
                            j.stage = JobStage::Merging;
                            j.waiting = Some(MergeWait {
                                number: n,
                                method,
                                delete,
                                started: Instant::now(),
                                last_poll: None,
                                gate_off: false,
                                blocked_since: None,
                                in_flight: false,
                            });
                            j.set(
                                Tone::Info,
                                tr::foot_job_merge_wait_title(&name, &n.to_string()),
                                tr::foot_job_merge_wait_first(),
                                vec![JobAction::Open(res.url.clone()), JobAction::Cancel],
                            );
                            let pr = self.footer_pr(sid, repo);
                            return Task::batch([open, pr]);
                        }
                        let Some(c) = client else {
                            return open;
                        };
                        let (s, r, repo_s) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
                        let status =
                            perform(sid, async move { c.pr_status(&s, &r).await }, move |st| {
                                FooterMsg::Job(repo_s, jgen, JobEvent::CreatedStatus(res, st))
                            });
                        let j = self.job_mut(sid, repo).expect("job");
                        j.busy = true;
                        Task::batch([open, status])
                    }
                }
            }
            JobEvent::CreatedStatus(res, st) => {
                let n = res.number.map(|n| n.to_string()).unwrap_or_default();
                let matched = st
                    .ok()
                    .flatten()
                    .filter(|p| Some(p.number) == res.number && p.state == PrState::Open);
                let merge = j.merge.is_some();
                if let Some(pr) = matched.as_ref().filter(|p| p.mergeable_state == "dirty") {
                    let base = pr.base_ref.clone();
                    self.job_end(
                        sid,
                        repo,
                        Tone::Warning,
                        tr::foot_job_conflict_title(&name, &n),
                        tr::foot_job_conflict_body(&base),
                        vec![JobAction::Open(res.url.clone())],
                    );
                } else {
                    let unknown = matched
                        .as_ref()
                        .is_none_or(|p| p.mergeable.is_none() || p.mergeable_state == "unknown");
                    let mut parts = vec![res.title.clone()];
                    if unknown {
                        parts.push(tr::foot_job_mergeable_unknown());
                    }
                    let exists = res.kind == PrResultKind::Exists;
                    if exists && merge {
                        parts.push(tr::foot_job_exists_no_merge());
                    }
                    self.job_end(
                        sid,
                        repo,
                        if exists && merge {
                            Tone::Warning
                        } else {
                            Tone::Success
                        },
                        if exists {
                            tr::foot_job_exists(&name, &n)
                        } else {
                            tr::foot_job_created(&name, &n)
                        },
                        parts.join(" · "),
                        vec![JobAction::Open(res.url.clone())],
                    );
                }
                self.footer_pr(sid, repo)
            }
            JobEvent::MergePoll(pr, gate) => {
                let Some(w) = j.waiting.as_mut() else {
                    return Task::none();
                };
                w.in_flight = false;
                let gate = match gate {
                    Some(Ok(g)) => Some(g),
                    Some(Err(_)) => {
                        w.gate_off = true;
                        None
                    }
                    None => None,
                };
                let pr = pr.ok().flatten();
                let blocked = pr.as_ref().is_some_and(|p| p.mergeable_state == "blocked");
                if blocked {
                    w.blocked_since.get_or_insert_with(Instant::now);
                } else {
                    w.blocked_since = None;
                }
                let blocked_for = w.blocked_since.map_or(0, |t| t.elapsed().as_secs());
                let target = j.target.clone();
                let step = meninx_client::merge_after_create_step(
                    pr.as_ref(),
                    w.number,
                    gate.as_ref(),
                    blocked_for,
                    Some(&target),
                );
                let (n, method, delete, started) = (w.number, w.method, w.delete, w.started);
                let url = pr.as_ref().map(|p| p.url.clone()).unwrap_or_default();
                match step {
                    MergeStep::Wait { checks } => {
                        if started.elapsed().as_secs() > meninx_client::MERGE_MAX_WAIT_SECS {
                            self.job_end(
                                sid,
                                repo,
                                Tone::Warning,
                                tr::foot_job_not_merged(&name, &n.to_string()),
                                tr::foot_job_merge_timeout(),
                                Vec::new(),
                            );
                            return Task::none();
                        }
                        let waited = started.elapsed().as_secs() / 60;
                        j.message = match checks {
                            Some((done, total, pending)) => tr::foot_job_merge_wait_checks(
                                &format!("{done}/{total}"),
                                &pending.join(", "),
                                &waited.to_string(),
                            ),
                            None => tr::foot_job_merge_wait(&waited.to_string()),
                        };
                        Task::none()
                    }
                    MergeStep::Stop(stop) => {
                        self.job_end(
                            sid,
                            repo,
                            Tone::Warning,
                            tr::foot_job_not_merged(&name, &n.to_string()),
                            stop_text(&stop),
                            if url.is_empty() {
                                Vec::new()
                            } else {
                                vec![JobAction::Open(url)]
                            },
                        );
                        Task::none()
                    }
                    MergeStep::Merge { sha } => {
                        let Some(c) = client else {
                            return Task::none();
                        };
                        j.busy = true;
                        j.message = tr::foot_job_merging();
                        let (s, r, repo_s) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
                        perform(
                            sid,
                            async move { c.pr_merge(&s, &r, n, &sha, method, delete).await },
                            move |res| FooterMsg::Job(repo_s, jgen, JobEvent::MergeDone(res)),
                        )
                    }
                }
            }
            JobEvent::MergeDone(res) => {
                let n = j
                    .waiting
                    .as_ref()
                    .map(|w| w.number)
                    .unwrap_or_default()
                    .to_string();
                match res {
                    Ok(m) => self.job_end(
                        sid,
                        repo,
                        Tone::Success,
                        tr::foot_job_merged(&name, &n),
                        if m.branch_deleted {
                            tr::foot_job_merged_deleted()
                        } else {
                            String::new()
                        },
                        Vec::new(),
                    ),
                    Err(e) => self.job_end(
                        sid,
                        repo,
                        Tone::Error,
                        tr::foot_job_not_merged(&name, &n),
                        i18n::proto_error(&e),
                        Vec::new(),
                    ),
                }
                let pr = self.footer_pr(sid, repo);
                Task::batch([pr, self.footer_sync(sid, repo, true)])
            }
        }
    }

    fn footer_open_recheck(&mut self, sid: &str, repo: &str) -> Task<Message> {
        if let Some(d) = self.detail_mut(sid) {
            d.footer.menu = None;
        }
        let Some(j) = self.footer(sid, repo).and_then(|f| f.job.as_ref()) else {
            return Task::none();
        };
        let JobStage::Reviewing(change) = &j.stage else {
            return Task::none();
        };
        self.modal = Some(Modal::Footer(Box::new(FooterModal::Recheck(RecheckForm {
            session: sid.to_owned(),
            repo: repo.to_owned(),
            change: change.clone(),
            commits_on: true,
            files_on: false,
            merge: j.merge.map(|_| true),
            push_off: !j.push,
        }))));
        Task::none()
    }

    fn footer_recheck_msg(&mut self, sid: &str, m: FooterMsg) -> Task<Message> {
        let Some(FooterModal::Recheck(f)) = self.footer_modal() else {
            return Task::none();
        };
        match m {
            FooterMsg::RecheckCommits(v) => f.commits_on = v,
            FooterMsg::RecheckFiles(v) => f.files_on = v,
            FooterMsg::RecheckMerge(v) => f.merge = Some(v),
            FooterMsg::RecheckCancel => {
                let repo = f.repo.clone();
                self.modal = None;
                return self.footer_job_action(sid, &repo, JobAction::Cancel);
            }
            FooterMsg::RecheckResolve => {
                let f = f.clone();
                self.modal = None;
                return self.footer_recheck_resolve(sid, &f);
            }
            _ => {}
        }
        Task::none()
    }

    /// Desk's recheck `resolve`.
    fn footer_recheck_resolve(&mut self, sid: &str, f: &RecheckForm) -> Task<Message> {
        let repo = f.repo.as_str();
        let Some(j) = self.job_mut(sid, repo) else {
            return Task::none();
        };
        if !matches!(j.stage, JobStage::Reviewing(_)) {
            return Task::none();
        }
        if f.merge == Some(false) {
            j.merge = None;
        }
        let change = f.change.clone();
        if !f.commits_in() && !change.rewritten {
            // Up to the last commit asked for; the rest stays local (and so do the files).
            return if j.push {
                self.job_create(sid, repo, None, Some(change.keep_up_to.clone()))
            } else {
                self.job_create(sid, repo, Some(false), None)
            };
        }
        if !f.files_in() || change.files.is_empty() {
            j.ignored
                .extend(change.files.iter().map(|x| x.path.clone()));
            let head = j.before.as_ref().map(|b| b.0.clone());
            return self.job_create(sid, repo, Some(true), head);
        }
        // The session commits the included files first, then the check runs again.
        let Some(c) = self.online() else {
            return Task::none();
        };
        let name = self.footer_repo_name(sid, repo);
        let j = self.job_mut(sid, repo).expect("job");
        let files: Vec<String> = change.files.iter().map(|x| x.path.clone()).collect();
        j.stage = JobStage::Writing;
        j.allowed = files.clone();
        j.files_included = true;
        j.committing = Some(files.clone());
        j.busy = true;
        j.set(
            Tone::Info,
            tr::foot_job_title(&name),
            tr::foot_job_committing(i18n::count(files.len())),
            vec![JobAction::CreateNow, JobAction::Cancel],
        );
        let jgen = j.jgen;
        let (s, r, repo_s) = (sid.to_owned(), repo.to_owned(), repo.to_owned());
        perform(
            sid,
            async move {
                let now = c.pr_drift(&s, &r, None, Vec::new()).await?;
                let text = c.prompt_commit(&s, &r, files, false).await?;
                c.send_prompt(&s, &text).await?;
                Ok(now.head)
            },
            move |res| FooterMsg::Job(repo_s, jgen, JobEvent::CommitAsked(res)),
        )
    }

    // ------------------------------------------------------------------ 새 브랜치로 이어가기

    fn footer_open_continue(&mut self, sid: &str, repo: &str) -> Task<Message> {
        if let Some(d) = self.detail_mut(sid) {
            d.footer.menu = None;
        }
        if repo.is_empty() {
            return Task::none();
        }
        let Some(c) = self.online() else {
            return Task::none();
        };
        let open_pr = self
            .footer(sid, repo)
            .and_then(RepoFooter::open_pr)
            .map(|p| (p.number, p.title.clone()));
        self.modal = Some(Modal::Footer(Box::new(FooterModal::Continue(
            ContinueForm {
                session: sid.to_owned(),
                repo: repo.to_owned(),
                info: Load::Loading,
                name: String::new(),
                picked: Vec::new(),
                move_files: true,
                open_pr,
                close_pr: true,
                check: None,
                check_seq: 0,
                running: false,
                result: None,
            },
        ))));
        let (s, r) = (sid.to_owned(), repo.to_owned());
        perform(
            sid,
            async move { c.git_branch_refresh_info(&s, &r, true).await },
            FooterMsg::ContinueInfo,
        )
    }

    /// Dry-run the picks (Desk: on every pick change).
    fn continue_check(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(FooterModal::Continue(f)) = self.footer_modal() else {
            return Task::none();
        };
        if f.info.ready().is_none() {
            return Task::none();
        }
        f.check_seq += 1;
        f.check = Some(Load::Loading);
        let (s, r, picks, files, seq) = (
            f.session.clone(),
            f.repo.clone(),
            f.picks(),
            f.with_files(),
            f.check_seq,
        );
        perform(
            sid,
            async move { c.git_branch_refresh_check(&s, &r, picks, files).await },
            move |res| FooterMsg::ContinueChecked(seq, res),
        )
    }

    fn footer_continue_msg(&mut self, sid: &str, m: FooterMsg) -> Task<Message> {
        let Some(FooterModal::Continue(f)) = self.footer_modal() else {
            return Task::none();
        };
        match m {
            FooterMsg::ContinueInfo(res) => {
                if let Ok(i) = &res {
                    // Desk `defaultPicks`: what the base does not have yet.
                    f.picked = i
                        .commits
                        .iter()
                        .filter(|c| !c.in_base)
                        .map(|c| c.full.clone())
                        .collect();
                }
                f.info = load(res);
                self.continue_check(sid)
            }
            FooterMsg::ContinueName(n) => {
                f.name = n;
                Task::none()
            }
            FooterMsg::ContinuePick(full) => {
                if let Some(i) = f.picked.iter().position(|x| *x == full) {
                    f.picked.remove(i);
                } else {
                    f.picked.push(full);
                }
                self.continue_check(sid)
            }
            FooterMsg::ContinueAll => {
                let all: Vec<String> = f
                    .info
                    .ready()
                    .map(|i| i.commits.iter().map(|c| c.full.clone()).collect())
                    .unwrap_or_default();
                f.picked = if f.picked.len() == all.len() {
                    Vec::new()
                } else {
                    all
                };
                self.continue_check(sid)
            }
            FooterMsg::ContinueMoveFiles(v) => {
                f.move_files = v;
                self.continue_check(sid)
            }
            FooterMsg::ContinueClosePr(v) => {
                f.close_pr = v;
                Task::none()
            }
            FooterMsg::ContinueChecked(seq, res) => {
                if seq == f.check_seq {
                    f.check = Some(load(res));
                }
                Task::none()
            }
            FooterMsg::ContinueRun => {
                if !f.can_run() {
                    return Task::none();
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(FooterModal::Continue(f)) = self.footer_modal() else {
                    return Task::none();
                };
                f.running = true;
                let opts = BranchRefreshOptions {
                    name: Some(f.name.trim().to_owned()).filter(|n| !n.is_empty()),
                    commits: f.picks(),
                    move_files: f.move_files,
                };
                let close = f.open_pr.as_ref().filter(|_| f.close_pr).map(|p| p.0);
                let (s, r) = (f.session.clone(), f.repo.clone());
                crate::script::log(&format!(
                    "footer: 새 브랜치로 이어가기 {opts:?} close {close:?}"
                ));
                perform(
                    sid,
                    async move {
                        let res = c.git_refresh_branch(&s, &r, opts).await;
                        // The old PR is closed by the PR backend (Desk `closePr`; no comment).
                        let closed = match (&res, close) {
                            (Ok(_), Some(n)) => Some(c.pr_close(&s, &r, n).await),
                            _ => None,
                        };
                        (res, closed)
                    },
                    |(res, closed)| FooterMsg::ContinueDone(res, closed),
                )
            }
            FooterMsg::ContinueDone(res, closed) => {
                f.running = false;
                let repo = f.repo.clone();
                match res {
                    Ok(r) => {
                        f.result = Some(result_lines(&r, closed.as_ref()));
                        let a = self.footer_pr(sid, &repo);
                        Task::batch([a, self.footer_sync(sid, &repo, false)])
                    }
                    Err(e) => {
                        let msg = tr::foot_continue_failed(&i18n::proto_error(&e));
                        self.footer_toast(sid, crate::notices::Tone::Error, msg.clone());
                        if let Some(FooterModal::Continue(f)) = self.footer_modal() {
                            f.check = Some(Load::Failed(msg));
                        }
                        Task::none()
                    }
                }
            }
            _ => Task::none(),
        }
    }
}

/// Desk `resultLines`.
pub fn result_lines(
    r: &BranchRefreshResult,
    closed: Option<&Result<PrClosed, ProtoError>>,
) -> Vec<String> {
    let mut out = vec![tr::foot_continue_result_branch(&r.branch)];
    if r.moved_commits > 0 {
        out.push(tr::foot_continue_result_moved(i64::from(r.moved_commits)));
    }
    if r.skipped_commits > 0 {
        out.push(tr::foot_continue_result_skipped(i64::from(
            r.skipped_commits,
        )));
    }
    if r.left_commits > 0 {
        out.push(tr::foot_continue_result_left(
            i64::from(r.left_commits),
            &r.previous,
        ));
    }
    match closed {
        Some(Ok(c)) => out.push(tr::foot_continue_result_closed(&c.number.to_string())),
        Some(Err(e)) => out.push(tr::foot_continue_result_not_closed(&i18n::proto_error(e))),
        None => {}
    }
    if r.files_left_in_stash {
        out.push(tr::foot_continue_result_files_stash(
            i64::from(r.stashed_files),
            &r.previous,
        ));
    } else {
        if r.moved_files > 0 {
            out.push(tr::foot_continue_result_files_moved(i64::from(
                r.moved_files,
            )));
        }
        if r.stashed_files > 0 {
            out.push(tr::foot_continue_result_files_stashed(
                i64::from(r.stashed_files),
                &r.previous,
            ));
        }
    }
    out
}

/// Server error ids of the PR backend and prompt delivery (`gh-*`, `github-*`, `pr-*`,
/// `prompt-*`).
pub fn proto_error_text(e: &ProtoError) -> Option<String> {
    let a = |name: &str| e.get(name).unwrap_or("").to_owned();
    Some(match e.id.as_str() {
        "gh-missing" => tr::err_gh_missing(&a("program")),
        "gh-auth" => tr::err_gh_auth(&a("host")),
        "gh-failed" => tr::err_gh_failed(&a("detail")),
        "github-no-credentials" => tr::err_github_no_credentials(&a("host")),
        "github-auth" => tr::err_github_auth(&a("host")),
        "github-failed" => tr::err_github_failed(&a("detail")),
        "pr-no-origin" => tr::err_pr_no_origin(),
        "pr-origin-unparsable" => tr::err_pr_origin_unparsable(&a("url")),
        "pr-gitlab" => tr::err_pr_gitlab(),
        "pr-gitlab-page" => tr::err_pr_gitlab_page(),
        "pr-detached" => tr::err_pr_detached(),
        "pr-same-branch" => tr::err_pr_same_branch(&a("branch")),
        "pr-no-remote-branch" => tr::err_pr_no_remote_branch(&a("branch")),
        "pr-bad-branch" => tr::err_pr_bad_branch(&a("branch")),
        "pr-bad-number" => tr::err_pr_bad_number(),
        "pr-bad-sha" => tr::err_pr_bad_sha(&a("sha")),
        "pr-not-on-branch" => tr::err_pr_not_on_branch(&a("sha")),
        "pr-changed" => tr::err_pr_changed(),
        "pr-no-text" => tr::err_pr_no_text(),
        "pr-empty-title" => tr::err_pr_empty_title(),
        "pr-bad-title" => tr::err_pr_bad_title(),
        "pr-no-commits" => tr::err_pr_no_commits(&a("base")),
        "pr-needs-workspace" => tr::err_pr_needs_workspace(),
        "pr-api" => tr::err_pr_api(&a("status"), &a("detail")),
        "pr-merge-refused" => tr::err_pr_merge_refused(&a("status"), &a("detail")),
        "pr-close-failed" => tr::err_pr_close_failed(&a("status"), &a("detail")),
        "pr-update-refused" => tr::err_pr_update_refused(&a("status"), &a("detail")),
        "pr-already-merged" => tr::err_pr_already_merged(&a("number")),
        "pr-not-open" => tr::err_pr_not_open(&a("number")),
        "pr-head-moved" => tr::err_pr_head_moved(&a("number"), &a("expected"), &a("actual")),
        "pr-push-failed" => tr::err_pr_push_failed(&a("detail")),
        "pr-create-unconfirmed" => tr::err_pr_create_unconfirmed(&a("detail")),
        "prompt-empty" => tr::err_prompt_empty(),
        "prompt-not-ready" => tr::err_prompt_not_ready(&a("id")),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(ahead: u32, behind: u32, unpublished: bool) -> SyncStatus {
        SyncStatus {
            ahead,
            behind,
            no_remote: false,
            upstream: (!unpublished).then(|| "origin/x".into()),
            unpublished,
            gone: false,
        }
    }

    #[test]
    fn primary_follows_desk() {
        assert_eq!(primary_op(None, false), SyncOp::Fetch);
        assert_eq!(primary_op(Some(&sync(2, 0, true)), false), SyncOp::Publish);
        assert_eq!(primary_op(Some(&sync(2, 0, true)), true), SyncOp::Fetch);
        assert_eq!(primary_op(Some(&sync(2, 1, false)), false), SyncOp::Pull);
        assert_eq!(primary_op(Some(&sync(2, 0, false)), false), SyncOp::Push);
        assert_eq!(primary_op(Some(&sync(2, 0, false)), true), SyncOp::Fetch);
        assert_eq!(primary_op(Some(&sync(0, 0, false)), false), SyncOp::Fetch);
    }

    #[test]
    fn branch_names() {
        for ok in ["", "  ", "feature/x", "hermes/a-2", "한글"] {
            assert!(branch_name_ok(ok), "{ok:?}");
        }
        for bad in [
            "-x", "/x", "x/", "x.", "x.lock", "a..b", "a//b", "a@{b", "@", "a b", "a~b", "a^b",
            "a:b", "a?b", "a*b", "a[b", "a\\b", "a/.b", ".a",
        ] {
            assert!(!branch_name_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn every_pr_error_id_has_a_text() {
        for id in [
            "gh-missing",
            "gh-auth",
            "gh-failed",
            "github-no-credentials",
            "github-auth",
            "github-failed",
            "pr-no-origin",
            "pr-origin-unparsable",
            "pr-gitlab",
            "pr-gitlab-page",
            "pr-detached",
            "pr-same-branch",
            "pr-no-remote-branch",
            "pr-bad-branch",
            "pr-bad-number",
            "pr-bad-sha",
            "pr-not-on-branch",
            "pr-changed",
            "pr-no-text",
            "pr-empty-title",
            "pr-bad-title",
            "pr-no-commits",
            "pr-needs-workspace",
            "pr-api",
            "pr-merge-refused",
            "pr-close-failed",
            "pr-update-refused",
            "pr-already-merged",
            "pr-not-open",
            "pr-head-moved",
            "pr-push-failed",
            "pr-create-unconfirmed",
            "prompt-empty",
            "prompt-not-ready",
        ] {
            let t = proto_error_text(&ProtoError::new(id).arg("number", "7"));
            assert!(t.is_some_and(|t| !t.is_empty()), "{id}");
        }
        assert!(proto_error_text(&ProtoError::new("git-busy")).is_none());
    }
}
