//! Browser control (`docs/browser-control.md`) through the agent bridge: a fake app answers
//! the daemon's site question, the user's approval and the `ide_browser_*` tools; the daemon
//! checks arguments, asks per site (origin) and turns a screenshot into an image result.
//!
//! Run with `IDE_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::*;
use ide_client::{Client, ClientEvent, ClientOptions, NewSession, Notify};
use ide_core::Paths;
use ide_proto::browser::{TAB_SITE, TOOL_ACT, TOOL_LIST, TOOL_OPEN, TOOL_READ, TOOL_SCREENSHOT};
use ide_proto::{
    AgentHello, AgentToolResult, Hello, Mux, MuxConfig, PROTO_VERSION, Request, Response, Role,
    RpcMessage, RpcReceiver,
};
use ide_server::bridge::{BridgeConfig, TOKEN_ENV};
use ide_server::{Server, ServerConfig};
use serde_json::{Value, json};

fn bridged(data: &std::path::Path) -> Arc<Server> {
    let mut cfg = ServerConfig::for_paths(Paths::new(data));
    cfg.bridge = Some(BridgeConfig {
        exe: PathBuf::from("ide-server-fake"),
        socket: "browser-test".into(),
    });
    Server::new(cfg)
}

async fn agent(server: &Arc<Server>, session: &str) -> (Mux, RpcReceiver) {
    let ctx = server.authorize(server.user()).unwrap();
    let spec = ctx.launch_spec(&ctx.resolve_session(session).unwrap()).unwrap();
    let token = spec.env.iter().find(|(k, _)| k == TOKEN_ENV).unwrap().1.clone();
    let (a, b) = ide_proto::memory_pair();
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

async fn call(mux: &Mux, rx: &mut RpcReceiver, id: u64, name: &str, args: Value) -> AgentToolResult {
    let req = Request::AgentCall {
        tool: name.into(),
        args: args.to_string(),
    };
    mux.send_rpc(&RpcMessage::Request { id, req }.encode()).unwrap();
    loop {
        if let RpcMessage::Response { id: got, result } = RpcMessage::decode(&rx.recv().await.unwrap()).unwrap()
            && got == id
        {
            let Ok(Response::AgentResult(r)) = result else {
                panic!("{result:?}")
            };
            return r;
        }
    }
}

/// What the fake app saw: (tool, args).
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_tools_ask_per_site() {
    let dir = tmp_dir("browser-tools");
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
    let (p, _) = c
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: vec![id1],
            original_repo: None,
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: "P".into(),
            start: false,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();

    let (mux, mut rx) = agent(&server, &p.id).await;

    // The fake app: tabs and their sites, scripted approval answers.
    let sites: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(vec![
        ("page".into(), "about:blank".into()),
        ("tab-2".into(), "https://other.dev/x".into()),
    ]));
    let answers = Arc::new(Mutex::new(VecDeque::<Result<String, String>>::new()));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let mut events = c.events().unwrap();
    let (c2, sites2, answers2, seen2) = (c.clone(), sites.clone(), answers.clone(), seen.clone());
    tokio::spawn(async move {
        while let Some(e) = events.next().await {
            let ClientEvent::Notify(Notify::AgentRequest { request, tool, args, .. }) = e else {
                continue;
            };
            let args: Value = serde_json::from_str(&args).unwrap();
            seen2.lock().unwrap().push((tool.clone(), args.clone()));
            let tab = args["tab"].as_str().unwrap_or("").to_owned();
            let url_of = |tab: &str| sites2.lock().unwrap().iter().find(|(t, _)| t == tab).map(|(_, u)| u.clone());
            let answer: Result<String, String> = match tool.as_str() {
                ide_proto::APPROVAL_REQUEST => answers2.lock().unwrap().pop_front().unwrap_or(Err("no answer".into())),
                TAB_SITE => match url_of(&tab) {
                    Some(u) => Ok(json!({"origin": ide_proto::browser::origin(&u), "url": u, "title": "T"}).to_string()),
                    None => Err(format!("no tab `{tab}`")),
                },
                TOOL_LIST => Ok(json!({"tabs": [{"tab": "page"}]}).to_string()),
                TOOL_OPEN => {
                    sites2.lock().unwrap().push(("tab-3".into(), args["url"].as_str().unwrap().into()));
                    Ok(json!({"tab": "tab-3"}).to_string())
                }
                TOOL_READ => Ok("Hello page".into()),
                TOOL_ACT => Ok(format!("did {}", args["action"].as_str().unwrap())),
                TOOL_SCREENSHOT => Ok(json!({"text": "shot", "image": {"data": "iVBORw0KGgo=", "mime_type": "image/png"}}).to_string()),
                other => Err(format!("unknown {other}")),
            };
            let _ = c2.agent_reply(request, answer).await;
        }
    });
    let asked = |seen: &Seen| seen.lock().unwrap().iter().filter(|(t, _)| t == ide_proto::APPROVAL_REQUEST).count();

    // List: nothing to approve.
    let r = call(&mux, &mut rx, 3, TOOL_LIST, json!({})).await;
    assert!(!r.is_error && r.text.contains("page"), "{r:?}");
    assert_eq!(asked(&seen), 0);

    // Bad arguments never reach the app.
    let n = seen.lock().unwrap().len();
    let r = call(&mux, &mut rx, 4, TOOL_ACT, json!({"tab": "page", "action": "hover"})).await;
    assert!(r.is_error && r.text.contains("unknown action"), "{r:?}");
    let r = call(&mux, &mut rx, 5, TOOL_OPEN, json!({"url": "javascript:alert(1)"})).await;
    assert!(r.is_error, "{r:?}");
    assert_eq!(seen.lock().unwrap().len(), n);

    // Open a local address: the user is asked for its site and allows it for the session.
    answers.lock().unwrap().push_back(Ok("session".into()));
    let r = call(&mux, &mut rx, 6, TOOL_OPEN, json!({"url": "localhost:5173/app"})).await;
    assert!(!r.is_error && r.text.contains("tab-3"), "{r:?}");
    {
        let s = seen.lock().unwrap();
        let (_, q) = s.iter().find(|(t, _)| t == ide_proto::APPROVAL_REQUEST).unwrap();
        assert_eq!(q["capability"], "browser.act");
        assert_eq!(q["target"], "http://localhost:5173");
        assert_eq!(q["target_title"], "http://localhost:5173");
        assert_eq!(q["requester_title"], "P");
        assert_eq!(q["detail"], "open http://localhost:5173/app");
        let (_, open) = s.iter().rfind(|(t, _)| t == TOOL_OPEN).unwrap();
        assert_eq!(open["url"], "http://localhost:5173/app");
    }

    // Acting on and reading that site now: kept grant (acting includes reading), the app gets
    // the approved origin to check.
    let r = call(&mux, &mut rx, 7, TOOL_ACT, json!({"tab": "tab-3", "action": "click", "selector": "#go"})).await;
    assert!(!r.is_error && r.text == "did click", "{r:?}");
    let r = call(&mux, &mut rx, 8, TOOL_READ, json!({"tab": "tab-3"})).await;
    assert_eq!(r.text, "Hello page");
    assert_eq!(asked(&seen), 1);
    {
        let s = seen.lock().unwrap();
        let (_, read) = s.iter().rfind(|(t, _)| t == TOOL_READ).unwrap();
        assert_eq!(read, &json!({"tab": "tab-3", "what": "text", "max_chars": 20000, "origin": "http://localhost:5173"}));
    }

    // Another site: reading it is asked; refused -> the app is not asked to act.
    answers.lock().unwrap().push_back(Err("refused".into()));
    let r = call(&mux, &mut rx, 9, TOOL_SCREENSHOT, json!({"tab": "tab-2"})).await;
    assert!(r.is_error && r.text.contains("did not allow"), "{r:?}");
    assert!(!seen.lock().unwrap().iter().any(|(t, _)| t == TOOL_SCREENSHOT));
    {
        let s = seen.lock().unwrap();
        let q = &s.iter().filter(|(t, _)| t == ide_proto::APPROVAL_REQUEST).nth(1).unwrap().1;
        assert_eq!((q["capability"].as_str(), q["target"].as_str()), (Some("browser.read"), Some("https://other.dev")));
        assert_eq!(q["detail"], "screenshot\nhttps://other.dev/x");
    }
    // Allowed once: the screenshot comes back as an image; nothing is kept.
    answers.lock().unwrap().push_back(Ok("once".into()));
    let r = call(&mux, &mut rx, 10, TOOL_SCREENSHOT, json!({"tab": "tab-2"})).await;
    assert!(!r.is_error, "{r:?}");
    assert_eq!(r.text, "shot");
    let img = r.image.clone().expect("image");
    assert_eq!((img.data.as_str(), img.mime_type.as_str()), ("iVBORw0KGgo=", "image/png"));
    assert_eq!(
        ide_server::bridge::mcp_content(r)["content"][1],
        json!({"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"})
    );

    // Navigating asks for the destination, not for the current page.
    answers.lock().unwrap().push_back(Err("refused".into()));
    let r = call(&mux, &mut rx, 11, TOOL_ACT, json!({"tab": "tab-3", "action": "navigate", "url": "https://bank.example/"})).await;
    assert!(r.is_error, "{r:?}");
    {
        let s = seen.lock().unwrap();
        let q = &s.iter().rfind(|(t, _)| t == ide_proto::APPROVAL_REQUEST).unwrap().1;
        assert_eq!(q["target"], "https://bank.example");
    }

    // An unknown tab: the app's error.
    let r = call(&mux, &mut rx, 12, TOOL_READ, json!({"tab": "tab-9"})).await;
    assert!(r.is_error && r.text.contains("no tab"), "{r:?}");

    // The settings list names the site.
    let list = c.approval_list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].capability.as_str(), list[0].target.as_str(), list[0].target_title.as_str()), ("browser.act", "http://localhost:5173", "http://localhost:5173"));

    // The tools are listed for agents.
    mux.send_rpc(&RpcMessage::Request { id: 13, req: Request::AgentTools }.encode()).unwrap();
    loop {
        if let RpcMessage::Response { id: 13, result } = RpcMessage::decode(&rx.recv().await.unwrap()).unwrap() {
            let Ok(Response::AgentTools(t)) = result else { panic!() };
            for name in ide_proto::browser::TOOLS {
                assert!(t.iter().any(|x| x.name == name), "{name}");
            }
            break;
        }
    }
    c.delete_session(&p.id, true).await.unwrap();
}
