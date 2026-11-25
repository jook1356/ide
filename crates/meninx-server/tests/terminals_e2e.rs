//! The agent's terminal tools (`docs/terminals.md`) through an agent connection, with real
//! shells in user terminals: list (own, related, unrelated), read (screen and scrollback),
//! input (keys, the user's approval answered by an app, kept grants, only own terminals).
//!
//! Run with `MENINX_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use meninx_client::{Client, ClientEvent, ClientOptions, NewSession, Notify, UTermOpen};
use meninx_core::Paths;
use meninx_proto::{
    AgentHello, Hello, Mux, MuxConfig, PROTO_VERSION, Request, Response, Role, RpcMessage,
    RpcReceiver,
};
use meninx_server::bridge::{BridgeConfig, TOKEN_ENV};
use meninx_server::{Server, ServerConfig};

fn bridged(data: &Path) -> Arc<Server> {
    let mut cfg = ServerConfig::for_paths(Paths::new(data));
    cfg.bridge = Some(BridgeConfig {
        exe: PathBuf::from("meninx-server-fake"),
        socket: "terminals-test".into(),
    });
    Server::new(cfg)
}

async fn agent(server: &Arc<Server>, session: &str) -> (Mux, RpcReceiver) {
    let ctx = server.authorize(server.user()).unwrap();
    let spec = ctx.launch_spec(&ctx.resolve_session(session).unwrap()).unwrap();
    let token = spec.env.iter().find(|(k, _)| k == TOKEN_ENV).unwrap().1.clone();
    let (a, b) = meninx_proto::memory_pair();
    let s = server.clone();
    let user = s.user().clone();
    tokio::spawn(async move { s.serve(b, user).await });
    let (mux, mut rx) = Mux::new(a, MuxConfig::new(Role::Client));
    let hello = Request::Hello(Hello {
        proto_version: PROTO_VERSION,
        client_version: "test-agent".into(),
        resume: None,
        agent: Some(AgentHello {
            session: session.into(),
            token,
        }),
    });
    mux.send_rpc(&RpcMessage::Request { id: 1, req: hello }.encode()).unwrap();
    let _ = rx.recv().await.unwrap();
    (mux, rx)
}

/// The agent's tool calls (request ids counted here).
struct Agent {
    mux: Mux,
    rx: RpcReceiver,
    next: u64,
}

impl Agent {
    /// One call: (is_error, parsed JSON or the text as a JSON string).
    async fn call(&mut self, name: &str, args: serde_json::Value) -> (bool, serde_json::Value) {
        self.next += 1;
        let id = self.next;
        let req = Request::AgentCall {
            tool: name.into(),
            args: args.to_string(),
        };
        self.mux.send_rpc(&RpcMessage::Request { id, req }.encode()).unwrap();
        loop {
            if let RpcMessage::Response { id: got, result } = RpcMessage::decode(&self.rx.recv().await.unwrap()).unwrap()
                && got == id
            {
                let Ok(Response::AgentResult(r)) = result else {
                    panic!("{result:?}")
                };
                let v = serde_json::from_str(&r.text).unwrap_or(serde_json::Value::String(r.text));
                return (r.is_error, v);
            }
        }
    }

    /// An error's text.
    async fn fails(&mut self, name: &str, args: serde_json::Value) -> String {
        let (bad, v) = self.call(name, args).await;
        assert!(bad, "{name} should fail: {v}");
        v.as_str().unwrap().to_owned()
    }

