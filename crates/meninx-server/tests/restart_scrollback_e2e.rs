//! A session's terminal output survives a server restart: the new server replays the old
//! output (from `sessions/<id>/output.log`) so it lands in the client's scrollback, and the
//! restarted process draws on a clean screen below it. A shell stands in for the agent CLI.
//! Run with `MENINX_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use common::*;
use meninx_client::{
    Client, ClientOptions, LocalChild, NewSession, SessionInfo, SessionState, Transport,
};
use meninx_core::Paths;

const T: Duration = Duration::from_secs(30);
const COLS: u16 = 100;
const ROWS: u16 = 30;

async fn connect(transport: impl Transport) -> Client {
    Client::connect(
        transport,
        ClientOptions {
            reconnect: false,
            ..ClientOptions::default()
        },
    )
    .await
    .expect("connect")
}

/// What a client terminal (alacritty, as `meninx-term`) shows after `bytes`: scrollback rows and
/// screen rows, trailing blanks trimmed.
fn render(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let config = Config {
        scrolling_history: 10_000,
        ..Config::default()
    };
    let mut term = Term::new(config, &TermSize::new(COLS as usize, ROWS as usize), VoidListener);
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, bytes);
    let grid = term.grid();
    let row = |r: i32| {
        let s: String = (0..grid.columns()).map(|c| grid[Line(r)][Column(c)].c).collect();
        s.trim_end().to_owned()
    };
    let history = grid.history_size() as i32;
    (
        (-history..0).map(row).collect(),
        (0..grid.screen_lines() as i32).map(row).collect(),
    )
}

fn print_lines(prefix: &str, n: u32) -> Vec<u8> {
    if cfg!(windows) {
        format!("for /L %i in (1,1,{n}) do @echo {prefix}-%i\r").into_bytes()
    } else {
        format!("i=1; while [ $i -le {n} ]; do echo {prefix}-$i; i=$((i+1)); done\r").into_bytes()
    }
}

fn log_has(path: &Path, needle: &str) -> bool {
    std::fs::read(path).is_ok_and(|b| String::from_utf8_lossy(&b).contains(needle))
}

/// A running shell session that printed 80 lines (logged to `sessions/<id>/output.log`).
async fn session_with_output(c: &Client, repo: &Path, data: &Path) -> (SessionInfo, PathBuf) {
    let repo_id = c
        .register_repo(&repo.display().to_string())
        .await
        .unwrap()
        .id;
    let (s, _) = c
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: vec![repo_id],
            original_repo: None,
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: "restart".into(),
            start: true,
            cols: COLS,
            rows: ROWS,
        })
        .await
        .expect("create session");
    assert_eq!(s.state, SessionState::Running);
    let log = Paths::new(data).session_output_log(&s.id);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    c.write(&s.id, print_lines("old-line", 80)).await.unwrap();
    assert!(
        wait_until(T, || log_has(&log, "old-line-80")).await,
        "output log written: {:?}",
        std::fs::read(&log).map(|b| String::from_utf8_lossy(&b).into_owned())
    );
    (s, log)
}

/// On the restarted server: the stopped session replays its old output; started again (as a
/// resumed agent would be), its new output lands on a clean screen below the old output, which
/// stays in the client's scrollback. Ends with the session deleted (log removed).
async fn check_after_restart(c: &Client, s: &SessionInfo, log: &Path) {
    let col = collect(c, Arc::new(AtomicU64::new(0)));
    let a = c.attach(&s.id, None).await.expect("attach");
    assert!(a.reset);
    assert!(
        wait_until(T, || term(&col, &s.id).text().contains("old-line-80")).await,
        "old output replayed: {:?}",
        term(&col, &s.id).text()
    );
    let started = c.start_session(&s.id).await.expect("start");
    assert_eq!(started.state, SessionState::Running);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    c.write(&s.id, b"echo new-output\r".to_vec()).await.unwrap();
    assert!(
        wait_until(T, || term(&col, &s.id).count("new-output") >= 2).await,
        "{:?}",
        term(&col, &s.id).text()
    );
    let bytes = term(&col, &s.id).data;
    assert!(col.lock().unwrap().errors.is_empty(), "{:?}", col.lock().unwrap().errors);
    let (history, screen) = render(&bytes);
    let pos = |rows: &[String], needle: &str| rows.iter().position(|r| r == needle);
    let first = pos(&history, "old-line-1").expect("old-line-1 in the scrollback");
    let last = pos(&history, "old-line-80").expect("old-line-80 in the scrollback");
    assert!(first < last);
    let rule = history
        .iter()
        .position(|r| r.starts_with("────"))
        .expect("a rule after the old output");
    assert!(rule > last, "rule below the old output");
    assert!(
        screen.iter().all(|r| !r.contains("old-line")),
        "the new process starts on a clean screen: {screen:#?}"
    );
    assert!(
        screen.iter().any(|r| r == "new-output"),
        "new output on screen: {screen:#?}"
    );
    // The log keeps both for the next restart; deleting the session removes it.
    assert!(wait_until(T, || log_has(log, "new-output")).await);
    assert!(log_has(log, "old-line-1"));
    c.stop_session(&s.id).await.unwrap();
    c.delete_session(&s.id, true).await.unwrap();
    assert!(!log.exists(), "log removed with the session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrollback_survives_a_server_restart() {
    let dir = tmp_dir("restart-scrollback");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");

    let server1 = server(&data);
    let c1 = connect(memory(&server1)).await;
    let (s, log) = session_with_output(&c1, &repo, &data).await;
    drop(c1);
    let s1 = server1.clone();
    tokio::task::spawn_blocking(move || s1.stop_all()).await.unwrap();
    drop(server1);

    // A new server on the same data dir.
    let server2 = server(&data);
    let c2 = connect(memory(&server2)).await;
    check_after_restart(&c2, &s, &log).await;
    drop(c2);
    let s2 = server2.clone();
    tokio::task::spawn_blocking(move || s2.stop_all()).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same with a real `meninx-server` daemon that is killed (no graceful stop): the log is
/// written as output arrives, so nothing depends on a clean shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrollback_survives_a_killed_daemon() {
    let dir = tmp_dir("restart-daemon");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let bin = env!("CARGO_BIN_EXE_meninx-server");
    let local = || {
        LocalChild::new(bin)
            .env("MENINX_DATA_DIR", data.display().to_string())
            .env("MENINX_SERVER_IDLE_EXIT_SECS", "60")
    };
    let pid_file = data.join("meninx-server.pid");
    let daemon_pid = || -> u32 {
        std::fs::read_to_string(&pid_file)
            .expect("daemon pid file")
            .trim()
            .parse()
            .unwrap()
    };

    let c1 = connect(local()).await;
    let (s, log) = session_with_output(&c1, &repo, &data).await;
    let pid = daemon_pid();
    drop(c1);
    // Only this test's daemon, by its pid.
    kill_pid(pid);
    assert!(wait_until(T, || !pid_alive(pid)).await, "daemon {pid} killed");

    let c2 = connect(local()).await;
    let pid2 = daemon_pid();
    assert_ne!(pid2, pid, "a new daemon");
    check_after_restart(&c2, &s, &log).await;
    c2.shutdown_server().await.unwrap();
    drop(c2);
    if !wait_until(Duration::from_secs(10), || !pid_file.exists()).await {
        kill_pid(pid2);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .output();
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
}

fn pid_alive(pid: u32) -> bool {
    let pid = sysinfo::Pid::from_u32(pid);
    let Ok(mut sys) = sysinfo::System::new() else {
        return false;
    };
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some()
}
