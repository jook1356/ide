//! LaunchSpec tests for every provider (fake programs on a fake `PATH`; nothing is launched),
//! declarative provider parsing, the registry, and — opt-in with `IDE_REAL_CLI=1` — safe checks
//! of the generated flags against the real `claude`/`codex` (no prompts, no login).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ide_core::{Account, Configuration, McpServer, SkillRef, TempDir};

use super::*;

fn fake_exe(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

struct Fixture {
    t: TempDir,
    env: HostEnv,
    bin: PathBuf,
    shell: PathBuf,
    cwd: PathBuf,
    skill: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn fixture(tag: &str) -> Fixture {
    let t = TempDir::new(tag);
    let bin = t.0.join("bin dir");
    std::fs::create_dir_all(&bin).unwrap();
    fake_exe(&bin, &exe("claude"));
    fake_exe(&bin, &exe("codex"));
    let shell = fake_exe(&bin, if cfg!(windows) { "cmd.exe" } else { "sh" });
    let env = HostEnv {
        path: Some(bin.clone().into_os_string()),
        pathext: Some(".COM;.EXE;.BAT;.CMD".into()),
        comspec: Some(shell.clone().into()),
        shell: Some(shell.clone().into()),
    };
    let cwd = t.0.join("worktree");
    std::fs::create_dir_all(&cwd).unwrap();
    let skill = t.0.join("skills src").join("lint-fix");
    std::fs::create_dir_all(skill.join("references")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: lint-fix\ndescription: Fix lint errors. Use when lint fails.\n---\nRun the linter.\n",
    )
    .unwrap();
    std::fs::write(skill.join("references").join("rules.md"), "rules").unwrap();
    Fixture {
        t,
        env,
        bin,
        shell,
        cwd,
        skill,
    }
}

fn account(f: &Fixture, provider: &str) -> Account {
    Account {
        provider: provider.into(),
        slug: "work".into(),
        name: "Work".into(),
        dir: f.t.0.join("accounts").join(provider).join("work"),
        created_at: 0,
    }
}

fn configuration(f: &Fixture) -> Configuration {
    Configuration {
        skills: vec![
            SkillRef {
                path: f.skill.clone(),
            },
            SkillRef {
                path: f.t.0.join("no-such-skill"),
            },
        ],
        mcp_servers: vec![McpServer {
            name: "docs".into(),
            command: "node".into(),
            args: vec!["server.js".into(), "--root".into(), "C:\\a b".into()],
            env: vec![("API_KEY".into(), "k\"1".into())],
        }],
        instructions: "Use pnpm.\nSay \"done\" at the end.".into(),
        ..Configuration::empty("frontend", "Frontend")
    }
}

fn input<'a>(
    f: &'a Fixture,
    acct: Option<&'a Account>,
    cfg: &'a Configuration,
) -> SessionInput<'a> {
    // Original-repo mode: nothing may be written into the cwd (flags and scratch only).
    SessionInput {
        session_id: "s1",
        title: "My session",
        ctx: SessionContext {
            mode: SessionMode::OriginalRepo,
            root: f.cwd.clone(),
            repos: Vec::new(),
            scratch: f.t.0.join("data").join("sessions").join("s1").join("agent"),
            guidance: None,
            shared_root: false,
        },
        account: acct,
        configuration: cfg,
        conversation: None,
        bridge: None,
    }
}

/// A workspace session: the session folder (cwd) holds one worktree; a second repo's
/// worktree and both git dirs lie outside it.
fn workspace_input<'a>(
    f: &'a Fixture,
    acct: Option<&'a Account>,
    cfg: &'a Configuration,
) -> SessionInput<'a> {
    let root = f.t.0.join("workspaces").join("s1");
    std::fs::create_dir_all(root.join("app")).unwrap();
    let mut i = input(f, acct, cfg);
    i.ctx = SessionContext {
        mode: SessionMode::Workspace,
        repos: vec![
            RepoInfo {
                name: "app".into(),
                path: root.join("app"),
                git_dir: Some(f.t.0.join("src").join("app").join(".git")),
            },
            RepoInfo {
                name: "old".into(),
                path: f.t.0.join("worktrees").join("old"),
                git_dir: None,
            },
        ],
        root,
        guidance: Some("# Session workspace s1\n- `app/`".into()),
        ..i.ctx
    };
    i
}

