//! Claude Code (`claude`).
//!
//! | Common | Mapping (verified with 2.1.329, see docs/agent-providers.md) |
//! |---|---|
//! | Account | `CLAUDE_CONFIG_DIR=<account dir>` |
//! | Login | `claude auth login`; status `claude auth status --json` (`loggedIn`) |
//! | Skills | workspace: project skills `<session folder>/.claude/skills/<name>/` (docs); original repo: generated plugin `<scratch>/plugin` (`.claude-plugin/plugin.json` + `skills/<name>/`) via `--plugin-dir` |
//! | Guidance | workspace: `<session folder>/CLAUDE.md` (project memory of the cwd) |
//! | MCP | `<scratch>/mcp.json` (`{"mcpServers": ...}`) via `--mcp-config` (a project `.mcp.json` would wait for approval: verified) |
//! | Instructions | `<scratch>/instructions.md` via `--append-system-prompt-file` |
//! | Outside dirs | `--add-dir` (legacy worktrees, the repositories' git dirs) |
//! | Session name | `-n <title>` |
//! | Hooks | `<scratch>/settings.json` (`{"hooks": ...}`) via `--settings`; exec form (`command` = the hook program, `args`), no shell (d) |
//! | Resume | fresh: `--session-id <uuid>`; next start: `--resume <uuid>` if `<config home>/projects/*/<uuid>.jsonl` exists; `SessionStart` hook `session_id` follows `/clear` and `/resume` |
//! | Inherited env | markers of a Claude Code that started the server (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, ...) are removed: with them the CLI runs as a child session and keeps no transcript (v 2.1.330) |

use ide_core::{McpServer, Result};

use crate::detect::{PROBE_TIMEOUT, run_capture};
use crate::fallback::{add_dirs, copy_skills, mcp_json};
use crate::{
    AccountDir, AccountStatus, AgentProvider, Caps, HookCaps, HookDecision, HookEvent, HookReply,
    HookTargets, HostEnv, LaunchPlan, ModeSupport, RootPrepared, SessionContext, Skill, Support,
    ToolUse,
};

/// Claude Code's screen rules (`screens/claude.toml`).
static SCREEN: std::sync::LazyLock<crate::screen::ScreenManifest> =
    std::sync::LazyLock::new(|| crate::screen::bundled(include_str!("../screens/claude.toml")));

/// Env vars a running Claude Code sets for the processes it starts (its tool shells). A server
/// started from such a shell (an agent building and running ide) passes them on; a session's
/// `claude` then takes itself for a child of that Claude Code and writes no transcript, so
/// neither `/resume` nor [`AgentProvider::resume_conversation`] finds it. Settings a user sets
/// on purpose (`CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CONFIG_DIR`, ...) are not in this list.
const INHERITED_SESSION_ENV: [&str; 13] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_CHILD_NOTES_DIR",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "AI_AGENT",
];

/// A Claude Code session id: a UUID (`8-4-4-4-12` hex digits). Anything else (a hook input
/// is not trusted) is never passed as an argument or used in a path.
pub(crate) fn is_session_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A random version-4 UUID (std only: two `RandomState` hashes of the time and a counter).
fn new_session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let word = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u64(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        h.finish()
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&word().to_le_bytes());
    b[8..].copy_from_slice(&word().to_le_bytes());
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Claude Code's config home for a session: the account's dir, else the server's
/// `CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn config_home(acct: Option<&AccountDir>) -> Option<std::path::PathBuf> {
    if let Some(a) = acct {
        return Some(a.dir.clone());
    }
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|h| !h.is_empty())
        .map(|h| std::path::PathBuf::from(h).join(".claude"))
}

/// Whether Claude Code kept a transcript of session `id` under `home`
/// (`projects/<cwd as a folder name>/<id>.jsonl`; every project folder is searched, so the
/// CLI's folder-name rules need not be repeated here).
pub(crate) fn has_transcript(home: &std::path::Path, id: &str) -> bool {
    let file = format!("{id}.jsonl");
    std::fs::read_dir(home.join("projects")).is_ok_and(|rd| {
        rd.flatten()
            .any(|e| e.path().join(&file).is_file())
    })
}

