//! Profiles: named sets of agent configuration (own Claude config dir and Codex home).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoAction, Result};
use crate::paths::{Paths, ensure_dir, now_unix, read_json, write_json};
use crate::slug::validate_slug;

/// Name of the profile created on first run.
pub const DEFAULT_PROFILE: &str = "default";

/// A named agent configuration. `name` is a slug (see [`crate::slug::validate_slug`]) and doubles
/// as the directory name under `profiles/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Unique slug.
    pub name: String,
    /// Directory used as `CLAUDE_CONFIG_DIR` (`profiles/<name>/claude`). Written to
    /// `profiles.json` for compatibility, but recomputed from the data dir on load.
    #[serde(default)]
    pub claude_config_dir: PathBuf,
    /// Directory used as `CODEX_HOME` (`profiles/<name>/codex`). Recomputed on load too.
    #[serde(default)]
    pub codex_home: PathBuf,
    /// Creation time, Unix seconds.
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfilesFile {
    profiles: Vec<Profile>,
}

/// Persistent list of profiles backed by `profiles.json`. Every mutation saves immediately.
#[derive(Debug)]
pub struct ProfileStore {
    paths: Paths,
    profiles: Vec<Profile>,
}

impl ProfileStore {
    /// Load `profiles.json` (creating the `default` profile if the file is missing or empty)
    /// and make sure every profile's directories exist.
    ///
    /// The directories stored in the file are ignored: they are recomputed from `paths`
    /// (`profiles/<name>/{claude,codex}`), so a moved or copied data dir keeps working. The
    /// fields are still written for backward compatibility. A stored name that is not a valid
    /// slug is rejected ([`Error::InvalidName`]) since it is used as a directory name.
    pub fn open(paths: &Paths) -> Result<Self> {
        let file: ProfilesFile = read_json(&paths.profiles_file())?.unwrap_or_default();
        let mut store = Self {
            paths: paths.clone(),
            profiles: Vec::with_capacity(file.profiles.len()),
        };
        for p in file.profiles {
            validate_slug(&p.name)?;
            let fresh = store.make_profile(&p.name, p.created_at);
            store.profiles.push(fresh);
        }
        if store.profiles.is_empty() {
            store.create(DEFAULT_PROFILE)?;
        } else {
            for p in &store.profiles {
                ensure_dir(&p.claude_config_dir)?;
                ensure_dir(&p.codex_home)?;
            }
        }
        Ok(store)
    }

    /// All profiles in creation order.
    pub fn list(&self) -> &[Profile] {
        &self.profiles
    }

    /// Look up a profile by name.
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Create a profile and its `claude`/`codex` directories.
    pub fn create(&mut self, name: &str) -> Result<&Profile> {
        validate_slug(name)?;
        if self.get(name).is_some() {
            return Err(Error::ProfileExists(name.to_owned()));
        }
        let profile = self.make_profile(name, now_unix());
        ensure_dir(&profile.claude_config_dir)?;
        ensure_dir(&profile.codex_home)?;
        self.profiles.push(profile);
        self.save()?;
        Ok(self.profiles.last().expect("just pushed"))
    }

