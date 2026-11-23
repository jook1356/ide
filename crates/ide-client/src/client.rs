//! [`Client`]: RPC, events, terminal attachments, reconnect + resume.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use ide_proto::{
    AccountInfo, AccountState, Attached, BrowseListing, ChunkDecoder,
    ConfigurationInfo, DefResult, DiffAgainst, DiffFile, FileContent, FileMatch, FsEntry, FsStat, PathMatches,
    GitChange, GitLog, GitRev, GitStatus, Hello, Mux, MuxConfig, MuxError, NewSession, Notify, PROTO_VERSION,
    BaseRuleInfo, BaseRulePreview, ProtoError, ProviderInfo, RepoFailureInfo, RepoMerge, RepoInfo, Request, Response, ResumeToken, Role, RpcMessage, RpcReceiver,
    SearchMatch, SearchSummary, SessionInfo, Stream, TermChunk, TextEncoding, TextSearch, ScriptGroup, UserTerm, Welcome, PortInfo, ShellInfo,
};
// --- local git actions (Changes / History footer)
use ide_proto::git_actions::{
    BranchRefreshCheck, BranchRefreshInfo, BranchRefreshOptions, BranchRefreshResult, CommitAction,
    GitBranches, PullMode, PushExpected, RemoteFetchStatus, StashAction, StashActionResult,
    StashEntry, SwitchMode, SyncStatus,
};
// --- pull requests and agent prompts
use ide_proto::pr::{
    CommitPrPreview, CommitPrTextOptions, PendingWork, PrClosed, PrDrift, PrExclusions, PrGate,
    PrLeftover, PrMergeMethod, PrMerged, PrStatus, PrTargets, PullRequestResult, UnmergedWork,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::transport::Transport;

/// Client settings.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Sent in `Hello`.
    pub client_version: String,
    /// Reconnect automatically after the connection drops (and retry failed connects).
    pub reconnect: bool,
    /// First reconnect delay.
    pub backoff_min: Duration,
    /// Largest reconnect delay.
    pub backoff_max: Duration,
    /// Per-request timeout.
    pub request_timeout: Duration,
    /// Transport connect + handshake timeout.
    pub connect_timeout: Duration,
    /// Terminal chunks buffered for the app before the server's stream is throttled.
    pub term_buffer: usize,
}

impl Default for ClientOptions {
    fn default() -> Self {
        ClientOptions {
            client_version: ide_proto::VERSION.to_owned(),
            reconnect: true,
            backoff_min: Duration::from_millis(100),
            backoff_max: Duration::from_secs(5),
            request_timeout: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(30),
            term_buffer: 64,
        }
    }
}

/// Something the app should react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// (Re)connected and handshaken. Terminals are being re-attached.
    Connected(Welcome),
    /// The connection dropped or a connect attempt failed (`error`). Reconnecting continues
    /// in the background if enabled.
    Disconnected {
        /// Why a connect attempt failed (`None` when an established connection dropped).
        error: Option<ProtoError>,
    },
    /// Server notification.
    Notify(Notify),
    /// Terminal output of an attached session, in order, exactly once. `reset` = clear the
    /// terminal model first (the chunk may be empty).
    Term(TermChunk),
    /// Re-attaching a terminal after a reconnect failed (e.g. the session was deleted).
    AttachFailed {
        /// Session id.
        session: String,
        /// Why.
        error: ProtoError,
    },
}

fn disconnected() -> ProtoError {
    ProtoError::new("disconnected")
}

#[derive(Debug, Clone, Default)]
struct Status {
    connected: Option<Welcome>,
    attempts: u64,
    last_error: Option<ProtoError>,
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Response, ProtoError>>>>;

/// One `search.text` result batch or its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    /// Matching lines (in walk order).
    Hits(Vec<SearchMatch>),
    /// The search ended (always last).
    Done(SearchSummary),
}

struct Conn {
    mux: Mux,
    pending: Pending,
    /// `search.text` routes of this connection by search id.
    searches: Mutex<HashMap<u64, mpsc::UnboundedSender<SearchEvent>>>,
    _child: Mutex<Option<tokio::process::Child>>,
}

impl Conn {
    async fn call(&self, id: u64, req: Request, timeout: Duration) -> Result<Response, ProtoError> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.mux.send_rpc(&RpcMessage::Request { id, req }.encode()) {
            self.pending.lock().unwrap().remove(&id);
            return Err(match e {
                MuxError::TooLarge => ProtoError::new("message-too-large"),
                _ => disconnected(),
            });
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(ProtoError::new("timeout"))
            }
        }
    }
}

struct AttachState {
    /// Offset of the next byte the app expects (`None` = full replay).
    next: Option<u64>,
    /// Incremented per attach; chunks of older streams are dropped.
    epoch: u64,
    /// A reset was delivered and no bytes since (a server reset at `next` is redundant).
    clean: bool,
}

struct Attachment {
    session: String,
    state: tokio::sync::Mutex<AttachState>,
    detached: AtomicBool,
}

type EventSlot = Arc<
    Mutex<
        Option<(
            mpsc::UnboundedReceiver<ClientEvent>,
            mpsc::Receiver<ClientEvent>,
        )>,
    >,
>;

struct Inner {
    id: u64,
    transport: Box<dyn Transport>,
    opts: ClientOptions,
    conn: Mutex<Option<Arc<Conn>>>,
    status: watch::Sender<Status>,
    next_id: AtomicU64,
    attachments: Mutex<HashMap<String, Arc<Attachment>>>,
    /// `(session, repo)` worktrees watched with `fs.watch` (re-watched after a reconnect).
    watched: Mutex<HashSet<(String, String)>>,
    ctrl_tx: mpsc::UnboundedSender<ClientEvent>,
    term_tx: mpsc::Sender<ClientEvent>,
    events: EventSlot,
    token: Mutex<Option<ResumeToken>>,
    instance: Mutex<Option<String>>,
    shutdown: watch::Sender<bool>,
    kick: tokio::sync::Notify,
}

struct Handle(Arc<Inner>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.shutdown.send_replace(true);
        if let Some(c) = self.0.conn.lock().unwrap().take() {
            c.mux.close();
        }
    }
}

/// A connection to a server (cheap to clone). Dropping the last clone disconnects.
#[derive(Clone)]
pub struct Client(Arc<Handle>);

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.id())
            .field("transport", &self.inner().transport.describe())
            .field("connected", &self.is_connected())
            .finish()
    }
}

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

impl Client {
    /// Start connecting in the background (needs a tokio runtime). Watch [`Client::events`]
    /// for `Connected`, or use [`Client::connect`] to wait for the first handshake.
    pub fn new(transport: impl Transport, opts: ClientOptions) -> Client {
        let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
        let (term_tx, term_rx) = mpsc::channel(opts.term_buffer.max(1));
        let inner = Arc::new(Inner {
            id: NEXT_CLIENT.fetch_add(1, Ordering::Relaxed),
            transport: Box::new(transport),
            opts,
            conn: Mutex::new(None),
            status: watch::channel(Status::default()).0,
            next_id: AtomicU64::new(1),
            attachments: Mutex::new(HashMap::new()),
            watched: Mutex::new(HashSet::new()),
            ctrl_tx,
            term_tx,
            events: Arc::new(Mutex::new(Some((ctrl_rx, term_rx)))),
            token: Mutex::new(None),
            instance: Mutex::new(None),
            shutdown: watch::channel(false).0,
            kick: tokio::sync::Notify::new(),
        });
        tokio::spawn(run(inner.clone()));
        Client(Arc::new(Handle(inner)))
    }

    /// Connect and wait for the first handshake; fails with the first attempt's error.
    pub async fn connect(
        transport: impl Transport,
        opts: ClientOptions,
    ) -> Result<Client, ProtoError> {
        let wait = opts.connect_timeout + Duration::from_secs(5);
        let client = Client::new(transport, opts);
        let mut rx = client.inner().status.subscribe();
        let r = tokio::time::timeout(
            wait,
            rx.wait_for(|s| s.connected.is_some() || s.last_error.is_some()),
        )
        .await;
        match r {
            Ok(Ok(s)) if s.connected.is_some() => Ok(client.clone()),
            Ok(Ok(s)) => Err(s.last_error.clone().unwrap_or_else(disconnected)),
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => Err(ProtoError::new("timeout")),
        }
    }

    fn inner(&self) -> &Arc<Inner> {
        &self.0.0
    }

    /// Process-unique id of this client.
    pub fn id(&self) -> u64 {
        self.inner().id
    }

    /// The event stream. Only one exists at a time: `None` while another [`ClientEvents`] is
    /// alive (it hands the receivers back when dropped).
    pub fn events(&self) -> Option<ClientEvents> {
        let (ctrl, term) = self.inner().events.lock().unwrap().take()?;
        Some(ClientEvents {
            ctrl: Some(ctrl),
            term: Some(term),
            slot: self.inner().events.clone(),
        })
    }

    /// True while connected and handshaken.
    pub fn is_connected(&self) -> bool {
        self.inner().status.borrow().connected.is_some()
    }

    /// The current connection's `Welcome`.
    pub fn welcome(&self) -> Option<Welcome> {
        self.inner().status.borrow().connected.clone()
    }

