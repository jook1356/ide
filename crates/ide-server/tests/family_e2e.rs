//! Session families (`docs/session-family.md`) through the client and the agent bridge, with
//! shell sessions and real git: separate and shared children, the depth limit, the delete /
//! detach / attach rules, merging a child and syncing from the parent, and the family tools
//! of an agent connection (list, create with a first prompt, send, merge, sync).
//!
//! Run with `IDE_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use ide_client::{Client, ClientOptions, MergeResult, NewSession};
use ide_core::Paths;
use ide_proto::{
    AgentHello, Hello, Mux, MuxConfig, PROTO_VERSION, Request, Response, Role, RpcMessage,
    RpcReceiver,
};
use ide_server::bridge::{BridgeConfig, TOKEN_ENV};
use ide_server::{Server, ServerConfig};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "-m", file]);
}

fn bridged(data: &Path) -> Arc<Server> {
    let mut cfg = ServerConfig::for_paths(Paths::new(data));
    cfg.bridge = Some(BridgeConfig {
        exe: PathBuf::from("ide-server-fake"),
        socket: "family-test".into(),
    });
    Server::new(cfg)
}

fn opts() -> ClientOptions {
    ClientOptions {
        reconnect: false,
        ..ClientOptions::default()
    }
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

/// One tool call: (is_error, parsed JSON or the text as a JSON string).
async fn tool(mux: &Mux, rx: &mut RpcReceiver, id: u64, name: &str, args: serde_json::Value) -> (bool, serde_json::Value) {
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
            let v = serde_json::from_str(&r.text).unwrap_or(serde_json::Value::String(r.text));
            return (r.is_error, v);
        }
    }
}

