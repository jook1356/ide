//! Services: profiles, sessions and terminals of one user, behind the authorization layer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ide_core::{
    AgentKind, ErrorMessage, LaunchEnv, LaunchSpec, Paths, ProfileStore, Session, SessionId,
    SessionStore,
};
use ide_proto::{
    Agent, BrowseConfig, NewSession, Notify, ProfileInfo, ProtoError, Request, Response,
    SessionInfo, SessionState, TextSearch, UserInfo,
};
use tokio::sync::broadcast;

use crate::audit::{Audit, FileAudit};
use crate::jail::{PathJail, RootJail, normalize_rel};
use crate::pty::TermHost;
use crate::runner::{Runner, SameUserRunner, SpawnSpec};
use crate::watch::Watchers;
use crate::{def, fsops, git, search};

/// Server config file in the data dir: `{ "browse_roots": ["D:/src", ...] }` adds roots
/// the repository picker (and `session.create`) may use besides the home dir.
pub const SERVER_CONFIG_FILE: &str = "server.json";

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;

/// Server setup.
pub struct ServerConfig {
    /// Data dir of the served user.
    pub paths: Paths,
    /// The OS user the server runs as (and serves).
    pub user: UserInfo,
    /// Spawns all processes.
    pub runner: Arc<dyn Runner>,
    /// Audit sink.
    pub audit: Arc<dyn Audit>,
    /// Program resolution environment (`None` = the process environment).
    pub launch_env: Option<LaunchEnv>,
    /// Browse roots (`None` = the home dir plus `browse_roots` of `<data>/server.json`).
    pub browse_roots: Option<Vec<PathBuf>>,
}

impl ServerConfig {
    /// Data dir from `IDE_DATA_DIR`/platform default, current OS user, [`SameUserRunner`],
    /// [`FileAudit`] at `<data>/audit.log`.
    pub fn from_env() -> Result<Self, ProtoError> {
        let paths = Paths::from_env().map_err(|e| core_error(&e))?;
        Ok(Self::for_paths(paths))
    }

    /// Like [`ServerConfig::from_env`] with an explicit data dir.
    pub fn for_paths(paths: Paths) -> Self {
        let user = crate::user::current_user();
        let audit: Arc<dyn Audit> = Arc::new(FileAudit::new(paths.root().join("audit.log")));
        ServerConfig {
            runner: Arc::new(SameUserRunner::new(user.clone(), audit.clone())),
            audit,
            paths,
            user,
            launch_env: None,
            browse_roots: None,
        }
    }
}

/// The server: per-user contexts, resume tokens, connection bookkeeping.
pub struct Server {
    config: ServerConfig,
    instance: String,
    users: Mutex<HashMap<String, Arc<UserCtx>>>,
    tokens: Mutex<HashSet<String>>,
    connections: AtomicUsize,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Server {
    /// A server.
    pub fn new(config: ServerConfig) -> Arc<Self> {
        Arc::new(Server {
            config,
            instance: random_token(),
            users: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashSet::new()),
            connections: AtomicUsize::new(0),
            shutdown: tokio::sync::watch::channel(false).0,
        })
    }

    /// The served user's data dir.
    pub fn paths(&self) -> &Paths {
        &self.config.paths
    }

    /// The OS user this server runs as.
    pub fn user(&self) -> &UserInfo {
        &self.config.user
    }

    /// Random id of this server process (clients reset terminal offsets when it changes).
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// Authorization: the context of `user` (the identity established by the transport).
    /// Phase 1 serves only the server's own OS user.
    pub fn authorize(&self, user: &UserInfo) -> Result<Arc<UserCtx>, ProtoError> {
        if user.id != self.config.user.id {
            return Err(ProtoError::new("unauthorized"));
        }
        let mut users = self.users.lock().unwrap();
        if let Some(c) = users.get(&user.id) {
            return Ok(c.clone());
        }
        let ctx = Arc::new(UserCtx::open(
            user.clone(),
            self.config.paths.clone(),
            self.config.runner.clone(),
            self.config.audit.clone(),
            self.config
                .launch_env
                .clone()
                .unwrap_or_else(LaunchEnv::from_process),
            self.config.browse_roots.clone(),
        )?);
        users.insert(user.id.clone(), ctx.clone());
        Ok(ctx)
    }

