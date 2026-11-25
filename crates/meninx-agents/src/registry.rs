//! [`Providers`]: the built-in providers plus declarative ones from `providers/*.toml`.

use std::path::Path;
use std::sync::Arc;

use meninx_core::{Error, Result};

use crate::{AgentProvider, Claude, Codex, DeclarativeProvider, Shell};

/// The provider registry.
#[derive(Clone)]
pub struct Providers {
    list: Vec<Arc<dyn AgentProvider>>,
    errors: Vec<String>,
}

impl std::fmt::Debug for Providers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Providers")
            .field("ids", &self.list.iter().map(|p| p.id()).collect::<Vec<_>>())
            .field("errors", &self.errors)
            .finish()
    }
}

impl Providers {
    /// Claude Code, Codex, Shell.
    pub fn builtin() -> Self {
        Providers {
            list: vec![Arc::new(Claude), Arc::new(Codex), Arc::new(Shell)],
            errors: Vec::new(),
        }
    }

    /// Built-ins plus every valid `*.toml` in `dir` (sorted by file name). Invalid files and
    /// ids that clash with an existing provider are skipped and listed in
    /// [`Providers::load_errors`].
    pub fn load(dir: &Path) -> Self {
        let mut me = Self::builtin();
        let mut files: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();
        for f in files {
            match DeclarativeProvider::from_file(&f) {
                Ok(p) if me.get(p.id()).is_some() => me.errors.push(format!(
                    "{}: provider id {:?} already exists",
                    f.display(),
                    p.id()
                )),
                Ok(p) => me.list.push(Arc::new(p)),
                Err(e) => me.errors.push(format!("{}: {e}", f.display())),
            }
        }
        for e in &me.errors {
            tracing::warn!("provider file skipped: {e}");
        }
        me
    }

    /// Add a provider (tests, embedders). Replaces one with the same id.
    pub fn add(&mut self, p: Arc<dyn AgentProvider>) {
        self.list.retain(|x| x.id() != p.id());
        self.list.push(p);
    }

    /// All providers (built-ins first).
    pub fn list(&self) -> &[Arc<dyn AgentProvider>] {
        &self.list
    }

    /// Look up a provider.
    pub fn get(&self, id: &str) -> Option<&dyn AgentProvider> {
        self.list.iter().find(|p| p.id() == id).map(|p| p.as_ref())
    }

    /// Look up a provider or fail with [`Error::ProviderNotFound`].
    pub fn require(&self, id: &str) -> Result<&dyn AgentProvider> {
        self.get(id)
            .ok_or_else(|| Error::ProviderNotFound(id.to_owned()))
    }

    /// Problems with provider files.
    pub fn load_errors(&self) -> &[String] {
        &self.errors
    }
}
