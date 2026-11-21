//! Base branches of new worktrees, mirroring Hermes Desk (`src/main/workspace.ts`
//! `createWorktree` / `mainBranch` / `configuredBase` / `refreshedBase` / `fetchRemoteBranch`,
//! `src/shared/baseBranchRule.ts`, `src/shared/baseBranch.ts`):
//!
//! - a repository's base is automatic ([`main_branch`]), a fixed local or remote-tracking ref
//!   ([`configured_base`]), or a rule ([`BaseBranchRule`]: highest version among a remote's
//!   branches matching a glob and an optional regex);
//! - a new session may pick another of origin's branches per repository;
//! - 기준 브랜치 최신화 (`refresh`, on by default in the new-session dialog) fetches the base
//!   from its remote first, without moving the original repository's local branch. A failed
//!   fetch fails that repository's worktree (Desk: "가져오기 실패 시 해당 레포는 생성하지
//!   않습니다"); without refreshing the last-fetched state is used.

use std::cmp::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::registry::{BaseBranch, RegisteredRepo};
use crate::repo::{Repo, git_timeout};

/// Desk `refreshedBase`: 60 s.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(60);
/// Desk `fetchRemoteBranch`: 120 s.
const FETCH_BRANCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Desk `remoteHeads` (`git ls-remote --heads`): 60 s.
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(60);

/// Rule-based base branch (Desk `BaseBranchRule`): instead of pinning one ref
/// (release/2026.19.0), pick the highest version among the remote's branches matching a glob and
/// an optional regex (release/2026.20.0 is chosen automatically once it is pushed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseBranchRule {
    /// Remote whose branches are listed with `git ls-remote --heads`.
    pub remote: String,
    /// Candidate branch glob: `*` matches anything (including `/`), `?` one character.
    pub pattern: String,
    /// Optional regex every candidate must also match (empty = all candidates).
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub matches: Option<String>,
}

/// What a rule picks (Desk `BaseBranchRuleResult`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BaseRuleResult {
    /// Branches matching the glob and the regex, highest version first.
    pub matched: Vec<String>,
    /// Glob candidates rejected by the regex.
    pub excluded: Vec<String>,
    /// The branch new worktrees start from, `None` when nothing matches.
    pub pick: Option<String>,
}

/// The glob as a regex (`^...$`); applied by us, so it never reaches git as an option.
pub fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from("^");
    for c in glob.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c if ".+^${}()|[]\\".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

/// Validation of a rule (Desk `ruleError`): `Err(InvalidBaseRule(field))`.
pub fn clean_rule(rule: &BaseBranchRule) -> Result<BaseBranchRule> {
    let remote_ok = rule
        .remote
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && rule
            .remote
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !remote_ok {
        return Err(Error::InvalidBaseRule("remote"));
    }
    if rule.pattern.trim().is_empty() {
        return Err(Error::InvalidBaseRule("pattern"));
    }
    let matches = rule.matches.clone().filter(|m| !m.is_empty());
    if let Some(m) = &matches
        && fancy_regex::Regex::new(m).is_err()
    {
        return Err(Error::InvalidBaseRule("match"));
    }
    Ok(BaseBranchRule {
        remote: rule.remote.clone(),
        pattern: rule.pattern.trim().to_owned(),
        matches,
    })
}

fn numbers(s: &str) -> Vec<u64> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().unwrap_or(u64::MAX))
        .collect()
}

/// Desk `compareVersions`: by the numeric components (2026.9.0 < 2026.19.0 < 2026.20.0); when
/// one list is a prefix of the other the shorter wins (release/2026.20.0 >
/// release/2026.20.0-rc1), then plain text.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (x, y) = (numbers(a), numbers(b));
    for (p, q) in x.iter().zip(&y) {
        if p != q {
            return p.cmp(q);
        }
    }
    if x.len() != y.len() {
        return y.len().cmp(&x.len());
    }
    a.cmp(b)
}

