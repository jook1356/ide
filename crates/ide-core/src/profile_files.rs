//! The files of a profile (configuration) as agents see and edit them (`ide_profile_get` /
//! `ide_profile_edit`, `docs/profile-editing.md`):
//!
//! ```text
//! instructions.md        the instructions
//! mcp/<name>.json        one MCP server: {"command": "...", "args": [...], "env": [["K", "v"]]}
//! skills/<name>/...      one skill: SKILL.md and the files it refers to
//! ```
//!
//! (`profile.json`, the name and switches, is not offered.) Edits are checked against what is
//! on disk at the moment they are applied (optimistic concurrency): a patch's `old` text must
//! still occur exactly once, a rewrite or delete names the revision it was based on. One
//! failing operation applies nothing. Callers serialize calls (the server holds the
//! configuration store's lock), so check and write are one step.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::configuration::{McpFile, validate_mcp_server};
use crate::skill::{self, SKILL_FILE};

/// `instructions.md`.
pub const INSTRUCTIONS_FILE: &str = "instructions.md";
/// `profile.json` (name, created, default repos, switched-off skills; not editable here).
pub const PROFILE_FILE: &str = "profile.json";
/// `mcp`: one `<name>.json` per MCP server.
pub const MCP_DIR: &str = "mcp";
/// `skills`.
pub const SKILLS_DIR: &str = "skills";
/// Largest file an edit may write.
pub const MAX_FILE: usize = 256 * 1024;

/// Revision of a file's content: 16 hex digits (FNV-1a, 64 bit). Only compares versions of
/// the same file; not a security hash.
pub fn rev(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// A file of a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Relative, `/`-separated.
    pub path: String,
    pub rev: String,
    pub size: u64,
}

/// A changed file: its new revision, `None` = deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub rev: Option<String>,
}

/// One edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// Replace `old` (must occur exactly once) with `new`.
    Patch { path: String, old: String, new: String },
    /// Write the whole file: `rev: None` creates it (it must not exist), `Some` replaces the
    /// version with that revision.
    Write {
        path: String,
        content: String,
        rev: Option<String>,
    },
    /// Delete a file, or a whole skill (`skills/<name>`, `rev` = its `SKILL.md`'s).
    Delete { path: String, rev: String },
}

impl EditOp {
    fn path(&self) -> &str {
        match self {
            EditOp::Patch { path, .. } | EditOp::Write { path, .. } | EditOp::Delete { path, .. } => path,
        }
    }
}

/// Why an edit was refused (nothing was applied). `Display` is English, for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// Not a path of a profile.
    BadPath { path: String, why: String },
    /// The file (or skill) does not exist.
    NotFound { path: String },
    /// Creating a file that exists.
    Exists { path: String, rev: String },
    /// The file changed since that revision.
    Stale {
        path: String,
        expected: String,
        rev: String,
        current: String,
    },
    /// A patch's `old` text is not in the file (any more).
    NoMatch {
        path: String,
        rev: String,
        current: String,
    },
    /// A patch's `old` text occurs more than once.
    Ambiguous { path: String, count: usize },
    /// The result would be invalid (MCP JSON, skill without `SKILL.md`, too large, ...).
    Invalid { path: String, why: String },
    /// Reading or writing failed.
    Io { path: String, detail: String },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::BadPath { path, why } => write!(f, "`{path}`: {why}"),
            EditError::NotFound { path } => write!(f, "`{path}` does not exist"),
            EditError::Exists { path, rev } => write!(
                f,
                "`{path}` already exists (rev {rev}): read it and patch it, or write it with that rev"
            ),
            EditError::Stale {
                path,
                expected,
                rev,
                current,
            } => write!(
                f,
                "`{path}` changed since you read it (your rev {expected}, now {rev}), probably \
                 by another session. Apply your change to the current content below.\n\
                 --- current content of {path} (rev {rev}) ---\n{current}"
            ),
            EditError::NoMatch { path, rev, current } => write!(
                f,
                "`old` text not found in `{path}` (it may have been changed by another session). \
                 Apply your change to the current content below.\n\
                 --- current content of {path} (rev {rev}) ---\n{current}"
            ),
            EditError::Ambiguous { path, count } => write!(
                f,
                "`old` text occurs {count} times in `{path}`: include more surrounding text so it is unique"
            ),
            EditError::Invalid { path, why } => write!(f, "`{path}`: {why}"),
            EditError::Io { path, detail } => write!(f, "`{path}`: {detail}"),
        }
    }
}

impl std::error::Error for EditError {}

