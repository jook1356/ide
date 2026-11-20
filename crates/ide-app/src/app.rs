//! App state, messages and update logic.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::futures::channel::oneshot;
use iced::widget::operation;
use iced::{Event, Point, Size, Subscription, Task, event, keyboard, mouse, window};
use ide_core::{
    AgentKind, LaunchSpec, NewSession, Paths, ProfileStore, Session, SessionId, SessionStore,
    worktree_exists,
};
use ide_i18n::LanguagePref;
use ide_term::{TermConfig, TermEvent, TermEventKind, TermSession, TermSpec, ViewEvent};
use ide_web::{WebEvent, WebRuntime, WebView, WebViewEvent};

use crate::i18n::{self, tr};
use crate::script::Script;
use crate::ui_state::{SPLIT_RANGE, UiState};

/// Width of the left sidebar in logical pixels.
pub const SIDEBAR_W: f32 = 264.0;
/// Width of the draggable divider between centre and detail.
pub const DIVIDER_W: f32 = 5.0;
/// URL of a new session's web tab.
pub const DEFAULT_URL: &str = "about:blank";

pub const TERM_ID: &str = "ide-term";
pub const RENAME_ID: &str = "ide-rename";
pub const NEW_REPO_ID: &str = "ide-new-repo";
pub const URL_ID: &str = "ide-url";
pub const PROFILE_NAME_ID: &str = "ide-profile-name";

/// Process state of a session as shown in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Not started in this app run (after restart) or stopped.
    Stopped,
    /// Spawned, no output yet.
    Starting,
    /// Output seen, process alive.
    Running,
    /// The process exited.
    Exited(Option<u32>),
    /// Spawning failed (see `Live::error`).
    Failed,
}

impl RunState {
    pub fn is_alive(self) -> bool {
        matches!(self, RunState::Starting | RunState::Running)
    }
}

/// Runtime part of a session (the persisted part lives in the `SessionStore`).
pub struct Live {
    pub id: SessionId,
    pub term: Option<TermSession>,
    pub state: RunState,
    pub term_title: String,
    pub error: Option<String>,
    pub web: Option<WebView>,
    pub url_input: String,
    pub web_title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
}

impl Live {
    fn new(id: SessionId, url: String) -> Self {
        Self {
            id,
            term: None,
            state: RunState::Stopped,
            term_title: String::new(),
            error: None,
            web: None,
            url_input: url,
            web_title: String::new(),
            loading: false,
            can_back: false,
            can_forward: false,
        }
    }
}

pub struct NewForm {
    pub repo: String,
    pub profile: String,
    pub agent: AgentKind,
    pub title: String,
    pub base_ref: String,
    pub error: Option<String>,
    pub busy: bool,
}

pub struct DeleteForm {
    pub id: SessionId,
    pub remove_worktree: bool,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Default)]
pub struct SettingsForm {
    pub new_name: String,
    pub renaming: Option<(String, String)>,
    pub error: Option<String>,
}

pub enum Modal {
    NewSession(NewForm),
    Delete(DeleteForm),
    Settings(SettingsForm),
}

pub type Created = Result<(Session, LaunchSpec), String>;

#[derive(Debug, Clone)]
pub enum Message {
    Term(TermEvent),
    TermView(ViewEvent),
    Web(WebViewEvent),

    Select(SessionId),
    Start(SessionId),
    Stop(SessionId),
    StartRename(SessionId),
    RenameInput(String),
    RenameCommit,

    AskDelete(SessionId),
    DeleteWorktree(bool),
    DeleteConfirm,
    /// Process stopped and (optionally) worktree + branch removed; `Err` = removal failed.
    DeleteReady(SessionId, Result<(), String>),

    OpenNew,
    NewRepo(String),
    NewBrowse,
    NewBrowsed(Option<PathBuf>),
    NewProfile(String),
    NewAgent(AgentKind),
    NewTitle(String),
    NewBase(String),
    NewSubmit,
    NewCreated(Box<Created>),
    CloseModal,

