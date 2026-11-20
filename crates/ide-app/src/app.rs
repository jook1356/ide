//! App state, messages and update logic.
//!
//! The app is a client of `ide-server`: profiles, sessions, worktrees and terminals live on the
//! server and are reached through [`ide_client::Client`] (RPC + notifications + terminal
//! streams). Only UI state (`ui.json`: selection, split, language, web URLs, ...) and the CEF web
//! views are local.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::operation;
use iced::{Event, Point, Size, Subscription, Task, event, keyboard, mouse, window};
use ide_client::{
    Agent, Client, ClientEvent, ClientOptions, LocalChild, NewSession, Notify, ProfileInfo,
    ProtoError, SessionInfo, SessionState, Ssh, Switchable, Transport, Welcome,
};
use ide_i18n::LanguagePref;
use ide_term::{TermConfig, TermModel, ViewEvent};
use ide_web::{WebEvent, WebRuntime, WebView, WebViewEvent};
use tokio::sync::mpsc;

use crate::i18n::{self, tr};
use crate::script::Script;
use crate::ui_state::{CloseAction, SPLIT_RANGE, ServerPref, UiState};

/// Width of the left sidebar in logical pixels.
pub const SIDEBAR_W: f32 = 264.0;
/// Width of the draggable divider between centre and detail.
pub const DIVIDER_W: f32 = 5.0;
/// URL of a new session's web tab.
pub const DEFAULT_URL: &str = "about:blank";
/// Profile name the server always has (used before the profile list arrived).
pub const DEFAULT_PROFILE: &str = "default";
/// Agent kinds offered in the new-session dialog.
pub const AGENTS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Shell];
/// Grid of a terminal before its widget was laid out.
const INITIAL_GRID: (u16, u16) = (120, 30);

pub const TERM_ID: &str = "ide-term";
pub const RENAME_ID: &str = "ide-rename";
pub const NEW_REPO_ID: &str = "ide-new-repo";
pub const URL_ID: &str = "ide-url";
pub const PROFILE_NAME_ID: &str = "ide-profile-name";
pub const SSH_HOST_ID: &str = "ide-ssh-host";

/// Process state of a session as shown in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// No process (never started since the server started, or stopped).
    Stopped,
    /// A start request is in flight.
    Starting,
    /// Process running (on the server).
    Running,
    /// The process exited by itself.
    Exited(Option<u32>),
    /// Starting failed (see `Live::error`).
    Failed,
}

impl RunState {
    pub fn is_alive(self) -> bool {
        matches!(self, RunState::Starting | RunState::Running)
    }
}

impl From<SessionState> for RunState {
    fn from(s: SessionState) -> Self {
        match s {
            SessionState::Stopped => RunState::Stopped,
            SessionState::Running => RunState::Running,
            SessionState::Exited(c) => RunState::Exited(c),
        }
    }
}

/// Connection to the server as shown in the sidebar footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    /// First connect in progress.
    Connecting,
    /// Connected and handshaken.
    Connected(Box<Welcome>),
    /// The connection dropped; the client reconnects in the background.
    Reconnecting,
    /// Connecting failed (the client keeps retrying while a client exists) or there is no
    /// client (server stopped, no server binary). Localized reason.
    Disconnected(String),
}

impl Conn {
    pub fn is_connected(&self) -> bool {
        matches!(self, Conn::Connected(_))
    }
}

/// One session: the server's view of it plus the local terminal model and web tab.
pub struct Live {
    pub info: SessionInfo,
    pub state: RunState,
    /// Terminal state fed by the server's output stream (survives reconnects).
    pub term: TermModel,
    /// `term.attach` was requested on the current client.
    pub attached: bool,
    /// Last grid size reported by the widget.
    pub size: Option<(u16, u16)>,
    pub error: Option<String>,
    pub web: Option<WebView>,
    pub url_input: String,
    pub web_title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
}

impl Live {
    fn new(info: SessionInfo, url: String, config: &TermConfig) -> Self {
        Self {
            state: info.state.into(),
            info,
            term: TermModel::new(INITIAL_GRID.0, INITIAL_GRID.1, config),
            attached: false,
            size: None,
            error: None,
            web: None,
            url_input: url,
            web_title: String::new(),
            loading: false,
            can_back: false,
            can_forward: false,
        }
    }

    pub fn id(&self) -> &str {
        &self.info.id
    }
}

pub struct NewForm {
    pub repo: String,
    pub profile: String,
    pub agent: Agent,
    pub title: String,
    pub base_ref: String,
    pub error: Option<String>,
    pub busy: bool,
}

pub struct DeleteForm {
    pub id: String,
    pub remove_worktree: bool,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Default)]
pub struct SettingsForm {
    pub new_name: String,
    pub renaming: Option<(String, String)>,
    pub error: Option<String>,
    pub busy: bool,
    /// Server choice being edited (applied with a confirm).
    pub ssh: bool,
    pub ssh_host: String,
    pub ssh_command: String,
    pub server_error: Option<String>,
}

impl SettingsForm {
    fn new(pref: &ServerPref) -> Self {
        let (ssh, ssh_host, ssh_command) = match pref {
            ServerPref::Local => (false, String::new(), String::new()),
            ServerPref::Ssh { host, command } => (true, host.clone(), command.clone()),
        };
        Self {
            ssh,
            ssh_host,
            ssh_command,
            ..Self::default()
        }
    }

    /// The edited server choice (`Err` = localized validation message).
    fn server_pref(&self) -> Result<ServerPref, String> {
        if !self.ssh {
            return Ok(ServerPref::Local);
        }
        let host = self.ssh_host.trim();
        if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
            return Err(tr::err_ssh_host());
        }
        Ok(ServerPref::Ssh {
            host: host.to_owned(),
            command: self.ssh_command.trim().to_owned(),
        })
    }
}

/// "Close the window while sessions run" dialog.
#[derive(Default)]
pub struct CloseForm {
    /// "Don't ask again".
    pub remember: bool,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Default)]
pub struct StopServerForm {
    pub error: Option<String>,
    pub busy: bool,
}

pub enum Modal {
    NewSession(NewForm),
    Delete(DeleteForm),
    Settings(SettingsForm),
    StopServer(StopServerForm),
    /// Confirm switching to another server.
    SwitchServer(ServerPref),
    Close(CloseForm),
}

/// A profile change requested from the settings dialog.
#[derive(Debug, Clone)]
pub enum ProfileOp {
    Created,
    Renamed { old: String, new: String },
    Deleted(String),
}

