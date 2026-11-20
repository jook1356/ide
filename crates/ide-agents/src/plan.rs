//! [`LaunchPlan`]: the launch being assembled.

use std::path::PathBuf;

use ide_core::{Error, IoAction, Result};

use crate::SessionInput;

/// A launch being assembled by a provider: program, arguments, env vars, and a per-session
/// scratch dir for generated files (never the worktree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// Session id.
    pub session_id: String,
    /// Session title.
    pub title: String,
    /// Configuration slug (e.g. for generated plugin names).
    pub configuration: String,
    /// Process cwd (the worktree).
    pub cwd: PathBuf,
    /// `sessions/<id>/agent`: generated files go here ([`LaunchPlan::write_file`]).
    pub scratch: PathBuf,
    /// Program name (resolved on `PATH` by [`crate::AgentProvider::finish`]).
    pub program: String,
    /// Arguments (without the program).
    pub args: Vec<String>,
    /// Env vars set on top of the inherited environment.
    pub env: Vec<(String, String)>,
    /// Text to append to the instructions (fallbacks such as skill lists).
    pub extra_instructions: Vec<String>,
    /// Configuration parts that could not be passed on.
    pub notes: Vec<String>,
}

impl LaunchPlan {
    /// An empty plan for `program`.
    pub fn new(program: &str, input: &SessionInput<'_>) -> Self {
        LaunchPlan {
            session_id: input.session_id.to_owned(),
            title: input.title.to_owned(),
            configuration: input.configuration.slug.clone(),
            cwd: input.cwd.to_path_buf(),
            scratch: input.scratch.clone(),
            program: program.to_owned(),
            args: Vec::new(),
            env: Vec::new(),
            extra_instructions: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Write `rel` (a `/`-separated path) inside the scratch dir, creating parents; returns
    /// the absolute path.
    pub fn write_file(&self, rel: &str, contents: &[u8]) -> Result<PathBuf> {
        let path = rel
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .fold(self.scratch.clone(), |p, c| p.join(c));
        if let Some(parent) = path.parent() {
            ide_core::ensure_dir(parent)?;
        }
        std::fs::write(&path, contents).map_err(|e| Error::io(IoAction::Writing, &path, e))?;
        Ok(path)
    }

    /// Set (or replace) an env var.
    pub fn set_env(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.env.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.env.push((key.to_owned(), value)),
        }
    }

    /// Append arguments.
    pub fn arg(&mut self, a: impl Into<String>) -> &mut Self {
        self.args.push(a.into());
        self
    }
}
