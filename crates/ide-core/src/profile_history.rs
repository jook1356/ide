//! History of a profile (configuration): its folder `<data>/configurations/<slug>/` is a git
//! repository of its own and every change is one commit (`docs/profile-editing.md`). Who made
//! a change is kept in trailers (`Ide-Source`, `Ide-Session`, `Ide-Session-Title`), the
//! summary line is the change's description (an agent's reason, or a fixed English text).
//!
//! History is best effort: without git the profile still works, it only has no history
//! (callers log the error).

use std::path::Path;

use crate::error::Result;
use crate::repo::git;

/// Who made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    /// The profiles page (or another client call).
    Ui,
    /// A session's agent through the bridge (`ide_profile_edit`).
    Agent,
    /// "Restore this version" of the history.
    Restore,
    /// First commit of a profile that had no history (also migrated ones).
    Import,
}

impl ChangeSource {
    /// Stable id (`ui`, `agent`, `restore`, `import`).
    pub fn id(self) -> &'static str {
        match self {
            ChangeSource::Ui => "ui",
            ChangeSource::Agent => "agent",
            ChangeSource::Restore => "restore",
            ChangeSource::Import => "import",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "agent" => ChangeSource::Agent,
            "restore" => ChangeSource::Restore,
            "import" => ChangeSource::Import,
            _ => ChangeSource::Ui,
        }
    }
}

/// What a commit records about a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeNote {
    pub source: ChangeSource,
    /// One line (longer text is cut).
    pub summary: String,
    /// The session whose agent made it.
    pub session: Option<String>,
    /// That session's title then.
    pub session_title: Option<String>,
}

impl ChangeNote {
    /// A change of `source` described by `summary`.
    pub fn new(source: ChangeSource, summary: impl Into<String>) -> Self {
        ChangeNote {
            source,
            summary: summary.into(),
            session: None,
            session_title: None,
        }
    }
}

/// One change of a profile's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Commit id (full hash).
    pub id: String,
    /// Unix seconds.
    pub time: u64,
    pub source: ChangeSource,
    pub summary: String,
    pub session: Option<String>,
    pub session_title: Option<String>,
    /// Changed files, relative to the profile folder (`/`-separated).
    pub files: Vec<String>,
}

const AUTHOR: [&str; 4] = ["-c", "user.name=ide", "-c", "user.email=ide@localhost"];

/// Make `dir` a repository of its own (no-op if it is one). Never runs git in a folder without
/// its own `.git`: git would act on an enclosing repository.
fn ensure_repo(dir: &Path) -> Result<()> {
    if dir.join(".git").exists() {
        return Ok(());
    }
    git(dir, ["init", "-q"])?;
    // Files are stored as written (no line ending conversion).
    git(dir, ["config", "core.autocrlf", "false"])?;
    git(dir, ["config", "core.safecrlf", "false"])?;
    Ok(())
}

/// One line of at most 200 characters.
fn one_line(s: &str) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = line.chars().take(200).collect();
    if line.chars().count() > 200 {
        out.push('…');
    }
    out
}

/// Commit everything in `dir` (creates the repository first). `None` when nothing changed.
pub fn commit(dir: &Path, note: &ChangeNote) -> Result<Option<String>> {
    ensure_repo(dir)?;
    git(dir, ["add", "-A"])?;
    let staged = crate::repo::git_raw(dir, &["diff", "--cached", "--quiet"], &[])?;
    if staged.code == Some(0) {
        return Ok(None);
    }
    let summary = match one_line(&note.summary) {
        s if s.is_empty() => "Change profile".to_owned(),
        s => s,
    };
    let mut msg = format!("{summary}\n\nIde-Source: {}\n", note.source.id());
    if let Some(s) = &note.session {
        msg.push_str(&format!("Ide-Session: {}\n", one_line(s)));
    }
    if let Some(t) = note.session_title.as_deref().map(one_line).filter(|t| !t.is_empty()) {
        msg.push_str(&format!("Ide-Session-Title: {t}\n"));
    }
    let mut args: Vec<&str> = AUTHOR.to_vec();
    // Internal store: the user's hooks and signing settings are not for these commits.
    args.extend(["commit", "-q", "--no-verify", "--no-gpg-sign", "-m", &msg]);
    git(dir, &args)?;
    Ok(Some(git(dir, ["rev-parse", "HEAD"])?))
}

