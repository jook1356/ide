# Automatic session titles

A session created without a title gets the default `<provider> · <repos>`. Its first prompt
then names it; an agent CLI that names its conversations itself replaces that name with its own
once it has one. A title the user gave (new-session dialog, rename, `meninx_create_child_session`)
is never replaced.

Marks as in `agent-providers.md`: (v) checked with the CLI version given, (d) docs, (a) assumption.

## State (`meninx_core::AutoTitle`, `Session::auto_title` in `sessions.json`)
| State | Meaning | Leaves it |
|---|---|---|
| `off` (default, not written) | the user's title, or already final | — |
| `pending` | the default title (created with an empty title) | first prompt with a title -> `provisional` (CLI names conversations) or `off` |
| `provisional` | named from the first prompt | the CLI's own title at the end of a turn -> `off` |

`SessionStore::set_title` (rename) always sets `off`; `set_auto_title` sets a title and the
next state. Sessions written before this field have `off` (no change for them).

## Provider contract (`meninx-agents`, trait `AgentProvider`)
```rust
fn prompt_title(&self, prompt: &str) -> Option<String>;   // default: title::from_prompt(prompt, &[])
fn names_conversations(&self) -> bool;                     // default: false
fn conversation_title(&self, acct: Option<&AccountDir>, id: &str) -> Option<String>; // default: None
```
- `prompt_title` runs synchronously on the hook, without a model call. The default
  (`title::from_prompt`) takes the first meaningful line (Markdown markers, code fences and
  `@file` mentions removed), cuts it at the end of its first sentence and at
  `TITLE_MAX` = 40 characters (word boundary, `…`). `None` = no request to name the session
  by; the next prompt is tried.
- `names_conversations` + `conversation_title`: the CLI's own name of the stored conversation
  (`Session::conversation`), read at each `Stopped` hook while the title is `provisional`
  (also while `pending`, e.g. when the CLI reported no prompt text).
- `SessionInput::auto_title` / `LaunchPlan::auto_title`: the title is not final; a CLI whose own
  naming a fixed name would switch off does not get it as one.
- The prompt text reaches the server through the hook: `HookEvent::PromptSubmitted { prompt }`
  -> `AgentHookEvent::PromptSubmitted { prompt }` (serde default: older hook programs send
  none). The server (`Service::auto_title`) only calls these trait methods; no provider id is
  matched outside `meninx-agents`.

## Providers
| | Claude Code | Codex | Shell / declarative |
|---|---|---|---|
| Prompt text | `UserPromptSubmit` hook `prompt` (d) | `UserPromptSubmit` hook `prompt` (d), only with the account integration installed | — (no hooks) |
| `prompt_title` | default rules; `/…` (slash commands) and `!…` (bash mode) name nothing | same (`/…`, `!…` shell commands) | default |
| Own title | (v 2.1.3xx) `{"type":"ai-title","aiTitle":..}` lines in `<config home>/projects/*/<session id>.jsonl`; written only when the CLI was started **without** `-n` (Meninx sessions with `-n` have `custom-title` / `agent-name` lines only). A later `/rename` writes `custom-title`, which wins (last line). | (v 0.172.0) `$CODEX_HOME/session_index.jsonl` lines `{"id":"<thread>","thread_name":"..","updated_at":".."}`; the last one of the thread wins | — |
| Conversation id | `--session-id` / `SessionStart` `session_id` (existing resume support) | `SessionStart` `session_id` (d; assumed to equal the thread id in `session_index.jsonl` (a)); stored but not used for resuming (`Caps::resume` off) | — |
| Launch | no `-n` while `auto_title`; `-n <title>` again once final (the CLI then keeps that name as `custom-title`) | nothing to pass | `{title}` placeholder gets the current title |

## Not done / to verify
- (a) Claude Code writes `ai-title` soon after the first prompt in a session started without
  `-n`, and the `UserPromptSubmit` input has `prompt` with the typed text; both need a logged-in
  session with a real prompt (paid: not run by tests). Seen in the user's own transcripts of
  sessions started outside Meninx.
- (a) Codex's `SessionStart` hook `session_id` equals the `session_index.jsonl` thread id.
- The running CLI is not told about a title found later (Claude would need `/rename <title>`
  typed into its TUI); the next start passes it with `-n`.
- A real summary of the request (model call such as `claude -p` / `codex exec`) is not used:
  it costs a paid turn per session. A provider could do it inside `prompt_title`, but that
  must not block the hook (would need an async variant).
- The app shows the new title through `SessionsChanged`; there is no UI to turn automatic titles
  off.
