//! Session handoff (`docs/session-handoff.md`): the session stays, its agent conversation is
//! replaced by a new one that continues from a handoff document or the old transcript.
//! Requests start it; [`UserCtx::handoff_tick`] (the screen tick) drives a `Previous` handoff,
//! which waits for the old conversation to write the document.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use meninx_core::handoff::{self as core_handoff, Method, OldState, Plan, Refusal};
use meninx_core::{PreviousConversation, Session, SessionKind, SessionTarget};
use meninx_proto::{
    AgentActivity, ChildAgent, HandoffMethod, HandoffPhase, HandoffResult, HandoffStatus, Notify,
    ProtoError,
};

use super::UserCtx;

/// How long the old conversation has to write the document (from the request on, including
/// waiting for a running turn to end).
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// After the request was typed in: the activity may still show the old idle state this long.
const TURN_GRACE: Duration = Duration::from_secs(5);

/// A handoff in progress (`UserCtx::handoffs`, by session id).
pub(crate) struct Handoff {
    plan: Plan,
    phase: HandoffPhase,
    /// The new conversation's agent (`None` = the session's).
    agent: Option<SessionTarget>,
    started: Instant,
    /// When the request was typed into the old conversation (`Writing`).
    sent: Option<Instant>,
    /// The old conversation was seen working on the request.
    saw_working: bool,
    doc: PathBuf,
    transcript: PathBuf,
}

fn method_of(m: Method) -> HandoffMethod {
    match m {
        Method::Previous => HandoffMethod::Previous,
        Method::Summarize => HandoffMethod::Summarize,
        Method::Transcript => HandoffMethod::Transcript,
    }
}

fn busy(a: AgentActivity) -> bool {
    matches!(a, AgentActivity::Working | AgentActivity::NeedsInput)
}

impl UserCtx {
    /// `SessionInfo::handoff`.
    pub(crate) fn handoff_status(&self, id: &str) -> Option<HandoffStatus> {
        self.handoffs.lock().unwrap().get(id).map(|h| HandoffStatus {
            method: method_of(h.plan.method),
            phase: h.phase,
        })
    }

    /// A handoff of `id` is running: its queued messages wait for the new conversation.
    pub(crate) fn in_handoff(&self, id: &str) -> bool {
        self.handoffs.lock().unwrap().contains_key(id)
    }

    /// `session.handoff`.
    pub(crate) fn session_handoff(
        &self,
        id: &str,
        method: HandoffMethod,
        agent: Option<ChildAgent>,
    ) -> Result<Session, ProtoError> {
        let s = self.resolve_session(id)?;
        if s.kind != SessionKind::Agent {
            return Err(ProtoError::new("handoff-not-agent"));
        }
        Self::not_preparing(&s)?;
        if self.in_handoff(s.id.as_str()) {
            return Err(ProtoError::new("handoff-busy"));
        }
        let agent = agent.map(|a| SessionTarget {
            provider: a.provider,
            account: a.account,
            configuration: a.configuration,
        });
        if let Some(t) = &agent {
            self.check_target(t)?;
        }
        let p = self.provider(&s.provider)?;
        let running = self.existing_host(s.id.as_str()).is_some_and(|h| h.is_running());
        let recorded = s.conversation.is_some();
        let old = OldState {
            running,
            busy: running && busy(self.bridge.activity(s.id.as_str())),
            resumable: !running && recorded && p.capabilities().resume,
            transcript: recorded && p.reads_transcripts(),
            recorded,
        };
        let requested = match method {
            HandoffMethod::Auto => None,
            HandoffMethod::Previous => Some(Method::Previous),
            HandoffMethod::Summarize => Some(Method::Summarize),
            HandoffMethod::Transcript => Some(Method::Transcript),
        };
        let plan = core_handoff::resolve(requested, old).map_err(|r| match r {
            Refusal::NoTranscript => {
                ProtoError::new(r.id()).arg("provider", p.display_name())
            }
            Refusal::NoConversation => ProtoError::new(r.id()),
        })?;
        let dir = core_handoff::dir(&self.paths.session_dir(s.id.as_str()));
        meninx_core::ensure_dir(&dir).map_err(|e| super::core_error(&e))?;
        let at = meninx_core::now_unix();
        let h = Handoff {
            plan,
            phase: HandoffPhase::Waiting,
            agent,
            started: Instant::now(),
            sent: None,
            saw_working: false,
            doc: core_handoff::document_path(&dir, at),
            transcript: core_handoff::transcript_path(&dir, at),
        };
        self.audit(
            "session.handoff",
            &format!("{} method={:?} fallback={}", s.id, plan.method, plan.fallback),
        );
        if plan.method == Method::Previous {
            if !running {
                // Resumes its conversation; the request is typed in once it is at its input.
                self.start(&s)?;
            }
            self.handoffs.lock().unwrap().insert(s.id.to_string(), h);
            self.notify(Notify::SessionsChanged);
            return self.resolve_session(id);
        }
        self.handoffs.lock().unwrap().insert(s.id.to_string(), h);
        let r = self.export_transcript(&s).and_then(|()| self.handoff_finish(&s, plan.method));
        if let Err(e) = r {
            self.handoff_failed(&s, e.clone());
            return Err(e);
        }
        self.resolve_session(id)
    }

