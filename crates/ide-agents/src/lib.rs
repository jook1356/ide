//! Agent providers: every agent CLI (Claude Code, Codex, a plain shell, declarative TOML
//! providers) implements one contract, [`AgentProvider`]. The IDE only deals with
//! provider-agnostic data — accounts ([`ide_core::Account`]) and configurations
//! ([`ide_core::Configuration`]: skills, MCP servers, instructions) — and each provider
//! translates that into its own flags, env vars and generated files.
//!
//! Rules (see `docs/agent-providers.md`):
//! - Never pollute the repo: inject through flags, env vars and the per-session scratch dir
//!   `sessions/<id>/agent/` ([`LaunchPlan::scratch`]).
//! - Accounts are never written (only by the CLI's own login flow).
//! - Unsupported capability -> documented fallback (skills become instructions) or a note in
//!   [`Assembled::notes`]; never silently dropped.
//! - Provider-specific code stays in this crate: callers go through [`Providers`] and the trait.
//!
//! ```no_run
//! # fn main() -> ide_core::Result<()> {
//! use ide_agents::*;
//! let paths = ide_core::Paths::from_env()?;
//! let providers = Providers::load(&paths.providers_dir());
//! let env = HostEnv::from_process();
//! let claude = providers.require("claude")?;
//! println!("{:?}", claude.detect(&env));
//! # let (session, configuration): (ide_core::Session, ide_core::Configuration) = todo!();
//! let input = SessionInput::for_session(&paths, &session, None, &configuration);
//! let built = assemble(claude, &input, &env)?;
//! // spawn built.spec in a PTY
//! # Ok(()) }
//! ```

mod claude;
mod codex;
pub mod declarative;
mod detect;
pub mod fallback;
mod plan;
mod registry;
mod shell;
mod skills;

use std::path::{Path, PathBuf};

use ide_core::{Account, Configuration, LaunchSpec, McpServer, Paths, Result, Session};
use serde::{Deserialize, Serialize};

pub use claude::Claude;
pub use codex::Codex;
pub use declarative::{DeclarativeProvider, DeclarativeSpec};
pub use detect::{Detection, parse_version, version_at_least};
pub use plan::LaunchPlan;
pub use registry::Providers;
pub use shell::Shell;
pub use skills::Skill;

/// The environment programs are resolved in (`PATH`, `PATHEXT`, `COMSPEC`, `SHELL`).
pub type HostEnv = ide_core::LaunchEnv;

/// An account as seen by a provider (its id and config home).
pub type AccountDir = Account;

/// How well a provider supports a configuration feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    /// Passed to the CLI in its own format.
    Native,
    /// Emulated (e.g. skills listed in the instructions).
    Fallback,
    /// Not passed on (the UI should disable it; launches record a note).
    Unsupported,
}

/// What a provider can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Sessions can use accounts of this provider.
    pub accounts: bool,
    /// [`AgentProvider::login_args`] exists.
    pub login: bool,
    /// Configuration skills.
    pub skills: Support,
    /// Configuration MCP servers.
    pub mcp: Support,
    /// Configuration instructions.
    pub instructions: Support,
    /// Status hooks (working / idle / needs input).
    pub hooks: Support,
    /// The session can be resumed by the CLI.
    pub resume: bool,
    /// The session title is passed as the CLI's session name.
    pub session_name: bool,
}

impl Caps {
    /// Nothing supported (a plain program).
    pub const NONE: Caps = Caps {
        accounts: false,
        login: false,
        skills: Support::Unsupported,
        mcp: Support::Unsupported,
        instructions: Support::Unsupported,
        hooks: Support::Unsupported,
        resume: false,
        session_name: false,
    };
}

/// Logged-in state of an account, as reported by the CLI itself (best effort).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountStatus {
    /// The CLI reports a login.
    LoggedIn,
    /// The CLI reports no login.
    LoggedOut,
    /// Not determinable (no status command, CLI missing, timeout, unexpected output).
    Unknown,
}

