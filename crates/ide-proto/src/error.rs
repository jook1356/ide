//! Protocol errors: a stable message id plus named arguments. No localized text crosses the
//! wire; clients translate the id (e.g. `ide-i18n` `err-<id>`).

use std::fmt;

use serde::{Deserialize, Serialize};

/// An error sent to clients.
///
/// Ids reused from `ide_core::ErrorMessage::id()` keep their argument names (`path`, `name`,
/// `id`, `detail`, `action`, `reason`, `args`, `status`, `program`). Protocol/server ids:
/// `bad-message`, `proto-version` (`server`, `client`), `not-handshaken`, `unauthorized`,
/// `session-running` (`id`), `session-not-running` (`id`), `spawn-failed` (`program`,
/// `detail`), `internal` (`detail`), `message-too-large`. Client-side ids: `disconnected`,
/// `timeout`, `connect-failed` (`detail`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtoError {
    /// Stable kebab-case id.
    pub id: String,
    /// Named arguments (untranslatable details such as git's stderr go in `detail`).
    pub args: Vec<(String, String)>,
}

impl ProtoError {
    /// An error without arguments.
    pub fn new(id: impl Into<String>) -> Self {
        ProtoError {
            id: id.into(),
            args: Vec::new(),
        }
    }

    /// Add an argument.
    pub fn arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.args.push((name.into(), value.into()));
        self
    }

    /// Look up an argument.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `internal` with a detail.
    pub fn internal(detail: impl fmt::Display) -> Self {
        ProtoError::new("internal").arg("detail", detail.to_string())
    }
}

/// English, for logs only.
impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)?;
        for (i, (k, v)) in self.args.iter().enumerate() {
            write!(f, "{}{k}={v:?}", if i == 0 { ": " } else { ", " })?;
        }
        Ok(())
    }
}

impl std::error::Error for ProtoError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_args() {
        let e = ProtoError::new("git")
            .arg("args", "worktree add")
            .arg("detail", "boom");
        assert_eq!(e.get("detail"), Some("boom"));
        assert_eq!(e.to_string(), "git: args=\"worktree add\", detail=\"boom\"");
        assert_eq!(ProtoError::new("x").to_string(), "x");
    }
}
