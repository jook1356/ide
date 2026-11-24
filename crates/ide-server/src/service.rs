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
    RepoFailureInfo, RepoInfo, Request, SkillInfo,
    Response, SessionInfo, SessionRepo, SessionState, Support, TextSearch, UserInfo, UserTerm,
};
use tokio::sync::broadcast;

use crate::audit::{Audit, FileAudit};
use crate::jail::{RootJail, normalize_rel};
use crate::pty::TermHost;
use crate::runner::{Runner, SameUserRunner, SpawnSpec};
use crate::watch::Watchers;
use crate::{def, fsops, git, scripts, search};

/// Server config file in the data dir. Its legacy `browse_roots` are only read once, by the
/// migration to the repository registry (folders that are git repos get registered).
pub const SERVER_CONFIG_FILE: &str = "server.json";

mod approvals;
mod branch_badges;
mod auto_answers;
pub(crate) mod browser;
mod terminals;
mod widgets;
mod write_guard;
mod previews;

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an agent CLI must wait at its input (and no dialog of it be auto-answered)
/// before a queued prompt is typed in: start dialogs come one after the other, with a blank
/// screen between them (`UserCtx::input_settled`).
const INPUT_SETTLE: Duration = Duration::from_millis(1500);
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
    /// The GitHub CLI, the `pr.*` requests' fallback for hosts git's credential store has no
    /// token for (`IDE_GH`; `None` = `gh` on `PATH`, else its usual install folders).
    pub gh: Option<PathBuf>,
    /// API root used for every GitHub host instead of `https://api.github.com` /
    /// `https://<host>/api/v3` (`IDE_GITHUB_API`; tests point it at a local fake GitHub).
    pub github_api: Option<String>,
    /// Extra environment of `git credential fill` (tests isolate git's config with
    /// `GIT_CONFIG_GLOBAL` / `GIT_CONFIG_NOSYSTEM` so the user's credential store is never read).
    pub git_credential_env: Vec<(String, String)>,
    /// The agent bridge's socket and program (`docs/agent-bridge.md`); set by the daemon,
    /// `None` = sessions get no bridge.
    pub bridge: Option<crate::bridge::BridgeConfig>,
    /// Check the sessions' branch badges in the background while a client is connected
    /// (`service/branch_badges.rs`; `IDE_BRANCH_BADGES=0` turns it off, e.g. for tests
    /// that script every git / GitHub call).
    pub branch_badges: bool,
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
            github_api: std::env::var(crate::pr::API_ENV)
                .ok()
                .filter(|v| !v.is_empty()),
            git_credential_env: Vec::new(),
            bridge: None,
            branch_badges: std::env::var(BRANCH_BADGES_ENV).map_or(true, |v| v != "0"),
        }
    }
}

/// `0` turns the background branch badge checks off (`ServerConfig::branch_badges`).
pub const BRANCH_BADGES_ENV: &str = "IDE_BRANCH_BADGES";

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
            GithubSetup {
                gh: self.config.gh.clone(),
                credentials: Arc::new(crate::pr::Credentials::new(
                    self.config.git_credential_env.clone(),
                )),
                http: Arc::new(crate::pr::HttpApi::new(self.config.github_api.clone())),
            },
            self.config.bridge.clone(),
        )?);
        let _ = ctx.this.set(Arc::downgrade(&ctx));
        users.insert(user.id.clone(), ctx.clone());
        crate::bridge::spawn_screen_watch(Arc::downgrade(&ctx));
        spawn_resume_preparing(Arc::downgrade(&ctx));
        if self.config.branch_badges {
            branch_badges::spawn_branch_badges(Arc::downgrade(&ctx));
        }
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

/// Create the worktrees a server that went away left half-way. Sessions that were running
/// then ([`Session::autostart`]) are not restarted here: a client resumes each one when the
/// user opens it (`SessionInfo::resume_on_open`), so a server start does not launch every
/// agent CLI at once.
fn spawn_resume_preparing(ctx: std::sync::Weak<UserCtx>) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        if let Some(c) = ctx.upgrade() {
            c.resume_preparing();
        }
    });
}

pub(crate) fn random_token() -> String {
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
    /// Sessions' user terminals (`uterm.*`).
    uterms: Mutex<UTerms>,
    events: broadcast::Sender<Notify>,
    runner: Arc<dyn Runner>,
    audit: Arc<dyn Audit>,
    launch_env: LaunchEnv,
    watchers: Watchers,
    write_lock: Mutex<()>,
    github: GithubSetup,
    /// The agent bridge: sessions' tokens, app calls in flight.
    bridge: crate::bridge::BridgeState,
    /// What the user allowed agents to do beyond their session (`docs/approvals.md`).
    grants: Mutex<ide_core::approval::Grants>,
    /// The user's auto answer choices per agent (`docs/auto-answers.md`) and when each
    /// session's dialog was last answered.
    auto_answers: Mutex<ide_core::auto_answer::AutoAnswers>,
    answered: Mutex<HashMap<String, std::time::Instant>>,
    /// Since when each running agent session's screen shows its CLI at its input
    /// ([`ide_agents::screen::ScreenManifest::at_input`]); queued prompts wait for it.
    at_input: Mutex<HashMap<String, std::time::Instant>>,
    /// This context itself (set once by [`Server::authorize`]), for tasks that outlive a call.
    this: std::sync::OnceLock<std::sync::Weak<UserCtx>>,
    /// Sessions whose pending worktrees a background job is creating
    /// ([`UserCtx::spawn_prepare`]).
    preparing: Mutex<HashSet<String>>,
    /// The session list's branch badges, kept across restarts (`branch_badges.rs`).
    branch_badges: branch_badges::BranchBadges,
}

/// How a user context reaches GitHub: credential-token cache + HTTPS client, `gh` fallback.
struct GithubSetup {
    gh: Option<PathBuf>,
    credentials: Arc<crate::pr::Credentials>,
    http: Arc<crate::pr::HttpApi>,
}

impl UserCtx {
    fn open(
        user: UserInfo,
        paths: Paths,
        runner: Arc<dyn Runner>,
        audit: Arc<dyn Audit>,
        launch_env: LaunchEnv,
        github: GithubSetup,
        bridge: Option<crate::bridge::BridgeConfig>,
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
        let grants = ide_core::approval::Grants::load(&paths);
        let auto_answers = ide_core::auto_answer::AutoAnswers::load(&paths);
        let branch_badges =
            branch_badges::BranchBadges::load(paths.root().join("branch-badges.json"));
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
            uterms: Mutex::new(UTerms::default()),
            events: broadcast::channel(4096).0,
            runner,
            audit,
            launch_env,
            watchers: Watchers::default(),
            write_lock: Mutex::new(()),
            github,
            bridge: crate::bridge::BridgeState::new(bridge),
            grants: Mutex::new(grants),
            auto_answers: Mutex::new(auto_answers),
            answered: Mutex::new(HashMap::new()),
            at_input: Mutex::new(HashMap::new()),
            this:std::sync::OnceLock::new(),
            preparing: Mutex::new(HashSet::new()),
            branch_badges,
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
        // Edits and git changes make the session's branch badge stale.
        if let Notify::FsChanged { session, .. } | Notify::GitChanged { session, .. } = &n {
            self.branch_badges.mark(session);
        }
        let _ = self.events.send(n);
    }

    /// Connections subscribed to this user's notifications (the apps; agent bridge
    /// connections do not subscribe).
    pub fn subscriber_count(&self) -> usize {
        self.events.receiver_count()
    }

    /// Agent bridge: does `token` belong to `session`?
    pub fn agent_check(&self, session: &str, token: &str) -> bool {
        self.bridge.check(session, token)
    }

