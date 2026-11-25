# Session handoff (세션 인수인계)

Status: implemented (protocol 23). Model and prompt texts: `ide-core/src/handoff.rs`; server:
`ide-server/src/service/handoff.rs`; transcript export: `AgentProvider::handoff_transcript`;
app: `ide-app/src/handoff_ui.rs` (row menu 세션 인수인계, dialog, running notice).

A session's agent conversation is replaced by a new one that continues from a handoff
document (or the previous transcript). Goes beyond Hermes Desk (no such feature).

## Model: same session, new conversation

The **session stays** (its id, folder, worktrees, branch `ide/<id>`, title, parent, children,
write lease, queued messages); only its agent conversation ([`Session::conversation`]) is
replaced, optionally with another agent (provider, account, configuration). Everything that
names the session keeps working without changes:

- Children keep `parent = <id>`; separate children keep their base `ide/<id>`; shared
  children keep working in the same folder.
- `ide_send_to_session` from a child reaches the new conversation: messages are routed by the
  session id. Messages that arrive during the handoff are held in the session's queue
  (`BridgeState::enqueue`, keyed by the session id, kept across process restarts) and typed in
  once the new conversation is at its input after its first turn.
- `ide_read_session` reads the current (new) conversation.

The replaced conversation is kept in [`Session::previous_conversations`] (provider, account,
conversation id, when), newest last, at most 20.

## Methods

| Method | 방식 (dialog) | What happens |
|---|---|---|
| `Previous` | 기존 대화가 요약 | The old conversation writes the handoff document: ide types a request into it (`handoff_request_prompt`) naming the file (`sessions/<id>/handoff/<n>-handoff.md`). Done when its turn ended and the file is not empty. A stopped session whose conversation can be resumed is started (resumed) first. |
| `Summarize` | 새 대화가 요약 | ide exports the old transcript (`<n>-transcript.md`); the new conversation's first prompt asks it to write the handoff document from it first, then continue. |
| `Transcript` | 요약 없이 넘김 | ide exports the old transcript; the new conversation reads it as its context, without writing a summary. |
| `Auto` | 자동 (default) | `Previous` when the old agent is running and idle (or stopped with a resumable conversation); `Summarize` when it is busy, or it cannot be resumed and its transcript can be read. A `Previous` that fails falls back to `Summarize` when the transcript can be read. |

`Previous` fails (and `Auto` falls back) when: the old process exits, its turn ends without
the file, or no file appears within 10 minutes (`HANDOFF_TIMEOUT`); a `Previous` started
while the agent works waits for its turn to end within the same time. An explicit `Previous`
does not fall back: the session is left as it was (old conversation still running) and the
error is reported.

`Summarize` / `Transcript` need the old provider's transcript
(`AgentProvider::handoff_transcript`: Claude Code and Codex): otherwise
`handoff-no-transcript`.

### Compaction (컴팩트된 대화)

Claude Code's compaction (`/compact` or automatic) keeps the conversation id and the same
transcript file: it appends a `{"type":"system","subtype":"compact_boundary"}` record and a
user record with `isCompactSummary: true` holding the summary, and keeps writing after them;
the records before the boundary stay in the file (seen with v 2.1.332). The export starts at
the **last** boundary: the compaction summary, then everything after it — what the model
itself still had in its context. Earlier records are left out (the summary covers them). With
no boundary the whole transcript is exported. Subagent (`isSidechain`) and meta records are
left out, as in `ide_read_session`; texts are kept whole, tool calls and results shortened
(`HANDOFF_TOOL_MAX`).

Codex keeps one rollout per thread (`<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<time>-<uuid>.jsonl`)
and records a compaction as `{"type":"compacted","payload":{"message","replacement_history"?}}`:
the replacement history is what the model kept (the user's earlier messages and the summary),
and later records continue after it. The export starts at the last such record: its
replacement history, or its `message` (the summary) when it has none, then everything after
it. Only `response_item` records are read (`event_msg` repeats them for the UI); Codex's own
context (`AGENTS.md` instructions, `<environment_context>` and other tagged user messages,
developer messages) and reasoning are left out. A session that only has the start placeholder
(`started:<ms>`, no `SessionStart` hook) finds its rollout as a resume does (the earliest
rollout of the session folder since the start). A compressed rollout (`.jsonl.zst`) is not
read (`handoff-transcript-failed`); the rollout of a live thread is plain JSONL.

## Agent change

The dialog picks agent, account and configuration (the session's at first), as the 하위 세션
만들기 dialog does; the request sends `agent: None` when unchanged. They are checked like
`session.create_child`'s (provider, account and configuration must exist). The session's
`provider` / `account` / `configuration` change when the new conversation starts; the
handoff document and transcript are plain Markdown, so any provider can read them.
`Previous` still asks the **old** agent to write the document.

## Files

`<data>/sessions/<id>/handoff/` (removed with the session): `<n>-handoff.md`,
`<n>-transcript.md` (`<n>` = Unix seconds of the request). The folder is passed to every agent
of the session as an outside directory (`SessionContext::extra_dirs`, `--add-dir`), so the
agent reads and writes it without a permission question; `sessions/<id>` is already a free
folder for the write guard (`docs/write-guard.md`). An agent started before the folder existed
may ask once.

## Server

- `session.handoff {id, method, agent?}` -> `Session` (at once; the work goes on in the
  background). Refused: not an agent session (`handoff-not-agent`), one already running
  (`handoff-busy`), no way to get the context (`handoff-no-transcript`), agent checks as
  above. Audit `session.handoff`.
- `session.handoff_cancel {id}` -> `Session`: drops a handoff that is waiting for the old
  agent's document (the old conversation keeps running). Audit `session.handoff_cancel`.
- State in memory (`UserCtx::handoffs`): a daemon restart drops a handoff in progress.
- Driven by the screen tick (`screen_tick` -> `handoff_tick`, every 0.7 s): sends the request
  when the old agent is at its input, watches its turn, the file, the process and the timeout.
- Finishing: stop the old process, push the old conversation to `previous_conversations`,
  apply the agent change, clear `conversation`, start with the first prompt
  (`handoff_first_prompt`; on the command line when the provider takes one there, else
  queued first). Audit `session.handoff_done`.
- While a handoff runs, queued messages to the session are not typed in (`deliver_pending`),
  and `ide_send_to_session` queues for it even when its process is down for the switch.
- `SessionInfo.handoff`: `Some(HandoffStatus {method, phase})` while it runs (`phase`:
  `Waiting` for the old agent's input, `Writing` the document, `Starting`).
  `Notify::Handoff {session, result}` when it ends: `Done {method}` (the method that made
  it, after a fallback `Summarize`) or `Failed {error}`.

## App

- Row menu (agent sessions): 세션 인수인계 opens the dialog: 방식 (자동 / 기존 대화가 요약 /
  새 대화가 요약 / 요약 없이 넘김, with hints), agent / account / configuration.
- Submit closes the dialog and shows a running notice for the session; `Notify::Handoff`
  turns it into a success toast or an error. The row shows 인수인계 중 while
  `SessionInfo.handoff` is set; the menu offers 인수인계 취소 then.
