//! [`LaunchPlan`]: the launch being assembled.

use std::path::PathBuf;

use meninx_core::{Error, IoAction, Result};

use crate::SessionInput;

/// A launch being assembled by a provider: program, arguments, env vars, and a per-session
/// scratch dir for generated files (never the worktree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// Session id.
    pub session_id: String,
    /// Session title.
    pub title: String,
    /// The title may still be replaced automatically ([`crate::SessionInput::auto_title`]).
    pub auto_title: bool,
    /// Configuration slug (e.g. for generated plugin names).
    pub configuration: String,
    /// Process cwd (session folder or original repository).
    pub cwd: PathBuf,
    /// Workspace or original-repo session (root files are refused for the latter).
    pub mode: crate::SessionMode,
    /// `sessions/<id>/agent`: generated files go here ([`LaunchPlan::write_file`]).
    pub scratch: PathBuf,
    /// Program name (resolved on `PATH` by [`crate::AgentProvider::finish`]).
    pub program: String,
    /// Arguments (without the program).
    pub args: Vec<String>,
    /// Env vars set on top of the inherited environment.
    pub env: Vec<(String, String)>,
    /// Inherited env vars removed before `env` is applied ([`LaunchPlan::remove_env`]).
    pub env_remove: Vec<String>,
    /// The CLI's conversation id this launch starts or resumes, if the provider set one
    /// ([`crate::AgentProvider::new_conversation`] / [`crate::AgentProvider::resume_conversation`]).
    pub conversation: Option<String>,
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
            auto_title: input.auto_title,
            configuration: input.configuration.slug.clone(),
            cwd: input.ctx.root.clone(),
            mode: input.ctx.mode,
            scratch: input.ctx.scratch.clone(),
            program: program.to_owned(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            conversation: None,
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
            meninx_core::ensure_dir(parent)?;
        }
        std::fs::write(&path, contents).map_err(|e| Error::io(IoAction::Writing, &path, e))?;
        Ok(path)
    }

    /// Write `rel` inside the session folder (the cwd) of a workspace session; refused for
    /// original-repo sessions (never write into a repository). Returns the absolute path.
    pub fn write_root_file(&self, rel: &str, contents: &[u8]) -> Result<PathBuf> {
        let path = self.root_path(rel)?;
        if let Some(parent) = path.parent() {
            meninx_core::ensure_dir(parent)?;
        }
        std::fs::write(&path, contents).map_err(|e| Error::io(IoAction::Writing, &path, e))?;
        Ok(path)
    }

    /// Create the folder `rel` inside the session folder of a workspace session.
    pub fn root_dir(&self, rel: &str) -> Result<PathBuf> {
        let path = self.root_path(rel)?;
        meninx_core::ensure_dir(&path)?;
        Ok(path)
    }

    /// Remove `rel` (file or folder) inside the session folder, if present (regeneration).
    pub fn clear_root_path(&self, rel: &str) -> Result<()> {
        let path = self.root_path(rel)?;
        let r = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match r {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(IoAction::Removing, &path, e)),
        }
    }

    fn root_path(&self, rel: &str) -> Result<PathBuf> {
        if self.mode != crate::SessionMode::Workspace {
            return Err(Error::io(
                IoAction::Writing,
                self.cwd.join(rel),
                std::io::Error::other("original-repo sessions get no generated files"),
            ));
        }
        Ok(rel
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .fold(self.cwd.clone(), |p, c| p.join(c)))
    }

    /// Set (or replace) an env var.
    pub fn set_env(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.env.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.env.push((key.to_owned(), value)),
        }
    }

    /// Do not pass an inherited env var on (no-op if it is not set).
    pub fn remove_env(&mut self, key: &str) {
        if !self.env_remove.iter().any(|k| k == key) {
            self.env_remove.push(key.to_owned());
        }
    }

    /// Append arguments.
    pub fn arg(&mut self, a: impl Into<String>) -> &mut Self {
        self.args.push(a.into());
        self
    }
}
