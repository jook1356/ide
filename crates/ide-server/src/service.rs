//! Services: providers, accounts, configurations, sessions and terminals of one user, behind
//! the authorization layer.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ide_agents::{AgentProvider, Detection, Providers, SessionInput};
use ide_core::{
    AccountStore, ConfigurationStore, ErrorMessage, LaunchEnv, LaunchSpec, Paths, RegisteredRepo,
    RepoId, RepoStore, Session, SessionId, SessionKind, SessionMode, SessionStore, SessionTarget,
    SessionWorktree,
};
use ide_proto::{
    AccountInfo, AccountState, ConfigurationInfo, McpServerInfo, ModeCaps, NewSession, Notify,
    BaseRuleInfo, BaseRulePreview, ProtoError, ProviderCaps, ProviderDetection, ProviderInfo,
    RepoFailureInfo, RepoInfo, Request,
    Response, SessionInfo, SessionRepo, SessionState, Support, TextSearch, UserInfo,
};
use tokio::sync::broadcast;

use crate::audit::{Audit, FileAudit};
use crate::jail::{RootJail, normalize_rel};
use crate::pty::TermHost;
use crate::runner::{Runner, SameUserRunner, SpawnSpec};
use crate::watch::Watchers;
use crate::{def, fsops, git, search};

/// Server config file in the data dir. Its legacy `browse_roots` are only read once, by the
/// migration to the repository registry (folders that are git repos get registered).
pub const SERVER_CONFIG_FILE: &str = "server.json";

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Env var naming the `gh` program ([`ServerConfig::gh`]).
pub const GH_ENV: &str = "IDE_GH";
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
    /// The GitHub CLI for the `pr.*` requests (`IDE_GH`; `None` = `gh` on `PATH`, else its
    /// usual install folders).
    pub gh: Option<PathBuf>,
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
            gh: std::env::var_os(GH_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
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
            self.config.gh.clone(),
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
    providers: Providers,
    detections: Mutex<HashMap<String, Detection>>,
    accounts: Mutex<AccountStore>,
    configs: Mutex<ConfigurationStore>,
    repos: Mutex<RepoStore>,
    sessions: Mutex<SessionStore>,
    hosts: Mutex<HashMap<String, Arc<TermHost>>>,
    events: broadcast::Sender<Notify>,
    runner: Arc<dyn Runner>,
    audit: Arc<dyn Audit>,
    launch_env: LaunchEnv,
    watchers: Watchers,
    write_lock: Mutex<()>,
    gh: Option<PathBuf>,
}