/// What a path names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Instructions,
    Mcp(String),
    /// `skills/<skill>/<rest>`.
    SkillFile(String),
    /// `skills/<skill>` (delete only).
    Skill(String),
}

fn bad(path: &str, why: &str) -> EditError {
    EditError::BadPath {
        path: path.to_owned(),
        why: why.to_owned(),
    }
}

/// `path` with `/` separators and no leading `./`.
fn normalize(path: &str) -> String {
    let p = path.trim().replace('\\', "/");
    p.trim_start_matches("./").to_owned()
}

fn classify(path: &str) -> Result<Kind, EditError> {
    const WHERE: &str = "a profile has instructions.md, mcp/<name>.json and skills/<name>/...";
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|c| c.is_empty() || c.starts_with('.') || c.contains(':') || c.trim() != *c)
    {
        return Err(bad(path, "empty, hidden or `..` path parts are not allowed"));
    }
    match parts.as_slice() {
        [INSTRUCTIONS_FILE] => Ok(Kind::Instructions),
        [MCP_DIR, file] => {
            let name = file
                .strip_suffix(".json")
                .ok_or_else(|| bad(path, "MCP servers are mcp/<name>.json"))?;
            crate::configuration::validate_mcp_name(name)
                .map_err(|_| bad(path, "MCP server names are 1-64 of A-Z a-z 0-9 _ -"))?;
            Ok(Kind::Mcp(name.to_owned()))
        }
        [SKILLS_DIR, name, rest @ ..] => {
            if !skill::valid_name(name) {
                return Err(bad(path, "skill folder names are lowercase a-z, 0-9 and - (e.g. pdf-tools)"));
            }
            if rest.is_empty() {
                Ok(Kind::Skill((*name).to_owned()))
            } else {
                Ok(Kind::SkillFile((*name).to_owned()))
            }
        }
        _ => Err(bad(path, WHERE)),
    }
}

fn io(path: &str, e: impl fmt::Display) -> EditError {
    EditError::Io {
        path: path.to_owned(),
        detail: e.to_string(),
    }
}

fn file_of(dir: &Path, path: &str) -> PathBuf {
    path.split('/').fold(dir.to_path_buf(), |p, c| p.join(c))
}

fn read_disk(dir: &Path, path: &str) -> Result<Option<Vec<u8>>, EditError> {
    let f = file_of(dir, path);
    if f.is_dir() {
        return Ok(None);
    }
    match std::fs::read(&f) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path, e)),
    }
}