    ProfilePicked(String),
    OpenSettings,
    SettingsNewName(String),
    SettingsCreate,
    SettingsStartRename(String),
    SettingsRenameInput(String),
    SettingsRenameCommit,
    SettingsDelete(String),
    SetLanguage(LanguagePref),

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

pub struct App {
    pub paths: Paths,
    pub runtime: WebRuntime,
    pub profiles: ProfileStore,
    pub store: SessionStore,
    pub sessions: Vec<Live>,
    pub selected: Option<SessionId>,
    pub ui: UiState,
    pub term_config: TermConfig,
    pub window: Option<window::Id>,
    pub window_size: Size,
    pub dragging: bool,
    pub modal: Option<Modal>,
    pub rename: Option<(SessionId, String)>,
    /// Problems outside a modal (persistence errors, ...), shown in the sidebar.
    pub banner: Option<String>,
    pub script: Option<Script>,
}

impl App {
    pub fn boot(paths: Paths, runtime: WebRuntime) -> (Self, Task<Message>) {
        let mut app = Self::load(paths, runtime);
        app.script = Script::from_env(&app.paths);
        let task = Task::batch([
            window::latest().map(Message::WindowOpened),
            app.after_select(),
        ]);
        (app, task)
    }

    /// Build the app state from the files in the data dir (no process is spawned).
    pub fn load(paths: Paths, runtime: WebRuntime) -> Self {
        let mut banner = None;
        let mut ui = UiState::load(paths.root());
        ide_i18n::apply(ui.language);
        let profiles = match ProfileStore::open(&paths) {
            Ok(p) => p,
            Err(e) => panic!("cannot open profiles in {}: {e}", paths.root().display()),
        };
        let store = SessionStore::open(&paths).unwrap_or_else(|e| {
            banner = Some(tr::err_sessions_unreadable(&i18n::error(&e)));
            // Do not overwrite the unreadable file: use a store rooted elsewhere.
            SessionStore::open(&Paths::new(paths.root().join("recovered")))
                .expect("empty session store")
        });
        if ui
            .profile
            .as_deref()
            .and_then(|p| profiles.get(p))
            .is_none()
        {
            ui.profile = profiles.list().first().map(|p| p.name.clone());
        }
        let sessions: Vec<Live> = store
            .list()
            .iter()
            .map(|s| {
                let url = ui
                    .web_urls
                    .get(s.id.as_str())
                    .cloned()
                    .unwrap_or_else(|| DEFAULT_URL.into());
                Live::new(s.id.clone(), url)
            })
            .collect();
        let selected = ui
            .selected
            .as_deref()
            .and_then(|id| sessions.iter().find(|l| l.id.as_str() == id))
            .map(|l| l.id.clone());
        Self {
            paths,
            runtime,
            profiles,
            store,
            sessions,
            selected,
            ui,
            term_config: TermConfig::default(),
            window: None,
            window_size: Size::new(1400.0, 860.0),
            dragging: false,
            modal: None,
            rename: None,
            banner,
            script: None,
        }
    }

    pub fn theme(&self) -> iced::Theme {
        crate::style::theme()
    }

    pub fn title(&self) -> String {
        "ide".into()
    }

    // ------------------------------------------------------------------ lookups

    pub fn live(&self, id: &SessionId) -> Option<&Live> {
        self.sessions.iter().find(|l| &l.id == id)
    }

    fn live_mut(&mut self, id: &SessionId) -> Option<&mut Live> {
        self.sessions.iter_mut().find(|l| &l.id == id)
    }

    pub fn session(&self, id: &SessionId) -> Option<&Session> {
        self.store.get(id)
    }

    pub fn selected_live(&self) -> Option<&Live> {
        self.selected.as_ref().and_then(|id| self.live(id))
    }