type Lists = (Vec<ProfileInfo>, Vec<SessionInfo>);

#[derive(Debug, Clone)]
pub enum Message {
    /// Client event of link generation `.0`.
    Client(u64, ClientEvent),
    Lists(u64, Result<Lists, ProtoError>),
    Attached(u64, String, Result<(), ProtoError>),
    TermInput(String, Vec<u8>),
    TermResize(String, u16, u16),
    TermView(ViewEvent),
    Web(WebViewEvent),

    Select(String),
    Start(String),
    Stop(String),
    SessionOp(String, Result<SessionInfo, ProtoError>),
    StartRename(String),
    RenameInput(String),
    RenameCommit,
    Renamed(Result<SessionInfo, ProtoError>),

    AskDelete(String),
    DeleteWorktree(bool),
    DeleteConfirm,
    DeleteDone(String, Result<(), ProtoError>),

    OpenNew,
    NewRepo(String),
    NewBrowse,
    NewBrowsed(Option<PathBuf>),
    NewProfile(String),
    NewAgent(Agent),
    NewTitle(String),
    NewBase(String),
    NewSubmit,
    NewCreated(Result<SessionInfo, ProtoError>),
    CloseModal,

    ProfilePicked(String),
    OpenSettings,
    SettingsNewName(String),
    SettingsCreate,
    SettingsStartRename(String),
    SettingsRenameInput(String),
    SettingsRenameCommit,
    SettingsDelete(String),
    ProfileDone(ProfileOp, Result<(), ProtoError>),
    SetLanguage(LanguagePref),

    AskStopServer,
    StopServerConfirm,
    ServerStopped(Result<(), ProtoError>),
    /// Reconnect now (or start a new client after the server was stopped).
    Retry,
    SettingsServerSsh(bool),
    SettingsSshHost(String),
    SettingsSshCommand(String),
    SettingsApplyServer,
    SwitchServerConfirm,
    SetOnClose(CloseAction),
    CloseRemember(bool),
    /// Close dialog: exit, sessions keep running in the daemon.
    CloseKeep,
    /// Close dialog: stop every session and the daemon, then exit.
    CloseStop,
    CloseStopped(Result<(), ProtoError>),

    UrlInput(String),
    UrlSubmit,
    WebBack,
    WebForward,
    WebReload,

    ToggleDetail,
    DividerPressed,
    DragMoved(Point),
    DragEnd,

    WindowOpened(Option<window::Id>),
    WindowResized(Size),
    CloseRequested,

    Tick,
    Shot(String, window::Screenshot),
}

/// Terminal input for the server, sent in order by one task per client.
enum Input {
    Write(String, Vec<u8>),
    Resize(String, u16, u16),
}

/// One client plus its ordered input pump. Dropping it disconnects (sessions keep running).
pub struct Link {
    pub client: Client,
    /// The transport's on/off switch (tests simulate an outage with it).
    pub switch: Switchable<Arc<dyn Transport>>,
    input: mpsc::UnboundedSender<Input>,
}

/// Writes keys/resizes in order and coalesces queued writes of one session into one request.
fn spawn_input_pump(client: Client) -> mpsc::UnboundedSender<Input> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            let mut merged: Vec<Input> = Vec::with_capacity(batch.len());
            for i in batch {
                match (merged.last_mut(), i) {
                    (Some(Input::Write(s, b)), Input::Write(s2, b2)) if *s == s2 => b.extend(b2),
                    (_, i) => merged.push(i),
                }
            }
            for i in merged {
                let r = match i {
                    Input::Write(s, b) => client.write(&s, b).await,
                    Input::Resize(s, c, r) => client.resize(&s, c, r).await,
                };
                if let Err(e) = r {
                    tracing::debug!("terminal input: {e}");
                }
            }
        }
    });
    tx
}

/// How to reach the server.
#[derive(Clone)]
pub struct Target {
    /// The transport, or why there is none (untranslated detail).
    pub transport: Result<Arc<dyn Transport>, String>,
    /// A local server (closing the window may stop it).
    pub local: bool,
    /// `ssh <host>` for remote servers (empty for local).
    pub remote: String,
}

/// `IDE_REMOTE=ssh:<host>` (tests, scripts) overrides the server setting.
pub fn build_target(pref: &ServerPref, remote_override: Option<&str>) -> Target {
    let ssh = |host: &str, command: &str| {
        let mut s = Ssh::new(host);
        if !command.is_empty() {
            s.server_command = command.to_owned();
        }
        Target {
            remote: s.describe(),
            transport: Ok(Arc::new(s) as Arc<dyn Transport>),
            local: false,
        }
    };
    if let Some(remote) = remote_override {
        return match remote.strip_prefix("ssh:") {
            Some(host) if !host.is_empty() => ssh(host, ""),
            _ => Target {
                transport: Err(format!("IDE_REMOTE={remote:?} (expected ssh:<host>)")),
                local: false,
                remote: remote.to_owned(),
            },
        };
    }
    match pref {
        ServerPref::Ssh { host, command } => ssh(host, command),
        ServerPref::Local => {
            let transport = LocalChild::find()
                .map(|mut l| {
                    // The server uses the same data dir (and so the same daemon).
                    if let Some(dir) = std::env::var("IDE_DATA_DIR").ok().filter(|v| !v.is_empty())
                    {
                        l = l.env("IDE_DATA_DIR", dir);
                    }
                    tracing::info!(server = %l.program.display(), "local server");
                    Arc::new(l) as Arc<dyn Transport>
                })
                .map_err(|e| e.to_string());
            Target {
                transport,
                local: true,
                remote: String::new(),
            }
        }
    }
}

pub struct App {
    pub data_dir: PathBuf,
    pub runtime: WebRuntime,
    /// `IDE_REMOTE` (overrides `ui.server`).
    pub remote_override: Option<String>,
    pub target: Target,
    pub link: Option<Link>,
    /// Incremented per new [`Link`]; results of older links are ignored.
    pub link_gen: u64,
    pub conn: Conn,
    /// Profile and session lists arrived at least once.
    pub loaded: bool,
    pub profiles: Vec<ProfileInfo>,
    pub sessions: Vec<Live>,
    pub selected: Option<String>,
    pub ui: UiState,
    pub term_config: TermConfig,
    pub window: Option<window::Id>,
    pub window_size: Size,
    pub dragging: bool,
    pub modal: Option<Modal>,
    pub rename: Option<(String, String)>,
    /// Problems outside a modal (failed RPCs, ...), shown in the sidebar.
    pub banner: Option<String>,
    pub script: Option<Script>,
    /// Test hook: keep drawing the web view under modals (reproduces the z-order bug).
    pub modal_web_unsafe: bool,
    /// `Welcome::server_instance` of the last connection: terminal offsets are only valid
    /// within one server process.
    pub server_instance: Option<String>,
}

