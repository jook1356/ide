//! Configurations: provider-agnostic bundles of skills, MCP servers and instructions that are
//! layered on top of any account per session (they replace the old "profiles"; the UI calls
//! them "프로필" / "profiles" again). Each one owns its skills (`skill.rs`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::account::clean_name;
use crate::error::{Error, IoAction, Result};
use crate::profile_files;
use crate::profile_history::{self, ChangeNote, ChangeSource, HistoryEntry};
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

    /// Check names: display name, MCP server names (format, uniqueness ignoring case: each is
    /// a file `mcp/<name>.json`), env keys.
    pub fn validate(&self) -> Result<()> {
        validate_slug(&self.slug)?;
        clean_name(&self.name)?;
        let mut seen: Vec<String> = Vec::new();
        for m in &self.mcp_servers {
            validate_mcp_server(m)?;
            let key = m.name.to_ascii_lowercase();
            if seen.contains(&key) {
                return Err(Error::InvalidMcpName(m.name.clone()));
            }
            seen.push(key);
        }
        Ok(())
    }
}

/// Check one MCP server: name, a command, env keys.
pub(crate) fn validate_mcp_server(m: &McpServer) -> Result<()> {
    validate_mcp_name(&m.name)?;
    if m.command.trim().is_empty() || m.env.iter().any(|(k, _)| k.is_empty() || k.contains('=')) {
        return Err(Error::InvalidMcpName(m.name.clone()));
    }
    Ok(())
}

/// `mcp/<name>.json`: one MCP server (the name is the file's).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct McpFile {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Vec<(String, String)>,
}

impl McpFile {
    pub(crate) fn into_server(self, name: &str) -> McpServer {
        McpServer {
            name: name.to_owned(),
            command: self.command,
            args: self.args,
            env: self.env,
        }
    }

    fn of(m: &McpServer) -> Self {
        McpFile {
            command: m.command.clone(),
            args: m.args.clone(),
            env: m.env.clone(),
        }
    }
}

/// `profile.json`: what is not a file of its own.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfileMeta {
    name: String,
    #[serde(default)]
    created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_repos: Option<Vec<crate::registry::RepoId>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disabled_skills: Vec<String>,
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

/// `configurations.json`: the order of the configurations (older files held them whole; they
/// are moved into their folders on open).
#[derive(Debug, Default, Deserialize)]
struct ConfigurationsFile {
    #[serde(default)]
    configurations: Vec<serde_json::Value>,
}

/// Persistent configurations. `configurations.json` lists their slugs in order; each one's
/// data lives in its folder `configurations/<slug>/` (`profile.json`, `instructions.md`,
/// `mcp/<name>.json`, `skills/`), a git repository of its own where every change is a commit
/// (`profile_history.rs`). Every mutation saves immediately. There is always at least one
/// configuration.
#[derive(Debug)]
pub struct ConfigurationStore {
    paths: Paths,
    configs: Vec<Configuration>,
}

/// Read configuration `slug` from its folder (`None`: no `profile.json`).
fn load_profile(paths: &Paths, slug: &str) -> Result<Option<Configuration>> {
    let dir = paths.configuration_dir(slug);
    let Some(meta) = read_json::<ProfileMeta>(&dir.join(profile_files::PROFILE_FILE))? else {
        return Ok(None);
    };
    let instructions = match std::fs::read_to_string(dir.join(profile_files::INSTRUCTIONS_FILE)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(IoAction::Reading, &dir, e)),
    };
    let mut mcp_servers = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join(profile_files::MCP_DIR)) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let Some(name) = f
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_owned)
            else {
                continue;
            };
            match read_json::<McpFile>(&f) {
                Ok(Some(m)) => {
                    let server = m.into_server(&name);
                    match validate_mcp_server(&server) {
                        Ok(()) => mcp_servers.push(server),
                        Err(e) => tracing::warn!(file = %f.display(), "MCP server left out: {e}"),
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(file = %f.display(), "MCP server left out: {e}"),
            }
        }
    }
    mcp_servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Some(Configuration {
        slug: slug.to_owned(),
        name: meta.name,
        skills: Vec::new(),
        disabled_skills: meta.disabled_skills,
        mcp_servers,
        instructions,
        default_repos: meta.default_repos,
        created_at: meta.created_at,
    }))
}

