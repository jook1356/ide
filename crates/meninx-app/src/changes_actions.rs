//! Hermes Desk's Changes / History tab actions (`ChangesPanel.tsx`: file checkboxes, select-all
//! and the 임시 보관 (stash) button + popover, the row context menu with the 변경 취소 modal,
//! the reveal-in-Explore row buttons; `StashSection.tsx`; `CommitContextMenu.tsx`;
//! `BranchSelect.tsx`) on the local git actions backend (`docs/changes-actions.md`, "Local git
//! actions"). State: [`ChangesActs`] (`Detail::acts`); messages: [`ActMsg`]
//! (`DetailMsg::Acts`); dialogs: [`ChangesModal`] (`Modal::Changes`). Drawing is in
//! `changes_view.rs`. The footer (커밋 맡기기, sync, PR menu) is not part of this module.
//!
//! # API for the Changes / History footer
//!
//! Everything is about the repo the side panel shows (`Detail::repo`):
//! - `Detail::checked_changes(&self) -> Vec<meninx_client::GitChange>`: the checked changed files,
//!   one per path, in list order (Desk's `checkedFiles`); a rename carries `orig_path`.
//! - `Detail::changed_count(&self) -> usize`: changed files (Desk's `snapshot.files.length`).
//! - `Detail::all_checked(&self) -> bool`: every changed file is checked (`prompt_commit`'s
//!   `all`); `false` when nothing changed.
//! - `Detail::git_status(&self) -> Option<&meninx_client::GitStatus>`: the last `git.status`
//!   (branch, upstream, ...).
//! - `App::refresh_repo(&mut self, sid: &str) -> Task<Message>`: Desk's `refresh()` after a git
//!   action (status, History, stashes and the open branch list).

use crate::notices::{Text, Tone};
use std::collections::{BTreeSet, HashMap};

use iced::Task;
use meninx_client::{
    BranchCheckout, ChangeKind, CommitAction, CommitInfo, GitBranches, GitChange, GitStatus,
    ProtoError, RemoteFetchStatus, StashAction, StashActionResult, StashEntry, SwitchMode,
};

use crate::app::{App, Message, Modal};
use crate::detail::{Detail, DetailMsg, SideTab, qualify};
use crate::i18n::{self, tr};

/// Widget ids of the inputs (focused when shown).
pub const STASH_MEMO_ID: &str = "meninx-stash-memo";
pub const BRANCH_QUERY_ID: &str = "meninx-branch-query";
pub const MODAL_INPUT_ID: &str = "meninx-changes-modal-input";
/// The 새 브랜치 dialog's base search input.
pub const BASE_QUERY_ID: &str = "meninx-changes-base-query";
/// Rows per branch group (Desk's `MAX_ROWS`); a long list is narrowed with the search box.
pub const MAX_ROWS: usize = 200;

/// The 임시 보관 button's popover (memo) and the stash list (`StashSection`).
pub struct StashUi {
    /// The popover is open.
    pub open: bool,
    pub memo: String,
    /// `git.stash_push` in flight.
    pub busy: bool,
    /// Every stash of the repository (`git.stash_list`); `None` until loaded.
    pub list: Option<Vec<StashEntry>>,
    seq: u64,
    /// The section is unfolded (Desk: open by default).
    pub section_open: bool,
    /// Every branch's stashes, not only this branch's.
    pub show_all: bool,
    /// Stashes with their file list shown.
    pub expanded: BTreeSet<String>,
    /// The stash action in flight.
    pub acting: Option<(String, StashAction)>,
}

impl Default for StashUi {
    fn default() -> Self {
        Self {
            open: false,
            memo: String::new(),
            busy: false,
            list: None,
            seq: 0,
            section_open: true,
            show_all: false,
            expanded: BTreeSet::new(),
            acting: None,
        }
    }
}

/// The `repo / branch` picker (`BranchSelect`).
#[derive(Default)]
pub struct BranchUi {
    pub open: bool,
    pub query: String,
    /// `git.branches` of the shown repo.
    pub list: Option<GitBranches>,
    /// `git.fetch_remotes` in flight.
    pub fetching: bool,
    /// The last background fetch failed (git's text, shown as the note's tooltip).
    pub fetch_failed: Option<String>,
}

/// Changes / History action state of one session's side panel (reset with the shown repo).
#[derive(Default)]
pub struct ChangesActs {
    /// The last `git.status` of the shown repo.
    pub status: Option<GitStatus>,
    /// Desk's `snapshot.files`: one entry per changed path, in list order.
    pub files: Vec<GitChange>,
    /// Paths the user unticked (Desk: every file starts checked).
    pub unchecked: BTreeSet<String>,
    /// The changed-file row whose context menu is open (a path can be listed as staged and
    /// unstaged).
    pub menu: Option<(meninx_views::ChangeGroup, String)>,
    pub stash: StashUi,
    /// The History commit (sha) whose context menu is open.
    pub commit_menu: Option<String>,
    pub branch: BranchUi,
}

impl ChangesActs {
    /// A new `git.status` of the shown repo.
    pub fn on_status(&mut self, s: &GitStatus) {
        self.files = snapshot_files(s);
        let paths: BTreeSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        self.unchecked.retain(|p| paths.contains(p.as_str()));
        if self
            .menu
            .as_ref()
            .is_some_and(|(_, m)| !paths.contains(m.as_str()))
        {
            self.menu = None;
        }
        self.status = Some(s.clone());
    }

