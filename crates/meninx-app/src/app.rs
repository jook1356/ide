//! App state, messages and update logic.
//!
//! The app is a client of `meninx-server`: providers, accounts, configurations, sessions,
//! worktrees and terminals live on the
//! server and are reached through [`meninx_client::Client`] (RPC + notifications + terminal
//! streams). Only UI state (`ui.json`: selection, split, language, web URLs, ...) and the CEF web
//! views are local.

use crate::notices::{Notice, NoticeKey, TaskState, Text};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::widget::{operation, text_editor};
use iced::{Event, Point, Size, Subscription, Task, event, keyboard, mouse, window};
use meninx_client::{
    AccountInfo, AccountState, AgentActivity, BrowseListing, Client, ClientEvent, ClientOptions,
    ConfigurationInfo, DefTarget, FileMatch, LocalChild, McpServerInfo, NewSession, Notify,
    ProtoError, ProviderInfo, RepoFailureInfo, RepoInfo, SessionInfo, SessionKind, SessionState,
    Ssh, Switchable, Transport, Welcome,
};
use meninx_i18n::LanguagePref;
use meninx_term::{TermConfig, TermModel, ViewEvent};
use meninx_web::{WebEvent, WebRuntime, WebView, WebViewEvent};
use tokio::sync::mpsc;

use crate::detail::{Detail, DetailMsg, ItemKey};
use crate::i18n::{self, tr};
use crate::script::Script;
use crate::session_sort::{SortCriterion, SortDir, SortKey};
use crate::ui_state::{CloseAction, DetailUi, FontSlot, Page, SPLIT_RANGE, ServerPref, UiState};

/// Width of the navigation rail in logical pixels, its right border included.
pub const RAIL_W: f32 = 64.0;
/// Default width of the session / profile list column in logical pixels.
pub const SIDEBAR_W: f32 = 264.0;
/// Narrowest and widest session / profile list.
const LIST_MIN: f32 = 200.0;
const LIST_MAX: f32 = 600.0;
/// Width of the draggable divider between centre and detail.
pub const DIVIDER_W: f32 = 5.0;
/// Narrowest side panel (Desk's `MIN_PANEL`), and what it leaves at least to the terminal and
/// the detail pane when widened.
const SIDE_MIN: f32 = 220.0;
const CENTRE_MIN: f32 = 320.0;
const DETAIL_MIN: f32 = 240.0;
/// URL of a new session's web tab.
pub const DEFAULT_URL: &str = "about:blank";
/// Configuration slug a fresh server has (used before the configuration list arrived).
pub const DEFAULT_CONFIGURATION: &str = "default";
/// Grid of a terminal before its widget was laid out.
const INITIAL_GRID: (u16, u16) = (120, 30);

pub const TERM_ID: &str = "meninx-term";
pub const NEW_TITLE_ID: &str = "meninx-new-title";
pub const URL_ID: &str = "meninx-url";
pub const CONFIG_NAME_ID: &str = "meninx-config-name";
pub const SSH_HOST_ID: &str = "meninx-ssh-host";
pub const PICK_PATH_ID: &str = "meninx-pick-path";
/// The folder list of the repos page's folder picker.
pub const PICK_LIST_ID: &str = "meninx-pick-list";

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

/// Connection to the server as shown by the rail's server indicator.
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
    /// The session's user terminals (`uterm.rs`).
    pub uterms: Vec<crate::uterm::UTermView>,
    /// Browser tabs beyond the session's web page (`browser.rs`).
    pub browsers: Vec<crate::browser::Browser>,
    /// The input box under the terminal: draft and sent history (`composer.rs`).
    pub composer: crate::composer::Composer,
    /// Files, diffs, tree, changes and search of the session.
    pub detail: Detail,
}

impl Live {
    fn new(info: SessionInfo, url: String, config: &TermConfig, detail: Option<&DetailUi>) -> Self {
        Self {
            detail: Detail::new(detail, info.repos.first().map(|r| r.repo.as_str())),
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
            uterms: Vec::new(),
            browsers: Vec::new(),
            composer: Default::default(),
        }
    }

    pub fn id(&self) -> &str {
        &self.info.id
    }
}

/// The new-session dialog (Hermes Desk's `NewSessionModal`).
pub struct NewForm {
    /// Provider id.
    pub provider: String,
    /// Account slug (`None` = the CLI's default login).
    pub account: Option<String>,
    /// Configuration slug.
    pub configuration: String,
    pub title: String,
    /// Desk's "워크트리 사용": a session folder with one worktree per checked repo; off = work
    /// in one registered repo's own checkout.
    pub use_worktrees: bool,
    /// Checked repos (ids; pre-checked: the configuration's default repos).
    pub repos: Vec<String>,
    /// The repo of an original-repo session.
    pub original_repo: Option<String>,
    /// Desk's "기준 브랜치 최신화" (on each time the dialog opens).
    pub refresh_base: bool,
    /// Base branch picked per repo id (origin's branch); missing = the repo's default.
    pub bases: std::collections::BTreeMap<String, String>,
    /// The repo list's search box (shown from `new_session_repos::SEARCH_MIN_REPOS` repos).
    pub repo_filter: String,
    pub error: Option<String>,
    pub busy: bool,
}

/// The repos page's folder picker: any folder the server's OS user can read
/// What a folder picked with [`PickRootForm`] is for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PickPurpose {
    /// A repository to register (the repos page).
    #[default]
    Repo,
    /// The start folder of a 공용 terminal.
    Terminal,
    /// A skill folder to copy into profile `.0`.
    Skill(String),
}

/// (`browse.pick_list`), to register as a repository. Server-side, so it works over SSH too.
#[derive(Default)]
pub struct PickRootForm {
    pub listing: Option<BrowseListing>,
    pub loading: bool,
    pub error: Option<String>,
    /// Incremented per request; older answers are dropped.
    pub seq: u64,
    /// The folder last asked for (`None` = the start list: drives / `/` and the home dir).
    pub dir: Option<String>,
    /// Highlighted entry of the listing (click, arrow keys).
    pub selected: Option<usize>,
    /// What the picked folder is for.
    pub purpose: PickPurpose,
    /// The path input above the list: the listed folder, or what the user typed / pasted
    /// (Enter opens it).
    pub path: String,
}

impl PickRootForm {
    /// The folder "Select this folder" registers: the highlighted entry, else the listed
    /// folder. `None` for the start list without a highlight and for filesystem roots.
    pub fn target(&self) -> Option<&str> {
        let l = self.listing.as_ref()?;
        let p = match self.selected.and_then(|i| l.entries.get(i)) {
            Some(e) => e.path.as_str(),
            None if l.path.is_empty() || l.parent.is_none() => return None,
            None => l.path.as_str(),
        };
        (!is_fs_root(p)).then_some(p)
    }
}

/// `C:\`, `C:`, `/`, `\\server\share` (a whole drive or share; never a repository).
pub fn is_fs_root(p: &str) -> bool {
    let t = p.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        return true;
    }
    let b = t.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return true;
    }
    // `\\server\share`: two parts after the leading separators.
    if let Some(rest) = t.strip_prefix(r"\\").or_else(|| t.strip_prefix("//")) {
        return rest.split(['\\', '/']).filter(|s| !s.is_empty()).count() <= 2;
    }
    false
}

/// Desk's `defaultReposFor`: `saved` ids still in `registered` (registry order), or every
/// registered repo when nothing was saved.
pub fn default_repos_for(saved: Option<&Vec<String>>, registered: &[String]) -> Vec<String> {
    match saved {
        None => registered.to_vec(),
        Some(s) => registered.iter().filter(|r| s.contains(r)).cloned().collect(),
    }
}

/// Scroll the folder picker's list to a relative position (0 = top, 1 = bottom).
fn pick_scroll(y: f32) -> Task<Message> {
    iced::widget::operation::scrollable::snap_to(
        PICK_LIST_ID,
        iced::widget::operation::scrollable::RelativeOffset {
            x: None,
            y: Some(y),
        },
        iced::widget::operation::Animation::Instant,
    )
}

/// Breadcrumb of a server path: `(label, path)` per level, the drive / `/` first.
/// `C:\Users\me` -> `C:`, `Users`, `me`; `/home/me` -> `/`, `home`, `me`.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    if path.is_empty() {
        return Vec::new();
    }
    // Server OS syntax: Windows paths use `\`, Unix ones start with `/`.
    let unix = path.starts_with('/') && !path.contains('\\');
    let sep = if unix { '/' } else { '\\' };
    let (root, rest): (String, &str) = if unix {
        ("/".into(), &path[1..])
    } else if let Some(r) = path.strip_prefix(r"\\") {
        // UNC: `\\server\share\` is one level.
        let mut it = r.splitn(3, '\\');
        let server = it.next().unwrap_or_default();
        let share = it.next().unwrap_or_default();
        (
            format!(r"\\{server}\{share}\"),
            it.next().unwrap_or_default(),
        )
    } else {
        match path.split_once('\\') {
            Some((d, r)) => (format!("{d}\\"), r),
            None => (format!("{path}\\"), ""),
        }
    };
    let label = if unix {
        "/".to_owned()
    } else {
        root.trim_end_matches('\\').to_owned()
    };
    let mut out = vec![(label, root.clone())];
    let mut acc = root;
    for part in rest.split(sep).filter(|s| !s.is_empty()) {
        if !acc.ends_with(sep) {
            acc.push(sep);
        }
        acc.push_str(part);
        out.push((part.to_owned(), acc.clone()));
    }
    out
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
    /// Repo the targets' paths are relative to.
    pub repo: String,
    pub name: String,
    pub targets: Vec<DefTarget>,
}

/// Confirm deleting a file / folder from the tree.
pub struct DeleteEntryForm {
    pub session: String,
    pub repo: String,
    pub path: String,
    /// The other selected entries (Explore's multi-selection).
    pub more: Vec<String>,
    pub is_dir: bool,
    pub error: Option<String>,
    pub busy: bool,
}

/// Closing a file with unsaved edits.
/// Desk's "세션 설정" repo part: the session's repos, repos to add (attach on save) and
/// detaching one (Meninx addition; Desk cannot remove a worktree mid-session).
pub struct SessionReposForm {
    pub session: String,
    /// The session name being edited (Desk's 세션 설정 "세션 이름"); renamed on save when changed.
    pub title: String,
    /// Registered repos checked to be attached on save.
    pub add: Vec<String>,
    /// Base branch chosen per repo id (attached: the session's base, saved with
    /// `session.set_base` when changed; added: the new worktree's base).
    pub bases: std::collections::BTreeMap<String, String>,
    pub error: Option<String>,
    pub busy: bool,
}

/// Closing items of which some are files with unsaved edits.
pub struct CloseDirtyForm {
    pub session: String,
    /// The unsaved files (qualified paths).
    pub dirty: Vec<String>,
    /// Every item to close (the unsaved files included).
    pub keys: Vec<ItemKey>,
}

pub struct DeleteForm {
    pub id: String,
    pub remove_worktree: bool,
}

/// State of the accounts, configurations, repos and settings pages.
#[derive(Default)]
pub struct SettingsForm {
    /// Provider of the account to add.
    pub acct_provider: String,
    pub acct_name: String,
    /// Account being renamed: (provider, slug, new name).
    pub acct_renaming: Option<(String, String, String)>,
    /// Last status check per account (provider, slug).
    pub acct_status: Vec<((String, String), Option<AccountState>)>,
    /// Account integrations (providers that have one), and after an install how to approve it
    /// in the CLI (`None` while checking).
    pub acct_integration: Vec<((String, String), Option<meninx_client::IntegrationInfo>)>,
    pub cfg_name: String,
    /// Profiles page (Desk's agents page): the profile picked in the list beside the rail
    /// (`None` = the first) and the armed delete of the title row.
    pub profile_sel: Option<String>,
    pub profile_delete_armed: bool,
    /// The shown profile's change history (`profile_history_ui.rs`).
    pub history: crate::profile_history_ui::HistoryState,
    /// The shown profile's editor (follows `profile_sel`; `sync_profile_edit`).
    pub cfg_edit: Option<Box<ConfigEdit>>,
    pub error: Option<String>,
    pub busy: bool,
    /// Server choice being edited (applied with a confirm).
    pub ssh: bool,
    pub ssh_host: String,
    pub ssh_command: String,
    pub server_error: Option<String>,
    /// Repos page: path typed to register.
    pub repo_input: String,
    /// Repos page: the repo shown right of the list (Desk's selected repo).
    pub repo_selected: Option<String>,
    /// Repos page: "unregister" was clicked once (Desk's confirm step).
    pub repo_confirm: bool,
    pub repos_error: Option<String>,
    pub repos_busy: bool,
    /// Repos page: the selected repo's base branch editor (Desk `RepoBaseSettings`).
    pub base: Option<BaseEdit>,
}

/// Desk's 레포 page "기준 브랜치" card: a fixed branch (`FixedBaseSettings`) or a rule
/// (`BaseBranchRuleSettings`).
#[derive(Default)]
pub struct BaseEdit {
    pub repo: String,
    /// "규칙으로 자동 선택" shown (else "고정 브랜치").
    pub rule_mode: bool,
    /// Local and remote-tracking refs (`repo.branches`); `None` while loading.
    pub refs: Option<Vec<String>>,
    /// Fixed branch being edited (`None` = automatic).
    pub fixed: Option<String>,
    pub remote: String,
    pub pattern: String,
    pub matches: String,
    pub preview: Option<meninx_client::BaseRulePreview>,
    pub preview_loading: bool,
    pub preview_error: Option<String>,
    pub saved: bool,
    pub busy: bool,
}

impl BaseEdit {
    /// The editor for `r` as saved (Desk's default rule: origin, release/*).
    pub fn new(r: &RepoInfo) -> Self {
        let rule = r.base_rule.clone().unwrap_or_else(|| meninx_client::BaseRuleInfo {
            remote: "origin".into(),
            pattern: "release/*".into(),
            matches: None,
        });
        BaseEdit {
            repo: r.id.clone(),
            rule_mode: r.base_rule.is_some(),
            fixed: r.base_branch.clone(),
            remote: rule.remote,
            pattern: rule.pattern,
            matches: rule.matches.unwrap_or_default(),
            ..Default::default()
        }
    }

    /// The rule as edited.
    pub fn rule(&self) -> meninx_client::BaseRuleInfo {
        meninx_client::BaseRuleInfo {
            remote: self.remote.trim().to_owned(),
            pattern: self.pattern.clone(),
            matches: (!self.matches.is_empty()).then(|| self.matches.clone()),
        }
    }
}

/// Desk's "Git 저장소가 아니에요" question when a plain folder is registered.
pub struct InitRepoForm {
    pub path: String,
    pub error: Option<String>,
    pub busy: bool,
}

impl SettingsForm {
    fn new(pref: &ServerPref) -> Self {
        let mut f = Self::default();
        f.reset_server(pref);
        f
    }

