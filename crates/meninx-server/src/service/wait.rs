//! `meninx_wait_session` (`docs/session-family.md`): an agent waits until related sessions
//! (typically the children it gave work) are done with their turn, instead of polling
//! `meninx_list_related_sessions`.
//!
//! Done = idle with none of the caller's messages still queued and no prompt it has not
//! started on, waiting for the user (`needs_input`), not running, or an agent whose activity
//! cannot be read. A state counts only after it held for [`SETTLE`] (a turn can look idle for a
//! moment between steps). The wait ends before the calling CLI would give up on the tool call
//! (`AgentProvider::mcp_call_limit`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use meninx_proto::{AgentActivity, AgentToolResult, SessionState};

use super::UserCtx;

/// Default and longest wait (seconds).
const DEFAULT_SECS: u64 = 600;
const MAX_SECS: u64 = 1800;
/// How often the targets are checked.
const POLL: Duration = Duration::from_millis(500);
/// How long a done state must hold before it counts.
pub(crate) const SETTLE: Duration = Duration::from_secs(2);
/// Kept free before the calling CLI's tool-call limit.
const LIMIT_MARGIN: Duration = Duration::from_secs(5);

/// What a waited-for session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitState {
    /// Its turn runs.
    Working,
    /// A prompt or message for it is not started yet (queued, or on its command line).
    PromptPending,
    /// Running, nothing reported yet (starting).
    Starting,
    /// Its conversation is being handed off to a new one.
    HandingOff,
    Idle,
    NeedsInput,
    NotRunning,
    /// Its CLI reports nothing Meninx can read (no hooks, no screen rules).
    Unreadable,
}

impl WaitState {
    pub(crate) fn done(self) -> bool {
        matches!(
            self,
            WaitState::Idle | WaitState::NeedsInput | WaitState::NotRunning | WaitState::Unreadable
        )
    }

    /// Done at once, without [`SETTLE`]: nothing will change by itself.
    fn final_now(self) -> bool {
        matches!(self, WaitState::NotRunning | WaitState::Unreadable)
    }

    fn name(self) -> &'static str {
        match self {
            WaitState::Working => "working",
            WaitState::PromptPending => "prompt_pending",
            WaitState::Starting => "starting",
            WaitState::HandingOff => "handing_off",
            WaitState::Idle => "idle",
            WaitState::NeedsInput => "needs_input",
            WaitState::NotRunning => "not_running",
            WaitState::Unreadable => "unreadable",
        }
    }
}

/// A session's wait state from what is known about it (pure: unit-tested).
pub(crate) fn classify(
    running: bool,
    handing_off: bool,
    activity: AgentActivity,
    pending: usize,
    expecting: bool,
    readable: bool,
) -> WaitState {
    if handing_off {
        return WaitState::HandingOff;
    }
    if !running {
        return WaitState::NotRunning;
    }
    match activity {
        // The user has to act first; queued messages wait for that too.
        AgentActivity::NeedsInput => WaitState::NeedsInput,
        AgentActivity::Working => WaitState::Working,
        _ if pending > 0 || expecting => WaitState::PromptPending,
        AgentActivity::Idle => WaitState::Idle,
        AgentActivity::Unknown if readable => WaitState::Starting,
        AgentActivity::Unknown => WaitState::Unreadable,
    }
}

/// Tracks how long each session has been done (the [`SETTLE`] rule).
#[derive(Default)]
pub(crate) struct Settle {
    since: HashMap<String, Instant>,
}

impl Settle {
    /// Record `state` of `id` seen at `now`; whether it counts as done.
    pub(crate) fn update(&mut self, id: &str, state: WaitState, now: Instant) -> bool {
        if !state.done() {
            self.since.remove(id);
            return false;
        }
        if state.final_now() {
            return true;
        }
        let since = *self.since.entry(id.to_owned()).or_insert(now);
        now.duration_since(since) >= SETTLE
    }
}

fn result(v: serde_json::Value, is_error: bool) -> AgentToolResult {
    AgentToolResult {
        text: if is_error {
            v.as_str().map_or_else(|| v.to_string(), str::to_owned)
        } else {
            serde_json::to_string_pretty(&v).expect("json")
        },
        is_error,
        image: None,
    }
}

impl UserCtx {
    /// What `id` is doing, for [`Self::wait_tool`].
    fn wait_state(&self, id: &str) -> WaitState {
        let running = self
            .existing_host(id)
            .is_some_and(|h| h.state() == SessionState::Running);
        let readable = self.bridge.is_hooked(id)
            || self.resolve_session(id).is_ok_and(|s| {
                self.providers
                    .get(&s.provider)
                    .and_then(|p| p.screen_manifest())
                    .is_some()
            });
        classify(
            running,
            self.in_handoff(id),
            self.bridge.activity(id),
            self.bridge.pending(id),
            self.bridge.expecting(id),
            readable,
        )
    }

