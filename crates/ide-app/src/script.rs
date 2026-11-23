//! `IDE_APP_SCRIPT=1`: an in-app end-to-end test against a real `ide-server`. Everything goes
//! through the app's own messages (the same code paths the buttons and dialogs use), terminal
//! input goes to the server like typed keys, and evidence is taken with iced's
//! `window::screenshot`. No OS input is used.
//!
//! Environment:
//! - `IDE_APP_TEST_REPO`: throwaway repo folder (created with `git init` + one commit if missing)
//! - `IDE_APP_TEST_REPO2`: a second throwaway repo for the multi-repo checks (default: a
//!   `lib-repo` folder beside the first)
//! - `IDE_APP_EVIDENCE`: screenshot/log folder (default `<data dir>/evidence`)
//! - `IDE_APP_SCRIPT_CLAUDE=1`: also open a Claude session (first-run screen only; never types)
//!   with a script account under `<data>/accounts/claude/` (never the user's own login)
//!
//! The script installs a declarative provider `fake` (`<data>/providers/script-fake.toml`,
//! `cmd.exe` echoing a marker as its "login") to exercise accounts and login terminals without
//! running a real CLI's login.
//! - `IDE_APP_SCRIPT_PAUSE=1`: hold key screens for an external read-only capture (or a
//!   comma-separated list of pause-name prefixes: hold only those)
//!
//! The script runs in English (it switches the language first; error checks match English
//! text) and has one Korean pass that screenshots the main screen and the dialogs. It ends by
//! stopping the server (settings page -> stop server), so no daemon is left running.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Task, window};
use ide_i18n::{Lang, LanguagePref, tr};
use ide_views::{ChangeGroup, ChangesMessage, CodeMessage, DiffMode, DiffViewMessage, TreeMessage};

use crate::app::{ALL_CONFIGURATIONS, App, Conn, McpField, Message, Modal, RunState, SortChange};
use crate::detail::{DetailMsg, FileBar, Item, ItemKey, SideTab};
use crate::session_sort::SortKey;
use crate::ui_state::{CloseAction, LOGIN_GROUP, Page, ServerPref};
use ide_client::{AccountState, SessionKind};

/// Changes / History tab action steps (`script_changes.rs`).
#[path = "script_changes.rs"]
mod changes;
/// Changes / History footer steps (`script_footer.rs`).
#[path = "script_footer.rs"]
mod footer_steps;
/// Detail window steps (`script_detail_window.rs`).
#[path = "script_detail_window.rs"]
mod detail_window_steps;

/// Session index meaning "the phase 2 session" (created through the folder browser).
const P2: usize = 99;

static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static T0: OnceLock<Instant> = OnceLock::new();

/// Script mode is on (`IDE_APP_SCRIPT=1`).
pub fn enabled() -> bool {
    LOG.get().is_some()
}

/// Append a line to the script log (no-op outside script mode).
pub fn log(s: &str) {
    let Some(f) = LOG.get() else { return };
    let t = T0.get_or_init(Instant::now).elapsed().as_secs_f32();
    let line = format!("[{t:8.3}] {s}\n");
    eprint!("{line}");
    let _ = f.lock().unwrap().write_all(line.as_bytes());
}