fn shell_session(repos: Vec<String>, start: bool) -> NewSession {
    NewSession {
        provider: "shell".into(),
        account: None,
        configuration: "default".into(),
        repos,
        original_repo: None,
        refresh_base: false,
        background: false,
        bases: Default::default(),
        title: "P".into(),
        start,
        cols: 80,
        rows: 24,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn families() {
    let dir = tmp_dir("family");
    let (r1, r2) = (dir.join("r1"), dir.join("r2"));
    init_repo(&r1);
    init_repo(&r2);
    let server = bridged(&dir.join("data"));
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    let id1 = c.register_repo(&r1.display().to_string()).await.unwrap().id;
    let id2 = c.register_repo(&r2.display().to_string()).await.unwrap().id;
    let (p, _) = c.create_session(shell_session(vec![id1.clone()], true)).await.unwrap();
    let pw = PathBuf::from(&p.repos[0].path);
    commit(&pw, "p.txt", "parent\n");

    // A separate child: only the parent's repos; its worktree starts at the parent's branch.
    let e = c
        .create_child_session(&p.id, false, vec![id2.clone()], "", None, false, 80, 24)
        .await
        .unwrap_err();
    assert_eq!(e.id, "repo-not-in-parent");
    let (a, f) = c.create_child_session(&p.id, false, Vec::new(), "A", None, false, 80, 24).await.unwrap();
    assert!(f.is_empty());
    assert_eq!((a.parent.as_deref(), a.shared), (Some(p.id.as_str()), false));
    assert_eq!(a.repos[0].base, p.repos[0].branch);
    let aw = PathBuf::from(&a.repos[0].path);
    assert!(aw.join("p.txt").is_file() && aw != pw);
    // A shared child: the parent's folder and worktrees.
    let (s, _) = c.create_child_session(&p.id, true, Vec::new(), "S", None, true, 80, 24).await.unwrap();
    assert_eq!((s.root.as_str(), s.repos[0].path.as_str()), (p.root.as_str(), p.repos[0].path.as_str()));

    // Rules.
    assert_eq!(c.delete_session(&p.id, true).await.unwrap_err().id, "session-has-children");
    assert_eq!(c.detach_repo(&p.id, &id1).await.unwrap_err().id, "repo-used-by-children");
    assert_eq!(c.attach_repo(&s.id, &id2).await.unwrap_err().id, "shared-child-repos");
    assert_eq!(c.attach_repo(&a.id, &id2).await.unwrap_err().id, "repo-not-in-parent");
    // The parent attaches a repo: its shared child follows; the separate child may take it.
    c.attach_repo(&p.id, &id2).await.unwrap();
    let all = c.sessions().await.unwrap();
    let s_now = all.iter().find(|x| x.id == s.id).unwrap();
    assert_eq!(s_now.repos.len(), 2, "the shared child follows the parent");
    let a2 = c.attach_repo(&a.id, &id2).await.unwrap();
    let p_now = all.iter().find(|x| x.id == p.id).unwrap();
    let p_r2 = p_now.repos.iter().find(|r| r.repo == id2).unwrap();
    assert_eq!(a2.repos.iter().find(|r| r.repo == id2).unwrap().base, p_r2.branch);

    // Depth: P -> A -> G; no deeper.
    let (g, _) = c.create_child_session(&a.id, false, Vec::new(), "G", None, false, 80, 24).await.unwrap();
    assert_eq!(
        c.create_child_session(&g.id, false, Vec::new(), "", None, false, 80, 24).await.unwrap_err().id,
        "family-too-deep"
    );

    // Merge a child, sync from the parent.
    commit(&aw, "a.txt", "child\n");
    let m = c.merge_child(&a.id, false).await.unwrap();
    let m1 = m.iter().find(|x| x.repo == id1).unwrap();
    assert_eq!(m1.result, MergeResult::Merged);
    assert!(pw.join("a.txt").is_file());
    assert_eq!(c.merge_child(&a.id, false).await.unwrap()[0].result, MergeResult::UpToDate);
    commit(&pw, "p2.txt", "more\n");
    let m = c.sync_parent(&a.id).await.unwrap();
    assert_eq!(m.iter().find(|x| x.repo == id1).unwrap().result, MergeResult::Merged);
    assert!(aw.join("p2.txt").is_file());
    assert!(c.merge_child(&s.id, false).await.unwrap().is_empty(), "a shared child has nothing to merge");
    assert_eq!(c.merge_child(&p.id, false).await.unwrap_err().id, "bad-request");

    // The agent bridge's family tools (the parent's agent).
    let (mux, mut rx) = agent(&server, &p.id).await;
    let (bad, v) = tool(&mux, &mut rx, 10, "ide_list_related_sessions", serde_json::json!({})).await;
    assert!(!bad, "{v}");
    let mut kids: Vec<&str> = v["sessions"].as_array().unwrap().iter().map(|x| x["title"].as_str().unwrap()).collect();
    kids.sort();
    assert_eq!(kids, ["A", "S"]);
    // A running related session gets the message at once (the shared child's shell).
    let (bad, v) = tool(&mux, &mut rx, 11, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "echo hi"})).await;
    assert!(!bad && v["delivered"] == true, "{v}");
    let (bad, v) = tool(&mux, &mut rx, 12, "ide_send_to_session", serde_json::json!({"session": g.id, "message": "x"})).await;
    assert!(bad && v.as_str().unwrap().contains("not a related session"), "{v}");
    let (bad, v) = tool(&mux, &mut rx, 13, "ide_merge_child", serde_json::json!({"child": g.id})).await;
    assert!(bad && v.as_str().unwrap().contains("not a child"), "{v}");
    let (bad, v) = tool(&mux, &mut rx, 14, "ide_merge_child", serde_json::json!({"child": a.id, "squash": true})).await;
    assert!(!bad, "{v}");
    assert_eq!(v["merges"][0]["result"], "up to date");
    let (bad, v) = tool(&mux, &mut rx, 15, "ide_sync_from_parent", serde_json::json!({})).await;
    assert!(bad && v.as_str().unwrap().contains("no parent"), "{v}");
    let (bad, v) = tool(
        &mux,
        &mut rx,
        16,
        "ide_create_child_session",
        serde_json::json!({"title": "B", "repos": ["r1"], "prompt": "echo first"}),
    )
    .await;
    assert!(!bad, "{v}");
    assert_eq!(v["repositories"].as_array().unwrap().len(), 1);
    let b_id = v["id"].as_str().unwrap().to_owned();
    let (bad, v) = tool(&mux, &mut rx, 17, "ide_create_child_session", serde_json::json!({"repos": ["nope"]})).await;
    assert!(bad && v.as_str().unwrap().contains("no repository"), "{v}");
    // The first prompt is typed in once the child's shell runs (a shell has no activity).
    let ctx = server.authorize(server.user()).unwrap();
    let delivered = wait_until(Duration::from_secs(10), || {
        let mut seen = std::collections::HashMap::new();
        ctx.screen_tick(&mut seen);
        ctx.queued(&b_id) == 0
    })
    .await;
    assert!(delivered, "the child's first prompt was delivered");

    // The shared folder's write lease: the parent holds it; the shared child's edits inside
    // the folder are refused until it asks for the access (the parent's shell is not working).
    let edit_in = |root: &str| ide_proto::AgentHookEvent::PreEdit {
        tool: "Write".into(),
        paths: vec![Path::new(root).join("x.txt").display().to_string()],
    };
    let writer = |id: &str| ctx.agent_hook(id, ide_proto::AgentHookEvent::SessionStart { conversation: None });
    assert_eq!(writer(&p.id), None);
    assert_eq!(ctx.agent_hook(&p.id, edit_in(&p.root)), None, "the owner writes");
    let refused = ctx.agent_hook(&s.id, edit_in(&p.root)).expect("refused");
    assert!(refused.contains("ide_request_write_access"), "{refused}");
    let outside = std::env::temp_dir().join("elsewhere.txt").display().to_string();
    let elsewhere = ide_proto::AgentHookEvent::PreEdit {
        tool: "Write".into(),
        paths: vec![outside],
    };
    assert_eq!(ctx.agent_hook(&s.id, elsewhere), None, "edits outside the folder are not guarded");
    assert_eq!(ctx.agent_hook(&a.id, edit_in(&a.root)), None, "a separate child has its own folder");
    let is_writer = async |id: &str| c.sessions().await.unwrap().into_iter().find(|x| x.id == id).unwrap().writer;
    assert!(is_writer(&p.id).await && !is_writer(&s.id).await);
    let (mux_s, mut rx_s) = agent(&server, &s.id).await;
    // An edit hook means the parent's turn runs: it ends before the child asks.
    ctx.agent_hook(&p.id, ide_proto::AgentHookEvent::Stopped);
    let (bad, v) = tool(&mux_s, &mut rx_s, 20, "ide_request_write_access", serde_json::json!({})).await;
    assert!(!bad && v["result"].as_str().unwrap().starts_with("granted"), "{v}");
    assert_eq!(ctx.agent_hook(&s.id, edit_in(&p.root)), None);
    assert!(ctx.agent_hook(&p.id, edit_in(&p.root)).is_some(), "now the owner waits");
    assert!(is_writer(&s.id).await && !is_writer(&p.id).await);
    let (bad, v) = tool(&mux_s, &mut rx_s, 21, "ide_release_write_access", serde_json::json!({})).await;
    assert!(!bad && v["result"].as_str().unwrap().starts_with("released"), "{v}");
    assert_eq!(ctx.agent_hook(&p.id, edit_in(&p.root)), None);
    // A busy holder keeps it.
    ctx.agent_hook(&p.id, ide_proto::AgentHookEvent::Stopped);
    let (bad, _) = tool(&mux_s, &mut rx_s, 22, "ide_request_write_access", serde_json::json!({})).await;
    assert!(!bad);
    ctx.agent_hook(&s.id, ide_proto::AgentHookEvent::PromptSubmitted { prompt: None });
    let (bad, v) = tool(&mux, &mut rx, 23, "ide_request_write_access", serde_json::json!({})).await;
    assert!(bad && v.as_str().unwrap().contains("is working"), "{v}");
    ctx.agent_hook(&s.id, ide_proto::AgentHookEvent::Stopped);
    let (bad, _) = tool(&mux, &mut rx, 24, "ide_request_write_access", serde_json::json!({})).await;
    assert!(!bad, "the parent takes it back once the child is idle");
    // A separate child's folder is not shared.
    let (mux_a, mut rx_a) = agent(&server, &a.id).await;
    let (bad, v) = tool(&mux_a, &mut rx_a, 25, "ide_request_write_access", serde_json::json!({})).await;
    assert!(!bad && v["result"].as_str().unwrap().contains("not shared"), "{v}");

    // Children first, then the parent; a shared child's delete keeps the parent's folder.
    c.delete_session(&s.id, true).await.unwrap();
    assert!(pw.is_dir(), "the shared folder stays");
    for id in [&g.id, &a.id, &b_id] {
        c.delete_session(id, true).await.unwrap();
    }
    c.delete_session(&p.id, true).await.unwrap();
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `ide_send_to_session` modes, `ide_sent_messages`, `ide_read_session` and the approvals
/// (`docs/approvals.md`) of `steer` / `interrupt`, answered by an app.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_modes_and_approvals() {
    use ide_client::{ClientEvent, Notify};
    use ide_proto::AgentHookEvent;

    let dir = tmp_dir("family-send");
    let r1 = dir.join("r1");
    init_repo(&r1);
    let server = bridged(&dir.join("data"));
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    let id1 = c.register_repo(&r1.display().to_string()).await.unwrap().id;
    let (p, _) = c.create_session(shell_session(vec![id1.clone()], true)).await.unwrap();
    let (s, _) = c.create_child_session(&p.id, true, Vec::new(), "S", None, true, 80, 24).await.unwrap();
    let ctx = server.authorize(server.user()).unwrap();
    let (mux, mut rx) = agent(&server, &p.id).await;

    // The app answers approval questions from a script of answers; it counts them.
    let answers = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<Result<String, String>>::new()));
    let asked = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let ended = Arc::new(std::sync::Mutex::new(Vec::<(u64, bool)>::new()));
    let mut events = c.events().unwrap();
    let (c2, answers2, asked2, ended2) = (c.clone(), answers.clone(), asked.clone(), ended.clone());
    tokio::spawn(async move {
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::AgentRequestEnded { request, timed_out }) = &e {
                ended2.lock().unwrap().push((*request, *timed_out));
            }
            if let ClientEvent::Notify(Notify::AgentRequest { request, tool, args, .. }) = e
                && tool == ide_proto::APPROVAL_REQUEST
            {
                asked2.lock().unwrap().push(serde_json::from_str(&args).unwrap());
                let a = answers2.lock().unwrap().pop_front().unwrap_or(Err("no answer".into()));
                let _ = c2.agent_reply(request, a).await;
            }
        }
    });
    let busy = |id: &str| ctx.agent_hook(id, AgentHookEvent::PromptSubmitted { prompt: None });
    let idle = |id: &str| ctx.agent_hook(id, AgentHookEvent::Stopped);

    // Reading: a shell keeps no conversation; only related sessions.
    let (bad, v) = tool(&mux, &mut rx, 10, "ide_read_session", serde_json::json!({"session": s.id})).await;
    assert!(bad && v.as_str().unwrap().contains("no recorded conversation"), "{v}");
    let (bad, v) = tool(&mux, &mut rx, 11, "ide_read_session", serde_json::json!({"session": p.id})).await;
    assert!(bad && v.as_str().unwrap().contains("not a related session"), "{v}");

    // Queue while busy: waiting, listed, cancelled.
    busy(&s.id);
    let (bad, v) = tool(&mux, &mut rx, 12, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "echo q"})).await;
    assert!(!bad && v["delivered"] == false, "{v}");
    let mid = v["message_id"].as_u64().unwrap();
    let (_, v) = tool(&mux, &mut rx, 13, "ide_sent_messages", serde_json::json!({})).await;
    assert_eq!(v["waiting"][0]["id"].as_u64(), Some(mid), "{v}");
    assert_eq!(v["waiting"][0]["to_title"], "S");
    let (bad, _) = tool(&mux, &mut rx, 14, "ide_sent_messages", serde_json::json!({"action": "cancel", "id": mid})).await;
    assert!(!bad);
    assert_eq!(ctx.queued(&s.id), 0);
    let (bad, _) = tool(&mux, &mut rx, 15, "ide_sent_messages", serde_json::json!({"action": "cancel", "id": mid})).await;
    assert!(bad, "already gone");
    let (bad, v) = tool(&mux, &mut rx, 16, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "x", "mode": "shout"})).await;
    assert!(bad && v.as_str().unwrap().contains("unknown mode"), "{v}");

    // Steer while busy: asked; refused -> nothing done.
    answers.lock().unwrap().push_back(Err("refused".into()));
    let (bad, v) = tool(&mux, &mut rx, 17, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "now", "mode": "steer"})).await;
    assert!(bad && v.as_str().unwrap().contains("did not allow"), "{v}");
    {
        let a = asked.lock().unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!((a[0]["capability"].as_str(), a[0]["target"].as_str()), (Some("session.steer"), Some(s.id.as_str())));
        assert_eq!((a[0]["requester_title"].as_str(), a[0]["target_title"].as_str(), a[0]["detail"].as_str()), (Some("P"), Some("S"), Some("now")));
    }
    // The apps were told the question is over (not timed out).
    for _ in 0..100 {
        if !ended.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(ended.lock().unwrap().iter().any(|&(_, t)| !t));
    assert_eq!(ctx.queued(&s.id), 0);
    // Allowed for the session: kept; a shell cannot take messages during a turn, though.
    answers.lock().unwrap().push_back(Ok("session".into()));
    let (bad, v) = tool(&mux, &mut rx, 18, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "now", "mode": "steer"})).await;
    assert!(bad && v.as_str().unwrap().contains("cannot take messages"), "{v}");
    let (bad, _) = tool(&mux, &mut rx, 19, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "now", "mode": "steer"})).await;
    assert!(bad);
    assert_eq!(asked.lock().unwrap().len(), 2, "the kept grant answers the third call");
    let list = c.approval_list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].capability.as_str(), list[0].requester_title.as_str(), list[0].target_title.as_str(), list[0].always), ("session.steer", "P", "S", false));
    // Interrupt: its own capability; a shell has no interrupt keys.
    answers.lock().unwrap().push_back(Ok("always".into()));
    let (bad, v) = tool(&mux, &mut rx, 20, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "stop", "mode": "interrupt"})).await;
    assert!(bad && v.as_str().unwrap().contains("cannot interrupt"), "{v}");
    assert_eq!(c.approval_list().await.unwrap().len(), 2);
    assert!(std::fs::read_to_string(dir.join("data").join("approvals.json")).unwrap().contains("session.interrupt"));

    // How long questions wait: 5 minutes unless changed; kept with the grants.
    assert_eq!(c.approval_wait().await.unwrap(), Some(300));
    c.approval_set_wait(None).await.unwrap();
    assert_eq!(c.approval_wait().await.unwrap(), None);
    c.approval_set_wait(Some(5)).await.unwrap();
    assert_eq!(c.approval_wait().await.unwrap(), Some(30), "at least 30 s");
    c.approval_set_wait(Some(3600)).await.unwrap();
    assert!(std::fs::read_to_string(dir.join("data").join("approvals.json")).unwrap().contains("\"wait_secs\": 3600"));

    // Revoke.
    let steer = c.approval_list().await.unwrap().into_iter().find(|g| g.capability == "session.steer").unwrap();
    c.approval_revoke(&steer.id).await.unwrap();
    assert_eq!(c.approval_revoke(&steer.id).await.unwrap_err().id, "approval-not-found");

    // Not in a turn: steer acts like queue, nothing is asked.
    idle(&s.id);
    let n = asked.lock().unwrap().len();
    let (bad, v) = tool(&mux, &mut rx, 21, "ide_send_to_session", serde_json::json!({"session": s.id, "message": "echo idle", "mode": "steer"})).await;
    assert!(!bad && v["delivered"] == true, "{v}");
    assert_eq!(asked.lock().unwrap().len(), n);

    // A deleted session takes the grants on it along.
    c.delete_session(&s.id, true).await.unwrap();
    assert!(c.approval_list().await.unwrap().is_empty());
    assert!(!std::fs::read_to_string(dir.join("data").join("approvals.json")).unwrap().contains("session.interrupt"));
    c.delete_session(&p.id, true).await.unwrap();
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A child with another agent than its parent's: through the app's request and the bridge tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_with_another_agent() {
    let dir = tmp_dir("family-agent");
    let r1 = dir.join("r1");
    init_repo(&r1);
    let server = bridged(&dir.join("data"));
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    let id1 = c.register_repo(&r1.display().to_string()).await.unwrap().id;
    let (p, _) = c.create_session(shell_session(vec![id1.clone()], false)).await.unwrap();
    let review = c.create_configuration("Review").await.unwrap();
    assert_ne!(review.slug, p.configuration);

    // `None`: the parent's agent.
    let (same, _) = c.create_child_session(&p.id, true, Vec::new(), "same", None, false, 80, 24).await.unwrap();
    assert_eq!((same.provider.as_str(), same.configuration.as_str()), ("shell", p.configuration.as_str()));
    // Another configuration.
    let other = ide_client::ChildAgent {
        provider: "shell".into(),
        account: None,
        configuration: review.slug.clone(),
    };
    let (o, _) = c.create_child_session(&p.id, true, Vec::new(), "other", Some(other), false, 80, 24).await.unwrap();
    assert_eq!(o.configuration, review.slug);
    // Unknown agents and configurations are refused.
    let bad = |provider: &str, configuration: &str| ide_client::ChildAgent {
        provider: provider.into(),
        account: None,
        configuration: configuration.into(),
    };
    let e = c.create_child_session(&p.id, true, Vec::new(), "", Some(bad("nope", "default")), false, 80, 24).await.unwrap_err();
    assert_eq!(e.id, "provider-not-found");
    let e = c.create_child_session(&p.id, true, Vec::new(), "", Some(bad("shell", "nope")), false, 80, 24).await.unwrap_err();
    assert_ne!(e.id, "", "{e:?}");

    // The bridge tool: `agent` / `profile` by id or name.
    let (mux, mut rx) = agent(&server, &p.id).await;
    let (bad_call, v) = tool(&mux, &mut rx, 10, "ide_create_child_session", serde_json::json!({"shared": true, "profile": "review"})).await;
    assert!(!bad_call, "{v}");
    assert_eq!((v["agent"].as_str(), v["profile"].as_str()), (Some("shell"), Some(review.slug.as_str())));
    let (bad_call, v) = tool(&mux, &mut rx, 11, "ide_create_child_session", serde_json::json!({"shared": true, "agent": "nope"})).await;
    assert!(bad_call && v.as_str().unwrap().contains("no agent"), "{v}");
    let (bad_call, v) = tool(&mux, &mut rx, 12, "ide_create_child_session", serde_json::json!({"shared": true, "profile": "nope"})).await;
    assert!(bad_call && v.as_str().unwrap().contains("no profile"), "{v}");

    for s in c.sessions().await.unwrap().iter().filter(|s| s.parent.as_deref() == Some(p.id.as_str())) {
        c.delete_session(&s.id, true).await.unwrap();
    }
    c.delete_session(&p.id, true).await.unwrap();
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Session handoff (`docs/session-handoff.md`) with shell sessions: the refusals, a child's
/// message held while the conversation is replaced, cancelling, and a `Previous` handoff whose
/// document the test writes in the old agent's place. The session keeps its id and children.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handoff() {
    use ide_client::{HandoffMethod, HandoffPhase};
    let dir = tmp_dir("family-handoff");
    let r1 = dir.join("r1");
    init_repo(&r1);
    let data = dir.join("data");
    let server = bridged(&data);
    let c = Client::connect(memory(&server), opts()).await.unwrap();
    let id1 = c.register_repo(&r1.display().to_string()).await.unwrap().id;
    let (p, _) = c.create_session(shell_session(vec![id1.clone()], true)).await.unwrap();
    let (a, _) = c.create_child_session(&p.id, false, Vec::new(), "A", None, false, 80, 24).await.unwrap();

    // A shell records no conversation: nothing to summarize (`Auto` would ask the running
    // agent itself).
    for m in [HandoffMethod::Summarize, HandoffMethod::Transcript] {
        assert_eq!(c.session_handoff(&p.id, m, None).await.unwrap_err().id, "handoff-no-conversation", "{m:?}");
    }
    let bad = ide_client::ChildAgent {
        provider: "nope".into(),
        account: None,
        configuration: "default".into(),
    };
    assert_eq!(
        c.session_handoff(&p.id, HandoffMethod::Previous, Some(bad)).await.unwrap_err().id,
        "provider-not-found"
    );
    assert_eq!(c.session_handoff_cancel(&p.id).await.unwrap_err().id, "handoff-none");

    // The running agent is asked to write the document.
    let info = c.session_handoff(&p.id, HandoffMethod::Previous, None).await.unwrap();
    assert_eq!(info.handoff.map(|h| h.method), Some(HandoffMethod::Previous));
    assert_eq!(c.session_handoff(&p.id, HandoffMethod::Previous, None).await.unwrap_err().id, "handoff-busy");
    c.session_handoff_cancel(&p.id).await.unwrap();
    let now = c.sessions().await.unwrap().into_iter().find(|s| s.id == p.id).unwrap();
    assert!(now.handoff.is_none());

    // Again, through to the new conversation.
    let t0 = ide_core::now_unix();
    c.session_handoff(&p.id, HandoffMethod::Previous, None).await.unwrap();
    let t1 = ide_core::now_unix();
    // A child's message waits for the new conversation.
    let (mux, mut rx) = agent(&server, &a.id).await;
    let (bad_call, v) = tool(&mux, &mut rx, 10, "ide_send_to_session", serde_json::json!({"session": p.id, "message": "echo done"})).await;
    assert!(!bad_call && v["delivered"] == false, "{v}");
    assert!(v["note"].as_str().unwrap().contains("handed off"), "{v}");
    let phase = || async {
        c.sessions().await.unwrap().into_iter().find(|s| s.id == p.id).unwrap().handoff.map(|h| h.phase)
    };
    let mut waited = 0;
    while phase().await != Some(HandoffPhase::Writing) {
        assert!(waited < 100, "the request was not typed in");
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    let hdir = data.join("sessions").join(&p.id).join("handoff");
    for at in t0..=t1 {
        std::fs::write(hdir.join(format!("{at}-handoff.md")), "# Handoff\n").unwrap();
    }
    let mut waited = 0;
    while phase().await.is_some() {
        assert!(waited < 300, "the handoff did not finish");
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    let all = c.sessions().await.unwrap();
    let now = all.iter().find(|s| s.id == p.id).unwrap();
    assert_eq!(now.state, ide_client::SessionState::Running);
    assert_eq!(all.iter().find(|s| s.id == a.id).unwrap().parent.as_deref(), Some(p.id.as_str()));
    let store = ide_core::SessionStore::open(&Paths::new(&data)).unwrap();
    let stored = store.get(&ide_core::SessionId::parse(&p.id).unwrap()).unwrap();
    assert_eq!(stored.previous_conversations.len(), 1);
    assert_eq!(stored.previous_conversations[0].provider, "shell");

    drop((mux, rx));
    c.delete_session(&a.id, true).await.unwrap();
    c.delete_session(&p.id, true).await.unwrap();
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
