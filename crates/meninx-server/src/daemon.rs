//! Per-user daemon: local socket listener (Windows named pipe / Unix socket), `--stdio` relay
//! to it, and detached daemon start.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::service::Server;

/// A connected local socket.
pub trait LocalIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> LocalIo for T {}

/// `<data>/meninx-server.pid` (written by the daemon while it runs).
pub fn pid_file(server: &Server) -> PathBuf {
    server.paths().root().join("meninx-server.pid")
}

/// Listen on `name` and serve clients until idle for `idle_exit` (no connections and no
/// running sessions), if given. Fails if another daemon already listens on `name`.
pub async fn run_daemon(
    server: Arc<Server>,
    name: &str,
    idle_exit: Option<Duration>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(server.paths().root()).ok();
    let pid = pid_file(&server);
    let mut idle_since = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut listener = Listener::bind(name).await?;
    std::fs::write(&pid, std::process::id().to_string()).ok();
    tracing::info!("daemon listening on {name} (pid {})", std::process::id());
    loop {
        tokio::select! {
            conn = listener.accept() => {
                let io = conn?;
                let server = server.clone();
                let user = server.user().clone();
                tokio::spawn(async move { server.serve(io, user).await });
            }
            _ = tick.tick() => {
                if server.connections() > 0 || server.running_sessions() > 0 {
                    idle_since = Instant::now();
                } else if let Some(limit) = idle_exit && idle_since.elapsed() >= limit {
                    tracing::info!("idle for {limit:?}; exiting");
                    break;
                }
            }
            _ = server.shutdown_requested() => {
                tracing::info!("shutdown requested; exiting");
                // Let the reply reach the client before the process exits.
                tokio::time::sleep(Duration::from_millis(300)).await;
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&pid);
    Ok(())
}

#[cfg(windows)]
struct Listener {
    name: String,
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(windows)]
impl Listener {
    async fn bind(name: &str) -> anyhow::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let next = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(name)
            .with_context(|| format!("create pipe {name} (is a daemon already running?)"))?;
        Ok(Listener {
            name: name.to_owned(),
            next,
        })
    }

    async fn accept(&mut self) -> anyhow::Result<Box<dyn LocalIo>> {
        use tokio::net::windows::named_pipe::ServerOptions;
        self.next.connect().await?;
        let fresh = ServerOptions::new()
            .reject_remote_clients(true)
            .create(&self.name)?;
        Ok(Box::new(std::mem::replace(&mut self.next, fresh)))
    }
}

#[cfg(unix)]
struct Listener(tokio::net::UnixListener);

#[cfg(unix)]
impl Listener {
    async fn bind(name: &str) -> anyhow::Result<Self> {
        if tokio::net::UnixStream::connect(name).await.is_ok() {
            anyhow::bail!("a daemon already listens on {name}");
        }
        let _ = std::fs::remove_file(name);
        if let Some(dir) = std::path::Path::new(name).parent() {
            std::fs::create_dir_all(dir)?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(Listener(tokio::net::UnixListener::bind(name)?))
    }

    async fn accept(&mut self) -> anyhow::Result<Box<dyn LocalIo>> {
        Ok(Box::new(self.0.accept().await?.0))
    }
}

/// Connect to a daemon's local socket.
pub async fn connect(name: &str) -> std::io::Result<Box<dyn LocalIo>> {
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        const ERROR_PIPE_BUSY: i32 = 231;
        let t = Instant::now();
        loop {
            match ClientOptions::new().open(name) {
                Ok(c) => return Ok(Box::new(c)),
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        && t.elapsed() < Duration::from_secs(5) =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(e) => return Err(e),
            }
        }
    }
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(name).await?))
    }
}

/// Start `exe --daemon` detached from this process (own process group, no console window,
/// outside this process' job when allowed). The environment (e.g. `MENINX_DATA_DIR`) is inherited.
pub fn spawn_daemon(exe: &std::path::Path) -> std::io::Result<u32> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        match cmd.spawn() {
            Ok(c) => return Ok(c.id()),
            Err(_) => {
                // The job does not allow breakaway: start inside it.
                cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|c| c.id())
}

/// `--stdio` relay: connect to the per-user daemon (starting it if needed) and copy
/// stdin -> socket and socket -> stdout until either side closes.
pub async fn relay_stdio(name: &str) -> anyhow::Result<()> {
    let io = match connect(name).await {
        Ok(io) => io,
        Err(_) => {
            let exe = std::env::current_exe().context("current exe")?;
            let pid = spawn_daemon(&exe).context("start daemon")?;
            tracing::info!("started daemon pid {pid}");
            let t = Instant::now();
            loop {
                match connect(name).await {
                    Ok(io) => break io,
                    Err(e) if t.elapsed() > Duration::from_secs(15) => {
                        return Err(e).context(format!("connect to daemon at {name}"));
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        }
    };
    relay(tokio::io::stdin(), tokio::io::stdout(), io).await;
    Ok(())
}

async fn relay<R, W, S>(mut input: R, mut output: W, sock: S)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sr, mut sw) = tokio::io::split(sock);
    let up = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match input.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sw.write_all(&buf[..n]).await.is_err() || sw.flush().await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    let down = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match sr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output.write_all(&buf[..n]).await.is_err() || output.flush().await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}