    /// Resume bookkeeping: `(token, resumed)`.
    pub(crate) fn resume(&self, token: Option<&str>) -> (String, bool) {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(t) = token
            && tokens.contains(t)
        {
            return (t.to_owned(), true);
        }
        if tokens.len() > 10_000 {
            tokens.clear();
        }
        let t = random_token();
        tokens.insert(t.clone());
        (t, false)
    }

    pub(crate) fn connection_opened(&self) {
        self.connections.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn connection_closed(&self) {
        self.connections.fetch_sub(1, Ordering::AcqRel);
    }

    /// Open client connections.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    /// Sessions with a running process (all users).
    pub fn running_sessions(&self) -> usize {
        self.users
            .lock()
            .unwrap()
            .values()
            .map(|u| u.running_sessions())
            .sum()
    }

    /// Ask the process serving this server to exit (`server.shutdown`).
    pub fn request_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Resolves once [`Server::request_shutdown`] was called.
    pub async fn shutdown_requested(&self) {
        let mut rx = self.shutdown.subscribe();
        let _ = rx.wait_for(|s| *s).await;
    }

    /// Stop every running session (graceful, like `session.stop`). Blocking: call from a
    /// blocking thread inside the runtime.
    pub fn stop_all(&self) {
        let users: Vec<Arc<UserCtx>> = self.users.lock().unwrap().values().cloned().collect();
        for u in users {
            u.stop_all();
        }
    }

    /// Kill every session process (server exit).
    pub fn kill_all(&self) {
        for u in self.users.lock().unwrap().values() {
            for h in u.hosts.lock().unwrap().values() {
                h.kill();
            }
        }
    }
}

fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// One user's state. Every resource id in a request is resolved here (in this user's stores
/// only) before a handler runs, so a connection can only reach its own user's resources.
pub struct UserCtx {
    user: UserInfo,
    paths: Paths,
    profiles: Mutex<ProfileStore>,
    sessions: Mutex<SessionStore>,
    hosts: Mutex<HashMap<String, Arc<TermHost>>>,
    events: broadcast::Sender<Notify>,
    runner: Arc<dyn Runner>,
    audit: Arc<dyn Audit>,
    launch_env: LaunchEnv,
    browse_roots: Option<Vec<PathBuf>>,
    watchers: Watchers,
    write_lock: Mutex<()>,
}

impl UserCtx {
    fn open(
        user: UserInfo,
        paths: Paths,
        runner: Arc<dyn Runner>,
        audit: Arc<dyn Audit>,
        launch_env: LaunchEnv,
        browse_roots: Option<Vec<PathBuf>>,
    ) -> Result<Self, ProtoError> {
        let profiles = ProfileStore::open(&paths).map_err(|e| core_error(&e))?;
        let sessions = SessionStore::open(&paths).map_err(|e| core_error(&e))?;
        Ok(UserCtx {
            user,
            paths,
            profiles: Mutex::new(profiles),
            sessions: Mutex::new(sessions),
            hosts: Mutex::new(HashMap::new()),
            events: broadcast::channel(4096).0,
            runner,
            audit,
            launch_env,
            browse_roots,
            watchers: Watchers::default(),
            write_lock: Mutex::new(()),
        })
    }

    /// The user.
    pub fn user(&self) -> &UserInfo {
        &self.user
    }

    /// Subscribe to this user's notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<Notify> {
        self.events.subscribe()
    }

    fn notify(&self, n: Notify) {
        let _ = self.events.send(n);
    }

    fn audit(&self, action: &str, target: &str) {
        self.audit.record(&self.user, action, target);
    }

    /// Record an action of this user in the audit log.
    pub fn audit_action(&self, action: &str, target: &str) {
        self.audit(action, target);
    }

    fn running_sessions(&self) -> usize {
        self.hosts
            .lock()
            .unwrap()
            .values()
            .filter(|h| h.is_running())
            .count()
    }

