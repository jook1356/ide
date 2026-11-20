# Client/server split and multi-user foundation

Status: agreed design. Phase 1 in progress.

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