/// Where status hooks should report to (phase "hooks"; no provider uses it yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookTargets {}

/// An agent CLI. Every assembly step has a default (fallback) implementation; providers
/// override what their CLI supports natively.
pub trait AgentProvider: Send + Sync {
    /// Stable id (`"claude"`), a slug: stored in sessions and accounts.
    fn id(&self) -> &str;
    /// Human-readable name (`"Claude Code"`).
    fn display_name(&self) -> &str;
    /// One-character icon for lists.
    fn glyph(&self) -> &str {
        "●"
    }
    /// Accent colour of the icon (`0xRRGGBB`), `None` = the UI's dim text colour.
    fn color(&self) -> Option<u32> {
        None
    }
    /// Program name searched on `PATH` (or a path).
    fn program(&self) -> &str;
    /// Arguments that print the version.
    fn version_args(&self) -> Vec<String> {
        vec!["--version".into()]
    }
    /// Oldest version this provider's mapping is known to work with.
    fn min_version(&self) -> Option<&str> {
        None
    }
    /// Resolve the program to an absolute path.
    fn program_path(&self, env: &HostEnv) -> Result<PathBuf> {
        ide_core::resolve_program(self.program(), env)
    }
    /// Installed? Path, version, min-version check. Runs `<program> --version` (blocking).
    fn detect(&self, env: &HostEnv) -> Detection {
        detect::detect(self, env)
    }
    /// What this provider supports.
    fn capabilities(&self) -> Caps;

    // ---- Accounts ----

    /// Env vars that point the CLI at an account (e.g. `CLAUDE_CONFIG_DIR`).
    fn account_env(&self, _acct: &AccountDir) -> Vec<(String, String)> {
        Vec::new()
    }
    /// Arguments of the CLI's login command (run with [`AgentProvider::account_env`] in a
    /// terminal; see [`login_spec`]). `None` = no login command.
    fn login_args(&self) -> Option<Vec<String>> {
        None
    }
    /// Logged in? Asks the CLI (blocking, local only); default [`AccountStatus::Unknown`].
    fn account_status(&self, _acct: &AccountDir, _env: &HostEnv) -> AccountStatus {
        AccountStatus::Unknown
    }

    // ---- Session assembly ----

    /// Start a plan: base arguments and the account's env vars. Default: `program` +
    /// [`AgentProvider::account_env`].
    fn begin(&self, plan: &mut LaunchPlan, acct: Option<&AccountDir>) {
        if let Some(a) = acct {
            plan.env.extend(self.account_env(a));
        }
    }
    /// Skills. Default: [`fallback::skills_as_instructions`].
    fn apply_skills(&self, plan: &mut LaunchPlan, skills: &[Skill]) -> Result<()> {
        fallback::skills_as_instructions(plan, skills);
        Ok(())
    }
    /// MCP servers. Default: unsupported (recorded in the plan's notes).
    fn apply_mcp(&self, plan: &mut LaunchPlan, servers: &[McpServer]) -> Result<()> {
        fallback::unsupported(plan, "mcp", servers.len());
        Ok(())
    }
    /// Instructions (configuration text plus fallback text). Default: unsupported.
    fn apply_instructions(&self, plan: &mut LaunchPlan, text: &str) -> Result<()> {
        fallback::unsupported(plan, "instructions", usize::from(!text.is_empty()));
        Ok(())
    }
    /// Status hooks. Default: nothing.
    fn apply_hooks(&self, _plan: &mut LaunchPlan, _hooks: &HookTargets) -> Result<()> {
        Ok(())
    }
    /// Turn the plan into a spec: resolve the program, wrap batch files. Default:
    /// [`fallback::finish`].
    fn finish(&self, plan: LaunchPlan, env: &HostEnv) -> Result<LaunchSpec> {
        fallback::finish(self, plan, env)
    }
}