impl App {
    pub fn boot(
        data_dir: PathBuf,
        runtime: WebRuntime,
        remote_override: Option<String>,
    ) -> (Self, Task<Message>) {
        let mut app = Self::load(data_dir, runtime, remote_override);
        app.script = Script::from_env(&app.data_dir);
        app.connect();
        let task = window::latest().map(Message::WindowOpened);
        (app, task)
    }

    /// App state from `ui.json` (no connection yet).
    pub fn load(data_dir: PathBuf, runtime: WebRuntime, remote_override: Option<String>) -> Self {
        let ui = UiState::load(&data_dir);
        ide_i18n::apply(ui.language);
        let target = build_target(&ui.server, remote_override.as_deref());
        let conn = match &target.transport {
            Ok(_) => Conn::Connecting,
            Err(e) => Conn::Disconnected(tr::err_server_missing(e)),
        };
        Self {
            data_dir,
            runtime,
            remote_override,
            target,
            link: None,
            link_gen: 0,
            conn,
            loaded: false,
            profiles: Vec::new(),
            sessions: Vec::new(),
            selected: None,
            ui,
            term_config: TermConfig::default(),
            window: None,
            window_size: Size::new(1400.0, 860.0),
            dragging: false,
            modal: None,
            rename: None,
            banner: None,
            script: None,
            modal_web_unsafe: false,
            server_instance: None,
        }
    }

    /// Start a new client (the previous one, if any, is dropped).
    pub fn connect(&mut self) {
        let transport = match &self.target.transport {
            Ok(t) => t.clone(),
            Err(e) => {
                self.conn = Conn::Disconnected(tr::err_server_missing(e));
                return;
            }
        };
        self.link = None;
        let switch = Switchable::new(transport);
        let client = Client::new(
            switch.clone(),
            ClientOptions {
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
                ..ClientOptions::default()
            },
        );
        crate::script::log(&format!(
            "client {} connecting ({})",
            client.id(),
            switch.describe()
        ));
        let input = spawn_input_pump(client.clone());
        self.link_gen += 1;
        self.link = Some(Link {
            client,
            switch,
            input,
        });
        self.conn = Conn::Connecting;
        for l in &mut self.sessions {
            l.attached = false;
        }
    }

    /// Drop the client (like closing the app): the server keeps every session running.
    pub fn disconnect(&mut self, reason: String) {
        self.link = None;
        self.conn = Conn::Disconnected(reason);
        for l in &mut self.sessions {
            l.attached = false;
        }
    }

    pub fn theme(&self) -> iced::Theme {
        crate::style::theme()
    }

    pub fn title(&self) -> String {
        "ide".into()
    }

    // ------------------------------------------------------------------ lookups

    pub fn client(&self) -> Option<Client> {
        self.link.as_ref().map(|l| l.client.clone())
    }

    /// The client, if connected (actions are disabled otherwise).
    fn online(&self) -> Option<Client> {
        self.conn.is_connected().then(|| self.client()).flatten()
    }

    pub fn connected(&self) -> bool {
        self.conn.is_connected() && self.link.is_some()
    }

    pub fn live(&self, id: &str) -> Option<&Live> {
        self.sessions.iter().find(|l| l.id() == id)
    }

    fn live_mut(&mut self, id: &str) -> Option<&mut Live> {
        self.sessions.iter_mut().find(|l| l.id() == id)
    }

    pub fn selected_live(&self) -> Option<&Live> {
        self.selected.as_deref().and_then(|id| self.live(id))
    }

    pub fn current_profile(&self) -> String {
        self.ui
            .profile
            .clone()
            .filter(|p| self.profiles.is_empty() || self.profiles.iter().any(|x| &x.name == p))
            .or_else(|| self.profiles.first().map(|p| p.name.clone()))
            .unwrap_or_else(|| DEFAULT_PROFILE.to_owned())
    }

    fn running_in_profile(&self, profile: &str) -> usize {
        self.sessions
            .iter()
            .filter(|l| l.info.profile == profile && l.state.is_alive())
            .count()
    }

    pub fn sessions_in_profile(&self, profile: &str) -> (usize, usize) {
        let total = self
            .sessions
            .iter()
            .filter(|l| l.info.profile == profile)
            .count();
        (total, self.running_in_profile(profile))
    }

    fn save_ui(&mut self) {
        self.ui.selected = self.selected.clone();
        self.ui.save(&self.data_dir);
    }

    // ------------------------------------------------------------------ server state

    fn refresh(&self) -> Task<Message> {
        let Some(c) = self.client() else {
            return Task::none();
        };
        let generation = self.link_gen;
        Task::perform(
            async move {
                let p = c.profiles().await?;
                let s = c.sessions().await?;
                Ok((p, s))
            },
            move |r| Message::Lists(generation, r),
        )
    }

    /// Merge the server's session list: keep terminal models and web views of known sessions.
    fn apply_lists(&mut self, profiles: Vec<ProfileInfo>, list: Vec<SessionInfo>) -> Task<Message> {
        self.profiles = profiles;
        let mut next = Vec::with_capacity(list.len());
        let mut old: Vec<Live> = std::mem::take(&mut self.sessions);
        for info in list {
            match old.iter().position(|l| l.info.id == info.id) {
                Some(i) => {
                    let mut l = old.remove(i);
                    // A start in flight stays "starting" until its reply arrives.
                    if l.state != RunState::Starting || info.state != SessionState::Stopped {
                        l.state = info.state.into();
                    }
                    l.info = info;
                    next.push(l);
                }
                None => {
                    let url = self
                        .ui
                        .web_urls
                        .get(&info.id)
                        .cloned()
                        .unwrap_or_else(|| DEFAULT_URL.into());
                    next.push(Live::new(info, url, &self.term_config));
                }
            }
        }
        for gone in old {
            if let Some(w) = gone.web {
                w.close();
            }
        }
        self.sessions = next;
        if !self.loaded {
            self.loaded = true;
            self.selected = self
                .ui
                .selected
                .clone()
                .filter(|id| self.live(id).is_some());
            crate::script::log(&format!(
                "lists loaded: {} profile(s), {} session(s)",
                self.profiles.len(),
                self.sessions.len()
            ));
        }
        if self
            .selected
            .as_deref()
            .is_some_and(|id| self.live(id).is_none())
        {
            let profile = self.current_profile();
            self.selected = self
                .sessions
                .iter()
                .find(|l| l.info.profile == profile)
                .map(|l| l.info.id.clone());
        }
        self.after_select()
    }