    /// Wait until connected.
    pub async fn wait_connected(&self, timeout: Duration) -> Result<Welcome, ProtoError> {
        let mut rx = self.inner().status.subscribe();
        match tokio::time::timeout(timeout, rx.wait_for(|s| s.connected.is_some())).await {
            Ok(Ok(s)) => Ok(s.connected.clone().expect("checked")),
            Ok(Err(_)) => Err(disconnected()),
            Err(_) => Err(ProtoError::new("timeout")),
        }
    }

    /// Drop the current connection; the background loop reconnects (if enabled) and resumes.
    pub fn reconnect(&self) {
        if let Some(c) = self.inner().conn.lock().unwrap().as_ref() {
            c.mux.close();
        }
        self.inner().kick.notify_waiters();
    }

    /// Offset of the next terminal byte expected for an attached session.
    pub async fn term_offset(&self, session: &str) -> Option<u64> {
        let a = self
            .inner()
            .attachments
            .lock()
            .unwrap()
            .get(session)
            .cloned()?;
        a.state.lock().await.next
    }

    async fn call(&self, req: Request) -> Result<Response, ProtoError> {
        self.inner().call(req).await
    }

    /// Round trip.
    pub async fn ping(&self) -> Result<(), ProtoError> {
        expect(self.call(Request::Ping).await, |r| {
            matches!(r, Response::Pong).then_some(())
        })
    }

    /// `provider.list` (`refresh` re-runs detection on the server).
    pub async fn providers(&self, refresh: bool) -> Result<Vec<ProviderInfo>, ProtoError> {
        expect(
            self.call(Request::ProviderList { refresh }).await,
            |r| match r {
                Response::Providers(p) => Some(p),
                _ => None,
            },
        )
    }

    /// `account.list`.
    pub async fn accounts(&self) -> Result<Vec<AccountInfo>, ProtoError> {
        expect(self.call(Request::AccountList).await, |r| match r {
            Response::Accounts(a) => Some(a),
            _ => None,
        })
    }

    /// `account.create`.
    pub async fn create_account(
        &self,
        provider: &str,
        name: &str,
    ) -> Result<AccountInfo, ProtoError> {
        expect_account(
            self.call(Request::AccountCreate {
                provider: provider.into(),
                name: name.into(),
            })
            .await,
        )
    }

    /// `account.rename` (display name).
    pub async fn rename_account(
        &self,
        provider: &str,
        slug: &str,
        name: &str,
    ) -> Result<AccountInfo, ProtoError> {
        expect_account(
            self.call(Request::AccountRename {
                provider: provider.into(),
                slug: slug.into(),
                name: name.into(),
            })
            .await,
        )
    }

