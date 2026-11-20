//! `IDE_APP_SCRIPT=1`: an in-app end-to-end test. Everything goes through the app's own
//! messages (the same code paths the buttons and dialogs use), bytes written to the PTYs and
//! iced `window::screenshot` evidence. No OS input is used.
//!
//! Environment:
//! - `IDE_APP_TEST_REPO`: throwaway repo folder (created with `git init` + one commit if missing)
//! - `IDE_APP_EVIDENCE`: screenshot/log folder (default `<data dir>/evidence`)
//! - `IDE_APP_SCRIPT_CLAUDE=1`: also open a Claude session (first-run screen only; never types)

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use iced::{Task, window};
use ide_core::{AgentKind, Paths, SessionId};

use crate::app::{App, Message, Modal, RunState};

static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static T0: OnceLock<Instant> = OnceLock::new();

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
    /// Open the new-session dialog and fill it (repo, agent, title) like a user would.
    FillNew {
        repo: Repo,
        agent: AgentKind,
        title: &'static str,
    },
    Submit,
    /// Wait for the dialog to close (session created) and remember the new session.
    WaitCreated(u64),
    /// Wait for an inline error in the dialog containing the text.
    ExpectNewError(&'static str, u64),
    CloseModal,
    WaitText(usize, &'static str, u64),
    Write(usize, &'static str),
    Select(usize),
    ExpectText(usize, &'static str),
    ExpectSelected(usize),
    Navigate(String),
    WaitWebTitle(&'static str, u64),
    SetSplit(f32),
    Shot(&'static str),
    /// With `IDE_APP_SCRIPT_PAUSE=1`: log `PAUSE <name>` and hold the screen for a few seconds
    /// so an external read-only window capture can be taken (iced's `window::screenshot` at this
    /// iced revision leaves text inputs empty).
    Pause(&'static str),
    Reload,
    ExpectReloaded,
    Start(usize),
    WaitRunning(usize, u64),
    AskDelete(usize),
    ConfirmDelete,
    WaitDeleted(usize, u64),
    GitWorktreeList,
    OpenSettings,
    SettingsCreate(&'static str),
    SettingsRename(&'static str, &'static str),
    ExpectSettingsError(&'static str),
    ExpectProfile(&'static str, bool),
    SettingsDelete(&'static str),
    ListProfileDir,
    TasklistMark,
    TasklistDiff,
    Kill(usize),
    WaitExited(usize, u64),
    Exit,
}

#[derive(Debug, Clone, Copy)]
enum Repo {
    Test,
    NotARepo,
}

pub struct Script {
    steps: Vec<Step>,
    pc: usize,
    started: Option<Instant>,
    waiting_shot: bool,
    ids: Vec<SessionId>,
    repo: PathBuf,
    evidence: PathBuf,
    pass: u32,
    fail: u32,
    done: bool,
    pids_before: Vec<(String, u32)>,
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
<style>body{margin:0;font:15px "Segoe UI",sans-serif;background:linear-gradient(160deg,#1d2433,#2a1f3a);color:#e6e6e6;height:100vh}
.c{padding:28px}h1{margin:0 0 8px;font-weight:600}code{background:#0006;padding:2px 6px;border-radius:4px}
.dot{display:inline-block;width:12px;height:12px;border-radius:6px;background:#3fb950;margin-right:8px;animation:p 1s infinite alternate}
@keyframes p{to{opacity:.3}}</style></head><body><div class="c"><h1><span class="dot"></span>Per-session web tab</h1>
<p>This page is a <code>data:</code> URL loaded in the session's own CEF view.</p>
<p>Switching sessions hides this view; its URL is remembered in <code>ui.json</code>.</p></div></body></html>"#;

impl Script {
    pub fn from_env(paths: &Paths) -> Option<Script> {
        let mode = std::env::var("IDE_APP_SCRIPT")
            .ok()
            .filter(|s| !s.is_empty() && s != "0")?;
        let evidence = std::env::var_os("IDE_APP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.root().join("evidence"));
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
            .unwrap_or_else(|| paths.root().join("test-repo"));
        let claude = std::env::var("IDE_APP_SCRIPT_CLAUDE").is_ok_and(|v| v == "1");
        log(&format!(
            "script {mode:?}: data dir {}, repo {}, evidence {}, claude {claude}",
            paths.root().display(),
            repo.display(),
            evidence.display()
        ));
        Some(Script {
            steps: steps(claude),
            pc: 0,
            started: None,
            waiting_shot: false,
            ids: Vec::new(),
            repo,
            evidence,
            pass: 0,
            fail: 0,
            done: false,
            pids_before: Vec::new(),
        })
    }

    pub fn done(&self) -> bool {
        self.done
    }

    fn id(&self, i: usize) -> Option<SessionId> {
        self.ids.get(i).cloned()
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
        Log("--- startup"),
        Wait(1500),
        EnsureRepo,
        Shot("01_empty"),
        Log("--- error path: not a git repo"),
        FillNew {
            repo: Repo::NotARepo,
            agent: AgentKind::Shell,
            title: "",
        },
        Submit,
        ExpectNewError("not a git repository", 15_000),
        Wait(300),
        Shot("02_new_session_error"),
        CloseModal,
        Log("--- create two shell sessions through the dialog"),
        FillNew {
            repo: Repo::Test,
            agent: AgentKind::Shell,
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
        Write(0, "dir\r"),
        WaitText(0, "README.md", 5000),
        FillNew {
            repo: Repo::Test,
            agent: AgentKind::Shell,
            title: "shell B",
        },
        Submit,
        WaitCreated(20_000),
        WaitText(1, ">", 15_000),
        Write(1, "echo hello from B\r"),
        WaitText(1, "hello from B", 5000),
        Write(1, "dir\r"),
        WaitText(1, "README.md", 5000),
        Wait(500),
        Shot("04_session_b"),
        Log("--- switch sessions: both PTYs keep their output"),
        Select(0),
        Wait(600),
        ExpectSelected(0),
        ExpectText(0, "hello from A"),
        ExpectText(0, "README.md"),
        Shot("05_switched_to_a"),
        Write(1, "echo B kept running in the background\r"),
        Wait(800),
        Select(1),
        Wait(600),
        ExpectText(1, "hello from B"),
        ExpectText(1, "B kept running in the background"),
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
        Wait(800),
        Log("--- profiles"),
        OpenSettings,
        SettingsCreate("work"),
        ExpectProfile("work", true),
        SettingsRename("work", "job"),
        ExpectProfile("job", true),
        ExpectProfile("work", false),
        SettingsRename("default", "renamed"),
        ExpectSettingsError("running"),
        ExpectProfile("default", true),
        Wait(300),
        Shot("08_profiles_rename_refused"),
        Pause("profiles"),
        SettingsDelete("job"),
        ExpectProfile("job", false),
        CloseModal,
        Log("--- restart: rebuild the app from the files on disk"),
        Reload,
        Wait(2000),
        ExpectReloaded,
        WaitWebTitle("ide detail tab", 30_000),
        Wait(800),
        Shot("09_reloaded_stopped"),
        Start(0),
        WaitRunning(0, 15_000),
        WaitText(0, ">", 15_000),
        Write(0, "echo restarted in the same worktree & cd\r"),
        WaitText(0, "restarted in the same worktree", 5000),
        Wait(500),
        Shot("10_restarted"),
        Log("--- delete session B with its worktree"),
        AskDelete(1),
        Wait(300),
        Shot("11_delete_confirm"),
        Pause("delete_confirm"),
        ConfirmDelete,
        WaitDeleted(1, 15_000),
        GitWorktreeList,
        Wait(500),
        Shot("12_after_delete"),
    ];
    if claude {
        s.extend([
            Log("--- Claude session in the profile's own CLAUDE_CONFIG_DIR (no input is sent)"),
            TasklistMark,
            FillNew {
                repo: Repo::Test,
                agent: AgentKind::Claude,
                title: "claude first-run",
            },
            Submit,
            WaitCreated(20_000),
            Wait(12_000),
            Shot("13_claude_first_run"),
            ListProfileDir,
            Kill(2),
            WaitExited(2, 10_000),
            Wait(1500),
            TasklistDiff,
            AskDelete(2),
            ConfirmDelete,
            WaitDeleted(2, 15_000),
            GitWorktreeList,
        ]);
    }
    s.push(Exit);
    s
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
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
    git(repo, &["add", "README.md"])?;
    git(repo, &["commit", "-q", "-m", "init"])?;
    Ok(())
}

fn tasklist() -> Vec<(String, u32)> {
    let Ok(out) = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    else {
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

/// Called on every `Message::Tick` while the script runs.
pub fn tick(app: &mut App) -> Task<Message> {
    let Some(mut script) = app.script.take() else {
        return Task::none();
    };
    let task = step(&mut script, app);
    app.script = Some(script);
    task
}

pub fn save_shot(app: &mut App, name: &str, shot: &window::Screenshot) {
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
    match result {
        Ok(()) => log(&format!(
            "screenshot {} ({}x{})",
            path.display(),
            shot.size.width,
            shot.size.height
        )),
        Err(e) => log(&format!("screenshot {name} failed: {e}")),
    }
    script.waiting_shot = false;
}

fn visible(app: &App, id: &SessionId) -> Vec<String> {
    app.live(id)
        .and_then(|l| l.term.as_ref())
        .map(|t| t.visible_text())
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
            Step::FillNew { repo, agent, title } => {
                tasks.push(app.update(Message::OpenNew));
                let path = match repo {
                    Repo::Test => sc.repo.display().to_string(),
                    Repo::NotARepo => {
                        let p = app.paths.root().join("not-a-repo");
                        let _ = std::fs::create_dir_all(&p);
                        p.display().to_string()
                    }
                };
                tasks.push(app.update(Message::NewRepo(path)));
                tasks.push(app.update(Message::NewAgent(agent)));
                tasks.push(app.update(Message::NewTitle(title.into())));
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
                } else if el > Duration::from_millis(ms) {
                    sc.check(false, "timeout waiting for session creation");
                } else {
                    next = false;
                }
            }
            Step::ExpectNewError(needle, ms) => {
                let err = match &app.modal {
                    Some(Modal::NewSession(f)) => f.error.clone(),
                    _ => None,
                };
                match err {
                    Some(e) => sc.check(e.contains(needle), &format!("inline error shown: {e:?}")),
                    None if el > Duration::from_millis(ms) => {
                        sc.check(false, "no inline error shown")
                    }
                    None => next = false,
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
                let found = visible(app, &sid).iter().any(|l| l.contains(needle));
                if found {
                    log(&format!("session #{i} shows {needle:?}"));
                } else if el > Duration::from_millis(ms) {
                    sc.check(
                        false,
                        &format!("timeout: session #{i} never showed {needle:?}"),
                    );
                    log(&format!("grid: {:#?}", visible(app, &sid)));
                } else {
                    next = false;
                }
            }
            Step::Write(i, s) => {
                if let Some(t) = sc.id(i).and_then(|sid| app.live(&sid)?.term.clone()) {
                    log(&format!("pty write #{i}: {s:?}"));
                    t.write(s.as_bytes());
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
                let ok = text.iter().any(|l| l.contains(needle));
                sc.check(ok, &format!("session #{i} still shows {needle:?}"));
                if !ok {
                    log(&format!("grid: {text:#?}"));
                }
            }
            Step::Navigate(url) => {
                log(&format!(
                    "navigate detail tab to a data: URL ({} chars)",
                    url.len()
                ));
                tasks.push(app.update(Message::UrlInput(url)));
                tasks.push(app.update(Message::UrlSubmit));
            }
            Step::WaitWebTitle(needle, ms) => {
                let title = app
                    .selected_live()
                    .map(|l| l.web_title.clone())
                    .unwrap_or_default();
                if title.contains(needle) {
                    sc.check(true, &format!("web tab title {title:?}"));
                } else if el > Duration::from_millis(ms) {
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
                let avail = app.window_size.width - crate::app::SIDEBAR_W - crate::app::DIVIDER_W;
                let x = crate::app::SIDEBAR_W + r * avail;
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
                if std::env::var("IDE_APP_SCRIPT_PAUSE").is_ok_and(|v| v == "1") {
                    if fresh {
                        log(&format!("PAUSE {name}"));
                    }
                    next = el >= Duration::from_millis(7000);
                }
            }
            Step::Reload => {
                let before: Vec<String> =
                    app.store.list().iter().map(|s| s.id.to_string()).collect();
                log(&format!("reload from disk; sessions before: {before:?}"));
                app.reload_from_disk();
            }
            Step::ExpectReloaded => {
                let ids: Vec<SessionId> = app.store.list().iter().map(|s| s.id.clone()).collect();
                sc.check(
                    ids == sc.ids,
                    &format!("sessions reloaded from disk: {ids:?}"),
                );
                let stopped = app
                    .sessions
                    .iter()
                    .all(|l| l.state == RunState::Stopped && l.term.is_none());
                sc.check(stopped, "reloaded sessions are stopped (no auto-spawn)");
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
            Step::Start(i) => {
                if let Some(sid) = sc.id(i) {
                    tasks.push(app.update(Message::Start(sid)));
                }
            }
            Step::WaitRunning(i, ms) => {
                let state = sc.id(i).and_then(|sid| app.live(&sid).map(|l| l.state));
                if state == Some(RunState::Running) {
                    sc.check(true, &format!("session #{i} running again"));
                } else if el > Duration::from_millis(ms) {
                    sc.check(false, &format!("session #{i} state {state:?}"));
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
                let gone = sid.as_ref().is_some_and(|s| app.store.get(s).is_none());
                let err = match &app.modal {
                    Some(Modal::Delete(f)) => f.error.clone(),
                    _ => None,
                };
                if gone {
                    let s = sid.unwrap();
                    let stored = ide_core::SessionStore::open(&app.paths)
                        .map(|st| st.get(&s).is_none())
                        .unwrap_or(false);
                    sc.check(stored, &format!("session #{i} removed from sessions.json"));
                } else if let Some(e) = err {
                    sc.check(false, &format!("delete failed: {e}"));
                    tasks.push(app.update(Message::CloseModal));
                } else if el > Duration::from_millis(ms) {
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
                    app.store.list().iter().map(|s| s.id.to_string()).collect();
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
                    let alive = live_ids.contains(&sid.to_string());
                    sc.check(
                        listed == alive,
                        &format!("session #{k} worktree listed={listed} (session exists={alive})"),
                    );
                    let branch = branches.contains(&sid.branch());
                    sc.check(
                        branch == alive,
                        &format!("session #{k} branch present={branch}"),
                    );
                }
            }
            Step::OpenSettings => tasks.push(app.update(Message::OpenSettings)),
            Step::SettingsCreate(name) => {
                tasks.push(app.update(Message::SettingsNewName(name.into())));
                tasks.push(app.update(Message::SettingsCreate));
            }
            Step::SettingsRename(old, new) => {
                tasks.push(app.update(Message::SettingsStartRename(old.into())));
                tasks.push(app.update(Message::SettingsRenameInput(new.into())));
                tasks.push(app.update(Message::SettingsRenameCommit));
            }
            Step::ExpectSettingsError(needle) => {
                let err = match &app.modal {
                    Some(Modal::Settings(f)) => f.error.clone(),
                    _ => None,
                };
                sc.check(
                    err.as_deref().is_some_and(|e| e.contains(needle)),
                    &format!("profile rename refused: {err:?}"),
                );
            }
            Step::ExpectProfile(name, exists) => {
                let ok = app.profiles.get(name).is_some() == exists
                    && app.paths.profile_dir(name).is_dir() == exists;
                sc.check(
                    ok,
                    &format!("profile {name:?} exists={exists} (store and disk)"),
                );
            }
            Step::SettingsDelete(name) => {
                tasks.push(app.update(Message::SettingsDelete(name.into())));
                // Keep the throwaway profile's (empty) dir from lingering in the test data dir.
                let _ = std::fs::remove_dir_all(app.paths.profile_dir(name));
            }
            Step::ListProfileDir => {
                let dir = app
                    .profiles
                    .get(&app.current_profile())
                    .map(|p| p.claude_config_dir.clone())
                    .unwrap_or_default();
                let entries: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                log(&format!(
                    "profile CLAUDE_CONFIG_DIR {} contains: {entries:?}",
                    dir.display()
                ));
                sc.check(
                    !entries.is_empty(),
                    "claude wrote its first-run state into the profile dir",
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
                sc.check(
                    !leftover,
                    "no claude/node process left after killing the PTY",
                );
            }
            Step::Kill(i) => {
                if let Some(sid) = sc.id(i) {
                    log(&format!("kill session #{i}"));
                    tasks.push(app.update(Message::Stop(sid)));
                }
            }
            Step::WaitExited(i, ms) => {
                let state = sc.id(i).and_then(|sid| app.live(&sid).map(|l| l.state));
                if matches!(state, Some(RunState::Exited(_))) {
                    sc.check(true, &format!("session #{i} exited after kill: {state:?}"));
                } else if el > Duration::from_millis(ms) {
                    sc.check(false, &format!("session #{i} did not exit: {state:?}"));
                } else {
                    next = false;
                }
            }
            Step::Exit => {
                log(&format!("DONE: {} passed, {} failed", sc.pass, sc.fail));
                sc.done = true;
                app.kill_all();
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