    /// `session.handoff_cancel`.
    pub(crate) fn session_handoff_cancel(&self, id: &str) -> Result<Session, ProtoError> {
        let s = self.resolve_session(id)?;
        let removed = {
            let mut all = self.handoffs.lock().unwrap();
            // A new conversation being started cannot be stopped half way.
            match all.get(s.id.as_str()) {
                Some(h) if h.phase != HandoffPhase::Starting => all.remove(s.id.as_str()),
                _ => None,
            }
        };
        if removed.is_none() {
            return Err(ProtoError::new("handoff-none"));
        }
        self.audit("session.handoff_cancel", s.id.as_str());
        self.notify(Notify::SessionsChanged);
        // Messages held for the new conversation go to the old one.
        self.deliver_pending(s.id.as_str());
        Ok(s)
    }

    /// Drive the running `Previous` handoffs (every screen tick).
    pub(crate) fn handoff_tick(&self) {
        let ids: Vec<String> = self.handoffs.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let Ok(s) = self.resolve_session(&id) else {
                // Deleted meanwhile.
                self.handoffs.lock().unwrap().remove(&id);
                continue;
            };
            if let Err(e) = self.handoff_step(&s) {
                self.handoff_fallback(&s, e);
            }
        }
    }

    /// One look at a `Previous` handoff: `Err` = the old conversation will not write it.
    fn handoff_step(&self, s: &Session) -> Result<(), ProtoError> {
        let id = s.id.as_str();
        let (phase, started, sent, saw_working, doc) = {
            let all = self.handoffs.lock().unwrap();
            let Some(h) = all.get(id) else {
                return Ok(());
            };
            if h.plan.method != Method::Previous {
                return Ok(());
            }
            (h.phase, h.started, h.sent, h.saw_working, h.doc.clone())
        };
        let running = self.existing_host(id).is_some_and(|h| h.is_running());
        // Without hooks or screen rules nothing reports its turns: the file alone decides.
        let tracked = self.provider(&s.provider).is_ok_and(|p| {
            p.capabilities().hooks != meninx_agents::Support::Unsupported || p.screen_manifest().is_some()
        });
        let activity = if tracked {
            self.bridge.activity(id)
        } else {
            AgentActivity::Unknown
        };
        let written = std::fs::metadata(&doc).is_ok_and(|m| m.is_file() && m.len() > 0);
        match phase {
            HandoffPhase::Starting => Ok(()),
            HandoffPhase::Waiting => {
                if !running {
                    return Err(ProtoError::new("handoff-exited"));
                }
                if started.elapsed() > HANDOFF_TIMEOUT {
                    return Err(ProtoError::new("handoff-timeout"));
                }
                if busy(activity) || !self.input_settled(id) {
                    return Ok(());
                }
                match self.send_prompt(id, &core_handoff::request_prompt(&doc)) {
                    Ok(()) => {
                        self.at_input.lock().unwrap().remove(id);
                        if tracked {
                            self.set_activity(id, AgentActivity::Working);
                        }
                        if let Some(h) = self.handoffs.lock().unwrap().get_mut(id) {
                            h.phase = HandoffPhase::Writing;
                            h.sent = Some(Instant::now());
                        }
                        self.audit("session.handoff_request", id);
                        self.notify(Notify::SessionsChanged);
                        Ok(())
                    }
                    Err(e) if e.id == "prompt-not-ready" => Ok(()),
                    Err(e) => Err(e),
                }
            }
            HandoffPhase::Writing => {
                let since = sent.map_or(Duration::ZERO, |t| t.elapsed());
                if busy(activity) {
                    if !saw_working
                        && let Some(h) = self.handoffs.lock().unwrap().get_mut(id)
                    {
                        h.saw_working = true;
                    }
                    if started.elapsed() > HANDOFF_TIMEOUT {
                        return if written {
                            self.handoff_finish(s, Method::Previous)
                        } else {
                            Err(ProtoError::new("handoff-timeout"))
                        };
                    }
                    return Ok(());
                }
                // Its turn ended (or it never showed one).
                if written && (saw_working || since > TURN_GRACE) {
                    return self.handoff_finish(s, Method::Previous);
                }
                if !running {
                    return Err(ProtoError::new("handoff-exited"));
                }
                if saw_working || since > TURN_GRACE * 6 {
                    return Err(ProtoError::new("handoff-no-document"));
                }
                Ok(())
            }
        }
    }

    /// A `Previous` handoff failed: fall back to `Summarize` when planned so, else give up.
    fn handoff_fallback(&self, s: &Session, e: ProtoError) {
        let fallback = {
            let mut all = self.handoffs.lock().unwrap();
            match all.get_mut(s.id.as_str()) {
                Some(h) if h.plan.fallback => {
                    h.plan = Plan {
                        method: Method::Summarize,
                        fallback: false,
                    };
                    true
                }
                _ => false,
            }
        };
        if !fallback {
            self.handoff_failed(s, e);
            return;
        }
        self.audit(
            "session.handoff_fallback",
            &format!("{} after {}", s.id, e.id),
        );
        let r = self
            .export_transcript(s)
            .and_then(|()| self.handoff_finish(s, Method::Summarize));
        if let Err(e) = r {
            self.handoff_failed(s, e);
        }
    }

    /// Write the old conversation's transcript to the handoff's transcript file.
    fn export_transcript(&self, s: &Session) -> Result<(), ProtoError> {
        let path = self
            .handoffs
            .lock()
            .unwrap()
            .get(s.id.as_str())
            .map(|h| h.transcript.clone())
            .ok_or_else(|| ProtoError::new("handoff-none"))?;
        let p = self.provider(&s.provider)?;
        let conversation = s
            .conversation
            .clone()
            .ok_or_else(|| ProtoError::new("handoff-no-conversation"))?;
        let account = s.account.as_ref().and_then(|slug| {
            self.accounts
                .lock()
                .unwrap()
                .require(&s.provider, slug)
                .ok()
                .cloned()
        });
        let t = match p.handoff_transcript(account.as_ref(), &conversation, &s.root) {
            None => {
                return Err(
                    ProtoError::new("handoff-no-transcript").arg("provider", p.display_name())
                );
            }
            Some(Err(e)) => return Err(ProtoError::new("handoff-transcript-failed").arg("detail", e)),
            Some(Ok(t)) => t,
        };
        std::fs::write(&path, t.markdown(&s.provider, &conversation)).map_err(|e| {
            ProtoError::new("handoff-transcript-failed").arg("detail", format!("{}: {e}", path.display()))
        })
    }

    /// Replace the conversation: stop the old process, keep its conversation in
    /// `previous_conversations`, apply the agent change and start the new one on its first
    /// prompt. A start that fails puts the old conversation back (and restarts it).
    fn handoff_finish(&self, s: &Session, method: Method) -> Result<(), ProtoError> {
        let (agent, doc, transcript) = {
            let mut all = self.handoffs.lock().unwrap();
            let h = all
                .get_mut(s.id.as_str())
                .ok_or_else(|| ProtoError::new("handoff-none"))?;
            h.phase = HandoffPhase::Starting;
            (h.agent.clone(), h.doc.clone(), h.transcript.clone())
        };
        self.notify(Notify::SessionsChanged);
        self.stop(s);
        let replaced = PreviousConversation {
            provider: s.provider.clone(),
            account: s.account.clone(),
            conversation: s.conversation.clone(),
            replaced_at: meninx_core::now_unix(),
        };
        let swap = |sessions: &mut meninx_core::SessionStore, new: bool| {
            sessions.update(&s.id, |x| {
                if new {
                    x.previous_conversations.push(replaced.clone());
                    let over = x
                        .previous_conversations
                        .len()
                        .saturating_sub(meninx_core::session::MAX_PREVIOUS_CONVERSATIONS);
                    x.previous_conversations.drain(..over);
                    x.conversation = None;
                    if let Some(t) = &agent {
                        x.provider = t.provider.clone();
                        x.account = t.account.clone();
                        x.configuration = t.configuration.clone();
                    }
                } else {
                    // Back to the old conversation.
                    if x.previous_conversations.last() == Some(&replaced) {
                        x.previous_conversations.pop();
                    }
                    x.conversation = s.conversation.clone();
                    x.provider = s.provider.clone();
                    x.account = s.account.clone();
                    x.configuration = s.configuration.clone();
                }
            })
        };
        swap(&mut self.sessions.lock().unwrap(), true).map_err(|e| super::core_error(&e))?;
        let new = self.resolve_session(s.id.as_str())?;
        let prompt = core_handoff::first_prompt(method, &doc, &transcript);
        // Held messages stay behind the first prompt.
        let held = self.bridge.pending(s.id.as_str()) > 0;
        match self.start_with(&new, Some(&prompt)) {
            Ok(in_args) => {
                if !in_args {
                    self.bridge.requeue(
                        s.id.as_str(),
                        crate::bridge::Queued {
                            id: 0,
                            from: None,
                            text: prompt,
                            at: meninx_core::now_unix(),
                        },
                    );
                }
            }
            Err(e) => {
                let _ = swap(&mut self.sessions.lock().unwrap(), false);
                if let Ok(old) = self.resolve_session(s.id.as_str()) {
                    let _ = self.start(&old);
                }
                return Err(e);
            }
        }
        self.handoffs.lock().unwrap().remove(s.id.as_str());
        self.audit(
            "session.handoff_done",
            &format!(
                "{} method={method:?} provider={} held_messages={held}",
                s.id, new.provider
            ),
        );
        self.notify(Notify::SessionsChanged);
        self.notify(Notify::Handoff {
            session: s.id.to_string(),
            result: HandoffResult::Done {
                method: method_of(method),
            },
        });
        Ok(())
    }

    /// Give up: the old conversation stays (running, if it was).
    fn handoff_failed(&self, s: &Session, error: ProtoError) {
        if self.handoffs.lock().unwrap().remove(s.id.as_str()).is_none() {
            return;
        }
        self.audit("session.handoff_failed", &format!("{} {}", s.id, error.id));
        self.notify(Notify::SessionsChanged);
        self.notify(Notify::Handoff {
            session: s.id.to_string(),
            result: HandoffResult::Failed { error },
        });
        self.deliver_pending(s.id.as_str());
    }
}
