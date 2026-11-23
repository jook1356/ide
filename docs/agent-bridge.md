# Agent bridge

Status: foundation (protocol 12). Sessions' agents reach the IDE through an MCP server;
two tools prove both paths (daemon-answered and app-answered). Features built on it (child /
parent sessions, task queue, terminals, browser, previews, mods) and status hooks come later.
Marks as in `agent-providers.md`: (v) verified, (d) docs, (a) assumption.

## Why

Hermes Desk injects a Python plugin into Hermes (`resources/hermes-plugin/hermes-desk-guard`):
tools (`create_child_session`, `send_to_session`, `run_task`, `browser_act`, `desk_tab`,
`desk_mod`, ...) that reach Desk through a file mailbox signed with an HMAC key, hooks that guard
writes, plus `HERMES.md` instructions. Claude Code and Codex have no plugin API like Hermes', but
both run MCP servers and take instructions: the bridge is an MCP server every session gets.

## Pieces

```
agent CLI (claude / codex, in the session's PTY)
  └─ runs `ide-server mcp`            stdio MCP server (bridge.rs::run_mcp_stdio)
        └─ local socket, Hello.agent  agent connection: one session, agent.* only
              └─ daemon (UserCtx)     answers itself (ide_session)
                    └─ Notify::AgentRequest -> app -> agent.reply   (ide_show_message)
```

- **Injection** (`ide-agents`, provider-agnostic): `SessionInput::bridge` (`Bridge { server,
  env, instructions }`) is set by the server for every agent session. `assemble()` adds the
  bridge's MCP server after the configuration's servers (a configuration server named `ide` is
  left out with a note), appends the bridge instructions after the configuration's, and sets the
  env vars on the agent process. Providers map them as they map configuration MCP servers
  (Claude `--mcp-config <scratch>/mcp.json`, Codex `-c mcp_servers.ide={...}`). Plain programs
  (shells) get the env vars only.
- **Env** (agent process and MCP server; MCP clients start servers with a reduced environment,
  so the server entry repeats them): `IDE_SESSION_ID`, `IDE_AGENT_TOKEN`, `IDE_SERVER_SOCKET`.
- **Token**: random per session and daemon run (`BridgeState`, memory only), the same on every
  launch of the session until the daemon restarts (the daemon owns the PTYs, so a restart ends
  the agents anyway); forgotten when the session is deleted. It is written into the session's
  scratch `mcp.json` (Claude) or the Codex command line, both visible only to the same OS user,
  who can reach the daemon anyway.
- **Agent connection** (`conn.rs::serve_agent`): `Hello.agent = {session, token}`; a wrong token
  or session is `unauthorized`. The connection gets no notifications and may send only
  `ping`, `agent.tools`, `agent.call` (anything else: `bad-request`). Tool calls are audited
  (`agent.call <session> <tool>`).
- **Daemon only**: the bridge needs the daemon's socket. `ServerConfig::bridge` is set in
  `--daemon` mode (`exe` = the running `ide-server`, `socket` = its pipe / socket);
  `--stdio --no-daemon` and in-process servers pass no bridge.
- **App-carried tools**: the daemon sends `Notify::AgentRequest {request, session, tool,
  args}` to the user's connections and waits up to 60 s for the first `agent.reply {request,
  result}`. No app connected = an immediate error for the agent. An app refuses tools it does
  not know, so an older app does not leave the agent waiting.

## Protocol (12)

| Request / event | Who | Payload |
|---|---|---|
| `Hello.agent` | agent | `{session, token}` (`None` for apps) |
| `agent.tools` | agent | -> `AgentTools([{name, description, input_schema (JSON text)}])` |
| `agent.call` | agent | `{tool, args (JSON text)}` -> `AgentResult {text, is_error}` |
| `Notify::AgentRequest` | daemon -> apps | `{request, session, tool, args}` |
| `agent.reply` | app | `{request, result: Ok(text) / Err(text)}` -> `Unit` |

Tool names shared by server and app: `ide_proto::AGENT_TOOL_*`.

## MCP side (`ide-server mcp`)

Newline-delimited JSON-RPC 2.0 on stdin / stdout (MCP stdio transport (d)): `initialize`
(echoes the client's `protocolVersion`, `capabilities.tools`), `ping`, `tools/list`,
`tools/call` (`content: [{type: text}]`, `isError`); notifications get no answer; other methods
`-32601`. It connects to the daemon once at start; if that fails (no daemon, refused token) the
server still answers MCP and every tool call reports why. Nothing is logged to stdout; errors at
start go to stderr.

## Tools (foundation)

| Tool | Answered by | What |
|---|---|---|
| `ide_session` | daemon | this session as JSON: id, title, agent, mode, folder, repositories (name, worktree folder, branch, base) |
| `ide_show_message` | app | `{message}`: a toast in the session's detail panel |

Base instructions (`bridge.rs::INSTRUCTIONS`, appended for every bridged session): ide manages
this session; use the `ide_*` tools instead of guessing.

## Verification

- `ide-server/tests/bridge_e2e.rs`: launch specs (Claude / Codex / shell) carry the bridge;
  agent connections (token checks, forbidden requests, `ide_session`, app round trip, no app);
  the real `ide-server mcp` against a real daemon (initialize, tools/list, tools/call).
- (a) Claude Code and Codex start the `ide` server and list its tools: not yet checked against
  the real CLIs (no prompts may be sent from tests; opening the TUI and listing MCP servers is
  allowed: `claude mcp list`-style checks are the next step).

## Next

1. Hooks (`HookTargets`): Claude `--settings` hooks / Codex hooks -> `ide-server hook <event>`
   over the same agent connection: busy ("답변 중") / idle / needs-input state, write guard,
   approvals.
2. Pushing to the agent: queued prompts / steering through the PTY (`session.send_prompt`).
3. Feature tools on this transport: related sessions (create child, send, read), run_task,
   terminals, browser (CEF), previews, mods.
