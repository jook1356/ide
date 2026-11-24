//! The agent bridge (`docs/agent-bridge.md`): connects a session's agent CLI to the IDE.
//!
//! - Every agent session gets an MCP server, `ide`, run by the agent CLI as
//!   `ide-server mcp` ([`run_mcp_stdio`]): a stdio MCP server that connects to the daemon's
//!   local socket as an *agent connection* (`Hello.agent`: session id + the session's bridge
//!   token) and forwards `tools/list` / `tools/call` as `agent.tools` / `agent.call`.
//! - The daemon answers a tool itself (e.g. [`TOOL_SESSION`]) or hands it to the connected
//!   apps as `Notify::AgentRequest` and waits for the first `agent.reply` (tools that act in
//!   the UI, e.g. [`TOOL_SHOW_MESSAGE`]).
//! - Tokens are per session and per daemon run (random, kept in memory): an agent connection
//!   acts for its own session only and may send only `agent.*` requests.
//!
//! The bridge needs the daemon's socket: a server without one (`--stdio --no-daemon`) passes
//! no bridge to its sessions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ide_agents::{Bridge, HookDecision, HookEvent, HookTargets, ToolUse};
use ide_core::McpServer;
use ide_proto::{
    AgentActivity, AgentHookEvent, AgentTool, AgentToolResult, Notify, ProtoError, SessionInfo,
};
use tokio::sync::oneshot;

/// Name of the bridge's MCP server (reserved: a configuration server of this name is left out).
/// Not `ide`: Claude Code takes an MCP server of that name for its own IDE integration (VS Code
/// extension: `closeAllDiffTabs`, ...) and does not offer it as a normal tool server.
pub const MCP_NAME: &str = "ide-bridge";
/// Env vars of the agent process and of the bridge's MCP server.
pub const SESSION_ENV: &str = "IDE_SESSION_ID";
pub const TOKEN_ENV: &str = "IDE_AGENT_TOKEN";
pub use crate::user::SOCKET_ENV;

/// `ide_session`: this session as JSON (answered by the daemon).
pub use ide_proto::AGENT_TOOL_SESSION as TOOL_SESSION;
/// `ide_show_message`: show a short message in the session's panel (answered by an app).
pub use ide_proto::AGENT_TOOL_SHOW_MESSAGE as TOOL_SHOW_MESSAGE;

/// `ide_widget`: the session's widgets (`docs/widgets.md`).
pub use ide_proto::AGENT_TOOL_WIDGET as TOOL_WIDGET;

/// `ide_preview`: show a page, Markdown, a file or a site in a tab (`docs/previews.md`).
pub use ide_proto::AGENT_TOOL_PREVIEW as TOOL_PREVIEW;

/// Session family tools (`docs/session-family.md`), answered by the daemon.
pub const TOOL_LIST_RELATED: &str = "ide_list_related_sessions";
pub const TOOL_CREATE_CHILD: &str = "ide_create_child_session";
pub const TOOL_SEND: &str = "ide_send_to_session";
pub const TOOL_MERGE_CHILD: &str = "ide_merge_child";
pub const TOOL_SYNC_PARENT: &str = "ide_sync_from_parent";
pub const TOOL_REQUEST_WRITE: &str = "ide_request_write_access";
pub const TOOL_RELEASE_WRITE: &str = "ide_release_write_access";
pub const TOOL_READ_SESSION: &str = "ide_read_session";
pub const TOOL_SENT_MESSAGES: &str = "ide_sent_messages";

/// Terminal tools (`docs/terminals.md`), answered by the daemon (`service/terminals.rs`).
pub const TOOL_TERMINAL_LIST: &str = "ide_terminal_list";
pub const TOOL_TERMINAL_READ: &str = "ide_terminal_read";
pub const TOOL_TERMINAL_INPUT: &str = "ide_terminal_input";

/// A tool of the session family.
pub fn is_family_tool(tool: &str) -> bool {
    [
        TOOL_LIST_RELATED,
        TOOL_CREATE_CHILD,
        TOOL_SEND,
        TOOL_MERGE_CHILD,
        TOOL_SYNC_PARENT,
        TOOL_REQUEST_WRITE,
        TOOL_RELEASE_WRITE,
        TOOL_READ_SESSION,
        TOOL_SENT_MESSAGES,
    ]
    .contains(&tool)
}

/// How long a tool carried out by an app may take.
pub(crate) const APP_TIMEOUT: Duration = Duration::from_secs(60);

/// Base instructions of every bridged session (appended to the configuration's).
const INSTRUCTIONS: &str = "\
## ide
You are running inside ide, the IDE that manages this session: its terminal, its \
repositories / worktrees and the panels around it. The `ide-bridge` MCP server connects you to the \
IDE. Use its tools (names start with `ide_`) instead of guessing about this session or the \
IDE: `ide_session` describes this session (folder, repositories, branches, base branches); \
`ide_show_message` shows a short note to the user in the IDE. Child sessions \
(`ide_create_child_session`) take parallel work; talk to related sessions with \
`ide_send_to_session`. To show the user something richer than text (a board, a form, a \
chart, a small tool), build a widget: call `ide_widget` with action `guide` first. To show \
the user a page, a Markdown document, an image or a site, use `ide_preview` (action `guide` \
explains it).";

/// Where the bridge's MCP server connects to: the daemon's socket and the `ide-server`
/// program (`ServerConfig::bridge`; only a daemon has one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    /// The `ide-server` executable the agent CLI runs as `ide-server mcp`.
    pub exe: PathBuf,
    /// The daemon's local socket (pipe name / socket path).
    pub socket: String,
}

/// A user's bridge state: the config, the sessions' tokens and the app calls in flight.
#[derive(Default)]
pub struct BridgeState {
    config: Option<BridgeConfig>,
    tokens: Mutex<HashMap<String, String>>,
    /// What each session's agent is doing (hooks / screen), until its process ends.
    activity: Mutex<HashMap<String, AgentActivity>>,
    /// Sessions whose CLI reported through hooks since its process started.
    hooked: Mutex<std::collections::HashSet<String>>,
    /// Work of each session's agent that can outlive its turn (hooks' `Background*`).
    background: Mutex<HashMap<String, Background>>,
    /// Write access of shared folders: folder owner's session id -> the holder's (missing =
    /// the owner).
    leases: Mutex<HashMap<String, String>>,
    /// Messages waiting to be typed into a session (`ide_send_to_session`, first prompts).
    queue: Mutex<HashMap<String, std::collections::VecDeque<Queued>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<String, String>>>>,
    seq: AtomicU64,
}

