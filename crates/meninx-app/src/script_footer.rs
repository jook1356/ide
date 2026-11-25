//! App script steps for the Changes / History footer (`footer.rs`): footer visibility per tab,
//! 커밋 맡기기 into a shell session (the commit prompt shows in the terminal), the sync button
//! against the local bare origin (publish, push, fetch + pull with counts), the "…" menu per
//! state, PR 올리기 (commit-based and AI mode) / 머지하기 / PR 보기 / PR 닫기 against a fake `gh`
//! (`MENINX_GH`; no network, git credentials from a scratch config), 새 브랜치로 이어가기, with
//! English `foot_*` and Korean `ko_foot_*` screenshots.
//!
//! The test repo's origin becomes `https://github.com/meninx-test/footer.git` with a local
//! `url.<bare origin>.insteadOf` (git pushes to the bare repo, the PR backend sees a GitHub
//! repo); restored at the end. A child module of `script.rs`: it uses the script's helpers.

use std::sync::Mutex;

use super::*;
use crate::footer::{FooterMenu, FooterModal, FooterMsg, MoreKind, SyncOp, Tone};
use meninx_client::PrState;

const GH_URL: &str = "https://github.com/meninx-test/footer.git";
const CHANGE: &str = "foot-change.txt";

/// The session worktree's branch before 새 브랜치로 이어가기.
static BRANCH: Mutex<String> = Mutex::new(String::new());

