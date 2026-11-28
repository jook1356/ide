# Meninx self-update (빌드 후 재시작)

The footer's hammer icon (Hermes Desk's "Desk 빌드 후 재시작", `docs/desk-self-update.md` there)
pulls the repository, builds a new Meninx **while this one keeps running**, then on the user's click
restarts into it. The server (daemon) and its sessions stay: the new app attaches to it. If the
protocol changed, the user restarts the server (Settings) afterwards.

## Parts

| Part | File | Role |
|---|---|---|
| Script | `scripts/meninx-update.ps1` | pull, build, wait for the restart, swap, start, health wait, roll back |
| App | `crates/meninx-app/src/self_update.rs` | availability, confirm dialog, start the script, poll the status file, quit on `swapping`, confirm `done` at startup, the footer |

## Availability

Windows; the exe runs from `<target>\release`; the repository it was built from
(`CARGO_MANIFEST_DIR/../..`) has `.git`, `Cargo.toml` and `scripts\meninx-update.ps1`.

## What is built

Always the **original repository's branch as merged**, never the files of a checkout: other
worktrees (Meninx sessions) share the original checkout's branch, so its files can lag behind it.

- Repository: the original one, found from the build path's common git dir
  (`git rev-parse --git-common-dir`), also when the binary was built in a worktree.
- Commit: after `git fetch origin`, `origin/<branch>` (the original checkout's branch, `main`),
  or the local branch when it is ahead of it (merged but not pushed). Without a remote: the local branch.
- Build checkout: `<target>-src` (e.g. `C:\t\meninx-src`), a detached worktree of the original
  repository set to that commit (`checkout --force --detach` + `clean -fd`); the original checkout
  is never touched. The last built commit is kept in `<data>\self-update\built-commit` (`from` of
  the next run).

## Files (`<data>\self-update\`)

`meninx-update.ps1` (copied before each run: the pull may rewrite the repository's copy),
`status.json` (UTF-8 without BOM, written tmp + rename), `update.log`, `apply-request`.

## Phases (status.json `phase`)

| Phase | Meaning |
|---|---|
| `pulling` | `git fetch origin`, pick the commit, set the build checkout to it (see above). A failed fetch builds the local branch. |
| `building` | `meninx.exe` / `meninx-server.exe` and the `remote` folder (the Linux servers, so they always match the app) are renamed to `*.<run>.old` (a running exe can be renamed, not overwritten), then `cargo app` (`crates/xtask`: the app, its server and the Linux servers for SSH installs, `docs/remote-install.md`; those are left out with a warning when zig is missing) in the build checkout with the user's `PATH`, `CEF_PATH`, `WGPU_BACKEND` and `CARGO_TARGET_DIR=<target>`. A failed build removes what it left and moves the old files back (`failed`). |
| `ready` | Built; the footer shows a purple restart icon. Clicking writes the run id to `apply-request`. If the app closes first: `done` with message `closed-before-restart` (the next start uses the new build). |
| `swapping` | The app that applied quits; the script waits for its pid (killed after 60 s) and starts the new `meninx.exe`. |
| `done` | Written by the new app at startup (`confirm_started`). |
| `rolledBack` | No `done` within 120 s or the new app exited: the new files (and `remote`) become `*.<run>.failed`, the old ones come back, the old app starts. |
| `failed` | A step before `ready` failed; the running app is unchanged. |

`*.old` / `*.failed` files are removed at the next run (a file still in use, such as the old
server copy a running daemon runs from, is kept).

The script is started with `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`
(like the daemon) so it outlives the app.

The footer shows each finished run's result once (click hides it; remembered as `update_seen`).
