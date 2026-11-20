//! Accounts: one login of one agent provider. An account is only a directory that the
//! provider's CLI uses as its config home (e.g. `CLAUDE_CONFIG_DIR`, `CODEX_HOME`); the IDE never
//! writes into it (only the CLI's own login flow does).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoAction, NameRule, Result};
use crate::paths::{Paths, ensure_dir, now_unix, read_json, write_json};
use crate::slug::{slugify, validate_slug};

/// One login of one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Provider id (`"claude"`, `"codex"`, a declarative provider's id).
    pub provider: String,
    /// Unique per provider; directory name of new accounts.
    pub slug: String,
    /// Display name (free text).
    pub name: String,
    /// The CLI's config home for this login (absolute).
    pub dir: PathBuf,
    /// Creation time, Unix seconds.
    pub created_at: u64,
}

/// On-disk form: `dir` is relative to the data root when it lies inside it (so a moved data
/// dir keeps working), absolute otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountRecord {
    provider: String,
    slug: String,
    name: String,
    dir: String,
    #[serde(default)]
    created_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccountsFile {
    #[serde(default)]
    accounts: Vec<AccountRecord>,
}

/// Persistent account list backed by `accounts.json`. Every mutation saves immediately.
#[derive(Debug)]
pub struct AccountStore {
    paths: Paths,
    accounts: Vec<Account>,
}

impl AccountStore {
    /// Load `accounts.json` (empty if missing). Stored slugs and provider ids must be valid
    /// slugs ([`Error::InvalidName`]): they are directory names.
    pub fn open(paths: &Paths) -> Result<Self> {
        let file: AccountsFile = read_json(&paths.accounts_file())?.unwrap_or_default();
        let mut accounts = Vec::with_capacity(file.accounts.len());
        for r in file.accounts {
            validate_slug(&r.provider)?;
            validate_slug(&r.slug)?;
            accounts.push(Account {
                dir: resolve_dir(paths.root(), &r.dir),
                provider: r.provider,
                slug: r.slug,
                name: r.name,
                created_at: r.created_at,
            });
        }
        Ok(Self {
            paths: paths.clone(),
            accounts,
        })
    }

    /// All accounts in creation order.
    pub fn list(&self) -> &[Account] {
        &self.accounts
    }

    /// Accounts of one provider.
    pub fn of_provider<'a>(&'a self, provider: &'a str) -> impl Iterator<Item = &'a Account> {
        self.accounts.iter().filter(move |a| a.provider == provider)
    }

    /// Look up an account.
    pub fn get(&self, provider: &str, slug: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.provider == provider && a.slug == slug)
    }

    /// Look up an account or fail with [`Error::AccountNotFound`].
    pub fn require(&self, provider: &str, slug: &str) -> Result<&Account> {
        self.get(provider, slug)
            .ok_or_else(|| Error::AccountNotFound {
                provider: provider.to_owned(),
                slug: slug.to_owned(),
            })
    }

    /// Create an account named `name` for `provider`: the slug is derived from the name (made
    /// unique per provider) and `accounts/<provider>/<slug>/` is created (empty: the CLI's
    /// login fills it).
    pub fn create(&mut self, provider: &str, name: &str) -> Result<&Account> {
        validate_slug(provider)?;
        let name = clean_name(name)?;
        let base = slugify(&name, provider);
        let mut slug = base.clone();
        let mut n = 2;
        while self.get(provider, &slug).is_some()
            || self.paths.account_dir(provider, &slug).exists()
        {
            slug = format!("{base}-{n}");
            n += 1;
        }
        let dir = self.paths.account_dir(provider, &slug);
        ensure_dir(&dir)?;
        self.accounts.push(Account {
            provider: provider.to_owned(),
            slug,
            name,
            dir,
            created_at: now_unix(),
        });
        self.save()?;
        Ok(self.accounts.last().expect("just pushed"))
    }

    /// Add an account that uses an existing directory (migration). No-op if the provider
    /// already has an account with this slug.
    pub fn adopt(&mut self, provider: &str, slug: &str, name: &str, dir: &Path) -> Result<()> {
        validate_slug(provider)?;
        validate_slug(slug)?;
        if self.get(provider, slug).is_some() {
            return Ok(());
        }
        self.accounts.push(Account {
            provider: provider.to_owned(),
            slug: slug.to_owned(),
            name: name.to_owned(),
            dir: dir.to_path_buf(),
            created_at: now_unix(),
        });
        self.save()
    }

    /// Change an account's display name (slug and directory stay).
    pub fn rename(&mut self, provider: &str, slug: &str, name: &str) -> Result<&Account> {
        let name = clean_name(name)?;
        let idx = self.index(provider, slug)?;
        self.accounts[idx].name = name;
        self.save()?;
        Ok(&self.accounts[idx])
    }

    /// Remove an account from the list. With `remove_files`, its directory (credentials,
    /// CLI history) is deleted too, but only if it lies inside the data dir.
    pub fn delete(&mut self, provider: &str, slug: &str, remove_files: bool) -> Result<Account> {
        let idx = self.index(provider, slug)?;
        let acct = self.accounts.remove(idx);
        self.save()?;
        if remove_files && is_inside(&acct.dir, self.paths.root()) {
            match std::fs::remove_dir_all(&acct.dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(IoAction::Removing, &acct.dir, e)),
            }
        }
        Ok(acct)
    }

    fn index(&self, provider: &str, slug: &str) -> Result<usize> {
        self.accounts
            .iter()
            .position(|a| a.provider == provider && a.slug == slug)
            .ok_or_else(|| Error::AccountNotFound {
                provider: provider.to_owned(),
                slug: slug.to_owned(),
            })
    }

    fn save(&self) -> Result<()> {
        let file = AccountsFile {
            accounts: self
                .accounts
                .iter()
                .map(|a| AccountRecord {
                    provider: a.provider.clone(),
                    slug: a.slug.clone(),
                    name: a.name.clone(),
                    dir: store_dir(self.paths.root(), &a.dir),
                    created_at: a.created_at,
                })
                .collect(),
        };
        write_json(&self.paths.accounts_file(), &file)
    }
}

