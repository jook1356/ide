//! Approvals (`docs/approvals.md`): the daemon checks the user's grants before it carries out
//! what agents may only do with the user's consent, and asks the apps when none is kept.

use std::sync::Arc;
use std::time::Duration;

use meninx_core::approval::Answer;
use meninx_proto::{APPROVAL_REQUEST, ApprovalInfo, ProtoError, Response};

use super::UserCtx;

/// Capabilities asked for (`docs/approvals.md`).
pub(crate) const CAP_STEER: &str = "session.steer";
pub(crate) const CAP_INTERRUPT: &str = "session.interrupt";

impl UserCtx {
    /// A target's name: a session's title; a site (browser control) names itself.
    fn title_of(&self, session: &str) -> String {
        match self.resolve_session(session) {
            Ok(s) => s.title,
            // A terminal (`terminal.input`, id `<session>:t<n>`), else a site (browser control).
            Err(_) => self
                .uterm_label(session)
                .or_else(|| session.contains(':').then(|| session.to_owned()))
                .unwrap_or_default(),
        }
    }

    /// What a grant's target is called: a session's title, or the folder itself.
    fn target_title(&self, capability: &str, target: &str) -> String {
        if capability == super::write_guard::CAP_WRITE {
            target.to_owned()
        } else {
            self.title_of(target)
        }
    }

    /// May `requester` do `capability` to `target`? A kept grant answers at once; otherwise
    /// the user is asked in the app (`detail`: what exactly, shown in the question). `Err`:
    /// why not, for the agent.
    pub(crate) async fn approve(
        self: &Arc<Self>,
        requester: &str,
        capability: &str,
        target: &str,
        detail: &str,
    ) -> Result<(), String> {
        if self.grants.lock().unwrap().allows(capability, requester, target) {
            return Ok(());
        }
        let args = serde_json::json!({
            "capability": capability,
            "requester": requester,
            "requester_title": self.title_of(requester),
            "target": target,
            "target_title": self.target_title(capability, target),
            "detail": detail,
        });
        let r = crate::bridge::ask_app_within(
            &self.bridge,
            requester,
            APPROVAL_REQUEST,
            args.to_string(),
            || self.subscriber_count() > 0,
            |n| self.notify(n),
            self.ask_wait(),
        )
        .await;
        let answer = if r.is_error { None } else { Answer::parse(&r.text) };
        // Why not: the user refused, nobody answered in time, no window (the audit tells them
        // apart).
        let why = match answer {
            Some(a) => a.as_str(),
            None => match r.text.as_str() {
                crate::bridge::TIMED_OUT => "timed out",
                crate::bridge::NO_WINDOW => "no window",
                crate::bridge::GONE => "window closed",
                _ => "refused",
            },
        };
        self.audit("approval", &format!("{capability} {requester} -> {target}: {why}"));
        let Some(answer) = answer else {
            return Err(format!(
                "the user did not allow it ({}): do not retry or work around it; tell the user in your reply what was not allowed and ask them",
                if r.text.is_empty() { "refused" } else { r.text.as_str() }
            ));
        };
        self.grants
            .lock()
            .unwrap()
            .record(&self.paths, capability, requester, target, answer)
            .map_err(|e| format!("could not keep the approval: {e}"))?;
        Ok(())
    }

    /// How long a question to the user waits (`None`: until answered), as the user chose
    /// (Settings -> 권한).
    pub(crate) fn ask_wait(&self) -> Option<Duration> {
        self.grants.lock().unwrap().wait().map(Duration::from_secs)
    }

    /// `approval.set_wait`.
    pub(crate) fn approval_set_wait(&self, secs: Option<u64>) -> Result<Response, ProtoError> {
        self.grants
            .lock()
            .unwrap()
            .set_wait(&self.paths, secs)
            .map_err(|e| super::core_error(&e))?;
        let shown = secs.map_or("none".to_owned(), |s| s.to_string());
        self.audit("approval.set_wait", &shown);
        Ok(Response::Unit)
    }

    /// `approval.list`.
    pub(crate) fn approval_list(&self) -> Response {
        let list = self.grants.lock().unwrap().list().to_vec();
        Response::Approvals(
            list.into_iter()
                .map(|g| ApprovalInfo {
                    requester_title: self.title_of(&g.requester),
                    target_title: self.target_title(&g.capability, &g.target),
                    id: g.id,
                    capability: g.capability,
                    requester: g.requester,
                    target: g.target,
                    always: g.always,
                    created: g.created,
                })
                .collect(),
        )
    }

    /// `approval.revoke`.
    pub(crate) fn approval_revoke(&self, id: &str) -> Result<Response, ProtoError> {
        let gone = self
            .grants
            .lock()
            .unwrap()
            .revoke(&self.paths, id)
            .map_err(|e| super::core_error(&e))?;
        if !gone {
            return Err(ProtoError::new("approval-not-found").arg("id", id));
        }
        self.audit("approval.revoke", id);
        Ok(Response::Unit)
    }

    /// A session was deleted: grants it asked for or that act on it go.
    pub(crate) fn approvals_forget_session(&self, session: &str) {
        let _ = self.grants.lock().unwrap().forget_session(&self.paths, session);
    }

    /// A target went or is new (a user terminal): its grants go.
    pub(crate) fn approvals_forget_target(&self, target: &str) {
        let _ = self.grants.lock().unwrap().forget_target(&self.paths, target);
    }
}
