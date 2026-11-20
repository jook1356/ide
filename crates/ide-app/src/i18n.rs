//! UI language glue: translated labels for protocol types and server errors.
//!
//! Every user-visible string goes through `ide_i18n::tr` (see AGENTS.md); this module maps the
//! language-agnostic wire values (agent kinds, run states, [`ProtoError`] ids + args) to it.
//! The server never sends localized text.

use ide_client::{Agent, ProtoError};
pub use ide_i18n::tr;

use crate::app::RunState;

/// Display name of an agent kind.
pub fn agent_label(agent: Agent) -> String {
    match agent {
        Agent::Claude => tr::agent_claude(),
        Agent::Codex => tr::agent_codex(),
        Agent::Shell => tr::agent_shell(),
    }
}

/// Short status of a session process (sidebar subtitle).
pub fn status_label(state: RunState) -> String {
    match state {
        RunState::Stopped => tr::status_stopped(),
        RunState::Starting => tr::status_starting(),
        RunState::Running => tr::status_running(),
        RunState::Exited(Some(code)) => tr::status_exited_code(&code.to_string()),
        RunState::Exited(None) => tr::status_exited(),
        RunState::Failed => tr::status_failed(),
    }
}

/// `ide_core::IoAction::id()` values.
fn io_action(id: &str) -> String {
    match id {
        "reading" => tr::io_reading(),
        "writing" => tr::io_writing(),
        "replacing" => tr::io_replacing(),
        "creating-directory" => tr::io_creating_directory(),
        "renaming" => tr::io_renaming(),
        "removing" => tr::io_removing(),
        other => other.to_owned(),
    }
}

/// `ide_core::NameRule::id()` values.
fn name_rule(id: &str) -> String {
    match id {
        "empty" => tr::name_rule_empty(),
        "too-long" => tr::name_rule_too_long(),
        "invalid-chars" => tr::name_rule_invalid_chars(),
        "bad-start" => tr::name_rule_bad_start(),
        other => other.to_owned(),
    }
}

/// A server/connection error in the current UI language. Every id the server, `ide-core` and
/// the client produce has a message; an unknown id (newer server) gets a generic message that
/// shows the id and its arguments.
pub fn proto_error(e: &ProtoError) -> String {
    let a = |name: &str| e.get(name).unwrap_or("").to_owned();
    match e.id.as_str() {
        // ide-core (ide_core::ErrorMessage ids)
        "io" => tr::err_io(&io_action(&a("action")), &a("path"), &a("detail")),
        "json" => tr::err_json(&a("path"), &a("detail")),
        "no-data-dir" => tr::err_no_data_dir(),
        "invalid-name" => tr::err_invalid_name(&a("name"), &name_rule(&a("reason"))),
        "profile-exists" => tr::err_profile_exists(&a("name")),
        "profile-not-found" => tr::err_profile_not_found(&a("name")),
        "last-profile" => tr::err_last_profile(&a("name")),
        "session-not-found" => tr::err_session_not_found(&a("id")),
        "invalid-session-id" => tr::err_invalid_session_id(&a("id")),
        "not-a-git-repo" => tr::err_not_a_git_repo(&a("path"), &a("detail")),
        "git-spawn" => tr::err_git_spawn(&a("detail")),
        "git" => tr::err_git(&a("args"), &a("status"), &a("detail")),
        "worktree-exists" => tr::err_worktree_exists(&a("path")),
        "program-not-found" => tr::err_program_not_found(&a("program")),
        // protocol / server
        "bad-message" => tr::err_bad_message(&a("detail")),
        "bad-request" => tr::err_bad_request(&a("method")),
        "proto-version" => tr::err_proto_version(&a("server"), &a("client")),
        "not-handshaken" => tr::err_not_handshaken(),
        "unauthorized" => tr::err_unauthorized(),
        "session-running" => tr::err_session_running(&a("id")),
        "session-not-running" => tr::err_session_not_running(&a("id")),
        "spawn-failed" => tr::err_spawn_failed(&a("program"), &a("detail")),
        "internal" => tr::err_internal(&a("detail")),
        "message-too-large" => tr::err_message_too_large(),
        "path-denied" => tr::err_path_denied(&a("path")),
        // client side
        "disconnected" => tr::err_disconnected(),
        "timeout" => tr::err_timeout(),
        "connect-failed" => tr::err_connect_failed(&a("detail")),
        other => {
            let details = e
                .args
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ");
            tr::err_unknown(other, &details)
        }
    }
}

/// A count as the `i64` the generated plural functions take.
pub fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_i18n::{Lang, set_language};

    #[test]
    fn server_errors_follow_the_language() {
        let e = ProtoError::new("not-a-git-repo")
            .arg("path", "C:/x")
            .arg("detail", "fatal: not a git repository");
        let invalid = ProtoError::new("invalid-name")
            .arg("name", "Bad Name")
            .arg("reason", "invalid-chars");
        let io = ProtoError::new("io")
            .arg("action", "creating-directory")
            .arg("path", "C:/d")
            .arg("detail", "denied");
        let unknown = ProtoError::new("quota-exceeded").arg("limit", "3");
        // One test changes the global language, so both languages are checked here.
        set_language(Lang::Ko);
        assert_eq!(
            proto_error(&e),
            "git 저장소가 아니에요: C:/x (fatal: not a git repository)"
        );
        assert!(proto_error(&invalid).contains("영문 소문자"));
        assert!(proto_error(&io).contains("폴더 만들기"));
        assert_eq!(
            proto_error(&unknown),
            "서버 오류가 발생했어요 (quota-exceeded): limit=3"
        );
        assert_eq!(status_label(RunState::Exited(Some(3))), "종료됨 (3)");
        assert_eq!(agent_label(Agent::Shell), "셸");
        set_language(Lang::En);
        assert!(proto_error(&e).contains("is not a git repository"));
        assert_eq!(
            proto_error(&ProtoError::new("disconnected")),
            "Not connected to the server."
        );
        assert_eq!(
            proto_error(&ProtoError::new("session-running").arg("id", "s1")),
            "Session s1 is already running."
        );
        assert_eq!(
            proto_error(&unknown),
            "Server error quota-exceeded: limit=3"
        );
        assert_eq!(status_label(RunState::Running), "running");
        assert_eq!(agent_label(Agent::Claude), "Claude");
    }
}
