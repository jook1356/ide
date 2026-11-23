# Agent bridge

Status: foundation (protocol 13). Sessions' agents reach the IDE through an MCP server (two
tools prove both paths, daemon-answered and app-answered); what they are doing comes from CLI
hooks (Claude Code) and from reading their terminal screen (Claude Code, Codex). Features built on it (child / parent sessions, the shared
workspace's write lease, task queue, terminals, browser, previews) come next; widgets (Desk's mods) are in `docs/widgets.md`.
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

- **Name** `ide-bridge`, not `ide`: Claude Code takes an MCP server named `ide` for its IDE
  integration (VS Code extension tools such as `closeAllDiffTabs`) and does not offer its tools
  as usual (v: seen in a real session).
- **Injection** (`ide-agents`, provider-agnostic): `SessionInput::bridge` (`Bridge { server,
  env, instructions }`) is set by the server for every agent session. `assemble()` adds the
  bridge's MCP server after the configuration's servers (a configuration server named `ide-bridge` is
  left out with a note), appends the bridge instructions after the configuration's, and sets the
  env vars on the agent process. Providers map them as they map configuration MCP servers
  (Claude `--mcp-config <scratch>/mcp.json`, Codex `-c mcp_servers.ide-bridge={...}`). Plain programs
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

## Hooks (protocol 13)

The IDE learns what an agent is doing (and can refuse a tool call) from the CLI's hooks.
Provider-agnostic types are in `ide-agents/src/hooks.rs`; everything CLI-specific stays in the
provider's file:

