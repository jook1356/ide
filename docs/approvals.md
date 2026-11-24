# Approvals

Status: implemented (protocol 18; the wait and ended questions: protocol 20). One approval system for the ide-specific things agents (and
later widgets) ask ide to do: typing into another session, interrupting it, and next terminals,
browsers, previews (`docs/agent-features-roadmap.md`). Code: `ide-core/src/approval.rs`
(grants), server `ide-server/src/service/approvals.rs` (`UserCtx::approve`, `approval.*`), app
`ide-app/src/approvals_ui.rs` (the question, the settings list).

## Why ide asks, not the agent CLI

- **The executor enforces.** A CLI's own permission prompt is skipped in its auto / bypass
  modes, and an agent (e.g. after a prompt injection) can claim anything. The daemon, which
  carries the action out, checks the user's grant every time.
- **Context.** The CLI shows a tool name and JSON; ide shows which session, which terminal,
  which site, and what is going on there.
- **Scope.** "This session may steer that one until ide restarts" or "always": the CLI cannot
  remember ide's targets, and each provider (Claude, Codex, shell) differs.
- **Callers without a CLI.** Widget pages call `ide_*` tools too (`docs/widgets.md`).
- **Visibility.** A question inside a background session's terminal goes unnoticed; ide's
  dialog is shown over the window wherever the user is.

To avoid asking twice, the CLI pre-allows the bridge's tools where it can (Claude Code:
`permissions.allow: ["mcp__ide-bridge"]` in the session's `--settings` file). The CLI keeps
asking for its own tools (shell, edits) as before.

## Tiers

| Tier | Examples | Approval |
|---|---|---|
| Read, own session or its family | `ide_session`, `ide_list_related_sessions`, `ide_read_session`, `ide_sent_messages`, `ide_terminal_list`, `ide_terminal_read` | none |
| Act within the family, as before | `ide_send_to_session` (queued), create / merge / sync children, write access | none |
| Act on another session's running turn, or outside ide | `ide_send_to_session` `steer` / `interrupt`; `ide_terminal_input` (`terminal.input`, `docs/terminals.md`) | **asked** |
| Use a site in ide's browser | `ide_browser_open` / `read` / `act` / `screenshot` (`browser.act` / `browser.read`, target = the site's origin; an act grant also allows reading): `docs/browser-control.md` | **asked** |
| Write outside the session (another checkout, another branch, another folder) | file edits and shell commands caught by the CLI's tool hook (`docs/write-guard.md`) | **asked**, per folder (`folder.write`) |

Adding a feature means picking its tier and, for asked ones, a capability name and the target
key.

## Grants

- Key: `(capability, requester session, target)`, e.g. `("session.interrupt", child-1,
  parent)`. A capability is a dotted name (`session.steer`, `session.interrupt`; later
  `terminal.input`, `browser.act`); the target is what it acts on (a session id; later a
  terminal id, a site origin).
- Answers: 거절 (nothing kept, the agent is told), 이번만 (nothing kept), 이 세션 동안 (kept in
  memory until the daemon restarts or either session is deleted), 항상 (kept in
  `<data>/approvals.json`).
- Settings -> 권한 lists every kept grant (requester, what, target, how long) with 취소.
- Deleting a session removes the grants it requested or that target it or its terminals; removing
  a terminal removes the grants on it.

## Asking

- `UserCtx::approve(requester, capability, target, detail)` (async): a kept grant answers at
  once; otherwise the daemon asks the connected apps (`Notify::AgentRequest`, tool
  `ide.approval`, args `{capability, requester, requester_title, target, target_title,
  detail}`) and waits as long as the user chose (Settings -> 권한 -> 승인 대기 시간: 1, 5
  (default), 15 minutes, 1 hour or no limit; kept as `wait_secs` in `approvals.json`, at least
  30 s). The app answers `once` / `session` / `always`, or an error (refused, closed). No app
  connected: refused at once ("no ide window is open"); the last app gone while waiting: refused.
- "No limit" still ends a day less two minutes later (`bridge::MAX_ASK`): the CLIs need a number
  for their `PreToolUse` hook (`hooks::TOOL_HOOK_SECS`, 1 day), and ide refuses before the CLI
  gives up on the hook (a hook that times out lets the tool run). A Codex account installed with
  the older 330 s limit shows its integration as outdated until installed again.
- The app shows one question at a time (a modal); questions arriving meanwhile wait in a queue.
  Closing the dialog refuses and shows the next waiting question.
- When a question is over (answered, perhaps in another window; timed out; its app gone) the
  daemon sends `Notify::AgentRequestEnded {request, timed_out}`; the apps drop it from the
  dialog or the queue (a shown question that timed out leaves a toast).
- Every answer is audited (`approval <capability> <requester> -> <target>: <answer>`; no answer:
  `refused`, `timed out`, `no window` or `window closed`). The agent is told not to retry or
  work around a refusal and to tell the user.

## Protocol (18)

| Request | Payload |
|---|---|
| `approval.list` | -> `Approvals([ApprovalInfo {id, capability, requester, requester_title, target, target_title, always, created}])` |
| `approval.revoke` | `{id}` -> `Unit` |
| `approval.wait` (20) | -> `ApprovalWait(Option<u64>)` (seconds; `None`: no limit) |
| `approval.set_wait` (20) | `{secs}` -> `Unit` |