#[test]
fn workspace_root_files() {
    let f = fixture("agents-root");
    let cfg = configuration(&f);
    let dirs = |_: &SessionInput<'_>| {
        vec![
            s(&f.t.0.join("src").join("app").join(".git")),
            s(&f.t.0.join("worktrees").join("old")),
        ]
    };
    // Claude: .claude/skills + CLAUDE.md in the session folder, MCP + instructions by flag.
    let inp = workspace_input(&f, None, &cfg);
    let root = inp.ctx.root.clone();
    std::fs::create_dir_all(root.join(".claude").join("skills").join("stale")).unwrap();
    let out = assemble(&Claude, &inp, &f.env).unwrap();
    assert!(root.join(".claude/skills/lint-fix/SKILL.md").is_file());
    assert!(root.join(".claude/skills/lint-fix/references/rules.md").is_file());
    assert!(!root.join(".claude/skills/stale").exists(), "regenerated");
    assert_eq!(
        std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
        "# Session workspace s1\n- `app/`"
    );
    assert!(!out.spec.args.contains(&"--plugin-dir".to_owned()), "{:?}", out.spec.args);
    let a = &out.spec.args;
    let add: Vec<String> = a
        .windows(2)
        .filter(|w| w[0] == "--add-dir")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(add, dirs(&inp));
    assert!(a.contains(&"--mcp-config".to_owned()));
    let instr = std::fs::read_to_string(inp.ctx.scratch.join("instructions.md")).unwrap();
    assert!(!instr.contains("Session workspace"), "guidance is in CLAUDE.md: {instr}");
    assert_eq!(std::fs::read_dir(root.join("app")).unwrap().count(), 0, "worktree untouched");

    // Codex: .agents/skills + AGENTS.md; skills no longer in the developer instructions.
    let inp = workspace_input(&f, None, &cfg);
    let out = assemble(&Codex, &inp, &f.env).unwrap();
    assert!(root.join(".agents/skills/lint-fix/SKILL.md").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "# Session workspace s1\n- `app/`"
    );
    let dev = out
        .spec
        .args
        .iter()
        .find(|a| a.starts_with("developer_instructions="))
        .unwrap();
    assert!(!dev.contains("lint-fix") && dev.contains("pnpm"), "{dev}");
    let add: Vec<String> = out
        .spec
        .args
        .windows(2)
        .filter(|w| w[0] == "--add-dir")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(add, dirs(&inp));

    // Shell: nothing written; a declarative-style default puts the guidance in the instructions.
    let inp = workspace_input(&f, None, &cfg);
    std::fs::remove_file(root.join("AGENTS.md")).unwrap();
    assemble(&Shell, &inp, &f.env).unwrap();
    assert!(!root.join("AGENTS.md").exists());

    // Original-repo mode refuses root files.
    let plan = LaunchPlan::new("x", &input(&f, None, &cfg));
    assert!(plan.write_root_file("CLAUDE.md", b"x").is_err());
    assert!(plan.root_dir(".claude").is_err());
    worktree_untouched(&f);

    // Caps per mode.
    assert_eq!(Codex.capabilities().mode(SessionMode::Workspace).skills, Support::Native);
    assert_eq!(Codex.capabilities().mode(SessionMode::OriginalRepo).skills, Support::Fallback);
    assert_eq!(Claude.capabilities().mode(SessionMode::OriginalRepo).skills, Support::Native);
}

fn env_of<'a>(spec: &'a LaunchSpec, key: &str) -> Option<&'a str> {
    spec.env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn worktree_untouched(f: &Fixture) {
    assert_eq!(
        std::fs::read_dir(&f.cwd).unwrap().count(),
        0,
        "nothing is written into the worktree"
    );
}

/// The args without Claude's `--session-id <id>` pair (checked to be the launch's
/// conversation, right after the name flag).
fn without_session_id(out: &Assembled) -> Vec<String> {
    let conv = out.conversation.clone().expect("a fresh conversation id");
    let i = out.spec.args.iter().position(|a| a == "--session-id").expect("--session-id");
    assert_eq!(out.spec.args[i + 1], conv);
    let mut args = out.spec.args.clone();
    args.drain(i..i + 2);
    args
}

#[test]
fn claude_resume() {
    let f = fixture("agents-claude-resume");
    let acct = account(&f, "claude");
    let cfg = Configuration::empty("default", "default");
    let mut inp = input(&f, Some(&acct), &cfg);

    // Fresh: a random v4 UUID, passed with --session-id.
    let a = assemble(&Claude, &inp, &f.env).unwrap();
    let b = assemble(&Claude, &inp, &f.env).unwrap();
    let id = a.conversation.clone().unwrap();
    assert_ne!(Some(id.clone()), b.conversation, "a new id per fresh start");
    assert_eq!(id.len(), 36);
    assert_eq!(&id[14..15], "4", "version 4: {id}");
    assert!(claude::is_session_id(&id));
    assert!(a.spec.args.windows(2).any(|w| w == ["--session-id", id.as_str()]));

    // Stored, but the CLI kept no transcript (in this account): a new conversation.
    inp.conversation = Some(&id);
    let out = assemble(&Claude, &inp, &f.env).unwrap();
    assert!(!out.spec.args.contains(&"--resume".to_owned()));
    assert_ne!(out.conversation.as_deref(), Some(id.as_str()));

    // With the transcript under the account's projects/<cwd folder>/: resumed.
    let project = acct.dir.join("projects").join("C--some-cwd");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(format!("{id}.jsonl")), "{}
").unwrap();
    let out = assemble(&Claude, &inp, &f.env).unwrap();
    assert_eq!(out.conversation.as_deref(), Some(id.as_str()));
    assert!(out.spec.args.windows(2).any(|w| w == ["--resume", id.as_str()]));
    assert!(!out.spec.args.contains(&"--session-id".to_owned()));

    // Not a session id (hook input is untrusted): never an argument.
    for bad in ["../x", "--help", "0000", ""] {
        inp.conversation = Some(bad);
        let out = assemble(&Claude, &inp, &f.env).unwrap();
        assert!(!out.spec.args.iter().any(|a| a == bad || a == "--resume"), "{bad}");
    }

    // A server started from a Claude Code tool shell: its session markers are not inherited,
    // user settings are.
    for k in ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_CHILD_SESSION", "AI_AGENT"] {
        assert!(out.spec.env_remove.iter().any(|r| r == k), "{k}");
    }
    assert!(!out.spec.env_remove.iter().any(|r| r == "CLAUDE_CONFIG_DIR"));

    // Providers without resume get neither flags nor an id.
    let out = assemble(&Codex, &inp, &f.env).unwrap();
    assert_eq!(out.conversation, None);
    assert!(out.spec.env_remove.is_empty());
}

