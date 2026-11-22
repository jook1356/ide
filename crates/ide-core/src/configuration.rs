//! Configurations: provider-agnostic bundles of skills, MCP servers and instructions that are
//! layered on top of any account per session (they replace the old "profiles"; the UI calls
//! them "프로필" / "profiles" again). Each one owns its skills (`skill.rs`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::account::clean_name;
use crate::error::{Error, IoAction, Result};
use crate::skill::{self, SKILL_FILE, SkillEntry};
use crate::paths::{Paths, now_unix, read_json, write_json};
use crate::slug::{slugify, validate_slug};

/// Slug of the configuration created on first run.
pub const DEFAULT_CONFIGURATION: &str = "default";

/// A skill: a folder containing `SKILL.md` (agent skills format: YAML front matter with
/// `name` and `description`, then instructions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRef {
    /// The skill folder (absolute, on the server).
    pub path: PathBuf,
}

/// A local (stdio) MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    /// Server name: 1..=64 chars of `[A-Za-z0-9_-]`, unique per configuration (used as a
    /// config key by the CLIs).
    pub name: String,
    /// Program to run.
    pub command: String,
    /// Arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables of the server process.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// A configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Configuration {
    /// Unique slug (derived from the first name; stays when renamed).
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Skills offered to the agent: the enabled skill folders of this configuration, filled
    /// by the store from `<data>/configurations/<slug>/skills/` (see `skill.rs`). Not saved;
    /// older files listed skill folders elsewhere here, which `open` copies in once.
    #[serde(default, skip_serializing)]
    pub skills: Vec<SkillRef>,
    /// Skill folder names switched off (Desk's `skills.disabled`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_skills: Vec<String>,
    /// MCP servers to add to the agent.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// Extra instructions appended to the agent's system/developer prompt.
    #[serde(default)]
    pub instructions: String,
    /// Repositories pre-checked in the new-session dialog (Desk: the agent's "기본 레포").
    /// `None` = every registered repository (Desk's default when nothing was saved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_repos: Option<Vec<crate::registry::RepoId>>,
    /// Creation time, Unix seconds.
    #[serde(default)]
    pub created_at: u64,
}

impl Configuration {
    /// Desk's `defaultReposFor`: the saved defaults that are still registered (in `registered`
    /// order), or every registered repository when none were saved.
    pub fn default_repos_for(
        &self,
        registered: &[crate::registry::RepoId],
    ) -> Vec<crate::registry::RepoId> {
        match &self.default_repos {
            None => registered.to_vec(),
            Some(saved) => registered
                .iter()
                .filter(|r| saved.contains(r))
                .cloned()
                .collect(),
        }
    }

    /// An empty configuration.
    pub fn empty(slug: &str, name: &str) -> Self {
        Configuration {
            slug: slug.to_owned(),
            name: name.to_owned(),
            skills: Vec::new(),
            disabled_skills: Vec::new(),
            mcp_servers: Vec::new(),
            instructions: String::new(),
            default_repos: None,
            created_at: now_unix(),
        }
    }

    /// Check names: display name, MCP server names (format, uniqueness), env keys.
    pub fn validate(&self) -> Result<()> {
        validate_slug(&self.slug)?;
        clean_name(&self.name)?;
        let mut seen: Vec<&str> = Vec::new();
        for m in &self.mcp_servers {
            validate_mcp_name(&m.name)?;
            if seen.contains(&m.name.as_str()) {
                return Err(Error::InvalidMcpName(m.name.clone()));
            }
            seen.push(&m.name);
            if m.command.trim().is_empty()
                || m.env.iter().any(|(k, _)| k.is_empty() || k.contains('='))
            {
                return Err(Error::InvalidMcpName(m.name.clone()));
            }
        }
        Ok(())
    }
}

/// MCP server names: 1..=64 chars of `[A-Za-z0-9_-]` (a TOML bare key and a JSON key).
pub fn validate_mcp_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::InvalidMcpName(name.to_owned()));
    }
    Ok(())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigurationsFile {
    #[serde(default)]
    configurations: Vec<Configuration>,
}