/// The last `limit` changes, newest first (empty without a history).
pub fn log(dir: &Path, limit: usize) -> Result<Vec<HistoryEntry>> {
    if !dir.join(".git").exists() {
        return Ok(Vec::new());
    }
    let has_head = crate::repo::git_raw(dir, &["rev-parse", "--verify", "-q", "HEAD"], &[])?;
    if has_head.code != Some(0) {
        return Ok(Vec::new());
    }
    let n = format!("-n{}", limit.max(1));
    let out = git(
        dir,
        [
            "log",
            &n,
            "--name-only",
            "--no-renames",
            "--format=%x1e%H%x1f%ct%x1f%s%x1f%(trailers:only=true,unfold=true)%x1d",
        ],
    )?;
    Ok(parse_log(&out))
}

fn parse_log(out: &str) -> Vec<HistoryEntry> {
    out.split('\u{1e}')
        .filter(|c| !c.trim().is_empty())
        .filter_map(|chunk| {
            let (head, names) = chunk.split_once('\u{1d}')?;
            let mut f = head.split('\u{1f}');
            let id = f.next()?.trim().to_owned();
            let time = f.next()?.trim().parse().unwrap_or(0);
            let summary = f.next().unwrap_or("").to_owned();
            let (mut source, mut session, mut session_title) = (ChangeSource::Ui, None, None);
            for line in f.next().unwrap_or("").lines() {
                let Some((k, v)) = line.split_once(':') else { continue };
                let v = v.trim().to_owned();
                match k.trim() {
                    "Ide-Source" => source = ChangeSource::parse(&v),
                    "Ide-Session" => session = Some(v),
                    "Ide-Session-Title" => session_title = Some(v),
                    _ => {}
                }
            }
            let files = names
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            Some(HistoryEntry {
                id,
                time,
                source,
                summary,
                session,
                session_title,
                files,
            })
        })
        .collect()
}

/// Make the folder's files what they were after change `id` (files added since are removed)
/// and commit that as a new change. Fails for an unknown id.
pub fn restore(dir: &Path, id: &str, note: &ChangeNote) -> Result<Option<String>> {
    ensure_repo(dir)?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(crate::Error::Git {
            args: format!("restore {id}"),
            status: "invalid id".into(),
            stderr: format!("not a change id: {id}"),
        });
    }
    let commit_id = format!("{id}^{{commit}}");
    git(dir, ["rev-parse", "--verify", "-q", &commit_id])?;
    // Index and work tree to that tree; tracked files missing there are removed.
    git(dir, ["read-tree", "-u", "--reset", id])?;
    commit(dir, note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    #[test]
    fn commit_log_restore() {
        let t = TempDir::new("profile-history");
        let dir = t.0.join("p");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "one").unwrap();
        let first = commit(&dir, &ChangeNote::new(ChangeSource::Import, "Import")).unwrap();
        assert!(first.is_some());
        // Nothing changed: no commit.
        assert_eq!(commit(&dir, &ChangeNote::new(ChangeSource::Ui, "x")).unwrap(), None);

        std::fs::write(dir.join("a.md"), "two").unwrap();
        std::fs::create_dir_all(dir.join("skills/s")).unwrap();
        std::fs::write(dir.join("skills/s/SKILL.md"), "s").unwrap();
        let note = ChangeNote {
            source: ChangeSource::Agent,
            summary: "Learned:\nuse pnpm".into(),
            session: Some("abc".into()),
            session_title: Some("Fix: build".into()),
        };
        commit(&dir, &note).unwrap().unwrap();

        let log = log(&dir, 10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].summary, "Learned: use pnpm");
        assert_eq!(log[0].source, ChangeSource::Agent);
        assert_eq!(log[0].session.as_deref(), Some("abc"));
        assert_eq!(log[0].session_title.as_deref(), Some("Fix: build"));
        assert_eq!(log[0].files, ["a.md", "skills/s/SKILL.md"]);
        assert_eq!(log[1].source, ChangeSource::Import);
        assert_eq!(log[1].files, ["a.md"]);

        restore(&dir, &log[1].id, &ChangeNote::new(ChangeSource::Restore, "Restore"))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.md")).unwrap(), "one");
        assert!(!dir.join("skills/s/SKILL.md").exists());
        assert_eq!(super::log(&dir, 10).unwrap()[0].source, ChangeSource::Restore);
        assert!(restore(&dir, "nothex", &ChangeNote::new(ChangeSource::Restore, "")).is_err());
        assert!(super::log(&t.0.join("none"), 5).unwrap().is_empty());
    }
}
