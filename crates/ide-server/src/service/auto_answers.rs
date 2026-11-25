//! Auto answers (`docs/auto-answers.md`): the screen watch answers an agent CLI's dialogs that
//! the user left on for that agent (e.g. Claude Code's folder trust questions), only for folders
//! ide set up for the session.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ide_agents::AgentProvider;
use ide_agents::screen::Answering;
use ide_core::Session;
use ide_proto::{AutoAnswerInfo, ProtoError, Response};

use super::UserCtx;
use crate::pty::TermHost;

/// The same step of a dialog (session, answer, folder, keys) is not typed again within this
/// time (the screen may still show it while the CLI takes the keys). A confirming step (with
/// Enter) waits the longest: a late Enter must not land on the next dialog.
const COOLDOWN: Duration = Duration::from_secs(5);
/// The same for a step that only moves a list's cursor: a lost move is harmless to repeat
/// (Enter follows only when the screen shows the cursor on the entry), so it is retried sooner.
const MOVE_COOLDOWN: Duration = Duration::from_secs(2);
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
    /// worktree's git dir lives in the repository), also those of worktrees still being
    /// created: the agent starts with their git dirs already (`--add-dir`), and their dialogs
    /// otherwise blocked the session until the worktree was made (seen 2026-11-25: 14 s).
    fn own_folders(&self, s: &Session) -> Vec<PathBuf> {
        let repos = self.repos.lock().unwrap();
        let mut v = vec![s.root.clone()];
        for w in &s.worktrees {
            v.push(PathBuf::from(&w.path));
            if let Some(r) = repos.get(&w.repo) {
                v.push(PathBuf::from(&r.path));
            }
        }
        for p in &s.pending {
            v.push(s.root.join(&p.name));
            if let Some(r) = repos.get(&p.repo) {
                v.push(PathBuf::from(&r.path));
            }
        }
        v
    }

    /// Answer the dialog `a` on `s`'s screen when the user left it on, it names one of the
    /// session's folders (when it names one) and that dialog (the same folder) was not just
    /// answered. `true`: answered now or held back by [`COOLDOWN`], so the screen must be read
    /// again later even if it does not change (one dialog per repository may follow another,
    /// and keys the CLI missed leave the dialog unchanged: it is answered again after
    /// [`COOLDOWN`]).
    pub(crate) fn auto_answer(&self, s: &Session, host: &TermHost, ans: &Answering<'_>) -> bool {
        let (a, path) = (ans.answer, ans.path.as_deref());
        let confirms = ans.keys.iter().any(|k| k.contains('\r'));
        let on = self.auto_answers.lock().unwrap().enabled(&s.provider, &a.id, a.default);
        if !on {
            return false;
        }
        if let Some(p) = path
            && !ide_core::auto_answer::inside_any(p, &self.own_folders(s))
        {
            tracing::info!("session {}: {} not answered: {p} is not one of its folders", s.id, a.id);
            return false;
        }
        {
            let mut last = self.answered.lock().unwrap();
            // Per folder: each repository's git dir is its own dialog. Per step: the Enter
            // that follows a move is not held back by the move.
            let key = format!(
                "{} {} {} {}",
                s.id,
                a.id,
                path.unwrap_or(""),
                ans.keys.concat().escape_debug()
            );
            let wait = if confirms { COOLDOWN } else { MOVE_COOLDOWN };
            if last.get(&key).is_some_and(|t| t.elapsed() < wait) {
                return true;
            }
            last.retain(|_, t| t.elapsed() < COOLDOWN);
            last.insert(key, Instant::now());
        }
        for (i, k) in ans.keys.iter().enumerate() {
            if i > 0 {
                std::thread::sleep(KEY_GAP);
            }
            if let Err(e) = host.write(k.as_bytes().to_vec()) {
                tracing::warn!("session {}: auto answer {}: {e}", s.id, a.id);
                return false;
            }
        }
        let step = if confirms { "confirm" } else { "move" };
        self.audit("auto_answer", &format!("{} {} {} {step}", s.id, a.id, path.unwrap_or("")));
        true
    }
}
