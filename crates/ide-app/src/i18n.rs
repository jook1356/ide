//! UI language glue: translated labels for core types and `ide_core` errors.
//!
//! Every user-visible string goes through `ide_i18n::tr` (see AGENTS.md); this module maps the
//! language-agnostic core values (agent kinds, run states, [`ide_core::ErrorMessage`]) to it.

use ide_core::{AgentKind, ErrorMessage, IoAction, NameRule};
pub use ide_i18n::tr;

use crate::app::RunState;

/// Display name of an agent kind.
pub fn agent_label(agent: AgentKind) -> String {
    match agent {
        AgentKind::Claude => tr::agent_claude(),
        AgentKind::Codex => tr::agent_codex(),
        AgentKind::Shell => tr::agent_shell(),
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

fn io_action(a: IoAction) -> String {
    match a {
        IoAction::Reading => tr::io_reading(),
        IoAction::Writing => tr::io_writing(),
        IoAction::Replacing => tr::io_replacing(),
        IoAction::CreatingDirectory => tr::io_creating_directory(),
        IoAction::Renaming => tr::io_renaming(),
        IoAction::Removing => tr::io_removing(),
    }
}

fn name_rule(r: NameRule) -> String {
    match r {
        NameRule::Empty => tr::name_rule_empty(),
        NameRule::TooLong => tr::name_rule_too_long(),
        NameRule::InvalidChars => tr::name_rule_invalid_chars(),
        NameRule::BadStart => tr::name_rule_bad_start(),
    }
}

/// An `ide_core` error in the current UI language.
pub fn error(e: &ide_core::Error) -> String {
    let p = |p: &std::path::Path| p.display().to_string();
    match e.i18n() {
        ErrorMessage::Io {
            action,
            path,
            detail,
        } => tr::err_io(&io_action(action), &p(path), &detail),
        ErrorMessage::Json { path, detail } => tr::err_json(&p(path), &detail),
        ErrorMessage::NoDataDir => tr::err_no_data_dir(),
        ErrorMessage::InvalidName { name, reason } => {
            tr::err_invalid_name(name, &name_rule(reason))
        }
        ErrorMessage::ProfileExists { name } => tr::err_profile_exists(name),
        ErrorMessage::ProfileNotFound { name } => tr::err_profile_not_found(name),
        ErrorMessage::LastProfile { name } => tr::err_last_profile(name),
        ErrorMessage::SessionNotFound { id } => tr::err_session_not_found(id),
        ErrorMessage::InvalidSessionId { id } => tr::err_invalid_session_id(id),
        ErrorMessage::NotAGitRepo { path, detail } => tr::err_not_a_git_repo(&p(path), detail),
        ErrorMessage::GitSpawn { detail } => tr::err_git_spawn(&detail),
        ErrorMessage::Git {
            args,
            status,
            detail,
        } => tr::err_git(args, status, detail),
        ErrorMessage::WorktreeExists { path } => tr::err_worktree_exists(&p(path)),
        ErrorMessage::ProgramNotFound { program } => tr::err_program_not_found(program),
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
    fn core_errors_follow_the_language() {
        let e = ide_core::Error::NotAGitRepo {
            path: "C:/x".into(),
            stderr: "fatal: not a git repository".into(),
        };
        let invalid = ide_core::validate_slug("Bad Name").unwrap_err();
        // One test changes the global language, so both languages are checked here.
        set_language(Lang::Ko);
        assert_eq!(
            error(&e),
            "git 저장소가 아니에요: C:/x (fatal: not a git repository)"
        );
        assert!(
            error(&invalid).contains("영문 소문자"),
            "{}",
            error(&invalid)
        );
        assert_eq!(status_label(RunState::Exited(Some(3))), "종료됨 (3)");
        assert_eq!(agent_label(AgentKind::Shell), "셸");
        set_language(Lang::En);
        assert!(error(&e).contains("is not a git repository"));
        assert_eq!(status_label(RunState::Running), "running");
        assert_eq!(agent_label(AgentKind::Claude), "Claude");
    }
}
