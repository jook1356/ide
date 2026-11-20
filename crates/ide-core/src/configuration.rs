//! Configurations: provider-agnostic bundles of skills, MCP servers and instructions that are
//! layered on top of any account per session (they replace the old "profiles").

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::account::clean_name;
use crate::error::{Error, Result};
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
    /// Skills to offer the agent.
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    /// MCP servers to add to the agent.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// Extra instructions appended to the agent's system/developer prompt.
    #[serde(default)]
    pub instructions: String,
    /// Creation time, Unix seconds.
    #[serde(default)]
    pub created_at: u64,
}

impl Configuration {
    /// An empty configuration.
    pub fn empty(slug: &str, name: &str) -> Self {
        Configuration {
            slug: slug.to_owned(),
            name: name.to_owned(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            instructions: String::new(),
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
        if store.configs.is_empty() {
            store.configs.push(Configuration::empty(
                DEFAULT_CONFIGURATION,
                DEFAULT_CONFIGURATION,
            ));
            store.save()?;
        }
        Ok(store)
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
        self.configs[idx] = config;
        self.save()?;
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

    fn mcp(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            command: "node".into(),
            args: vec!["s.js".into()],
            env: vec![("K".into(), "v".into())],
        }
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
        edited.skills = vec![SkillRef {
            path: t.0.join("skills").join("lint"),
        }];
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