/// Files under `dir/rel` (recursive, `/`-separated paths relative to `dir`; hidden entries
/// skipped).
fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(file_of(dir, rel)) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(dir, &p, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

/// The editable files of the profile in `dir` (sorted).
pub fn list(dir: &Path) -> Vec<FileInfo> {
    let mut paths = Vec::new();
    if file_of(dir, INSTRUCTIONS_FILE).is_file() {
        paths.push(INSTRUCTIONS_FILE.to_owned());
    }
    walk(dir, MCP_DIR, &mut paths);
    walk(dir, SKILLS_DIR, &mut paths);
    let mut out: Vec<FileInfo> = paths
        .into_iter()
        .filter(|p| classify(p).is_ok())
        .filter_map(|p| {
            let bytes = std::fs::read(file_of(dir, &p)).ok()?;
            Some(FileInfo {
                rev: rev(&bytes),
                size: bytes.len() as u64,
                path: p,
            })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// A file's text and revision.
pub fn read(dir: &Path, path: &str) -> Result<(String, String), EditError> {
    let path = normalize(path);
    if let Kind::Skill(_) = classify(&path)? {
        return Err(bad(&path, "a skill is a folder: read skills/<name>/SKILL.md"));
    }
    let bytes = read_disk(dir, &path)?.ok_or_else(|| EditError::NotFound { path: path.clone() })?;
    let r = rev(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| EditError::Invalid {
        path: path.clone(),
        why: "not a UTF-8 text file".into(),
    })?;
    Ok((text, r))
}

/// The planned end state of the touched files (`None` = deleted).
struct Plan<'a> {
    dir: &'a Path,
    files: BTreeMap<String, Option<Vec<u8>>>,
    /// Skills deleted as a whole.
    gone: BTreeSet<String>,
}

impl Plan<'_> {
    fn current(&self, path: &str) -> Result<Option<Vec<u8>>, EditError> {
        match self.files.get(path) {
            Some(v) => Ok(v.clone()),
            None => read_disk(self.dir, path),
        }
    }
}

fn text_of(path: &str, bytes: Vec<u8>) -> Result<String, EditError> {
    String::from_utf8(bytes).map_err(|_| EditError::Invalid {
        path: path.to_owned(),
        why: "not a UTF-8 text file: write it whole instead".into(),
    })
}

/// Apply `ops` to the profile in `dir` (all or nothing); the changed files.
pub fn apply(dir: &Path, ops: &[EditOp]) -> Result<Vec<Changed>, EditError> {
    if ops.is_empty() {
        return Err(bad("", "no operations"));
    }
    let mut plan = Plan {
        dir,
        files: BTreeMap::new(),
        gone: BTreeSet::new(),
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut skills: BTreeSet<String> = BTreeSet::new();
    let mut mcp: BTreeSet<(String, String)> = BTreeSet::new();
    for op in ops {
        let path = normalize(op.path());
        let kind = classify(&path)?;
        if !seen.insert(path.clone()) {
            return Err(bad(&path, "named twice in one call: one operation per file"));
        }
        match &kind {
            Kind::SkillFile(s) => {
                skills.insert(s.clone());
            }
            Kind::Mcp(n) => {
                mcp.insert((path.clone(), n.clone()));
            }
            Kind::Skill(s) if !matches!(op, EditOp::Delete { .. }) => {
                return Err(bad(&path, &format!("a skill is a folder: write skills/{s}/{SKILL_FILE}")));
            }
            _ => {}
        }
        match op {
            EditOp::Patch { old, new, .. } => {
                if old.is_empty() {
                    return Err(bad(&path, "`old` is empty: give the text to replace (or write the file)"));
                }
                let cur = plan
                    .current(&path)?
                    .ok_or_else(|| EditError::NotFound { path: path.clone() })?;
                let r = rev(&cur);
                let text = text_of(&path, cur)?;
                let count = text.matches(old.as_str()).count();
                if count == 0 {
                    return Err(EditError::NoMatch {
                        path,
                        rev: r,
                        current: text,
                    });
                }
                if count > 1 {
                    return Err(EditError::Ambiguous { path, count });
                }
                plan.files
                    .insert(path, Some(text.replacen(old.as_str(), new, 1).into_bytes()));
            }
            EditOp::Write { content, rev: base, .. } => {
                let cur = plan.current(&path)?;
                match (base, cur) {
                    (None, Some(c)) => {
                        return Err(EditError::Exists { path, rev: rev(&c) });
                    }
                    (Some(_), None) => return Err(EditError::NotFound { path }),
                    (Some(b), Some(c)) if rev(&c) != b.trim() => {
                        return Err(EditError::Stale {
                            expected: b.trim().to_owned(),
                            rev: rev(&c),
                            current: String::from_utf8_lossy(&c).into_owned(),
                            path,
                        });
                    }
                    _ => {}
                }
                plan.files.insert(path, Some(content.clone().into_bytes()));
            }
            EditOp::Delete { rev: base, .. } => {
                if kind == Kind::Instructions {
                    return Err(EditError::Invalid {
                        path,
                        why: "instructions.md cannot be deleted: write it empty instead".into(),
                    });
                }
                let check = match &kind {
                    Kind::Skill(s) => format!("{SKILLS_DIR}/{s}/{SKILL_FILE}"),
                    _ => path.clone(),
                };
                let cur = plan
                    .current(&check)?
                    .ok_or_else(|| EditError::NotFound { path: path.clone() })?;
                if rev(&cur) != base.trim() {
                    return Err(EditError::Stale {
                        expected: base.trim().to_owned(),
                        rev: rev(&cur),
                        current: String::from_utf8_lossy(&cur).into_owned(),
                        path: check,
                    });
                }
                if let Kind::Skill(s) = &kind {
                    let mut all = Vec::new();
                    walk(dir, &format!("{SKILLS_DIR}/{s}"), &mut all);
                    let prefix = format!("{SKILLS_DIR}/{s}/");
                    if let Some(p) = seen.iter().find(|p| p.starts_with(&prefix)) {
                        return Err(bad(p, "named twice in one call: the whole skill is deleted"));
                    }
                    for p in all {
                        seen.insert(p.clone());
                        plan.files.insert(p, None);
                    }
                    plan.gone.insert(s.clone());
                } else {
                    plan.files.insert(path, None);
                }
            }
        }
    }

    // The end state must be valid.
    for (path, v) in &plan.files {
        if let Some(b) = v
            && b.len() > MAX_FILE
        {
            return Err(EditError::Invalid {
                path: path.clone(),
                why: format!("larger than {} KiB", MAX_FILE / 1024),
            });
        }
    }
    for (path, name) in &mcp {
        let Some(bytes) = plan.current(path)? else { continue };
        let f: McpFile = serde_json::from_slice(&bytes).map_err(|e| EditError::Invalid {
            path: path.clone(),
            why: format!(
                "not an MCP server ({e}); expected {{\"command\": \"...\", \"args\": [\"...\"], \"env\": [[\"KEY\", \"value\"]]}}"
            ),
        })?;
        validate_mcp_server(&f.into_server(name)).map_err(|_| EditError::Invalid {
            path: path.clone(),
            why: "`command` is empty or an env key is empty or contains `=`".into(),
        })?;
    }
    for s in skills.iter().filter(|s| !plan.gone.contains(*s)) {
        let skill_md = format!("{SKILLS_DIR}/{s}/{SKILL_FILE}");
        let mut remaining = Vec::new();
        walk(dir, &format!("{SKILLS_DIR}/{s}"), &mut remaining);
        let prefix = format!("{SKILLS_DIR}/{s}/");
        let left = remaining
            .iter()
            .filter(|p| !matches!(plan.files.get(*p), Some(None)))
            .count()
            + plan
                .files
                .iter()
                .filter(|(p, v)| p.starts_with(&prefix) && v.is_some() && !remaining.contains(p))
                .count();
        if left > 0 && plan.current(&skill_md)?.is_none() {
            return Err(EditError::Invalid {
                path: skill_md,
                why: format!(
                    "a skill needs {SKILL_FILE} (front matter `name` and `description`, then instructions); \
                     delete the whole skill with path skills/{s}"
                ),
            });
        }
    }

    // Write.
    let mut changed = Vec::new();
    for (path, v) in &plan.files {
        let f = file_of(dir, path);
        match v {
            Some(bytes) => {
                if let Some(parent) = f.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(path, e))?;
                }
                std::fs::write(&f, bytes).map_err(|e| io(path, e))?;
                changed.push(Changed {
                    path: path.clone(),
                    rev: Some(rev(bytes)),
                });
            }
            None => {
                match std::fs::remove_file(&f) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io(path, e)),
                }
                changed.push(Changed {
                    path: path.clone(),
                    rev: None,
                });
            }
        }
    }
    for s in &plan.gone {
        let d = file_of(dir, &format!("{SKILLS_DIR}/{s}"));
        if d.exists() {
            std::fs::remove_dir_all(&d).map_err(|e| io(&format!("{SKILLS_DIR}/{s}"), e))?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let t = TempDir::new("profile-files");
        let dir = t.0.join("p");
        std::fs::create_dir_all(dir.join("skills/lint")).unwrap();
        std::fs::create_dir_all(dir.join("mcp")).unwrap();
        std::fs::write(dir.join(INSTRUCTIONS_FILE), "Use pnpm.\nNever push.\n").unwrap();
        std::fs::write(dir.join("skills/lint/SKILL.md"), "---\nname: lint\n---\nRun it.\n").unwrap();
        std::fs::write(dir.join("skills/lint/rules.txt"), "x").unwrap();
        std::fs::write(dir.join("mcp/docs.json"), r#"{"command":"node","args":["d.js"]}"#).unwrap();
        std::fs::write(dir.join(PROFILE_FILE), "{}").unwrap();
        (t, dir)
    }

    fn patch(path: &str, old: &str, new: &str) -> EditOp {
        EditOp::Patch {
            path: path.into(),
            old: old.into(),
            new: new.into(),
        }
    }

    #[test]
    fn list_and_read() {
        let (_t, dir) = setup();
        let files: Vec<String> = list(&dir).into_iter().map(|f| f.path).collect();
        assert_eq!(
            files,
            ["instructions.md", "mcp/docs.json", "skills/lint/SKILL.md", "skills/lint/rules.txt"]
        );
        let (text, r) = read(&dir, "instructions.md").unwrap();
        assert_eq!(r, rev(text.as_bytes()));
        assert!(matches!(read(&dir, "profile.json"), Err(EditError::BadPath { .. })));
        assert!(matches!(read(&dir, "../x"), Err(EditError::BadPath { .. })));
        assert!(matches!(read(&dir, "skills/lint"), Err(EditError::BadPath { .. })));
        assert!(matches!(read(&dir, "skills/x/SKILL.md"), Err(EditError::NotFound { .. })));
    }

    #[test]
    fn patch_checks_the_current_text() {
        let (_t, dir) = setup();
        apply(&dir, &[patch("instructions.md", "Use pnpm.", "Use pnpm 9.")]).unwrap();
        // Another session's patch of the same text no longer applies.
        let e = apply(&dir, &[patch("instructions.md", "Use pnpm.", "Use npm.")]).unwrap_err();
        match e {
            EditError::NoMatch { current, .. } => assert!(current.contains("pnpm 9")),
            e => panic!("{e:?}"),
        }
        // A patch elsewhere in the same file still applies.
        apply(&dir, &[patch("instructions.md", "Never push.", "Never force-push.")]).unwrap();
        assert_eq!(
            read(&dir, "instructions.md").unwrap().0,
            "Use pnpm 9.\nNever force-push.\n"
        );
        std::fs::write(dir.join(INSTRUCTIONS_FILE), "a a").unwrap();
        assert!(matches!(
            apply(&dir, &[patch("instructions.md", "a", "b")]),
            Err(EditError::Ambiguous { count: 2, .. })
        ));
    }

    #[test]
    fn write_and_delete_check_the_revision() {
        let (_t, dir) = setup();
        let (_, r) = read(&dir, "skills/lint/SKILL.md").unwrap();
        let w = |rev: Option<&str>, c: &str| EditOp::Write {
            path: "skills/lint/SKILL.md".into(),
            content: c.into(),
            rev: rev.map(str::to_owned),
        };
        assert!(matches!(apply(&dir, &[w(None, "x")]), Err(EditError::Exists { .. })));
        apply(&dir, &[w(Some(&r), "---\nname: lint\n---\nv2\n")]).unwrap();
        // Based on the old revision: refused, current content returned.
        match apply(&dir, &[w(Some(&r), "v3")]).unwrap_err() {
            EditError::Stale { current, .. } => assert!(current.contains("v2")),
            e => panic!("{e:?}"),
        }
        // Delete the whole skill with its SKILL.md revision.
        let (_, r2) = read(&dir, "skills/lint/SKILL.md").unwrap();
        let del = |rev: &str| EditOp::Delete {
            path: "skills/lint".into(),
            rev: rev.into(),
        };
        assert!(matches!(apply(&dir, &[del(&r)]), Err(EditError::Stale { .. })));
        let changed = apply(&dir, &[del(&r2)]).unwrap();
        assert_eq!(changed.len(), 2);
        assert!(!dir.join("skills/lint").exists());
        assert!(matches!(
            apply(&dir, &[EditOp::Delete { path: "instructions.md".into(), rev: "x".into() }]),
            Err(EditError::Invalid { .. })
        ));
    }

    #[test]
    fn all_or_nothing_and_validation() {
        let (_t, dir) = setup();
        let new_skill = EditOp::Write {
            path: "skills/pdf/notes.md".into(),
            content: "n".into(),
            rev: None,
        };
        // A new skill needs its SKILL.md: nothing written.
        assert!(matches!(
            apply(&dir, &[new_skill.clone()]),
            Err(EditError::Invalid { .. })
        ));
        assert!(!dir.join("skills/pdf").exists());
        let skill_md = EditOp::Write {
            path: "skills/pdf/SKILL.md".into(),
            content: "---\nname: pdf\ndescription: d\n---\n".into(),
            rev: None,
        };
        // A failing op later in the call: the first is not applied either.
        assert!(apply(&dir, &[skill_md.clone(), patch("instructions.md", "nope", "x")]).is_err());
        assert!(!dir.join("skills/pdf").exists());
        apply(&dir, &[skill_md, new_skill]).unwrap();
        assert!(dir.join("skills/pdf/notes.md").is_file());

        let mcp = |c: &str| EditOp::Write {
            path: "mcp/db.json".into(),
            content: c.into(),
            rev: None,
        };
        assert!(matches!(apply(&dir, &[mcp("{}")]), Err(EditError::Invalid { .. })));
        assert!(matches!(
            apply(&dir, &[mcp(r#"{"command":" "}"#)]),
            Err(EditError::Invalid { .. })
        ));
        apply(&dir, &[mcp(r#"{"command":"db","env":[["K","v"]]}"#)]).unwrap();
        assert!(matches!(
            apply(&dir, &[patch("instructions.md", "pnpm", "a"), patch("./instructions.md", "push", "b")]),
            Err(EditError::BadPath { .. })
        ));
        assert!(matches!(
            apply(&dir, &[EditOp::Write { path: "mcp/a b.json".into(), content: "{}".into(), rev: None }]),
            Err(EditError::BadPath { .. })
        ));
        assert!(matches!(
            apply(&dir, &[EditOp::Write { path: "skills/Big/SKILL.md".into(), content: "x".into(), rev: None }]),
            Err(EditError::BadPath { .. })
        ));
    }
}
