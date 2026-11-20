//! A plain shell (`COMSPEC` / `cmd.exe` on Windows, `$SHELL` / `/bin/sh` elsewhere). No
//! accounts; configurations are ignored by design (nothing to pass them to).

use std::path::PathBuf;

use ide_core::{McpServer, Result};

use crate::{AgentProvider, Caps, HostEnv, LaunchPlan, Skill};

/// The user's shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct Shell;

impl AgentProvider for Shell {
    fn id(&self) -> &str {
        "shell"
    }
    fn display_name(&self) -> &str {
        "Shell"
    }
    fn glyph(&self) -> &str {
        "❯"
    }
    fn program(&self) -> &str {
        if cfg!(windows) { "cmd.exe" } else { "sh" }
    }
    fn version_args(&self) -> Vec<String> {
        Vec::new()
    }
    fn program_path(&self, env: &HostEnv) -> Result<PathBuf> {
        ide_core::shell_program(env)
    }
    fn capabilities(&self) -> Caps {
        Caps::NONE
    }
    fn apply_skills(&self, _plan: &mut LaunchPlan, _skills: &[Skill]) -> Result<()> {
        Ok(())
    }
    fn apply_mcp(&self, _plan: &mut LaunchPlan, _servers: &[McpServer]) -> Result<()> {
        Ok(())
    }
    fn apply_instructions(&self, _plan: &mut LaunchPlan, _text: &str) -> Result<()> {
        Ok(())
    }
}
