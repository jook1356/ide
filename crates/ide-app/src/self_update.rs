//! ide self-update from the app footer (`docs/self-update.md`; Hermes Desk's footer "Desk 빌드 후
//! 재시작", `DeskUpdateControl` / `selfUpdate.ts`): the update script (`scripts/ide-update.ps1`)
//! pulls the repository (a pull that cannot fast-forward is skipped), builds, waits for the
//! user's restart, swaps and starts the new app, and rolls back when it does not come up. The
//! server (daemon) and its sessions stay; a changed protocol needs a server restart by the user.
//!
//! This manages the app's own binary, so it is the one place the app starts a build (the app
//! otherwise leaves processes, git and files to the server).

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::widget::{button, column, container, row, space, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message, Modal};
use crate::i18n::tr;
use crate::icons::{Icon, icon};
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// How often a run's status file is read.
pub const POLL: Duration = Duration::from_secs(1);
/// Height of the footer bar.
pub const FOOTER_H: f32 = 22.0;

/// The update script's status file (`docs/self-update.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    pub run_id: String,
    pub phase: String,
    pub at: u64,
    pub from: String,
    pub to: String,
    pub message: String,
    pub script_pid: u32,
}

impl Status {
    /// The script is working (or waiting for the restart).
    pub fn running(&self) -> bool {
        matches!(self.phase.as_str(), "pulling" | "building" | "ready" | "swapping")
    }
    fn finished(&self) -> bool {
        matches!(self.phase.as_str(), "done" | "failed" | "rolledBack")
    }
}

#[derive(Debug, Clone)]
pub enum UpdateMsg {
    /// Ask first (the confirm dialog).
    Ask,
    Start,
    Tick,
    /// The build is ready: restart into it.
    Apply,
    /// Hide the last result.
    Dismiss,
}

/// The repository this app was built from (`crates/ide-app/../..`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Where the app runs from: `<target>\release` with `ide.exe` (and `ide-server.exe`).
fn app_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    (dir.file_name()? == "release").then_some(dir)
}

/// Self-update works here: Windows, run from a release build of a repository checkout that
/// has the script.
pub fn available() -> bool {
    let root = repo_root();
    cfg!(windows)
        && app_dir().is_some()
        && root.join(".git").exists()
        && root.join("Cargo.toml").is_file()
        && root.join("scripts").join("ide-update.ps1").is_file()
}

fn files(data_dir: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let d = data_dir.join("self-update");
    (
        d.join("status.json"),
        d.join("update.log"),
        d.join("apply-request"),
        d.join("ide-update.ps1"),
    )
}

/// Read the status file (a BOM is skipped).
pub fn read_status(data_dir: &Path) -> Option<Status> {
    let (status, ..) = files(data_dir);
    let text = std::fs::read_to_string(status).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// At startup: a run waiting for this app to come up (`swapping`) is confirmed (`done`).
pub fn confirm_started(data_dir: &Path) {
    let Some(mut s) = read_status(data_dir) else {
        return;
    };
    if s.phase == "swapping" {
        s.phase = "done".into();
        s.message.clear();
        let (status, ..) = files(data_dir);
        if let Ok(json) = serde_json::to_string(&s) {
            let _ = write_atomic(&status, &json);
        }
    }
}

/// Start a run: copy the script, start it detached; returns its run id.
fn start(data_dir: &Path) -> Result<String, String> {
    let root = repo_root();
    let dir = app_dir().ok_or("not a release build")?;
    let target = dir.parent().ok_or("no target dir")?.to_path_buf();
    let (status, log, apply, script) = files(data_dir);
    std::fs::create_dir_all(status.parent().expect("dir")).map_err(|e| e.to_string())?;
    std::fs::copy(root.join("scripts").join("ide-update.ps1"), &script)
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&apply);
    let _ = std::fs::write(&log, "");
    let run = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let first = Status {
        run_id: run.clone(),
        phase: "pulling".into(),
        ..Default::default()
    };
    write_atomic(&status, &serde_json::to_string(&first).expect("json"))
        .map_err(|e| e.to_string())?;
    spawn_script(&script, &[
        ("-RepoRoot", root.as_os_str()),
        ("-AppDir", dir.as_os_str()),
        ("-TargetDir", target.as_os_str()),
        ("-StatusFile", status.as_os_str()),
        ("-LogFile", log.as_os_str()),
        ("-ApplyFile", apply.as_os_str()),
        ("-AppPid", std::ffi::OsStr::new(&std::process::id().to_string())),
        ("-RunId", std::ffi::OsStr::new(&run)),
    ])
    .map_err(|e| e.to_string())?;
    Ok(run)
}

