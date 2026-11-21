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

use ide_core::{McpServer, Result};

use crate::detect::{PROBE_TIMEOUT, run_capture};
use crate::fallback::{add_dirs, copy_skills, mcp_json};
use crate::{
    AccountDir, AccountStatus, AgentProvider, Caps, HostEnv, LaunchPlan, ModeSupport, RootPrepared,
    SessionContext, Skill, Support,
};

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
    fn capabilities(&self) -> Caps {
        Caps {
            accounts: true,
            login: true,
            skills: Support::Native,
            mcp: Support::Native,
            instructions: Support::Native,
            hooks: Support::Unsupported,
            resume: false,
            session_name: true,
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
        if let Some(a) = acct {
            plan.env.extend(self.account_env(a));
        }
        if !plan.title.trim().is_empty() {
            let title = plan.title.trim().to_owned();
            plan.arg("-n").arg(title);
        }
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
}