impl UserCtx {
    fn open(
        user: UserInfo,
        paths: Paths,
        runner: Arc<dyn Runner>,
        audit: Arc<dyn Audit>,
        launch_env: LaunchEnv,
        gh: Option<PathBuf>,
    ) -> Result<Self, ProtoError> {
        let report = ide_core::migrate_profiles(&paths).map_err(|e| core_error(&e))?;
        if report != Default::default() {
            tracing::info!(
                "migrated {} profile(s) to accounts/configurations, {} session(s)",
                report.profiles,
                report.sessions
            );
            audit.record(
                &user,
                "migrate.profiles",
                &format!("profiles={} sessions={}", report.profiles, report.sessions),
            );
        }
        let accounts = AccountStore::open(&paths).map_err(|e| core_error(&e))?;
        let configs = ConfigurationStore::open(&paths).map_err(|e| core_error(&e))?;
        let mut sessions = SessionStore::open(&paths).map_err(|e| core_error(&e))?;
        let mut repos = RepoStore::open(&paths).map_err(|e| core_error(&e))?;
        let roots: Vec<PathBuf> = legacy_browse_roots(&paths);
        let m = ide_core::migrate_workspaces(&paths, &mut repos, &mut sessions, &roots)
            .map_err(|e| core_error(&e))?;
        if m != Default::default() {
            tracing::info!(
                "migrated to the repository registry: {} repo(s) registered, {} session(s) moved to workspaces",
                m.repos,
                m.sessions
            );
            audit.record(
                &user,
                "migrate.workspaces",
                &format!("repos={} sessions={}", m.repos, m.sessions),
            );
        }
        // Desk: sessions made before per-session bases get today's base pinned.
        if ide_core::pin_session_bases(&mut sessions, &repos) {
            sessions.persist().map_err(|e| core_error(&e))?;
        }
        let providers = Providers::load(&paths.providers_dir());
        Ok(UserCtx {
            user,
            paths,
            providers,
            detections: Mutex::new(HashMap::new()),
            accounts: Mutex::new(accounts),
            configs: Mutex::new(configs),
            repos: Mutex::new(repos),
            sessions: Mutex::new(sessions),
            hosts: Mutex::new(HashMap::new()),
            events: broadcast::channel(4096).0,
            runner,
            audit,
            launch_env,
            watchers: Watchers::default(),
            write_lock: Mutex::new(()),
            gh,
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

    /// The jail scope of file, git, search and definition requests: the worktree of `repo`
    /// attached to session `id` of this user. Nothing else is reachable through them.
    pub fn worktree_root(
        &self,
        id: &str,
        repo: &str,
    ) -> Result<(Session, SessionWorktree, RootJail), ProtoError> {
        let s = self.resolve_session(id)?;
        let not_attached = || {
            ProtoError::new("repo-not-attached").arg("name", repo)
        };
        let rid = RepoId::parse(repo).map_err(|_| not_attached())?;
        let wt = s.worktree(&rid).cloned().ok_or_else(not_attached)?;
        let j = RootJail::new(&wt.path)?;
        Ok((s, wt, j))
    }

    /// The registered repository of a worktree (its main checkout, for base resolution).
    fn repo_path_of(&self, wt: &SessionWorktree) -> Option<PathBuf> {
        self.repos
            .lock()
            .unwrap()
            .get(&wt.repo)
            .map(|r| r.path.clone())
    }

    /// The session's base branch for a worktree (short name, `""` if none): what
    /// `git.status` compares with and 새 브랜치로 이어가기 starts from.
    pub(crate) fn session_base(&self, s: &Session, wt: &SessionWorktree) -> String {
        s.base_of(self.repos.lock().unwrap().get(&wt.repo), wt)
    }

    /// Record the branch a session worktree is now on (Desk's `setWorktreeBranch`, after
    /// 새 브랜치로 이어가기): saved, the session's instructions regenerated, `SessionsChanged`.
    pub(crate) fn set_worktree_branch(
        &self,
        s: &Session,
        wt: &SessionWorktree,
        branch: &str,
    ) -> Result<(), ProtoError> {
        let mut updated = self.resolve_session(s.id.as_str())?;
        let Some(w) = updated.worktrees.iter_mut().find(|w| w.repo == wt.repo) else {
            return Ok(());
        };
        if w.branch == branch {
            return Ok(());
        }
        w.branch = branch.to_owned();
        self.sessions
            .lock()
            .unwrap()
            .upsert(updated.clone())
            .map_err(|e| core_error(&e))?;
        self.refresh_root(&updated);
        self.notify(Notify::SessionsChanged);
        Ok(())
    }

    /// Start (or join) the watcher of a session's repo worktree for one connection.
    pub fn watch(
        &self,
        id: &str,
        repo: &str,
    ) -> Result<broadcast::Receiver<Notify>, ProtoError> {
        let (s, wt, j) = self.worktree_root(id, repo)?;
        self.watchers.acquire(
            s.id.as_str(),
            wt.repo.as_str(),
            &j,
            crate::watch::git_dir_of(j.root()),
        )
    }

    /// Release one connection's watch of a session's repo worktree.
    pub fn unwatch(&self, id: &str, repo: &str) {
        self.watchers.release(&crate::watch::key(id, repo));
    }

    /// Connections watching a session's repo worktree.
    pub fn watching(&self, id: &str, repo: &str) -> usize {
        self.watchers.watching(&crate::watch::key(id, repo))
    }

    /// Validate a text search: the worktrees to search (`q.repo`, else every attached repo, in
    /// attach order) and the compiled query.
    pub fn search_prepare(
        &self,
        q: &TextSearch,
    ) -> Result<(Vec<(String, RootJail)>, search::Prepared), ProtoError> {
        let s = self.resolve_session(&q.session)?;
        let mut roots = Vec::new();
        for wt in &s.worktrees {
            if q.repo.as_deref().is_some_and(|r| r != wt.repo.as_str()) {
                continue;
            }
            if let Ok(j) = RootJail::new(&wt.path) {
                roots.push((wt.repo.to_string(), j));
            }
        }
        if let Some(r) = &q.repo
            && roots.is_empty()
        {
            return Err(ProtoError::new("repo-not-attached").arg("name", r));
        }
        let first = roots
            .first()
            .map_or_else(|| s.root.clone(), |(_, j)| j.root().to_path_buf());
        let p = search::prepare(&first, q)?;
        Ok((roots, p))
    }

    fn audit_fs(&self, action: &str, session: &str, repo: &str, rel: &str) {
        self.audit(action, &format!("{session} {repo} {rel}"));
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
        let repos = self.repos.lock().unwrap();
        SessionInfo {
            id: s.id.to_string(),
            title: s.title.clone(),
            kind: match s.kind {
                SessionKind::Agent => ide_proto::SessionKind::Agent,
                SessionKind::Login => ide_proto::SessionKind::Login,
            },
            provider: s.provider.clone(),
            account: s.account.clone(),
            configuration: s.configuration.clone(),
            mode: match s.mode {
                SessionMode::Workspace => ide_proto::SessionMode::Workspace,
                SessionMode::OriginalRepo => ide_proto::SessionMode::OriginalRepo,
            },
            root: s.root.display().to_string(),
            repos: s
                .worktrees
                .iter()
                .map(|w| SessionRepo {
                    repo: w.repo.to_string(),
                    name: repos
                        .get(&w.repo)
                        .map_or_else(|| w.name.clone(), |r| r.name.clone()),
                    folder: w.name.clone(),
                    path: w.path.display().to_string(),
                    branch: w.branch.clone(),
                    base: s.base_of(repos.get(&w.repo), w),
                })
                .collect(),
            created_at: s.created_at,
            last_active: s.last_active,
            state: host.as_ref().map_or(SessionState::Stopped, |h| h.state()),
            term_title: host.as_ref().map(|h| h.title()).unwrap_or_default(),
            term_end: host.as_ref().map_or(0, |h| h.end()),
        }
    }

    fn repo_info(&self, r: &RegisteredRepo, sessions: &SessionStore) -> RepoInfo {
        let default_base = ide_core::repo_default_base(r);
        let base = match &default_base {
            Some(b) => b.clone(),
            None => ide_core::Repo::open(&r.path)
                .and_then(|repo| ide_core::detect_base(&repo, None))
                .map(|b| b.name)
                .unwrap_or_default(),
        };
        RepoInfo {
            id: r.id.to_string(),
            name: r.name.clone(),
            path: r.path.display().to_string(),
            base,
            sessions: sessions.count_using_repo(&r.id) as u32,
            base_branch: r.base_branch.clone(),
            base_rule: r.base_branch_rule.as_ref().map(|x| BaseRuleInfo {
                remote: x.remote.clone(),
                pattern: x.pattern.clone(),
                matches: x.matches.clone(),
            }),
            default_base,
        }
    }

    /// The registered repo `id` (cloned) and its opened git repository.
    fn repo_git(&self, id: &str) -> Result<(RegisteredRepo, ide_core::Repo), ProtoError> {
        let rid = self.parse_repo(id)?;
        let r = self
            .repos
            .lock()
            .unwrap()
            .require(&rid)
            .map_err(|e| core_error(&e))?
            .clone();
        let repo = ide_core::Repo::open(&r.path).map_err(|e| core_error(&e))?;
        Ok((r, repo))
    }

    fn parse_repo(&self, id: &str) -> Result<RepoId, ProtoError> {
        RepoId::parse(id).map_err(|e| core_error(&e))
    }

    /// Repository registry requests.
    fn handle_repos(&self, req: Request) -> Result<Response, ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        match req {
            Request::RepoList => {
                let list: Vec<RegisteredRepo> = self.repos.lock().unwrap().list().to_vec();
                let sessions = self.sessions.lock().unwrap();
                Ok(Response::Repos(
                    list.iter().map(|r| self.repo_info(r, &sessions)).collect(),
                ))
            }
            Request::RepoRegister { path, init } => {
                // Any folder the user picks (the folder picker is not limited). A git
                // repository's main checkout is registered; a plain folder only with `init`
                // (Desk: "초기화하고 추가" -> git init + first commit).
                let p = crate::jail::canonicalize(std::path::Path::new(path.trim())).map_err(|_| {
                    ProtoError::new("fs-not-found").arg("path", path.clone())
                })?;
                let r = self.repos.lock().unwrap().register(&p, init).map_err(core)?;
                self.audit(
                    "repo.register",
                    &format!("{} {}{}", r.id, r.path.display(), if init { " init" } else { "" }),
                );
                self.notify(Notify::ReposChanged);
                let sessions = self.sessions.lock().unwrap();
                Ok(Response::Repo(self.repo_info(&r, &sessions)))
            }
            Request::RepoUnregister { id } => {
                let rid = self.parse_repo(&id)?;
                let sessions = self.sessions.lock().unwrap();
                let mut repos = self.repos.lock().unwrap();
                let name = repos.require(&rid).map_err(core)?.name.clone();
                let count = sessions.count_using_repo(&rid);
                if count > 0 {
                    return Err(core(ide_core::Error::RepoInUse { name, count }));
                }
                repos.unregister(&rid).map_err(core)?;
                drop((repos, sessions));
                self.audit("repo.unregister", &id);
                self.notify(Notify::ReposChanged);
                Ok(Response::Unit)
            }
            Request::RepoSetBaseBranch { id, base } => {
                let rid = self.parse_repo(&id)?;
                let r = self
                    .repos
                    .lock()
                    .unwrap()
                    .set_base_branch(&rid, base.as_deref())
                    .map_err(core)?;
                self.audit("repo.set_base_branch", &format!("{id} {}", base.as_deref().unwrap_or("-")));
                self.notify(Notify::ReposChanged);
                let sessions = self.sessions.lock().unwrap();
                Ok(Response::Repo(self.repo_info(&r, &sessions)))
            }
            Request::RepoSetBaseRule { id, rule } => {
                let rid = self.parse_repo(&id)?;
                let rule = rule.map(|x| ide_core::BaseBranchRule {
                    remote: x.remote,
                    pattern: x.pattern,
                    matches: x.matches,
                });
                // Validated against the remote's live branches (`git ls-remote`, up to 60 s)
                // outside the registry lock, then saved.
                if let Some(rule) = &rule {
                    let (_, repo) = self.repo_git(&id)?;
                    let (res, _) = ide_core::base::resolve_rule(&repo, rule).map_err(core)?;
                    res.pick.ok_or_else(|| core(ide_core::Error::BaseRuleNoMatch))?;
                }
                let r = self
                    .repos
                    .lock()
                    .unwrap()
                    .set_base_rule(&rid, rule.as_ref())
                    .map_err(core)?;
                self.audit(
                    "repo.set_base_rule",
                    &format!("{id} {}", rule.as_ref().map_or("-", |x| x.pattern.as_str())),
                );
                self.notify(Notify::ReposChanged);
                let sessions = self.sessions.lock().unwrap();
                Ok(Response::Repo(self.repo_info(&r, &sessions)))
            }
            Request::RepoPreviewBaseRule { id, rule } => {
                let (_, repo) = self.repo_git(&id)?;
                let rule = ide_core::BaseBranchRule {
                    remote: rule.remote,
                    pattern: rule.pattern,
                    matches: rule.matches,
                };
                let (res, branches) = ide_core::base::resolve_rule(&repo, &rule).map_err(core)?;
                Ok(Response::BaseRulePreview(BaseRulePreview {
                    matched: res.matched,
                    excluded: res.excluded,
                    pick: res.pick,
                    branches,
                }))
            }
            Request::RepoBranches { id } => {
                let (_, repo) = self.repo_git(&id)?;
                let (current, branches) = ide_core::base::branch_refs(&repo).map_err(core)?;
                Ok(Response::Branches { current, branches })
            }
            Request::RepoOriginBranches { id } => {
                let (_, repo) = self.repo_git(&id)?;
                let branches = ide_core::base::origin_branches(&repo).map_err(core)?;
                Ok(Response::Branches {
                    current: None,
                    branches,
                })
            }
            Request::SessionSetBase { id, repo, base } => {
                let s = self.resolve_session(&id)?;
                let rid = self.parse_repo(&repo)?;
                let r = self.repos.lock().unwrap().require(&rid).map_err(core)?.clone();
                let mut updated = s.clone();
                ide_core::set_session_base(&mut updated, &r, base.as_deref()).map_err(core)?;
                self.sessions
                    .lock()
                    .unwrap()
                    .upsert(updated.clone())
                    .map_err(core)?;
                self.refresh_root(&updated);
                self.audit(
                    "session.set_base",
                    &format!("{id} {repo} {}", base.as_deref().unwrap_or("-")),
                );
                self.notify(Notify::SessionsChanged);
                Ok(Response::Session(self.info(&updated)))
            }
            Request::SessionAttachRepo { id, repo, base } => {
                let s = self.resolve_session(&id)?;
                let rid = self.parse_repo(&repo)?;
                let r = self.repos.lock().unwrap().require(&rid).map_err(core)?.clone();
                let mut updated = s.clone();
                let plan = ide_core::attach_repo(&mut updated, &r, base.as_deref()).map_err(core)?;
                let _ = self.repos.lock().unwrap().record_plan(
                    &rid,
                    plan.rule_resolved.as_deref(),
                    plan.fixed_upgrade.as_deref(),
                );
                self.sessions
                    .lock()
                    .unwrap()
                    .upsert(updated.clone())
                    .map_err(core)?;
                self.refresh_root(&updated);
                self.audit("session.attach_repo", &format!("{id} {repo}"));
                self.notify(Notify::SessionsChanged);
                self.notify(Notify::ReposChanged);
                Ok(Response::Session(self.info(&updated)))
            }
            Request::SessionDetachRepo { id, repo } => {
                let s = self.resolve_session(&id)?;
                let rid = self.parse_repo(&repo)?;
                let r = self.repos.lock().unwrap().get(&rid).cloned();
                self.watchers.remove_key(&crate::watch::key(&id, &repo));
                let mut updated = s.clone();
                ide_core::detach_repo(&self.paths, &mut updated, &rid, r.as_ref()).map_err(core)?;
                self.sessions
                    .lock()
                    .unwrap()
                    .upsert(updated.clone())
                    .map_err(core)?;
                self.refresh_root(&updated);
                self.audit("session.detach_repo", &format!("{id} {repo}"));
                self.notify(Notify::SessionsChanged);
                self.notify(Notify::ReposChanged);
                Ok(Response::Session(self.info(&updated)))
            }
            other => self.handle_files(other),
        }
    }

    /// Regenerate a workspace session's root files (guidance, skills) after its repos changed,
    /// so a restarted agent sees them; the running agent keeps what it read at start.
    fn refresh_root(&self, s: &Session) {
        if s.is_workspace() && let Err(e) = self.launch_spec(s) {
            tracing::warn!("session {}: regenerating its files failed: {e:?}", s.id);
        }
    }

    /// Detection of a provider (cached; `refresh` re-runs `--version`).
    fn detection(&self, p: &dyn AgentProvider, refresh: bool) -> Detection {
        if !refresh && let Some(d) = self.detections.lock().unwrap().get(p.id()) {
            return d.clone();
        }
        let d = p.detect(&self.launch_env);
        self.detections
            .lock()
            .unwrap()
            .insert(p.id().to_owned(), d.clone());
        d
    }

    fn provider_infos(&self, refresh: bool) -> Vec<ProviderInfo> {
        self.providers
            .list()
            .iter()
            .map(|p| {
                let d = self.detection(p.as_ref(), refresh);
                let c = p.capabilities();
                ProviderInfo {
                    id: p.id().to_owned(),
                    name: p.display_name().to_owned(),
                    glyph: p.glyph().to_owned(),
                    color: p.color(),
                    detection: ProviderDetection {
                        path: d.path.as_ref().map(|p| p.display().to_string()),
                        version: d.version.clone(),
                        min_version: d.min_version.clone(),
                        usable: d.usable(),
                        error: d.error.clone(),
                    },
                    caps: ProviderCaps {
                        accounts: c.accounts,
                        login: c.login,
                        skills: support(c.skills),
                        mcp: support(c.mcp),
                        instructions: support(c.instructions),
                        hooks: support(c.hooks),
                        resume: c.resume,
                        session_name: c.session_name,
                        original_repo: ModeCaps {
                            skills: support(c.original_repo.skills),
                            mcp: support(c.original_repo.mcp),
                            instructions: support(c.original_repo.instructions),
                        },
                    },
                }
            })
            .collect()
    }

    fn account_info(a: &ide_core::Account) -> AccountInfo {
        AccountInfo {
            provider: a.provider.clone(),
            slug: a.slug.clone(),
            name: a.name.clone(),
            created_at: a.created_at,
        }
    }

    fn provider(&self, id: &str) -> Result<&dyn AgentProvider, ProtoError> {
        self.providers.require(id).map_err(|e| core_error(&e))
    }

    /// Account-related requests.
    fn handle_accounts(&self, req: Request) -> Result<Response, ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        match req {
            Request::ProviderList { refresh } => {
                Ok(Response::Providers(self.provider_infos(refresh)))
            }
            Request::AccountList => Ok(Response::Accounts(
                self.accounts
                    .lock()
                    .unwrap()
                    .list()
                    .iter()
                    .map(Self::account_info)
                    .collect(),
            )),
            Request::AccountCreate { provider, name } => {
                let p = self.provider(&provider)?;
                if !p.capabilities().accounts {
                    return Err(core(ide_core::Error::ProviderNoLogin(provider)));
                }
                let a = Self::account_info(
                    self.accounts
                        .lock()
                        .unwrap()
                        .create(&provider, &name)
                        .map_err(core)?,
                );
                self.audit("account.create", &format!("{}/{}", a.provider, a.slug));
                self.notify(Notify::AccountsChanged);
                Ok(Response::Account(a))
            }
            Request::AccountRename {
                provider,
                slug,
                name,
            } => {
                let a = Self::account_info(
                    self.accounts
                        .lock()
                        .unwrap()
                        .rename(&provider, &slug, &name)
                        .map_err(core)?,
                );
                self.audit("account.rename", &format!("{provider}/{slug}"));
                self.notify(Notify::AccountsChanged);
                Ok(Response::Account(a))
            }
            Request::AccountDelete {
                provider,
                slug,
                remove_files,
            } => {
                let sessions = self.sessions.lock().unwrap();
                let mut accounts = self.accounts.lock().unwrap();
                let name = accounts
                    .require(&provider, &slug)
                    .map_err(core)?
                    .name
                    .clone();
                let count = sessions.count_using_account(&provider, &slug);
                if count > 0 {
                    return Err(core(ide_core::Error::AccountInUse { name, count }));
                }
                accounts
                    .delete(&provider, &slug, remove_files)
                    .map_err(core)?;
                drop((accounts, sessions));
                self.audit(
                    "account.delete",
                    &format!("{provider}/{slug} remove_files={remove_files}"),
                );
                self.notify(Notify::AccountsChanged);
                Ok(Response::Unit)
            }
            Request::AccountStatus { provider, slug } => {
                let p = self.provider(&provider)?;
                let acct = self
                    .accounts
                    .lock()
                    .unwrap()
                    .require(&provider, &slug)
                    .map_err(core)?
                    .clone();
                let st = match p.account_status(&acct, &self.launch_env) {
                    ide_agents::AccountStatus::LoggedIn => AccountState::LoggedIn,
                    ide_agents::AccountStatus::LoggedOut => AccountState::LoggedOut,
                    ide_agents::AccountStatus::Unknown => AccountState::Unknown,
                };
                Ok(Response::AccountState(st))
            }
            Request::AccountLogin {
                provider,
                slug,
                cols,
                rows,
            } => self
                .account_login(&provider, &slug, cols, rows)
                .map(Response::Session),
            Request::ConfigurationList => Ok(Response::Configurations(
                self.configs
                    .lock()
                    .unwrap()
                    .list()
                    .iter()
                    .map(configuration_info)
                    .collect(),
            )),
            Request::ConfigurationCreate { name } => {
                let c =
                    configuration_info(self.configs.lock().unwrap().create(&name).map_err(core)?);
                self.audit("configuration.create", &c.slug);
                self.notify(Notify::ConfigurationsChanged);
                Ok(Response::Configuration(c))
            }
            Request::ConfigurationUpdate(info) => {
                let c = configuration_info(
                    self.configs
                        .lock()
                        .unwrap()
                        .update(configuration_from_info(info))
                        .map_err(core)?,
                );
                self.audit("configuration.update", &c.slug);
                // Workspace sessions of this configuration get their root files (skills,
                // guidance) regenerated now; a running agent sees them on its next start.
                let users: Vec<Session> = self
                    .sessions
                    .lock()
                    .unwrap()
                    .list()
                    .iter()
                    .filter(|s| s.configuration == c.slug)
                    .cloned()
                    .collect();
                for s in &users {
                    self.refresh_root(s);
                }
                self.notify(Notify::ConfigurationsChanged);
                Ok(Response::Configuration(c))
            }
            Request::ConfigurationDelete { slug } => {
                let sessions = self.sessions.lock().unwrap();
                let mut configs = self.configs.lock().unwrap();
                let name = configs.require(&slug).map_err(core)?.name.clone();
                let count = sessions.count_using_configuration(&slug);
                if count > 0 {
                    return Err(core(ide_core::Error::ConfigurationInUse { name, count }));
                }
                configs.delete(&slug).map_err(core)?;
                drop((configs, sessions));
                self.audit("configuration.delete", &slug);
                self.notify(Notify::ConfigurationsChanged);
                Ok(Response::Unit)
            }
            other => self.handle_repos(other),
        }
    }

    fn account_login(
        &self,
        provider: &str,
        slug: &str,
        cols: u16,
        rows: u16,
    ) -> Result<SessionInfo, ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        let p = self.provider(provider)?;
        let acct = self
            .accounts
            .lock()
            .unwrap()
            .require(provider, slug)
            .map_err(core)?
            .clone();
        // Fail before creating anything if there is no login command / program.
        let probe_cwd = self.paths.root().to_path_buf();
        ide_agents::login_spec(p, &acct, &probe_cwd, &self.launch_env).map_err(core)?;
        let title = format!("{} · {}", p.display_name(), acct.name);
        let session =
            ide_core::create_login_session(&self.paths, provider, slug, &title).map_err(core)?;
        self.sessions
            .lock()
            .unwrap()
            .upsert(session.clone())
            .map_err(core)?;
        self.host_sized(
            session.id.as_str(),
            if cols == 0 { DEFAULT_COLS } else { cols },
            if rows == 0 { DEFAULT_ROWS } else { rows },
        );
        self.audit(
            "account.login",
            &format!("{provider}/{slug} {}", session.id),
        );
        self.notify(Notify::SessionsChanged);
        self.start(&session)?;
        Ok(self.info(&session))
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
            Request::SessionList => {
                let list: Vec<Session> = self.sessions.lock().unwrap().list().to_vec();
                Ok(Response::Sessions(
                    list.iter().map(|s| self.info(s)).collect(),
                ))
            }
            Request::SessionCreate(req) => self.session_create(req),
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
                // Watchers hold the worktree folders open: stop them before deleting.
                self.watchers.remove(s.id.as_str());
                {
                    let repos = self.repos.lock().unwrap();
                    ide_core::remove_session_files(&self.paths, &s, &repos, remove_worktree)
                        .map_err(core)?;
                }
                self.sessions.lock().unwrap().remove(&s.id).map_err(core)?;
                self.hosts.lock().unwrap().remove(s.id.as_str());
                self.notify(Notify::ReposChanged);
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
            other => self.handle_accounts(other),
        }
    }

    /// Phase 2 requests: files, folder picker, file search, git, go-to-definition. Every
    /// session request is confined to the worktree of `repo` in `session`.
    fn handle_files(&self, req: Request) -> Result<Response, ProtoError> {
        match req {
            Request::FsList {
                session,
                repo,
                dir,
                include_ignored,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let rel = normalize_rel(&dir)?;
                let badges = git::badges(j.root(), &rel);
                fsops::list(&j, &rel, include_ignored, &badges).map(Response::Entries)
            }
            Request::FsStat {
                session,
                repo,
                path,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                fsops::stat(&j, &path).map(Response::Stat)
            }
            Request::FsRead {
                session,
                repo,
                path,
                max_bytes,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                fsops::read(&j, &path, max_bytes).map(Response::File)
            }
            Request::FsWrite {
                session,
                repo,
                path,
                content,
                expected_version,
                encoding,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
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
                self.audit_fs("fs.write", &session, &repo, &normalize_rel(&path)?);
                Ok(Response::Written { version })
            }
            Request::FsCreateFile {
                session,
                repo,
                path,
                content,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let version = {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::create_file(&j, &path, &content)?
                };
                self.audit_fs("fs.create_file", &session, &repo, &normalize_rel(&path)?);
                Ok(Response::Written { version })
            }
            Request::FsCreateDir {
                session,
                repo,
                path,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                fsops::create_dir(&j, &path)?;
                self.audit_fs("fs.create_dir", &session, &repo, &normalize_rel(&path)?);
                Ok(Response::Unit)
            }
            Request::FsRename {
                session,
                repo,
                from,
                to,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::rename(&j, &from, &to)?;
                }
                let target = format!("{} -> {}", normalize_rel(&from)?, normalize_rel(&to)?);
                self.audit_fs("fs.rename", &session, &repo, &target);
                Ok(Response::Unit)
            }
            Request::FsDelete {
                session,
                repo,
                path,
                recursive,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::delete(&j, &path, recursive)?;
                }
                self.audit_fs("fs.delete", &session, &repo, &normalize_rel(&path)?);
                Ok(Response::Unit)
            }
            Request::BrowsePickList { abs_dir } => {
                fsops::pick_list(abs_dir.as_deref(), home_dir().as_deref()).map(Response::Browse)
            }
            Request::SearchFiles {
                session,
                repo,
                query,
                max_results,
            } => {
                let s = self.resolve_session(&session)?;
                let mut roots: Vec<(String, RootJail)> = Vec::new();
                for wt in &s.worktrees {
                    if repo.as_deref().is_some_and(|r| r != wt.repo.as_str()) {
                        continue;
                    }
                    if let Ok(j) = RootJail::new(&wt.path) {
                        roots.push((wt.repo.to_string(), j));
                    }
                }
                let refs: Vec<(String, &std::path::Path)> =
                    roots.iter().map(|(r, j)| (r.clone(), j.root())).collect();
                Ok(Response::FileMatches(search::files(&refs, &query, max_results)))
            }
            Request::GitStatus {
                session,
                repo,
                base,
            } => {
                let (s, wt, j) = self.worktree_root(&session, &repo)?;
                let main = self.repo_path_of(&wt);
                let base = base.or_else(|| {
                    let b = s.base_of(self.repos.lock().unwrap().get(&wt.repo), &wt);
                    (!b.is_empty()).then_some(b)
                });
                git::status(j.root(), main.as_deref(), base.as_deref()).map(Response::GitStatus)
            }
            Request::GitDiff {
                session,
                repo,
                path,
                against,
                base,
            } => {
                let (s, wt, j) = self.worktree_root(&session, &repo)?;
                let path = match path {
                    Some(p) => {
                        let n = normalize_rel(&p)?;
                        j.resolve(&n)?;
                        Some(n)
                    }
                    None => None,
                };
                let main = self.repo_path_of(&wt);
                let base = base.or_else(|| {
                    let b = s.base_of(self.repos.lock().unwrap().get(&wt.repo), &wt);
                    (!b.is_empty()).then_some(b)
                });
                git::diff(
                    j.root(),
                    main.as_deref(),
                    path.as_deref(),
                    against,
                    base.as_deref(),
                )
                .map(Response::Diff)
            }
            Request::GitFileAt {
                session,
                repo,
                path,
                rev,
                base,
            } => {
                let (s, wt, j) = self.worktree_root(&session, &repo)?;
                let rel = normalize_rel(&path)?;
                if rel.is_empty() {
                    return Err(ProtoError::new("fs-is-a-dir").arg("path", ""));
                }
                j.resolve(&rel)?;
                let main = self.repo_path_of(&wt);
                let base = base.or_else(|| {
                    let b = s.base_of(self.repos.lock().unwrap().get(&wt.repo), &wt);
                    (!b.is_empty()).then_some(b)
                });
                let blob = git::file_at(j.root(), main.as_deref(), &rel, &rev, base.as_deref())?;
                Ok(Response::FileAt(
                    blob.map(|b| fsops::content_from_bytes(&rel, &b, 0)),
                ))
            }
            Request::GitLog {
                session,
                repo,
                skip,
                limit,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                git::log(j.root(), skip, limit).map(Response::GitLog)
            }
            Request::GitUnpushed { session, repo } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                Ok(Response::GitUnpushed(git::unpushed(j.root())))
            }
            Request::GitCommitFiles { session, repo, sha } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                git::commit_files(j.root(), &sha).map(Response::GitChanges)
            }
            Request::GitCommitDiff {
                session,
                repo,
                sha,
                path,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                // Only the syntax is checked: the file may no longer exist in the worktree.
                let path = path.map(|p| normalize_rel(&p)).transpose()?;
                git::commit_diff(j.root(), &sha, path.as_deref()).map(Response::Diff)
            }
            // --- local git actions (Changes / History footer)
            req @ (Request::GitDiscard { .. }
            | Request::GitStashList { .. }
            | Request::GitStashPush { .. }
            | Request::GitStashAction { .. }
            | Request::GitCommitAction { .. }
            | Request::GitSyncStatus { .. }
            | Request::GitPush { .. }
            | Request::GitPull { .. }
            | Request::GitBranches { .. }
            | Request::GitFetchRemotes { .. }
            | Request::GitSwitchBranch { .. }
            | Request::GitCreateBranch { .. }
            | Request::GitBranchRefreshInfo { .. }
            | Request::GitBranchRefreshCheck { .. }
            | Request::GitRefreshBranch { .. }) => self.handle_git_actions(req),
            Request::DefLookup {
                session,
                repo,
                path,
                line,
                col,
                text,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                def::lookup(&j, &path, line, col, text).map(Response::Def)
            }
            // --- pull requests and agent prompts
            req @ (Request::PrStatus { .. }
            | Request::PrGate { .. }
            | Request::PrFindOpen { .. }
            | Request::PrRemoteBranches { .. }
            | Request::PrPendingWork { .. }
            | Request::PrLeftover { .. }
            | Request::PrResetFile { .. }
            | Request::PrDrift { .. }
            | Request::PrUnmerged { .. }
            | Request::PrCommitPreview { .. }
            | Request::PrCommitText { .. }
            | Request::PrCreate { .. }
            | Request::PrMerge { .. }
            | Request::PrClose { .. }
            | Request::PrUpdateBranch { .. }
            | Request::PromptCommit { .. }
            | Request::PromptPr { .. }
            | Request::SessionSendPrompt { .. }) => crate::pr::handle(self, req),
            other => Err(ProtoError::new("bad-request").arg("method", other.method())),
        }
    }

    fn session_create(&self, req: NewSession) -> Result<Response, ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        let p = self.provider(&req.provider)?;
        if let Some(slug) = &req.account {
            if !p.capabilities().accounts {
                return Err(core(ide_core::Error::AccountNotFound {
                    provider: req.provider.clone(),
                    slug: slug.clone(),
                }));
            }
            self.accounts
                .lock()
                .unwrap()
                .require(&req.provider, slug)
                .map_err(core)?;
        }
        self.configs
            .lock()
            .unwrap()
            .require(&req.configuration)
            .map_err(core)?;
        // Only registered repositories (the registry is the scope of what sessions reach).
        let repo_ids = req
            .repos
            .iter()
            .map(|r| self.parse_repo(r))
            .collect::<Result<Vec<_>, _>>()?;
        let original = req
            .original_repo
            .as_deref()
            .map(|r| self.parse_repo(r))
            .transpose()?;
        let core_req = ide_core::NewSession {
            target: SessionTarget {
                provider: req.provider.clone(),
                account: req.account.clone(),
                configuration: req.configuration.clone(),
            },
            title: req.title.clone(),
            provider_name: p.display_name().to_owned(),
            repos: repo_ids,
            original_repo: original,
            refresh_base: req.refresh_base,
            bases: req
                .bases
                .iter()
                .filter_map(|(k, v)| RepoId::parse(k).ok().map(|k| (k, v.clone())))
                .collect(),
        };
        // The program must resolve before the folder and worktrees are created. Worktrees
        // (and 기준 브랜치 최신화 fetches) run on a snapshot of the registry, not under its lock.
        let snapshot = self.repos.lock().unwrap().clone();
        let (session, failures, plans) =
            ide_core::create_session(&self.paths, &snapshot, &core_req, |_| {
                p.program_path(&self.launch_env).map(drop)
            })
            .map_err(core)?;
        // Desk: rule picks that changed are reported ("기준 브랜치가 규칙에 따라 바뀌었습니다").
        let mut base_changes = Vec::new();
        {
            let mut repos = self.repos.lock().unwrap();
            for (rid, plan) in &plans {
                let before = repos.get(rid).and_then(|r| r.rule_resolved.clone());
                let name = repos.get(rid).map(|r| r.name.clone()).unwrap_or_default();
                if let (Some(b), Some(a)) = (&before, &plan.rule_resolved)
                    && b != a
                {
                    let short = |x: &str| x.trim_start_matches("refs/remotes/").to_owned();
                    base_changes.push(format!("{name}: {} → {}", short(b), short(a)));
                }
                let _ = repos.record_plan(
                    rid,
                    plan.rule_resolved.as_deref(),
                    plan.fixed_upgrade.as_deref(),
                );
            }
        }
        self.sessions
            .lock()
            .unwrap()
            .upsert(session.clone())
            .map_err(core)?;
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
            &format!(
                "{} {} mode={:?} repos={} provider={} account={} configuration={}",
                session.id,
                session.root.display(),
                session.mode,
                session
                    .worktrees
                    .iter()
                    .map(|w| w.repo.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                session.provider,
                session.account.as_deref().unwrap_or("-"),
                session.configuration
            ),
        );
        for f in &failures {
            self.audit(
                "session.create.repo_failed",
                &format!("{} {} {}", session.id, f.repo, f.error),
            );
        }
        self.notify(Notify::SessionsChanged);
        self.notify(Notify::ReposChanged);
        if req.start {
            self.start(&session)?;
        } else {
            // Write the session folder's files now (guidance, skills), not only at start.
            self.refresh_root(&session);
        }
        Ok(Response::SessionCreated {
            base_changes,
            session: self.info(&session),
            failures: failures
                .iter()
                .map(|f| RepoFailureInfo {
                    repo: f.repo.to_string(),
                    name: f.name.clone(),
                    error: core_error(&f.error),
                })
                .collect(),
        })
    }

    /// The launch spec of a session (generates its scratch files).
    pub fn launch_spec(&self, s: &Session) -> Result<LaunchSpec, ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        let p = self.provider(&s.provider)?;
        let account = match &s.account {
            Some(slug) => Some(
                self.accounts
                    .lock()
                    .unwrap()
                    .require(&s.provider, slug)
                    .map_err(core)?
                    .clone(),
            ),
            None => None,
        };
        if s.kind == SessionKind::Login {
            let acct = account
                .ok_or_else(|| core(ide_core::Error::ProviderNoLogin(s.provider.clone())))?;
            return ide_agents::login_spec(p, &acct, &s.root, &self.launch_env).map_err(core);
        }
        let configuration = self
            .configs
            .lock()
            .unwrap()
            .require(&s.configuration)
            .map_err(core)?
            .clone();
        let repos = self.repos.lock().unwrap();
        let guidance = ide_core::session_guidance(s, &repos);
        let input = SessionInput::for_session(
            &self.paths,
            s,
            account.as_ref(),
            &configuration,
            &repos,
            guidance,
        );
        drop(repos);
        let built = ide_agents::assemble(p, &input, &self.launch_env).map_err(core)?;
        for n in &built.notes {
            tracing::warn!("session {}: {n}", s.id);
            self.audit("session.note", &format!("{} {n}", s.id));
        }
        Ok(built.spec)
    }

    fn start(&self, s: &Session) -> Result<(), ProtoError> {
        if self
            .existing_host(s.id.as_str())
            .is_some_and(|h| matches!(h.state(), SessionState::Exited(_)))
        {
            self.after_exit(s);
        }
        let spec = self.launch_spec(s)?;
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
            self.after_exit(s);
        }
    }

    /// The provider's [`ide_agents::AgentProvider::after_exit`] for a session that stopped
    /// (here or, before a restart, by itself).
    fn after_exit(&self, s: &Session) {
        let Ok(p) = self.provider(&s.provider) else {
            return;
        };
        let account = s.account.as_ref().and_then(|slug| {
            self.accounts
                .lock()
                .unwrap()
                .require(&s.provider, slug)
                .ok()
                .cloned()
        });
        let repos = self.repos.lock().unwrap();
        let ctx = SessionInput::for_session(
            &self.paths,
            s,
            None,
            &ide_core::Configuration::empty("x", "x"),
            &repos,
            None,
        )
        .ctx;
        drop(repos);
        p.after_exit(&ctx, account.as_ref());
    }
}

