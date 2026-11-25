//! Opt-in launch check of the real `claude` and `codex` with the flags generated from a
//! configuration (skills, MCP server, instructions) and a fresh, never-logged-in account dir
//! inside `MENINX_TEST_TMP`: each TUI is started in a PTY through the server, its screen is read
//! for ~12 s, then the session is stopped and deleted. Nothing is typed (no prompt, no login,
//! no first-run choice). Run:
//! `MENINX_REAL_CLI=1 cargo test -p meninx-server --test real_cli -- --ignored --nocapture`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use meninx_client::{Client, ClientOptions, ConfigurationInfo, McpServerInfo, NewSession};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn real_cli_launch() {
    if std::env::var_os("MENINX_REAL_CLI").is_none() {
        eprintln!("set MENINX_REAL_CLI=1");
        return;
    }
    let dir = tmp_dir("real-cli");
    let repo = dir.join("repo");
    init_repo(&repo);
    let skill = dir.join("skills").join("probe-skill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: probe-skill\ndescription: Launch probe. Never used.\n---\nNothing.\n",
    )
    .unwrap();
    let server = server(&dir.join("data"));
    let c = Client::connect(
        memory(&server),
        ClientOptions {
            reconnect: false,
            ..ClientOptions::default()
        },
    )
    .await
    .unwrap();
    let col = collect(&c, Arc::default());
    let cfg = c.create_configuration("Launch probe").await.unwrap();
    let cfg = c
        .update_configuration(ConfigurationInfo {
            skills: Vec::new(),
            mcp_servers: vec![McpServerInfo {
                name: "probe".into(),
                command: "cmd.exe".into(),
                args: vec!["/d".into(), "/c".into(), "exit".into()],
                env: vec![("PROBE".into(), "1".into())],
            }],
            instructions: "Launch probe: say \"hi\" & stop.".into(),
            ..cfg
        })
        .await
        .unwrap();
    let cfg = c
        .skill_import(&cfg.slug, &skill.display().to_string())
        .await
        .unwrap();
    for p in c.providers(true).await.unwrap() {
        println!(
            "provider {} usable={} {:?}",
            p.id, p.detection.usable, p.detection
        );
    }
    let repo_id = c
        .register_repo(&repo.display().to_string())
        .await
        .unwrap()
        .id;
    for provider in ["claude", "codex"] {
        let acct = c.create_account(provider, "Probe").await.unwrap();
        let (s, _) = c
            .create_session(NewSession {
                provider: provider.into(),
                account: Some(acct.slug.clone()),
                configuration: cfg.slug.clone(),
                repos: vec![repo_id.clone()],
                original_repo: None,
                refresh_base: false,
                background: false,
                bases: Default::default(),
                title: format!("{provider} launch probe"),
                start: true,
                cols: 120,
                rows: 32,
            })
            .await
            .expect("session starts");
        let ctx = server.authorize(server.user()).unwrap();
        let spec = ctx
            .launch_spec(&ctx.resolve_session(&s.id).unwrap())
            .unwrap();
        println!("--- {provider}: {} {:?}", spec.program.display(), spec.args);
        c.attach(&s.id, None).await.unwrap();
        tokio::time::sleep(Duration::from_secs(12)).await;
        let buf = term(&col, &s.id);
        println!(
            "--- {provider}: {} bytes in {} chunk(s) received",
            buf.data.len(),
            buf.chunks
        );
        let text = buf.text();
        let info = c
            .sessions()
            .await
            .unwrap()
            .into_iter()
            .find(|x| x.id == s.id)
            .unwrap();
        println!("--- {provider} state after 12 s: {:?}", info.state);
        println!("--- {provider} screen (raw text, ANSI stripped by the collector):\n{text}\n---");
        let lower = text.to_lowercase();
        for bad in [
            "unknown option",
            "unexpected argument",
            "error loading config",
            "invalid mcp",
            "is not a valid",
        ] {
            assert!(!lower.contains(bad), "{provider}: {bad:?} on screen");
        }
        assert_eq!(
            info.state,
            meninx_client::SessionState::Running,
            "{provider} still running (it did not exit on its arguments)"
        );
        c.stop_session(&s.id).await.unwrap();
        c.delete_session(&s.id, true).await.unwrap();
        let files: Vec<String> = std::fs::read_dir(
            dir.join("data")
                .join("accounts")
                .join(provider)
                .join(&acct.slug),
        )
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
        println!("--- {provider} account dir after the run: {files:?}");
    }
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