    /// What is kept while the repo is hidden (`repo_cache.rs`): the status, files, checkboxes
    /// and stash list; open menus, popovers and actions in flight are dropped, the branch list
    /// is loaded again when the picker opens.
    pub fn kept(self) -> Self {
        Self {
            status: self.status,
            files: self.files,
            unchecked: self.unchecked,
            menu: None,
            stash: StashUi {
                list: self.stash.list,
                seq: self.stash.seq,
                section_open: self.stash.section_open,
                show_all: self.stash.show_all,
                expanded: self.stash.expanded,
                ..StashUi::default()
            },
            commit_menu: None,
            branch: BranchUi::default(),
        }
    }

    /// The checked-out branch (`None`: detached or not loaded).
    pub fn branch_name(&self) -> Option<&str> {
        self.status.as_ref().and_then(|s| s.branch.as_deref())
    }

    pub fn is_checked(&self, path: &str) -> bool {
        !self.unchecked.contains(path)
    }
}

/// One entry per changed path (Desk's `snapshot.files`): the kind against HEAD decides how
/// `git.discard` undoes it, so a conflict wins over a staged change, a staged change over a
/// working-tree change, and untracked files come last. Order: as the Changes list shows them.
pub fn snapshot_files(s: &GitStatus) -> Vec<GitChange> {
    let untracked = s.untracked.iter().map(|p| GitChange {
        path: p.clone(),
        orig_path: None,
        kind: ChangeKind::Untracked,
    });
    let mut by_path: HashMap<String, GitChange> = HashMap::new();
    for c in s
        .conflicted
        .iter()
        .chain(&s.staged)
        .chain(&s.unstaged)
        .cloned()
        .chain(untracked.clone())
    {
        by_path.entry(c.path.clone()).or_insert(c);
    }
    let mut out = Vec::new();
    for c in s
        .staged
        .iter()
        .chain(&s.conflicted)
        .chain(&s.unstaged)
        .cloned()
        .chain(untracked)
    {
        if let Some(c) = by_path.remove(&c.path) {
            out.push(c);
        }
    }
    out
}

/// Desk's `stashTitle`: the memo, else what made it.
pub fn stash_title(s: &StashEntry) -> String {
    if !s.message.is_empty() {
        s.message.clone()
    } else if s.auto {
        tr::chg_stash_auto()
    } else {
        tr::chg_stash_no_memo()
    }
}

/// The stash paths of a partial 임시 보관: the checked files and both sides of a rename.
pub fn stash_paths(files: &[GitChange]) -> Vec<String> {
    files
        .iter()
        .flat_map(|f| std::iter::once(f.path.clone()).chain(f.orig_path.clone()))
        .collect()
}

/// What an attached commit is sent as: its full id and subject (and the worktree it is in, when
/// the session has several).
pub fn commit_ref(c: &CommitInfo, worktree: Option<&str>) -> String {
    let mut s = format!("commit {} \"{}\"", c.sha, c.subject.trim());
    if let Some(w) = worktree {
        s.push_str(&format!(" in {w}"));
    }
    s
}

/// The commit context menu actions that need a confirmation (`CommitContextMenu` labels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitKind {
    Checkout,
    Branch,
    Revert,
    CherryPick,
}

impl CommitKind {
    pub const ALL: [CommitKind; 4] = [
        CommitKind::Checkout,
        CommitKind::Branch,
        CommitKind::Revert,
        CommitKind::CherryPick,
    ];

    pub fn label(self) -> String {
        match self {
            CommitKind::Checkout => tr::chg_commit_checkout(),
            CommitKind::Branch => tr::chg_commit_branch(),
            CommitKind::Revert => tr::chg_commit_revert(),
            CommitKind::CherryPick => tr::chg_commit_cherry_pick(),
        }
    }

    pub fn description(self) -> String {
        match self {
            CommitKind::Checkout => tr::chg_commit_checkout_desc(),
            CommitKind::Branch => tr::chg_commit_branch_desc(),
            CommitKind::Revert => tr::chg_commit_revert_desc(),
            CommitKind::CherryPick => tr::chg_commit_cherry_pick_desc(),
        }
    }

    fn action(self, name: &str) -> CommitAction {
        match self {
            CommitKind::Checkout => CommitAction::Checkout,
            CommitKind::Branch => CommitAction::Branch {
                name: name.trim().to_owned(),
            },
            CommitKind::Revert => CommitAction::Revert,
            CommitKind::CherryPick => CommitAction::CherryPick,
        }
    }
}

/// What a [`ChangesModal`] asks.
#[derive(Debug, Clone)]
pub enum ModalKind {
    /// 변경 취소 of these files.
    Discard(Vec<GitChange>),
    /// 임시 보관 버리기.
    StashDrop(StashEntry),
    /// A History commit action (`name`: the new branch of [`CommitKind::Branch`]).
    Commit {
        kind: CommitKind,
        commit: CommitInfo,
        name: String,
        /// The worktree (Desk's `대상: {dir}`).
        target: String,
    },
    /// 브랜치 전환 (`remote`: a remote branch to fetch and check out).
    Switch {
        name: String,
        remote: Option<String>,
        from: String,
        dirty: usize,
        mode: SwitchMode,
    },
    /// 새 브랜치.
    Create {
        name: String,
        base: Option<String>,
        dirty: usize,
        mode: SwitchMode,
        /// The base picker's list is open, filtered by this (Desk's searchable `Select`).
        base_open: bool,
        base_query: String,
    },
}