#[derive(Debug, Clone)]
enum Step {
    Log(&'static str),
    Wait(u64),
    EnsureRepo,
    /// Wait until the client is connected and the lists arrived.
    WaitReady(u64),
    /// Open the new-session dialog and fill it (repo, provider, title) like a user would.
    FillNew {
        repo: Repo,
        provider: &'static str,
        title: &'static str,
    },
    /// Pick a configuration in the open new-session dialog.
    NewConfiguration(&'static str),
    Submit,
    /// Wait for the dialog to close (session created) and remember the new session.
    WaitCreated(u64),
    CloseModal,
    WaitText(usize, &'static str, u64),
    /// Terminal input as typed keys (sent to the server).
    Write(usize, &'static str),
    /// Text sent from the input box under the terminal (`composer.rs`).
    Compose(usize, &'static str),
    /// Text put in the input box without sending (screenshots of a long draft).
    ComposeDraft(usize, &'static str),
    /// An attachment chip inserted at the input box's cursor.
    ComposeChip(usize, &'static str),
    /// A clipboard image pasted into the input box (synthetic: the real clipboard is the
    /// user's), and a file dropped on the window from this repo.
    ComposeImage(usize),
    DropFile(usize, &'static str),
    /// Wait until the input box's text contains the needle.
    WaitComposer(usize, &'static str, u64),
    Select(usize),
    ExpectText(usize, &'static str),
    /// Exactly `n` visible lines of session `i` contain the text.
    ExpectLines(usize, &'static str, usize),
    ExpectSelected(usize),
    Navigate(String),
    WaitWebTitle(&'static str, u64),
    SetSplit(f32),
    Shot(&'static str),
    /// With `IDE_APP_SCRIPT_PAUSE=1`: log `PAUSE <name>` and hold the screen for a few seconds
    /// so an external read-only window capture can be taken (iced's `window::screenshot` at this
    /// iced revision leaves text inputs empty).
    Pause(&'static str),
    /// Mean brightness of the detail (web) area in screenshot `a` is below `ratio` x the one in
    /// screenshot `b`.
    ExpectDarker(&'static str, &'static str, f32),
    /// Draw the web view under modals (reproduces the z-order bug) or not.
    ModalWebUnsafe(bool),
    /// Like closing the window: drop the client and all app state (sessions keep running).
    CloseApp,
    /// Like starting the app again: new client, state rebuilt from `ui.json` + the server.
    ReopenApp,
    ExpectReloaded,
    /// Simulated network outage: the transport refuses connects and the connection is dropped.
    TransportOffline,
    TransportOnline,
    /// Wait until the footer shows a non-connected state.
    WaitDisconnected(u64),
    /// Wait until connected; `true` = the server must report a resumed client.
    WaitConnected(bool, u64),
    Start(usize),
    WaitRunning(usize, u64),
    /// Stop through the sidebar's stop button.
    Stop(usize),
    WaitStopped(usize, u64),
    AskDelete(usize),
    ConfirmDelete,
    WaitDeleted(usize, u64),
    GitWorktreeList,
    /// Show a page through the navigation rail (no-op when it is already shown, so it never
    /// toggles the session list; use `App(Message::Page(..))` for a real click).
    GoPage(Page),
    ExpectPage(Page),
    /// The session list is open (or hidden) and the terminal's width follows.
    ExpectSessionsOpen(bool),
    /// The session list's groups (key, titles in order); an empty title list skips the titles.
    ExpectSidebar(&'static [(&'static str, &'static [&'static str])]),
    /// `ui.json` stores this page and session-list state.
    ExpectSavedRail(Page, bool),
    /// The rail's work button on the work page: (session list open, hovered) -> the
    /// collapse / expand icon while hovered and the hide / show tooltip.
    ExpectWorkToggle(bool, bool),
    /// The footer's server indicator contains the text.
    ExpectServerTooltip(&'static str),
    /// The registered repos arrived (repos page).
    WaitReposLoaded(u64),
    /// Repos page: type a path and register it.
    RegisterTyped(Repo),
    /// The repo is registered (listed on the repos page and in `repos.json`).
    WaitRepoRegistered(Repo, u64),
    /// The repos page shows an error containing the text.
    ExpectReposError(&'static str, u64),
    /// Repos page: unregister a repo (`Repo2`), first click + confirm.
    Unregister(Repo),
    /// The repo is no longer registered.
    WaitRepoGone(Repo, u64),
    ExpectSettingsError(&'static str),
    /// Configurations page: add a configuration by name.
    CfgCreate(&'static str),
    /// The configuration slug is listed (or not).
    WaitConfig(&'static str, bool, u64),
    /// Open the editor of a configuration and fill instructions, one skill folder (the test
    /// repo) and one MCP server like a user would.
    CfgFill(&'static str),
    CfgSave,
    /// The saved configuration has the filled values (server list and `configurations.json`).
    WaitConfigSaved(&'static str, u64),
    CfgDelete(&'static str),
    /// Accounts page: add an account (provider, name): creates it and opens its login terminal.
    AcctAdd(&'static str, &'static str),
    /// A login session appeared and is selected; remember it as the next session index.
    WaitLogin(u64),
    /// The account is listed (or not) and its dir exists (or not).
    WaitAccount(&'static str, &'static str, bool, u64),
    AcctRename(&'static str, &'static str, &'static str),
    WaitAccountName(&'static str, &'static str, &'static str, u64),
    AcctStatus(&'static str, &'static str),
    WaitAccountStatus(&'static str, &'static str, u64),
    AcctDelete(&'static str, &'static str),
    /// Create an account without opening its login (for the Claude first-run check).
    AcctCreateQuiet(&'static str, &'static str),
    ListAccountDir(&'static str, &'static str),
    TasklistMark,
    TasklistDiff,
    /// Switch the UI language through the settings message.
    SetLang(LanguagePref),
    /// The current language is `Lang` and `tr::new_session()` reads the given text.
    ExpectLang(Lang, &'static str),
    /// `ui.json` on disk stores this language choice.
    ExpectSavedLang(LanguagePref),
    /// Remember the daemon pid from `<data>/ide-server.pid`.
    MarkDaemon,
    AskStopServer,
    ConfirmStopServer,
    /// The remembered daemon exited and its pid file is gone.
    WaitDaemonGone(u64),
    Retry,
    /// Settings page: choose SSH with this host (`None` = local); does not apply.
    SettingsServer(Option<&'static str>),
    ApplyServer,
    ConfirmSwitch,
    /// `ui.json` stores SSH (`true`) or local.
    ExpectSavedServer(bool),
    /// The footer names this server.
    ExpectFooter(&'static str),
    ExpectNotConnected,
    /// No `ide-server` child of this app (SSH selected: nothing local is started).
    ExpectNoServerChild,
    /// Like clicking the window's close button.
    CloseWindow,
    ExpectCloseDialog,
    CloseRemember(bool),
    CloseStop,
    ExpectSavedOnClose(CloseAction),
    Exit,

    // ---- phase 2: files, changes, search, definitions (on the phase 2 session)
    /// The repos page's folder picker shows the start list with the test repo's drive.
    WaitPickStart(u64),
    /// Walk the picker from the drive (never the home shortcut) down to the test repo
    /// (`true`) or its parent (`false`) by opening one listed folder per level, like
    /// double-clicks.
    PickWalk(bool, u64),
    /// Highlight the test repo's entry in the picker (it lists the parent).
    PickHighlightRepo,
    /// Phase 2 terminal: `cd` into the session's first worktree (the shell starts in the
    /// session folder).
    CdRepoP2,
    /// ---- multi-repo: the second test repo exists.
    EnsureRepo2,
    /// Configurations page: open the editor and make both test repos the explicit defaults.
    CfgDefaults(&'static str),
    /// The saved configuration has both test repos as default repos.
    WaitCfgDefaults(&'static str, u64),
    /// The open new-session dialog has exactly the two test repos checked.
    ExpectNewRepos(usize),
    /// The phase 2 session has `n` repos, each a worktree folder on branch `ide/<id>`.
    WaitSessionRepos(usize, u64),
    /// Show repo 1 or 2 in the phase 2 session's Changes / Explore.
    PickRepo(Repo),
    /// Write a file into the phase 2 session's worktree of a repo (an external change).
    WriteInRepo(Repo, &'static str),
    /// The text search results come from `n` repos.
    ExpectSearchRepos(usize),
    /// Session repo dialog: detach / attach the second repo.
    DetachRepo2,
    AttachRepo2,
    /// Remember the phase 2 session's folder, worktrees and branch for [`Step::ExpectCleaned`].
    MarkCleanup,
    /// The deleted session's folder, worktrees and branches are gone; the repos are intact.
    ExpectCleaned,
    /// Remember the newest session as the phase 2 session.
    MarkP2,
    /// A detail message to the phase 2 session.
    D(DetailMsg),
    /// An app message.
    App(Message),
    TreeReveal(&'static str),
    TreeKey(Named),
    /// Ctrl+<char> on the tree (copy / cut / paste).
    TreeCtrl(&'static str),
    /// Drag an entry onto the row of another (its folder) inside the tree.
    TreeDrag(&'static str, &'static str),
    /// Open the tree's context menu on an entry (None: close it).
    TreeMenu(Option<&'static str>),
    TreeRename(&'static str, &'static str),
    WaitDir(&'static str, u64),
    /// The file is open, shown and loaded.
    WaitFile(&'static str, u64),
    TypeIn(&'static str, &'static str),
    /// Put the cursor at 1-based line / column.
    GoTo(&'static str, usize, usize),
    /// Put the cursor on the first occurrence of the needle (plus a char offset) and Ctrl+click.
    CtrlClickOn(&'static str, &'static str, usize),
    Code(&'static str, CodeMessage),
    ExpectDirty(&'static str, bool),
    /// The buffer was saved and the file on disk contains (or not) the needle.
    WaitSaved(&'static str, &'static str, bool, u64),
    WaitBuffer(&'static str, &'static str, u64),
    WaitBar(&'static str, Option<FileBar>, u64),
    WaitActive(ItemKey, u64),
    WaitChange(&'static str, u64),
    OpenChange(ChangeGroup, &'static str),
    WaitDiff(u64),
    WaitSearch(usize, u64),
    /// Explore is shown with a query and its file-name matches include the path.
    WaitExploreFiles(&'static str, u64),
    /// Explore is shown without a query: the tree, no search results.
    ExpectExploreTree,
    OpenFirstHit(&'static str),
    ExpectCursorLine(&'static str, usize),
    WaitQuick(&'static str, u64),
    WaitToast(u64),
    /// A path in the phase 2 worktree exists (or not).
    WaitExists(&'static str, bool, u64),
    ExpectSavedDetail(&'static str),
    ExpectDeleteModal,
    /// The item is open (e.g. it followed a rename).
    ExpectOpen(ItemKey),
    /// Write raw bytes to the phase 2 session terminal.
    WriteP2(&'static str),
    /// The "Git 저장소가 아니에요" (init) dialog is shown for the plain folder.
    ExpectInitDialog(u64),
    /// The plain folder has (or not) a `.git` (initialised: branch main, "Initial commit").
    ExpectPlainInitialised(bool),
    /// The test repo gets a local bare `origin` (`<data>/script-origin.git`) with `main`,
    /// `release/2026.9.0` and `release/2026.19.0`.
    EnsureOrigin,
    /// Point the test repo's origin at a missing folder (`true`) or back (`false`).
    SetOriginUrl(bool),
    /// Show a repo on the repos page.
    SelectRepo(Repo),
    /// The repos page base editor listed the refs.
    WaitBaseRefs(u64),
    /// The rule preview picked this branch.
    WaitBasePreview(&'static str, u64),
    /// The repo's default base for new sessions (`None` = automatic).
    WaitRepoDefault(Repo, Option<&'static str>, u64),
    /// 기준 브랜치 최신화 in the new-session dialog.
    ExpectNewRefresh(bool),
    /// The phase 2 session's base of the test repo.
    WaitP2Base(&'static str, u64),
    /// Open the phase 2 session's repos dialog and pick this base for the test repo.
    SessionBaseP2(&'static str),
    /// The app banner contains the needle.
    ExpectBanner(&'static str, u64),
    /// Commit `(path, content)` with this subject in the phase 2 worktree (only that path).
    CommitP2(&'static str, &'static str, &'static str),
    /// History lists this subject first (newest first) with older commits below it.
    WaitHistory(&'static str, u64),
    /// History marks this many commits as not pushed (and the tab badge shows it).
    WaitUnpushed(usize, u64),
    /// Unfold the History commit with this subject.
    ExpandCommit(&'static str),
    /// The unfolded commit lists the file.
    WaitCommitFiles(&'static str, &'static str, u64),
    /// Open the commit with this subject (one of its files, or the whole commit).
    OpenCommitP2(&'static str, Option<&'static str>),
    /// The commit diff (of the file, or the whole commit) is shown with this added line and
    /// remembered in `ui.json`.
    WaitCommitDiff(Option<&'static str>, &'static str, u64),
    /// Changes / History tab actions (`script_changes.rs`).
    Chg(changes::CStep),
    /// Changes / History footer (`script_footer.rs`).
    Foot(footer_steps::FStep),
    /// Detail window steps.
    Pop(detail_window_steps::PStep),
}

/// SSH host that never resolves (`.invalid` is reserved).
const UNREACHABLE: &str = "ide-test@unreachable.invalid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repo {
    Test,
    /// The second test repo (multi-repo checks).
    Second,
    PlainFolder,
}

pub struct Script {
    steps: Vec<Step>,
    pc: usize,
    started: Option<Instant>,
    waiting_shot: bool,
    ids: Vec<String>,
    /// Ids of sessions the script deleted.
    gone: Vec<String>,
    repo: PathBuf,
    evidence: PathBuf,
    data: PathBuf,
    pass: u32,
    fail: u32,
    done: bool,
    pids_before: Vec<(String, u32)>,
    /// Mean brightness of the detail area per screenshot.
    luma: HashMap<String, f32>,
    daemon_pid: Option<u32>,
    probe: Probe,
    children: Option<std::sync::mpsc::Receiver<String>>,
    finished: bool,
    /// Index of the phase 2 session in `ids`.
    p2: Option<usize>,
    /// Second test repo.
    repo2: PathBuf,
    /// [`Step::MarkCleanup`]: (session folder, worktrees, branch).
    cleanup: Option<(PathBuf, Vec<PathBuf>, String)>,
    /// The last screenshot (name, pixels).
    last_shot: Option<(String, window::Screenshot)>,
}

fn data_url(html: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("data:text/html;charset=utf-8;base64,");
    for c in html.as_bytes().chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>ide detail tab</title>
<style>body{margin:0;font:15px "Segoe UI",sans-serif;background:linear-gradient(160deg,#3a5a8c,#7a4a9c);color:#f4f4f4;height:100vh}
.c{padding:28px}h1{margin:0 0 8px;font-weight:600}code{background:#0006;padding:2px 6px;border-radius:4px}
.dot{display:inline-block;width:12px;height:12px;border-radius:6px;background:#3fb950;margin-right:8px;animation:p 1s infinite alternate}
@keyframes p{to{opacity:.3}}</style></head><body><div class="c"><h1><span class="dot"></span>Per-session web tab</h1>
<p>This page is a <code>data:</code> URL loaded in the session's own CEF view.</p>
<p>Switching sessions hides this view; its URL is remembered in <code>ui.json</code>.</p></div></body></html>"#;

impl Script {
    pub fn from_env(data: &Path) -> Option<Script> {
        let mode = std::env::var("IDE_APP_SCRIPT")
            .ok()
            .filter(|s| !s.is_empty() && s != "0")?;
        let evidence = std::env::var_os("IDE_APP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| data.join("evidence"));
        let _ = std::fs::create_dir_all(&evidence);
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(evidence.join("script.log"))
            .ok()?;
        let _ = LOG.set(Mutex::new(f));
        T0.get_or_init(Instant::now);
        let repo = std::env::var_os("IDE_APP_TEST_REPO")
            .map(PathBuf::from)
            .unwrap_or_else(|| data.join("test-repo"));
        let repo2 = std::env::var_os("IDE_APP_TEST_REPO2")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                repo.parent()
                    .unwrap_or(data)
                    .join("lib-repo")
            });
        let claude = std::env::var("IDE_APP_SCRIPT_CLAUDE").is_ok_and(|v| v == "1");
        install_fake_provider(data);
        // `IDE_APP_SCRIPT_ONLY=p2`: startup, the phase 2 checks and the server stop only.
        let only = std::env::var("IDE_APP_SCRIPT_ONLY").unwrap_or_default();
        let only_p2 = only == "p2";
        // `IDE_APP_SCRIPT_ONLY=pop`: startup, the detail window checks and the server stop.
        let only_pop = only == "pop";
        log(&format!(
            "script {mode:?}: data dir {}, repo {}, evidence {}, claude {claude}",
            data.display(),
            repo.display(),
            evidence.display()
        ));
        Some(Script {
            steps: {
                let mut s = if only_pop {
                    detail_window_steps::steps_only()
                } else if only_p2 {
                    steps_p2_only()
                } else {
                    steps(claude)
                };
                if !only_pop {
                    changes::splice(&mut s);
                    footer_steps::splice(&mut s);
                    detail_window_steps::splice(&mut s);
                }
                s
            },
            pc: 0,
            started: None,
            waiting_shot: false,
            ids: Vec::new(),
            gone: Vec::new(),
            repo,
            evidence,
            data: data.to_path_buf(),
            pass: 0,
            fail: 0,
            done: false,
            pids_before: Vec::new(),
            luma: HashMap::new(),
            daemon_pid: None,
            probe: Probe::default(),
            children: None,
            finished: false,
            p2: None,
            repo2,
            cleanup: None,
            last_shot: None,
        })
    }

    pub fn done(&self) -> bool {
        self.done
    }

    fn id(&self, i: usize) -> Option<String> {
        let i = if i == P2 { self.p2? } else { i };
        self.ids.get(i).cloned()
    }

    /// Folder of a test repo (`PlainFolder`: a plain folder, created on demand).
    fn repo_path(&self, r: Repo) -> PathBuf {
        match r {
            Repo::Test => self.repo.clone(),
            Repo::Second => self.repo2.clone(),
            Repo::PlainFolder => {
                let p = self.data.join("not-a-repo");
                let _ = std::fs::create_dir_all(&p);
                p
            }
        }
    }

    fn repo_parent(&self) -> String {
        self.repo
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }

    fn repo_name(&self) -> String {
        self.repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn check(&mut self, ok: bool, what: &str) {
        if ok {
            self.pass += 1;
            log(&format!("PASS {what}"));
        } else {
            self.fail += 1;
            log(&format!("FAIL {what}"));
        }
    }
}

fn steps(claude: bool) -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup: connect to the local server"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        WaitReady(30_000),
        MarkDaemon,
        EnsureRepo,
        Wait(500),
        Shot("01_empty"),
        Log("--- repos page: register the test repo (error path: not a git repo)"),
        GoPage(Page::Repos),
        RegisterTyped(Repo::PlainFolder),
        ExpectInitDialog(15_000),
        Wait(300),
        Shot("02_register_not_a_repo"),
        CloseModal,
        ExpectPlainInitialised(false),
        RegisterTyped(Repo::Test),
        WaitRepoRegistered(Repo::Test, 15_000),
        GoPage(Page::Work),
        Log("--- create two shell sessions through the dialog"),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "shell A",
        },
        Wait(300),
        Shot("03_new_session_dialog"),
        Pause("new_session_dialog"),
        Submit,
        WaitCreated(20_000),
        WaitText(0, ">", 15_000),
        Write(0, "echo hello from A\r"),
        WaitText(0, "hello from A", 5000),
        // The input box under the terminal: two lines (cmd has no bracketed paste, so each
        // runs as typed).
        Compose(0, "echo composer one\necho composer two"),
        WaitText(0, "composer two", 5000),
        ExpectText(0, "composer one"),
        // One line: the send button sits with the same gap on all sides.
        ComposeDraft(0, "look at "),
        ComposeChip(0, "C:\\shots\\screen shot.png"),
        // A pasted screenshot goes to the server (attach.put) and comes back as a chip; a
        // dropped file of this machine (local server) keeps its own path.
        ComposeImage(0),
        WaitComposer(0, "screenshot-", 5000),
        DropFile(0, "README.md"),
        WaitComposer(0, "README.md", 3000),
        Wait(300),
        Pause("composer_one_line"),
        // A long draft: the box stops growing and scrolls (its last lines in view).
        ComposeDraft(0, "draft line 1\ndraft line 2\ndraft line 3\ndraft line 4\ndraft line 5\ndraft line 6\ndraft line 7\ndraft line 8\ndraft line 9\ndraft line 10\ndraft line 11\ndraft line 12\ndraft line 13\ndraft line 14\ndraft line 15"),
        Wait(400),
        Shot("03b_composer_long_draft"),
        Pause("composer_long_draft"),
        ComposeDraft(0, ""),
        // The shell starts in the session folder; the repo is a worktree below it.
        Write(0, "dir /b /s README.md\r"),
        WaitText(0, "README.md", 5000),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "shell B",
        },
        Submit,
        WaitCreated(20_000),
        WaitText(1, ">", 15_000),
        Write(1, "echo hello from B\r"),
        WaitText(1, "hello from B", 5000),
        Write(1, "dir /b /s README.md\r"),
        WaitText(1, "README.md", 5000),
        Wait(500),
        Shot("04_session_b"),
        Log("--- switch sessions: background terminals stay attached"),
        Select(0),
        Wait(600),
        ExpectSelected(0),
        ExpectText(0, "hello from A"),
        ExpectText(0, "README.md"),
        Shot("05_switched_to_a"),
        Write(1, "echo B kept running in the background\r"),
        Wait(800),
        ExpectText(1, "B kept running in the background"),
        Select(1),
        Wait(400),
        ExpectText(1, "hello from B"),
        Select(0),
        Wait(300),
        ExpectText(0, "hello from A"),
        Log("--- detail web tab"),
        Navigate(data_url(PAGE)),
        WaitWebTitle("ide detail tab", 30_000),
        SetSplit(0.55),
        Wait(1500),
        Shot("06_web_tab"),
        Pause("web_tab"),
        Select(1),
        Wait(800),
        Shot("07_session_b_own_web_tab"),
        Select(0),
        Wait(1500),
        Log("--- modal over the web view (z-order fix)"),
        ModalWebUnsafe(true),
        App(Message::OpenNew),
        Wait(1000),
        Shot("08a_modal_over_web_unfixed"),
        Pause("modal_over_web_unfixed"),
        CloseModal,
        ModalWebUnsafe(false),
        Wait(800),
        App(Message::OpenNew),
        Wait(1000),
        Shot("08b_modal_over_web_fixed"),
        Pause("modal_over_web_fixed"),
        ExpectDarker("08b_modal_over_web_fixed", "06_web_tab", 0.7),
        CloseModal,
        Wait(800),
        Shot("08c_modal_closed_web_back"),
        Log("--- navigation rail: pages cover the work page (and its web view)"),
        ExpectPage(Page::Work),
        ExpectSessionsOpen(true),
        ExpectWorkToggle(true, false),
        App(Message::RailHover(true)),
        ExpectWorkToggle(true, true),
        Wait(300),
        Shot("rail_01_work_hovered"),
        App(Message::RailHover(false)),
        ExpectServerTooltip("Server: Local · connected · "),
        GoPage(Page::Settings),
        ExpectPage(Page::Settings),
        Wait(1000),
        Shot("rail_02_page_over_web"),
        ExpectDarker("rail_02_page_over_web", "06_web_tab", 0.5),
        Log("--- clicking another page's active button does not toggle the session list"),
        App(Message::Page(Page::Settings)),
        ExpectPage(Page::Settings),
        ExpectSessionsOpen(true),
        GoPage(Page::Work),
        Wait(800),
        Shot("rail_03_back_to_work_web"),
        Log("--- the active work button toggles the session list (saved in ui.json)"),
        App(Message::Page(Page::Work)),
        ExpectPage(Page::Work),
        ExpectSessionsOpen(false),
        ExpectWorkToggle(false, false),
        App(Message::RailHover(true)),
        ExpectWorkToggle(false, true),
        App(Message::RailHover(false)),
        ExpectSavedRail(Page::Work, false),
        Wait(500),
        Shot("rail_04_sessions_hidden"),
        ExpectText(0, "hello from A"),
        App(Message::Page(Page::Work)),
        ExpectSessionsOpen(true),
        ExpectSavedRail(Page::Work, true),
        Log("--- configurations"),
        GoPage(Page::Configurations),
        ExpectSavedRail(Page::Configurations, true),
        CfgCreate("Work"),
        WaitConfig("work", true, 5000),
        CfgFill("work"),
        Wait(400),
        Shot("09_configuration_editor"),
        Pause("configuration_editor"),
        CfgSave,
        WaitConfigSaved("work", 5000),
        CfgDelete("default"),
        Wait(1000),
        ExpectSettingsError("profile “default”; delete them first"),
        GoPage(Page::Work),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "work config",
        },
        NewConfiguration("work"),
        Wait(400),
        Shot("09a_new_session_configuration"),
        Pause("new_session_configuration"),
        CloseModal,
        Log("--- accounts: add -> login terminal (fake provider), rename, status, delete"),
        GoPage(Page::Accounts),
        AcctAdd("fake", "Script login"),
        WaitLogin(15_000),
        WaitText(2, "FAKE-LOGIN-OK", 15_000),
        Wait(500),
        Shot("09b_login_terminal"),
        GoPage(Page::Accounts),
        WaitAccount("fake", "script-login", true, 5000),
        AcctRename("fake", "script-login", "Renamed login"),
        WaitAccountName("fake", "script-login", "Renamed login", 5000),
        AcctStatus("fake", "script-login"),
        WaitAccountStatus("fake", "script-login", 15_000),
        AcctDelete("fake", "script-login"),
        Wait(1000),
        ExpectSettingsError("account “Renamed login”; delete them first"),
        Wait(300),
        Shot("09c_accounts"),
        Pause("accounts"),
        GoPage(Page::Work),
        Log("--- session list: 전체 groups by configuration, sort criteria, folding"),
        App(Message::ConfigurationPicked(ALL_CONFIGURATIONS.into())),
        ExpectSidebar(&[("default", &["shell A", "shell B"]), (LOGIN_GROUP, &[])]),
        App(Message::Sort(SortChange::Add(SortKey::Name))),
        App(Message::Sort(SortChange::Flip(SortKey::Name))),
        ExpectSidebar(&[("default", &["shell B", "shell A"]), (LOGIN_GROUP, &[])]),
        App(Message::Sort(SortChange::Add(SortKey::Created))),
        App(Message::Sort(SortChange::Move(SortKey::Created, -1))),
        App(Message::Sort(SortChange::Flip(SortKey::Created))),
        ExpectSidebar(&[("default", &["shell A", "shell B"]), (LOGIN_GROUP, &[])]),
        App(Message::ToggleSortMenu),
        Wait(400),
        Shot("09d_session_sort_menu"),
        Pause("session_sort_menu"),
        App(Message::CloseSortMenu),
        App(Message::ToggleGroupFold(LOGIN_GROUP.into())),
        Wait(300),
        Shot("09e_session_groups_folded"),
        Pause("session_groups"),
        App(Message::ToggleGroupFold(LOGIN_GROUP.into())),
        App(Message::Sort(SortChange::Reset)),
        App(Message::ConfigurationPicked("default".into())),
        ExpectSidebar(&[("", &["shell A", "shell B"]), (LOGIN_GROUP, &[])]),
        AskDelete(2),
        ConfirmDelete,
        WaitDeleted(2, 15_000),
        GoPage(Page::Accounts),
        AcctDelete("fake", "script-login"),
        WaitAccount("fake", "script-login", false, 5000),
        GoPage(Page::Configurations),
        CfgDelete("work"),
        WaitConfig("work", false, 5000),
        GoPage(Page::Work),
        Select(0),
        Log("--- i18n: Korean UI, then back to English"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        ExpectLang(Lang::Ko, "새 세션"),
        ExpectSavedLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(500),
        Shot("ko_01_main"),
        Pause("ko_main"),
        App(Message::ConfigurationPicked(ALL_CONFIGURATIONS.into())),
        App(Message::Sort(SortChange::Add(SortKey::Updated))),
        App(Message::Sort(SortChange::Add(SortKey::Name))),
        App(Message::ToggleSortMenu),
        Wait(400),
        Shot("ko_01b_session_sort"),
        Pause("ko_session_sort"),
        App(Message::CloseSortMenu),
        App(Message::Sort(SortChange::Reset)),
        App(Message::ConfigurationPicked("default".into())),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "한국어 세션",
        },
        Wait(300),
        Shot("ko_02_new_session_dialog"),
        Pause("ko_new_session_dialog"),
        CloseModal,
        GoPage(Page::Repos),
        Log("--- plain folder: Desk's git init question, then init + register"),
        RegisterTyped(Repo::PlainFolder),
        ExpectInitDialog(15_000),
        Wait(300),
        Shot("ko_03_init_repo_dialog"),
        Pause("ko_init_repo_dialog"),
        App(Message::RepoInitConfirm),
        WaitRepoRegistered(Repo::PlainFolder, 20_000),
        ExpectPlainInitialised(true),
        Wait(300),
        Shot("ko_03b_plain_folder_registered"),
        Unregister(Repo::PlainFolder),
        WaitRepoGone(Repo::PlainFolder, 10_000),
        GoPage(Page::Work),
        Log("--- Korean pages of the navigation rail"),
        ExpectServerTooltip("서버: 로컬 · 연결됨 · "),
        Wait(300),
        Shot("ko_rail_1_work"),
        Pause("ko_rail_1_work"),
        App(Message::Page(Page::Work)),
        ExpectWorkToggle(false, false),
        App(Message::RailHover(true)),
        ExpectWorkToggle(false, true),
        Wait(300),
        Shot("ko_rail_2_sessions_hidden"),
        Pause("ko_rail_2_sessions_hidden"),
        App(Message::RailHover(false)),
        App(Message::Page(Page::Work)),
        ExpectSessionsOpen(true),
        GoPage(Page::Accounts),
        Wait(300),
        Shot("ko_rail_3_accounts"),
        Pause("ko_rail_3_accounts"),
        GoPage(Page::Configurations),
        Wait(300),
        Shot("ko_rail_4_configurations"),
        Pause("ko_rail_4_configurations"),
        GoPage(Page::Repos),
        Wait(800),
        Shot("ko_rail_5_repos"),
        Pause("ko_rail_5_repos"),
        GoPage(Page::Settings),
        Wait(300),
        Shot("ko_04_settings"),
        Pause("ko_rail_6_settings"),
        AskStopServer,
        Wait(300),
        Shot("ko_05_stop_server_confirm"),
        CloseModal,
        GoPage(Page::Work),
        TransportOffline,
        WaitDisconnected(5000),
        Wait(500),
        Shot("ko_06_disconnected"),
        Pause("ko_disconnected"),
        TransportOnline,
        WaitConnected(true, 20_000),
        SetLang(LanguagePref::Fixed(Lang::En)),
        ExpectLang(Lang::En, "New session"),
        ExpectSavedLang(LanguagePref::Fixed(Lang::En)),
        Log("--- close the app while a session runs ping; reopen: full output via replay"),
        Write(0, "cls\r"),
        Wait(500),
        Write(0, "ping -n 6 127.0.0.1 & echo PING-%OS%\r"),
        WaitText(0, "TTL=", 10_000),
        // The reopened app restores the page and the hidden session list from ui.json.
        App(Message::Page(Page::Work)),
        GoPage(Page::Repos),
        CloseApp,
        Wait(300),
        Shot("10_closed"),
        Wait(7000),
        ReopenApp,
        WaitReady(30_000),
        ExpectPage(Page::Repos),
        ExpectSessionsOpen(false),
        WaitReposLoaded(10_000),
        GoPage(Page::Work),
        App(Message::Page(Page::Work)),
        ExpectSessionsOpen(true),
        ExpectReloaded,
        WaitText(0, "PING-Windows_NT", 20_000),
        ExpectLines(0, "TTL=", 6),
        WaitWebTitle("ide detail tab", 30_000),
        Wait(800),
        Shot("11_reopened_replayed"),
        Log("--- kill the transport: automatic reconnect + exact resume"),
        Write(0, "cls\r"),
        Wait(500),
        Write(0, "ping -n 5 127.0.0.1 & echo NET-%OS%\r"),
        WaitText(0, "TTL=", 10_000),
        TransportOffline,
        WaitDisconnected(5000),
        Wait(400),
        Shot("12_transport_down"),
        Pause("transport_down"),
        Wait(3500),
        TransportOnline,
        WaitConnected(true, 20_000),
        WaitText(0, "NET-Windows_NT", 20_000),
        ExpectLines(0, "TTL=", 5),
        Wait(500),
        Shot("13_resumed"),
        Log("--- stop / start in the same worktree"),
        Stop(0),
        WaitStopped(0, 15_000),
        Wait(300),
        Shot("14_stopped"),
        Start(0),
        WaitRunning(0, 15_000),
        WaitText(0, "Microsoft", 15_000),
        Write(0, "echo restarted in the same worktree & cd\r"),
        WaitText(0, "restarted in the same worktree", 5000),
        Wait(500),
        Shot("15_restarted"),
        Log("--- delete session B with its worktree"),
        AskDelete(1),
        Wait(300),
        Shot("16_delete_confirm"),
        Pause("delete_confirm"),
        ConfirmDelete,
        WaitDeleted(1, 15_000),
        GitWorktreeList,
        Wait(500),
        Shot("17_after_delete"),
    ];
    if claude {
        s.extend([
            Log("--- Claude session with a script account's CLAUDE_CONFIG_DIR (no input is sent)"),
            AcctCreateQuiet("claude", "Script first-run"),
            WaitAccount("claude", "script-first-run", true, 5000),
            TasklistMark,
            FillNew {
                repo: Repo::Test,
                provider: "claude",
                title: "claude first-run",
            },
            Submit,
            WaitCreated(20_000),
            Wait(12_000),
            Shot("18_claude_first_run"),
            ListAccountDir("claude", "script-first-run"),
            Stop(3),
            WaitStopped(3, 10_000),
            Wait(1500),
            TasklistDiff,
            AskDelete(3),
            ConfirmDelete,
            WaitDeleted(3, 15_000),
            GitWorktreeList,
        ]);
    }
    s.extend(phase2());
    s.extend([
        Log("--- stop the server from the settings page (sessions end, daemon exits)"),
        GoPage(Page::Settings),
        AskStopServer,
        Wait(300),
        Shot("19_stop_server_confirm"),
        Pause("stop_server_confirm"),
        ConfirmStopServer,
        WaitDisconnected(15_000),
        WaitDaemonGone(10_000),
        ExpectServerTooltip("Server: Local · disconnected"),
        Wait(300),
        Shot("20_server_stopped"),
        Log("--- retry starts a new server; the session is still listed (stopped)"),
        Retry,
        WaitConnected(false, 30_000),
        WaitReady(10_000),
        MarkDaemon,
        WaitStopped(0, 5000),
        GoPage(Page::Work),
        Wait(500),
        Shot("21_new_server"),
        Log("--- server setting: SSH to an unreachable host (no local server is started)"),
        App(Message::Page(Page::Settings)),
        SettingsServer(Some(UNREACHABLE)),
        Wait(300),
        Shot("22_settings_ssh"),
        Pause("settings_ssh"),
        ApplyServer,
        Wait(300),
        Shot("23_switch_server_confirm"),
        ConfirmSwitch,
        ExpectSavedServer(true),
        ExpectFooter("ssh ide-test@unreachable.invalid"),
        WaitDisconnected(5000),
        Wait(6000),
        ExpectNoServerChild,
        ExpectNotConnected,
        Shot("24_ssh_unreachable"),
        Pause("ssh_unreachable"),
        ExpectPage(Page::Settings),
        ExpectServerTooltip("Server: ssh ide-test@unreachable.invalid"),
        Log("--- back to the local server (the local daemon kept running)"),
        GoPage(Page::Settings),
        SettingsServer(None),
        ApplyServer,
        ConfirmSwitch,
        ExpectSavedServer(false),
        WaitReady(30_000),
        ExpectFooter("Local"),
        Log("--- close the window while a session runs: dialog, stop everything"),
        Start(0),
        WaitRunning(0, 15_000),
        CloseWindow,
        ExpectCloseDialog,
        CloseRemember(true),
        Wait(300),
        Shot("25_close_dialog"),
        Pause("close_dialog"),
        CloseStop,
        ExpectSavedOnClose(CloseAction::Stop),
        // The app exits once the server confirmed the shutdown (DONE is logged on exit).
        Wait(60_000),
        Log("FAIL the app did not exit after the close dialog"),
        Exit,
    ]);
    s
}

/// Startup, phase 2 and the server stop (`IDE_APP_SCRIPT_ONLY=p2`).
fn steps_p2_only() -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup (phase 2 only)"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        WaitReady(30_000),
        MarkDaemon,
        EnsureRepo,
        GoPage(Page::Repos),
        RegisterTyped(Repo::Test),
        WaitRepoRegistered(Repo::Test, 15_000),
        GoPage(Page::Work),
    ];
    s.extend(phase2());
    s.extend([
        GoPage(Page::Settings),
        AskStopServer,
        ConfirmStopServer,
        WaitDisconnected(15_000),
        WaitDaemonGone(10_000),
        Exit,
    ]);
    s
}

/// History fixture: a commit made in the phase 2 worktree.
const HIST_FILE: &str = "src/history.ts";
const HIST_LINE: &str = "export const history = 'hist-line'\n";
const HIST_ADDED: &str = "export const history = 'hist-line'";
const HIST_SUBJECT: &str = "Add history fixture";

/// Phase 2: browse roots, the server-side folder browser, the detail area (tree, editor,
/// live refresh, save conflicts, changes + diffs, search, quick open, go to definition,
/// rename / delete) and a Korean pass.
fn phase2() -> Vec<Step> {
    use Step::*;
    let main = "src/main.ts";
    let util = "src/util.ts";
    let memo = "한글 메모.txt";
    let renamed = "메모-바뀜.txt";
    let enter = |n: Named| TreeKey(n);
    vec![
        Log("--- phase 2: repos page folder picker (browse.pick_list) registers a repo"),
        GoPage(Page::Repos),
        WaitRepoRegistered(Repo::Test, 5000),
        Wait(300),
        Shot("30_repos_page"),
        Pause("repos_page"),
        App(Message::PickOpen),
        WaitPickStart(10_000),
        Wait(200),
        Shot("30a_repo_picker_start"),
        PickWalk(true, 20_000),
        Wait(200),
        Shot("30b_repo_picker_repo"),
        App(Message::PickConfirm),
        WaitRepoRegistered(Repo::Test, 5000),
        GoPage(Page::Work),
        Log("--- new session with the registered repo"),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "phase 2",
        },
        Wait(200),
        Shot("31_new_session_repos"),
        Pause("new_session_repos"),
        Submit,
        WaitCreated(20_000),
        MarkP2,
        WaitText(P2, ">", 15_000),
        CdRepoP2,
        Log("--- explorer: lazy tree, open a file"),
        D(DetailMsg::Side(SideTab::Explore)),
        WaitDir("", 10_000),
        TreeReveal("src/main.ts"),
        WaitDir("src", 10_000),
        TreeReveal("src/main.ts"),
        enter(Named::Enter),
        WaitFile(main, 10_000),
        Wait(400),
        Shot("33_tree_file_open"),
        Pause("tree_file_open"),
        Log("--- edit + save (fs.write with the read version)"),
        GoTo(main, 1, 1),
        TypeIn(main, "// edited in ide\n"),
        ExpectDirty(main, true),
        Wait(200),
        Shot("34_dirty"),
        Pause("dirty"),
        Code(main, CodeMessage::Save),
        WaitSaved(main, "// edited in ide", true, 10_000),
        Log("--- external change (terminal) on a clean buffer: reload + tree/changes refresh"),
        WriteP2("echo external-1>> src\\main.ts\r"),
        WaitBuffer(main, "external-1", 10_000),
        D(DetailMsg::Side(SideTab::Changes)),
        WaitChange(main, 10_000),
        Wait(400),
        Shot("35_external_change_reloaded"),
        Log("--- external change on a dirty buffer: 'changed on disk' bar"),
        D(DetailMsg::Side(SideTab::Explore)),
        GoTo(main, 1, 1),
        TypeIn(main, "// unsaved line\n"),
        ExpectDirty(main, true),
        WriteP2("echo external-2>> src\\main.ts\r"),
        WaitBar(main, Some(FileBar::ChangedOnDisk), 10_000),
        Wait(300),
        Shot("36_changed_on_disk_bar"),
        Pause("changed_on_disk_bar"),
        Log("--- save conflict: inline bar, compare, overwrite"),
        Code(main, CodeMessage::Save),
        WaitBar(main, Some(FileBar::Conflict), 10_000),
        Wait(300),
        Shot("37_save_conflict_bar"),
        D(DetailMsg::BarCompare(main.into())),
        WaitActive(ItemKey::Compare(main.into()), 10_000),
        Wait(400),
        Shot("38_compare_disk_vs_buffer"),
        Pause("compare"),
        D(DetailMsg::Activate(ItemKey::File(main.into()))),
        D(DetailMsg::BarOverwrite(main.into())),
        WaitSaved(main, "// unsaved line", true, 10_000),
        WaitSaved(main, "external-2", false, 10_000),
        WaitBar(main, None, 5000),
        Log("--- changes -> diff (unified / split)"),
        D(DetailMsg::Side(SideTab::Changes)),
        WaitChange(main, 10_000),
        OpenChange(ChangeGroup::Unstaged, main),
        WaitDiff(10_000),
        Wait(400),
        Shot("39_diff_unified"),
        Pause("diff_unified"),
        D(DetailMsg::Diff(
            ItemKey::Diff {
                staged: false,
                path: main.into(),
            },
            DiffViewMessage::SetMode(DiffMode::Split),
        )),
        Wait(400),
        Shot("40_diff_split"),
        Pause("diff_split"),
        D(DetailMsg::Diff(
            ItemKey::Diff {
                staged: false,
                path: main.into(),
            },
            DiffViewMessage::SetMode(DiffMode::Unified),
        )),
        Log("--- open items list"),
        App(Message::ToggleOpenItems),
        Wait(300),
        Shot("41_open_items"),
        Pause("open_items"),
        App(Message::CloseOpenItems),
        Log("--- Explore search: file names + text, cleared -> tree"),
        App(Message::FindInFiles),
        ExpectExploreTree,
        // Typing only (no Enter): file names at once, the text search after the pause.
        D(DetailMsg::SearchInput("util".into())),
        WaitExploreFiles(util, 10_000),
        WaitSearch(1, 10_000),
        Wait(300),
        Shot("41a_explore_search"),
        Pause("explore_search"),
        D(DetailMsg::SearchInput(String::new())),
        ExpectExploreTree,
        WaitDir("", 10_000),
        Wait(300),
        Shot("41b_explore_cleared"),
        Log("--- text search (streamed), open a result at its line"),
        D(DetailMsg::SearchInput("greet".into())),
        D(DetailMsg::SearchSubmit),
        WaitSearch(2, 10_000),
        Wait(300),
        Shot("42_search_results"),
        Pause("search"),
        OpenFirstHit(util),
        WaitFile(util, 10_000),
        ExpectCursorLine(util, 1),
        Wait(300),
        Shot("43_search_hit_opened"),
        Log("--- Ctrl+P quick open (Korean file name)"),
        App(Message::QuickOpen),
        App(Message::QuickInput("메모".into())),
        WaitQuick(memo, 10_000),
        Wait(300),
        Shot("44_quick_open"),
        Pause("quick_open"),
        App(Message::QuickSubmit),
        WaitFile(memo, 10_000),
        WaitBuffer(memo, "첫 줄", 5000),
        Wait(300),
        Shot("45_korean_file"),
        Log("--- binary file placeholder"),
        D(DetailMsg::Side(SideTab::Explore)),
        TreeReveal("logo.bin"),
        enter(Named::Enter),
        WaitFile("logo.bin", 10_000),
        Wait(300),
        Shot("46_binary_file"),
        Log("--- go to definition across files (unsaved buffer sent)"),
        D(DetailMsg::Activate(ItemKey::File(main.into()))),
        WaitFile(main, 5000),
        CtrlClickOn(main, "greet('world')", 1),
        WaitActive(ItemKey::File(util.into()), 10_000),
        ExpectCursorLine(util, 1),
        Wait(300),
        Shot("47_definition_opened"),
        Pause("definition"),
        TreeReveal("src/notes.md"),
        enter(Named::Enter),
        WaitFile("src/notes.md", 10_000),
        CtrlClickOn("src/notes.md", "notes", 0),
        WaitToast(10_000),
        Wait(200),
        Shot("48_definition_none_toast"),
        Log("--- rename / delete through the tree"),
        D(DetailMsg::Side(SideTab::Explore)),
        TreeRename(memo, renamed),
        WaitExists(renamed, true, 10_000),
        WaitExists(memo, false, 5000),
        ExpectOpen(ItemKey::File(renamed.into())),
        Wait(300),
        Shot("49_renamed"),
        TreeReveal("logo.bin"),
        TreeKey(Named::Delete),
        ExpectDeleteModal,
        Wait(200),
        Shot("50_delete_confirm"),
        Pause("delete_confirm"),
        App(Message::DeleteEntryConfirm),
        WaitExists("logo.bin", false, 10_000),
        Wait(300),
        Shot("51_deleted"),
        Log("--- Explore: copy / paste into a folder, drag a file to the root"),
        TreeReveal("README.md"),
        TreeCtrl("c"),
        TreeReveal("src/main.ts"),
        TreeCtrl("v"),
        WaitExists("src/README.md", true, 10_000),
        TreeCtrl("v"),
        WaitExists("src/README copy.md", true, 10_000),
        TreeDrag("src/README copy.md", "README.md"),
        WaitExists("README copy.md", true, 10_000),
        WaitExists("src/README copy.md", false, 5000),
        Wait(300),
        Shot("51b_explore_moved"),
        TreeMenu(Some("src/main.ts")),
        Wait(300),
        Shot("51c_explore_menu"),
        Pause("explore_menu"),
        TreeMenu(None),
        ExpectSavedDetail("file:src/main.ts"),
        Log("--- History: commits newest first, a commit's files, file and whole-commit diffs"),
        D(DetailMsg::Side(SideTab::History)),
        WaitHistory("init", 10_000),
        // A commit in the worktree refreshes History (git change notification).
        CommitP2(HIST_FILE, HIST_LINE, HIST_SUBJECT),
        WaitHistory(HIST_SUBJECT, 15_000),
        // No remote: nothing is marked as unpushed (Desk's history marks).
        WaitUnpushed(0, 5000),
        ExpandCommit(HIST_SUBJECT),
        WaitCommitFiles(HIST_SUBJECT, HIST_FILE, 10_000),
        Wait(300),
        Shot("52_history_expanded"),
        Pause("history"),
        OpenCommitP2(HIST_SUBJECT, Some(HIST_FILE)),
        WaitCommitDiff(Some(HIST_FILE), HIST_ADDED, 10_000),
        Wait(300),
        Shot("53_history_file_diff"),
        Pause("history_file_diff"),
        OpenCommitP2(HIST_SUBJECT, None),
        WaitCommitDiff(None, HIST_ADDED, 10_000),
        Wait(300),
        Shot("54_history_commit_diff"),
        Log("--- phase 2 Korean pass"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        ExpectLang(Lang::Ko, "새 세션"),
        D(DetailMsg::Side(SideTab::History)),
        WaitHistory(HIST_SUBJECT, 10_000),
        OpenCommitP2(HIST_SUBJECT, Some(HIST_FILE)),
        WaitCommitDiff(Some(HIST_FILE), HIST_ADDED, 10_000),
        Wait(300),
        Shot("ko_hist_01_history"),
        Pause("ko_hist"),
        D(DetailMsg::Side(SideTab::Changes)),
        OpenChange(ChangeGroup::Unstaged, main),
        WaitDiff(10_000),
        Wait(300),
        Shot("ko_10_changes_diff"),
        Pause("ko_changes_diff"),
        App(Message::FindInFiles),
        D(DetailMsg::SearchInput("util".into())),
        WaitExploreFiles(util, 10_000),
        WaitSearch(1, 10_000),
        Wait(300),
        Shot("ko_11_explore_search"),
        Pause("ko_explore_search"),
        D(DetailMsg::SearchInput(String::new())),
        ExpectExploreTree,
        D(DetailMsg::Activate(ItemKey::File(main.into()))),
        WaitFile(main, 5000),
        GoTo(main, 1, 1),
        TypeIn(main, "// 저장 안 한 줄\n"),
        WriteP2("echo external-3>> src\\main.ts\r"),
        WaitBar(main, Some(FileBar::ChangedOnDisk), 10_000),
        Wait(300),
        Shot("ko_12_changed_on_disk"),
        Pause("ko_changed_on_disk"),
        D(DetailMsg::BarReload(main.into())),
        WaitBar(main, None, 10_000),
        ExpectDirty(main, false),
        App(Message::ToggleOpenItems),
        Wait(300),
        Shot("ko_13_open_items"),
        App(Message::CloseOpenItems),
        App(Message::QuickOpen),
        App(Message::QuickInput("util".into())),
        WaitQuick(util, 10_000),
        Wait(300),
        Shot("ko_14_quick_open"),
        CloseModal,
        TreeReveal("src/notes.md"),
        enter(Named::Enter),
        WaitFile("src/notes.md", 10_000),
        CtrlClickOn("src/notes.md", "notes", 0),
        WaitToast(10_000),
        Wait(200),
        Shot("ko_15_definition_none"),
        GoPage(Page::Repos),
        WaitRepoRegistered(Repo::Test, 5000),
        Wait(300),
        Shot("ko_17_repos_page"),
        Pause("ko_repos_page"),
        App(Message::PickOpen),
        WaitPickStart(10_000),
        Wait(300),
        Shot("ko_18_root_picker_drives"),
        Pause("ko_root_picker_drives"),
        PickWalk(false, 20_000),
        PickHighlightRepo,
        Wait(300),
        Shot("ko_19_root_picker_folder"),
        Pause("ko_root_picker_folder"),
        // Enter opens the highlighted folder.
        App(Message::PickEnter),
        PickWalk(true, 5000),
        CloseModal,
        GoPage(Page::Work),
    ]
    .into_iter()
    .chain(multi_repo())
    .chain(base_branches())
    .chain([
        SetLang(LanguagePref::Fixed(Lang::En)),
        ExpectLang(Lang::En, "New session"),
    ])
    .collect()
}

/// Multi-repo (in Korean): a second repo registered, both as a configuration's default
/// repos, a session with both worktrees, Changes / Explore switching repos, search over both,
/// detach / attach, unregister refused while used, delete cleanup.
fn multi_repo() -> Vec<Step> {
    use Step::*;
    vec![
        Log("--- multi-repo: register a second repo"),
        EnsureRepo2,
        GoPage(Page::Repos),
        RegisterTyped(Repo::Second),
        WaitRepoRegistered(Repo::Second, 15_000),
        Wait(300),
        Shot("ko_20_repos_page_two_repos"),
        Pause("ko_repos_page_two_repos"),
        Log("--- multi-repo: a configuration with both as default repos"),
        GoPage(Page::Configurations),
        CfgCreate("Multi"),
        WaitConfig("multi", true, 5000),
        CfgDefaults("multi"),
        Wait(300),
        Shot("ko_21_configuration_default_repos"),
        Pause("ko_configuration_default_repos"),
        CfgSave,
        WaitCfgDefaults("multi", 5000),
        GoPage(Page::Work),
        Log("--- multi-repo: new session pre-checks the configuration's default repos"),
        App(Message::OpenNew),
        App(Message::NewRefreshBase(false)),
        App(Message::NewProvider("shell".into())),
        App(Message::NewConfiguration("multi".into())),
        App(Message::NewTitle("멀티 레포".into())),
        ExpectNewRepos(2),
        Wait(300),
        Shot("ko_22_new_session_default_repos"),
        Pause("ko_new_session_default_repos"),
        Submit,
        WaitCreated(20_000),
        MarkP2,
        WaitSessionRepos(2, 15_000),
        WaitText(P2, ">", 15_000),
        Log("--- multi-repo: Changes / Explore switch repos"),
        WriteInRepo(Repo::Second, "lib-change.txt"),
        D(DetailMsg::Side(SideTab::Changes)),
        PickRepo(Repo::Second),
        WaitChange("lib-change.txt", 10_000),
        Wait(400),
        Shot("ko_23_multi_repo_changes"),
        Pause("ko_multi_repo_changes"),
        D(DetailMsg::Side(SideTab::Explore)),
        WaitDir("", 10_000),
        PickRepo(Repo::Test),
        WaitDir("", 10_000),
        Wait(300),
        Shot("ko_24_multi_repo_explore"),
        Log("--- multi-repo: search covers both repos"),
        App(Message::FindInFiles),
        D(DetailMsg::SearchInput("repo".into())),
        D(DetailMsg::SearchSubmit),
        WaitSearch(2, 10_000),
        ExpectSearchRepos(2),
        Wait(300),
        Shot("ko_25_multi_repo_search"),
        Log("--- multi-repo: detach / attach the second repo"),
        DetachRepo2,
        WaitSessionRepos(1, 15_000),
        Wait(300),
        Shot("ko_26_session_repos_detached"),
        Pause("ko_session_repos"),
        AttachRepo2,
        WaitSessionRepos(2, 20_000),
        Log("--- multi-repo: unregister refused while a session uses it"),
        GoPage(Page::Repos),
        Unregister(Repo::Second),
        ExpectReposError("세션이 1개", 10_000),
        GoPage(Page::Work),
        Log("--- multi-repo: delete cleans every worktree and branch"),
        MarkCleanup,
        AskDelete(P2),
        ConfirmDelete,
        WaitDeleted(P2, 20_000),
        ExpectCleaned,
        GoPage(Page::Repos),
        Unregister(Repo::Second),
        WaitRepoGone(Repo::Second, 10_000),
        GoPage(Page::Work),
    ]
}

/// Hermes Desk's base branches (in Korean), against a local bare origin of the test repo:
/// fixed base on the repos page, a rule with its preview, a new session with 기준 브랜치
/// 최신화 from the rule's pick, the session's own base, then an unreachable origin (that repo
/// fails with 최신화; without it the session starts from the local state).
fn base_branches() -> Vec<Step> {
    use Step::*;
    let delete_p2 = || [AskDelete(P2), ConfirmDelete, WaitDeleted(P2, 20_000)];
    let mut v = vec![
        Log("--- base branches: fixed and rule base on the repos page"),
        EnsureOrigin,
        GoPage(Page::Repos),
        SelectRepo(Repo::Test),
        WaitBaseRefs(10_000),
        Wait(300),
        Shot("ko_30_repo_base_fixed"),
        Pause("ko_repo_base_fixed"),
        App(Message::BaseFixed(Some("refs/remotes/origin/main".into()))),
        App(Message::BaseFixedSave),
        WaitRepoDefault(Repo::Test, Some("main"), 10_000),
        App(Message::BaseMode(true)),
        WaitBasePreview("release/2026.19.0", 15_000),
        Wait(300),
        Shot("ko_31_repo_base_rule"),
        Pause("ko_repo_base_rule"),
        App(Message::BaseRuleSave),
        WaitRepoDefault(Repo::Test, Some("release/2026.19.0"), 15_000),
        GoPage(Page::Work),
        Log("--- base branches: new session with 기준 브랜치 최신화 from the rule's pick"),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "기준 브랜치",
        },
        App(Message::NewRefreshBase(true)),
        ExpectNewRefresh(true),
        Wait(300),
        Shot("ko_32_new_session_base"),
        Pause("ko_new_session_base"),
        Submit,
        WaitCreated(30_000),
        MarkP2,
        WaitP2Base("release/2026.19.0", 15_000),
        SessionBaseP2("main"),
        Wait(300),
        Shot("ko_33_session_repos_base"),
        Pause("ko_session_repos_base"),
        App(Message::SessionReposSave),
        WaitP2Base("main", 15_000),
        Log("--- History: a local commit is marked as not pushed (local bare origin)"),
        D(DetailMsg::Side(SideTab::History)),
        WaitHistory("init", 10_000),
        WaitUnpushed(0, 10_000),
        CommitP2("history-local.txt", "local only\n", "로컬 커밋"),
        WaitHistory("로컬 커밋", 15_000),
        WaitUnpushed(1, 10_000),
        ExpandCommit("로컬 커밋"),
        WaitCommitFiles("로컬 커밋", "history-local.txt", 10_000),
        Wait(300),
        Shot("ko_hist_02_unpushed"),
        Pause("ko_hist_unpushed"),
    ];
    v.extend(delete_p2());
    v.extend([
        Log("--- base branches: unreachable origin"),
        GoPage(Page::Repos),
        SelectRepo(Repo::Test),
        App(Message::BaseRuleClear),
        WaitRepoDefault(Repo::Test, None, 15_000),
        SetOriginUrl(true),
        GoPage(Page::Work),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "오프라인",
        },
        App(Message::NewRefreshBase(true)),
        Submit,
        WaitCreated(30_000),
        MarkP2,
        ExpectBanner("기준 브랜치 최신화 실패", 15_000),
        Wait(300),
        Shot("ko_34_offline_refresh_failed"),
        Pause("ko_offline_refresh_failed"),
    ]);
    v.extend(delete_p2());
    v.extend([
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "오프라인 로컬",
        },
        Submit,
        WaitCreated(30_000),
        MarkP2,
        WaitSessionRepos(1, 15_000),
    ]);
    v.extend(delete_p2());
    v.push(SetOriginUrl(false));
    v
}

/// A command without a console window (the app is a GUI process).
fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Log the summary once (also called when the app exits through the close dialog).
pub fn finish(app: &mut App) {
    if let Some(sc) = app.script.as_mut()
        && !sc.finished
    {
        sc.finished = true;
        sc.done = true;
        log(&format!("DONE: {} passed, {} failed", sc.pass, sc.fail));
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = command("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=ide-test",
            "-c",
            "user.email=ide-test@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

fn ensure_repo(repo: &Path) -> Result<(), String> {
    if repo.join(".git").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(repo).map_err(|e| e.to_string())?;
    git(repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join("README.md"), "# ide test repo\n").map_err(|e| e.to_string())?;
    // Phase 2 fixtures: a TS import for go-to-definition, a Korean file name, a binary file.
    let src = repo.join("src");
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    let files: [(PathBuf, &[u8]); 5] = [
        (
            src.join("util.ts"),
            b"export function greet(name: string): string {\n  return `hello ${name}`\n}\n",
        ),
        (
            src.join("main.ts"),
            b"import { greet } from './util'\n\nconsole.log(greet('world'))\n",
        ),
        (src.join("notes.md"), b"# notes\n\nPlain markdown.\n"),
        (
            repo.join("한글 메모.txt"),
            "첫 줄\n둘째 줄: 한국어 파일 이름\n".as_bytes(),
        ),
        (
            repo.join("logo.bin"),
            &[
                0x89, b'P', b'N', b'G', 0, 0, 0, 0x0d, 1, 2, 3, 0, 0xff, 0xfe,
            ],
        ),
    ];
    for (p, bytes) in files {
        std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
    }
    git(repo, &["add", "."])?;
    git(repo, &["commit", "-q", "-m", "init"])?;
    Ok(())
}

fn tasklist() -> Vec<(String, u32)> {
    let Ok(out) = command("tasklist").args(["/FO", "CSV", "/NH"]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let cols: Vec<&str> = l.split("\",\"").collect();
            let name = cols.first()?.trim_start_matches('"').to_owned();
            let pid = cols.get(1)?.parse().ok()?;
            Some((name, pid))
        })
        .collect()
}

/// Whether `pid` runs, checked on a helper thread (`tasklist` can take seconds; the UI
/// thread must not block). `None` until the answer is there.
#[derive(Default)]
struct Probe(Option<std::sync::mpsc::Receiver<bool>>);

impl Probe {
    fn poll(&mut self, pid: u32) -> Option<bool> {
        let rx = self.0.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let out = command("tasklist")
                    .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                    .output();
                let alive = out.is_ok_and(|o| {
                    String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))
                });
                let _ = tx.send(alive);
            });
            rx
        });
        let r = rx.try_recv().ok();
        if r.is_some() {
            self.0 = None;
        }
        r
    }
}

/// `path` is `dir` or below it (either separator, case-insensitive like Windows paths).
fn path_under(path: &str, dir: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").to_lowercase();
    let (p, d) = (norm(path), norm(dir));
    let d = d.trim_end_matches('/');
    p == d || p.starts_with(&format!("{d}/"))
}

/// Id of a registered test repo.
fn registered(sc: &Script, app: &App, r: Repo) -> Option<String> {
    let p = sc.repo_path(r);
    app.repos
        .iter()
        .find(|x| crate::app::same_path(&x.path, &p.display().to_string()))
        .map(|x| x.id.clone())
}

/// The second test repo: a README and one source file (searchable "repo").
fn ensure_repo2(repo: &Path) -> Result<(), String> {
    if repo.join(".git").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(repo.join("lib")).map_err(|e| e.to_string())?;
    git(repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join("README.md"), "# second test repo\n").map_err(|e| e.to_string())?;
    std::fs::write(repo.join("lib").join("mod.ts"), "export const lib = 'lib'\n")
        .map_err(|e| e.to_string())?;
    git(repo, &["add", "."])?;
    git(repo, &["commit", "-q", "-m", "init"])?;
    Ok(())
}

/// A phase 2 path as an item path: qualified with the session's shown repo.
fn qp(sc: &Script, app: &App, path: &str) -> String {
    if !crate::detail::split_q(path).0.is_empty() {
        return path.to_owned();
    }
    let repo = p2_detail(sc, app).map(|d| d.repo.clone()).unwrap_or_default();
    crate::detail::qualify(&repo, path)
}

fn qk(sc: &Script, app: &App, key: ItemKey) -> ItemKey {
    let repo = p2_detail(sc, app).map(|d| d.repo.clone());
    key.clone().qualified(repo.as_deref()).unwrap_or(key)
}

/// A detail message of the script with its plain paths qualified.
fn qmsg(sc: &Script, app: &App, m: DetailMsg) -> DetailMsg {
    match m {
        DetailMsg::Activate(k) => DetailMsg::Activate(qk(sc, app, k)),
        DetailMsg::Close(k) => DetailMsg::Close(qk(sc, app, k)),
        DetailMsg::Diff(k, x) => DetailMsg::Diff(qk(sc, app, k), x),
        DetailMsg::BarCompare(p) => DetailMsg::BarCompare(qp(sc, app, &p)),
        DetailMsg::BarOverwrite(p) => DetailMsg::BarOverwrite(qp(sc, app, &p)),
        DetailMsg::BarReload(p) => DetailMsg::BarReload(qp(sc, app, &p)),
        DetailMsg::BarDismiss(p) => DetailMsg::BarDismiss(qp(sc, app, &p)),
        DetailMsg::Code(p, x) => DetailMsg::Code(qp(sc, app, &p), x),
        DetailMsg::OpenAt(p, l, c) => DetailMsg::OpenAt(qp(sc, app, &p), l, c),
        other => other,
    }
}

/// The phase 2 session's worktree of its shown repo.
fn p2_worktree(sc: &Script, app: &App) -> PathBuf {
    let repo = p2_detail(sc, app).map(|d| d.repo.clone()).unwrap_or_default();
    sc.id(P2)
        .and_then(|s| app.live(&s))
        .and_then(|l| l.info.repos.iter().find(|r| r.repo == repo))
        .map(|r| PathBuf::from(&r.path))
        .unwrap_or_default()
}

/// The phase 2 session's detail area.
fn p2_detail<'a>(sc: &Script, app: &'a App) -> Option<&'a crate::detail::Detail> {
    sc.id(P2).and_then(|s| app.detail(&s))
}

fn p2_file<'a>(sc: &Script, app: &'a App, path: &str) -> Option<&'a crate::detail::FileItem> {
    let q = qp(sc, app, path);
    p2_detail(sc, app).and_then(|d| d.file(&q))
}

/// Phase 2 steps: `(advance, task)`.
fn step_p2(sc: &mut Script, app: &mut App, st: Step, el: Duration) -> (bool, Task<Message>) {
    let timeout = |ms: u64| el > Duration::from_millis(ms);
    let sid = sc.id(P2).unwrap_or_default();
    let mut task = Task::none();
    // `wait(cond, ms, what)`: pass, fail on timeout, or keep waiting.
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
        Step::WaitPickStart(ms) => {
            let repo = sc.repo.display().to_string();
            let ok = match &app.modal {
                Some(Modal::PickRoot(f)) => f.listing.as_ref().is_some_and(|l| {
                    l.path.is_empty()
                        && l.entries
                            .iter()
                            .any(|e| crate::app::is_fs_root(&e.path) && path_under(&repo, &e.path))
                }),
                _ => false,
            };
            wait!(
                ok,
                ms,
                format!("folder picker start list has the drive of {repo}")
            )
        }
        Step::PickWalk(to_repo, ms) => {
            let target = if to_repo {
                sc.repo.display().to_string()
            } else {
                sc.repo_parent()
            };
            let Some(Modal::PickRoot(f)) = &app.modal else {
                sc.check(false, "folder picker is not open");
                return (true, task);
            };
            enum Walk {
                Wait,
                Reached(bool),
                Open(String),
                Stuck(String),
            }
            let walk = match &f.listing {
                _ if f.loading => Walk::Wait,
                None => Walk::Wait,
                Some(l) if !l.path.is_empty() && crate::app::same_path(&l.path, &target) => {
                    Walk::Reached(l.is_git_repo)
                }
                // The listed folder on the way (at the start list: the drive, the shortest
                // match, so the walk does not take the home shortcut).
                Some(l) => l
                    .entries
                    .iter()
                    .filter(|e| path_under(&target, &e.path))
                    .min_by_key(|e| e.path.len())
                    .map(|e| Walk::Open(e.path.clone()))
                    .unwrap_or_else(|| Walk::Stuck(l.path.clone())),
            };
            match walk {
                Walk::Wait => wait!(false, ms, format!("folder picker walks to {target}")),
                Walk::Reached(git) => {
                    sc.check(
                        !to_repo || git,
                        &format!("folder picker reached {target} (git {git})"),
                    );
                    true
                }
                Walk::Open(p) => {
                    log(&format!("picker: open {p:?}"));
                    task = app.update(Message::PickGo(Some(p)));
                    false
                }
                Walk::Stuck(at) => {
                    sc.check(
                        false,
                        &format!("folder picker: no way to {target} from {at:?}"),
                    );
                    true
                }
            }
        }
        Step::PickHighlightRepo => {
            let name = sc.repo_name();
            let i = match &app.modal {
                Some(Modal::PickRoot(f)) => f.listing.as_ref().and_then(|l| {
                    l.entries
                        .iter()
                        .position(|e| e.name == name && e.is_git_repo)
                }),
                _ => None,
            };
            sc.check(
                i.is_some(),
                &format!("folder picker lists {name} marked git"),
            );
            if let Some(i) = i {
                // Like a click, then an arrow-key no-op that scrolls the row into view.
                task = Task::batch([
                    app.update(Message::PickHighlight(i)),
                    app.update(Message::PickMove(0)),
                ]);
            }
            true
        }
        Step::CdRepoP2 => {
            let wt = app
                .live(&sid)
                .and_then(|l| l.info.repos.first())
                .map(|r| r.path.clone())
                .unwrap_or_default();
            log(&format!("phase 2 terminal: cd {wt}"));
            task = app.update(Message::TermInput(
                sid,
                format!("cd /d \"{wt}\"\r").into_bytes(),
            ));
            true
        }
        Step::EnsureRepo2 => {
            let r = ensure_repo2(&sc.repo2);
            sc.check(
                r.is_ok(),
                &format!("second test repo ready at {} {r:?}", sc.repo2.display()),
            );
            true
        }
        Step::CfgDefaults(slug) => {
            let ids: Vec<String> = [Repo::Test, Repo::Second]
                .iter()
                .filter_map(|r| registered(sc, app, *r))
                .collect();
            let mut ts = vec![app.update(Message::CfgEdit(slug.into()))];
            // `None` (= all) becomes an explicit list: uncheck everything, check both.
            for r in app.repos.iter().map(|r| r.id.clone()).collect::<Vec<_>>() {
                ts.push(app.update(Message::CfgDefaultRepo(r, false)));
            }
            for id in &ids {
                ts.push(app.update(Message::CfgDefaultRepo(id.clone(), true)));
            }
            let got = app
                .settings
                .cfg_edit
                .as_ref()
                .and_then(|e| e.default_repos.clone());
            sc.check(
                got.as_ref().is_some_and(|g| g.len() == 2 && ids.iter().all(|i| g.contains(i))),
                &format!("configuration editor default repos {got:?}"),
            );
            task = Task::batch(ts);
            true
        }
        Step::WaitCfgDefaults(slug, ms) => {
            let ids: Vec<String> = [Repo::Test, Repo::Second]
                .iter()
                .filter_map(|r| registered(sc, app, *r))
                .collect();
            let got = app
                .configs
                .iter()
                .find(|c| c.slug == slug)
                .and_then(|c| c.default_repos.clone());
            let stored = std::fs::read_to_string(sc.data.join("configurations.json"))
                .map(|t| ids.iter().all(|i| t.contains(i.as_str())))
                .unwrap_or(false);
            wait!(
                got.as_ref().is_some_and(|g| g.len() == 2) && stored,
                ms,
                format!("configuration {slug} default repos {got:?} (stored {stored})")
            )
        }
        Step::ExpectNewRepos(n) => {
            let got = match &app.modal {
                Some(Modal::NewSession(f)) => f.repos.clone(),
                _ => Vec::new(),
            };
            let want: Vec<String> = [Repo::Test, Repo::Second]
                .iter()
                .filter_map(|r| registered(sc, app, *r))
                .collect();
            sc.check(
                got.len() == n && want.iter().all(|w| got.contains(w)),
                &format!("new-session dialog pre-checked the default repos {got:?}"),
            );
            true
        }
        Step::WaitSessionRepos(n, ms) => {
            let info = app.live(&sid).map(|l| l.info.clone());
            let ok = info.as_ref().is_some_and(|s| {
                s.repos.len() == n
                    && s.repos.iter().all(|r| {
                        let p = Path::new(&r.path);
                        p.join("README.md").is_file()
                            && p.starts_with(&s.root)
                            && r.branch == format!("ide/{}", s.id)
                    })
            });
            let what = info
                .map(|s| {
                    s.repos
                        .iter()
                        .map(|r| format!("{} {} {}", r.name, r.branch, r.path))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            wait!(ok, ms, format!("session has {n} worktree(s): {what:?}"))
        }
        Step::PickRepo(r) => {
            match registered(sc, app, r) {
                Some(id) => {
                    task = app.update(Message::Detail(sid, DetailMsg::PickRepo(id)));
                }
                None => sc.check(false, &format!("{r:?} is not registered")),
            }
            true
        }
        Step::WriteInRepo(r, name) => {
            let id = registered(sc, app, r).unwrap_or_default();
            let wt = app
                .live(&sid)
                .and_then(|l| l.info.repos.iter().find(|x| x.repo == id))
                .map(|x| PathBuf::from(&x.path));
            match wt {
                Some(wt) => {
                    let ok = std::fs::write(wt.join(name), "changed in the second repo\n").is_ok();
                    sc.check(ok, &format!("wrote {name} into {}", wt.display()));
                }
                None => sc.check(false, &format!("{r:?} has no worktree in the session")),
            }
            true
        }
        Step::ExpectSearchRepos(n) => {
            let mut repos: Vec<String> = p2_detail(sc, app)
                .map(|d| {
                    d.search
                        .groups
                        .iter()
                        .map(|(p, _)| crate::detail::split_q(p).0.to_owned())
                        .collect()
                })
                .unwrap_or_default();
            repos.dedup();
            sc.check(
                repos.len() == n,
                &format!("search results grouped by repo: {repos:?}"),
            );
            true
        }
        Step::DetachRepo2 => {
            let id = registered(sc, app, Repo::Second).unwrap_or_default();
            task = Task::batch([
                app.update(Message::SessionRepos(sid)),
                app.update(Message::SessionRepoDetach(id)),
            ]);
            true
        }
        Step::AttachRepo2 => {
            let id = registered(sc, app, Repo::Second).unwrap_or_default();
            let open = matches!(app.modal, Some(Modal::SessionRepos(_)));
            sc.check(open, "session repos dialog stays open after a detach");
            task = Task::batch([
                app.update(Message::SessionRepoAdd(id, true)),
                app.update(Message::SessionReposSave),
            ]);
            true
        }
        Step::MarkCleanup => {
            sc.cleanup = app.live(&sid).map(|l| {
                (
                    PathBuf::from(&l.info.root),
                    l.info.repos.iter().map(|r| PathBuf::from(&r.path)).collect(),
                    format!("ide/{}", l.info.id),
                )
            });
            log(&format!("cleanup check for {:?}", sc.cleanup));
            true
        }
        Step::ExpectCleaned => {
            match sc.cleanup.clone() {
                Some((root, wts, branch)) => {
                    sc.check(!root.exists(), &format!("session folder {} removed", root.display()));
                    for w in &wts {
                        sc.check(!w.exists(), &format!("worktree {} removed", w.display()));
                    }
                    for r in [sc.repo.clone(), sc.repo2.clone()] {
                        let b = git(&r, &["branch", "--list", &branch]).unwrap_or_default();
                        let wl = git(&r, &["worktree", "list"]).unwrap_or_default();
                        sc.check(
                            b.is_empty() && !wl.contains(&branch) && r.join("README.md").is_file(),
                            &format!("{}: branch {branch} and its worktree gone, repo kept", r.display()),
                        );
                    }
                }
                None => sc.check(false, "no session marked for the cleanup check"),
            }
            true
        }
        Step::ExpectInitDialog(ms) => {
            let path = match &app.modal {
                Some(Modal::InitRepo(f)) => Some(f.path.clone()),
                _ => None,
            };
            let want = sc.repo_path(Repo::PlainFolder);
            wait!(
                path.as_deref()
                    .is_some_and(|p| crate::app::same_path(p, &want.display().to_string())),
                ms,
                format!("init dialog for the plain folder: {path:?}")
            )
        }
        Step::ExpectPlainInitialised(want) => {
            let d = sc.repo_path(Repo::PlainFolder);
            let has = d.join(".git").exists();
            let detail = if has {
                format!(
                    "branch {:?}, log {:?}",
                    git(&d, &["branch", "--show-current"]),
                    git(&d, &["log", "--format=%s"])
                )
            } else {
                String::new()
            };
            let ok = has == want
                && (!want
                    || (detail.contains("\"main\"") && detail.contains("Initial commit")));
            sc.check(ok, &format!("plain folder initialised = {has} (want {want}) {detail}"));
            true
        }
        Step::EnsureOrigin => {
            let repo = sc.repo.clone();
            let bare = sc.data.join("script-origin.git");
            let r = (|| -> Result<(), String> {
                if git(&repo, &["remote"])?.lines().any(|l| l.trim() == "origin") {
                    return Ok(());
                }
                if !bare.exists() {
                    git(&sc.data, &["init", "-q", "--bare", "-b", "main", "script-origin.git"])?;
                }
                git(&repo, &["remote", "add", "origin", &bare.display().to_string()])?;
                git(
                    &repo,
                    &[
                        "push",
                        "-q",
                        "origin",
                        "HEAD:refs/heads/main",
                        "HEAD:refs/heads/release/2026.9.0",
                        "HEAD:refs/heads/release/2026.19.0",
                    ],
                )?;
                git(&repo, &["fetch", "-q", "origin", "+refs/heads/main:refs/remotes/origin/main"])?;
                Ok(())
            })();
            sc.check(r.is_ok(), &format!("local bare origin {} {r:?}", bare.display()));
            true
        }
        Step::SetOriginUrl(offline) => {
            let url = if offline {
                sc.data.join("gone-origin.git")
            } else {
                sc.data.join("script-origin.git")
            };
            let r = git(&sc.repo, &["remote", "set-url", "origin", &url.display().to_string()]);
            sc.check(r.is_ok(), &format!("origin url -> {} {r:?}", url.display()));
            true
        }
        Step::SelectRepo(r) => {
            match registered(sc, app, r) {
                Some(id) => task = app.update(Message::RepoSelect(id)),
                None => sc.check(false, &format!("{r:?} is not registered")),
            }
            true
        }
        Step::WaitBaseRefs(ms) => {
            let refs = app.settings.base.as_ref().and_then(|b| b.refs.clone());
            wait!(
                refs.as_ref()
                    .is_some_and(|r| r.iter().any(|x| x == "refs/remotes/origin/main")),
                ms,
                format!("repos page base refs {refs:?}")
            )
        }
        Step::WaitBasePreview(pick, ms) => {
            let got = app
                .settings
                .base
                .as_ref()
                .and_then(|b| b.preview.as_ref())
                .map(|p| (p.pick.clone(), p.matched.clone()));
            wait!(
                got.as_ref().is_some_and(|(p, _)| p.as_deref() == Some(pick)),
                ms,
                format!("base rule preview {got:?}")
            )
        }
        Step::WaitRepoDefault(r, want, ms) => {
            let id = registered(sc, app, r);
            let got = app
                .repos
                .iter()
                .find(|x| Some(&x.id) == id.as_ref())
                .map(|x| (x.default_base.clone(), x.base.clone()));
            let busy = app.settings.base.as_ref().is_some_and(|b| b.busy);
            wait!(
                !busy && got.as_ref().is_some_and(|(d, _)| d.as_deref() == want),
                ms,
                format!("{r:?} default base {got:?} (want {want:?}), error {:?}", app.settings.repos_error)
            )
        }
        Step::ExpectNewRefresh(want) => {
            let got = match &app.modal {
                Some(Modal::NewSession(f)) => Some(f.refresh_base),
                _ => None,
            };
            sc.check(got == Some(want), &format!("기준 브랜치 최신화 = {got:?} (want {want})"));
            true
        }
        Step::WaitP2Base(want, ms) => {
            let id = registered(sc, app, Repo::Test).unwrap_or_default();
            let got = app
                .live(&sid)
                .and_then(|l| l.info.repos.iter().find(|r| r.repo == id).map(|r| r.base.clone()));
            wait!(
                got.as_deref() == Some(want),
                ms,
                format!("session base of the test repo {got:?} (want {want})")
            )
        }
        Step::SessionBaseP2(b) => {
            let id = registered(sc, app, Repo::Test).unwrap_or_default();
            task = Task::batch([
                app.update(Message::SessionRepos(sid.clone())),
                app.update(Message::SessionRepoBase(id, b.into())),
            ]);
            true
        }
        Step::ExpectBanner(needle, ms) => {
            let b = app.banner.clone();
            wait!(
                b.as_deref().is_some_and(|x| x.contains(needle)),
                ms,
                format!("banner {b:?} (want {needle:?})")
            )
        }
        Step::MarkP2 => {
            sc.p2 = sc.ids.len().checked_sub(1);
            log(&format!("phase 2 session: {:?}", sc.id(P2)));
            true
        }
        Step::D(m) => {
            let m = qmsg(sc, app, m);
            log(&format!("detail <- {m:?}"));
            task = app.update(Message::Detail(sid, m));
            true
        }
        Step::App(m) => {
            log(&format!("app <- {m:?}"));
            task = app.update(m);
            true
        }
        Step::TreeReveal(p) => {
            let q = qp(sc, app, p);
            task = app.tree_reveal(&sid, &q);
            true
        }
        Step::TreeKey(k) => {
            task = app.update(Message::Detail(
                sid,
                DetailMsg::Tree(TreeMessage::Key(Key::Named(k), Modifiers::empty())),
            ));
            true
        }
        Step::TreeCtrl(c) => {
            task = app.update(Message::Detail(
                sid,
                DetailMsg::Tree(TreeMessage::Key(Key::Character(c.into()), Modifiers::CTRL)),
            ));
            true
        }
        Step::TreeDrag(from, onto) => {
            let (qf, qo) = (qp(sc, app, from), qp(sc, app, onto));
            let t1 = app.tree_reveal(&sid, &qf);
            let t2 = app.tree_reveal(&sid, &qo);
            let rows = p2_detail(sc, app).map(|d| (d.tree.row_of(from), d.tree.row_of(onto)));
            // The rows appear once the tree has listed their folders.
            let Some((Some(a), Some(b))) = rows else {
                let done = wait!(false, 10_000, format!("drag {from} onto {onto}: rows {rows:?}"));
                return (done, Task::batch([t1, t2]));
            };
            let m = |x| Message::Detail(sid.clone(), DetailMsg::Tree(x));
            let t3 = app.update(m(TreeMessage::DragStart(a)));
            let t4 = app.update(m(TreeMessage::DragOver(b)));
            let t5 = app.update(m(TreeMessage::DragEnd));
            task = Task::batch([t1, t2, t3, t4, t5]);
            true
        }
        Step::TreeMenu(path) => {
            let m = |x| Message::Detail(sid.clone(), DetailMsg::Tree(x));
            let Some(path) = path else {
                task = app.update(m(TreeMessage::Menu(ide_views::MenuAction::Close)));
                return (true, task);
            };
            let row = p2_detail(sc, app).and_then(|d| d.tree.row_of(path));
            let Some(row) = row else {
                sc.check(false, &format!("menu on {path}: no row"));
                return (true, task);
            };
            let t1 = app.update(m(TreeMessage::Context(row)));
            let t2 = app.update(m(TreeMessage::ContextAt(iced::Point::new(120.0, 160.0))));
            task = Task::batch([t1, t2]);
            true
        }
        Step::TreeRename(from, to) => {
            let q = qp(sc, app, from);
            let t1 = app.tree_reveal(&sid, &q);
            let m = |x| Message::Detail(sid.clone(), DetailMsg::Tree(x));
            let t2 = app.update(m(TreeMessage::Key(
                Key::Named(Named::F2),
                Modifiers::empty(),
            )));
            let t3 = app.update(m(TreeMessage::EditInput(to.into())));
            let t4 = app.update(m(TreeMessage::EditSubmit));
            task = Task::batch([t1, t2, t3, t4]);
            true
        }
        Step::WaitDir(dir, ms) => {
            let ok = p2_detail(sc, app).is_some_and(|d| d.loaded_dirs.contains(dir));
            let rows = p2_detail(sc, app)
                .map(|d| d.tree.visible_rows())
                .unwrap_or(0);
            wait!(ok, ms, format!("tree listed {dir:?} ({rows} rows shown)"))
        }
        Step::WaitFile(path, ms) => {
            let key = ItemKey::File(qp(sc, app, path));
            let active = p2_detail(sc, app).is_some_and(|d| d.active == key);
            let f = p2_file(sc, app, path);
            let loaded = f.is_some_and(|f| f.view.is_some() || f.binary);
            let what = f
                .map(|f| {
                    format!(
                        "lines {:?}, binary {}, error {:?}",
                        f.view.as_ref().map(|v| v.line_count()),
                        f.binary,
                        f.error
                    )
                })
                .unwrap_or_default();
            wait!(active && loaded, ms, format!("file {path} shown: {what}"))
        }
        Step::TypeIn(path, s) => {
            use iced::advanced::text::editor::{Action, Edit};
            let mut ts = Vec::new();
            for c in s.chars() {
                let edit = if c == '\n' {
                    Edit::Enter
                } else {
                    Edit::Insert(c)
                };
                let q = qp(sc, app, path);
                ts.push(app.update(Message::Detail(
                    sid.clone(),
                    DetailMsg::Code(q, CodeMessage::Action(Action::Edit(edit))),
                )));
            }
            log(&format!("typed {s:?} into {path}"));
            task = Task::batch(ts);
            true
        }
        Step::GoTo(path, l, c) => {
            let q = qp(sc, app, path);
            if let Some(v) = app
                .detail_mut(&sid)
                .and_then(|d| d.file_mut(&q))
                .and_then(|f| f.view.as_mut())
            {
                task = v.go_to(l, c);
            }
            true
        }
        Step::CtrlClickOn(path, needle, off) => {
            let pos = p2_file(sc, app, path)
                .and_then(|f| f.view.as_ref())
                .and_then(|v| {
                    v.text().lines().enumerate().find_map(|(i, l)| {
                        l.find(needle)
                            .map(|b| (i + 1, l[..b].chars().count() + 1 + off))
                    })
                });
            match pos {
                Some((l, c)) => {
                    log(&format!("ctrl+click {path}:{l}:{c} ({needle:?})"));
                    let q = qp(sc, app, path);
                    let t = app
                        .detail_mut(&sid)
                        .and_then(|d| d.file_mut(&q))
                        .and_then(|f| f.view.as_mut())
                        .map(|v| v.go_to::<Message>(l, c))
                        .unwrap_or_else(Task::none);
                    let t2 = app.update(Message::Detail(
                        sid,
                        DetailMsg::Code(q, CodeMessage::CtrlClick),
                    ));
                    task = Task::batch([t, t2]);
                }
                None => sc.check(false, &format!("{needle:?} not in {path}")),
            }
            true
        }
        Step::Code(path, m) => {
            let q = qp(sc, app, path);
            task = app.update(Message::Detail(sid, DetailMsg::Code(q, m)));
            true
        }
        Step::ExpectDirty(path, want) => {
            let d = p2_file(sc, app, path).map(|f| f.dirty());
            sc.check(d == Some(want), &format!("{path} dirty={d:?}"));
            true
        }
        Step::WaitSaved(path, needle, present, ms) => {
            let f = p2_file(sc, app, path);
            let clean = f.is_some_and(|f| !f.dirty() && !f.saving && f.bar.is_none());
            let wt = p2_worktree(sc, app);
            let disk = std::fs::read_to_string(wt.join(path)).unwrap_or_default();
            wait!(
                clean && disk.contains(needle) == present,
                ms,
                format!("{path} saved; on disk contains {needle:?} = {present}")
            )
        }
        Step::WaitBuffer(path, needle, ms) => {
            let text = p2_file(sc, app, path)
                .and_then(|f| f.view.as_ref())
                .map(|v| v.text())
                .unwrap_or_default();
            wait!(
                text.contains(needle),
                ms,
                format!("buffer of {path} shows {needle:?}")
            )
        }
        Step::WaitBar(path, bar, ms) => {
            let got = p2_file(sc, app, path).map(|f| f.bar);
            wait!(
                got == Some(bar),
                ms,
                format!("{path} bar {got:?} (want {bar:?})")
            )
        }
        Step::CommitP2(path, content, subject) => {
            let wt = p2_worktree(sc, app);
            let r = (|| -> Result<String, String> {
                let file = wt.join(path);
                if let Some(dir) = file.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&file, content).map_err(|e| e.to_string())?;
                git(&wt, &["add", "--", path])?;
                git(&wt, &["commit", "-q", "-m", subject, "--", path])?;
                git(&wt, &["rev-parse", "--short", "HEAD"])
            })();
            sc.check(
                r.is_ok(),
                &format!("committed {path} as {subject:?} in {}: {r:?}", wt.display()),
            );
            true
        }
        Step::WaitHistory(subject, ms) => {
            let h = p2_detail(sc, app).map(|d| &d.history);
            let got: Vec<(String, i64)> = h
                .map(|h| {
                    h.commits
                        .iter()
                        .take(4)
                        .map(|c| (c.subject.clone(), c.date))
                        .collect()
                })
                .unwrap_or_default();
            let shown = p2_detail(sc, app).is_some_and(|d| d.side == SideTab::History);
            // Newest first: the subject on top, dates not increasing below it.
            let ok = shown
                && got.first().is_some_and(|(s, _)| s == subject)
                && (subject == "init" || got.len() >= 2)
                && got.windows(2).all(|w| w[0].1 >= w[1].1);
            wait!(ok, ms, format!("History lists (newest first) {got:?}"))
        }
        Step::WaitUnpushed(n, ms) => {
            let h = p2_detail(sc, app).map(|d| &d.history);
            let got = h.map(|h| (h.unpushed.len(), h.badge()));
            let want_badge = (n > 0).then(|| n.to_string());
            let ok = h.is_some_and(|h| h.loaded && h.unpushed.len() == n && h.badge() == want_badge)
                && h.is_some_and(|h| {
                    h.commits
                        .iter()
                        .take(n)
                        .all(|c| h.unpushed.contains(&c.sha))
                });
            wait!(ok, ms, format!("History unpushed (count, badge) = {got:?}"))
        }
        Step::ExpandCommit(subject) => {
            let sha = p2_detail(sc, app)
                .and_then(|d| d.history.commits.iter().find(|c| c.subject == subject))
                .map(|c| c.sha.clone());
            match sha {
                Some(sha) => {
                    if !p2_detail(sc, app).is_some_and(|d| d.history.expanded.contains(&sha)) {
                        task = app.update(Message::Detail(sid, DetailMsg::ToggleCommit(sha)));
                    }
                }
                None => sc.check(false, &format!("History has no commit {subject:?}")),
            }
            true
        }
        Step::WaitCommitFiles(subject, path, ms) => {
            let d = p2_detail(sc, app);
            let files = d
                .and_then(|d| {
                    let c = d.history.commits.iter().find(|c| c.subject == subject)?;
                    d.history.expanded.contains(&c.sha).then_some(())?;
                    d.history.files.get(&c.sha)
                })
                .map(|f| match f {
                    crate::history::CommitFiles::Loaded(v) => {
                        Ok(v.iter().map(|x| x.path.clone()).collect::<Vec<_>>())
                    }
                    crate::history::CommitFiles::Loading => Err("loading".to_owned()),
                    crate::history::CommitFiles::Failed(e) => Err(e.clone()),
                });
            let ok = matches!(&files, Some(Ok(v)) if v.iter().any(|p| p == path));
            wait!(ok, ms, format!("commit {subject:?} files {files:?}"))
        }
        Step::OpenCommitP2(subject, path) => {
            let sha = p2_detail(sc, app)
                .and_then(|d| d.history.commits.iter().find(|c| c.subject == subject))
                .map(|c| c.sha.clone());
            match sha {
                Some(sha) => {
                    task = app.update(Message::Detail(
                        sid,
                        DetailMsg::OpenCommit(sha, path.map(str::to_owned)),
                    ))
                }
                None => sc.check(false, &format!("History has no commit {subject:?}")),
            }
            true
        }
        Step::WaitCommitDiff(path, added, ms) => {
            let d = p2_detail(sc, app);
            let active = d.map(|d| d.active.clone());
            let item = d.and_then(|d| match d.active_item() {
                Some(Item::Diff(x)) => Some(x),
                _ => None,
            });
            let is_commit = matches!(
                &active,
                Some(ItemKey::Commit { path: p, .. }) if p.as_deref() == path
            );
            let adds = item.map(|x| x.script_adds.clone()).unwrap_or_default();
            let ok = is_commit
                && item.is_some_and(|x| x.view.is_some() && x.commit.is_some())
                && adds.iter().any(|l| l == added);
            if ok {
                let key = active.as_ref().and_then(ItemKey::encode).unwrap_or_default();
                let saved = crate::ui_state::UiState::load(&app.data_dir);
                let open = saved.detail.get(&sid).map(|d| d.open.clone());
                sc.check(
                    open.as_ref().is_some_and(|o| o.contains(&key)),
                    &format!("ui.json remembers {key}: {open:?}"),
                );
            }
            let rows = item.and_then(|x| x.view.as_ref().map(|v| v.row_count()));
            wait!(
                ok,
                ms,
                format!("commit diff {active:?} rows {rows:?} added {adds:?}")
            )
        }
        Step::WaitActive(key, ms) => {
            let key = qk(sc, app, key);
            let got = p2_detail(sc, app).map(|d| d.active.clone());
            let loaded = match p2_detail(sc, app).and_then(|d| d.item(&key)) {
                Some(Item::File(f)) => f.view.is_some(),
                Some(Item::Diff(x)) => x.view.is_some(),
                _ => true,
            };
            wait!(
                got.as_ref() == Some(&key) && loaded,
                ms,
                format!("detail shows {got:?}")
            )
        }
        Step::WaitChange(path, ms) => {
            let total = p2_detail(sc, app).map(|d| d.changes.total()).unwrap_or(0);
            let ok = p2_detail(sc, app).is_some_and(|d| {
                ChangeGroup::ALL
                    .iter()
                    .any(|g| d.changes.entries(*g).iter().any(|e| e.path == path))
            });
            wait!(ok, ms, format!("changes list has {path} ({total} changed)"))
        }
        Step::OpenChange(g, path) => {
            let i = p2_detail(sc, app)
                .and_then(|d| d.changes.entries(g).iter().position(|e| e.path == path));
            match i {
                Some(i) => {
                    task = app.update(Message::Detail(
                        sid,
                        DetailMsg::Changes(ChangesMessage::Open(g, i)),
                    ))
                }
                None => sc.check(false, &format!("{path} not in {g:?}")),
            }
            true
        }
        Step::WaitDiff(ms) => {
            let d = p2_detail(sc, app);
            let ok = d.is_some_and(|d| {
                matches!(d.active, ItemKey::Diff { .. })
                    && matches!(d.active_item(), Some(Item::Diff(x)) if x.view.is_some())
            });
            let rows = d.and_then(|d| match d.active_item() {
                Some(Item::Diff(x)) => x.view.as_ref().map(|v| (v.mode(), v.row_count())),
                _ => None,
            });
            wait!(ok, ms, format!("diff shown: {rows:?}"))
        }
        Step::WaitSearch(files, ms) => {
            let s = p2_detail(sc, app).map(|d| &d.search);
            let done = s.is_some_and(|s| !s.running && s.summary.is_some());
            let got = s.map(|s| (s.groups.len(), s.hit_count(), s.error.clone()));
            wait!(
                done && got.as_ref().is_some_and(|g| g.0 >= files),
                ms,
                format!("search streamed (files, hits, error) = {got:?}")
            )
        }
        Step::WaitExploreFiles(path, ms) => {
            let d = p2_detail(sc, app);
            let files: Vec<String> = d
                .map(|d| d.search.files.iter().map(|f| f.path.clone()).collect())
                .unwrap_or_default();
            let shown = d.is_some_and(|d| d.side == SideTab::Explore && d.search.active());
            wait!(
                shown && files.iter().any(|f| f == path),
                ms,
                format!("explore file-name matches {files:?}")
            )
        }
        Step::ExpectExploreTree => {
            let d = p2_detail(sc, app);
            let ok = d.is_some_and(|d| {
                d.side == SideTab::Explore
                    && !d.search.active()
                    && d.search.files.is_empty()
                    && d.search.groups.is_empty()
            });
            sc.check(ok, "explore shows the tree (no query, no results)");
            true
        }
        Step::OpenFirstHit(path) => {
            let hit = p2_detail(sc, app).and_then(|d| {
                d.search
                    .groups
                    .iter()
                    .find(|(p, _)| crate::detail::split_q(p).1 == path)
                    .and_then(|(_, h)| h.first().cloned())
            });
            match hit {
                Some(h) => {
                    log(&format!("open search hit {}:{}:{}", h.path, h.line, h.col));
                    task = app.update(Message::Detail(
                        sid,
                        DetailMsg::OpenAt(crate::detail::qualify(&h.repo, &h.path), h.line, h.col),
                    ));
                }
                None => sc.check(false, &format!("no search hit in {path}")),
            }
            true
        }
        Step::ExpectCursorLine(path, line) => {
            let c = p2_file(sc, app, path)
                .and_then(|f| f.view.as_ref())
                .map(|v| v.cursor());
            sc.check(
                c.is_some_and(|c| c.0 == line),
                &format!("{path} cursor {c:?} (want line {line})"),
            );
            true
        }
        Step::WaitQuick(needle, ms) => {
            let r: Vec<String> = match &app.modal {
                Some(Modal::QuickOpen(f)) => f.results.iter().map(|m| m.path.clone()).collect(),
                _ => Vec::new(),
            };
            wait!(
                r.first().is_some_and(|p| p.contains(needle)),
                ms,
                format!("quick open results {r:?}")
            )
        }
        Step::WaitToast(ms) => {
            let t = p2_detail(sc, app).and_then(|d| d.toast.as_ref().map(|t| t.0.clone()));
            wait!(t.is_some(), ms, format!("transient message {t:?}"))
        }
        Step::WaitExists(rel, want, ms) => {
            let wt = p2_worktree(sc, app);
            let exists = wt.join(rel).exists();
            wait!(exists == want, ms, format!("{rel} exists={exists}"))
        }
        Step::ExpectSavedDetail(key) => {
            // `file:<path>` with the path qualified by the repo.
            let key = match key.strip_prefix("file:") {
                Some(p) => format!("file:{}", qp(sc, app, p)),
                None => key.to_owned(),
            };
            let saved = crate::ui_state::UiState::load(&app.data_dir);
            let open = saved.detail.get(&sid).map(|d| d.open.clone());
            sc.check(
                open.as_ref().is_some_and(|o| o.contains(&key)),
                &format!("ui.json remembers the open items: {open:?}"),
            );
            true
        }
        Step::ExpectDeleteModal => {
            let ok = matches!(&app.modal, Some(Modal::DeleteEntry(f)) if f.path == "logo.bin");
            sc.check(ok, "delete confirm shown for logo.bin");
            true
        }
        Step::ExpectOpen(key) => {
            let key = qk(sc, app, key);
            let open = p2_detail(sc, app).is_some_and(|d| d.item(&key).is_some());
            wait!(open, 5000, format!("{key:?} is open"))
        }
        Step::WriteP2(s) => {
            log(&format!("type into the phase 2 terminal: {s:?}"));
            task = app.update(Message::TermInput(sid, s.as_bytes().to_vec()));
            true
        }
        _ => true,
    };
    (next, task)
}

/// Called on every `Message::Tick` while the script runs.
pub fn tick(app: &mut App) -> Task<Message> {
    let Some(mut script) = app.script.take() else {
        return Task::none();
    };
    let task = step(&mut script, app);
    if app.script.is_none() {
        app.script = Some(script);
    }
    task
}

/// Mean luma of the detail pane (between the divider and the side panel, below the header and
/// right of the centred modal card) of a screenshot.
fn detail_luma(app: &App, shot: &window::Screenshot) -> f32 {
    let (w, h) = (shot.size.width as usize, shot.size.height as usize);
    let scale = shot.size.width as f32 / app.window_size.width.max(1.0);
    let lw = app.window_size.width;
    let pane_l = app.left_w() + app.ui.split_ratio * app.split_avail() + crate::app::DIVIDER_W;
    let pane_r = lw - app.side_w() - crate::app::DIVIDER_W - 8.0;
    let left = pane_l.max(lw / 2.0 + 270.0);
    let px = |x: f32| ((x * scale) as usize).min(w);
    let (mut x0, x1) = (px(left), px(pane_r));
    if x0 + 8 >= x1 {
        x0 = x1.saturating_sub(8);
    }
    let (y0, y1) = (h * 30 / 100, h * 90 / 100);
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for y in (y0..y1).step_by(4) {
        for x in (x0..x1).step_by(4) {
            let i = (y * w + x) * 4;
            if let Some(px) = shot.rgba.get(i..i + 3) {
                sum += 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

pub fn save_shot(app: &mut App, name: &str, shot: &window::Screenshot) {
    let luma = detail_luma(app, shot);
    let Some(script) = app.script.as_mut() else {
        return;
    };
    let path = script.evidence.join(format!("{name}.png"));
    let result = (|| -> anyhow::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
        let mut enc = png::Encoder::new(file, shot.size.width, shot.size.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&shot.rgba)?;
        Ok(())
    })();
    script.luma.insert(name.to_owned(), luma);
    script.last_shot = Some((name.to_owned(), shot.clone()));
    match result {
        Ok(()) => log(&format!(
            "screenshot {} ({}x{}, detail luma {luma:.1})",
            path.display(),
            shot.size.width,
            shot.size.height
        )),
        Err(e) => log(&format!("screenshot {name} failed: {e}")),
    }
    script.waiting_shot = false;
}

fn visible(app: &App, id: &str) -> Vec<String> {
    app.live(id)
        .map(|l| l.term.visible_text())
        .unwrap_or_default()
}

fn step(sc: &mut Script, app: &mut App) -> Task<Message> {
    if sc.waiting_shot || sc.done {
        return Task::none();
    }
    let mut tasks = Vec::new();
    // Several instant steps per tick; stop at the first one that waits.
    for _ in 0..8 {
        let Some(st) = sc.steps.get(sc.pc).cloned() else {
            sc.done = true;
            break;
        };
        let fresh = sc.started.is_none();
        let started = *sc.started.get_or_insert_with(Instant::now);
        let el = started.elapsed();
        let timeout = |ms: u64| el > Duration::from_millis(ms);
        let mut next = true;
        match st {
            Step::Log(s) => log(s),
            Step::Wait(ms) => next = el >= Duration::from_millis(ms),
            Step::EnsureRepo => {
                let r = ensure_repo(&sc.repo);
                sc.check(
                    r.is_ok(),
                    &format!("test repo ready at {} {r:?}", sc.repo.display()),
                );
            }
            Step::WaitReady(ms) => {
                if app.connected() && app.loaded {
                    let v = match &app.conn {
                        Conn::Connected(w) => w.server_version.clone(),
                        _ => String::new(),
                    };
                    sc.check(true, &format!("connected to server {v}, lists loaded"));
                } else if timeout(ms) {
                    sc.check(false, &format!("not ready: {:?}", app.conn));
                } else {
                    next = false;
                }
            }
            Step::FillNew {
                repo,
                provider,
                title,
            } => {
                tasks.push(app.update(Message::OpenNew));
                // Only this repo checked (like unchecking the other defaults).
                let want = registered(sc, app, repo);
                sc.check(want.is_some(), &format!("{repo:?} is registered for the dialog"));
                for id in app.repos.iter().map(|r| r.id.clone()).collect::<Vec<_>>() {
                    let on = Some(&id) == want.as_ref();
                    tasks.push(app.update(Message::NewRepoChecked(id, on)));
                }
                tasks.push(app.update(Message::NewProvider(provider.into())));
                tasks.push(app.update(Message::NewTitle(title.into())));
                // Desk: 기준 브랜치 최신화 is on when the dialog opens. These sessions start
                // from the local base (fixture commits are local only); base_branches() turns
                // it back on.
                let on = matches!(&app.modal, Some(Modal::NewSession(f)) if f.refresh_base);
                sc.check(on, "기준 브랜치 최신화 is on when the dialog opens");
                tasks.push(app.update(Message::NewRefreshBase(false)));
            }
            Step::NewConfiguration(slug) => {
                tasks.push(app.update(Message::NewConfiguration(slug.into())));
                let picked =
                    matches!(&app.modal, Some(Modal::NewSession(f)) if f.configuration == slug);
                sc.check(
                    picked,
                    &format!("new-session dialog uses configuration {slug:?}"),
                );
            }
            Step::Submit => tasks.push(app.update(Message::NewSubmit)),
            Step::WaitCreated(ms) => {
                let open = matches!(app.modal, Some(Modal::NewSession(_)));
                if !open {
                    let sel = app.selected.clone();
                    sc.check(
                        sel.as_ref().is_some_and(|s| !sc.ids.contains(s)),
                        &format!("session created and selected: {sel:?}"),
                    );
                    if let Some(s) = sel {
                        sc.ids.push(s);
                    }
                } else if let Some(Modal::NewSession(f)) = &app.modal
                    && let Some(e) = &f.error
                {
                    let e = e.clone();
                    sc.check(false, &format!("session creation failed: {e}"));
                    tasks.push(app.update(Message::CloseModal));
                } else if timeout(ms) {
                    sc.check(false, "timeout waiting for session creation");
                } else {
                    next = false;
                }
            }
            Step::CloseModal => tasks.push(app.update(Message::CloseModal)),
            Step::WaitText(i, needle, ms) => {
                let Some(sid) = sc.id(i) else {
                    sc.check(false, &format!("no session #{i}"));
                    sc.pc += 1;
                    sc.started = None;
                    continue;
                };
                // Long paths soft-wrap in a narrow terminal: also match across lines.
                let lines = visible(app, &sid);
                let found = lines.iter().any(|l| l.contains(needle)) || lines.concat().contains(needle);
                if found {
                    log(&format!("session #{i} shows {needle:?}"));
                } else if timeout(ms) {
                    sc.check(
                        false,
                        &format!("timeout: session #{i} never showed {needle:?}"),
                    );
                    log(&format!("grid: {:#?}", visible(app, &sid)));
                } else {
                    next = false;
                }
            }
            Step::Compose(i, s) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("compose into #{i}: {s:?}"));
                    let shown = app
                        .live(&sid)
                        .is_some_and(|l| app.composer_shown(l) && l.state.is_alive());
                    sc.check(shown, &format!("session #{i} shows the input box"));
                    if let Some(l) = app.live_mut(&sid) {
                        l.composer.set_text(s);
                    }
                    tasks.push(app.update(Message::Composer(
                        crate::composer::ComposerMsg::Send(sid.clone()),
                    )));
                    let cleared = app
                        .live(&sid)
                        .is_some_and(|l| l.composer.content.text().trim().is_empty());
                    sc.check(cleared, "input box cleared after sending");
                }
            }
            Step::ComposeImage(i) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("paste a 4x2 image into #{i}"));
                    let image = iced::clipboard::Image {
                        rgba: iced::advanced::graphics::core::Bytes::from(vec![200u8; 4 * 2 * 4]),
                        size: iced::Size::new(4, 2),
                    };
                    tasks.push(app.update(Message::Composer(
                        crate::composer::ComposerMsg::PasteImage(
                            sid,
                            crate::composer::Dest::Composer,
                            image,
                        ),
                    )));
                }
            }
            Step::DropFile(i, rel) => {
                if sc.id(i).is_some() {
                    let path = sc.repo.join(rel);
                    log(&format!("drop {} on the window", path.display()));
                    tasks.push(app.update(Message::FileDropped(path)));
                }
            }
            Step::WaitComposer(i, needle, ms) => {
                let text = sc
                    .id(i)
                    .and_then(|sid| {
                        app.live(&sid)
                            .map(|l| format!("{} {}", l.composer.content.text(), l.composer.labels().join(" ")))
                    })
                    .unwrap_or_default();
                if text.contains(needle) {
                    sc.check(true, &format!("input box of #{i} has {needle:?}"));
                } else if timeout(ms) {
                    sc.check(false, &format!("input box of #{i} never had {needle:?}: {text:?}"));
                } else {
                    next = false;
                }
            }
            Step::ComposeChip(i, path) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("chip into #{i}: {path}"));
                    if let Some(l) = app.live_mut(&sid) {
                        l.composer.insert_chip(path);
                    }
                }
            }
            Step::ComposeDraft(i, s) => {
                if let Some(sid) = sc.id(i) {
                    use iced::widget::text_editor::{Action, Edit, Motion};
                    use std::sync::Arc;
                    log(&format!("draft into #{i}: {} line(s)", s.lines().count()));
                    // As typed: select all, paste the text (the box follows the cursor).
                    let edit = |a| Message::Composer(crate::composer::ComposerMsg::Edit(sid.clone(), a));
                    tasks.push(app.update(edit(Action::SelectAll)));
                    let a = if s.is_empty() {
                        Action::Edit(Edit::Delete)
                    } else {
                        Action::Edit(Edit::Paste(Arc::new(s.to_owned())))
                    };
                    tasks.push(app.update(edit(a)));
                    tasks.push(app.update(edit(Action::Move(Motion::DocumentEnd))));
                }
            }
            Step::Write(i, s) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("type into #{i}: {s:?}"));
                    tasks.push(app.update(Message::TermInput(sid, s.as_bytes().to_vec())));
                }
            }
            Step::Select(i) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("select session #{i} ({sid})"));
                    tasks.push(app.update(Message::Select(sid)));
                }
            }
            Step::ExpectSelected(i) => {
                let ok = sc.id(i).is_some() && app.selected == sc.id(i);
                sc.check(ok, &format!("session #{i} is selected"));
            }
            Step::ExpectText(i, needle) => {
                let text = sc.id(i).map(|sid| visible(app, &sid)).unwrap_or_default();
                let ok = text.iter().any(|l| l.contains(needle)) || text.concat().contains(needle);
                sc.check(ok, &format!("session #{i} shows {needle:?}"));
                if !ok {
                    log(&format!("grid: {text:#?}"));
                }
            }
            Step::ExpectLines(i, needle, n) => {
                let text = sc.id(i).map(|sid| visible(app, &sid)).unwrap_or_default();
                let got = text.iter().filter(|l| l.contains(needle)).count();
                sc.check(
                    got == n,
                    &format!("session #{i}: {got} line(s) with {needle:?}, expected {n}"),
                );
                if got != n {
                    log(&format!("grid: {text:#?}"));
                }
            }
            Step::Navigate(url) => {
                log(&format!(
                    "navigate detail tab to a data: URL ({} chars)",
                    url.len()
                ));
                // The session's page is not open by default: open it at `url`.
                if let Some(sid) = app.selected.clone() {
                    tasks.push(app.footer_open_url(&sid, &url));
                }
            }
            Step::WaitWebTitle(needle, ms) => {
                let title = app
                    .selected_live()
                    .map(|l| l.web_title.clone())
                    .unwrap_or_default();
                if title.contains(needle) {
                    sc.check(true, &format!("web tab title {title:?}"));
                } else if timeout(ms) {
                    sc.check(
                        false,
                        &format!("web tab title never became {needle:?} (got {title:?})"),
                    );
                } else {
                    next = false;
                }
            }
            Step::SetSplit(r) => {
                // Same path as dragging the divider: press, move, release.
                let x = app.left_w() + r * app.split_avail();
                tasks.push(app.update(Message::DividerPressed));
                tasks.push(app.update(Message::DragMoved(iced::Point::new(x, 300.0))));
                tasks.push(app.update(Message::DragEnd));
                log(&format!("split ratio now {:.3}", app.ui.split_ratio));
            }
            Step::Shot(name) => {
                if let Some(w) = app.window {
                    sc.waiting_shot = true;
                    let name = name.to_string();
                    tasks.push(window::screenshot(w).map(move |s| Message::Shot(name.clone(), s)));
                    sc.pc += 1;
                    sc.started = None;
                    break;
                }
            }
            Step::Pause(name) => {
                // `IDE_APP_SCRIPT_PAUSE=1` holds every pause; a comma-separated list of name
                // prefixes holds only those.
                let hold_this = std::env::var("IDE_APP_SCRIPT_PAUSE").is_ok_and(|v| {
                    v == "1" || v.split(',').any(|p| !p.is_empty() && name.starts_with(p))
                });
                if hold_this {
                    if fresh {
                        log(&format!("PAUSE {name}"));
                    }
                    // `IDE_APP_SCRIPT_PAUSE_MS` lengthens the hold for slower external capture.
                    let hold = std::env::var("IDE_APP_SCRIPT_PAUSE_MS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(7000);
                    next = el >= Duration::from_millis(hold);
                }
            }
            Step::ExpectDarker(a, b, ratio) => {
                let (la, lb) = (sc.luma.get(a).copied(), sc.luma.get(b).copied());
                let ok = matches!((la, lb), (Some(x), Some(y)) if y > 0.0 && x < y * ratio);
                sc.check(
                    ok,
                    &format!("detail area of {a} ({la:?}) darker than {b} ({lb:?}) x{ratio}"),
                );
            }
            Step::ModalWebUnsafe(v) => {
                log(&format!("draw web view under modals: {v}"));
                app.modal_web_unsafe = v;
            }
            Step::CloseApp => {
                let ttl = sc
                    .id(0)
                    .map(|sid| visible(app, &sid))
                    .unwrap_or_default()
                    .iter()
                    .filter(|l| l.contains("TTL="))
                    .count();
                log(&format!(
                    "close the app (drop client + state); session #0 shows {ttl} reply line(s) so far"
                ));
                sc.check(ttl < 6, "ping still running when the app closes");
                tasks.push(app.close_for_reopen());
            }
            Step::ReopenApp => {
                log("reopen the app (new client, state from ui.json + server)");
                app.connect();
            }
            Step::ExpectReloaded => {
                let ids: Vec<String> = app.sessions.iter().map(|l| l.info.id.clone()).collect();
                let expected: Vec<String> = sc
                    .ids
                    .iter()
                    .filter(|id| !sc.gone.contains(id))
                    .cloned()
                    .collect();
                sc.check(
                    ids == expected,
                    &format!("sessions listed by the server: {ids:?}"),
                );
                let running = app.sessions.iter().all(|l| l.state == RunState::Running);
                sc.check(running, "sessions kept running while the app was closed");
                sc.check(
                    app.selected == sc.id(0),
                    &format!("selected session restored: {:?}", app.selected),
                );
                sc.check(
                    (app.ui.split_ratio - 0.55).abs() < 0.01,
                    &format!("split ratio restored: {:.3}", app.ui.split_ratio),
                );
                let url = app
                    .selected_live()
                    .map(|l| l.url_input.clone())
                    .unwrap_or_default();
                sc.check(
                    url.starts_with("data:text/html"),
                    "web URL restored for the session",
                );
            }
            Step::TransportOffline => {
                if let Some(l) = &app.link {
                    log("transport offline + drop the connection");
                    l.switch.set_online(false);
                    l.client.reconnect();
                }
            }
            Step::TransportOnline => {
                if let Some(l) = &app.link {
                    log("transport online");
                    l.switch.set_online(true);
                }
            }
            Step::WaitDisconnected(ms) => {
                if !app.conn.is_connected() {
                    sc.check(true, &format!("footer shows {:?}", app.conn));
                } else if timeout(ms) {
                    sc.check(false, "still connected");
                } else {
                    next = false;
                }
            }
            Step::WaitConnected(resumed, ms) => {
                if let Conn::Connected(w) = &app.conn {
                    let ok = !resumed || w.resumed;
                    sc.check(
                        ok,
                        &format!("reconnected automatically (resumed={})", w.resumed),
                    );
                } else if timeout(ms) {
                    sc.check(false, &format!("not reconnected: {:?}", app.conn));
                } else {
                    next = false;
                }
            }
            Step::Start(i) => {
                if let Some(sid) = sc.id(i) {
                    tasks.push(app.update(Message::Start(sid)));
                }
            }
            Step::WaitRunning(i, ms) => {
                let state = sc.id(i).and_then(|sid| app.live(&sid).map(|l| l.state));
                if state == Some(RunState::Running) {
                    sc.check(true, &format!("session #{i} running again"));
                } else if timeout(ms) {
                    sc.check(false, &format!("session #{i} state {state:?}"));
                } else {
                    next = false;
                }
            }
            Step::Stop(i) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("stop session #{i}"));
                    tasks.push(app.update(Message::Stop(sid)));
                }
            }
            Step::WaitStopped(i, ms) => {
                let state = sc.id(i).and_then(|sid| app.live(&sid).map(|l| l.state));
                if matches!(state, Some(RunState::Stopped | RunState::Exited(_))) {
                    sc.check(true, &format!("session #{i} not running: {state:?}"));
                } else if timeout(ms) {
                    sc.check(false, &format!("session #{i} still {state:?}"));
                } else {
                    next = false;
                }
            }
            Step::AskDelete(i) => {
                if let Some(sid) = sc.id(i) {
                    tasks.push(app.update(Message::AskDelete(sid)));
                    tasks.push(app.update(Message::DeleteWorktree(true)));
                }
            }
            Step::ConfirmDelete => tasks.push(app.update(Message::DeleteConfirm)),
            Step::WaitDeleted(i, ms) => {
                let sid = sc.id(i);
                let gone = sid.as_ref().is_some_and(|s| app.live(s).is_none());
                let err = match &app.modal {
                    Some(Modal::Delete(f)) => f.error.clone(),
                    _ => None,
                };
                if gone {
                    let s = sid.unwrap();
                    sc.gone.push(s.clone());
                    let stored = std::fs::read_to_string(sc.data.join("sessions.json"))
                        .map(|t| !t.contains(&s))
                        .unwrap_or(false);
                    sc.check(stored, &format!("session #{i} removed from sessions.json"));
                } else if let Some(e) = err {
                    sc.check(false, &format!("delete failed: {e}"));
                    tasks.push(app.update(Message::CloseModal));
                } else if timeout(ms) {
                    sc.check(false, "timeout waiting for delete");
                } else {
                    next = false;
                }
            }
            Step::GitWorktreeList => {
                let out = git(&sc.repo, &["worktree", "list"]).unwrap_or_else(|e| e);
                log(&format!("git worktree list:\n{out}"));
                let branches = git(&sc.repo, &["branch", "--list"]).unwrap_or_else(|e| e);
                log(&format!("git branch --list:\n{branches}"));
                let live_ids: Vec<String> =
                    app.sessions.iter().map(|l| l.info.id.clone()).collect();
                let lines = out.lines().count();
                sc.check(
                    lines == 1 + live_ids.len(),
                    &format!(
                        "worktree list has main + {} session worktree(s)",
                        live_ids.len()
                    ),
                );
                for (k, sid) in sc.ids.clone().iter().enumerate() {
                    let listed = out.contains(sid.as_str());
                    let alive = live_ids.contains(sid);
                    sc.check(
                        listed == alive,
                        &format!("session #{k} worktree listed={listed} (session exists={alive})"),
                    );
                    let branch = branches.contains(&format!("ide/{sid}"));
                    sc.check(
                        branch == alive,
                        &format!("session #{k} branch present={branch}"),
                    );
                }
            }
            Step::GoPage(p) => {
                if app.ui.page != p {
                    tasks.push(app.update(Message::Page(p)));
                }
            }
            Step::ExpectPage(p) => {
                sc.check(app.ui.page == p, &format!("page {:?} shown", app.ui.page));
            }
            Step::ExpectSidebar(want) => {
                let got: Vec<(String, Vec<String>)> = app
                    .sidebar_groups()
                    .into_iter()
                    .map(|(k, _, v)| (k, v.iter().map(|l| l.info.title.clone()).collect()))
                    .collect();
                let ok = got.len() == want.len()
                    && got.iter().zip(want.iter()).all(|((k, t), (wk, wt))| {
                        k == wk && (wt.is_empty() || t.iter().map(String::as_str).eq(wt.iter().copied()))
                    });
                sc.check(ok, &format!("session list groups {got:?} (want {want:?})"));
            }
            Step::ExpectSessionsOpen(open) => {
                let left = crate::app::RAIL_W
                    + if open {
                        app.list_w() + crate::app::DIVIDER_W
                    } else {
                        0.0
                    };
                sc.check(
                    app.ui.sessions_open == open && (app.left_w() - left).abs() < 0.5,
                    &format!(
                        "session list open={} (terminal starts at x={}, terminal + detail {:.0} px)",
                        app.ui.sessions_open,
                        app.left_w(),
                        app.split_avail()
                    ),
                );
            }
            Step::ExpectSavedRail(p, open) => {
                let s = crate::ui_state::UiState::load(&app.data_dir);
                sc.check(
                    s.page == p && s.sessions_open == open,
                    &format!(
                        "ui.json page {:?}, sessions_open {}",
                        s.page, s.sessions_open
                    ),
                );
            }
            Step::ExpectWorkToggle(open, hovered) => {
                use crate::icons::Icon;
                let (icon, label) = app.rail_entry(Page::Work);
                let want_icon = match (hovered, open) {
                    (false, _) => Icon::Messages,
                    (true, true) => Icon::SidebarCollapse,
                    (true, false) => Icon::SidebarExpand,
                };
                let want_label = if open {
                    tr::sessions_hide()
                } else {
                    tr::sessions_show()
                };
                sc.check(
                    icon == want_icon && label == want_label,
                    &format!("rail work button: {icon:?}, tooltip {label:?}"),
                );
            }
            Step::WaitReposLoaded(ms) => {
                if registered(sc, app, Repo::Test).is_some() {
                    sc.check(true, &format!("repos loaded: {} registered", app.repos.len()));
                } else if timeout(ms) {
                    sc.check(false, "the registered repos never arrived");
                } else {
                    next = false;
                }
            }
            Step::RegisterTyped(r) => {
                let p = sc.repo_path(r).display().to_string();
                log(&format!("repos page: register {p:?}"));
                app.settings.repos_error = None;
                tasks.push(app.update(Message::RepoInput(p)));
                tasks.push(app.update(Message::RepoAdd));
            }
            Step::WaitRepoRegistered(r, ms) => {
                let id = registered(sc, app, r);
                let stored = id.as_ref().is_some_and(|id| {
                    std::fs::read_to_string(sc.data.join("repos.json"))
                        .is_ok_and(|t| t.contains(id.as_str()))
                });
                let shown = app.settings.repo_selected.is_some();
                if id.is_some() && stored && shown && !app.settings.repos_busy {
                    let base = app
                        .repos
                        .iter()
                        .find(|x| Some(&x.id) == id.as_ref())
                        .map(|x| x.base.clone());
                    sc.check(
                        base.as_deref() == Some("main"),
                        &format!("{r:?} registered as {id:?} (base {base:?}, in repos.json)"),
                    );
                } else if timeout(ms) {
                    sc.check(
                        false,
                        &format!(
                            "{r:?} not registered (id {id:?}, stored {stored}, error {:?})",
                            app.settings.repos_error
                        ),
                    );
                } else {
                    next = false;
                }
            }
            Step::ExpectReposError(needle, ms) => {
                let err = app.settings.repos_error.clone();
                if err.as_deref().is_some_and(|e| e.contains(needle)) {
                    sc.check(true, &format!("repos page error shown: {err:?}"));
                } else if timeout(ms) {
                    sc.check(false, &format!("repos page error {err:?} (want {needle:?})"));
                } else {
                    next = false;
                }
            }
            Step::Unregister(r) => match registered(sc, app, r) {
                Some(id) => {
                    app.settings.repos_error = None;
                    tasks.push(app.update(Message::RepoSelect(id.clone())));
                    tasks.push(app.update(Message::RepoConfirm(true)));
                    tasks.push(app.update(Message::RepoUnregister(id)));
                }
                None => sc.check(false, &format!("{r:?} is not registered")),
            },
            Step::WaitRepoGone(r, ms) => {
                let gone = registered(sc, app, r).is_none();
                if gone && !app.settings.repos_busy {
                    sc.check(true, &format!("{r:?} unregistered (files kept: {})", sc.repo_path(r).join("README.md").is_file()));
                } else if timeout(ms) {
                    sc.check(false, &format!("{r:?} still registered ({:?})", app.settings.repos_error));
                } else {
                    next = false;
                }
            }
            Step::ExpectServerTooltip(needle) => {
                let t = app.server_tooltip();
                sc.check(t.contains(needle), &format!("footer server indicator {t:?}"));
            }
            Step::ExpectSettingsError(needle) => {
                let err = app.settings.error.clone();
                sc.check(
                    err.as_deref().is_some_and(|e| e.contains(needle)),
                    &format!("settings error shown: {err:?}"),
                );
            }
            Step::CfgCreate(name) => {
                tasks.push(app.update(Message::CfgName(name.into())));
                tasks.push(app.update(Message::CfgAdd));
            }
            Step::WaitConfig(slug, exists, ms) => {
                let listed = app.configs.iter().any(|c| c.slug == slug);
                if listed == exists {
                    sc.check(true, &format!("configuration {slug:?} listed={exists}"));
                } else if timeout(ms) {
                    sc.check(false, &format!("configuration {slug:?} listed={listed}"));
                } else {
                    next = false;
                }
            }
            Step::CfgFill(slug) => {
                use crate::skills_ui::SkillMsg;
                use iced::widget::text_editor::{Action, Edit};
                tasks.push(app.update(Message::CfgEdit(slug.into())));
                for c in "Be brief.\nUse the test repo.".chars() {
                    let edit = if c == '\n' {
                        Edit::Enter
                    } else {
                        Edit::Insert(c)
                    };
                    tasks.push(app.update(Message::CfgInstructions(Action::Edit(edit))));
                }
                tasks.push(app.update(Message::Skill(SkillMsg::NewOpen(true))));
                tasks.push(app.update(Message::Skill(SkillMsg::NewName("Lint rules".into()))));
                tasks.push(app.update(Message::Skill(SkillMsg::NewDescription(
                    "Lints the test repo.".into(),
                ))));
                tasks.push(app.update(Message::Skill(SkillMsg::NewCreate)));
                tasks.push(app.update(Message::CfgMcpAdd));
                for (field, v) in [
                    (McpField::Name, "docs"),
                    (McpField::Command, "node"),
                    (McpField::Args, "server.js --port 3"),
                    (McpField::Env, "TOKEN=abc"),
                ] {
                    tasks.push(app.update(Message::CfgMcp(0, field, v.into())));
                }
                log("configuration editor filled");
            }
            Step::CfgSave => tasks.push(app.update(Message::CfgSave)),
            Step::WaitConfigSaved(slug, ms) => {
                let c = app.configs.iter().find(|c| c.slug == slug);
                let ok = c.is_some_and(|c| {
                    c.instructions == "Be brief.\nUse the test repo."
                        && c.skills.len() == 1
                        && c.mcp_servers.len() == 1
                        && c.mcp_servers[0].args == ["server.js", "--port", "3"]
                        && c.mcp_servers[0].env == [("TOKEN".to_owned(), "abc".to_owned())]
                });
                let stored = std::fs::read_to_string(sc.data.join("configurations.json"))
                    .is_ok_and(|t| t.contains("Be brief.") && t.contains("\"docs\""));
                if ok && stored {
                    sc.check(true, &format!("configuration {slug:?} saved: {c:?}"));
                } else if timeout(ms) {
                    sc.check(false, &format!("configuration {slug:?} not saved: {c:?}"));
                } else {
                    next = false;
                }
            }
            Step::CfgDelete(slug) => tasks.push(app.update(Message::CfgDelete(slug.into()))),
            Step::AcctAdd(provider, name) => {
                tasks.push(app.update(Message::AcctProvider(provider.into())));
                tasks.push(app.update(Message::AcctName(name.into())));
                tasks.push(app.update(Message::AcctAdd));
            }
            Step::WaitLogin(ms) => {
                let login = app
                    .selected_live()
                    .filter(|l| l.info.kind == SessionKind::Login)
                    .map(|l| l.info.id.clone());
                if let Some(id) = login {
                    sc.check(
                        app.modal.is_none() && app.ui.page == Page::Work,
                        &format!("login terminal {id} opened and selected on the work page"),
                    );
                    sc.ids.push(id);
                } else if let Some(e) = &app.settings.error {
                    let e = e.clone();
                    sc.check(false, &format!("account/login failed: {e}"));
                } else if timeout(ms) {
                    sc.check(false, "timeout waiting for the login terminal");
                } else {
                    next = false;
                }
            }
            Step::WaitAccount(provider, slug, exists, ms) => {
                let listed = app
                    .accounts
                    .iter()
                    .any(|a| a.provider == provider && a.slug == slug);
                let dir = sc.data.join("accounts").join(provider).join(slug);
                if listed == exists && dir.is_dir() == exists {
                    sc.check(
                        true,
                        &format!(
                            "account {provider}/{slug} exists={exists} (list and {})",
                            dir.display()
                        ),
                    );
                } else if timeout(ms) {
                    sc.check(
                        false,
                        &format!(
                            "account {provider}/{slug}: listed={listed} dir={}",
                            dir.is_dir()
                        ),
                    );
                } else {
                    next = false;
                }
            }
            Step::AcctRename(provider, slug, name) => {
                tasks.push(app.update(Message::AcctStartRename(provider.into(), slug.into())));
                tasks.push(app.update(Message::AcctRenameInput(name.into())));
                tasks.push(app.update(Message::AcctRenameCommit));
            }
            Step::WaitAccountName(provider, slug, name, ms) => {
                let ok = app
                    .accounts
                    .iter()
                    .any(|a| a.provider == provider && a.slug == slug && a.name == name);
                if ok {
                    sc.check(
                        true,
                        &format!("account {provider}/{slug} renamed to {name:?}"),
                    );
                } else if timeout(ms) {
                    sc.check(false, &format!("account {provider}/{slug} not renamed"));
                } else {
                    next = false;
                }
            }
            Step::AcctStatus(provider, slug) => {
                tasks.push(app.update(Message::AcctStatus(provider.into(), slug.into())));
            }
            Step::WaitAccountStatus(provider, slug, ms) => {
                let st = app
                    .settings
                    .acct_status
                    .iter()
                    .find(|(k, _)| k.0 == provider && k.1 == slug)
                    .and_then(|(_, s)| *s);
                match st {
                    // The fake provider has no status command.
                    Some(s) => sc.check(
                        s == AccountState::Unknown,
                        &format!("account {provider}/{slug} status {s:?}"),
                    ),
                    None if timeout(ms) => sc.check(false, "no account status"),
                    None => next = false,
                }
            }
            Step::AcctDelete(provider, slug) => {
                tasks.push(app.update(Message::AcctDelete(provider.into(), slug.into())));
            }
            Step::AcctCreateQuiet(provider, name) => {
                if let Some(c) = app.client() {
                    tasks.push(Task::perform(
                        async move { c.create_account(provider, name).await },
                        |r| {
                            log(&format!("account created (no login): {r:?}"));
                            Message::Expire
                        },
                    ));
                }
            }
            Step::ListAccountDir(provider, slug) => {
                let dir = sc.data.join("accounts").join(provider).join(slug);
                let entries: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                log(&format!(
                    "account dir {} contains: {entries:?}",
                    dir.display()
                ));
                sc.check(
                    !entries.is_empty(),
                    "the CLI wrote its first-run state into the account dir",
                );
            }
            Step::TasklistMark => sc.pids_before = tasklist(),
            Step::TasklistDiff => {
                let now = tasklist();
                let new: Vec<_> = now.iter().filter(|p| !sc.pids_before.contains(p)).collect();
                log(&format!(
                    "processes started since mark and still alive: {new:?}"
                ));
                let leftover = new.iter().any(|(n, _)| {
                    let n = n.to_ascii_lowercase();
                    n.starts_with("claude") || n.starts_with("node")
                });
                sc.check(!leftover, "no claude/node process left after stopping");
            }
            Step::SetLang(pref) => tasks.push(app.update(Message::SetLanguage(pref))),
            Step::ExpectLang(lang, new_session) => {
                let now = ide_i18n::current();
                let shown = tr::new_session();
                sc.check(
                    now == lang && shown == new_session,
                    &format!("UI language {} shows {shown:?}", now.code()),
                );
            }
            Step::ExpectSavedLang(pref) => {
                let saved = crate::ui_state::UiState::load(&app.data_dir).language;
                sc.check(saved == pref, &format!("ui.json language {saved:?}"));
            }
            Step::MarkDaemon => {
                let pid = std::fs::read_to_string(sc.data.join("ide-server.pid"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                match pid {
                    None => sc.check(false, "no daemon pid file"),
                    Some(p) => match sc.probe.poll(p) {
                        Some(alive) => {
                            log(&format!("daemon pid {p}"));
                            sc.check(alive, "server daemon is running");
                            sc.daemon_pid = Some(p);
                        }
                        None => next = false,
                    },
                }
            }
            Step::AskStopServer => tasks.push(app.update(Message::AskStopServer)),
            Step::ConfirmStopServer => tasks.push(app.update(Message::StopServerConfirm)),
            Step::WaitDaemonGone(ms) => {
                let pid_file = sc.data.join("ide-server.pid").exists();
                let alive = match (pid_file, sc.daemon_pid) {
                    (true, _) => Some(true),
                    (false, None) => Some(false),
                    (false, Some(p)) => sc.probe.poll(p),
                };
                if alive.is_none() {
                    next = timeout(ms);
                    if next {
                        sc.check(false, "daemon probe timed out");
                    }
                } else if alive == Some(false) {
                    sc.check(
                        true,
                        &format!("daemon {:?} exited, pid file removed", sc.daemon_pid),
                    );
                } else if timeout(ms) {
                    sc.check(
                        false,
                        &format!("daemon still there (pid file {pid_file}, alive {alive:?})"),
                    );
                } else {
                    next = false;
                }
            }
            Step::SettingsServer(host) => {
                tasks.push(app.update(Message::SettingsServerSsh(host.is_some())));
                if let Some(h) = host {
                    tasks.push(app.update(Message::SettingsSshHost(h.into())));
                }
            }
            Step::ApplyServer => tasks.push(app.update(Message::SettingsApplyServer)),
            Step::ConfirmSwitch => {
                let ok = matches!(app.modal, Some(Modal::SwitchServer(_)));
                sc.check(ok, "switch-server confirm shown");
                tasks.push(app.update(Message::SwitchServerConfirm));
            }
            Step::ExpectSavedServer(ssh) => {
                let saved = crate::ui_state::UiState::load(&app.data_dir).server;
                let ok = matches!(saved, ServerPref::Ssh { .. }) == ssh;
                sc.check(ok, &format!("ui.json server {saved:?}"));
            }
            Step::ExpectFooter(needle) => {
                let l = app.server_label();
                sc.check(l.contains(needle), &format!("footer server label {l:?}"));
            }
            Step::ExpectNotConnected => {
                sc.check(
                    !app.conn.is_connected() && app.sessions.is_empty(),
                    &format!("not connected, no sessions listed: {:?}", app.conn),
                );
            }
            Step::ExpectNoServerChild => {
                let rx = sc.children.get_or_insert_with(|| {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let me = std::process::id();
                    std::thread::spawn(move || {
                        let q = format!(
                            "Get-CimInstance Win32_Process -Filter \"ParentProcessId={me}\" | \
                             ForEach-Object {{ $_.Name }}"
                        );
                        let out = command("powershell")
                            .args(["-NoProfile", "-Command", &q])
                            .output()
                            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                            .unwrap_or_default();
                        let _ = tx.send(out);
                    });
                    rx
                });
                match rx.try_recv() {
                    Ok(out) => {
                        sc.children = None;
                        let names: Vec<&str> = out.split_whitespace().collect();
                        log(&format!("child processes of the app: {names:?}"));
                        sc.check(
                            !names
                                .iter()
                                .any(|n| n.eq_ignore_ascii_case("ide-server.exe")),
                            "no local ide-server started while SSH is selected",
                        );
                    }
                    Err(_) if timeout(30_000) => sc.check(false, "child probe timed out"),
                    Err(_) => next = false,
                }
            }
            Step::CloseWindow => tasks.push(app.update(Message::CloseRequested)),
            Step::ExpectCloseDialog => {
                sc.check(
                    matches!(app.modal, Some(Modal::Close(_))),
                    "close dialog shown (local server, session running)",
                );
            }
            Step::CloseRemember(v) => tasks.push(app.update(Message::CloseRemember(v))),
            Step::CloseStop => tasks.push(app.update(Message::CloseStop)),
            Step::ExpectSavedOnClose(a) => {
                let saved = crate::ui_state::UiState::load(&app.data_dir).on_close;
                sc.check(saved == a, &format!("ui.json on_close {saved:?}"));
            }
            Step::Retry => tasks.push(app.update(Message::Retry)),
            st @ (Step::WaitPickStart(_)
            | Step::PickWalk(..)
            | Step::PickHighlightRepo
            | Step::CdRepoP2
            | Step::EnsureRepo2
            | Step::CfgDefaults(_)
            | Step::WaitCfgDefaults(..)
            | Step::ExpectNewRepos(_)
            | Step::WaitSessionRepos(..)
            | Step::PickRepo(_)
            | Step::WriteInRepo(..)
            | Step::ExpectSearchRepos(_)
            | Step::DetachRepo2
            | Step::AttachRepo2
            | Step::MarkCleanup
            | Step::ExpectCleaned
            | Step::MarkP2
            | Step::D(_)
            | Step::App(_)
            | Step::TreeReveal(_)
            | Step::TreeKey(_)
            | Step::TreeCtrl(_)
            | Step::TreeDrag(..)
            | Step::TreeMenu(_)
            | Step::TreeRename(..)
            | Step::WaitDir(..)
            | Step::WaitFile(..)
            | Step::TypeIn(..)
            | Step::GoTo(..)
            | Step::CtrlClickOn(..)
            | Step::Code(..)
            | Step::ExpectDirty(..)
            | Step::WaitSaved(..)
            | Step::WaitBuffer(..)
            | Step::WaitBar(..)
            | Step::WaitActive(..)
            | Step::WaitChange(..)
            | Step::OpenChange(..)
            | Step::WaitDiff(_)
            | Step::WaitSearch(..)
            | Step::WaitExploreFiles(..)
            | Step::ExpectExploreTree
            | Step::OpenFirstHit(_)
            | Step::ExpectCursorLine(..)
            | Step::WaitQuick(..)
            | Step::WaitToast(_)
            | Step::WaitExists(..)
            | Step::ExpectSavedDetail(_)
            | Step::ExpectDeleteModal
            | Step::ExpectOpen(_)
            | Step::WriteP2(_)
            | Step::ExpectInitDialog(_)
            | Step::ExpectPlainInitialised(_)
            | Step::EnsureOrigin
            | Step::SetOriginUrl(_)
            | Step::SelectRepo(_)
            | Step::WaitBaseRefs(_)
            | Step::WaitBasePreview(..)
            | Step::WaitRepoDefault(..)
            | Step::ExpectNewRefresh(_)
            | Step::WaitP2Base(..)
            | Step::SessionBaseP2(_)
            | Step::ExpectBanner(..)
            | Step::CommitP2(..)
            | Step::WaitHistory(..)
            | Step::WaitUnpushed(..)
            | Step::ExpandCommit(_)
            | Step::WaitCommitFiles(..)
            | Step::OpenCommitP2(..)
            | Step::WaitCommitDiff(..)) => {
                let (n, t) = step_p2(sc, app, st, el);
                next = n;
                tasks.push(t);
            }
            Step::Chg(x) => {
                let (n, t) = changes::run(sc, app, x, el);
                next = n;
                tasks.push(t);
            }
            Step::Foot(x) => {
                let (n, t) = footer_steps::run(sc, app, x, el);
                next = n;
                tasks.push(t);
            }
            Step::Pop(x) => {
                let (n, t) = detail_window_steps::run(sc, app, x, el);
                next = n;
                tasks.push(t);
            }
            Step::Exit => {
                sc.finished = true;
                log(&format!("DONE: {} passed, {} failed", sc.pass, sc.fail));
                sc.done = true;
                app.link = None;
                tasks.push(iced::exit());
                break;
            }
        }
        if !next {
            break;
        }
        sc.pc += 1;
        sc.started = None;
    }
    Task::batch(tasks)
}

/// The script's declarative provider: `cmd.exe` stands in for an agent CLI whose "login" echoes
/// a marker and the account dir it got (written before the server reads `providers/`).
fn install_fake_provider(data: &Path) {
    let dir = data.join("providers");
    let _ = std::fs::create_dir_all(&dir);
    let toml = r#"# Written by the in-app test script (IDE_APP_SCRIPT=1).
id = "fake"
name = "Fake CLI (script)"
program = "cmd.exe"
version_args = ["/d", "/c", "echo fake 1.0.0"]
glyph = "◇"
account_env = "FAKE_CLI_HOME"
login_args = ["/d", "/k", "echo FAKE-LOGIN-OK %FAKE_CLI_HOME%"]
"#;
    let r = std::fs::write(dir.join("script-fake.toml"), toml);
    log(&format!(
        "fake provider installed in {}: {r:?}",
        dir.display()
    ));
}
