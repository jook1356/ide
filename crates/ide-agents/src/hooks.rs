//! Agent hooks, provider-agnostic (`docs/agent-bridge.md`, "Hooks"): what a CLI reports
//! about its turn (started, finished, waiting for the user, about to use a tool) and what the
//! IDE answers (nothing, or deny a tool call with a reason).
//!
//! A provider maps these to its CLI in three places, all in its own file:
//! [`AgentProvider::apply_hooks`](crate::AgentProvider::apply_hooks) registers the hook
//! program with the CLI, [`AgentProvider::parse_hook`](crate::AgentProvider::parse_hook) turns
//! the CLI's hook input into a [`HookEvent`], and
//! [`AgentProvider::hook_reply`](crate::AgentProvider::hook_reply) turns a [`HookDecision`]
//! into the CLI's hook output. The hook program is `ide-server hook --provider <id> <event>`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Support;

/// What the CLI reported, normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEvent {
    /// The CLI started (or resumed / cleared) its conversation.
    SessionStart,
    /// The user's prompt was submitted: a turn started.
    PromptSubmitted,
    /// The turn finished.
    Stopped,
    /// The CLI waits for the user: a permission prompt (`permission`) or idle input.
    NeedsInput { permission: bool },
    /// The agent is about to use a tool.
    PreTool(ToolUse),
}

/// A tool call, as far as the IDE cares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolUse {
    /// A file edit (write, edit, patch) of these paths (absolute when the CLI says so).
    Edit { tool: String, paths: Vec<String> },
    /// A shell command.
    Shell { tool: String, command: String },
    /// Anything else.
    Other { tool: String },
}

/// The IDE's answer to a hook.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookDecision {
    /// No opinion: the CLI's own flow continues (never an "allow": that would skip the CLI's
    /// permission prompts).
    #[default]
    NoOpinion,
    /// Refuse a tool call; the reason is shown to the agent.
    Deny { reason: String },
}

/// What the hook program prints and exits with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookReply {
    pub stdout: String,
    pub exit_code: i32,
}

/// What a provider's hooks can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCaps {
    /// Turn status (working / idle / needs input).
    pub status: Support,
    /// Refusing file edits.
    pub guard_edits: Support,
    /// Refusing shell commands (a shell can still write files: never complete).
    pub guard_shell: Support,
}

impl HookCaps {
    /// No hooks.
    pub const NONE: HookCaps = HookCaps {
        status: Support::Unsupported,
        guard_edits: Support::Unsupported,
        guard_shell: Support::Unsupported,
    };
}

/// Where hooks report to: the hook program (`ide-server`, run as
/// `<program> hook --provider <id> <event>`). `None` = no hooks (no bridge).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookTargets {
    pub program: Option<PathBuf>,
}

impl HookTargets {
    /// The hook program's arguments for one event of `provider`.
    pub fn args(provider: &str, event: &str) -> Vec<String> {
        vec![
            "hook".to_owned(),
            "--provider".to_owned(),
            provider.to_owned(),
            event.to_owned(),
        ]
    }
}