/// A dialog of the Changes / History actions (session + repo it acts on).
#[derive(Debug, Clone)]
pub struct ChangesModal {
    pub session: String,
    pub repo: String,
    pub busy: bool,
    pub error: Option<String>,
    pub kind: ModalKind,
}

/// Messages of the Changes / History actions (`DetailMsg::Acts`).
#[derive(Debug, Clone)]
pub enum ActMsg {
    /// Tick / untick a changed file.
    Check(String, bool),
    /// The select-all checkbox.
    CheckAll(bool),
    /// Desk's `Filter` input above the changed files (only the shown rows change).
    Filter(String),
    /// Open (`Some(row)`) / close the context menu of a changed file.
    Menu(Option<(meninx_views::ChangeGroup, String)>),
    /// Ask to discard the changes of these paths (the modal).
    Discard(Vec<String>),
    /// `(files asked, failures)`.
    Discarded(usize, Vec<(String, ProtoError)>),
    /// Show a path of the shown repo in Explore.
    Reveal(String),
    StashToggle,
    StashClose,
    StashMemo(String),
    StashSubmit,
    /// `(files stashed, result)`.
    StashPushed(usize, Result<(), ProtoError>),
    /// `git.stash_list` of repo `.0` for load `.1`.
    Stashes(String, u64, Result<Vec<StashEntry>, ProtoError>),
    StashSection,
    StashShowAll,
    StashExpand(String),
    /// Pop / apply a stash now; drop asks first.
    StashAct(String, StashAction),
    StashDone(StashAction, Result<StashActionResult, ProtoError>),
    /// Open (`Some(sha)`) / close a History commit's context menu.
    CommitMenu(Option<String>),
    /// Click outside the open row / commit menu.
    CloseMenus,
    /// Copy a commit's sha (`false`) or subject (`true`).
    Copy(String, bool),
    Copied(bool, bool),
    /// Put a commit into the session's input box (입력창에 첨부).
    Attach(String),
    /// A commit action picked in the menu: asks for confirmation.
    CommitPick(String, CommitKind),
    CommitDone(Result<(), ProtoError>),
    /// The `repo / branch` picker.
    BranchToggle,
    BranchClose,
    BranchQuery(String),
    Branches(String, Result<GitBranches, ProtoError>),
    Fetched(String, Result<RemoteFetchStatus, ProtoError>),
    /// Show another repo of the session.
    PickRepo(String),
    /// A local branch (`remote: None`) or a remote one picked: asks how to switch.
    PickBranch(String, Option<String>),
    NewBranch,
    /// Reload status, History, stashes (Desk's 저장소 상태 새로고침).
    Refresh,
    /// Dialog fields and buttons.
    ModalInput(String),
    ModalBase(String),
    /// The base picker's list opened / closed, and its search text.
    ModalBaseOpen(bool),
    ModalBaseQuery(String),
    ModalMode(SwitchMode),
    ModalConfirm,
    Switched(Result<(), ProtoError>),
    Created(Result<(), ProtoError>),
}

fn dm(sid: &str, m: ActMsg) -> Message {
    Message::Detail(sid.to_owned(), DetailMsg::Acts(m))
}

// ---------------------------------------------------------------------------- detail API

impl Detail {
    /// The checked changed files of the shown repo (Desk's `checkedFiles`).
    pub fn checked_changes(&self) -> Vec<GitChange> {
        self.acts
            .files
            .iter()
            .filter(|f| self.acts.is_checked(&f.path))
            .cloned()
            .collect()
    }

    /// Changed files of the shown repo (one per path).
    pub fn changed_count(&self) -> usize {
        self.acts.files.len()
    }

    /// Every changed file is checked (and there is one).
    pub fn all_checked(&self) -> bool {
        !self.acts.files.is_empty()
            && self
                .acts
                .files
                .iter()
                .all(|f| self.acts.is_checked(&f.path))
    }

    /// The last `git.status` of the shown repo.
    #[allow(dead_code, reason = "footer API (docs at the top of this module)")]
    pub fn git_status(&self) -> Option<&GitStatus> {
        self.acts.status.as_ref()
    }
}

// ---------------------------------------------------------------------------- logic

impl App {
    /// Desk's `refresh()`: status, History, stashes and (when open) the branch list.
    pub fn refresh_repo(&mut self, sid: &str) -> Task<Message> {
        let branches = if self.detail(sid).is_some_and(|d| d.acts.branch.open) {
            self.load_branches(sid)
        } else {
            Task::none()
        };
        Task::batch([
            self.load_status(sid),
            self.load_history(sid),
            self.load_stashes(sid),
            branches,
        ])
    }

    /// After a `git.status` answer: the stash list follows the tree like Desk's `useStashes`.
    pub fn after_status(&mut self, sid: &str) -> Task<Message> {
        if self.detail(sid).is_some_and(|d| d.status_error.is_none()) {
            self.load_stashes(sid)
        } else {
            Task::none()
        }
    }