/// Persistent configuration list backed by `configurations.json`. Every mutation saves
/// immediately. There is always at least one configuration.
#[derive(Debug)]
pub struct ConfigurationStore {
    paths: Paths,
    configs: Vec<Configuration>,
}

impl ConfigurationStore {
    /// Load `configurations.json`; creates the `default` configuration if there is none.
    /// Run [`crate::migrate::migrate_profiles`] first so legacy profiles become configurations.
    pub fn open(paths: &Paths) -> Result<Self> {
        let file: ConfigurationsFile = read_json(&paths.configurations_file())?.unwrap_or_default();
        for c in &file.configurations {
            validate_slug(&c.slug)?;
        }
        let mut store = Self {
            paths: paths.clone(),
            configs: file.configurations,
        };
        let mut dirty = false;
        if store.configs.is_empty() {
            store.configs.push(Configuration::empty(
                DEFAULT_CONFIGURATION,
                DEFAULT_CONFIGURATION,
            ));
            dirty = true;
        }
        // Older files pointed at skill folders anywhere: copy them in once (a missing one is
        // dropped; the path stays in the log).
        for i in 0..store.configs.len() {
            let legacy = std::mem::take(&mut store.configs[i].skills);
            if legacy.is_empty() {
                continue;
            }
            dirty = true;
            let dir = paths.configuration_skills_dir(&store.configs[i].slug);
            for r in legacy {
                let base = skill::folder_name(
                    &r.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                let to = dir.join(skill::unique_name(&dir, &base));
                if let Err(e) = skill::copy_skill(&r.path, &to) {
                    tracing::warn!(from = %r.path.display(), "skill not copied: {e}");
                }
            }
        }
        for i in 0..store.configs.len() {
            store.refresh(i);
        }
        if dirty {
            store.save()?;
        }
        Ok(store)
    }

    /// Fill `skills` of configuration `i` from its skill folder.
    fn refresh(&mut self, i: usize) {
        let c = &self.configs[i];
        let dir = self.paths.configuration_skills_dir(&c.slug);
        let skills = skill::scan(&dir, &c.disabled_skills)
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| SkillRef {
                path: dir.join(s.name),
            })
            .collect();
        self.configs[i].skills = skills;
    }

    fn index(&self, slug: &str) -> Result<usize> {
        self.configs
            .iter()
            .position(|c| c.slug == slug)
            .ok_or_else(|| Error::ConfigurationNotFound(slug.to_owned()))
    }

    /// The skills of configuration `slug` (also the switched-off ones), as on disk now.
    pub fn skill_list(&self, slug: &str) -> Vec<SkillEntry> {
        let off = self.get(slug).map(|c| c.disabled_skills.as_slice()).unwrap_or(&[]);
        skill::scan(&self.paths.configuration_skills_dir(slug), off)
    }

    /// The skills of configuration `slug` (also the switched-off ones), rescanned from disk
    /// (its `skills` follow).
    pub fn skills(&mut self, slug: &str) -> Result<Vec<SkillEntry>> {
        let i = self.index(slug)?;
        self.refresh(i);
        let c = &self.configs[i];
        Ok(skill::scan(
            &self.paths.configuration_skills_dir(slug),
            &c.disabled_skills,
        ))
    }

    /// The folder of skill `name` of configuration `slug` (which must exist).
    fn skill_dir(&self, slug: &str, name: &str) -> Result<PathBuf> {
        self.index(slug)?;
        let dir = self.paths.configuration_skills_dir(slug).join(name);
        if !skill::valid_name(name) || !dir.join(SKILL_FILE).is_file() {
            return Err(Error::SkillNotFound(name.to_owned()));
        }
        Ok(dir)
    }