/// Apply a validated rule to a remote's branch names (Desk `applyBaseBranchRule`).
pub fn apply_rule(rule: &BaseBranchRule, branches: &[String]) -> BaseRuleResult {
    let Ok(glob) = fancy_regex::Regex::new(&glob_to_regex(rule.pattern.trim())) else {
        return BaseRuleResult::default();
    };
    let re = rule
        .matches
        .as_deref()
        .filter(|m| !m.is_empty())
        .and_then(|m| fancy_regex::Regex::new(m).ok());
    let test = |r: &fancy_regex::Regex, s: &str| r.is_match(s).unwrap_or(false);
    let candidates: Vec<&String> = branches.iter().filter(|b| test(&glob, b)).collect();
    let mut matched: Vec<String> = candidates
        .iter()
        .filter(|b| re.as_ref().is_none_or(|r| test(r, b)))
        .map(|b| (*b).clone())
        .collect();
    matched.sort_by(|a, b| compare_versions(b, a));
    let excluded = candidates
        .into_iter()
        .filter(|b| !matched.contains(b))
        .cloned()
        .collect();
    BaseRuleResult {
        pick: matched.first().cloned(),
        matched,
        excluded,
    }
}

/// `refs/heads/x`, `refs/remotes/origin/x`, `origin/x` -> `x` (Desk `shortBranch`).
pub fn short_branch(r: &str) -> String {
    if let Some(n) = r.strip_prefix("refs/heads/") {
        return n.to_owned();
    }
    if let Some(rest) = r.strip_prefix("refs/remotes/") {
        return rest.split_once('/').map_or(rest, |(_, n)| n).to_owned();
    }
    r.strip_prefix("origin/").unwrap_or(r).to_owned()
}

/// The repository's default base for new sessions as a branch name (Desk `repoDefaultBase`):
/// the rule's last pick, else the fixed setting; `None` = automatic.
pub fn repo_default_base(r: &RegisteredRepo) -> Option<String> {
    let v = if r.base_branch_rule.is_some() {
        r.rule_resolved.as_deref().or(r.base_branch.as_deref())
    } else {
        r.base_branch.as_deref()
    };
    v.filter(|v| !v.is_empty()).map(short_branch)
}

fn remotes(repo: &Repo) -> Result<Vec<String>> {
    Ok(repo
        .git(["remote"])?
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect())
}

fn ref_format_ok(repo: &Repo, r: &str) -> bool {
    repo.git(["check-ref-format", r]).is_ok()
}

fn reason(e: &Error) -> String {
    match e {
        Error::Git { stderr, .. } => stderr.clone(),
        other => other.to_string(),
    }
}

/// Branch names on a remote right now (`git ls-remote --heads`), not the local tracking refs.
pub fn remote_heads(repo: &Repo, remote: &str) -> Result<Vec<String>> {
    if !remotes(repo)?.iter().any(|r| r == remote) {
        return Err(Error::RemoteNotFound(remote.to_owned()));
    }
    let out = git_timeout(
        repo.root(),
        &["ls-remote", "--heads", "--", remote],
        LS_REMOTE_TIMEOUT,
    )
    .map_err(|e| match e {
        e @ Error::GitTimeout { .. } => e,
        e => Error::RemoteListFailed(reason(&e)),
    })?;
    Ok(out
        .lines()
        .filter_map(|l| l.trim().split('\t').nth(1))
        .filter_map(|r| r.strip_prefix("refs/heads/"))
        .map(str::to_owned)
        .collect())
}

/// What `rule` picks right now from the remote's live branch list, plus that list.
pub fn resolve_rule(repo: &Repo, rule: &BaseBranchRule) -> Result<(BaseRuleResult, Vec<String>)> {
    let rule = clean_rule(rule)?;
    let branches = remote_heads(repo, &rule.remote)?;
    Ok((apply_rule(&rule, &branches), branches))
}