| Step | Trait method | Claude Code (`claude.rs`) |
|---|---|---|
| register | `apply_hooks(plan, HookTargets { program })` | `<scratch>/settings.json` `{"hooks": ...}` via `--settings` (d); exec form `{"type":"command","command":<ide-server>,"args":["hook","--provider","claude",<event>],"timeout":10}` (no shell: no quoting of the path (d)) |
| events | `parse_hook(event, stdin) -> HookEvent` | `SessionStart`, `UserPromptSubmit` -> PromptSubmitted, `Stop` -> Stopped, `Notification` (`permission_prompt` / `elicitation_dialog`) -> NeedsInput, `PreToolUse` (`Edit|Write|MultiEdit|NotebookEdit`) -> PreTool(Edit {paths}) |
| answer | `hook_reply(event, decision) -> HookReply` | deny: `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":...}}`, exit 0; no opinion: nothing, exit 0 (never "allow": that would skip Claude's permission prompts) (d) |
| caps | `hook_caps()` | status, edit guard: native; shell guard: unsupported |

- The hook program is `ide-server hook --provider <id> <event>`: the provider parses stdin,
  the event goes to the daemon over an agent connection (`agent.hook`, env of the agent
  process: same session id / token / socket as the MCP server), the provider prints the answer.
  It never breaks the agent: no env, no daemon, a timeout (5 s) or an unknown event = no
  opinion. `ide-server` usage errors exit 1, never 2 (CLIs read 2 as "block").
- Activity (`SessionInfo.activity`, `Notify::Activity`; also set by screen detection): Unknown until the process reports
  (reset on every start, Unknown while not running); SessionStart / Stop -> Idle; prompt / tool
  use -> Working (Desk's 답변 중); permission prompt / question -> NeedsInput (Desk's 응답 필요);
  the user typing in the terminal while NeedsInput -> Working (the answer). The app shows the
  phase on the session row and Desk's 새 답변 for a turn that ended while another session was
  shown (cleared when selected).
- Decisions: `agent.hook` answers `Some(reason)` to refuse a tool call: the shared folder's write
  lease (`docs/session-family.md`) refuses edits of members that do not hold it.
- Codex: through the account integration (below). Its docs (developers.openai.com/codex/hooks)
  give hooks only in `hooks.json` / `config.toml` files (system, `$CODEX_HOME`, project), not
  as a command-line override, and every non-managed hook needs trust review (`/hooks`,
  hash-based); `--dangerously-bypass-hook-trust` would also run the user's own untrusted hooks.
  `codex.rs` parses `SessionStart`, `UserPromptSubmit`, `Stop`, `PermissionRequest` and
  `PreToolUse` (`apply_patch` / `Edit` / `Write` -> edits, paths from the patch's `*** Add /
  Update / Delete File:` / `*** Move to:` lines, relative to the hook input's `cwd`) and answers
  a deny like Claude Code (`hookSpecificOutput.permissionDecision`) (d).

## Account integration (protocol 15)

For CLIs whose hooks can only live in their config home (Codex). Only on the user's request
(accounts page: 설치 / 갱신 / 제거), the one exception to "accounts are never written":

- `AgentProvider::integration_status / integration_install / integration_uninstall`
  (`ide-agents/src/integration.rs`); `Caps.integration` says a provider has one. Codex writes
  ide's entries into `CODEX_HOME/hooks.json` (the account dir): one group per event, command
  `<ide-server> hook --provider codex <event>` (path quoted only when it has blanks: the command
  runs through a shell (a)), `timeout` 10; `PreToolUse` matcher `apply_patch|Edit|Write`. ide's
  entries are found by their command (` hook --provider codex `); the user's own hooks and the
  file's other keys are kept; installing again replaces ide's entries; uninstall removes only
  them. Status: missing / installed / outdated (installed for another program path).
- After an install Codex must trust the new hooks once: `approve_with = "/hooks"`, shown on
  the accounts page ("Approve ide's hooks once in the Codex CLI with /hooks").
- `account.integration {provider, slug, action: Status | Install | Uninstall}` ->
  `Integration {state, approve_with}`; install / uninstall need the daemon (the hook program)
  and are audited. The hooks run for every Codex start with that account, also outside ide:
  without an ide session's env the hook program exits at once with no opinion.
- (a) Not checked against the real Codex: that it reads the entries, the trust prompt, the
  shell used on Windows, the hook input fields.
- (a) Not yet checked against the real Claude Code: that `--settings` hooks load next to the
  user's settings, that the exec form (`args`) runs on the supported versions, and that the
  events fire as documented. Opening a session in the app shows it (SessionStart -> the row's
  activity; a prompt -> 답변 중).

## Screen detection

What a CLI is doing, read from its terminal (`ide-agents/src/screen.rs`), like herdr's agent
detection (github.com/herdrdev/herdr, Apache-2.0: the approach, the rule model and the UI texts
the rules look for; code and rules are this project's). It needs no hooks, so it gives Codex
its status, and it shows Claude Code's permission prompts at once (its `Notification` hook
fires only after a few seconds).

- Rules per provider: `crates/ide-agents/screens/<provider>.toml`, returned by
  `AgentProvider::screen_manifest()`. A rule has a state (`idle` / `working` / `blocked` /
  `unknown` = keep the previous state), a priority (the highest matching rule wins, the
  earlier one on a tie), a region and gates: `contains` (all needles, case-insensitive),
  `regex` (all match the region), `line_regex` (each matches some line), `all` / `any` /
  `not` of nested gates. No match: idle.
- Regions: `title` (OSC title), `whole`, `bottom(N)`, `bottom_non_empty(N)`,
  `top_non_empty(N)`, `after_last_prompt`, `before_current_prompt`, `without_current_prompt`
  (a prompt line is `>` / `›` alone or followed by a space, also in a `│` box, or `❯`; the
  current one has no response line `•` / `■` / `✗` / `✓` / `⏺` after it), `after_last_rule`
  (`───`), `prompt_box` (between the second rule from the bottom and the next),
  `above_prompt_box`.
- Server: `TermHost::screen_text()` (the server-side terminal model's visible rows);
  `bridge::spawn_screen_watch` reads running agent sessions (not login terminals) every
  0.7 s when their output or title changed. Without hooks since the process started, the
  screen decides the activity; with hooks it only adds a visible dialog (-> NeedsInput) and
  its end (NeedsInput -> what the screen shows).
- (a) The rules are not yet checked against the real CLIs' screens here (tests use screens
  written after the UI texts); a CLI update can change them: fix the manifest.

## Protocol (12, 13)

| Request / event | Who | Payload |
|---|---|---|
| `Hello.agent` | agent | `{session, token}` (`None` for apps) |
| `agent.tools` | agent | -> `AgentTools([{name, description, input_schema (JSON text)}])` |
| `agent.call` | agent | `{tool, args (JSON text)}` -> `AgentResult {text, is_error}` |
| `Notify::AgentRequest` | daemon -> apps | `{request, session, tool, args}` |
| `agent.reply` | app | `{request, result: Ok(text) / Err(text)}` -> `Unit` |
| `agent.hook` (13) | agent | `{event: AgentHookEvent}` -> `AgentHookDecision(Option<reason>)` |
| `Notify::Activity` (13) | daemon -> apps | `{session, activity}`; also `SessionInfo.activity` |

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
| `ide_list_related_sessions`, `ide_create_child_session`, `ide_send_to_session`, `ide_merge_child`, `ide_sync_from_parent` | daemon | session families: `docs/session-family.md` |
| `ide_widget` | daemon (`guide`, `list`), app (`open`, `close`) | widgets: `docs/widgets.md` |

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

Plan for the features on top of the bridge: `docs/agent-features-roadmap.md`.

1. Codex hooks (see Hooks); the write lease of shared workspaces on `agent.hook`.
2. Pushing to the agent: queued prompts / steering through the PTY (`session.send_prompt`).
3. Feature tools on this transport: related sessions (create child, send, read), run_task,
   terminals, browser (CEF), previews (widgets: done, `docs/widgets.md`).