/// A message waiting to be typed into a session.
#[derive(Debug, Clone)]
pub struct Queued {
    pub id: u64,
    /// The sending session (`None`: ide itself).
    pub from: Option<String>,
    pub text: String,
    /// Unix seconds.
    pub at: u64,
}

/// A session's background work: the ids still running and whether its turn ended (`Stopped`
/// since the last `PromptSubmitted`). Provider-agnostic: only the hook events' ids.
#[derive(Debug, Default)]
struct Background {
    running: std::collections::HashSet<String>,
    turn_ended: bool,
}

impl BridgeState {
    pub fn new(config: Option<BridgeConfig>) -> Self {
        BridgeState {
            config,
            ..Default::default()
        }
    }

    /// Where hooks report to: the daemon's `ide-server` (none without a daemon).
    pub fn hook_targets(&self) -> HookTargets {
        HookTargets {
            program: self.config.as_ref().map(|c| c.exe.clone()),
            trusted_mcp: None,
        }
    }

    /// The bridge of a session (creates its token on first use), `None` without a daemon.
    pub fn for_session(&self, session: &str) -> Option<Bridge> {
        let c = self.config.as_ref()?;
        let token = self
            .tokens
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_insert_with(crate::service::random_token)
            .clone();
        let env = vec![
            (SESSION_ENV.to_owned(), session.to_owned()),
            (TOKEN_ENV.to_owned(), token),
            (SOCKET_ENV.to_owned(), c.socket.clone()),
        ];
        Some(Bridge {
            server: McpServer {
                name: MCP_NAME.to_owned(),
                command: c.exe.display().to_string(),
                args: vec!["mcp".to_owned()],
                // MCP clients start servers with a reduced environment: pass ours explicitly.
                env: env.clone(),
            },
            env,
            instructions: INSTRUCTIONS.to_owned(),
            hooks: HookTargets {
                program: Some(c.exe.clone()),
                trusted_mcp: Some(MCP_NAME.to_owned()),
            },
        })
    }

    /// A session's agent activity.
    pub fn activity(&self, session: &str) -> AgentActivity {
        self.activity
            .lock()
            .unwrap()
            .get(session)
            .copied()
            .unwrap_or_default()
    }

    /// A new process of `session`: nothing reported yet.
    pub fn reset(&self, session: &str) {
        self.activity.lock().unwrap().remove(session);
        self.hooked.lock().unwrap().remove(session);
        self.background.lock().unwrap().remove(session);
    }

    /// The activity a hook event of `session` means (`None`: unchanged). A turn that ends
    /// while background work runs stays working until the last of it ends.
    pub fn hook_activity(&self, session: &str, event: &AgentHookEvent) -> Option<AgentActivity> {
        let mut all = self.background.lock().unwrap();
        let bg = all.entry(session.to_owned()).or_default();
        Some(match event {
            AgentHookEvent::SessionStart { .. } => {
                *bg = Background::default();
                AgentActivity::Idle
            }
            AgentHookEvent::PromptSubmitted => {
                bg.turn_ended = false;
                AgentActivity::Working
            }
            AgentHookEvent::Stopped | AgentHookEvent::StoppedRunning { .. } => {
                if let AgentHookEvent::StoppedRunning { background } = event {
                    // The CLI's own account replaces what was counted (ends no hook reports).
                    bg.running = background.iter().cloned().collect();
                }
                bg.turn_ended = true;
                if bg.running.is_empty() {
                    AgentActivity::Idle
                } else {
                    AgentActivity::Working
                }
            }
            // Tool calls also come from background work: they do not reopen the turn.
            AgentHookEvent::PreEdit { .. }
            | AgentHookEvent::PreShell { .. }
            | AgentHookEvent::PreTool { .. } => AgentActivity::Working,
            AgentHookEvent::NeedsInput { .. } => AgentActivity::NeedsInput,
            AgentHookEvent::BackgroundStarted { id } => {
                bg.running.insert(id.clone());
                return None;
            }
            AgentHookEvent::BackgroundEnded { id } => {
                // An unknown id (or a repeated end) changes nothing.
                if !bg.running.remove(id) || !(bg.turn_ended && bg.running.is_empty()) {
                    return None;
                }
                AgentActivity::Idle
            }
        })
    }

    /// Background work of `session` still runs.
    fn background_running(&self, session: &str) -> bool {
        self.background
            .lock()
            .unwrap()
            .get(session)
            .is_some_and(|b| !b.running.is_empty())
    }

    /// Who holds the write access of `owner`'s folder.
    pub fn writer(&self, owner: &str) -> String {
        self.leases
            .lock()
            .unwrap()
            .get(owner)
            .cloned()
            .unwrap_or_else(|| owner.to_owned())
    }

    /// Give the write access of `owner`'s folder to `holder`.
    pub fn set_writer(&self, owner: &str, holder: &str) {
        let mut l = self.leases.lock().unwrap();
        if holder == owner {
            l.remove(owner);
        } else {
            l.insert(owner.to_owned(), holder.to_owned());
        }
    }

    /// Queue a message for `session` (`from`: the sending session); its id.
    pub fn enqueue(&self, session: &str, from: Option<&str>, text: String) -> u64 {
        let id = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.queue
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_default()
            .push_back(Queued {
                id,
                from: from.map(str::to_owned),
                text,
                at: ide_core::now_unix(),
            });
        id
    }

    /// The next queued message of `session`.
    pub fn dequeue(&self, session: &str) -> Option<Queued> {
        self.queue.lock().unwrap().get_mut(session)?.pop_front()
    }