#[derive(Debug, Clone)]
pub(super) enum FStep {
    /// GitHub-looking origin (insteadOf -> the bare origin), fresh fake `gh` state.
    Setup,
    Restore,
    /// The footer is shown (Changes / History) or not (Explore).
    Shown(bool),
    Msg(Box<FooterMsg>),
    /// Sync status: (ahead, behind, unpublished).
    WaitSync(u32, u32, bool, u64),
    /// No sync operation in flight.
    WaitIdle(u64),
    WriteFile(&'static str, &'static str),
    RemoveFile(&'static str),
    WaitNoChanges(u64),
    /// A commit on origin's copy of the session branch (someone else pushed).
    RemoteCommit,
    /// The PR status arrived with this state (`None` = no PR).
    WaitPr(Option<PrState>, u64),
    /// The "…" menu rows (kind, enabled), in order.
    More(&'static [(MoreKind, bool)]),
    /// A footer dialog is shown and loaded: "pending", "pr", "merge", "close",
    /// "continue", "continue-done".
    WaitModal(&'static str, u64),
    WaitNoModal(u64),
    /// The PR job ended with this tone.
    WaitJob(Tone, u64),
    /// The AI PR job is waiting for the PR file.
    WaitJobFile(u64),
    /// Write the PR file like the agent would.
    WritePrFile,
    /// The web tab shows the fake PR page.
    ExpectPrPage,
    MarkBranch,
    /// The worktree is on another branch than at [`FStep::MarkBranch`].
    WaitBranchChanged(u64),
}

fn f(x: FStep) -> Step {
    Step::Foot(x)
}

fn m(x: FooterMsg) -> Step {
    Step::Foot(FStep::Msg(Box::new(x)))
}

/// Run the footer steps after the base-branch pass (it leaves the bare origin in place).
pub(super) fn splice(steps: &mut Vec<Step>) {
    let at = steps
        .iter()
        .rposition(|s| matches!(s, Step::SetOriginUrl(false)));
    if let Some(at) = at {
        steps.splice(at + 1..at + 1, self::steps());
    }
}

fn ko(name: &'static str) -> [Step; 5] {
    [
        Step::SetLang(LanguagePref::Fixed(Lang::Ko)),
        Step::Wait(400),
        Step::Shot(name),
        Step::Pause(name),
        Step::SetLang(LanguagePref::Fixed(Lang::En)),
    ]
}

fn steps() -> Vec<Step> {
    use FStep::*;
    use Step::{
        App as A, AskDelete, CommitP2, ConfirmDelete, D, FillNew, Log, MarkP2, Shot, Submit, Wait,
        WaitChange, WaitCreated, WaitDeleted, WaitHistory, WaitText, WriteP2,
    };
    let mut v = vec![
        Log("--- footer: commit / sync / PR footer (fake gh, local bare origin)"),
        Step::SetLang(LanguagePref::Fixed(Lang::En)),
        f(Setup),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "footer",
        },
        Submit,
        WaitCreated(30_000),
        MarkP2,
        WaitText(P2, ">", 15_000),
        // Prompts go to a program that only echoes its input (never to cmd itself).
        WriteP2("findstr \"^\"\r"),
        Wait(500),
        Log("--- footer: shown under Changes and History, hidden under Explore"),
        D(DetailMsg::Side(SideTab::History)),
        f(Shown(true)),
        D(DetailMsg::Side(SideTab::Explore)),
        f(Shown(false)),
        D(DetailMsg::Side(SideTab::Changes)),
        f(Shown(true)),
        f(WaitSync(0, 0, true, 20_000)),
        Log("--- footer: 커밋 맡기기 sends the commit prompt to the shell session"),
        f(WriteFile(CHANGE, "footer change\n")),
        WaitChange(CHANGE, 10_000),
        Wait(300),
        Shot("foot_01_changes_footer"),
        m(FooterMsg::Commit),
        WaitText(P2, "- push", 10_000),
        Wait(300),
        Shot("foot_02_commit_prompt"),
        Log("--- footer: publish, push, fetch + pull with counts"),
        CommitP2("foot-1.txt", "one\n", "Footer commit 1"),
        f(WaitSync(1, 0, true, 15_000)),
        m(FooterMsg::Menu(FooterMenu::Sync)),
        Wait(300),
        Shot("foot_03_sync_menu"),
        Step::Pause("foot_sync_menu"),
    ];
    v.extend(ko("ko_foot_02_sync_menu"));
    v.extend([
        m(FooterMsg::CloseMenu),
        m(FooterMsg::Sync(SyncOp::Publish)),
        f(WaitIdle(30_000)),
        f(WaitSync(0, 0, false, 15_000)),
        CommitP2("foot-2.txt", "two\n", "Footer commit 2"),
        f(WaitSync(1, 0, false, 15_000)),
        Wait(300),
        Shot("foot_04_push_count"),
        m(FooterMsg::Sync(SyncOp::Push)),
        f(WaitIdle(30_000)),
        f(WaitSync(0, 0, false, 15_000)),
        f(RemoteCommit),
        m(FooterMsg::Sync(SyncOp::Fetch)),
        f(WaitIdle(30_000)),
        f(WaitSync(0, 1, false, 15_000)),
        Wait(300),
        Shot("foot_05_pull_count"),
        m(FooterMsg::Sync(SyncOp::Pull)),
        f(WaitIdle(30_000)),
        f(WaitSync(0, 0, false, 15_000)),
        D(DetailMsg::Side(SideTab::History)),
        WaitHistory("Remote footer commit", 15_000),
        D(DetailMsg::Side(SideTab::Changes)),
        Log("--- footer: … menu without a PR"),
        f(RemoveFile(CHANGE)),
        f(WaitNoChanges(10_000)),
        f(WaitPr(None, 15_000)),
        f(More(&[
            (MoreKind::CreatePr, true),
            (MoreKind::Continue, true),
        ])),
        m(FooterMsg::Menu(FooterMenu::More)),
        Wait(300),
        Shot("foot_06_more_menu"),
        m(FooterMsg::CloseMenu),
        Log("--- footer: PR 올리기 (commit history) -> open PR"),
        m(FooterMsg::OpenPr),
        f(WaitModal("pr", 20_000)),
        Wait(300),
        Shot("foot_07_pr_dialog"),
        Step::Pause("foot_pr_dialog"),
    ]);
    v.extend(ko("ko_foot_04_pr_dialog"));
    v.extend([
        m(FooterMsg::PrSubmit(false)),
        f(WaitNoModal(20_000)),
        f(WaitJob(Tone::Success, 30_000)),
        f(WaitPr(Some(PrState::Open), 15_000)),
        f(ExpectPrPage),
        D(DetailMsg::Side(SideTab::Changes)),
        Wait(500),
        Shot("foot_08_pr_open"),
        f(More(&[
            (MoreKind::Merge, true),
            (MoreKind::View, true),
            (MoreKind::Close, true),
            (MoreKind::Continue, true),
        ])),
        m(FooterMsg::Menu(FooterMenu::More)),
        Wait(300),
        Shot("foot_09_more_open_pr"),
    ]);
    v.extend(ko("ko_foot_03_more_menu"));
    v.extend([
        m(FooterMsg::CloseMenu),
        Log("--- footer: an uncommitted change disables 머지하기"),
        f(WriteFile(CHANGE, "again\n")),
        WaitChange(CHANGE, 10_000),
        f(More(&[
            (MoreKind::Merge, false),
            (MoreKind::View, true),
            (MoreKind::Close, true),
            (MoreKind::Continue, true),
        ])),
        f(RemoveFile(CHANGE)),
        f(WaitNoChanges(10_000)),
        Log("--- footer: PR 보기 and 머지하기"),
        m(FooterMsg::ViewPr),
        f(ExpectPrPage),
        m(FooterMsg::OpenMerge),
        f(WaitModal("merge", 5000)),
        Wait(300),
        Shot("foot_10_merge_dialog"),
        Step::Pause("foot_merge_dialog"),
    ]);
    v.extend(ko("ko_foot_05_merge_dialog"));
    v.extend([
        m(FooterMsg::MergeConfirm),
        f(WaitNoModal(20_000)),
        f(WaitPr(Some(PrState::Merged), 15_000)),
        Log("--- footer: a second PR (pending step), then PR 닫기"),
        CommitP2("foot-3.txt", "three\n", "Footer commit 3"),
        f(WaitPr(Some(PrState::Merged), 15_000)),
        f(More(&[(MoreKind::CreatePr, true), (MoreKind::Continue, true)])),
        m(FooterMsg::OpenPr),
        f(WaitModal("pending", 20_000)),
        Wait(300),
        Shot("foot_11_pr_pending"),
        m(FooterMsg::PrContinue),
        f(WaitModal("pr", 20_000)),
        m(FooterMsg::PrSubmit(false)),
        f(WaitNoModal(20_000)),
        f(WaitJob(Tone::Success, 30_000)),
        f(WaitPr(Some(PrState::Open), 15_000)),
        D(DetailMsg::Side(SideTab::Changes)),
        m(FooterMsg::OpenClosePr),
        f(WaitModal("close", 5000)),
        Wait(300),
        Shot("foot_12_close_confirm"),
        m(FooterMsg::ClosePrConfirm),
        f(WaitNoModal(20_000)),
        f(WaitPr(Some(PrState::Closed), 15_000)),
        Log("--- footer: PR 올리기 in AI mode (the prompt goes to the shell, the PR file is written)"),
        CommitP2("foot-4.txt", "four\n", "Footer commit 4"),
        f(WaitSync(1, 0, false, 15_000)),
        m(FooterMsg::OpenPr),
        f(WaitModal("pending", 20_000)),
        m(FooterMsg::PrContinue),
        f(WaitModal("pr", 20_000)),
        m(FooterMsg::PrFromCommits(false)),
        Wait(500),
        m(FooterMsg::PrSubmit(false)),
        f(WaitNoModal(20_000)),
        f(WaitJobFile(20_000)),
        WaitText(P2, "origin/main", 15_000),
        D(DetailMsg::Side(SideTab::Changes)),
        Wait(300),
        Shot("foot_13_ai_job_waiting"),
        f(WritePrFile),
        f(WaitJob(Tone::Success, 30_000)),
        f(WaitPr(Some(PrState::Open), 15_000)),
        D(DetailMsg::Side(SideTab::Changes)),
        Wait(300),
        Shot("foot_14_ai_pr_created"),
        Log("--- footer: 새 브랜치로 이어가기 (closes the open PR)"),
        f(MarkBranch),
        m(FooterMsg::OpenContinue),
        f(WaitModal("continue", 30_000)),
        Wait(300),
        Shot("foot_15_continue_dialog"),
        Step::Pause("foot_continue_dialog"),
    ]);
    v.extend(ko("ko_foot_06_branch_continue"));
    v.extend([
        m(FooterMsg::ContinueRun),
        f(WaitModal("continue-done", 30_000)),
        Wait(300),
        Shot("foot_16_continue_done"),
        Step::CloseModal,
        f(WaitBranchChanged(15_000)),
        f(WaitPr(None, 15_000)),
        Log("--- footer: Korean footer"),
        D(DetailMsg::Side(SideTab::Changes)),
    ]);
    v.extend(ko("ko_foot_01_footer"));
    v.extend([
        WriteP2("\x03"),
        Wait(300),
        f(Restore),
        AskDelete(P2),
        ConfirmDelete,
        WaitDeleted(P2, 20_000),
        A(Message::CloseModal),
    ]);
    v
}

fn footer_of<'a>(sc: &Script, app: &'a App) -> Option<&'a crate::footer::RepoFooter> {
    let d = p2_detail(sc, app)?;
    d.footer.repos.get(&d.repo)
}