    /// `meninx_wait_session` of `session`'s agent.
    pub(crate) async fn wait_tool(self: &Arc<Self>, session: &str, args: &str) -> AgentToolResult {
        let fail = |t: String| result(serde_json::Value::String(t), true);
        let args: serde_json::Value =
            match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
                Ok(v) => v,
                Err(e) => return fail(format!("invalid arguments: {e}")),
            };
        let me = match self.resolve_session(session) {
            Ok(s) => s,
            Err(e) => return fail(format!("session not available: {e}")),
        };
        let mut ids: Vec<String> = Vec::new();
        for v in args.get("sessions").and_then(|v| v.as_array()).into_iter().flatten() {
            let Some(id) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                return fail("`sessions` holds session ids (strings)".into());
            };
            if !ids.iter().any(|x| x == id) {
                ids.push(id.to_owned());
            }
        }
        if ids.is_empty() {
            return fail("`sessions` is required: the related sessions to wait for".into());
        }
        let mut titles = HashMap::new();
        for id in &ids {
            match self.related_target(&me, id) {
                Ok(t) => {
                    titles.insert(id.clone(), t.title);
                }
                Err(e) => return fail(e),
            }
        }
        let any = match args.get("mode").and_then(|v| v.as_str()).unwrap_or("all") {
            "all" => false,
            "any" => true,
            other => return fail(format!("unknown mode `{other}` (all, any)")),
        };
        let asked = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_SECS)
            .clamp(1, MAX_SECS);
        let mut limit = Duration::from_secs(asked);
        // The caller's CLI ends the tool call itself after its own limit: answer before that.
        let cli_limit = self
            .provider(&me.provider)
            .ok()
            .and_then(|p| p.mcp_call_limit());
        let mut note = None;
        if let Some(cli) = cli_limit
            && limit + LIMIT_MARGIN > cli
        {
            limit = cli.saturating_sub(LIMIT_MARGIN).max(Duration::from_secs(1));
            note = Some(format!(
                "your CLI ends tool calls after {} s, so this wait is at most {} s: call again to keep waiting",
                cli.as_secs(),
                limit.as_secs()
            ));
        }

        let start = Instant::now();
        let mut settle = Settle::default();
        let (states, finished) = loop {
            let now = Instant::now();
            let states: Vec<(String, WaitState, bool)> = ids
                .iter()
                .map(|id| {
                    let st = self.wait_state(id);
                    let done = settle.update(id, st, now);
                    (id.clone(), st, done)
                })
                .collect();
            let finished = if any {
                states.iter().any(|(_, _, d)| *d)
            } else {
                states.iter().all(|(_, _, d)| *d)
            };
            if finished || start.elapsed() >= limit {
                break (states, finished);
            }
            tokio::time::sleep(POLL.min(limit.saturating_sub(start.elapsed()))).await;
        };
        let rows: Vec<serde_json::Value> = states
            .iter()
            .map(|(id, st, done)| {
                serde_json::json!({
                    "id": id,
                    "title": titles.get(id).cloned().unwrap_or_default(),
                    "state": st.name(),
                    "done": done,
                    "queued_messages": self.bridge.pending(id),
                })
            })
            .collect();
        let mut v = serde_json::json!({
            "done": finished,
            "timed_out": !finished,
            "waited_seconds": start.elapsed().as_secs(),
            "sessions": rows,
        });
        if let Some(n) = note {
            v["note"] = serde_json::Value::String(n);
        }
        result(v, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentActivity::*;

    #[test]
    fn states() {
        // (running, handing_off, activity, pending, expecting, readable)
        assert_eq!(classify(false, false, Working, 0, false, true), WaitState::NotRunning);
        assert_eq!(classify(true, true, Idle, 0, false, true), WaitState::HandingOff);
        assert_eq!(classify(true, false, Working, 0, false, true), WaitState::Working);
        assert_eq!(classify(true, false, Idle, 0, false, true), WaitState::Idle);
        // A queued message or a prompt on the command line: not done while idle.
        assert_eq!(classify(true, false, Idle, 1, false, true), WaitState::PromptPending);
        assert_eq!(classify(true, false, Idle, 0, true, true), WaitState::PromptPending);
        assert_eq!(classify(true, false, Unknown, 0, true, true), WaitState::PromptPending);
        // Waiting for the user wins: the queued message waits for that too.
        assert_eq!(classify(true, false, NeedsInput, 2, true, true), WaitState::NeedsInput);
        assert_eq!(classify(true, false, Unknown, 0, false, true), WaitState::Starting);
        assert_eq!(classify(true, false, Unknown, 0, false, false), WaitState::Unreadable);
        for (s, done) in [
            (WaitState::Working, false),
            (WaitState::PromptPending, false),
            (WaitState::Starting, false),
            (WaitState::HandingOff, false),
            (WaitState::Idle, true),
            (WaitState::NeedsInput, true),
            (WaitState::NotRunning, true),
            (WaitState::Unreadable, true),
        ] {
            assert_eq!(s.done(), done, "{s:?}");
        }
    }

    #[test]
    fn settle() {
        let t0 = Instant::now();
        let mut s = Settle::default();
        assert!(!s.update("a", WaitState::Idle, t0), "idle must hold first");
        assert!(!s.update("a", WaitState::Idle, t0 + Duration::from_millis(1500)));
        assert!(s.update("a", WaitState::Idle, t0 + SETTLE));
        // Working again starts over; a change between done states keeps the time.
        assert!(!s.update("a", WaitState::Working, t0 + Duration::from_secs(3)));
        assert!(!s.update("a", WaitState::Idle, t0 + Duration::from_secs(4)));
        assert!(!s.update("a", WaitState::NeedsInput, t0 + Duration::from_secs(5)));
        assert!(s.update("a", WaitState::NeedsInput, t0 + Duration::from_secs(6)));
        // Not running: done at once.
        assert!(s.update("b", WaitState::NotRunning, t0));
    }
}
