# Auto answers

Status: implemented (protocol 19). Some agent CLIs ask questions ide already knows the answer
to, e.g. Claude Code's folder trust questions on the first start in a new session folder: the
session then waits, and a first prompt queued by the parent (`ide_create_child_session`) is
never typed in. ide answers such dialogs itself, per agent, as the user chooses.

Code: rules `crates/ide-agents/screens/<provider>.toml` (`[[answers]]`, engine
`ide-agents/src/screen.rs` `ScreenManifest::answer`), choices `ide-core/src/auto_answer.rs`,
server `ide-server/src/service/auto_answers.rs` (called by the screen watch), app
`ide-app/src/auto_answers_ui.rs` (Settings -> 에이전트 자동 응답).

## Why on the screen

ide never writes into account folders (`docs/agent-bridge.md`), so Claude Code's trust store
(`.claude.json` in the config home) is not pre-filled, and Claude Code has no flag to trust a
folder. The screen watch (every 0.7 s when the screen changed) already reads each running agent
session; an answer rule matches a dialog the same way a state rule does and types keys.

## Rules

```toml
[[answers]]
id = "trust_workspace"            # stable: the user's choice and the UI text use it
default = true                    # on unless the user turns it off (default: true)
region = "whole"                  # as for state rules
path_after = "accessing workspace:"  # the folder: first non-blank line after this line
contains = ["accessing workspace:", "yes, i trust this folder", "enter to confirm"]
keys = ["\u001b[B", "\r"]         # written one after the other, 150 ms apart
```

Gates are the state rules' (`contains`, `regex`, `line_regex`, `all`, `any`, `not`).

An `id` names the **kind** of dialog, not the agent's wording: the app shows one provider-neutral
name per kind (`trust_workspace`: the session's own folder; `trust_added_dir`: a folder added to
the session). A provider answering the same kind reuses the id; a new kind needs its UI text in
`auto_answers_ui.rs` and the locales (an unknown id is shown as is). Everything agent-specific
(texts on the screen, keys) stays in the provider's manifest, never in core, server or app.

## Safety

- A rule with `path_after` answers only when the named folder is the session's folder, one of
  its worktrees or one of their repositories (a worktree's git dir lives there); otherwise the
  dialog stays for the user (logged). A folder line that is cut or wrapped does not match.
- The same dialog of a session is answered at most once in 5 s (a list must not be moved past
  its answer while the CLI redraws).
- Every answer is audited (`auto_answer <session> <id> <folder>`).
- Only agent sessions (not shells or login terminals) are read.

## Rules shipped

| Agent | Id | Dialog | Keys |
|---|---|---|---|
| Claude Code | `trust_workspace` | "Accessing workspace: <folder> … ❯ No, exit / Yes, I trust this folder" (v: seen 2026-11-24) | Down, Enter |
| Claude Code | `trust_added_dir` | "Adding a new directory: <folder> … ❯ No, exit / Yes, add it" (an `--add-dir` folder such as a repository's git dir) (v) | Down, Enter |

Codex: its trust question ("Do you trust the contents of this directory?", numbered choices)
has no rule yet: the folder is not on a line of its own and the keys were not checked.

## Settings

`<data>/auto-answers.json` keeps only what differs from the defaults (`on` / `off` per provider
id). `ProviderInfo.answers` lists each agent's answers with the user's choice;
`provider.set_auto_answer {provider, answer, enabled}` changes one.
