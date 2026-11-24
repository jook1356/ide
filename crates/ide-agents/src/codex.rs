//! OpenAI Codex CLI (`codex`).
//!
//! | Common | Mapping (verified with 0.160.1, see docs/agent-providers.md) |
//! |---|---|
//! | Account | `CODEX_HOME=<account dir>` |
//! | Login | `codex login`; status `codex login status` (exit 0 = logged in) |
//! | Daemon | `--no-daemon` (never adopt/leave the shared app-server daemon) |
//! | Skills | workspace: `<session folder>/.agents/skills/<name>/` (discovered from the cwd, also outside git: verified); original repo: fallback, listed in the developer instructions (nothing may be written into the repo; see docs/agent-providers.md for the rejected CODEX_HOME overlay) |
//! | Guidance | workspace: `<session folder>/AGENTS.md` (verified: read from a non-git cwd) |
//! | Outside dirs | `--add-dir` (writable: legacy worktrees, the repositories' git dirs) |
//! | First prompt | `-- "<prompt>"` (last; a child session's task, new sessions only) |
//! | Sandbox | `--sandbox workspace-write` unless the account's `config.toml` sets `sandbox_mode` (a session folder is not a trusted project: Codex would start read-only and refuse `--add-dir`, v 0.172.0) |
//! | MCP | `-c mcp_servers.<name>={command="..", args=[..], env={..}}` |
//! | Instructions | `-c developer_instructions="..."` (added as a developer message; `model_instructions_file` would replace the built-in instructions) |
//! | Hooks | account integration (user-installed): ide's entries in `CODEX_HOME/hooks.json`, approved once in Codex with `/hooks` (d) |
//! | Windows sandbox | account integration also runs `codex sandbox setup --elevated --current-user` once per Codex home (UAC; it writes `[windows] sandbox = "elevated"` and `.sandbox/setup_marker.json`, v 0.172.0): without it Codex is read-only and refuses `--add-dir` |
//! | Resume | Codex picks its own id: a fresh start stores the placeholder `started:<unix ms>`; the `SessionStart` hook's `session_id` (with the integration) replaces it. Next start: `codex resume <uuid> ...` (the subcommand first; it takes every option used here, v 0.172.0) when `<CODEX_HOME>/sessions/**/rollout-*-<uuid>.jsonl[.zst]` exists; a placeholder resolves to the earliest rollout of the same cwd written since that start. Codex writes the rollout with the first prompt: an untouched session starts fresh |

use ide_core::{McpServer, Result};

use crate::detect::{PROBE_TIMEOUT, run_capture};
use crate::fallback::{add_dirs, copy_skills, toml_string};
use crate::{
    AccountDir, AccountStatus, AgentProvider, Caps, HookCaps, HookDecision, HookEvent, HookReply,
    HookTargets, HostEnv, IntegrationReport, IntegrationStatus, LaunchPlan, ModeSupport,
    RootPrepared, SessionContext, Skill, Support, ToolUse,
};

/// Codex CLI.
#[derive(Debug, Default, Clone, Copy)]
pub struct Codex;

/// Codex's screen rules (`screens/codex.toml`): its status without hooks.
static SCREEN: std::sync::LazyLock<crate::screen::ScreenManifest> =
    std::sync::LazyLock::new(|| crate::screen::bundled(include_str!("../screens/codex.toml")));

/// Codex's config home: the account's dir, else `CODEX_HOME`, else `~/.codex`.
fn codex_home(acct: Option<&AccountDir>) -> Option<std::path::PathBuf> {
    if let Some(a) = acct {
        return Some(a.dir.clone());
    }
    if let Some(d) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|h| !h.is_empty())
        .map(|h| std::path::PathBuf::from(h).join(".codex"))
}

/// The user set `sandbox_mode` in Codex's `config.toml` (read only; an unreadable file counts
/// as not set).
pub(crate) fn sets_sandbox(acct: Option<&AccountDir>) -> bool {
    config_table(acct).contains_key("sandbox_mode")
}

/// Codex's `config.toml` of a Codex home as a table (missing or unreadable: empty).
fn config_table(acct: Option<&AccountDir>) -> toml::Table {
    codex_home(acct)
        .and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok())
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .unwrap_or_default()
}

