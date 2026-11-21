//! How a prompt reaches an agent CLI through its terminal (`session.send_prompt`): the
//! provider says how its CLI takes input ([`AgentProvider::prompt_input`]), [`prompt_writes`]
//! turns a text into the PTY writes. Provider-specific choices stay here (see
//! `docs/changes-actions.md`, "Pull requests and agent prompts").

use serde::{Deserialize, Serialize};

/// How a CLI takes a prompt typed into its terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "mode")]
pub enum PromptInput {
    /// A full-screen TUI (Claude Code, Codex): the whole text as one bracketed paste
    /// (`ESC[200~` … `ESC[201~`, newlines as CR, like the app's own paste), then Enter (CR)
    /// after `submit_delay_ms`, so the TUI has taken the paste before the submit key. Only
    /// while the TUI has bracketed paste on (DECSET 2004), i.e. it shows its input box;
    /// otherwise the prompt is refused ([`PromptRefusal::NotReady`]).
    Paste {
        /// Pause between the paste and Enter, in milliseconds.
        #[serde(default = "default_submit_delay")]
        submit_delay_ms: u64,
    },
    /// A shell or plain program: every line typed and entered (CR after each line).
    Lines,
}

fn default_submit_delay() -> u64 {
    PASTE_SUBMIT_DELAY_MS
}

/// Default pause between a TUI paste and its Enter.
pub const PASTE_SUBMIT_DELAY_MS: u64 = 300;

/// One write to the PTY, after waiting `delay_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptWrite {
    /// Pause before this write (ms).
    pub delay_ms: u64,
    /// Bytes to write.
    pub bytes: Vec<u8>,
}

/// Why a prompt is not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptRefusal {
    /// Nothing left after removing control characters.
    Empty,
    /// The TUI has not enabled bracketed paste (still starting, a dialog, a login screen):
    /// typing the text there would answer whatever it shows.
    NotReady,
}

/// The prompt text without control characters (ESC included, so it can never end a paste or
/// start a sequence): CRLF / CR become LF; tabs and newlines stay; surrounding blank space is
/// trimmed.
pub fn clean_prompt(text: &str) -> String {
    let t = text.replace("\r\n", "\n").replace('\r', "\n");
    let t: String = t
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();
    t.trim().to_owned()
}

/// The PTY writes that submit `text` for a CLI taking input as `input`.
/// `bracketed_paste`: the terminal's DECSET 2004 state as the server's terminal model sees it.
pub fn prompt_writes(
    input: PromptInput,
    text: &str,
    bracketed_paste: bool,
) -> Result<Vec<PromptWrite>, PromptRefusal> {
    let text = clean_prompt(text);
    if text.is_empty() {
        return Err(PromptRefusal::Empty);
    }
    let cr = text.replace('\n', "\r");
    match input {
        PromptInput::Paste { submit_delay_ms } => {
            if !bracketed_paste {
                return Err(PromptRefusal::NotReady);
            }
            Ok(vec![
                PromptWrite {
                    delay_ms: 0,
                    bytes: format!("\x1b[200~{cr}\x1b[201~").into_bytes(),
                },
                PromptWrite {
                    delay_ms: submit_delay_ms,
                    bytes: b"\r".to_vec(),
                },
            ])
        }
        PromptInput::Lines => Ok(vec![PromptWrite {
            delay_ms: 0,
            bytes: format!("{cr}\r").into_bytes(),
        }]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_and_lines() {
        let paste = PromptInput::Paste {
            submit_delay_ms: 300,
        };
        let w = prompt_writes(paste, "  [커밋 요청] a\r\n- x\x1b[201~\x07\n", true).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(
            w[0].bytes,
            "\x1b[200~[커밋 요청] a\r- x[201~\x1b[201~".as_bytes()
        );
        assert_eq!((w[1].delay_ms, w[1].bytes.as_slice()), (300, &b"\r"[..]));
        assert_eq!(
            prompt_writes(paste, "hi", false),
            Err(PromptRefusal::NotReady)
        );
        assert_eq!(
            prompt_writes(paste, " \x1b\r\n", true),
            Err(PromptRefusal::Empty)
        );
        let w = prompt_writes(PromptInput::Lines, "echo a\necho b", false).unwrap();
        assert_eq!(
            w,
            vec![PromptWrite {
                delay_ms: 0,
                bytes: b"echo a\recho b\r".to_vec()
            }]
        );
    }

    #[test]
    fn providers_and_declarative() {
        use crate::AgentProvider;
        let paste = PromptInput::Paste {
            submit_delay_ms: PASTE_SUBMIT_DELAY_MS,
        };
        assert_eq!(crate::Claude.prompt_input(), paste);
        assert_eq!(crate::Codex.prompt_input(), paste);
        assert_eq!(crate::Shell.prompt_input(), PromptInput::Lines);
        let base = "id = \"tui\"\nname = \"T\"\nprogram = \"t\"\n";
        let p = |extra: &str| {
            crate::DeclarativeProvider::new(
                crate::DeclarativeSpec::parse(&format!("{base}{extra}")).unwrap(),
            )
            .prompt_input()
        };
        assert_eq!(p(""), PromptInput::Lines);
        assert_eq!(p("[prompt]\nmode = \"paste\"\n"), paste);
        assert_eq!(
            p("[prompt]\nmode = \"paste\"\nsubmit_delay_ms = 50\n"),
            PromptInput::Paste {
                submit_delay_ms: 50
            }
        );
        assert_eq!(p("[prompt]\nmode = \"lines\"\n"), PromptInput::Lines);
        assert!(crate::DeclarativeSpec::parse(&format!("{base}[prompt]\nmode = \"x\"\n")).is_err());
    }
}