    /// `meninx_terminal_read` until `f` holds (or a timeout: the last text, for the assert).
    async fn read_until(&mut self, terminal: &str, lines: u64, f: impl Fn(&str) -> bool) -> String {
        let t = Instant::now();
        loop {
            let (bad, v) = self.call("meninx_terminal_read", serde_json::json!({"terminal": terminal, "lines": lines})).await;
            let text = v.as_str().unwrap_or_default().to_owned();
            assert!(!bad, "{text}");
            if f(&text) || t.elapsed() > Duration::from_secs(30) {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn shell_session(repos: Vec<String>, title: &str) -> NewSession {
    NewSession {
        provider: "shell".into(),
        account: None,
        configuration: "default".into(),
        repos,
        original_repo: None,
        refresh_base: false,
        background: false,
        bases: Default::default(),
        title: title.into(),
        start: false,
        cols: 80,
        rows: 24,
    }
}

/// A command line that prints `row-1` .. `row-<n>`, one per line.
fn rows_command(n: u32) -> String {
    if cfg!(windows) {
        format!("for /L %i in (1,1,{n}) do @echo row-%i")
    } else {
        format!("for i in $(seq 1 {n}); do echo row-$i; done")
    }
}

/// A line of the text that is exactly `want` (output, not the typed command line).
fn has_line(text: &str, want: &str) -> bool {
    text.lines().any(|l| l.trim() == want)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_tools() {
    let dir = tmp_dir("terminals");
    let r1 = dir.join("r1");
    init_repo(&r1);
    let server = bridged(&dir.join("data"));
    let c = Client::connect(
        memory(&server),
        ClientOptions {
            reconnect: false,
            ..ClientOptions::default()
        },
    )
    .await
    .unwrap();
    let id1 = c.register_repo(&r1.display().to_string()).await.unwrap().id;
    let (p, _) = c.create_session(shell_session(vec![id1.clone()], "P")).await.unwrap();
    let (s, _) = c.create_child_session(&p.id, true, Vec::new(), "S", None, false, 80, 24).await.unwrap();
    let (u, _) = c.create_session(shell_session(vec![id1.clone()], "U")).await.unwrap();
    // cmd on Windows (fast to start), the login shell elsewhere.
    let shell = cfg!(windows).then(|| "cmd".to_owned());
    let open = |session: &str, title: &str| UTermOpen {
        session: session.into(),
        title: Some(title.into()),
        shell: shell.clone(),
        ..UTermOpen::default()
    };
    let ts = c.uterm_open(open(&s.id, "work")).await.unwrap();
    let tp = c.uterm_open(open(&p.id, "parent term")).await.unwrap();
    let tu = c.uterm_open(open(&u.id, "other")).await.unwrap();

    // The app answers approval questions from a script of answers and records them.
    let answers = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<Result<String, String>>::new()));
    let asked = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let mut events = c.events().unwrap();
    let (c2, answers2, asked2) = (c.clone(), answers.clone(), asked.clone());
    tokio::spawn(async move {
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::AgentRequest { request, tool, args, .. }) = e
                && tool == meninx_proto::APPROVAL_REQUEST
            {
                asked2.lock().unwrap().push(serde_json::from_str(&args).unwrap());
                let a = answers2.lock().unwrap().pop_front().unwrap_or(Err("no answer".into()));
                let _ = c2.agent_reply(request, a).await;
            }
        }
    });

    let (mux, rx) = agent(&server, &s.id).await;
    let mut a = Agent { mux, rx, next: 1 };

    // The tools are listed.
    {
        let names: Vec<String> = meninx_server::bridge::tools().into_iter().map(|t| t.name).collect();
        for t in ["meninx_terminal_list", "meninx_terminal_read", "meninx_terminal_input"] {
            assert!(names.iter().any(|n| n == t), "{t}");
        }
    }

    // List: own terminals; a related session's; not an unrelated one's.
    let (bad, v) = a.call("meninx_terminal_list", serde_json::json!({})).await;
    assert!(!bad, "{v}");
    assert_eq!(v["session"], s.id.as_str());
    let terms = v["terminals"].as_array().unwrap();
    assert_eq!(terms.len(), 1, "{v}");
    assert_eq!((terms[0]["id"].as_str(), terms[0]["title"].as_str()), (Some(ts.id.as_str()), Some("work")));
    assert_eq!(terms[0]["running"], true);
    assert!(terms[0]["ports"].as_array().unwrap().is_empty());
    let (bad, v) = a.call("meninx_terminal_list", serde_json::json!({"session": p.id})).await;
    assert!(!bad && v["terminals"][0]["id"] == tp.id.as_str(), "{v}");
    assert!(a.fails("meninx_terminal_list", serde_json::json!({"session": u.id})).await.contains("not a related session"));

    // Read: own and related terminals; not an unrelated or unknown one.
    let text = a.read_until(&ts.id, 50, |t| !t.contains("(empty)")).await;
    assert!(text.starts_with(&format!("terminal {} \"work\" (running", ts.id)), "{text}");
    let text = a.read_until(&tp.id, 0, |_| true).await;
    assert!(text.contains(&format!("session {}", p.id)), "{text}");
    assert!(a.fails("meninx_terminal_read", serde_json::json!({"terminal": tu.id})).await.contains("no terminal"));
    assert!(a.fails("meninx_terminal_read", serde_json::json!({"terminal": "nope"})).await.contains("no terminal"));

    // Input: bad keys and missing input fail before anything is asked.
    let e = a.fails("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "keys": ["enterr"]})).await;
    assert!(e.contains("unknown key `enterr`"), "{e}");
    assert!(a.fails("meninx_terminal_input", serde_json::json!({"terminal": ts.id})).await.contains("give `text`"));
    // Only own terminals take input.
    let e = a.fails("meninx_terminal_input", serde_json::json!({"terminal": tp.id, "text": "x"})).await;
    assert!(e.contains("only type into this session's own terminals"), "{e}");
    assert!(asked.lock().unwrap().is_empty());

    // Refused: nothing typed.
    answers.lock().unwrap().push_back(Err("refused".into()));
    let e = a.fails("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "text": "echo hello-meninx", "keys": ["enter"]})).await;
    assert!(e.contains("did not allow"), "{e}");
    {
        let q = asked.lock().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!((q[0]["capability"].as_str(), q[0]["target"].as_str()), (Some("terminal.input"), Some(ts.id.as_str())));
        assert_eq!(q[0]["requester_title"], "S");
        assert_eq!(q[0]["target_title"], "work (S)");
        assert_eq!(q[0]["detail"], "echo hello-meninx\n[enter]");
    }
    let text = a.read_until(&ts.id, 50, |_| true).await;
    assert!(!text.contains("hello-meninx"), "{text}");

    // Allowed for the session: typed, the output appears; the next input is not asked.
    answers.lock().unwrap().push_back(Ok("session".into()));
    let (bad, v) = a.call("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "text": "echo hello-meninx", "keys": ["enter"]})).await;
    assert!(!bad, "{v}");
    let text = a.read_until(&ts.id, 50, |t| has_line(t, "hello-meninx")).await;
    assert!(has_line(&text, "hello-meninx"), "{text}");
    let (bad, v) = a.call("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "text": format!("{}\n", rows_command(120))})).await;
    assert!(!bad, "{v}");
    assert_eq!(asked.lock().unwrap().len(), 2, "the kept grant answers");
    let list = c.approval_list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].capability.as_str(), list[0].target.as_str(), list[0].target_title.as_str()), ("terminal.input", ts.id.as_str(), "work (S)"));

    // Scrollback: rows that left the screen are read above it, oldest first.
    let text = a.read_until(&ts.id, 2000, |t| has_line(t, "row-120") && has_line(t, "row-1")).await;
    assert!(has_line(&text, "row-1") && has_line(&text, "row-120"), "{text}");
    let (scroll, screen) = text.split_once("--- screen ---").unwrap();
    assert!(scroll.contains("--- scrollback:") && has_line(scroll, "row-1"), "{text}");
    assert!(has_line(screen, "row-120"), "{text}");
    let first = text.find("row-1\n").unwrap();
    assert!(first < text.find("row-2\n").unwrap());
    // A small `lines` leaves the oldest out.
    let (_, v) = a.call("meninx_terminal_read", serde_json::json!({"terminal": ts.id, "lines": 5})).await;
    let small = v.as_str().unwrap();
    assert!(!has_line(small, "row-1") && small.contains("ask for more with `lines`"), "{small}");

    // Ctrl+C stops a running command (a key, typed with the kept grant). Windows: the command
    // ends by itself; whether ConPTY's Ctrl+C reaches it depends on the test runner (a process
    // tree started with Ctrl+C ignored keeps ignoring it), so only the typing is checked.
    let long = if cfg!(windows) { "ping -n 4 127.0.0.1" } else { "sleep 60; echo not-stopped" };
    a.call("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "text": long, "keys": "enter"})).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (bad, v) = a.call("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "keys": ["ctrl+c"]})).await;
    assert!(!bad, "{v}");
    let (bad, _) = a.call("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "text": "echo after-stop", "keys": ["enter"]})).await;
    assert!(!bad);
    let text = a.read_until(&ts.id, 0, |t| has_line(t, "after-stop")).await;
    assert!(has_line(&text, "after-stop"), "{text}");
    assert_eq!(asked.lock().unwrap().len(), 2);

    // A stopped terminal takes no input; removing it drops its grants.
    c.uterm_stop(&ts.id).await.unwrap();
    let t = Instant::now();
    while c.uterm_list(&s.id).await.unwrap()[0].state == meninx_proto::SessionState::Running && t.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let e = a.fails("meninx_terminal_input", serde_json::json!({"terminal": ts.id, "keys": ["enter"]})).await;
    assert!(e.contains("is not running"), "{e}");
    c.uterm_remove(&ts.id).await.unwrap();
    assert!(c.approval_list().await.unwrap().is_empty());

    for t in [&tp.id, &tu.id] {
        c.uterm_remove(t).await.unwrap();
    }
    c.delete_session(&s.id, true).await.unwrap();
    c.delete_session(&p.id, true).await.unwrap();
    c.delete_session(&u.id, true).await.unwrap();
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
