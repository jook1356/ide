//! `IDE_APP_SCRIPT=1`: an in-app end-to-end test against a real `ide-server`. Everything goes
//! through the app's own messages (the same code paths the buttons and dialogs use), terminal
//! input goes to the server like typed keys, and evidence is taken with iced's
//! `window::screenshot`. No OS input is used.
//!
//! Environment:
//! - `IDE_APP_TEST_REPO`: throwaway repo folder (created with `git init` + one commit if missing)
//! - `IDE_APP_EVIDENCE`: screenshot/log folder (default `<data dir>/evidence`)
//! - `IDE_APP_SCRIPT_CLAUDE=1`: also open a Claude session (first-run screen only; never types)
//! - `IDE_APP_SCRIPT_PAUSE=1`: hold key screens for an external read-only capture
//!
//! The script runs in English (it switches the language first; error checks match English
//! text) and has one Korean pass that screenshots the main screen and the dialogs. It ends by
//! stopping the server (settings -> stop server), so no daemon is left running.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use iced::{Task, window};
use ide_client::Agent;
use ide_i18n::{Lang, LanguagePref, tr};

use crate::app::{App, Conn, Message, Modal, RunState};
use crate::ui_state::{CloseAction, ServerPref};

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
    /// Wait until the client is connected and the lists arrived.
    WaitReady(u64),
    /// Open the new-session dialog and fill it (repo, agent, title) like a user would.
    FillNew {
        repo: Repo,
        agent: Agent,
        title: &'static str,
    },
    Submit,
    /// Wait for the dialog to close (session created) and remember the new session.
    WaitCreated(u64),
    /// Wait for an inline error in the dialog containing the text.
    ExpectNewError(&'static str, u64),
    CloseModal,
    WaitText(usize, &'static str, u64),
    /// Terminal input as typed keys (sent to the server).
    Write(usize, &'static str),
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
    OpenSettings,
    SettingsCreate(&'static str),
    SettingsRename(&'static str, &'static str),
    ExpectSettingsError(&'static str),
    WaitProfile(&'static str, bool, u64),
    SettingsDelete(&'static str),
    ListProfileDir,
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
    /// Settings: choose SSH with this host (`None` = local); does not apply.
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
}

/// SSH host that never resolves (`.invalid` is reserved).
const UNREACHABLE: &str = "ide-test@unreachable.invalid";

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
    ids: Vec<String>,
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
        let claude = std::env::var("IDE_APP_SCRIPT_CLAUDE").is_ok_and(|v| v == "1");
        log(&format!(
            "script {mode:?}: data dir {}, repo {}, evidence {}, claude {claude}",
            data.display(),
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
        })
    }

    pub fn done(&self) -> bool {
        self.done
    }

    fn id(&self, i: usize) -> Option<String> {
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
        Log("--- startup: connect to the local server"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        WaitReady(30_000),
        MarkDaemon,
        EnsureRepo,
        Wait(500),
        Shot("01_empty"),
        Log("--- error path: not a git repo (server error id -> translated message)"),
        FillNew {
            repo: Repo::NotARepo,
            agent: Agent::Shell,
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
            agent: Agent::Shell,
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
            agent: Agent::Shell,
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
        OpenSettings,
        Wait(1000),
        Shot("08a_modal_over_web_unfixed"),
        Pause("modal_over_web_unfixed"),
        CloseModal,
        ModalWebUnsafe(false),
        Wait(800),
        OpenSettings,
        Wait(1000),
        Shot("08b_modal_over_web_fixed"),
        Pause("modal_over_web_fixed"),
        ExpectDarker("08b_modal_over_web_fixed", "06_web_tab", 0.7),
        CloseModal,
        Wait(800),
        Shot("08c_modal_closed_web_back"),
        Log("--- profiles"),
        OpenSettings,
        SettingsCreate("work"),
        WaitProfile("work", true, 5000),
        SettingsRename("work", "job"),
        WaitProfile("job", true, 5000),
        WaitProfile("work", false, 5000),
        SettingsRename("default", "renamed"),
        ExpectSettingsError("running"),
        WaitProfile("default", true, 1000),
        Wait(300),
        Shot("09_profiles_rename_refused"),
        Pause("profiles"),
        SettingsDelete("job"),
        WaitProfile("job", false, 5000),
        CloseModal,
        Log("--- i18n: Korean UI, then back to English"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        ExpectLang(Lang::Ko, "새 세션"),
        ExpectSavedLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(500),
        Shot("ko_01_main"),
        Pause("ko_main"),
        FillNew {
            repo: Repo::Test,
            agent: Agent::Shell,
            title: "한국어 세션",
        },
        Wait(300),
        Shot("ko_02_new_session_dialog"),
        Pause("ko_new_session_dialog"),
        CloseModal,
        FillNew {
            repo: Repo::NotARepo,
            agent: Agent::Shell,
            title: "",
        },
        Submit,
        ExpectNewError("git 저장소가 아니에요", 15_000),
        Wait(300),
        Shot("ko_03_new_session_error"),
        Pause("ko_new_session_error"),
        CloseModal,
        OpenSettings,
        Wait(300),
        Shot("ko_04_settings"),
        Pause("ko_settings"),
        AskStopServer,
        Wait(300),
        Shot("ko_05_stop_server_confirm"),
        CloseModal,
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
        CloseApp,
        Wait(300),
        Shot("10_closed"),
        Wait(7000),
        ReopenApp,
        WaitReady(30_000),
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
            Log("--- Claude session in the profile's own CLAUDE_CONFIG_DIR (no input is sent)"),
            TasklistMark,
            FillNew {
                repo: Repo::Test,
                agent: Agent::Claude,
                title: "claude first-run",
            },
            Submit,
            WaitCreated(20_000),
            Wait(12_000),
            Shot("18_claude_first_run"),
            ListProfileDir,
            Stop(2),
            WaitStopped(2, 10_000),
            Wait(1500),
            TasklistDiff,
            AskDelete(2),
            ConfirmDelete,
            WaitDeleted(2, 15_000),
            GitWorktreeList,
        ]);
    }
    s.extend([
        Log("--- stop the server from the settings (sessions end, daemon exits)"),
        OpenSettings,
        AskStopServer,
        Wait(300),
        Shot("19_stop_server_confirm"),
        Pause("stop_server_confirm"),
        ConfirmStopServer,
        WaitDisconnected(15_000),
        WaitDaemonGone(10_000),
        Wait(300),
        Shot("20_server_stopped"),
        Log("--- retry starts a new server; the session is still listed (stopped)"),
        Retry,
        WaitConnected(false, 30_000),
        WaitReady(10_000),
        MarkDaemon,
        WaitStopped(0, 5000),
        Wait(500),
        Shot("21_new_server"),
        Log("--- server setting: SSH to an unreachable host (no local server is started)"),
        OpenSettings,
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
        Log("--- back to the local server (the local daemon kept running)"),
        OpenSettings,
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
    git(repo, &["add", "README.md"])?;
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

/// Mean luma of the detail area (right part of the window, below the URL bar and outside the
/// centred modal card) of a screenshot.
fn detail_luma(shot: &window::Screenshot) -> f32 {
    let (w, h) = (shot.size.width as usize, shot.size.height as usize);
    let (x0, x1, y0, y1) = (w * 80 / 100, w * 96 / 100, h * 30 / 100, h * 90 / 100);
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
    let luma = detail_luma(shot);
    script.luma.insert(name.to_owned(), luma);
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
            Step::FillNew { repo, agent, title } => {
                tasks.push(app.update(Message::OpenNew));
                let path = match repo {
                    Repo::Test => sc.repo.display().to_string(),
                    Repo::NotARepo => {
                        let p = sc.data.join("not-a-repo");
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
                } else if timeout(ms) {
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
                    None if timeout(ms) => sc.check(false, "no inline error shown"),
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
                let ok = text.iter().any(|l| l.contains(needle));
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
                app.close_for_reopen();
            }
            Step::ReopenApp => {
                log("reopen the app (new client, state from ui.json + server)");
                app.connect();
            }
            Step::ExpectReloaded => {
                let ids: Vec<String> = app.sessions.iter().map(|l| l.info.id.clone()).collect();
                sc.check(
                    ids == sc.ids,
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
            Step::WaitProfile(name, exists, ms) => {
                let listed = app.profiles.iter().any(|p| p.name == name);
                let on_disk = sc.data.join("profiles").join(name).is_dir();
                if listed == exists && on_disk == exists {
                    sc.check(
                        true,
                        &format!("profile {name:?} exists={exists} (server list and disk)"),
                    );
                } else if timeout(ms) {
                    sc.check(
                        false,
                        &format!("profile {name:?}: listed={listed} on_disk={on_disk}"),
                    );
                } else {
                    next = false;
                }
            }
            Step::SettingsDelete(name) => {
                tasks.push(app.update(Message::SettingsDelete(name.into())));
                // `remove_files: false` keeps the dir: remove the throwaway one (test only).
                let _ = std::fs::remove_dir_all(sc.data.join("profiles").join(name));
            }
            Step::ListProfileDir => {
                let dir = sc
                    .data
                    .join("profiles")
                    .join(app.current_profile())
                    .join("claude");
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
