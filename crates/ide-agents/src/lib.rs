//! Agent providers: every agent CLI (Claude Code, Codex, a plain shell, declarative TOML
//! providers) implements one contract, [`AgentProvider`]. The IDE only deals with
//! provider-agnostic data — accounts ([`ide_core::Account`]) and configurations
//! ([`ide_core::Configuration`]: skills, MCP servers, instructions) — and each provider
//! translates that into its own flags, env vars and generated files.
//!
//! Rules (see `docs/agent-providers.md`):
//! - Never pollute a repository: inject through flags, env vars, the per-session scratch dir
//!   `sessions/<id>/agent/` ([`LaunchPlan::scratch`]) and, for workspace sessions only, files in
//!   the session folder ([`AgentProvider::prepare_root`]; the folder is the agent's cwd and lies
//!   outside every repository).
//! - Accounts are never written (only by the CLI's own login flow), except ide's own hook
//!   entries when the user installs an account integration ([`integration`]).
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
//! # let repos = ide_core::RepoStore::open(&paths)?;
//! let guidance = ide_core::session_guidance(&session, &repos);
//! let input = SessionInput::for_session(&paths, &session, None, &configuration, &repos, guidance);
//! let built = assemble(claude, &input, &env)?;
//! // spawn built.spec in a PTY
//! # Ok(()) }
//! ```

mod claude;
mod codex;
pub mod declarative;
mod detect;
pub mod fallback;
pub mod hooks;
pub mod integration;
pub mod screen;
mod plan;
mod prompt;
mod registry;
mod shell;
mod skills;
pub mod title;

use std::path::{Path, PathBuf};

use ide_core::{Account, Configuration, LaunchSpec, McpServer, Paths, Result, Session};
pub use ide_core::SessionMode;
use serde::{Deserialize, Serialize};

pub use claude::Claude;
pub use codex::Codex;
pub use declarative::{DeclarativeProvider, DeclarativeSpec};
pub use detect::{Detection, parse_version, version_at_least};
pub use hooks::{HookCaps, HookDecision, HookEvent, HookReply, HookTargets, ToolUse};
pub use integration::{IntegrationReport, IntegrationStatus};
pub use plan::LaunchPlan;
pub use prompt::{
    PASTE_SUBMIT_DELAY_MS, PromptInput, PromptRefusal, PromptWrite, clean_prompt, prompt_writes,
};
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

/// Support of the configuration parts in one session mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeSupport {
    /// Skills.
    pub skills: Support,
    /// MCP servers.
    pub mcp: Support,
    /// Instructions.
    pub instructions: Support,
}

/// What a provider can do. `skills` / `mcp` / `instructions` are for workspace sessions;
/// [`Caps::original_repo`] for original-repo sessions (nothing may be written into the cwd).
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
    /// Accounts can get a one-time integration ([`integration`]).
    pub integration: bool,
    /// Support in original-repo sessions.
    pub original_repo: ModeSupport,
}

impl Caps {
    /// Support in a session mode.
    pub fn mode(&self, mode: SessionMode) -> ModeSupport {
        match mode {
            SessionMode::Workspace => ModeSupport {
                skills: self.skills,
                mcp: self.mcp,
                instructions: self.instructions,
            },
            SessionMode::OriginalRepo => self.original_repo,
        }
    }

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
        integration: false,
        original_repo: ModeSupport {
            skills: Support::Unsupported,
            mcp: Support::Unsupported,
            instructions: Support::Unsupported,
        },
    };
}

/// One entry of a conversation transcript ([`AgentProvider::transcript`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// `user`, `assistant` or `tool` (a tool call or its result).
    pub role: String,
    /// The text (long texts and tool results shortened).
    pub text: String,
    /// When, as the CLI wrote it (empty when unknown).
    pub at: String,
}

/// A conversation exported for a session handoff ([`AgentProvider::handoff_transcript`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffTranscript {
    pub entries: Vec<TranscriptEntry>,
    /// It starts at the CLI's last compaction (its summary first).
    pub compacted: bool,
}