/// Can Codex write in this home's sessions? Outside Windows always; on Windows Codex runs
/// commands only inside its Windows sandbox, set up once per Codex home (`codex sandbox setup
/// --elevated`, which writes `[windows] sandbox = "elevated"` and `.sandbox/setup_marker.json`,
/// v 0.172.0); without it every session is read-only and `--add-dir` is refused. A user who
/// chose `sandbox_mode` or another `windows.sandbox` themselves is left alone.
pub(crate) fn sandbox_ready(acct: Option<&AccountDir>) -> bool {
    sandbox_ready_in(acct, cfg!(windows))
}

pub(crate) fn sandbox_ready_in(acct: Option<&AccountDir>, windows: bool) -> bool {
    if !windows {
        return true;
    }
    let t = config_table(acct);
    if t.contains_key("sandbox_mode") {
        return true;
    }
    match t.get("windows").and_then(|w| w.get("sandbox")).and_then(|s| s.as_str()) {
        None => false,
        Some("elevated") => codex_home(acct)
            .is_some_and(|h| h.join(".sandbox").join("setup_marker.json").is_file()),
        Some(_) => true,
    }
}

/// Run `codex sandbox setup --elevated --current-user` for the Codex home `home`, elevated:
/// Windows shows its UAC prompt (the user's consent; refusing it is an error). Blocking.
fn setup_windows_sandbox(codex: &std::path::Path, home: &std::path::Path) -> Result<()> {
    use std::process::Command;
    let fail = |d: String| ide_core::Error::SandboxSetup(d);
    // Never from tests: it changes the machine and asks the user (UAC).
    if cfg!(test) || std::env::var_os("IDE_TEST_TMP").is_some() {
        return Err(fail("not run from tests".into()));
    }
    let q = |p: &std::path::Path| p.display().to_string().replace('\'', "''");
    let tag = format!("ide-codex-sandbox-{}", std::process::id());
    let dir = std::env::temp_dir();
    let (script, log) = (dir.join(format!("{tag}.ps1")), dir.join(format!("{tag}.log")));
    // The elevated process gets a fresh environment: CODEX_HOME is set inside the script.
    let body = format!(
        "$env:CODEX_HOME = '{home}'\r\n& '{codex}' sandbox setup --elevated --current-user --codex-home '{home}' *> '{log}'\r\nexit $LASTEXITCODE\r\n",
        home = q(home),
        codex = q(codex),
        log = q(&log),
    );
    std::fs::write(&script, body).map_err(|e| fail(e.to_string()))?;
    // Start-Process joins the argument list with blanks: the script path is quoted for it.
    let outer = format!(
        "try {{ $p = Start-Process -FilePath powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','\"{}\"'); exit $p.ExitCode }} \
         catch {{ exit 1223 }}",
        q(&script)
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .status();
    let detail = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&log);
    match status {
        Ok(s) if s.success() => Ok(()),
        // ERROR_CANCELLED: the UAC prompt was refused.
        Ok(s) if s.code() == Some(1223) => Err(fail("the administrator prompt was refused".into())),
        Ok(s) => Err(fail(format!(
            "exit {}: {}",
            s.code().unwrap_or(-1),
            detail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")
        ))),
        Err(e) => Err(fail(e.to_string())),
    }
}

/// The `-c` override value for one MCP server (an inline TOML table).
pub(crate) fn mcp_override(s: &McpServer) -> String {
    let args: Vec<String> = s.args.iter().map(|a| toml_string(a)).collect();
    let mut v = format!(
        "mcp_servers.{}={{command={}, args=[{}]",
        s.name,
        toml_string(&s.command),
        args.join(", ")
    );
    if !s.env.is_empty() {
        let env: Vec<String> = s
            .env
            .iter()
            .map(|(k, val)| format!("{}={}", toml_string(k), toml_string(val)))
            .collect();
        v.push_str(&format!(", env={{{}}}", env.join(", ")));
    }
    v.push('}');
    v
}

impl AgentProvider for Codex {
    fn id(&self) -> &str {
        "codex"
    }
    fn display_name(&self) -> &str {
        "Codex"
    }
    fn glyph(&self) -> &str {
        "◆"
    }
    fn color(&self) -> Option<u32> {
        Some(0x7aa2f7)
    }
    fn program(&self) -> &str {
        "codex"
    }
    fn min_version(&self) -> Option<&str> {
        // `--no-daemon` and the config keys below were verified on 0.160.1.
        Some("0.160.0")
    }
    fn prompt_input(&self) -> crate::PromptInput {
        // A full-screen TUI: one bracketed paste, then Enter (not verified against the real
        // CLI: no prompts are sent to it from tests).
        crate::PromptInput::Paste {
            submit_delay_ms: crate::PASTE_SUBMIT_DELAY_MS,
        }
    }
    fn first_prompt_args(&self, prompt: &str) -> Option<Vec<String>> {
        // `codex [options] -- "<prompt>"` (the TUI's optional `[PROMPT]`): submitted as the
        // first turn after the trust question. `--` ends the options (`--add-dir` and
        // `-c` take values; a prompt may start with `-`).
        Some(vec!["--".into(), prompt.to_owned()])
    }