/// Start the script detached from the app: its own process group, outside the app's job when
/// allowed, with a hidden console (Windows PowerShell 5.1 exits at once without one).
#[cfg(windows)]
fn spawn_script(script: &Path, args: &[(&str, &std::ffi::OsStr)]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in args {
        cmd.arg(k).arg(v);
    }
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
    if cmd.spawn().is_ok() {
        return Ok(());
    }
    // The job does not allow breakaway: start inside it.
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    cmd.spawn().map(drop)
}

#[cfg(not(windows))]
fn spawn_script(_script: &Path, _args: &[(&str, &std::ffi::OsStr)]) -> std::io::Result<()> {
    Err(std::io::Error::other("self-update runs on Windows only"))
}

impl App {
    pub(crate) fn on_update(&mut self, m: UpdateMsg) -> Task<Message> {
        match m {
            UpdateMsg::Ask => {
                self.modal = Some(Modal::SelfUpdate);
                Task::none()
            }
            UpdateMsg::Start => {
                self.modal = None;
                match start(&self.data_dir) {
                    Ok(run) => {
                        crate::script::log(&format!("self-update run {run}"));
                        self.update_run = Some(run);
                        self.update_status = read_status(&self.data_dir);
                    }
                    Err(e) => self.update_error = Some(tr::update_start_failed(&e)),
                }
                Task::none()
            }
            UpdateMsg::Tick => {
                let s = read_status(&self.data_dir);
                // Ours swaps: this app quits (the server and its sessions stay).
                if let Some(s) = &s
                    && s.phase == "swapping"
                    && self.update_run.as_deref() == Some(s.run_id.as_str())
                {
                    return iced::exit();
                }
                self.update_status = s;
                Task::none()
            }
            UpdateMsg::Apply => {
                let Some(s) = &self.update_status else {
                    return Task::none();
                };
                if s.phase == "ready" {
                    let (_, _, apply, _) = files(&self.data_dir);
                    // Only the app that started the run quits on `swapping`.
                    self.update_run = Some(s.run_id.clone());
                    if let Err(e) = write_atomic(&apply, &s.run_id) {
                        self.update_error = Some(tr::update_start_failed(&e.to_string()));
                    }
                }
                Task::none()
            }
            UpdateMsg::Dismiss => {
                self.update_error = None;
                if let Some(s) = &self.update_status
                    && s.finished()
                {
                    self.ui.update_seen = Some(s.run_id.clone());
                    self.save_ui();
                }
                Task::none()
            }
        }
    }

    /// The notification-centre copy of the footer's update state: (signature, notice).
    pub(crate) fn update_notice(&self) -> Option<(String, crate::notice_view::AppNotice)> {
        use crate::notices::{ActionStyle, Notice, Text};
        let s = self.update_status.as_ref()?;
        if s.phase == "ready" {
            return Some((
                format!("{}:ready", s.run_id),
                Notice::info(Text::tr(tr::update_ready))
                    .action(Text::tr(tr::notice_restart), Message::Update(UpdateMsg::Apply), ActionStyle::Primary)
                    .toast(),
            ));
        }
        if !s.finished() || self.ui.update_seen.as_deref() == Some(s.run_id.as_str()) {
            return None;
        }
        let n = match s.phase.as_str() {
            "done" if s.message == "closed-before-restart" => Notice::success(Text::tr(tr::update_done_later)),
            "done" if !s.from.is_empty() && s.from != s.to => {
                let (from, to) = (s.from.clone(), s.to.clone());
                Notice::success(Text::tr(move || tr::update_done(&from, &to)))
            }
            "done" => Notice::success(Text::tr(tr::update_done_same)),
            "rolledBack" => Notice::error(Text::tr(tr::update_rolled_back)),
            _ => {
                let m = last_line(&s.message).to_owned();
                Notice::error(Text::tr(move || tr::update_failed(&m)))
            }
        };
        Some((format!("{}:{}", s.run_id, s.phase), n.toast()))
    }