    /// The family tools of the agent bridge (`docs/session-family.md`), answered here;
    /// `None` = not a family tool. Blocking (git, the session store).
    fn family_tool(&self, session: &str, tool: &str, args: &serde_json::Value) -> Option<ide_proto::AgentToolResult> {
        use crate::bridge::{TOOL_CREATE_CHILD, TOOL_LIST_RELATED, TOOL_MERGE_CHILD, TOOL_SEND, TOOL_SYNC_PARENT};
        let ok = |v: serde_json::Value| ide_proto::AgentToolResult {
            text: serde_json::to_string_pretty(&v).expect("json"),
            is_error: false,
            image: None,
        };
        let err = |t: String| ide_proto::AgentToolResult { text: t, is_error: true, image: None };
        let s = match self.resolve_session(session) {
            Ok(s) => s,
            Err(e) => return Some(err(format!("session not available: {e}"))),
        };
        let all = self.sessions.lock().unwrap().list().to_vec();
        let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
        Some(match tool {
            TOOL_LIST_RELATED => {
                let repos = self.repos.lock().unwrap().clone();
                let rows: Vec<serde_json::Value> = ide_core::family::related(&all, &s)
                    .iter()
                    .map(|x| {
                        let relation = if s.parent.as_ref() == Some(&x.id) {
                            "parent"
                        } else if x.parent.as_ref() == Some(&s.id) {
                            "child"
                        } else {
                            "sibling"
                        };
                        let info = self.info(x);
                        serde_json::json!({
                            "id": x.id.to_string(),
                            "title": x.title,
                            "relation": relation,
                            "shared_folder": x.shared,
                            "running": info.state == SessionState::Running,
                            "activity": format!("{:?}", info.activity),
                            // Messages (a child's first prompt too) not typed in yet.
                            "queued_messages": self.bridge.pending(x.id.as_str()),
                            "repositories": x.worktrees.iter().map(|w| serde_json::json!({
                                "name": repos.get(&w.repo).map_or_else(|| w.name.clone(), |r| r.name.clone()),
                                "branch": w.branch,
                                "folder": w.path,
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                ok(serde_json::json!({ "sessions": rows }))
            }
            TOOL_CREATE_CHILD => {
                let shared = args.get("shared").and_then(|v| v.as_bool()).unwrap_or(false);
                let repos = self.repos.lock().unwrap().clone();
                let mut ids = Vec::new();
                for want in args
                    .get("repos")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str())
                {
                    let found = s.worktrees.iter().find(|w| {
                        w.name.eq_ignore_ascii_case(want)
                            || repos.get(&w.repo).is_some_and(|r| r.name.eq_ignore_ascii_case(want))
                    });
                    match found {
                        Some(w) => ids.push(w.repo.to_string()),
                        None => return Some(err(format!("this session has no repository `{want}`"))),
                    }
                }
                let prompt = text("prompt");
                let first = Some(prompt.as_str()).filter(|p| !p.is_empty());
                match self.create_child(session, shared, &ids, &text("title"), Some(first), (0, 0)) {
                    Ok((Response::SessionCreated { session: c, failures, .. }, in_args)) => {
                        // On its command line the CLI submits it itself after its start
                        // dialogs; otherwise it is typed in once the CLI is at its input.
                        if first.is_some() && !in_args {
                            self.bridge.enqueue(&c.id, Some(s.id.as_str()), prompt.clone());
                            self.deliver_pending(&c.id);
                        }
                        ok(serde_json::json!({
                            "id": c.id,
                            "title": c.title,
                            "shared_folder": c.shared,
                            "folder": c.root,
                            "repositories": c.repos.iter().map(|r| serde_json::json!({
                                "name": r.name, "branch": r.branch, "base": r.base, "folder": r.path,
                            })).collect::<Vec<_>>(),
                            "failed_repositories": failures.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
                            "first_prompt": match (first, in_args) {
                                (None, _) => "none",
                                (Some(_), true) => "passed on its command line: its agent starts on it",
                                (Some(_), false) => "queued until it is ready",
                            },
                        }))
                    }
                    Ok(_) => err("unexpected answer".into()),
                    Err(e) => err(format!("could not create the child session: {e}")),
                }
            }
            TOOL_SEND => {
                // `steer` / `interrupt` were approved by the user before (`agent_call`).
                let message = text("message");
                if message.is_empty() {
                    return Some(err("`message` is required".into()));
                }
                let mode = send_mode(args).map_err(err);
                let mode = match mode {
                    Ok(m) => m,
                    Err(e) => return Some(e),
                };
                let target = match self.related_target(&s, &text("session")) {
                    Ok(t) => t,
                    Err(e) => return Some(err(e)),
                };
                let tid = target.id.as_str();
                // A session that was running when the server went away and has not been
                // opened since (`SessionInfo::resume_on_open`) is woken by the message: its
                // conversation resumes and the message is typed in once it is ready. So is a
                // stopped child of the sender (its CLI ended, e.g. it exited at a start dialog):
                // the parent hands it work and nobody else would start it again.
                let mut woken = false;
                if (target.autostart || target.parent.as_ref() == Some(&s.id))
                    && target.kind == SessionKind::Agent
                    && !self.existing_host(tid).is_some_and(|h| h.is_running())
                {
                    if let Err(e) = self.start(&target) {
                        let _ = self.sessions.lock().unwrap().set_autostart(&target.id, false);
                        return Some(err(format!("could not wake session `{tid}`: {e}")));
                    }
                    self.audit("session.wake", &format!("{} -> {tid}", s.id));
                    woken = true;
                }
                let Some(host) = self.existing_host(tid).filter(|h| h.is_running()) else {
                    return Some(err(format!("session `{tid}` is not running")));
                };
                let provider = match self.provider(&target.provider) {
                    Ok(p) => p,
                    Err(e) => return Some(err(e.to_string())),
                };
                let full = format!(
                    "[ide] Message from session \"{}\" ({}):\n{message}",
                    s.title, s.id
                );
                let busy = matches!(
                    self.bridge.activity(tid),
                    ide_proto::AgentActivity::Working | ide_proto::AgentActivity::NeedsInput
                );
                let mut note = None;
                match mode {
                    "steer" if busy => {
                        if !provider.steers() {
                            return Some(err(format!(
                                "{}'s agent cannot take messages during its turn: use `queue` or `interrupt`",
                                target.title
                            )));
                        }
                        match self.send_prompt(tid, &full) {
                            Ok(()) => {
                                return Some(ok(serde_json::json!({
                                    "delivered": true,
                                    "note": "typed in during its turn: its agent sees it while working",
                                })));
                            }
                            // A dialog of the CLI is open: wait like `queue`.
                            Err(e) if e.id == "prompt-not-ready" => {
                                note = Some("it could not take it now (waiting for the user?): queued instead");
                            }
                            Err(e) => return Some(err(format!("could not type it in: {e}"))),
                        }
                    }
                    "interrupt" if busy => {
                        let Some(keys) = provider.interrupt_keys() else {
                            return Some(err(format!(
                                "ide cannot interrupt {}'s agent: use `queue`",
                                target.title
                            )));
                        };
                        if let Err(e) = host.write(keys.to_vec()) {
                            return Some(err(format!("could not interrupt it: {e}")));
                        }
                        self.audit("agent.interrupt", &format!("{} -> {tid}", s.id));
                        note = Some("its turn was interrupted: the message is typed in when its agent is back at its input");
                    }
                    _ => {}
                }
                let id = self.bridge.enqueue(tid, Some(s.id.as_str()), full);
                self.deliver_pending(tid);
                let queued = self.bridge.pending(tid) > 0;
                if woken {
                    note = Some("it was not running (not opened since the server restarted): it was started and resumes its conversation; the message is typed in when it is ready (see ide_sent_messages)");
                }
                ok(serde_json::json!({
                    "delivered": !queued,
                    "message_id": id,
                    "note": note.unwrap_or(if queued {
                        "queued: delivered when its current turn ends (see ide_sent_messages)"
                    } else {
                        "typed into its terminal"
                    }),
                }))
            }
            crate::bridge::TOOL_READ_SESSION => {
                let target = match self.related_target(&s, &text("session")) {
                    Ok(t) => t,
                    Err(e) => return Some(err(e)),
                };
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30)
                    .clamp(1, 200) as usize;
                let Some(conversation) = target.conversation.clone() else {
                    return Some(err(format!(
                        "{} has no recorded conversation yet (its agent has not started, or its CLI does not report one)",
                        target.title
                    )));
                };
                let provider = match self.provider(&target.provider) {
                    Ok(p) => p,
                    Err(e) => return Some(err(e.to_string())),
                };
                let account = target.account.as_ref().and_then(|slug| {
                    self.accounts
                        .lock()
                        .unwrap()
                        .require(&target.provider, slug)
                        .ok()
                        .cloned()
                });
                match provider.transcript(account.as_ref(), &conversation, limit) {
                    None => err(format!(
                        "ide cannot read {}'s conversations",
                        provider.display_name()
                    )),
                    Some(Err(e)) => err(format!("could not read its conversation: {e}")),
                    Some(Ok(entries)) => ok(serde_json::json!({
                        "session": target.id.to_string(),
                        "title": target.title,
                        "activity": format!("{:?}", self.bridge.activity(target.id.as_str())),
                        "entries": entries.iter().map(|e| serde_json::json!({
                            "role": e.role, "text": e.text, "at": e.at,
                        })).collect::<Vec<_>>(),
                    })),
                }
            }
            crate::bridge::TOOL_SENT_MESSAGES => {
                let title = |id: &str| {
                    all.iter()
                        .find(|x| x.id.as_str() == id)
                        .map_or_else(String::new, |x| x.title.clone())
                };
                match text("action").as_str() {
                    "" | "list" => ok(serde_json::json!({
                        "waiting": self.bridge.sent_by(s.id.as_str()).iter().map(|(to, m)| {
                            let preview: String = m.text.chars().take(200).collect();
                            serde_json::json!({
                                "id": m.id,
                                "to": to,
                                "to_title": title(to),
                                "text": preview,
                                "queued_at": m.at,
                            })
                        }).collect::<Vec<_>>(),
                    })),
                    "cancel" => {
                        let Some(id) = args.get("id").and_then(|v| v.as_u64()) else {
                            return Some(err("`id` is required".into()));
                        };
                        if self.bridge.cancel(s.id.as_str(), id) {
                            ok(serde_json::json!({ "cancelled": id }))
                        } else {
                            err(format!("no waiting message {id} of this session (already typed in?)"))
                        }
                    }
                    other => err(format!("unknown action `{other}` (list, cancel)")),
                }
            }
            TOOL_MERGE_CHILD => {
                let child = text("child");
                let squash = args.get("squash").and_then(|v| v.as_bool()).unwrap_or(false);
                if !all.iter().any(|x| x.id.as_str() == child && x.parent.as_ref() == Some(&s.id)) {
                    return Some(err(format!("`{child}` is not a child of this session")));
                }
                match self.merge_child(&child, squash) {
                    Ok(r) => ok(serde_json::json!({ "merges": merges_json(&r) })),
                    Err(e) => err(format!("merge failed: {e}")),
                }
            }
            crate::bridge::TOOL_REQUEST_WRITE | crate::bridge::TOOL_RELEASE_WRITE => {
                if !self.shares_folder(&s) {
                    return Some(ok(serde_json::json!({
                        "result": "this folder is not shared: you may edit freely"
                    })));
                }
                let owner = self.folder_owner(&s);
                let holder = self.bridge.writer(owner.as_str());
                if tool == crate::bridge::TOOL_RELEASE_WRITE {
                    if holder != s.id.as_str() {
                        return Some(ok(serde_json::json!({ "result": "you do not hold it" })));
                    }
                    self.bridge.set_writer(owner.as_str(), owner.as_str());
                    self.notify(Notify::SessionsChanged);
                    return Some(ok(serde_json::json!({ "result": "released to the folder's owner" })));
                }
                if holder == s.id.as_str() {
                    return Some(ok(serde_json::json!({ "result": "you already hold it" })));
                }
                let busy = self.existing_host(&holder).is_some_and(|h| h.is_running())
                    && matches!(
                        self.bridge.activity(&holder),
                        ide_proto::AgentActivity::Working | ide_proto::AgentActivity::NeedsInput
                    );
                if busy {
                    return Some(err(format!(
                        "session `{holder}` holds the write access and is working: ask it with \
                         ide_send_to_session, or try again when it is idle"
                    )));
                }
                self.bridge.set_writer(owner.as_str(), s.id.as_str());
                self.audit("agent.write_access", &format!("{} from {holder}", s.id));
                self.notify(Notify::SessionsChanged);
                ok(serde_json::json!({ "result": "granted: you may edit files in the shared folder" }))
            }
            TOOL_SYNC_PARENT => {
                if s.parent.is_none() {
                    return Some(err("this session has no parent".into()));
                }
                match self.sync_parent(session) {
                    Ok(r) => ok(serde_json::json!({ "merges": merges_json(&r) })),
                    Err(e) => err(format!("sync failed: {e}")),
                }
            }
            _ => return None,
        })
    }

    /// The related session `to` of `s` (parent, child, sibling), or why not.
    fn related_target(&self, s: &Session, to: &str) -> Result<Session, String> {
        let all = self.sessions.lock().unwrap().list().to_vec();
        ide_core::family::related(&all, s)
            .into_iter()
            .find(|x| x.id.as_str() == to)
            .cloned()
            .ok_or_else(|| format!("`{to}` is not a related session (see ide_list_related_sessions)"))
    }

    /// `ide_send_to_session` `steer` / `interrupt`: the user's approval (`docs/approvals.md`),
    /// asked before the family tool runs. `Ok` for other tools and modes.
    async fn approve_family(self: &Arc<Self>, session: &str, tool: &str, args: &serde_json::Value) -> Result<(), String> {
        if tool != crate::bridge::TOOL_SEND {
            return Ok(());
        }
        let capability = match send_mode(args)? {
            "steer" => approvals::CAP_STEER,
            "interrupt" => approvals::CAP_INTERRUPT,
            _ => return Ok(()),
        };
        let s = self.resolve_session(session).map_err(|e| e.to_string())?;
        let to = args.get("session").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
        let target = self.related_target(&s, &to)?;
        // Not in a turn: both act like `queue`, nothing to ask.
        if !matches!(
            self.bridge.activity(target.id.as_str()),
            ide_proto::AgentActivity::Working | ide_proto::AgentActivity::NeedsInput
        ) {
            return Ok(());
        }
        let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let detail: String = message.chars().take(400).collect();
        self.approve(s.id.as_str(), capability, target.id.as_str(), &detail).await
    }

    /// Messages queued for `session` (tests, diagnostics).
    pub fn queued(&self, session: &str) -> usize {
        self.bridge.pending(session)
    }

    /// Type the next queued message into `session` when it can take one (not working / not
    /// waiting for the user, its TUI at its input); otherwise it stays queued.
    pub(crate) fn deliver_pending(&self, session: &str) {
        use ide_proto::AgentActivity;
        if matches!(
            self.bridge.activity(session),
            AgentActivity::Working | AgentActivity::NeedsInput
        ) {
            return;
        }
        if self.bridge.pending(session) == 0 || !self.input_settled(session) {
            return;
        }
        let Some(msg) = self.bridge.dequeue(session) else {
            return;
        };
        match self.send_prompt(session, &msg.text) {
            Ok(()) => {
                // Typed in: the turn starts (hooks / the screen confirm it).
                self.at_input.lock().unwrap().remove(session);
                self.set_activity(session, AgentActivity::Working);
            }
            // Not ready, or not running (yet / any more): it waits for the next start.
            Err(e) if e.id == "prompt-not-ready" || e.id == "session-not-running" => {
                self.bridge.requeue(session, msg)
            }
            Err(e) => tracing::warn!("session {session}: a queued message was dropped: {e}"),
        }
    }

    /// `session`'s CLI can take a typed prompt: its screen rules show it at its input
    /// (no start dialog, no auto-answered dialog) for [`INPUT_SETTLE`], and none of its
    /// dialogs was auto-answered within that time (the next one may not be drawn yet).
    /// Providers without screen rules: only the terminal's paste mode decides
    /// (`ide_agents::prompt_writes`).
    fn input_settled(&self, session: &str) -> bool {
        let Ok(s) = self.resolve_session(session) else {
            return true;
        };
        let Some(m) = self.providers.get(&s.provider).and_then(|p| p.screen_manifest()) else {
            return true;
        };
        let Some(host) = self.existing_host(session) else {
            return true; // `send_prompt` says it is not running
        };
        let lines = host.screen_text();
        let title = host.title();
        let since = {
            let mut at = self.at_input.lock().unwrap();
            if !m.at_input(&ide_agents::screen::ScreenInput { lines: &lines, title: &title }) {
                at.remove(session);
                return false;
            }
            *at.entry(session.to_owned()).or_insert_with(std::time::Instant::now)
        };
        let prefix = format!("{session} ");
        let answered_now = self
            .answered
            .lock()
            .unwrap()
            .iter()
            .any(|(k, t)| k.starts_with(&prefix) && t.elapsed() < INPUT_SETTLE);
        since.elapsed() >= INPUT_SETTLE && !answered_now
    }

    /// Agent bridge: carry out a tool call of `session`'s agent (the daemon answers itself or
    /// asks the connected apps).
    pub async fn agent_call(self: &Arc<Self>, session: &str, tool: &str, args: &str) -> ide_proto::AgentToolResult {
        if let Some(r) = self.terminal_tool(session, tool, args).await {
            return r;
        }
        if ide_proto::browser::TOOLS.contains(&tool) {
            return self.browser_tool(session, tool, args).await;
        }
        if tool == crate::bridge::TOOL_WIDGET {
            return match serde_json::from_str::<serde_json::Value>(if args.trim().is_empty() { "{}" } else { args }) {
                Ok(v) => self.widget_tool(session, &v).await,
                Err(e) => ide_proto::AgentToolResult {
                    text: format!("invalid arguments: {e}"),
                    is_error: true,
                    image: None,
                },
            };
        }
        if tool == crate::bridge::TOOL_PREVIEW {
            return match serde_json::from_str::<serde_json::Value>(if args.trim().is_empty() { "{}" } else { args }) {
                Ok(v) => self.preview_tool(session, &v).await,
                Err(e) => ide_proto::AgentToolResult {
                    text: format!("invalid arguments: {e}"),
                    is_error: true,
                    image: None,
                },
            };
        }
        if crate::bridge::is_family_tool(tool) {
            let parsed: serde_json::Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
                Ok(v) => v,
                Err(e) => {
                    return ide_proto::AgentToolResult {
                        text: format!("invalid arguments: {e}"),
                        is_error: true,
                        image: None,
                    };
                }
            };
            if let Err(why) = self.approve_family(session, tool, &parsed).await {
                return ide_proto::AgentToolResult {
                    text: why,
                    is_error: true,
                    image: None,
                };
            }
            let (me, session, tool) = (self.clone(), session.to_owned(), tool.to_owned());
            return tokio::task::spawn_blocking(move || me.family_tool(&session, &tool, &parsed))
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| ide_proto::AgentToolResult {
                    text: "the tool failed".into(),
                    is_error: true,
                    image: None,
                });
        }
        // Apps subscribe to the notifications; agent connections do not.
        let apps = self.subscriber_count() > 0;
        let info = || self.resolve_session(session).map(|s| self.info(&s));
        crate::bridge::call(&self.bridge, session, tool, args, info, apps, |n| self.notify(n)).await
    }

    /// Agent bridge: a hook of `session`'s agent CLI. Updates the session's activity; the
    /// answer is `Some(reason)` to refuse a tool call (no rule refuses anything yet).
    pub fn agent_hook(&self, session: &str, event: ide_proto::AgentHookEvent) -> Option<String> {
        self.bridge.mark_hooked(session);
        // The CLI's conversation changed (a new one after `/clear`, another via `/resume`):
        // the next start resumes that one.
        if let ide_proto::AgentHookEvent::SessionStart {
            conversation: Some(c),
        } = &event
            && let Ok(id) = SessionId::parse(session)
        {
            let _ = self
                .sessions
                .lock()
                .unwrap()
                .set_conversation(&id, Some(c.clone()));
        }
        if let Some(a) = self.bridge.hook_activity(session, &event) {
            self.set_activity(session, a);
        }
        match &event {
            ide_proto::AgentHookEvent::PreEdit { paths, .. } => self.edit_refusal(session, paths),
            _ => None,
        }
    }

    /// A tool hook (`PreEdit` / `PreShell`): the shared-folder lease, then the write guard,
    /// which may wait for the user's answer (`docs/write-guard.md`).
    pub async fn agent_tool_hook(
        self: &Arc<Self>,
        session: &str,
        event: ide_proto::AgentHookEvent,
    ) -> Option<String> {
        if let Some(reason) = self.agent_hook(session, event.clone()) {
            return Some(reason);
        }
        self.write_refusal(session, &event).await
    }

    /// The session whose folder `s` works in (a shared child's owner, else itself).
    pub(crate) fn folder_owner(&self, s: &Session) -> SessionId {
        let sessions = self.sessions.lock().unwrap();
        let mut cur = s.clone();
        for _ in 0..=ide_core::family::MAX_DEPTH {
            if !cur.shared {
                break;
            }
            match cur.parent.as_ref().and_then(|p| sessions.get(p)) {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        cur.id
    }

    /// `s` works in a folder another session shares.
    pub(crate) fn shares_folder(&self, s: &Session) -> bool {
        s.is_workspace()
            && self
                .sessions
                .lock()
                .unwrap()
                .list()
                .iter()
                .any(|x| x.id != s.id && ide_core::same_path(&x.root, &s.root))
    }

    /// The write lease of shared folders: an edit inside a shared folder by a session that does
    /// not hold its write access is refused (`Some(reason)` for the agent).
    fn edit_refusal(&self, session: &str, paths: &[String]) -> Option<String> {
        let s = self.resolve_session(session).ok()?;
        if !self.shares_folder(&s) {
            return None;
        }
        let owner = self.folder_owner(&s);
        let holder = self.bridge.writer(owner.as_str());
        if holder == s.id.as_str() {
            return None;
        }
        let inside = paths.is_empty()
            || paths.iter().any(|p| {
                let p = std::path::Path::new(p);
                !p.is_absolute() || p.starts_with(&s.root)
            });
        if !inside {
            return None;
        }
        let title = self
            .resolve_session(&holder)
            .map(|h| h.title)
            .unwrap_or_default();
        self.audit("agent.edit_refused", &format!("{session} holder={holder}"));
        Some(format!(
            "This folder is shared with other sessions and session \"{title}\" ({holder}) holds \
             its write access. Ask for it with ide_request_write_access (granted when that \
             session is not working), coordinate with ide_send_to_session, or do the work in a \
             separate child session (ide_create_child_session)."
        ))
    }

    fn set_activity(&self, session: &str, a: ide_proto::AgentActivity) {
        if self.bridge.set_activity(session, a) {
            self.notify(Notify::Activity {
                session: session.to_owned(),
                activity: a,
            });
        }
    }

    /// Screen detection (`bridge::spawn_screen_watch`): read the screens of running agent
    /// sessions whose provider has screen rules and update their activity. `seen` keeps each
    /// session's last (output end, title): unchanged screens are not read again.
    pub fn screen_tick(&self, seen: &mut HashMap<String, (u64, String)>) {
        let hosts: Vec<(String, Arc<TermHost>)> = self
            .hosts
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, h)| h.state() == SessionState::Running)
            .map(|(k, h)| (k.clone(), h.clone()))
            .collect();
        seen.retain(|k, _| hosts.iter().any(|(id, _)| id == k));
        self.at_input
            .lock()
            .unwrap()
            .retain(|k, _| hosts.iter().any(|(id, _)| id == k));
        for (id, host) in hosts {
            self.screen_read(&id, &host, seen);
            // After the screen was read: a queued message waits for the CLI's input.
            if self.bridge.pending(&id) > 0 {
                self.deliver_pending(&id);
            }
        }
    }

    /// One session's part of [`Self::screen_tick`].
    fn screen_read(&self, id: &str, host: &TermHost, seen: &mut HashMap<String, (u64, String)>) {
        let key = (host.end(), host.title());
        if seen.get(id) == Some(&key) {
            return;
        }
        seen.insert(id.to_owned(), key.clone());
        let Ok(s) = self.resolve_session(id) else {
            return;
        };
        if s.kind != SessionKind::Agent {
            return;
        }
        let Some(m) = self
            .providers
            .get(&s.provider)
            .and_then(|p| p.screen_manifest())
        else {
            return;
        };
        let lines = host.screen_text();
        let input = ide_agents::screen::ScreenInput {
            lines: &lines,
            title: &key.1,
        };
        {
            let mut at = self.at_input.lock().unwrap();
            if m.at_input(&input) {
                at.entry(id.to_owned()).or_insert_with(std::time::Instant::now);
            } else {
                at.remove(id);
            }
        }
        // A dialog ide answers by itself (`docs/auto-answers.md`).
        if let Some(ans) = m.answer(&input)
            && self.auto_answer(&s, host, &ans)
        {
            // Answered or held back: read this screen again on the next tick even if it does
            // not change (keys the CLI missed leave the dialog as it was).
            seen.remove(id);
        }
        let Some(state) = m.detect(&input) else {
            return;
        };
        if let Some(a) = self
            .bridge
            .screen_says(id, crate::bridge::screen_activity(state))
        {
            self.set_activity(id, a);
        }
    }

    /// Agent bridge: an app's answer to a `Notify::AgentRequest`.
    pub fn agent_reply(&self, request: u64, result: Result<String, String>) {
        self.bridge.reply(request, result);
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
        self.uterm_kill_all();
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

    /// Session hosts keep their output in `sessions/<id>/output.log`
    /// ([`TermHost::attach_log`]): a host made after a server restart replays what the session
    /// printed before it.
    fn host_sized(&self, id: &str, cols: u16, rows: u16) -> Arc<TermHost> {
        self.hosts
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| {
                let host = TermHost::new(id, cols, rows, self.events.clone());
                host.attach_log(&self.paths.session_output_log(id));
                host
            })
            .clone()
    }

    fn existing_host(&self, id: &str) -> Option<Arc<TermHost>> {
        self.hosts.lock().unwrap().get(id).cloned()
    }

    fn info(&self, s: &Session) -> SessionInfo {
        let unguarded = self.unguarded(s);
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
            pinned: s.pinned,
            archived: s.archived,
            activity: match host.as_ref().map(|h| h.state()) {
                Some(SessionState::Running) => self.bridge.activity(s.id.as_str()),
                _ => ide_proto::AgentActivity::Unknown,
            },
            parent: s.parent.as_ref().map(|p| p.to_string()),
            shared: s.shared,
            writer: self.shares_folder(s)
                && self.bridge.writer(self.folder_owner(s).as_str()) == s.id.as_str(),
            preparing: s.preparing(),
            unguarded,
            resume_on_open: s.autostart
                && s.kind == SessionKind::Agent
                && !host.as_ref().is_some_and(|h| h.is_running()),
            branch: self.branch_badges.get(s.id.as_str()),
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
                Self::not_preparing(&s)?;
                let rid = self.parse_repo(&repo)?;
                let r = self.repos.lock().unwrap().require(&rid).map_err(core)?.clone();
                let mut updated = s.clone();
                if let Some(pid) = &s.parent {
                    // A child: only its parent's repos, branched from the parent's.
                    let parent = self.resolve_session(pid.as_str())?;
                    let repos = self.repos.lock().unwrap().clone();
                    ide_core::family::attach_child_repo(&mut updated, &parent, &repos, &rid)
                        .map_err(core)?;
                } else {
                    let plan =
                        ide_core::attach_repo(&mut updated, &r, base.as_deref()).map_err(core)?;
                    let _ = self.repos.lock().unwrap().record_plan(
                        &rid,
                        plan.rule_resolved.as_deref(),
                        plan.fixed_upgrade.as_deref(),
                    );
                }
                self.sessions
                    .lock()
                    .unwrap()
                    .upsert(updated.clone())
                    .map_err(core)?;
                self.refresh_root(&updated);
                self.sync_shared_children(&updated);
                self.audit("session.attach_repo", &format!("{id} {repo}"));
                self.notify(Notify::SessionsChanged);
                self.notify(Notify::ReposChanged);
                Ok(Response::Session(self.info(&updated)))
            }
            Request::SessionDetachRepo { id, repo } => {
                let s = self.resolve_session(&id)?;
                Self::not_preparing(&s)?;
                let rid = self.parse_repo(&repo)?;
                let r = self.repos.lock().unwrap().get(&rid).cloned();
                if s.shared {
                    return Err(core(ide_core::Error::SharedChildRepos));
                }
                let used = {
                    let sessions = self.sessions.lock().unwrap();
                    ide_core::family::children_using(sessions.list(), &s.id, &rid)
                };
                if used > 0 {
                    return Err(core(ide_core::Error::RepoUsedByChildren {
                        name: r.as_ref().map_or_else(|| repo.clone(), |r| r.name.clone()),
                        count: used,
                    }));
                }
                self.watchers.remove_key(&crate::watch::key(&id, &repo));
                let mut updated = s.clone();
                ide_core::detach_repo(&self.paths, &mut updated, &rid, r.as_ref()).map_err(core)?;
                self.sessions
                    .lock()
                    .unwrap()
                    .upsert(updated.clone())
                    .map_err(core)?;
                self.refresh_root(&updated);
                self.sync_shared_children(&updated);
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

    /// Shared children work in their parent's worktrees: keep their lists the parent's.
    fn sync_shared_children(&self, parent: &Session) {
        let mut sessions = self.sessions.lock().unwrap();
        let ids: Vec<SessionId> = ide_core::family::children(sessions.list(), &parent.id)
            .iter()
            .filter(|c| c.shared)
            .map(|c| c.id.clone())
            .collect();
        for id in ids {
            let _ = sessions.update(&id, |c| {
                c.worktrees = parent.worktrees.clone();
                c.bases = parent.bases.clone();
            });
        }
    }

    /// `session.create_child` (`docs/session-family.md`): the parent's agent, shared or own
    /// worktrees branched from the parent's.
    pub fn session_create_child(
        &self,
        parent: &str,
        shared: bool,
        repos: &[String],
        title: &str,
        start: bool,
        size: (u16, u16),
    ) -> Result<Response, ProtoError> {
        self.create_child(parent, shared, repos, title, start.then_some(None), size)
            .map(|(r, _)| r)
    }

    /// [`Self::session_create_child`]; `start`: `Some(first prompt)` starts the child (the
    /// prompt, when there is one, on its command line if its provider takes it there). The
    /// `bool`: the prompt went on the command line (else the caller queues it).
    fn create_child(
        &self,
        parent: &str,
        shared: bool,
        repos: &[String],
        title: &str,
        start: Option<Option<&str>>,
        (cols, rows): (u16, u16),
    ) -> Result<(Response, bool), ProtoError> {
        let core = |e: ide_core::Error| core_error(&e);
        let ps = self.resolve_session(parent)?;
        Self::not_preparing(&ps)?;
        let p = self.provider(&ps.provider)?;
        let repo_ids = repos
            .iter()
            .map(|r| self.parse_repo(r))
            .collect::<Result<Vec<_>, _>>()?;
        let snapshot = self.repos.lock().unwrap().clone();
        let all = self.sessions.lock().unwrap().list().to_vec();
        let req = ide_core::family::NewChild {
            target: ps.target(),
            title: title.to_owned(),
            shared,
            repos: repo_ids,
        };
        let (session, failures) =
            ide_core::family::create_child(&self.paths, &snapshot, &all, &ps, &req, |_| {
                p.program_path(&self.launch_env).map(drop)
            })
            .map_err(core)?;
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
            "session.create_child",
            &format!(
                "{} parent={} shared={shared} repos={}",
                session.id,
                ps.id,
                session
                    .worktrees
                    .iter()
                    .map(|w| w.repo.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
        self.notify(Notify::SessionsChanged);
        self.notify(Notify::ReposChanged);
        let prompt_in_args = match start {
            Some(prompt) => self.start_with(&session, prompt)?,
            None => {
                self.refresh_root(&session);
                false
            }
        };
        // The parent's guidance lists its children.
        self.refresh_root(&ps);
        let r = Response::SessionCreated {
            base_changes: Vec::new(),
            session: self.info(&session),
            failures: failures
                .iter()
                .map(|f| RepoFailureInfo {
                    repo: f.repo.to_string(),
                    name: f.name.clone(),
                    error: core_error(&f.error),
                })
                .collect(),
        };
        Ok((r, prompt_in_args))
    }

    /// A child and its parent (`bad-request` for a session without a parent).
    fn child_and_parent(&self, id: &str) -> Result<(Session, Session), ProtoError> {
        let c = self.resolve_session(id)?;
        let pid = c
            .parent
            .clone()
            .ok_or_else(|| ProtoError::new("bad-request").arg("detail", "not a child session"))?;
        let p = self.resolve_session(pid.as_str())?;
        Ok((c, p))
    }

    /// Merge `(repo, worktree, branch)` triples; the worktrees are `notify_session`'s.
    fn merges(
        &self,
        pairs: Vec<(RepoId, PathBuf, String)>,
        squash: bool,
        message: &str,
        notify_session: &str,
    ) -> Vec<ide_proto::RepoMerge> {
        use ide_core::family::MergeOutcome;
        use ide_proto::MergeResult;
        let repos = self.repos.lock().unwrap().clone();
        pairs
            .into_iter()
            .map(|(rid, dir, branch)| {
                let result = match ide_core::family::merge_branch(&dir, &branch, squash, message) {
                    MergeOutcome::Merged => MergeResult::Merged,
                    MergeOutcome::UpToDate => MergeResult::UpToDate,
                    MergeOutcome::Conflicts(files) => MergeResult::Conflicts { files },
                    MergeOutcome::Failed(error) => MergeResult::Failed { error },
                };
                self.notify(Notify::GitChanged {
                    session: notify_session.to_owned(),
                    repo: rid.to_string(),
                });
                ide_proto::RepoMerge {
                    repo: rid.to_string(),
                    name: repos
                        .get(&rid)
                        .map_or_else(|| rid.to_string(), |r| r.name.clone()),
                    result,
                }
            })
            .collect()
    }

    /// `session.merge_child`: each of the child's branches into the parent's worktree of that
    /// repo (a shared child has nothing to merge).
    pub fn merge_child(&self, id: &str, squash: bool) -> Result<Vec<ide_proto::RepoMerge>, ProtoError> {
        let (c, p) = self.child_and_parent(id)?;
        if c.shared {
            return Ok(Vec::new());
        }
        let pairs = c
            .worktrees
            .iter()
            .filter_map(|w| {
                p.worktree(&w.repo)
                    .map(|pw| (w.repo.clone(), pw.path.clone(), w.branch.clone()))
            })
            .collect();
        let message = format!("Merge child session \"{}\"", c.title);
        let r = self.merges(pairs, squash, &message, p.id.as_str());
        self.audit("session.merge_child", &format!("{} into {} squash={squash}", c.id, p.id));
        Ok(r)
    }

    /// `session.sync_parent`: the parent's branches into the child's worktrees.
    pub fn sync_parent(&self, id: &str) -> Result<Vec<ide_proto::RepoMerge>, ProtoError> {
        let (c, p) = self.child_and_parent(id)?;
        if c.shared {
            return Ok(Vec::new());
        }
        let pairs = c
            .worktrees
            .iter()
            .filter_map(|w| {
                p.worktree(&w.repo)
                    .map(|pw| (w.repo.clone(), w.path.clone(), pw.branch.clone()))
            })
            .collect();
        let message = format!("Sync from parent session \"{}\"", p.title);
        let r = self.merges(pairs, false, &message, c.id.as_str());
        self.audit("session.sync_parent", &format!("{} from {}", c.id, p.id));
        Ok(r)
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
                        integration: c.integration,
                        original_repo: ModeCaps {
                            skills: support(c.original_repo.skills),
                            mcp: support(c.original_repo.mcp),
                            instructions: support(c.original_repo.instructions),
                        },
                    },
                    answers: self.answers_of(p.as_ref()),
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
            Request::AccountIntegration {
                provider,
                slug,
                action,
            } => {
                use ide_agents::IntegrationStatus as S;
                use ide_proto::{IntegrationAction as A, IntegrationInfo, IntegrationState as I};
                let p = self.provider(&provider)?;
                let acct = self
                    .accounts
                    .lock()
                    .unwrap()
                    .require(&provider, &slug)
                    .map_err(core)?
                    .clone();
                let targets = self.bridge.hook_targets();
                if action != A::Status && targets.program.is_none() {
                    return Err(ProtoError::new("bad-request")
                        .arg("detail", "the integration needs the ide daemon"));
                }
                let mut approve_with = None;
                match action {
                    A::Status => {}
                    A::Install => {
                        approve_with = p.integration_install(&acct, &targets).map_err(core)?.approve_with;
                        self.audit("account.integration_install", &format!("{provider}/{slug}"));
                    }
                    A::Uninstall => {
                        p.integration_uninstall(&acct).map_err(core)?;
                        self.audit("account.integration_uninstall", &format!("{provider}/{slug}"));
                    }
                }
                let state = match p.integration_status(&acct, &targets) {
                    S::NotNeeded => I::NotNeeded,
                    S::Missing => I::Missing,
                    S::Installed => I::Installed,
                    S::Outdated => I::Outdated,
                };
                if action != A::Status {
                    // Sessions of the account show whether their writes are guarded.
                    self.notify(Notify::SessionsChanged);
                }
                Ok(Response::Integration(IntegrationInfo { state, approve_with }))
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
            Request::ConfigurationList => {
                let store = self.configs.lock().unwrap();
                Ok(Response::Configurations(
                    store
                        .list()
                        .iter()
                        .map(|c| configuration_info(&store, c))
                        .collect(),
                ))
            }
            Request::ConfigurationCreate { name } => {
                let c = {
                    let mut store = self.configs.lock().unwrap();
                    let slug = store.create(&name).map_err(core)?.slug.clone();
                    self.configuration_of(&store, &slug)?
                };
                self.audit("configuration.create", &c.slug);
                self.notify(Notify::ConfigurationsChanged);
                Ok(Response::Configuration(c))
            }
            Request::ConfigurationUpdate(info) => {
                let c = {
                    let mut store = self.configs.lock().unwrap();
                    let slug = store
                        .update(configuration_from_info(info))
                        .map_err(core)?
                        .slug
                        .clone();
                    self.configuration_of(&store, &slug)?
                };
                self.audit("configuration.update", &c.slug);
                self.configuration_changed(&c.slug);
                Ok(Response::Configuration(c))
            }
            Request::SkillRead {
                configuration,
                name,
            } => Ok(Response::SkillText(
                self.configs
                    .lock()
                    .unwrap()
                    .skill_read(&configuration, &name)
                    .map_err(core)?,
            )),
            r @ (Request::SkillCreate { .. }
            | Request::SkillImport { .. }
            | Request::SkillWrite { .. }
            | Request::SkillDelete { .. }
            | Request::SkillEnable { .. }) => {
                let mut store = self.configs.lock().unwrap();
                let (what, slug, name) = match r {
                    Request::SkillCreate {
                        configuration,
                        name,
                        description,
                    } => {
                        let n = store
                            .skill_create(&configuration, &name, &description)
                            .map_err(core)?;
                        ("skill.create", configuration, n)
                    }
                    Request::SkillImport {
                        configuration,
                        path,
                    } => {
                        let n = store
                            .skill_import(&configuration, std::path::Path::new(&path))
                            .map_err(core)?;
                        ("skill.import", configuration, n)
                    }
                    Request::SkillWrite {
                        configuration,
                        name,
                        text,
                    } => {
                        store
                            .skill_write(&configuration, &name, &text)
                            .map_err(core)?;
                        ("skill.write", configuration, name)
                    }
                    Request::SkillDelete {
                        configuration,
                        name,
                    } => {
                        store.skill_delete(&configuration, &name).map_err(core)?;
                        ("skill.delete", configuration, name)
                    }
                    Request::SkillEnable {
                        configuration,
                        name,
                        enabled,
                    } => {
                        store
                            .skill_set_enabled(&configuration, &name, enabled)
                            .map_err(core)?;
                        ("skill.enable", configuration, name)
                    }
                    _ => unreachable!("matched above"),
                };
                let c = self.configuration_of(&store, &slug)?;
                drop(store);
                self.audit(what, &format!("{slug}/{name}"));
                self.configuration_changed(&slug);
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

    /// Configuration `slug` as the protocol shows it (with its skills).
    fn configuration_of(
        &self,
        store: &ide_core::ConfigurationStore,
        slug: &str,
    ) -> Result<ConfigurationInfo, ProtoError> {
        let c = store.require(slug).map_err(|e| core_error(&e))?;
        Ok(configuration_info(store, c))
    }

    /// Configuration `slug` changed: workspace sessions using it get their root files (skills,
    /// guidance) regenerated now (a running agent sees them on its next start), and clients
    /// reload the list.
    fn configuration_changed(&self, slug: &str) {
        let users: Vec<Session> = self
            .sessions
            .lock()
            .unwrap()
            .list()
            .iter()
            .filter(|s| s.configuration == slug)
            .cloned()
            .collect();
        for s in &users {
            self.refresh_root(s);
        }
        self.notify(Notify::ConfigurationsChanged);
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
        if let Some(h) = self.uterm_host(session) {
            return h.write(bytes);
        }
        let s = self.resolve_session(session)?;
        self.existing_host(s.id.as_str())
            .ok_or_else(|| ProtoError::new("session-not-running").arg("id", session))?
            .write(bytes)?;
        // The user answered a permission prompt / question: the turn goes on.
        if self.bridge.activity(s.id.as_str()) == ide_proto::AgentActivity::NeedsInput {
            self.set_activity(s.id.as_str(), ide_proto::AgentActivity::Working);
        }
        Ok(())
    }

    /// Terminal resize.
    pub fn term_resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), ProtoError> {
        if let Some(h) = self.uterm_host(session) {
            h.resize(cols, rows);
            return Ok(());
        }
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
            Request::SessionCreateChild {
                parent,
                shared,
                repos,
                title,
                start,
                cols,
                rows,
            } => self.session_create_child(&parent, shared, &repos, &title, start, (cols, rows)),
            Request::SessionMergeChild { id, squash } => {
                Ok(Response::Merges(self.merge_child(&id, squash)?))
            }
            Request::SessionSyncParent { id } => Ok(Response::Merges(self.sync_parent(&id)?)),
            Request::WidgetList => Ok(Response::Widgets(self.widget_list())),
            Request::WidgetFile { session, widget, path } => self.widget_file(&session, &widget, &path),
            Request::WidgetRegister { session, widget, tools } => self.widget_register(&session, &widget, &tools),
            Request::WidgetDelete { widget } => self.widget_delete(&widget),
            Request::WidgetSetOpen { session, widget, open } => self.widget_set_open(&session, &widget, open),
            Request::WidgetData { session, widget, op } => self.widget_data(&session, &widget, op),
            Request::ProviderSetAutoAnswer { provider, answer, enabled } => self.set_auto_answer(&provider, &answer, enabled),
            Request::PreviewFile { session, preview, path } => self.preview_file(&session, &preview, &path),
            Request::ApprovalList => Ok(self.approval_list()),
            Request::ApprovalRevoke { id } => self.approval_revoke(&id),
            Request::ApprovalWait => Ok(Response::ApprovalWait(self.grants.lock().unwrap().wait())),
            Request::ApprovalSetWait { secs } => self.approval_set_wait(secs),
            Request::ScriptsList { session, repo } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                Ok(Response::Scripts(scripts::discover(j.root())))
            }
            r @ (Request::UTermOpen { .. }
            | Request::UTermList { .. }
            | Request::UTermShells
            | Request::UTermPorts { .. }
            | Request::UTermStop { .. }
            | Request::UTermRerun { .. }
            | Request::UTermRemove { .. }) => self.handle_uterm(r),
            Request::AttachPut {
                session,
                name,
                upload,
                data,
            } => self.attach_put(&session, &name, upload.as_deref(), &data),
            Request::SessionStart { id } => {
                let s = self.resolve_session(&id)?;
                if let Err(e) = self.start(&s) {
                    // A session waiting to be resumed that cannot start is not offered again.
                    if s.autostart {
                        tracing::warn!("session {}: resume failed: {e:?}", s.id);
                        let _ = self.sessions.lock().unwrap().set_autostart(&s.id, false);
                    }
                    return Err(e);
                }
                Ok(Response::Session(self.info(&s)))
            }
            Request::SessionStop { id } => {
                let s = self.resolve_session(&id)?;
                self.stop(&s);
                // Stopped by the user: not restarted when the server starts again.
                let _ = self.sessions.lock().unwrap().set_autostart(&s.id, false);
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
                let kids = {
                    let sessions = self.sessions.lock().unwrap();
                    ide_core::family::children(sessions.list(), &s.id).len()
                };
                if kids > 0 {
                    return Err(core(ide_core::Error::SessionHasChildren { count: kids }));
                }
                // A shared child's folder and worktrees are its parent's: never removed here.
                let remove_worktree = remove_worktree && !s.shared;
                self.stop(&s);
                self.uterm_remove_session(s.id.as_str());
                // Watchers hold the worktree folders open: stop them before deleting.
                self.watchers.remove(s.id.as_str());
                // The output log is in the session's folder: close it first.
                if let Some(h) = self.existing_host(s.id.as_str()) {
                    h.detach_log();
                }
                {
                    let repos = self.repos.lock().unwrap();
                    ide_core::remove_session_files(&self.paths, &s, &repos, remove_worktree)
                        .map_err(core)?;
                }
                self.sessions.lock().unwrap().remove(&s.id).map_err(core)?;
                self.hosts.lock().unwrap().remove(s.id.as_str());
                self.bridge.forget(s.id.as_str());
                self.widget_forget_session(s.id.as_str());
                self.approvals_forget_session(s.id.as_str());
                // The parent's guidance lists its children.
                if let Some(p) = s.parent.as_ref().and_then(|p| self.resolve_session(p.as_str()).ok()) {
                    self.refresh_root(&p);
                }
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
            Request::SessionSetFlags {
                id,
                pinned,
                archived,
            } => {
                let s = self.resolve_session(&id)?;
                self.sessions
                    .lock()
                    .unwrap()
                    .set_flags(&s.id, pinned, archived)
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
                trash,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                {
                    let _g = self.write_lock.lock().unwrap();
                    if trash {
                        fsops::trash(&j, &path)?;
                    } else {
                        fsops::delete(&j, &path, recursive)?;
                    }
                }
                self.audit_fs("fs.delete", &session, &repo, &normalize_rel(&path)?);
                Ok(Response::Unit)
            }
            Request::FsCopy {
                session,
                repo,
                from,
                external,
                to_dir,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let to = {
                    let _g = self.write_lock.lock().unwrap();
                    fsops::copy(&j, &from, external, &to_dir)?
                };
                self.audit_fs("fs.copy", &session, &repo, &format!("{from} -> {to}"));
                Ok(Response::FsCopied(to))
            }
            Request::GitRemoteUrl {
                session,
                repo,
                path,
            } => {
                let (_, _, j) = self.worktree_root(&session, &repo)?;
                let rel = normalize_rel(&path)?;
                let origin = crate::pr::origin_repo(j.root())?;
                let branch = ide_core::git_raw(j.root(), &["rev-parse", "--abbrev-ref", "HEAD"], &[])
                    .ok()
                    .filter(|o| o.code == Some(0))
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    .filter(|b| !b.is_empty() && b != "HEAD")
                    .unwrap_or_else(|| "HEAD".into());
                let enc = |s: &str| s.replace('%', "%25").replace(' ', "%20").replace('#', "%23");
                let segments: Vec<String> = rel.split('/').map(enc).collect();
                Ok(Response::Url(format!(
                    "{}/blob/{}/{}",
                    origin.web,
                    enc(&branch),
                    segments.join("/")
                )))
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
            Request::SearchPaths {
                session,
                repo,
                query,
                max_results,
                include_ignored,
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
                Ok(Response::PathMatches(search::paths(
                    &refs,
                    &query,
                    max_results,
                    include_ignored,
                )))
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
            background: req.background,
        };
        // The program must resolve before the folder and worktrees are created. Worktrees
        // (and 기준 브랜치 최신화 fetches) run on a snapshot of the registry, not under its lock.
        let snapshot = self.repos.lock().unwrap().clone();
        let (session, failures, plans) =
            ide_core::create_session(&self.paths, &snapshot, &core_req, |_| {
                p.program_path(&self.launch_env).map(drop)
            })
            .map_err(core)?;
        let base_changes = self.record_plans(&plans);
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
        // Background mode: the agent already runs (its guidance lists the worktrees as being
        // created); they follow now.
        if session.preparing() {
            self.spawn_prepare(&session.id);
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

    /// Desk: rule picks that changed are reported ("기준 브랜치가 규칙에 따라 바뀌었습니다").
    /// Records each worktree's base plan in the registry and returns those changes.
    fn record_plans(&self, plans: &[(RepoId, ide_core::BasePlan)]) -> Vec<String> {
        let mut base_changes = Vec::new();
        let mut repos = self.repos.lock().unwrap();
        for (rid, plan) in plans {
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
        base_changes
    }

    /// `session-preparing` while a session's worktrees are still being created (attach,
    /// detach and children would race the background job).
    fn not_preparing(s: &Session) -> Result<(), ProtoError> {
        if s.preparing() {
            return Err(ProtoError::new("session-preparing").arg("id", s.id.as_str()));
        }
        Ok(())
    }

    /// Start the background job that creates `id`'s pending worktrees (one job per session).
    fn spawn_prepare(&self, id: &SessionId) {
        let Some(ctx) = self.this.get().and_then(std::sync::Weak::upgrade) else {
            tracing::warn!("session {id}: no context for creating its worktrees");
            return;
        };
        if !self.preparing.lock().unwrap().insert(id.to_string()) {
            return;
        }
        let id = id.clone();
        std::thread::spawn(move || {
            ctx.prepare_worktrees(&id);
            ctx.preparing.lock().unwrap().remove(id.as_str());
        });
    }

    /// Resume the worktree creation of sessions that still have pending worktrees (server
    /// start).
    fn resume_preparing(&self) {
        let ids: Vec<SessionId> = self
            .sessions
            .lock()
            .unwrap()
            .list()
            .iter()
            .filter(|s| s.preparing())
            .map(|s| s.id.clone())
            .collect();
        for id in ids {
            self.spawn_prepare(&id);
        }
    }

    /// Create a session's pending worktrees one after the other (Desk's order of
    /// `createSession`), outside the store's lock: after each one the session gets the worktree
    /// (or loses the pending entry on failure), its guidance is rewritten, the marker file lists
    /// what is left and the app is told. At the end `SessionPrepared` carries the failures and
    /// base changes. A session deleted meanwhile gets the worktree just made removed again.
    fn prepare_worktrees(&self, id: &SessionId) {
        let mut failures = Vec::new();
        let mut base_changes = Vec::new();
        loop {
            let Some(s) = self.sessions.lock().unwrap().get(id).cloned() else {
                return;
            };
            let Some(p) = s.pending.first().cloned() else {
                break;
            };
            let r = self.repos.lock().unwrap().get(&p.repo).cloned();
            let mut work = s.clone();
            work.pending.retain(|x| x.repo != p.repo);
            let result = match &r {
                Some(r) => ide_core::add_worktree(&mut work, r, p.refresh, p.pick.as_deref()),
                None => Err(ide_core::Error::RepoNotFound(p.repo.to_string())),
            };
            let wt = result.as_ref().ok().and_then(|_| work.worktree(&p.repo).cloned());
            let base = work.bases.get(&p.repo).cloned();
            // Into the stored session as it is now (it may have changed meanwhile).
            let updated = {
                let mut sessions = self.sessions.lock().unwrap();
                if sessions.get(id).is_some() {
                    let stored = sessions.update(id, |cur| {
                        cur.pending.retain(|x| x.repo != p.repo);
                        if let Some(w) = &wt
                            && cur.worktree(&w.repo).is_none()
                        {
                            cur.worktrees.push(w.clone());
                            if let Some(b) = &base {
                                cur.bases.insert(p.repo.clone(), b.clone());
                            }
                        }
                    });
                    if let Err(e) = stored {
                        tracing::warn!("session {id}: saving its new worktree failed: {e}");
                    }
                    sessions.get(id).cloned()
                } else {
                    None
                }
            };
            let Some(updated) = updated else {
                // Deleted while the worktree was being made: remove it again (with its branch).
                if let Some(w) = wt {
                    let mut gone = work;
                    gone.worktrees = vec![w];
                    let repos = self.repos.lock().unwrap();
                    if let Err(e) = ide_core::remove_session_files(&self.paths, &gone, &repos, true) {
                        tracing::warn!("session {id}: removing a worktree of a deleted session failed: {e}");
                    }
                }
                self.notify(Notify::ReposChanged);
                return;
            };
            match result {
                Ok(plan) => {
                    base_changes.extend(self.record_plans(&[(p.repo.clone(), plan)]));
                    self.audit("session.prepare.repo", &format!("{id} {}", p.repo));
                }
                Err(error) => {
                    self.audit(
                        "session.create.repo_failed",
                        &format!("{id} {} {error}", p.repo),
                    );
                    failures.push(RepoFailureInfo {
                        repo: p.repo.to_string(),
                        name: r.as_ref().map_or_else(|| p.name.clone(), |r| r.name.clone()),
                        error: core_error(&error),
                    });
                }
            }
            // The guidance first, then the marker: once the marker is gone the guidance is
            // final.
            self.refresh_root(&updated);
            if let Err(e) = ide_core::write_pending_marker(&updated) {
                tracing::warn!("session {id}: updating its pending-worktrees file failed: {e}");
            }
            self.sync_shared_children(&updated);
            self.notify(Notify::SessionsChanged);
            self.notify(Notify::ReposChanged);
            if let Some(w) = updated.worktree(&p.repo) {
                self.notify(Notify::GitChanged {
                    session: id.to_string(),
                    repo: w.repo.to_string(),
                });
            }
        }
        // A marker left behind (e.g. pending emptied elsewhere) goes too.
        if let Some(s) = self.sessions.lock().unwrap().get(id).cloned() {
            let _ = ide_core::write_pending_marker(&s);
        }
        self.notify(Notify::SessionPrepared {
            session: id.to_string(),
            failures,
            base_changes,
        });
    }

    /// The launch spec of a session (generates its scratch files).
    pub fn launch_spec(&self, s: &Session) -> Result<LaunchSpec, ProtoError> {
        Ok(self.assemble(s, None)?.spec)
    }

    /// The assembled launch of a session: spec + the conversation it starts or resumes (+ the
    /// first prompt on the command line, when the provider takes it there).
    fn assemble(&self, s: &Session, first_prompt: Option<&str>) -> Result<ide_agents::Assembled, ProtoError> {
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
            return ide_agents::login_spec(p, &acct, &s.root, &self.launch_env)
                .map(|spec| ide_agents::Assembled {
                    spec,
                    notes: Vec::new(),
                    conversation: None,
                    prompt_in_args: false,
                })
                .map_err(core);
        }
        let configuration = {
            let mut store = self.configs.lock().unwrap();
            // Picks up skill folders changed on disk since the last skill.* call.
            let _ = store.skills(&s.configuration);
            store.require(&s.configuration).map_err(core)?.clone()
        };
        let family = {
            let sessions = self.sessions.lock().unwrap();
            let all = sessions.list();
            let parent = s.parent.as_ref().and_then(|p| all.iter().find(|x| &x.id == p));
            let kids = ide_core::family::children(all, &s.id);
            ide_core::family::family_guidance(s, parent, &kids)
        };
        let repos = self.repos.lock().unwrap();
        let guidance = match (ide_core::session_guidance(s, &repos), family) {
            (Some(g), Some(f)) => Some(format!("{g}\n\n{f}")),
            (g, f) => g.or(f),
        };
        let input = SessionInput::for_session(
            &self.paths,
            s,
            account.as_ref(),
            &configuration,
            &repos,
            guidance,
        )
        .with_bridge(self.bridge.for_session(s.id.as_str()))
        .with_first_prompt(first_prompt);
        drop(repos);
        let built = ide_agents::assemble(p, &input, &self.launch_env).map_err(core)?;
        for n in &built.notes {
            tracing::warn!("session {}: {n}", s.id);
            self.audit("session.note", &format!("{} {n}", s.id));
        }
        Ok(built)
    }

    fn start(&self, s: &Session) -> Result<(), ProtoError> {
        self.start_with(s, None).map(drop)
    }

    /// [`Self::start`] with a first prompt: `true` when it went on the command line
    /// ([`ide_agents::Assembled::prompt_in_args`]); otherwise the caller queues it.
    fn start_with(&self, s: &Session, first_prompt: Option<&str>) -> Result<bool, ProtoError> {
        if self
            .existing_host(s.id.as_str())
            .is_some_and(|h| matches!(h.state(), SessionState::Exited(_)))
        {
            self.after_exit(s);
        }
        let built = self.assemble(s, first_prompt)?;
        // A new process: nothing reported yet.
        self.bridge.reset(s.id.as_str());
        let host = self.host(s);
        host.start(|cols, rows| {
            self.runner
                .spawn_pty(&self.user, &SpawnSpec::from_launch(&built.spec, cols, rows))
        })?;
        {
            let mut sessions = self.sessions.lock().unwrap();
            let _ = sessions.touch(&s.id);
            if built.conversation.is_some() {
                let _ = sessions.set_conversation(&s.id, built.conversation.clone());
            }
            if s.kind == SessionKind::Agent && self.resumable(s) {
                let _ = sessions.set_autostart(&s.id, true);
                self.clear_autostart_on_exit(&host, &s.id);
            }
        }
        self.audit("session.start", s.id.as_str());
        if built.prompt_in_args {
            let bytes = first_prompt.map_or(0, str::len);
            self.audit("session.first_prompt_arg", &format!("{} {bytes} bytes", s.id));
        }
        Ok(built.prompt_in_args)
    }

    /// The session's provider resumes conversations (only those sessions are offered for a
    /// resume after a server start: anything else would come back as a fresh conversation).
    fn resumable(&self, s: &Session) -> bool {
        self.provider(&s.provider)
            .is_ok_and(|p| p.capabilities().resume)
    }

    /// Keep [`Session::autostart`] current for the process just started on `host`: when it exits
    /// by itself (`/exit`, a crash) it is not restarted next time. A kill (`session.stop`,
    /// which clears the mark itself, or a server shutdown, which keeps it) ends the watch.
    fn clear_autostart_on_exit(&self, host: &Arc<TermHost>, id: &SessionId) {
        let (Some(me), Ok(rt)) = (
            self.this.get().cloned(),
            tokio::runtime::Handle::try_current(),
        ) else {
            return;
        };
        let mut state = host.subscribe_state();
        let id = id.clone();
        rt.spawn(async move {
            loop {
                let now = *state.borrow_and_update();
                match now {
                    SessionState::Exited(_) => {
                        if let Some(c) = me.upgrade() {
                            let _ = c.sessions.lock().unwrap().set_autostart(&id, false);
                        }
                        return;
                    }
                    SessionState::Stopped => return,
                    _ => {}
                }
                if state.changed().await.is_err() {
                    return;
                }
            }
        });
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

    /// GitHub for the `pr.*` requests: git's credential token + HTTPS, `gh` as the fallback
    /// (`gh-missing` inside when it cannot be found).
    pub(crate) fn github(&self) -> crate::pr::GithubTransport {
        let gh = crate::pr::resolve_gh(self.github.gh.as_deref(), &self.launch_env).map(|program| {
            Arc::new(crate::pr::GhCli {
                program,
                env: self.launch_env.clone(),
            }) as Arc<dyn crate::pr::GhApi>
        });
        crate::pr::GithubTransport {
            credentials: self.github.credentials.clone(),
            http: self.github.http.clone(),
            gh,
        }
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

/// `ide_send_to_session`'s `mode` (default `queue`).
fn send_mode(args: &serde_json::Value) -> Result<&'static str, String> {
    match args.get("mode").and_then(|v| v.as_str()).unwrap_or("").trim() {
        "" | "queue" => Ok("queue"),
        "steer" => Ok("steer"),
        "interrupt" => Ok("interrupt"),
        other => Err(format!("unknown mode `{other}` (queue, steer, interrupt)")),
    }
}

/// Merge results as JSON for the agent.
fn merges_json(r: &[ide_proto::RepoMerge]) -> Vec<serde_json::Value> {
    r.iter()
        .map(|m| {
            let (result, detail) = match &m.result {
                ide_proto::MergeResult::Merged => ("merged", serde_json::Value::Null),
                ide_proto::MergeResult::UpToDate => ("up to date", serde_json::Value::Null),
                ide_proto::MergeResult::Conflicts { files } => ("conflicts (merge left in progress: resolve, then commit)", serde_json::json!(files)),
                ide_proto::MergeResult::Failed { error } => ("failed (nothing changed)", serde_json::json!(error)),
            };
            serde_json::json!({ "repository": m.name, "result": result, "detail": detail })
        })
        .collect()
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
        | ErrorMessage::SkillNotFound { name }
        | ErrorMessage::SkillExists { name }
        | ErrorMessage::InvalidMcpName { name } => p.arg("name", name),
        ErrorMessage::ProviderNotFound { provider }
        | ErrorMessage::ProviderNoLogin { provider } => p.arg("provider", provider),
        ErrorMessage::SessionNotFound { id } | ErrorMessage::InvalidSessionId { id } => {
            p.arg("id", id)
        }
        ErrorMessage::InvalidSkill { path } => p.arg("path", path.display().to_string()),
        ErrorMessage::NotAGitRepo { path, detail } => p
            .arg("path", path.display().to_string())
            .arg("detail", detail),
        ErrorMessage::GitSpawn { detail } => p.arg("detail", detail),
        ErrorMessage::SandboxSetup { detail } => p.arg("detail", detail),
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
        ErrorMessage::FamilyNeedsWorkspace { id } => p.arg("id", id),
        ErrorMessage::FamilyTooDeep | ErrorMessage::SharedChildRepos => p,
        ErrorMessage::RepoNotInParent { name } => p.arg("name", name),
        ErrorMessage::SessionHasChildren { count } => p.arg("count", count.to_string()),
        ErrorMessage::RepoUsedByChildren { name, count } => {
            p.arg("name", name).arg("count", count.to_string())
        }
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

fn configuration_info(
    store: &ide_core::ConfigurationStore,
    c: &ide_core::Configuration,
) -> ConfigurationInfo {
    ConfigurationInfo {
        slug: c.slug.clone(),
        name: c.name.clone(),
        skills: store
            .skill_list(&c.slug)
            .into_iter()
            .map(|s| SkillInfo {
                name: s.name,
                title: s.title,
                description: s.description,
                enabled: s.enabled,
            })
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
        // Skills change through `skill.*` (the store keeps them).
        skills: Vec::new(),
        disabled_skills: Vec::new(),
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

// --- user terminals (Hermes Desk's `main/terminals.ts`)

/// The sessions' user terminals: kept while the server runs.
#[derive(Default)]
struct UTerms {
    /// Last number given to a terminal (ids stay unique while the server runs).
    seq: u64,
    list: Vec<UTermEntry>,
}

struct UTermEntry {
    id: String,
    session: String,
    title: String,
    command: String,
    /// How it is (re)started.
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    /// The shell's name for numbering shells (`None` for a command).
    shell: Option<String>,
    host: Arc<TermHost>,
}

/// What `uterm.open` asks for.
struct OpenArgs {
    session: String,
    repo: Option<String>,
    folder: Option<String>,
    command: Option<String>,
    title: Option<String>,
    shell: Option<String>,
    /// 공용: the folder to start in.
    start: Option<String>,
}

/// A shell user terminals can run.
struct Shell {
    id: String,
    name: String,
    program: PathBuf,
    /// Arguments of an interactive shell.
    args: Vec<String>,
    /// Arguments before a command line it runs (`-c`, `-Command`, ...).
    run: Vec<String>,
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

/// The shells of this computer, the default first (Desk's `main/shells.ts`): on Windows
/// PowerShell 7, Windows PowerShell, Command Prompt, Git Bash and WSL when installed; elsewhere
/// the login shell (`$SHELL`) and the other shells of `/etc/shells`.
fn shells() -> Vec<Shell> {
    let mut out: Vec<Shell> = Vec::new();
    if cfg!(windows) {
        if let Some(p) = ide_core::which("pwsh") {
            out.push(Shell {
                id: "pwsh".into(),
                name: "PowerShell 7".into(),
                program: p,
                args: strings(&["-NoLogo"]),
                run: strings(&["-NoLogo", "-Command"]),
            });
        }
        if let Some(p) = ide_core::which("powershell") {
            out.push(Shell {
                id: "powershell".into(),
                name: "Windows PowerShell".into(),
                program: p,
                args: strings(&["-NoLogo"]),
                run: strings(&["-NoLogo", "-Command"]),
            });
        }
        if let Some(p) = std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .or_else(|| ide_core::which("cmd"))
        {
            out.push(Shell {
                id: "cmd".into(),
                name: "Command Prompt".into(),
                program: p,
                args: Vec::new(),
                run: strings(&["/C"]),
            });
        }
        // Git for Windows: <git>\cmd\git.exe -> <git>\bin\bash.exe.
        let git_bash = ide_core::which("git")
            .and_then(|g| g.parent()?.parent().map(|d| d.join("bin").join("bash.exe")))
            .filter(|p| p.is_file())
            .or_else(|| {
                let p = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
                p.is_file().then_some(p)
            });
        if let Some(p) = git_bash {
            out.push(Shell {
                id: "git-bash".into(),
                name: "Git Bash".into(),
                program: p,
                args: strings(&["--login", "-i"]),
                run: strings(&["--login", "-c"]),
            });
        }
        if let Some(p) = ide_core::which("wsl") {
            out.push(Shell {
                id: "wsl".into(),
                name: "WSL".into(),
                program: p,
                args: Vec::new(),
                run: strings(&["-e", "sh", "-lc"]),
            });
        }
        return out;
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(s) = std::env::var_os("SHELL").map(PathBuf::from) {
        paths.push(s);
    }
    if let Ok(text) = std::fs::read_to_string("/etc/shells") {
        paths.extend(
            text.lines()
                .map(str::trim)
                .filter(|l| l.starts_with('/'))
                .map(PathBuf::from),
        );
    }
    paths.push(PathBuf::from("/bin/sh"));
    for p in paths {
        let Some(id) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if !p.is_file() || out.iter().any(|s| s.id == id) {
            continue;
        }
        out.push(Shell {
            name: id.clone(),
            id,
            program: p,
            args: strings(&["-l"]),
            run: strings(&["-lc"]),
        });
    }
    out
}

impl UserCtx {
    /// The terminal behind a `term.*` id: a user terminal, else a session.
    pub fn term_host(&self, id: &str) -> Result<Arc<TermHost>, ProtoError> {
        if let Some(h) = self.uterm_host(id) {
            return Ok(h);
        }
        let s = self.resolve_session(id)?;
        Ok(self.host(&s))
    }

    fn uterm_host(&self, id: &str) -> Option<Arc<TermHost>> {
        let u = self.uterms.lock().unwrap();
        u.list.iter().find(|t| t.id == id).map(|t| t.host.clone())
    }

    fn uterm_info(t: &UTermEntry) -> UserTerm {
        UserTerm {
            id: t.id.clone(),
            session: t.session.clone(),
            title: t.title.clone(),
            command: t.command.clone(),
            cwd: t.cwd.display().to_string(),
            state: t.host.state(),
            end: t.host.end(),
        }
    }

    /// Start (or restart) a user terminal's process; when it exits, a line saying so is added
    /// to its output (Desk's `[종료됨: code N]`; terminal content is not translated).
    fn uterm_spawn(
        &self,
        host: &Arc<TermHost>,
        program: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
    ) -> Result<(), ProtoError> {
        host.start(|cols, rows| {
            self.runner.spawn_pty(
                &self.user,
                &SpawnSpec {
                    program: program.to_path_buf(),
                    args: args.to_vec(),
                    cwd: cwd.to_path_buf(),
                    env: Vec::new(),
                    env_remove: Vec::new(),
                    cols,
                    rows,
                },
            )
        })?;
        let mut state = host.subscribe_state();
        let host = host.clone();
        tokio::runtime::Handle::current().spawn(async move {
            while state.changed().await.is_ok() {
                let s = *state.borrow();
                if let SessionState::Exited(code) = s {
                    let code = code.map_or_else(|| "?".to_owned(), |c| c.to_string());
                    let line = format!("\r\n\x1b[90m[exited: code {code}]\x1b[0m\r\n");
                    host.ingest(line.as_bytes());
                    break;
                }
            }
        });
        Ok(())
    }

    fn uterm_open(&self, a: OpenArgs) -> Result<UserTerm, ProtoError> {
        let OpenArgs {
            session,
            repo,
            folder,
            command,
            title,
            shell,
            start,
        } = a;
        let session = session.as_str();
        // An empty session: a 공용 terminal (no session; starts in the home folder).
        let s = if session.is_empty() {
            None
        } else {
            Some(self.resolve_session(session)?)
        };
        let cwd = match &repo {
            Some(r) => {
                let j = self.worktree_root(session, r)?.2;
                match folder.as_deref().filter(|f| !f.is_empty()) {
                    Some(f) => j.resolve(f)?,
                    None => j.root().to_path_buf(),
                }
            }
            None => match &s {
                Some(s) => s.root.clone(),
                // 공용: the folder picked for it, else the home folder.
                None => match start.as_deref().filter(|p| !p.trim().is_empty()) {
                    Some(p) => crate::jail::canonicalize(std::path::Path::new(p.trim()))
                        .ok()
                        .filter(|p| p.is_dir())
                        .ok_or_else(|| ProtoError::new("fs-not-found").arg("path", p))?,
                    None => home_dir().unwrap_or_else(|| PathBuf::from(".")),
                },
            },
        };
        let all = shells();
        let sh = shell
            .as_deref()
            .and_then(|id| all.iter().find(|s| s.id == id))
            .or_else(|| all.first())
            .ok_or_else(|| ProtoError::new("uterm-no-shell"))?;
        let (program, shell, shell_name) = (sh.program.clone(), sh.id.clone(), sh.name.clone());
        let command = command.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
        let args: Vec<String> = match &command {
            Some(c) => sh.run.iter().cloned().chain([c.clone()]).collect(),
            None => sh.args.clone(),
        };
        let mut u = self.uterms.lock().unwrap();
        u.seq += 1;
        let id = match &s {
            Some(s) => format!("{}:t{}", s.id, u.seq),
            None => format!("g:t{}", u.seq),
        };
        let title = title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| match &command {
                Some(c) => c.clone(),
                None => {
                    let n = u
                        .list
                        .iter()
                        .filter(|t| {
                            t.session == session && t.shell.as_deref() == Some(shell.as_str())
                        })
                        .count();
                    format!("{shell_name} {}", n + 1)
                }
            });
        let host = TermHost::with_scrollback(&id, DEFAULT_COLS, DEFAULT_ROWS, self.events.clone(), terminals::SCROLLBACK);
        self.uterm_spawn(&host, &program, &args, &cwd)?;
        let entry = UTermEntry {
            id: id.clone(),
            session: session.to_owned(),
            title,
            command: command.clone().unwrap_or_else(|| shell.clone()),
            program,
            args,
            cwd,
            shell: command.is_none().then_some(shell),
            host,
        };
        let info = Self::uterm_info(&entry);
        u.list.push(entry);
        drop(u);
        // A terminal id given again (after a restart) is a new terminal: no grants carry over.
        self.approvals_forget_target(&id);
        self.audit("uterm.open", &format!("{id} {}", info.command));
        self.notify(Notify::UTermsChanged {
            session: session.to_owned(),
        });
        Ok(info)
    }

    fn uterm_find(&self, id: &str) -> Result<(String, Arc<TermHost>), ProtoError> {
        let u = self.uterms.lock().unwrap();
        u.list
            .iter()
            .find(|t| t.id == id)
            .map(|t| (t.session.clone(), t.host.clone()))
            .ok_or_else(|| ProtoError::new("uterm-not-found").arg("id", id))
    }

    /// Stop and forget the user terminals of `session` (it is being deleted).
    fn uterm_remove_session(&self, session: &str) {
        let gone: Vec<UTermEntry> = {
            let mut u = self.uterms.lock().unwrap();
            let (gone, keep) = std::mem::take(&mut u.list)
                .into_iter()
                .partition(|t| t.session == session);
            u.list = keep;
            gone
        };
        for t in gone {
            t.host.kill();
        }
    }

    /// Kill every user terminal (server shutdown).
    fn uterm_kill_all(&self) {
        for t in &self.uterms.lock().unwrap().list {
            if t.host.is_running() {
                t.host.kill();
            }
        }
    }

    /// `uterm.*` requests.
    /// `attach.put`: store (a piece of) an attachment under the session's data folder.
    fn attach_put(
        &self,
        session: &str,
        name: &str,
        upload: Option<&str>,
        data: &[u8],
    ) -> Result<Response, ProtoError> {
        use std::io::Write as _;
        let s = self.resolve_session(session)?;
        let too_large =
            || ProtoError::new("attach-too-large").arg("max", (ide_proto::ATTACH_MAX >> 20).to_string());
        if data.len() > ide_proto::ATTACH_CHUNK {
            return Err(too_large());
        }
        let dir = self.paths.session_dir(&s.id.to_string()).join("attachments");
        let io = |e: std::io::Error| ProtoError::new("io").arg("detail", e.to_string());
        std::fs::create_dir_all(&dir).map_err(io)?;
        let (file_name, mut file) = match upload {
            None => {
                let name = unique_name(&dir, &attachment_name(name));
                let f = std::fs::File::create(dir.join(&name)).map_err(io)?;
                (name, f)
            }
            Some(n) => {
                // Only a name this folder handed out (no separators, existing).
                if attachment_name(n) != n || !dir.join(n).is_file() {
                    return Err(ProtoError::new("attach-not-found").arg("name", n));
                }
                let f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(dir.join(n))
                    .map_err(io)?;
                (n.to_owned(), f)
            }
        };
        file.write_all(data).map_err(io)?;
        let path = dir.join(&file_name);
        if file.metadata().map(|m| m.len()).unwrap_or(0) > ide_proto::ATTACH_MAX {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(too_large());
        }
        if upload.is_none() {
            self.audit("attach.put", &format!("{session}/{file_name}"));
        }
        Ok(Response::AttachPath {
            name: file_name,
            path: path.display().to_string(),
        })
    }

    fn handle_uterm(&self, req: Request) -> Result<Response, ProtoError> {
        match req {
            Request::UTermOpen {
                session,
                repo,
                folder,
                command,
                title,
                shell,
                cwd,
            } => self
                .uterm_open(OpenArgs {
                    session,
                    repo,
                    folder,
                    command,
                    title,
                    shell,
                    start: cwd,
                })
                .map(Response::UTerm),
            Request::UTermShells => Ok(Response::Shells(
                shells()
                    .into_iter()
                    .map(|s| ide_proto::ShellInfo {
                        id: s.id,
                        name: s.name,
                        path: s.program.display().to_string(),
                    })
                    .collect(),
            )),
            Request::UTermPorts { session } => {
                let mut roots: Vec<u32> = Vec::new();
                if !session.is_empty() {
                    let s = self.resolve_session(&session)?;
                    roots.extend(self.existing_host(s.id.as_str()).and_then(|h| h.pid()));
                }
                roots.extend(
                    self.uterms
                        .lock()
                        .unwrap()
                        .list
                        .iter()
                        .filter(|t| t.session == session)
                        .filter_map(|t| t.host.pid()),
                );
                Ok(Response::Ports(crate::ports::under(&roots)))
            }
            Request::UTermList { session } => {
                if !session.is_empty() {
                    self.resolve_session(&session)?;
                }
                let u = self.uterms.lock().unwrap();
                Ok(Response::UTerms(
                    u.list
                        .iter()
                        .filter(|t| t.session == session)
                        .map(Self::uterm_info)
                        .collect(),
                ))
            }
            Request::UTermStop { id } => {
                let (_, host) = self.uterm_find(&id)?;
                if host.is_running() {
                    self.audit("uterm.stop", &id);
                    host.kill();
                }
                Ok(Response::Unit)
            }
            Request::UTermRerun { id } => {
                let (program, args, cwd, host) = {
                    let u = self.uterms.lock().unwrap();
                    let t = u
                        .list
                        .iter()
                        .find(|t| t.id == id)
                        .ok_or_else(|| ProtoError::new("uterm-not-found").arg("id", &id))?;
                    (t.program.clone(), t.args.clone(), t.cwd.clone(), t.host.clone())
                };
                if !host.is_running() {
                    self.audit("uterm.rerun", &id);
                    self.uterm_spawn(&host, &program, &args, &cwd)?;
                }
                Ok(Response::Unit)
            }
            Request::UTermRemove { id } => {
                let (session, host) = self.uterm_find(&id)?;
                host.kill();
                self.uterms.lock().unwrap().list.retain(|t| t.id != id);
                self.approvals_forget_target(&id);
                self.audit("uterm.remove", &id);
                self.notify(Notify::UTermsChanged { session });
                Ok(Response::Unit)
            }
            _ => Err(ProtoError::new("bad-request")),
        }
    }
}

/// A safe file name for an attachment: the last component of `name`, characters Windows
/// forbids replaced with `_`, "attachment" when nothing is left.
fn attachment_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last
        .chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_matches('.').to_owned();
    if clean.is_empty() {
        "attachment".into()
    } else {
        clean
    }
}

/// `name` in `dir`, or `stem-2.ext`, `stem-3.ext`, ... when it is taken.
fn unique_name(dir: &std::path::Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_owned();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|c| !dir.join(c).exists())
        .expect("a free name")
}

#[cfg(test)]
mod attach_tests {
    use super::*;

    #[test]
    fn attachment_names() {
        assert_eq!(attachment_name(r"C:\shots\a b.png"), "a b.png");
        assert_eq!(attachment_name("../../evil.txt"), "evil.txt");
        assert_eq!(attachment_name("a:b?.png"), "a_b_.png");
        assert_eq!(attachment_name(".."), "attachment");
        assert_eq!(attachment_name(""), "attachment");
        let t = std::env::temp_dir().join(format!("ide-attach-{}", std::process::id()));
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("x.png"), "").unwrap();
        std::fs::write(t.join("x-2.png"), "").unwrap();
        assert_eq!(unique_name(&t, "x.png"), "x-3.png");
        assert_eq!(unique_name(&t, "y"), "y");
        let _ = std::fs::remove_dir_all(&t);
    }
}
