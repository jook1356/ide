//! Auto answers (`docs/auto-answers.md`): the screen watch answers an agent CLI's dialogs that
//! the user left on for that agent (e.g. Claude Code's folder trust questions), only for folders
//! ide set up for the session.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ide_agents::AgentProvider;
use ide_agents::screen::AutoAnswer;
use ide_core::Session;
use ide_proto::{AutoAnswerInfo, ProtoError, Response};

use super::UserCtx;
use crate::pty::TermHost;

/// The same dialog of a session is not answered again within this time (the screen may still
/// show it while the CLI takes the keys).
const COOLDOWN: Duration = Duration::from_secs(5);
/// Pause between the writes of one answer (a list moves, then Enter).
const KEY_GAP: Duration = Duration::from_millis(150);

impl UserCtx {
    /// The answers of a provider and the user's choice for each.
    pub(crate) fn answers_of(&self, p: &dyn AgentProvider) -> Vec<AutoAnswerInfo> {
        let Some(m) = p.screen_manifest() else {
            return Vec::new();
        };
        let choices = self.auto_answers.lock().unwrap();
        m.answers
            .iter()
            .map(|a| AutoAnswerInfo {
                id: a.id.clone(),
                enabled: choices.enabled(p.id(), &a.id, a.default),
            })
            .collect()
    }

    /// `provider.set_auto_answer`.
    pub(crate) fn set_auto_answer(&self, provider: &str, answer: &str, enabled: bool) -> Result<Response, ProtoError> {
        let p = self.provider(provider)?;
        let Some(a) = p.screen_manifest().and_then(|m| m.answers.iter().find(|a| a.id == answer)) else {
            return Err(ProtoError::new("bad-request").arg("method", "provider.set_auto_answer"));
        };
        self.auto_answers
            .lock()
            .unwrap()
            .set(&self.paths, provider, answer, enabled, a.default)
            .map_err(|e| super::core_error(&e))?;
        self.audit("provider.set_auto_answer", &format!("{provider} {answer} {enabled}"));
        Ok(Response::Unit)
    }

    /// The folders ide set up for `s`: its folder, its worktrees and their repositories (a
    /// worktree's git dir lives in the repository).
    fn own_folders(&self, s: &Session) -> Vec<PathBuf> {
        let repos = self.repos.lock().unwrap();
        let mut v = vec![s.root.clone()];
        for w in &s.worktrees {
            v.push(PathBuf::from(&w.path));
            if let Some(r) = repos.get(&w.repo) {
                v.push(PathBuf::from(&r.path));
            }
        }
        v
    }

    /// Answer the dialog `a` on `s`'s screen when the user left it on, it names one of the
    /// session's folders (when it names one) and it was not just answered.
    pub(crate) fn auto_answer(&self, s: &Session, host: &TermHost, a: &AutoAnswer, path: Option<&str>) {
        let on = self.auto_answers.lock().unwrap().enabled(&s.provider, &a.id, a.default);
        if !on {
            return;
        }
        if let Some(p) = path
            && !ide_core::auto_answer::inside_any(p, &self.own_folders(s))
        {
            tracing::info!("session {}: {} not answered: {p} is not one of its folders", s.id, a.id);
            return;
        }
        {
            let mut last = self.answered.lock().unwrap();
            let key = format!("{} {}", s.id, a.id);
            if last.get(&key).is_some_and(|t| t.elapsed() < COOLDOWN) {
                return;
            }
            last.insert(key, Instant::now());
        }
        for (i, k) in a.keys.iter().enumerate() {
            if i > 0 {
                std::thread::sleep(KEY_GAP);
            }
            if let Err(e) = host.write(k.as_bytes().to_vec()) {
                tracing::warn!("session {}: auto answer {}: {e}", s.id, a.id);
                return;
            }
        }
        self.audit("auto_answer", &format!("{} {} {}", s.id, a.id, path.unwrap_or("")));
    }
}
