# Client/server split and multi-user foundation

Status: agreed design. Phase 1 implemented, including the app migration (`ide-app` is a pure
client) — see "Phase 1 implementation" and "App integration" at the end.

## Goals
- The app (`ide`) only draws: terminal rendering, IME, editor views, CEF display, layout, theme, language.
- A backend daemon (`ide-server`) does all work: agent CLIs (claude, codex, shells) in PTYs, git
  worktrees, files, search, diffs, go-to-definition, preview bundling/serving, mods, dev servers.
- The same server binary runs locally (child of the app) or on a company server (over SSH).
- Transport-agnostic protocol: stdio, SSH, WebSocket and in-memory pipes carry the same frames.

## Topology: one server per user
```
app ──stdio (child process)──────────────► ide-server   (local, my account)
app ──ssh host ide-server --stdio─────────► ide-server   (remote, my account on the server)
phone/browser ─WebSocket+SSO─► gateway ──► ide-server   (later; gateway connects as the user)
```
- Each user's server runs under that user's OS account. Authentication is done by the transport
  (local: none needed; SSH: the user's key/agent; gateway: SSO). OS accounts isolate users' files,
  logins and agent processes, so one user's agent cannot read another's data.
- A server stays alive after the client disconnects (agents keep running) and is re-attached by the
  next client. `ide-server --stdio` connects to the already-running per-user daemon through a local
  socket (Unix socket / Windows named pipe) and relays bytes; it starts the daemon if none is running.

## Crates
| Crate | Side | Role |
|---|---|---|
| `ide-proto` | shared | Frame codec, message types (serde), RPC enum, stream mux + per-stream flow control, resume state, error ids for i18n. Transport-agnostic (`AsyncRead + AsyncWrite`). |
| `ide-core` | server | Profiles, sessions, worktrees, launch specs, session ops (exists). |
| `ide-server` | server | Daemon: local socket listener, `--stdio` relay mode, services (sessions, profiles, PTY host), `Runner`, path jail, audit hook. |
| `ide-client` | client | Starts/attaches a server through a `Transport` (local child, `ssh` command, later WebSocket), RPC calls, stream subscriptions as iced `Subscription`s, automatic reconnect + resume. |
| `ide-term` | client | Terminal widget + client-side VT model fed by bytes (no PTY code). |
| `ide-web` | client | CEF. Preview/mod HTTP arrives over protocol streams and is served to CEF by a client-local proxy (phase 3). |
| `ide-i18n` | client | UI strings. The server sends error ids + args, never localized text. |
| `ide-app` | client | UI only; talks to `ide-client`; never spawns processes or touches project files. |

## Framing
Every transport carries a sequence of frames:
```
u32 length (big-endian, of the rest) | u8 kind | u32 stream_id | payload
```
- `kind`: `Rpc` (MessagePack `RpcMessage`: Request{id, method}, Response{id, result|error},
  Notify{event}), `Data` (raw stream bytes), `Window` (flow-control credit), `Open`/`Close` (stream lifecycle).
- Stream 0 is the control/RPC stream. Terminal output and, later, HTTP tunnels are separate streams,
  so one busy stream never blocks another.
- Flow control: each stream has a credit window (e.g. 256 KiB); the receiver grants more with
  `Window` frames as it consumes data. A slow client stalls only its own stream; the server keeps
  reading the PTY into the ring buffer, so the agent never blocks.
- WebSocket: one binary WebSocket message = one frame (same codec). Max frame 1 MiB; larger payloads split.

## What each transport must provide vs what the protocol does
| Concern | Where |
|---|---|
| Ordered byte delivery | Transport (stdio, SSH channel, WebSocket, memory pipe) |
| Message boundaries, multiplexing, flow control | Protocol |
| Reconnect + resume from the last offset | Protocol (the transport is simply re-opened) |
| Preview/mod HTTP | Protocol streams + client-local proxy (no per-transport port forwarding) |
| Authentication | Per transport: none for a local child, SSH for remote, SSO at the gateway. The protocol only receives the resulting `UserInfo`. |

## Handshake and versioning
`Hello{proto_version, client_version, resume: Option<ResumeToken>}` ->
`Welcome{proto_version, server_version, user: UserInfo, capabilities, resume_token}`.
A major-version mismatch is a clear error shown to the user.

## Terminal streaming
- The server owns the PTY and an `alacritty_terminal` instance per session. Output bytes go to the
  server-side Term, a per-session ring buffer (8 MiB) addressed by a monotonically increasing byte
  offset, and every attached client stream.
- **Query replies come from the server.** The server-side Term answers DSR/DA/cursor-position and
  similar queries (it sees all output in order, even with no client attached). Client-side Terms only
  render and never write replies back.
- Attach: `term.attach{session, from_offset?}` -> the server replays from `from_offset` if still in the
  ring, otherwise from the ring start with a `reset` flag (the client clears its model first), then
  streams live bytes. Each data chunk carries its start offset, so resume after a reconnect is exact.
- Input: `term.write{session, bytes}` (committed IME text, keys, paste), `term.resize{session, cols, rows}`.
  IME composition stays client-side. PTY size follows the most recently active client.
- Title, bell and exit status are server events (`Notify`), available without a client.

## Multi-user foundation (now)
| Piece | Rule |
|---|---|
| Identity | `UserInfo` comes from the transport/auth step, never from request fields. Local and SSH: the OS user running the server. `Authenticator` trait reserved for the gateway. |
| Data layout | Per-user data dir (the server's account): `profiles/`, `sessions.json`, `worktrees/`, `ui/`. The model keeps `user_id` so a gateway can address users. |
| Authorization | One layer resolves every resource id (session, profile, stream) for the connection's user before handlers run. |
| Path jail | Client paths resolve inside the user's allowed roots (their worktrees, registered repos); canonicalize; reject `..` and symlink escapes. All file/git/search RPCs go through it (phase 2). |
| Runner | All process spawning goes through `trait Runner`. Now: `SameUserRunner`. Containers/limits are optional later. |
| Secrets | Claude/Codex logins stay in the server-side profile dirs; never sent to clients. |
| Audit | `audit(user, action, target)` hook on session start/stop, process spawn, file writes. Now: a log file. |

Deferred: gateway + SSO, admin UI, quotas/limits, containers, sharing sessions between users.

## Compliance note
Agent CLIs run under each user's own login (per-user profiles on a per-user server). One subscription
account must not be shared by several people; check the Claude/Codex terms for server-side use before
deploying to a company server.

## Phases
1. **Local split**: `ide-proto`, `ide-server`, `ide-client`; sessions/profiles/worktrees and PTYs behind
   RPC; app starts a local server over stdio; detach/re-attach with replay; in-memory transport tests.
   SSH transport = running `ssh <host> ide-server --stdio` with the system OpenSSH client.
2. **Files**: tree, read/write, watch, search, git status/diff, go-to-definition (port Hermes Desk logic).
3. **Previews/mods**: server-side bundling and dev servers; HTTP over protocol streams to a
   client-local proxy for CEF.
4. **Remote UX**: connection profiles (local / SSH hosts), latency indicators, server install/upgrade over SSH.
5. **Gateway** (optional): WebSocket + SSO for phone/browser, connecting to per-user servers.

## Phase 1 implementation
Implemented in `crates/ide-proto`, `crates/ide-server` (lib + bin `ide-server`) and
`crates/ide-client`; `ide-app` uses them (see "App integration").

| Piece | State |
|---|---|
| Frame codec, 1 MiB max, kinds Rpc/Data/Window/Open/Close | done (`ide_proto::frame`) |
| MessagePack RPC (`rmp-serde`, named fields), typed `Request`/`Response`/`Notify`, `PROTO_VERSION` | done (`ide_proto::rpc`) |
| Errors as stable id + named args (`ProtoError`); `ide-core` errors keep their `ErrorMessage` ids | done |
| Mux over `AsyncRead + AsyncWrite`, per-stream credit window (256 KiB), control frames before data | done (`ide_proto::Mux`) |
| Hello/Welcome with resume token | done |
| `ide-server --stdio` (relay to the daemon, starts it detached), `--daemon`, `--stdio --no-daemon` | done |
| Profiles, sessions (create via `ide-core` incl. worktree, start/stop/restart/delete/rename) | done |
| Server-side Term answering queries; 8 MiB ring with byte offsets; attach/resume; title/bell/state notifications | done |
| `UserInfo` from the transport, authorization (`Server::authorize` + `UserCtx::resolve_session`), `Runner`/`SameUserRunner`, file audit, per-user data dir | done |
| Path jail | module + tests (`ide_server::PathJail`), not used by any RPC yet (phase 2) |
| Client transports: local child, SSH (`ssh <host> ide-server --stdio`), memory; reconnect + exact resume; iced `Subscription` (feature `iced`) | done; SSH not tested against a real host |

Deviations and details fixed during implementation:
- **Handshake** is ordinary RPC on stream 0: the first message must be `Request::Hello`, answered
  by `Response::Welcome` (or an error: `proto-version`, `unauthorized`, `not-handshaken`).
  `Welcome` also carries `resumed` and `server_instance` (random per server process): terminal
  offsets are only valid within one instance, so after a server restart the client re-attaches
  without an offset (full replay starting with a reset).
- **RPC messages above one frame** are fragmented: `Rpc` frames with `stream_id` 1 = fragment,
  more follows; 0 = last/complete. Max message 16 MiB.
- **Stream lifecycle**: `Open` payload = `u32` initial window the opener grants; the accepter
  answers with a `Window` frame. Client-opened ids are odd, server-opened ids even. `Close` is
  queued behind the stream's data, so it never overtakes it.
- **Terminal stream format**: a byte stream of records `u64 offset | u8 flags (bit0 reset) |
  u32 len | bytes` (records may straddle frames). The session is implied by the stream.
  A reset can be signalled by an empty chunk (`TermChunk { reset: true, bytes: [] }`), which the
  client emits right after an attach that could not resume.
- **Attach** is per connection and session: attaching again replaces the previous stream.
  `term.attach` without an offset always replays from the ring start with `reset`.
- **Local socket name**: `\\.\pipe\ide-server-<user>`; with `IDE_DATA_DIR` set, a hash of the data
  dir is appended so separate data dirs get separate daemons; `IDE_SERVER_SOCKET` overrides it.
  Unix: `<data>/ide-server.sock` (data dir chmod 700).
- **Daemon lifetime**: runs until killed, `server.shutdown` (stops every session, replies, exits),
  or until idle (no connections, no running sessions) for `--idle-exit SECS` /
  `IDE_SERVER_IDLE_EXIT_SECS` (default 600; 0 = never). Writes `<data>/ide-server.pid`; logs to
  `<data>/logs/ide-server.log` (`IDE_SERVER_LOG` filter); audit to `<data>/audit.log` (JSON lines).
- **Wire types** are the protocol's own (`SessionInfo` with string paths, `Agent`), not `ide-core`
  structs, so a Unix server can serve a Windows client.
- **PTY size**: the most recent `term.resize` wins (no separate activity tracking).
- **Colour query replies** (OSC 4/10/11/12) use the client's default dark palette; there is no
  RPC to set the palette yet.
- **UserInfo**: name from `USERNAME`/`USER`, id `DOMAIN\name` on Windows (not a SID/uid).
- **Windows query observation**: conhost (ConPTY) answers a program's own DSR itself; the query
  that reaches the server is ConPTY's startup `ESC[6n` (`PSEUDOCONSOLE_INHERIT_CURSOR`), which the
  server answers so sessions start with no client attached. DA/OSC/size replies are unit-tested
  on the server Term directly.

Known gaps (phase 1):
- `ide-core` runs `git` itself, so git processes do not go through `Runner`.
- The Windows pipe uses the default DACL (remote clients rejected); restricting it explicitly to
  the owner is still to do. No `Authenticator` trait yet (gateway phase).
- The resume token only reports `resumed`; there is no other per-client server state.
- Stopping a session kills the PTY's direct child; descendants end when the pseudoconsole closes.

## App integration (phase 1 completion)
- `ide-app` depends on `ide-client` (+ `ide-proto`, `ide-term`, `ide-web`, `ide-i18n`), not on
  `ide-core`/`portable-pty`. It never spawns agents, runs git or touches project files. Local state
  only: `<data>/ui.json` (selection, split, detail collapsed, language, per-session web URL, last
  repo folder, server choice, close behaviour) and the CEF profile (`<data>/web`).
- **Server choice**: Settings -> Server: Local (default: `ide-server --stdio` next to `ide.exe`,
  `IDE_DATA_DIR` passed through; the relay attaches to the running daemon or starts it detached
  without a console) or SSH (`ssh <host> [command]`, `Ssh::new`; no local server is started).
  Switching asks first and reconnects; sessions on the previous server keep running there.
  `IDE_REMOTE=ssh:<host>` overrides the setting. SSH is verified only for the UI/persistence path
  and "unreachable host -> disconnected, no local `ide-server` child"; not against a real host.
- **Terminals**: one `TermModel` per session. A session is attached (`client.attach(id,
  last_offset or None)`) once per client when it is shown, running or has output; attachments stay
  open, so background sessions stay current and switching is instant. `ClientEvent::Term` chunks
  are fed in order (`reset` -> `model.reset()` first). Input goes through one ordered pump per
  client that coalesces queued keystrokes (`term.write`), plus `term.resize` from the widget; the
  size is re-sent after every reconnect. A new `server_instance` resets all models (full replay).
- **Connection UX**: sidebar footer = server (Local / `ssh host`) + state: connecting, connected
  (server version), reconnecting, disconnected (translated reason) with a retry button. While not
  connected the terminal stays visible read-only (banner) and server actions are disabled.
- **Errors**: every `ProtoError` id (ide-core, protocol, client) maps to a `tr::err_*` message in
  `ide-app/src/i18n.rs`; unknown ids show `err-unknown` with the id and args.
- **Closing the window** never stops agents by itself: with a local server and running sessions
  the app asks "keep running in the background" (default) / "stop all agents and the server" /
  cancel, with "don't ask again" (stored as `on_close` in `ui.json`, changeable in Settings). With
  nothing running it exits without asking (and stops the daemon only if `on_close` = stop).
  Reopening re-attaches and replays the scrollback. Settings also has "Stop server…" (confirm).
- **Modal z-order**: while a modal is open the web view's GPU primitive is not drawn (the detail
  area shows a plain panel under the backdrop), so nothing can draw above the backdrop or the
  dialog. Captured before/after in `IDE_APP_SCRIPT` (`08a`/`08b`). Note: on this machine the
  unfixed build also showed the backdrop over the page (pixel check: page 109 -> 74, linear-space
  blending, so a bright page still looks bright); the fix holds whatever the cause.
