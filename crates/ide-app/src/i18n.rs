//! UI language glue: translated labels for protocol types and server errors.
//!
//! Every user-visible string goes through `ide_i18n::tr` (see AGENTS.md); this module maps the
//! language-agnostic wire values (run states, [`ProtoError`] ids + args) to it.
//! The server never sends localized text.

use ide_client::ProtoError;
pub use ide_i18n::tr;

use crate::app::RunState;

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
    let n = |name: &str| e.get(name).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    match e.id.as_str() {
        // ide-core (ide_core::ErrorMessage ids)
        "io" => tr::err_io(&io_action(&a("action")), &a("path"), &a("detail")),
        "json" => tr::err_json(&a("path"), &a("detail")),
        "no-data-dir" => tr::err_no_data_dir(),
        "invalid-name" => tr::err_invalid_name(&a("name"), &name_rule(&a("reason"))),
        "account-not-found" => tr::err_account_not_found(&a("name"), &a("provider")),
        "account-in-use" => tr::err_account_in_use(n("count"), &a("name")),
        "configuration-not-found" => tr::err_configuration_not_found(&a("name")),
        "configuration-in-use" => tr::err_configuration_in_use(n("count"), &a("name")),
        "last-configuration" => tr::err_last_configuration(&a("name")),
        "skill-not-found" => tr::err_skill_not_found(&a("name")),
        "skill-exists" => tr::err_skill_exists(&a("name")),
        "skill-invalid" => tr::err_skill_invalid(&a("path")),
        "attach-too-large" => tr::err_attach_too_large(&a("max")),
        "attach-not-found" => tr::err_attach_not_found(&a("name")),
        "provider-not-found" => tr::err_provider_not_found(&a("provider")),
        "provider-no-login" => tr::err_provider_no_login(&a("provider")),
        "invalid-mcp-name" => tr::err_invalid_mcp_name(&a("name")),
        "session-not-found" => tr::err_session_not_found(&a("id")),
        "uterm-no-shell" => tr::err_uterm_no_shell(),
        "uterm-not-found" => tr::err_uterm_not_found(&a("id")),
        "invalid-session-id" => tr::err_invalid_session_id(&a("id")),
        "not-a-git-repo" => tr::err_not_a_git_repo(&a("path"), &a("detail")),
        "git-spawn" => tr::err_git_spawn(&a("detail")),
        "git" => tr::err_git(&a("args"), &a("status"), &a("detail")),
        "worktree-exists" => tr::err_worktree_exists(&a("path")),
        "program-not-found" => tr::err_program_not_found(&a("program")),
        "repo-not-found" => tr::err_repo_not_found(&a("id")),
        "repo-in-use" => tr::err_repo_in_use(n("count"), &a("name")),
        "repo-not-attached" => tr::err_repo_not_attached(&a("name")),
        "not-a-workspace-session" => tr::err_not_a_workspace_session(&a("id")),
        "family-needs-workspace" => tr::err_family_needs_workspace(&a("id")),
        "family-too-deep" => tr::err_family_too_deep(),
        "repo-not-in-parent" => tr::err_repo_not_in_parent(&a("name")),
        "widget" => tr::err_widget(&a("detail")),
        "widget-changed" => tr::err_widget_changed(&a("widget")),
        "widget-not-allowed" => tr::err_widget_not_allowed(&a("tool")),
        "preview" => tr::err_preview(&a("detail")),
        "approval-not-found" => tr::err_approval_not_found(),
        "session-has-children" => tr::err_session_has_children(n("count")),
        "repo-used-by-children" => tr::err_repo_used_by_children(n("count"), &a("name")),
        "shared-child-repos" => tr::err_shared_child_repos(),
        "not-a-git-folder" => tr::err_not_a_git_folder(&a("path")),
        "folder-not-found" => tr::err_folder_not_found(&a("path")),
        "corrupt-git-dir" => tr::err_corrupt_git_dir(&a("path")),
        "git-timeout" => tr::err_git_timeout(&a("command"), &a("secs")),
        "base-refresh-failed" => tr::err_base_refresh_failed(&a("detail")),
        "base-no-remote" => tr::err_base_no_remote(),
        "base-not-branch" => tr::err_base_not_branch(),
        "base-fetch-failed" => tr::err_base_fetch_failed(&a("detail")),
        "base-rule-no-match" => tr::err_base_rule_no_match(),
        "invalid-base-rule" => match a("field").as_str() {
            "remote" => tr::err_invalid_base_rule_remote(),
            "match" => tr::err_invalid_base_rule_match(),
            _ => tr::err_invalid_base_rule_pattern(),
        },
        "remote-not-found" => tr::err_remote_not_found(&a("remote")),
        "remote-list-failed" => tr::err_remote_list_failed(&a("detail")),
        "invalid-base-branch" => tr::err_invalid_base_branch(&a("branch")),
        "origin-branch-missing" => tr::err_origin_branch_missing(&a("branch")),
        "origin-branch-unknown" => tr::err_origin_branch_unknown(&a("branch")),
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
        // phase 2: files, watch, search, git, definitions
        "path-invalid" => tr::err_path_invalid(&a("path")),
        "fs-not-found" => tr::err_fs_not_found(&a("path")),
        "fs-exists" => tr::err_fs_exists(&a("path")),
        "fs-permission" => tr::err_fs_permission(&a("path")),
        "fs-dir-not-empty" => tr::err_fs_dir_not_empty(&a("path")),
        "fs-not-a-dir" => tr::err_fs_not_a_dir(&a("path")),
        "fs-is-a-dir" => tr::err_fs_is_a_dir(&a("path")),
        "fs-io" => tr::err_fs_io(&a("path"), &a("detail")),
        "fs-conflict" => tr::err_fs_conflict(&a("path")),
        "fs-bad-encoding" => tr::err_fs_bad_encoding(&a("encoding")),
        "fs-unencodable" => tr::err_fs_unencodable(&a("path"), &a("encoding")),
        "watch-failed" => tr::err_watch_failed(&a("detail")),
        "search-empty" => tr::err_search_empty(),
        "search-bad-regex" => tr::err_search_bad_regex(&a("detail")),
        "search-bad-glob" => tr::err_search_bad_glob(&a("glob"), &a("detail")),
        "git-bad-rev" => tr::err_git_bad_rev(&a("rev")),
        "git-no-base" => tr::err_git_no_base(&a("base")),
        // local git actions (Changes / History; docs/changes-actions.md)
        "git-bad-branch" => tr::err_git_bad_branch(&a("name")),
        "git-branch-exists" => tr::err_git_branch_exists(&a("name")),
        "git-branch-checked-out" => tr::err_git_branch_checked_out(&a("branch"), &a("path")),
        "git-unknown-remote" => tr::err_git_unknown_remote(&a("remote")),
        "git-detached" => tr::err_git_detached(),
        "git-no-upstream" => tr::err_git_no_upstream(),
        "git-push-changed" => tr::err_git_push_changed(),
        "git-busy" => tr::err_git_busy(),
        "git-in-progress" => tr::err_git_in_progress(),
        "git-dirty" => tr::err_git_dirty(),
        "git-merge-commit" => tr::err_git_merge_commit(),
        "git-stash-empty" => tr::err_git_stash_empty(),
        "git-stash-gone" => tr::err_git_stash_gone(),
        "git-refresh-moved" => tr::err_git_refresh_moved(),
        "git-refresh-conflict" => {
            tr::err_git_refresh_conflict(&a("sha"), &a("subject"), &a("files"), n("more"))
        }
        "git-merge-tree" => tr::err_git_merge_tree(&a("detail")),
        // `def.lookup` "nothing found" reasons (`DefResult::None`)
        "def-invalid-position" => tr::err_def_invalid_position(),
        "def-unsupported-file" => tr::err_def_unsupported_file(&a("path")),
        "def-no-name" => tr::err_def_no_name(),
        "def-external-module" => tr::err_def_external_module(&a("spec")),
        "def-module-not-found" => tr::err_def_module_not_found(&a("spec")),
        "def-external-name" => tr::err_def_external_name(&a("spec")),
        "def-not-found" => tr::err_def_not_found(&a("name")),
        // client side
        "disconnected" => tr::err_disconnected(),
        "timeout" => tr::err_timeout(),
        "connect-failed" => tr::err_connect_failed(&a("detail")),
        // PR backend and prompt delivery (gh-*, github-*, pr-*, prompt-*; `footer.rs`)
        id if ["gh-", "github-", "pr-", "prompt-"].iter().any(|p| id.starts_with(p)) => {
            crate::footer::proto_error_text(e).unwrap_or_else(|| tr::err_unknown(id, ""))
        }
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
        assert_eq!(
            proto_error(&ProtoError::new("fs-conflict").arg("path", "a.ts")),
            "읽은 뒤 디스크에서 파일이 바뀌었어요: a.ts"
        );
        assert_eq!(
            proto_error(&ProtoError::new("def-not-found").arg("name", "foo")),
            "정의를 찾지 못했어요: foo"
        );
        assert_eq!(
            proto_error(
                &ProtoError::new("account-in-use")
                    .arg("name", "Work")
                    .arg("count", "2")
            ),
            "“Work” 계정을 쓰는 세션이 2개 있어요. 세션을 먼저 삭제하세요."
        );
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
        for id in [
            "path-invalid",
            "fs-not-found",
            "fs-exists",
            "fs-permission",
            "fs-dir-not-empty",
            "fs-not-a-dir",
            "fs-is-a-dir",
            "fs-io",
            "fs-conflict",
            "fs-bad-encoding",
            "fs-unencodable",
            "watch-failed",
            "search-empty",
            "search-bad-regex",
            "search-bad-glob",
            "git-bad-rev",
            "git-no-base",
            "bad-request",
            "def-invalid-position",
            "def-unsupported-file",
            "def-no-name",
            "def-external-module",
            "def-module-not-found",
            "def-external-name",
            "def-not-found",
            "account-not-found",
            "account-in-use",
            "configuration-not-found",
            "configuration-in-use",
            "last-configuration",
            "skill-not-found",
            "skill-exists",
            "skill-invalid",
            "attach-too-large",
            "attach-not-found",
            "provider-not-found",
            "provider-no-login",
            "invalid-mcp-name",
            "git-bad-branch",
            "git-branch-exists",
            "git-branch-checked-out",
            "git-unknown-remote",
            "git-detached",
            "git-no-upstream",
            "git-push-changed",
            "git-busy",
            "git-in-progress",
            "git-dirty",
            "git-merge-commit",
            "git-stash-empty",
            "git-stash-gone",
            "git-refresh-moved",
            "git-refresh-conflict",
            "git-merge-tree",
        ] {
            let e = ProtoError::new(id);
            assert!(
                !proto_error(&e).starts_with("Server error"),
                "{id} has no message"
            );
        }
        assert_eq!(
            proto_error(
                &ProtoError::new("configuration-in-use")
                    .arg("name", "Web")
                    .arg("count", "1")
            ),
            "1 session uses profile “Web”; delete them first."
        );
    }
}
