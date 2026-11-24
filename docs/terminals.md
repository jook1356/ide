# Terminal tools (agents read and type into user terminals)

Status: implemented (roadmap feature 1, `docs/agent-features-roadmap.md`; no new protocol
requests). Hermes Desk's `terminal_inspect` / `terminal_input` for ide's agents: the agent of a
session reads the user's terminals of that session (the terminal panel's shells and commands,
`uterm.*`), and types into them once the user allowed it. Code: `ide-server/src/service/terminals.rs`
(tools, key table, read output), `pty.rs` (`TermHost::history_text`, `app_cursor`), `ports.rs`
(`by_root`). Test: `ide-server/tests/terminals_e2e.rs`.

## Tools (answered by the daemon)

| Tool | Args | What |
|---|---|---|
| `ide_terminal_list` | `{session?}` | this session's user terminals (or a related session's): id, title, command, folder, state (`running`, `stopped`, `exited (code N)`), running, and the TCP ports listening in each terminal's process tree (`ports::by_root`, the same scan as `uterm.ports`) |
| `ide_terminal_read` | `{terminal, lines?}` | a header (id, title, state, command, folder, size, the owning session when it is not this one), up to `lines` rows that scrolled above the screen (default 50, at most 2000, 0 = none; leading blank rows dropped; a hint when more are kept), then the visible screen (trailing blank rows dropped). Plain text, no escape sequences |
| `ide_terminal_input` | `{terminal, text?, keys?}` | types `text` (line breaks become Enter, `\r`), then `keys` (an array of names, or one string of blank-separated names); asks the user first (below) |

## Scope

- Own session: list, read, type. Related sessions (parent, children, siblings;
  `ide_core::family::related`): list and read only; typing into them is refused with a pointer
  to `ide_send_to_session`. Other sessions' terminals and 공용 terminals (`g:t<n>`) are "not
  found" for agents.
- The session's own agent terminal is not among them (its conversation: `ide_read_session`).

## Reading

User terminals' server-side `Term` keeps 2000 rows of scrollback (`TermHost::with_scrollback`,
`terminals::SCROLLBACK`; sessions' agent terminals keep none, as before). Reading the grid
instead of the output ring means what an agent reads is what the terminal shows: cursor
movement, `\r` overwrites and ConPTY's repaints are already applied, wide characters are one
character, colours and other escape sequences are gone. Rows are the grid's rows: a long line
the terminal wrapped is several rows. Memory: alacritty grows the history as rows scroll out, so
an idle terminal costs nothing extra; a full one about 2000 x columns cells.

## Keys

One table, `service/terminals.rs::key_bytes(name, app_cursor)` (unit-tested), case-insensitive,
modifiers `ctrl` / `alt` (`meta`, `option`) / `shift` joined with `+`:

| Name | Bytes |
|---|---|
| `enter` (`return`), `tab`, `esc` (`escape`), `backspace`, `space` | `\r`, `\t`, `ESC`, `DEL` (0x7f), ` ` |
| `shift+tab` (`backtab`) | `ESC [ Z` |
| `up` `down` `right` `left` `home` `end` | `ESC [ A`..`D`, `H`, `F`; `ESC O x` when the program set application cursor keys (`ESC[?1h`); with modifiers `ESC [ 1 ; m x` (xterm: m = 1 + shift + 2 alt + 4 ctrl) |
| `insert` `delete` (`del`) `pageup` (`pgup`) `pagedown` (`pgdn`) `f5`..`f12` | `ESC [ n ~` (2, 3, 5, 6, 15, 17-21, 23, 24); with modifiers `ESC [ n ; m ~` |
| `f1`..`f4` | `ESC O P`..`S`; with modifiers `ESC [ 1 ; m P`..`S` |
| `ctrl+<letter>`, `ctrl+@`/`space`, `ctrl+[ \ ] ^ _ ?` | 0x01-0x1a, 0x00, 0x1b-0x1f, 0x7f (`ctrl+c` = 0x03); `ctrl+backspace` = 0x08 |
| a single character (`y`, `+`, `é`) | itself; `shift+` upper-cases it |
| `alt+<any of the above>` | `ESC` before it |

Combinations without a standard encoding (`ctrl+enter`, `shift+backspace`) are refused, not
approximated. Ctrl+C is input too: it stops a running command and needs the same approval.

## Approval

`ide_terminal_input` asks through `docs/approvals.md`: capability `terminal.input`, target the
terminal id, detail the text (first 400 characters, control characters shown as U+FFFD) and the
keys as `[name]`. The question names the terminal as "<terminal title> (<session title>)"
(`UserCtx::uterm_label`). Kept grants follow the terminal: removing it, or a new terminal getting
the same id after a daemon restart (ids count from 1 again), drops them, and deleting the session
drops the grants on its terminals (`Grants::forget_session` also matches `<session>:...`
targets). So "always" lasts as long as the terminal does. Every input is audited
(`terminal.input <session> -> <terminal>`).

## Verified

- Unit: the key table, text and key-name parsing, the read output's trimming, scrollback rows of
  a `TermHost`, `ports::by_root`, grants of terminal targets (`ide-core` `approval::tests`).
- `terminals_e2e.rs` with real shells (Windows: `cmd`): list (own / related / unrelated), read
  (header, related read only, unknown), input errors before any question, a refused and an
  allowed question (capability, target, titles, detail), output appearing, the kept grant
  answering, 120 rows read back from the scrollback in order, `lines` limiting it, a stopped
  terminal refusing input, removal dropping the grant.
- (a) Ctrl+C stopping a program through ConPTY is not checked on Windows: under the test runner
  `ping` kept running after 0x03 (a process tree started with Ctrl+C ignored passes that on to its
  children). On Unix the test checks that `sleep` is stopped. Check by hand in the app: ask an
  agent to stop a running command in one of its terminals.
