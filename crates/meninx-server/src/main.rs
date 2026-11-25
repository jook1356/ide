//! `meninx-server --stdio` | `--stdio --no-daemon` | `--daemon [--idle-exit SECS]` | `mcp` |
//! `hook --provider <id> <event>` | `--version`. `mcp` is the agent bridge's MCP server and
//! `hook` its hook program, both run by a session's agent CLI (`docs/agent-bridge.md`).
//! The daemon exits after `--idle-exit` / `MENINX_SERVER_IDLE_EXIT_SECS` seconds (default 600,
//! 0 = never) without clients and running sessions.

use std::sync::Arc;
use std::time::Duration;

use meninx_server::{Server, ServerConfig, daemon, socket_name};

/// Default `--idle-exit` of the daemon.
const DEFAULT_IDLE_EXIT_SECS: u64 = 600;

const USAGE: &str =
    "usage: meninx-server --stdio [--no-daemon] | --daemon [--idle-exit SECS] | mcp | hook --provider ID EVENT | --version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--version") {
        println!("meninx-server {}", meninx_proto::VERSION);
        return;
    }
    if args.first().map(String::as_str) == Some("mcp") {
        // stdout carries MCP messages: no logging, errors to stderr (shown by the agent CLI).
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let code = match rt.block_on(meninx_server::bridge::run_mcp_stdio()) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("meninx-server mcp: {e:#}");
                1
            }
        };
        std::process::exit(code);
    }
    if args.first().map(String::as_str) == Some("hook") {
        // stdout is the agent CLI's hook output: nothing else may be printed.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let code = rt.block_on(meninx_server::bridge::run_hook(&args));
        std::process::exit(code);
    }
    // Daemon idle exit (no clients, no running sessions): default 10 minutes; 0 = never.
    let idle_exit = args
        .iter()
        .position(|a| a == "--idle-exit")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            std::env::var("MENINX_SERVER_IDLE_EXIT_SECS")
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
        // Not 2: agent CLIs read exit code 2 of a hook program as "block".
        std::process::exit(1);
    };

    let mut config = match ServerConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("meninx-server: {e}");
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
                    eprintln!("meninx-server: {e:#}");
                    1
                }
            },
            Mode::Daemon => {
                // Sessions' agents reach this daemon through its socket (the agent bridge).
                config.bridge = std::env::current_exe()
                    .ok()
                    .map(|exe| meninx_server::bridge::BridgeConfig {
                        exe,
                        socket: name.clone(),
                    });
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

/// Logs go to `<data>/logs/meninx-server.log` (stdout carries the protocol). `MENINX_SERVER_LOG`
/// sets the filter (default `info`).
fn init_logging(config: &ServerConfig, mode: Mode) {
    let dir = config.paths.root().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("meninx-server.log"))
    {
        Ok(f) => f,
        Err(_) => return,
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("MENINX_SERVER_LOG")
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
    tracing::debug!("meninx-server {tag} pid {}", std::process::id());
}
