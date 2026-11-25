//! One-time migration of the legacy profile model (`profiles.json`: one Claude config dir and
//! one Codex home per profile) to accounts and configurations.
//!
//! Every profile `p` becomes:
//! - account `claude/p` using `profiles/p/claude` (the existing dir, nothing is copied),
//! - account `codex/p` using `profiles/p/codex`,
//! - configuration `p` (empty: profiles had no skills/MCP/instructions of their own).
//!
//! Sessions without a provider (written before providers existed) get `provider` = their old
//! `agent`, `account` = the migrated account of their profile for that provider (none for the
//! shell) and `configuration` = their profile. `profiles.json` and the profile dirs are left in
//! place. The legacy profile format named exactly these two CLIs, which is why their ids appear
//! here and nowhere else outside `meninx-agents`.

use serde::{Deserialize, Serialize};

use crate::account::AccountStore;
use crate::configuration::{Configuration, ConfigurationStore};
use crate::error::Result;
use crate::paths::{Paths, read_json, write_json};
use crate::session::{SessionStore, SessionTarget};
use crate::slug::validate_slug;

/// Provider ids of the legacy profile dirs: (provider, sub-dir of `profiles/<name>/`).
const LEGACY_DIRS: [(&str, &str); 2] = [("claude", "claude"), ("codex", "codex")];

/// What [`migrate_profiles`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MigrationReport {
    /// Profiles turned into accounts + configurations (0 if already migrated / none).
    pub profiles: usize,
    /// Sessions given a provider/account/configuration.
    pub sessions: usize,
}

#[derive(Debug, Deserialize)]
struct LegacyProfile {
    name: String,
    #[serde(default)]
    created_at: u64,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyProfilesFile {
    #[serde(default)]
    profiles: Vec<LegacyProfile>,
}

/// Migrate `profiles.json` (only if neither `accounts.json` nor `configurations.json` exists
/// yet) and legacy sessions (always; idempotent). Call before opening the stores.
pub fn migrate_profiles(paths: &Paths) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();
    let fresh = !paths.accounts_file().exists() && !paths.configurations_file().exists();
    if fresh && let Some(file) = read_json::<LegacyProfilesFile>(&paths.profiles_file())? {
        let profiles: Vec<LegacyProfile> = file
            .profiles
            .into_iter()
            .filter(|p| validate_slug(&p.name).is_ok())
            .collect();
        if !profiles.is_empty() {
            let mut accounts = AccountStore::open(paths)?;
            for p in &profiles {
                for (provider, sub) in LEGACY_DIRS {
                    let dir = paths.profile_dir(&p.name).join(sub);
                    accounts.adopt(provider, &p.name, &p.name, &dir)?;
                }
            }
            #[derive(Serialize)]
            struct Out {
                configurations: Vec<Configuration>,
            }
            let configurations = profiles
                .iter()
                .map(|p| Configuration {
                    created_at: p.created_at,
                    ..Configuration::empty(&p.name, &p.name)
                })
                .collect();
            write_json(&paths.configurations_file(), &Out { configurations })?;
            report.profiles = profiles.len();
        }
    }