    /// Put a message back at the front (it could not be typed yet).
    pub fn requeue(&self, session: &str, msg: Queued) {
        self.queue
            .lock()
            .unwrap()
            .entry(session.to_owned())
            .or_default()
            .push_front(msg);
    }

    /// The messages `from` queued that are still waiting: (target session, message).
    pub fn sent_by(&self, from: &str) -> Vec<(String, Queued)> {
        let q = self.queue.lock().unwrap();
        let mut out: Vec<(String, Queued)> = q
            .iter()
            .flat_map(|(to, msgs)| {
                msgs.iter()
                    .filter(|m| m.from.as_deref() == Some(from))
                    .map(move |m| (to.clone(), m.clone()))
            })
            .collect();
        out.sort_by_key(|(_, m)| m.id);
        out
    }

    /// Drop a waiting message `from` queued (`false`: none with that id).
    pub fn cancel(&self, from: &str, id: u64) -> bool {
        let mut q = self.queue.lock().unwrap();
        for msgs in q.values_mut() {
            if let Some(i) = msgs.iter().position(|m| m.id == id && m.from.as_deref() == Some(from)) {
                msgs.remove(i);
                return true;
            }
        }
        false
    }

    /// Messages queued for `session`.
    pub fn pending(&self, session: &str) -> usize {
        self.queue.lock().unwrap().get(session).map_or(0, |q| q.len())
    }

    /// A hook of `session`'s CLI arrived.
    pub fn mark_hooked(&self, session: &str) {
        self.hooked.lock().unwrap().insert(session.to_owned());
    }

    /// The activity to apply for what the screen shows (`None`: keep the current one): without
    /// hooks the screen decides; with hooks it only adds a visible dialog (hooks report it late
    /// or not at all) and its end. An idle screen while background work runs is working.
    pub fn screen_says(&self, session: &str, seen: AgentActivity) -> Option<AgentActivity> {
        let applies = !self.hooked.lock().unwrap().contains(session)
            || seen == AgentActivity::NeedsInput
            || self.activity(session) == AgentActivity::NeedsInput;
        if !applies {
            return None;
        }
        if seen == AgentActivity::Idle && self.background_running(session) {
            return Some(AgentActivity::Working);
        }
        Some(seen)
    }

    /// Set a session's agent activity; `true` when it changed.
    pub fn set_activity(&self, session: &str, a: AgentActivity) -> bool {
        let mut m = self.activity.lock().unwrap();
        let old = m.insert(session.to_owned(), a).unwrap_or_default();
        old != a
    }

    /// Does `token` belong to `session`?
    pub fn check(&self, session: &str, token: &str) -> bool {
        !token.is_empty()
            && self
                .tokens
                .lock()
                .unwrap()
                .get(session)
                .is_some_and(|t| t == token)
    }

    /// Forget a session's token (deleted session).
    pub fn forget(&self, session: &str) {
        self.tokens.lock().unwrap().remove(session);
        // A deleted holder hands the access back to the folder's owner.
        self.leases.lock().unwrap().retain(|_, h| h != session);
        self.queue.lock().unwrap().remove(session);
        self.reset(session);
    }

    /// Register an app call: its request id and the receiver of the answer.
    fn begin(&self) -> (u64, oneshot::Receiver<Result<String, String>>) {
        let id = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    fn end(&self, id: u64) {
        self.pending.lock().unwrap().remove(&id);
    }

    /// An app's answer (`agent.reply`); later answers to the same request are ignored.
    pub fn reply(&self, id: u64, result: Result<String, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(result);
        }
    }
}

/// A provider's hook event on the wire.
pub fn wire_event(e: HookEvent) -> AgentHookEvent {
    match e {
        HookEvent::SessionStart { conversation } => AgentHookEvent::SessionStart { conversation },
        HookEvent::PromptSubmitted => AgentHookEvent::PromptSubmitted,
        HookEvent::Stopped => AgentHookEvent::Stopped,
        HookEvent::NeedsInput { permission } => AgentHookEvent::NeedsInput { permission },
        HookEvent::PreTool(ToolUse::Edit { tool, paths }) => AgentHookEvent::PreEdit { tool, paths },
        HookEvent::PreTool(ToolUse::Shell { tool, command, cwd }) => {
            AgentHookEvent::PreShell { tool, command, cwd }
        }
        HookEvent::PreTool(ToolUse::Other { tool }) => AgentHookEvent::PreTool { tool },
        HookEvent::BackgroundStarted { id } => AgentHookEvent::BackgroundStarted { id },
        HookEvent::BackgroundEnded { id } => AgentHookEvent::BackgroundEnded { id },
        HookEvent::StoppedRunning { background } => AgentHookEvent::StoppedRunning { background },
    }
}

/// The bridge's tools.
pub fn tools() -> Vec<AgentTool> {
    let mut all = core_tools();
    // Browser control (`docs/browser-control.md`).
    all.extend(crate::service::browser::tools());
    all
}