// --- pull requests and agent prompts
impl UserCtx {
    /// A `(session, repo)` worktree as `crate::pr` sees it (jailed like every git request).
    pub(crate) fn pr_target(
        &self,
        session: &str,
        repo: &str,
    ) -> Result<crate::pr::PrTarget, ProtoError> {
        let (s, wt, j) = self.worktree_root(session, repo)?;
        let repos = self.repos.lock().unwrap();
        let reg = repos.get(&wt.repo);
        let repo_name = reg.map_or_else(|| wt.name.clone(), |r| r.name.clone());
        Ok(crate::pr::PrTarget {
            dir: j.root().to_path_buf(),
            session_base: s.base_of(reg, &wt),
            branch: wt.branch.clone(),
            pr_file: s
                .is_workspace()
                .then(|| ide_core::pr_file_path(&s.root, &repo_name)),
            repo_name,
        })
    }

    /// The `gh` CLI (`gh-missing` when it cannot be found).
    pub(crate) fn gh_cli(&self) -> Result<crate::pr::GhCli, ProtoError> {
        Ok(crate::pr::GhCli {
            program: crate::pr::resolve_gh(self.gh.as_deref(), &self.launch_env)?,
            env: self.launch_env.clone(),
        })
    }

    /// `session.send_prompt`: submit `text` to a running session the way its provider takes
    /// a prompt ([`ide_agents::AgentProvider::prompt_input`]).
    pub(crate) fn send_prompt(&self, session: &str, text: &str) -> Result<(), ProtoError> {
        let s = self.resolve_session(session)?;
        let host = self
            .existing_host(s.id.as_str())
            .ok_or_else(|| ProtoError::new("session-not-running").arg("id", session))?;
        let input = self.provider(&s.provider)?.prompt_input();
        crate::pr::send_prompt(&host, input, text)?;
        self.audit("session.send_prompt", &format!("{session} {} bytes", text.len()));
        Ok(())
    }
}