/// The transcript file of session `id` under `home` (`projects/*/<id>.jsonl`).
fn transcript_file(home: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    let file = format!("{id}.jsonl");
    std::fs::read_dir(home.join("projects"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
}

/// Longest text of one entry, and of a tool call / result.
const TEXT_MAX: usize = 4000;
const TOOL_MAX: usize = 300;

fn shorten(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(" …(shortened)");
    out
}

/// The text of a `tool_result`'s `content` (a string or text blocks).
fn result_text(c: &serde_json::Value) -> String {
    match c {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Claude Code's transcript (JSONL, one record per line) as entries: user prompts, the
/// assistant's texts, tool calls and results (shortened); subagent (`isSidechain`) and meta
/// records, thinking and other record types are left out. The last `limit` entries.
pub(crate) fn parse_transcript(jsonl: &str, limit: usize) -> Vec<crate::TranscriptEntry> {
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if !matches!(kind, "user" | "assistant")
            || v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true)
            || v.get("isMeta").and_then(|b| b.as_bool()) == Some(true)
        {
            continue;
        }
        let at = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_owned();
        let mut push = |role: &str, text: String| {
            if !text.trim().is_empty() {
                out.push(crate::TranscriptEntry {
                    role: role.to_owned(),
                    text,
                    at: at.clone(),
                });
            }
        };
        let content = v.get("message").and_then(|m| m.get("content"));
        match content {
            Some(serde_json::Value::String(s)) => push(kind, shorten(s, TEXT_MAX)),
            Some(serde_json::Value::Array(blocks)) => {
                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => push(
                            kind,
                            shorten(b.get("text").and_then(|t| t.as_str()).unwrap_or(""), TEXT_MAX),
                        ),
                        "tool_use" => {
                            let name = b.get("name").and_then(|t| t.as_str()).unwrap_or("?");
                            let input = b.get("input").map(|i| i.to_string()).unwrap_or_default();
                            push("tool", format!("call {name} {}", shorten(&input, TOOL_MAX)));
                        }
                        "tool_result" => {
                            let text = result_text(b.get("content").unwrap_or(&serde_json::Value::Null));
                            let err = b.get("is_error").and_then(|e| e.as_bool()) == Some(true);
                            let head = if err { "error" } else { "result" };
                            push("tool", format!("{head}: {}", shorten(&text, TOOL_MAX)));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let skip = out.len().saturating_sub(limit);
    out.split_off(skip)
}

/// Claude Code's file-editing tools (their `tool_input.file_path` / `notebook_path`).
const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The hook program entry of one event (exec form: no shell, so paths need no quoting (d)).
fn hook_entry(program: &str, event: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": program,
        "args": HookTargets::args("claude", event),
        "timeout": 10,
    })
}

/// Claude Code.
#[derive(Debug, Default, Clone, Copy)]
pub struct Claude;

/// Name of the generated plugin for a configuration (kebab-case; must not start with
/// `claude-`/`anthropic-`, which `claude plugin validate` reserves).
pub(crate) fn plugin_name(configuration: &str) -> String {
    format!("ide-{configuration}")
}

impl AgentProvider for Claude {
    fn id(&self) -> &str {
        "claude"
    }
    fn display_name(&self) -> &str {
        "Claude Code"
    }
    fn glyph(&self) -> &str {
        "✻"
    }
    fn color(&self) -> Option<u32> {
        Some(0xd97757)
    }
    fn program(&self) -> &str {
        "claude"
    }
    fn min_version(&self) -> Option<&str> {
        // The flags below were verified on 2.1.318 and 2.1.329.
        Some("2.1.318")
    }
    fn prompt_input(&self) -> crate::PromptInput {
        // A full-screen TUI: one bracketed paste, then Enter (not verified against the real
        // CLI: no prompts are sent to it from tests).
        crate::PromptInput::Paste {
            submit_delay_ms: crate::PASTE_SUBMIT_DELAY_MS,
        }
    }

    fn transcript(
        &self,
        acct: Option<&AccountDir>,
        id: &str,
        limit: usize,
    ) -> Option<std::result::Result<Vec<crate::TranscriptEntry>, String>> {
        Some((|| {
            if !is_session_id(id) {
                return Err(format!("`{id}` is not a Claude Code conversation id"));
            }
            let home = config_home(acct).ok_or("no Claude Code config folder")?;
            let file = transcript_file(&home, id).ok_or("Claude Code kept no transcript of it yet")?;
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok(parse_transcript(&text, limit))
        })())
    }

    /// Esc stops Claude Code's turn (d).
    fn interrupt_keys(&self) -> Option<&'static [u8]> {
        Some(b"\x1b")
    }

    /// A prompt sent while Claude Code works is shown to it during the turn (d).
    fn steers(&self) -> bool {
        true
    }
    fn capabilities(&self) -> Caps {
        Caps {
            accounts: true,
            login: true,
            skills: Support::Native,
            mcp: Support::Native,
            instructions: Support::Native,
            hooks: Support::Native,
            resume: true,
            session_name: true,
            integration: false,
            original_repo: ModeSupport {
                skills: Support::Native,
                mcp: Support::Native,
                instructions: Support::Native,
            },
        }
    }

    fn account_env(&self, acct: &AccountDir) -> Vec<(String, String)> {
        vec![("CLAUDE_CONFIG_DIR".into(), acct.dir.display().to_string())]
    }
    fn login_args(&self) -> Option<Vec<String>> {
        Some(vec!["auth".into(), "login".into()])
    }
    fn account_status(&self, acct: &AccountDir, env: &HostEnv) -> AccountStatus {
        let Ok(path) = self.program_path(env) else {
            return AccountStatus::Unknown;
        };
        let args = ["auth", "status", "--json"].map(String::from);
        match run_capture(&path, &args, &self.account_env(acct), env, PROBE_TIMEOUT) {
            Ok(out) => match serde_json::from_str::<serde_json::Value>(out.stdout.trim()) {
                Ok(v) => match v.get("loggedIn").and_then(|b| b.as_bool()) {
                    Some(true) => AccountStatus::LoggedIn,
                    Some(false) => AccountStatus::LoggedOut,
                    None => AccountStatus::Unknown,
                },
                Err(_) => AccountStatus::Unknown,
            },
            Err(_) => AccountStatus::Unknown,
        }
    }

    fn begin(&self, plan: &mut LaunchPlan, acct: Option<&AccountDir>) {
        for k in INHERITED_SESSION_ENV {
            plan.remove_env(k);
        }
        if let Some(a) = acct {
            plan.env.extend(self.account_env(a));
        }
        if !plan.title.trim().is_empty() {
            let title = plan.title.trim().to_owned();
            plan.arg("-n").arg(title);
        }
    }

    fn new_conversation(&self, plan: &mut LaunchPlan) -> Option<String> {
        let id = new_session_id();
        plan.arg("--session-id").arg(id.clone());
        Some(id)
    }

    fn resume_conversation(
        &self,
        plan: &mut LaunchPlan,
        id: &str,
        acct: Option<&AccountDir>,
    ) -> bool {
        // `--resume` with an unknown id ends the CLI with an error: resume only what it kept.
        if !is_session_id(id) || !config_home(acct).is_some_and(|h| has_transcript(&h, id)) {
            return false;
        }
        plan.arg("--resume").arg(id);
        true
    }

    fn apply_skills(&self, plan: &mut LaunchPlan, skills: &[Skill]) -> Result<()> {
        let name = plugin_name(&plan.configuration);
        let manifest = serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "description": format!("Skills of the ide configuration \"{}\"", plan.configuration),
            "author": { "name": "ide" },
        });
        let manifest_path = plan.write_file(
            "plugin/.claude-plugin/plugin.json",
            serde_json::to_string_pretty(&manifest)
                .expect("json")
                .as_bytes(),
        )?;
        let root = manifest_path
            .parent()
            .and_then(|p| p.parent())
            .expect("plugin root")
            .to_path_buf();
        copy_skills(skills, &root.join("skills"))?;
        plan.arg("--plugin-dir").arg(root.display().to_string());
        Ok(())
    }

    fn prepare_root(
        &self,
        plan: &mut LaunchPlan,
        ctx: &SessionContext,
        _config: &ide_core::Configuration,
        skills: &[Skill],
    ) -> Result<RootPrepared> {
        plan.clear_root_path(".claude/skills")?;
        plan.clear_root_path("CLAUDE.md")?;
        if !skills.is_empty() {
            copy_skills(skills, &plan.root_dir(".claude/skills")?)?;
        }
        if let Some(g) = &ctx.guidance {
            plan.write_root_file("CLAUDE.md", g.as_bytes())?;
        }
        Ok(RootPrepared {
            skills: true,
            guidance: ctx.guidance.is_some(),
        })
    }
    fn apply_dirs(&self, plan: &mut LaunchPlan, dirs: &[std::path::PathBuf]) -> Result<()> {
        add_dirs(plan, dirs);
        Ok(())
    }
    fn apply_mcp(&self, plan: &mut LaunchPlan, servers: &[McpServer]) -> Result<()> {
        let json = serde_json::to_string_pretty(&mcp_json(servers)).expect("json");
        let path = plan.write_file("mcp.json", json.as_bytes())?;
        plan.arg("--mcp-config").arg(path.display().to_string());
        Ok(())
    }

    fn apply_instructions(&self, plan: &mut LaunchPlan, text: &str) -> Result<()> {
        let path = plan.write_file("instructions.md", text.as_bytes())?;
        plan.arg("--append-system-prompt-file")
            .arg(path.display().to_string());
        Ok(())
    }

    fn screen_manifest(&self) -> Option<&'static crate::screen::ScreenManifest> {
        Some(&SCREEN)
    }

    fn hook_caps(&self) -> HookCaps {
        HookCaps {
            status: Support::Native,
            guard_edits: Support::Native,
            guard_shell: Support::Unsupported,
        }
    }

    fn apply_hooks(&self, plan: &mut LaunchPlan, hooks: &HookTargets) -> Result<()> {
        let Some(program) = &hooks.program else {
            return Ok(());
        };
        let p = program.display().to_string();
        let group = |event: &str, matcher: Option<&str>| {
            let mut g = serde_json::json!({ "hooks": [hook_entry(&p, event)] });
            if let Some(m) = matcher {
                g["matcher"] = m.into();
            }
            serde_json::json!([g])
        };
        let mut settings = serde_json::json!({
            "hooks": {
                "SessionStart": group("SessionStart", None),
                "UserPromptSubmit": group("UserPromptSubmit", None),
                "Stop": group("Stop", None),
                // Waiting for the user: a tool permission or an MCP question (not idle_prompt:
                // the turn already ended with Stop).
                "Notification": group("Notification", Some("permission_prompt|elicitation_dialog")),
                // Every tool: edits are guarded, and any call marks the session working (a
                // turn the CLI resumes on its own, e.g. with a background agent's result,
                // sends no UserPromptSubmit). Every hook run is a process start.
                "PreToolUse": group("PreToolUse", None),
                // Subagents, also background ones that outlive the turn (Stop).
                "SubagentStart": group("SubagentStart", None),
                "SubagentStop": group("SubagentStop", None),
            }
        });
        // The bridge's tools run without Claude's prompt: ide asks itself (`mcp__<server>`
        // allows every tool of a server (d)).
        if let Some(server) = &hooks.trusted_mcp {
            settings["permissions"] = serde_json::json!({ "allow": [format!("mcp__{server}")] });
        }
        let json = serde_json::to_string_pretty(&settings).expect("json");
        let path = plan.write_file("settings.json", json.as_bytes())?;
        plan.arg("--settings").arg(path.display().to_string());
        Ok(())
    }

    fn parse_hook(&self, event: &str, input: &[u8]) -> Option<HookEvent> {
        let v: serde_json::Value = serde_json::from_slice(input).unwrap_or_default();
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        match event {
            "SessionStart" => Some(HookEvent::SessionStart {
                conversation: Some(s("session_id"))
                    .filter(|id| is_session_id(id))
                    .map(str::to_owned),
            }),
            "UserPromptSubmit" => Some(HookEvent::PromptSubmitted),
            "Stop" => Some(HookEvent::Stopped),
            // Without an id the start and its end cannot be paired: ignored.
            "SubagentStart" => Some(HookEvent::BackgroundStarted {
                id: Some(s("agent_id")).filter(|i| !i.is_empty())?.to_owned(),
            }),
            "SubagentStop" => Some(HookEvent::BackgroundEnded {
                id: Some(s("agent_id")).filter(|i| !i.is_empty())?.to_owned(),
            }),
            "Notification" => match s("notification_type") {
                "permission_prompt" => Some(HookEvent::NeedsInput { permission: true }),
                "elicitation_dialog" => Some(HookEvent::NeedsInput { permission: false }),
                _ => None,
            },
            "PreToolUse" => {
                let tool = s("tool_name").to_owned();
                let ti = v.get("tool_input").cloned().unwrap_or_default();
                let field = |k: &str| ti.get(k).and_then(|x| x.as_str()).map(str::to_owned);
                Some(HookEvent::PreTool(if EDIT_TOOLS.contains(&tool.as_str()) {
                    let paths = field("file_path")
                        .or_else(|| field("notebook_path"))
                        .into_iter()
                        .collect();
                    ToolUse::Edit { tool, paths }
                } else if tool == "Bash" {
                    ToolUse::Shell {
                        tool,
                        command: field("command").unwrap_or_default(),
                    }
                } else {
                    ToolUse::Other { tool }
                }))
            }
            _ => None,
        }
    }

    fn hook_reply(&self, event: &str, decision: &HookDecision) -> HookReply {
        match (event, decision) {
            ("PreToolUse", HookDecision::Deny { reason }) => HookReply {
                stdout: serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                })
                .to_string(),
                exit_code: 0,
            },
            // Silent exit 0: no decision, the normal permission flow continues (d).
            _ => HookReply::default(),
        }
    }
}