fn core_tools() -> Vec<AgentTool> {
    vec![
        AgentTool {
            name: TOOL_SESSION.into(),
            description: "Describe this ide session: id, title, agent, mode (workspace or \
                          original repository), its folder, and every repository with its \
                          worktree folder, branch and base branch."
                .into(),
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_SHOW_MESSAGE.into(),
            description: "Show a short message to the user in this session's ide panel (a \
                          notice that disappears after a few seconds). Fails when no ide \
                          window is open."
                .into(),
            input_schema: r#"{"type":"object","properties":{"message":{"type":"string","description":"The text to show (one or two sentences)."}},"required":["message"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_LIST_RELATED.into(),
            description: "List this session's related sessions: its parent, its children and its \
                          siblings, with id, title, relation, whether it shares the parent's \
                          folder, whether it runs, what its agent is doing, and its repositories \
                          (branch, folder)."
                .into(),
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#.into(),
        },
        AgentTool {
            name: TOOL_CREATE_CHILD.into(),
            description: "Create and start a child session of this session (same agent). \
                          `shared: true` works in this session's folder and worktrees (for \
                          reviews, research, questions); otherwise the child gets its own \
                          worktrees of `repos` (default: all of this session's), each branched \
                          from this session's branch, to work in parallel; merge it back later \
                          with ide_merge_child. `prompt` is typed into the child when its agent \
                          is ready. Children go at most two levels below the first session."
                .into(),
            input_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"shared":{"type":"boolean"},"repos":{"type":"array","items":{"type":"string"},"description":"Repository names of this session."},"prompt":{"type":"string","description":"The child's first task."}},"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_SEND.into(),
            description: "Send a message to a related session (parent, child or sibling; see \
                          ide_list_related_sessions), marked as coming from this session. \
                          `mode`: `queue` (default) types it in at once when that agent is \
                          idle, else when its current turn ends; `steer` types it in now, \
                          also during its turn (its agent sees it while working); `interrupt` \
                          stops its current turn first, then types it in. `steer` and \
                          `interrupt` ask the user for approval first. Check or cancel \
                          waiting messages with ide_sent_messages."
                .into(),
            input_schema: r#"{"type":"object","properties":{"session":{"type":"string","description":"The session id."},"message":{"type":"string"},"mode":{"type":"string","enum":["queue","steer","interrupt"],"description":"Default queue."}},"required":["session","message"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_READ_SESSION.into(),
            description: "Read the recent conversation of a related session (parent, child or \
                          sibling): user prompts, the agent's answers, tool calls and results \
                          (long ones shortened). Works for agents whose transcripts ide can read \
                          (Claude Code)."
                .into(),
            input_schema: r#"{"type":"object","properties":{"session":{"type":"string","description":"The session id."},"limit":{"type":"integer","description":"The last N entries (default 30, at most 200)."}},"required":["session"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_SENT_MESSAGES.into(),
            description: "Messages this session sent with ide_send_to_session that are still \
                          waiting to be typed in (`list`, the default), or drop one (`cancel` \
                          {id})."
                .into(),
            input_schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["list","cancel"]},"id":{"type":"integer","description":"The message id, for cancel."}},"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_TERMINAL_LIST.into(),
            description: "List the user's terminals of this session (the ones in ide's terminal \
                          panel, not your own): id, title, command, folder, state, and the TCP \
                          ports their processes listen on. `session`: a related session's \
                          terminals instead (read only)."
                .into(),
            input_schema: r#"{"type":"object","properties":{"session":{"type":"string","description":"A related session's id (default: this session)."}},"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_TERMINAL_READ.into(),
            description: "Read a terminal of this session or of a related session: its visible \
                          screen as text and up to `lines` earlier lines that scrolled above it \
                          (default 50, at most 2000; 0 = the screen only)."
                .into(),
            input_schema: r#"{"type":"object","properties":{"terminal":{"type":"string","description":"The terminal id (ide_terminal_list)."},"lines":{"type":"integer","description":"Scrollback lines above the screen (default 50)."}},"required":["terminal"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_TERMINAL_INPUT.into(),
            description: "Type into one of this session's terminals: `text` as typed (a line \
                          break presses Enter), then `keys` by name (enter, tab, esc, \
                          backspace, up, down, left, right, home, end, pageup, pagedown, \
                          f1-f12, ctrl+c, alt+f, shift+tab, a single character, ...). The user \
                          is asked to allow it first. Ctrl+C stops a running command: it is \
                          input too. Read the result with ide_terminal_read."
                .into(),
            input_schema: r#"{"type":"object","properties":{"terminal":{"type":"string","description":"The terminal id (ide_terminal_list)."},"text":{"type":"string"},"keys":{"type":"array","items":{"type":"string"},"description":"Key names pressed after the text, e.g. [\"enter\"] or [\"ctrl+c\"]."}},"required":["terminal"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_MERGE_CHILD.into(),
            description: "Merge a child session's committed work into this session's worktrees \
                          (per repository: merged, up to date, conflicts left in progress for \
                          you to resolve and commit, or failed with nothing changed). `squash` \
                          makes one commit per repository. Uncommitted work is not merged."
                .into(),
            input_schema: r#"{"type":"object","properties":{"child":{"type":"string","description":"The child session's id."},"squash":{"type":"boolean"}},"required":["child"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_REQUEST_WRITE.into(),
            description: "In a folder shared with other sessions, only the session holding its \
                          write access may edit files (edits of the others are refused). Ask \
                          for it here: granted at once when the holder is not working, else \
                          refused (ask the holder with ide_send_to_session, or work in a \
                          separate child session)."
                .into(),
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#.into(),
        },
        AgentTool {
            name: TOOL_RELEASE_WRITE.into(),
            description: "Give the shared folder's write access back to the folder's owner \
                          when you are done editing."
                .into(),
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#.into(),
        },
        AgentTool {
            name: TOOL_WIDGET.into(),
            description: "Widgets: small web apps (HTML/JS, any framework) you draft in this \
                          session's folder and register once; then every session can open \
                          them as a tab next to the terminal. `guide`: how to build one \
                          (folder, widget.json, the window.ide API: ide tools and stored \
                          data). `list`: registered widgets and this session's drafts. \
                          `register` {widget}: register or update a draft (the user is asked \
                          first for a new widget, another session's widget or changed \
                          tools). `unregister` {widget}: remove one this session registered. \
                          `open` / `close` {widget}: show or close a registered widget here \
                          (open again to reload it after registering changes)."
                .into(),
            input_schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["guide","list","register","unregister","open","close"]},"widget":{"type":"string","description":"The widget id (its folder name), for register / unregister / open / close."}},"required":["action"],"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_PREVIEW.into(),
            description: "Show the user something in a tab next to your terminal (no approval \
                          needed): `kind` `html` / `markdown` with `content` (a page you write, \
                          Markdown rendered), `file` with `path` (an HTML page, Markdown file or \
                          image inside this session's folder), `url` with `url` (a web page, \
                          e.g. your dev server). The same `id` (default: from the title) \
                          replaces the preview and reloads its tab. `guide`: details; `list`; \
                          `close` {id}."
                .into(),
            input_schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["show","guide","list","close"],"description":"Default show."},"title":{"type":"string","description":"The tab's title (show)."},"kind":{"type":"string","enum":["html","markdown","file","url"]},"content":{"type":"string","description":"HTML or Markdown text (kind html / markdown)."},"path":{"type":"string","description":"A file inside the session folder, relative to it or absolute (kind file)."},"url":{"type":"string","description":"An http(s) address (kind url)."},"id":{"type":"string","description":"1-40 of a-z, 0-9 and -; the same id replaces the preview."}},"additionalProperties":false}"#
                .into(),
        },
        AgentTool {
            name: TOOL_SYNC_PARENT.into(),
            description: "Merge the parent session's newer commits into this child session's \
                          worktrees (same results as ide_merge_child)."
                .into(),
            input_schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#.into(),
        },
    ]
}