    fn load_stashes(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(d) = self.detail_mut(sid).filter(|d| !d.repo.is_empty()) else {
            return Task::none();
        };
        d.acts.stash.seq += 1;
        let (seq, repo) = (d.acts.stash.seq, d.repo.clone());
        let (s, r, sid) = (sid.to_owned(), repo.clone(), sid.to_owned());
        Task::perform(async move { c.git_stash_list(&s, &r).await }, move |res| {
            dm(&sid, ActMsg::Stashes(repo.clone(), seq, res))
        })
    }

    fn load_branches(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(repo) = self
            .detail(sid)
            .map(|d| d.repo.clone())
            .filter(|r| !r.is_empty())
        else {
            return Task::none();
        };
        let (s, r, sid) = (sid.to_owned(), repo.clone(), sid.to_owned());
        Task::perform(async move { c.git_branches(&s, &r).await }, move |res| {
            dm(&sid, ActMsg::Branches(repo.clone(), res))
        })
    }

    fn toast(&mut self, sid: &str, tone: Tone, m: impl Into<Text>) {
        let m = m.into();
        crate::script::log(&format!("toast {sid}: {}", m.get()));
        if let Some(d) = self.detail_mut(sid) {
            d.toast(tone, m);
        }
    }

    fn changes_modal(&mut self) -> Option<&mut ChangesModal> {
        match &mut self.modal {
            Some(Modal::Changes(f)) => Some(f),
            _ => None,
        }
    }

