# ide self-update (빌드 후 재시작)

The footer's hammer icon (Hermes Desk's "Desk 빌드 후 재시작", `docs/desk-self-update.md` there)
pulls the repository, builds a new ide **while this one keeps running**, then on the user's click
restarts into it. The server (daemon) and its sessions stay: the new app attaches to it. If the
protocol changed, the user restarts the server (Settings) afterwards.

## Parts

| Part | File | Role |
|---|---|---|
| Script | `scripts/ide-update.ps1` | pull, build, wait for the restart, swap, start, health wait, roll back |
| App | `crates/ide-app/src/self_update.rs` | availability, confirm dialog, start the script, poll the status file, quit on `swapping`, confirm `done` at startup, the footer |

## Availability

Windows; the exe runs from `<target>\release`; the repository it was built from
(`CARGO_MANIFEST_DIR/../..`) has `.git`, `Cargo.toml` and `scripts\ide-update.ps1`.
The script always works in the **original repository**: when that path is a worktree (an ide
session's), it switches to the checkout its common git dir belongs to (`git rev-parse --git-common-dir`).

## Files (`<data>\self-update\`)

`ide-update.ps1` (copied before each run: the pull may rewrite the repository's copy),
`status.json` (UTF-8 without BOM, written tmp + rename), `update.log`, `apply-request`.

## Phases (status.json `phase`)

| Phase | Meaning |
|---|---|
| `pulling` | First, if another worktree on the same branch (an ide session's) moved the branch while this checkout's files still equal an earlier commit of it (nothing unsaved), `git reset --hard HEAD` brings them up. Then `git pull --ff-only`; if it fails (local changes, conflicts, no upstream) the checkout is built as it is. |
| `building` | `ide.exe` / `ide-server.exe` are renamed to `*.<run>.old` (a running exe can be renamed, not overwritten), then `cargo build --release -p ide-app -p ide-server` with the user's `PATH`, `CEF_PATH`, `WGPU_BACKEND` and `CARGO_TARGET_DIR=<target>`. A failed build moves the old files back (`failed`). |
| `ready` | Built; the footer shows a purple restart icon. Clicking writes the run id to `apply-request`. If the app closes first: `done` with message `closed-before-restart` (the next start uses the new build). |
| `swapping` | The app that applied quits; the script waits for its pid (killed after 60 s) and starts the new `ide.exe`. |
| `done` | Written by the new app at startup (`confirm_started`). |
| `rolledBack` | No `done` within 120 s or the new app exited: the new files become `*.<run>.failed`, the old ones come back, the old app starts. |
| `failed` | A step before `ready` failed; the running app is unchanged. |

`*.old` / `*.failed` files are removed at the next run (a file still in use, such as the old
server copy a running daemon runs from, is kept).

The script is started with `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`
(like the daemon) so it outlives the app.

The footer shows each finished run's result once (click hides it; remembered as `update_seen`).