#[test]
fn claude_spec() {
    let f = fixture("agents-claude");
    let acct = account(&f, "claude");
    let cfg = configuration(&f);
    let inp = input(&f, Some(&acct), &cfg);
    let out = assemble(&Claude, &inp, &f.env).unwrap();
    let scratch = &inp.ctx.scratch;
    let plugin = scratch.join("plugin");
    assert_eq!(out.spec.program, f.bin.join(exe("claude")));
    assert_eq!(
        without_session_id(&out),
        vec![
            "-n".to_owned(),
            "My session".into(),
            "--plugin-dir".into(),
            s(&plugin),
            "--mcp-config".into(),
            s(&scratch.join("mcp.json")),
            "--append-system-prompt-file".into(),
            s(&scratch.join("instructions.md")),
        ]
    );
    assert_eq!(out.spec.cwd, f.cwd);
    assert_eq!(
        env_of(&out.spec, "CLAUDE_CONFIG_DIR"),
        Some(s(&acct.dir).as_str())
    );
    assert_eq!(env_of(&out.spec, "CODEX_HOME"), None);
    assert_eq!(env_of(&out.spec, "TERM"), Some("xterm-256color"));
    assert_eq!(env_of(&out.spec, "COLORTERM"), Some("truecolor"));
    assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
    assert!(out.notes[0].contains("no-such-skill"));

    // Plugin: manifest + skills/<folder>/ copied with its files.
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(plugin.join(".claude-plugin").join("plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["name"], "ide-frontend");
    assert_eq!(manifest["version"], "1.0.0");
    assert_eq!(manifest["author"]["name"], "ide");
    let copied = plugin.join("skills").join("lint-fix");
    assert!(copied.join("SKILL.md").is_file());
    assert!(copied.join("references").join("rules.md").is_file());
    // MCP JSON.
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(scratch.join("mcp.json")).unwrap()).unwrap();
    assert_eq!(
        mcp,
        serde_json::json!({"mcpServers": {"docs": {
            "type": "stdio", "command": "node",
            "args": ["server.js", "--root", "C:\\a b"],
            "env": {"API_KEY": "k\"1"}
        }}})
    );
    // Instructions: configuration text only (skills are native).
    assert_eq!(
        std::fs::read_to_string(scratch.join("instructions.md")).unwrap(),
        cfg.instructions
    );
    worktree_untouched(&f);

    // Without account, skills, MCP and instructions: just the name flag.
    let empty = Configuration::empty("default", "default");
    let out = assemble(&Claude, &input(&f, None, &empty), &f.env).unwrap();
    assert_eq!(without_session_id(&out), ["-n", "My session"]);
    assert_eq!(env_of(&out.spec, "CLAUDE_CONFIG_DIR"), None);
    assert!(!scratch.exists(), "stale generated files are removed");
}

#[test]
fn codex_spec() {
    let f = fixture("agents-codex");
    let acct = account(&f, "codex");
    let cfg = configuration(&f);
    let inp = input(&f, Some(&acct), &cfg);
    let out = assemble(&Codex, &inp, &f.env).unwrap();
    assert_eq!(out.spec.program, f.bin.join(exe("codex")));
    let a = &out.spec.args;
    assert_eq!(a.len(), 5, "{a:?}");
    assert_eq!(a[0], "--no-daemon");
    assert_eq!((a[1].as_str(), a[3].as_str()), ("-c", "-c"));
    assert_eq!(
        a[2],
        r#"mcp_servers.docs={command="node", args=["server.js", "--root", "C:\\a b"], env={"API_KEY"="k\"1"}}"#
    );
    // Both overrides are valid TOML with the intended values.
    let t: toml::Table = toml::from_str(&a[2]).unwrap();
    let docs = &t["mcp_servers"]["docs"];
    assert_eq!(docs["command"].as_str(), Some("node"));
    assert_eq!(docs["args"][2].as_str(), Some("C:\\a b"));
    assert_eq!(docs["env"]["API_KEY"].as_str(), Some("k\"1"));
    assert!(a[4].starts_with("developer_instructions=\""), "{}", a[4]);
    assert!(!a[4].contains('\n'), "single-line argument");
    let t: toml::Table = toml::from_str(&a[4]).unwrap();
    let text = t["developer_instructions"].as_str().unwrap();
    assert!(
        text.starts_with("Use pnpm.\nSay \"done\" at the end.\n\n## Skills\n"),
        "{text}"
    );
    assert!(
        text.contains("- lint-fix: Fix lint errors. Use when lint fails. (file: "),
        "{text}"
    );
    assert!(text.contains(&s(&f.skill.join("SKILL.md"))), "{text}");
    assert_eq!(env_of(&out.spec, "CODEX_HOME"), Some(s(&acct.dir).as_str()));
    assert_eq!(env_of(&out.spec, "CLAUDE_CONFIG_DIR"), None);
    assert_eq!(env_of(&out.spec, "TERM"), Some("xterm-256color"));
    assert!(out.notes[0].contains("no-such-skill"));
    worktree_untouched(&f);

    let empty = Configuration::empty("default", "default");
    let out = assemble(&Codex, &input(&f, None, &empty), &f.env).unwrap();
    assert_eq!(out.spec.args, ["--no-daemon"]);
}

#[test]
fn shell_spec_ignores_configuration() {
    let f = fixture("agents-shell");
    let cfg = configuration(&f);
    let inp = input(&f, None, &cfg);
    let out = assemble(&Shell, &inp, &f.env).unwrap();
    assert_eq!(out.spec.program, f.shell);
    assert!(out.spec.args.is_empty());
    let keys: Vec<_> = out.spec.env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["TERM", "COLORTERM"]);
    assert!(!inp.ctx.scratch.exists());
    assert_eq!(Shell.capabilities(), Caps::NONE);
    let d = Shell.detect(&f.env);
    assert!(d.usable(), "{d:?}");
    assert_eq!(d.path.as_deref(), Some(f.shell.as_path()));
}