fn ok(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: text.into(),
        is_error: false,
        image: None,
    }
}

fn err(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: text.into(),
        is_error: true,
        image: None,
    }
}

/// `ide_session`'s answer.
pub fn session_json(s: &SessionInfo) -> String {
    let repos: Vec<serde_json::Value> = s
        .repos
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "folder": r.path,
                "branch": r.branch,
                "base": r.base,
            })
        })
        .collect();
    let v = serde_json::json!({
        "id": s.id,
        "title": s.title,
        "agent": s.provider,
        "mode": match s.mode {
            ide_proto::SessionMode::Workspace => "workspace",
            ide_proto::SessionMode::OriginalRepo => "original-repo",
        },
        "folder": s.root,
        "repositories": repos,
    });
    serde_json::to_string_pretty(&v).expect("json")
}

/// Carry out a tool call of `session`'s agent. `info` resolves the session (blocking); `apps`
/// tells whether an app is connected and `notify` hands a request to the apps.
pub async fn call(
    state: &BridgeState,
    session: &str,
    tool: &str,
    args: &str,
    info: impl FnOnce() -> Result<SessionInfo, ProtoError>,
    apps: bool,
    notify: impl Fn(Notify),
) -> AgentToolResult {
    let args: serde_json::Value = match serde_json::from_str(if args.trim().is_empty() {
        "{}"
    } else {
        args
    }) {
        Ok(v) => v,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    match tool {
        TOOL_SESSION => match info() {
            Ok(s) => ok(session_json(&s)),
            Err(e) => err(format!("session not available: {e}")),
        },
        TOOL_SHOW_MESSAGE => {
            let Some(message) = args
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::trim)
                .filter(|m| !m.is_empty())
            else {
                return err("`message` is required");
            };
            let args = serde_json::json!({ "message": message }).to_string();
            ask_app(state, session, tool, args, apps, notify).await
        }
        _ => err(format!("unknown tool `{tool}`")),
    }
}

/// Hand a tool call to the connected apps (`Notify::AgentRequest`) and wait for the first
/// answer (`agent.reply`), at most [`APP_TIMEOUT`]. `apps`: an app is connected.
pub async fn ask_app(
    state: &BridgeState,
    session: &str,
    tool: &str,
    args: String,
    apps: bool,
    notify: impl Fn(Notify),
) -> AgentToolResult {
    ask_app_within(state, session, tool, args, || apps, notify, Some(APP_TIMEOUT)).await
}

/// The longest any question waits, even "until answered": below the CLIs' `PreToolUse` hook
/// limit ([`ide_agents::hooks::TOOL_HOOK_SECS`]), so ide's refusal arrives before the CLI gives
/// up on the hook (and runs the tool).
pub const MAX_ASK: Duration = Duration::from_secs(ide_agents::hooks::TOOL_HOOK_SECS - 120);
/// How often a question without a time limit checks that an app is still connected.
const APPS_CHECK: Duration = Duration::from_secs(2);

/// [`ask_app`] waiting at most `timeout` (a question to the user takes longer).
pub async fn ask_app_within(
    state: &BridgeState,
    session: &str,
    tool: &str,
    args: String,
    apps: impl Fn() -> bool,
    notify: impl Fn(Notify),
    timeout: Option<Duration>,
) -> AgentToolResult {
    if !apps() {
        return err(NO_WINDOW);
    }
    let (id, mut rx) = state.begin();
    notify(Notify::AgentRequest {
        request: id,
        session: session.to_owned(),
        tool: tool.to_owned(),
        args,
    });
    let limit = timeout.unwrap_or(MAX_ASK).min(MAX_ASK);
    let start = tokio::time::Instant::now();
    // Waits in steps: a question without a time limit gives up when the last app went (it
    // would wait for nobody).
    let r = loop {
        let left = limit.saturating_sub(start.elapsed());
        if left.is_zero() {
            break Err(TIMED_OUT);
        }
        match tokio::time::timeout(left.min(APPS_CHECK), &mut rx).await {
            Ok(Ok(answer)) => break Ok(answer),
            Ok(Err(_)) => break Err(GONE),
            Err(_) if !apps() => break Err(GONE),
            Err(_) => {}
        }
    };
    state.end(id);
    notify(Notify::AgentRequestEnded {
        request: id,
        timed_out: r == Err(TIMED_OUT),
    });
    match r {
        Ok(Ok(text)) => ok(text),
        Ok(Err(e)) => err(e),
        Err(why) => err(why),
    }
}

/// Why an app did not answer (also shown in the audit, `docs/approvals.md`).
pub const NO_WINDOW: &str = "no ide window is open";
pub const TIMED_OUT: &str = "the user did not answer in time";
pub const GONE: &str = "the ide window went away";

// ---------------------------------------------------------------- screen detection

/// How often running agent sessions' screens are read (only those whose output or title
/// changed since).
const SCREEN_TICK: Duration = Duration::from_millis(700);

/// Read the screens of a user's running agent sessions in the background (until the user
/// context goes away). Needs a tokio runtime (none: nothing is watched).
pub fn spawn_screen_watch(ctx: std::sync::Weak<crate::service::UserCtx>) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        let mut seen: HashMap<String, (u64, String)> = HashMap::new();
        let mut tick = tokio::time::interval(SCREEN_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(c) = ctx.upgrade() else {
                return;
            };
            seen = tokio::task::spawn_blocking(move || {
                c.screen_tick(&mut seen);
                seen
            })
            .await
            .unwrap_or_default();
        }
    });
}