/// Write `path` unless it already holds `bytes`.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|b| b == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        crate::paths::ensure_dir(parent)?;
    }
    std::fs::write(path, bytes).map_err(|e| Error::io(IoAction::Writing, path, e))
}

/// Write configuration `c` into its folder (MCP files of removed servers are deleted).
fn store_profile(paths: &Paths, c: &Configuration) -> Result<()> {
    let dir = paths.configuration_dir(&c.slug);
    let meta = ProfileMeta {
        name: c.name.clone(),
        created_at: c.created_at,
        default_repos: c.default_repos.clone(),
        disabled_skills: c.disabled_skills.clone(),
    };
    let mut json = serde_json::to_vec_pretty(&meta).expect("json");
    json.push(b'\n');
    write_if_changed(&dir.join(profile_files::PROFILE_FILE), &json)?;
    write_if_changed(&dir.join(profile_files::INSTRUCTIONS_FILE), c.instructions.as_bytes())?;
    let mcp_dir = dir.join(profile_files::MCP_DIR);
    let keep: Vec<String> = c.mcp_servers.iter().map(|m| format!("{}.json", m.name)).collect();
    if let Ok(rd) = std::fs::read_dir(&mcp_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_file() && !keep.contains(&name) {
                std::fs::remove_file(e.path())
                    .map_err(|err| Error::io(IoAction::Removing, e.path(), err))?;
            }
        }
    }
    for m in &c.mcp_servers {
        let mut json = serde_json::to_vec_pretty(&McpFile::of(m)).expect("json");
        json.push(b'\n');
        write_if_changed(&mcp_dir.join(format!("{}.json", m.name)), &json)?;
    }
    Ok(())
}

impl ConfigurationStore {
    /// Load `configurations.json` and the configurations' folders; creates the `default`
    /// configuration if there is none. Older files that held the configurations whole are
    /// moved into the folders. Run [`crate::migrate::migrate_profiles`] first so legacy
    /// profiles become configurations.
    pub fn open(paths: &Paths) -> Result<Self> {
        let file: ConfigurationsFile = read_json(&paths.configurations_file())?.unwrap_or_default();
        let mut configs = Vec::new();
        let mut dirty = false;
        for v in file.configurations {
            let slug = v
                .get("slug")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned();
            validate_slug(&slug)?;
            if configs.iter().any(|c: &Configuration| c.slug == slug) {
                continue;
            }
            match load_profile(paths, &slug)? {
                Some(c) => configs.push(c),
                None => {
                    // An older file holding the whole configuration (or a lost folder).
                    dirty = true;
                    let c = serde_json::from_value::<Configuration>(v)
                        .unwrap_or_else(|_| Configuration::empty(&slug, &slug));
                    configs.push(c);
                }
            }
        }
        let mut store = Self {
            paths: paths.clone(),
            configs,
        };
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
        // Folders without a history get one (their state now is the first change).
        for c in &store.configs {
            let dir = paths.configuration_dir(&c.slug);
            if !dir.join(".git").exists() {
                store_profile(paths, c)?;
                store.commit(&c.slug, &ChangeNote::new(ChangeSource::Import, "Start history"));
            }
        }
        Ok(store)
    }

    /// Record the current files of configuration `slug` as one change (best effort: without
    /// git there is no history; logged).
    fn commit(&self, slug: &str, note: &ChangeNote) {
        if let Err(e) = profile_history::commit(&self.paths.configuration_dir(slug), note) {
            tracing::warn!(configuration = slug, "profile change not recorded: {e}");
        }
    }

    /// Save configuration `slug` and record the change.
    fn save_change(&self, slug: &str, note: &ChangeNote) -> Result<()> {
        self.save()?;
        self.commit(slug, note);
        Ok(())
    }

    /// Read configuration `slug` again from its folder (after its files changed).
    fn reload(&mut self, slug: &str) -> Result<()> {
        let i = self.index(slug)?;
        if let Some(c) = load_profile(&self.paths, slug)? {
            self.configs[i] = c;
        }
        self.refresh(i);
        Ok(())
    }

