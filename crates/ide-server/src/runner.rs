//! Process spawning. Every process the server starts goes through a [`Runner`] so that
//! containers/limits/other-user execution can be plugged in later.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use ide_proto::{ProtoError, UserInfo};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::audit::Audit;

/// What to run in a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    /// Program (resolved path).
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// Initial size.
    pub cols: u16,
    /// Initial size.
    pub rows: u16,
}

impl SpawnSpec {
    /// From an `ide-core` launch spec.
    pub fn from_launch(spec: &ide_core::LaunchSpec, cols: u16, rows: u16) -> Self {
        SpawnSpec {
            program: spec.program.clone(),
            args: spec.args.clone(),
            cwd: spec.cwd.clone(),
            env: spec.env.clone(),
            cols,
            rows,
        }
    }
}

/// A process running in a PTY.
pub struct PtyChild {
    /// PTY output.
    pub reader: Box<dyn Read + Send>,
    /// PTY input.
    pub writer: Box<dyn Write + Send>,
    /// The PTY (resize; dropping it closes the pseudoconsole).
    pub master: Box<dyn MasterPty + Send>,
    /// The process.
    pub child: Box<dyn Child + Send + Sync>,
}

/// Spawns processes for a user.
pub trait Runner: Send + Sync {
    /// Start `spec` in a new PTY as `user`.
    fn spawn_pty(&self, user: &UserInfo, spec: &SpawnSpec) -> Result<PtyChild, ProtoError>;
}

/// Runs everything as the server's own OS user (the only mode in phase 1). Refuses other
/// users and audits every spawn.
pub struct SameUserRunner {
    user: UserInfo,
    audit: Arc<dyn Audit>,
}

impl SameUserRunner {
    /// A runner for `user` (the server process' user).
    pub fn new(user: UserInfo, audit: Arc<dyn Audit>) -> Self {
        SameUserRunner { user, audit }
    }
}

impl Runner for SameUserRunner {
    fn spawn_pty(&self, user: &UserInfo, spec: &SpawnSpec) -> Result<PtyChild, ProtoError> {
        if user.id != self.user.id {
            return Err(ProtoError::new("unauthorized"));
        }
        let fail = |e: &dyn std::fmt::Display| {
            ProtoError::new("spawn-failed")
                .arg("program", spec.program.display().to_string())
                .arg("detail", e.to_string())
        };
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows.max(1),
                cols: spec.cols.max(2),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| fail(&e))?;
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        self.audit.record(
            user,
            "process.spawn",
            &format!("{} (cwd {})", spec.program.display(), spec.cwd.display()),
        );
        let child = pair.slave.spawn_command(cmd).map_err(|e| fail(&e))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| fail(&e))?;
        let writer = pair.master.take_writer().map_err(|e| fail(&e))?;
        Ok(PtyChild {
            reader,
            writer,
            master: pair.master,
            child,
        })
    }
}