    /// Show the saved server choice again (entering the settings page).
    fn reset_server(&mut self, pref: &ServerPref) {
        (self.ssh, self.ssh_host, self.ssh_command) = match pref {
            ServerPref::Local => (false, String::new(), String::new()),
            ServerPref::Ssh { host, command } => (true, host.clone(), command.clone()),
        };
        self.server_error = None;
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
    StopServer(StopServerForm),
    /// End an incompatible local server and start this app's (`old_server.rs`).
    RestartOldServer(StopServerForm),
    /// Confirm switching to another server.
    SwitchServer(ServerPref),
    Close(CloseForm),
    QuickOpen(QuickForm),
    DefPicker(DefPickerForm),
    DeleteEntry(DeleteEntryForm),
    CloseDirty(CloseDirtyForm),
    /// Repos page: choose a folder on the server to register as a repository.
    PickRoot(PickRootForm),
    /// The session's repos (attach / detach).
    SessionRepos(SessionReposForm),
    /// Register a plain folder: `git init` it first?
    InitRepo(InitRepoForm),
    /// Changes / History action dialogs (discard, stash drop, commit action, branch switch /
    /// create; `changes_actions.rs`).
    Changes(crate::changes_actions::ChangesModal),
    /// Changes / History footer dialogs (PR, merge, close, sync confirm, branch continue;
    /// `footer.rs`).
    Footer(Box<crate::footer::FooterModal>),
    /// A profile skill's `SKILL.md` (`skills_ui.rs`).
    SkillEdit(crate::skills_ui::SkillEditForm),
    /// Profiles page: "프로필 추가" (the name).
    AddProfile,
    /// 하위 세션 만들기 (`family_ui.rs`).
    NewChild(crate::family_ui::ChildForm),
    /// 세션 인수인계 (`handoff_ui.rs`).
    Handoff(crate::handoff_ui::HandoffForm),
    /// The session list's edit mode: delete the checked sessions (`session_edit.rs`).
    BulkDelete(crate::session_edit::BulkDeleteForm),
    /// The profiles / repos / widgets list's edit mode: delete the checked items (`list_edit.rs`).
    ListDelete(crate::list_edit::ListDeleteForm),
    /// Meninx 빌드 후 재시작: confirm (`self_update.rs`).
    SelfUpdate,
    /// An agent asks to register a widget (`widgets_ui.rs`).
    WidgetRegister(crate::widgets_ui::RegisterForm),
    /// The daemon asks the user to approve something (`approvals_ui.rs`).
    Approval(crate::approvals_ui::ApprovalForm),
}

/// The configuration editor of the configurations page (plain fields; the user will redesign it).
pub struct ConfigEdit {
    pub base: ConfigurationInfo,
    pub name: String,
    pub instructions: text_editor::Content,
    /// The skills part (its changes apply at once; the list is `base.skills`).
    pub skills: crate::skills_ui::SkillsEdit,
    pub mcp: Vec<McpRow>,
    /// Desk's "기본 레포" (`None` = every registered repo).
    pub default_repos: Option<Vec<String>>,
    /// The saved profile changed elsewhere (e.g. a session's `meninx_profile_edit`) while this
    /// editor had unsaved edits: saving would overwrite that change.
    pub outdated: bool,
}

impl ConfigEdit {
    fn new(c: &ConfigurationInfo) -> Self {
        ConfigEdit {
            outdated: false,
            base: c.clone(),
            name: c.name.clone(),
            instructions: text_editor::Content::with_text(&c.instructions),
            skills: Default::default(),
            default_repos: c.default_repos.clone(),
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
            skills: Vec::new(),
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
            default_repos: self.default_repos.clone(),
            ..self.base.clone()
        }
    }

