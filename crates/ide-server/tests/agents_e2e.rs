//! Providers, accounts and configurations through the client: legacy profile migration on
//! server start, Claude/Codex launch specs (fake programs on the server's `PATH`; nothing is
//! launched) with a configuration's skills, MCP servers and instructions.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::*;
use ide_client::{
    Client, ClientOptions, ConfigurationInfo, McpServerInfo, NewSession, SessionKind,
};
use ide_core::{LaunchEnv, Paths};
use ide_server::{Server, ServerConfig};

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

fn server_with_fake_clis(data: &Path, bin: &Path) -> Arc<Server> {
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
    Server::new(cfg)
}

async fn connect(server: &Arc<Server>) -> Client {
    Client::connect(
        memory(server),
        ClientOptions {
            reconnect: false,
            ..ClientOptions::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_profiles_are_migrated_on_start() {
    let dir = tmp_dir("agents-migrate");
    let data = dir.join("data");
    for sub in ["claude", "codex"] {
        std::fs::create_dir_all(data.join("profiles").join("work").join(sub)).unwrap();
    }
    std::fs::write(
        data.join("profiles.json"),
        r#"{"profiles":[{"name":"work","claude_config_dir":"","codex_home":"","created_at":3}]}"#,
    )
    .unwrap();
    std::fs::write(
        data.join("sessions.json"),
        r#"{"sessions":[{"id":"abc","title":"old","profile":"work","repo_path":"R","worktree_path":"W","branch":"ide/abc","agent":"codex","created_at":1,"last_active":2}]}"#,
    )
    .unwrap();
    let server = server(&data);
    let c = connect(&server).await;
    let accounts = c.accounts().await.unwrap();
    let ids: Vec<_> = accounts
        .iter()
        .map(|a| format!("{}/{}", a.provider, a.slug))
        .collect();
    assert_eq!(ids, ["claude/work", "codex/work"]);
    let configs = c.configurations().await.unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(
        (configs[0].slug.as_str(), configs[0].created_at),
        ("work", 3)
    );
    let s = &c.sessions().await.unwrap()[0];
    assert_eq!(
        (
            s.provider.as_str(),
            s.account.as_deref(),
            s.configuration.as_str(),
            s.kind
        ),
        ("codex", Some("work"), "work", SessionKind::Agent)
    );
    // The account still uses the old dir: the server knows it, the client never sees paths.
    let ctx = server.authorize(server.user()).unwrap();
    let spec = ctx.launch_spec(&ctx.resolve_session("abc").unwrap());
    // codex is not on the (real) PATH of every machine; either way the error names it.
    if let Ok(spec) = spec {
        let home = spec.env.iter().find(|(k, _)| k == "CODEX_HOME").unwrap();
        assert_eq!(
            PathBuf::from(&home.1),
            data.join("profiles").join("work").join("codex")
        );
    }
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_and_codex_specs_from_a_configuration() {
    let dir = tmp_dir("agents-specs");
    let repo = dir.join("repo");
    init_repo(&repo);
    let data = dir.join("data");
    let server = server_with_fake_clis(&data, &dir.join("bin"));
    let c = connect(&server).await;

    let providers = c.providers(true).await.unwrap();
    for id in ["claude", "codex"] {
        let p = providers.iter().find(|p| p.id == id).unwrap();
        // Found on PATH, but the empty fake program prints no version: not usable.
        assert!(p.detection.path.is_some(), "{id}: {:?}", p.detection);
        assert!(!p.detection.usable);
        assert!(p.caps.accounts && p.caps.login);
    }

    let skill = dir.join("skills").join("review");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: review\ndescription: Review diffs.\n---\nCheck tests.\n",
    )
    .unwrap();
    let cfg = c.create_configuration("Reviewer").await.unwrap();
    let cfg = c
        .update_configuration(ConfigurationInfo {
            skills: vec![skill.display().to_string()],
            mcp_servers: vec![McpServerInfo {
                name: "docs".into(),
                command: "node".into(),
                args: vec!["docs.js".into()],
                env: vec![("TOKEN".into(), "t".into())],
            }],
            instructions: "Review only.".into(),
            ..cfg
        })
        .await
        .unwrap();

    for provider in ["claude", "codex"] {
        let acct = c.create_account(provider, "Team").await.unwrap();
        let s = c
            .create_session(NewSession {
                provider: provider.into(),
                account: Some(acct.slug.clone()),
                configuration: cfg.slug.clone(),
                repo_path: repo.display().to_string(),
                title: String::new(),
                base_ref: String::new(),
                start: false,
                cols: 80,
                rows: 24,
            })
            .await
            .unwrap();
        let ctx = server.authorize(server.user()).unwrap();
        let session = ctx.resolve_session(&s.id).unwrap();
        let spec = ctx.launch_spec(&session).unwrap();
        let scratch = data.join("sessions").join(&s.id).join("agent");
        let acct_dir = data.join("accounts").join(provider).join("team");
        let env = |k: &str| {
            spec.env
                .iter()
                .find(|(x, _)| x == k)
                .map(|(_, v)| PathBuf::from(v))
        };
        let p = |x: PathBuf| x.display().to_string();
        assert_eq!(spec.cwd, PathBuf::from(&s.worktree_path));
        if provider == "claude" {
            assert_eq!(s.title, "Claude Code · repo");
            assert_eq!(
                spec.args,
                vec![
                    "-n".to_owned(),
                    "Claude Code · repo".into(),
                    "--plugin-dir".into(),
                    p(scratch.join("plugin")),
                    "--mcp-config".into(),
                    p(scratch.join("mcp.json")),
                    "--append-system-prompt-file".into(),
                    p(scratch.join("instructions.md")),
                ]
            );
            assert_eq!(env("CLAUDE_CONFIG_DIR"), Some(acct_dir));
            assert!(
                scratch
                    .join("plugin")
                    .join("skills")
                    .join("review")
                    .join("SKILL.md")
                    .is_file()
            );
        } else {
            assert_eq!(spec.args[0], "--no-daemon");
            assert_eq!(
                spec.args[1..4],
                [
                    "-c".to_owned(),
                    r#"mcp_servers.docs={command="node", args=["docs.js"], env={"TOKEN"="t"}}"#
                        .into(),
                    "-c".into()
                ]
            );
            assert!(
                spec.args[4].starts_with("developer_instructions=\"Review only.\\n\\n## Skills\\n"),
                "{}",
                spec.args[4]
            );
            assert_eq!(env("CODEX_HOME"), Some(acct_dir));
        }
        // Nothing generated inside the worktree (only the repo's own file).
        let wt: Vec<_> = std::fs::read_dir(&s.worktree_path)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != ".git")
            .collect();
        assert_eq!(wt, ["README.md"]);
        assert_eq!(
            c.delete_account(provider, "team", false)
                .await
                .unwrap_err()
                .id,
            "account-in-use"
        );
        c.delete_session(&s.id, true).await.unwrap();
        assert!(
            !scratch.exists(),
            "generated files removed with the session"
        );
        c.delete_account(provider, "team", true).await.unwrap();
    }
    // A login session for an account of a provider without a login command cannot exist;
    // for a missing account the error names it.
    let e = c.login_account("claude", "nope", 80, 24).await.unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("provider")),
        ("account-not-found", Some("claude"))
    );
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
