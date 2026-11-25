# Auto answers

Status: implemented (protocol 19). Some agent CLIs ask questions Meninx already knows the answer
to, e.g. Claude Code's folder trust questions on the first start in a new session folder: the
session then waits, and a first prompt queued by the parent (`meninx_create_child_session`) is
never typed in. Meninx answers such dialogs itself, per agent, as the user chooses.

Code: rules `crates/meninx-agents/screens/<provider>.toml` (`[[answers]]`, engine
`meninx-agents/src/screen.rs` `ScreenManifest::answer`), choices `meninx-core/src/auto_answer.rs`,
server `meninx-server/src/service/auto_answers.rs` (called by the screen watch), app
`meninx-app/src/auto_answers_ui.rs` (Settings -> 에이전트 자동 응답).

## Why on the screen

Meninx never writes into account folders (`docs/agent-bridge.md`), so Claude Code's trust store
(`.claude.json` in the config home) is not pre-filled, and Claude Code has no flag to trust a
folder. The screen watch (every 0.7 s when the screen changed) already reads each running agent
session; an answer rule matches a dialog the same way a state rule does and types keys.

## Rules

```toml
[[answers]]
id = "trust_workspace"            # stable: the user's choice and the UI text use it
default = true                    # on unless the user turns it off (default: true)
region = "whole"                  # as for state rules
path_after = "accessing workspace:"  # the folder: first non-blank line after the last such line
contains = ["accessing workspace:", "no, exit", "yes, i trust this folder"]
any = [{ contains = ["enter to confirm"] }, { contains = ["then enter"] }]
choose = "yes, i trust this folder"  # the list entry to pick (moves, then Enter on a later read)
# or: keys = ["\u001b[B", "\r"]     # fixed writes, one after the other, 150 ms apart
```

Gates are the state rules' (`contains`, `regex`, `line_regex`, `all`, `any`, `not`).

An `id` names the **kind** of dialog, not the agent's wording: the app shows one provider-neutral
name per kind (`trust_workspace`: the session's own folder; `trust_added_dir`: a folder added to
the session). A provider answering the same kind reuses the id; a new kind needs its UI text in
`auto_answers_ui.rs` and the locales (an unknown id is shown as is). Everything agent-specific
(texts on the screen, keys) stays in the provider's manifest, never in core, server or app.

## Safety

- A rule with `path_after` answers only when the named folder is the session's folder, one of
  its worktrees or one of their repositories (a worktree's git dir lives there), worktrees still
  being created included (the agent starts with their git dirs; seen 2026-11-25: refused, the
  "Adding a new directory" dialog blocked the session for 14 s until the worktree was made);
  otherwise the dialog stays for the user (logged). A folder line that is cut or wrapped does not match.
- The same step of a dialog (session, answer, folder, keys) is typed at most once in 5 s (2 s
  for a step that only moves the cursor); after an answer,
  and while held back by that, the screen is read again on every tick, even when it does not
  change (one "Adding a new directory" follows another, one per repository; keys the CLI missed
  leave the dialog as it was, and it is answered again after 5 s; seen 2026-11-24: a missed
  answer waited 23 s for an unrelated redraw).
- Queued prompts wait until no dialog was answered for 1.5 s and the CLI shows its input
  (`docs/session-family.md`).
- `choose` reads where the list's cursor (`❯`) is and types only Down / Up to the entry; Enter
  follows on a later read, when the screen shows the cursor on the entry. Never a move and Enter
  together: seen 2026-11-25, a Down typed right after the dialog was drawn was lost and the
  Enter confirmed "No, exit" (Claude Code quit). It never moves past the entry, and nothing is
  typed without a cursor next to it.
- Every step is audited (`auto_answer <session> <id> <folder> move|confirm`).
- Only agent sessions (not shells or login terminals) are read.

## Rules shipped

| Agent | Id | Dialog | Keys |
|---|---|---|---|
| Claude Code | `trust_workspace` | "Accessing workspace: <folder> … ❯ No, exit / Yes, I trust this folder" (v: seen 2026-11-24) | `choose` (Down; Enter once on Yes) |
| Claude Code | `trust_added_dir` | "Adding a new directory: <folder> … ❯ No, exit / Yes, add it" (an `--add-dir` folder such as a repository's git dir; one per repository; footers "Enter to confirm · Esc to cancel" or "Esc to exit · … press ↓ to Yes, then Enter") (v) | `choose` (Down; Enter once on Yes) |

Codex: its trust question ("Do you trust the contents of this directory?", numbered choices)
has no rule yet: the folder is not on a line of its own and the keys were not checked.

## Settings

`<data>/auto-answers.json` keeps only what differs from the defaults (`on` / `off` per provider
id). `ProviderInfo.answers` lists each agent's answers with the user's choice;
`provider.set_auto_answer {provider, answer, enabled}` changes one.