/// Fetch one remote branch into its tracking ref and return its commit (Desk
/// `fetchRemoteBranch`).
pub fn fetch_remote_branch(repo: &Repo, remote: &str, branch: &str, tracking: &str) -> Result<String> {
    let source = format!("refs/heads/{branch}");
    for r in [source.as_str(), tracking] {
        if !ref_format_ok(repo, r) {
            return Err(Error::InvalidBaseBranch(branch.to_owned()));
        }
    }
    git_timeout(
        repo.root(),
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--",
            remote,
            &format!("+{source}:{tracking}"),
        ],
        FETCH_BRANCH_TIMEOUT,
    )
    .map_err(|e| Error::BaseFetchFailed(reason(&e)))?;
    repo.git(["rev-parse", "--verify", &format!("{tracking}^{{commit}}")])
}

/// 기준 브랜치 최신화 of an automatic / fixed base (Desk `refreshedBase`): fetch the selected
/// branch from its remote (a local branch: its upstream, else `origin`) without moving the
/// original repository's local branch; returns the fetched commit.
pub fn refreshed_base(repo: &Repo, start: &str) -> Result<String> {
    let full = repo
        .git(["rev-parse", "--symbolic-full-name", start])
        .unwrap_or_default();
    let (remote, source, tracking) = if let Some(rest) = full.strip_prefix("refs/remotes/") {
        let (remote, name) = rest.split_once('/').unwrap_or((rest, ""));
        (remote.to_owned(), format!("refs/heads/{name}"), full.clone())
    } else if let Some(name) = full.strip_prefix("refs/heads/") {
        let up = repo.git([
            "for-each-ref",
            "--format=%(upstream:remotename)%09%(upstream:remoteref)%09%(upstream)",
            &full,
        ])?;
        let parts: Vec<&str> = up.trim().split('\t').collect();
        let get = |i: usize| parts.get(i).copied().filter(|s| !s.is_empty());
        let remote = get(0).unwrap_or("origin").to_owned();
        let source = get(1).map_or_else(|| full.clone(), str::to_owned);
        let tracking = get(2).map_or_else(|| format!("refs/remotes/{remote}/{name}"), str::to_owned);
        (remote, source, tracking)
    } else {
        return Err(Error::BaseNotBranch);
    };
    if remote == "." || !remotes(repo)?.contains(&remote) {
        return Err(Error::BaseNoRemote);
    }
    git_timeout(
        repo.root(),
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--",
            &remote,
            &format!("+{source}:{tracking}"),
        ],
        REFRESH_TIMEOUT,
    )
    .map_err(|e| Error::BaseRefreshFailed(reason(&e)))?;
    repo.git(["rev-parse", "--verify", &format!("{tracking}^{{commit}}")])
}

/// Desk `configuredBase`: only locally present branch refs (`refs/heads/x`, `refs/remotes/o/x`,
/// legacy short names tried as both), never options, tags, hashes or `.../HEAD`.
pub fn configured_base(repo: &Repo, branch: &str) -> Result<BaseBranch> {
    if branch.is_empty() || branch.starts_with('-') || branch == "HEAD" {
        return Err(Error::InvalidBaseBranch(branch.to_owned()));
    }
    let refs = if branch.starts_with("refs/") {
        vec![branch.to_owned()]
    } else {
        vec![format!("refs/heads/{branch}"), format!("refs/remotes/{branch}")]
    };
    for r in refs {
        if !(r.starts_with("refs/heads/") || r.starts_with("refs/remotes/"))
            || (r.starts_with("refs/remotes/") && r.ends_with("/HEAD"))
            || !ref_format_ok(repo, &r)
            || repo.git(["show-ref", "--verify", "--quiet", &r]).is_err()
        {
            continue;
        }
        repo.git(["rev-parse", "--verify", &format!("{r}^{{commit}}")])?;
        return Ok(BaseBranch {
            name: short_branch(&r),
            start: Some(r),
        });
    }
    Err(Error::InvalidBaseBranch(branch.to_owned()))
}