fn git_env(dir: &Path, envs: &[(&str, &str)], args: &[&str]) -> Result<String, String> {
    let mut c = command("git");
    c.arg("-C").arg(dir);
    c.args([
        "-c",
        "user.name=meninx-test",
        "-c",
        "user.email=meninx-test@example.invalid",
        "-c",
        "commit.gpgsign=false",
    ]);
    for (k, v) in envs {
        c.env(k, v);
    }
    let out = c.args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

pub(super) fn run(
    sc: &mut Script,
    app: &mut App,
    st: FStep,
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
    let next = match st {
        FStep::Setup => {
            let bare = sc.data.join("script-origin.git");
            let r = (|| -> Result<(), String> {
                git(&sc.repo, &["remote", "set-url", "origin", GH_URL])?;
                let key = format!(
                    "url.{}.insteadOf",
                    bare.display().to_string().replace('\\', "/")
                );
                git(&sc.repo, &["config", &key, GH_URL])?;
                // The fake gh's PRs (`tests/fake-gh/gh.js`): `MENINX_FAKE_GH_STATE`, else next to it.
                let state = std::env::var_os("MENINX_FAKE_GH_STATE")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("MENINX_GH")
                            .and_then(|g| PathBuf::from(g).parent().map(Path::to_path_buf))
                    });
                if let Some(d) = state {
                    let _ = std::fs::remove_file(d.join("state.json"));
                }
                Ok(())
            })();
            log(&format!(
                "MENINX_GH={:?} MENINX_GITHUB_API={:?} GIT_CONFIG_GLOBAL={:?}",
                std::env::var("MENINX_GH"),
                std::env::var("MENINX_GITHUB_API"),
                std::env::var("GIT_CONFIG_GLOBAL")
            ));
            let safe = std::env::var_os("MENINX_GH").is_some()
                && std::env::var_os("GIT_CONFIG_GLOBAL").is_some()
                && std::env::var("GIT_CONFIG_NOSYSTEM").is_ok_and(|v| v == "1");
            sc.check(
                r.is_ok() && safe,
                &format!(
                    "origin {GH_URL} -> {} (fake gh, scratch git config: {safe}) {r:?}",
                    bare.display()
                ),
            );
            true
        }
        FStep::Restore => {
            let bare = sc.data.join("script-origin.git");
            let key = format!(
                "url.{}.insteadOf",
                bare.display().to_string().replace('\\', "/")
            );
            let _ = git(&sc.repo, &["config", "--unset", &key]);
            let r = git(
                &sc.repo,
                &["remote", "set-url", "origin", &bare.display().to_string()],
            );
            sc.check(r.is_ok(), &format!("origin restored {r:?}"));
            true
        }
        FStep::Shown(want) => {
            let shown = p2_detail(sc, app)
                .is_some_and(|d| !d.repo.is_empty() && crate::footer::shown_on(d.side));
            let side = p2_detail(sc, app).map(|d| d.side);
            sc.check(
                shown == want,
                &format!("footer shown = {shown} on {side:?}"),
            );
            true
        }
        FStep::Msg(x) => {
            log(&format!("footer <- {x:?}"));
            task = app.update(Message::Footer(sid, *x));
            true
        }
        FStep::WaitSync(ahead, behind, unpublished, ms) => {
            let s = footer_of(sc, app).and_then(|f| f.sync.clone());
            let ok = s.as_ref().is_some_and(|s| {
                s.ahead == ahead
                    && s.behind == behind
                    && s.unpublished == unpublished
                    && !s.no_remote
            }) && footer_of(sc, app).is_some_and(|f| f.running.is_none());
            wait!(
                ok,
                ms,
                format!(
                    "sync status {s:?} (want ahead {ahead}, behind {behind}, unpublished {unpublished})"
                )
            )
        }
        FStep::WaitIdle(ms) => {
            let r = footer_of(sc, app).and_then(|f| f.running);
            wait!(
                r.is_none(),
                ms,
                format!("no sync operation running ({r:?})")
            )
        }
        FStep::WriteFile(path, content) => {
            let wt = p2_worktree(sc, app);
            let r = std::fs::write(wt.join(path), content);
            sc.check(r.is_ok(), &format!("wrote {path}: {r:?}"));
            true
        }
        FStep::RemoveFile(path) => {
            let wt = p2_worktree(sc, app);
            let r = std::fs::remove_file(wt.join(path));
            sc.check(r.is_ok(), &format!("removed {path}: {r:?}"));
            true
        }
        FStep::WaitNoChanges(ms) => {
            let n = p2_detail(sc, app).map_or(99, |d| d.changed_count());
            wait!(n == 0, ms, format!("no changed files ({n})"))
        }
        FStep::RemoteCommit => {
            let wt = p2_worktree(sc, app);
            let idx = sc.data.join("foot-remote.idx");
            let blob_file = sc.data.join("foot-remote.txt");
            let idx_s = idx.display().to_string();
            let r = (|| -> Result<String, String> {
                std::fs::write(&blob_file, "from the remote\n").map_err(|e| e.to_string())?;
                let branch = git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"])?;
                let head = git(&wt, &["rev-parse", "HEAD"])?;
                let env = [("GIT_INDEX_FILE", idx_s.as_str())];
                git_env(&wt, &env, &["read-tree", "HEAD"])?;
                let blob = git(
                    &wt,
                    &["hash-object", "-w", &blob_file.display().to_string()],
                )?;
                git_env(
                    &wt,
                    &env,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("100644,{blob},remote-footer.txt"),
                    ],
                )?;
                let tree = git_env(&wt, &env, &["write-tree"])?;
                let c = git(
                    &wt,
                    &[
                        "commit-tree",
                        &tree,
                        "-p",
                        &head,
                        "-m",
                        "Remote footer commit",
                    ],
                )?;
                git(
                    &wt,
                    &["push", "-q", "origin", &format!("{c}:refs/heads/{branch}")],
                )?;
                Ok(format!("{c} on {branch}"))
            })();
            let _ = std::fs::remove_file(&idx);
            let _ = std::fs::remove_file(&blob_file);
            sc.check(r.is_ok(), &format!("remote commit {r:?}"));
            true
        }
        FStep::WaitPr(want, ms) => {
            let fo = footer_of(sc, app);
            let got = fo.and_then(|f| f.pr.as_ref().map(|p| (p.number, p.state)));
            let ok = fo.is_some_and(|f| f.pr_loaded) && got.map(|g| g.1) == want;
            let err = fo.and_then(|f| f.pr_error.clone());
            wait!(
                ok,
                ms,
                format!("PR status {got:?} (want {want:?}, error {err:?})")
            )
        }
        FStep::More(want) => {
            let got: Vec<(MoreKind, bool)> = app
                .footer_more(&sid)
                .iter()
                .map(|e| (e.kind, e.enabled))
                .collect();
            sc.check(got == want, &format!("… menu {got:?} (want {want:?})"));
            true
        }
        FStep::WaitModal(kind, ms) => {
            let (ok, what) = match &app.modal {
                Some(Modal::Footer(x)) => match (kind, x.as_ref()) {
                    ("pending", FooterModal::Pr(p)) => (
                        p.asking() && p.pending_work().is_some(),
                        format!(
                            "pending {:?}",
                            p.pending_work().map(|w| (w.commit_count, w.files.len()))
                        ),
                    ),
                    ("pr", FooterModal::Pr(p)) => {
                        let preview = matches!(&p.preview, Some(crate::footer::Load::Ready(_)));
                        (
                            !p.asking() && p.target.is_some() && (!p.from_commits || preview),
                            format!(
                                "PR dialog target {:?} title {:?} preview {preview}",
                                p.target, p.title
                            ),
                        )
                    }
                    ("merge", FooterModal::Merge(x)) => (true, format!("merge #{}", x.pr.number)),
                    ("close", FooterModal::ClosePr(x)) => (true, format!("close #{}", x.number)),
                    ("continue", FooterModal::Continue(x)) => {
                        let ready = x.info.ready().is_some()
                            && matches!(&x.check, Some(crate::footer::Load::Ready(c)) if c.conflict.is_none());
                        (
                            ready,
                            format!(
                                "continue: {} commits picked of {:?}, close PR {:?}",
                                x.picked.len(),
                                x.info.ready().map(|i| i.commits.len()),
                                x.open_pr
                            ),
                        )
                    }
                    ("continue-done", FooterModal::Continue(x)) => (
                        x.result.is_some(),
                        format!("continue result {:?}", x.result),
                    ),
                    _ => (false, "another footer dialog".to_owned()),
                },
                _ => (false, "no footer dialog".to_owned()),
            };
            wait!(ok, ms, format!("{kind} dialog: {what}"))
        }
        FStep::WaitNoModal(ms) => {
            let err = match &app.modal {
                Some(Modal::Footer(x)) => match x.as_ref() {
                    FooterModal::Pr(p) => p.request_error.clone(),
                    FooterModal::Merge(x) => x.error.clone(),
                    FooterModal::ClosePr(x) => x.error.clone(),
                    _ => None,
                },
                _ => None,
            };
            wait!(
                app.modal.is_none(),
                ms,
                format!("dialog closed (error {err:?})")
            )
        }
        FStep::WaitJob(tone, ms) => {
            let j = footer_of(sc, app).and_then(|f| f.job.clone());
            let ok = j.as_ref().is_some_and(|j| !j.running() && j.tone == tone);
            let what = j
                .as_ref()
                .map(|j| format!("{:?} {:?}: {} · {}", j.stage, j.tone, j.title, j.message));
            wait!(ok, ms, format!("PR job {what:?} (want {tone:?})"))
        }
        FStep::WaitJobFile(ms) => {
            let p = footer_of(sc, app)
                .and_then(|f| f.job.as_ref())
                .and_then(|j| j.pr_file.clone());
            wait!(p.is_some(), ms, format!("AI PR job waits for {p:?}"))
        }
        FStep::WritePrFile => {
            let p = footer_of(sc, app)
                .and_then(|f| f.job.as_ref())
                .and_then(|j| j.pr_file.clone());
            let r = match &p {
                Some(p) => std::fs::write(
                    p,
                    "AI footer PR\n\nWritten by the script in place of the agent.\n",
                ),
                None => Err(std::io::Error::other("no PR file")),
            };
            sc.check(
                r.is_ok(),
                &format!("PR file written like the agent: {p:?} {r:?}"),
            );
            true
        }
        FStep::ExpectPrPage => {
            let l = app.live(&sid);
            let url = l.map(|l| l.url_input.clone()).unwrap_or_default();
            let active = l.is_some_and(|l| l.detail.active == ItemKey::Web);
            sc.check(
                active && url.contains("Fake%20PR"),
                &format!(
                    "PR page in the web tab (active {active}): {}",
                    url.chars().take(60).collect::<String>()
                ),
            );
            true
        }
        FStep::MarkBranch => {
            let b = git(
                &p2_worktree(sc, app),
                &["rev-parse", "--abbrev-ref", "HEAD"],
            )
            .unwrap_or_default();
            log(&format!("footer: worktree branch {b}"));
            *BRANCH.lock().unwrap() = b;
            true
        }
        FStep::WaitBranchChanged(ms) => {
            let before = BRANCH.lock().unwrap().clone();
            let now = git(
                &p2_worktree(sc, app),
                &["rev-parse", "--abbrev-ref", "HEAD"],
            )
            .unwrap_or_default();
            let recorded = app
                .live(&sid)
                .and_then(|l| l.info.repos.first())
                .map(|r| r.branch.clone())
                .unwrap_or_default();
            wait!(
                !now.is_empty() && now != before && recorded == now,
                ms,
                format!("worktree moved {before} -> {now} (session records {recorded})")
            )
        }
    };
    (next, task)
}
