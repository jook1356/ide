//! Auto answers (`docs/auto-answers.md`): which of an agent's dialogs ide answers by itself.
//! The answers themselves are the providers' (`ide-agents` screen manifests); this keeps the
//! user's choice per agent: `<data>/auto-answers.json`, only what differs from the defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, read_json, write_json};

/// `<data>/auto-answers.json`.
pub fn file(paths: &Paths) -> PathBuf {
    paths.root().join("auto-answers.json")
}

/// The user's choices: answer ids turned on / off per provider id.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoAnswers {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    on: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    off: BTreeMap<String, Vec<String>>,
}

impl AutoAnswers {
    pub fn load(paths: &Paths) -> AutoAnswers {
        read_json(&file(paths)).ok().flatten().unwrap_or_default()
    }

    /// Is answer `id` of `provider` on (`default`: the provider's default)?
    pub fn enabled(&self, provider: &str, id: &str, default: bool) -> bool {
        let has = |m: &BTreeMap<String, Vec<String>>| m.get(provider).is_some_and(|v| v.iter().any(|x| x == id));
        if has(&self.off) {
            false
        } else if has(&self.on) {
            true
        } else {
            default
        }
    }

    /// Turn an answer on or off (kept only where it differs from `default`).
    pub fn set(&mut self, paths: &Paths, provider: &str, id: &str, enabled: bool, default: bool) -> crate::Result<()> {
        for m in [&mut self.on, &mut self.off] {
            if let Some(v) = m.get_mut(provider) {
                v.retain(|x| x != id);
                if v.is_empty() {
                    m.remove(provider);
                }
            }
        }
        if enabled != default {
            let m = if enabled { &mut self.on } else { &mut self.off };
            m.entry(provider.to_owned()).or_default().push(id.to_owned());
        }
        write_json(&file(paths), self)
    }
}

/// Is `path` one of `roots` or inside one? (Windows: case and `/` vs `\` do not matter.)
pub fn inside_any(path: &str, roots: &[PathBuf]) -> bool {
    let norm = |p: &str| {
        let mut s = p.trim().replace('/', "\\");
        while s.len() > 3 && s.ends_with('\\') {
            s.pop();
        }
        if cfg!(windows) { s.to_lowercase() } else { s.replace('\\', "/") }
    };
    let p = norm(path);
    let sep = if cfg!(windows) { '\\' } else { '/' };
    !p.is_empty()
        && roots.iter().any(|r| {
            let r = norm(&r.display().to_string());
            !r.is_empty() && (p == r || p.strip_prefix(&r).is_some_and(|rest| rest.starts_with(sep)))
        })
}

/// [`inside_any`] for one root.
pub fn inside(path: &str, root: &Path) -> bool {
    inside_any(path, &[root.to_path_buf()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        let t = crate::paths::TempDir::new("auto-answers");
        let paths = Paths::new(t.0.clone());
        let mut a = AutoAnswers::load(&paths);
        assert!(a.enabled("claude", "trust", true));
        assert!(!a.enabled("claude", "other", false));
        a.set(&paths, "claude", "trust", false, true).unwrap();
        a.set(&paths, "claude", "other", true, false).unwrap();
        let a2 = AutoAnswers::load(&paths);
        assert!(!a2.enabled("claude", "trust", true));
        assert!(a2.enabled("claude", "other", false));
        assert!(a2.enabled("codex", "trust", true), "per provider");
        // Back to the default: nothing kept.
        let mut a3 = a2.clone();
        a3.set(&paths, "claude", "trust", true, true).unwrap();
        a3.set(&paths, "claude", "other", false, false).unwrap();
        assert_eq!(AutoAnswers::load(&paths), AutoAnswers::default());
    }

    #[test]
    fn folders() {
        let roots = [PathBuf::from(if cfg!(windows) { "C:\\w\\s1" } else { "/w/s1" })];
        let (inner, other, prefix) = if cfg!(windows) {
            ("c:/W/s1/ide/.git", "C:\\w\\s2", "C:\\w\\s1x")
        } else {
            ("/w/s1/ide/.git", "/w/s2", "/w/s1x")
        };
        assert!(inside_any(inner, &roots));
        assert!(inside_any(&roots[0].display().to_string(), &roots));
        assert!(!inside_any(other, &roots));
        assert!(!inside_any(prefix, &roots), "a name prefix is not inside");
        assert!(!inside_any("Yes, I trust this folder", &roots));
        assert!(!inside_any("", &roots));
    }
}