/// origin's branches from the last fetch (Desk `originBranches`, base pickers): sorted, no HEAD.
pub fn origin_branches(repo: &Repo) -> Result<Vec<String>> {
    let out = repo.git(["for-each-ref", "--format=%(refname)", "refs/remotes/origin"])?;
    let mut v: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|r| r.starts_with("refs/remotes/origin/") && !r.ends_with("/HEAD"))
        .map(|r| r["refs/remotes/origin/".len()..].to_owned())
        .collect();
    v.sort();
    Ok(v)
}

/// Local and remote-tracking branch refs (full names, no `.../HEAD`) and the current branch
/// (Desk `branches`, the fixed-base picker of the 레포 page).
pub fn branch_refs(repo: &Repo) -> Result<(Option<String>, Vec<String>)> {
    let out = repo.git(["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"])?;
    let refs = out
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty() && !(r.starts_with("refs/remotes/") && r.ends_with("/HEAD")))
        .map(str::to_owned)
        .collect();
    Ok((repo.current_branch().unwrap_or(None), refs))
}

/// How one worktree's start point was chosen ([`plan_base`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasePlan {
    /// Base branch name recorded for the worktree (and as the session's base).
    pub name: String,
    /// Commit / revision to branch from; `None` = unborn (orphan branch).
    pub start: Option<String>,
    /// A rule's new pick (`refs/remotes/<remote>/<branch>`), to store as `rule_resolved`.
    pub rule_resolved: Option<String>,
    /// A legacy short fixed base resolved to its full ref, to store as `base_branch`.
    pub fixed_upgrade: Option<String>,
}

/// Desk `createWorktree`'s base choice, in order: a base `picked` for this session (origin's
/// branch: fetched first when `refresh`, else as of the last fetch); the repository's rule
/// (re-resolved and fetched every time; no match or an unreachable remote fails); else the
/// fixed or automatic base (fetched first when `refresh`; the local branch is never moved).
pub fn plan_base(
    repo: &Repo,
    r: &RegisteredRepo,
    refresh: bool,
    picked: Option<&str>,
) -> Result<BasePlan> {
    if let Some(picked) = picked.filter(|p| !p.trim().is_empty()) {
        let pick = short_branch(picked.trim());
        let tracking = format!("refs/remotes/origin/{pick}");
        let commit = if refresh {
            fetch_remote_branch(repo, "origin", &pick, &tracking)?
        } else {
            repo.git([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{tracking}^{{commit}}"),
            ])
            .map_err(|_| Error::OriginBranchMissing(pick.clone()))?
        };
        return Ok(BasePlan {
            name: pick,
            start: Some(commit),
            rule_resolved: None,
            fixed_upgrade: None,
        });
    }
    if let Some(rule) = &r.base_branch_rule {
        let rule = clean_rule(rule)?;
        let (result, _) = resolve_rule(repo, &rule)?;
        let pick = result.pick.ok_or(Error::BaseRuleNoMatch)?;
        let tracking = format!("refs/remotes/{}/{pick}", rule.remote);
        let commit = fetch_remote_branch(repo, &rule.remote, &pick, &tracking)?;
        return Ok(BasePlan {
            name: pick,
            start: Some(commit),
            rule_resolved: Some(tracking),
            fixed_upgrade: None,
        });
    }
    let (base, fixed_upgrade) = match r.base_branch.as_deref() {
        Some(b) => {
            let base = configured_base(repo, b)?;
            let up = base.start.clone().filter(|s| s != b);
            (base, up)
        }
        None => (main_branch(repo)?, None),
    };
    let start = match (&base.start, refresh) {
        (None, _) => None,
        (Some(s), true) => Some(refreshed_base(repo, s)?),
        (Some(s), false) => Some(s.clone()),
    };
    Ok(BasePlan {
        name: base.name,
        start,
        rule_resolved: None,
        fixed_upgrade,
    })
}