    fn stop_all(&self) {
        let hosts: Vec<(String, Arc<TermHost>)> = self
            .hosts
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, h) in hosts {
            if h.is_running() {
                self.audit("session.stop", &id);
                let stopped = tokio::runtime::Handle::current().block_on(h.stop(STOP_TIMEOUT));
                if !stopped {
                    tracing::warn!("session {id} did not stop within {STOP_TIMEOUT:?}");
                }
            }
        }
    }

    /// Resolve a session id of this user.
    pub fn resolve_session(&self, id: &str) -> Result<Session, ProtoError> {
        let not_found = || ProtoError::new("session-not-found").arg("id", id);
        let sid = SessionId::parse(id).map_err(|_| not_found())?;
        self.sessions
            .lock()
            .unwrap()
            .get(&sid)
            .cloned()
            .ok_or_else(not_found)
    }

    /// The jail of a session's worktree (the session must belong to this user).
    pub fn session_root(&self, id: &str) -> Result<(Session, RootJail), ProtoError> {
        let s = self.resolve_session(id)?;
        let j = RootJail::new(&s.worktree_path)?;
        Ok((s, j))
    }

    /// Browse roots: the configured override, else the home dir plus `browse_roots` from
    /// `<data>/server.json` (re-read on every call). Missing folders are skipped.
    pub fn browse_jail(&self) -> PathJail {
        if let Some(r) = &self.browse_roots {
            return PathJail::new(r.iter().cloned());
        }
        let mut roots: Vec<PathBuf> = home_dir().into_iter().collect();
        roots.extend(self.read_server_config().1.into_iter().map(PathBuf::from));
        PathJail::new(roots)
    }

    /// `<data>/server.json` as a JSON object (empty if missing or invalid) and its
    /// `browse_roots` strings.
    fn read_server_config(&self) -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
        let cfg = self.paths.root().join(SERVER_CONFIG_FILE);
        let Ok(text) = std::fs::read_to_string(&cfg) else {
            return Default::default();
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => {
                let roots = map
                    .get("browse_roots")
                    .and_then(|r| r.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str().map(str::to_owned))
                    .collect();
                (map, roots)
            }
            Ok(_) => Default::default(),
            Err(e) => {
                tracing::warn!("{}: {e}", cfg.display());
                Default::default()
            }
        }
    }

    /// `browse.config`: the home dir and the extra roots saved in `server.json`.
    pub fn browse_config(&self) -> BrowseConfig {
        BrowseConfig {
            home: home_dir().map(|h| h.display().to_string()),
            roots: self.read_server_config().1,
        }
    }

    /// `browse.set_roots`: validate every root (absolute, existing folder, not a filesystem
    /// root; canonicalized, duplicates dropped) and store them in `server.json` (other keys
    /// are kept; the file is created if missing).
    pub fn set_browse_roots(&self, roots: Vec<String>) -> Result<BrowseConfig, ProtoError> {
        const MAX_ROOTS: usize = 64;
        if roots.len() > MAX_ROOTS {
            return Err(ProtoError::new("bad-request").arg("method", "browse.set_roots"));
        }
        let mut clean: Vec<String> = Vec::new();
        for r in &roots {
            let p = validate_browse_root(r)?.display().to_string();
            if !clean.contains(&p) {
                clean.push(p);
            }
        }
        let _g = self.write_lock.lock().unwrap();
        let (mut map, _) = self.read_server_config();
        map.insert(
            "browse_roots".into(),
            serde_json::Value::Array(clean.iter().cloned().map(Into::into).collect()),
        );
        let dir = self.paths.root();
        let path = dir.join(SERVER_CONFIG_FILE);
        let tmp = dir.join(format!("{SERVER_CONFIG_FILE}.tmp"));
        let io = |e: std::io::Error| {
            ProtoError::new("fs-io")
                .arg("path", path.display().to_string())
                .arg("detail", e.to_string())
        };
        let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(map))
            .map_err(|e| ProtoError::new("internal").arg("detail", e.to_string()))?;
        std::fs::create_dir_all(dir).map_err(io)?;
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
        self.audit("browse.set_roots", &clean.join(";"));
        Ok(self.browse_config())
    }

    /// Start (or join) the watcher of a session for one connection.
    pub fn watch(&self, id: &str) -> Result<broadcast::Receiver<Notify>, ProtoError> {
        let (s, j) = self.session_root(id)?;
        self.watchers
            .acquire(s.id.as_str(), &j, crate::watch::git_dir_of(j.root()))
    }

    /// Release one connection's watch of a session.
    pub fn unwatch(&self, id: &str) {
        self.watchers.release(id);
    }

    /// Connections watching a session.
    pub fn watching(&self, id: &str) -> usize {
        self.watchers.watching(id)
    }

    /// Validate a text search: the session root and the compiled query.
    pub fn search_prepare(
        &self,
        q: &TextSearch,
    ) -> Result<(RootJail, search::Prepared), ProtoError> {
        let (_, j) = self.session_root(&q.session)?;
        let p = search::prepare(j.root(), q)?;
        Ok((j, p))
    }

    fn audit_fs(&self, action: &str, session: &str, rel: &str) {
        self.audit(action, &format!("{session} {rel}"));
    }

    /// The terminal host of a resolved session (created idle on first use).
    pub fn host(&self, session: &Session) -> Arc<TermHost> {
        self.host_sized(session.id.as_str(), DEFAULT_COLS, DEFAULT_ROWS)
    }

    fn host_sized(&self, id: &str, cols: u16, rows: u16) -> Arc<TermHost> {
        self.hosts
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| TermHost::new(id, cols, rows, self.events.clone()))
            .clone()
    }

    fn existing_host(&self, id: &str) -> Option<Arc<TermHost>> {
        self.hosts.lock().unwrap().get(id).cloned()
    }

    fn info(&self, s: &Session) -> SessionInfo {
        let host = self.existing_host(s.id.as_str());
        SessionInfo {
            id: s.id.to_string(),
            title: s.title.clone(),
            profile: s.profile.clone(),
            repo_path: s.repo_path.display().to_string(),
            worktree_path: s.worktree_path.display().to_string(),
            branch: s.branch.clone(),
            agent: agent_to_proto(s.agent),
            created_at: s.created_at,
            last_active: s.last_active,
            state: host.as_ref().map_or(SessionState::Stopped, |h| h.state()),
            term_title: host.as_ref().map(|h| h.title()).unwrap_or_default(),
            term_end: host.as_ref().map_or(0, |h| h.end()),
        }
    }

    fn profile_info(p: &ide_core::Profile) -> ProfileInfo {
        ProfileInfo {
            name: p.name.clone(),
            created_at: p.created_at,
        }
    }

    /// Terminal input (fast; called inline so writes stay ordered).
    pub fn term_write(&self, session: &str, bytes: Vec<u8>) -> Result<(), ProtoError> {
        let s = self.resolve_session(session)?;
        self.existing_host(s.id.as_str())
            .ok_or_else(|| ProtoError::new("session-not-running").arg("id", session))?
            .write(bytes)
    }

    /// Terminal resize.
    pub fn term_resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), ProtoError> {
        let s = self.resolve_session(session)?;
        self.host(&s).resize(cols, rows);
        Ok(())
    }

    /// Handle a non-terminal request. Blocking (git, disk): run on a blocking thread.
    pub fn handle_blocking(&self, req: Request) -> Result<Response, ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        match req {
            Request::ProfileList => Ok(Response::Profiles(
                self.profiles
                    .lock()
                    .unwrap()
                    .list()
                    .iter()
                    .map(Self::profile_info)
                    .collect(),
            )),
            Request::ProfileCreate { name } => {
                let p =
                    Self::profile_info(self.profiles.lock().unwrap().create(&name).map_err(core)?);
                self.audit("profile.create", &name);
                self.notify(Notify::ProfilesChanged);
                Ok(Response::Profile(p))
            }
            Request::ProfileRename { old, new } => {
                let p = {
                    let mut profiles = self.profiles.lock().unwrap();
                    let p = Self::profile_info(profiles.rename(&old, &new).map_err(core)?);
                    self.sessions
                        .lock()
                        .unwrap()
                        .rename_profile(&old, &new)
                        .map_err(core)?;
                    p
                };
                self.audit("profile.rename", &format!("{old} -> {new}"));
                self.notify(Notify::ProfilesChanged);
                self.notify(Notify::SessionsChanged);
                Ok(Response::Profile(p))
            }
            Request::ProfileDelete { name, remove_files } => {
                self.profiles
                    .lock()
                    .unwrap()
                    .delete(&name, remove_files)
                    .map_err(core)?;
                self.audit("profile.delete", &name);
                self.notify(Notify::ProfilesChanged);
                Ok(Response::Unit)
            }
            Request::SessionList => {
                let list: Vec<Session> = self.sessions.lock().unwrap().list().to_vec();
                Ok(Response::Sessions(
                    list.iter().map(|s| self.info(s)).collect(),
                ))
            }
            Request::SessionCreate(req) => self.session_create(req).map(Response::Session),
            Request::SessionStart { id } => {
                let s = self.resolve_session(&id)?;
                self.start(&s)?;
                Ok(Response::Session(self.info(&s)))
            }
            Request::SessionStop { id } => {
                let s = self.resolve_session(&id)?;
                self.stop(&s);
                Ok(Response::Session(self.info(&s)))
            }
            Request::SessionRestart { id } => {
                let s = self.resolve_session(&id)?;
                self.stop(&s);
                self.start(&s)?;
                Ok(Response::Session(self.info(&s)))
            }
            Request::SessionDelete {
                id,
                remove_worktree,
            } => {
                let s = self.resolve_session(&id)?;
                self.stop(&s);
                ide_core::delete_session(
                    &self.paths,
                    &mut self.sessions.lock().unwrap(),
                    &s.id,
                    remove_worktree,
                )
                .map_err(core)?;
                self.hosts.lock().unwrap().remove(s.id.as_str());
                self.watchers.remove(s.id.as_str());
                self.audit(
                    "session.delete",
                    &format!("{id} remove_worktree={remove_worktree}"),
                );
                self.notify(Notify::SessionsChanged);
                Ok(Response::Unit)
            }
            Request::SessionRename { id, title } => {
                let s = self.resolve_session(&id)?;
                let title = title.trim().to_owned();
                self.sessions
                    .lock()
                    .unwrap()
                    .set_title(&s.id, title.clone())
                    .map_err(core)?;
                self.notify(Notify::SessionsChanged);
                let s = self.resolve_session(&id)?;
                Ok(Response::Session(self.info(&s)))
            }
            other => self.handle_files(other),
        }
    }

    /// Phase 2 requests: files, browsing, file search, git, go-to-definition.
    fn handle_files(&self, req: Request) -> Result<Response, ProtoError> {
        match req {
            Request::FsList {
                session,
                dir,
                include_ignored,
            } => {
                let (_, j) = self.session_root(&session)?;
                let rel = normalize_rel(&dir)?;
                let badges = git::badges(j.root(), &rel);
                fsops::list(&j, &rel, include_ignored, &badges).map(Response::Entries)
            }
            Request::FsStat { session, path } => {
                let (_, j) = self.session_root(&session)?;
                fsops::stat(&j, &path).map(Response::Stat)
            }
            Request::FsRead {
                session,
                path,
                max_bytes,
            } => {
                let (_, j) = self.session_root(&session)?;
                fsops::read(&j, &path, max_bytes).map(Response::File)
            }
            Request::FsWrite {
                session,
                path,
                content,
                expected_version,
                encoding,
            } => {
                let (_, j) = self.session_root(&session)?;
                let version = {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::write(
                        &j,
                        &path,
                        &content,
                        expected_version.as_deref(),
                        encoding.as_ref(),
                    )?
                };
                self.audit_fs("fs.write", &session, &normalize_rel(&path)?);
                Ok(Response::Written { version })
            }
            Request::FsCreateFile {
                session,
                path,
                content,
            } => {
                let (_, j) = self.session_root(&session)?;
                let version = {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::create_file(&j, &path, &content)?
                };
                self.audit_fs("fs.create_file", &session, &normalize_rel(&path)?);
                Ok(Response::Written { version })
            }
            Request::FsCreateDir { session, path } => {
                let (_, j) = self.session_root(&session)?;
                fsops::create_dir(&j, &path)?;
                self.audit_fs("fs.create_dir", &session, &normalize_rel(&path)?);
                Ok(Response::Unit)
            }
            Request::FsRename { session, from, to } => {
                let (_, j) = self.session_root(&session)?;
                {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::rename(&j, &from, &to)?;
                }
                let target = format!("{} -> {}", normalize_rel(&from)?, normalize_rel(&to)?);
                self.audit_fs("fs.rename", &session, &target);
                Ok(Response::Unit)
            }
            Request::FsDelete {
                session,
                path,
                recursive,
            } => {
                let (_, j) = self.session_root(&session)?;
                {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::delete(&j, &path, recursive)?;
                }
                self.audit_fs("fs.delete", &session, &normalize_rel(&path)?);
                Ok(Response::Unit)
            }
            Request::BrowseRoots => Ok(Response::Roots(
                self.browse_jail()
                    .roots()
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect(),
            )),
            Request::BrowseList { abs_dir } => {
                fsops::browse(&self.browse_jail(), &abs_dir).map(Response::Browse)
            }
            Request::BrowseConfig => Ok(Response::BrowseConfig(self.browse_config())),
            Request::BrowseSetRoots { roots } => {
                self.set_browse_roots(roots).map(Response::BrowseConfig)
            }
            Request::SearchFiles {
                session,
                query,
                max_results,
            } => {
                let (_, j) = self.session_root(&session)?;
                Ok(Response::FileMatches(search::files(
                    j.root(),
                    &query,
                    max_results,
                )))
            }
            Request::GitStatus { session, base } => {
                let (s, j) = self.session_root(&session)?;
                git::status(j.root(), Some(&s.repo_path), base.as_deref()).map(Response::GitStatus)
            }
            Request::GitDiff {
                session,
                path,
                against,
                base,
            } => {
                let (s, j) = self.session_root(&session)?;
                let path = match path {
                    Some(p) => {
                        let n = normalize_rel(&p)?;
                        j.resolve(&n)?;
                        Some(n)
                    }
                    None => None,
                };
                git::diff(
                    j.root(),
                    Some(&s.repo_path),
                    path.as_deref(),
                    against,
                    base.as_deref(),
                )
                .map(Response::Diff)
            }
            Request::GitFileAt {
                session,
                path,
                rev,
                base,
            } => {
                let (s, j) = self.session_root(&session)?;
                let rel = normalize_rel(&path)?;
                if rel.is_empty() {
                    return Err(ProtoError::new("fs-is-a-dir").arg("path", ""));
                }
                j.resolve(&rel)?;
                let blob = git::file_at(j.root(), Some(&s.repo_path), &rel, &rev, base.as_deref())?;
                Ok(Response::FileAt(
                    blob.map(|b| fsops::content_from_bytes(&rel, &b, 0)),
                ))
            }
            Request::DefLookup {
                session,
                path,
                line,
                col,
                text,
            } => {
                let (_, j) = self.session_root(&session)?;
                def::lookup(&j, &path, line, col, text).map(Response::Def)
            }
            other => Err(ProtoError::new("bad-request").arg("method", other.method())),
        }
    }

    fn session_create(&self, req: NewSession) -> Result<SessionInfo, ProtoError> {
        let profile = self
            .profiles
            .lock()
            .unwrap()
            .get(&req.profile)
            .cloned()
            .ok_or_else(|| ProtoError::new("profile-not-found").arg("name", &req.profile))?;
        // The repository must lie inside the browse roots (path jail).
        let repo_path = self.browse_jail().resolve(&req.repo_path)?;
        let core_req = ide_core::NewSession {
            repo_path,
            agent: agent_from_proto(req.agent),
            title: req.title.clone(),
            base_ref: req.base_ref.clone(),
        };
        let (session, _) =
            ide_core::create_session_with(&self.paths, &profile, &core_req, &self.launch_env)
                .map_err(|e| core_error(&e))?;
        self.sessions
            .lock()
            .unwrap()
            .upsert(session.clone())
            .map_err(|e| core_error(&e))?;
        self.host_sized(
            session.id.as_str(),
            if req.cols == 0 {
                DEFAULT_COLS
            } else {
                req.cols
            },
            if req.rows == 0 {
                DEFAULT_ROWS
            } else {
                req.rows
            },
        );
        self.audit(
            "session.create",
            &format!("{} {}", session.id, session.worktree_path.display()),
        );
        self.notify(Notify::SessionsChanged);
        if req.start {
            self.start(&session)?;
        }
        Ok(self.info(&session))
    }

    fn start(&self, s: &Session) -> Result<(), ProtoError> {
        let profile = self
            .profiles
            .lock()
            .unwrap()
            .get(&s.profile)
            .cloned()
            .ok_or_else(|| ProtoError::new("profile-not-found").arg("name", &s.profile))?;
        let spec = LaunchSpec::for_session_with(s, &profile, &self.launch_env)
            .map_err(|e| core_error(&e))?;
        let host = self.host(s);
        host.start(|cols, rows| {
            self.runner
                .spawn_pty(&self.user, &SpawnSpec::from_launch(&spec, cols, rows))
        })?;
        let _ = self.sessions.lock().unwrap().touch(&s.id);
        self.audit("session.start", s.id.as_str());
        Ok(())
    }

    fn stop(&self, s: &Session) {
        if let Some(h) = self.existing_host(s.id.as_str())
            && h.is_running()
        {
            self.audit("session.stop", s.id.as_str());
            let stopped = tokio::runtime::Handle::current().block_on(h.stop(STOP_TIMEOUT));
            if !stopped {
                tracing::warn!("session {} did not stop within {STOP_TIMEOUT:?}", s.id);
            }
        }
    }
}