#[test]
fn missing_program_fails_cleanly() {
    let f = fixture("agents-missing");
    let env = HostEnv {
        path: Some(f.t.0.join("empty").into_os_string()),
        ..f.env.clone()
    };
    let cfg = Configuration::empty("default", "default");
    let err = assemble(&Codex, &input(&f, None, &cfg), &env).unwrap_err();
    assert!(matches!(&err, ide_core::Error::ProgramNotFound { program } if program == "codex"));
    let d = Claude.detect(&env);
    assert!(
        d.path.is_none() && !d.usable() && d.error.is_some(),
        "{d:?}"
    );
}

#[test]
fn login_specs() {
    let f = fixture("agents-login");
    let c = account(&f, "claude");
    let spec = login_spec(&Claude, &c, &f.cwd, &f.env).unwrap();
    assert_eq!(spec.program, f.bin.join(exe("claude")));
    assert_eq!(spec.args, ["auth", "login"]);
    assert_eq!(env_of(&spec, "CLAUDE_CONFIG_DIR"), Some(s(&c.dir).as_str()));
    let x = account(&f, "codex");
    let spec = login_spec(&Codex, &x, &f.cwd, &f.env).unwrap();
    assert_eq!(spec.args, ["login"]);
    assert_eq!(env_of(&spec, "CODEX_HOME"), Some(s(&x.dir).as_str()));
    assert!(matches!(
        login_spec(&Shell, &x, &f.cwd, &f.env),
        Err(ide_core::Error::ProviderNoLogin(_))
    ));
}

const GEMINI: &str = r#"
id = "gemini"
name = "Gemini CLI"
program = "codex"            # any fake program on the test PATH
args = ["--home", "{account_dir}", "--sid={session_id}"]
min_version = "0.1"
glyph = "✦"
account_env = "GEMINI_CLI_HOME"
login_args = ["auth", "login"]

[env]
GEMINI_SCRATCH = "{scratch_dir}"

[instructions]
mechanism = "flag"
flag = "--system-prompt-file"

[mcp]
mechanism = "unsupported"
"#;

#[test]
fn declarative_parse_and_spec() {
    let spec = DeclarativeSpec::parse(GEMINI).unwrap();
    assert_eq!(spec.id, "gemini");
    assert_eq!(spec.version_args, ["--version"]);
    let p = DeclarativeProvider::new(spec);
    let caps = p.capabilities();
    assert!(caps.accounts && caps.login);
    assert_eq!(
        (caps.skills, caps.mcp, caps.instructions),
        (Support::Fallback, Support::Unsupported, Support::Native)
    );
    assert_eq!(p.glyph(), "✦");

    let f = fixture("agents-decl");
    let acct = account(&f, "gemini");
    let cfg = configuration(&f);
    let inp = input(&f, Some(&acct), &cfg);
    let out = assemble(&p, &inp, &f.env).unwrap();
    let instr = inp.ctx.scratch.join("instructions.md");
    assert_eq!(
        out.spec.args,
        vec![
            "--home".to_owned(),
            s(&acct.dir),
            "--sid=s1".into(),
            "--system-prompt-file".into(),
            s(&instr),
        ]
    );
    assert_eq!(
        env_of(&out.spec, "GEMINI_CLI_HOME"),
        Some(s(&acct.dir).as_str())
    );
    assert_eq!(
        env_of(&out.spec, "GEMINI_SCRATCH"),
        Some(s(&inp.ctx.scratch).as_str())
    );
    let text = std::fs::read_to_string(&instr).unwrap();
    assert!(
        text.starts_with("Use pnpm.") && text.contains("- lint-fix: "),
        "{text}"
    );
    // MCP is not dropped silently.
    assert!(
        out.notes
            .iter()
            .any(|n| n.starts_with("mcp: not supported")),
        "{:?}",
        out.notes
    );
    assert_eq!(
        login_spec(&p, &acct, &f.cwd, &f.env).unwrap().args,
        ["auth", "login"]
    );

    // env and file mechanisms.
    let env_p = DeclarativeProvider::new(
        DeclarativeSpec::parse(
            "id='x'\nname='X'\nprogram='codex'\n[instructions]\nmechanism='env'\nenv='X_SYSTEM'",
        )
        .unwrap(),
    );
    let out = assemble(&env_p, &inp, &f.env).unwrap();
    assert!(out.spec.args.is_empty());
    assert_eq!(env_of(&out.spec, "X_SYSTEM"), Some(s(&instr).as_str()));
    assert!(!env_p.capabilities().accounts && env_p.login_args().is_none());
    let file_p = DeclarativeProvider::new(
        DeclarativeSpec::parse(
            "id='y'\nname='Y'\nprogram='codex'\nargs=['--rules','{instructions_file}']\n[instructions]\nmechanism='file'\nfile='RULES.md'",
        )
        .unwrap(),
    );
    let empty = Configuration::empty("default", "default");
    let inp2 = input(&f, None, &empty);
    let out = assemble(&file_p, &inp2, &f.env).unwrap();
    let rules = inp2.ctx.scratch.join("RULES.md");
    assert_eq!(out.spec.args, vec!["--rules".to_owned(), s(&rules)]);
    assert!(rules.is_file(), "file mechanism always writes the file");
    // No instructions mechanism: skills cannot fall back either.
    let none_p = DeclarativeProvider::new(
        DeclarativeSpec::parse("id='z'\nname='Z'\nprogram='codex'").unwrap(),
    );
    assert_eq!(none_p.capabilities().skills, Support::Unsupported);
    let out = assemble(&none_p, &inp, &f.env).unwrap();
    assert!(
        out.notes
            .iter()
            .any(|n| n.starts_with("instructions: not supported"))
    );
    worktree_untouched(&f);
}