    pub fn on_changes_act(&mut self, sid: &str, m: ActMsg) -> Task<Message> {
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        let a = &mut d.acts;
        match m {
            ActMsg::Check(path, on) => {
                if on {
                    a.unchecked.remove(&path);
                } else {
                    a.unchecked.insert(path);
                }
                Task::none()
            }
            ActMsg::CheckAll(on) => {
                a.unchecked = if on {
                    BTreeSet::new()
                } else {
                    a.files.iter().map(|f| f.path.clone()).collect()
                };
                Task::none()
            }
            ActMsg::Menu(p) => {
                a.menu = p;
                Task::none()
            }
            ActMsg::Filter(q) => {
                d.changes.set_filter(q);
                Task::none()
            }
            ActMsg::Discard(paths) => {
                a.menu = None;
                let files: Vec<GitChange> = a
                    .files
                    .iter()
                    .filter(|f| paths.contains(&f.path))
                    .cloned()
                    .collect();
                if files.is_empty() {
                    return Task::none();
                }
                let repo = d.repo.clone();
                self.open_items = false;
                self.modal = Some(Modal::Changes(ChangesModal {
                    session: sid.to_owned(),
                    repo,
                    busy: false,
                    error: None,
                    kind: ModalKind::Discard(files),
                }));
                Task::none()
            }
            ActMsg::Discarded(n, failed) => {
                if matches!(self.changes_modal(), Some(f) if matches!(f.kind, ModalKind::Discard(_)))
                {
                    self.modal = None;
                }
                let (tone, msg) = if failed.is_empty() {
                    (Tone::Success, tr::chg_discard_done(i18n::count(n)))
                } else {
                    let lines: Vec<String> = failed
                        .iter()
                        .map(|(p, e)| format!("{p}: {}", i18n::proto_error(e)))
                        .collect();
                    (Tone::Error, tr::chg_discard_failed(&lines.join("\n")))
                };
                self.toast(sid, tone, msg);
                self.refresh_repo(sid)
            }
            ActMsg::Reveal(path) => {
                a.menu = None;
                a.commit_menu = None;
                let q = qualify(&d.repo, &path);
                let t = self.on_detail(sid.to_owned(), DetailMsg::Side(SideTab::Explore));
                Task::batch([t, self.tree_reveal(sid, &q)])
            }
            ActMsg::StashToggle => {
                a.stash.open = !a.stash.open;
                if a.stash.open {
                    iced::widget::operation::focus(STASH_MEMO_ID)
                } else {
                    Task::none()
                }
            }
            ActMsg::StashClose => {
                if !a.stash.busy {
                    a.stash.open = false;
                }
                Task::none()
            }
            ActMsg::StashMemo(v) => {
                a.stash.memo = v;
                Task::none()
            }
            ActMsg::StashSubmit => {
                let checked = d.checked_changes();
                let a = &mut d.acts;
                if a.stash.busy || checked.is_empty() {
                    return Task::none();
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let d = self.detail_mut(sid).expect("session");
                let partial = checked.len() < d.acts.files.len();
                let paths = partial.then(|| stash_paths(&checked));
                let memo = d.acts.stash.memo.trim().to_owned();
                d.acts.stash.busy = true;
                let n = checked.len();
                let (s, r, sid) = (sid.to_owned(), d.repo.clone(), sid.to_owned());
                crate::script::log(&format!("stash push {n} files, paths {paths:?}"));
                Task::perform(
                    async move { c.git_stash_push(&s, &r, &memo, paths).await },
                    move |res| dm(&sid, ActMsg::StashPushed(n, res)),
                )
            }
            ActMsg::StashPushed(n, res) => {
                a.stash.busy = false;
                let (tone, msg) = match res {
                    Ok(()) => {
                        a.stash.open = false;
                        a.stash.memo.clear();
                        (Tone::Success, tr::chg_stash_done(i18n::count(n)))
                    }
                    Err(e) => (Tone::Error, tr::chg_stash_failed(&i18n::proto_error(&e))),
                };
                self.toast(sid, tone, msg);
                self.refresh_repo(sid)
            }
            ActMsg::Stashes(repo, seq, res) => {
                if d.repo != repo {
                    // Hidden now: its kept view lists the stashes again when shown.
                    d.cache.invalidate(&repo, crate::repo_cache::Refresh::FILES);
                    return Task::none();
                }
                if a.stash.seq != seq {
                    return Task::none();
                }
                // Desk's `useStashes`: a failure leaves the list empty.
                a.stash.list = Some(res.unwrap_or_default());
                Task::none()
            }
            ActMsg::StashSection => {
                a.stash.section_open ^= true;
                Task::none()
            }
            ActMsg::StashShowAll => {
                a.stash.show_all ^= true;
                Task::none()
            }
            ActMsg::StashExpand(sha) => {
                if !a.stash.expanded.remove(&sha) {
                    a.stash.expanded.insert(sha);
                }
                Task::none()
            }
            ActMsg::StashAct(sha, action) => {
                if a.stash.acting.is_some() {
                    return Task::none();
                }
                if action == StashAction::Drop {
                    let Some(entry) = a
                        .stash
                        .list
                        .iter()
                        .flatten()
                        .find(|s| s.sha == sha)
                        .cloned()
                    else {
                        return Task::none();
                    };
                    let repo = d.repo.clone();
                    self.modal = Some(Modal::Changes(ChangesModal {
                        session: sid.to_owned(),
                        repo,
                        busy: false,
                        error: None,
                        kind: ModalKind::StashDrop(entry),
                    }));
                    return Task::none();
                }
                self.run_stash_action(sid, sha, action)
            }
            ActMsg::StashDone(action, res) => {
                a.stash.acting = None;
                if matches!(self.changes_modal(), Some(f) if matches!(f.kind, ModalKind::StashDrop(_)))
                {
                    self.modal = None;
                }
                let (tone, msg) = match res {
                    Ok(r) if !r.conflicts.is_empty() => (
                        Tone::Warning,
                        tr::chg_stash_conflicts(i18n::count(r.conflicts.len())),
                    ),
                    Ok(_) => (
                        Tone::Success,
                        match action {
                            StashAction::Pop => tr::chg_stash_popped(),
                            StashAction::Apply => tr::chg_stash_applied(),
                            StashAction::Drop => tr::chg_stash_dropped(),
                        },
                    ),
                    Err(e) => {
                        let e = i18n::proto_error(&e);
                        (
                            Tone::Error,
                            match action {
                                StashAction::Pop => tr::chg_stash_pop_failed(&e),
                                StashAction::Apply => tr::chg_stash_apply_failed(&e),
                                StashAction::Drop => tr::chg_stash_drop_failed(&e),
                            },
                        )
                    }
                };
                self.toast(sid, tone, msg);
                self.refresh_repo(sid)
            }
            ActMsg::CommitMenu(sha) => {
                a.commit_menu = sha;
                Task::none()
            }
            ActMsg::CloseMenus => {
                a.menu = None;
                a.commit_menu = None;
                Task::none()
            }
            ActMsg::Copy(sha, subject) => {
                a.commit_menu = None;
                let Some(c) = d.history.commit(&sha) else {
                    return Task::none();
                };
                let text = if subject {
                    c.subject.clone()
                } else {
                    c.sha.clone()
                };
                let sid = sid.to_owned();
                iced::clipboard::write(text)
                    .map(move |r| dm(&sid, ActMsg::Copied(subject, r.is_ok())))
            }
            ActMsg::Attach(sha) => {
                a.commit_menu = None;
                let Some(c) = d.history.commit(&sha).cloned() else {
                    return Task::none();
                };
                let repo = d.repo.clone();
                // Several worktrees: say which one the commit is in.
                let path = self
                    .live(sid)
                    .filter(|l| l.info.repos.len() > 1)
                    .and_then(|l| l.info.repos.iter().find(|r| r.repo == repo))
                    .map(|r| r.path.clone());
                let label = crate::composer::ref_label(&format!("{} {}", c.short, c.subject));
                let text = commit_ref(&c, path.as_deref());
                self.deliver_ref(sid, &label, &text)
            }
            ActMsg::Copied(subject, ok) => {
                let (tone, msg): (Tone, fn() -> String) = match (ok, subject) {
                    (false, _) => (Tone::Error, tr::chg_copy_failed),
                    (true, false) => (Tone::Success, tr::chg_copied_sha),
                    (true, true) => (Tone::Success, tr::chg_copied_subject),
                };
                self.toast(sid, tone, Text::tr(msg));
                Task::none()
            }
            ActMsg::CommitPick(sha, kind) => {
                a.commit_menu = None;
                let Some(commit) = d.history.commit(&sha).cloned() else {
                    return Task::none();
                };
                let repo = d.repo.clone();
                let target = self
                    .live(sid)
                    .and_then(|l| l.info.repos.iter().find(|r| r.repo == repo))
                    .map(|r| r.path.clone())
                    .unwrap_or_default();
                self.modal = Some(Modal::Changes(ChangesModal {
                    session: sid.to_owned(),
                    repo,
                    busy: false,
                    error: None,
                    kind: ModalKind::Commit {
                        kind,
                        commit,
                        name: String::new(),
                        target,
                    },
                }));
                if kind == CommitKind::Branch {
                    iced::widget::operation::focus(MODAL_INPUT_ID)
                } else {
                    Task::none()
                }
            }
            ActMsg::CommitDone(res) => {
                let ok = res.is_ok();
                if let Some(f) = self.changes_modal() {
                    f.busy = false;
                    match res {
                        Ok(()) => self.modal = None,
                        Err(e) => f.error = Some(i18n::proto_error(&e)),
                    }
                }
                if ok {
                    self.toast(sid, Tone::Success, Text::tr(tr::chg_commit_done));
                }
                // Also after a failure: a conflict may leave an operation in progress.
                self.refresh_repo(sid)
            }
            ActMsg::BranchToggle => {
                a.branch.open = !a.branch.open;
                if !a.branch.open {
                    return Task::none();
                }
                a.branch.query.clear();
                // Desk's `openLoad`: the lists now, the remotes refreshed in the background.
                a.branch.fetching = true;
                let fetch = match self.online() {
                    Some(c) => {
                        let d = self.detail(sid).expect("session");
                        let (s, r, sid2, repo) = (
                            sid.to_owned(),
                            d.repo.clone(),
                            sid.to_owned(),
                            d.repo.clone(),
                        );
                        Task::perform(
                            async move { c.git_fetch_remotes(&s, &r).await },
                            move |res| dm(&sid2, ActMsg::Fetched(repo.clone(), res)),
                        )
                    }
                    None => Task::none(),
                };
                Task::batch([self.load_branches(sid), fetch])
            }
            ActMsg::BranchClose => {
                a.branch.open = false;
                Task::none()
            }
            ActMsg::BranchQuery(q) => {
                a.branch.query = q;
                Task::none()
            }
            ActMsg::Branches(repo, res) => {
                if d.repo != repo {
                    return Task::none();
                }
                match res {
                    Ok(b) => a.branch.list = Some(b),
                    Err(e) => {
                        let msg = i18n::proto_error(&e);
                        self.toast(sid, Tone::Error, msg);
                    }
                }
                Task::none()
            }
            ActMsg::Fetched(repo, res) => {
                if d.repo != repo {
                    return Task::none();
                }
                a.branch.fetching = false;
                match res {
                    Ok(st) => {
                        a.branch.fetch_failed =
                            (!st.ok).then(|| st.error.clone().unwrap_or_default());
                        if st.fetched {
                            return self.load_branches(sid);
                        }
                    }
                    Err(e) => a.branch.fetch_failed = Some(i18n::proto_error(&e)),
                }
                Task::none()
            }
            ActMsg::PickRepo(repo) => {
                a.branch.open = false;
                if d.repo == repo {
                    return Task::none();
                }
                self.on_detail(sid.to_owned(), DetailMsg::PickRepo(repo))
            }
            ActMsg::PickBranch(name, remote) => {
                a.branch.open = false;
                let from = a.branch_name().unwrap_or_default().to_owned();
                if remote.is_none() && Some(name.as_str()) == a.branch_name() {
                    return Task::none();
                }
                let dirty = d.changed_count();
                let repo = d.repo.clone();
                self.modal = Some(Modal::Changes(ChangesModal {
                    session: sid.to_owned(),
                    repo,
                    busy: false,
                    error: None,
                    kind: ModalKind::Switch {
                        name,
                        remote,
                        from,
                        dirty,
                        mode: SwitchMode::Carry,
                    },
                }));
                Task::none()
            }
            ActMsg::NewBranch => {
                a.branch.open = false;
                let from = a.branch_name().unwrap_or_default().to_owned();
                let dirty = d.changed_count();
                let repo = d.repo.clone();
                self.modal = Some(Modal::Changes(ChangesModal {
                    session: sid.to_owned(),
                    repo,
                    busy: false,
                    error: None,
                    kind: ModalKind::Create {
                        name: String::new(),
                        base: (!from.is_empty()).then_some(from),
                        dirty,
                        mode: SwitchMode::Carry,
                        base_open: false,
                        base_query: String::new(),
                    },
                }));
                // The base list comes from `git.branches`.
                Task::batch([
                    self.load_branches(sid),
                    iced::widget::operation::focus(MODAL_INPUT_ID),
                ])
            }
            ActMsg::Refresh => self.refresh_repo(sid),
            ActMsg::ModalInput(v) => {
                if let Some(f) = self.changes_modal() {
                    match &mut f.kind {
                        ModalKind::Commit { name, .. } | ModalKind::Create { name, .. } => {
                            *name = v
                        }
                        _ => {}
                    }
                }
                Task::none()
            }
            ActMsg::ModalBase(v) => {
                if let Some(ChangesModal {
                    kind:
                        ModalKind::Create {
                            base,
                            base_open,
                            base_query,
                            ..
                        },
                    ..
                }) = self.changes_modal()
                {
                    *base = Some(v);
                    *base_open = false;
                    base_query.clear();
                }
                Task::none()
            }
            ActMsg::ModalBaseOpen(open) => {
                if let Some(ChangesModal {
                    kind:
                        ModalKind::Create {
                            base_open,
                            base_query,
                            ..
                        },
                    ..
                }) = self.changes_modal()
                {
                    *base_open = open;
                    base_query.clear();
                }
                if open {
                    iced::widget::operation::focus(BASE_QUERY_ID)
                } else {
                    Task::none()
                }
            }
            ActMsg::ModalBaseQuery(q) => {
                if let Some(ChangesModal {
                    kind: ModalKind::Create { base_query, .. },
                    ..
                }) = self.changes_modal()
                {
                    *base_query = q;
                }
                Task::none()
            }
            ActMsg::ModalMode(m) => {
                if let Some(f) = self.changes_modal() {
                    match &mut f.kind {
                        ModalKind::Switch { mode, .. } | ModalKind::Create { mode, .. } => {
                            *mode = m
                        }
                        _ => {}
                    }
                }
                Task::none()
            }
            ActMsg::ModalConfirm => self.confirm_changes_modal(),
            ActMsg::Switched(res) => {
                if !matches!(
                    self.changes_modal(),
                    Some(ChangesModal {
                        kind: ModalKind::Switch { .. },
                        ..
                    })
                ) {
                    return self.refresh_repo(sid);
                }
                let Some(Modal::Changes(ChangesModal {
                    kind:
                        ModalKind::Switch {
                            name,
                            remote,
                            from,
                            dirty,
                            mode,
                        },
                    ..
                })) = self.modal.take()
                else {
                    return self.refresh_repo(sid);
                };
                let tone = if res.is_ok() { Tone::Success } else { Tone::Error };
                let msg = match (res, &remote) {
                    (Ok(()), _) if mode == SwitchMode::Stash && dirty > 0 => {
                        tr::chg_switched_stash(&from, &name)
                    }
                    (Ok(()), Some(r)) => tr::chg_switched_remote(r, &name),
                    (Ok(()), None) => tr::chg_switched(&name),
                    (Err(e), Some(_)) => tr::chg_switch_remote_failed(&i18n::proto_error(&e)),
                    (Err(e), None) => tr::chg_switch_failed(&i18n::proto_error(&e)),
                };
                self.toast(sid, tone, msg);
                self.refresh_repo(sid)
            }
            ActMsg::Created(res) => {
                match res {
                    Ok(()) => {
                        let msg = match self.changes_modal().map(|f| f.kind.clone()) {
                            Some(ModalKind::Create { name, base, .. }) => {
                                tr::chg_created(&base.unwrap_or_default(), name.trim())
                            }
                            _ => tr::chg_commit_done(),
                        };
                        self.modal = None;
                        self.toast(sid, Tone::Success, msg);
                    }
                    Err(e) => {
                        if let Some(f) = self.changes_modal() {
                            f.busy = false;
                            f.error = Some(tr::chg_create_failed(&i18n::proto_error(&e)));
                        }
                    }
                }
                self.refresh_repo(sid)
            }
        }
    }

    fn run_stash_action(&mut self, sid: &str, sha: String, action: StashAction) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        d.acts.stash.acting = Some((sha.clone(), action));
        let (s, r, sid) = (sid.to_owned(), d.repo.clone(), sid.to_owned());
        crate::script::log(&format!("stash {action:?} {sha}"));
        Task::perform(
            async move { c.git_stash_action(&s, &r, &sha, action).await },
            move |res| dm(&sid, ActMsg::StashDone(action, res)),
        )
    }

