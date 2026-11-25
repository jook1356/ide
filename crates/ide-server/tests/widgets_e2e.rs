//! Widgets (`docs/widgets.md`) without any agent CLI: the agent drafts widgets in its folder
//! and registers them (a new one asks the user through the app); an app lists the registered
//! widgets, reads their files (only inside the copy), calls tools, keeps data and open tabs.
//!
//! Run with `IDE_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::Path;

use common::*;
use ide_client::{Client, ClientEvent, ClientOptions, NewSession, Notify, WidgetDataOp};

fn opts() -> ClientOptions {
    ClientOptions {
        reconnect: false,
        ..ClientOptions::default()
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn widgets_end_to_end() {
    let dir = tmp_dir("widgets");
    let repo = dir.join("repo");
    init_repo(&repo);
    let server = server(&dir.join("data"));
    let app = Client::connect(memory(&server), opts()).await.unwrap();
    let repo_id = app.register_repo(&repo.display().to_string()).await.unwrap().id;
    let (s, _) = app
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: Vec::new(),
            original_repo: Some(repo_id),
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: String::new(),
            start: false,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    let sid = s.id.as_str();
    // The session folder of an original-repo session is the repository.
    let widgets = repo.join(".ide").join("widgets");
    write(
        &widgets.join("board").join("widget.json"),
        r#"{"name":"Board","description":"Tasks","tools":["ide_session"],"scope":"family"}"#,
    );
    write(&widgets.join("board").join("index.html"), "<html><head></head><body>hi</body></html>");
    write(&widgets.join("board").join("js").join("app.js"), "console.log(1)");
    write(&widgets.join("broken").join("widget.json"), "{");
    write(&widgets.join("ghost").join("widget.json"), r#"{"name":"G","tools":["ide_nope"]}"#);
    write(&widgets.join("ghost").join("index.html"), "");
    write(&repo.join("secret.txt"), "no");

    // Drafts: the agent's list shows them with their problems; nothing is registered yet.
    let ctx = server.authorize(server.user()).unwrap();
    let call = |args: &'static str| {
        let ctx = ctx.clone();
        let sid = sid.to_owned();
        async move { ctx.agent_call(&sid, "ide_widget", args).await }
    };
    let r = call(r#"{"action":"guide"}"#).await;
    assert!(!r.is_error && r.text.contains("widget.json") && r.text.contains("register"), "{}", r.text);
    let r = call(r#"{"action":"list"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["drafts"][0]["id"], "board");
    assert!(v["drafts"][0]["problem"].is_null());
    assert!(v["drafts"][1]["problem"].as_str().unwrap().contains("not valid"));
    assert!(v["drafts"][2]["problem"].as_str().unwrap().contains("ide_nope"));
    assert_eq!(v["registered"].as_array().unwrap().len(), 0);
    assert!(app.widget_list().await.unwrap().is_empty());
    assert_eq!(app.widget_file(sid, "board", "").await.unwrap_err().id, "widget");
    for (args, needle) in [
        (r#"{"action":"register","widget":"broken"}"#, "not valid"),
        (r#"{"action":"register","widget":"ghost"}"#, "ide_nope"),
        (r#"{"action":"register"}"#, "`widget`"),
        (r#"{"action":"open","widget":"board"}"#, "not registered"),
        (r#"{"action":"dance"}"#, "unknown action"),
    ] {
        let r = ctx.agent_call(sid, "ide_widget", args).await;
        assert!(r.is_error && r.text.contains(needle), "{args}: {}", r.text);
    }

    // Registering a new widget asks the user through the app; the app registers what it showed.
    let mut events = app.events().unwrap();
    let app2 = app.clone();
    let sid2 = s.id.clone();
    let answering = tokio::spawn(async move {
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::AgentRequest { request, session, tool, args }) = e {
                assert_eq!((session.as_str(), tool.as_str()), (sid2.as_str(), "ide_widget"));
                let v: serde_json::Value = serde_json::from_str(&args).unwrap();
                match v["action"].as_str() {
                    Some("register") => {
                        assert_eq!((v["widget"].as_str(), v["ask"].as_str()), (Some("board"), Some("new")));
                        assert_eq!(v["tools"], serde_json::json!(["ide_session"]));
                        let e = app2
                            .widget_register(&sid2, "board", vec!["ide_session".into(), "ide_show_message".into()])
                            .await
                            .unwrap_err();
                        assert_eq!(e.id, "widget-changed");
                        app2.widget_register(&sid2, "board", vec!["ide_session".into()]).await.unwrap();
                        app2.agent_reply(request, Ok("registered".into())).await.unwrap();
                    }
                    Some("open") => {
                        assert_eq!(v["widget"].as_str(), Some("board"));
                        app2.agent_reply(request, Ok("opened".into())).await.unwrap();
                        return;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    });
    let r = call(r#"{"action":"register","widget":"board"}"#).await;
    assert_eq!((r.is_error, r.text.as_str()), (false, "registered"));
    let r = call(r#"{"action":"open","widget":"board"}"#).await;
    assert_eq!((r.is_error, r.text.as_str()), (false, "opened"));
    answering.await.unwrap();
    let list = app.widget_list().await.unwrap();
    assert_eq!(list.len(), 1);
    let board = &list[0];
    assert_eq!((board.id.as_str(), board.name.as_str(), board.scope.as_str()), ("board", "Board", "family"));
    assert_eq!((board.version, board.created_by.as_str()), (1, sid));
    assert_eq!(board.tools, ["ide_session"]);

    // Files come from the registered copy: below the widget only, the draft not before an update.
    let (mime, data) = app.widget_file(sid, "board", "").await.unwrap();
    assert_eq!((mime.as_str(), data.as_slice()), ("text/html; charset=utf-8", &b"<html><head></head><body>hi</body></html>"[..]));
    let (mime, _) = app.widget_file(sid, "board", "js/app.js").await.unwrap();
    assert_eq!(mime, "text/javascript; charset=utf-8");
    for bad in ["../../secret.txt", "../broken/widget.json", "nope.js"] {
        let e = app.widget_file(sid, "board", bad).await.unwrap_err();
        assert_eq!(e.id, "widget", "{bad}");
    }
    assert_eq!(app.widget_file("zzz", "board", "").await.unwrap_err().id, "session-not-found");
    write(&widgets.join("board").join("index.html"), "v2");
    assert_ne!(app.widget_file(sid, "board", "").await.unwrap().1, b"v2");
    // Updating your own widget with the same tools does not ask.
    let r = call(r#"{"action":"register","widget":"board"}"#).await;
    assert!(!r.is_error && r.text.contains("\"version\": 2"), "{}", r.text);
    assert_eq!(app.widget_file(sid, "board", "").await.unwrap().1, b"v2");

    // Tools: only the ones allowed at registration.
    let r = app.widget_call(sid, "board", "ide_session", "{}").await.unwrap();
    assert!(!r.is_error, "{}", r.text);
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["id"], sid);
    let e = app.widget_call(sid, "board", "ide_show_message", "{}").await.unwrap_err();
    assert_eq!(e.id, "widget-not-allowed");
    assert_eq!(app.widget_call(sid, "nope", "ide_session", "{}").await.unwrap_err().id, "widget");

    // Data.
    let get = |k: &str| WidgetDataOp::Get { key: k.into() };
    assert_eq!(app.widget_data(sid, "board", get("todo")).await.unwrap(), None);
    app.widget_data(sid, "board", WidgetDataOp::Set { key: "todo".into(), value: r#"["a","b"]"#.into() })
        .await
        .unwrap();
    app.widget_data(sid, "board", WidgetDataOp::Set { key: "n".into(), value: "3".into() })
        .await
        .unwrap();
    assert_eq!(app.widget_data(sid, "board", get("todo")).await.unwrap().as_deref(), Some(r#"["a","b"]"#));
    assert_eq!(app.widget_data(sid, "board", WidgetDataOp::Keys).await.unwrap().as_deref(), Some(r#"["n","todo"]"#));
    let e = app
        .widget_data(sid, "board", WidgetDataOp::Set { key: "x".into(), value: "not json".into() })
        .await
        .unwrap_err();
    assert_eq!(e.id, "widget");
    app.widget_data(sid, "board", WidgetDataOp::Delete { key: "n".into() }).await.unwrap();
    assert_eq!(app.widget_data(sid, "board", WidgetDataOp::Keys).await.unwrap().as_deref(), Some(r#"["todo"]"#));
    // Family scope: kept under the family's root session (this session has no parent).
    assert!(dir.join("data").join("sessions").join(sid).join("widgets-family").join("board.json").is_file());

    // Open tabs are remembered.
    app.widget_set_open(sid, "board", true).await.unwrap();
    assert_eq!(app.widget_list().await.unwrap()[0].open, [sid]);
    app.widget_set_open(sid, "board", false).await.unwrap();
    assert!(app.widget_list().await.unwrap()[0].open.is_empty());
    app.widget_set_open(sid, "board", true).await.unwrap();

    // Unregister: by the agent of the registering session (or the user, `widget.delete`).
    let r = call(r#"{"action":"unregister","widget":"board"}"#).await;
    assert!(!r.is_error, "{}", r.text);
    assert!(app.widget_list().await.unwrap().is_empty());
    assert_eq!(app.widget_file(sid, "board", "").await.unwrap_err().id, "widget");
    assert_eq!(app.widget_delete("board").await.unwrap_err().id, "widget");

    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The host API (`widget.host`): files, programs, terminals and sessions for widgets that
/// declare them, and the user's block policy (`widget.set_blocked`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn widget_host_api() {
    let dir = tmp_dir("widget-host");
    let repo = dir.join("repo");
    init_repo(&repo);
    let server = server(&dir.join("data"));
    let app = Client::connect(memory(&server), opts()).await.unwrap();
    let repo_id = app.register_repo(&repo.display().to_string()).await.unwrap().id;
    let (s, _) = app
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: Vec::new(),
            original_repo: Some(repo_id),
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: String::new(),
            start: false,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    let sid = s.id.as_str();
    let widgets = repo.join(".ide").join("widgets");
    write(
        &widgets.join("ops").join("widget.json"),
        r#"{"name":"Ops","tools":["exec","fs","terminal","sessions"],"scope":"global"}"#,
    );
    write(&widgets.join("ops").join("index.html"), "<p>ops</p>");
    write(&widgets.join("plain").join("widget.json"), r#"{"name":"Plain"}"#);
    write(&widgets.join("plain").join("index.html"), "<p>plain</p>");
    let tools = |t: &[&str]| t.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    app.widget_register(sid, "ops", tools(&["exec", "fs", "terminal", "sessions"])).await.unwrap();
    app.widget_register(sid, "plain", Vec::new()).await.unwrap();
    let host = |method: &'static str, args: serde_json::Value| {
        let app = app.clone();
        let sid = sid.to_owned();
        async move {
            let r = app.widget_host(&sid, "ops", method, &args.to_string()).await.unwrap();
            assert!(!r.is_error, "{method}: {}", r.text);
            serde_json::from_str::<serde_json::Value>(&r.text).unwrap()
        }
    };

    // Files: relative paths are under the session folder.
    host("fs.write", serde_json::json!({ "path": "out/state.json", "text": "{\"a\":1}" })).await;
    assert_eq!(std::fs::read_to_string(repo.join("out").join("state.json")).unwrap(), "{\"a\":1}");
    assert_eq!(host("fs.read", serde_json::json!({ "path": "out/state.json" })).await["text"], "{\"a\":1}");
    // Programs: in the session folder by default.
    let v = host("exec", serde_json::json!({ "command": "echo widget-exec" })).await;
    assert_eq!((v["code"].as_i64(), v["stdout"].as_str().map(str::trim)), (Some(0), Some("widget-exec")));
    // Sessions.
    let v = host("sessions.list", serde_json::json!({})).await;
    assert!(v["sessions"].as_array().unwrap().iter().any(|x| x["id"] == sid), "{v}");
    assert_eq!(host("sessions.get", serde_json::json!({})).await["id"], sid);
    // Terminals: a command in a chosen folder, listed, read, removed.
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    let t = host(
        "term.open",
        serde_json::json!({ "command": "echo from-term", "cwd": "sub", "title": "probe" }),
    )
    .await;
    let id = t["id"].as_str().unwrap().to_owned();
    assert!(t["folder"].as_str().unwrap().ends_with("sub"), "{t}");
    let v = host("term.list", serde_json::json!({ "session": "*" })).await;
    assert!(v["terminals"].as_array().unwrap().iter().any(|x| x["id"] == id.as_str()), "{v}");
    let mut seen = String::new();
    for _ in 0..100 {
        seen = host("term.read", serde_json::json!({ "id": id })).await["text"].as_str().unwrap().to_owned();
        if seen.contains("from-term") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(seen.contains("from-term"), "{seen}");
    host("term.remove", serde_json::json!({ "id": id })).await;
    let v = host("term.list", serde_json::json!({})).await;
    assert!(v["terminals"].as_array().unwrap().is_empty(), "{v}");
    // Errors of a call come back as an error result; unknown methods too.
    let r = app.widget_host(sid, "ops", "fs.read", r#"{"path":"missing.txt"}"#).await.unwrap();
    assert!(r.is_error);
    let r = app.widget_host(sid, "ops", "rm.rf", "{}").await.unwrap();
    assert!(r.is_error && r.text.contains("unknown method"), "{}", r.text);

    // Undeclared features are refused; blocked ones too, until unblocked.
    let e = app.widget_host(sid, "plain", "exec", r#"{"command":"echo x"}"#).await.unwrap_err();
    assert_eq!(e.id, "widget-not-allowed");
    app.widget_set_blocked("ops", "exec", true).await.unwrap();
    assert_eq!(app.widget_list().await.unwrap().iter().find(|w| w.id == "ops").unwrap().blocked, ["exec"]);
    let e = app.widget_host(sid, "ops", "exec", r#"{"command":"echo x"}"#).await.unwrap_err();
    assert_eq!(e.id, "widget-blocked");
    host("fs.stat", serde_json::json!({ "path": "out" })).await;
    app.widget_set_blocked("ops", "*", true).await.unwrap();
    let e = app.widget_host(sid, "ops", "fs.stat", r#"{"path":"out"}"#).await.unwrap_err();
    assert_eq!(e.id, "widget-blocked");
    app.widget_set_blocked("ops", "*", false).await.unwrap();
    app.widget_set_blocked("ops", "exec", false).await.unwrap();
    host("exec", serde_json::json!({ "command": "echo again" })).await;
    assert_eq!(app.widget_set_blocked("ops", "ide_nope", true).await.unwrap_err().id, "widget");
    // Deleting the widget forgets its blocks.
    app.widget_set_blocked("ops", "fs", true).await.unwrap();
    app.widget_delete("ops").await.unwrap();
    assert!(!std::fs::read_to_string(dir.join("data").join("widgets").join("policy.json")).unwrap().contains("ops"));

    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}