#[test]
fn declarative_rejects_bad_files() {
    for (bad, why) in [
        ("id='Bad Id'\nname='x'\nprogram='p'", "id"),
        ("id='x'\nname=''\nprogram='p'", "name"),
        ("id='x'\nname='x'\nprogram='p'\nunknown=1", "unknown field"),
        (
            "id='x'\nname='x'\nprogram='p'\n[mcp]\nmechanism='json'",
            "mcp",
        ),
        (
            "id='x'\nname='x'\nprogram='p'\n[instructions]\nmechanism='flag'",
            "flag",
        ),
        (
            "id='x'\nname='x'\nprogram='p'\n[instructions]\nmechanism='env'",
            "env",
        ),
        (
            "id='x'\nname='x'\nprogram='p'\n[instructions]\nmechanism='file'\nfile='../x'",
            "file",
        ),
        (
            "id='x'\nname='x'\nprogram='p'\n[instructions]\nmechanism='magic'",
            "mechanism",
        ),
        ("not toml", "syntax"),
    ] {
        assert!(DeclarativeSpec::parse(bad).is_err(), "{why}: {bad}");
    }
}

#[test]
fn registry_loads_declarative_files() {
    let f = fixture("agents-registry");
    let dir = f.t.0.join("providers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("gemini.toml"), GEMINI).unwrap();
    std::fs::write(dir.join("bad.toml"), "id = 1").unwrap();
    std::fs::write(dir.join("clash.toml"), "id='claude'\nname='x'\nprogram='p'").unwrap();
    std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
    let reg = Providers::load(&dir);
    let ids: Vec<_> = reg.list().iter().map(|p| p.id().to_owned()).collect();
    assert_eq!(ids, ["claude", "codex", "shell", "gemini"]);
    assert_eq!(reg.load_errors().len(), 2, "{:?}", reg.load_errors());
    assert_eq!(reg.require("gemini").unwrap().display_name(), "Gemini CLI");
    assert!(matches!(
        reg.require("nope"),
        Err(ide_core::Error::ProviderNotFound(_))
    ));
    // The sample shipped in docs parses.
    let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("providers")
        .join("example.toml");
    DeclarativeSpec::parse(&std::fs::read_to_string(sample).unwrap()).unwrap();
    // Missing dir: built-ins only.
    assert_eq!(Providers::load(&f.t.0.join("none")).list().len(), 3);
    let mut reg = reg;
    reg.add(Arc::new(Shell));
    assert_eq!(reg.list().len(), 4);
}

#[test]
fn detection_runs_the_version_command() {
    let f = fixture("agents-detect");
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd.exe", vec!["/d", "/c", "echo tool 1.2.3"])
    } else {
        ("sh", vec!["-c", "echo tool 1.2.3"])
    };
    let real = HostEnv::from_process();
    let mk = |min: &str| {
        DeclarativeProvider::new(
            DeclarativeSpec::parse(&format!(
                "id='t'\nname='T'\nprogram='{program}'\nversion_args={args:?}\nmin_version='{min}'"
            ))
            .unwrap(),
        )
    };
    let d = mk("1.0").detect(&real);
    assert_eq!(d.version.as_deref(), Some("1.2.3"), "{d:?}");
    assert!(d.usable(), "{d:?}");
    let d = mk("1.10").detect(&real);
    assert!(!d.usable() && d.path.is_some(), "{d:?}");
    assert!(d.error.unwrap().contains("older than 1.10"));
    drop(f);
}

