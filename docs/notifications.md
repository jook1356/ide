# Notification centre

Hermes Desk's notice centre (`lib/notices.ts`, `components/NoticeCenter.tsx`) in the ide:
outcomes that are not questions go to one list. The navigation rail's bell shows the unread count
and opens the list, and a notice can also pop up as a toast.

- Store: `crates/ide-app/src/notices.rs`. It has no widgets and is generic over the app's
  `Message`, so it can be unit-tested.
- UI: `crates/ide-app/src/notice_view.rs`. This covers the bell, the list panel, the toasts and
  the producers that mirror other state.

## Model

A `Notice` has these parts:

| Part | Meaning |
|---|---|
| `tone` | `Info`, `Success`, `Warning` or `Error` (`footer.rs` re-exports it as its PR job tone) |
| `title`, `body` | `Text`, either `Plain` (user data, git or server output) or `Tr` (a closure that calls `tr::*` each time the notice is drawn, so a language switch re-translates it) |
| `session`, `repo` | What the notice is about: a session chip in the list (clicking it shows that session) and the repository name |
| `key` | `NoticeKey`: a notice that follows one piece of work. Pushing again with the same key replaces the notice in place, keeping its id and position. |
| `read` | Counted by the bell while unread. `quiet()` pushes the notice already read, as history only. |
| `task` | `Running`, `Done` or `Failed`. While any notice is running, the bell shows a turning loader and the list header shows "N running". **Clear** keeps running notices. |
| `open` | A `Message` sent when the notice or its toast is clicked. Clicking also marks it read and closes the list. |
| `actions` | Buttons, each with a label, a `Message` and a style. |
| `toast` | Pops up on the window: success and info for 5 s, warnings for 10 s, errors for 8 s. |

The store keeps at most 200 notices, dropping the oldest finished ones first. It is in memory
only; buttons are messages, which mean nothing after a restart.

## Where notices come from

