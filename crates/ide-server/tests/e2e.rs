//! Server + client end to end: in-memory transport and real `ide-server` child processes.
//! Run with `IDE_TEST_TMP=<dir inside the workspace>`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::*;
use ide_client::{
    Client, ClientOptions, LocalChild, NewSession, SessionInfo, SessionState, Switchable, Transport,
};
use ide_proto::{
    Hello, Mux, MuxConfig, PROTO_VERSION, Request, Response, Role, RpcMessage, UserInfo,
};

const T: Duration = Duration::from_secs(30);

fn opts() -> ClientOptions {
    ClientOptions {
        backoff_min: Duration::from_millis(50),
        backoff_max: Duration::from_millis(500),
        ..ClientOptions::default()
    }
}

fn delay() -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(0))
}

async fn session_info(c: &Client, id: &str) -> SessionInfo {
    c.sessions()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .expect("session listed")
}

struct Scenario {
    c1: Client,
    col1: Shared,
    c2: Client,
    col2: Shared,
    session: SessionInfo,
}

/// Profiles CRUD, session create (Shell, started with no client attached), attach, echo,
/// disconnect client 1, write through client 2, reconnect client 1 and check that it resumed
/// with exactly the missed bytes.
async fn resume_scenario<A, B>(main: Switchable<A>, second: B, repo: &Path) -> Scenario
where
    A: Transport + Clone,
    B: Transport,
{
    let c1 = Client::connect(main.clone(), opts())
        .await
        .expect("connect c1");
    let col1 = collect(&c1, delay());
    assert!(!c1.welcome().unwrap().user.name.is_empty());

    // Providers, accounts, configurations.
    let providers = c1.providers(false).await.unwrap();
    let ids: Vec<_> = providers.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(&ids[..3], ["claude", "codex", "shell"]);
    let shell = providers.iter().find(|p| p.id == "shell").unwrap();
    assert!(shell.detection.usable, "{:?}", shell.detection);
    assert!(!shell.caps.accounts);
    let slugs =
        |c: Vec<ide_client::ConfigurationInfo>| c.into_iter().map(|c| c.slug).collect::<Vec<_>>();
    assert_eq!(slugs(c1.configurations().await.unwrap()), ["default"]);
    let work = c1.create_configuration("Work stuff").await.unwrap();
    assert_eq!(work.slug, "work-stuff");
    let e = c1.create_configuration("  ").await.unwrap_err();
    assert_eq!(e.id, "invalid-name");
    let mut edit = work.clone();
    edit.instructions = "Be brief.".into();
    edit.skills = vec!["C:/skills/lint".into(), "  ".into()];
    edit.mcp_servers = vec![ide_client::McpServerInfo {
        name: "bad name".into(),
        command: "node".into(),
        ..Default::default()
    }];
    let e = c1.update_configuration(edit.clone()).await.unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("name")),
        ("invalid-mcp-name", Some("bad name"))
    );
    edit.mcp_servers[0].name = "docs".into();
    let saved = c1.update_configuration(edit).await.unwrap();
    assert_eq!(saved.skills, ["C:/skills/lint"], "blank skill rows dropped");
    assert_eq!(saved.instructions, "Be brief.");
    let acct = c1.create_account("claude", "Work").await.unwrap();
    assert_eq!(
        (acct.provider.as_str(), acct.slug.as_str()),
        ("claude", "work")
    );
    assert_eq!(
        c1.create_account("shell", "x").await.unwrap_err().id,
        "provider-no-login"
    );
    assert_eq!(
        c1.create_account("nope", "x").await.unwrap_err().id,
        "provider-not-found"
    );
    let renamed = c1.rename_account("claude", "work", "Job").await.unwrap();
    assert_eq!(renamed.name, "Job");
    assert_eq!(c1.accounts().await.unwrap().len(), 1);

    // Session: created and started with no client attached.
    // Session: created and started with no client attached.
    let req = NewSession {
        provider: "shell".into(),
        account: None,
        configuration: "default".into(),
        repo_path: repo.display().to_string(),
        title: String::new(),
        base_ref: String::new(),
        start: true,
        cols: 100,
        rows: 30,
    };
    let bad = c1
        .create_session(NewSession {
            repo_path: repo.join("nope").display().to_string(),
            ..req.clone()
        })
        .await
        .unwrap_err();
    for (bad_req, id) in [
        (
            NewSession {
                configuration: "nope".into(),
                ..req.clone()
            },
            "configuration-not-found",
        ),
        (
            NewSession {
                provider: "nope".into(),
                ..req.clone()
            },
            "provider-not-found",
        ),
        (
            NewSession {
                account: Some("work".into()),
                ..req.clone()
            },
            "account-not-found",
        ),
    ] {
        assert_eq!(c1.create_session(bad_req).await.unwrap_err().id, id);
    }
    assert!(bad.id == "not-a-git-repo" || bad.id == "io", "{bad}");
    let s = c1
        .create_session(NewSession {
            configuration: "work-stuff".into(),
            ..req
        })
        .await
        .expect("create session");
    assert_eq!(s.state, SessionState::Running);
    assert_eq!(
        (
            s.provider.as_str(),
            s.account.as_deref(),
            s.configuration.as_str()
        ),
        ("shell", None, "work-stuff")
    );
    assert_eq!(
        c1.delete_configuration("work-stuff").await.unwrap_err().id,
        "configuration-in-use"
    );
    c1.delete_account("claude", "work", true).await.unwrap();
    assert!(Path::new(&s.worktree_path).is_dir());
    assert!(s.branch.starts_with("ide/"));
    let renamed = c1.rename_session(&s.id, "renamed").await.unwrap();
    assert_eq!(renamed.title, "renamed");
    assert_eq!(
        c1.write("nosuchid", b"x".to_vec()).await.unwrap_err().id,
        "session-not-found"
    );

    // Let the shell come up with nobody attached: ConPTY's startup cursor query (ESC[6n)
    // must be answered by the server or the console stalls.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let a = c1.attach(&s.id, None).await.expect("attach");
    assert!(a.reset);
    assert!(
        wait_until(T, || term(&col1, &s.id).text().contains('>')).await,
        "prompt: {:?}",
        term(&col1, &s.id).text()
    );
    assert!(
        term(&col1, &s.id).data.windows(4).any(|w| w == b"\x1b[6n"),
        "startup DSR seen"
    );

    c1.write(&s.id, b"echo hello-1\r".to_vec()).await.unwrap();
    assert!(
        wait_until(T, || term(&col1, &s.id).count("hello-1") >= 2).await,
        "{:?}",
        term(&col1, &s.id).text()
    );

    // Drop client 1's connection and keep it offline.
    main.set_online(false);
    c1.reconnect();
    assert!(wait_until(T, || col1.lock().unwrap().disconnected >= 1).await);
    assert!(!c1.is_connected());
    assert_eq!(c1.ping().await.unwrap_err().id, "disconnected");

    // Client 2 keeps working while client 1 is away.
    let c2 = Client::connect(second, opts()).await.expect("connect c2");
    let col2 = collect(&c2, delay());
    c2.attach(&s.id, None).await.unwrap();
    c2.write(&s.id, b"echo marker-2\r".to_vec()).await.unwrap();
    assert!(wait_until(T, || term(&col2, &s.id).count("marker-2") >= 2).await);
    assert_eq!(term(&col1, &s.id).count("marker-2"), 0);
    let state = session_info(&c2, &s.id).await;
    assert_eq!(state.state, SessionState::Running);

    // Client 1 comes back and resumes from its last offset.
    main.set_online(true);
    assert!(
        wait_until(T, || col1.lock().unwrap().connected >= 2).await,
        "reconnected"
    );
    assert!(c1.welcome().unwrap().resumed);
    assert!(wait_until(T, || term(&col1, &s.id).count("marker-2") >= 2).await);
    c2.write(&s.id, b"echo done-3\r".to_vec()).await.unwrap();
    let done =
        || term(&col1, &s.id).count("done-3") >= 2 && term(&col2, &s.id).count("done-3") >= 2;
    assert!(wait_until(T, done).await);
    // Both converge on the server's end offset with identical bytes.
    let mut end;
    let converged = {
        let t0 = Instant::now();
        loop {
            end = session_info(&c1, &s.id).await.term_end;
            let (b1, b2) = (term(&col1, &s.id), term(&col2, &s.id));
            if b1.end() == end && b2.end() == end {
                break true;
            }
            if t0.elapsed() > T {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let (b1, b2) = (term(&col1, &s.id), term(&col2, &s.id));
    assert!(converged, "c1 {} c2 {} server {end}", b1.end(), b2.end());
    assert_eq!(b1.base, 0);
    assert_eq!(b1.data, b2.data, "resumed stream equals a full replay");
    assert_eq!(b1.resets, 1, "only the initial reset");
    assert!(
        col1.lock().unwrap().errors.is_empty(),
        "{:?}",
        col1.lock().unwrap().errors
    );
    assert!(
        col2.lock().unwrap().errors.is_empty(),
        "{:?}",
        col2.lock().unwrap().errors
    );
    eprintln!(
        "resume ok: {} bytes, c1 got {} chunks",
        b1.data.len(),
        b1.chunks
    );

    Scenario {
        c1,
        col1,
        c2,
        col2,
        session: s,
    }
}

async fn delete_and_check(c: &Client, s: &SessionInfo) {
    let stopped = c.stop_session(&s.id).await.unwrap();
    assert_eq!(stopped.state, SessionState::Stopped);
    c.delete_session(&s.id, true).await.unwrap();
    assert!(!Path::new(&s.worktree_path).exists());
    assert!(c.sessions().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_transport_end_to_end() {
    let dir = tmp_dir("mem");
    let repo = dir.join("repo");
    init_repo(&repo);
    let server = server(&dir.join("data"));
    let main = Switchable::new(memory(&server));
    let sc = resume_scenario(main, memory(&server), &repo).await;
    let Scenario {
        c1,
        col1,
        c2,
        col2,
        session: s,
    } = sc;
    let wt = PathBuf::from(&s.worktree_path);

    if let Some(py) = python() {
        // A program asks for the cursor position while no client is attached.
        std::fs::write(
            wt.join("dsr.py"),
            "import sys, msvcrt\n\
             sys.stdout.write('\\x1b[6n'); sys.stdout.flush()\n\
             r = ''\n\
             while not r.endswith('R'):\n    r += msvcrt.getwch()\n\
             print('DSR-REPLY:' + r.replace('\\x1b', 'ESC'))\n",
        )
        .unwrap();
        c1.detach(&s.id).await.unwrap();
        c2.detach(&s.id).await.unwrap();
        let resume_at = term(&col1, &s.id).end();
        c2.write(&s.id, format!("\"{}\" dsr.py\r", py.display()).into_bytes())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        c1.attach(&s.id, Some(resume_at)).await.unwrap();
        assert!(
            wait_until(T, || term(&col1, &s.id).text().contains("DSR-REPLY:ESC[")).await,
            "{:?}",
            term(&col1, &s.id).text()
        );
        let text = term(&col1, &s.id).text();
        let line = text.lines().find(|l| l.contains("DSR-REPLY:ESC[")).unwrap();
        eprintln!(
            "dsr reply (no client attached): {line}; ESC[6n seen {} times in the PTY output",
            text.matches("\x1b[6n").count()
        );

        // Flood ~5 MB through a slow client; RPC must stay responsive.
        std::fs::write(
            wt.join("flood.py"),
            "import sys\n\
             w = sys.stdout.write\n\
             for i in range(50000):\n    w('%07d' % i + 'y' * 92 + '\\n')\n\
             sys.stdout.flush()\n\
             print('FLOOD' + '-DONE')\n",
        )
        .unwrap();
        let slow_delay = Arc::new(AtomicU64::new(200));
        let c3 = Client::connect(memory(&server), opts()).await.unwrap();
        let col3 = collect(&c3, slow_delay.clone());
        c3.attach(&s.id, None).await.unwrap();
        let start_end = session_info(&c1, &s.id).await.term_end;
        let t0 = Instant::now();
        c1.write(
            &s.id,
            format!("\"{}\" flood.py\r", py.display()).into_bytes(),
        )
        .await
        .unwrap();
        let mut max_ping = Duration::ZERO;
        let mut pings = 0;
        while term(&col1, &s.id).count("FLOOD-DONE") < 1 {
            assert!(t0.elapsed() < Duration::from_secs(120), "flood timed out");
            let p = Instant::now();
            c3.ping().await.unwrap();
            c1.sessions().await.unwrap();
            max_ping = max_ping.max(p.elapsed());
            pings += 1;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let flood_secs = t0.elapsed().as_secs_f64();
        let server_end = session_info(&c1, &s.id).await.term_end;
        let slow_end = term(&col3, &s.id).end();
        eprintln!(
            "flood: {} bytes of PTY output in {flood_secs:.1}s; slow client at {} of {server_end}; \
             {pings} RPC round trips, max {max_ping:?}",
            server_end - start_end,
            slow_end
        );
        assert!(server_end - start_end > 1_000_000, "flood produced output");
        assert!(
            slow_end < server_end,
            "slow client lags (flow control engaged)"
        );
        assert!(
            max_ping < Duration::from_secs(2),
            "RPC stalled: {max_ping:?}"
        );
        // Let the slow client catch up: same bytes as the fast one.
        slow_delay.store(0, Ordering::Relaxed);
        let caught_up = wait_until(Duration::from_secs(60), || {
            let (a, b) = (term(&col3, &s.id), term(&col1, &s.id));
            a.end() >= server_end && b.end() >= server_end
        })
        .await;
        assert!(caught_up);
        let (a, b) = (term(&col3, &s.id), term(&col1, &s.id));
        let n = server_end as usize;
        assert_eq!(a.data[..n], b.data[..n]);
        assert!(col3.lock().unwrap().errors.is_empty());
    } else {
        eprintln!("python not found: DSR program and flood checks skipped");
    }
    assert!(
        col1.lock().unwrap().errors.is_empty(),
        "{:?}",
        col1.lock().unwrap().errors
    );
    assert!(col2.lock().unwrap().errors.is_empty());
    // State notifications arrived.
    assert!(col1.lock().unwrap().notifies.iter().any(|n| matches!(
        n,
        ide_client::Notify::State {
            state: SessionState::Running,
            ..
        }
    ) || matches!(
        n,
        ide_client::Notify::SessionsChanged
    )));

    delete_and_check(&c1, &s).await;
    let audit = std::fs::read_to_string(dir.join("data").join("audit.log")).unwrap();
    for action in [
        "session.create",
        "process.spawn",
        "session.start",
        "session.stop",
        "session.delete",
    ] {
        assert!(audit.contains(action), "audit has {action}: {audit}");
    }
    drop((c1, c2));
    let _ = std::fs::remove_dir_all(&dir);
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .output();
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .output();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_child_daemon_end_to_end() {
    let dir = tmp_dir("child");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let bin = env!("CARGO_BIN_EXE_ide-server");
    let local = || {
        LocalChild::new(bin)
            .env("IDE_DATA_DIR", data.display().to_string())
            .env("IDE_SERVER_IDLE_EXIT_SECS", "60")
    };
    let main = Switchable::new(local());
    let sc = resume_scenario(main, local(), &repo).await;
    // The daemon owns the session: it outlived client 1's relay process.
    let pid: u32 = std::fs::read_to_string(data.join("ide-server.pid"))
        .expect("daemon pid file")
        .trim()
        .parse()
        .unwrap();
    eprintln!("daemon pid {pid}");
    let welcome = sc.c1.welcome().unwrap();
    assert_eq!(welcome.user, ide_server::current_user());
    delete_and_check(&sc.c1, &sc.session).await;
    // `server.shutdown` stops the daemon (and its sessions); the pid file goes away.
    let extra = sc
        .c1
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repo_path: repo.display().to_string(),
            title: "shutdown".into(),
            base_ref: String::new(),
            start: true,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    assert_eq!(extra.state, SessionState::Running);
    sc.c1.shutdown_server().await.unwrap();
    drop(sc);
    let gone = wait_until(Duration::from_secs(10), || {
        !data.join("ide-server.pid").exists()
    })
    .await;
    if !gone {
        kill_pid(pid);
    }
    assert!(gone, "daemon exited after server.shutdown");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdio_no_daemon() {
    let dir = tmp_dir("nodaemon");
    let data = dir.join("data");
    let t = LocalChild::new(env!("CARGO_BIN_EXE_ide-server"))
        .no_daemon()
        .env("IDE_DATA_DIR", data.display().to_string());
    let c = Client::connect(
        t,
        ClientOptions {
            reconnect: false,
            ..opts()
        },
    )
    .await
    .unwrap();
    c.ping().await.unwrap();
    assert_eq!(c.configurations().await.unwrap()[0].slug, "default");
    assert!(!data.join("ide-server.pid").exists(), "no daemon started");
    drop(c);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Raw handshake: version mismatch, wrong user, request before Hello.
#[tokio::test]
async fn handshake_errors() {
    let dir = tmp_dir("hs");
    let server = server(&dir.join("data"));
    async fn hello(
        server: &Arc<ide_server::Server>,
        user: UserInfo,
        req: Request,
    ) -> Result<Response, ide_proto::ProtoError> {
        let (a, b) = ide_proto::memory_pair();
        let s = server.clone();
        tokio::spawn(async move { s.serve(b, user).await });
        let (mux, mut rx) = Mux::new(a, MuxConfig::new(Role::Client));
        mux.send_rpc(&RpcMessage::Request { id: 7, req }.encode())
            .unwrap();
        match RpcMessage::decode(&rx.recv().await.unwrap()).unwrap() {
            RpcMessage::Response { id: 7, result } => result,
            other => panic!("{other:?}"),
        }
    }
    let me = server.user().clone();
    let h = |v| {
        Request::Hello(Hello {
            proto_version: v,
            client_version: "test".into(),
            resume: None,
        })
    };
    let e = hello(&server, me.clone(), h(PROTO_VERSION + 1))
        .await
        .unwrap_err();
    assert_eq!(e.id, "proto-version");
    assert_eq!(e.get("server"), Some(PROTO_VERSION.to_string().as_str()));
    let other = UserInfo {
        name: "mallory".into(),
        id: "X\\mallory".into(),
    };
    assert_eq!(
        hello(&server, other, h(PROTO_VERSION))
            .await
            .unwrap_err()
            .id,
        "unauthorized"
    );
    assert_eq!(
        hello(&server, me.clone(), Request::Ping)
            .await
            .unwrap_err()
            .id,
        "not-handshaken"
    );
    match hello(&server, me, h(PROTO_VERSION)).await.unwrap() {
        Response::Welcome(w) => {
            assert!(!w.resumed);
            assert_eq!(
                w.capabilities,
                [
                    "providers",
                    "accounts",
                    "configurations",
                    "sessions",
                    "term",
                    "files",
                    "browse",
                    "browse-pick",
                    "search",
                    "git",
                    "def"
                ]
            );
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