    /// Rename a profile, moving its directory. Sessions referencing the old name must be
    /// updated by the caller (see [`crate::session::SessionStore::rename_profile`]); agents
    /// running with the old directories should be stopped first.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<&Profile> {
        validate_slug(new)?;
        let idx = self.index(old)?;
        if old == new {
            return Ok(&self.profiles[idx]);
        }
        if self.get(new).is_some() {
            return Err(Error::ProfileExists(new.to_owned()));
        }
        let from = self.paths.profile_dir(old);
        let to = self.paths.profile_dir(new);
        if from.exists() {
            std::fs::rename(&from, &to).map_err(|e| Error::io(IoAction::Renaming, &from, e))?;
        }
        let renamed = self.make_profile(new, self.profiles[idx].created_at);
        ensure_dir(&renamed.claude_config_dir)?;
        ensure_dir(&renamed.codex_home)?;
        self.profiles[idx] = renamed;
        self.save()?;
        Ok(&self.profiles[idx])
    }

    /// Delete a profile. The last remaining profile cannot be deleted. With `remove_files`, the
    /// profile directory (agent config, credentials, skills) is deleted from disk too.
    pub fn delete(&mut self, name: &str, remove_files: bool) -> Result<Profile> {
        let idx = self.index(name)?;
        if self.profiles.len() == 1 {
            return Err(Error::LastProfile(name.to_owned()));
        }
        let profile = self.profiles.remove(idx);
        self.save()?;
        if remove_files {
            let dir = self.paths.profile_dir(name);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(IoAction::Removing, dir, e)),
            }
        }
        Ok(profile)
    }

    fn index(&self, name: &str) -> Result<usize> {
        self.profiles
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| Error::ProfileNotFound(name.to_owned()))
    }

    fn make_profile(&self, name: &str, created_at: u64) -> Profile {
        let dir = self.paths.profile_dir(name);
        Profile {
            name: name.to_owned(),
            claude_config_dir: dir.join("claude"),
            codex_home: dir.join("codex"),
            created_at,
        }
    }

    fn save(&self) -> Result<()> {
        #[derive(Serialize)]
        struct Out<'a> {
            profiles: &'a [Profile],
        }
        write_json(
            &self.paths.profiles_file(),
            &Out {
                profiles: &self.profiles,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    #[test]
    fn default_profile_on_first_run() {
        let t = TempDir::new("prof-default");
        let paths = Paths::new(&t.0);
        let store = ProfileStore::open(&paths).unwrap();
        assert_eq!(store.list().len(), 1);
        let p = &store.list()[0];
        assert_eq!(p.name, DEFAULT_PROFILE);
        assert!(p.claude_config_dir.is_dir());
        assert!(p.codex_home.is_dir());
        assert_eq!(
            p.claude_config_dir,
            t.0.join("profiles").join("default").join("claude")
        );
        assert!(paths.profiles_file().is_file());
    }

    #[test]
    fn crud_and_persistence() {
        let t = TempDir::new("prof-crud");
        let paths = Paths::new(&t.0);
        let mut store = ProfileStore::open(&paths).unwrap();
        store.create("work").unwrap();
        store.create("personal").unwrap();
        assert!(matches!(store.create("work"), Err(Error::ProfileExists(_))));
        assert!(matches!(
            store.create("Bad Name"),
            Err(Error::InvalidName { .. })
        ));

        std::fs::write(
            store.get("work").unwrap().claude_config_dir.join("marker"),
            "x",
        )
        .unwrap();
        let renamed = store.rename("work", "job").unwrap().clone();
        assert_eq!(renamed.name, "job");
        assert!(
            renamed.claude_config_dir.join("marker").is_file(),
            "dir moved"
        );
        assert!(!paths.profile_dir("work").exists());
        assert!(matches!(
            store.rename("job", "personal"),
            Err(Error::ProfileExists(_))
        ));
        assert!(matches!(
            store.rename("nope", "x"),
            Err(Error::ProfileNotFound(_))
        ));

        store.delete("personal", true).unwrap();
        assert!(!paths.profile_dir("personal").exists());

        let reopened = ProfileStore::open(&paths).unwrap();
        assert_eq!(reopened.list(), store.list());
        let names: Vec<_> = reopened.list().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["default", "job"]);
    }

    #[test]
    fn dirs_recomputed_from_paths_on_load() {
        let t = TempDir::new("prof-moved");
        // A file written by an older build / another data dir location, with stale absolute
        // dirs and without the optional fields being meaningful.
        let paths = Paths::new(t.0.join("moved"));
        std::fs::create_dir_all(paths.root()).unwrap();
        std::fs::write(
            paths.profiles_file(),
            r#"{"profiles":[{"name":"work","claude_config_dir":"Z:/old/profiles/work/claude","codex_home":"Z:/old/profiles/work/codex","created_at":7}]}"#,
        )
        .unwrap();
        let store = ProfileStore::open(&paths).unwrap();
        let p = store.get("work").unwrap();
        assert_eq!(
            p.claude_config_dir,
            paths.profile_dir("work").join("claude")
        );
        assert_eq!(p.codex_home, paths.profile_dir("work").join("codex"));
        assert_eq!(p.created_at, 7);
        assert!(p.claude_config_dir.is_dir() && p.codex_home.is_dir());

        // Still the same file format (fields present) after a save.
        let mut store = store;
        store.create("other").unwrap();
        let json = std::fs::read_to_string(paths.profiles_file()).unwrap();
        assert!(json.contains("\"claude_config_dir\""), "{json}");
        assert!(!json.contains("Z:/old"), "{json}");

        // Names are directory names: a bad one in the file is rejected.
        std::fs::write(
            paths.profiles_file(),
            r#"{"profiles":[{"name":"../evil","claude_config_dir":"","codex_home":"","created_at":0}]}"#,
        )
        .unwrap();
        assert!(matches!(
            ProfileStore::open(&paths),
            Err(Error::InvalidName { .. })
        ));
    }

    #[test]
    fn cannot_delete_last() {
        let t = TempDir::new("prof-last");
        let mut store = ProfileStore::open(&Paths::new(&t.0)).unwrap();
        assert!(matches!(
            store.delete("default", false),
            Err(Error::LastProfile(_))
        ));
        store.create("other").unwrap();
        store.delete("default", false).unwrap();
        assert!(t.0.join("profiles").join("default").is_dir(), "files kept");
        let reopened = ProfileStore::open(&Paths::new(&t.0)).unwrap();
        assert_eq!(reopened.list().len(), 1);
        assert_eq!(reopened.list()[0].name, "other");
    }
}