    /// A new skill from a name and a description (a `SKILL.md` with that front matter);
    /// returns its folder name.
    pub fn skill_create(&mut self, slug: &str, name: &str, description: &str) -> Result<String> {
        let i = self.index(slug)?;
        let name = clean_name(name)?;
        let folder = skill::folder_name(&name);
        let dir = self.paths.configuration_skills_dir(slug);
        if dir.join(&folder).exists() {
            return Err(Error::SkillExists(folder));
        }
        let path = dir.join(&folder);
        std::fs::create_dir_all(&path)
            .map_err(|e| Error::io(IoAction::CreatingDirectory, &path, e))?;
        let file = path.join(SKILL_FILE);
        std::fs::write(&file, skill::template(&folder, description))
            .map_err(|e| Error::io(IoAction::Writing, &file, e))?;
        self.refresh(i);
        Ok(folder)
    }

    /// Copy the skill folder `src` (with a `SKILL.md`) into configuration `slug`; returns its
    /// folder name (made unique).
    pub fn skill_import(&mut self, slug: &str, src: &Path) -> Result<String> {
        let i = self.index(slug)?;
        let base = skill::folder_name(
            &src.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let dir = self.paths.configuration_skills_dir(slug);
        let folder = skill::unique_name(&dir, &base);
        let to = dir.join(&folder);
        if let Err(e) = skill::copy_skill(src, &to) {
            let _ = std::fs::remove_dir_all(&to);
            return Err(e);
        }
        self.refresh(i);
        Ok(folder)
    }

    /// The `SKILL.md` text of skill `name`.
    pub fn skill_read(&self, slug: &str, name: &str) -> Result<String> {
        let file = self.skill_dir(slug, name)?.join(SKILL_FILE);
        std::fs::read_to_string(&file).map_err(|e| Error::io(IoAction::Reading, &file, e))
    }

    /// Replace the `SKILL.md` text of skill `name`.
    pub fn skill_write(&mut self, slug: &str, name: &str, text: &str) -> Result<()> {
        let file = self.skill_dir(slug, name)?.join(SKILL_FILE);
        std::fs::write(&file, text).map_err(|e| Error::io(IoAction::Writing, &file, e))?;
        let i = self.index(slug)?;
        self.refresh(i);
        Ok(())
    }

    /// Delete skill `name` (its folder).
    pub fn skill_delete(&mut self, slug: &str, name: &str) -> Result<()> {
        let dir = self.skill_dir(slug, name)?;
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(IoAction::Removing, &dir, e))?;
        let i = self.index(slug)?;
        self.configs[i].disabled_skills.retain(|n| n != name);
        self.refresh(i);
        self.save()
    }

    /// Switch skill `name` on or off for new sessions.
    pub fn skill_set_enabled(&mut self, slug: &str, name: &str, enabled: bool) -> Result<()> {
        self.skill_dir(slug, name)?;
        let i = self.index(slug)?;
        let off = &mut self.configs[i].disabled_skills;
        off.retain(|n| n != name);
        if !enabled {
            off.push(name.to_owned());
        }
        self.refresh(i);
        self.save()
    }

    /// All configurations in creation order.
    pub fn list(&self) -> &[Configuration] {
        &self.configs
    }

    /// Look up a configuration.
    pub fn get(&self, slug: &str) -> Option<&Configuration> {
        self.configs.iter().find(|c| c.slug == slug)
    }

    /// Look up a configuration or fail with [`Error::ConfigurationNotFound`].
    pub fn require(&self, slug: &str) -> Result<&Configuration> {
        self.get(slug)
            .ok_or_else(|| Error::ConfigurationNotFound(slug.to_owned()))
    }

    /// Create an empty configuration; the slug is derived from `name` (made unique).
    pub fn create(&mut self, name: &str) -> Result<&Configuration> {
        let name = clean_name(name)?;
        let base = slugify(&name, "config");
        let mut slug = base.clone();
        let mut n = 2;
        while self.get(&slug).is_some() {
            slug = format!("{base}-{n}");
            n += 1;
        }
        self.configs.push(Configuration::empty(&slug, &name));
        self.save()?;
        Ok(self.configs.last().expect("just pushed"))
    }