    /// Attach the terminal of every shown or running session, and of sessions with output
    /// (once per client). Attachments survive reconnects inside the client, so background
    /// sessions stay current and switching is instant.
    fn ensure_attached(&mut self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let generation = self.link_gen;
        let selected = self.selected.clone();
        let mut tasks = Vec::new();
        for l in &mut self.sessions {
            let shown = selected.as_deref() == Some(l.id());
            if l.attached || !(shown || l.state.is_alive() || l.info.term_end > 0) {
                continue;
            }
            l.attached = true;
            let off = l.term.last_offset();
            let from = (off > 0).then_some(off);
            let (c, id) = (c.clone(), l.info.id.clone());
            crate::script::log(&format!("attach {id} from {from:?}"));
            tasks.push(Task::perform(
                {
                    let id = id.clone();
                    async move { c.attach(&id, from).await.map(drop) }
                },
                move |r| Message::Attached(generation, id.clone(), r),
            ));
        }
        Task::batch(tasks)
    }

    fn send_input(&self, input: Input) {
        if let Some(l) = &self.link {
            let _ = l.input.send(input);
        }
    }

    fn on_client(&mut self, ev: ClientEvent) -> Task<Message> {
        match ev {
            ClientEvent::Connected(w) => {
                crate::script::log(&format!(
                    "connected: server {} (instance {}, resumed {})",
                    w.server_version, w.server_instance, w.resumed
                ));
                // A different server process (e.g. after "stop server"): its terminal offsets
                // start over and the old scrollback is gone, so start every model afresh.
                if self
                    .server_instance
                    .replace(w.server_instance.clone())
                    .is_some_and(|prev| prev != w.server_instance)
                {
                    for l in &mut self.sessions {
                        l.term.reset();
                        l.attached = false;
                    }
                }
                self.conn = Conn::Connected(Box::new(w));
                // The PTY size follows the most recent client: re-send ours.
                if let Some(l) = self.selected_live()
                    && let Some((c, r)) = l.size
                {
                    self.send_input(Input::Resize(l.info.id.clone(), c, r));
                }
                Task::batch([self.refresh(), self.ensure_attached()])
            }
            ClientEvent::Disconnected { error } => {
                crate::script::log(&format!("disconnected: {error:?}"));
                self.conn = match error {
                    None => Conn::Reconnecting,
                    Some(e) => Conn::Disconnected(i18n::proto_error(&e)),
                };
                Task::none()
            }
            ClientEvent::Notify(n) => self.on_notify(n),
            ClientEvent::Term(chunk) => {
                if let Some(l) = self.live_mut(&chunk.session) {
                    if chunk.reset {
                        l.term.reset();
                    }
                    if !chunk.bytes.is_empty() {
                        l.term.feed(chunk.offset, &chunk.bytes);
                    }
                }
                Task::none()
            }
            ClientEvent::AttachFailed { session, error } => {
                crate::script::log(&format!("attach {session} failed: {error}"));
                if let Some(l) = self.live_mut(&session) {
                    l.attached = false;
                }
                Task::none()
            }
        }
    }

    fn on_notify(&mut self, n: Notify) -> Task<Message> {
        match n {
            Notify::State { session, state } => {
                crate::script::log(&format!("session {session} state {state:?}"));
                if let Some(l) = self.live_mut(&session) {
                    l.state = state.into();
                    if state == SessionState::Running {
                        l.error = None;
                    }
                }
                self.ensure_attached()
            }
            Notify::Title { session, title } => {
                if let Some(l) = self.live_mut(&session) {
                    l.info.term_title = title;
                }
                Task::none()
            }
            Notify::Bell { .. } => Task::none(),
            Notify::SessionsChanged | Notify::ProfilesChanged => self.refresh(),
        }
    }

    // ------------------------------------------------------------------ web views

    fn detail_visible(&self) -> bool {
        !self.ui.detail_collapsed
    }

    /// Create the selected session's web view if needed and hide every other one.
    fn sync_web(&mut self) {
        let show = self
            .detail_visible()
            .then(|| self.selected.clone())
            .flatten();
        if let Some(id) = &show {
            let runtime = self.runtime.clone();
            if let Some(live) = self.live_mut(id)
                && live.web.is_none()
            {
                let url = if live.url_input.trim().is_empty() {
                    DEFAULT_URL.to_owned()
                } else {
                    live.url_input.clone()
                };
                match WebView::new(&runtime, &url, Size::new(640.0, 600.0), 1.0) {
                    Ok(v) => live.web = Some(v),
                    Err(e) => live.error = Some(tr::err_web_view(&format!("{e:#}"))),
                }
            }
        }
        for live in &self.sessions {
            if let Some(web) = &live.web {
                web.set_hidden(show.as_deref() != Some(live.id()));
            }
        }
    }

