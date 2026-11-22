//! Skills: folders with a `SKILL.md` (agent skills format).

use std::path::PathBuf;

use ide_core::SkillRef;
use ide_core::skill::front_matter;

/// A loaded skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// `name` from the front matter (else the folder name).
    pub name: String,
    /// `description` from the front matter (may be empty).
    pub description: String,
    /// The skill folder.
    pub dir: PathBuf,
}

impl Skill {
    /// `<dir>/SKILL.md`.
    pub fn skill_md(&self) -> PathBuf {
        self.dir.join("SKILL.md")
    }

    /// Read a skill folder; `None` if `SKILL.md` is missing.
    pub fn read(dir: &std::path::Path) -> Option<Skill> {
        let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
        let (name, description) = front_matter(&text);
        let folder = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "skill".into());
        Some(Skill {
            name: name.filter(|n| !n.is_empty()).unwrap_or(folder),
            description: description.unwrap_or_default(),
            dir: dir.to_path_buf(),
        })
    }
}

/// Load the configuration's skills; missing ones are recorded in `notes`.
pub(crate) fn load(refs: &[SkillRef], notes: &mut Vec<String>) -> Vec<Skill> {
    refs.iter()
        .filter_map(|r| {
            let s = Skill::read(&r.path);
            if s.is_none() {
                notes.push(format!("skill skipped (no SKILL.md): {}", r.path.display()));
            }
            s
        })
        .collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_parsing() {
        let (n, d) = front_matter("---\nname: lint\ndescription: \"Run: the linter\"\n---\nbody");
        assert_eq!(n.as_deref(), Some("lint"));
        assert_eq!(d.as_deref(), Some("Run: the linter"));
        assert_eq!(front_matter("no front matter"), (None, None));
    }
}
