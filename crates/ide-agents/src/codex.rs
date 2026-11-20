//! OpenAI Codex CLI (`codex`).
//!
//! | Common | Mapping (verified with 0.160.1, see docs/agent-providers.md) |
//! |---|---|
//! | Account | `CODEX_HOME=<account dir>` |
//! | Login | `codex login`; status `codex login status` (exit 0 = logged in) |
//! | Daemon | `--no-daemon` (never adopt/leave the shared app-server daemon) |
//! | Skills | fallback: listed in the developer instructions (Codex only discovers skills in the repo, `$HOME/.agents/skills` and `$CODEX_HOME/skills`; `skills.config` only enables/disables discovered ones) |
//! | MCP | `-c mcp_servers.<name>={command="..", args=[..], env={..}}` |
//! | Instructions | `-c developer_instructions="..."` (added as a developer message; `model_instructions_file` would replace the built-in instructions) |

use ide_core::{McpServer, Result};

use crate::detect::{PROBE_TIMEOUT, run_capture};
use crate::fallback::toml_string;
use crate::{AccountDir, AccountStatus, AgentProvider, Caps, HostEnv, LaunchPlan, Support};

/// Codex CLI.
#[derive(Debug, Default, Clone, Copy)]
pub struct Codex;

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
    fn capabilities(&self) -> Caps {
        Caps {
            accounts: true,
            login: true,
            skills: Support::Fallback,
            mcp: Support::Native,
            instructions: Support::Native,
            hooks: Support::Unsupported,
            resume: false,
            session_name: false,
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