    pub fn current_profile(&self) -> String {
        self.ui
            .profile
            .clone()
            .unwrap_or_else(|| ide_core::DEFAULT_PROFILE.to_owned())
    }

    fn running_in_profile(&self, profile: &str) -> usize {
        self.store
            .list()
            .iter()
            .filter(|s| s.profile == profile)
            .filter(|s| self.live(&s.id).is_some_and(|l| l.state.is_alive()))
            .count()
    }

    pub fn sessions_in_profile(&self, profile: &str) -> (usize, usize) {
        let total = self
            .store
            .list()
            .iter()
            .filter(|s| s.profile == profile)
            .count();
        (total, self.running_in_profile(profile))
    }

    fn save_ui(&mut self) {
        self.ui.selected = self.selected.as_ref().map(|s| s.to_string());
        self.ui.save(self.paths.root());
    }

    // ------------------------------------------------------------------ processes

    fn term_spec(spec: &LaunchSpec) -> TermSpec {
        let mut t = TermSpec::new(spec.program.display().to_string()).cwd(&spec.cwd);
        for a in &spec.args {
            t = t.arg(a);
        }
        for (k, v) in &spec.env {
            t = t.env(k, v);
        }
        t
    }

    /// Spawn `spec` as the process of session `id` (replacing a previous one).
    fn spawn_with(&mut self, id: &SessionId, spec: &LaunchSpec) -> Result<(), String> {
        tracing::info!(
            session = %id,
            program = %spec.program.display(),
            args = ?spec.args,
            cwd = %spec.cwd.display(),
            env = ?spec.env,
            "spawning"
        );
        crate::script::log(&format!(
            "spawn session {id}: program={} args={:?} cwd={} env={:?}",
            spec.program.display(),
            spec.args,
            spec.cwd.display(),
            spec.env
        ));
        let result = TermSession::spawn(&Self::term_spec(spec), &self.term_config);
        let live = self
            .live_mut(id)
            .ok_or_else(|| tr::err_session_not_found(id.as_str()))?;
        if let Some(old) = live.term.take() {
            old.kill();
        }
        match result {
            Ok(term) => {
                live.term = Some(term);
                live.state = RunState::Starting;
                live.term_title.clear();
                live.error = None;
                let _ = self.store.touch(id);
                Ok(())
            }
            Err(e) => {
                let msg = tr::err_start_process(&format!("{e:#}"));
                live.state = RunState::Failed;
                live.error = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// (Re)start a stored session in its existing worktree.
    fn start(&mut self, id: &SessionId) -> Result<(), String> {
        let session = self
            .session(id)
            .cloned()
            .ok_or_else(|| tr::err_session_not_found(id.as_str()))?;
        let result = (|| {
            if !worktree_exists(&session.worktree_path) {
                return Err(tr::err_worktree_missing(
                    &session.worktree_path.display().to_string(),
                ));
            }
            let profile = self
                .profiles
                .get(&session.profile)
                .ok_or_else(|| tr::err_profile_gone(&session.profile))?;
            LaunchSpec::for_session(&session, profile).map_err(|e| i18n::error(&e))
        })();
        match result {
            Ok(spec) => self.spawn_with(id, &spec),
            Err(msg) => {
                if let Some(live) = self.live_mut(id) {
                    live.state = RunState::Failed;
                    live.error = Some(msg.clone());
                }
                Err(msg)
            }
        }
    }

    pub fn kill_all(&mut self) {
        for live in &self.sessions {
            if let Some(t) = &live.term {
                t.kill();
            }
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
                web.set_hidden(show.as_ref() != Some(&live.id));
            }
        }
    }

    fn after_select(&mut self) -> Task<Message> {
        self.sync_web();
        if self.selected_live().is_some_and(|l| l.term.is_some()) && self.modal.is_none() {
            operation::focus(TERM_ID)
        } else {
            Task::none()
        }
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Term(ev) => self.on_term(ev),
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
                let _ = self.start(&id);
                self.selected = Some(id);
                self.save_ui();
                self.after_select()
            }
            Message::Stop(id) => {
                if let Some(t) = self.live(&id).and_then(|l| l.term.as_ref()) {
                    t.kill();
                }
                Task::none()
            }
            Message::StartRename(id) => {
                let title = self
                    .session(&id)
                    .map(|s| s.title.clone())
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
                if let Some((id, title)) = self.rename.take() {
                    let title = title.trim();
                    if !title.is_empty()
                        && let Err(e) = self.store.set_title(&id, title)
                    {
                        self.banner = Some(tr::err_rename_failed(&i18n::error(&e)));
                    }
                }
                self.after_select()
            }

            Message::AskDelete(id) => {
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
                let Some(Modal::Delete(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let id = f.id.clone();
                let remove = f.remove_worktree;
                // Kill the PTY first and wait (off the UI thread) until the process is gone, so
                // it no longer holds the worktree directory; then remove worktree + branch there
                // too (git can be slow).
                let term = self.live(&id).and_then(|l| l.term.clone());
                if let Some(t) = &term {
                    t.kill();
                }
                let session = self.session(&id).cloned();
                let paths = self.paths.clone();
                let (tx, rx) = oneshot::channel::<Result<(), String>>();
                std::thread::spawn(move || {
                    if let Some(t) = term {
                        let t0 = Instant::now();
                        while t.is_running() && t0.elapsed() < Duration::from_secs(5) {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        // Let ConPTY release the console's processes.
                        std::thread::sleep(Duration::from_millis(150));
                    }
                    let result = match (&session, remove) {
                        (Some(s), true) => {
                            ide_core::remove_session_files(&paths, s).map_err(|e| i18n::error(&e))
                        }
                        _ => Ok(()),
                    };
                    let _ = tx.send(result);
                });
                Task::perform(rx, move |r| {
                    Message::DeleteReady(
                        id.clone(),
                        r.unwrap_or_else(|_| Err(tr::err_delete_cancelled())),
                    )
                })
            }
            Message::DeleteReady(id, removed) => {
                let remove = match &self.modal {
                    Some(Modal::Delete(f)) if f.id == id => f.remove_worktree,
                    _ => return Task::none(),
                };
                // Files are gone (or kept); only the store entry is left to remove.
                let result = removed.and_then(|()| {
                    ide_core::delete_session(&self.paths, &mut self.store, &id, false)
                        .map_err(|e| i18n::error(&e))
                });
                match result {
                    Ok(s) => {
                        crate::script::log(&format!(
                            "deleted session {id} (remove_worktree={remove}, worktree {})",
                            s.worktree_path.display()
                        ));
                        if let Some(idx) = self.sessions.iter().position(|l| l.id == id) {
                            let live = self.sessions.remove(idx);
                            if let Some(w) = live.web {
                                w.close();
                            }
                        }
                        self.ui.web_urls.remove(id.as_str());
                        if self.selected.as_ref() == Some(&id) {
                            let profile = self.current_profile();
                            self.selected = self
                                .store
                                .list()
                                .iter()
                                .find(|s| s.profile == profile)
                                .map(|s| s.id.clone());
                        }
                        self.modal = None;
                        self.save_ui();
                        self.after_select()
                    }
                    Err(e) => {
                        if let Some(Modal::Delete(f)) = &mut self.modal {
                            f.busy = false;
                            f.error = Some(e);
                        }
                        Task::none()
                    }
                }
            }

            Message::OpenNew => {
                let profile = self.current_profile();
                self.modal = Some(Modal::NewSession(NewForm {
                    repo: self.ui.last_repo.clone().unwrap_or_default(),
                    profile,
                    agent: AgentKind::Claude,
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
            Message::NewCreated(result) => self.on_created(*result),
            Message::CloseModal => {
                // Escape also cancels an inline rename.
                if self.rename.take().is_some() && self.modal.is_none() {
                    return self.after_select();
                }
                if matches!(&self.modal, Some(Modal::Delete(f)) if f.busy) {
                    return Task::none();
                }
                self.modal = None;
                self.after_select()
            }

            Message::ProfilePicked(p) => {
                if self.profiles.get(&p).is_some() {
                    self.ui.profile = Some(p.clone());
                    if self
                        .selected
                        .as_ref()
                        .and_then(|id| self.session(id))
                        .map(|s| &s.profile)
                        != Some(&p)
                    {
                        self.selected = self
                            .store
                            .list()
                            .iter()
                            .find(|s| s.profile == p)
                            .map(|s| s.id.clone());
                    }
                    self.save_ui();
                }
                self.after_select()
            }
            Message::OpenSettings => {
                self.modal = Some(Modal::Settings(SettingsForm::default()));
                operation::focus(PROFILE_NAME_ID)
            }
            Message::SettingsNewName(v) => {
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    f.new_name = v;
                }
                Task::none()
            }
            Message::SettingsCreate => {
                let Some(Modal::Settings(f)) = &mut self.modal else {
                    return Task::none();
                };
                let name = f.new_name.trim().to_owned();
                match self.profiles.create(&name) {
                    Ok(_) => {
                        f.new_name.clear();
                        f.error = None;
                    }
                    Err(e) => f.error = Some(i18n::error(&e)),
                }
                Task::none()
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
                let result = self.rename_profile(&old, &new);
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    match result {
                        Ok(()) => {
                            f.renaming = None;
                            f.error = None;
                        }
                        Err(e) => f.error = Some(e),
                    }
                }
                Task::none()
            }
            Message::SettingsDelete(name) => {
                let (total, _) = self.sessions_in_profile(&name);
                let result = if total > 0 {
                    Err(tr::err_profile_in_use(i18n::count(total), &name))
                } else {
                    self.profiles
                        .delete(&name, false)
                        .map(drop)
                        .map_err(|e| i18n::error(&e))
                };
                if result.is_ok() && self.ui.profile.as_deref() == Some(name.as_str()) {
                    self.ui.profile = self.profiles.list().first().map(|p| p.name.clone());
                    self.save_ui();
                }
                if let Some(Modal::Settings(f)) = &mut self.modal {
                    f.error = result.err();
                }
                Task::none()
            }

            Message::SetLanguage(pref) => {
                // Every `view` reads the current language, so the switch is instant.
                self.ui.language = pref;
                let lang = ide_i18n::apply(pref);
                crate::script::log(&format!("language {pref:?} -> {}", lang.code()));
                self.save_ui();
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
            Message::CloseRequested => {
                self.save_ui();
                self.kill_all();
                iced::exit()
            }

            Message::Tick => crate::script::tick(self),
            Message::Shot(name, shot) => {
                crate::script::save_shot(self, &name, &shot);
                Task::none()
            }
        }
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

    fn rename_profile(&mut self, old: &str, new: &str) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        let running = self.running_in_profile(old);
        if running > 0 {
            return Err(tr::err_profile_running(i18n::count(running), old));
        }
        self.profiles
            .rename(old, new)
            .map_err(|e| i18n::error(&e))?;
        self.store
            .rename_profile(old, new)
            .map_err(|e| i18n::error(&e))?;
        if self.ui.profile.as_deref() == Some(old) {
            self.ui.profile = Some(new.to_owned());
            self.save_ui();
        }
        Ok(())
    }

    fn submit_new(&mut self) -> Task<Message> {
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
        let Some(profile) = self.profiles.get(&f.profile).cloned() else {
            f.error = Some(tr::err_profile_not_found(&f.profile));
            return Task::none();
        };
        f.busy = true;
        f.error = None;
        let req = NewSession {
            repo_path: PathBuf::from(&repo),
            agent: f.agent,
            title: f.title.clone(),
            base_ref: f.base_ref.clone(),
        };
        self.ui.last_repo = Some(repo);
        let paths = self.paths.clone();
        let (tx, rx) = oneshot::channel::<Created>();
        // git runs off the UI thread.
        std::thread::spawn(move || {
            let r = ide_core::create_session(&paths, &profile, &req).map_err(|e| i18n::error(&e));
            let _ = tx.send(r);
        });
        Task::perform(rx, |r| {
            Message::NewCreated(Box::new(
                r.unwrap_or_else(|_| Err(tr::err_create_cancelled())),
            ))
        })
    }

    fn on_created(&mut self, result: Created) -> Task<Message> {
        let (session, spec) = match result {
            Ok(v) => v,
            Err(e) => {
                crate::script::log(&format!("new session error: {e}"));
                if let Some(Modal::NewSession(f)) = &mut self.modal {
                    f.busy = false;
                    f.error = Some(e);
                }
                return Task::none();
            }
        };
        if let Err(e) = self.store.upsert(session.clone()) {
            if let Some(Modal::NewSession(f)) = &mut self.modal {
                f.busy = false;
                f.error = Some(tr::err_session_save(&i18n::error(&e)));
            }
            return Task::none();
        }
        crate::script::log(&format!(
            "created session {} {:?} worktree={} branch={}",
            session.id,
            session.title,
            session.worktree_path.display(),
            session.branch
        ));
        self.sessions
            .push(Live::new(session.id.clone(), DEFAULT_URL.into()));
        let _ = self.spawn_with(&session.id, &spec);
        self.ui.profile = Some(session.profile.clone());
        self.selected = Some(session.id);
        self.modal = None;
        self.save_ui();
        self.after_select()
    }

    fn on_term(&mut self, ev: TermEvent) -> Task<Message> {
        let Some(live) = self
            .sessions
            .iter_mut()
            .find(|l| l.term.as_ref().is_some_and(|t| t.id() == ev.id))
        else {
            return Task::none();
        };
        if let Some(t) = &live.term {
            t.ack();
        }
        match ev.kind {
            TermEventKind::Output => {
                if live.state == RunState::Starting {
                    live.state = RunState::Running;
                }
            }
            TermEventKind::Title(t) => live.term_title = t,
            TermEventKind::Bell => {}
            TermEventKind::Exited(code) => {
                crate::script::log(&format!("session {} exited ({code:?})", live.id));
                live.state = RunState::Exited(code);
            }
        }
        Task::none()
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
                    w.set_hidden(show.as_ref() != Some(&live.id));
                }
            }
            WebEvent::UrlChanged(u) => {
                live.url_input = u.clone();
                self.ui.web_urls.insert(live.id.to_string(), u);
                self.ui.save(self.paths.root());
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
        for live in &self.sessions {
            if let Some(t) = &live.term {
                subs.push(t.subscription().map(Message::Term));
            }
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

    /// Simulates an app restart for the test script: stop every process, drop all state and
    /// rebuild it from the files on disk.
    pub fn reload_from_disk(&mut self) {
        self.save_ui();
        self.kill_all();
        let t0 = Instant::now();
        while self
            .sessions
            .iter()
            .any(|l| l.term.as_ref().is_some_and(|t| t.is_running()))
            && t0.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        for live in &self.sessions {
            if let Some(w) = &live.web {
                w.close();
            }
        }
        let window = self.window;
        let size = self.window_size;
        let script = self.script.take();
        *self = App::load(self.paths.clone(), self.runtime.clone());
        self.window = window;
        self.window_size = size;
        self.script = script;
        self.sync_web();
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

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn urls() {
        assert_eq!(normalize_url(""), "about:blank");
        assert_eq!(normalize_url(" example.com "), "https://example.com");
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("http://x"), "http://x");
        assert_eq!(normalize_url("data:text/html,hi"), "data:text/html,hi");
        assert_eq!(normalize_url("about:blank"), "about:blank");
    }
}
