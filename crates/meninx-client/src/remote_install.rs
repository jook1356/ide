//! Check and install `meninx-server` on an SSH server (`docs/remote-install.md`). The app ships
//! static Linux builds; the right one is copied to `~/.meninx/bin/meninx-server` over the same
//! `ssh` the connection uses ([`Ssh::command_args`]), which [`crate::transport::DEFAULT_SSH_COMMAND`]
//! then runs.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;

use crate::transport::Ssh;

/// What [`probe`] found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// `linux-x86_64`, `linux-aarch64`, or `uname -sm` lowercased and joined with `-` for
    /// systems without a build (e.g. `darwin-arm64`).
    pub platform: String,
    /// The server in `~/.meninx/bin`, if one is there.
    pub installed: Option<Installed>,
}

/// An installed server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Its `--version` line (`meninx-server 0.1.0`).
    pub version: String,
    /// POSIX `cksum` of the file: CRC and size (compare with [`cksum`] of the bundled one).
    pub cksum: (u32, u64),
}

/// Why a remote command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// `ssh` itself failed (exit 255: host unknown, refused, authentication, ...): its message.
    Ssh(String),
    /// The command ran and failed (exit code, stderr).
    Remote(i32, String),
    /// No answer in time.
    Timeout,
    /// `ssh` could not be started.
    Spawn(String),
    /// The output was not understood.
    Output(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Ssh(m) => write!(f, "{m}"),
            Failure::Remote(code, m) if m.is_empty() => write!(f, "exit {code}"),
            Failure::Remote(code, m) => write!(f, "{m} (exit {code})"),
            Failure::Timeout => write!(f, "timeout"),
            Failure::Spawn(m) => write!(f, "ssh: {m}"),
            Failure::Output(m) => write!(f, "unexpected output: {m}"),
        }
    }
}

const PROBE: &str = "sh -c 'uname -sm; b=\"$HOME/.meninx/bin/meninx-server\"; \
                     if [ -x \"$b\" ]; then \"$b\" --version; cksum < \"$b\"; else echo none; fi'";

const INSTALL: &str = "sh -c 'set -e; d=\"$HOME/.meninx/bin\"; mkdir -p \"$d\"; \
                       cat > \"$d/meninx-server.new\"; chmod 755 \"$d/meninx-server.new\"; \
                       mv -f \"$d/meninx-server.new\" \"$d/meninx-server\"; \"$d/meninx-server\" --version'";

/// Ends the running daemon (its pid file in the default data dir), so the next connection
/// starts the installed server. Its sessions stop.
const RESTART: &str = "sh -c 'p=\"${XDG_DATA_HOME:-$HOME/.local/share}/meninx/meninx-server.pid\"; \
                       if [ -f \"$p\" ]; then kill \"$(cat \"$p\")\" 2>/dev/null || true; fi'";

/// The platform, and the installed server if any.
pub async fn probe(ssh: &Ssh) -> Result<Probe, Failure> {
    let out = run(ssh, PROBE, None, Duration::from_secs(30)).await?;
    parse_probe(&out)
}

/// Copy `binary` to `~/.meninx/bin/meninx-server` (replacing one there; a running daemon
/// keeps its old file) -> the new server's `--version` line.
pub async fn install(ssh: &Ssh, binary: Vec<u8>) -> Result<String, Failure> {
    let out = run(ssh, INSTALL, Some(binary), Duration::from_secs(600)).await?;
    Ok(out.trim().to_owned())
}

/// End the running daemon (see [`RESTART`]).
pub async fn restart(ssh: &Ssh) -> Result<(), Failure> {
    run(ssh, RESTART, None, Duration::from_secs(30)).await.map(drop)
}