/// Inputs of [`assemble`].
#[derive(Debug, Clone)]
pub struct SessionInput<'a> {
    /// Session id.
    pub session_id: &'a str,
    /// Session title (session name for CLIs that support one).
    pub title: &'a str,
    /// Process cwd (the worktree).
    pub cwd: &'a Path,
    /// Scratch dir for generated files (`sessions/<id>/agent`); emptied by [`assemble`].
    pub scratch: PathBuf,
    /// The account, if the session uses one.
    pub account: Option<&'a AccountDir>,
    /// The configuration.
    pub configuration: &'a Configuration,
}

impl<'a> SessionInput<'a> {
    /// The input for a stored session (scratch dir from `paths`).
    pub fn for_session(
        paths: &Paths,
        session: &'a Session,
        account: Option<&'a AccountDir>,
        configuration: &'a Configuration,
    ) -> Self {
        SessionInput {
            session_id: session.id.as_str(),
            title: &session.title,
            cwd: &session.worktree_path,
            scratch: paths.session_agent_dir(session.id.as_str()),
            account,
            configuration,
        }
    }
}

/// Result of [`assemble`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    /// What to spawn.
    pub spec: LaunchSpec,
    /// Configuration parts that were not passed on (log/show them).
    pub notes: Vec<String>,
}

/// Build the launch spec of a session: begin -> skills -> MCP -> instructions (configuration
/// text + fallback text) -> hooks -> finish. Regenerates the scratch dir. `TERM` and
/// `COLORTERM` are always set.
pub fn assemble(
    provider: &dyn AgentProvider,
    input: &SessionInput<'_>,
    env: &HostEnv,
) -> Result<Assembled> {
    match std::fs::remove_dir_all(&input.scratch) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ide_core::Error::io(
                ide_core::IoAction::Removing,
                &input.scratch,
                e,
            ));
        }
    }
    let mut plan = LaunchPlan::new(provider.program(), input);
    provider.begin(&mut plan, input.account);
    let cfg = input.configuration;
    let skills = skills::load(&cfg.skills, &mut plan.notes);
    if !skills.is_empty() {
        provider.apply_skills(&mut plan, &skills)?;
    }
    if !cfg.mcp_servers.is_empty() {
        provider.apply_mcp(&mut plan, &cfg.mcp_servers)?;
    }
    let mut text = cfg.instructions.trim().to_owned();
    for extra in std::mem::take(&mut plan.extra_instructions) {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(extra.trim());
    }
    if !text.is_empty() {
        provider.apply_instructions(&mut plan, &text)?;
    }
    provider.apply_hooks(&mut plan, &HookTargets::default())?;
    plan.set_env("TERM", "xterm-256color");
    plan.set_env("COLORTERM", "truecolor");
    let notes = plan.notes.clone();
    let spec = provider.finish(plan, env)?;
    Ok(Assembled { spec, notes })
}

/// The spec of an account's login command (in `cwd`), or [`ide_core::Error::ProviderNoLogin`].
pub fn login_spec(
    provider: &dyn AgentProvider,
    acct: &AccountDir,
    cwd: &Path,
    env: &HostEnv,
) -> Result<LaunchSpec> {
    let args = provider
        .login_args()
        .filter(|_| provider.capabilities().accounts)
        .ok_or_else(|| ide_core::Error::ProviderNoLogin(provider.id().to_owned()))?;
    let mut env_vars = provider.account_env(acct);
    env_vars.push(("TERM".into(), "xterm-256color".into()));
    env_vars.push(("COLORTERM".into(), "truecolor".into()));
    let program = provider.program_path(env)?;
    let (program, args) = ide_core::wrap_batch_file(program, args, env)?;
    Ok(LaunchSpec {
        program,
        args,
        cwd: cwd.to_path_buf(),
        env: env_vars,
    })
}

#[cfg(test)]
mod tests;