    /// Add a configuration with a given slug (migration). No-op if it exists.
    pub fn adopt(&mut self, config: Configuration) -> Result<()> {
        config.validate()?;
        if self.get(&config.slug).is_none() {
            self.configs.push(config);
            self.save()?;
        }
        Ok(())
    }

    /// Replace a configuration's name, skills, MCP servers and instructions (matched by slug;
    /// `created_at` is kept).
    pub fn update(&mut self, mut config: Configuration) -> Result<&Configuration> {
        config.name = clean_name(&config.name)?;
        config.validate()?;
        let idx = self
            .configs
            .iter()
            .position(|c| c.slug == config.slug)
            .ok_or_else(|| Error::ConfigurationNotFound(config.slug.clone()))?;
        config.created_at = self.configs[idx].created_at;
        // Skills are managed with the `skill_*` calls (their folder is the source).
        config.disabled_skills = std::mem::take(&mut self.configs[idx].disabled_skills);
        self.configs[idx] = config;
        self.save()?;
        self.refresh(idx);
        Ok(&self.configs[idx])
    }

    /// Delete a configuration (not the last one).
    pub fn delete(&mut self, slug: &str) -> Result<Configuration> {
        let idx = self
            .configs
            .iter()
            .position(|c| c.slug == slug)
            .ok_or_else(|| Error::ConfigurationNotFound(slug.to_owned()))?;
        if self.configs.len() == 1 {
            return Err(Error::LastConfiguration(self.configs[idx].name.clone()));
        }
        let c = self.configs.remove(idx);
        let dir = self.paths.configuration_dir(slug);
        if dir.exists() && let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), "configuration files not removed: {e}");
        }
        self.save()?;
        Ok(c)
    }

    fn save(&self) -> Result<()> {
        #[derive(Serialize)]
        struct Out<'a> {
            configurations: &'a [Configuration],
        }
        write_json(
            &self.paths.configurations_file(),
            &Out {
                configurations: &self.configs,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;
    use crate::skill::template;

    fn mcp(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            command: "node".into(),
            args: vec!["s.js".into()],
            env: vec![("K".into(), "v".into())],
        }
    }

    #[test]
    fn owned_skills() {
        let t = TempDir::new("cfg-skills");
        let paths = Paths::new(&t.0);
        // An older file listing a skill folder elsewhere: copied in on open.
        let outside = t.0.join("outside").join("My Lint");
        std::fs::create_dir_all(outside.join(".git")).unwrap();
        std::fs::write(outside.join("SKILL.md"), template("lint", "Lints")).unwrap();
        std::fs::write(outside.join("rules.txt"), "x").unwrap();
        std::fs::write(
            paths.configurations_file(),
            format!(
                r#"{{"configurations":[{{"slug":"default","name":"default","skills":[{{"path":{:?}}},{{"path":"C:/nope"}}]}}]}}"#,
                outside.display().to_string()
            ),
        )
        .unwrap();
        let mut store = ConfigurationStore::open(&paths).unwrap();
        let dir = paths.configuration_skills_dir("default");
        assert!(dir.join("my-lint").join("rules.txt").is_file());
        assert!(!dir.join("my-lint").join(".git").exists());
        assert_eq!(store.get("default").unwrap().skills.len(), 1);
        assert!(!std::fs::read_to_string(paths.configurations_file()).unwrap().contains("outside"));

        // Create, read, write, switch off, delete.
        assert_eq!(store.skill_create("default", "PDF tools", "Fills PDFs").unwrap(), "pdf-tools");
        assert!(matches!(
            store.skill_create("default", "pdf tools", ""),
            Err(Error::SkillExists(_))
        ));
        let list = store.skills("default").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].description, "Fills PDFs");
        store.skill_write("default", "pdf-tools", &template("pdf", "New")).unwrap();
        assert_eq!(store.skills("default").unwrap()[1].title, "pdf");
        assert!(store.skill_read("default", "pdf-tools").unwrap().contains("New"));
        store.skill_set_enabled("default", "my-lint", false).unwrap();
        assert_eq!(store.get("default").unwrap().skills.len(), 1);
        let reopened = ConfigurationStore::open(&paths).unwrap();
        assert_eq!(reopened.get("default").unwrap().disabled_skills, ["my-lint"]);
        assert_eq!(reopened.get("default").unwrap().skills.len(), 1);
        assert!(matches!(
            store.skill_read("default", "../x"),
            Err(Error::SkillNotFound(_))
        ));
        assert!(matches!(
            store.skill_import("default", &t.0.join("outside")),
            Err(Error::InvalidSkill(_))
        ));
        assert_eq!(store.skill_import("default", &outside).unwrap(), "my-lint-2");
        store.skill_delete("default", "my-lint").unwrap();
        assert!(store.get("default").unwrap().disabled_skills.is_empty());
        // An update keeps the skills; deleting the configuration removes its folder.
        let c = store.get("default").unwrap().clone();
        store.update(Configuration { skills: Vec::new(), ..c }).unwrap();
        assert_eq!(store.get("default").unwrap().skills.len(), 2);
        store.create("other").unwrap();
        store.delete("default").unwrap();
        assert!(!paths.configuration_dir("default").exists());
    }

    #[test]
    fn crud_and_validation() {
        let t = TempDir::new("cfg");
        let paths = Paths::new(&t.0);
        let mut store = ConfigurationStore::open(&paths).unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].slug, DEFAULT_CONFIGURATION);

        let c = store.create("Frontend work").unwrap().clone();
        assert_eq!(c.slug, "frontend-work");
        assert_eq!(
            store.create("frontend work").unwrap().slug,
            "frontend-work-2"
        );

        let mut edited = c.clone();
        edited.name = " Frontend ".into();
        edited.instructions = "Use pnpm.\nNever push.".into();
        edited.mcp_servers = vec![mcp("docs"), mcp("db_2")];
        edited.created_at = 0;
        let saved = store.update(edited.clone()).unwrap().clone();
        assert_eq!(saved.name, "Frontend");
        assert_eq!(saved.created_at, c.created_at);

        for bad in ["", "has space", "a.b", "ü"] {
            let mut x = edited.clone();
            x.mcp_servers = vec![mcp(bad)];
            assert!(
                matches!(store.update(x), Err(Error::InvalidMcpName(_))),
                "{bad}"
            );
        }
        let mut dup = edited.clone();
        dup.mcp_servers = vec![mcp("a"), mcp("a")];
        assert!(matches!(store.update(dup), Err(Error::InvalidMcpName(_))));
        let mut missing = edited.clone();
        missing.slug = "nope".into();
        assert!(matches!(
            store.update(missing),
            Err(Error::ConfigurationNotFound(_))
        ));

        // Default repos: none saved = all; saved = the still registered ones.
        let (r1, r2) = (crate::registry::RepoId::new(), crate::registry::RepoId::new());
        let all = vec![r1.clone(), r2.clone()];
        assert_eq!(saved.default_repos_for(&all), all);
        let mut with = saved.clone();
        with.default_repos = Some(vec![r2.clone(), crate::registry::RepoId::new()]);
        store.update(with.clone()).unwrap();
        assert_eq!(
            store.get(&with.slug).unwrap().default_repos_for(&all),
            vec![r2.clone()]
        );
        with.default_repos = Some(Vec::new());
        assert!(with.default_repos_for(&all).is_empty());

        let reopened = ConfigurationStore::open(&paths).unwrap();
        assert_eq!(reopened.list(), store.list());
        assert_eq!(reopened.get("frontend-work").unwrap().mcp_servers.len(), 2);

        let mut store = reopened;
        store.delete("frontend-work-2").unwrap();
        store.delete("default").unwrap();
        assert!(matches!(
            store.delete("frontend-work"),
            Err(Error::LastConfiguration(_))
        ));
        assert!(matches!(
            store.delete("nope"),
            Err(Error::ConfigurationNotFound(_))
        ));
    }
}