| Source | How | Tone, read state, toast |
|---|---|---|
| Detail-pane toasts (Changes, History, Explore, footer sync / merge / close, item and session menus, user terminals, attachments, go to definition, file operations) | `Detail::toast(tone, text)` puts the toast in an outbox. `App::update` moves it to the centre after each message (`take_detail_toasts`) with the session as the notice's session. | Success and info are toasts and history only (read). Warnings and errors are toasts and unread. |
| Agent `ide_show_message` | `agent_bridge.rs` | Info, unread, toast. Clicking selects the session. |
| Lists refresh failed, pin / archive failed, some worktrees not created, base branches changed by a rule, family merge / sync result | Former sidebar banner (`App::banner` is gone) | Error / info / mixed (a merge with conflicts is a warning, one with a failure is an error) |
| PR job (`RepoFooter::job`) | `sync_state_notices`, key `PrJob{session, repo}`. It changes only when the job's tone, title, text, buttons or running state change. | Progress (running, info) is quiet. Results, and questions to the user (recheck, conflict), are unread and pop up when the title changes. The job's buttons send `FooterMsg::JobAction`. The footer card stays. |
| `RepoFooter::pr_error` (never shown before) | key `PrInfo{session, repo}`. Removed when the PR status works again. | Warning, quiet |
| `git.sync_status` failure (only logged before) | key `Other("sync:<session>:<repo>")` | Warning, quiet |
| Session delete (one session, or the list's edit mode) | `App::start_delete` / `delete_done`, key `Other("delete:<n>")`. The dialog closes on confirm and the sessions leave the list (`App::deleting`, `in_filter`) while the server stops them and removes worktrees and branches; failed ones come back. | Running info (quiet), then a success toast (quiet) or an error toast (unread) naming each failed session and why |
| Self-update | `self_update.rs` `update_notice`: "ready" with a Restart button, and the result of the run | Info / success / error, toast. The footer line stays. |
| Server connection | `conn_notice`, key `Conn`: lost (reconnecting), disconnected (Reconnect button), back again | Warning / error / success (quiet) |
| Agent activity (`Notify::Activity`) | `activity_notice`, key `Attention{session}`: `NeedsInput` gives "Waiting for you", `Working` to `Idle` gives "The answer is done" | Warning / success, unread, toast. Clicking shows the session. |
| Session process exited by itself | `exit_notice`, key `Attention{session}` for a non-zero code | Error, unread, toast. A clean exit is info, quiet. |
| Terminal bell (`Notify::Bell`, ignored before) | `bell_notice`, key `Bell{session}` | Info, unread, no toast |
| Session web view / browser tab `LoadError` (only logged before) | `load_error_notice`. `ERR_ABORTED` (redirects, new navigations) is skipped. | Warning, unread, no toast |
| Session terminal attach failed | `ClientEvent::AttachFailed` | Warning, quiet |

The session's agent notices (activity, exit, bell) appear only when the user is not looking at that
session. "Looking" means the app window is focused, the work page is shown and that session is
selected (`seeing`). Selecting the session, or focusing the window again with it selected, marks its
`Attention` and `Bell` notices read. While the window is not focused, these notices also flash the
taskbar button (`window::request_user_attention`, `Informational`).

## Not in the centre

- Questions stay modal: every `Modal` (delete, discard, stash drop, sync confirm, close PR, merge,
  recheck, ...).
- Validation errors stay in their dialog (`NewForm::error`, `ChangesModal::error`, ...).
- State shown in place stays there: session row badges (PR conflict, commit needed, ...), the
  activity line, the connection dot, `FileBar`, the footer strips and the self-update footer line.
  Some of these are mirrored as described above.
- Page errors stay on their page (`SettingsForm::error`, `repos_error`, base branch card).

## UI

- **Bell**: the rail's last button, at the bottom. The unread count is a red pill ("9+" over nine).
  While work is running, a turning loader (`icons::spinner`, Tabler `loader-2`, redrawn every
  `SPIN_FRAME`) sits in its lower right corner, beside any unread pill. The tooltip reads
  "Notifications", plus "· N tasks running" while work runs.
- **List**: a panel right of the bell (a layer over the window, `notice_layer`), its bottom level with
  the bell's bottom and growing upwards: 360 px wide, up to 520 px high and at least 8 px below the
  top of the window. A click outside closes it.
  - Header: "Notifications", a running badge, and buttons for mark all read (checks), clear finished
    (trash) and close.
  - Items, newest first: tone icon (a turning loader while running), bold title, time (`HH:MM`), mark read (✓, unread only), remove
    (×, not while running), session chip (and repository), body and action buttons.
  - An unread item has a lighter ground and an accent border. Errors and warnings keep their tone's
    border.
  - Clicking outside closes the list.
- **Toasts**: bottom right, at most three, oldest on top, each with a × button. Clicking a toast does
  what clicking its notice does, or else hides the toast.
  - The detail window shows the toasts of the session it shows.
  - The app window shows all the others.

## Test script

- `Step::WaitToast` reads the newest toast that is up for the phase 2 session's workspace owner
  (`App::toast_text`).
- `Step::ExpectBanner` now looks for the needle in any notice's title or text.
- `notice …` log lines record every notice.

## Open

- Most detail-pane toasts still pass a text formatted when the event happened (`Text::Plain` with
  `tr::*` already applied). The zero-argument ones and the new notices use `Text::tr`. Moving the
  rest means capturing their arguments in the closure.
- The session row's "New answer" still comes from `App::new_answers`. Deriving it from the unread
  `Attention` notice would unify the two.
- There are no OS notifications (Windows toasts). There is also no "pin as a column" mode (Desk's
  `notices-pinned`).
- `NeedsInput` loses the hook's `permission` flag (`bridge.rs`), so "waiting for approval" and
  "waiting for an answer" look the same.