/// A screen state as an activity.
pub fn screen_activity(s: ide_agents::screen::ScreenState) -> AgentActivity {
    match s {
        ide_agents::screen::ScreenState::Idle => AgentActivity::Idle,
        ide_agents::screen::ScreenState::Working => AgentActivity::Working,
        ide_agents::screen::ScreenState::Blocked => AgentActivity::NeedsInput,
    }
}

// ---------------------------------------------------------------- `ide-server hook`

/// How long a hook waits for the daemon (a hook blocks the agent's tool call).
const HOOK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a tool hook waits once connected: a write outside the session waits for the
/// user's answer (`docs/write-guard.md`; the daemon gives up first, [`MAX_ASK`]), and stops
/// before the CLI's own limit for the hook.
const TOOL_HOOK_TIMEOUT: Duration = Duration::from_secs(ide_agents::hooks::TOOL_HOOK_SECS - 60);

/// `ide-server hook --provider <id> <event>`: run by the agent CLI for one hook (stdin: the
/// CLI's hook input). The provider turns it into an event, the daemon answers, the provider
/// turns the answer into the CLI's output. Any failure is "no opinion" (exit 0, nothing
/// printed): a hook must never break the agent. Returns the exit code.
pub async fn run_hook(args: &[String]) -> i32 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let provider_id = args
        .iter()
        .position(|a| a == "--provider")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let event = args
        .iter()
        .skip(1)
        .rfind(|a| !a.starts_with("--") && **a != provider_id)
        .cloned()
        .unwrap_or_default();
    let mut input = Vec::new();
    let _ = tokio::io::stdin()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut input)
        .await;
    let providers = match ide_core::Paths::from_env() {
        Ok(p) => ide_agents::Providers::load(&p.providers_dir()),
        Err(_) => ide_agents::Providers::load(std::path::Path::new("")),
    };
    let Some(provider) = providers.get(&provider_id) else {
        return 0;
    };
    let Some(ev) = provider.parse_hook(&event, &input) else {
        return 0;
    };
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let decision = match (var(SESSION_ENV), var(TOKEN_ENV), var(SOCKET_ENV)) {
        (Some(s), Some(t), Some(k)) => {
            let tool = matches!(&ev, HookEvent::PreTool(ToolUse::Edit { .. } | ToolUse::Shell { .. }));
            let wait = if tool { TOOL_HOOK_TIMEOUT } else { HOOK_TIMEOUT };
            match tokio::time::timeout(HOOK_TIMEOUT, AgentLink::connect(&k, &s, &t)).await {
                Ok(link) => match tokio::time::timeout(wait, link.hook(wire_event(ev))).await {
                    Ok(Ok(Some(reason))) => HookDecision::Deny { reason },
                    // The daemon took longer than any question may (it gives up first): a write
                    // it may still be asking about does not run.
                    Err(_) if tool => HookDecision::Deny {
                        reason: "ide did not answer in time: do not retry or work around it; ask the user in your reply".into(),
                    },
                    _ => HookDecision::NoOpinion,
                },
                Err(_) => HookDecision::NoOpinion,
            }
        }
        _ => HookDecision::NoOpinion,
    };
    let reply = provider.hook_reply(&event, &decision);
    if !reply.stdout.is_empty() {
        let mut out = tokio::io::stdout();
        let _ = out.write_all(reply.stdout.as_bytes()).await;
        let _ = out.flush().await;
    }
    reply.exit_code
}

// ---------------------------------------------------------------- `ide-server mcp`

/// MCP protocol revision answered when the client asks for none we know.
const MCP_PROTOCOL: &str = "2025-06-18";

/// `ide-server mcp`: a stdio MCP server (newline-delimited JSON-RPC 2.0) for the session in
/// [`SESSION_ENV`], connected to the daemon at [`SOCKET_ENV`] with [`TOKEN_ENV`]. Returns when
/// stdin closes.
pub async fn run_mcp_stdio() -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (session, token, socket) = match (var(SESSION_ENV), var(TOKEN_ENV), var(SOCKET_ENV)) {
        (Some(s), Some(t), Some(k)) => (s, t, k),
        _ => anyhow::bail!(
            "{SESSION_ENV}, {TOKEN_ENV} and {SOCKET_ENV} must be set (run by an ide session)"
        ),
    };
    let link = AgentLink::connect(&socket, &session, &token).await;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(msg) => handle_mcp(&link, msg).await,
            Err(e) => Some(rpc_error(serde_json::Value::Null, -32700, &e.to_string())),
        };
        if let Some(a) = answer {
            let mut text = a.to_string();
            text.push('\n');
            out.write_all(text.as_bytes()).await?;
            out.flush().await?;
        }
    }
    Ok(())
}

/// A tool result as MCP `tools/call` content: the text, then the image if there is one
/// (`{"type": "image", "data", "mimeType"}`).
pub fn mcp_content(r: AgentToolResult) -> serde_json::Value {
    let mut content = vec![serde_json::json!({"type": "text", "text": r.text})];
    if let Some(img) = r.image {
        content.push(serde_json::json!({"type": "image", "data": img.data, "mimeType": img.mime_type}));
    }
    serde_json::json!({"content": content, "isError": r.is_error})
}

fn rpc_error(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// One MCP message; `None` for notifications.
async fn handle_mcp(link: &AgentLink, msg: serde_json::Value) -> Option<serde_json::Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(serde_json::Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(MCP_PROTOCOL);
            Ok(serde_json::json!({
                "protocolVersion": asked,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": MCP_NAME, "version": ide_proto::VERSION},
            }))
        }
        "ping" => Ok(serde_json::json!({})),
        "tools/list" => match link.tools().await {
            Ok(tools) => Ok(serde_json::json!({
                "tools": tools
                    .iter()
                    .map(|t| {
                        let schema: serde_json::Value = serde_json::from_str(&t.input_schema)
                            .unwrap_or_else(|_| serde_json::json!({"type": "object"}));
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": schema,
                        })
                    })
                    .collect::<Vec<_>>()
            })),
            Err(e) => Err((-32603, e)),
        },
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let r = match link.call(name, &args.to_string()).await {
                Ok(r) => r,
                Err(e) => err(e),
            };
            Ok(mcp_content(r))
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, m)) => rpc_error(id, code, &m),
    })
}