/// The user's home dir (`USERPROFILE` / `HOME`), always a browse root.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The legacy `browse_roots` of `<data>/server.json` (only read by the registry migration).
fn legacy_browse_roots(paths: &Paths) -> Vec<PathBuf> {
    let cfg = paths.root().join(SERVER_CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(&cfg) else {
        return Vec::new();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("browse_roots")?.as_array().map(|a| {
                a.iter()
                    .filter_map(|r| r.as_str().map(PathBuf::from))
                    .collect()
            })
        })
        .unwrap_or_default()
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
        ErrorMessage::AccountNotFound { provider, slug } => {
            p.arg("provider", provider).arg("name", slug)
        }
        ErrorMessage::AccountInUse { name, count }
        | ErrorMessage::ConfigurationInUse { name, count } => {
            p.arg("name", name).arg("count", count.to_string())
        }
        ErrorMessage::ConfigurationNotFound { name }
        | ErrorMessage::LastConfiguration { name }
        | ErrorMessage::InvalidMcpName { name } => p.arg("name", name),
        ErrorMessage::ProviderNotFound { provider }
        | ErrorMessage::ProviderNoLogin { provider } => p.arg("provider", provider),
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
        ErrorMessage::RepoNotFound { id } => p.arg("id", id),
        ErrorMessage::RepoInUse { name, count } => {
            p.arg("name", name).arg("count", count.to_string())
        }
        ErrorMessage::RepoNotAttached { name } => p.arg("name", name),
        ErrorMessage::NotAWorkspaceSession { id } => p.arg("id", id),
        ErrorMessage::NotAGitFolder { path }
        | ErrorMessage::FolderNotFound { path }
        | ErrorMessage::CorruptGitDir { path } => p.arg("path", path.display().to_string()),
        ErrorMessage::GitTimeout { command, secs } => {
            p.arg("command", command).arg("secs", secs.to_string())
        }
        ErrorMessage::BaseRefreshFailed { detail }
        | ErrorMessage::BaseFetchFailed { detail }
        | ErrorMessage::RemoteListFailed { detail } => p.arg("detail", detail),
        ErrorMessage::BaseNoRemote | ErrorMessage::BaseNotBranch | ErrorMessage::BaseRuleNoMatch => p,
        ErrorMessage::InvalidBaseRule { field } => p.arg("field", field),
        ErrorMessage::RemoteNotFound { remote } => p.arg("remote", remote),
        ErrorMessage::InvalidBaseBranch { branch }
        | ErrorMessage::OriginBranchMissing { branch }
        | ErrorMessage::OriginBranchUnknown { branch } => p.arg("branch", branch),
    }
}

