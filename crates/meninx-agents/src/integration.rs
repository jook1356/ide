//! One-time account integration (`docs/agent-bridge.md`, "Account integration"): a CLI whose
//! hooks can only be configured in its own config home (Codex: `CODEX_HOME/hooks.json`) gets
//! Meninx's hook entries written there, but only when the user asks for it on the accounts page.
//! Only Meninx's entries are added, replaced or removed; everything else in the file is kept.
//! Providers that inject hooks per session (Claude Code: `--settings`) need none.

use serde::{Deserialize, Serialize};

/// An account's integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrationStatus {
    /// The provider needs no integration.
    NotNeeded,
    /// Not installed.
    Missing,
    /// Installed and current.
    Installed,
    /// Installed for another hook program (e.g. Meninx moved): install again.
    Outdated,
}

/// What an install did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrationReport {
    /// The CLI must approve the new hooks itself; this is how (a command typed in the CLI,
    /// shown as is: not translated).
    pub approve_with: Option<String>,
}