/// The bridge's connection to the daemon (an agent connection); its error text when the
/// daemon could not be reached or refused the token (reported by every tool call).
struct AgentLink {
    conn: Result<(ide_proto::Mux, tokio::sync::Mutex<ide_proto::RpcReceiver>), String>,
    next: AtomicU64,
}

impl AgentLink {
    async fn connect(socket: &str, session: &str, token: &str) -> Self {
        let conn = Self::open(socket, session, token)
            .await
            .map_err(|e| format!("ide is not reachable: {e:#}"));
        AgentLink {
            conn: conn.map(|(m, rx)| (m, tokio::sync::Mutex::new(rx))),
            next: AtomicU64::new(2),
        }
    }

    async fn open(
        socket: &str,
        session: &str,
        token: &str,
    ) -> anyhow::Result<(ide_proto::Mux, ide_proto::RpcReceiver)> {
        use ide_proto::{AgentHello, Hello, MuxConfig, PROTO_VERSION, Request, Response, Role, RpcMessage};
        let io = crate::daemon::connect(socket).await?;
        let (mux, mut rx) = ide_proto::Mux::new(io, MuxConfig::new(Role::Client));
        let hello = Request::Hello(Hello {
            proto_version: PROTO_VERSION,
            client_version: format!("ide-server-mcp {}", ide_proto::VERSION),
            resume: None,
            agent: Some(AgentHello {
                session: session.to_owned(),
                token: token.to_owned(),
            }),
        });
        mux.send_rpc(&RpcMessage::Request { id: 1, req: hello }.encode())
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let first = tokio::time::timeout(Duration::from_secs(15), rx.recv())
            .await
            .map_err(|_| anyhow::anyhow!("no answer from the ide server"))?
            .ok_or_else(|| anyhow::anyhow!("the ide server closed the connection"))?;
        match RpcMessage::decode(&first)? {
            RpcMessage::Response {
                result: Ok(Response::Welcome(_)),
                ..
            } => Ok((mux, rx)),
            RpcMessage::Response { result: Err(e), .. } => Err(anyhow::anyhow!("{e}")),
            _ => Err(anyhow::anyhow!("unexpected answer from the ide server")),
        }
    }

    /// One request, one answer (MCP clients call tools one at a time per server; the lock
    /// keeps answers paired with their requests).
    async fn request(&self, req: ide_proto::Request) -> Result<ide_proto::Response, String> {
        use ide_proto::RpcMessage;
        let (mux, rx) = self.conn.as_ref().map_err(Clone::clone)?;
        let mut rx = rx.lock().await;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        mux.send_rpc(&RpcMessage::Request { id, req }.encode())
            .map_err(|_| "the ide server connection is closed".to_owned())?;
        loop {
            let msg = rx
                .recv()
                .await
                .ok_or_else(|| "the ide server connection is closed".to_owned())?;
            match RpcMessage::decode(&msg) {
                Ok(RpcMessage::Response { id: got, result }) if got == id => {
                    return result.map_err(|e| e.to_string());
                }
                _ => continue,
            }
        }
    }

    async fn tools(&self) -> Result<Vec<AgentTool>, String> {
        match self.request(ide_proto::Request::AgentTools).await? {
            ide_proto::Response::AgentTools(t) => Ok(t),
            _ => Err("unexpected answer".into()),
        }
    }

    async fn hook(&self, event: AgentHookEvent) -> Result<Option<String>, String> {
        match self.request(ide_proto::Request::AgentHook { event }).await? {
            ide_proto::Response::AgentHookDecision(d) => Ok(d),
            _ => Err("unexpected answer".into()),
        }
    }

