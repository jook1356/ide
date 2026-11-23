//! The agent bridge (`docs/agent-bridge.md`) without any agent CLI: launch specs carry the
//! `ide` MCP server and its instructions; an agent connection (session token) gets the tools,
//! nothing else; an app-carried tool round-trips through a connected app; the real
//! `ide-server mcp` speaks MCP over stdio to a real daemon.
//!
//! Run with `IDE_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use ide_client::{
    Client, ClientEvent, ClientOptions, ConfigurationInfo, LocalChild, McpServerInfo, NewSession,
    Notify,
};
use ide_core::{LaunchEnv, Paths};
use ide_proto::{
    AgentHello, Hello, Mux, MuxConfig, PROTO_VERSION, ProtoError, Request, Response, Role,
    RpcMessage, RpcReceiver,
};
use ide_server::bridge::{BridgeConfig, SESSION_ENV, SOCKET_ENV, TOKEN_ENV};
use ide_server::{Server, ServerConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn fake_exe(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    });
    std::fs::write(&p, "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

const FAKE_SOCKET: &str = "ide-bridge-test-socket";

/// An in-process server with a bridge (fake socket / program) and fake `claude` / `codex`.
fn bridged_server(data: &Path, bin: &Path) -> Arc<Server> {
    std::fs::create_dir_all(bin).unwrap();
    fake_exe(bin, "claude");
    fake_exe(bin, "codex");
    let mut cfg = ServerConfig::for_paths(Paths::new(data));
    let real = LaunchEnv::from_process();
    cfg.launch_env = Some(LaunchEnv {
        path: Some(bin.as_os_str().to_owned()),
        pathext: Some(".EXE".into()),
        ..real
    });
    cfg.bridge = Some(BridgeConfig {
        exe: bin.join("ide-server-fake"),
        socket: FAKE_SOCKET.into(),
    });
    Server::new(cfg)
}

fn opts() -> ClientOptions {
    ClientOptions {
        reconnect: false,
        ..ClientOptions::default()
    }
}

fn new_session(provider: &str, original_repo: Option<String>, repos: Vec<String>) -> NewSession {
    NewSession {
        provider: provider.into(),
        account: None,
        configuration: "default".into(),
        repos,
        original_repo,
        refresh_base: false,
        bases: Default::default(),
        title: String::new(),
        start: false,
        cols: 80,
        rows: 24,
    }
}

/// A raw agent connection (what `ide-server mcp` opens) over a memory pair.
async fn agent_conn(
    server: &Arc<Server>,
    session: &str,
    token: &str,
) -> (Mux, RpcReceiver, Result<Response, ProtoError>) {
    let (a, b) = ide_proto::memory_pair();
    let s = server.clone();
    let user = s.user().clone();
    tokio::spawn(async move { s.serve(b, user).await });
    let (mux, mut rx) = Mux::new(a, MuxConfig::new(Role::Client));
    let req = Request::Hello(Hello {
        proto_version: PROTO_VERSION,
        client_version: "test-agent".into(),
        resume: None,
        agent: Some(AgentHello {
            session: session.into(),
            token: token.into(),
        }),
    });
    mux.send_rpc(&RpcMessage::Request { id: 1, req }.encode())
        .unwrap();
    let r = match RpcMessage::decode(&rx.recv().await.unwrap()).unwrap() {
        RpcMessage::Response { id: 1, result } => result,
        other => panic!("{other:?}"),
    };
    (mux, rx, r)
}

async fn agent_req(mux: &Mux, rx: &mut RpcReceiver, id: u64, req: Request) -> Result<Response, ProtoError> {
    mux.send_rpc(&RpcMessage::Request { id, req }.encode())
        .unwrap();
    loop {
        match RpcMessage::decode(&rx.recv().await.unwrap()).unwrap() {
            RpcMessage::Response { id: got, result } if got == id => return result,
            RpcMessage::Notify(n) => panic!("agent connections get no notifications: {n:?}"),
            _ => continue,
        }
    }
}

async fn call(mux: &Mux, rx: &mut RpcReceiver, id: u64, tool: &str, args: &str) -> ide_proto::AgentToolResult {
    match agent_req(
        mux,
        rx,
        id,
        Request::AgentCall {
            tool: tool.into(),
            args: args.into(),
        },
    )
    .await
    {
        Ok(Response::AgentResult(r)) => r,
        other => panic!("{other:?}"),
    }
}

fn env_of(env: &[(String, String)], k: &str) -> Option<String> {
    env.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_specs_carry_the_bridge() {
    let dir = tmp_dir("bridge-specs");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let bin = dir.join("bin");
    let server = bridged_server(&data, &bin);
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    let cfg = c.configurations().await.unwrap().remove(0);
    // A configuration server named like the bridge's is left out (the name is reserved).
    c.update_configuration(ConfigurationInfo {
        mcp_servers: vec![McpServerInfo {
            name: "ide".into(),
            command: "node".into(),
            args: vec!["shadow.js".into()],
            env: Vec::new(),
        }],
        instructions: "Be brief.".into(),
        ..cfg
    })
    .await
    .unwrap();
    let repo_id = c
        .register_repo(&repo.display().to_string())
        .await
        .unwrap()
        .id;
    let ctx = server.authorize(server.user()).unwrap();
    for provider in ["claude", "codex", "shell"] {
        let (s, _) = c
            .create_session(new_session(provider, Some(repo_id.clone()), Vec::new()))
            .await
            .unwrap();
        let spec = ctx.launch_spec(&ctx.resolve_session(&s.id).unwrap()).unwrap();
        let token = env_of(&spec.env, TOKEN_ENV).expect("token env");
        assert_eq!(token.len(), 32);
        assert_eq!(env_of(&spec.env, SESSION_ENV).as_deref(), Some(s.id.as_str()));
        assert_eq!(env_of(&spec.env, SOCKET_ENV).as_deref(), Some(FAKE_SOCKET));
        // The same token on every launch of the session (until the daemon restarts).
        let again = ctx.launch_spec(&ctx.resolve_session(&s.id).unwrap()).unwrap();
        assert_eq!(env_of(&again.env, TOKEN_ENV), Some(token.clone()));
        let scratch = data.join("sessions").join(&s.id).join("agent");
        match provider {
            "claude" => {
                let mcp: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(scratch.join("mcp.json")).unwrap(),
                )
                .unwrap();
                let ide = &mcp["mcpServers"]["ide"];
                assert_eq!(ide["command"], bin.join("ide-server-fake").display().to_string());
                assert_eq!(ide["args"], serde_json::json!(["mcp"]));
                assert_eq!(ide["env"][TOKEN_ENV], token.as_str());
                assert_eq!(ide["env"][SESSION_ENV], s.id.as_str());
                assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1, "{mcp}");
                let text = std::fs::read_to_string(scratch.join("instructions.md")).unwrap();
                assert!(text.starts_with("Be brief.\n\n## ide\n"), "{text}");
            }
            "codex" => {
                let ide = spec
                    .args
                    .iter()
                    .find(|a| a.starts_with("mcp_servers.ide="))
                    .expect("ide mcp override");
                assert!(ide.contains(r#"args=["mcp"]"#) && ide.contains(&token), "{ide}");
                assert!(!spec.args.iter().any(|a| a.contains("shadow.js")));
                assert!(spec.args.iter().any(|a| a.starts_with("developer_instructions=")
                    && a.contains("## ide")));
            }
            _ => {
                // A plain shell gets the env vars only.
                assert!(!scratch.join("mcp.json").exists());
            }
        }
        c.delete_session(&s.id, true).await.unwrap();
        // A deleted session's token no longer opens a bridge connection.
        let (_m, _rx, r) = agent_conn(&server, &s.id, &token).await;
        assert_eq!(r.unwrap_err().id, "unauthorized");
    }
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_connection_tools_and_app_round_trip() {
    let dir = tmp_dir("bridge-agent");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let server = bridged_server(&data, &dir.join("bin"));
    let app = Client::connect(memory(&server), opts()).await.unwrap();
    let repo_id = app
        .register_repo(&repo.display().to_string())
        .await
        .unwrap()
        .id;
    let (s, _) = app
        .create_session(new_session("shell", Some(repo_id), Vec::new()))
        .await
        .unwrap();
    let ctx = server.authorize(server.user()).unwrap();
    let spec = ctx.launch_spec(&ctx.resolve_session(&s.id).unwrap()).unwrap();
    let token = env_of(&spec.env, TOKEN_ENV).unwrap();

    // Wrong token / another session's id: refused.
    let (_m, _rx, r) = agent_conn(&server, &s.id, "nope").await;
    assert_eq!(r.unwrap_err().id, "unauthorized");
    let (_m, _rx, r) = agent_conn(&server, "other", &token).await;
    assert_eq!(r.unwrap_err().id, "unauthorized");

    let (mux, mut rx, r) = agent_conn(&server, &s.id, &token).await;
    assert!(matches!(r, Ok(Response::Welcome(_))), "{r:?}");
    let tools = match agent_req(&mux, &mut rx, 2, Request::AgentTools).await {
        Ok(Response::AgentTools(t)) => t,
        other => panic!("{other:?}"),
    };
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "ide_session",
            "ide_show_message",
            "ide_list_related_sessions",
            "ide_create_child_session",
            "ide_send_to_session",
            "ide_merge_child",
            "ide_sync_from_parent",
        ]
    );
    for t in &tools {
        let schema: serde_json::Value = serde_json::from_str(&t.input_schema).unwrap();
        assert_eq!(schema["type"], "object");
    }
    // Only agent.* requests.
    let e = agent_req(&mux, &mut rx, 3, Request::SessionList).await.unwrap_err();
    assert_eq!(e.id, "bad-request");

    let r = call(&mux, &mut rx, 4, "ide_session", "{}").await;
    assert!(!r.is_error, "{}", r.text);
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["id"], s.id.as_str());
    assert_eq!(v["mode"], "original-repo");
    assert_eq!(v["repositories"][0]["name"], "repo");
    let r = call(&mux, &mut rx, 5, "nope", "{}").await;
    assert!(r.is_error && r.text.contains("unknown tool"), "{}", r.text);
    let r = call(&mux, &mut rx, 6, "ide_show_message", "{}").await;
    assert!(r.is_error && r.text.contains("message"), "{}", r.text);

    // An app carries out ide_show_message: it gets the request and its answer goes back.
    let mut events = app.events().unwrap();
    let app2 = app.clone();
    let sid = s.id.clone();
    let answering = tokio::spawn(async move {
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::AgentRequest {
                request,
                session,
                tool,
                args,
            }) = e
            {
                assert_eq!((session.as_str(), tool.as_str()), (sid.as_str(), "ide_show_message"));
                let v: serde_json::Value = serde_json::from_str(&args).unwrap();
                app2.agent_reply(request, Ok(format!("shown: {}", v["message"].as_str().unwrap())))
                    .await
                    .unwrap();
                return;
            }
        }
    });
    let r = call(&mux, &mut rx, 7, "ide_show_message", r#"{"message":"Build is green."}"#).await;
    assert_eq!((r.is_error, r.text.as_str()), (false, "shown: Build is green."));
    answering.await.unwrap();

    // No app connected: refused at once instead of waiting.
    drop(app);
    let closed = wait_until(Duration::from_secs(5), || ctx.subscriber_count() == 0).await;
    assert!(closed, "the app's subscription ended");
    let r = call(&mux, &mut rx, 8, "ide_show_message", r#"{"message":"x"}"#).await;
    assert!(r.is_error && r.text.contains("no ide window"), "{}", r.text);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `ide-server mcp` (the real binary) against a real daemon: initialize, tools/list,
/// tools/call, with the env a session's agent CLI would pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_stdio_against_a_daemon() {
    let dir = tmp_dir("bridge-mcp");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // A fake `claude` first on the daemon's PATH: creating a workspace session generates its
    // launch files (with the bridge) without running anything.
    fake_exe(&bin_dir, "claude");
    let path = std::env::join_paths(
        std::iter::once(bin_dir.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_ide-server");
    let c = Client::connect(
        LocalChild::new(exe)
            .env("IDE_DATA_DIR", data.display().to_string())
            .env("IDE_SERVER_IDLE_EXIT_SECS", "60")
            .env("PATH", path.to_string_lossy().into_owned()),
        opts(),
    )
    .await
    .unwrap();
    let repo_id = c
        .register_repo(&repo.display().to_string())
        .await
        .unwrap()
        .id;
    let (s, _) = c
        .create_session(new_session("claude", None, vec![repo_id]))
        .await
        .unwrap();
    let mcp: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.join("sessions").join(&s.id).join("agent").join("mcp.json"))
            .unwrap(),
    )
    .unwrap();
    let ide = &mcp["mcpServers"]["ide"];
    assert!(ide_core::same_path(Path::new(ide["command"].as_str().unwrap()), Path::new(exe)));

    let mut cmd = tokio::process::Command::new(ide["command"].as_str().unwrap());
    cmd.args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in ide["env"].as_object().unwrap() {
        cmd.env(k, v.as_str().unwrap());
    }
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    // Writes one message; reads the answer unless it is a notification (no id).
    let mut ask = async |msg: serde_json::Value| -> serde_json::Value {
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        if msg.get("id").is_none() {
            return serde_json::Value::Null;
        }
        let line = tokio::time::timeout(Duration::from_secs(20), out.next_line())
            .await
            .expect("mcp answer in time")
            .unwrap()
            .expect("mcp output");
        serde_json::from_str(&line).unwrap()
    };
    let init = ask(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}))
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "ide");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    // Notifications get no answer: the next line answers the next request.
    ask(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    let list = ask(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools[0]["name"], "ide_session");
    assert_eq!(tools[0]["inputSchema"]["type"], "object");
    let r = ask(serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"ide_session","arguments":{}}}))
    .await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!((v["id"].as_str(), v["mode"].as_str()), (Some(s.id.as_str()), Some("workspace")));
    let r = ask(serde_json::json!({"jsonrpc":"2.0","id":4,"method":"nope"})).await;
    assert_eq!(r["error"]["code"], -32601);
    drop(ask);
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("mcp exits when stdin closes")
        .unwrap();
    assert!(status.success());

    // `ide-server hook` (the real binary) as Claude Code runs it: stdin = the hook input,
    // the session env; no opinion = nothing printed, exit 0; the daemon notifies the activity.
    let mut events = c.events().unwrap();
    let hook = async |event: &str, input: &str, with_env: bool| -> (i32, String) {
        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(["hook", "--provider", "claude", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for k in [SESSION_ENV, TOKEN_ENV, SOCKET_ENV] {
            cmd.env_remove(k);
        }
        if with_env {
            for (k, v) in ide["env"].as_object().unwrap() {
                cmd.env(k, v.as_str().unwrap());
            }
        }
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).await.unwrap();
        drop(stdin);
        let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .expect("hook exits in time")
            .unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
    };
    assert_eq!(hook("UserPromptSubmit", r#"{"prompt":"x"}"#, true).await, (0, String::new()));
    let working = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::Activity { session, activity }) = e
                && session == s.id
            {
                return activity;
            }
        }
        panic!("events ended");
    })
    .await
    .expect("activity notified");
    assert_eq!(working, ide_proto::AgentActivity::Working);
    let edit = r#"{"tool_name":"Write","tool_input":{"file_path":"C:/x/a.rs"}}"#;
    assert_eq!(hook("PreToolUse", edit, true).await, (0, String::new()));
    // Without the session env (or with nothing the IDE uses): no opinion, never an error.
    assert_eq!(hook("PreToolUse", edit, false).await, (0, String::new()));
    assert_eq!(hook("Nope", "{}", true).await, (0, String::new()));
    // Usage errors exit 1, not 2 (a CLI reads a hook's exit code 2 as "block").
    let usage = tokio::process::Command::new(exe).output().await.unwrap();
    assert_eq!(usage.status.code(), Some(1));

    c.delete_session(&s.id, true).await.unwrap();
    c.shutdown_server().await.unwrap();
    drop(c);
    let _ = wait_until(Duration::from_secs(10), || !data.join("ide-server.pid").exists()).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hooks through an agent connection: the activity follows the turn (prompt -> working,
/// permission prompt -> needs input, the user's answer typed in the terminal -> working,
/// stop -> idle), apps are notified, `SessionInfo.activity` shows it while the process runs;
/// no rule refuses anything yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hooks_drive_the_activity() {
    use ide_proto::{AgentActivity, AgentHookEvent};
    let dir = tmp_dir("bridge-hooks");
    let repo = dir.join("repo");
    init_repo(&repo);
    let server = bridged_server(&dir.join("data"), &dir.join("bin"));
    let app = Client::connect(memory(&server), opts()).await.unwrap();
    let mut events = app.events().unwrap();
    let repo_id = app
        .register_repo(&repo.display().to_string())
        .await
        .unwrap()
        .id;
    let mut ns = new_session("shell", Some(repo_id), Vec::new());
    ns.start = true;
    let (s, _) = app.create_session(ns).await.unwrap();
    let ctx = server.authorize(server.user()).unwrap();
    let token = env_of(
        &ctx.launch_spec(&ctx.resolve_session(&s.id).unwrap()).unwrap().env,
        TOKEN_ENV,
    )
    .unwrap();
    let (mux, mut rx, r) = agent_conn(&server, &s.id, &token).await;
    assert!(r.is_ok());
    let mut id = 10;
    let mut hook = async |event: AgentHookEvent| {
        id += 1;
        match agent_req(&mux, &mut rx, id, Request::AgentHook { event }).await {
            Ok(Response::AgentHookDecision(d)) => d,
            other => panic!("{other:?}"),
        }
    };
    let mut next_activity = async || {
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = events.next().await {
                if let ClientEvent::Notify(Notify::Activity { activity, .. }) = e {
                    return activity;
                }
            }
            panic!("events ended");
        })
        .await
        .expect("activity notified")
    };
    let info = async || {
        app.sessions()
            .await
            .unwrap()
            .into_iter()
            .find(|x| x.id == s.id)
            .unwrap()
            .activity
    };

    assert_eq!(info().await, AgentActivity::Unknown, "nothing reported yet");
    assert_eq!(hook(AgentHookEvent::SessionStart).await, None);
    assert_eq!(next_activity().await, AgentActivity::Idle);
    assert_eq!(hook(AgentHookEvent::PromptSubmitted).await, None);
    assert_eq!(next_activity().await, AgentActivity::Working);
    let edit = AgentHookEvent::PreEdit {
        tool: "Write".into(),
        paths: vec!["a.rs".into()],
    };
    assert_eq!(hook(edit).await, None, "no rule refuses edits yet");
    assert_eq!(hook(AgentHookEvent::NeedsInput { permission: true }).await, None);
    assert_eq!(next_activity().await, AgentActivity::NeedsInput);
    assert_eq!(info().await, AgentActivity::NeedsInput);
    // The user answers in the terminal.
    app.write(&s.id, b"1".to_vec()).await.unwrap();
    assert_eq!(next_activity().await, AgentActivity::Working);
    assert_eq!(hook(AgentHookEvent::Stopped).await, None);
    assert_eq!(next_activity().await, AgentActivity::Idle);
    assert_eq!(info().await, AgentActivity::Idle);
    // A stopped process reports nothing.
    app.stop_session(&s.id).await.unwrap();
    let mut unknown = false;
    for _ in 0..50 {
        if info().await == AgentActivity::Unknown {
            unknown = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(unknown, "activity resets when the process ends");
    app.delete_session(&s.id, true).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
