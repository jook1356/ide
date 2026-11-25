//! App script steps for the Changes / History tab actions (`changes_actions.rs`): checkboxes,
//! partial stash + pop, discard of a modified and an untracked file through the modal, the
//! History commit menu (branch from a commit, revert), branch switching with carried changes
//! and a new branch with stashed changes, plus English and Korean (`ko_chg_*`) screenshots.
//! Runs on the phase 2 session right after the History checks (no network: local branches and
//! stashes only) and leaves its worktree as it found it (same branch, same changes).
//!
//! A child module of `script.rs` (`mod changes`): it uses the script's private helpers.

use std::sync::Mutex;

use meninx_client::{StashAction, SwitchMode};

use super::*;
use crate::changes_actions::{ActMsg, ChangesModal, CommitKind, ModalKind};

/// Fixtures in the phase 2 worktree.
const MODIFIED: &str = HIST_FILE;
const MODIFIED_LINE: &str = "// changed by the changes-actions script\n";
const UNTRACKED: &str = "chg/new.txt";
const STASHED: &str = "chg/stash-me.txt";
const CARRIED: &str = "chg/carry.txt";
const STASH_MEMO: &str = "script stash";
const ALL_MEMO: &str = "script stash all";
const FROM_COMMIT: &str = "chg-from-commit";
const NEW_BRANCH: &str = "chg-new";

/// The branch the phase 2 worktree had before these steps.
static ORIG: Mutex<String> = Mutex::new(String::new());

fn orig() -> String {
    ORIG.lock().unwrap().clone()
}

