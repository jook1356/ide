//! Audit hook: `audit(user, action, target)` for session start/stop, process spawns and (later)
//! file writes. Phase 1 writes JSON lines to `<data>/audit.log`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use meninx_proto::UserInfo;

/// Receives audit records.
pub trait Audit: Send + Sync {
    /// Record that `user` did `action` on `target`.
    fn record(&self, user: &UserInfo, action: &str, target: &str);
}

/// Discards records.
#[derive(Debug, Default)]
pub struct NoAudit;

impl Audit for NoAudit {
    fn record(&self, _: &UserInfo, _: &str, _: &str) {}
}

/// Appends JSON lines `{"ts":..,"user":..,"action":..,"target":..}` to a file.
#[derive(Debug)]
pub struct FileAudit {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileAudit {
    /// Log to `path` (created on first record).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileAudit {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    /// The log file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Audit for FileAudit {
    fn record(&self, user: &UserInfo, action: &str, target: &str) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let line = serde_json::json!({
            "ts": ts, "user": user.id, "action": action, "target": target,
        });
        let _g = self.lock.lock().unwrap();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = res {
            tracing::warn!("audit log {}: {e}", self.path.display());
        }
    }
}