    if paths.sessions_file().exists() {
        let mut sessions = SessionStore::open(paths)?;
        if sessions.list().iter().any(|s| s.provider.is_empty()) {
            let accounts = AccountStore::open(paths)?;
            let configs = ConfigurationStore::open(paths)?;
            let fallback = configs.list()[0].slug.clone();
            report.sessions = sessions.migrate_legacy(|profile, agent| SessionTarget {
                provider: agent.to_owned(),
                account: accounts.get(agent, profile).map(|a| a.slug.clone()),
                configuration: configs
                    .get(profile)
                    .map(|c| c.slug.clone())
                    .unwrap_or_else(|| fallback.clone()),
            })?;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::DEFAULT_CONFIGURATION;
    use crate::paths::test_util::TempDir;
    use crate::session::SessionKind;

    /// A data dir as written by the profile-era build: two profiles (one renamed away from
    /// `default`), their dirs with credentials, and sessions of every agent kind.
    fn legacy_fixture(root: &std::path::Path) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join("profiles.json"),
            r#"{"profiles":[
                {"name":"work","claude_config_dir":"Z:/old/profiles/work/claude","codex_home":"Z:/old/profiles/work/codex","created_at":5},
                {"name":"personal","claude_config_dir":"","codex_home":"","created_at":6}
            ]}"#,
        )
        .unwrap();
        for p in ["work", "personal"] {
            for sub in ["claude", "codex"] {
                let d = root.join("profiles").join(p).join(sub);
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join(".credentials.json"), p).unwrap();
            }
        }
        std::fs::write(
            root.join("sessions.json"),
            r#"{"sessions":[
                {"id":"aaa","title":"c","profile":"work","repo_path":"R","worktree_path":"W1","branch":"meninx/aaa","agent":"claude","created_at":1,"last_active":2},
                {"id":"bbb","title":"x","profile":"personal","repo_path":"R","worktree_path":"W2","branch":"meninx/bbb","agent":"codex","created_at":1,"last_active":2},
                {"id":"ccc","title":"s","profile":"work","repo_path":"R","worktree_path":"W3","branch":"meninx/ccc","agent":"shell","created_at":1,"last_active":2},
                {"id":"ddd","title":"g","profile":"gone","repo_path":"R","worktree_path":"W4","branch":"meninx/ddd","agent":"claude","created_at":1,"last_active":2}
            ]}"#,
        )
        .unwrap();
    }

    #[test]
    fn profiles_become_accounts_and_configurations() {
        let t = TempDir::new("migrate");
        let root = t.0.join("data");
        legacy_fixture(&root);
        let paths = Paths::new(&root);

        let r = migrate_profiles(&paths).unwrap();
        assert_eq!(
            r,
            MigrationReport {
                profiles: 2,
                sessions: 4
            }
        );

        let accounts = AccountStore::open(&paths).unwrap();
        assert_eq!(accounts.list().len(), 4);
        let a = accounts.get("claude", "work").unwrap();
        assert_eq!(a.dir, paths.profile_dir("work").join("claude"), "no copy");
        assert_eq!(a.name, "work");
        assert!(a.dir.join(".credentials.json").is_file());
        assert_eq!(
            accounts.get("codex", "personal").unwrap().dir,
            paths.profile_dir("personal").join("codex")
        );

        let configs = ConfigurationStore::open(&paths).unwrap();
        let slugs: Vec<_> = configs.list().iter().map(|c| c.slug.as_str()).collect();
        assert_eq!(
            slugs,
            ["work", "personal"],
            "no extra default configuration"
        );
        assert_eq!(configs.get("work").unwrap().created_at, 5);

        let sessions = SessionStore::open(&paths).unwrap();
        let by = |id: &str| {
            sessions
                .list()
                .iter()
                .find(|s| s.id.as_str() == id)
                .unwrap()
                .clone()
        };
        let s = by("aaa");
        assert_eq!(
            (
                s.provider.as_str(),
                s.account.as_deref(),
                s.configuration.as_str()
            ),
            ("claude", Some("work"), "work")
        );
        assert_eq!(s.kind, SessionKind::Agent);
        let s = by("bbb");
        assert_eq!(
            (
                s.provider.as_str(),
                s.account.as_deref(),
                s.configuration.as_str()
            ),
            ("codex", Some("personal"), "personal")
        );
        let s = by("ccc");
        assert_eq!((s.provider.as_str(), s.account.as_deref()), ("shell", None));
        // A session of a profile that no longer exists: first configuration, no account.
        let s = by("ddd");
        assert_eq!(
            (
                s.provider.as_str(),
                s.account.as_deref(),
                s.configuration.as_str()
            ),
            ("claude", None, "work")
        );
        let json = std::fs::read_to_string(paths.sessions_file()).unwrap();
        assert!(!json.contains("\"profile\""), "{json}");

        // Idempotent; legacy files are left in place.
        assert_eq!(
            migrate_profiles(&paths).unwrap(),
            MigrationReport::default()
        );
        assert!(paths.profiles_file().is_file());
        assert_eq!(AccountStore::open(&paths).unwrap().list().len(), 4);
    }

    #[test]
    fn fresh_data_dir_gets_default_configuration_only() {
        let t = TempDir::new("migrate-fresh");
        let paths = Paths::new(&t.0);
        assert_eq!(
            migrate_profiles(&paths).unwrap(),
            MigrationReport::default()
        );
        assert!(AccountStore::open(&paths).unwrap().list().is_empty());
        let configs = ConfigurationStore::open(&paths).unwrap();
        assert_eq!(configs.list().len(), 1);
        assert_eq!(configs.list()[0].slug, DEFAULT_CONFIGURATION);
        // A later profiles.json (e.g. an old build ran meanwhile) is not re-imported.
        legacy_fixture(&t.0);
        std::fs::remove_file(t.0.join("sessions.json")).unwrap();
        assert_eq!(migrate_profiles(&paths).unwrap().profiles, 0);
    }
}