    /// The dialog's main button.
    fn confirm_changes_modal(&mut self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(f) = self.changes_modal() else {
            return Task::none();
        };
        if f.busy {
            return Task::none();
        }
        let (s, r) = (f.session.clone(), f.repo.clone());
        let sid = s.clone();
        match f.kind.clone() {
            ModalKind::Discard(files) => {
                f.busy = true;
                let n = files.len();
                crate::script::log(&format!("discard {files:?}"));
                // One file per call like Desk; failures are collected.
                Task::perform(
                    async move {
                        let mut failed = Vec::new();
                        for file in files {
                            let p = file.path.clone();
                            if let Err(e) = c.git_discard(&s, &r, file).await {
                                failed.push((p, e));
                            }
                        }
                        failed
                    },
                    move |failed| dm(&sid, ActMsg::Discarded(n, failed)),
                )
            }
            ModalKind::StashDrop(entry) => {
                f.busy = true;
                self.run_stash_action(&sid, entry.sha, StashAction::Drop)
            }
            ModalKind::Commit {
                kind, commit, name, ..
            } => {
                if kind == CommitKind::Branch && name.trim().is_empty() {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let action = kind.action(&name);
                crate::script::log(&format!("commit action {action:?} {}", commit.sha));
                Task::perform(
                    async move { c.git_commit_action(&s, &r, &commit.sha, action).await },
                    move |res| dm(&sid, ActMsg::CommitDone(res)),
                )
            }
            ModalKind::Switch {
                name,
                remote,
                dirty,
                mode,
                ..
            } => {
                f.busy = true;
                let mode = if dirty > 0 { mode } else { SwitchMode::Carry };
                crate::script::log(&format!("switch {name} {remote:?} {mode:?}"));
                Task::perform(
                    async move { c.git_switch_branch(&s, &r, &name, remote, mode).await },
                    move |res| dm(&sid, ActMsg::Switched(res)),
                )
            }
            ModalKind::Create {
                name,
                base,
                dirty,
                mode,
                ..
            } => {
                let name = name.trim().to_owned();
                let Some(base) = base.filter(|_| !name.is_empty()) else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let mode = if dirty > 0 { mode } else { SwitchMode::Carry };
                crate::script::log(&format!("create branch {name} from {base} {mode:?}"));
                Task::perform(
                    async move { c.git_create_branch(&s, &r, &name, &base, mode).await },
                    move |res| dm(&sid, ActMsg::Created(res)),
                )
            }
        }
    }
}

/// A local branch row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRow {
    pub name: String,
    /// Checked out in another session's worktree: git refuses the switch.
    pub disabled: bool,
    /// Where else it is checked out: `(path, the registered repo folder)`.
    pub holder: Option<(String, bool)>,
}

