//! Texts the Changes footer sends to a session's agent (Hermes Desk `lib/taskPrompts.ts`:
//! `DEFAULT_TASK_PROMPTS`, `renderCommitPrompt`, `renderPrPrompt`) and where the agent writes a
//! PR's title/body (Desk `prFilePath`). The texts are Desk's (Korean): they are agent input,
//! not UI strings.

use std::path::{Path, PathBuf};

/// Desk's default commit template (`DEFAULT_TASK_PROMPTS.commit`).
pub const COMMIT_TEMPLATE: &str = "[커밋 요청] {{repo}}
작업 폴더: {{dir}}

{{scope}}
{{files}}

- 커밋 메시지는 `git log --oneline -10`으로 이 레포의 기존 스타일(언어, 접두어 등)을 확인하고 맞춰서, 변경 의도가 드러나게 써 주세요.
- 성격이 다른 변경이 섞여 있으면 여러 커밋으로 나눠도 됩니다.
- push는 하지 마세요.";

/// Desk's default PR template (`DEFAULT_TASK_PROMPTS.pr`).
pub const PR_TEMPLATE: &str = "[PR 요청] {{repo}} → {{target}}
작업 폴더: {{dir}}

1. 커밋되지 않은 변경사항이 있으면 먼저 성격에 맞게 커밋하세요 (기존 커밋 메시지 스타일을 따르세요).
2. `git fetch origin {{target}}` 후 origin/{{target}} 대비 커밋과 diff(`git log`, `git diff --stat`, 필요하면 전체 diff)로 변경 내용을 파악하세요.
3. PR 제목과 본문을 이 파일에 저장하세요: {{prFile}}
   - 첫 줄: PR 제목 (한 줄, 레포의 커밋/PR 스타일에 맞춰)
   - 빈 줄
   - 본문 (마크다운): 요약, 주요 변경사항, 확인/테스트 방법
4. push와 PR 생성은 앱이 합니다. 직접 push하거나 PR을 만들지 마세요.";