/// The user's home dir (`USERPROFILE` / `HOME`), always a browse root.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// A browse root as given by the client: an absolute path (no `..`, no NUL/control
/// characters) of an existing folder that is not a filesystem root. Returns it canonical.
pub fn validate_browse_root(raw: &str) -> Result<PathBuf, ProtoError> {
    use std::path::Component;
    let s = raw.trim();
    let invalid = || ProtoError::new("path-invalid").arg("path", raw);
    if s.is_empty() || s.chars().any(char::is_control) {
        return Err(invalid());
    }
    let p = std::path::Path::new(s);
    if !p.is_absolute() {
        return Err(invalid());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ProtoError::new("path-denied").arg("path", raw));
    }
    let canon = crate::jail::canonicalize(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ProtoError::new("fs-not-found").arg("path", raw),
        std::io::ErrorKind::PermissionDenied => ProtoError::new("fs-permission").arg("path", raw),
        _ => ProtoError::new("fs-io")
            .arg("path", raw)
            .arg("detail", e.to_string()),
    })?;
    if !canon.is_dir() {
        return Err(ProtoError::new("fs-not-a-dir").arg("path", raw));
    }
    // A whole drive (`C:\`, `/`, `\\server\share`) would make the jail meaningless.
    let normal = canon
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();
    if normal == 0 {
        return Err(ProtoError::new("path-denied").arg("path", raw));
    }
    Ok(canon)
}