fn support(s: ide_agents::Support) -> Support {
    match s {
        ide_agents::Support::Native => Support::Native,
        ide_agents::Support::Fallback => Support::Fallback,
        ide_agents::Support::Unsupported => Support::Unsupported,
    }
}

fn configuration_info(c: &ide_core::Configuration) -> ConfigurationInfo {
    ConfigurationInfo {
        slug: c.slug.clone(),
        name: c.name.clone(),
        skills: c
            .skills
            .iter()
            .map(|s| s.path.display().to_string())
            .collect(),
        mcp_servers: c
            .mcp_servers
            .iter()
            .map(|m| McpServerInfo {
                name: m.name.clone(),
                command: m.command.clone(),
                args: m.args.clone(),
                env: m.env.clone(),
            })
            .collect(),
        instructions: c.instructions.clone(),
        default_repos: c
            .default_repos
            .as_ref()
            .map(|v| v.iter().map(|r| r.to_string()).collect()),
        created_at: c.created_at,
    }
}

fn configuration_from_info(i: ConfigurationInfo) -> ide_core::Configuration {
    ide_core::Configuration {
        slug: i.slug,
        name: i.name,
        skills: i
            .skills
            .into_iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .map(|s| ide_core::SkillRef {
                path: PathBuf::from(s),
            })
            .collect(),
        mcp_servers: i
            .mcp_servers
            .into_iter()
            .map(|m| ide_core::McpServer {
                name: m.name.trim().to_owned(),
                command: m.command.trim().to_owned(),
                args: m.args,
                env: m.env,
            })
            .collect(),
        instructions: i.instructions,
        default_repos: i.default_repos.map(|v| {
            v.iter()
                .filter_map(|r| ide_core::RepoId::parse(r).ok())
                .collect()
        }),
        created_at: i.created_at,
    }
}