impl HandoffTranscript {
    /// As Markdown for the new conversation to read.
    pub fn markdown(&self, provider: &str, conversation: &str) -> String {
        let scope = if self.compacted {
            "It was compacted: this starts at its last compaction, whose summary (the first user \
             entry) covers the earlier part."
        } else {
            "The whole conversation."
        };
        let mut out = format!(
            "# Previous conversation\n\nProvider `{provider}`, conversation `{conversation}`. \
             {scope} Tool calls and results are shortened; subagents' own work is left out.\n"
        );
        let mut in_tools = false;
        for e in &self.entries {
            if e.role == "tool" {
                if !in_tools {
                    out.push('\n');
                }
                out.push_str("> ");
                out.push_str(&e.text.trim().replace('\n', "\n> "));
                out.push('\n');
                in_tools = true;
            } else {
                let at = if e.at.is_empty() { String::new() } else { format!(" · {}", e.at) };
                out.push_str(&format!("\n## {}{at}\n\n{}\n", e.role, e.text.trim()));
                in_tools = false;
            }
        }
        out
    }
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
    /// How the CLI takes a prompt typed into its terminal (`session.send_prompt`). Default:
    /// typed lines (shells, plain programs).
    fn prompt_input(&self) -> PromptInput {
        PromptInput::Lines
    }
    /// The arguments that hand a new conversation its first prompt on the command line
    /// (appended last; `prompt` is already cleaned, see [`prompt::clean_prompt`]). The CLI
    /// then submits it itself once it is ready, after its start dialogs: nothing has to be
    /// typed in at the right moment. `None`: the prompt is typed in when the CLI is at its
    /// input (`session.send_prompt`). Default: `None`.
    fn first_prompt_args(&self, _prompt: &str) -> Option<Vec<String>> {
        None
    }

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

    /// The config home the CLI uses for this session (default: the account dir). A provider
    /// may return a per-session overlay here.
    fn account_home(&self, acct: &AccountDir, _ctx: &SessionContext) -> Result<PathBuf> {
        Ok(acct.dir.clone())
    }
    /// Called after the session's process exited or was stopped (e.g. to sync state of an
    /// overlay back). Default: nothing.
    fn after_exit(&self, _ctx: &SessionContext, _acct: Option<&AccountDir>) {}

    // ---- Conversations (resume across restarts; only used when `Caps::resume`) ----

    /// A fresh start: pick the CLI's conversation id up front if the CLI takes one (add the
    /// flag to `plan`) and return it; the server stores it so the next start can resume it.
    /// `None`: the CLI chooses (its `SessionStart` hook may still report the id, see
    /// [`HookEvent::SessionStart`]). Default: `None`.
    fn new_conversation(&self, _plan: &mut LaunchPlan) -> Option<String> {
        None
    }
    /// Continue the stored conversation `id` (from [`AgentProvider::new_conversation`] or a
    /// `SessionStart` hook): add the CLI's resume flags and return the id of the conversation
    /// the launch continues (stored on the session; a provider may resolve a placeholder it
    /// stored itself into the CLI's real id), or `None` when it cannot be resumed (e.g. the
    /// CLI kept no transcript of it); then the launch starts a new conversation. `acct` is the
    /// account with its [`AgentProvider::account_home`]. Default: `None`.
    fn resume_conversation(
        &self,
        _plan: &mut LaunchPlan,
        _id: &str,
        _acct: Option<&AccountDir>,
    ) -> Option<String> {
        None
    }

    /// A session title from the user's first prompt (`docs/session-titles.md`), made at once
    /// without a model call. `None`: not a request to name the session by (e.g. one of the
    /// CLI's own commands); the next prompt is tried. Default: [`title::from_prompt`].
    fn prompt_title(&self, prompt: &str) -> Option<String> {
        title::from_prompt(prompt, &[])
    }

    /// Whether the CLI names its conversations itself (read with
    /// [`AgentProvider::conversation_title`]); the prompt title is then provisional. Default:
    /// no.
    fn names_conversations(&self) -> bool {
        false
    }

    /// The CLI's own title of conversation `id` (the stored [`Session::conversation`]), once it
    /// has one. Default: `None`.
    fn conversation_title(&self, _acct: Option<&AccountDir>, _id: &str) -> Option<String> {
        None
    }