    /// Poll the status file while a run is going.
    pub(crate) fn update_polling(&self) -> bool {
        self.update_available && self.update_status.as_ref().is_some_and(Status::running)
    }

    /// The footer: Desk's thin status bar (only the self-update control here).
    pub(crate) fn app_footer(&self) -> El<'_> {
        // Left: the server indicator (Desk's process dot + dim label); opens the settings page.
        let server = button(
            row![
                container(space().width(8).height(8)).style(style::dot(self.conn_color())),
                text(self.server_tooltip()).size(11).color(style::DIM),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([0, 4])
        .style(style::icon)
        .on_press(Message::Page(crate::ui_state::Page::Settings));
        let mut r = row![server, space().width(Fill)].spacing(8).align_y(Alignment::Center);
        let s = self.update_status.as_ref();
        // The last result, once (until dismissed).
        let result = s
            .filter(|s| s.finished() && self.ui.update_seen.as_deref() != Some(s.run_id.as_str()))
            .map(|s| match s.phase.as_str() {
                "done" if s.message == "closed-before-restart" => (tr::update_done_later(), style::GREEN),
                "done" if !s.from.is_empty() && s.from != s.to => {
                    (tr::update_done(&s.from, &s.to), style::GREEN)
                }
                "done" => (tr::update_done_same(), style::GREEN),
                "rolledBack" => (tr::update_rolled_back(), style::RED),
                _ => (tr::update_failed(last_line(&s.message)), style::RED),
            })
            .or_else(|| self.update_error.clone().map(|e| (e, style::RED)));
        if let Some((line, color)) = result {
            r = r.push(
                button(text(line).size(11).color(color))
                    .padding([0, 6])
                    .style(style::icon)
                    .on_press(Message::Update(UpdateMsg::Dismiss))
                    .boxed(),
            );
        }
        if self.update_available {
            let small = |i: Icon, c, msg: Option<Message>, label: String| {
                tip(
                    button(icon(i, 12.0, c))
                        .padding(3)
                        .style(style::icon)
                        .on_press_maybe(msg)
                        .boxed(),
                    label,
                )
            };
            match s.map(|s| s.phase.as_str()) {
                Some("ready") => {
                    r = r.push(small(
                        Icon::Refresh,
                        style::PURPLE,
                        Some(Message::Update(UpdateMsg::Apply)),
                        tr::update_ready(),
                    ));
                }
                Some(p @ ("pulling" | "building" | "swapping")) => {
                    let step = match p {
                        "pulling" => tr::update_step_pulling(),
                        "building" => tr::update_step_building(),
                        _ => tr::update_step_swapping(),
                    };
                    r = r.push(text(step).size(11).color(style::DIM).boxed());
                    r = r.push(small(Icon::Hammer, style::FAINT, None, tr::update_button()));
                }
                _ => {
                    r = r.push(small(
                        Icon::Hammer,
                        style::DIM,
                        Some(Message::Update(UpdateMsg::Ask)),
                        tr::update_button(),
                    ));
                }
            }
        }
        // A top border separates the bar from the rail / session list (same ground).
        let bar = container(r.padding([0, 8]))
            .height(FOOTER_H)
            .width(Fill)
            .align_y(Alignment::Center)
            .style(style::bg(style::SIDEBAR));
        column![
            container(space().height(1).width(Fill)).style(style::bg(style::BORDER)),
            bar,
        ]
        .boxed()
    }

    pub(crate) fn update_card(&self) -> El<'_> {
        column![
            text(tr::update_confirm_title()).size(18).font(style::BOLD),
            text(tr::update_confirm_body()).size(13).color(style::DIM),
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::CloseModal),
                button(text(tr::update_start()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press(Message::Update(UpdateMsg::Start)),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .width(480)
        .boxed()
    }
}

/// The first line of a script message (its detail; the log tail follows).
fn last_line(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim()
}