fn has_commit(repo: &Repo, rev: &str) -> bool {
    repo.git([
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &format!("{rev}^{{commit}}"),
    ])
    .is_ok()
}

/// Desk `mainBranch()`: `main`, else `master`, else origin's default branch; start from
/// `origin/<name>` when the local branch is missing or only behind it. Else the current branch
/// (unborn: no start).
pub fn main_branch(repo: &Repo) -> Result<BaseBranch> {
    let origin_default = repo
        .git(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .unwrap_or_default();
    let origin_default = origin_default.trim().trim_start_matches("origin/").to_owned();
    for name in ["main", "master", origin_default.as_str()] {
        if name.is_empty() {
            continue;
        }
        let local = has_commit(repo, &format!("refs/heads/{name}"));
        let remote = has_commit(repo, &format!("refs/remotes/origin/{name}"));
        let start = match (local, remote) {
            (false, false) => continue,
            (true, false) => name.to_owned(),
            (false, true) => format!("origin/{name}"),
            (true, true) => {
                let behind = repo
                    .git(["merge-base", "--is-ancestor", name, &format!("origin/{name}")])
                    .is_ok();
                if behind {
                    format!("origin/{name}")
                } else {
                    name.to_owned()
                }
            }
        };
        return Ok(BaseBranch {
            name: name.to_owned(),
            start: Some(start),
        });
    }
    let current = repo.current_branch()?.unwrap_or_default();
    if !current.is_empty() && !has_commit(repo, "HEAD") {
        return Ok(BaseBranch {
            name: current,
            start: None,
        });
    }
    let name = if current.is_empty() {
        "HEAD".to_owned()
    } else {
        current
    };
    Ok(BaseBranch {
        start: Some(name.clone()),
        name,
    })
}

/// Desk `initRepository`: make a plain folder a repository: `git init --initial-branch=main`,
/// `git add --all` (.gitignore respected) and one commit "Initial commit" (`--allow-empty
/// --no-verify`; a machine without a git identity commits as "ide"). On failure the new `.git`
/// is removed again.
pub fn init_repository(dir: &std::path::Path) -> Result<()> {
    let git_dir = dir.join(".git");
    if git_dir.exists() {
        return Err(Error::CorruptGitDir(git_dir));
    }
    let run = || -> Result<()> {
        crate::repo::git(dir, ["init", "--initial-branch=main"])?;
        crate::repo::git(dir, ["add", "--all"])?;
        let identity = crate::repo::git(dir, ["config", "user.email"])
            .is_ok_and(|v| !v.trim().is_empty());
        let mut args: Vec<&str> = Vec::new();
        if !identity {
            args.extend(["-c", "user.name=ide", "-c", "user.email=ide@localhost"]);
        }
        args.extend(["commit", "--allow-empty", "--no-verify", "-m", "Initial commit"]);
        crate::repo::git(dir, args)?;
        Ok(())
    };
    run().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&git_dir);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::repo::test_repo;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn rules() {
        assert_eq!(glob_to_regex("release/*.?"), "^release/.*\\..$");
        assert_eq!(compare_versions("r/2026.9.0", "r/2026.19.0"), Ordering::Less);
        assert_eq!(compare_versions("r/2026.20.0", "r/2026.20.0-rc1"), Ordering::Greater);
        let rule = BaseBranchRule {
            remote: "origin".into(),
            pattern: "release/*".into(),
            matches: None,
        };
        let branches = s(&[
            "main",
            "release/2026.9.0",
            "release/2026.19.0",
            "release/2026.20.0-rc1",
            "release/2026.20.0",
        ]);
        let r = apply_rule(&rule, &branches);
        assert_eq!(r.pick.as_deref(), Some("release/2026.20.0"));
        assert_eq!(r.matched.len(), 4);
        assert!(r.excluded.is_empty());
        let rule = BaseBranchRule {
            matches: Some(r"^release/\d+\.\d+\.\d+$".into()),
            ..rule
        };
        let r = apply_rule(&rule, &branches);
        assert_eq!(r.excluded, s(&["release/2026.20.0-rc1"]));
        assert_eq!(r.matched[2], "release/2026.9.0");
        for (bad, field) in [
            (("-x", "a"), "remote"),
            (("origin", " "), "pattern"),
        ] {
            let rule = BaseBranchRule {
                remote: bad.0.into(),
                pattern: bad.1.into(),
                matches: None,
            };
            assert!(matches!(clean_rule(&rule), Err(Error::InvalidBaseRule(f)) if f == field));
        }
        let bad = BaseBranchRule {
            remote: "origin".into(),
            pattern: "*".into(),
            matches: Some("(".into()),
        };
        assert!(matches!(clean_rule(&bad), Err(Error::InvalidBaseRule("match"))));
        assert_eq!(short_branch("refs/remotes/up/a/b"), "a/b");
        assert_eq!(short_branch("origin/x"), "x");
        assert_eq!(short_branch("refs/heads/y"), "y");
    }

    #[test]
    fn init_plain_folder() {
        let t = TempDir::new("init");
        let d = t.0.join("plain 폴더");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), "a").unwrap();
        std::fs::write(d.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(d.join("ignored.txt"), "x").unwrap();
        init_repository(&d).unwrap();
        let repo = Repo::open(&d).unwrap();
        assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));
        let files = test_repo::run(&d, &["ls-files"]);
        assert!(files.contains("a.txt") && !files.contains("ignored.txt"), "{files}");
        assert_eq!(test_repo::run(&d, &["log", "--format=%s"]), "Initial commit");
        // A .git already there: refused, nothing removed.
        assert!(matches!(init_repository(&d), Err(Error::CorruptGitDir(_))));
        assert!(d.join(".git").is_dir());
    }

    #[test]
    fn configured_and_remote_bases() {
        let t = TempDir::new("cfgbase");
        let origin = t.0.join("origin.git");
        let a = t.0.join("a");
        test_repo::init(&a);
        test_repo::run(&t.0, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
        test_repo::run(&a, &["remote", "add", "origin", &origin.display().to_string()]);
        test_repo::run(&a, &["branch", "release/2026.1.0"]);
        test_repo::run(&a, &["branch", "release/2026.2.0"]);
        test_repo::run(&a, &["push", "-q", "origin", "main", "release/2026.1.0", "release/2026.2.0"]);
        test_repo::run(&a, &["fetch", "-q", "origin"]);
        let repo = Repo::open(&a).unwrap();
        let b = configured_base(&repo, "refs/remotes/origin/release/2026.1.0").unwrap();
        assert_eq!(b.name, "release/2026.1.0");
        assert!(configured_base(&repo, "refs/remotes/origin/HEAD").is_err());
        assert!(configured_base(&repo, "-x").is_err());
        assert_eq!(
            origin_branches(&repo).unwrap(),
            s(&["main", "release/2026.1.0", "release/2026.2.0"])
        );
        let (cur, refs) = branch_refs(&repo).unwrap();
        assert_eq!(cur.as_deref(), Some("main"));
        assert!(refs.contains(&"refs/remotes/origin/main".to_owned()));
        let rule = BaseBranchRule {
            remote: "origin".into(),
            pattern: "release/*".into(),
            matches: None,
        };
        let (r, all) = resolve_rule(&repo, &rule).unwrap();
        assert_eq!(r.pick.as_deref(), Some("release/2026.2.0"));
        assert_eq!(all.len(), 3);
        let rule = BaseBranchRule {
            remote: "nope".into(),
            ..rule
        };
        assert!(matches!(resolve_rule(&repo, &rule), Err(Error::RemoteNotFound(_))));
    }
}