    fn after_select(&mut self) -> Task<Message> {
        self.sync_web();
        let attach = self.ensure_attached();
        if self.selected_live().is_some() && self.modal.is_none() && self.rename.is_none() {
            Task::batch([attach, operation::focus(TERM_ID)])
        } else {
            attach
        }
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Client(generation, ev) => {
                if generation != self.link_gen || self.link.is_none() {
                    return Task::none();
                }
                self.on_client(ev)
            }
            Message::Lists(generation, r) => {
                if generation != self.link_gen {
                    return Task::none();
                }
                match r {
                    Ok((p, s)) => self.apply_lists(p, s),
                    Err(e) => {
                        if e.id != "disconnected" {
                            self.banner = Some(i18n::proto_error(&e));
                        }
                        Task::none()
                    }
                }
            }
            Message::Attached(generation, id, r) => {
                if generation == self.link_gen
                    && let Err(e) = r
                {
                    crate::script::log(&format!("attach {id}: {e}"));
                    // `disconnected`: the client attaches on the next connect by itself.
                    if e.id != "disconnected"
                        && let Some(l) = self.live_mut(&id)
                    {
                        l.attached = false;
                    }
                }
                Task::none()
            }
            Message::TermInput(id, bytes) => {
                if self.connected() {
                    self.send_input(Input::Write(id, bytes));
                }
                Task::none()
            }
            Message::TermResize(id, cols, rows) => {
                if let Some(l) = self.live_mut(&id) {
                    l.size = Some((cols, rows));
                }
                if self.connected() {
                    self.send_input(Input::Resize(id, cols, rows));
                }
                Task::none()
            }
            Message::TermView(ev) => {
                if let ViewEvent::Copied(text) = ev {
                    tracing::debug!(chars = text.chars().count(), "terminal copy");
                }
                Task::none()
            }
            Message::Web(ev) => {
                self.on_web(ev);
                Task::none()
            }

            Message::Select(id) => {
                if self.live(&id).is_none() {
                    return Task::none();
                }
                self.selected = Some(id);
                self.save_ui();
                self.after_select()
            }
            Message::Start(id) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                if let Some(l) = self.live_mut(&id) {
                    l.state = RunState::Starting;
                    l.error = None;
                }
                self.selected = Some(id.clone());
                self.save_ui();
                let task = Task::perform(
                    {
                        let id = id.clone();
                        async move { c.start_session(&id).await }
                    },
                    move |r| Message::SessionOp(id.clone(), r),
                );
                Task::batch([task, self.after_select()])
            }
            Message::Stop(id) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                Task::perform(
                    {
                        let id = id.clone();
                        async move { c.stop_session(&id).await }
                    },
                    move |r| Message::SessionOp(id.clone(), r),
                )
            }
            Message::SessionOp(id, r) => {
                match r {
                    Ok(info) => {
                        crate::script::log(&format!("session {id} now {:?}", info.state));
                        if let Some(l) = self.live_mut(&id) {
                            l.state = info.state.into();
                            l.info = info;
                        }
                    }
                    Err(e) => {
                        let msg = i18n::proto_error(&e);
                        crate::script::log(&format!("session {id} op failed: {e}"));
                        if let Some(l) = self.live_mut(&id) {
                            l.state = RunState::Failed;
                            l.error = Some(msg);
                        }
                    }
                }
                self.ensure_attached()
            }
            Message::StartRename(id) => {
                if !self.connected() {
                    return Task::none();
                }
                let title = self
                    .live(&id)
                    .map(|l| l.info.title.clone())
                    .unwrap_or_default();
                self.rename = Some((id, title));
                operation::focus(RENAME_ID)
            }
            Message::RenameInput(t) => {
                if let Some((_, v)) = &mut self.rename {
                    *v = t;
                }
                Task::none()
            }
            Message::RenameCommit => {
                let mut task = Task::none();
                if let Some((id, title)) = self.rename.take() {
                    let title = title.trim().to_owned();
                    if !title.is_empty()
                        && let Some(c) = self.online()
                    {
                        task = Task::perform(
                            async move { c.rename_session(&id, &title).await },
                            Message::Renamed,
                        );
                    }
                }
                Task::batch([task, self.after_select()])
            }
            Message::Renamed(r) => {
                match r {
                    Ok(info) => {
                        let id = info.id.clone();
                        if let Some(l) = self.live_mut(&id) {
                            l.info = info;
                        }
                    }
                    Err(e) => self.banner = Some(tr::err_rename_failed(&i18n::proto_error(&e))),
                }
                Task::none()
            }

            Message::AskDelete(id) => {
                if !self.connected() {
                    return Task::none();
                }
                self.modal = Some(Modal::Delete(DeleteForm {
                    id,
                    remove_worktree: true,
                    error: None,
                    busy: false,
                }));
                Task::none()
            }
            Message::DeleteWorktree(v) => {
                if let Some(Modal::Delete(f)) = &mut self.modal {
                    f.remove_worktree = v;
                }
                Task::none()
            }
            Message::DeleteConfirm => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Delete(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let id = f.id.clone();
                let remove = f.remove_worktree;
                // The server stops the process, waits for it and removes worktree + branch.
                Task::perform(
                    {
                        let id = id.clone();
                        async move { c.delete_session(&id, remove).await }
                    },
                    move |r| Message::DeleteDone(id.clone(), r),
                )
            }
            Message::DeleteDone(id, r) => {
                let remove = match &self.modal {
                    Some(Modal::Delete(f)) if f.id == id => f.remove_worktree,
                    _ => false,
                };
                match r {
                    Ok(()) => {
                        let worktree = self
                            .live(&id)
                            .map(|l| l.info.worktree_path.clone())
                            .unwrap_or_default();
                        crate::script::log(&format!(
                            "deleted session {id} (remove_worktree={remove}, worktree {worktree})"
                        ));
                        if let Some(idx) = self.sessions.iter().position(|l| l.id() == id) {
                            let live = self.sessions.remove(idx);
                            if let Some(w) = live.web {
                                w.close();
                            }
                        }
                        self.ui.web_urls.remove(&id);
                        if self.selected.as_deref() == Some(id.as_str()) {
                            let profile = self.current_profile();
                            self.selected = self
                                .sessions
                                .iter()
                                .find(|l| l.info.profile == profile)
                                .map(|l| l.info.id.clone());
                        }
                        if matches!(&self.modal, Some(Modal::Delete(f)) if f.id == id) {
                            self.modal = None;
                        }
                        self.save_ui();
                        self.after_select()
                    }
                    Err(e) => {
                        if let Some(Modal::Delete(f)) = &mut self.modal {
                            f.busy = false;
                            f.error = Some(i18n::proto_error(&e));
                        }
                        Task::none()
                    }
                }
            }

            Message::OpenNew => {
                if !self.connected() {
                    return Task::none();
                }
                let profile = self.current_profile();
                self.modal = Some(Modal::NewSession(NewForm {
                    repo: self.ui.last_repo.clone().unwrap_or_default(),
                    profile,
                    agent: Agent::Claude,
                    title: String::new(),
                    base_ref: String::new(),
                    error: None,
                    busy: false,
                }));
                operation::focus(NEW_REPO_ID)
            }
            Message::NewRepo(v) => self.with_new(|f| f.repo = v),
            Message::NewBrowse => {
                let start = match &self.modal {
                    Some(Modal::NewSession(f)) if !f.repo.trim().is_empty() => {
                        Some(PathBuf::from(f.repo.trim()))
                    }
                    _ => None,
                };
                let title = tr::repo_dialog_title();
                Task::perform(
                    async move {
                        let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
                        if let Some(dir) = start.filter(|d| d.is_dir()) {
                            dialog = dialog.set_directory(dir);
                        }
                        dialog.pick_folder().await.map(|h| h.path().to_path_buf())
                    },
                    Message::NewBrowsed,
                )
            }
            Message::NewBrowsed(path) => match path {
                Some(p) => self.with_new(|f| f.repo = p.display().to_string()),
                None => Task::none(),
            },
            Message::NewProfile(v) => self.with_new(|f| f.profile = v),
            Message::NewAgent(a) => self.with_new(|f| f.agent = a),
            Message::NewTitle(v) => self.with_new(|f| f.title = v),
            Message::NewBase(v) => self.with_new(|f| f.base_ref = v),
            Message::NewSubmit => self.submit_new(),
            Message::NewCreated(result) => self.on_created(result),
            Message::CloseModal => {
                // Escape also cancels an inline rename.
                if self.rename.take().is_some() && self.modal.is_none() {
                    return self.after_select();
                }
                let busy = match &self.modal {
                    Some(Modal::Delete(f)) => f.busy,
                    Some(Modal::StopServer(f)) => f.busy,
                    Some(Modal::Close(f)) => f.busy,
                    _ => false,
                };
                if busy {
                    return Task::none();
                }
                self.modal = None;
                self.after_select()
            }

            Message::ProfilePicked(p) => {
                if self.profiles.iter().any(|x| x.name == p) {
                    self.ui.profile = Some(p.clone());
                    let sel_profile = self.selected_live().map(|l| l.info.profile.clone());
                    if sel_profile.as_deref() != Some(p.as_str()) {
                        self.selected = self
                            .sessions
                            .iter()
                            .find(|l| l.info.profile == p)
                            .map(|l| l.info.id.clone());
                    }
                    self.save_ui();
                }
                self.after_select()
            }
            Message::OpenSettings => {
                self.modal = Some(Modal::Settings(SettingsForm::new(&self.ui.server)));
                operation::focus(PROFILE_NAME_ID)
            }
            Message::SettingsNewName(v) => {
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    f.new_name = v;
                }
                Task::none()
            }
            Message::SettingsCreate => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                let name = f.new_name.trim().to_owned();
                f.busy = true;
                Task::perform(
                    async move { c.create_profile(&name).await.map(drop) },
                    |r| Message::ProfileDone(ProfileOp::Created, r),
                )
            }
            Message::SettingsStartRename(name) => {
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    f.renaming = Some((name.clone(), name));
                    f.error = None;
                }
                Task::none()
            }
            Message::SettingsRenameInput(v) => {
                if let Some(Modal::Settings(f)) = &mut self.modal
                    && let Some((_, new)) = &mut f.renaming
                {
                    *new = v;
                }
                Task::none()
            }
            Message::SettingsRenameCommit => {
                let renaming = match &self.modal {
                    Some(Modal::Settings(f)) => f.renaming.clone(),
                    _ => None,
                };
                let Some((old, new)) = renaming else {
                    return Task::none();
                };
                let new = new.trim().to_owned();
                if old == new {
                    if let Some(Modal::Settings(f)) = &mut self.modal {
                        f.renaming = None;
                    }
                    return Task::none();
                }
                let running = self.running_in_profile(&old);
                if running > 0 {
                    if let Some(Modal::Settings(f)) = &mut self.modal {
                        f.error = Some(tr::err_profile_running(i18n::count(running), &old));
                    }
                    return Task::none();
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let op = ProfileOp::Renamed {
                    old: old.clone(),
                    new: new.clone(),
                };
                Task::perform(
                    async move { c.rename_profile(&old, &new).await.map(drop) },
                    move |r| Message::ProfileDone(op.clone(), r),
                )
            }
            Message::SettingsDelete(name) => {
                let (total, _) = self.sessions_in_profile(&name);
                if total > 0 {
                    if let Some(Modal::Settings(f)) = &mut self.modal {
                        f.error = Some(tr::err_profile_in_use(i18n::count(total), &name));
                    }
                    return Task::none();
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let op = ProfileOp::Deleted(name.clone());
                Task::perform(
                    async move { c.delete_profile(&name, false).await },
                    move |r| Message::ProfileDone(op.clone(), r),
                )
            }
            Message::ProfileDone(op, r) => {
                crate::script::log(&format!("profile {op:?}: {r:?}"));
                let ok = r.is_ok();
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    f.busy = false;
                    match r {
                        Ok(()) => {
                            f.error = None;
                            match &op {
                                ProfileOp::Created => f.new_name.clear(),
                                ProfileOp::Renamed { .. } => f.renaming = None,
                                ProfileOp::Deleted(_) => {}
                            }
                        }
                        Err(e) => f.error = Some(i18n::proto_error(&e)),
                    }
                }
                if ok {
                    match &op {
                        ProfileOp::Renamed { old, new }
                            if self.ui.profile.as_deref() == Some(old.as_str()) =>
                        {
                            self.ui.profile = Some(new.clone());
                            self.save_ui();
                        }
                        ProfileOp::Deleted(name)
                            if self.ui.profile.as_deref() == Some(name.as_str()) =>
                        {
                            self.ui.profile = None;
                            self.save_ui();
                        }
                        _ => {}
                    }
                }
                // The server also notifies; refresh now so the dialog updates without a race.
                self.refresh()
            }

            Message::SetLanguage(pref) => {
                // Every `view` reads the current language, so the switch is instant.
                self.ui.language = pref;
                let lang = ide_i18n::apply(pref);
                crate::script::log(&format!("language {pref:?} -> {}", lang.code()));
                self.save_ui();
                Task::none()
            }

            Message::AskStopServer => {
                if self.connected() {
                    self.modal = Some(Modal::StopServer(StopServerForm::default()));
                }
                Task::none()
            }
            Message::StopServerConfirm => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::StopServer(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                Task::perform(
                    async move { c.shutdown_server().await },
                    Message::ServerStopped,
                )
            }
            Message::ServerStopped(r) => match r {
                Ok(()) => {
                    crate::script::log("server stopped by the user");
                    // Drop the client: reconnecting would start a new daemon.
                    self.disconnect(tr::conn_server_stopped());
                    for l in &mut self.sessions {
                        if l.state.is_alive() {
                            l.state = RunState::Stopped;
                        }
                    }
                    self.modal = None;
                    Task::none()
                }
                Err(e) => {
                    if let Some(Modal::StopServer(f)) = &mut self.modal {
                        f.busy = false;
                        f.error = Some(i18n::proto_error(&e));
                    }
                    Task::none()
                }
            },
            Message::Retry => {
                match &self.link {
                    Some(l) => l.client.reconnect(),
                    None => self.connect(),
                }
                Task::none()
            }

            Message::UrlInput(v) => {
                if let Some(id) = self.selected.clone()
                    && let Some(live) = self.live_mut(&id)
                {
                    live.url_input = v;
                }
                Task::none()
            }
            Message::UrlSubmit => {
                self.sync_web();
                if let Some(live) = self.selected_live() {
                    let url = normalize_url(&live.url_input);
                    if let Some(w) = &live.web {
                        w.navigate(&url);
                    }
                }
                Task::none()
            }
            Message::WebBack => self.with_web(|w| w.go_back()),
            Message::WebForward => self.with_web(|w| w.go_forward()),
            Message::WebReload => self.with_web(|w| w.reload()),

            Message::ToggleDetail => {
                self.ui.detail_collapsed = !self.ui.detail_collapsed;
                self.save_ui();
                self.sync_web();
                Task::none()
            }
            Message::DividerPressed => {
                self.dragging = true;
                Task::none()
            }
            Message::DragMoved(p) => {
                if self.dragging {
                    let avail = (self.window_size.width - SIDEBAR_W - DIVIDER_W).max(1.0);
                    self.ui.split_ratio =
                        ((p.x - SIDEBAR_W) / avail).clamp(SPLIT_RANGE.0, SPLIT_RANGE.1);
                }
                Task::none()
            }
            Message::DragEnd => {
                if self.dragging {
                    self.dragging = false;
                    self.save_ui();
                }
                Task::none()
            }

            Message::WindowOpened(id) => {
                self.window = id;
                match id {
                    Some(id) => window::size(id).map(Message::WindowResized),
                    None => Task::none(),
                }
            }
            Message::WindowResized(size) => {
                self.window_size = size;
                Task::none()
            }
            Message::CloseRequested => self.on_close_requested(),
            Message::CloseRemember(v) => {
                if let Some(Modal::Close(f)) = &mut self.modal {
                    f.remember = v;
                }
                Task::none()
            }
            Message::CloseKeep => {
                if let Some(Modal::Close(f)) = &self.modal
                    && f.remember
                {
                    self.ui.on_close = CloseAction::Keep;
                }
                self.exit_app()
            }
            Message::CloseStop => {
                let remember = matches!(&self.modal, Some(Modal::Close(f)) if f.remember);
                if remember {
                    self.ui.on_close = CloseAction::Stop;
                    self.save_ui();
                }
                let Some(c) = self.online() else {
                    return self.exit_app();
                };
                if let Some(Modal::Close(f)) = &mut self.modal {
                    f.busy = true;
                    f.error = None;
                }
                Task::perform(
                    async move { c.shutdown_server().await },
                    Message::CloseStopped,
                )
            }
            Message::CloseStopped(r) => match r {
                Ok(()) => {
                    crate::script::log("closing: server stopped");
                    self.exit_app()
                }
                Err(e) if e.id == "disconnected" => self.exit_app(),
                Err(e) => {
                    if let Some(Modal::Close(f)) = &mut self.modal {
                        f.busy = false;
                        f.error = Some(i18n::proto_error(&e));
                    }
                    Task::none()
                }
            },
            Message::SetOnClose(a) => {
                self.ui.on_close = a;
                self.save_ui();
                Task::none()
            }
            Message::SettingsServerSsh(v) => self.with_settings(|f| {
                f.ssh = v;
                f.server_error = None;
            }),
            Message::SettingsSshHost(v) => self.with_settings(|f| f.ssh_host = v),
            Message::SettingsSshCommand(v) => self.with_settings(|f| f.ssh_command = v),
            Message::SettingsApplyServer => {
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                match f.server_pref() {
                    Err(e) => f.server_error = Some(e),
                    Ok(pref) if pref == self.ui.server => f.server_error = None,
                    Ok(pref) => self.modal = Some(Modal::SwitchServer(pref)),
                }
                Task::none()
            }
            Message::SwitchServerConfirm => {
                let Some(Modal::SwitchServer(pref)) = self.modal.take() else {
                    return Task::none();
                };
                crate::script::log(&format!("switch server to {pref:?}"));
                // Sessions on the previous server keep running there.
                self.ui.server = pref;
                self.close_for_reopen();
                self.connect();
                Task::none()
            }

            Message::Tick => crate::script::tick(self),
            Message::Shot(name, shot) => {
                crate::script::save_shot(self, &name, &shot);
                Task::none()
            }
        }
    }

    fn with_settings(&mut self, f: impl FnOnce(&mut SettingsForm)) -> Task<Message> {
        if let Some(Modal::Settings(form)) = &mut self.modal {
            f(form);
        }
        Task::none()
    }

    /// Window close: with a local server and running sessions, ask (unless remembered)
    /// whether to keep them running in the background or stop everything.
    fn on_close_requested(&mut self) -> Task<Message> {
        if matches!(self.modal, Some(Modal::Close(_))) {
            return Task::none();
        }
        let local = self.target.local && self.connected();
        let running = self.sessions.iter().any(|l| l.state.is_alive());
        crate::script::log(&format!(
            "close requested (local {local}, running {running}, setting {:?})",
            self.ui.on_close
        ));
        match self.ui.on_close {
            CloseAction::Ask if local && running => {
                self.rename = None;
                self.modal = Some(Modal::Close(CloseForm::default()));
                Task::none()
            }
            CloseAction::Stop if local => {
                self.modal = Some(Modal::Close(CloseForm {
                    busy: true,
                    ..CloseForm::default()
                }));
                self.update(Message::CloseStop)
            }
            // Keep (or nothing running): the daemon keeps running and exits by itself when
            // idle (no clients, no sessions) for its `--idle-exit` time.
            _ => self.exit_app(),
        }
    }

    fn exit_app(&mut self) -> Task<Message> {
        self.save_ui();
        crate::script::finish(self);
        self.link = None;
        iced::exit()
    }

    fn with_new(&mut self, f: impl FnOnce(&mut NewForm)) -> Task<Message> {
        if let Some(Modal::NewSession(form)) = &mut self.modal {
            f(form);
        }
        Task::none()
    }

    fn with_web(&mut self, f: impl FnOnce(&WebView)) -> Task<Message> {
        if let Some(w) = self.selected_live().and_then(|l| l.web.as_ref()) {
            f(w);
        }
        Task::none()
    }

    fn submit_new(&mut self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let (cols, rows) = self
            .selected_live()
            .and_then(|l| l.size)
            .unwrap_or(INITIAL_GRID);
        let Some(Modal::NewSession(f)) = &mut self.modal else {
            return Task::none();
        };
        if f.busy {
            return Task::none();
        }
        let repo = f.repo.trim().trim_matches('"').to_owned();
        if repo.is_empty() {
            f.error = Some(tr::err_choose_repo());
            return Task::none();
        }
        f.busy = true;
        f.error = None;
        let req = NewSession {
            profile: f.profile.clone(),
            repo_path: repo.clone(),
            agent: f.agent,
            title: f.title.clone(),
            base_ref: f.base_ref.clone(),
            start: true,
            cols,
            rows,
        };
        self.ui.last_repo = Some(repo);
        // git runs on the server.
        Task::perform(
            async move { c.create_session(req).await },
            Message::NewCreated,
        )
    }

    fn on_created(&mut self, result: Result<SessionInfo, ProtoError>) -> Task<Message> {
        let info = match result {
            Ok(v) => v,
            Err(e) => {
                crate::script::log(&format!("new session error: {e}"));
                if let Some(Modal::NewSession(f)) = &mut self.modal {
                    f.busy = false;
                    f.error = Some(i18n::proto_error(&e));
                }
                return Task::none();
            }
        };
        crate::script::log(&format!(
            "created session {} {:?} worktree={} branch={} state={:?}",
            info.id, info.title, info.worktree_path, info.branch, info.state
        ));
        let id = info.id.clone();
        let profile = info.profile.clone();
        match self.live_mut(&id) {
            Some(l) => {
                l.state = info.state.into();
                l.info = info;
            }
            None => {
                let live = Live::new(info, DEFAULT_URL.into(), &self.term_config);
                self.sessions.push(live);
            }
        }
        self.ui.profile = Some(profile);
        self.selected = Some(id);
        self.modal = None;
        self.save_ui();
        self.after_select()
    }

    fn on_web(&mut self, ev: WebViewEvent) {
        let show = self
            .detail_visible()
            .then(|| self.selected.clone())
            .flatten();
        let Some(live) = self
            .sessions
            .iter_mut()
            .find(|l| l.web.as_ref().is_some_and(|w| w.id() == ev.view))
        else {
            return;
        };
        match ev.event {
            WebEvent::Created => {
                if let Some(w) = &live.web {
                    w.set_hidden(show.as_deref() != Some(live.info.id.as_str()));
                }
            }
            WebEvent::UrlChanged(u) => {
                live.url_input = u.clone();
                self.ui.web_urls.insert(live.info.id.clone(), u);
                self.ui.save(&self.data_dir);
            }
            WebEvent::TitleChanged(t) => live.web_title = t,
            WebEvent::LoadingStateChanged {
                is_loading,
                can_go_back,
                can_go_forward,
            } => {
                live.loading = is_loading;
                live.can_back = can_go_back;
                live.can_forward = can_go_forward;
            }
            WebEvent::LoadError { url, error } => {
                tracing::warn!("web load error {url}: {error}");
            }
            WebEvent::NewWindowRequested(url) => {
                if let Some(w) = &live.web {
                    w.navigate(&url);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ subscriptions

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            ide_web::events().map(Message::Web),
            window::resize_events().map(|(_, s)| Message::WindowResized(s)),
            window::close_requests().map(|_| Message::CloseRequested),
        ];
        if let Some(l) = &self.link {
            subs.push(
                ide_client::iced_support::subscription(&l.client)
                    .with(self.link_gen)
                    .map(|(generation, e)| Message::Client(generation, e)),
            );
        }
        if self.dragging {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::DragMoved(position))
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::DragEnd)
                }
                _ => None,
            }));
        }
        if self.modal.is_some() || self.rename.is_some() {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseModal),
                _ => None,
            }));
        }
        if self.script.as_ref().is_some_and(|s| !s.done()) {
            subs.push(iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    /// Simulates closing the app (test script): drop the client and every piece of state; the
    /// server keeps the sessions running. [`App::connect`] then acts like reopening it.
    pub fn close_for_reopen(&mut self) {
        self.save_ui();
        self.link = None;
        for live in &self.sessions {
            if let Some(w) = &live.web {
                w.close();
            }
        }
        let window = self.window;
        let size = self.window_size;
        let script = self.script.take();
        *self = App::load(
            self.data_dir.clone(),
            self.runtime.clone(),
            self.remote_override.clone(),
        );
        self.window = window;
        self.window_size = size;
        self.script = script;
        self.conn = Conn::Disconnected(tr::conn_closed());
    }
}

/// Keep URLs with a scheme (`https://`, `about:`, `data:`, ...); add `http://` to local hosts
/// and `https://` to other bare host names.
pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return DEFAULT_URL.into();
    }
    let lower = s.to_ascii_lowercase();
    let has_scheme = lower.contains("://")
        || ["about:", "data:", "file:", "javascript:", "chrome:"]
            .iter()
            .any(|p| lower.starts_with(p));
    if has_scheme {
        s.to_owned()
    } else if lower.starts_with("localhost") || lower.starts_with("127.0.0.1") {
        format!("http://{s}")
    } else {
        format!("https://{s}")
    }
}