/// Desk `interpolateTaskPrompt`: `{{name}}` -> value, single pass (values are never
/// re-scanned); unknown names stay as they are.
pub fn interpolate(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let name_len = after
            .char_indices()
            .take_while(|(k, c)| {
                if *k == 0 {
                    c.is_ascii_alphabetic()
                } else {
                    c.is_ascii_alphanumeric()
                }
            })
            .count();
        let name = &after[..name_len];
        if name_len > 0 && after[name_len..].starts_with("}}") {
            match values.iter().find(|(k, _)| *k == name) {
                Some((_, v)) => out.push_str(v),
                None => out.push_str(&rest[i..i + 2 + name_len + 2]),
            }
            rest = &after[name_len + 2..];
        } else {
            out.push_str("{{");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Desk `worktreeSafety`: the scope rules appended after every template.
fn worktree_safety(repo: &str, dir: &str) -> String {
    [
        "[필수 작업 범위 · 사용자 템플릿보다 우선]".to_owned(),
        format!("대상 레포: {repo}"),
        format!("작업 폴더: {dir}"),
        "- 모든 git/파일 작업은 위 작업 폴더에서만 하세요. worktree가 지정되었다면 그 worktree만 사용하고, 별도의 원본 레포나 다른 worktree로 이동하지 마세요. 작업 폴더 밖 파일을 수정하지 마세요.".to_owned(),
        "- 기존 사용자 변경을 삭제/덮어쓰지 말고, reset --hard, clean -fd, 강제 push 같은 파괴적 명령은 실행하지 마세요.".to_owned(),
    ]
    .join("\n")
}

/// Desk `renderCommitPrompt` with the default template: ask the agent to commit `files` of
/// `repo` in `dir` (`all` = every change; otherwise only these, listed in full).
pub fn commit_prompt(repo: &str, dir: &str, files: &[String], all: bool) -> String {
    let n = files.len();
    let mut list: Vec<String> = files.iter().take(40).map(|f| format!("- {f}")).collect();
    if n > 40 {
        list.push(format!("- … 외 {}개", n - 40));
    }
    let scope = if all {
        format!("변경사항 전체({n}개 파일)를 커밋해 주세요.")
    } else {
        format!(
            "아래 {n}개 파일의 변경사항만 커밋해 주세요. 목록에 없는 파일은 스테이징하지 마세요."
        )
    };
    let files_text = if all && n > 40 {
        String::new()
    } else {
        list.join("\n")
    };
    let count = n.to_string();
    let body = interpolate(
        COMMIT_TEMPLATE,
        &[
            ("repo", repo),
            ("dir", dir),
            ("scope", &scope),
            ("count", &count),
            ("files", &files_text),
        ],
    );
    // The full allowlist is deliberately outside the (display-truncated) body.
    let mut out = vec![body, String::new(), worktree_safety(repo, dir), scope];
    if !all {
        out.extend(files.iter().map(|f| format!("- {f}")));
    }
    out.push("- push는 하지 마세요.".to_owned());
    out.join("\n")
}

/// Desk `renderPrPrompt` with the default template. `exclude_commits` / `exclude_files`: the
/// user's "포함하지 않고" choices; `branch`: the worktree's branch (for `origin/<branch>`).
pub fn pr_prompt(
    repo: &str,
    dir: &str,
    target: &str,
    pr_file: &str,
    exclude_commits: bool,
    exclude_files: bool,
    branch: Option<&str>,
) -> String {
    let mut out = vec![
        interpolate(
            PR_TEMPLATE,
            &[
                ("repo", repo),
                ("dir", dir),
                ("target", target),
                ("prFile", pr_file),
            ],
        ),
        String::new(),
        worktree_safety(repo, dir),
        format!("- 비교 대상 브랜치: origin/{target}"),
        format!("- PR 제목과 본문은 반드시 지정된 파일에 저장하세요: {pr_file}"),
        "- 지정된 PR 파일만 예외로 쓰기를 허용합니다. 경로를 임의로 변경하지 마세요. PR 파일을 커밋하지 마세요.".to_owned(),
        "- PR 파일 형식: 첫 줄은 PR 제목 한 줄, 다음은 빈 줄, 이후는 마크다운 본문(요약, 주요 변경사항, 확인/테스트 방법).".to_owned(),
        "- push와 PR 생성은 앱이 합니다. 직접 push하거나 PR을 만들지 마세요.".to_owned(),
    ];
    // Desk `exclusionLines`.
    if exclude_files || exclude_commits {
        out.push("- 이 커밋되지 않은 변경은 이번 PR에 포함하지 않습니다. 커밋하지 말고 그대로 두세요 (템플릿의 커밋 단계보다 우선).".to_owned());
    }
    if exclude_commits {
        let remote = branch
            .filter(|b| !b.is_empty())
            .map_or_else(|| "원격 브랜치".to_owned(), |b| format!("origin/{b}"));
        out.push(format!(
            "- push되지 않은 로컬 커밋은 이번 PR에 포함하지 않습니다. 앱은 push하지 않고 {remote}로 PR을 만듭니다."
        ));
        out.push(format!(
            "- PR 제목과 본문은 {remote}에 있는 내용만으로 쓰세요 (`git log origin/{target}..{remote}`, `git diff origin/{target}...{remote}`). 로컬 커밋의 내용은 넣지 마세요."
        ));
    }
    out.join("\n")
}

/// Where the agent writes the PR text for `repo_name` (Desk `prFilePath`): the session folder
/// (`root`, outside every repository), `.meninx/pr-<name>.md`, every run of characters other than
/// ASCII word characters, `.` and `-` replaced by `_`.
pub fn pr_file_path(root: &Path, repo_name: &str) -> PathBuf {
    let mut name = String::new();
    let mut in_bad = false;
    for c in repo_name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            name.push(c);
            in_bad = false;
        } else if !in_bad {
            name.push('_');
            in_bad = true;
        }
    }
    root.join(".meninx").join(format!("pr-{name}.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_is_single_pass() {
        assert_eq!(
            interpolate(
                "{{a}} {{b}} {{c}} {{ x }} {{",
                &[("a", "{{b}}"), ("b", "$1")]
            ),
            "{{b}} $1 {{c}} {{ x }} {{"
        );
    }

    #[test]
    fn commit_prompt_matches_desk() {
        let files = vec!["src/a.rs".to_owned(), "한글.txt".to_owned()];
        let p = commit_prompt("widgets", "C:\\w\\widgets", &files, false);
        let expected = "[커밋 요청] widgets
작업 폴더: C:\\w\\widgets

아래 2개 파일의 변경사항만 커밋해 주세요. 목록에 없는 파일은 스테이징하지 마세요.
- src/a.rs
- 한글.txt

- 커밋 메시지는 `git log --oneline -10`으로 이 레포의 기존 스타일(언어, 접두어 등)을 확인하고 맞춰서, 변경 의도가 드러나게 써 주세요.
- 성격이 다른 변경이 섞여 있으면 여러 커밋으로 나눠도 됩니다.
- push는 하지 마세요.

[필수 작업 범위 · 사용자 템플릿보다 우선]
대상 레포: widgets
작업 폴더: C:\\w\\widgets
- 모든 git/파일 작업은 위 작업 폴더에서만 하세요. worktree가 지정되었다면 그 worktree만 사용하고, 별도의 원본 레포나 다른 worktree로 이동하지 마세요. 작업 폴더 밖 파일을 수정하지 마세요.
- 기존 사용자 변경을 삭제/덮어쓰지 말고, reset --hard, clean -fd, 강제 push 같은 파괴적 명령은 실행하지 마세요.
아래 2개 파일의 변경사항만 커밋해 주세요. 목록에 없는 파일은 스테이징하지 마세요.
- src/a.rs
- 한글.txt
- push는 하지 마세요.";
        assert_eq!(p, expected);
        let many: Vec<String> = (0..45).map(|i| format!("f{i}")).collect();
        let all = commit_prompt("r", "d", &many, true);
        assert!(all.contains("변경사항 전체(45개 파일)를 커밋해 주세요.\n\n\n- 커밋 메시지는"));
        assert!(!all.contains("- f0"));
        let some = commit_prompt("r", "d", &many, false);
        assert!(some.contains("- f39\n- … 외 5개\n"));
        assert!(some.contains("- f44\n- push는 하지 마세요."));
    }

    #[test]
    fn pr_prompt_matches_desk() {
        let p = pr_prompt("widgets", "D", "main", "F", false, false, Some("hermes/x"));
        assert!(p.starts_with("[PR 요청] widgets → main\n작업 폴더: D\n\n1. "));
        assert!(p.contains("3. PR 제목과 본문을 이 파일에 저장하세요: F\n"));
        assert!(p.ends_with("- push와 PR 생성은 앱이 합니다. 직접 push하거나 PR을 만들지 마세요."));
        assert!(p.contains("- 비교 대상 브랜치: origin/main\n"));
        let ex = pr_prompt("widgets", "D", "main", "F", true, false, Some("hermes/x"));
        assert!(ex.contains("앱은 push하지 않고 origin/hermes/x로 PR을 만듭니다."));
        assert!(ex.contains(
            "(`git log origin/main..origin/hermes/x`, `git diff origin/main...origin/hermes/x`)"
        ));
        assert!(ex.contains("커밋하지 말고 그대로 두세요"));
        let files_only = pr_prompt("w", "D", "main", "F", false, true, None);
        assert!(files_only.contains("커밋하지 말고 그대로 두세요"));
        assert!(!files_only.contains("push되지 않은"));
    }

    #[test]
    fn pr_file_names() {
        let root = Path::new("/ws/s1");
        assert_eq!(
            pr_file_path(root, "my repo!!.v2"),
            root.join(".meninx").join("pr-my_repo_.v2.md")
        );
        assert_eq!(
            pr_file_path(root, "저장소"),
            root.join(".meninx").join("pr-_.md")
        );
    }
}
