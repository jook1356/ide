//! Agent providers, accounts and configurations on the wire (see `docs/agent-providers.md`).
//! Account dirs and other server paths of accounts stay on the server.

use serde::{Deserialize, Serialize};

/// How well a provider supports a configuration feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Support {
    /// Passed to the CLI natively.
    Native,
    /// Emulated (e.g. skills listed in the instructions).
    Fallback,
    /// Not passed on.
    Unsupported,
}

/// What a provider can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCaps {
    /// Sessions can use accounts of this provider.
    pub accounts: bool,
    /// Accounts can log in from the IDE (`account.login`).
    pub login: bool,
    /// Configuration skills.
    pub skills: Support,
    /// Configuration MCP servers.
    pub mcp: Support,
    /// Configuration instructions.
    pub instructions: Support,
    /// Status hooks.
    pub hooks: Support,
    /// Resumable sessions.
    pub resume: bool,
    /// The session title is passed as the CLI's session name.
    pub session_name: bool,
}

/// Installed? Version?
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProviderDetection {
    /// Program path on the server (`None` = not found).
    pub path: Option<String>,
    /// Reported version.
    pub version: Option<String>,
    /// Oldest supported version.
    pub min_version: Option<String>,
    /// Installed and new enough.
    pub usable: bool,
    /// Why not usable (English).
    pub error: Option<String>,
}

/// One agent provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Stable id (`"claude"`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// One-character icon.
    pub glyph: String,
    /// Icon colour `0xRRGGBB` (`None` = dim text colour).
    pub color: Option<u32>,
    /// Detection result.
    pub detection: ProviderDetection,
    /// Capabilities.
    pub caps: ProviderCaps,
}

/// One account (login) of a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Provider id.
    pub provider: String,
    /// Unique per provider.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Unix seconds.
    pub created_at: u64,
}

/// Logged-in state reported by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountState {
    /// Logged in.
    LoggedIn,
    /// Not logged in.
    LoggedOut,
    /// Unknown (no status command, CLI missing, unexpected output).
    Unknown,
}

/// A local (stdio) MCP server of a configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct McpServerInfo {
    /// `[A-Za-z0-9_-]{1,64}`, unique per configuration.
    pub name: String,
    /// Program.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra env vars.
    pub env: Vec<(String, String)>,
}

/// A configuration: skills, MCP servers, instructions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigurationInfo {
    /// Unique slug (identifies it in `configuration.update`).
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Skill folders (server paths).
    pub skills: Vec<String>,
    /// MCP servers.
    pub mcp_servers: Vec<McpServerInfo>,
    /// Instructions text.
    pub instructions: String,
    /// Unix seconds.
    pub created_at: u64,
}

/// What a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SessionKind {
    /// An agent (or shell) in a git worktree.
    #[default]
    Agent,
    /// An account's login command (no repository).
    Login,
}
