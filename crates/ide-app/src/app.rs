//! App state, messages and update logic.
//!
//! The app is a client of `ide-server`: providers, accounts, configurations, sessions,
//! worktrees and terminals live on the
//! server and are reached through [`ide_client::Client`] (RPC + notifications + terminal
//! streams). Only UI state (`ui.json`: selection, split, language, web URLs, ...) and the CEF web
//! views are local.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{operation, text_editor};
use iced::{Event, Point, Size, Subscription, Task, event, keyboard, mouse, window};
use ide_client::{
    AccountInfo, AccountState, BrowseConfig, BrowseListing, Client, ClientEvent, ClientOptions,
    ConfigurationInfo, DefTarget, FileMatch, LocalChild, McpServerInfo, NewSession, Notify,
    ProtoError, ProviderInfo, SessionInfo, SessionKind, SessionState, Ssh, Switchable, Transport,
    Welcome,
};
use ide_i18n::LanguagePref;
use ide_term::{TermConfig, TermModel, ViewEvent};
use ide_web::{WebEvent, WebRuntime, WebView, WebViewEvent};
use tokio::sync::mpsc;

use crate::detail::{Detail, DetailMsg, ItemKey};
use crate::i18n::{self, tr};
use crate::script::Script;
use crate::ui_state::{CloseAction, DetailUi, SPLIT_RANGE, ServerPref, UiState};

/// Width of the left sidebar in logical pixels.
pub const SIDEBAR_W: f32 = 264.0;
/// Width of the draggable divider between centre and detail.
pub const DIVIDER_W: f32 = 5.0;
/// URL of a new session's web tab.
pub const DEFAULT_URL: &str = "about:blank";
/// Configuration slug a fresh server has (used before the configuration list arrived).
pub const DEFAULT_CONFIGURATION: &str = "default";
/// Grid of a terminal before its widget was laid out.
const INITIAL_GRID: (u16, u16) = (120, 30);

pub const TERM_ID: &str = "ide-term";
pub const RENAME_ID: &str = "ide-rename";
pub const NEW_REPO_ID: &str = "ide-new-repo";
pub const URL_ID: &str = "ide-url";
pub const CONFIG_NAME_ID: &str = "ide-config-name";
pub const SSH_HOST_ID: &str = "ide-ssh-host";
pub const ROOT_INPUT_ID: &str = "ide-root-input";

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
    /// Files, diffs, tree, changes and search of the session.
    pub detail: Detail,
}

impl Live {
    fn new(info: SessionInfo, url: String, config: &TermConfig, detail: Option<&DetailUi>) -> Self {
        Self {
            detail: Detail::new(detail),
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
    /// Provider id.
    pub provider: String,
    /// Account slug (`None` = the CLI's default login).
    pub account: Option<String>,
    /// Configuration slug.
    pub configuration: String,
    pub title: String,
    pub base_ref: String,
    pub error: Option<String>,
    pub busy: bool,
    /// Server-side folder browser (open while `Some`).
    pub browser: Option<FolderBrowser>,
}

/// Folders on the server (works over SSH too): the browse roots and one listed folder.
#[derive(Default)]
pub struct FolderBrowser {
    pub roots: Vec<String>,
    pub listing: Option<BrowseListing>,
    pub loading: bool,
    pub error: Option<String>,
    /// Incremented per request; older answers are dropped.
    pub seq: u64,
    /// The folder last asked for.
    pub dir: String,
}

/// Ctrl+P: files of the session by fuzzy name.
#[derive(Default)]
pub struct QuickForm {
    pub query: String,
    pub results: Vec<FileMatch>,
    pub selected: usize,
    pub seq: u64,
    pub error: Option<String>,
}

/// Several go-to-definition candidates.
pub struct DefPickerForm {
    pub session: String,
    pub name: String,
    pub targets: Vec<DefTarget>,
}

/// Confirm deleting a file / folder from the tree.
pub struct DeleteEntryForm {
    pub session: String,
    pub path: String,
    pub is_dir: bool,
    pub error: Option<String>,
    pub busy: bool,
}

/// Closing a file with unsaved edits.
pub struct CloseDirtyForm {
    pub session: String,
    pub path: String,
}

pub struct DeleteForm {
    pub id: String,
    pub remove_worktree: bool,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Default)]
pub struct SettingsForm {
    /// Provider of the account to add.
    pub acct_provider: String,
    pub acct_name: String,
    /// Account being renamed: (provider, slug, new name).
    pub acct_renaming: Option<(String, String, String)>,
    /// Last status check per account (provider, slug).
    pub acct_status: Vec<((String, String), Option<AccountState>)>,
    pub cfg_name: String,
    /// Configuration being edited.
    pub cfg_edit: Option<Box<ConfigEdit>>,
    pub error: Option<String>,
    pub busy: bool,
    /// Server choice being edited (applied with a confirm).
    pub ssh: bool,
    pub ssh_host: String,
    pub ssh_command: String,
    pub server_error: Option<String>,
    /// Browse roots on the server (`None` until loaded).
    pub roots: Option<BrowseConfig>,
    pub root_input: String,
    pub roots_error: Option<String>,
    pub roots_busy: bool,
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
    QuickOpen(QuickForm),
    DefPicker(DefPickerForm),
    DeleteEntry(DeleteEntryForm),
    CloseDirty(CloseDirtyForm),
}

/// The configuration editor of the settings dialog (plain fields; the user will redesign it).
pub struct ConfigEdit {
    pub base: ConfigurationInfo,
    pub name: String,
    pub instructions: text_editor::Content,
    /// Skill folders (server paths), one per row.
    pub skills: Vec<String>,
    pub mcp: Vec<McpRow>,
}

impl ConfigEdit {
    fn new(c: &ConfigurationInfo) -> Self {
        ConfigEdit {
            base: c.clone(),
            name: c.name.clone(),
            instructions: text_editor::Content::with_text(&c.instructions),
            skills: c.skills.clone(),
            mcp: c
                .mcp_servers
                .iter()
                .map(|m| McpRow {
                    name: m.name.clone(),
                    command: m.command.clone(),
                    args: m.args.join(" "),
                    env: m
                        .env
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                })
                .collect(),
        }
    }