/// Map an `ide-core` error to a wire error (same ids, named args).
pub fn core_error(e: &ide_core::Error) -> ProtoError {
    let m = e.i18n();
    let p = ProtoError::new(m.id());
    match m {
        ErrorMessage::Io {
            action,
            path,
            detail,
        } => p
            .arg("action", action.id())
            .arg("path", path.display().to_string())
            .arg("detail", detail),
        ErrorMessage::Json { path, detail } => p
            .arg("path", path.display().to_string())
            .arg("detail", detail),
        ErrorMessage::NoDataDir => p,
        ErrorMessage::InvalidName { name, reason } => {
            p.arg("name", name).arg("reason", reason.id())
        }
        ErrorMessage::ProfileExists { name }
        | ErrorMessage::ProfileNotFound { name }
        | ErrorMessage::LastProfile { name } => p.arg("name", name),
        ErrorMessage::SessionNotFound { id } | ErrorMessage::InvalidSessionId { id } => {
            p.arg("id", id)
        }
        ErrorMessage::NotAGitRepo { path, detail } => p
            .arg("path", path.display().to_string())
            .arg("detail", detail),
        ErrorMessage::GitSpawn { detail } => p.arg("detail", detail),
        ErrorMessage::Git {
            args,
            status,
            detail,
        } => p
            .arg("args", args)
            .arg("status", status)
            .arg("detail", detail),
        ErrorMessage::WorktreeExists { path } => p.arg("path", path.display().to_string()),
        ErrorMessage::ProgramNotFound { program } => p.arg("program", program),
    }
}

fn agent_to_proto(a: AgentKind) -> Agent {
    match a {
        AgentKind::Claude => Agent::Claude,
        AgentKind::Codex => Agent::Codex,
        AgentKind::Shell => Agent::Shell,
    }
}

fn agent_from_proto(a: Agent) -> AgentKind {
    match a {
        Agent::Claude => AgentKind::Claude,
        Agent::Codex => AgentKind::Codex,
        Agent::Shell => AgentKind::Shell,
    }
}