#[derive(Debug, Clone)]
pub(super) enum CStep {
    /// Write the fixtures (a modified tracked file, two untracked files).
    Fixture,
    /// Write an untracked file into the worktree.
    Write(&'static str),
    /// An action message to the phase 2 session.
    Do(ActMsg),
    /// The checkbox of a changed path.
    ExpectChecked(&'static str, bool),
    /// Select-all state: `Some(true)` all, `Some(false)` none, `None` some.
    ExpectAll(Option<bool>),
    /// Remember the checked-out branch.
    MarkBranch,
    /// The checked-out branch (`None`: the remembered one).
    WaitBranch(Option<&'static str>, u64),
    /// The stash list has (or not) an entry with this memo (`""`: an app-made one) and, when
    /// given, exactly these files.
    WaitStash(&'static str, Option<&'static [&'static str]>, bool, u64),
    /// Pop / apply / drop the stash with this memo (`""`: the app-made one).
    StashBy(&'static str, StashAction),
    /// Unfold the stash with this memo.
    ExpandStash(&'static str),
    /// The open modal: its kind (`discard`, `commit`, `switch`, `create`, `drop`) and, for a
    /// discard, the file count and the new-file warning.
    ExpectModal(&'static str, Option<(usize, bool)>),
    /// The open modal shows an error containing this text.
    WaitModalError(&'static str, u64),
    WaitNoModal(u64),
    /// No changes left.
    WaitClean(u64),
    /// The file on disk contains (or not) the text.
    WaitDisk(&'static str, &'static str, bool, u64),
    /// Open the context menu of a History commit (by subject).
    CommitMenu(&'static str),
    /// Pick a commit menu action (by subject).
    CommitPick(&'static str, CommitKind),
    /// History's newest commit starts with this subject.
    WaitHistoryTop(&'static str, u64),
    /// The branch picker listed this local branch.
    WaitBranches(&'static str, u64),
    /// Pick the remembered branch in the picker.
    PickOrig,
    ExpectSide(SideTab),
    /// Delete the script's branches in the test repo.
    DeleteBranches,
}

fn c(x: CStep) -> Step {
    Step::Chg(x)
}

/// Insert the steps before phase 2's Korean pass (after the History checks).
pub(super) fn splice(steps: &mut Vec<Step>) {
    let at = steps
        .iter()
        .position(|s| matches!(s, Step::Log(l) if l.starts_with("--- phase 2 Korean pass")));
    if let Some(at) = at {
        steps.splice(at..at, self::steps());
    }
}

fn steps() -> Vec<Step> {
    use CStep::*;
    use Step::{D, Log, Shot, Wait, WaitChange, WaitExists, WaitToast};
    let a = |m: ActMsg| c(Do(m));
    let mut v = vec![
        Log("--- changes actions: checkboxes, select-all"),
        c(MarkBranch),
        D(DetailMsg::Side(SideTab::Changes)),
        c(Fixture),
        WaitChange(UNTRACKED, 10_000),
        WaitChange(STASHED, 10_000),
        WaitChange(MODIFIED, 10_000),
        c(ExpectAll(Some(true))),
        a(ActMsg::Check(UNTRACKED.into(), false)),
        c(ExpectChecked(UNTRACKED, false)),
        c(ExpectAll(None)),
        a(ActMsg::CheckAll(false)),
        c(ExpectAll(Some(false))),
        a(ActMsg::CheckAll(true)),
        c(ExpectAll(Some(true))),
        Log("--- changes actions: stash the checked file only (popover memo), then pop"),
        a(ActMsg::CheckAll(false)),
        a(ActMsg::Check(STASHED.into(), true)),
        a(ActMsg::StashToggle),
        a(ActMsg::StashMemo(STASH_MEMO.into())),
        Wait(300),
        Shot("60_changes_checkboxes_stash_popover"),
        Step::Pause("chg_stash_popover"),
        a(ActMsg::StashSubmit),
        c(WaitStash(STASH_MEMO, Some(&[STASHED]), true, 10_000)),
        WaitExists(STASHED, false, 10_000),
        WaitExists(UNTRACKED, true, 1000),
        c(ExpandStash(STASH_MEMO)),
        Wait(300),
        Shot("61_stash_section"),
        Step::Pause("chg_stash_section"),
        c(StashBy(STASH_MEMO, StashAction::Pop)),
        c(WaitStash(STASH_MEMO, None, false, 10_000)),
        WaitExists(STASHED, true, 10_000),
        WaitToast(5000),
        Log("--- changes actions: context menu, discard a modified and an untracked file"),
        WaitChange(STASHED, 10_000),
        a(ActMsg::CheckAll(false)),
        a(ActMsg::Check(MODIFIED.into(), true)),
        a(ActMsg::Check(UNTRACKED.into(), true)),
        a(ActMsg::Menu(Some((ChangeGroup::Unstaged, MODIFIED.into())))),
        Wait(300),
        Shot("62_change_context_menu"),
        Step::Pause("chg_context_menu"),
        // The menu's 체크한 파일 모두 변경 취소 (2).
        a(ActMsg::Discard(vec![MODIFIED.into(), UNTRACKED.into()])),
        c(ExpectModal("discard", Some((2, true)))),
        Wait(300),
        Shot("63_discard_modal"),
        Step::Pause("chg_discard_modal"),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(10_000)),
        c(WaitDisk(MODIFIED, MODIFIED_LINE.trim(), false, 10_000)),
        WaitExists(UNTRACKED, false, 10_000),
        WaitToast(5000),
        a(ActMsg::CheckAll(true)),
        Log("--- changes actions: reveal in Explore"),
        a(ActMsg::Reveal(MODIFIED.into())),
        c(ExpectSide(SideTab::Explore)),
        D(DetailMsg::Side(SideTab::History)),
        Log("--- changes actions: commit actions refuse a dirty tree (git-dirty in the dialog)"),
        c(CommitPick(HIST_SUBJECT, CommitKind::Revert)),
        c(ExpectModal("commit", None)),
        a(ActMsg::ModalConfirm),
        c(WaitModalError("uncommitted", 10_000)),
        Step::CloseModal,
        Log("--- changes actions: stash everything (no paths), clean tree"),
        D(DetailMsg::Side(SideTab::Changes)),
        a(ActMsg::CheckAll(true)),
        a(ActMsg::StashToggle),
        a(ActMsg::StashMemo(ALL_MEMO.into())),
        a(ActMsg::StashSubmit),
        c(WaitStash(ALL_MEMO, None, true, 10_000)),
        c(WaitClean(10_000)),
        Log("--- changes actions: History commit menu: branch from a commit, then revert it"),
        D(DetailMsg::Side(SideTab::History)),
        c(CommitMenu(HIST_SUBJECT)),
        Wait(300),
        Shot("64_commit_menu"),
        Step::Pause("chg_commit_menu"),
        c(CommitPick(HIST_SUBJECT, CommitKind::Branch)),
        a(ActMsg::ModalInput(FROM_COMMIT.into())),
        c(ExpectModal("commit", None)),
        Wait(300),
        Shot("65_commit_branch_confirm"),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(15_000)),
        c(WaitBranch(Some(FROM_COMMIT), 15_000)),
        c(WaitHistoryTop(HIST_SUBJECT, 15_000)),
        c(CommitPick(HIST_SUBJECT, CommitKind::Revert)),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(15_000)),
        c(WaitHistoryTop("Revert \"Add history fixture\"", 15_000)),
        Wait(300),
        Shot("66_history_after_revert"),
        Log("--- changes actions: branch picker: switch back carrying an untracked file"),
        c(Write(CARRIED)),
        WaitChange(CARRIED, 10_000),
        a(ActMsg::BranchToggle),
        c(WaitBranches(FROM_COMMIT, 10_000)),
        Wait(300),
        Shot("67_branch_picker"),
        Step::Pause("chg_branch_picker"),
        c(PickOrig),
        c(ExpectModal("switch", None)),
        a(ActMsg::ModalMode(SwitchMode::Carry)),
        Wait(300),
        Shot("68_switch_modal"),
        Step::Pause("chg_switch_modal"),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(15_000)),
        c(WaitBranch(None, 15_000)),
        WaitExists(CARRIED, true, 2000),
        Log("--- changes actions: new branch, leaving the change stashed on the current one"),
        a(ActMsg::BranchToggle),
        c(WaitBranches(FROM_COMMIT, 10_000)),
        a(ActMsg::NewBranch),
        c(ExpectModal("create", None)),
        a(ActMsg::ModalInput(NEW_BRANCH.into())),
        a(ActMsg::ModalMode(SwitchMode::Stash)),
        Wait(300),
        Shot("69_new_branch_modal"),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(15_000)),
        c(WaitBranch(Some(NEW_BRANCH), 15_000)),
        WaitExists(CARRIED, false, 5000),
        c(WaitStash("", Some(&[CARRIED]), true, 10_000)),
        Log("--- changes actions: back to the session's branch, restore its changes"),
        a(ActMsg::BranchToggle),
        c(WaitBranches(NEW_BRANCH, 10_000)),
        c(PickOrig),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(15_000)),
        c(WaitBranch(None, 15_000)),
        c(StashBy("", StashAction::Pop)),
        c(WaitStash("", None, false, 10_000)),
        c(StashBy(ALL_MEMO, StashAction::Pop)),
        c(WaitStash(ALL_MEMO, None, false, 10_000)),
        WaitChange("src/main.ts", 10_000),
        WaitExists(STASHED, true, 5000),
        c(DeleteBranches),
        Log("--- changes actions: Korean screenshots"),
        Step::SetLang(LanguagePref::Fixed(Lang::Ko)),
        Step::ExpectLang(Lang::Ko, "새 세션"),
        D(DetailMsg::Side(SideTab::Changes)),
        a(ActMsg::CheckAll(true)),
        a(ActMsg::Check(CARRIED.into(), false)),
        a(ActMsg::StashToggle),
        Wait(300),
        Shot("ko_chg_01_changes_stash_popover"),
        Step::Pause("ko_chg_stash_popover"),
        a(ActMsg::StashClose),
        a(ActMsg::Menu(Some((ChangeGroup::Untracked, STASHED.into())))),
        Wait(300),
        Shot("ko_chg_02_context_menu"),
        a(ActMsg::Menu(None)),
        a(ActMsg::Discard(vec![STASHED.into(), CARRIED.into()])),
        c(ExpectModal("discard", Some((2, true)))),
        Wait(300),
        Shot("ko_chg_03_discard_modal"),
        Step::Pause("ko_chg_discard_modal"),
        a(ActMsg::ModalConfirm),
        c(WaitNoModal(10_000)),
        WaitExists(STASHED, false, 10_000),
        WaitExists(CARRIED, false, 10_000),
        // A stash to show (the untracked fixture), popped again afterwards.
        c(Write(STASHED)),
        WaitChange(STASHED, 10_000),
        a(ActMsg::CheckAll(false)),
        a(ActMsg::Check(STASHED.into(), true)),
        a(ActMsg::StashToggle),
        a(ActMsg::StashMemo("한글 메모".into())),
        a(ActMsg::StashSubmit),
        c(WaitStash("한글 메모", Some(&[STASHED]), true, 10_000)),
        c(ExpandStash("한글 메모")),
        Wait(300),
        Shot("ko_chg_04_stash_section"),
        Step::Pause("ko_chg_stash_section"),
        c(StashBy("한글 메모", StashAction::Drop)),
        c(ExpectModal("drop", None)),
        Wait(300),
        Shot("ko_chg_05_stash_drop"),
        a(ActMsg::ModalConfirm),
        c(WaitStash("한글 메모", None, false, 10_000)),
        a(ActMsg::CheckAll(true)),
        D(DetailMsg::Side(SideTab::History)),
        c(CommitMenu(HIST_SUBJECT)),
        Wait(300),
        Shot("ko_chg_06_commit_menu"),
        Step::Pause("ko_chg_commit_menu"),
        c(CommitPick(HIST_SUBJECT, CommitKind::CherryPick)),
        Wait(300),
        Shot("ko_chg_07_commit_confirm"),
        Step::CloseModal,
        a(ActMsg::BranchToggle),
        c(WaitBranches("", 10_000)),
        Wait(300),
        Shot("ko_chg_08_branch_picker"),
        Step::Pause("ko_chg_branch_picker"),
        a(ActMsg::BranchClose),
        a(ActMsg::NewBranch),
        a(ActMsg::ModalInput("feature/한글".into())),
        Wait(300),
        Shot("ko_chg_09_new_branch"),
        Step::CloseModal,
        Step::SetLang(LanguagePref::Fixed(Lang::En)),
        Step::ExpectLang(Lang::En, "New session"),
        c(WaitBranch(None, 5000)),
    ];
    v.push(Log("--- changes actions: done"));
    v
}

fn sha_of(sc: &Script, app: &App, subject: &str) -> Option<String> {
    p2_detail(sc, app)
        .and_then(|d| d.history.commits.iter().find(|c| c.subject == subject))
        .map(|c| c.sha.clone())
}

fn stash_matches(e: &meninx_client::StashEntry, memo: &str) -> bool {
    if memo.is_empty() {
        e.auto
    } else {
        e.message == memo
    }
}

/// `(advance, task)` of a [`CStep`].
pub(super) fn run(
    sc: &mut Script,
    app: &mut App,
    st: CStep,
    el: Duration,
) -> (bool, Task<Message>) {
    let timeout = |ms: u64| el > Duration::from_millis(ms);
    let sid = sc.id(P2).unwrap_or_default();
    let mut task = Task::none();
    macro_rules! wait {
        ($ok:expr, $ms:expr, $what:expr) => {{
            if $ok {
                sc.check(true, &$what);
                true
            } else if timeout($ms) {
                sc.check(false, &format!("timeout: {}", $what));
                true
            } else {
                false
            }
        }};
    }
    let send = |app: &mut App, m: ActMsg| {
        log(&format!("acts <- {m:?}"));
        app.update(Message::Detail(sid.clone(), DetailMsg::Acts(m)))
    };
    let next = match st {
        CStep::Fixture => {
            let wt = p2_worktree(sc, app);
            let r = (|| -> Result<(), String> {
                let m = wt.join(MODIFIED);
                let mut text = std::fs::read_to_string(&m).map_err(|e| e.to_string())?;
                text.push_str(MODIFIED_LINE);
                std::fs::write(&m, text).map_err(|e| e.to_string())?;
                std::fs::create_dir_all(wt.join("chg")).map_err(|e| e.to_string())?;
                std::fs::write(wt.join(UNTRACKED), "new file\n").map_err(|e| e.to_string())?;
                std::fs::write(wt.join(STASHED), "stash me\n").map_err(|e| e.to_string())?;
                Ok(())
            })();
            sc.check(
                r.is_ok(),
                &format!("changes fixtures in {}: {r:?}", wt.display()),
            );
            true
        }
        CStep::Write(rel) => {
            let wt = p2_worktree(sc, app);
            let p = wt.join(rel);
            let r = p
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(&p, format!("{rel}\n")));
            sc.check(r.is_ok(), &format!("wrote {rel}: {r:?}"));
            true
        }
        CStep::Do(m) => {
            task = send(app, m);
            true
        }
        CStep::ExpectChecked(path, want) => {
            let got = p2_detail(sc, app).map(|d| d.acts.is_checked(path));
            sc.check(got == Some(want), &format!("{path} checked = {got:?}"));
            true
        }
        CStep::ExpectAll(want) => {
            let got = p2_detail(sc, app).map(|d| {
                let (n, total) = (d.checked_changes().len(), d.changed_count());
                let state = if n == total {
                    Some(true)
                } else if n == 0 {
                    Some(false)
                } else {
                    None
                };
                (state, n, total, d.all_checked())
            });
            sc.check(
                got.is_some_and(|g| g.0 == want && g.3 == (want == Some(true))),
                &format!("select-all {want:?}: (state, checked, total, all) = {got:?}"),
            );
            true
        }
        CStep::MarkBranch => {
            let b = p2_detail(sc, app)
                .and_then(|d| d.acts.branch_name().map(str::to_owned))
                .unwrap_or_default();
            sc.check(!b.is_empty(), &format!("phase 2 branch {b:?}"));
            *ORIG.lock().unwrap() = b;
            true
        }
        CStep::WaitBranch(want, ms) => {
            let want = want.map_or_else(orig, str::to_owned);
            let got = p2_detail(sc, app).and_then(|d| d.acts.branch_name().map(str::to_owned));
            let shown = p2_detail(sc, app)
                .and_then(|d| d.acts.branch.list.as_ref())
                .and_then(|b| b.current.clone());
            wait!(
                got.as_deref() == Some(want.as_str()),
                ms,
                format!("checked-out branch {got:?} (want {want:?}; picker list {shown:?})")
            )
        }
        CStep::WaitStash(memo, files, present, ms) => {
            let list = p2_detail(sc, app).and_then(|d| d.acts.stash.list.clone());
            let found = list
                .iter()
                .flatten()
                .find(|e| stash_matches(e, memo))
                .cloned();
            let ok = match (&found, present) {
                (Some(e), true) => files.is_none_or(|f| {
                    let mut got: Vec<&str> = e.files.iter().map(|x| x.path.as_str()).collect();
                    got.sort();
                    let mut want = f.to_vec();
                    want.sort();
                    got == want
                }),
                (None, false) => list.is_some(),
                _ => false,
            };
            let what: Vec<(String, bool, String, usize)> = list
                .iter()
                .flatten()
                .map(|e| (e.message.clone(), e.auto, e.branch.clone(), e.files.len()))
                .collect();
            wait!(
                ok,
                ms,
                format!("stash {memo:?} present={present} files {files:?}: list {what:?}")
            )
        }
        CStep::StashBy(memo, action) => {
            let sha = p2_detail(sc, app)
                .and_then(|d| d.acts.stash.list.as_ref())
                .and_then(|l| l.iter().find(|e| stash_matches(e, memo)))
                .map(|e| e.sha.clone());
            match sha {
                Some(sha) => task = send(app, ActMsg::StashAct(sha, action)),
                None => sc.check(false, &format!("no stash {memo:?}")),
            }
            true
        }
        CStep::ExpandStash(memo) => {
            let sha = p2_detail(sc, app)
                .and_then(|d| d.acts.stash.list.as_ref())
                .and_then(|l| l.iter().find(|e| stash_matches(e, memo)))
                .map(|e| e.sha.clone());
            match sha {
                Some(sha) => task = send(app, ActMsg::StashExpand(sha)),
                None => sc.check(false, &format!("no stash {memo:?}")),
            }
            true
        }
        CStep::ExpectModal(kind, discard) => {
            let got = match &app.modal {
                Some(Modal::Changes(ChangesModal { kind: k, .. })) => Some(match k {
                    ModalKind::Discard(files) => (
                        "discard",
                        Some((
                            files.len(),
                            files.iter().any(|f| {
                                matches!(
                                    f.kind,
                                    meninx_client::ChangeKind::Untracked
                                        | meninx_client::ChangeKind::Added
                                )
                            }),
                        )),
                    ),
                    ModalKind::StashDrop(_) => ("drop", None),
                    ModalKind::Commit { .. } => ("commit", None),
                    ModalKind::Switch { .. } => ("switch", None),
                    ModalKind::Create { .. } => ("create", None),
                }),
                _ => None,
            };
            sc.check(
                got == Some((kind, discard)),
                &format!("dialog {got:?} (want {kind} {discard:?})"),
            );
            true
        }
        CStep::WaitModalError(needle, ms) => {
            let e = match &app.modal {
                Some(Modal::Changes(f)) => f.error.clone(),
                _ => None,
            };
            wait!(
                e.as_deref().is_some_and(|e| e.contains(needle)),
                ms,
                format!("dialog error {e:?} (want {needle:?})")
            )
        }
        CStep::WaitNoModal(ms) => {
            let err = match &app.modal {
                Some(Modal::Changes(f)) => f.error.clone(),
                _ => None,
            };
            wait!(
                app.modal.is_none(),
                ms,
                format!("dialog closed (error {err:?})")
            )
        }
        CStep::WaitClean(ms) => {
            let n = p2_detail(sc, app).map(|d| d.changed_count());
            wait!(n == Some(0), ms, format!("no changes left ({n:?})"))
        }
        CStep::WaitDisk(rel, needle, present, ms) => {
            let wt = p2_worktree(sc, app);
            let text = std::fs::read_to_string(wt.join(rel)).unwrap_or_default();
            wait!(
                text.contains(needle) == present,
                ms,
                format!("{rel} on disk contains {needle:?} = {present}")
            )
        }
        CStep::CommitMenu(subject) => {
            match sha_of(sc, app, subject) {
                Some(sha) => task = send(app, ActMsg::CommitMenu(Some(sha))),
                None => sc.check(false, &format!("History has no commit {subject:?}")),
            }
            true
        }
        CStep::CommitPick(subject, kind) => {
            match sha_of(sc, app, subject) {
                Some(sha) => task = send(app, ActMsg::CommitPick(sha, kind)),
                None => sc.check(false, &format!("History has no commit {subject:?}")),
            }
            true
        }
        CStep::WaitHistoryTop(prefix, ms) => {
            let top = p2_detail(sc, app)
                .and_then(|d| d.history.commits.first())
                .map(|c| c.subject.clone());
            wait!(
                top.as_deref().is_some_and(|t| t.starts_with(prefix)),
                ms,
                format!("History top {top:?} (want {prefix:?})")
            )
        }
        CStep::WaitBranches(name, ms) => {
            let b = p2_detail(sc, app).and_then(|d| d.acts.branch.list.clone());
            let open = p2_detail(sc, app).is_some_and(|d| d.acts.branch.open);
            let names: Vec<String> = b
                .iter()
                .flat_map(|b| b.local.iter().map(|l| l.name.clone()))
                .collect();
            wait!(
                open && b.is_some() && (name.is_empty() || names.iter().any(|n| n == name)),
                ms,
                format!("branch picker lists {names:?} (want {name:?})")
            )
        }
        CStep::PickOrig => {
            task = send(app, ActMsg::PickBranch(orig(), None));
            true
        }
        CStep::ExpectSide(want) => {
            let got = p2_detail(sc, app).map(|d| d.side);
            sc.check(got == Some(want), &format!("side tab {got:?}"));
            true
        }
        CStep::DeleteBranches => {
            let r = git(&sc.repo, &["branch", "-D", FROM_COMMIT, NEW_BRANCH]);
            sc.check(r.is_ok(), &format!("deleted the script's branches: {r:?}"));
            true
        }
    };
    (next, task)
}