    /// The edited configuration (`args` and `env` are split on whitespace; `env` items are
    /// `KEY=VALUE`).
    pub fn to_info(&self) -> ConfigurationInfo {
        let text = self.instructions.text();
        ConfigurationInfo {
            name: self.name.trim().to_owned(),
            instructions: text.trim_end_matches('\n').to_owned(),
            skills: self.skills.clone(),
            mcp_servers: self
                .mcp
                .iter()
                .filter(|m| !(m.name.trim().is_empty() && m.command.trim().is_empty()))
                .map(|m| McpServerInfo {
                    name: m.name.trim().to_owned(),
                    command: m.command.trim().to_owned(),
                    args: m.args.split_whitespace().map(str::to_owned).collect(),
                    env: m
                        .env
                        .split_whitespace()
                        .map(|kv| match kv.split_once('=') {
                            Some((k, v)) => (k.to_owned(), v.to_owned()),
                            None => (kv.to_owned(), String::new()),
                        })
                        .collect(),
                })
                .collect(),
            ..self.base.clone()
        }
    }
}

/// One MCP server row of the configuration editor.
#[derive(Debug, Clone, Default)]
pub struct McpRow {
    pub name: String,
    pub command: String,
    pub args: String,
    pub env: String,
}

/// Which field of an MCP row changed.
#[derive(Debug, Clone, Copy)]
pub enum McpField {
    Name,
    Command,
    Args,
    Env,
}

/// An entry of a pick list: a key (slug / id) and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub key: String,
    pub label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// An account / configuration change requested from the settings dialog.
#[derive(Debug, Clone)]
pub enum SettingsOp {
    AccountRenamed,
    AccountDeleted,
    ConfigCreated,
    ConfigSaved,
    ConfigDeleted(String),
}

type Lists = (
    Vec<ProviderInfo>,
    Vec<AccountInfo>,
    Vec<ConfigurationInfo>,
    Vec<SessionInfo>,
);

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
    /// Show / hide the folder browser of the new-session dialog.
    NewBrowse,
    BrowseRoots(Result<Vec<String>, ProtoError>),
    /// List a folder in the browser (`""` = the first root).
    BrowseGo(String),
    BrowseListed(u64, Result<BrowseListing, ProtoError>),
    NewProvider(String),
    /// Account slug; `""` = the CLI's default login.
    NewAccount(String),
    NewConfiguration(String),
    NewTitle(String),
    NewBase(String),
    NewSubmit,
    NewCreated(Result<SessionInfo, ProtoError>),
    CloseModal,

