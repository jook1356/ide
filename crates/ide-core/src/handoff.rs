//! Session handoff (`docs/session-handoff.md`): which way the context reaches the new
//! conversation, and the prompts of both sides. UI-free; the server drives it.

use std::path::{Path, PathBuf};

/// How the context reaches the new conversation (the protocol's `HandoffMethod`, kept apart so
/// `ide-core` does not depend on the protocol).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The old conversation writes the handoff document.
    Previous,
    /// The new conversation writes it from the old transcript first.
    Summarize,
    /// The new conversation reads the old transcript as is.
    Transcript,
}

/// What the user asked for (`None` = automatic).
pub type Requested = Option<Method>;

/// The old conversation's state when a handoff is asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OldState {
    /// Its process runs.
    pub running: bool,
    /// It is in a turn or waits for the user (a request would wait for the turn's end).
    pub busy: bool,
    /// It is stopped but its conversation can be resumed (recorded, the provider resumes).
    pub resumable: bool,
    /// Its transcript can be exported (recorded, the provider reads transcripts).
    pub transcript: bool,
    /// A conversation is recorded at all.
    pub recorded: bool,
}

/// The resolved way: the method, and whether a failed `Previous` falls back to `Summarize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub method: Method,
    pub fallback: bool,
}

/// Why no handoff can be made (the protocol error id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `handoff-no-conversation`: nothing recorded to continue from.
    NoConversation,
    /// `handoff-no-transcript`: the provider's transcripts cannot be read.
    NoTranscript,
}

impl Refusal {
    /// The protocol error id.
    pub fn id(self) -> &'static str {
        match self {
            Refusal::NoConversation => "handoff-no-conversation",
            Refusal::NoTranscript => "handoff-no-transcript",
        }
    }
}

/// Resolve `requested` against the old conversation's state.
///
/// Automatic: `Previous` when the old agent is idle and running, or stopped and resumable
/// (falling back to `Summarize` when the transcript can be read); `Summarize` when it is
/// busy or cannot be continued and the transcript can be read; else `Previous` waits for a
/// busy agent's turn without a fallback.
pub fn resolve(requested: Requested, old: OldState) -> Result<Plan, Refusal> {
    let no_transcript = || {
        if old.recorded {
            Refusal::NoTranscript
        } else {
            Refusal::NoConversation
        }
    };
    let can_write = old.running || old.resumable;
    match requested {
        Some(Method::Previous) => can_write
            .then_some(Plan {
                method: Method::Previous,
                fallback: false,
            })
            .ok_or(Refusal::NoConversation),
        Some(m @ (Method::Summarize | Method::Transcript)) => old
            .transcript
            .then_some(Plan {
                method: m,
                fallback: false,
            })
            .ok_or_else(no_transcript),
        None => {
            if can_write && !(old.running && old.busy) {
                Ok(Plan {
                    method: Method::Previous,
                    fallback: old.transcript,
                })
            } else if old.transcript {
                Ok(Plan {
                    method: Method::Summarize,
                    fallback: false,
                })
            } else if can_write {
                Ok(Plan {
                    method: Method::Previous,
                    fallback: false,
                })
            } else {
                Err(no_transcript())
            }
        }
    }
}

/// `<session dir>/handoff`: the handoff files of a session.
pub fn dir(session_dir: &Path) -> PathBuf {
    session_dir.join("handoff")
}

/// The handoff document of the handoff made at `at` (Unix seconds).
pub fn document_path(dir: &Path, at: u64) -> PathBuf {
    dir.join(format!("{at}-handoff.md"))
}

/// The exported transcript of the handoff made at `at`.
pub fn transcript_path(dir: &Path, at: u64) -> PathBuf {
    dir.join(format!("{at}-transcript.md"))
}

/// What the handoff document covers (both writers get the same outline).
const OUTLINE: &str = "\
- Goal: what the user wants overall, and their requests and decisions so far (quote the \
user's own words where they matter: preferences, rules, corrections).
- State: what is done and what is in progress — files, branches, commits, uncommitted work, \
commands that build and test.
- Open problems and anything that failed or was ruled out (and why).
- Next steps, in order.
- Related sessions: children and what each is doing or was asked to do, messages you are \
waiting for.
- Anything else the next agent must know to continue without this conversation (gotchas, \
where things are, what not to touch).";

fn p(path: &Path) -> String {
    path.display().to_string()
}

/// Typed into the old conversation (`Previous`): write the handoff document to `doc`.
pub fn request_prompt(doc: &Path) -> String {
    format!(
        "[ide] This session is being handed off: a new agent conversation will replace this one \
         and continue your work in this same session (same folder, worktrees, branch and child \
         sessions; their messages will reach the new conversation). Write a handoff document \
         for it, in Markdown, to this file (create it; write nothing else):\n{}\n\nCover:\n{OUTLINE}\n\n\
         Be complete but concise: the new agent has only this document. Do not continue other \
         work, do not message other sessions; end your turn once the file is written.",
        p(doc)
    )
}