    /// The folder of configuration `slug` (its files; a git repository).
    pub fn folder(&self, slug: &str) -> Result<PathBuf> {
        self.index(slug)?;
        Ok(self.paths.configuration_dir(slug))
    }

    /// The editable files of configuration `slug` (`profile_files.rs`).
    pub fn files(&self, slug: &str) -> Result<Vec<profile_files::FileInfo>> {
        Ok(profile_files::list(&self.folder(slug)?))
    }

    /// Apply edits to the files of configuration `slug` (checked against the current files;
    /// all or nothing) and record them as one change.
    pub fn edit_files(
        &mut self,
        slug: &str,
        ops: &[profile_files::EditOp],
        note: &ChangeNote,
    ) -> Result<std::result::Result<Vec<profile_files::Changed>, profile_files::EditError>> {
        let dir = self.folder(slug)?;
        let changed = match profile_files::apply(&dir, ops) {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        self.reload(slug)?;
        // A deleted skill is no longer switched off.
        let i = self.index(slug)?;
        let present: Vec<String> = skill::scan(&self.paths.configuration_skills_dir(slug), &[])
            .into_iter()
            .map(|s| s.name)
            .collect();
        let before = self.configs[i].disabled_skills.len();
        self.configs[i].disabled_skills.retain(|n| present.contains(n));
        if self.configs[i].disabled_skills.len() != before {
            self.save()?;
        }
        self.commit(slug, note);
        Ok(Ok(changed))
    }

    /// The last `limit` changes of configuration `slug`, newest first.
    pub fn history(&self, slug: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
        profile_history::log(&self.folder(slug)?, limit)
    }

    /// Make configuration `slug` what it was after change `id` (recorded as a new change).
    pub fn restore(&mut self, slug: &str, id: &str, note: &ChangeNote) -> Result<()> {
        let dir = self.folder(slug)?;
        profile_history::restore(&dir, id, note)?;
        self.reload(slug)?;
        // The slug list is unchanged; the name may be.
        self.save()
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
        self.commit(slug, &ChangeNote::new(ChangeSource::Ui, format!("Add skill {folder}")));
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
        self.commit(slug, &ChangeNote::new(ChangeSource::Ui, format!("Import skill {folder}")));
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
        self.commit(slug, &ChangeNote::new(ChangeSource::Ui, format!("Edit skill {name}")));
        Ok(())
    }

    /// Delete skill `name` (its folder).
    pub fn skill_delete(&mut self, slug: &str, name: &str) -> Result<()> {
        let dir = self.skill_dir(slug, name)?;
        std::fs::remove_dir_all(&dir).map_err(|e| Error::io(IoAction::Removing, &dir, e))?;
        let i = self.index(slug)?;
        self.configs[i].disabled_skills.retain(|n| n != name);
        self.refresh(i);
        self.save_change(slug, &ChangeNote::new(ChangeSource::Ui, format!("Delete skill {name}")))
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
        let what = if enabled { "Switch on" } else { "Switch off" };
        self.save_change(slug, &ChangeNote::new(ChangeSource::Ui, format!("{what} skill {name}")))
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
        self.save_change(&slug, &ChangeNote::new(ChangeSource::Ui, "Create profile"))?;
        Ok(self.configs.last().expect("just pushed"))
    }

    /// Add a configuration with a given slug (migration). No-op if it exists.
    pub fn adopt(&mut self, config: Configuration) -> Result<()> {
        config.validate()?;
        if self.get(&config.slug).is_none() {
            let slug = config.slug.clone();
            self.configs.push(config);
            self.save_change(&slug, &ChangeNote::new(ChangeSource::Import, "Start history"))?;
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
        // One file per server: listed by name, as they are read back.
        config.mcp_servers.sort_by(|a, b| a.name.cmp(&b.name));
        let slug = config.slug.clone();
        self.configs[idx] = config;
        self.save_change(&slug, &ChangeNote::new(ChangeSource::Ui, "Edit profile"))?;
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

    /// Write `configurations.json` (the slugs in order) and every configuration's folder.
    fn save(&self) -> Result<()> {
        #[derive(Serialize)]
        struct Entry<'a> {
            slug: &'a str,
        }
        #[derive(Serialize)]
        struct Out<'a> {
            configurations: Vec<Entry<'a>>,
        }
        for c in &self.configs {
            store_profile(&self.paths, c)?;
        }
        write_json(
            &self.paths.configurations_file(),
            &Out {
                configurations: self.configs.iter().map(|c| Entry { slug: &c.slug }).collect(),
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
    fn folders_edits_and_history() {
        use crate::profile_files::{EditError, EditOp};
        let t = TempDir::new("cfg-folders");
        let paths = Paths::new(&t.0);
        // An older file holding the configuration whole: moved into its folder.
        std::fs::write(
            paths.configurations_file(),
            r#"{"configurations":[{"slug":"web","name":"Web","instructions":"Use pnpm.","mcp_servers":[{"name":"docs","command":"node","args":["d.js"],"env":[["K","v"]]}],"disabled_skills":["gone"],"created_at":7}]}"#,
        )
        .unwrap();
        let mut store = ConfigurationStore::open(&paths).unwrap();
        let dir = paths.configuration_dir("web");
        assert_eq!(std::fs::read_to_string(dir.join("instructions.md")).unwrap(), "Use pnpm.");
        assert!(dir.join("mcp/docs.json").is_file());
        assert!(dir.join(".git").is_dir());
        let index = std::fs::read_to_string(paths.configurations_file()).unwrap();
        assert!(!index.contains("pnpm"), "{index}");
        let reopened = ConfigurationStore::open(&paths).unwrap();
        assert_eq!(reopened.list(), store.list());
        assert_eq!(reopened.get("web").unwrap().created_at, 7);

        // An agent's edit: checked, applied, read back, recorded.
        let note = ChangeNote {
            source: ChangeSource::Agent,
            summary: "Pin pnpm".into(),
            session: Some("s1".into()),
            session_title: None,
        };
        let patch = |old: &str, new: &str| EditOp::Patch {
            path: "instructions.md".into(),
            old: old.into(),
            new: new.into(),
        };
        let changed = store.edit_files("web", &[patch("pnpm", "pnpm 9")], &note).unwrap().unwrap();
        assert_eq!(changed[0].path, "instructions.md");
        assert_eq!(store.get("web").unwrap().instructions, "Use pnpm 9.");
        let stale = store.edit_files("web", &[patch("Use pnpm.", "x")], &note).unwrap();
        assert!(matches!(stale, Err(EditError::NoMatch { .. })));
        store
            .edit_files(
                "web",
                &[
                    EditOp::Write {
                        path: "mcp/db.json".into(),
                        content: r#"{"command":"db"}"#.into(),
                        rev: None,
                    },
                    EditOp::Write {
                        path: "skills/pdf/SKILL.md".into(),
                        content: skill::template("pdf", "PDFs"),
                        rev: None,
                    },
                ],
                &note,
            )
            .unwrap()
            .unwrap();
        let c = store.get("web").unwrap();
        assert_eq!(c.mcp_servers.len(), 2);
        assert_eq!(c.skills.len(), 1);
        assert!(c.disabled_skills.is_empty(), "a missing skill is no longer switched off");

        let log = store.history("web", 10).unwrap();
        assert_eq!(log.len(), 3, "{log:?}");
        assert_eq!(log[0].session.as_deref(), Some("s1"));
        assert_eq!(log[2].source, ChangeSource::Import);

        // A profiles page edit is recorded too; restore goes back to the first change.
        let mut ui = store.get("web").unwrap().clone();
        ui.name = "Web 2".into();
        store.update(ui).unwrap();
        assert_eq!(store.history("web", 10).unwrap()[0].source, ChangeSource::Ui);
        store
            .restore("web", &log[2].id, &ChangeNote::new(ChangeSource::Restore, "Restore"))
            .unwrap();
        let c = store.get("web").unwrap();
        assert_eq!((c.name.as_str(), c.instructions.as_str()), ("Web", "Use pnpm."));
        assert_eq!((c.mcp_servers.len(), c.skills.len()), (1, 0));
        assert!(!dir.join("skills/pdf").exists());
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
