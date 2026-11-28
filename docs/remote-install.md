# Installing the server on an SSH host

Status: implemented (2026-11-28; checked against sshd in WSL Ubuntu 22.04, x86_64: app script `MENINX_APP_SCRIPT_ONLY=servers` with `MENINX_APP_TEST_SSH`). Code: `meninx-client/src/remote_install.rs` (probe, install,
restart, POSIX `cksum`), `meninx-app/src/server_menu.rs` (the server dialog's check and
설치 / 업데이트), `crates/xtask` (the bundled builds). See
`docs/multi-server.md` for the server list.

## Builds the app ships

- Static Linux builds of `meninx-server` (musl, x86_64 and aarch64; profile `remote`: no
  debug info, stripped) in `<app folder>/remote/linux-<arch>/meninx-server`, cross-built on
  Windows with `cargo zigbuild` (zig on `PATH` or `<toolchain>/zig/zig.exe` beside
  `CARGO_HOME`, and `cargo install cargo-zigbuild`) by `cargo app` / `cargo app-debug`
  (`crates/xtask`), so the self-update builds them too; without zig they are left out with a
  warning (the dialog then says there is no build).
- The app and these builds come from the same commit, so an install always speaks the app's
  protocol. `MENINX_REMOTE_SERVERS=<dir>` points the app at another folder (tests).

## On the host

- Installed to `~/.meninx/bin/meninx-server` (no root, no `PATH` change). The default SSH
  command (`meninx_client::transport::DEFAULT_SSH_COMMAND`, used when an entry has no server
  command) runs it through `sh`, else `meninx-server` on `PATH`.
- Everything runs over the entry's own `ssh` (`Ssh::command_args`: same options, `BatchMode`;
  `MENINX_SSH_CONFIG=<file>` adds `-F <file>`), as `sh -c '...'` so the login shell does not
  matter:
  - **probe**: `uname -sm`; the installed server's `--version` and `cksum` (POSIX, present
    everywhere), or `none`.
  - **install**: the build on stdin -> `meninx-server.new`, `chmod 755`, `mv -f` over the old
    one (a running daemon keeps its file), `--version`.
  - **restart**: `kill` the pid in `${XDG_DATA_HOME:-~/.local/share}/meninx/meninx-server.pid`
    (the daemon's default data dir); the next connection starts the installed server.

## The dialog's check (연결 테스트)

Probe, then a throwaway connection:

| Found | Shown | Action |
|---|---|---|
| connected, same build (or nothing to compare with) | 연결됨 · server version | — |
| nothing installed, nothing answered | 이 서버에 Meninx 서버가 없어요 | 설치 (when the app has a build for the platform; else "no build for <platform>") |
| another build, connects | 다른 빌드가 설치돼 있어요 | 업데이트: replaces the file; the running daemon keeps going until it is idle |
| another build, `proto-version` | 이 앱과 맞지 않아요 | 업데이트: replaces the file and restarts the daemon (its sessions stop) |
| `ssh` failed (exit 255) | ssh's own message (host, refused, authentication) | — |

An entry with its own server command is the user's: only the connection is checked.
Install / update run the check again at the end.

## Not done

- macOS hosts (no build); Windows hosts over SSH.
- Agent CLIs on the host (`claude`, `codex`, `git`): not checked yet.
