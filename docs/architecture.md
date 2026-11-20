# Architecture and MVP

## Why this shape
Decided by spikes (Slint / iced / egui) recorded outside this repo. iced won on the deciding test:
a terminal hosting Claude Code / Codex TUIs (lowest draw cost, CPU and memory; Korean IME via the
public `Shell::request_input_method` API; verified with the real Microsoft Korean IME). CEF is
composited as a GPU texture (cef-rs `accelerated_osr`, wgpu 30 = iced master's wgpu).

## Crates
| Crate | Role | Depends on UI? |
|---|---|---|
| `ide-core` | Profiles, sessions, git worktrees, agent launch specs, persistence. Pure logic, unit-tested. | no |
| `ide-term` | PTY + `alacritty_terminal` state (`TermSession`) and the iced terminal widget (`TerminalView`): rendering, selection/copy, scrollback, resize, Korean IME. | iced |
| `ide-web` | CEF runtime init/subprocess handling and an iced widget showing one browser off-screen (GPU texture import, input forwarding, IME). | iced |
| `ide-app` | The binary: window layout, sidebar, session lifecycle, wiring core + term + web. | iced |

## Core concepts
- **Profile**: a named set of agent config. Holds its own Claude config dir and Codex home so skills
  and MCP servers stay per profile. Launching an agent sets `CLAUDE_CONFIG_DIR` / `CODEX_HOME` to the
  profile's dirs. Stored under the app data dir: `profiles/<name>/{claude,codex}/`.
- **Session**: one agent run = (profile, repo, worktree, agent kind, terminal). Each session gets its
  own git worktree on its own branch (`ide/<session-id>`) under the app data dir
  (`worktrees/<repo-slug>/<session-id>`), so sessions never share a checkout.
- **Agent kind**: `Claude` (`claude`), `Codex` (`codex`), `Shell` (cmd / pwsh / $SHELL).
  The agent's own TUI is the chat UI (no structured stream parsing in the MVP).
- **Detail area**: per-session tabs (web preview first; files/diffs later). Tabs belong to a session,
  so switching sessions switches the detail area.

## MVP (milestone 1)
1. Window layout: left sidebar (profiles + sessions), centre terminal of the selected session,
   right detail area with tabs; resizable split.
2. New-session flow: pick repo folder, profile, agent kind -> create worktree -> spawn the agent in a PTY.
3. Terminal widget ported from the spike: colours, wide chars, box/block drawn as quads, selection +
   Ctrl+C / Ctrl+V (bracketed paste), wheel scrollback, resize, Korean IME preedit at the cursor.
4. Session switching keeps every PTY alive (background sessions keep running).
5. Detail area: one CEF tab per session showing a URL (preview placeholder).
6. Persistence: profiles and session list (not scrollback) survive restart; worktrees are reused.

Later: file viewer/editor (iced `text_editor` + highlighter), diff viewer, previews (bundled
tsx/vue/svelte), mods, structured agent output, settings UI, packaging (bundle CEF).
