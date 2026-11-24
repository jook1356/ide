//! Approvals (`docs/approvals.md`): what the user allowed agents to do beyond their own
//! session. A grant is keyed by `(capability, requester session, target)`; it lasts while the
//! daemon runs ("이 세션 동안") or always (kept in `<data>/approvals.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, now_unix, read_json, write_json};

/// A kept grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Stable id (for revoking).
    pub id: String,
    /// What it allows: a dotted name (`session.steer`, `session.interrupt`, ...).
    pub capability: String,
    /// The session whose agent asked.
    pub requester: String,
    /// What it acts on (a session id, a terminal id, a site origin, ...).
    pub target: String,
    /// Kept across daemon restarts.
    pub always: bool,
    /// Unix seconds.
    pub created: u64,
}

/// How the user answered a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Allowed this time only (nothing kept).
    Once,
    /// Allowed until the daemon restarts or either session goes.
    Session,
    /// Allowed always (kept on disk).
    Always,
}

impl Answer {
    /// The app's answer text (`once` / `session` / `always`).
    pub fn parse(s: &str) -> Option<Answer> {
        match s.trim() {
            "once" => Some(Answer::Once),
            "session" => Some(Answer::Session),
            "always" => Some(Answer::Always),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Answer::Once => "once",
            Answer::Session => "session",
            Answer::Always => "always",
        }
    }
}

/// `<data>/approvals.json`.
pub fn file(paths: &Paths) -> PathBuf {
    paths.root().join("approvals.json")
}

/// The grants of one user: the kept ones (`always`) and those of this daemon run.
#[derive(Debug, Default)]
pub struct Grants {
    list: Vec<Grant>,
    next: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    grants: Vec<Grant>,
}

impl Grants {
    /// The kept grants of `<data>/approvals.json` (none when missing or unreadable).
    pub fn load(paths: &Paths) -> Grants {
        let stored: Stored = read_json(&file(paths)).ok().flatten().unwrap_or_default();
        Grants {
            list: stored.grants.into_iter().filter(|g| g.always).collect(),
            next: 0,
        }
    }

    fn save(&self, paths: &Paths) -> crate::Result<()> {
        let stored = Stored {
            grants: self.list.iter().filter(|g| g.always).cloned().collect(),
        };
        write_json(&file(paths), &stored)
    }

    /// Every grant, oldest first.
    pub fn list(&self) -> &[Grant] {
        &self.list
    }

    /// Is `(capability, requester, target)` allowed?
    pub fn allows(&self, capability: &str, requester: &str, target: &str) -> bool {
        self.list
            .iter()
            .any(|g| g.capability == capability && g.requester == requester && g.target == target)
    }

    /// Keep an answer (`Once` keeps nothing). An existing grant of the same key is replaced.
    pub fn record(
        &mut self,
        paths: &Paths,
        capability: &str,
        requester: &str,
        target: &str,
        answer: Answer,
    ) -> crate::Result<()> {
        if answer == Answer::Once {
            return Ok(());
        }
        self.list
            .retain(|g| !(g.capability == capability && g.requester == requester && g.target == target));
        self.next += 1;
        let created = now_unix();
        self.list.push(Grant {
            id: format!("{created:x}-{}", self.next),
            capability: capability.to_owned(),
            requester: requester.to_owned(),
            target: target.to_owned(),
            always: answer == Answer::Always,
            created,
        });
        if answer == Answer::Always {
            self.save(paths)?;
        }
        Ok(())
    }

    /// Remove a grant (`false`: no such id).
    pub fn revoke(&mut self, paths: &Paths, id: &str) -> crate::Result<bool> {
        let Some(i) = self.list.iter().position(|g| g.id == id) else {
            return Ok(false);
        };
        let g = self.list.remove(i);
        if g.always {
            self.save(paths)?;
        }
        Ok(true)
    }

    /// A session went: the grants it asked for or that act on it go too.
    pub fn forget_session(&mut self, paths: &Paths, session: &str) -> crate::Result<()> {
        let kept_before = self.list.iter().filter(|g| g.always).count();
        self.list.retain(|g| g.requester != session && g.target != session);
        if self.list.iter().filter(|g| g.always).count() != kept_before {
            self.save(paths)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants() {
        let t = crate::paths::TempDir::new("approvals");
        let paths = Paths::new(t.0.clone());
        let mut g = Grants::load(&paths);
        assert!(!g.allows("session.steer", "a", "b"));
        assert_eq!(Answer::parse("always"), Some(Answer::Always));
        assert_eq!(Answer::parse("yes"), None);

        g.record(&paths, "session.steer", "a", "b", Answer::Once).unwrap();
        assert!(!g.allows("session.steer", "a", "b"), "once keeps nothing");
        g.record(&paths, "session.steer", "a", "b", Answer::Session).unwrap();
        assert!(g.allows("session.steer", "a", "b"));
        assert!(!g.allows("session.steer", "b", "a"), "per requester");
        assert!(!g.allows("session.interrupt", "a", "b"), "per capability");
        g.record(&paths, "session.interrupt", "a", "c", Answer::Always).unwrap();
        assert_eq!(g.list().len(), 2);

        // Only `always` survives a restart.
        let mut g = Grants::load(&paths);
        assert!(!g.allows("session.steer", "a", "b"));
        assert!(g.allows("session.interrupt", "a", "c"));
        // Replacing keeps one per key.
        g.record(&paths, "session.interrupt", "a", "c", Answer::Always).unwrap();
        assert_eq!(g.list().len(), 1);

        let id = g.list()[0].id.clone();
        assert!(g.revoke(&paths, &id).unwrap());
        assert!(!g.revoke(&paths, &id).unwrap());
        assert!(Grants::load(&paths).list().is_empty());

        g.record(&paths, "session.steer", "a", "b", Answer::Always).unwrap();
        g.record(&paths, "session.steer", "x", "y", Answer::Session).unwrap();
        g.forget_session(&paths, "b").unwrap();
        assert_eq!(g.list().len(), 1);
        assert!(Grants::load(&paths).list().is_empty(), "the kept grant on `b` went");
    }
}