    /// Esc stops Codex's turn ("esc to interrupt" on its screen) (a).
    fn interrupt_keys(&self) -> Option<&'static [u8]> {
        Some(b"\x1b")
    }

    /// A prompt sent while Codex works is sent to it in that turn (a).
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
            hooks: Support::Unsupported,
            resume: true,
            session_name: false,
            integration: true,
            original_repo: ModeSupport {
                skills: Support::Fallback,
                mcp: Support::Native,
                instructions: Support::Native,
            },
        }
    }

    fn screen_manifest(&self) -> Option<&'static crate::screen::ScreenManifest> {
        Some(&SCREEN)
    }

    fn hook_caps(&self) -> HookCaps {
        // Only with the account integration installed (and approved in Codex).
        HookCaps {
            status: Support::Native,
            guard_edits: Support::Native,
            guard_shell: Support::Native,
        }
    }

    fn default_state_dirs(&self) -> Vec<std::path::PathBuf> {
        // Codex's config home without an account: `CODEX_HOME`, else `~/.codex`.
        if let Some(d) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
            return vec![d.into()];
        }
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .filter(|h| !h.is_empty())
            .map(|h| std::path::PathBuf::from(h).join(".codex"))
            .into_iter()
            .collect()
    }

    fn integration_status(&self, acct: &AccountDir, hooks: &HookTargets) -> IntegrationStatus {
        let Some(program) = &hooks.program else {
            return IntegrationStatus::NotNeeded;
        };
        let Ok(v) = read_hooks(acct) else {
            return IntegrationStatus::Missing;
        };
        let ours = our_commands(&v);
        if ours.is_empty() {
            IntegrationStatus::Missing
        } else if ours != expected_commands(program) || !timeouts_current(&v) {
            IntegrationStatus::Outdated
        } else if !sandbox_ready(Some(acct)) {
            // The Windows sandbox of this Codex home is not set up yet: install does it.
            IntegrationStatus::Missing
        } else {
            IntegrationStatus::Installed
        }
    }

    fn integration_install(
        &self,
        acct: &AccountDir,
        hooks: &HookTargets,
    ) -> Result<IntegrationReport> {
        let Some(program) = &hooks.program else {
            return Ok(IntegrationReport::default());
        };
        let mut v = read_hooks(acct)?;
        if !v.is_object() {
            v = serde_json::json!({});
        }
        strip_ours(&mut v);
        if !v.get("hooks").is_some_and(|h| h.is_object()) {
            v["hooks"] = serde_json::json!({});
        }
        for (event, matcher) in HOOK_EVENTS {
            let mut group = serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": hook_command(program, event),
                    "timeout": crate::hooks::hook_timeout_secs(event),
                }]
            });
            if let Some(m) = matcher {
                group["matcher"] = m.into();
            }
            let list = v["hooks"]
                .as_object_mut()
                .expect("object")
                .entry(event)
                .or_insert_with(|| serde_json::json!([]));
            if !list.is_array() {
                *list = serde_json::json!([]);
            }
            list.as_array_mut().expect("array").push(group);
        }
        let path = hooks_file(acct);
        ide_core::ensure_dir(&acct.dir)?;
        let text = serde_json::to_string_pretty(&v).expect("json");
        std::fs::write(&path, text)
            .map_err(|e| ide_core::Error::io(ide_core::IoAction::Writing, &path, e))?;
        // Windows: Codex writes only inside its sandbox, set up once per Codex home (UAC).
        if !sandbox_ready(Some(acct)) {
            let codex = self.program_path(&crate::HostEnv::from_process())?;
            setup_windows_sandbox(&codex, &acct.dir)?;
            if !sandbox_ready(Some(acct)) {
                return Err(ide_core::Error::SandboxSetup(
                    "Codex reported success but its sandbox setup is not recorded".into(),
                ));
            }
        }
        Ok(IntegrationReport {
            approve_with: Some("/hooks".into()),
        })
    }

    fn integration_uninstall(&self, acct: &AccountDir) -> Result<()> {
        let path = hooks_file(acct);
        if !path.exists() {
            return Ok(());
        }
        let mut v = read_hooks(acct)?;
        strip_ours(&mut v);
        let text = serde_json::to_string_pretty(&v).expect("json");
        std::fs::write(&path, text)
            .map_err(|e| ide_core::Error::io(ide_core::IoAction::Writing, &path, e))
    }

    fn parse_hook(&self, event: &str, input: &[u8]) -> Option<HookEvent> {
        let v: serde_json::Value = serde_json::from_slice(input).unwrap_or_default();
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        match event {
            // `session_id` (d): the thread Codex started or resumed (`/new`, `/resume`). Hook
            // input is untrusted: only a UUID is kept (it becomes an argument and a file name).
            "SessionStart" => Some(HookEvent::SessionStart {
                conversation: Some(s("session_id"))
                    .filter(|id| crate::claude::is_session_id(id))
                    .map(str::to_owned),
            }),
            "UserPromptSubmit" => Some(HookEvent::PromptSubmitted),
            "Stop" => Some(HookEvent::Stopped),
            "PermissionRequest" => Some(HookEvent::NeedsInput { permission: true }),
            "PreToolUse" => {
                let tool = s("tool_name").to_owned();
                let ti = v.get("tool_input").cloned().unwrap_or_default();
                Some(HookEvent::PreTool(match tool.as_str() {
                    "apply_patch" | "Edit" | "Write" => {
                        let mut paths = Vec::new();
                        patch_paths(&ti, &mut paths);
                        if let Some(p) = ti.get("file_path").and_then(|p| p.as_str()) {
                            paths.push(p.to_owned());
                        }
                        // Patch paths are relative to the session's cwd.
                        let cwd = std::path::Path::new(s("cwd"));
                        let paths = paths
                            .into_iter()
                            .map(|p| {
                                let pp = std::path::Path::new(&p);
                                if pp.is_absolute() || s("cwd").is_empty() {
                                    p
                                } else {
                                    cwd.join(pp).display().to_string()
                                }
                            })
                            .collect();
                        ToolUse::Edit { tool, paths }
                    }
                    "Bash" | "exec_command" | "shell" | "PowerShell" => ToolUse::Shell {
                        cwd: Some(s("cwd")).filter(|c| !c.is_empty()).map(str::to_owned),
                        command: ti
                            .get("command")
                            .map(|c| match c {
                                serde_json::Value::String(t) => t.clone(),
                                other => other.to_string(),
                            })
                            .unwrap_or_default(),
                        tool,
                    },
                    _ => ToolUse::Other { tool },
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
            _ => HookReply::default(),
        }
    }

    fn account_env(&self, acct: &AccountDir) -> Vec<(String, String)> {
        vec![("CODEX_HOME".into(), acct.dir.display().to_string())]
    }
    fn login_args(&self) -> Option<Vec<String>> {
        Some(vec!["login".into()])
    }
    fn account_status(&self, acct: &AccountDir, env: &HostEnv) -> AccountStatus {
        let Ok(path) = self.program_path(env) else {
            return AccountStatus::Unknown;
        };
        let args = ["login", "status"].map(String::from);
        match run_capture(&path, &args, &self.account_env(acct), env, PROBE_TIMEOUT) {
            Ok(out) if out.status == Some(0) => AccountStatus::LoggedIn,
            Ok(out) if format!("{}{}", out.stdout, out.stderr).contains("Not logged in") => {
                AccountStatus::LoggedOut
            }
            _ => AccountStatus::Unknown,
        }
    }

    fn begin(&self, plan: &mut LaunchPlan, acct: Option<&AccountDir>) {
        if let Some(a) = acct {
            plan.env.extend(self.account_env(a));
        }
        plan.arg("--no-daemon");
        // A session folder is neither a git repository nor a trusted project, so Codex would
        // start read-only and refuse `--add-dir` ("the effective permissions do not allow
        // additional writable roots", v 0.172.0). Ask for what a trusted project gets, unless
        // the user chose a sandbox in the account's config.toml.
        if !sets_sandbox(acct) {
            plan.arg("--sandbox").arg("workspace-write");
        }
    }

    /// Codex takes no id up front: remember when this conversation started, so a later start
    /// can find its rollout without the `SessionStart` hook.
    fn new_conversation(&self, _plan: &mut LaunchPlan) -> Option<String> {
        Some(format!("{STARTED}{}", now_ms()))
    }

    fn resume_conversation(
        &self,
        plan: &mut LaunchPlan,
        id: &str,
        acct: Option<&AccountDir>,
    ) -> Option<String> {
        let sessions = codex_home(acct)?.join("sessions");
        let id = match id.strip_prefix(STARTED) {
            Some(ms) => started_rollout(&sessions, &plan.cwd, ms.parse().ok()?)?,
            // `codex resume` with an unknown id ends the CLI: resume only what it kept.
            None if crate::claude::is_session_id(id) && has_rollout(&sessions, id) => id.to_owned(),
            None => return None,
        };
        // The subcommand goes first: every option after it belongs to `codex resume`.
        plan.args.splice(0..0, ["resume".to_owned(), id.clone()]);
        Some(id)
    }

    fn prepare_root(
        &self,
        plan: &mut LaunchPlan,
        ctx: &SessionContext,
        _config: &ide_core::Configuration,
        skills: &[Skill],
    ) -> Result<RootPrepared> {
        plan.clear_root_path(".agents/skills")?;
        plan.clear_root_path("AGENTS.md")?;
        if !skills.is_empty() {
            copy_skills(skills, &plan.root_dir(".agents/skills")?)?;
        }
        if let Some(g) = &ctx.guidance {
            plan.write_root_file("AGENTS.md", g.as_bytes())?;
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
        for s in servers {
            plan.arg("-c").arg(mcp_override(s));
        }
        Ok(())
    }

    fn apply_instructions(&self, plan: &mut LaunchPlan, text: &str) -> Result<()> {
        plan.arg("-c")
            .arg(format!("developer_instructions={}", toml_string(text)));
        // Keep a copy for inspection (not read by Codex).
        plan.write_file("instructions.md", text.as_bytes())?;
        Ok(())
    }
}

// ---------------------------------------------------------------- conversations (resume)

/// The stored placeholder of a conversation Codex has not named to ide yet:
/// `started:<unix ms of the launch>`.
const STARTED: &str = "started:";

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Codex's rollout files under `sessions` (`YYYY/MM/DD/rollout-<time>-<uuid>.jsonl`, older
/// ones also `.jsonl.zst`).
fn rollouts(sessions: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, depth: u8, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let path = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 4 {
                    walk(&path, depth + 1, out);
                }
            } else if e.file_name().to_string_lossy().starts_with("rollout-") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(sessions, 0, &mut out);
    out
}

/// Did Codex keep a rollout of thread `id`?
fn has_rollout(sessions: &std::path::Path, id: &str) -> bool {
    let (plain, packed) = (format!("-{id}.jsonl"), format!("-{id}.jsonl.zst"));
    rollouts(sessions).iter().any(|p| {
        let n = p.file_name().unwrap_or_default().to_string_lossy();
        n.ends_with(&plain) || n.ends_with(&packed)
    })
}

/// The same folder as Codex wrote it in a rollout (`\\?\` prefix, separators, trailing
/// separator and, on Windows, letter case do not matter).
fn same_dir(a: &str, b: &std::path::Path) -> bool {
    let norm = |s: &str| {
        let s = s.strip_prefix(r"\\?\").unwrap_or(s);
        let s = if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.to_owned() };
        s.trim_end_matches(['\\', '/']).to_owned()
    };
    norm(a) == norm(&b.display().to_string())
}

/// The thread of a conversation stored as a placeholder: the earliest rollout (Codex writes
/// it with the first prompt) whose `session_meta` names `cwd`, written since the launch at
/// `since_ms`. Without the hook's exact id two Codex sessions in one folder may swap
/// threads (shared-folder children); a later `/new` inside the TUI is not followed.
fn started_rollout(sessions: &std::path::Path, cwd: &std::path::Path, since_ms: u128) -> Option<String> {
    use std::io::BufRead;
    let since = std::time::UNIX_EPOCH
        .checked_add(std::time::Duration::from_millis(since_ms.try_into().ok()?))?;
    let mut found: Option<(std::time::SystemTime, String)> = None;
    for path in rollouts(sessions) {
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        // Creation time where the file system keeps one, else the last write (never earlier).
        let Some(born) = meta.created().or_else(|_| meta.modified()).ok().filter(|t| *t >= since) else {
            continue;
        };
        if found.as_ref().is_some_and(|(t, _)| *t <= born) {
            continue;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut first = String::new();
        if std::io::BufReader::new(file).read_line(&mut first).is_err() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&first).unwrap_or_default();
        let p = &v["payload"];
        let id = p["id"].as_str().unwrap_or_default();
        if v["type"] == "session_meta"
            && crate::claude::is_session_id(id)
            && p["cwd"].as_str().is_some_and(|c| same_dir(c, cwd))
        {
            found = Some((born, id.to_owned()));
        }
    }
    found.map(|(_, id)| id)
}

// ---------------------------------------------------------------- hooks (account integration)

/// The hook events ide registers in `CODEX_HOME/hooks.json`, with their matchers.
const HOOK_EVENTS: [(&str, Option<&str>); 5] = [
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("Stop", None),
    ("PermissionRequest", None),
    // File edits and shell commands (writes outside the session need the user's approval,
    // `docs/write-guard.md`; every hook run is a process start).
    ("PreToolUse", Some("apply_patch|Edit|Write|Bash|exec_command|shell|PowerShell")),
];

/// What marks ide's entries in `hooks.json` (their command runs the hook program for Codex).
const MARK: &str = " hook --provider codex ";

/// The command line of one event: the program, quoted only when it has blanks (the command
/// runs through a shell (a)).
fn hook_command(program: &std::path::Path, event: &str) -> String {
    let p = program.display().to_string();
    let p = if p.chars().any(char::is_whitespace) {
        format!("\"{p}\"")
    } else {
        p
    };
    format!("{p}{MARK}{event}")
}

fn hooks_file(acct: &AccountDir) -> std::path::PathBuf {
    acct.dir.join("hooks.json")
}

fn read_hooks(acct: &AccountDir) -> Result<serde_json::Value> {
    let path = hooks_file(acct);
    match std::fs::read_to_string(&path) {
        Ok(t) if t.trim().is_empty() => Ok(serde_json::json!({})),
        Ok(t) => serde_json::from_str(&t).map_err(|e| {
            ide_core::Error::io(
                ide_core::IoAction::Reading,
                &path,
                std::io::Error::other(e.to_string()),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(ide_core::Error::io(ide_core::IoAction::Reading, &path, e)),
    }
}

/// Is this handler one of ide's?
fn is_ours(h: &serde_json::Value) -> bool {
    h.get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.contains(MARK))
}

/// Remove ide's handlers (and groups / events left empty) from a hooks.json value.
fn strip_ours(v: &mut serde_json::Value) {
    let Some(events) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return;
    };
    for groups in events.values_mut() {
        if let Some(groups) = groups.as_array_mut() {
            for g in groups.iter_mut() {
                if let Some(hs) = g.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                    hs.retain(|h| !is_ours(h));
                }
            }
            groups.retain(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .is_none_or(|h| !h.is_empty())
            });
        }
    }
    events.retain(|_, g| g.as_array().is_none_or(|a| !a.is_empty()));
}

/// ide's commands found in a hooks.json value.
fn our_commands(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(events) = v.get("hooks").and_then(|h| h.as_object()) {
        for groups in events.values() {
            for g in groups.as_array().into_iter().flatten() {
                for h in g.get("hooks").and_then(|h| h.as_array()).into_iter().flatten() {
                    if is_ours(h) {
                        out.push(h["command"].as_str().unwrap_or_default().to_owned());
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Do ide's handlers have the time limits ide sets now (an older install let a write
/// outside the session wait only 330 s for the user's answer)?
fn timeouts_current(v: &serde_json::Value) -> bool {
    let Some(events) = v.get("hooks").and_then(|h| h.as_object()) else {
        return true;
    };
    events.iter().all(|(event, groups)| {
        groups
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|g| g.get("hooks").and_then(|h| h.as_array()).into_iter().flatten())
            .filter(|h| is_ours(h))
            .all(|h| h.get("timeout").and_then(|t| t.as_u64()) == Some(crate::hooks::hook_timeout_secs(event)))
    })
}

fn expected_commands(program: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = HOOK_EVENTS
        .iter()
        .map(|(e, _)| hook_command(program, e))
        .collect();
    v.sort();
    v
}

/// The files an `apply_patch` input touches (`*** Add/Update/Delete File:`, `*** Move to:`),
/// found in any string of the tool input.
fn patch_paths(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => {
            for line in s.lines() {
                let l = line.trim();
                for head in [
                    "*** Add File: ",
                    "*** Update File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ] {
                    if let Some(p) = l.strip_prefix(head) {
                        let p = p.trim().to_owned();
                        if !p.is_empty() && !out.contains(&p) {
                            out.push(p);
                        }
                    }
                }
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| patch_paths(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| patch_paths(x, out)),
        _ => {}
    }
}