    /// Sidebar: show the sessions of this configuration.
    ConfigurationPicked(String),
    OpenSettings,
    AcctProvider(String),
    AcctName(String),
    /// Create the account and open its login command in a terminal.
    AcctAdd,
    AcctCreated(Result<AccountInfo, ProtoError>),
    AcctLogin(String, String),
    LoginOpened(Result<SessionInfo, ProtoError>),
    AcctStatus(String, String),
    AcctStatusDone(String, String, Result<AccountState, ProtoError>),
    AcctStartRename(String, String),
    AcctRenameInput(String),
    AcctRenameCommit,
    AcctDelete(String, String),
    CfgName(String),
    CfgAdd,
    CfgEdit(String),
    CfgEditName(String),
    CfgInstructions(text_editor::Action),
    CfgSkill(usize, String),
    CfgSkillAdd,
    CfgSkillRemove(usize),
    CfgMcp(usize, McpField, String),
    CfgMcpAdd,
    CfgMcpRemove(usize),
    CfgSave,
    CfgCancel,
    CfgDelete(String),
    SettingsDone(SettingsOp, Result<(), ProtoError>),
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

    /// Detail area of a session.
    Detail(String, DetailMsg),
    ToggleOpenItems,
    CloseOpenItems,
    QuickOpen,
    QuickInput(String),
    QuickResults(u64, Result<Vec<FileMatch>, ProtoError>),
    QuickMove(i32),
    QuickSubmit,
    QuickPick(String),
    FindInFiles,
    DefPick(usize),
    DeleteEntryConfirm,
    CloseDirtySave,
    CloseDirtyDiscard,
    /// Drop expired transient messages.
    Expire,
    RootsLoaded(Result<BrowseConfig, ProtoError>),
    RootInput(String),
    RootAdd,
    RootRemove(usize),
    RootsSaved(Result<BrowseConfig, ProtoError>),

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
    /// Provider, account, configuration and session lists arrived at least once.
    pub loaded: bool,
    pub providers: Vec<ProviderInfo>,
    pub accounts: Vec<AccountInfo>,
    pub configs: Vec<ConfigurationInfo>,
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
    /// The "open items" list of the detail header is open.
    pub open_items: bool,
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
            providers: Vec::new(),
            accounts: Vec::new(),
            configs: Vec::new(),
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
            open_items: false,
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
            l.detail.watched = false;
            l.detail.stale = true;
        }
    }

    /// Drop the client (like closing the app): the server keeps every session running.
    pub fn disconnect(&mut self, reason: String) {
        self.link = None;
        self.conn = Conn::Disconnected(reason);
        for l in &mut self.sessions {
            l.attached = false;
            l.detail.watched = false;
            l.detail.stale = true;
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
    pub(crate) fn online(&self) -> Option<Client> {
        self.conn.is_connected().then(|| self.client()).flatten()
    }

    /// Width shared by the terminal and the detail pane (the divider's range): the window
    /// minus the sidebar, the divider and the side panel.
    pub fn split_avail(&self) -> f32 {
        let side = if self.ui.detail_collapsed || self.selected.is_none() {
            0.0
        } else {
            crate::detail::SIDE_W + 1.0
        };
        (self.window_size.width - SIDEBAR_W - 1.0 - DIVIDER_W - side).max(1.0)
    }

    pub fn connected(&self) -> bool {
        self.conn.is_connected() && self.link.is_some()
    }

    pub fn live(&self, id: &str) -> Option<&Live> {
        self.sessions.iter().find(|l| l.id() == id)
    }

    pub(crate) fn live_mut(&mut self, id: &str) -> Option<&mut Live> {
        self.sessions.iter_mut().find(|l| l.id() == id)
    }

    pub fn selected_live(&self) -> Option<&Live> {
        self.selected.as_deref().and_then(|id| self.live(id))
    }

    /// Configuration whose sessions the sidebar shows.
    pub fn current_configuration(&self) -> String {
        self.ui
            .configuration
            .clone()
            .filter(|p| self.configs.is_empty() || self.configs.iter().any(|x| &x.slug == p))
            .or_else(|| self.configs.first().map(|p| p.slug.clone()))
            .unwrap_or_else(|| DEFAULT_CONFIGURATION.to_owned())
    }

    /// Whether the sidebar lists a session: sessions of the current configuration, and login
    /// terminals always.
    pub fn in_filter(&self, info: &SessionInfo) -> bool {
        info.kind == SessionKind::Login || info.configuration == self.current_configuration()
    }

    /// Sessions using a configuration: (total, running).
    pub fn sessions_in_configuration(&self, slug: &str) -> (usize, usize) {
        let of = || {
            self.sessions
                .iter()
                .filter(move |l| l.info.kind == SessionKind::Agent && l.info.configuration == slug)
        };
        (of().count(), of().filter(|l| l.state.is_alive()).count())
    }

    /// A provider by id.
    pub fn provider(&self, id: &str) -> Option<&ProviderInfo> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Display name of a provider (the id until the list arrived).
    pub fn provider_name(&self, id: &str) -> String {
        self.provider(id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| id.to_owned())
    }

    /// Display name of an account.
    pub fn account_name(&self, provider: &str, slug: &str) -> String {
        self.accounts
            .iter()
            .find(|a| a.provider == provider && a.slug == slug)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| slug.to_owned())
    }

    /// The default account for a new session of `provider`: its first account.
    fn default_account(&self, provider: &str) -> Option<String> {
        self.provider(provider)
            .filter(|p| p.caps.accounts)
            .and_then(|_| self.accounts.iter().find(|a| a.provider == provider))
            .map(|a| a.slug.clone())
    }

    pub(crate) fn save_ui(&mut self) {
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
                let p = c.providers(false).await?;
                let a = c.accounts().await?;
                let k = c.configurations().await?;
                let s = c.sessions().await?;
                Ok((p, a, k, s))
            },
            move |r| Message::Lists(generation, r),
        )
    }

    /// Merge the server's session list: keep terminal models and web views of known sessions.
    fn apply_lists(&mut self, lists: Lists) -> Task<Message> {
        let (providers, accounts, configs, list) = lists;
        self.providers = providers;
        self.accounts = accounts;
        self.configs = configs;
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
                    let saved = self.ui.detail.get(&info.id).cloned();
                    next.push(Live::new(info, url, &self.term_config, saved.as_ref()));
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
                "lists loaded: {} provider(s), {} account(s), {} configuration(s), {} session(s)",
                self.providers.len(),
                self.accounts.len(),
                self.configs.len(),
                self.sessions.len()
            ));
        }
        if self
            .selected
            .as_deref()
            .is_some_and(|id| self.live(id).is_none())
        {
            self.selected = self
                .sessions
                .iter()
                .find(|l| self.in_filter(&l.info))
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
                // Watches and listings start over on the new connection (the gap is not
                // replayed): refresh the shown detail area.
                for l in &mut self.sessions {
                    l.detail.watched = false;
                    l.detail.stale = true;
                }
                // The PTY size follows the most recent client: re-send ours.
                if let Some(l) = self.selected_live()
                    && let Some((c, r)) = l.size
                {
                    self.send_input(Input::Resize(l.info.id.clone(), c, r));
                }
                Task::batch([self.refresh(), self.ensure_attached(), self.sync_detail()])
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
            Notify::SessionsChanged | Notify::AccountsChanged | Notify::ConfigurationsChanged => {
                self.refresh()
            }
            Notify::FsChanged {
                session,
                paths,
                rescan,
            } => self.on_fs_notify(&session, Some((paths, rescan))),
            Notify::GitChanged { session } => self.on_fs_notify(&session, None),
            // Routed to the `SearchStream`s by the client; late ones are dropped.
            Notify::SearchHits { .. } | Notify::SearchDone { .. } => Task::none(),
        }
    }

    // ------------------------------------------------------------------ web views

    fn detail_visible(&self) -> bool {
        !self.ui.detail_collapsed
    }

    /// Create the selected session's web view if needed and hide every other one (and the
    /// selected one while another detail item is shown).
    pub(crate) fn sync_web(&mut self) {
        let show = self
            .detail_visible()
            .then(|| self.selected.clone())
            .flatten()
            .filter(|id| {
                self.live(id)
                    .is_some_and(|l| l.detail.active == ItemKey::Web)
            });
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
        self.open_items = false;
        let attach = Task::batch([self.ensure_attached(), self.sync_detail()]);
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
                    Ok(lists) => self.apply_lists(lists),
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
                        self.ui.detail.remove(&id);
                        if self.selected.as_deref() == Some(id.as_str()) {
                            self.selected = self
                                .sessions
                                .iter()
                                .find(|l| self.in_filter(&l.info))
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
                let provider = self
                    .providers
                    .iter()
                    .find(|p| p.detection.usable)
                    .map(|p| p.id.clone())
                    .unwrap_or_default();
                self.modal = Some(Modal::NewSession(NewForm {
                    repo: self.ui.last_repo.clone().unwrap_or_default(),
                    account: self.default_account(&provider),
                    provider,
                    configuration: self.current_configuration(),
                    title: String::new(),
                    base_ref: String::new(),
                    error: None,
                    busy: false,
                    browser: None,
                }));
                self.open_items = false;
                operation::focus(NEW_REPO_ID)
            }
            Message::NewRepo(v) => self.with_new(|f| f.repo = v),
            Message::NewBrowse => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::NewSession(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.browser.take().is_some() {
                    return Task::none();
                }
                f.browser = Some(FolderBrowser::default());
                // Start at the typed folder when it is browsable, else at the first root.
                let start = f.repo.trim().trim_matches('"').to_owned();
                let roots = Task::perform(
                    {
                        let c = c.clone();
                        async move { c.browse_roots().await }
                    },
                    Message::BrowseRoots,
                );
                Task::batch([roots, self.browse_go(start)])
            }
            Message::BrowseRoots(r) => {
                if let Some(Modal::NewSession(NewForm {
                    browser: Some(b), ..
                })) = &mut self.modal
                {
                    match r {
                        Ok(roots) => b.roots = roots,
                        Err(e) => b.error = Some(i18n::proto_error(&e)),
                    }
                }
                Task::none()
            }
            Message::BrowseGo(dir) => self.browse_go(dir),
            Message::BrowseListed(seq, r) => {
                let Some(Modal::NewSession(f)) = &mut self.modal else {
                    return Task::none();
                };
                let Some(b) = &mut f.browser else {
                    return Task::none();
                };
                if seq != b.seq {
                    return Task::none();
                }
                b.loading = false;
                match r {
                    Ok(l) => {
                        crate::script::log(&format!(
                            "browse {} (git repo {}, {} folder(s))",
                            l.path,
                            l.is_git_repo,
                            l.entries.len()
                        ));
                        // Browsing into a repository picks it.
                        if l.is_git_repo {
                            f.repo = l.path.clone();
                            f.error = None;
                        }
                        b.error = None;
                        b.listing = Some(l);
                        Task::none()
                    }
                    // A typed path outside the roots: start at the first root instead.
                    Err(_) if b.listing.is_none() && !b.dir.is_empty() => {
                        self.browse_go(String::new())
                    }
                    Err(e) => {
                        b.error = Some(i18n::proto_error(&e));
                        Task::none()
                    }
                }
            }
            Message::NewProvider(p) => {
                let account = self.default_account(&p);
                self.with_new(|f| {
                    f.provider = p;
                    f.account = account;
                })
            }
            Message::NewAccount(a) => self.with_new(|f| f.account = (!a.is_empty()).then_some(a)),
            Message::NewConfiguration(v) => self.with_new(|f| f.configuration = v),
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
                    Some(Modal::DeleteEntry(f)) => f.busy,
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

            Message::ConfigurationPicked(p) => {
                if self.configs.iter().any(|x| x.slug == p) {
                    self.ui.configuration = Some(p.clone());
                    let shown = self
                        .selected_live()
                        .is_some_and(|l| self.in_filter(&l.info));
                    if !shown {
                        self.selected = self
                            .sessions
                            .iter()
                            .find(|l| self.in_filter(&l.info))
                            .map(|l| l.info.id.clone());
                    }
                    self.save_ui();
                }
                self.after_select()
            }
            Message::OpenSettings => {
                self.open_items = false;
                let mut form = SettingsForm::new(&self.ui.server);
                form.acct_provider = self
                    .providers
                    .iter()
                    .find(|p| p.caps.accounts)
                    .map(|p| p.id.clone())
                    .unwrap_or_default();
                self.modal = Some(Modal::Settings(form));
                let roots = match self.online() {
                    Some(c) => {
                        Task::perform(async move { c.browse_config().await }, Message::RootsLoaded)
                    }
                    None => Task::none(),
                };
                Task::batch([operation::focus(CONFIG_NAME_ID), roots])
            }
            Message::RootsLoaded(r) => self.with_settings(|f| {
                f.roots_busy = false;
                match r {
                    Ok(c) => {
                        crate::script::log(&format!("browse roots: {:?}", c.roots));
                        f.roots = Some(c);
                    }
                    Err(e) => f.roots_error = Some(i18n::proto_error(&e)),
                }
            }),
            Message::RootInput(v) => self.with_settings(|f| f.root_input = v),
            Message::RootAdd | Message::RootRemove(_) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                let mut roots = f
                    .roots
                    .as_ref()
                    .map(|c| c.roots.clone())
                    .unwrap_or_default();
                match message {
                    Message::RootRemove(i) if i < roots.len() => {
                        roots.remove(i);
                    }
                    Message::RootAdd => {
                        let v = f.root_input.trim().trim_matches('"').to_owned();
                        if v.is_empty() {
                            return Task::none();
                        }
                        roots.push(v);
                    }
                    _ => return Task::none(),
                }
                f.roots_busy = true;
                f.roots_error = None;
                Task::perform(
                    async move { c.browse_set_roots(roots).await },
                    Message::RootsSaved,
                )
            }
            Message::RootsSaved(r) => self.with_settings(|f| {
                f.roots_busy = false;
                match r {
                    Ok(c) => {
                        crate::script::log(&format!("browse roots saved: {:?}", c.roots));
                        f.roots = Some(c);
                        f.root_input.clear();
                        f.roots_error = None;
                    }
                    Err(e) => {
                        crate::script::log(&format!("browse roots refused: {e}"));
                        f.roots_error = Some(i18n::proto_error(&e));
                    }
                }
            }),
            Message::AcctProvider(p) => self.with_settings(|f| f.acct_provider = p),
            Message::AcctName(v) => self.with_settings(|f| f.acct_name = v),
            Message::AcctAdd => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.busy || f.acct_provider.is_empty() {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let (provider, name) = (f.acct_provider.clone(), f.acct_name.trim().to_owned());
                Task::perform(
                    async move { c.create_account(&provider, &name).await },
                    Message::AcctCreated,
                )
            }
            Message::AcctCreated(r) => {
                crate::script::log(&format!("account created: {r:?}"));
                match r {
                    Ok(a) => {
                        if let Some(Modal::Settings(f)) = &mut self.modal {
                            f.acct_name.clear();
                        }
                        let login = self.provider(&a.provider).is_some_and(|p| p.caps.login);
                        let refresh = self.refresh();
                        if login {
                            Task::batch([
                                refresh,
                                self.update(Message::AcctLogin(a.provider, a.slug)),
                            ])
                        } else {
                            let _ = self.with_settings(|f| f.busy = false);
                            refresh
                        }
                    }
                    Err(e) => self.with_settings(|f| {
                        f.busy = false;
                        f.error = Some(i18n::proto_error(&e));
                    }),
                }
            }
            Message::AcctLogin(provider, slug) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let (cols, rows) = self
                    .selected_live()
                    .and_then(|l| l.size)
                    .unwrap_or(INITIAL_GRID);
                let _ = self.with_settings(|f| f.busy = true);
                Task::perform(
                    async move { c.login_account(&provider, &slug, cols, rows).await },
                    Message::LoginOpened,
                )
            }
            Message::LoginOpened(r) => match r {
                Ok(info) => {
                    crate::script::log(&format!(
                        "login session {} {:?} for {}/{}",
                        info.id,
                        info.title,
                        info.provider,
                        info.account.as_deref().unwrap_or("-")
                    ));
                    // Shown like a new session: the settings dialog closes.
                    self.on_created(Ok(info))
                }
                Err(e) => {
                    crate::script::log(&format!("login error: {e}"));
                    self.with_settings(|f| {
                        f.busy = false;
                        f.error = Some(i18n::proto_error(&e));
                    })
                }
            },
            Message::AcctStatus(provider, slug) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let key = (provider.clone(), slug.clone());
                let _ = self.with_settings(|f| {
                    f.acct_status.retain(|(k, _)| k != &key);
                    f.acct_status.push((key, None));
                });
                Task::perform(
                    {
                        let (p, s) = (provider.clone(), slug.clone());
                        async move { c.account_status(&p, &s).await }
                    },
                    move |r| Message::AcctStatusDone(provider.clone(), slug.clone(), r),
                )
            }
            Message::AcctStatusDone(provider, slug, r) => {
                crate::script::log(&format!("account status {provider}/{slug}: {r:?}"));
                self.with_settings(|f| {
                    let key = (provider, slug);
                    f.acct_status.retain(|(k, _)| k != &key);
                    f.acct_status
                        .push((key, Some(r.unwrap_or(AccountState::Unknown))));
                })
            }
            Message::AcctStartRename(provider, slug) => {
                let name = self.account_name(&provider, &slug);
                self.with_settings(|f| {
                    f.acct_renaming = Some((provider, slug, name));
                    f.error = None;
                })
            }
            Message::AcctRenameInput(v) => self.with_settings(|f| {
                if let Some((_, _, n)) = &mut f.acct_renaming {
                    *n = v;
                }
            }),
            Message::AcctRenameCommit => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                let Some((provider, slug, name)) = f.acct_renaming.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { c.rename_account(&provider, &slug, &name).await.map(drop) },
                    |r| Message::SettingsDone(SettingsOp::AccountRenamed, r),
                )
            }
            Message::AcctDelete(provider, slug) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                Task::perform(
                    async move { c.delete_account(&provider, &slug, true).await },
                    |r| Message::SettingsDone(SettingsOp::AccountDeleted, r),
                )
            }
            Message::CfgName(v) => self.with_settings(|f| f.cfg_name = v),
            Message::CfgAdd => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                let name = f.cfg_name.trim().to_owned();
                f.busy = true;
                Task::perform(
                    async move { c.create_configuration(&name).await.map(drop) },
                    |r| Message::SettingsDone(SettingsOp::ConfigCreated, r),
                )
            }
            Message::CfgEdit(slug) => {
                let Some(cfg) = self.configs.iter().find(|c| c.slug == slug).cloned() else {
                    return Task::none();
                };
                self.with_settings(|f| {
                    f.cfg_edit = Some(Box::new(ConfigEdit::new(&cfg)));
                    f.error = None;
                })
            }
            Message::CfgEditName(v) => self.with_cfg(|e| e.name = v),
            Message::CfgInstructions(a) => self.with_cfg(|e| e.instructions.perform(a)),
            Message::CfgSkill(i, v) => self.with_cfg(|e| {
                if let Some(s) = e.skills.get_mut(i) {
                    *s = v;
                }
            }),
            Message::CfgSkillAdd => self.with_cfg(|e| e.skills.push(String::new())),
            Message::CfgSkillRemove(i) => self.with_cfg(|e| {
                if i < e.skills.len() {
                    e.skills.remove(i);
                }
            }),
            Message::CfgMcp(i, field, v) => self.with_cfg(|e| {
                if let Some(m) = e.mcp.get_mut(i) {
                    match field {
                        McpField::Name => m.name = v,
                        McpField::Command => m.command = v,
                        McpField::Args => m.args = v,
                        McpField::Env => m.env = v,
                    }
                }
            }),
            Message::CfgMcpAdd => self.with_cfg(|e| e.mcp.push(McpRow::default())),
            Message::CfgMcpRemove(i) => self.with_cfg(|e| {
                if i < e.mcp.len() {
                    e.mcp.remove(i);
                }
            }),
            Message::CfgCancel => self.with_settings(|f| f.cfg_edit = None),
            Message::CfgSave => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                let Some(e) = &f.cfg_edit else {
                    return Task::none();
                };
                let info = e.to_info();
                f.busy = true;
                Task::perform(
                    async move { c.update_configuration(info).await.map(drop) },
                    |r| Message::SettingsDone(SettingsOp::ConfigSaved, r),
                )
            }
            Message::CfgDelete(slug) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let op = SettingsOp::ConfigDeleted(slug.clone());
                Task::perform(
                    async move { c.delete_configuration(&slug).await },
                    move |r| Message::SettingsDone(op.clone(), r),
                )
            }
            Message::SettingsDone(op, r) => {
                crate::script::log(&format!("settings {op:?}: {r:?}"));
                let ok = r.is_ok();
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    f.busy = false;
                    match r {
                        Ok(()) => {
                            f.error = None;
                            match &op {
                                SettingsOp::ConfigCreated => f.cfg_name.clear(),
                                SettingsOp::ConfigSaved => f.cfg_edit = None,
                                SettingsOp::AccountRenamed => f.acct_renaming = None,
                                _ => {}
                            }
                        }
                        Err(e) => f.error = Some(i18n::proto_error(&e)),
                    }
                }
                if ok
                    && let SettingsOp::ConfigDeleted(slug) = &op
                    && self.ui.configuration.as_deref() == Some(slug.as_str())
                {
                    self.ui.configuration = None;
                    self.save_ui();
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
                self.open_items = false;
                self.save_ui();
                self.sync_web();
                self.sync_detail()
            }

            Message::Detail(sid, m) => self.on_detail(sid, m),
            Message::ToggleOpenItems => {
                self.open_items = !self.open_items;
                Task::none()
            }
            Message::CloseOpenItems => {
                self.open_items = false;
                Task::none()
            }
            Message::QuickOpen => {
                if self.modal.is_some() {
                    return Task::none();
                }
                self.open_quick()
            }
            Message::QuickInput(q) => self.quick_query(q),
            Message::QuickResults(seq, r) => {
                if let Some(Modal::QuickOpen(f)) = &mut self.modal
                    && f.seq == seq
                {
                    match r {
                        Ok(m) => {
                            f.results = m;
                            f.error = None;
                        }
                        Err(e) => f.error = Some(i18n::proto_error(&e)),
                    }
                    f.selected = 0;
                }
                Task::none()
            }
            Message::QuickMove(d) => {
                if let Some(Modal::QuickOpen(f)) = &mut self.modal
                    && !f.results.is_empty()
                {
                    let n = f.results.len() as i64;
                    f.selected = (f.selected as i64 + d as i64).rem_euclid(n) as usize;
                }
                Task::none()
            }
            Message::QuickSubmit => {
                let path = match &self.modal {
                    Some(Modal::QuickOpen(f)) => f.results.get(f.selected).map(|m| m.path.clone()),
                    _ => None,
                };
                match path {
                    Some(p) => self.update(Message::QuickPick(p)),
                    None => Task::none(),
                }
            }
            Message::QuickPick(path) => {
                self.modal = None;
                let Some(sid) = self.selected.clone() else {
                    return Task::none();
                };
                crate::script::log(&format!("quick open {path}"));
                if self.ui.detail_collapsed {
                    self.ui.detail_collapsed = false;
                    self.save_ui();
                }
                let t = self.open_file_at(&sid, &path, 0, 0);
                Task::batch([t, self.sync_detail()])
            }
            Message::FindInFiles => self.open_search(),
            Message::DefPick(i) => {
                let Some(Modal::DefPicker(f)) = self.modal.take() else {
                    return Task::none();
                };
                match f.targets.get(i) {
                    Some(t) => self.open_file_at(&f.session, &t.path, t.line, t.col),
                    None => Task::none(),
                }
            }
            Message::DeleteEntryConfirm => self.delete_entry(),
            Message::CloseDirtySave | Message::CloseDirtyDiscard => {
                let Some(Modal::CloseDirty(f)) = self.modal.take() else {
                    return Task::none();
                };
                if matches!(message, Message::CloseDirtySave) {
                    return self.save_file(&f.session, &f.path, None, true);
                }
                if let Some(file) = self
                    .detail_mut(&f.session)
                    .and_then(|d| d.file_mut(&f.path))
                    && let Some(v) = &mut file.view
                {
                    let _ = v.update(ide_views::CodeMessage::Revert);
                }
                self.on_detail(f.session, DetailMsg::Close(ItemKey::File(f.path)))
            }
            Message::Expire => {
                let now = std::time::Instant::now();
                for l in &mut self.sessions {
                    if l.detail
                        .toast
                        .as_ref()
                        .is_some_and(|(_, t)| now.duration_since(*t) > crate::detail::TOAST_FOR)
                    {
                        l.detail.toast = None;
                    }
                }
                Task::none()
            }
            Message::DividerPressed => {
                self.dragging = true;
                Task::none()
            }
            Message::DragMoved(p) => {
                if self.dragging {
                    let avail = self.split_avail();
                    self.ui.split_ratio =
                        ((p.x - SIDEBAR_W - 1.0) / avail).clamp(SPLIT_RANGE.0, SPLIT_RANGE.1);
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

    fn with_cfg(&mut self, f: impl FnOnce(&mut ConfigEdit)) -> Task<Message> {
        if let Some(Modal::Settings(SettingsForm {
            cfg_edit: Some(e), ..
        })) = &mut self.modal
        {
            f(e);
        }
        Task::none()
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

    /// List `dir` in the new-session dialog's folder browser.
    fn browse_go(&mut self, dir: String) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(Modal::NewSession(NewForm {
            browser: Some(b), ..
        })) = &mut self.modal
        else {
            return Task::none();
        };
        b.seq += 1;
        b.loading = true;
        b.dir = dir.clone();
        let seq = b.seq;
        Task::perform(async move { c.browse_list(&dir).await }, move |r| {
            Message::BrowseListed(seq, r)
        })
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
            provider: f.provider.clone(),
            account: f.account.clone(),
            configuration: f.configuration.clone(),
            repo_path: repo.clone(),
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
        let configuration = (info.kind == SessionKind::Agent).then(|| info.configuration.clone());
        match self.live_mut(&id) {
            Some(l) => {
                l.state = info.state.into();
                l.info = info;
            }
            None => {
                let live = Live::new(info, DEFAULT_URL.into(), &self.term_config, None);
                self.sessions.push(live);
            }
        }
        if let Some(c) = configuration {
            self.ui.configuration = Some(c);
        }
        self.selected = Some(id);
        self.modal = None;
        self.save_ui();
        self.after_select()
    }

    fn on_web(&mut self, ev: WebViewEvent) {
        let show = self
            .detail_visible()
            .then(|| self.selected.clone())
            .flatten()
            .filter(|id| {
                self.live(id)
                    .is_some_and(|l| l.detail.active == ItemKey::Web)
            });
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
        if matches!(self.modal, Some(Modal::QuickOpen(_))) {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
                    ..
                }) => Some(Message::QuickMove(1)),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
                    ..
                }) => Some(Message::QuickMove(-1)),
                _ => None,
            }));
        } else if self.open_items {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseOpenItems),
                _ => None,
            }));
        }
        // Window-wide shortcuts: Ctrl+P (quick open), Ctrl+Shift+F (find in files). The
        // physical key is used so they also work with the Korean input mode on.
        subs.push(event::listen_with(|e, _, _| match e {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if modifiers.command() && !modifiers.alt() => {
                match (key.to_latin(physical_key), modifiers.shift()) {
                    (Some('p'), false) => Some(Message::QuickOpen),
                    (Some('f'), true) => Some(Message::FindInFiles),
                    _ => None,
                }
            }
            _ => None,
        }));
        if self.sessions.iter().any(|l| l.detail.toast.is_some()) {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Message::Expire));
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
