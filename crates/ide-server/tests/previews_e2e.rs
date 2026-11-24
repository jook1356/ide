//! Previews (`docs/previews.md`) without any agent CLI: the agent's `ide_preview` stores HTML /
//! Markdown text and points at files of its session folder or web addresses; an app opens the
//! tab (it answers the `Notify::AgentRequest`) and reads the page and its files through
//! `preview.file`, only inside the session folder.
//!
//! Run with `IDE_TEST_TMP=<dir outside any git repository>`.

mod common;

use std::path::Path;

use common::*;
use ide_client::{Client, ClientEvent, ClientOptions, NewSession, Notify};

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
async fn previews_end_to_end() {
    let dir = tmp_dir("previews");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let server = server(&data);
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
    let sid = s.id.as_str().to_owned();
    // The session folder of an original-repo session is the repository.
    write(&repo.join("site").join("index.html"), "<link href=\"css/a.css\">");
    write(&repo.join("site").join("css").join("a.css"), "p{}");
    write(&repo.join("docs").join("README.md"), "# Read me\n\n- [ ] todo\n");
    write(&repo.join("shot.png"), "PNG");
    write(&repo.join("src.rs"), "fn main() {}");
    write(&dir.join("outside.html"), "secret");

    let ctx = server.authorize(server.user()).unwrap();
    let call = |args: serde_json::Value| {
        let ctx = ctx.clone();
        let sid = sid.clone();
        async move { ctx.agent_call(&sid, "ide_preview", &args.to_string()).await }
    };

    // Answered by the daemon: the guide, the list, refused arguments (nothing reaches an app).
    let r = call(serde_json::json!({"action": "guide"})).await;
    assert!(!r.is_error && r.text.contains("ide-preview://") && r.text.contains("`file`"), "{}", r.text);
    let r = call(serde_json::json!({"action": "list"})).await;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&r.text).unwrap()["previews"], serde_json::json!([]));
    for (args, needle) in [
        (serde_json::json!({"title": "x", "kind": "pdf"}), "`kind`"),
        (serde_json::json!({"title": "x", "kind": "html"}), "`content`"),
        (serde_json::json!({"title": " ", "kind": "html", "content": ""}), "`title`"),
        (serde_json::json!({"title": "x", "kind": "html", "content": "", "id": "Bad Id"}), "not valid"),
        (serde_json::json!({"title": "x", "kind": "file", "path": "../outside.html"}), "inside the session folder"),
        (serde_json::json!({"title": "x", "kind": "file", "path": dir.join("outside.html")}), "inside the session folder"),
        (serde_json::json!({"title": "x", "kind": "file", "path": "src.rs"}), "a file preview shows"),
        (serde_json::json!({"title": "x", "kind": "url", "url": "javascript:alert(1)"}), "http"),
        (serde_json::json!({"action": "close"}), "`id`"),
        (serde_json::json!({"action": "dance"}), "unknown action"),
    ] {
        let r = call(args.clone()).await;
        assert!(r.is_error && r.text.contains(needle), "{args}: {}", r.text);
    }

    // The app is asked to show (or close) the tab, and answers.
    let mut events = app.events().unwrap();
    let app2 = app.clone();
    let sid2 = sid.clone();
    let answering = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(e) = events.next().await {
            if let ClientEvent::Notify(Notify::AgentRequest { request, session, tool, args }) = e {
                assert_eq!((session.as_str(), tool.as_str()), (sid2.as_str(), "ide_preview"));
                let v: serde_json::Value = serde_json::from_str(&args).unwrap();
                let first = !seen.contains(&v["preview"].as_str().unwrap().to_owned());
                seen.push(v["preview"].as_str().unwrap().to_owned());
                let answer = match v["action"].as_str() {
                    Some("show") if first => "opened",
                    Some("show") => "reloaded",
                    Some("close") => "closed",
                    other => panic!("unexpected {other:?}"),
                };
                app2.agent_reply(request, Ok(answer.into())).await.unwrap();
                if v["action"] == "close" {
                    return seen;
                }
            }
        }
        seen
    });

    // HTML text: written under the session's data folder, served as the page.
    let r = call(serde_json::json!({"title": "Login mock-up", "kind": "html", "content": "<p>one</p>"})).await;
    assert!(!r.is_error, "{}", r.text);
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!((v["id"].as_str(), v["kind"].as_str(), v["tab"].as_str()), (Some("login-mock-up"), Some("html"), Some("opened")));
    assert!(data.join("sessions").join(&sid).join("previews").join("login-mock-up.html").is_file());
    let (mime, body) = app.preview_file(&sid, "login-mock-up", "").await.unwrap();
    assert_eq!((mime.as_str(), body.as_slice()), ("text/html; charset=utf-8", &b"<p>one</p>"[..]));
    // Its relative files come from the session folder.
    let (mime, body) = app.preview_file(&sid, "login-mock-up", "/shot.png").await.unwrap();
    assert_eq!((mime.as_str(), body.as_slice()), ("image/png", &b"PNG"[..]));
    assert_eq!(app.preview_file(&sid, "login-mock-up", "/../outside.html").await.unwrap_err().id, "preview");

    // The same id replaces it and reloads the tab.
    let r = call(serde_json::json!({"title": "Login mock-up", "kind": "html", "content": "<p>two</p>"})).await;
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["tab"].as_str(), Some("reloaded"), "{}", r.text);
    assert_eq!(app.preview_file(&sid, "login-mock-up", "").await.unwrap().1, b"<p>two</p>");

    // Markdown, rendered.
    let r = call(serde_json::json!({"title": "Plan", "kind": "markdown", "content": "## Steps\n\n| a | b |\n|---|---|\n", "id": "plan"})).await;
    assert!(!r.is_error, "{}", r.text);
    let html = String::from_utf8(app.preview_file(&sid, "plan", "").await.unwrap().1).unwrap();
    assert!(html.contains("<h2>Steps</h2>") && html.contains("<table>"), "{html}");

    // A file of the session folder: its page and the files beside it; Markdown files rendered.
    let r = call(serde_json::json!({"title": "Site", "kind": "file", "path": "site/index.html"})).await;
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(app.preview_file(&sid, "site", "").await.unwrap().1, b"<link href=\"css/a.css\">");
    let (mime, body) = app.preview_file(&sid, "site", "css/a.css").await.unwrap();
    assert_eq!((mime.as_str(), body.as_slice()), ("text/css; charset=utf-8", &b"p{}"[..]));
    let r = call(serde_json::json!({"title": "Readme", "kind": "file", "path": repo.join("docs").join("README.md")})).await;
    assert!(!r.is_error, "{}", r.text);
    let html = String::from_utf8(app.preview_file(&sid, "readme", "").await.unwrap().1).unwrap();
    assert!(html.contains("<h1>Read me</h1>"), "{html}");

    // A web address: the app opens it as a browser tab (nothing to serve).
    let r = call(serde_json::json!({"title": "Dev server", "kind": "url", "url": "http://localhost:5173/"})).await;
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(app.preview_file(&sid, "dev-server", "").await.unwrap_err().id, "preview");

    // Previews of other sessions are not reachable.
    assert!(app.preview_file("nope", "plan", "").await.is_err());

    let r = call(serde_json::json!({"action": "list"})).await;
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    let mut ids: Vec<&str> = v["previews"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, ["dev-server", "login-mock-up", "plan", "readme", "site"]);

    // Close: the tab goes and the preview is forgotten.
    let r = call(serde_json::json!({"action": "close", "id": "plan"})).await;
    let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!((v["closed"].as_str(), v["tab"].as_str()), (Some("plan"), Some("closed")));
    assert_eq!(app.preview_file(&sid, "plan", "").await.unwrap_err().id, "preview");
    let seen = answering.await.unwrap();
    assert_eq!(seen, ["login-mock-up", "login-mock-up", "plan", "site", "readme", "dev-server", "plan"]);

    // The tool is listed for agents.
    assert!(ide_server::bridge::tools().iter().any(|t| t.name == "ide_preview"));
}