/// The picker's local branch rows (Desk's `branchList` + `branchHolders` decisions).
pub fn branch_rows(b: &GitBranches) -> Vec<BranchRow> {
    let row = |name: &str, disabled: bool, holder: Option<(String, bool)>| BranchRow {
        name: name.to_owned(),
        disabled,
        holder,
    };
    let mut rows: Vec<BranchRow> = b
        .local
        .iter()
        .map(|l| match &l.checkout {
            BranchCheckout::Worktree(p) => row(&l.name, true, Some((p.clone(), false))),
            BranchCheckout::MainRepo(p) => row(&l.name, false, Some((p.clone(), true))),
            BranchCheckout::Here | BranchCheckout::None => row(&l.name, false, None),
        })
        .collect();
    // Desk: a current branch missing from the list is shown (disabled) on top.
    if let Some(cur) = &b.current
        && !rows.iter().any(|r| r.name == *cur)
    {
        rows.insert(0, row(cur, true, None));
    }
    rows
}

/// The session key of a session workspace path (`<root>/<key>/<repo>`), else `None` (Desk's
/// `sessionOf`: `YYYYMMDD-HHMM-xxxx`).
pub fn session_of(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let parent = parts.len().checked_sub(2).map(|i| parts[i])?;
    let b = parent.as_bytes();
    let ok = b.len() == 18
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'-'
        && b[9..13].iter().all(u8::is_ascii_digit)
        && b[13] == b'-'
        && b[14..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c));
    ok.then(|| parent.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meninx_client::{LocalBranch, RemoteBranch};

    fn ch(path: &str, kind: ChangeKind) -> GitChange {
        GitChange {
            path: path.into(),
            orig_path: None,
            kind,
        }
    }

    fn status() -> GitStatus {
        GitStatus {
            branch: Some("main".into()),
            head: String::new(),
            upstream: None,
            base: None,
            merge_base: None,
            staged: vec![
                ch("a.ts", ChangeKind::Added),
                GitChange {
                    path: "new.ts".into(),
                    orig_path: Some("old.ts".into()),
                    kind: ChangeKind::Renamed,
                },
            ],
            unstaged: vec![
                ch("a.ts", ChangeKind::Modified),
                ch("b.ts", ChangeKind::Modified),
            ],
            untracked: vec!["n.txt".into()],
            conflicted: vec![ch("c.ts", ChangeKind::Conflicted)],
        }
    }

    #[test]
    fn one_file_per_path_with_the_head_kind() {
        let files = snapshot_files(&status());
        let got: Vec<(&str, ChangeKind)> =
            files.iter().map(|f| (f.path.as_str(), f.kind)).collect();
        assert_eq!(
            got,
            [
                ("a.ts", ChangeKind::Added),
                ("new.ts", ChangeKind::Renamed),
                ("c.ts", ChangeKind::Conflicted),
                ("b.ts", ChangeKind::Modified),
                ("n.txt", ChangeKind::Untracked),
            ]
        );
        assert_eq!(
            stash_paths(&files[..2]),
            ["a.ts", "new.ts", "old.ts"].map(String::from)
        );
    }

    #[test]
    fn checks_and_footer_api() {
        let mut d = Detail::new(None, Some("0123abcd"));
        d.acts.on_status(&status());
        assert_eq!(d.changed_count(), 5);
        assert!(d.all_checked());
        d.acts.unchecked.insert("b.ts".into());
        d.acts.unchecked.insert("gone.ts".into());
        assert!(!d.all_checked());
        assert_eq!(d.checked_changes().len(), 4);
        // A new status forgets paths that are no longer changed.
        d.acts.on_status(&status());
        assert_eq!(d.acts.unchecked.len(), 1);
        assert_eq!(
            d.git_status().and_then(|s| s.branch.as_deref()),
            Some("main")
        );
        assert_eq!(d.acts.branch_name(), Some("main"));
    }

    #[test]
    fn branch_rows_and_holders() {
        let b = GitBranches {
            current: Some("feat".into()),
            local: vec![
                LocalBranch {
                    name: "main".into(),
                    checkout: BranchCheckout::MainRepo("C:/r".into()),
                },
                LocalBranch {
                    name: "other".into(),
                    checkout: BranchCheckout::Worktree("C:/w/20260101-1200-abcd/r".into()),
                },
            ],
            remote: vec![RemoteBranch {
                remote: "origin".into(),
                branch: "x".into(),
                short_ref: "origin/x".into(),
                has_local: false,
            }],
        };
        let rows = branch_rows(&b);
        assert_eq!((rows[0].name.as_str(), rows[0].disabled), ("feat", true));
        assert_eq!(rows[1].holder, Some(("C:/r".into(), true)));
        assert!(!rows[1].disabled);
        assert!(rows[2].disabled);
        assert_eq!(
            session_of("C:/w/20260101-1200-abcd/r").as_deref(),
            Some("20260101-1200-abcd")
        );
        assert_eq!(session_of("C:/r"), None);
    }
}