/// A display name: trimmed, non-empty, at most 100 chars, no control characters.
pub(crate) fn clean_name(name: &str) -> Result<String> {
    let n = name.trim();
    let bad = |reason| {
        Err(Error::InvalidName {
            name: name.to_owned(),
            reason,
        })
    };
    if n.is_empty() {
        return bad(NameRule::Empty);
    }
    if n.chars().count() > 100 {
        return bad(NameRule::TooLong);
    }
    if n.chars().any(char::is_control) {
        return bad(NameRule::InvalidChars);
    }
    Ok(n.to_owned())
}

fn resolve_dir(root: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        stored
            .split('/')
            .fold(root.to_path_buf(), |acc, c| acc.join(c))
    }
}

fn store_dir(root: &Path, dir: &Path) -> String {
    match dir.strip_prefix(root) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => dir.display().to_string(),
    }
}

fn is_inside(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir) && path != dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    #[test]
    fn crud_and_persistence() {
        let t = TempDir::new("acct");
        let paths = Paths::new(&t.0);
        let mut store = AccountStore::open(&paths).unwrap();
        assert!(store.list().is_empty());

        let a = store.create("claude", "Work Account").unwrap().clone();
        assert_eq!(
            (a.slug.as_str(), a.name.as_str()),
            ("work-account", "Work Account")
        );
        assert_eq!(a.dir, paths.account_dir("claude", "work-account"));
        assert!(a.dir.is_dir());
        // Same name again: unique slug.
        let b = store.create("claude", "work account").unwrap().clone();
        assert_eq!(b.slug, "work-account-2");
        // Same slug for another provider is fine; non-ASCII names fall back to the provider id.
        let c = store.create("codex", "개인").unwrap().clone();
        assert_eq!((c.slug.as_str(), c.name.as_str()), ("codex", "개인"));
        assert!(matches!(
            store.create("claude", "  "),
            Err(Error::InvalidName { .. })
        ));
        assert!(matches!(
            store.create("Bad Provider", "x"),
            Err(Error::InvalidName { .. })
        ));

        store.rename("claude", "work-account", "Job").unwrap();
        assert!(matches!(
            store.rename("claude", "nope", "x"),
            Err(Error::AccountNotFound { .. })
        ));
        std::fs::write(b.dir.join("marker"), "x").unwrap();
        store.delete("claude", "work-account-2", true).unwrap();
        assert!(!b.dir.exists());

        let reopened = AccountStore::open(&paths).unwrap();
        assert_eq!(reopened.list(), store.list());
        assert_eq!(reopened.get("claude", "work-account").unwrap().name, "Job");
        assert_eq!(reopened.of_provider("codex").count(), 1);
        // Dirs inside the data root are stored relative.
        let json = std::fs::read_to_string(paths.accounts_file()).unwrap();
        assert!(json.contains("\"accounts/claude/work-account\""), "{json}");
    }

    #[test]
    fn adopt_keeps_dir_and_never_deletes_outside() {
        let t = TempDir::new("acct-adopt");
        let paths = Paths::new(t.0.join("data"));
        let outside = t.0.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let mut store = AccountStore::open(&paths).unwrap();
        store.adopt("codex", "home", "home", &outside).unwrap();
        store.adopt("codex", "home", "dup", &outside).unwrap();
        assert_eq!(store.list().len(), 1);
        let reopened = AccountStore::open(&paths).unwrap();
        assert_eq!(reopened.get("codex", "home").unwrap().dir, outside);
        let mut store = reopened;
        store.delete("codex", "home", true).unwrap();
        assert!(
            outside.is_dir(),
            "dirs outside the data dir are never removed"
        );
    }
}