/// Safe checks of the generated flags against the real CLIs: `claude plugin validate` on the
/// generated plugin (and on a copy of the MCP JSON as a plugin `.mcp.json`), `codex mcp list
/// --json` and `codex debug prompt-input` (renders the model input locally; nothing is sent)
/// with the generated `-c` overrides, `codex features list`. Isolated account dirs inside
/// `IDE_TEST_TMP`; no prompt is sent and nobody logs in.
/// Run: `IDE_REAL_CLI=1 cargo test -p ide-agents real_cli -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_cli_checks() {
    if std::env::var_os("IDE_REAL_CLI").is_none() {
        eprintln!("set IDE_REAL_CLI=1");
        return;
    }
    let real = HostEnv::from_process();
    let f = fixture("agents-real");
    let cfg = configuration(&f);
    let run = |program: &Path, args: &[String], vars: &[(String, String)], cwd: &Path| {
        let mut c = std::process::Command::new(program);
        c.args(args)
            .envs(vars.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd);
        let o = c.output().unwrap();
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };

    // ---- Claude ----
    let d = Claude.detect(&real);
    println!("claude detection: {d:?}");
    assert!(d.usable());
    let acct = Account {
        dir: f.t.0.join("claude-account"),
        ..account(&f, "claude")
    };
    std::fs::create_dir_all(&acct.dir).unwrap();
    let inp = input(&f, Some(&acct), &cfg);
    let out = assemble(&Claude, &inp, &real).unwrap();
    println!("claude spec: {:#?}", out.spec);
    let plugin = inp.ctx.scratch.join("plugin");
    let (code, so, se) = run(
        &out.spec.program,
        &[
            "plugin".into(),
            "validate".into(),
            "--json".into(),
            s(&plugin),
        ],
        &out.spec.env,
        &f.cwd,
    );
    println!("claude plugin validate (plugin): {code:?}\n{so}\n{se}");
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&so).unwrap();
    assert_eq!(v["success"], true);
    assert_eq!(v["provides"]["skills"][0], "ide-frontend:lint-fix");
    // The --mcp-config file has the plugin `.mcp.json` format: validate a copy.
    let probe = f.t.0.join("mcp-probe");
    std::fs::create_dir_all(probe.join(".claude-plugin")).unwrap();
    std::fs::write(
        probe.join(".claude-plugin").join("plugin.json"),
        r#"{"name":"ide-mcp-probe","version":"1.0.0","description":"probe","author":{"name":"ide"}}"#,
    )
    .unwrap();
    std::fs::copy(inp.ctx.scratch.join("mcp.json"), probe.join(".mcp.json")).unwrap();
    let (code, so, se) = run(
        &out.spec.program,
        &[
            "plugin".into(),
            "validate".into(),
            "--json".into(),
            s(&probe),
        ],
        &out.spec.env,
        &f.cwd,
    );
    println!("claude plugin validate (mcp.json): {code:?}\n{so}\n{se}");
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&so).unwrap();
    assert_eq!(v["provides"]["mcpServers"][0]["name"], "docs");
    // Every generated flag is a documented flag of this version.
    let (_, help, _) = run(&out.spec.program, &["--help".into()], &out.spec.env, &f.cwd);
    // (`--append-system-prompt-file` has no own line in `--help`; it is listed as
    // `--append-system-prompt[-file]` and documented in the CLI reference.)
    for flag in [
        "--plugin-dir",
        "--mcp-config",
        "--append-system-prompt[-file]",
        "--name",
    ] {
        assert!(help.contains(flag), "{flag} in claude --help");
    }
    let (code, so, _) = run(
        &out.spec.program,
        &["auth".into(), "status".into(), "--json".into()],
        &out.spec.env,
        &f.cwd,
    );
    println!("claude auth status (scratch account): {code:?} {so}");
    assert_eq!(
        Claude.account_status(&acct, &real),
        AccountStatus::LoggedOut
    );

    // ---- Codex ----
    let d = Codex.detect(&real);
    println!("codex detection: {d:?}");
    assert!(d.usable());
    let acct = Account {
        dir: f.t.0.join("codex-account"),
        ..account(&f, "codex")
    };
    std::fs::create_dir_all(&acct.dir).unwrap();
    let inp = input(&f, Some(&acct), &cfg);
    let out = assemble(&Codex, &inp, &real).unwrap();
    println!("codex spec: {:#?}", out.spec);
    let overrides: Vec<String> = out.spec.args[1..].to_vec(); // without --no-daemon
    let with = |pre: &[&str]| {
        let mut v: Vec<String> = pre.iter().map(|x| x.to_string()).collect();
        v.extend(overrides.iter().cloned());
        v
    };
    let (code, so, se) = run(
        &out.spec.program,
        &with(&["mcp", "list", "--json"]),
        &out.spec.env,
        &f.cwd,
    );
    println!("codex mcp list: {code:?}\n{so}\n{se}");
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&so).unwrap();
    assert_eq!(v[0]["name"], "docs");
    assert_eq!(v[0]["transport"]["command"], "node");
    assert_eq!(v[0]["transport"]["args"][2], "C:\\a b");
    let (code, so, se) = run(
        &out.spec.program,
        &with(&["debug", "prompt-input"]),
        &out.spec.env,
        &f.cwd,
    );
    assert_eq!(code, Some(0), "{se}");
    let items: serde_json::Value = serde_json::from_str(&so).unwrap();
    let dev: Vec<String> = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["role"] == "developer")
        .flat_map(|i| i["content"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c["text"].as_str().map(str::to_owned))
        .collect();
    println!("codex developer messages: {}", dev.len());
    let ours = dev
        .iter()
        .find(|t| t.starts_with("Use pnpm."))
        .expect("developer_instructions present in the model input");
    println!("codex developer_instructions as rendered:\n{ours}");
    assert!(ours.contains("Say \"done\" at the end."));
    assert!(ours.contains("- lint-fix: Fix lint errors."));
    let (code, so, _) = run(
        &out.spec.program,
        &with(&["features", "list"]),
        &out.spec.env,
        &f.cwd,
    );
    assert_eq!(code, Some(0));
    println!(
        "codex features (skills/hooks/plugins):\n{}",
        so.lines()
            .filter(|l| l.contains("skill") || l.contains("hook") || l.starts_with("plugins"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let (_, help, _) = run(&out.spec.program, &["--help".into()], &out.spec.env, &f.cwd);
    assert!(help.contains("--no-daemon"));
    assert_eq!(Codex.account_status(&acct, &real), AccountStatus::LoggedOut);
}

/// Claude hooks: `--settings <scratch>/settings.json` with exec-form entries (the hook program
/// plus `hook --provider claude <event>`), only with a hook program; hook input -> events;
/// a deny -> Claude's PreToolUse JSON, anything else -> silent exit 0.
#[test]
fn claude_hooks() {
    let f = fixture("agents-claude-hooks");
    let cfg = Configuration::empty("plain", "Plain");
    let program = f.bin.join(exe("ide-server"));
    let mut inp = input(&f, None, &cfg);
    let out = assemble(&Claude, &inp, &f.env).unwrap();
    assert!(!out.spec.args.contains(&"--settings".to_owned()), "no bridge, no hooks");
    inp.bridge = Some(Bridge {
        server: McpServer {
            name: "ide".into(),
            command: s(&program),
            args: vec!["mcp".into()],
            env: Vec::new(),
        },
        env: vec![("IDE_SESSION_ID".into(), "s1".into())],
        instructions: "## ide".into(),
        hooks: HookTargets {
            program: Some(program.clone()),
        },
    });
    let out = assemble(&Claude, &inp, &f.env).unwrap();
    let settings = inp.ctx.scratch.join("settings.json");
    let i = out.spec.args.iter().position(|a| a == "--settings").expect("--settings");
    assert_eq!(out.spec.args[i + 1], s(&settings));
    assert_eq!(env_of(&out.spec, "IDE_SESSION_ID"), Some("s1"));
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "Notification",
        "PreToolUse",
        "SubagentStart",
        "SubagentStop",
    ] {
        let h = &v["hooks"][event][0]["hooks"][0];
        assert_eq!(h["type"], "command", "{event}");
        assert_eq!(h["command"], s(&program), "{event}");
        assert_eq!(h["args"], serde_json::json!(["hook", "--provider", "claude", event]));
    }
    assert_eq!(v["hooks"]["PreToolUse"][0].get("matcher"), None, "every tool");
    assert_eq!(
        v["hooks"]["Notification"][0]["matcher"],
        "permission_prompt|elicitation_dialog"
    );

    let p = |e: &str, j: serde_json::Value| Claude.parse_hook(e, j.to_string().as_bytes());
    assert_eq!(p("Stop", serde_json::json!({})), Some(HookEvent::Stopped));
    assert_eq!(p("UserPromptSubmit", serde_json::json!({"prompt": "x"})), Some(HookEvent::PromptSubmitted));
    assert_eq!(
        p("Notification", serde_json::json!({"notification_type": "permission_prompt"})),
        Some(HookEvent::NeedsInput { permission: true })
    );
    assert_eq!(p("Notification", serde_json::json!({"notification_type": "idle_prompt"})), None);
    assert_eq!(
        p("PreToolUse", serde_json::json!({"tool_name": "Write", "tool_input": {"file_path": "C:/w/a.rs"}})),
        Some(HookEvent::PreTool(ToolUse::Edit {
            tool: "Write".into(),
            paths: vec!["C:/w/a.rs".into()]
        }))
    );
    assert_eq!(
        p("PreToolUse", serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}})),
        Some(HookEvent::PreTool(ToolUse::Shell {
            tool: "Bash".into(),
            command: "ls".into()
        }))
    );
    assert_eq!(
        p("PreToolUse", serde_json::json!({"tool_name": "Agent", "tool_input": {"run_in_background": true}})),
        Some(HookEvent::PreTool(ToolUse::Other { tool: "Agent".into() }))
    );
    assert_eq!(
        p("SubagentStart", serde_json::json!({"agent_id": "a1", "agent_type": "Explore"})),
        Some(HookEvent::BackgroundStarted { id: "a1".into() })
    );
    assert_eq!(
        p("SubagentStop", serde_json::json!({"agent_id": "a1"})),
        Some(HookEvent::BackgroundEnded { id: "a1".into() })
    );
    assert_eq!(p("SubagentStart", serde_json::json!({})), None, "no id: cannot pair");
    assert_eq!(p("SubagentStop", serde_json::json!({"agent_id": ""})), None);
    assert_eq!(Claude.parse_hook("Stop", b"not json"), Some(HookEvent::Stopped));
    let id = "0f8e2a4c-1b3d-4e5f-8a7b-9c0d1e2f3a4b";
    assert_eq!(
        p("SessionStart", serde_json::json!({"session_id": id, "source": "clear"})),
        Some(HookEvent::SessionStart { conversation: Some(id.into()) })
    );
    assert_eq!(
        p("SessionStart", serde_json::json!({"session_id": "../../x"})),
        Some(HookEvent::SessionStart { conversation: None }),
        "not a session id"
    );
    assert_eq!(Claude.parse_hook("Nope", b"{}"), None);

    assert_eq!(Claude.hook_reply("PreToolUse", &HookDecision::NoOpinion), HookReply::default());
    let r = Claude.hook_reply(
        "PreToolUse",
        &HookDecision::Deny {
            reason: "P writes here".into(),
        },
    );
    assert_eq!(r.exit_code, 0);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(v["hookSpecificOutput"]["permissionDecisionReason"], "P writes here");
    // A deny only means something for PreToolUse.
    assert_eq!(Claude.hook_reply("Stop", &HookDecision::Deny { reason: "x".into() }), HookReply::default());
    // Codex: hooks come from the account integration, never per session.
    assert_eq!(Codex.hook_caps().guard_edits, Support::Native);
    let out = assemble(&Codex, &inp, &f.env).unwrap();
    assert!(
        !out.spec.args.iter().any(|a| a.starts_with("hooks") || a == "--settings"),
        "{:?}",
        out.spec.args
    );
}