/// Compare two server paths for grouping (separators ignored; case too for Windows paths).
pub fn same_path(a: &str, b: &str) -> bool {
    fn norm(p: &str) -> String {
        let p = p.replace('\\', "/");
        let p = p.trim_end_matches('/');
        if cfg!(windows) || p.contains(':') {
            p.to_lowercase()
        } else {
            p.to_owned()
        }
    }
    norm(a) == norm(b)
}

/// Last component of a server path (either separator).
pub fn path_name(p: &str) -> &str {
    p.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::{normalize_url, path_name, same_path};

    #[test]
    fn urls() {
        assert_eq!(normalize_url(""), "about:blank");
        assert_eq!(normalize_url(" example.com "), "https://example.com");
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("http://x"), "http://x");
        assert_eq!(normalize_url("data:text/html,hi"), "data:text/html,hi");
        assert_eq!(normalize_url("about:blank"), "about:blank");
    }

    #[test]
    fn paths() {
        assert!(same_path("C:\\src\\Repo\\", "c:/src/repo"));
        assert!(!same_path("C:/src/a", "C:/src/b"));
        assert_eq!(path_name("C:\\src\\my-repo\\"), "my-repo");
        assert_eq!(path_name("/home/me/repo"), "repo");
        assert_eq!(path_name("repo"), "repo");
    }
}
