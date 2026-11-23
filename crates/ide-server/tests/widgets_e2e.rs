//! Widgets (`docs/widgets.md`) without any agent CLI: an app lists a session's widgets, reads
//! their files (only inside the widget), approves their tools, calls tools and keeps data;
//! the agent's `ide_widget` tool answers `guide` / `list` and hands `open` to the app.
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

    // List: usable widgets and the problems of the others.
    let list = app.widget_list(sid).await.unwrap();
    let ids: Vec<&str> = list.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["board", "broken", "ghost"]);
    let board = &list[0];
    assert_eq!((board.name.as_str(), board.scope.as_str()), ("Board", "family"));
    assert_eq!((board.granted, board.error.as_deref()), (false, None));
    assert!(list[1].error.as_deref().unwrap().contains("not valid"), "{:?}", list[1].error);
    assert!(list[2].error.as_deref().unwrap().contains("ide_nope"), "{:?}", list[2].error);

    // Files: the entry page, files below the widget, nothing outside it.
    let (mime, data) = app.widget_file(sid, "board", "").await.unwrap();
    assert_eq!((mime.as_str(), data.as_slice()), ("text/html; charset=utf-8", &b"<html><head></head><body>hi</body></html>"[..]));
    let (mime, _) = app.widget_file(sid, "board", "js/app.js").await.unwrap();
    assert_eq!(mime, "text/javascript; charset=utf-8");
    for bad in ["../../secret.txt", "../broken/widget.json", "nope.js"] {
        let e = app.widget_file(sid, "board", bad).await.unwrap_err();
        assert_eq!(e.id, "widget", "{bad}");
    }
    assert_eq!(app.widget_file(sid, "nope", "").await.unwrap_err().id, "widget");
    assert_eq!(app.widget_file("zzz", "board", "").await.unwrap_err().id, "session-not-found");

    // Tools: only declared ones, only after the user approved exactly that list.
    let e = app.widget_call(sid, "board", "ide_session", "{}").await.unwrap_err();
    assert_eq!(e.id, "widget-not-allowed");
    let e = app.widget_grant(sid, "board", vec!["ide_session".into(), "ide_show_message".into()]).await.unwrap_err();
    assert_eq!(e.id, "widget-changed");
    app.widget_grant(sid, "board", vec!["ide_session".into()]).await.unwrap();
    assert!(app.widget_list(sid).await.unwrap()[0].granted);
    let r = app.widget_call(sid, "board", "ide_session", "{}").await.unwrap();
    assert!(!r.is_error, "{}", r.text);
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["id"], sid);
    let e = app.widget_call(sid, "board", "ide_show_message", "{}").await.unwrap_err();
    assert_eq!(e.id, "widget-not-allowed");
    // A changed tool list needs a new approval.
    write(
        &widgets.join("board").join("widget.json"),
        r#"{"name":"Board","tools":["ide_session","ide_list_related_sessions"],"scope":"family"}"#,
    );
    assert!(!app.widget_list(sid).await.unwrap()[0].granted);
    let e = app.widget_call(sid, "board", "ide_session", "{}").await.unwrap_err();
    assert_eq!(e.id, "widget-not-allowed");
    app.widget_grant(sid, "board", vec!["ide_list_related_sessions".into(), "ide_session".into()])
        .await
        .unwrap();
    assert!(!app.widget_call(sid, "board", "ide_session", "{}").await.unwrap().is_error);

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

    // The agent's tool.
    let ctx = server.authorize(server.user()).unwrap();
    let r = ctx.agent_call(sid, "ide_widget", r#"{"action":"guide"}"#).await;
    assert!(!r.is_error && r.text.contains("widget.json") && r.text.contains(".ide"), "{}", r.text);
    let r = ctx.agent_call(sid, "ide_widget", r#"{"action":"list"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["widgets"][0]["id"], "board");
    assert_eq!(v["widgets"][0]["approved_by_user"], true);
    assert!(v["widgets"][1]["problem"].is_string());
    let r = ctx.agent_call(sid, "ide_widget", r#"{"action":"open","widget":"broken"}"#).await;
    assert!(r.is_error && r.text.contains("broken"), "{}", r.text);
    let r = ctx.agent_call(sid, "ide_widget", r#"{"action":"open"}"#).await;
    assert!(r.is_error && r.text.contains("`widget`"), "{}", r.text);
    let r = ctx.agent_call(sid, "ide_widget", r#"{"action":"dance"}"#).await;
    assert!(r.is_error, "{}", r.text);

    // `open` goes to the app.
    let mut events = app.events().unwrap();
    let app2 = app.clone();
    let sid2 = s.id.clone();
    let answering = tokio::spawn(async move {
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::AgentRequest { request, session, tool, args }) = e {
                assert_eq!((session.as_str(), tool.as_str()), (sid2.as_str(), "ide_widget"));
                let v: serde_json::Value = serde_json::from_str(&args).unwrap();
                assert_eq!((v["action"].as_str(), v["widget"].as_str()), (Some("open"), Some("board")));
                app2.agent_reply(request, Ok("opened".into())).await.unwrap();
                return;
            }
        }
    });
    let r = ctx.agent_call(sid, "ide_widget", r#"{"action":"open","widget":"board"}"#).await;
    assert_eq!((r.is_error, r.text.as_str()), (false, "opened"));
    answering.await.unwrap();

    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}
