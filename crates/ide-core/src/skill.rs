//! Skills owned by a configuration (shown as "프로필" / "profile"): each one is a folder
//! `<data>/configurations/<slug>/skills/<name>/` with a `SKILL.md` (agent skills format: YAML
//! front matter with `name` and `description`, then instructions) and any files it refers to.
//! The folder is the source of truth: the configuration only remembers which skills are off.
//! Hermes Desk keeps an agent's skills in its profile home the same way.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoAction, Result};
use crate::slug::slugify;

/// `SKILL.md`.
pub const SKILL_FILE: &str = "SKILL.md";

/// A skill of a configuration, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    /// Folder name (a slug; identifies the skill).
    pub name: String,
    /// `name` from the front matter (else the folder name).
    pub title: String,
    /// `description` from the front matter (may be empty).
    pub description: String,
    /// Offered to new sessions.
    pub enabled: bool,
}

/// `name` and `description` of a `---` YAML front matter block (single-line values, quotes
/// stripped).
pub fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let (mut name, mut desc) = (None, None);
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_owned();
            match k.trim() {
                "name" => name = Some(v),
                "description" => desc = Some(v),
                _ => {}
            }
        }
    }
    (name, desc)
}

/// The `SKILL.md` of a new skill.
pub fn template(name: &str, description: &str) -> String {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n",
        one_line(name),
        one_line(description)
    )
}

/// A skill folder name: what [`slugify`] makes of it, so it is safe as one path component.
pub fn folder_name(text: &str) -> String {
    slugify(text, "skill")
}

/// Whether `name` can be a skill folder of the store (no separators, no `..`).
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && folder_name(name) == name
}

/// The skills in `dir` (sorted by folder name); folders without `SKILL.md` are left out.
pub(crate) fn scan(dir: &Path, disabled: &[String]) -> Vec<SkillEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SkillEntry> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(e.path().join(SKILL_FILE)).ok()?;
            let (title, description) = front_matter(&text);
            Some(SkillEntry {
                title: title.filter(|t| !t.is_empty()).unwrap_or_else(|| name.clone()),
                description: description.unwrap_or_default(),
                enabled: !disabled.contains(&name),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A folder name in `dir` based on `base` that is not taken (`base`, `base-2`, ...).
pub(crate) fn unique_name(dir: &Path, base: &str) -> String {
    let mut name = base.to_owned();
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

/// Copy the skill folder `src` to `dst` (regular files and folders; links and `.git` are
/// skipped). `src` must contain `SKILL.md`.
pub(crate) fn copy_skill(src: &Path, dst: &Path) -> Result<()> {
    if !src.join(SKILL_FILE).is_file() {
        return Err(Error::InvalidSkill(src.to_path_buf()));
    }
    copy_dir(src, dst)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| Error::io(IoAction::CreatingDirectory, dst, e))?;
    let rd = std::fs::read_dir(src).map_err(|e| Error::io(IoAction::Reading, src, e))?;
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let name = e.file_name();
        let to: PathBuf = dst.join(&name);
        if t.is_dir() {
            if name != ".git" {
                copy_dir(&e.path(), &to)?;
            }
        } else if t.is_file() {
            std::fs::copy(e.path(), &to).map_err(|err| Error::io(IoAction::Writing, &to, err))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_and_template() {
        let t = template("lint", "Runs the\nlinter");
        assert_eq!(t, "---\nname: lint\ndescription: Runs the linter\n---\n\n");
        assert_eq!(
            front_matter(&t),
            (Some("lint".into()), Some("Runs the linter".into()))
        );
        assert_eq!(front_matter("no front matter"), (None, None));
        assert_eq!(
            front_matter("\u{feff}---\nname: \"x\"\n---\nname: y"),
            (Some("x".into()), None)
        );
    }

    #[test]
    fn names() {
        assert!(valid_name("pdf-tools"));
        for bad in ["", "..", "a/b", "a\\b", "Big", "a b"] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert_eq!(folder_name("PDF Tools!"), "pdf-tools");
    }
}