    /// Edits not saved yet (name, instructions, MCP servers, default repos).
    pub fn dirty(&self) -> bool {
        let i = self.to_info();
        let b = &self.base;
        i.name != b.name
            || i.instructions != b.instructions
            || i.mcp_servers != b.mcp_servers
            || i.default_repos != b.default_repos
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

/// Key of the session list picker's "every configuration" entry (not a valid slug).
pub const ALL_CONFIGURATIONS: &str = ":all";

/// A change made in the session list's sort menu.
#[derive(Debug, Clone, Copy)]
pub enum SortChange {
    Add(SortKey),
    Remove(SortKey),
    /// Ascending <-> descending.
    Flip(SortKey),
    /// One place up (-1) or down (+1).
    Move(SortKey, i32),
    /// Back to the server order.
    Reset,
}

/// An account / configuration change requested from the accounts / configurations page.
#[derive(Debug, Clone)]
pub enum SettingsOp {
    AccountRenamed,
    AccountDeleted,
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
    /// Lists of link generation `.0`, request `.1` of [`App::refresh`].
    Lists(u64, u64, Result<Lists, ProtoError>),
    Attached(u64, String, Result<(), ProtoError>),
    TermInput(String, Vec<u8>),
    TermResize(String, u16, u16),
    TermView(ViewEvent),
    Web(WebViewEvent),

    Select(String),
    Start(String),
    Stop(String),
    SessionOp(String, Result<SessionInfo, ProtoError>),
    /// Pin / unpin (`.1`) and archive / unarchive (`.2`) a session (`None` = unchanged).
    SetSessionFlags(String, Option<bool>, Option<bool>),
    SessionFlagsSet(Result<SessionInfo, ProtoError>),

    AskDelete(String),
    DeleteWorktree(bool),
    DeleteConfirm,
    /// Background deletion over (`App::start_delete`): its notice key and per session
    /// (id, name, result).
    DeleteDone(String, Vec<(String, String, Result<(), ProtoError>)>),

    OpenNew,
    /// "워크트리 사용" on / off.
    NewUseWorktrees(bool),
    /// Check / uncheck a repo for the session folder.
    NewRepoChecked(String, bool),
    /// The repo of an original-repo session.
    NewOriginalRepo(String),
    /// The repo list's search box.
    NewRepoFilter(String),
    NewProvider(String),
    /// Account slug; `""` = the CLI's default login.
    NewAccount(String),
    NewConfiguration(String),
    NewTitle(String),
    NewSubmit,
    NewCreated(Result<(SessionInfo, Vec<RepoFailureInfo>, Vec<String>), ProtoError>),
    CloseModal,

    /// Sidebar: show the sessions of this configuration.
    ConfigurationPicked(String),
    /// Navigation rail click: show the page (clicking the active work page toggles the
    /// session list).
    Page(Page),
    /// The pointer entered (`true`) or left (`false`) a rail page button.
    RailHover(Page, bool),
    /// The pointer entered (`true`) or left (`false`) a session row.
    SessionHover(String, bool),
    AcctProvider(String),
    AcctName(String),
    /// Create the account and open its login command in a terminal.
    AcctAdd,
    AcctCreated(Result<AccountInfo, ProtoError>),
    AcctLogin(String, String),
    LoginOpened(Result<SessionInfo, ProtoError>),
    AcctStatus(String, String),
    /// An account's integration: check, install or uninstall.
    AcctIntegration(String, String, meninx_client::IntegrationAction),
    AcctIntegrationDone(String, String, Result<meninx_client::IntegrationInfo, ProtoError>),
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
    /// The profile's own skills (`skills_ui.rs`).
    Skill(crate::skills_ui::SkillMsg),
    ProfileHistory(crate::profile_history_ui::HistoryMsg),
    CfgMcp(usize, McpField, String),
    CfgMcpAdd,
    CfgMcpRemove(usize),
    /// Check / uncheck a default repo of the edited configuration.
    CfgDefaultRepo(String, bool),
    CfgSave,
    CfgCancel,
    CfgDelete(String),
    /// Profiles page: the "프로필 추가" dialog, a created profile, the delete confirm.
    ProfileAdding(bool),
    ProfileCreated(Result<ConfigurationInfo, ProtoError>),
    ProfileDeleteArm(bool),
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
    /// Settings page: a font picked (blank = the default).
    SetFont(FontSlot, String),
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
    /// The side panel's left edge is pressed (width drag).
    SideResizePressed,
    /// The session / profile list's right edge is pressed (width drag).
    ListResizePressed,
    DragMoved(Point),
    /// The pointer moved in a window while the divider is dragged.
    WindowDrag(window::Id, Point),
    DragEnd,

    /// Detail area of a session.
    Detail(String, DetailMsg),
    /// Changes / History footer of a session (`footer.rs`; `""` = its timer).
    Footer(String, crate::footer::FooterMsg),
    ToggleOpenItems,
    CloseOpenItems,
    /// Session list sort menu (header button).
    ToggleSortMenu,
    CloseSortMenu,
    Sort(SortChange),
    /// Fold / unfold a session list group (configuration slug or `LOGIN_GROUP`).
    ToggleGroupFold(String),
    /// Fold / unfold a session list group's 보관함.
    ToggleArchiveOpen(String),
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
    Notice(crate::notice_view::NoticeMsg),
    ReposLoaded(Result<Vec<RepoInfo>, ProtoError>),
    RepoInput(String),
    /// Register the typed path.
    RepoAdd,
    RepoRegistered(Result<RepoInfo, ProtoError>),
    /// Repos page: show a repo.
    RepoSelect(String),
    /// Repos page: the first / cancelled click of "unregister".
    RepoConfirm(bool),
    RepoUnregister(String),
    RepoUnregistered(String, Result<(), ProtoError>),
    /// "초기화하고 추가": register the plain folder with `git init`.
    RepoInitConfirm,
    /// origin's branches of a repo (base pickers).
    OriginBranches(String, Result<Vec<String>, ProtoError>),
    /// New-session dialog: 기준 브랜치 최신화.
    NewRefreshBase(bool),
    /// New-session dialog: base branch of a repo.
    NewBase(String, String),
    /// Session repos dialog: base branch of a repo.
    SessionRepoBase(String, String),
    /// Repos page base editor.
    BaseRefs(String, Result<(Option<String>, Vec<String>), ProtoError>),
    BaseMode(bool),
    /// Fixed base picked (`None` = automatic).
    BaseFixed(Option<String>),
    BaseCancel,
    BaseFixedSave,
    BaseRuleRemote(String),
    BaseRulePattern(String),
    BaseRuleMatch(String),
    BaseRuleCheck,
    BaseRulePreviewed(String, Result<meninx_client::BaseRulePreview, ProtoError>),
    BaseRuleSave,
    BaseRuleClear,
    BaseSaved(Result<RepoInfo, ProtoError>),
    /// Open the session's repos dialog.
    SessionRepos(String),
    SessionReposTitle(String),
    SessionRepoAdd(String, bool),
    SessionReposSave,
    SessionRepoDetach(String),
    /// Attach (`true`: closes the dialog) / detach finished.
    SessionReposDone(bool, Result<SessionInfo, ProtoError>),
    /// Open the repos page's folder picker.
    PickOpen,
    /// List a folder in the picker (`None` = the start list).
    PickGo(Option<String>),
    /// The picker's path input changed.
    PickPath(String),
    /// Enter in the picker's path input: open the typed folder.
    PickPathGo,
    PickListed(u64, Result<BrowseListing, ProtoError>),
    /// Highlight an entry of the picker.
    PickHighlight(usize),
    /// Arrow keys in the picker.
    PickMove(i32),
    /// Enter in the picker: open the highlighted folder.
    PickEnter,
    /// "Select this folder": register it.
    PickConfirm,

    WindowOpened(Option<window::Id>),
    /// A window (the app window or the detail window) changed size.
    WindowResized(window::Id, Size),
    WindowMoved(window::Id, Point),
    /// A window got (`true`) or lost the keyboard focus.
    WindowFocus(window::Id, bool),
    /// A window's close button (routed: the app window asks / exits, the detail window hides).
    WindowCloseRequest(window::Id),
    WindowClosed(window::Id),
    /// The app window's close button.
    CloseRequested,
    /// The detail area's own window (`detail_window.rs`).
    DetailWindow(crate::detail_window::WinMsg),
    /// A title bar or resize grip action on a window (`frame.rs`).
    Frame(window::Id, crate::frame::FrameMsg),
    /// The detail window's 열린 항목 tab list (`detail_rail.rs`).
    Rail(crate::detail_rail::RailMsg),
    /// An open item's right-click menu opened (at the pointer) or closed (`item_menu.rs`).
    ItemMenu(Option<crate::item_menu::ItemMenu>),
    ItemAct(crate::item_menu::ItemAct),
    /// Open (`Some`) or close a session row's right-click menu.
    SessionMenu(Option<crate::session_menu::SessionMenu>),
    SessionAct(crate::session_menu::SessionAct),
    /// Session families (`family_ui.rs`).
    Family(crate::family_ui::FamilyMsg),
    /// Session handoff (`handoff_ui.rs`).
    Handoff(crate::handoff_ui::HandoffMsg),
    /// The session list's edit mode (`session_edit.rs`).
    SessionEdit(crate::session_edit::EditMsg),
    /// The profiles / repos / widgets list's edit mode (`list_edit.rs`).
    ListEdit(crate::list_edit::ListEditMsg),
    /// An incompatible local server (`old_server.rs`).
    OldServer(crate::old_server::OldServerMsg),
    /// Self-update from the footer (`self_update.rs`).
    Update(crate::self_update::UpdateMsg),
    /// Close the session menu, then handle the message (세션 설정, 삭제).
    SessionMenuThen(Box<Message>),
    ItemDrag(crate::item_menu::DragMsg),
    /// User terminals of the detail area (`uterm.rs`).
    UTerm(crate::uterm::UTermMsg),
    /// Browser tabs and 즐겨찾기 (`browser.rs`).
    Browser(crate::browser::BrowserMsg),
    Widget(crate::widgets_ui::WidgetMsg),
    Preview(crate::previews_ui::PreviewMsg),
    Approval(crate::approvals_ui::ApprovalMsg),
    AutoAnswer(crate::auto_answers_ui::AutoAnswerMsg),
    /// The input box under an agent terminal.
    Composer(crate::composer::ComposerMsg),
    /// An in-app file drag: the pointer entered (`Some`) or left a drop target, and moved.
    DropHover(crate::composer::Dest, bool),
    DragPointer(iced::Point),
    /// A file dropped on a window (`composer.rs`); in-app scripts send it directly.
    FileDropped(std::path::PathBuf),
    /// A file dropped on a window by the OS: `FileDropped` and the main window brought to the
    /// front (`composer.rs`).
    OsFileDropped(std::path::PathBuf),
    /// The keyboard's modifiers changed.
    Modifiers(keyboard::Modifiers),
    /// "+ 새로 열기" → 폴더 선택… (the server folder picker, for a 공용 terminal).
    PickTermFolder,
    /// Where a Changes row or commit was right-clicked (window coordinates).
    SideMenuAt(Point),
    /// Explore's 파일 찾기 설정 popover, and its gitignore setting.
    ExploreSettings(bool),
    ExploreHideIgnored(bool),

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

/// `MENINX_REMOTE=ssh:<host>` (tests, scripts) overrides the server setting.
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
                transport: Err(format!("MENINX_REMOTE={remote:?} (expected ssh:<host>)")),
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
                    if let Some(dir) = std::env::var("MENINX_DATA_DIR").ok().filter(|v| !v.is_empty())
                    {
                        l = l.env("MENINX_DATA_DIR", dir);
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
    /// `MENINX_REMOTE` (overrides `ui.server`).
    pub remote_override: Option<String>,
    pub target: Target,
    pub link: Option<Link>,
    /// Incremented per new [`Link`]; results of older links are ignored.
    pub link_gen: u64,
    /// The last list request sent ([`App::refresh`]) and the newest one applied: refreshes
    /// overlap (each `SessionsChanged` starts one) and their answers may arrive out of order,
    /// so an older answer must not replace a newer list (a just-created session would vanish
    /// from the list until the next change).
    pub lists_asked: std::cell::Cell<u64>,
    pub lists_applied: u64,
    pub conn: Conn,
    /// Provider, account, configuration and session lists arrived at least once.
    pub loaded: bool,
    pub providers: Vec<ProviderInfo>,
    pub accounts: Vec<AccountInfo>,
    pub configs: Vec<ConfigurationInfo>,
    /// Registered repositories (Hermes Desk's `ws.repos`).
    pub repos: Vec<RepoInfo>,
    /// origin's branches per repo id, as of the last fetch (base pickers).
    pub origin_branches: std::collections::HashMap<String, Vec<String>>,
    pub sessions: Vec<Live>,
    pub selected: Option<String>,
    pub ui: UiState,
    pub term_config: TermConfig,
    pub window: Option<window::Id>,
    pub window_size: Size,
    /// The app window is maximised (its title bar shows the restore glyph, no resize grips).
    pub window_maximized: bool,
    /// The app window is minimised; while it is, whether the pinned detail window was maximised.
    pub window_minimized: bool,
    pub remaximize_detail: bool,
    /// The detail area's own window while it is open (`detail_window.rs`).
    pub detail_window: Option<window::Id>,
    pub detail_window_size: Size,
    /// The detail window's owner as last set by the pin: (app window, owner) handles.
    pub detail_owner: Option<(Option<isize>, Option<isize>)>,
    /// The detail window was created (its `window::open` finished).
    pub detail_window_ready: bool,
    /// The detail window's position (logical px) once known.
    pub detail_window_pos: Option<Point>,
    /// The detail window is maximised (its title bar shows the restore glyph, no resize grips).
    pub detail_window_maximized: bool,
    pub dragging: bool,
    /// The side panel's left edge is dragged.
    pub side_dragging: bool,
    /// The session / profile list's right edge is dragged.
    pub list_dragging: bool,
    /// The 스크립트 section's top edge is dragged: (pointer y at the first move, height then).
    pub scripts_drag: Option<(Option<f32>, f32)>,
    pub modal: Option<Modal>,
    /// Accounts, configurations, repos and settings pages.
    pub settings: SettingsForm,
    /// Settings page: the font pickers (UI, code, terminal), listing the installed families.
    pub font_pickers: [iced::widget::combo_box::State<String>; 3],
    /// The UI font in effect (read at start; a change applies after a restart).
    pub boot_ui_font: String,
    /// The rail page button under the pointer (on its own page it shows the list toggle icon).
    pub rail_hover: Option<Page>,
    /// The session row under the pointer (shows its pin / delete buttons).
    pub session_hover: Option<String>,
    /// Self-update (`self_update.rs`): possible here, the last run's status, the run this app
    /// started or applied (it quits when that run swaps), a start error.
    pub update_available: bool,
    /// The local server speaks another protocol (`old_server.rs`).
    pub server_outdated: bool,
    pub update_status: Option<crate::self_update::Status>,
    pub update_run: Option<String>,
    pub update_error: Option<String>,
    /// Sessions whose agent finished a turn while another session was shown (Desk's 새 답변);
    /// cleared when the session is selected.
    pub new_answers: std::collections::HashSet<String>,
    /// Sessions this app asked the server to resume on open (`SessionInfo::resume_on_open`)
    /// and that have not run since: asked once, so a failing start is not repeated.
    pub resumed: std::collections::HashSet<String>,
    /// Notification centre (`notices.rs`, `notice_view.rs`).
    pub notices: crate::notice_view::AppNotices,
    /// The bell's list is open.
    pub notice_open: bool,
    /// What notices that mirror other state (PR jobs, self-update) last showed, by key: they
    /// change only when this does (`notice_view.rs` `sync_state_notices`).
    pub notice_sigs: std::collections::HashMap<NoticeKey, String>,
    /// Sessions being deleted in the background: hidden from the list until the server is
    /// done (they come back when it fails).
    pub deleting: std::collections::HashSet<String>,
    /// Numbers the deletion notices.
    pub delete_seq: u64,
    /// The app window has the keyboard focus (agent notices also flash the taskbar without).
    pub focused: bool,
    pub script: Option<Script>,
    /// Test hook: keep drawing the web view under modals (reproduces the z-order bug).
    pub modal_web_unsafe: bool,
    /// `Welcome::server_instance` of the last connection: terminal offsets are only valid
    /// within one server process.
    pub server_instance: Option<String>,
    /// The "open items" list of the detail header is open.
    pub open_items: bool,
    /// The detail window's icon rail is widened (hovered; `detail_rail.rs`).
    pub rail_open: bool,
    /// The tab list's settings menu is open.
    pub rail_menu: bool,
    /// Count of hovers of the icon rail: a pending open only applies to the latest.
    pub rail_arm: u64,
    /// The open item menu (right-click on an item of the tab list or the popover).
    pub item_menu: Option<crate::item_menu::ItemMenu>,
    pub session_menu: Option<crate::session_menu::SessionMenu>,
    /// An open item dragged within its group.
    pub item_drag: Option<crate::item_menu::ItemDrag>,
    /// The "+ 새로 열기" menu is open.
    pub new_menu: bool,
    /// Registered widgets and their open tabs, page and launcher state (`widgets_ui.rs`).
    pub widgets: crate::widgets_ui::WidgetsState,
    /// Approval questions waiting and the kept grants (`approvals_ui.rs`).
    pub approvals: crate::approvals_ui::ApprovalsState,
    /// Settings -> 에이전트 자동 응답: the last error (`auto_answers_ui.rs`).
    pub auto_answer_error: Option<String>,
    /// "+ 새로 열기" opens 공용 items (no session) instead of the session's.
    pub new_global: bool,
    /// Its address field, and the bookmark being renamed (url, name).
    pub new_address: String,
    pub bookmark_edit: Option<(String, String)>,
    /// 공용 browser tabs (no session), and the last browser tab id given.
    pub global_browsers: Vec<crate::browser::Browser>,
    /// 공용 user terminals (no session).
    pub global_uterms: Vec<crate::uterm::UTermView>,
    /// 공통 tabs (Desk's common items): (owner session, item), shown at the top of every
    /// session's 열린 항목; kept while the app runs.
    pub common: Vec<(String, crate::detail::ItemKey)>,
    /// "+ 새로 열기": the server's shells (asked once), the picked start folder (session: a
    /// repo or the session folder, `None` = the Changes panel's repo; 공용: `None` = home),
    /// the servers listening in its terminals, and the keyboard's modifiers (Alt+클릭 = 공용).
    /// A file dragged inside the app (from the Explore tree): (session, server path), where
    /// it would drop (the pointer is over the input box / the terminal), and the pointer
    /// (for the label following it).
    pub file_drag: Option<(String, Vec<String>)>,
    /// Entries cut or copied in Explore (`explore_actions.rs`).
    pub explore_clip: Option<crate::explore_actions::ExploreClip>,
    pub drop_target: Option<crate::composer::Dest>,
    pub drag_pointer: Option<iced::Point>,
    pub shells: Option<Vec<meninx_client::ShellInfo>>,
    pub new_dir_repo: Option<Option<String>>,
    pub new_dir_global: Option<String>,
    pub ports: Vec<meninx_client::PortInfo>,
    pub modifiers: keyboard::Modifiers,
    pub next_browser: u32,
    /// Where the side panel's open menu was asked for.
    pub side_menu_at: Option<Point>,
    pub explore_settings: bool,
    /// The session list's sort menu is open.
    pub sort_open: bool,
    /// The session list's edit mode (`session_edit.rs`).
    pub session_edit: crate::session_edit::SessionEdit,
    /// The profiles / repos / widgets list's edit mode (`list_edit.rs`).
    pub list_edit: crate::list_edit::ListEdit,
}

impl App {
    pub fn boot(
        data_dir: PathBuf,
        runtime: WebRuntime,
        remote_override: Option<String>,
    ) -> (Self, Task<Message>) {
        let mut app = Self::load(data_dir, runtime, remote_override);
        crate::widgets_ui::install();
        crate::previews_ui::install();
        app.script = Script::from_env(&app.data_dir);
        // Self-update: a run that started this app learns it came up.
        app.update_available = crate::self_update::available();
        crate::self_update::confirm_started(&app.data_dir);
        app.update_status = crate::self_update::read_status(&app.data_dir);
        app.connect();
        // The app window (a daemon opens none by itself, frameless like the detail window);
        // closing it is handled by the app.
        let (_, open) = window::open(crate::frame::frameless(window::Settings {
            size: Size::new(1440.0, 880.0),
            min_size: Some(Size::new(640.0, 400.0)),
            exit_on_close_request: false,
            ..window::Settings::default()
        }));
        let task = open.map(|id| Message::WindowOpened(Some(id)));
        (app, task)
    }

    /// App state from `ui.json` (no connection yet).
    pub fn load(data_dir: PathBuf, runtime: WebRuntime, remote_override: Option<String>) -> Self {
        let ui = UiState::load(&data_dir);
        meninx_i18n::apply(ui.language);
        meninx_views::theme::set_code_font(&ui.fonts.code);
        let mut term_config = TermConfig::default();
        if !ui.fonts.terminal.is_empty() {
            term_config.font_family = ui.fonts.terminal.clone();
        }
        let fonts = installed_font_families();
        let font_pickers = [(); 3].map(|_| iced::widget::combo_box::State::new(fonts.clone()));
        let boot_ui_font = ui.fonts.ui.clone();
        let target = build_target(&ui.server, remote_override.as_deref());
        let settings = SettingsForm::new(&ui.server);
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
            lists_asked: std::cell::Cell::new(0),
            lists_applied: 0,
            conn,
            loaded: false,
            providers: Vec::new(),
            accounts: Vec::new(),
            configs: Vec::new(),
            repos: Vec::new(),
            origin_branches: Default::default(),
            sessions: Vec::new(),
            selected: None,
            ui,
            term_config,
            window: None,
            window_size: Size::new(1400.0, 860.0),
            window_maximized: false,
            window_minimized: false,
            remaximize_detail: false,
            detail_window: None,
            detail_window_size: Size::new(960.0, 640.0),
            detail_owner: None,
            detail_window_ready: false,
            detail_window_pos: None,
            detail_window_maximized: false,
            dragging: false,
            side_dragging: false,
            list_dragging: false,
            scripts_drag: None,
            modal: None,
            settings,
            font_pickers,
            boot_ui_font,
            rail_hover: None,
            session_hover: None,
            new_answers: Default::default(),
            resumed: Default::default(),
            update_available: false,
            server_outdated: false,
            update_status: None,
            update_run: None,
            update_error: None,
            notices: Default::default(),
            notice_open: false,
            notice_sigs: Default::default(),
            deleting: Default::default(),
            delete_seq: 0,
            focused: true,
            script: None,
            modal_web_unsafe: false,
            server_instance: None,
            open_items: false,
            rail_open: false,
            rail_menu: false,
            rail_arm: 0,
            item_menu: None,
            session_menu: None,
            item_drag: None,
            new_menu: false,
            widgets: Default::default(),
            approvals: Default::default(),
            auto_answer_error: None,
            new_global: false,
            new_address: String::new(),
            bookmark_edit: None,
            global_browsers: Vec::new(),
            global_uterms: Vec::new(),
            common: Vec::new(),
            shells: None,
            file_drag: None,
            explore_clip: None,
            drop_target: None,
            drag_pointer: None,
            new_dir_repo: None,
            new_dir_global: None,
            ports: Vec::new(),
            modifiers: keyboard::Modifiers::empty(),
            next_browser: 0,
            side_menu_at: None,
            explore_settings: false,
            sort_open: false,
            session_edit: Default::default(),
            list_edit: Default::default(),
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
        for t in &mut self.global_uterms {
            t.attached = false;
        }
        for l in &mut self.sessions {
            l.attached = false;
            for t in &mut l.uterms {
                t.attached = false;
            }
            l.detail.watched.clear();
            l.detail.stale = true;
        }
    }

    /// Drop the client (like closing the app): the server keeps every session running.
    pub fn disconnect(&mut self, reason: String) {
        self.link = None;
        self.conn = Conn::Disconnected(reason);
        for t in &mut self.global_uterms {
            t.attached = false;
        }
        for l in &mut self.sessions {
            l.attached = false;
            for t in &mut l.uterms {
                t.attached = false;
            }
            l.detail.watched.clear();
            l.detail.stale = true;
        }
    }

    pub fn theme(&self, _window: window::Id) -> iced::Theme {
        crate::style::theme()
    }

    pub fn title(&self, window: window::Id) -> String {
        if Some(window) == self.detail_window {
            return self.detail_window_title();
        }
        "Meninx".into()
    }

    // ------------------------------------------------------------------ lookups

    pub fn client(&self) -> Option<Client> {
        self.link.as_ref().map(|l| l.client.clone())
    }

    /// The client, if connected (actions are disabled otherwise).
    pub(crate) fn online(&self) -> Option<Client> {
        self.conn.is_connected().then(|| self.client()).flatten()
    }

    /// Width left of the terminal: the rail and, when open, the session list and its handle.
    pub fn left_w(&self) -> f32 {
        if self.ui.sessions_open {
            RAIL_W + self.list_w() + DIVIDER_W
        } else {
            RAIL_W
        }
    }

    /// The session / profile list's width: the dragged one, leaving the terminal its minimum.
    pub fn list_w(&self) -> f32 {
        let max = (self.window_size.width - RAIL_W - DIVIDER_W - CENTRE_MIN).min(LIST_MAX);
        self.ui.list_width.clamp(LIST_MIN, max.max(LIST_MIN))
    }

    /// Width shared by the terminal and the detail pane (the divider's range): the window
    /// minus the rail, the session list, the divider and the side panel.
    pub fn split_avail(&self) -> f32 {
        let side = if self.ui.detail_collapsed || self.selected.is_none() {
            0.0
        } else {
            self.side_w() + DIVIDER_W
        };
        (self.window_size.width - self.left_w() - DIVIDER_W - side).max(1.0)
    }

    /// The side panel's width: the dragged one, kept within what the window leaves it.
    pub fn side_w(&self) -> f32 {
        let detail = if self.detail_in_app() {
            DIVIDER_W + DETAIL_MIN
        } else {
            0.0
        };
        let max = self.window_size.width - self.left_w() - DIVIDER_W - CENTRE_MIN - detail;
        self.ui.side_width.clamp(SIDE_MIN, max.max(SIDE_MIN))
    }

    /// The work page (sessions, terminal, detail) is shown.
    pub fn on_work_page(&self) -> bool {
        self.ui.page == Page::Work
    }

    pub fn connected(&self) -> bool {
        self.conn.is_connected() && self.link.is_some()
    }

    pub fn live(&self, id: &str) -> Option<&Live> {
        self.sessions.iter().find(|l| l.id() == id)
    }


    /// Drops a deleted session from the app: its row, web view, saved detail state and the
    /// selection (which moves to the first listed session). The caller saves the UI state.
    pub(crate) fn forget_session(&mut self, id: &str) {
        self.notices.forget_session(id);
        self.new_answers.remove(id);
        if let Some(idx) = self.sessions.iter().position(|l| l.id() == id) {
            let live = self.sessions.remove(idx);
            if let Some(w) = live.web {
                w.close();
            }
        }
        self.ui.web_urls.remove(id);
        self.ui.detail.remove(id);
        self.session_edit.checked.remove(id);
        if self.selected.as_deref() == Some(id) {
            self.selected = self
                .sessions
                .iter()
                .find(|l| self.in_filter(&l.info))
                .map(|l| l.info.id.clone());
        }
    }
    /// Deletes `ids` (in this order, one at a time) in the background: the dialog is already
    /// closed, the sessions leave the list now and a running notice follows the work (the
    /// server stops each process, waits for it and removes its worktrees and branches, which
    /// can take a while).
    pub(crate) fn start_delete(&mut self, ids: Vec<String>, remove: bool) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let ids: Vec<(String, String)> = ids
            .into_iter()
            .filter(|id| !self.deleting.contains(id))
            .map(|id| {
                let name = self.session_name(&id).unwrap_or_else(|| id.clone());
                (id, name)
            })
            .collect();
        if ids.is_empty() {
            return Task::none();
        }
        self.delete_seq += 1;
        let key = format!("delete:{}", self.delete_seq);
        for (id, _) in &ids {
            self.deleting.insert(id.clone());
            self.session_edit.checked.remove(id);
        }
        if self.selected.as_ref().is_some_and(|s| self.deleting.contains(s)) {
            self.selected = self
                .sessions
                .iter()
                .find(|l| self.in_filter(&l.info))
                .map(|l| l.info.id.clone());
        }
        let n = ids.len() as i64;
        let names: Vec<&str> = ids.iter().map(|(_, name)| name.as_str()).collect();
        self.notify(
            Notice::info(Text::tr(move || tr::notice_session_deleting(n)))
                .body(names.join(", "))
                .key(NoticeKey::Other(key.clone()))
                .task(TaskState::Running)
                .quiet(),
        );
        crate::script::log(&format!(
            "deleting sessions {:?} in the background (remove_worktree={remove})",
            ids.iter().map(|(id, _)| id).collect::<Vec<_>>()
        ));
        let work = Task::perform(
            async move {
                let mut out = Vec::with_capacity(ids.len());
                for (id, name) in ids {
                    let r = c.delete_session(&id, remove).await;
                    out.push((id, name, r));
                }
                out
            },
            move |r| Message::DeleteDone(key.clone(), r),
        );
        Task::batch([self.after_select(), work])
    }

    /// A background deletion is over: deleted sessions go, failed ones come back to the list
    /// and the notice says which failed and why.
    fn delete_done(
        &mut self,
        key: String,
        results: Vec<(String, String, Result<(), ProtoError>)>,
    ) -> Task<Message> {
        let mut deleted = Vec::new();
        let mut failed = Vec::new();
        for (id, name, r) in results {
            self.deleting.remove(&id);
            match r {
                Ok(()) => {
                    crate::script::log(&format!("deleted session {id}"));
                    self.forget_session(&id);
                    deleted.push(name);
                }
                Err(e) => {
                    crate::script::log(&format!("deleting session {id} failed: {e:?}"));
                    failed.push(format!("{name}: {}", i18n::proto_error(&e)));
                }
            }
        }
        self.save_ui();
        let key = NoticeKey::Other(key);
        let n = if failed.is_empty() {
            let count = deleted.len() as i64;
            Notice::success(Text::tr(move || tr::notice_session_deleted(count)))
                .body(deleted.join(", "))
                .task(TaskState::Done)
                .toast()
                .quiet()
        } else {
            Notice::error(Text::tr(tr::notice_session_delete_failed))
                .body(failed.join("\n"))
                .task(TaskState::Failed)
                .toast()
        };
        self.notify(n.key(key));
        self.after_select()
    }

    pub(crate) fn live_mut(&mut self, id: &str) -> Option<&mut Live> {
        self.sessions.iter_mut().find(|l| l.id() == id)
    }

    pub fn selected_live(&self) -> Option<&Live> {
        self.selected.as_deref().and_then(|id| self.live(id))
    }

    /// The session whose folder `sid` works in: a shared child's owner (up its parents),
    /// else `sid` itself. Sessions of one folder share one detail area (open items, browser
    /// tabs, web page, side panel); terminals stay each session's (`docs/session-family.md`).
    pub fn workspace_owner(&self, sid: &str) -> String {
        let mut cur = sid.to_owned();
        for _ in 0..=crate::family_ui::MAX_DEPTH {
            match self.live(&cur) {
                Some(l) if l.info.shared => match &l.info.parent {
                    Some(p) if self.live(p).is_some() => cur = p.clone(),
                    _ => break,
                },
                _ => break,
            }
        }
        cur
    }

    /// The session whose detail area is shown: the selected session's folder owner.
    pub fn shown_live(&self) -> Option<&Live> {
        let id = self.workspace_owner(self.selected.as_deref()?);
        self.live(&id)
    }

    /// Whose terminals a detail area of `owner` shows: the selected session when it works in
    /// that folder, else the owner.
    pub(crate) fn term_live<'a>(&'a self, owner: &'a Live) -> &'a Live {
        match self.selected_live() {
            Some(s) if self.workspace_owner(s.id()) == owner.id() => s,
            _ => owner,
        }
    }

    /// The session a terminal opened from `sid`'s detail area belongs to: the selected
    /// session when it works in that folder (terminals are each session's), else `sid`.
    pub(crate) fn term_session(&self, sid: &str) -> String {
        match self.selected.as_deref() {
            Some(s) if self.workspace_owner(s) == self.workspace_owner(sid) => s.to_owned(),
            _ => sid.to_owned(),
        }
    }

    /// Is this item shown in the detail area now? Terminals: only the shown session's (and
    /// the 공용 ones).
    pub(crate) fn item_shown(&self, owner: &Live, k: &ItemKey) -> bool {
        match k {
            ItemKey::Terminal(id) => {
                id.starts_with("g:") || id.starts_with(&format!("{}:", self.term_live(owner).id()))
            }
            _ => true,
        }
    }

    /// Whether `owner`'s pane shows its web page: the page is open (an item that can be
    /// closed, not open by default) and active, or the active item is another session's
    /// terminal (the pane falls back to the page).
    pub(crate) fn page_shown(&self, owner: &Live) -> bool {
        let d = &owner.detail;
        d.item(&ItemKey::Web).is_some()
            && (d.active == ItemKey::Web || !self.item_shown(owner, &d.active))
    }

    /// Close `sid`'s web page item's view and forget its address (the item itself is removed
    /// by the caller).
    pub(crate) fn close_page(&mut self, sid: &str) {
        let owner = self.workspace_owner(sid);
        if let Some(l) = self.live_mut(&owner) {
            if let Some(w) = l.web.take() {
                w.close();
            }
            l.url_input.clear();
            l.web_title.clear();
            l.loading = false;
            l.can_back = false;
            l.can_forward = false;
            l.error = None;
        }
        self.ui.web_urls.remove(&owner);
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

    /// Whether the sidebar lists a session: every session under 전체, else sessions of the
    /// current configuration, and login terminals always.
    pub fn in_filter(&self, info: &SessionInfo) -> bool {
        !self.deleting.contains(&info.id)
            && (self.ui.sessions_all
                || info.kind == SessionKind::Login
                || info.configuration == self.current_configuration())
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

    /// Name of a registered repo (falls back to a session's attached repo, then the id).
    pub fn repo_name(&self, id: &str) -> String {
        self.repos
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
            .or_else(|| {
                self.sessions
                    .iter()
                    .flat_map(|l| l.info.repos.iter())
                    .find(|r| r.repo == id)
                    .map(|r| r.name.clone())
            })
            .unwrap_or_else(|| id.to_owned())
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
    pub(crate) fn default_account(&self, provider: &str) -> Option<String> {
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

    pub(crate) fn refresh(&self) -> Task<Message> {
        let Some(c) = self.client() else {
            return Task::none();
        };
        let generation = self.link_gen;
        let seq = self.lists_asked.get() + 1;
        self.lists_asked.set(seq);
        Task::perform(
            async move {
                let p = c.providers(false).await?;
                let a = c.accounts().await?;
                let k = c.configurations().await?;
                let s = c.sessions().await?;
                Ok((p, a, k, s))
            },
            move |r| Message::Lists(generation, seq, r),
        )
    }

    /// Check the integration of accounts whose provider has one and that are not known yet.
    fn integration_checks(&mut self) -> Task<Message> {
        let todo: Vec<(String, String)> = self
            .accounts
            .iter()
            .filter(|a| {
                self.providers
                    .iter()
                    .any(|p| p.id == a.provider && p.caps.integration)
            })
            .map(|a| (a.provider.clone(), a.slug.clone()))
            .filter(|k| !self.settings.acct_integration.iter().any(|(x, _)| x == k))
            .collect();
        Task::batch(todo.into_iter().map(|(p, s)| {
            self.update_inner(Message::AcctIntegration(p, s, meninx_client::IntegrationAction::Status))
        }))
    }

    /// Merge the server's session list: keep terminal models and web views of known sessions.
    fn apply_lists(&mut self, lists: Lists) -> Task<Message> {
        let (providers, accounts, configs, list) = lists;
        if self.settings.acct_provider.is_empty()
            && let Some(p) = providers.iter().find(|p| p.caps.accounts)
        {
            self.settings.acct_provider = p.id.clone();
        }
        self.providers = providers;
        self.accounts = accounts;
        self.configs = configs;
        self.sync_profile_edit();
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
        // Widgets: their open tabs are restored once the sessions are known.
        // The shown session continues where it was when the server went away.
        let resume = match self.selected.clone() {
            Some(id) => self.resume_on_open(&id),
            None => Task::none(),
        };
        Task::batch([self.after_select(), resume, self.load_widgets()])
    }

    /// Resume a session that was running when the server last went away, now that it is
    /// opened: the server does not restart every session by itself when it starts (each agent
    /// CLI costs memory and CPU), so only the sessions the user opens run again.
    fn resume_on_open(&mut self, id: &str) -> Task<Message> {
        let due = self
            .live(id)
            .is_some_and(|l| l.info.resume_on_open && l.state == RunState::Stopped);
        if !due || !self.resumed.insert(id.to_owned()) {
            return Task::none();
        }
        crate::script::log(&format!("session {id} resumed on open"));
        self.update_inner(Message::Start(id.to_owned()))
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
                self.server_outdated = false;
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
                        l.uterms.clear();
                        l.attached = false;
                    }
                    self.global_uterms.clear();
                }
                self.conn = Conn::Connected(Box::new(w));
                self.conn_notice();
                // Watches and listings start over on the new connection (the gap is not
                // replayed): refresh the shown detail area.
                for l in &mut self.sessions {
                    l.detail.watched.clear();
                    l.detail.stale = true;
                }
                // The PTY size follows the most recent client: re-send ours.
                if let Some(l) = self.selected_live()
                    && let Some((c, r)) = l.size
                {
                    self.send_input(Input::Resize(l.info.id.clone(), c, r));
                }
                Task::batch([
                    self.refresh(),
                    self.ensure_attached(),
                    self.sync_detail(),
                    self.load_repos(),
                ])
            }
            ClientEvent::Disconnected { error } => {
                crate::script::log(&format!("disconnected: {error:?}"));
                self.server_outdated = self.target.local
                    && error.as_ref().is_some_and(|e| e.id == "proto-version");
                self.conn = match error {
                    None => Conn::Reconnecting,
                    Some(e) => Conn::Disconnected(i18n::proto_error(&e)),
                };
                self.conn_notice();
                Task::none()
            }
            ClientEvent::Notify(n) => self.on_notify(n),
            ClientEvent::Term(chunk) => {
                if self.uterm_chunk(&chunk) {
                    return Task::none();
                }
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
                if self.live(&session).is_some() {
                    self.notify(
                        Notice::warning(Text::tr(tr::notice_attach_failed))
                            .body(error.to_string())
                            .session(session.clone())
                            .open(Message::Notice(crate::notice_view::NoticeMsg::GoSession(session)))
                            .quiet(),
                    );
                }
                Task::none()
            }
        }
    }

    fn on_notify(&mut self, n: Notify) -> Task<Message> {
        match n {
            Notify::State { session, state } => {
                if self.uterm_notify(&session, Some(state), None) {
                    return Task::none();
                }
                crate::script::log(&format!("session {session} state {state:?}"));
                let mut was = None;
                if let Some(l) = self.live_mut(&session) {
                    was = Some(l.state);
                    l.state = state.into();
                    if state == SessionState::Running {
                        l.error = None;
                        l.info.resume_on_open = false;
                    }
                }
                if state == SessionState::Running {
                    self.resumed.remove(&session);
                }
                // The process ended by itself (not stopped from Meninx).
                let flash = match (was, state) {
                    (Some(RunState::Running), SessionState::Exited(code)) => {
                        self.exit_notice(&session, code)
                    }
                    _ => Task::none(),
                };
                Task::batch([flash, self.ensure_attached()])
            }
            Notify::Activity { session, activity } => {
                let shown = self.selected.as_deref() == Some(session.as_str());
                let mut was = None;
                if let Some(l) = self.live_mut(&session) {
                    was = Some(l.info.activity);
                    l.info.activity = activity;
                }
                if was == Some(AgentActivity::Working) && activity == AgentActivity::Idle && !shown {
                    self.new_answers.insert(session.clone());
                }
                match was {
                    Some(w) => self.activity_notice(&session, w, activity),
                    None => Task::none(),
                }
            }
            Notify::AgentRequest {
                request,
                session,
                tool,
                args,
            } => self.on_agent_request(request, session, tool, args),
            Notify::AgentRequestEnded { request, timed_out } => self.agent_request_ended(request, timed_out),
            Notify::Title { session, title } => {
                if self.uterm_notify(&session, None, Some(title.clone())) {
                    return Task::none();
                }
                if let Some(l) = self.live_mut(&session) {
                    l.info.term_title = title;
                }
                Task::none()
            }
            Notify::Bell { session } => self.bell_notice(&session),
            Notify::Branch { session, badge } => {
                if let Some(l) = self.live_mut(&session) {
                    l.info.branch = Some(badge);
                }
                Task::none()
            }
            Notify::SessionsChanged | Notify::AccountsChanged | Notify::ConfigurationsChanged => {
                self.refresh()
            }
            Notify::ConfigurationEdited {
                name,
                session_title,
                summary,
                ..
            } => self.profile_edited_notice(name, session_title, summary),
            Notify::SessionPrepared {
                session,
                failures,
                base_changes,
            } => {
                crate::script::log(&format!("session {session} worktrees ready"));
                self.worktree_notices(&session, &failures, &base_changes);
                self.refresh()
            }
            Notify::ReposChanged => self.load_repos(),
            Notify::WidgetsChanged => self.load_widgets(),
            Notify::UTermsChanged { session } => self.load_uterms(&session),
            Notify::FsChanged {
                session,
                repo,
                paths,
                rescan,
            } => self.on_fs_notify(&session, &repo, Some((paths, rescan))),
            Notify::GitChanged { session, repo } => self.on_fs_notify(&session, &repo, None),
            Notify::Handoff { session, result } => self.handoff_ended(session, result),
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
            .map(|id| self.workspace_owner(&id))
            .filter(|id| {
                // The page shows when it is active, or when the active item is another
                // session's terminal (the pane falls back to the page).
                self.live(id).is_some_and(|l| self.page_shown(l))
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
        let shown = self.detail_visible().then(|| self.shown_browser()).flatten();
        self.sync_browsers(shown);
    }

    /// [`Self::after_select`] for other modules.
    pub(crate) fn select_shown(&mut self) -> Task<Message> {
        self.after_select()
    }

    pub(crate) fn after_select(&mut self) -> Task<Message> {
        self.sync_web();
        self.open_items = false;
        let uterms = match self.selected.clone() {
            Some(s) => self.load_uterms(&s),
            None => Task::none(),
        };
        let uterms = Task::batch([uterms, self.load_uterms("")]);
        let attach = Task::batch([self.ensure_attached(), self.sync_detail(), uterms]);
        if let Some(live) = self.selected_live()
            && self.modal.is_none()
            && self.on_work_page()
        {
            // The input box, when the session shows one, takes the keys; else the terminal.
            let target = if self.composer_shown(live) && live.state.is_alive() {
                crate::composer::COMPOSER_ID
            } else {
                TERM_ID
            };
            Task::batch([attach, operation::focus(target)])
        } else {
            attach
        }
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.update_inner(message);
        // An approval question waits for the dialog in front of it to close.
        self.next_approval();
        // Toasts the detail panes asked for go to the notification centre.
        self.take_detail_toasts();
        self.sync_state_notices();
        // The detail window follows the state (detached, shown, a session to show).
        Task::batch([task, self.sync_detail_window()])
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Client(generation, ev) => {
                if generation != self.link_gen || self.link.is_none() {
                    return Task::none();
                }
                self.on_client(ev)
            }
            Message::Lists(generation, seq, r) => {
                if generation != self.link_gen {
                    return Task::none();
                }
                match r {
                    // A newer list was applied already.
                    Ok(_) if seq < self.lists_applied => Task::none(),
                    Ok(lists) => {
                        self.lists_applied = seq;
                        let t = self.apply_lists(lists);
                        Task::batch([t, self.integration_checks(), self.load_profile_history()])
                    }
                    Err(e) => {
                        if e.id != "disconnected" {
                            self.notify(
                                Notice::error(Text::tr(tr::err_lists_failed))
                                    .body(i18n::proto_error(&e))
                                    .key(NoticeKey::Other("lists".into()))
                                    .toast(),
                            );
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
                match ev {
                    ViewEvent::Copied(text) => {
                        tracing::debug!(chars = text.chars().count(), "terminal copy");
                        Task::none()
                    }
                    // The shown terminal is the selected session's: files / an image on the
                    // clipboard go in as paths (`composer.rs`).
                    ViewEvent::PasteNotText => match self.selected.clone() {
                        Some(sid) => self.terminal_paste_not_text(&sid),
                        None => Task::none(),
                    },
                    _ => Task::none(),
                }
            }
            Message::Web(ev) => {
                self.on_web(ev);
                Task::none()
            }

            Message::Select(id) => {
                if self.live(&id).is_none() {
                    return Task::none();
                }
                self.new_answers.remove(&id);
                self.notices_seen(&id);
                self.selected = Some(id.clone());
                self.save_ui();
                Task::batch([self.after_select(), self.resume_on_open(&id)])
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
            Message::SetSessionFlags(id, pinned, archived) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                Task::perform(
                    async move { c.set_session_flags(&id, pinned, archived).await },
                    Message::SessionFlagsSet,
                )
            }
            Message::SessionFlagsSet(r) => {
                match r {
                    Ok(info) => {
                        let id = info.id.clone();
                        if let Some(l) = self.live_mut(&id) {
                            l.info = info;
                        }
                    }
                    Err(e) => {
                        self.notify(
                            Notice::error(Text::tr(tr::err_session_flags_failed))
                                .body(i18n::proto_error(&e))
                                .toast(),
                        );
                    }
                }
                Task::none()
            }

            Message::AskDelete(id) => {
                if !self.connected() || self.deleting.contains(&id) {
                    return Task::none();
                }
                self.modal = Some(Modal::Delete(DeleteForm {
                    id,
                    remove_worktree: true,
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
                let Some(Modal::Delete(f)) = &self.modal else {
                    return Task::none();
                };
                let (ids, remove) = (vec![f.id.clone()], f.remove_worktree);
                self.modal = None;
                self.start_delete(ids, remove)
            }
            Message::DeleteDone(key, results) => self.delete_done(key, results),

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
                let configuration = self.current_configuration();
                let repos = self.default_repos_for(&configuration);
                let original_repo = self
                    .ui
                    .last_repo
                    .clone()
                    .filter(|r| self.repos.iter().any(|x| &x.id == r))
                    .or_else(|| self.repos_by_recent().first().map(|r| r.id.clone()));
                self.modal = Some(Modal::NewSession(NewForm {
                    account: self.default_account(&provider),
                    provider,
                    configuration,
                    title: String::new(),
                    use_worktrees: true,
                    repos,
                    original_repo,
                    refresh_base: true,
                    bases: Default::default(),
                    repo_filter: String::new(),
                    error: None,
                    busy: false,
                }));
                self.open_items = false;
                Task::batch([
                    operation::focus(NEW_TITLE_ID),
                    self.load_repos(),
                    self.load_origin_branches(),
                ])
            }
            Message::NewUseWorktrees(v) => self.with_new(|f| f.use_worktrees = v),
            Message::NewRepoChecked(id, on) => {
                let order: Vec<String> = self.repos.iter().map(|r| r.id.clone()).collect();
                self.with_new(|f| {
                    f.repos.retain(|r| *r != id);
                    if on {
                        f.repos.push(id);
                    }
                    // Keep the registry order (worktree folders are made in this order).
                    f.repos
                        .sort_by_key(|r| order.iter().position(|o| o == r).unwrap_or(usize::MAX));
                })
            }
            Message::NewOriginalRepo(id) => self.with_new(|f| f.original_repo = Some(id)),
            Message::NewRepoFilter(v) => self.with_new(|f| f.repo_filter = v),
            Message::NewRefreshBase(v) => self.with_new(|f| f.refresh_base = v),
            Message::NewBase(repo, b) => self.with_new(|f| {
                f.bases.insert(repo, b);
            }),
            Message::OriginBranches(repo, r) => {
                match r {
                    Ok(list) => {
                        self.origin_branches.insert(repo, list);
                    }
                    Err(e) => crate::script::log(&format!("origin branches {repo}: {e}")),
                }
                Task::none()
            }
            Message::SessionRepoBase(repo, b) => {
                if let Some(Modal::SessionRepos(f)) = &mut self.modal {
                    f.bases.insert(repo, b);
                }
                Task::none()
            }
            Message::RepoInitConfirm => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::InitRepo(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.busy {
                    return Task::none();
                }
                f.busy = true;
                f.error = None;
                let path = f.path.clone();
                crate::script::log(&format!("repo init+register {path:?}"));
                Task::perform(
                    async move { c.register_repo_init(&path, true).await },
                    Message::RepoRegistered,
                )
            }
            Message::BaseRefs(repo, r) => {
                if let Some(b) = self.settings.base.as_mut().filter(|b| b.repo == repo) {
                    match r {
                        Ok((_, refs)) => b.refs = Some(refs),
                        Err(e) => {
                            b.refs = Some(Vec::new());
                            self.settings.repos_error = Some(i18n::proto_error(&e));
                        }
                    }
                }
                Task::none()
            }
            Message::BaseMode(rule) => {
                let Some(b) = self.settings.base.as_mut() else {
                    return Task::none();
                };
                b.rule_mode = rule;
                b.saved = false;
                if rule && b.preview.is_none() && !b.preview_loading {
                    return self.update(Message::BaseRuleCheck);
                }
                Task::none()
            }
            Message::BaseFixed(v) => {
                if let Some(b) = self.settings.base.as_mut() {
                    b.fixed = v;
                    b.saved = false;
                }
                Task::none()
            }
            Message::BaseCancel => {
                self.settings.repos_error = None;
                self.open_base_edit(true)
            }
            Message::BaseRuleRemote(v) => self.with_base(|b| b.remote = v),
            Message::BaseRulePattern(v) => self.with_base(|b| b.pattern = v),
            Message::BaseRuleMatch(v) => self.with_base(|b| b.matches = v),
            Message::BaseRuleCheck => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(b) = self.settings.base.as_mut() else {
                    return Task::none();
                };
                b.preview_loading = true;
                b.preview_error = None;
                let (repo, rule) = (b.repo.clone(), b.rule());
                Task::perform(
                    {
                        let repo = repo.clone();
                        async move { c.preview_repo_base_rule(&repo, rule).await }
                    },
                    move |r| Message::BaseRulePreviewed(repo.clone(), r),
                )
            }
            Message::BaseRulePreviewed(repo, r) => {
                if let Some(b) = self.settings.base.as_mut().filter(|b| b.repo == repo) {
                    b.preview_loading = false;
                    match r {
                        Ok(p) => {
                            crate::script::log(&format!(
                                "base rule preview {repo}: pick={:?} matched={:?} excluded={:?}",
                                p.pick, p.matched, p.excluded
                            ));
                            b.preview = Some(p);
                        }
                        Err(e) => {
                            b.preview = None;
                            b.preview_error = Some(i18n::proto_error(&e));
                        }
                    }
                }
                Task::none()
            }
            Message::BaseFixedSave | Message::BaseRuleSave | Message::BaseRuleClear => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(b) = self.settings.base.as_mut() else {
                    return Task::none();
                };
                if b.busy {
                    return Task::none();
                }
                b.busy = true;
                b.saved = false;
                self.settings.repos_error = None;
                let repo = b.repo.clone();
                crate::script::log(&format!(
                    "base save {repo}: fixed={:?} rule={:?} clear={}",
                    b.fixed,
                    matches!(message, Message::BaseRuleSave).then(|| b.rule()),
                    matches!(message, Message::BaseRuleClear)
                ));
                match message {
                    Message::BaseFixedSave => {
                        let fixed = b.fixed.clone();
                        Task::perform(
                            async move { c.set_repo_base_branch(&repo, fixed.as_deref()).await },
                            Message::BaseSaved,
                        )
                    }
                    Message::BaseRuleSave => {
                        let rule = b.rule();
                        Task::perform(
                            async move { c.set_repo_base_rule(&repo, Some(rule)).await },
                            Message::BaseSaved,
                        )
                    }
                    _ => Task::perform(
                        async move { c.set_repo_base_rule(&repo, None).await },
                        Message::BaseSaved,
                    ),
                }
            }
            Message::BaseSaved(r) => {
                if let Some(b) = self.settings.base.as_mut() {
                    b.busy = false;
                }
                match r {
                    Ok(info) => {
                        crate::script::log(&format!(
                            "base saved {}: base={} fixed={:?} rule={:?}",
                            info.name, info.base, info.base_branch, info.base_rule
                        ));
                        if let Some(x) = self.repos.iter_mut().find(|x| x.id == info.id) {
                            *x = info.clone();
                        }
                        if let Some(b) = self.settings.base.as_mut().filter(|b| b.repo == info.id) {
                            // Desk: the mode follows whether a rule is saved.
                            let (refs, preview) = (b.refs.take(), b.preview.take());
                            *b = BaseEdit::new(&info);
                            b.refs = refs;
                            b.preview = preview;
                            b.saved = true;
                        }
                    }
                    Err(e) => {
                        crate::script::log(&format!("base save refused: {e}"));
                        self.settings.repos_error = Some(i18n::proto_error(&e));
                    }
                }
                Task::none()
            }
            Message::NewProvider(p) => {
                let account = self.default_account(&p);
                self.with_new(|f| {
                    f.provider = p;
                    f.account = account;
                })
            }
            Message::NewAccount(a) => self.with_new(|f| f.account = (!a.is_empty()).then_some(a)),
            Message::NewConfiguration(v) => {
                // Desk: the agent's default repos are the first selection.
                let repos = self.default_repos_for(&v);
                self.with_new(|f| {
                    f.configuration = v;
                    f.repos = repos;
                })
            }
            Message::NewTitle(v) => self.with_new(|f| f.title = v),
            Message::NewSubmit => self.submit_new(),
            Message::NewCreated(result) => self.on_created(result),
            Message::CloseModal => {
                let busy = match &self.modal {
                    Some(Modal::DeleteEntry(f)) => f.busy,
                    Some(Modal::StopServer(f)) => f.busy,
                    Some(Modal::Close(f)) => f.busy,
                    Some(Modal::SessionRepos(f)) => f.busy,
                    Some(Modal::NewChild(f)) => f.busy,
                    Some(Modal::Handoff(f)) => f.busy,
                    Some(Modal::ListDelete(f)) => f.busy,
                    Some(Modal::Changes(f)) => f.busy,
                    Some(Modal::Footer(f)) => f.busy(),
                    _ => false,
                };
                if busy {
                    return Task::none();
                }
                // A widget registration closed without 허용 is refused (the agent is told).
                let refused = Task::batch([self.widget_register_closed(), self.approval_closed()]);
                self.modal = None;
                // The next waiting question, if any.
                self.next_approval();
                Task::batch([refused, self.after_select()])
            }

            Message::ConfigurationPicked(p) => {
                let all = p == ALL_CONFIGURATIONS;
                if all || self.configs.iter().any(|x| x.slug == p) {
                    self.ui.sessions_all = all;
                    if !all {
                        self.ui.configuration = Some(p.clone());
                    }
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
            Message::Page(page) => {
                if page == self.ui.page {
                    // The profiles / repos page's button shows / hides its list (Desk).
                    if matches!(page, Page::Configurations | Page::Repos | Page::Widgets) {
                        self.ui.sessions_open = !self.ui.sessions_open;
                        self.save_ui();
                    }
                    // Like Hermes Desk: the active work button shows / hides the session list.
                    if page == Page::Work {
                        self.ui.sessions_open = !self.ui.sessions_open;
                        crate::script::log(&format!(
                            "session list open: {}",
                            self.ui.sessions_open
                        ));
                        self.save_ui();
                    }
                    return Task::none();
                }
                crate::script::log(&format!("page {page:?}"));
                self.ui.page = page;
                self.list_edit = Default::default();
                self.open_items = false;
                self.rail_hover = None;
                self.settings.error = None;
                self.save_ui();
                if page == Page::Configurations {
                    self.sync_profile_edit();
                }
                match page {
                    Page::Work => self.after_select(),
                    // The work page stays in the widget tree under the page (terminal and
                    // editors keep their state): take the keyboard focus away from it.
                    _ => {
                        let unfocus = iced::advanced::widget::operate(
                            iced::advanced::widget::operation::focusable::unfocus(),
                        );
                        let load = match page {
                            Page::Repos => self.load_repos(),
                            Page::Widgets => self.load_widgets(),
                            Page::Configurations => self.load_profile_history(),
                            Page::Settings => {
                                self.settings.reset_server(&self.ui.server);
                                self.load_approvals()
                            }
                            _ => Task::none(),
                        };
                        Task::batch([unfocus, load])
                    }
                }
            }
            Message::RailHover(page, entered) => {
                if entered {
                    self.rail_hover = Some(page);
                } else if self.rail_hover == Some(page) {
                    self.rail_hover = None;
                }
                Task::none()
            }
            Message::SessionHover(id, entered) => {
                if entered {
                    self.session_hover = Some(id);
                } else if self.session_hover.as_deref() == Some(id.as_str()) {
                    // Leaving a row only clears it if the pointer has not entered another one yet.
                    self.session_hover = None;
                }
                Task::none()
            }
            Message::ReposLoaded(r) => {
                match r {
                    Ok(list) => {
                        crate::script::log(&format!(
                            "repos: {:?}",
                            list.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
                        ));
                        let f = &mut self.settings;
                        if f
                            .repo_selected
                            .as_ref()
                            .is_none_or(|s| !list.iter().any(|r| &r.id == s))
                        {
                            f.repo_selected = list.first().map(|r| r.id.clone());
                            f.repo_confirm = false;
                        }
                        self.repos = list;
                        let mut tasks = vec![self.open_base_edit(false)];
                        if matches!(self.modal, Some(Modal::NewSession(_) | Modal::SessionRepos(_))) {
                            tasks.push(self.load_origin_branches());
                        }
                        return Task::batch(tasks);
                    }
                    Err(e) => {
                        if e.id != "disconnected" {
                            self.settings.repos_error = Some(i18n::proto_error(&e));
                        }
                    }
                }
                Task::none()
            }
            Message::RepoInput(v) => self.with_settings(|f| f.repo_input = v),
            Message::RepoAdd => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let f = &mut self.settings;
                let path = f.repo_input.trim().trim_matches('"').to_owned();
                if path.is_empty() || f.repos_busy {
                    return Task::none();
                }
                f.repos_busy = true;
                f.repos_error = None;
                Task::perform(
                    async move { c.register_repo(&path).await },
                    Message::RepoRegistered,
                )
            }
            Message::RepoRegistered(r) => {
                let f = &mut self.settings;
                f.repos_busy = false;
                let init_open = matches!(self.modal, Some(Modal::InitRepo(_)));
                match r {
                    Err(e) if init_open => {
                        crate::script::log(&format!("repo init refused: {e}"));
                        if let Some(Modal::InitRepo(m)) = &mut self.modal {
                            m.busy = false;
                            m.error = Some(i18n::proto_error(&e));
                        }
                        Task::none()
                    }
                    Err(e) if e.id == "not-a-git-folder" => {
                        // Desk: a plain folder -> "Git 저장소가 아니에요 ... 초기화해서 추가할까요?"
                        let path = e.get("path").map_or_else(|| f.repo_input.clone(), str::to_owned);
                        crate::script::log(&format!("repo register: plain folder {path:?}, asking to init"));
                        self.modal = Some(Modal::InitRepo(InitRepoForm {
                            path,
                            error: None,
                            busy: false,
                        }));
                        Task::none()
                    }
                    Ok(repo) => {
                        if init_open {
                            self.modal = None;
                        }
                        crate::script::log(&format!(
                            "repo registered: {} {} (base {:?})",
                            repo.name, repo.path, repo.base
                        ));
                        f.repo_input.clear();
                        f.repo_selected = Some(repo.id.clone());
                        f.repo_confirm = false;
                        if !self.repos.iter().any(|x| x.id == repo.id) {
                            self.repos.push(repo);
                        }
                        self.load_repos()
                    }
                    Err(e) => {
                        crate::script::log(&format!("repo register refused: {e}"));
                        f.repos_error = Some(i18n::proto_error(&e));
                        Task::none()
                    }
                }
            }
            Message::RepoSelect(id) => {
                let f = &mut self.settings;
                f.repo_selected = Some(id);
                f.repo_confirm = false;
                f.repos_error = None;
                self.open_base_edit(false)
            }
            Message::RepoConfirm(v) => self.with_settings(|f| f.repo_confirm = v),
            Message::RepoUnregister(id) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                self.settings.repos_busy = true;
                self.settings.repos_error = None;
                Task::perform(
                    {
                        let id = id.clone();
                        async move { c.unregister_repo(&id).await }
                    },
                    move |r| Message::RepoUnregistered(id.clone(), r),
                )
            }
            Message::RepoUnregistered(id, r) => {
                let f = &mut self.settings;
                f.repos_busy = false;
                f.repo_confirm = false;
                crate::script::log(&format!("repo unregister {id}: {r:?}"));
                match r {
                    Ok(()) => {
                        self.repos.retain(|x| x.id != id);
                        f.repo_selected = self.repos.first().map(|r| r.id.clone());
                        self.load_repos()
                    }
                    Err(e) => {
                        f.repos_error = Some(i18n::proto_error(&e));
                        Task::none()
                    }
                }
            }
            Message::SessionRepos(sid) => {
                if !self.connected() || self.live(&sid).is_none() {
                    return Task::none();
                }
                self.open_items = false;
                let title = self.live(&sid).map(|l| l.info.title.clone()).unwrap_or_default();
                self.modal = Some(Modal::SessionRepos(SessionReposForm {
                    session: sid,
                    title,
                    add: Vec::new(),
                    bases: Default::default(),
                    error: None,
                    busy: false,
                }));
                Task::batch([self.load_repos(), self.load_origin_branches()])
            }
            Message::SessionReposTitle(t) => {
                if let Some(Modal::SessionRepos(f)) = &mut self.modal {
                    f.title = t;
                }
                Task::none()
            }
            Message::SessionRepoAdd(id, on) => {
                if let Some(Modal::SessionRepos(f)) = &mut self.modal {
                    f.add.retain(|r| *r != id);
                    if on {
                        f.add.push(id);
                    }
                }
                Task::none()
            }
            Message::SessionReposSave | Message::SessionRepoDetach(_) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                // The session's current title, to rename only when changed (and not emptied).
                let old_title = match &self.modal {
                    Some(Modal::SessionRepos(f)) => {
                        self.live(&f.session).map(|l| l.info.title.clone()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                // The attached repos' current (session) bases, to save only changed ones.
                let current: Vec<(String, String)> = match &self.modal {
                    Some(Modal::SessionRepos(f)) => self
                        .live(&f.session)
                        .map(|l| {
                            l.info
                                .repos
                                .iter()
                                .map(|r| (r.repo.clone(), r.base.clone()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let Some(Modal::SessionRepos(f)) = &mut self.modal else {
                    return Task::none();
                };
                if f.busy {
                    return Task::none();
                }
                let sid = f.session.clone();
                let new_title = f.title.trim().to_owned();
                let rename = (!new_title.is_empty() && new_title != old_title).then_some(new_title);
                let changes: Vec<(String, String)> = current
                    .iter()
                    .filter_map(|(r, cur)| {
                        f.bases
                            .get(r)
                            .filter(|b| *b != cur)
                            .map(|b| (r.clone(), b.clone()))
                    })
                    .collect();
                match message {
                    Message::SessionRepoDetach(repo) => {
                        f.busy = true;
                        f.error = None;
                        crate::script::log(&format!("detach {repo} from {sid}"));
                        Task::perform(async move { c.detach_repo(&sid, &repo).await }, |r| {
                            Message::SessionReposDone(false, r)
                        })
                    }
                    _ if f.add.is_empty() && changes.is_empty() && rename.is_none() => {
                        self.modal = None;
                        self.after_select()
                    }
                    _ => {
                        f.busy = true;
                        f.error = None;
                        let add: Vec<(String, Option<String>)> = std::mem::take(&mut f.add)
                            .into_iter()
                            .map(|r| {
                                let b = f.bases.get(&r).cloned();
                                (r, b)
                            })
                            .collect();
                        crate::script::log(&format!(
                            "session settings save {sid}: rename {rename:?}, bases {changes:?}, attach {add:?}"
                        ));
                        // Desk: `wsSetSessionBase` for changed bases, then one worktree after
                        // the other (`wsAddRepoToSessions` with the picked base).
                        Task::perform(
                            async move {
                                let mut last = None;
                                if let Some(t) = rename {
                                    last = Some(c.rename_session(&sid, &t).await?);
                                }
                                for (r, b) in changes {
                                    last = Some(c.set_session_base(&sid, &r, Some(&b)).await?);
                                }
                                for (r, b) in add {
                                    last = Some(c.attach_repo_base(&sid, &r, b.as_deref()).await?);
                                }
                                Ok(last.expect("something to save"))
                            },
                            |r| Message::SessionReposDone(true, r),
                        )
                    }
                }
            }
            Message::SessionReposDone(close, r) => {
                let Some(Modal::SessionRepos(f)) = &mut self.modal else {
                    return self.refresh();
                };
                f.busy = false;
                match r {
                    Ok(info) => {
                        crate::script::log(&format!(
                            "session {} repos: {:?}",
                            info.id,
                            info.repos.iter().map(|r| r.name.as_str()).collect::<Vec<_>>()
                        ));
                        let id = info.id.clone();
                        // Save closes the dialog (Desk); a detach keeps it open.
                        if close {
                            self.modal = None;
                        }
                        if let Some(l) = self.live_mut(&id) {
                            l.info = info;
                        }
                        Task::batch([self.sync_detail(), self.load_repos()])
                    }
                    Err(e) => {
                        f.error = Some(i18n::proto_error(&e));
                        Task::batch([self.refresh(), self.load_repos()])
                    }
                }
            }
            Message::PickOpen => {
                if self.online().is_none() {
                    return Task::none();
                }
                self.modal = Some(Modal::PickRoot(PickRootForm::default()));
                self.pick_go(None)
            }
            Message::PickGo(dir) => self.pick_go(dir),
            Message::PickPath(v) => {
                if let Some(Modal::PickRoot(f)) = &mut self.modal {
                    f.path = v;
                }
                Task::none()
            }
            Message::PickPathGo => {
                let Some(Modal::PickRoot(f)) = &self.modal else {
                    return Task::none();
                };
                let typed = f.path.trim().trim_matches('"').trim().to_owned();
                self.pick_go((!typed.is_empty()).then_some(typed))
            }
            Message::PickListed(seq, r) => {
                let Some(Modal::PickRoot(f)) = &mut self.modal else {
                    return Task::none();
                };
                if seq != f.seq {
                    return Task::none();
                }
                f.loading = false;
                match r {
                    Ok(l) => {
                        crate::script::log(&format!(
                            "pick {:?} (git repo {}, {} folder(s), truncated {})",
                            l.path,
                            l.is_git_repo,
                            l.entries.len(),
                            l.truncated
                        ));
                        f.error = None;
                        f.selected = None;
                        f.path = l.path.clone();
                        f.listing = Some(l);
                        pick_scroll(0.0)
                    }
                    // A typed path that cannot be listed: show the start list instead.
                    Err(_) if f.listing.is_none() && f.dir.is_some() => self.pick_go(None),
                    Err(e) => {
                        f.error = Some(i18n::proto_error(&e));
                        Task::none()
                    }
                }
            }
            Message::PickHighlight(i) => {
                if let Some(Modal::PickRoot(f)) = &mut self.modal
                    && f.listing.as_ref().is_some_and(|l| i < l.entries.len())
                {
                    f.selected = Some(i);
                }
                Task::none()
            }
            Message::PickMove(d) => {
                if let Some(Modal::PickRoot(f)) = &mut self.modal
                    && let Some(l) = &f.listing
                    && !l.entries.is_empty()
                {
                    let n = l.entries.len() as i64;
                    let i = match f.selected {
                        Some(i) => (i as i64 + i64::from(d)).clamp(0, n - 1),
                        None if d < 0 => n - 1,
                        None => 0,
                    };
                    f.selected = Some(i as usize);
                    // Keep the highlighted row in view (row i of n at the relative offset
                    // i / (n - 1) is always inside the viewport).
                    return pick_scroll(if n > 1 {
                        i as f32 / (n - 1) as f32
                    } else {
                        0.0
                    });
                }
                Task::none()
            }
            Message::PickEnter => {
                let dir = match &self.modal {
                    Some(Modal::PickRoot(f)) => f
                        .selected
                        .and_then(|i| f.listing.as_ref()?.entries.get(i))
                        .map(|e| e.path.clone()),
                    _ => None,
                };
                match dir {
                    Some(d) => self.pick_go(Some(d)),
                    None => Task::none(),
                }
            }
            Message::PickConfirm => {
                let (target, purpose) = match &self.modal {
                    Some(Modal::PickRoot(f)) => (f.target().map(str::to_owned), f.purpose.clone()),
                    _ => (None, PickPurpose::Repo),
                };
                let Some(path) = target else {
                    return Task::none();
                };
                if purpose == PickPurpose::Terminal {
                    self.modal = None;
                    return self.on_uterm(crate::uterm::UTermMsg::PickedDir(path));
                }
                if let PickPurpose::Skill(_) = purpose {
                    self.modal = None;
                    return self.on_skill(crate::skills_ui::SkillMsg::Picked(path));
                }
                crate::script::log(&format!("pick: register {path:?}"));
                self.modal = None;
                self.settings.repo_input = path;
                self.update(Message::RepoAdd)
            }
            Message::AcctProvider(p) => self.with_settings(|f| f.acct_provider = p),
            Message::AcctName(v) => self.with_settings(|f| f.acct_name = v),
            Message::AcctAdd => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let f = &mut self.settings;
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
                        self.settings.acct_name.clear();
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
                    // Shown like a new session, on the work page.
                    self.settings.busy = false;
                    self.on_created(Ok((info, Vec::new(), Vec::new())))
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
            Message::AcctIntegration(provider, slug, action) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let key = (provider.clone(), slug.clone());
                let _ = self.with_settings(|f| {
                    f.acct_integration.retain(|(k, _)| k != &key);
                    f.acct_integration.push((key, None));
                });
                Task::perform(
                    {
                        let (p, s) = (provider.clone(), slug.clone());
                        async move { c.account_integration(&p, &s, action).await }
                    },
                    move |r| Message::AcctIntegrationDone(provider.clone(), slug.clone(), r),
                )
            }
            Message::AcctIntegrationDone(provider, slug, r) => {
                crate::script::log(&format!("account integration {provider}/{slug}: {r:?}"));
                self.with_settings(|f| {
                    let key = (provider, slug);
                    f.acct_integration.retain(|(k, _)| k != &key);
                    match r {
                        Ok(i) => f.acct_integration.push((key, Some(i))),
                        Err(e) => f.error = Some(i18n::proto_error(&e)),
                    }
                })
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
                let Some((provider, slug, name)) = self.settings.acct_renaming.clone() else {
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
                let f = &mut self.settings;
                let name = f.cfg_name.trim().to_owned();
                f.busy = true;
                Task::perform(
                    async move { c.create_configuration(&name).await },
                    Message::ProfileCreated,
                )
            }
            Message::ProfileCreated(r) => {
                crate::script::log(&format!("profile created: {:?}", r.as_ref().map(|c| &c.slug)));
                self.settings.busy = false;
                match r {
                    Ok(c) => {
                        if matches!(self.modal, Some(Modal::AddProfile)) {
                            self.modal = None;
                        }
                        let f = &mut self.settings;
                        f.cfg_name.clear();
                        f.error = None;
                        f.profile_delete_armed = false;
                        // Like Desk: the new profile is shown right away.
                        f.profile_sel = Some(c.slug.clone());
                        f.cfg_edit = None;
                        if !self.configs.iter().any(|x| x.slug == c.slug) {
                            self.configs.push(c);
                        }
                        self.sync_profile_edit();
                    }
                    Err(e) => self.settings.error = Some(i18n::proto_error(&e)),
                }
                self.refresh()
            }
            Message::ProfileAdding(open) => {
                self.modal = open.then_some(Modal::AddProfile);
                let f = &mut self.settings;
                f.cfg_name.clear();
                f.error = None;
                if open {
                    operation::focus(CONFIG_NAME_ID)
                } else {
                    Task::none()
                }
            }
            Message::ProfileDeleteArm(on) => self.with_settings(|f| f.profile_delete_armed = on),
            Message::CfgEdit(slug) => {
                // Picks the profile shown on the profiles page (its editor follows).
                let f = &mut self.settings;
                if f.profile_sel.as_deref() != Some(slug.as_str()) {
                    f.profile_delete_armed = false;
                    f.error = None;
                }
                f.profile_sel = Some(slug);
                if f.cfg_edit.as_ref().is_some_and(|e| Some(&e.base.slug) != f.profile_sel.as_ref()) {
                    f.cfg_edit = None;
                }
                self.sync_profile_edit();
                self.load_profile_history()
            }
            Message::CfgEditName(v) => self.with_cfg(|e| e.name = v),
            Message::CfgInstructions(a) => self.with_cfg(|e| e.instructions.perform(a)),
            Message::Skill(m) => self.on_skill(m),
            Message::ProfileHistory(m) => self.on_profile_history(m),
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
            Message::CfgDefaultRepo(id, on) => {
                let registered: Vec<String> = self.repos.iter().map(|r| r.id.clone()).collect();
                self.with_cfg(|e| {
                    // `None` (every repo) becomes an explicit list on the first change.
                    let mut list = crate::app::default_repos_for(e.default_repos.as_ref(), &registered);
                    list.retain(|r| *r != id);
                    if on {
                        list.push(id);
                    }
                    list.sort_by_key(|r| registered.iter().position(|o| o == r));
                    e.default_repos = Some(list);
                })
            }
            Message::CfgCancel => {
                // 되돌리기: the editor starts again from the saved profile.
                self.settings.cfg_edit = None;
                self.sync_profile_edit();
                Task::none()
            }
            Message::CfgSave => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let f = &mut self.settings;
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
                let f = &mut self.settings;
                f.busy = false;
                match r {
                    Ok(()) => {
                        f.error = None;
                        match &op {
                            SettingsOp::ConfigSaved => f.cfg_edit = None,
                            SettingsOp::AccountRenamed => f.acct_renaming = None,
                            _ => {}
                        }
                    }
                    Err(e) => f.error = Some(i18n::proto_error(&e)),
                }
                if ok && let SettingsOp::ConfigDeleted(slug) = &op {
                    let f = &mut self.settings;
                    f.profile_delete_armed = false;
                    if f.profile_sel.as_deref() == Some(slug.as_str()) {
                        f.profile_sel = None;
                        f.cfg_edit = None;
                    }
                }
                if ok
                    && let SettingsOp::ConfigDeleted(slug) = &op
                    && self.ui.configuration.as_deref() == Some(slug.as_str())
                {
                    self.ui.configuration = None;
                    self.save_ui();
                }
                // The server also notifies; refresh now so the page updates without a race.
                self.refresh()
            }

            Message::SetLanguage(pref) => {
                // Every `view` reads the current language, so the switch is instant.
                self.ui.language = pref;
                let lang = meninx_i18n::apply(pref);
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
                if let Some(b) = self.shown_browser() {
                    if let Some(b) = self.browser_mut(b) {
                        b.url_input = v;
                    }
                } else if let Some(id) = self.selected.clone().map(|i| self.workspace_owner(&i))
                    && let Some(live) = self.live_mut(&id)
                {
                    live.url_input = v;
                }
                Task::none()
            }
            Message::UrlSubmit => {
                self.sync_web();
                if let Some(b) = self.shown_browser() {
                    if let Some(b) = self.browser_mut(b)
                        && let Some(url) = crate::browser::address_to_url(&b.url_input)
                        && let Some(v) = &b.view
                    {
                        v.navigate(&url);
                    }
                } else if let Some(live) = self.shown_live() {
                    let url = crate::browser::address_to_url(&live.url_input)
                        .unwrap_or_else(|| normalize_url(""));
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

            Message::Detail(sid, m) => {
                // The footer follows status / History answers (`footer.rs`).
                let hook = crate::footer::detail_hook(&m);
                let t = self.on_detail(sid.clone(), m);
                Task::batch([t, self.footer_after_detail(&sid, hook)])
            }
            Message::Footer(sid, m) => self.on_footer(sid, m),
            Message::ToggleOpenItems => {
                self.open_items = !self.open_items;
                Task::none()
            }
            Message::CloseOpenItems => {
                self.open_items = false;
                Task::none()
            }
            Message::ToggleSortMenu => {
                self.sort_open = !self.sort_open;
                Task::none()
            }
            Message::CloseSortMenu => {
                self.sort_open = false;
                Task::none()
            }
            Message::Sort(change) => {
                let list = &mut self.ui.session_sort;
                match change {
                    SortChange::Add(key) => {
                        if !list.iter().any(|c| c.key == key) {
                            list.push(SortCriterion {
                                key,
                                dir: key.default_dir(),
                            });
                        }
                    }
                    SortChange::Remove(key) => list.retain(|c| c.key != key),
                    SortChange::Flip(key) => {
                        for c in list.iter_mut().filter(|c| c.key == key) {
                            c.dir = match c.dir {
                                SortDir::Asc => SortDir::Desc,
                                SortDir::Desc => SortDir::Asc,
                            };
                        }
                    }
                    SortChange::Move(key, delta) => {
                        crate::session_sort::move_criterion(list, key, delta)
                    }
                    SortChange::Reset => list.clear(),
                }
                crate::script::log(&format!("session sort: {:?}", self.ui.session_sort));
                self.save_ui();
                Task::none()
            }
            Message::ToggleArchiveOpen(key) => {
                if !self.ui.archive_open.remove(&key) {
                    self.ui.archive_open.insert(key);
                }
                self.save_ui();
                Task::none()
            }
            Message::ToggleGroupFold(key) => {
                if !self.ui.folded_groups.remove(&key) {
                    self.ui.folded_groups.insert(key);
                }
                self.save_ui();
                Task::none()
            }
            Message::QuickOpen => {
                if self.modal.is_some() || !self.on_work_page() {
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
                    Some(Modal::QuickOpen(f)) => f
                        .results
                        .get(f.selected)
                        .map(|m| crate::detail::qualify(&m.repo, &m.path)),
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
            Message::FindInFiles if self.on_work_page() => self.open_search(),
            Message::FindInFiles => Task::none(),
            Message::DefPick(i) => {
                let Some(Modal::DefPicker(f)) = self.modal.take() else {
                    return Task::none();
                };
                match f.targets.get(i) {
                    Some(t) => self.open_file_at(
                        &f.session,
                        &crate::detail::qualify(&f.repo, &t.path),
                        t.line,
                        t.col,
                    ),
                    None => Task::none(),
                }
            }
            Message::DeleteEntryConfirm => self.delete_entry(),
            Message::CloseDirtySave | Message::CloseDirtyDiscard => {
                let Some(Modal::CloseDirty(f)) = self.modal.take() else {
                    return Task::none();
                };
                let sid = f.session.as_str();
                if matches!(message, Message::CloseDirtySave) {
                    // The unsaved files close once saved; the others now.
                    let mut tasks: Vec<Task<Message>> = f
                        .dirty
                        .iter()
                        .map(|p| self.save_file(sid, p, None, true))
                        .collect();
                    let rest: Vec<ItemKey> = f
                        .keys
                        .iter()
                        .filter(|k| !matches!(k, ItemKey::File(p) if f.dirty.contains(p)))
                        .cloned()
                        .collect();
                    tasks.push(self.close_items(sid, &rest));
                    self.persist_detail(sid);
                    return Task::batch(tasks);
                }
                for p in &f.dirty {
                    if let Some(file) = self.detail_mut(sid).and_then(|d| d.file_mut(p))
                        && let Some(v) = &mut file.view
                    {
                        let _ = v.update(meninx_views::CodeMessage::Revert);
                    }
                }
                let t = self.close_items(sid, &f.keys);
                self.persist_detail(sid);
                t
            }
            Message::Expire => {
                let now = std::time::Instant::now();
                self.notices.expire(now);
                Task::none()
            }
            Message::DividerPressed => {
                self.dragging = true;
                Task::none()
            }
            Message::SideResizePressed => {
                self.side_dragging = true;
                Task::none()
            }
            Message::ListResizePressed => {
                self.list_dragging = true;
                Task::none()
            }
            Message::DragMoved(p) => {
                if self.dragging {
                    let avail = self.split_avail();
                    self.ui.split_ratio =
                        ((p.x - self.left_w()) / avail).clamp(SPLIT_RANGE.0, SPLIT_RANGE.1);
                } else if self.side_dragging {
                    // The panel reaches the window's right edge; the pointer is on its handle.
                    self.ui.side_width = self.window_size.width - p.x - DIVIDER_W / 2.0;
                    self.ui.side_width = self.side_w();
                } else if self.scripts_drag.is_some() {
                    self.scripts_drag_to(p.y);
                } else if self.list_dragging {
                    // The list starts at the rail's right edge; the pointer is on its handle.
                    self.ui.list_width = p.x - RAIL_W - DIVIDER_W / 2.0;
                    self.ui.list_width = self.list_w();
                }
                Task::none()
            }
            Message::WindowDrag(id, p) => {
                if Some(id) == self.window {
                    return self.update_inner(Message::DragMoved(p));
                }
                Task::none()
            }
            Message::DragEnd => {
                if self.dragging
                    || self.side_dragging
                    || self.list_dragging
                    || self.scripts_drag.is_some()
                {
                    self.dragging = false;
                    self.side_dragging = false;
                    self.list_dragging = false;
                    self.scripts_drag = None;
                    self.save_ui();
                }
                Task::none()
            }

            Message::WindowOpened(id) => {
                self.window = id;
                match id {
                    Some(id) => Task::batch([
                        window::size(id).map(move |s| Message::WindowResized(id, s)),
                        crate::frame::enable_snap_layout(id),
                    ]),
                    None => Task::none(),
                }
            }
            Message::WindowResized(id, size) => {
                if Some(id) == self.detail_window {
                    return self.on_detail_window_resized(id, size);
                } else if Some(id) == self.window {
                    self.window_size = size;
                    return Task::batch([
                        crate::frame::query_maximized(id),
                        App::query_minimized(id),
                    ]);
                }
                Task::none()
            }
            Message::WindowFocus(id, on) => {
                if Some(id) == self.detail_window && !on {
                    // A moved / resized detail window is read here on Windows (it sends the
                    // app no `Moved` messages).
                    return crate::detail_window::save_placement(id);
                }
                if Some(id) == self.window {
                    self.focused = on;
                    // Looking at the selected session again: its notices are read.
                    if on
                        && self.on_work_page()
                        && let Some(sid) = self.selected.clone()
                    {
                        self.notices_seen(&sid);
                    }
                }
                Task::none()
            }
            Message::WindowMoved(id, p) => {
                if Some(id) == self.detail_window {
                    self.on_detail_window_moved(p);
                }
                Task::none()
            }
            Message::WindowCloseRequest(id) => {
                if Some(id) == self.detail_window {
                    self.on_detail_window_close()
                } else if Some(id) == self.window {
                    self.on_close_requested()
                } else {
                    Task::none()
                }
            }
            Message::WindowClosed(id) => self.on_window_closed(id),
            Message::DetailWindow(m) => self.on_detail_window(m),
            Message::Rail(m) => self.on_rail(m),
            Message::ItemMenu(m) => self.on_item_menu(m),
            Message::ItemAct(a) => self.on_item_act(a),
            Message::SessionMenu(m) => {
                self.session_menu = m;
                Task::none()
            }
            Message::SessionAct(a) => self.on_session_act(a),
            Message::Family(m) => self.on_family(m),
            Message::Handoff(m) => self.on_handoff(m),
            Message::SessionEdit(m) => self.on_session_edit(m),
            Message::ListEdit(m) => self.on_list_edit(m),
            Message::Update(m) => self.on_update(m),
            Message::OldServer(m) => self.on_old_server(m),
            Message::Notice(m) => self.on_notice(m),
            Message::SessionMenuThen(m) => {
                self.session_menu = None;
                self.update_inner(*m)
            }
            Message::ItemDrag(m) => self.on_item_drag(m),
            Message::UTerm(m) => self.on_uterm(m),
            Message::Browser(m) => self.on_browser(m),
            Message::Widget(m) => self.on_widget(m),
            Message::Preview(m) => self.on_preview(m),
            Message::Approval(m) => self.on_approval(m),
            Message::AutoAnswer(m) => self.on_auto_answer(m),
            Message::Composer(m) => self.on_composer(m),
            Message::FileDropped(p) => self.on_file_dropped(p),
            Message::OsFileDropped(p) => self.on_os_file_dropped(p),
            Message::DropHover(t, entered) => {
                // Leaving clears only that target (another one may have been entered first).
                if entered {
                    self.drop_target = Some(t);
                } else if self.drop_target == Some(t) {
                    self.drop_target = None;
                }
                Task::none()
            }
            Message::DragPointer(p) => {
                self.drag_pointer = Some(p);
                Task::none()
            }
            Message::Modifiers(m) => {
                self.modifiers = m;
                Task::none()
            }
            Message::PickTermFolder => {
                if self.online().is_none() {
                    return Task::none();
                }
                self.modal = Some(Modal::PickRoot(PickRootForm {
                    purpose: PickPurpose::Terminal,
                    ..PickRootForm::default()
                }));
                self.pick_go(self.new_dir_global.clone())
            }
            Message::ExploreSettings(open) => {
                self.explore_settings = open;
                Task::none()
            }
            Message::ExploreHideIgnored(hide) => self.set_explore_hide_ignored(hide),
            Message::SideMenuAt(p) => {
                self.side_menu_at = Some(p);
                Task::none()
            }
            Message::Frame(id, m) => self.on_frame(id, m),
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
            Message::SetFont(slot, name) => {
                self.ui.fonts.set(slot, &name);
                let name = self.ui.fonts.get(slot).to_owned();
                match slot {
                    // iced's default font is fixed at start: applied by the next start.
                    FontSlot::Ui => {}
                    FontSlot::Code => meninx_views::theme::set_code_font(&name),
                    FontSlot::Terminal => {
                        self.term_config.font_family = if name.is_empty() {
                            TermConfig::default().font_family
                        } else {
                            name
                        };
                    }
                }
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
                let f = &mut self.settings;
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
                let closed = self.close_for_reopen();
                self.connect();
                closed
            }

            Message::Tick => crate::script::tick(self),
            Message::Shot(name, shot) => {
                crate::script::save_shot(self, &name, &shot);
                Task::none()
            }
        }
    }

    /// The profile shown on the profiles page: the picked one, else the first.
    pub(crate) fn profile_shown(&self) -> Option<&ConfigurationInfo> {
        let sel = self.settings.profile_sel.as_deref();
        sel.and_then(|s| self.configs.iter().find(|c| c.slug == s))
            .or_else(|| self.configs.first())
    }

    /// Keep the editor on the shown profile: (re)created from the saved profile when there is
    /// none or it edits another one; kept (with its unsaved edits) otherwise.
    pub(crate) fn sync_profile_edit(&mut self) {
        let Some(shown) = self.profile_shown().cloned() else {
            self.settings.cfg_edit = None;
            return;
        };
        let f = &mut self.settings;
        match &mut f.cfg_edit {
            Some(e) if e.base.slug == shown.slug => {
                let b = &e.base;
                let changed = b.name != shown.name
                    || b.instructions != shown.instructions
                    || b.mcp_servers != shown.mcp_servers
                    || b.default_repos != shown.default_repos;
                if changed && !e.dirty() {
                    // Changed elsewhere (a session, another window) and nothing edited here:
                    // show the new version.
                    let skills = std::mem::take(&mut e.skills);
                    **e = ConfigEdit::new(&shown);
                    e.skills = skills;
                } else {
                    // Skills change outside the editor (applied at once): follow the list.
                    e.base.skills = shown.skills;
                    e.outdated |= changed;
                }
            }
            _ => f.cfg_edit = Some(Box::new(ConfigEdit::new(&shown))),
        }
    }

    fn with_cfg(&mut self, f: impl FnOnce(&mut ConfigEdit)) -> Task<Message> {
        if let Some(e) = &mut self.settings.cfg_edit {
            f(e);
        }
        Task::none()
    }

    fn with_settings(&mut self, f: impl FnOnce(&mut SettingsForm)) -> Task<Message> {
        f(&mut self.settings);
        Task::none()
    }

    /// Load the registered repos (repos page, new-session dialog, configuration editor).
    pub(crate) fn load_repos(&mut self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        Task::perform(async move { c.repos().await }, Message::ReposLoaded)
    }

    /// Desk's `defaultReposFor` for a configuration: its saved default repos that are still
    /// registered, or every registered repo when none were saved.
    pub fn default_repos_for(&self, configuration: &str) -> Vec<String> {
        let registered: Vec<String> = self.repos.iter().map(|r| r.id.clone()).collect();
        let saved = self
            .configs
            .iter()
            .find(|c| c.slug == configuration)
            .and_then(|c| c.default_repos.as_ref());
        default_repos_for(saved, &registered)
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
        self.exit_after_saving_detail()
    }

    /// List `dir` in the repos page's folder picker (`None` = the start list).
    pub(crate) fn pick_go(&mut self, dir: Option<String>) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(Modal::PickRoot(f)) = &mut self.modal else {
            return Task::none();
        };
        f.seq += 1;
        f.loading = true;
        f.dir = dir.clone();
        let seq = f.seq;
        Task::perform(
            async move { c.browse_pick_list(dir.as_deref()).await },
            move |r| Message::PickListed(seq, r),
        )
    }

    fn with_base(&mut self, f: impl FnOnce(&mut BaseEdit)) -> Task<Message> {
        if let Some(b) = self.settings.base.as_mut() {
            f(b);
            b.saved = false;
        }
        Task::none()
    }

    /// The repos page's base editor for the selected repo, as saved (`force`: also when it
    /// already shows that repo). Loads the refs (fixed picker) and, for a rule, its preview.
    pub(crate) fn open_base_edit(&mut self, force: bool) -> Task<Message> {
        let Some(r) = self
            .settings
            .repo_selected
            .as_deref()
            .and_then(|id| self.repos.iter().find(|r| r.id == id))
            .cloned()
        else {
            self.settings.base = None;
            return Task::none();
        };
        if !force && self.settings.base.as_ref().is_some_and(|b| b.repo == r.id) {
            return Task::none();
        }
        self.settings.base = Some(BaseEdit::new(&r));
        let Some(c) = self.online() else {
            return Task::none();
        };
        let id = r.id.clone();
        let refs = Task::perform(
            {
                let (c, id) = (c.clone(), id.clone());
                async move { c.repo_branches(&id).await }
            },
            {
                let id = id.clone();
                move |res| Message::BaseRefs(id.clone(), res)
            },
        );
        if r.base_rule.is_some() {
            Task::batch([refs, self.update(Message::BaseRuleCheck)])
        } else {
            refs
        }
    }

    /// origin's branches of every registered repo (base pickers of the new-session and session
    /// repos dialogs).
    pub(crate) fn load_origin_branches(&self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        Task::batch(self.repos.iter().map(|r| {
            let (c, id) = (c.clone(), r.id.clone());
            Task::perform(
                {
                    let id = id.clone();
                    async move { c.repo_origin_branches(&id).await }
                },
                move |res| Message::OriginBranches(id.clone(), res),
            )
        }))
    }

    fn with_new(&mut self, f: impl FnOnce(&mut NewForm)) -> Task<Message> {
        if let Some(Modal::NewSession(form)) = &mut self.modal {
            f(form);
        }
        Task::none()
    }

    /// Run `f` on the page shown in the selected session's pane (a browser tab or its web page).
    fn with_web(&mut self, f: impl FnOnce(&WebView)) -> Task<Message> {
        let view = match (self.shown_browser(), self.shown_live()) {
            (Some(id), Some(l)) => self.browser(l, id).and_then(|b| b.view.as_ref()),
            (None, Some(l)) => l.web.as_ref(),
            _ => None,
        };
        if let Some(w) = view {
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
        let (repos, original_repo) = if f.use_worktrees {
            (f.repos.clone(), None)
        } else {
            match f.original_repo.clone() {
                Some(r) => (Vec::new(), Some(r)),
                None => {
                    f.error = Some(tr::err_choose_repo());
                    return Task::none();
                }
            }
        };
        f.busy = true;
        f.error = None;
        // Desk: only bases that differ from the repo default (the default keeps following the
        // repo's rule).
        let bases = f
            .bases
            .iter()
            .filter(|(id, b)| {
                repos.contains(id)
                    && self
                        .repos
                        .iter()
                        .find(|r| &r.id == *id)
                        .is_none_or(|r| r.default_base.as_deref() != Some(b.as_str()))
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let req = NewSession {
            provider: f.provider.clone(),
            account: f.account.clone(),
            configuration: f.configuration.clone(),
            refresh_base: f.refresh_base && original_repo.is_none(),
            bases,
            // The dialog closes once the session folder exists; the worktrees follow in the
            // background (the session list shows 워크트리 생성 중 meanwhile).
            background: original_repo.is_none() && !repos.is_empty(),
            repos,
            original_repo: original_repo.clone(),
            title: f.title.clone(),
            start: true,
            cols,
            rows,
        };
        if original_repo.is_some() {
            self.ui.last_repo = original_repo;
        }
        // git runs on the server.
        Task::perform(
            async move { c.create_session_full(req).await },
            Message::NewCreated,
        )
    }

    /// What creating a session's worktrees reported (at creation, or at `SessionPrepared` when
    /// they were made in the background).
    pub(crate) fn worktree_notices(
        &mut self,
        session: &str,
        failures: &[RepoFailureInfo],
        base_changes: &[String],
    ) {
        // Desk: "일부 레포의 워크트리를 만들지 못했습니다" (the session exists anyway).
        if !failures.is_empty() {
            let detail = failures
                .iter()
                .map(|f| format!("{}: {}", f.name, i18n::proto_error(&f.error)))
                .collect::<Vec<_>>()
                .join("\n");
            crate::script::log(&format!("worktree failures: {detail}"));
            self.notify(
                Notice::error(Text::tr(tr::err_some_worktrees_failed))
                    .body(detail)
                    .session(session.to_owned())
                    .toast(),
            );
        }
        // Desk: "기준 브랜치가 규칙에 따라 바뀌었습니다" (one line per repo).
        if !base_changes.is_empty() {
            let detail = base_changes.join("\n");
            crate::script::log(&format!("base changes: {detail}"));
            self.notify(
                Notice::info(Text::tr(tr::base_changed_by_rule))
                    .body(detail)
                    .session(session.to_owned())
                    .toast(),
            );
        }
    }

    pub(crate) fn on_created(
        &mut self,
        result: Result<(SessionInfo, Vec<RepoFailureInfo>, Vec<String>), ProtoError>,
    ) -> Task<Message> {
        let (info, failures, base_changes) = match result {
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
            "created session {} {:?} mode={:?} root={} repos={:?} state={:?}",
            info.id,
            info.title,
            info.mode,
            info.root,
            info.repos
                .iter()
                .map(|r| format!("{}@{}", r.name, r.branch))
                .collect::<Vec<_>>(),
            info.state
        ));
        self.worktree_notices(&info.id, &failures, &base_changes);
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
        // A login terminal opened from the accounts page is shown on the work page.
        self.ui.page = Page::Work;
        self.save_ui();
        self.after_select()
    }

    fn on_web(&mut self, ev: WebViewEvent) {
        if self.browser_web_event(&ev) {
            return;
        }
        let show = self
            .detail_visible()
            .then(|| self.selected.clone())
            .flatten()
            .map(|id| self.workspace_owner(&id))
            .filter(|id| {
                // The page shows when it is active, or when the active item is another
                // session's terminal (the pane falls back to the page).
                self.live(id).is_some_and(|l| self.page_shown(l))
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
                if let Some(n) = crate::notice_view::load_error_notice(&url, &error) {
                    self.notices.push(n.session(live.info.id.clone()));
                }
            }
            WebEvent::NewWindowRequested(url) => {
                // Desk: a page's new window is a new browser tab of the session.
                let sid = live.info.id.clone();
                let _ = self.open_browser(Some(sid), false, url);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ subscriptions

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            meninx_web::events().map(Message::Web),
            crate::widgets_ui::subscription(),
            crate::previews_ui::subscription(),
            window::resize_events().map(|(id, s)| Message::WindowResized(id, s)),
            window::close_requests().map(Message::WindowCloseRequest),
            window::close_events().map(Message::WindowClosed),
        ];
        subs.push(event::listen_with(|e, _, id| match e {
            Event::Window(window::Event::Focused) => Some(Message::WindowFocus(id, true)),
            Event::Window(window::Event::Unfocused) => Some(Message::WindowFocus(id, false)),
            _ => None,
        }));
        // Not on Windows: there the placement is read from the OS (`detail_window.rs`), and a
        // message for each step of a drag rebuilt every window's view while it moved.
        if self.detail_window.is_some() && !cfg!(windows) {
            subs.push(event::listen_with(|e, _, id| match e {
                Event::Window(window::Event::Moved(p)) => Some(Message::WindowMoved(id, p)),
                _ => None,
            }));
        }
        if let Some(l) = &self.link {
            subs.push(
                meninx_client::iced_support::subscription(&l.client)
                    .with(self.link_gen)
                    .map(|(generation, e)| Message::Client(generation, e)),
            );
        }
        if self.dragging || self.side_dragging || self.list_dragging || self.scripts_drag.is_some() {
            // The dividers are in the app window: the detail window's pointer does not drive them.
            subs.push(event::listen_with(|e, _, id| match e {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::WindowDrag(id, position))
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::DragEnd)
                }
                _ => None,
            }));
        }
        if self.modal.is_some() {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseModal),
                _ => None,
            }));
        } else if self.session_menu.is_some() {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::SessionMenu(None)),
                _ => None,
            }));
        } else if self.item_menu.is_some() {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::ItemMenu(None)),
                _ => None,
            }));
        }
        if matches!(self.modal, Some(Modal::PickRoot(_))) {
            // Keys the focused path input handles (Enter = open the typed path) are captured
            // and skip the list's keys.
            subs.push(event::listen_with(|e, status, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(named),
                    ..
                }) if status == event::Status::Ignored => match named {
                    keyboard::key::Named::ArrowDown => Some(Message::PickMove(1)),
                    keyboard::key::Named::ArrowUp => Some(Message::PickMove(-1)),
                    keyboard::key::Named::Enter => Some(Message::PickEnter),
                    _ => None,
                },
                _ => None,
            }));
        } else if matches!(self.modal, Some(Modal::QuickOpen(_))) {
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
        } else if self.sort_open {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CloseSortMenu),
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
        if self.notices.has_toast() {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Message::Expire));
        }
        if self.notices.running() > 0 {
            // Turns the bell's and the list's loaders (`icons::spinner`).
            subs.push(iced::time::every(crate::icons::SPIN_FRAME).map(|_| Message::Expire));
        }
        subs.push(self.footer_subscription());
        subs.push(event::listen_with(|e, _, _| match e {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Modifiers(m)),
            Event::Window(window::Event::FileDropped(p)) => Some(Message::OsFileDropped(p)),
            _ => None,
        }));
        if self.file_drag.is_some() {
            subs.push(event::listen_with(|e, _, _| match e {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::DragPointer(position))
                }
                _ => None,
            }));
        }
        if self.new_menu {
            subs.push(
                iced::time::every(Duration::from_secs(3))
                    .map(|_| Message::UTerm(crate::uterm::UTermMsg::PortsTick)),
            );
        }
        if self.update_polling() {
            subs.push(
                iced::time::every(crate::self_update::POLL)
                    .map(|_| Message::Update(crate::self_update::UpdateMsg::Tick)),
            );
        }
        if self.script.as_ref().is_some_and(|s| !s.done()) {
            subs.push(iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    /// Simulates closing the app (test script): drop the client and every piece of state; the
    /// server keeps the sessions running. [`App::connect`] then acts like reopening it.
    pub fn close_for_reopen(&mut self) -> Task<Message> {
        self.save_ui();
        // The detail window closes with the app (it stays detached in `ui.json`).
        let detail_window = self
            .detail_window
            .take()
            .map(|id| crate::detail_window::save_placement(id).chain(window::close(id)));
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
        detail_window.unwrap_or_else(Task::none)
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

/// Font families installed on this machine (first, usually English, name of each face),
/// sorted and deduplicated: the Settings font pickers' choices.
fn installed_font_families() -> Vec<String> {
    let mut fs = iced::advanced::graphics::text::font_system()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    let mut names: Vec<String> = fs
        .raw()
        .db()
        .faces()
        .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .filter(|n| !n.is_empty() && !n.starts_with('.') && n != "Iced-Icons")
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::{PickRootForm, crumbs, is_fs_root, normalize_url, same_path};
    use meninx_client::{BrowseEntry, BrowseListing};

    fn c(path: &str) -> Vec<(&'static str, &'static str)> {
        crumbs(path)
            .into_iter()
            .map(|(a, b)| (&*a.leak(), &*b.leak()))
            .collect()
    }

    #[test]
    fn breadcrumbs() {
        assert!(crumbs("").is_empty());
        assert_eq!(c(r"C:\"), [("C:", r"C:\")]);
        assert_eq!(
            c(r"C:\Users\사용자"),
            [
                ("C:", r"C:\"),
                ("Users", r"C:\Users"),
                ("사용자", r"C:\Users\사용자")
            ]
        );
        assert_eq!(c("/"), [("/", "/")]);
        assert_eq!(
            c("/home/me"),
            [("/", "/"), ("home", "/home"), ("me", "/home/me")]
        );
        assert_eq!(
            c(r"\\srv\share\x"),
            [(r"\\srv\share", r"\\srv\share\"), ("x", r"\\srv\share\x")]
        );
    }

    #[test]
    fn filesystem_roots() {
        for p in [r"C:\", "C:", "d:/", "/", r"\\srv\share", r"\\srv\share\"] {
            assert!(is_fs_root(p), "{p}");
        }
        for p in [r"C:\src", "/home", r"\\srv\share\x"] {
            assert!(!is_fs_root(p), "{p}");
        }
    }

    #[test]
    fn pick_target() {
        let entry = |p: &str| BrowseEntry {
            name: p.into(),
            path: p.into(),
            is_git_repo: false,
        };
        let mut f = PickRootForm {
            listing: Some(BrowseListing {
                path: String::new(),
                parent: None,
                is_git_repo: false,
                entries: vec![entry(r"C:\Users\me"), entry(r"C:\")],
                truncated: false,
            }),
            ..PickRootForm::default()
        };
        // Start list: nothing without a highlight; a drive is refused; home is fine.
        assert_eq!(f.target(), None);
        f.selected = Some(1);
        assert_eq!(f.target(), None);
        f.selected = Some(0);
        assert_eq!(f.target(), Some(r"C:\Users\me"));
        // A folder: the listed folder itself unless an entry is highlighted.
        f.listing = Some(BrowseListing {
            path: r"D:\repository".into(),
            parent: Some(r"D:\".into()),
            is_git_repo: false,
            entries: vec![entry(r"D:\repository\a")],
            truncated: false,
        });
        f.selected = None;
        assert_eq!(f.target(), Some(r"D:\repository"));
        f.selected = Some(0);
        assert_eq!(f.target(), Some(r"D:\repository\a"));
    }

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
    }
}