/// The new conversation's first prompt.
pub fn first_prompt(method: Method, doc: &Path, transcript: &Path) -> String {
    let intro = "[ide] You take over this session from a previous agent conversation (a \
                 handoff): same folder, worktrees, branch and child sessions; child sessions \
                 keep running, and their messages now reach you.";
    match method {
        Method::Previous => format!(
            "{intro}\n\nThe previous conversation wrote a handoff document: read it first, \
             entirely:\n{}\n\nThen check the current state (git status, related sessions with \
             `ide_list_related_sessions` when there are any) and continue where it left off. \
             If the next step is unclear, ask the user before acting.",
            p(doc)
        ),
        Method::Summarize => format!(
            "{intro}\n\nThe previous conversation's transcript (from its last compaction on, \
             tool calls and results shortened) is here:\n{}\n\nFirst read it entirely and write \
             a handoff document, in Markdown, to:\n{}\n\nCover:\n{OUTLINE}\n\nThen check the \
             current state (git status, related sessions with `ide_list_related_sessions` when \
             there are any) and continue from your document. The previous conversation may \
             have ended abnormally (stuck or crashed): do not repeat its last action blindly. \
             If the next step is unclear, ask the user before acting.",
            p(transcript),
            p(doc)
        ),
        Method::Transcript => format!(
            "{intro}\n\nThe previous conversation's transcript (from its last compaction on, \
             tool calls and results shortened) is here:\n{}\n\nRead it entirely: it is your \
             context. Then check the current state (git status, related sessions with \
             `ide_list_related_sessions` when there are any) and continue where it left off. \
             The previous conversation may have ended abnormally: do not repeat its last action \
             blindly. If the next step is unclear, ask the user before acting.",
            p(transcript)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(running: bool, busy: bool, resumable: bool, transcript: bool) -> OldState {
        OldState {
            running,
            busy,
            resumable,
            transcript,
            recorded: transcript || resumable || running,
        }
    }

    fn plan(method: Method, fallback: bool) -> Result<Plan, Refusal> {
        Ok(Plan { method, fallback })
    }

    #[test]
    fn auto_prefers_the_old_conversation_when_it_can_write() {
        assert_eq!(resolve(None, st(true, false, true, true)), plan(Method::Previous, true));
        // No transcript (e.g. Codex): no fallback.
        assert_eq!(resolve(None, st(true, false, true, false)), plan(Method::Previous, false));
        // Stopped but resumable.
        assert_eq!(resolve(None, st(false, false, true, true)), plan(Method::Previous, true));
    }

    #[test]
    fn auto_summarizes_when_busy_or_dead() {
        assert_eq!(resolve(None, st(true, true, true, true)), plan(Method::Summarize, false));
        assert_eq!(resolve(None, st(false, false, false, true)), plan(Method::Summarize, false));
        // Busy and no transcript: wait for its turn.
        assert_eq!(resolve(None, st(true, true, true, false)), plan(Method::Previous, false));
        assert_eq!(resolve(None, st(false, false, false, false)), Err(Refusal::NoConversation));
        let mut old = st(false, false, false, false);
        old.recorded = true;
        assert_eq!(resolve(None, old), Err(Refusal::NoTranscript));
    }

    #[test]
    fn explicit_methods_need_their_source() {
        let dead = st(false, false, false, false);
        assert_eq!(resolve(Some(Method::Previous), dead), Err(Refusal::NoConversation));
        assert_eq!(
            resolve(Some(Method::Previous), st(true, true, false, false)),
            plan(Method::Previous, false)
        );
        let codex = OldState {
            running: true,
            recorded: true,
            ..Default::default()
        };
        assert_eq!(resolve(Some(Method::Summarize), codex), Err(Refusal::NoTranscript));
        assert_eq!(
            resolve(Some(Method::Transcript), st(false, false, false, true)),
            plan(Method::Transcript, false)
        );
    }

    #[test]
    fn prompts_name_their_files() {
        let d = Path::new("/d");
        let (doc, tr) = (document_path(d, 7), transcript_path(d, 7));
        assert!(request_prompt(&doc).contains(&p(&doc)));
        assert!(first_prompt(Method::Previous, &doc, &tr).contains(&p(&doc)));
        let s = first_prompt(Method::Summarize, &doc, &tr);
        assert!(s.contains(&p(&doc)) && s.contains(&p(&tr)));
        let t = first_prompt(Method::Transcript, &doc, &tr);
        assert!(t.contains(&p(&tr)) && !t.contains(&p(&doc)));
    }
}