    /// The last `limit` entries of the stored conversation `id` (`ide_read_session`,
    /// `docs/session-family.md`). `None`: this CLI's transcripts cannot be read. Default: `None`.
    fn transcript(
        &self,
        _acct: Option<&AccountDir>,
        _id: &str,
        _limit: usize,
    ) -> Option<std::result::Result<Vec<TranscriptEntry>, String>> {
        None
    }

    /// Whether [`AgentProvider::handoff_transcript`] can read this CLI's conversations (asked
    /// before a handoff picks its method). Default: no.
    fn reads_transcripts(&self) -> bool {
        false
    }

    /// The stored conversation `id` for a session handoff (`docs/session-handoff.md`): from
    /// its last compaction on, texts whole. `None`: this CLI's transcripts cannot be read.
    /// Default: `None`.
    fn handoff_transcript(
        &self,
        _acct: Option<&AccountDir>,
        _id: &str,
    ) -> Option<std::result::Result<HandoffTranscript, String>> {
        None
    }

    // ---- Turn control (`ide_send_to_session` modes) ----

    /// Bytes that stop the agent's current turn (typed into its terminal). `None`: no way to
    /// interrupt. Default: `None`.
    fn interrupt_keys(&self) -> Option<&'static [u8]> {
        None
    }
    /// A prompt typed while the agent works reaches it in that turn (steering) instead of
    /// waiting. Default: `false`.
    fn steers(&self) -> bool {
        false
    }

    // ---- Session assembly ----

    /// Materialize project-level files into the session folder (`ctx.root`), where the CLI
    /// discovers them from its cwd, and report what this covered. Only called for workspace
    /// sessions; must never write elsewhere (use [`LaunchPlan::write_root_file`], which refuses
    /// original-repo sessions). Regenerated on every start. Default: nothing (skills and the
    /// guidance go through [`AgentProvider::apply_skills`] / the instructions).
    fn prepare_root(
        &self,
        _plan: &mut LaunchPlan,
        _ctx: &SessionContext,
        _config: &Configuration,
        _skills: &[Skill],
    ) -> Result<RootPrepared> {
        Ok(RootPrepared::default())
    }
    /// Directories outside the cwd the agent must reach (worktrees kept outside the session
    /// folder, the repositories' shared git dirs for commits). Default: nothing.
    fn apply_dirs(&self, _plan: &mut LaunchPlan, _dirs: &[PathBuf]) -> Result<()> {
        Ok(())
    }

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
    /// Hooks: register `hooks.program` with the CLI (`docs/agent-bridge.md`). Called only
    /// with a program. Default: nothing.
    fn apply_hooks(&self, _plan: &mut LaunchPlan, _hooks: &HookTargets) -> Result<()> {
        Ok(())
    }
    /// What this provider's hooks can do. Default: nothing.
    fn hook_caps(&self) -> HookCaps {
        HookCaps::NONE
    }
    /// The folders the CLI keeps its own state in (memory, plans, settings) when a session
    /// runs without an account; with one, that is the account dir. The write guard
    /// (`docs/write-guard.md`) lets agents write there. Default: none.
    fn default_state_dirs(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    /// The CLI's hook input (`event`: the name [`AgentProvider::apply_hooks`] registered,
    /// `input`: what the CLI wrote to stdin) as an event; `None` = not one the IDE uses.
    fn parse_hook(&self, _event: &str, _input: &[u8]) -> Option<HookEvent> {
        None
    }
    /// Rules that read what the CLI is doing from its terminal screen and title
    /// ([`screen`]). Default: none.
    fn screen_manifest(&self) -> Option<&'static screen::ScreenManifest> {
        None
    }
    /// The account's integration with `hooks.program` ([`integration`]). Default: not needed.
    fn integration_status(&self, _acct: &AccountDir, _hooks: &HookTargets) -> IntegrationStatus {
        IntegrationStatus::NotNeeded
    }
    /// Install (or update) the account's integration: only ide's entries are written.
    fn integration_install(
        &self,
        _acct: &AccountDir,
        _hooks: &HookTargets,
    ) -> Result<IntegrationReport> {
        Ok(IntegrationReport::default())
    }
    /// Remove ide's entries from the account.
    fn integration_uninstall(&self, _acct: &AccountDir) -> Result<()> {
        Ok(())
    }
    /// The CLI's hook output for a decision. Default: print nothing, exit 0 (no opinion).
    fn hook_reply(&self, _event: &str, _decision: &HookDecision) -> HookReply {
        HookReply::default()
    }
    /// Turn the plan into a spec: resolve the program, wrap batch files. Default:
    /// [`fallback::finish`].
    fn finish(&self, plan: LaunchPlan, env: &HostEnv) -> Result<LaunchSpec> {
        fallback::finish(self, plan, env)
    }
}

