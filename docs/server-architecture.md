# Client/server split and multi-user foundation

Status: agreed design. Phase 1 implemented, including the app migration (`ide-app` is a pure
client) — see "Phase 1 implementation" and "App integration". Phase 2 backend (files, watch,
browse, search, git, go-to-definition) implemented in proto/server/client — see "Phase 2
backend" at the end; the UI widgets (`ide-views`) are integrated separately.

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
| `ide-core` | server | Accounts, configurations (+ profile migration), sessions, worktrees, session ops. |
| `ide-agents` | server | Agent providers (Claude, Codex, shell, declarative TOML): detection, account env, login, configuration -> launch spec. See `docs/agent-providers.md`. |
| `ide-server` | server | Daemon: local socket listener, `--stdio` relay mode, services (providers, accounts, configurations, sessions, PTY host), `Runner`, path jail, audit hook. |
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
| Data layout | Per-user data dir (the server's account): `accounts.json` + `accounts/`, `configurations.json`, `providers/`, `sessions.json` + `sessions/<id>/agent/`, `worktrees/`, `ui/`. The model keeps `user_id` so a gateway can address users. |
| Authorization | One layer resolves every resource id (session, account, configuration, stream) for the connection's user before handlers run. |
| Path jail | Client paths resolve inside the user's allowed roots (their worktrees, registered repos); canonicalize; reject `..` and symlink escapes. All file/git/search RPCs go through it (phase 2). |
| Runner | All process spawning goes through `trait Runner`. Now: `SameUserRunner`. Containers/limits are optional later. |
| Secrets | Claude/Codex logins stay in the server-side account dirs; never sent to clients (accounts are listed without paths). |
| Audit | `audit(user, action, target)` hook on session start/stop, process spawn, file writes. Now: a log file. |

Deferred: gateway + SSO, admin UI, quotas/limits, containers, sharing sessions between users.

## Compliance note
Agent CLIs run under each user's own login (per-user accounts on a per-user server). One subscription
account must not be shared by several people; check the Claude/Codex terms for server-side use before
deploying to a company server.

## Phases
1. **Local split**: `ide-proto`, `ide-server`, `ide-client`; sessions/accounts/worktrees and PTYs behind
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
| Providers, accounts (incl. login sessions), configurations; sessions (create via `ide-core` incl. worktree, start/stop/restart/delete/rename) | done |
| Server-side Term answering queries; 8 MiB ring with byte offsets; attach/resume; title/bell/state notifications | done |
| `UserInfo` from the transport, authorization (`Server::authorize` + `UserCtx::resolve_session`), `Runner`/`SameUserRunner`, file audit, per-user data dir | done |
| Path jail | module + tests (`ide_server::PathJail`); used by every phase 2 RPC and `session.create` |
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

## App integration (phase 2)
`ide-app/src/detail.rs` (state + logic) and `detail_view.rs` (drawing), mirroring Hermes Desk's
`DetailTabs` + `ChangesPanel`:
- **Layout**: sidebar | terminal | divider | detail pane | side panel (fixed 290 px, outside the
  split; the divider shares the rest). The detail pane has one header row: the shown item's
  identity (file-type icon, name, dirty dot, "kind · folder") or, for the web page, its
  navigation (back / forward / reload / URL); then an "Open items" switcher (popover grouped
  Browser / Changes / Explore with close buttons) and the collapse button. No tab strip.
  The side panel has the tabs Changes (count badge) / Explore / Search.
- **Per session** ([`Detail`] in `Live`): open items (`Web` is always there; files in
  `CodeView`, diffs in `DiffView`, disk-vs-buffer comparisons), the tree, the changes list, the
  search. Open items, the shown item and the side tab are stored in `ui.json`
  (`detail.<session>`) and restored lazily (files are read when shown).
- **Watch**: only the shown session is watched (`fs.watch`; the others are unwatched and
  marked stale, refreshed when shown again; everything is marked stale on reconnect).
  `FsChanged` re-lists the loaded folders on the changed paths' ancestry (all on `rescan`),
  refreshes `git.status`, re-reads open files (clean buffers reload; dirty ones get a "changed on
  disk" bar; a missing file gets a "deleted on disk" bar) and reloads open diffs of those paths.
  `GitChanged` re-lists loaded folders (badges), `git.status` and diffs.
- **Files**: `fs.read` (4 MiB; binary -> placeholder, truncated -> read-only + note), save with
  `fs.write{expected_version}`; `fs-conflict` -> bar with reload (drop edits) / save my version
  (no version check) / compare (DiffView disk -> buffer, computed client-side with `similar`) /
  dismiss. Closing a dirty file asks (save / don't save / cancel).
- **Tree actions** (context menu / F2 / Delete of `FileTree`): new file / folder, rename (open
  items follow), delete (confirm; recursive for folders; open items below are closed).
- **Changes**: `git.status` -> staged / unstaged (+ conflicted) / untracked; click -> `git.diff`
  (`Index` for staged, else `WorkingTree`) plus the whole new side (`fs.read` / `git.file_at`
  Index) so gaps can expand; unified / split from the diff header, remembered per session.
- **Search** (Ctrl+Shift+F): `search.text` streamed into the panel through `Task::run` +
  `abortable` (a new search aborts the old one, dropping the `SearchStream` cancels it on the
  server); results grouped by file (collapsible), matches highlighted; click opens at line:col.
- **Ctrl+P**: quick-open modal over `search.files` (stale replies dropped by sequence number),
  arrows + Enter. Shortcuts are window-wide (`event::listen_with`, physical keys so they work in
  Korean input mode); the terminal also receives the key.
- **Go to definition**: `CodeEvent::Definition` -> `def.lookup` with the buffer text; `One` opens
  the target at line:col, `Many` shows a picker, `None` shows the translated reason as a
  transient message (5 s) at the bottom of the pane.
- **New-session dialog**: the native folder picker (`rfd`) is gone; a folder icon opens a
  server-side browser (`browse.roots` in a picker, `browse.list`, parent button, git repos marked
  green; browsing into a repository fills the folder field). The text field stays.

## Phase 2 backend (files, search, git, definitions)
Implemented in `ide-proto` (`files.rs` wire types + `Request`/`Response`/`Notify` variants),
`ide-server` (`jail`, `fsops`, `watch`, `search`, `git`, `def/`) and `ide-client` (typed methods,
`SearchStream`). Capabilities in `Welcome`: `files`, `browse`, `search`, `git`, `def`.

| Piece | Rule / state |
|---|---|
| Paths on the wire | Root-relative to the session worktree, `/`-separated (`""` = root) for every session RPC; absolute only for `browse.*` and `session.create.repo_path`. Lines 1-based, columns 1-based **chars**; search match ranges are byte offsets into `line_text`. |
| Jail | `RootJail` per session (canonical worktree): rejects absolute/drive/UNC paths, `..`, `:` (ADS), control chars, `<>"|?*`, Windows device names, trailing dot/space; resolves the longest existing prefix (symlinks and junctions) and requires it inside the root. Delete/rename/stat act on the link itself (`resolve_entry`), never its target; recursive delete removes links without descending. Writes into `.git` are denied; `.git` is never listed. `PathJail` = browse roots: the home dir (`USERPROFILE`/`HOME`) plus `browse_roots` of `<data>/server.json` (`{ "browse_roots": ["D:/src"] }`, re-read per call), or `ServerConfig::browse_roots`. `session.create` requires the repo inside the browse roots (new: `path-denied` otherwise). |
| fs | list (dirs first; `ignored` from every `.gitignore` root..dir + `info/exclude` + global excludes, ignored parents propagate; git badge per entry, `Modified` for dirs with changes below), stat, read (default 2 MiB, max 8 MiB; BOM/UTF-16/UTF-8, else `chardetng` guess e.g. EUC-KR; binary = NUL in the first 8000 bytes; `truncated`), write (`expected_version` conflict check under a per-user write lock; encodes back to the read encoding), create_file/create_dir/rename/delete. Mutations are audited (`fs.write`, `fs.create_file`, `fs.create_dir`, `fs.rename`, `fs.delete`; target `<session> <path>`). |
| Versions | `h<len>-<fnv1a64>` of the whole file (≤ 16 MiB), else `m<len>-<mtime ns>`; blobs from `git.file_at` use the content hash too. |
| Watch | `fs.watch{session}` per connection (idempotent); one `notify` watcher per session shared by all watching connections, dropped with the last unwatch/disconnect/session delete. Debounce: 80 ms quiet, ≤ 400 ms delay; > 1000 paths -> `FsChanged{rescan: true}`; events under `.git` and the linked worktree's git dir -> `GitChanged`. Notifications go only to watching connections. The client re-watches after a reconnect (no replay of the gap). |
| Search | `search.text`: client-chosen `search` id, reply `Unit` first, then `Notify::SearchHits` batches (100 lines / 100 ms) and one `SearchDone` to that connection only; `search.cancel` or disconnect stops it. `ignore` walker (gitignore unless `include_ignored`, hidden files included, `.git` skipped, files > 8 MiB skipped, binary files skipped), `grep-regex` (literal unless `regex`, case, whole word), override globs. Default cap 2000 lines. `search.files`: `nucleo-matcher` path matching over non-ignored files (≤ 300k), smart case. |
| Git | `git.status`: porcelain v2 (`--find-renames`, untracked files listed individually), upstream ahead/behind, base = request `base` or the branch checked out in the session's repo (else `main`/`master`), ahead/behind + merge base. `git.diff`: `WorkingTree` (index->worktree), `Index` (HEAD->index), `Head` (HEAD->worktree), `Base` (merge base->worktree); structured hunks with old/new line numbers, `no_newline`, binary flag, renames (the old path is added to the pathspec when asking for one renamed file), untracked files as all-added (`--no-index`). `git.file_at`: `Head`, `Index`, `Base`, `Rev(commit-ish)` -> `Option<FileContent>`. Revisions starting with `-` or containing `:`/`..`/whitespace -> `git-bad-rev`. All via `ide_core::git_raw` (`-c core.quotepath=false`, `GIT_TERMINAL_PROMPT=0`, stdin closed, `GIT_OPTIONAL_LOCKS=0`, no console window) — **not through `Runner` yet**; move it there when runners isolate users (containers). |
| Definitions | `def.lookup{session, path, line, col, text?}`: port of Hermes Desk `definitions.ts` + langpacks (JS, TS + tsconfig `paths`/`baseUrl`/`extends`/solution `references`, Python, Vue SFC): import specifier -> module file; `ns.member` of a module import; parameters of the enclosing function win; nearest same-file declaration above; import bindings followed through re-exports (`export {a as b} from`, `export *`, `export default x`, Python `__init__` re-exports) up to 3 hops; external modules -> the import binding; finally `git grep -w --untracked` over the pack's globs, top-level declarations preferred, same directory first, ≤ 50 targets. Reads are jailed and ≤ 2 MiB. The TS unit tests are ported (`def/tests.rs`, `def/{js,py,ts,vue}.rs`). Deviation: identifiers may contain non-ASCII characters (Hermes Desk treated them as separators). |

Error ids added (args): `path-invalid` (`path`), `fs-not-found` / `fs-exists` / `fs-permission` /
`fs-dir-not-empty` / `fs-not-a-dir` / `fs-is-a-dir` (`path`), `fs-io` (`path`, `detail`),
`fs-conflict` (`path`, `expected`, `actual`; `actual` empty = deleted on disk),
`fs-bad-encoding` (`encoding`), `fs-unencodable` (`path`, `encoding`), `watch-failed`
(`detail`), `search-empty`, `search-bad-regex` (`detail`), `search-bad-glob` (`glob`,
`detail`), `git-bad-rev` (`rev`), `git-no-base` (`base`), `bad-request` (`method`). `def.lookup`
"nothing found" reasons (inside `DefResult::None.reason`): `def-invalid-position`,
`def-unsupported-file` (`path`), `def-no-name`, `def-external-module` (`spec`),
`def-module-not-found` (`spec`), `def-external-name` (`spec`), `def-not-found` (`name`).
Existing ids reused: `path-denied` (`path`), `session-not-found` (`id`), `git` / `git-spawn`,
`message-too-large` (now also sent when a reply exceeds 16 MiB).

Browse roots are editable from the app (Settings -> Repository folders): `browse.config`
(home dir + the `browse_roots` stored in `server.json`) and `browse.set_roots{roots}` (each root
absolute, existing, a folder and not a filesystem root -> canonical, deduplicated, at most 64;
`path-invalid` / `path-denied` / `fs-not-found` / `fs-not-a-dir`; other keys of `server.json`
are kept; written atomically; audited as `browse.set_roots`).

Known gaps (phase 2): no file-list cache for `search.files` (walks per call); text search is
single-threaded; diff lines strip a trailing `\r` (CRLF is not shown); `fs.list` badges run one
`git status` per listing; no trash/recycle bin; the watcher does not follow links; writes are not
atomic (in-place, like the editor in Hermes Desk); git runs outside `Runner`.
