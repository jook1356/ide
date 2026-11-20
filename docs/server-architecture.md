# Client/server split and multi-user foundation (design draft)

Status: draft for review. Nothing here is implemented yet.

## Goals
- The app (`ide`) only draws: terminal rendering, IME, editor views, CEF display, layout, theme, language.
- A backend daemon (`ide-server`) does all work: agent CLIs (claude, codex, shells) in PTYs, git
  worktrees, files, search, diffs, go-to-definition, preview bundling/serving, mods, dev servers.
- The same server runs locally (spawned by the app) or on a company server (remote).
- Multi-user from day one in the data model, identity and API; auth providers and OS isolation plug in later.

## Crates
| Crate | Side | Role |
|---|---|---|
| `ide-proto` | shared | Versioned message types (serde), RPC enums, stream frames, error codes (i18n ids). No I/O. |
| `ide-core` | server | Profiles, sessions, worktrees, launch specs (exists). Becomes user-scoped. |
| `ide-server` | server | Daemon: transport, auth, authorization layer, path jail, `Runner`, PTY host, file/git/search services, HTTP proxy for previews. |
| `ide-client` | client | Connection, handshake, RPC calls, stream subscriptions, reconnect; exposes iced `Subscription`s. |
| `ide-term` | client | Terminal widget + VT parser (PTY code moves to the server). |
| `ide-web` | client | CEF (exists). Loads preview/mod URLs served through the server proxy. |
| `ide-i18n` | client | UI strings (server sends error ids + args, never localized text). |
| `ide-app` | client | UI only; talks to `ide-client`, never spawns processes or touches files itself. |

## Transport
- One WebSocket per client connection (local: `127.0.0.1` + random port + one-time token; remote:
  TLS, or an SSH tunnel at first). Frames: RPC request/response/notification (MessagePack or JSON)
  and binary stream frames `{stream_id, seq, bytes}` multiplexed on the same socket.
- Handshake: `Hello{proto_version, client_version, auth}` -> `Welcome{user, server_version, capabilities}`.
  Version mismatch is a clear error, not undefined behaviour.
- HTTP on the same port for previews/mods: `/u/<user>/s/<session>/preview/...` and port forwards
  `/u/<user>/s/<session>/port/<n>/...`, authorized by the same session token (cookie for CEF).

## Terminal streaming
- The server owns the PTY. Output bytes go to a per-session ring buffer (e.g. 8 MiB) and to every
  attached client stream. On attach/reconnect the client sends its last `seq`; the server replays from
  there, or from the ring start (the client resets its parser) if it fell too far behind.
- The server also keeps an `alacritty_terminal` instance per session so it can answer
  `snapshot()` (current screen as VT) for a fresh client and expose the title/exit state without a client.
- Input: `write(session, bytes)`, `resize(session, cols, rows)`. IME stays client-side; only committed
  text is sent. Agents keep running when no client is attached.

## Multi-user foundation (now)
| Piece | Rule |
|---|---|
| Identity | Every connection is authenticated by an `Authenticator` (trait). Now: `LocalTokenAuth` (single local user). Later: OIDC/SSO, API tokens. The `UserId` comes only from the connection context, never from request fields. |
| Data layout | `data/users/<user_id>/{profiles,sessions.json,worktrees,ui}`; server-global config in `data/server.toml`. The local single user is a normal user (`local`). |
| Authorization | One `Authz` layer checks ownership for every resource id (session, profile, file handle, stream) before handlers run. Handlers receive already-authorized handles. |
| Path jail | Client paths are resolved against the user's allowed roots (their worktrees, registered repos). Canonicalize, reject `..` escapes and symlinks leaving the root. All file/git/search RPCs go through it. |
| Runner | All process spawning goes through `trait Runner { spawn(user, LaunchSpec) -> Child }`. Now: `SameUserRunner` (current OS account). Later: `UnixUserRunner` (per-user OS account) and `ContainerRunner` (namespaces/cgroups) on a Linux server. |
| Secrets | Claude/Codex logins live in per-user profile dirs (mode 0700 on Unix). Never sent to clients. |
| Audit | `audit(user, action, target)` hook on session start/stop, file writes, process spawns. Now: log file. |
| Protocol version | In the handshake from the first release. |

Deferred: SSO integration, admin UI, quotas/limits, container runtime, sharing sessions between users.

**Why OS isolation matters:** agents run arbitrary code. If all users' agents run as one OS account,
one user's agent can read another user's sources and credentials no matter what the app checks.
`Runner` keeps that change local to one module.

## Compliance note
Agent CLIs run under each user's own login (per-user profiles). One subscription account must not be
shared by several people; check the Claude/Codex terms for server-side use before deploying.

## Phases
1. Local split: `ide-proto`, `ide-server`, `ide-client`; move PTY + sessions/profiles/worktrees behind
   RPC; app spawns the local server; reconnect + replay; all multi-user foundation pieces with one user.
2. Files: tree, read/write, watch, search, git status/diff, go-to-definition (port Hermes Desk logic).
3. Previews/mods: server-side bundling + HTTP proxy, dev-server port forwarding.
4. Remote: TLS/SSH tunnel connect UI, connection profiles, latency handling.
5. Server deployment: SSO, per-user OS isolation runner, audit storage.