/// One repository of a session as seen by providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Repository name.
    pub name: String,
    /// Its worktree (original-repo sessions: the repository itself).
    pub path: PathBuf,
    /// The repository's shared git dir (`<repo>/.git`), if known: commits in a worktree write
    /// there.
    pub git_dir: Option<PathBuf>,
}

/// Where and how a session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    /// Workspace or original-repo session.
    pub mode: SessionMode,
    /// The agent's cwd (session folder or original repository).
    pub root: PathBuf,
    /// Attached repositories.
    pub repos: Vec<RepoInfo>,
    /// Scratch dir for generated files (`sessions/<id>/agent`); emptied by [`assemble`].
    pub scratch: PathBuf,
    /// More directories outside `root` the agent reaches (the session's handoff folder,
    /// `docs/session-handoff.md`).
    pub extra_dirs: Vec<PathBuf>,
    /// Session guidance (workspace sessions; Desk's `HERMES.md` text).
    pub guidance: Option<String>,
    /// The cwd belongs to another session (a shared child works in its parent's folder): no
    /// files are written into it; the guidance goes into the instructions.
    pub shared_root: bool,
}

impl SessionContext {
    /// Directories outside `root` the agent needs: worktrees not below it and every repo's git
    /// dir (workspace sessions; an original-repo session's git dir is inside its root).
    pub fn outside_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut push = |p: &Path| {
            if !p.starts_with(&self.root) && !out.iter().any(|o| ide_core::same_path(o, p)) {
                out.push(p.to_path_buf());
            }
        };
        for r in &self.repos {
            push(&r.path);
            if let Some(g) = &r.git_dir {
                push(g);
            }
        }
        for d in &self.extra_dirs {
            push(d);
        }
        out
    }
}

/// What [`AgentProvider::prepare_root`] covered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RootPrepared {
    /// Skills were written where the CLI discovers them.
    pub skills: bool,
    /// The guidance was written where the CLI reads project context.
    pub guidance: bool,
}

/// The agent bridge of a session (`docs/agent-bridge.md`): the MCP server that connects the
/// agent to the IDE, env vars for the agent process, and the base instructions that tell the
/// agent about it. Added by the server to every session; not part of any configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    /// The bridge's MCP server (its name is reserved: a configuration server of the same name
    /// is left out).
    pub server: McpServer,
    /// Env vars of the agent process (the bridge's session id, token and server socket).
    pub env: Vec<(String, String)>,
    /// Base instructions appended to the configuration's.
    pub instructions: String,
    /// Where the CLI's hooks report to.
    pub hooks: HookTargets,
}

/// Inputs of [`assemble`].
#[derive(Debug, Clone)]
pub struct SessionInput<'a> {
    /// Session id.
    pub session_id: &'a str,
    /// Session title (session name for CLIs that support one).
    pub title: &'a str,
    /// The title may still be replaced automatically ([`ide_core::AutoTitle`] not `Off`): a
    /// CLI that names its conversations itself is not given it as a fixed name.
    pub auto_title: bool,
    /// Where the session runs (cwd, repos, scratch dir).
    pub ctx: SessionContext,
    /// The account, if the session uses one.
    pub account: Option<&'a AccountDir>,
    /// The configuration.
    pub configuration: &'a Configuration,
    /// The stored conversation to resume ([`Session::conversation`]), if any.
    pub conversation: Option<&'a str>,
    /// The agent bridge (`None`: not connected, e.g. a server without a daemon socket).
    pub bridge: Option<Bridge>,
    /// The first prompt of a new session (a child's task): passed on the command line when
    /// the provider takes one ([`AgentProvider::first_prompt_args`], see
    /// [`Assembled::prompt_in_args`]).
    pub first_prompt: Option<&'a str>,
}

