//! `ide-server --stdio` | `--stdio --no-daemon` | `--daemon [--idle-exit SECS]` | `--version`.
//! The daemon exits after `--idle-exit` / `IDE_SERVER_IDLE_EXIT_SECS` seconds (default 600,
//! 0 = never) without clients and running sessions.

use std::sync::Arc;
use std::time::Duration;

use ide_server::{Server, ServerConfig, daemon, socket_name};

/// Default `--idle-exit` of the daemon.
const DEFAULT_IDLE_EXIT_SECS: u64 = 600;

const USAGE: &str =
    "usage: ide-server --stdio [--no-daemon] | --daemon [--idle-exit SECS] | --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--version") {
        println!("ide-server {}", ide_proto::VERSION);
        return;
    }
    // Daemon idle exit (no clients, no running sessions): default 10 minutes; 0 = never.
    let idle_exit = args
        .iter()
        .position(|a| a == "--idle-exit")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            std::env::var("IDE_SERVER_IDLE_EXIT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(DEFAULT_IDLE_EXIT_SECS);
    let idle_exit = (idle_exit > 0).then(|| Duration::from_secs(idle_exit));
    let mode = if has("--daemon") {
        Mode::Daemon
    } else if has("--stdio") && has("--no-daemon") {
        Mode::Direct
    } else if has("--stdio") {
        Mode::Relay
    } else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };

    let config = match ServerConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ide-server: {e}");
            std::process::exit(1);
        }
    };
    init_logging(&config, mode);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = rt.block_on(async move {
        let name = socket_name(&config.paths, &config.user);
        match mode {
            Mode::Relay => match daemon::relay_stdio(&name).await {
                Ok(()) => 0,
                Err(e) => {
                    tracing::error!("relay: {e:#}");
                    eprintln!("ide-server: {e:#}");
                    1
                }
            },
            Mode::Daemon => {
                let server = Server::new(config);
                let r = daemon::run_daemon(server.clone(), &name, idle_exit).await;
                server.kill_all();
                match r {
                    Ok(()) => 0,
                    Err(e) => {
                        tracing::error!("daemon: {e:#}");
                        1
                    }
                }
            }
            Mode::Direct => {
                let server: Arc<Server> = Server::new(config);
                let io = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
                let user = server.user().clone();
                tokio::select! {
                    _ = server.serve(io, user) => {}
                    _ = server.shutdown_requested() => {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                }
                server.kill_all();
                0
            }
        }
    });
    // tokio's stdin reader thread may still block on a read: exit without waiting for it.
    std::process::exit(code);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Relay,
    Daemon,
    Direct,
}

/// Logs go to `<data>/logs/ide-server.log` (stdout carries the protocol). `IDE_SERVER_LOG`
/// sets the filter (default `info`).
fn init_logging(config: &ServerConfig, mode: Mode) {
    let dir = config.paths.root().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("ide-server.log"))
    {
        Ok(f) => f,
        Err(_) => return,
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("IDE_SERVER_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let tag = match mode {
        Mode::Relay => "relay",
        Mode::Daemon => "daemon",
        Mode::Direct => "direct",
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
    tracing::debug!("ide-server {tag} pid {}", std::process::id());
}