    async fn call(&self, tool: &str, args: &str) -> Result<AgentToolResult, String> {
        let req = ide_proto::Request::AgentCall {
            tool: tool.to_owned(),
            args: args.to_owned(),
        };
        match self.request(req).await? {
            ide_proto::Response::AgentResult(r) => Ok(r),
            _ => Err("unexpected answer".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_agents::AgentProvider;

    /// The screen as the server's terminal model sees it (escape sequences applied), read by
    /// the providers' screen rules.
    /// A question to the apps ends (and the apps are told) when its time is up or when the
    /// last app went, even without a time limit.
    #[tokio::test]
    async fn questions_end() {
        let state = BridgeState::default();
        let sent = Mutex::new(Vec::new());
        let notify = |n: Notify| sent.lock().unwrap().push(n);
        let r = ask_app_within(&state, "s", "t", "{}".into(), || true, notify, Some(Duration::from_millis(50))).await;
        assert!(r.is_error && r.text == TIMED_OUT, "{}", r.text);
        let ended = |n: &Notify| match n {
            Notify::AgentRequestEnded { request, timed_out } => Some((*request, *timed_out)),
            _ => None,
        };
        {
            let s = sent.lock().unwrap();
            assert!(matches!(s[0], Notify::AgentRequest { request: 1, .. }));
            assert_eq!(ended(&s[1]), Some((1, true)));
        }
        // No limit: the app goes away meanwhile.
        let apps = std::sync::atomic::AtomicBool::new(true);
        let ask = ask_app_within(&state, "s", "t", "{}".into(), || apps.load(Ordering::Relaxed), notify, None);
        let gone = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            apps.store(false, Ordering::Relaxed);
        };
        let (r, ()) = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(ask, gone) })
            .await
            .expect("ends");
        assert!(r.is_error && r.text == GONE, "{}", r.text);
        assert_eq!(ended(&sent.lock().unwrap()[3]), Some((2, false)));
        // Answered.
        let ask = ask_app_within(&state, "s", "t", "{}".into(), || true, notify, None);
        let answer = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            state.reply(3, Ok("once".into()));
        };
        let (r, ()) = tokio::join!(ask, answer);
        assert!(!r.is_error && r.text == "once");
        assert_eq!(ended(&sent.lock().unwrap()[5]), Some((3, false)));
        // No app at all: refused at once, nothing sent.
        let r = ask_app_within(&state, "s", "t", "{}".into(), || false, notify, None).await;
        assert!(r.is_error && r.text == NO_WINDOW);
        assert_eq!(sent.lock().unwrap().len(), 6);
    }

    #[test]
    fn screen_of_a_terminal() {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let host = crate::pty::TermHost::new("s", 60, 8, tx);
        // A Claude-like screen: a status line drawn in colour, the input box below.
        host.ingest(
            "\u{1b}[2J\u{1b}[H⏺ Reading files\r\n\r\n\u{1b}[38;5;174m✻\u{1b}[0m Thinking… (3s · esc to interrupt)\r\n\r\n────────\r\n> \r\n────────".as_bytes(),
        );
        let lines = host.screen_text();
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[2], "✻ Thinking… (3s · esc to interrupt)");
        assert_eq!(lines[7], "");
        let claude = ide_agents::Claude;
        let m = claude.screen_manifest().unwrap();
        let input = ide_agents::screen::ScreenInput { lines: &lines, title: "" };
        assert_eq!(
            m.detect(&input).map(screen_activity),
            Some(AgentActivity::Working)
        );
        // The turn ends: the status line is replaced.
        host.ingest("\u{1b}[3;1H\u{1b}[2K⏺ Done.".as_bytes());
        let lines = host.screen_text();
        let input = ide_agents::screen::ScreenInput { lines: &lines, title: "✳ Claude Code" };
        assert_eq!(m.detect(&input).map(screen_activity), Some(AgentActivity::Idle));
    }

    /// Without hooks the screen decides; with hooks it only adds a visible dialog and its end.
    #[test]
    fn hooks_and_screen() {
        use AgentActivity::{Idle, NeedsInput, Working};
        let b = BridgeState::new(None);
        assert_eq!(b.screen_says("s", Working), Some(Working));
        assert_eq!(b.screen_says("s", Idle), Some(Idle));
        b.mark_hooked("s");
        b.set_activity("s", Working);
        assert_eq!(b.screen_says("s", Idle), None, "hooks keep the turn");
        assert_eq!(b.screen_says("s", NeedsInput), Some(NeedsInput), "a dialog shows at once");
        b.set_activity("s", NeedsInput);
        assert_eq!(b.screen_says("s", Working), Some(Working), "the dialog went away");
        b.hook_activity("s", &AgentHookEvent::BackgroundStarted { id: "a".into() });
        assert_eq!(b.screen_says("s", Idle), Some(Working), "background work runs");
        b.reset("s");
        assert_eq!(b.activity("s"), AgentActivity::Unknown);
        assert_eq!(b.screen_says("s", Idle), Some(Idle), "a new process: no hooks, no work");
    }

    /// A turn that ends while background work runs stays working until the last of it ends.
    #[test]
    fn background_work_holds_the_turn() {
        use AgentActivity::{Idle, NeedsInput, Working};
        use AgentHookEvent as E;
        let b = BridgeState::new(None);
        let h = |e: E| b.hook_activity("s", &e);
        let start = |id: &str| E::BackgroundStarted { id: id.into() };
        let end = |id: &str| E::BackgroundEnded { id: id.into() };
        let tool = || E::PreTool { tool: "Read".into() };

        // No background work: as before.
        assert_eq!(h(E::PromptSubmitted), Some(Working));
        assert_eq!(h(E::Stopped), Some(Idle));

        // Work started inside a turn and ended inside it (a foreground subagent).
        assert_eq!(h(E::PromptSubmitted), Some(Working));
        assert_eq!(h(start("f")), None);
        assert_eq!(h(end("f")), None, "the turn still runs");
        assert_eq!(h(E::Stopped), Some(Idle));

        // Two background agents outlive the turn.
        assert_eq!(h(E::PromptSubmitted), Some(Working));
        assert_eq!(h(start("a")), None);
        assert_eq!(h(start("b")), None);
        assert_eq!(h(E::Stopped), Some(Working), "held by a and b");
        assert_eq!(h(tool()), Some(Working), "a background agent's tool call");
        assert_eq!(h(E::NeedsInput { permission: true }), Some(NeedsInput));
        assert_eq!(h(end("a")), None, "b still runs");
        // The CLI resumes with a's result (no prompt), ends that turn: b still holds it.
        assert_eq!(h(tool()), Some(Working));
        assert_eq!(h(E::Stopped), Some(Working));
        assert_eq!(h(end("b")), Some(Idle), "the last one ended");
        assert_eq!(h(end("b")), None, "an unknown or repeated end changes nothing");

        // Background shells: no hook starts or ends them, the CLI's record at Stop does.
        let stop_with = |ids: &[&str]| E::StoppedRunning {
            background: ids.iter().map(|i| i.to_string()).collect(),
        };
        assert_eq!(h(E::PromptSubmitted), Some(Working));
        assert_eq!(h(stop_with(&["sh1", "sh2"])), Some(Working), "2 shells still running");
        assert_eq!(b.screen_says("s", Idle), Some(Working), "an idle prompt while shells run");
        // sh1 ends: the CLI resumes with its notification (tools, no prompt) and stops again.
        assert_eq!(h(tool()), Some(Working));
        assert_eq!(h(stop_with(&["sh2"])), Some(Working));
        assert_eq!(h(stop_with(&[])), Some(Idle), "the last one ended");
        // The record replaces what hooks counted: a lost SubagentStop cannot hold the session.
        assert_eq!(h(start("lost")), None);
        assert_eq!(h(stop_with(&[])), Some(Idle));
        // A start reported after the record is still counted until its end.
        assert_eq!(h(E::PromptSubmitted), Some(Working));
        assert_eq!(h(stop_with(&["sh3"])), Some(Working));
        assert_eq!(h(start("a")), None);
        assert_eq!(h(end("sh3")), None, "a still runs");
        assert_eq!(h(end("a")), Some(Idle));

        // A new conversation forgets work it never saw end.
        assert_eq!(h(start("c")), None);
        assert_eq!(h(E::SessionStart { conversation: None }), Some(Idle));
        assert_eq!(h(E::PromptSubmitted), Some(Working));
        assert_eq!(h(E::Stopped), Some(Idle));
    }
}