impl<'a> SessionInput<'a> {
    /// The input for a stored session (scratch dir from `paths`; `repos` resolves its
    /// attached repositories to `(name, repository path)`; `guidance` from
    /// [`ide_core::session_guidance`]).
    pub fn for_session(
        paths: &Paths,
        session: &'a Session,
        account: Option<&'a AccountDir>,
        configuration: &'a Configuration,
        repos: &ide_core::RepoStore,
        guidance: Option<String>,
    ) -> Self {
        let info = |repo: &ide_core::RepoId, name: &str, path: PathBuf| {
            let r = repos.get(repo);
            RepoInfo {
                name: r.map_or_else(|| name.to_owned(), |r| r.name.clone()),
                path,
                git_dir: r.map(|r| r.path.join(".git")).filter(|g| g.is_dir()),
            }
        };
        // Worktrees still being created count already (at their planned folder): an agent
        // started meanwhile needs their git dirs to commit there later.
        let repos = session
            .worktrees
            .iter()
            .map(|w| info(&w.repo, &w.name, w.path.clone()))
            .chain(
                session
                    .pending
                    .iter()
                    .map(|p| info(&p.repo, &p.name, session.root.join(&p.name))),
            )
            .collect();
        SessionInput {
            session_id: session.id.as_str(),
            title: &session.title,
            auto_title: !session.auto_title.is_off(),
            ctx: SessionContext {
                mode: session.mode,
                root: session.root.clone(),
                repos,
                scratch: paths.session_agent_dir(session.id.as_str()),
                extra_dirs: vec![ide_core::handoff::dir(&paths.session_dir(session.id.as_str()))],
                guidance,
                shared_root: session.shared,
            },
            account,
            configuration,
            conversation: session.conversation.as_deref(),
            bridge: None,
            first_prompt: None,
        }
    }

    /// The same input with the agent bridge.
    pub fn with_bridge(mut self, bridge: Option<Bridge>) -> Self {
        self.bridge = bridge;
        self
    }

    /// The same input with a first prompt.
    pub fn with_first_prompt(mut self, prompt: Option<&'a str>) -> Self {
        self.first_prompt = prompt;
        self
    }
}

/// Result of [`assemble`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    /// What to spawn.
    pub spec: LaunchSpec,
    /// Configuration parts that were not passed on (log/show them).
    pub notes: Vec<String>,
    /// The conversation this launch starts or resumes (store it on the session), if known.
    pub conversation: Option<String>,
    /// [`SessionInput::first_prompt`] is on the command line: do not type it in as well.
    /// `false`: the provider takes none, the launch resumes a conversation, the prompt is
    /// longer than [`MAX_ARG_PROMPT`] or the CLI runs through `cmd.exe` (a batch file, whose
    /// command line cannot carry newlines and `& | < >` safely).
    pub prompt_in_args: bool,
}

/// The longest first prompt passed on the command line (characters); a longer one is typed
/// in (Windows limits a command line to 32767 characters, flags and paths included).
pub const MAX_ARG_PROMPT: usize = 12_000;