    /// `account.delete`.
    pub async fn delete_account(
        &self,
        provider: &str,
        slug: &str,
        remove_files: bool,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::AccountDelete {
                provider: provider.into(),
                slug: slug.into(),
                remove_files,
            })
            .await,
        )
    }

    /// `account.status` (runs the CLI on the server; slow).
    pub async fn account_status(
        &self,
        provider: &str,
        slug: &str,
    ) -> Result<AccountState, ProtoError> {
        expect(
            self.call(Request::AccountStatus {
                provider: provider.into(),
                slug: slug.into(),
            })
            .await,
            |r| match r {
                Response::AccountState(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `account.login`: a started login session (terminal) for the account.
    pub async fn login_account(
        &self,
        provider: &str,
        slug: &str,
        cols: u16,
        rows: u16,
    ) -> Result<SessionInfo, ProtoError> {
        expect(
            self.call(Request::AccountLogin {
                provider: provider.into(),
                slug: slug.into(),
                cols,
                rows,
            })
            .await,
            |r| match r {
                Response::Session(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `configuration.list`.
    pub async fn configurations(&self) -> Result<Vec<ConfigurationInfo>, ProtoError> {
        expect(self.call(Request::ConfigurationList).await, |r| match r {
            Response::Configurations(c) => Some(c),
            _ => None,
        })
    }

    /// `configuration.create`.
    pub async fn create_configuration(&self, name: &str) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(
            self.call(Request::ConfigurationCreate { name: name.into() })
                .await,
        )
    }

    /// `configuration.update`.
    pub async fn update_configuration(
        &self,
        c: ConfigurationInfo,
    ) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(self.call(Request::ConfigurationUpdate(c)).await)
    }

    /// `attach.put`: store `data` as an attachment of `session` (in pieces of
    /// [`ide_proto::ATTACH_CHUNK`]); returns its absolute path on the server.
    pub async fn attach_file(
        &self,
        session: &str,
        name: &str,
        data: &[u8],
    ) -> Result<String, ProtoError> {
        let mut upload: Option<String> = None;
        let mut path = String::new();
        // An empty file is still created (one empty piece).
        let empty: &[u8] = &[];
        let pieces = data
            .chunks(ide_proto::ATTACH_CHUNK)
            .chain(std::iter::once(empty).filter(|_| data.is_empty()));
        for piece in pieces {
            let r = self
                .call(Request::AttachPut {
                    session: session.into(),
                    name: name.into(),
                    upload: upload.clone(),
                    data: piece.to_vec(),
                })
                .await;
            let (n, p) = expect(r, |r| match r {
                Response::AttachPath { name, path } => Some((name, path)),
                _ => None,
            })?;
            upload.get_or_insert(n);
            path = p;
        }
        Ok(path)
    }

    /// `skill.create`: a new skill (a `SKILL.md` from `name` and `description`).
    pub async fn skill_create(
        &self,
        configuration: &str,
        name: &str,
        description: &str,
    ) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(
            self.call(Request::SkillCreate {
                configuration: configuration.into(),
                name: name.into(),
                description: description.into(),
            })
            .await,
        )
    }

    /// `skill.import`: copy a skill folder (server path) into the configuration.
    pub async fn skill_import(
        &self,
        configuration: &str,
        path: &str,
    ) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(
            self.call(Request::SkillImport {
                configuration: configuration.into(),
                path: path.into(),
            })
            .await,
        )
    }

    /// `skill.read`: the skill's `SKILL.md`.
    pub async fn skill_read(&self, configuration: &str, name: &str) -> Result<String, ProtoError> {
        expect(
            self.call(Request::SkillRead {
                configuration: configuration.into(),
                name: name.into(),
            })
            .await,
            |r| match r {
                Response::SkillText(t) => Some(t),
                _ => None,
            },
        )
    }

    /// `skill.write`: replace the skill's `SKILL.md`.
    pub async fn skill_write(
        &self,
        configuration: &str,
        name: &str,
        text: &str,
    ) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(
            self.call(Request::SkillWrite {
                configuration: configuration.into(),
                name: name.into(),
                text: text.into(),
            })
            .await,
        )
    }

    /// `skill.delete`.
    pub async fn skill_delete(
        &self,
        configuration: &str,
        name: &str,
    ) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(
            self.call(Request::SkillDelete {
                configuration: configuration.into(),
                name: name.into(),
            })
            .await,
        )
    }

    /// `skill.enable`: offer the skill to new sessions or not.
    pub async fn skill_enable(
        &self,
        configuration: &str,
        name: &str,
        enabled: bool,
    ) -> Result<ConfigurationInfo, ProtoError> {
        expect_configuration(
            self.call(Request::SkillEnable {
                configuration: configuration.into(),
                name: name.into(),
                enabled,
            })
            .await,
        )
    }

    /// `configuration.delete`.
    pub async fn delete_configuration(&self, slug: &str) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::ConfigurationDelete { slug: slug.into() })
                .await,
        )
    }

    /// `session.list`.
    pub async fn sessions(&self) -> Result<Vec<SessionInfo>, ProtoError> {
        expect(self.call(Request::SessionList).await, |r| match r {
            Response::Sessions(s) => Some(s),
            _ => None,
        })
    }

    /// `session.create` (session folder + one worktree per repo, or an original-repo
    /// session -> optional start). Returns the session and the repos whose worktree failed
    /// (the session exists anyway, as in Hermes Desk).
    pub async fn create_session(
        &self,
        req: NewSession,
    ) -> Result<(SessionInfo, Vec<RepoFailureInfo>), ProtoError> {
        self.create_session_full(req).await.map(|(s, f, _)| (s, f))
    }

    /// [`Client::create_session`] plus the rule picks that changed (Desk's notice "기준
    /// 브랜치가 규칙에 따라 바뀌었습니다", one "name: old → new" line per repo).
    pub async fn create_session_full(
        &self,
        req: NewSession,
    ) -> Result<(SessionInfo, Vec<RepoFailureInfo>, Vec<String>), ProtoError> {
        expect(self.call(Request::SessionCreate(req)).await, |r| match r {
            Response::SessionCreated {
                session,
                failures,
                base_changes,
            } => Some((session, failures, base_changes)),
            _ => None,
        })
    }

    /// `session.attach_repo`: give a workspace session a worktree of a registered repo.
    pub async fn attach_repo(&self, id: &str, repo: &str) -> Result<SessionInfo, ProtoError> {
        self.attach_repo_base(id, repo, None).await
    }

    /// `session.attach_repo` with a base branch for the new worktree (origin's branch).
    pub async fn attach_repo_base(
        &self,
        id: &str,
        repo: &str,
        base: Option<&str>,
    ) -> Result<SessionInfo, ProtoError> {
        expect_session(
            self.call(Request::SessionAttachRepo {
                id: id.into(),
                repo: repo.into(),
                base: base.map(Into::into),
            })
            .await,
        )
    }

    /// `session.set_base`: the session's own base branch for one repo (`None` = repo default).
    pub async fn set_session_base(
        &self,
        id: &str,
        repo: &str,
        base: Option<&str>,
    ) -> Result<SessionInfo, ProtoError> {
        expect_session(
            self.call(Request::SessionSetBase {
                id: id.into(),
                repo: repo.into(),
                base: base.map(Into::into),
            })
            .await,
        )
    }

    /// `session.detach_repo`: remove a repo's worktree from a workspace session (branch kept).
    pub async fn detach_repo(&self, id: &str, repo: &str) -> Result<SessionInfo, ProtoError> {
        self.inner()
            .watched
            .lock()
            .unwrap()
            .remove(&(id.to_owned(), repo.to_owned()));
        expect_session(
            self.call(Request::SessionDetachRepo {
                id: id.into(),
                repo: repo.into(),
            })
            .await,
        )
    }

    /// `repo.list`: registered repositories.
    pub async fn repos(&self) -> Result<Vec<RepoInfo>, ProtoError> {
        expect(self.call(Request::RepoList).await, |r| match r {
            Response::Repos(r) => Some(r),
            _ => None,
        })
    }

    /// `repo.register`: register the git repository containing `path` (server path).
    pub async fn register_repo(&self, path: &str) -> Result<RepoInfo, ProtoError> {
        self.register_repo_init(path, false).await
    }

    /// `repo.register` with `init`: a plain folder (`not-a-git-folder` otherwise) is
    /// git-initialised (branch `main`, one "Initial commit") and registered.
    pub async fn register_repo_init(&self, path: &str, init: bool) -> Result<RepoInfo, ProtoError> {
        expect_repo(
            self.call(Request::RepoRegister {
                path: path.into(),
                init,
            })
            .await,
        )
    }

    /// `repo.set_base_branch`: fixed base (full ref from [`Client::repo_branches`]) or `None`
    /// = automatic; replaces a rule.
    pub async fn set_repo_base_branch(
        &self,
        id: &str,
        base: Option<&str>,
    ) -> Result<RepoInfo, ProtoError> {
        expect_repo(
            self.call(Request::RepoSetBaseBranch {
                id: id.into(),
                base: base.map(Into::into),
            })
            .await,
        )
    }

    /// `repo.set_base_rule`: save (checked against the remote) or remove a base branch rule.
    pub async fn set_repo_base_rule(
        &self,
        id: &str,
        rule: Option<BaseRuleInfo>,
    ) -> Result<RepoInfo, ProtoError> {
        expect_repo(
            self.call(Request::RepoSetBaseRule {
                id: id.into(),
                rule,
            })
            .await,
        )
    }

    /// `repo.preview_base_rule`: what a rule picks from the remote right now.
    pub async fn preview_repo_base_rule(
        &self,
        id: &str,
        rule: BaseRuleInfo,
    ) -> Result<BaseRulePreview, ProtoError> {
        expect(
            self.call(Request::RepoPreviewBaseRule {
                id: id.into(),
                rule,
            })
            .await,
            |r| match r {
                Response::BaseRulePreview(p) => Some(p),
                _ => None,
            },
        )
    }

    /// `repo.branches`: (current branch, local + remote-tracking refs).
    pub async fn repo_branches(&self, id: &str) -> Result<(Option<String>, Vec<String>), ProtoError> {
        expect(self.call(Request::RepoBranches { id: id.into() }).await, |r| match r {
            Response::Branches { current, branches } => Some((current, branches)),
            _ => None,
        })
    }

    /// `repo.origin_branches`: origin's branch names as of the last fetch.
    pub async fn repo_origin_branches(&self, id: &str) -> Result<Vec<String>, ProtoError> {
        expect(
            self.call(Request::RepoOriginBranches { id: id.into() }).await,
            |r| match r {
                Response::Branches { branches, .. } => Some(branches),
                _ => None,
            },
        )
    }

    /// `repo.unregister` (`repo-in-use` while sessions have it attached).
    pub async fn unregister_repo(&self, id: &str) -> Result<(), ProtoError> {
        expect_unit(self.call(Request::RepoUnregister { id: id.into() }).await)
    }

    /// `session.start`.
    pub async fn start_session(&self, id: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionStart { id: id.into() }).await)
    }

    /// `session.stop` (waits until the process is gone).
    pub async fn stop_session(&self, id: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionStop { id: id.into() }).await)
    }

    /// `session.restart`.
    pub async fn restart_session(&self, id: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(self.call(Request::SessionRestart { id: id.into() }).await)
    }

    /// `uterm.open`: a shell (or `command`) in `repo`'s worktree (`None` = the session folder).
    pub async fn uterm_open(&self, o: UTermOpen) -> Result<UserTerm, ProtoError> {
        expect(
            self.call(Request::UTermOpen {
                session: o.session,
                repo: o.repo,
                folder: o.folder,
                command: o.command,
                title: o.title,
                shell: o.shell,
                cwd: o.cwd,
            })
            .await,
            |r| match r {
                Response::UTerm(t) => Some(t),
                _ => None,
            },
        )
    }

    /// `uterm.shells`: the shells terminals can run (the default first).
    pub async fn uterm_shells(&self) -> Result<Vec<ShellInfo>, ProtoError> {
        expect(self.call(Request::UTermShells).await, |r| match r {
            Response::Shells(s) => Some(s),
            _ => None,
        })
    }

    /// `uterm.ports`: ports listening in the session's terminals (`""`: the 공용 ones).
    pub async fn uterm_ports(&self, session: &str) -> Result<Vec<PortInfo>, ProtoError> {
        expect(
            self.call(Request::UTermPorts {
                session: session.into(),
            })
            .await,
            |r| match r {
                Response::Ports(p) => Some(p),
                _ => None,
            },
        )
    }

    /// `scripts.list`: runnable scripts of `repo`'s worktree.
    pub async fn scripts_list(&self, session: &str, repo: &str) -> Result<Vec<ScriptGroup>, ProtoError> {
        expect(
            self.call(Request::ScriptsList {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
            |r| match r {
                Response::Scripts(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `uterm.list`: the session's user terminals.
    pub async fn uterm_list(&self, session: &str) -> Result<Vec<UserTerm>, ProtoError> {
        expect(
            self.call(Request::UTermList {
                session: session.into(),
            })
            .await,
            |r| match r {
                Response::UTerms(t) => Some(t),
                _ => None,
            },
        )
    }

    pub async fn uterm_stop(&self, id: &str) -> Result<(), ProtoError> {
        expect_unit(self.call(Request::UTermStop { id: id.into() }).await)
    }

    pub async fn uterm_rerun(&self, id: &str) -> Result<(), ProtoError> {
        expect_unit(self.call(Request::UTermRerun { id: id.into() }).await)
    }

    /// `uterm.remove` (stops it first).
    pub async fn uterm_remove(&self, id: &str) -> Result<(), ProtoError> {
        self.forget(id);
        expect_unit(self.call(Request::UTermRemove { id: id.into() }).await)
    }

    /// `session.delete` (stops it first; optionally removes worktree + branch).
    pub async fn delete_session(&self, id: &str, remove_worktree: bool) -> Result<(), ProtoError> {
        self.forget(id);
        expect_unit(
            self.call(Request::SessionDelete {
                id: id.into(),
                remove_worktree,
            })
            .await,
        )
    }

    /// `session.rename`.
    pub async fn rename_session(&self, id: &str, title: &str) -> Result<SessionInfo, ProtoError> {
        expect_session(
            self.call(Request::SessionRename {
                id: id.into(),
                title: title.into(),
            })
            .await,
        )
    }

    /// `agent.reply`: the app's answer to a `Notify::AgentRequest` of the agent bridge
    /// (`Ok`: the tool's text for the agent, `Err`: why it failed).
    pub async fn agent_reply(&self, request: u64, result: Result<String, String>) -> Result<(), ProtoError> {
        expect_unit(self.call(Request::AgentReply { request, result }).await)
    }

    /// `session.create_child`: a child of `parent` (shared folder, or own worktrees of `repos`
    /// branched from the parent's; empty = all of the parent's).
    #[allow(clippy::too_many_arguments)]
    pub async fn create_child_session(
        &self,
        parent: &str,
        shared: bool,
        repos: Vec<String>,
        title: &str,
        start: bool,
        cols: u16,
        rows: u16,
    ) -> Result<(SessionInfo, Vec<RepoFailureInfo>), ProtoError> {
        match self
            .call(Request::SessionCreateChild {
                parent: parent.into(),
                shared,
                repos,
                title: title.into(),
                start,
                cols,
                rows,
            })
            .await?
        {
            Response::SessionCreated {
                session, failures, ..
            } => Ok((session, failures)),
            _ => Err(ProtoError::new("bad-message").arg("detail", "expected a session")),
        }
    }

    /// `session.merge_child`: the child's branches into its parent's worktrees.
    pub async fn merge_child(&self, id: &str, squash: bool) -> Result<Vec<RepoMerge>, ProtoError> {
        match self
            .call(Request::SessionMergeChild {
                id: id.into(),
                squash,
            })
            .await?
        {
            Response::Merges(m) => Ok(m),
            _ => Err(ProtoError::new("bad-message").arg("detail", "expected merges")),
        }
    }

    /// `session.sync_parent`: the parent's branches into the child's worktrees.
    pub async fn sync_parent(&self, id: &str) -> Result<Vec<RepoMerge>, ProtoError> {
        match self.call(Request::SessionSyncParent { id: id.into() }).await? {
            Response::Merges(m) => Ok(m),
            _ => Err(ProtoError::new("bad-message").arg("detail", "expected merges")),
        }
    }

    /// `session.set_flags`: pin / unpin and archive / unarchive (`None` = unchanged).
    pub async fn set_session_flags(
        &self,
        id: &str,
        pinned: Option<bool>,
        archived: Option<bool>,
    ) -> Result<SessionInfo, ProtoError> {
        expect_session(
            self.call(Request::SessionSetFlags {
                id: id.into(),
                pinned,
                archived,
            })
            .await,
        )
    }

    /// Attach a session's terminal: output from `from_offset` (`None` = everything the server
    /// still has, starting with a reset) arrives as [`ClientEvent::Term`]. The attachment is
    /// remembered: after a reconnect it resumes from the last delivered offset. If not
    /// connected, it is attached on the next connect and this returns `disconnected`.
    pub async fn attach(
        &self,
        session: &str,
        from_offset: Option<u64>,
    ) -> Result<Attached, ProtoError> {
        let inner = self.inner();
        let att = inner
            .attachments
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_insert_with(|| {
                Arc::new(Attachment {
                    session: session.to_owned(),
                    state: tokio::sync::Mutex::new(AttachState {
                        next: None,
                        epoch: 0,
                        clean: false,
                    }),
                    detached: AtomicBool::new(false),
                })
            })
            .clone();
        {
            let mut st = att.state.lock().await;
            st.next = from_offset;
            st.clean = false;
        }
        let conn = inner.current().ok_or_else(disconnected)?;
        inner.attach_on(&conn, &att).await
    }

    /// Stop streaming a session's terminal.
    pub async fn detach(&self, session: &str) -> Result<(), ProtoError> {
        self.forget(session);
        if self.is_connected() {
            expect_unit(
                self.call(Request::TermDetach {
                    session: session.into(),
                })
                .await,
            )?;
        }
        Ok(())
    }

    fn forget(&self, session: &str) {
        if let Some(a) = self.inner().attachments.lock().unwrap().remove(session) {
            a.detached.store(true, Ordering::Release);
        }
    }

    /// Terminal input (keys, committed IME text, paste).
    pub async fn write(&self, session: &str, bytes: impl Into<Vec<u8>>) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::TermWrite {
                session: session.into(),
                bytes: bytes.into(),
            })
            .await,
        )
    }

    /// `server.shutdown`: the server stops every session and exits. The reply arrives before
    /// the process exits; afterwards the connection drops (and a local transport would start
    /// a new daemon on the next reconnect, so drop the client first if that is not wanted).
    pub async fn shutdown_server(&self) -> Result<(), ProtoError> {
        expect_unit(self.call(Request::ServerShutdown).await)
    }

    /// Terminal resize (the most recent caller sets the PTY size).
    pub async fn resize(&self, session: &str, cols: u16, rows: u16) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::TermResize {
                session: session.into(),
                cols,
                rows,
            })
            .await,
        )
    }

    // ---- Phase 2: files, folder picker, search, git, go-to-definition ----
    // `(session, repo)` names one worktree; paths are root-relative to it (`/`-separated,
    // `""` = root).

    /// `fs.list`: entries of `dir` (directories first). `.git` is never listed; ignored
    /// entries only with `include_ignored` (flagged `ignored`).
    pub async fn fs_list(
        &self,
        session: &str,
        repo: &str,
        dir: &str,
        include_ignored: bool,
    ) -> Result<Vec<FsEntry>, ProtoError> {
        expect(
            self.call(Request::FsList {
                session: session.into(),
                repo: repo.into(),
                dir: dir.into(),
                include_ignored,
            })
            .await,
            |r| match r {
                Response::Entries(e) => Some(e),
                _ => None,
            },
        )
    }

    /// `fs.stat` (a symlink is reported as such, not followed).
    pub async fn fs_stat(&self, session: &str, repo: &str, path: &str) -> Result<FsStat, ProtoError> {
        expect(
            self.call(Request::FsStat {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
            })
            .await,
            |r| match r {
                Response::Stat(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `fs.read` (`max_bytes` 0 = 2 MiB; at most 8 MiB). Keep `version` and `encoding` for
    /// [`Client::fs_write`].
    pub async fn fs_read(
        &self,
        session: &str,
        repo: &str,
        path: &str,
        max_bytes: u64,
    ) -> Result<FileContent, ProtoError> {
        expect(
            self.call(Request::FsRead {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                max_bytes,
            })
            .await,
            |r| match r {
                Response::File(f) => Some(f),
                _ => None,
            },
        )
    }

    /// `fs.write`: replace the file's content; returns the new version. With
    /// `expected_version`, fails with `fs-conflict` (args `path`, `expected`, `actual`) when
    /// the file changed on disk since it was read. `encoding` `None` = UTF-8 without BOM.
    pub async fn fs_write(
        &self,
        session: &str,
        repo: &str,
        path: &str,
        content: impl Into<String>,
        expected_version: Option<&str>,
        encoding: Option<TextEncoding>,
    ) -> Result<String, ProtoError> {
        expect_written(
            self.call(Request::FsWrite {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                content: content.into(),
                expected_version: expected_version.map(str::to_owned),
                encoding,
            })
            .await,
        )
    }

    /// `fs.create_file` (`fs-exists` if it exists); returns the version.
    pub async fn fs_create_file(
        &self,
        session: &str,
        repo: &str,
        path: &str,
        content: impl Into<String>,
    ) -> Result<String, ProtoError> {
        expect_written(
            self.call(Request::FsCreateFile {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                content: content.into(),
            })
            .await,
        )
    }

    /// `fs.create_dir` (with parents; `fs-exists` if it exists).
    pub async fn fs_create_dir(&self, session: &str, repo: &str, path: &str) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsCreateDir {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
            })
            .await,
        )
    }

    /// `fs.rename` / move inside the session (`fs-exists` if `to` exists).
    pub async fn fs_rename(&self, session: &str, repo: &str, from: &str, to: &str) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsRename {
                session: session.into(),
                repo: repo.into(),
                from: from.into(),
                to: to.into(),
            })
            .await,
        )
    }

    /// `fs.delete` (permanent; folders with content need `recursive`).
    pub async fn fs_delete(
        &self,
        session: &str,
        repo: &str,
        path: &str,
        recursive: bool,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsDelete {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                recursive,
                trash: false,
            })
            .await,
        )
    }

    /// `fs.delete` with `trash`: to the recycle bin / trash (Explore's 삭제).
    pub async fn fs_trash(&self, session: &str, repo: &str, path: &str) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::FsDelete {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                recursive: true,
                trash: true,
            })
            .await,
        )
    }

    /// `fs.copy`: copy `from` (root-relative, or an absolute server path with `external`) into
    /// `to_dir`; returns the copy's root-relative path.
    pub async fn fs_copy(
        &self,
        session: &str,
        repo: &str,
        from: &str,
        external: bool,
        to_dir: &str,
    ) -> Result<String, ProtoError> {
        expect(
            self.call(Request::FsCopy {
                session: session.into(),
                repo: repo.into(),
                from: from.into(),
                external,
                to_dir: to_dir.into(),
            })
            .await,
            |r| match r {
                Response::FsCopied(p) => Some(p),
                _ => None,
            },
        )
    }

    /// `git.remote_url`: the web address of `path` on the repo's origin.
    pub async fn git_remote_url(&self, session: &str, repo: &str, path: &str) -> Result<String, ProtoError> {
        expect(
            self.call(Request::GitRemoteUrl {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
            })
            .await,
            |r| match r {
                Response::Url(u) => Some(u),
                _ => None,
            },
        )
    }

    /// `fs.watch`: `Notify::FsChanged` / `Notify::GitChanged` for the session arrive as
    /// [`ClientEvent::Notify`]. Remembered: re-watched after a reconnect (refresh what is
    /// shown on `Connected`, changes during the gap are not replayed).
    pub async fn fs_watch(&self, session: &str, repo: &str) -> Result<(), ProtoError> {
        self.inner()
            .watched
            .lock()
            .unwrap()
            .insert((session.to_owned(), repo.to_owned()));
        expect_unit(
            self.call(Request::FsWatch {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
        )
    }

    /// Stop watching a session's repo worktree.
    pub async fn fs_unwatch(&self, session: &str, repo: &str) -> Result<(), ProtoError> {
        self.inner()
            .watched
            .lock()
            .unwrap()
            .remove(&(session.to_owned(), repo.to_owned()));
        if !self.is_connected() {
            return Ok(());
        }
        expect_unit(
            self.call(Request::FsUnwatch {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
        )
    }

    /// `browse.pick_list`: folders anywhere the server's OS user can read, for choosing a
    /// repository to register (`None` = the start list: drives or `/`, plus the home dir; path `""`).
    pub async fn browse_pick_list(
        &self,
        abs_dir: Option<&str>,
    ) -> Result<BrowseListing, ProtoError> {
        expect(
            self.call(Request::BrowsePickList {
                abs_dir: abs_dir.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::Browse(b) => Some(b),
                _ => None,
            },
        )
    }

    /// `search.text`: start a search; results stream through the returned [`SearchStream`]
    /// (dropping it cancels the search). Query errors (`search-bad-regex`, ...) are returned
    /// here.
    pub async fn search_text(&self, query: TextSearch) -> Result<SearchStream, ProtoError> {
        let inner = self.inner();
        let conn = inner.current().ok_or_else(disconnected)?;
        let search = inner.next_id();
        let (tx, rx) = mpsc::unbounded_channel();
        conn.searches.lock().unwrap().insert(search, tx);
        let r = conn
            .call(
                inner.next_id(),
                Request::SearchText { search, query },
                inner.opts.request_timeout,
            )
            .await;
        if let Err(e) = expect_unit(r) {
            conn.searches.lock().unwrap().remove(&search);
            return Err(e);
        }
        Ok(SearchStream {
            id: search,
            rx,
            inner: Arc::downgrade(inner),
            done: false,
        })
    }

    /// `search.files`: fuzzy file-name search (`max_results` 0 = 100), best first.
    /// Explore's 파일 찾기 (`search.paths`): files and folders whose path contains `query`.
    pub async fn search_paths(
        &self,
        session: &str,
        repo: Option<&str>,
        query: &str,
        max_results: u32,
        include_ignored: bool,
    ) -> Result<PathMatches, ProtoError> {
        expect(
            self.call(Request::SearchPaths {
                session: session.into(),
                repo: repo.map(str::to_owned),
                query: query.into(),
                max_results,
                include_ignored,
            })
            .await,
            |r| match r {
                Response::PathMatches(m) => Some(m),
                _ => None,
            },
        )
    }

    pub async fn search_files(
        &self,
        session: &str,
        repo: Option<&str>,
        query: &str,
        max_results: u32,
    ) -> Result<Vec<FileMatch>, ProtoError> {
        expect(
            self.call(Request::SearchFiles {
                session: session.into(),
                repo: repo.map(str::to_owned),
                query: query.into(),
                max_results,
            })
            .await,
            |r| match r {
                Response::FileMatches(m) => Some(m),
                _ => None,
            },
        )
    }

    /// `git.status` (`base` = branch to compare with; `None` = the branch checked out in the
    /// session's repository, else `main`/`master`).
    pub async fn git_status(
        &self,
        session: &str,
        repo: &str,
        base: Option<&str>,
    ) -> Result<GitStatus, ProtoError> {
        expect(
            self.call(Request::GitStatus {
                session: session.into(),
                repo: repo.into(),
                base: base.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::GitStatus(s) => Some(s),
                _ => None,
            },
        )
    }

    /// `git.diff`: structured hunks of one file (`path`) or of every changed tracked file.
    pub async fn git_diff(
        &self,
        session: &str,
        repo: &str,
        path: Option<&str>,
        against: DiffAgainst,
        base: Option<&str>,
    ) -> Result<Vec<DiffFile>, ProtoError> {
        expect(
            self.call(Request::GitDiff {
                session: session.into(),
                repo: repo.into(),
                path: path.map(str::to_owned),
                against,
                base: base.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::Diff(d) => Some(d),
                _ => None,
            },
        )
    }

    /// `git.file_at`: content at a revision (`None` if the file does not exist there), for
    /// side-by-side views.
    pub async fn git_file_at(
        &self,
        session: &str,
        repo: &str,
        path: &str,
        rev: GitRev,
        base: Option<&str>,
    ) -> Result<Option<FileContent>, ProtoError> {
        expect(
            self.call(Request::GitFileAt {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                rev,
                base: base.map(str::to_owned),
            })
            .await,
            |r| match r {
                Response::FileAt(f) => Some(f),
                _ => None,
            },
        )
    }

    /// `git.log`: a page of HEAD's history, newest first (`limit` 0 = Hermes Desk's page of
    /// [`ide_proto::GIT_LOG_PAGE`]); `has_more` = ask again with `skip + commits.len()`.
    pub async fn git_log(
        &self,
        session: &str,
        repo: &str,
        skip: u32,
        limit: u32,
    ) -> Result<GitLog, ProtoError> {
        expect(
            self.call(Request::GitLog {
                session: session.into(),
                repo: repo.into(),
                skip,
                limit,
            })
            .await,
            |r| match r {
                Response::GitLog(l) => Some(l),
                _ => None,
            },
        )
    }

    /// `git.unpushed`: full ids of the commits the History tab marks as not pushed (at most
    /// [`ide_proto::GIT_UNPUSHED_MAX`]; the badge shows `500+` at the cap). Empty without a
    /// remote.
    pub async fn git_unpushed(&self, session: &str, repo: &str) -> Result<Vec<String>, ProtoError> {
        expect(
            self.call(Request::GitUnpushed {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
            |r| match r {
                Response::GitUnpushed(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.commit_files`: files a commit changed against its first parent (renames detected).
    pub async fn git_commit_files(
        &self,
        session: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<GitChange>, ProtoError> {
        expect(
            self.call(Request::GitCommitFiles {
                session: session.into(),
                repo: repo.into(),
                sha: sha.into(),
            })
            .await,
            |r| match r {
                Response::GitChanges(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.commit_diff`: structured diff of a commit against its first parent (root commit:
    /// everything added); `path` `None` = every file.
    pub async fn git_commit_diff(
        &self,
        session: &str,
        repo: &str,
        sha: &str,
        path: Option<String>,
    ) -> Result<Vec<DiffFile>, ProtoError> {
        expect(
            self.call(Request::GitCommitDiff {
                session: session.into(),
                repo: repo.into(),
                sha: sha.into(),
                path,
            })
            .await,
            |r| match r {
                Response::Diff(d) => Some(d),
                _ => None,
            },
        )
    }

    // --- local git actions (Changes / History footer)

    /// `git.discard`: discard one file's uncommitted change (`file` as listed by `git.status`;
    /// `kind` decides how: untracked deleted, added removed, renamed undone, else restored).
    pub async fn git_discard(
        &self,
        session: &str,
        repo: &str,
        file: GitChange,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::GitDiscard {
                session: session.into(),
                repo: repo.into(),
                file,
            })
            .await,
        )
    }

    /// `git.stash_list`: every stash of the repository, newest first.
    pub async fn git_stash_list(
        &self,
        session: &str,
        repo: &str,
    ) -> Result<Vec<StashEntry>, ProtoError> {
        expect(
            self.call(Request::GitStashList {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
            |r| match r {
                Response::GitStashes(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.stash_push`: stash every change (`paths` `None`) or only `paths` (both sides of a
    /// rename), untracked files included; `message` may be empty.
    pub async fn git_stash_push(
        &self,
        session: &str,
        repo: &str,
        message: &str,
        paths: Option<Vec<String>>,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::GitStashPush {
                session: session.into(),
                repo: repo.into(),
                message: message.into(),
                paths,
            })
            .await,
        )
    }

    /// `git.stash_action`: pop / apply / drop the stash `sha`; conflicted paths if pop/apply
    /// conflicted (the stash is kept then).
    pub async fn git_stash_action(
        &self,
        session: &str,
        repo: &str,
        sha: &str,
        action: StashAction,
    ) -> Result<StashActionResult, ProtoError> {
        expect(
            self.call(Request::GitStashAction {
                session: session.into(),
                repo: repo.into(),
                sha: sha.into(),
                action,
            })
            .await,
            |r| match r {
                Response::GitStashResult(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.commit_action`: checkout (detached) / new branch at / revert / cherry-pick commit
    /// `sha` (full id).
    pub async fn git_commit_action(
        &self,
        session: &str,
        repo: &str,
        sha: &str,
        action: CommitAction,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::GitCommitAction {
                session: session.into(),
                repo: repo.into(),
                sha: sha.into(),
                action,
            })
            .await,
        )
    }

    /// `git.sync_status`: ahead / behind / publish state; `fetch` updates the upstream first.
    pub async fn git_sync_status(
        &self,
        session: &str,
        repo: &str,
        fetch: bool,
        merged_head: Option<String>,
    ) -> Result<SyncStatus, ProtoError> {
        expect(
            self.call(Request::GitSyncStatus {
                session: session.into(),
                repo: repo.into(),
                fetch,
                merged_head,
            })
            .await,
            |r| match r {
                Response::GitSyncStatus(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.push`: push / publish the checked-out branch; returns the branch.
    pub async fn git_push(
        &self,
        session: &str,
        repo: &str,
        force: bool,
        expected: Option<PushExpected>,
    ) -> Result<String, ProtoError> {
        expect(
            self.call(Request::GitPush {
                session: session.into(),
                repo: repo.into(),
                force,
                expected,
            })
            .await,
            |r| match r {
                Response::GitBranchName(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.pull`: merge / rebase / reset to the upstream; returns the branch.
    pub async fn git_pull(
        &self,
        session: &str,
        repo: &str,
        mode: PullMode,
    ) -> Result<String, ProtoError> {
        expect(
            self.call(Request::GitPull {
                session: session.into(),
                repo: repo.into(),
                mode,
            })
            .await,
            |r| match r {
                Response::GitBranchName(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.branches`: local branches (with who has them checked out) and remote-tracking
    /// branches, from local refs.
    pub async fn git_branches(&self, session: &str, repo: &str) -> Result<GitBranches, ProtoError> {
        expect(
            self.call(Request::GitBranches {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
            |r| match r {
                Response::GitBranches(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.fetch_remotes`: throttled `git fetch --prune` of every remote (never an error:
    /// failures are in the status).
    pub async fn git_fetch_remotes(
        &self,
        session: &str,
        repo: &str,
    ) -> Result<RemoteFetchStatus, ProtoError> {
        expect(
            self.call(Request::GitFetchRemotes {
                session: session.into(),
                repo: repo.into(),
            })
            .await,
            |r| match r {
                Response::GitFetchStatus(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.switch_branch`: switch to local `branch`, or (with `remote`) to that remote's
    /// branch.
    pub async fn git_switch_branch(
        &self,
        session: &str,
        repo: &str,
        branch: &str,
        remote: Option<String>,
        mode: SwitchMode,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::GitSwitchBranch {
                session: session.into(),
                repo: repo.into(),
                branch: branch.into(),
                remote,
                mode,
            })
            .await,
        )
    }

    /// `git.create_branch`: new branch `name` from `base` (local branch or `remote/branch`),
    /// switched to.
    pub async fn git_create_branch(
        &self,
        session: &str,
        repo: &str,
        name: &str,
        base: &str,
        mode: SwitchMode,
    ) -> Result<(), ProtoError> {
        expect_unit(
            self.call(Request::GitCreateBranch {
                session: session.into(),
                repo: repo.into(),
                name: name.into(),
                base: base.into(),
                mode,
            })
            .await,
        )
    }

    /// `git.branch_refresh_info`: 새 브랜치로 이어가기 dialog data (`fetch` = dialog opening).
    pub async fn git_branch_refresh_info(
        &self,
        session: &str,
        repo: &str,
        fetch: bool,
    ) -> Result<BranchRefreshInfo, ProtoError> {
        expect(
            self.call(Request::GitBranchRefreshInfo {
                session: session.into(),
                repo: repo.into(),
                fetch,
            })
            .await,
            |r| match r {
                Response::GitRefreshInfo(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.branch_refresh_check`: dry run of the picked commits (and uncommitted changes).
    pub async fn git_branch_refresh_check(
        &self,
        session: &str,
        repo: &str,
        commits: Vec<String>,
        move_files: bool,
    ) -> Result<BranchRefreshCheck, ProtoError> {
        expect(
            self.call(Request::GitBranchRefreshCheck {
                session: session.into(),
                repo: repo.into(),
                commits,
                move_files,
            })
            .await,
            |r| match r {
                Response::GitRefreshCheck(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `git.refresh_branch`: continue on a new branch from the fresh `origin/<base>`.
    pub async fn git_refresh_branch(
        &self,
        session: &str,
        repo: &str,
        options: BranchRefreshOptions,
    ) -> Result<BranchRefreshResult, ProtoError> {
        expect(
            self.call(Request::GitRefreshBranch {
                session: session.into(),
                repo: repo.into(),
                options,
            })
            .await,
            |r| match r {
                Response::GitRefreshResult(v) => Some(v),
                _ => None,
            },
        )
    }

    /// `def.lookup`: go to definition at 1-based `line`/`col` (`col` in chars) of `path`;
    /// `text` = the editor's unsaved content (`None` = the file on disk).
    pub async fn def_lookup(
        &self,
        session: &str,
        repo: &str,
        path: &str,
        line: u32,
        col: u32,
        text: Option<String>,
    ) -> Result<DefResult, ProtoError> {
        expect(
            self.call(Request::DefLookup {
                session: session.into(),
                repo: repo.into(),
                path: path.into(),
                line,
                col,
                text,
            })
            .await,
            |r| match r {
                Response::Def(d) => Some(d),
                _ => None,
            },
        )
    }

    // --- pull requests and agent prompts

    /// `pr.status`: the checked-out branch's PR (open first, else the latest), `None` = none.
    pub async fn pr_status(&self, session: &str, repo: &str) -> Result<Option<PrStatus>, ProtoError> {
        let req = Request::PrStatus {
            session: session.into(),
            repo: repo.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrStatus(s) => Some(s),
            _ => None,
        })
    }

    /// `pr.gate`: review decision, checks and merge state of PR `number`.
    pub async fn pr_gate(&self, session: &str, repo: &str, number: u64) -> Result<PrGate, ProtoError> {
        let req = Request::PrGate {
            session: session.into(),
            repo: repo.into(),
            number,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrGate(g) => Some(g),
            _ => None,
        })
    }

    /// `pr.find_open`: the branch's open PR (`kind: Exists`), `None` = none open.
    pub async fn pr_find_open(
        &self,
        session: &str,
        repo: &str,
    ) -> Result<Option<PullRequestResult>, ProtoError> {
        let req = Request::PrFindOpen {
            session: session.into(),
            repo: repo.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrFound(f) => Some(f),
            _ => None,
        })
    }

    /// `pr.remote_branches`: origin's branches, its default and the current branch.
    pub async fn pr_remote_branches(&self, session: &str, repo: &str) -> Result<PrTargets, ProtoError> {
        let req = Request::PrRemoteBranches {
            session: session.into(),
            repo: repo.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrTargets(t) => Some(t),
            _ => None,
        })
    }

    /// `pr.pending_work`: unpushed commits and uncommitted files.
    pub async fn pr_pending_work(&self, session: &str, repo: &str) -> Result<PendingWork, ProtoError> {
        let req = Request::PrPendingWork {
            session: session.into(),
            repo: repo.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrPendingWork(p) => Some(p),
            _ => None,
        })
    }

    /// `pr.leftover`: the PR text left behind and what the branch has for `base`.
    pub async fn pr_leftover(
        &self,
        session: &str,
        repo: &str,
        base: Option<&str>,
    ) -> Result<PrLeftover, ProtoError> {
        let req = Request::PrLeftover {
            session: session.into(),
            repo: repo.into(),
            base: base.map(Into::into),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrLeftover(l) => Some(l),
            _ => None,
        })
    }

    /// `pr.reset_file`: empty the PR file before asking the agent; returns its path.
    pub async fn pr_reset_file(&self, session: &str, repo: &str) -> Result<String, ProtoError> {
        let req = Request::PrResetFile {
            session: session.into(),
            repo: repo.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrFile { path } => Some(path),
            _ => None,
        })
    }

    /// `pr.drift`: HEAD, uncommitted files and the commits since `since`.
    pub async fn pr_drift(
        &self,
        session: &str,
        repo: &str,
        since: Option<&str>,
        allowed: Vec<String>,
    ) -> Result<PrDrift, ProtoError> {
        let req = Request::PrDrift {
            session: session.into(),
            repo: repo.into(),
            since: since.map(Into::into),
            allowed,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrDrift(d) => Some(d),
            _ => None,
        })
    }

    /// `pr.unmerged`: work the session's base lacks ("커밋 보유"); `None` = cannot tell.
    pub async fn pr_unmerged(
        &self,
        session: &str,
        repo: &str,
    ) -> Result<Option<UnmergedWork>, ProtoError> {
        let req = Request::PrUnmerged {
            session: session.into(),
            repo: repo.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrUnmerged(u) => Some(u),
            _ => None,
        })
    }

    /// `pr.commit_preview`: PR text from the commits `origin/<base>` lacks (fetches).
    pub async fn pr_commit_preview(
        &self,
        session: &str,
        repo: &str,
        base: &str,
        push: bool,
    ) -> Result<CommitPrPreview, ProtoError> {
        let req = Request::PrCommitPreview {
            session: session.into(),
            repo: repo.into(),
            base: base.into(),
            push,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrCommitPreview(p) => Some(p),
            _ => None,
        })
    }

    /// `pr.commit_text`: write the PR file from the commits; returns the title.
    pub async fn pr_commit_text(
        &self,
        session: &str,
        repo: &str,
        base: &str,
        options: CommitPrTextOptions,
    ) -> Result<String, ProtoError> {
        let req = Request::PrCommitText {
            session: session.into(),
            repo: repo.into(),
            base: base.into(),
            options,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrTitle(t) => Some(t),
            _ => None,
        })
    }

    /// `pr.create`: push (unless `push` false) and open the PR into `base` from the PR file.
    #[allow(clippy::too_many_arguments)]
    pub async fn pr_create(
        &self,
        session: &str,
        repo: &str,
        base: &str,
        draft: bool,
        push: bool,
        push_sha: Option<&str>,
        options: Option<CommitPrTextOptions>,
    ) -> Result<PullRequestResult, ProtoError> {
        let req = Request::PrCreate {
            session: session.into(),
            repo: repo.into(),
            base: base.into(),
            draft,
            push,
            push_sha: push_sha.map(Into::into),
            options,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrCreated(c) => Some(c),
            _ => None,
        })
    }

    /// `pr.merge`: merge PR `number` only if its head is still `sha`.
    pub async fn pr_merge(
        &self,
        session: &str,
        repo: &str,
        number: u64,
        sha: &str,
        method: PrMergeMethod,
        delete_branch: bool,
    ) -> Result<PrMerged, ProtoError> {
        let req = Request::PrMerge {
            session: session.into(),
            repo: repo.into(),
            number,
            sha: sha.into(),
            method,
            delete_branch,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrMerged(m) => Some(m),
            _ => None,
        })
    }

    /// `pr.close`: close open PR `number` without merging.
    pub async fn pr_close(&self, session: &str, repo: &str, number: u64) -> Result<PrClosed, ProtoError> {
        let req = Request::PrClose {
            session: session.into(),
            repo: repo.into(),
            number,
        };
        expect(self.call(req).await, |r| match r {
            Response::PrClosed(c) => Some(c),
            _ => None,
        })
    }

    /// `pr.update_branch`: GitHub's "Update branch"; GitHub's message, if any.
    pub async fn pr_update_branch(
        &self,
        session: &str,
        repo: &str,
        number: u64,
        sha: &str,
    ) -> Result<Option<String>, ProtoError> {
        let req = Request::PrUpdateBranch {
            session: session.into(),
            repo: repo.into(),
            number,
            sha: sha.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::PrBranchUpdated { message } => Some(message),
            _ => None,
        })
    }

    /// `prompt.commit`: Desk's "커밋 맡기기" prompt for `files` (`all` = every change).
    pub async fn prompt_commit(
        &self,
        session: &str,
        repo: &str,
        files: Vec<String>,
        all: bool,
    ) -> Result<String, ProtoError> {
        let req = Request::PromptCommit {
            session: session.into(),
            repo: repo.into(),
            files,
            all,
        };
        expect(self.call(req).await, |r| match r {
            Response::Prompt(p) => Some(p),
            _ => None,
        })
    }

    /// `prompt.pr`: Desk's PR prompt for `target` (names the PR file).
    pub async fn prompt_pr(
        &self,
        session: &str,
        repo: &str,
        target: &str,
        exclude: PrExclusions,
    ) -> Result<String, ProtoError> {
        let req = Request::PromptPr {
            session: session.into(),
            repo: repo.into(),
            target: target.into(),
            exclude,
        };
        expect(self.call(req).await, |r| match r {
            Response::Prompt(p) => Some(p),
            _ => None,
        })
    }

    /// `session.send_prompt`: submit `text` to the session's agent (`prompt-not-ready` while a
    /// TUI is not at its input; `session-not-running`).
    pub async fn send_prompt(&self, session: &str, text: &str) -> Result<(), ProtoError> {
        let req = Request::SessionSendPrompt {
            session: session.into(),
            text: text.into(),
        };
        expect(self.call(req).await, |r| match r {
            Response::Unit => Some(()),
            _ => None,
        })
    }
}

/// Results of one [`Client::search_text`]. Dropping it before `Done` cancels the search.
pub struct SearchStream {
    id: u64,
    rx: mpsc::UnboundedReceiver<SearchEvent>,
    inner: std::sync::Weak<Inner>,
    done: bool,
}

impl std::fmt::Debug for SearchStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchStream")
            .field("id", &self.id)
            .field("done", &self.done)
            .finish()
    }
}

impl SearchStream {
    /// The search id (as in `Notify::SearchHits`).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Next batch, then `Done`, then `None`. `None` before `Done` = the connection dropped.
    pub async fn next(&mut self) -> Option<SearchEvent> {
        if self.done {
            return None;
        }
        let e = self.rx.recv().await;
        if matches!(e, Some(SearchEvent::Done(_)) | None) {
            self.done = true;
        }
        e
    }

    /// Wait for everything: all hits and the summary (`None` if the connection dropped).
    pub async fn collect(mut self) -> (Vec<SearchMatch>, Option<SearchSummary>) {
        let mut hits = Vec::new();
        while let Some(e) = self.next().await {
            match e {
                SearchEvent::Hits(h) => hits.extend(h),
                SearchEvent::Done(s) => return (hits, Some(s)),
            }
        }
        (hits, None)
    }

    /// Ask the server to stop; a `Done` with `cancelled` follows.
    pub fn cancel(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let search = self.id;
        rt.spawn(async move {
            let _ = inner.call(Request::SearchCancel { search }).await;
        });
    }
}

impl Drop for SearchStream {
    fn drop(&mut self) {
        if !self.done {
            self.cancel();
            if let Some(inner) = self.inner.upgrade()
                && let Some(c) = inner.current()
            {
                c.searches.lock().unwrap().remove(&self.id);
            }
        }
    }
}

fn expect<T>(
    r: Result<Response, ProtoError>,
    f: impl FnOnce(Response) -> Option<T>,
) -> Result<T, ProtoError> {
    f(r?).ok_or_else(|| ProtoError::new("bad-message").arg("detail", "unexpected response"))
}

fn expect_repo(r: Result<Response, ProtoError>) -> Result<RepoInfo, ProtoError> {
    expect(r, |r| match r {
        Response::Repo(r) => Some(r),
        _ => None,
    })
}

fn expect_unit(r: Result<Response, ProtoError>) -> Result<(), ProtoError> {
    expect(r, |r| matches!(r, Response::Unit).then_some(()))
}

fn expect_session(r: Result<Response, ProtoError>) -> Result<SessionInfo, ProtoError> {
    expect(r, |r| match r {
        Response::Session(s) => Some(s),
        _ => None,
    })
}

fn expect_written(r: Result<Response, ProtoError>) -> Result<String, ProtoError> {
    expect(r, |r| match r {
        Response::Written { version } => Some(version),
        _ => None,
    })
}

fn expect_account(r: Result<Response, ProtoError>) -> Result<AccountInfo, ProtoError> {
    expect(r, |r| match r {
        Response::Account(a) => Some(a),
        _ => None,
    })
}

fn expect_configuration(r: Result<Response, ProtoError>) -> Result<ConfigurationInfo, ProtoError> {
    expect(r, |r| match r {
        Response::Configuration(c) => Some(c),
        _ => None,
    })
}

impl Inner {
    fn current(&self) -> Option<Arc<Conn>> {
        self.conn.lock().unwrap().clone()
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn call(&self, req: Request) -> Result<Response, ProtoError> {
        let conn = self.current().ok_or_else(disconnected)?;
        conn.call(self.next_id(), req, self.opts.request_timeout)
            .await
    }

    async fn attach_on(
        self: &Arc<Self>,
        conn: &Arc<Conn>,
        att: &Arc<Attachment>,
    ) -> Result<Attached, ProtoError> {
        let mut st = att.state.lock().await;
        let resp = conn
            .call(
                self.next_id(),
                Request::TermAttach {
                    session: att.session.clone(),
                    from_offset: st.next,
                },
                self.opts.request_timeout,
            )
            .await?;
        let Response::Attached(a) = resp else {
            return Err(ProtoError::new("bad-message").arg("detail", "unexpected response"));
        };
        let stream = conn
            .mux
            .take_incoming(a.stream)
            .ok_or_else(|| ProtoError::internal("terminal stream missing"))?;
        st.epoch += 1;
        let epoch = st.epoch;
        st.next = Some(a.start_offset);
        if a.reset {
            st.clean = true;
            let _ = self
                .term_tx
                .send(ClientEvent::Term(TermChunk {
                    session: att.session.clone(),
                    offset: a.start_offset,
                    reset: true,
                    bytes: Bytes::new(),
                }))
                .await;
        }
        drop(st);
        tokio::spawn(pump(self.clone(), att.clone(), epoch, stream));
        Ok(a)
    }
}

/// Decode a terminal stream and deliver chunks exactly once (trimming overlap by offset).
async fn pump(inner: Arc<Inner>, att: Arc<Attachment>, epoch: u64, mut stream: Stream) {
    let mut dec = ChunkDecoder::default();
    while let Some(data) = stream.recv().await {
        dec.push(&data);
        loop {
            let (offset, reset, bytes) = match dec.next_record() {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("terminal stream {}: {e}", att.session);
                    return;
                }
            };
            let mut st = att.state.lock().await;
            if st.epoch != epoch || att.detached.load(Ordering::Acquire) {
                return;
            }
            let next = st.next.unwrap_or(offset);
            let (offset, bytes, reset) =
                if (reset && !(st.clean && offset == next)) || offset > next {
                    (offset, bytes, true)
                } else {
                    let skip = (next - offset) as usize;
                    if skip >= bytes.len() {
                        continue;
                    }
                    (next, bytes.slice(skip..), false)
                };
            st.next = Some(offset + bytes.len() as u64);
            st.clean = false;
            let chunk = TermChunk {
                session: att.session.clone(),
                offset,
                reset,
                bytes,
            };
            if inner.term_tx.send(ClientEvent::Term(chunk)).await.is_err() {
                return;
            }
        }
    }
}

async fn reader(conn: Arc<Conn>, mut rx: RpcReceiver, ctrl_tx: mpsc::UnboundedSender<ClientEvent>) {
    while let Some(m) = rx.recv().await {
        match RpcMessage::decode(&m) {
            Ok(RpcMessage::Response { id, result }) => {
                let tx = conn.pending.lock().unwrap().remove(&id);
                if let Some(tx) = tx {
                    let _ = tx.send(result);
                }
            }
            Ok(RpcMessage::Notify(n)) => {
                // Search results go to their stream; unknown searches fall through.
                let n = match n {
                    Notify::SearchHits { search, hits } => {
                        let tx = conn.searches.lock().unwrap().get(&search).cloned();
                        match tx {
                            Some(tx) => {
                                let _ = tx.send(SearchEvent::Hits(hits));
                                continue;
                            }
                            None => Notify::SearchHits { search, hits },
                        }
                    }
                    Notify::SearchDone { search, summary } => {
                        let tx = conn.searches.lock().unwrap().remove(&search);
                        match tx {
                            Some(tx) => {
                                let _ = tx.send(SearchEvent::Done(summary));
                                continue;
                            }
                            None => Notify::SearchDone { search, summary },
                        }
                    }
                    other => other,
                };
                let _ = ctrl_tx.send(ClientEvent::Notify(n));
            }
            Ok(RpcMessage::Request { .. }) => {}
            Err(e) => tracing::warn!("bad message from server: {e}"),
        }
    }
    conn.pending.lock().unwrap().clear();
    conn.searches.lock().unwrap().clear();
}

async fn connect_once(inner: &Arc<Inner>) -> Result<(Arc<Conn>, Welcome), ProtoError> {
    let opts = &inner.opts;
    let c = match tokio::time::timeout(opts.connect_timeout, inner.transport.connect()).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return Err(ProtoError::new("connect-failed").arg("detail", e.to_string()));
        }
        Err(_) => return Err(ProtoError::new("timeout")),
    };
    let (mux, rx) = Mux::new(c.io, MuxConfig::new(Role::Client));
    let conn = Arc::new(Conn {
        mux,
        pending: Mutex::new(HashMap::new()),
        searches: Mutex::new(HashMap::new()),
        _child: Mutex::new(c.child),
    });
    tokio::spawn(reader(conn.clone(), rx, inner.ctrl_tx.clone()));
    let hello = Hello {
        proto_version: PROTO_VERSION,
        client_version: opts.client_version.clone(),
        resume: inner.token.lock().unwrap().clone(),
        agent: None,
    };
    let r = conn
        .call(inner.next_id(), Request::Hello(hello), opts.connect_timeout)
        .await;
    let fail = |e: ProtoError| {
        conn.mux.close();
        Err(e)
    };
    match r {
        Ok(Response::Welcome(w)) if w.proto_version == PROTO_VERSION => Ok((conn.clone(), w)),
        Ok(Response::Welcome(w)) => fail(
            ProtoError::new("proto-version")
                .arg("server", w.proto_version.to_string())
                .arg("client", PROTO_VERSION.to_string()),
        ),
        Ok(_) => fail(ProtoError::new("bad-message").arg("detail", "expected welcome")),
        Err(e) => fail(e),
    }
}

async fn run(inner: Arc<Inner>) {
    let mut shutdown = inner.shutdown.subscribe();
    let mut backoff = inner.opts.backoff_min;
    let mut failure_reported = false;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let attempt = tokio::select! {
            r = connect_once(&inner) => r,
            _ = shutdown.wait_for(|s| *s) => break,
        };
        match attempt {
            Ok((conn, welcome)) => {
                backoff = inner.opts.backoff_min;
                failure_reported = false;
                // Offsets are per server instance: after a server restart, replay fully.
                let new_instance = {
                    let mut inst = inner.instance.lock().unwrap();
                    let changed = inst
                        .as_deref()
                        .is_some_and(|i| i != welcome.server_instance);
                    *inst = Some(welcome.server_instance.clone());
                    changed
                };
                let atts: Vec<Arc<Attachment>> = inner
                    .attachments
                    .lock()
                    .unwrap()
                    .values()
                    .cloned()
                    .collect();
                if new_instance {
                    for a in &atts {
                        a.state.lock().await.next = None;
                    }
                }
                *inner.token.lock().unwrap() = Some(welcome.resume_token.clone());
                *inner.conn.lock().unwrap() = Some(conn.clone());
                inner.status.send_modify(|s| {
                    s.connected = Some(welcome.clone());
                    s.attempts += 1;
                    s.last_error = None;
                });
                let _ = inner.ctrl_tx.send(ClientEvent::Connected(welcome));
                let watched: Vec<(String, String)> =
                    inner.watched.lock().unwrap().iter().cloned().collect();
                for (session, repo) in watched {
                    let (inner2, conn2) = (inner.clone(), conn.clone());
                    tokio::spawn(async move {
                        let r = conn2
                            .call(
                                inner2.next_id(),
                                Request::FsWatch {
                                    session: session.clone(),
                                    repo: repo.clone(),
                                },
                                inner2.opts.request_timeout,
                            )
                            .await;
                        if let Err(e) = r {
                            tracing::debug!("re-watch {session}: {e}");
                        }
                    });
                }
                for a in atts {
                    let (inner2, conn2) = (inner.clone(), conn.clone());
                    tokio::spawn(async move {
                        if let Err(error) = inner2.attach_on(&conn2, &a).await
                            && !a.detached.load(Ordering::Acquire)
                            && !conn2.mux.is_closed()
                        {
                            let _ = inner2.ctrl_tx.send(ClientEvent::AttachFailed {
                                session: a.session.clone(),
                                error,
                            });
                        }
                    });
                }
                tokio::select! {
                    _ = conn.mux.closed() => {}
                    _ = shutdown.wait_for(|s| *s) => {}
                }
                conn.mux.close();
                *inner.conn.lock().unwrap() = None;
                inner.status.send_modify(|s| s.connected = None);
                let _ = inner
                    .ctrl_tx
                    .send(ClientEvent::Disconnected { error: None });
            }
            Err(e) => {
                tracing::debug!("connect to {} failed: {e}", inner.transport.describe());
                inner.status.send_modify(|s| {
                    s.attempts += 1;
                    s.last_error = Some(e.clone());
                });
                if !failure_reported {
                    failure_reported = true;
                    let _ = inner
                        .ctrl_tx
                        .send(ClientEvent::Disconnected { error: Some(e) });
                }
                if !inner.opts.reconnect {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = inner.kick.notified() => {}
                    _ = shutdown.wait_for(|s| *s) => break,
                }
                backoff = (backoff * 2).min(inner.opts.backoff_max);
                continue;
            }
        }
        if !inner.opts.reconnect {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(inner.opts.backoff_min) => {}
            _ = shutdown.wait_for(|s| *s) => break,
        }
    }
    if let Some(c) = inner.conn.lock().unwrap().take() {
        c.mux.close();
    }
}

/// The client's event stream (see [`ClientEvent`]). Connection events and notifications are
/// delivered before queued terminal output. Not reading it throttles terminal streams (the
/// server keeps buffering in its ring) but never blocks RPC.
pub struct ClientEvents {
    ctrl: Option<mpsc::UnboundedReceiver<ClientEvent>>,
    term: Option<mpsc::Receiver<ClientEvent>>,
    slot: EventSlot,
}

impl ClientEvents {
    /// Next event; `None` once the client is gone.
    pub async fn next(&mut self) -> Option<ClientEvent> {
        let ctrl = self.ctrl.as_mut()?;
        let term = self.term.as_mut()?;
        tokio::select! {
            biased;
            Some(e) = ctrl.recv() => Some(e),
            Some(e) = term.recv() => Some(e),
            else => None,
        }
    }

    /// Next event if one is ready.
    pub fn try_next(&mut self) -> Option<ClientEvent> {
        if let Ok(e) = self.ctrl.as_mut()?.try_recv() {
            return Some(e);
        }
        self.term.as_mut()?.try_recv().ok()
    }
}

impl Drop for ClientEvents {
    fn drop(&mut self) {
        if let (Some(c), Some(t)) = (self.ctrl.take(), self.term.take()) {
            *self.slot.lock().unwrap() = Some((c, t));
        }
    }
}

/// What `uterm.open` asks for: a shell (or `command`) in `repo`'s worktree / its `folder`, the
/// session folder, or for a 공용 terminal (empty `session`) `cwd` or the home folder.
#[derive(Debug, Clone, Default)]
pub struct UTermOpen {
    pub session: String,
    pub repo: Option<String>,
    pub folder: Option<String>,
    pub command: Option<String>,
    pub title: Option<String>,
    /// `uterm.shells` id (`None` = the default).
    pub shell: Option<String>,
    pub cwd: Option<String>,
}