fn parse_probe(out: &str) -> Result<Probe, Failure> {
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let uname = lines.next().ok_or_else(|| Failure::Output(out.into()))?;
    let platform = platform(uname);
    let installed = match lines.next() {
        Some("none") | None => None,
        Some(version) => {
            let ck = lines.next().ok_or_else(|| Failure::Output(out.into()))?;
            let mut it = ck.split_whitespace();
            let crc = it.next().and_then(|v| v.parse().ok());
            let size = it.next().and_then(|v| v.parse().ok());
            let (Some(crc), Some(size)) = (crc, size) else {
                return Err(Failure::Output(out.into()));
            };
            Some(Installed {
                version: version.to_owned(),
                cksum: (crc, size),
            })
        }
    };
    Ok(Probe { platform, installed })
}

/// `uname -sm` -> the name of the app's bundled build folder.
fn platform(uname: &str) -> String {
    let mut it = uname.split_whitespace();
    let os = it.next().unwrap_or_default().to_ascii_lowercase();
    let arch = match it.next().unwrap_or_default() {
        "x86_64" | "amd64" => "x86_64".to_owned(),
        "aarch64" | "arm64" => "aarch64".to_owned(),
        other => other.to_ascii_lowercase(),
    };
    format!("{os}-{arch}")
}

/// Run `remote` on the host with `input` on its stdin -> its stdout.
async fn run(ssh: &Ssh, remote: &str, input: Option<Vec<u8>>, limit: Duration) -> Result<String, Failure> {
    let mut cmd = tokio::process::Command::new(&ssh.ssh_program);
    cmd.args(ssh.command_args(remote))
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| Failure::Spawn(e.to_string()))?;
    let feed = match (input, child.stdin.take()) {
        (Some(bytes), Some(mut stdin)) => Some(tokio::spawn(async move {
            let r = stdin.write_all(&bytes).await;
            drop(stdin);
            r
        })),
        _ => None,
    };
    let out = tokio::time::timeout(limit, child.wait_with_output())
        .await
        .map_err(|_| Failure::Timeout)?
        .map_err(|e| Failure::Spawn(e.to_string()))?;
    if let Some(f) = feed {
        let _ = f.await;
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    match out.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Some(255) => Err(Failure::Ssh(if stderr.is_empty() { "ssh failed".into() } else { stderr })),
        Some(code) => Err(Failure::Remote(code, stderr)),
        None => Err(Failure::Remote(-1, stderr)),
    }
}

/// POSIX `cksum` of `data`: CRC-32 (polynomial 0x04C11DB7, the length appended) and the size.
pub fn cksum(data: &[u8]) -> (u32, u64) {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = (i as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            }
            *e = c;
        }
        t
    });
    let step = |crc: u32, b: u8| (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize];
    let mut crc = data.iter().fold(0u32, |c, &b| step(c, b));
    let mut n = data.len() as u64;
    while n != 0 {
        crc = step(crc, (n & 0xff) as u8);
        n >>= 8;
    }
    (!crc, data.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_cksum() {
        // `printf 123456789 | cksum` and `printf '' | cksum`.
        assert_eq!(cksum(b"123456789"), (930_766_865, 9));
        assert_eq!(cksum(b""), (4_294_967_295, 0));
    }

    #[test]
    fn probe_output() {
        assert_eq!(
            parse_probe("Linux x86_64\nnone\n").unwrap(),
            Probe {
                platform: "linux-x86_64".into(),
                installed: None
            }
        );
        assert_eq!(
            parse_probe("Linux aarch64\nmeninx-server 0.1.0\n123 456\n").unwrap(),
            Probe {
                platform: "linux-aarch64".into(),
                installed: Some(Installed {
                    version: "meninx-server 0.1.0".into(),
                    cksum: (123, 456)
                })
            }
        );
        assert_eq!(parse_probe("Darwin arm64\nnone").unwrap().platform, "darwin-aarch64");
        assert!(parse_probe("").is_err());
        assert!(parse_probe("Linux x86_64\nmeninx-server 0.1.0\n").is_err());
    }
}
