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

use ide_agents::Bridge;
use ide_core::McpServer;
use ide_proto::{AgentTool, AgentToolResult, Notify, ProtoError, SessionInfo};
use tokio::sync::oneshot;

/// Name of the bridge's MCP server (reserved: a configuration server of this name is left out).
pub const MCP_NAME: &str = "ide";
/// Env vars of the agent process and of the bridge's MCP server.
pub const SESSION_ENV: &str = "IDE_SESSION_ID";
pub const TOKEN_ENV: &str = "IDE_AGENT_TOKEN";
pub use crate::user::SOCKET_ENV;

/// `ide_session`: this session as JSON (answered by the daemon).
pub use ide_proto::AGENT_TOOL_SESSION as TOOL_SESSION;
/// `ide_show_message`: show a short message in the session's panel (answered by an app).
pub use ide_proto::AGENT_TOOL_SHOW_MESSAGE as TOOL_SHOW_MESSAGE;

/// How long a tool carried out by an app may take.
const APP_TIMEOUT: Duration = Duration::from_secs(60);

/// Base instructions of every bridged session (appended to the configuration's).
const INSTRUCTIONS: &str = "\
## ide
You are running inside ide, the IDE that manages this session: its terminal, its \
repositories / worktrees and the panels around it. The `ide` MCP server connects you to the \
IDE. Use its tools (names start with `ide_`) instead of guessing about this session or the \
IDE: `ide_session` describes this session (folder, repositories, branches, base branches); \
`ide_show_message` shows a short note to the user in the IDE.";

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
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<String, String>>>>,
    seq: AtomicU64,
}

impl BridgeState {
    pub fn new(config: Option<BridgeConfig>) -> Self {
        BridgeState {
            config,
            ..Default::default()
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
        })
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

/// The bridge's tools.
pub fn tools() -> Vec<AgentTool> {
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
    ]
}

fn ok(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: text.into(),
        is_error: false,
    }
}

fn err(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: text.into(),
        is_error: true,
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
    notify: impl FnOnce(Notify),
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
            if !apps {
                return err("no ide window is open");
            }
            let args = serde_json::json!({ "message": message }).to_string();
            let (id, rx) = state.begin();
            notify(Notify::AgentRequest {
                request: id,
                session: session.to_owned(),
                tool: tool.to_owned(),
                args,
            });
            let r = tokio::time::timeout(APP_TIMEOUT, rx).await;
            state.end(id);
            match r {
                Ok(Ok(Ok(text))) => ok(text),
                Ok(Ok(Err(e))) => err(e),
                Ok(Err(_)) => err("the ide window went away"),
                Err(_) => err("the ide window did not answer"),
            }
        }
        _ => err(format!("unknown tool `{tool}`")),
    }
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
            Ok(serde_json::json!({
                "content": [{"type": "text", "text": r.text}],
                "isError": r.is_error,
            }))
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