/// Build the launch spec of a session: account home -> begin -> conversation (new or resumed,
/// providers with `Caps::resume`) -> (workspace sessions)
/// prepare_root -> skills (unless the root covered them) -> MCP (configuration servers + the
/// bridge) -> instructions (configuration text + bridge text + guidance unless the root
/// covered it + fallback text) -> outside dirs -> hooks -> finish. Regenerates the scratch dir. `TERM` and `COLORTERM` are always set.
pub fn assemble(
    provider: &dyn AgentProvider,
    input: &SessionInput<'_>,
    env: &HostEnv,
) -> Result<Assembled> {
    let ctx = &input.ctx;
    match std::fs::remove_dir_all(&ctx.scratch) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ide_core::Error::io(
                ide_core::IoAction::Removing,
                &ctx.scratch,
                e,
            ));
        }
    }
    let mut plan = LaunchPlan::new(provider.program(), input);
    let account = match input.account {
        Some(a) => {
            let mut a = a.clone();
            a.dir = provider.account_home(&a, ctx)?;
            Some(a)
        }
        None => None,
    };
    provider.begin(&mut plan, account.as_ref());
    let mut resumed = false;
    if provider.capabilities().resume {
        let continued = input
            .conversation
            .and_then(|id| provider.resume_conversation(&mut plan, id, account.as_ref()));
        resumed = continued.is_some();
        plan.conversation = match continued {
            Some(id) => Some(id),
            None => provider.new_conversation(&mut plan),
        };
    }
    let cfg = input.configuration;
    let skills = skills::load(&cfg.skills, &mut plan.notes);
    // Files go into the session folder only when it is known: a legacy session migrated
    // without one has an empty root, which would mean the server's working directory.
    let prepared = if ctx.mode == SessionMode::Workspace && ctx.root.is_absolute() && !ctx.shared_root
    {
        provider.prepare_root(&mut plan, ctx, cfg, &skills)?
    } else {
        RootPrepared::default()
    };
    if !skills.is_empty() && !prepared.skills {
        provider.apply_skills(&mut plan, &skills)?;
    }
    let mut servers = cfg.mcp_servers.clone();
    let mut bridge_text = None;
    if let Some(b) = &input.bridge {
        plan.env.extend(b.env.iter().cloned());
        // Plain programs (shells) take no MCP servers: they only get the env vars.
        if provider.capabilities().mode(ctx.mode).mcp != Support::Unsupported {
            if servers.iter().any(|s| s.name == b.server.name) {
                plan.notes.push(format!(
                    "mcp: the configuration's server \"{}\" is left out (name reserved by the ide bridge)",
                    b.server.name
                ));
                servers.retain(|s| s.name != b.server.name);
            }
            servers.push(b.server.clone());
            bridge_text = Some(b.instructions.clone());
        }
    }
    if !servers.is_empty() {
        provider.apply_mcp(&mut plan, &servers)?;
    }
    let mut text = cfg.instructions.trim().to_owned();
    if let Some(b) = bridge_text.filter(|t| !t.trim().is_empty()) {
        if !text.is_empty() {
            text.push_str("

");
        }
        text.push_str(b.trim());
    }
    let guidance = ctx.guidance.as_deref().filter(|_| !prepared.guidance);
    for extra in guidance
        .map(str::to_owned)
        .into_iter()
        .chain(std::mem::take(&mut plan.extra_instructions))
    {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(extra.trim());
    }
    if !text.is_empty() {
        provider.apply_instructions(&mut plan, &text)?;
    }
    let dirs = ctx.outside_dirs();
    if !dirs.is_empty() {
        provider.apply_dirs(&mut plan, &dirs)?;
    }
    if let Some(h) = input.bridge.as_ref().map(|b| &b.hooks).filter(|h| h.program.is_some()) {
        provider.apply_hooks(&mut plan, h)?;
    }
    plan.set_env("TERM", "xterm-256color");
    plan.set_env("COLORTERM", "truecolor");
    let notes = plan.notes.clone();
    let conversation = plan.conversation.clone();
    let mut spec = provider.finish(plan, env)?;
    // Last, after `finish` resolved the program: through `cmd.exe` the prompt stays out.
    let prompt_args = input
        .first_prompt
        .filter(|_| !resumed && !runs_through_cmd(&spec))
        .map(prompt::clean_prompt)
        .filter(|p| !p.is_empty() && p.chars().count() <= MAX_ARG_PROMPT)
        .and_then(|p| provider.first_prompt_args(&p));
    let prompt_in_args = prompt_args.is_some();
    spec.args.extend(prompt_args.into_iter().flatten());
    Ok(Assembled {
        spec,
        notes,
        conversation,
        prompt_in_args,
    })
}

/// The spec runs a batch file through `cmd.exe` ([`ide_core::wrap_batch_file`]).
fn runs_through_cmd(spec: &LaunchSpec) -> bool {
    cfg!(windows) && spec.args.len() >= 3 && spec.args[..3] == ["/d", "/s", "/c"]
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
        env_remove: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
