//! How a client reaches a server: anything yielding an ordered byte stream.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncRead, AsyncWrite};

/// A bidirectional byte stream.
pub trait AsyncIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> AsyncIo for T {}

/// An open transport connection. A child process, if any, is killed when this is dropped.
pub struct Connection {
    /// The byte stream.
    pub io: Box<dyn AsyncIo>,
    /// Process backing the stream (`meninx-server --stdio`, `ssh ...`).
    pub child: Option<tokio::process::Child>,
}

impl Connection {
    /// A connection without a child process.
    pub fn new(io: impl AsyncIo) -> Self {
        Connection {
            io: Box::new(io),
            child: None,
        }
    }
}

/// Boxed connect future.
pub type ConnectFuture = Pin<Box<dyn Future<Output = io::Result<Connection>> + Send>>;

/// Opens connections to a server. Called again for every reconnect.
pub trait Transport: Send + Sync + 'static {
    /// Open a new connection.
    fn connect(&self) -> ConnectFuture;
    /// For logs/UI, e.g. `local` or `ssh devbox`.
    fn describe(&self) -> String;
}

impl<T: Transport + ?Sized> Transport for Arc<T> {
    fn connect(&self) -> ConnectFuture {
        (**self).connect()
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}

/// Environment variable naming the `meninx-server` binary.
pub const SERVER_BIN_ENV: &str = "MENINX_SERVER_BIN";

/// Runs `meninx-server --stdio` as a child process and talks over its stdin/stdout.
#[derive(Debug, Clone)]
pub struct LocalChild {
    /// Server binary.
    pub program: PathBuf,
    /// Arguments (default `--stdio`).
    pub args: Vec<String>,
    /// Extra environment for the child (e.g. `MENINX_DATA_DIR`).
    pub env: Vec<(String, String)>,
}

impl LocalChild {
    /// `program --stdio`.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        LocalChild {
            program: program.into(),
            args: vec!["--stdio".into()],
            env: Vec::new(),
        }
    }

    /// The server binary from `MENINX_SERVER_BIN`, else `meninx-server[.exe]` next to the current
    /// executable.
    pub fn find() -> io::Result<Self> {
        if let Some(p) = std::env::var_os(SERVER_BIN_ENV).filter(|p| !p.is_empty()) {
            return Ok(Self::new(PathBuf::from(p)));
        }
        let exe = std::env::current_exe()?;
        let name = format!("meninx-server{}", std::env::consts::EXE_SUFFIX);
        let candidate = exe.with_file_name(&name);
        if candidate.is_file() {
            return Ok(Self::new(candidate));
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{name} not found next to {} (set {SERVER_BIN_ENV})",
                exe.display()
            ),
        ))
    }

    /// Serve directly on stdio without the per-user daemon (`--no-daemon`): sessions end
    /// with the connection.
    pub fn no_daemon(mut self) -> Self {
        self.args.push("--no-daemon".into());
        self
    }

    /// Add an environment variable for the child.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

fn spawn_stdio(mut cmd: tokio::process::Command) -> io::Result<Connection> {
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    Ok(Connection {
        io: Box::new(tokio::io::join(stdout, stdin)),
        child: Some(child),
    })
}

impl Transport for LocalChild {
    fn connect(&self) -> ConnectFuture {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        Box::pin(async move { spawn_stdio(cmd) })
    }

    fn describe(&self) -> String {
        "local".into()
    }
}

/// Runs `ssh <host> meninx-server --stdio` with the system OpenSSH client. Authentication is
/// SSH's (keys/agent); `BatchMode` stops it from prompting, since there is no terminal.
#[derive(Debug, Clone)]
pub struct Ssh {
    /// `host` or `user@host` (or an alias from `~/.ssh/config`).
    pub host: String,
    /// Remote command (default `meninx-server --stdio`).
    pub server_command: String,
    /// The ssh binary (default `ssh`).
    pub ssh_program: PathBuf,
    /// Extra ssh options placed before the host (e.g. `-p 2222`).
    pub extra_args: Vec<String>,
}

impl Ssh {
    /// Defaults for `host`.
    pub fn new(host: impl Into<String>) -> Self {
        Ssh {
            host: host.into(),
            server_command: "meninx-server --stdio".into(),
            ssh_program: PathBuf::from("ssh"),
            extra_args: Vec::new(),
        }
    }

    /// The full argument list passed to ssh.
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![
            "-T".to_owned(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
        ];
        a.extend(self.extra_args.iter().cloned());
        a.push("--".into());
        a.push(self.host.clone());
        a.push(self.server_command.clone());
        a
    }
}

impl Transport for Ssh {
    fn connect(&self) -> ConnectFuture {
        let mut cmd = tokio::process::Command::new(&self.ssh_program);
        cmd.args(self.args());
        Box::pin(async move { spawn_stdio(cmd) })
    }

    fn describe(&self) -> String {
        format!("ssh {}", self.host)
    }
}

type Factory = dyn Fn() -> io::Result<Box<dyn AsyncIo>> + Send + Sync;

/// In-process transport (tests, embedded servers): `factory` returns the client end of a new
/// connection, typically one side of [`meninx_proto::memory_pair`] whose other side it hands to
/// `Server::serve`.
#[derive(Clone)]
pub struct Memory {
    factory: Arc<Factory>,
}

impl Memory {
    /// A memory transport.
    pub fn new(factory: impl Fn() -> io::Result<Box<dyn AsyncIo>> + Send + Sync + 'static) -> Self {
        Memory {
            factory: Arc::new(factory),
        }
    }
}

impl Transport for Memory {
    fn connect(&self) -> ConnectFuture {
        let r = (self.factory)().map(|io| Connection { io, child: None });
        Box::pin(async move { r })
    }

    fn describe(&self) -> String {
        "memory".into()
    }
}

/// Wraps a transport with an on/off switch: while offline, `connect` fails (simulates a
/// network outage together with [`crate::Client::reconnect`]).
#[derive(Clone)]
pub struct Switchable<T> {
    inner: T,
    online: Arc<AtomicBool>,
}

impl<T: Transport> Switchable<T> {
    /// Online at first.
    pub fn new(inner: T) -> Self {
        Switchable {
            inner,
            online: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Toggle.
    pub fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::Release);
    }
}

impl<T: Transport> Transport for Switchable<T> {
    fn connect(&self) -> ConnectFuture {
        if self.online.load(Ordering::Acquire) {
            self.inner.connect()
        } else {
            Box::pin(async {
                Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "transport offline",
                ))
            })
        }
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_args() {
        let mut s = Ssh::new("me@devbox");
        s.extra_args = vec!["-p".into(), "2222".into()];
        assert_eq!(
            s.args(),
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ServerAliveInterval=15",
                "-p",
                "2222",
                "--",
                "me@devbox",
                "meninx-server --stdio"
            ]
        );
        assert_eq!(s.describe(), "ssh me@devbox");
    }
}