/// Codex's account integration: ide's entries in `CODEX_HOME/hooks.json` next to the user's
/// own hooks (kept), status (missing / installed / outdated), uninstall; Codex hook input.
#[test]
fn codex_integration() {
    let f = fixture("agents-codex-integration");
    let acct = account(&f, "codex");
    std::fs::create_dir_all(&acct.dir).unwrap();
    let program = f.bin.join(exe("ide-server"));
    let targets = HookTargets {
        program: Some(program.clone()),
    };
    assert!(Codex.capabilities().integration && !Claude.capabilities().integration);
    assert_eq!(Claude.integration_status(&acct, &targets), IntegrationStatus::NotNeeded);
    assert_eq!(Codex.integration_status(&acct, &targets), IntegrationStatus::Missing);
    // The user's own hook stays.
    let own = serde_json::json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-me"}]}]}, "description": "mine"});
    std::fs::write(acct.dir.join("hooks.json"), own.to_string()).unwrap();
    let r = Codex.integration_install(&acct, &targets).unwrap();
    assert_eq!(r.approve_with.as_deref(), Some("/hooks"));
    assert_eq!(Codex.integration_status(&acct, &targets), IntegrationStatus::Installed);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(acct.dir.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(v["description"], "mine");
    let stop = v["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2, "{v}");
    assert_eq!(stop[0]["hooks"][0]["command"], "notify-me");
    let pre = &v["hooks"]["PreToolUse"][0];
    assert_eq!(pre["matcher"], "apply_patch|Edit|Write");
    let cmd = pre["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.ends_with(" hook --provider codex PreToolUse"), "{cmd}");
    assert!(cmd.starts_with('"'), "a path with blanks is quoted: {cmd}");
    // Installing again replaces ide's entries instead of adding more.
    Codex.integration_install(&acct, &targets).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(acct.dir.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
    // Another program (the IDE moved): outdated.
    let moved = HookTargets {
        program: Some(f.t.0.join("elsewhere").join(exe("ide-server"))),
    };
    assert_eq!(Codex.integration_status(&acct, &moved), IntegrationStatus::Outdated);
    Codex.integration_uninstall(&acct).unwrap();
    assert_eq!(Codex.integration_status(&acct, &targets), IntegrationStatus::Missing);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(acct.dir.join("hooks.json")).unwrap()).unwrap();
    assert_eq!(v, own, "only the user's hooks are left");

    // Hook input.
    let patch = serde_json::json!({
        "cwd": f.cwd.display().to_string(),
        "tool_name": "apply_patch",
        "tool_input": {"command": ["apply_patch", "*** Begin Patch
*** Update File: src/a.rs
@@
-x
+y
*** Add File: b.rs
+z
*** End Patch
"]},
    });
    assert_eq!(
        Codex.parse_hook("PreToolUse", patch.to_string().as_bytes()),
        Some(HookEvent::PreTool(ToolUse::Edit {
            tool: "apply_patch".into(),
            paths: vec![
                f.cwd.join("src/a.rs").display().to_string(),
                f.cwd.join("b.rs").display().to_string(),
            ],
        }))
    );
    assert_eq!(Codex.parse_hook("PermissionRequest", b"{}"), Some(HookEvent::NeedsInput { permission: true }));
    assert_eq!(Codex.parse_hook("Stop", b"{}"), Some(HookEvent::Stopped));
    let deny = Codex.hook_reply("PreToolUse", &HookDecision::Deny { reason: "r".into() });
    let v: serde_json::Value = serde_json::from_str(&deny.stdout).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
}
