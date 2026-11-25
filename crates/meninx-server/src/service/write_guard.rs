//! Write guard (`docs/write-guard.md`): an agent writes freely only inside its session (the
//! session folder, its worktrees on their own branch, the repository's current branch for
//! original-repo sessions, its scratch and account folders). Every other folder needs the
//! user's approval, per folder, through the approval system (`docs/approvals.md`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meninx_core::write_guard::{Need, Scope, ScopeRepo, Write, needs, shell_writes};
use meninx_core::{Session, SessionKind};
use meninx_proto::AgentHookEvent;

use super::{UserCtx, home_dir};

/// The capability of writing a folder outside the session.
pub(crate) const CAP_WRITE: &str = "folder.write";

/// The folder as shown and kept (forward slashes).
fn folder_key(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

impl UserCtx {
    /// Whether `s`'s writes go unguarded because its provider's hooks need an account
    /// integration that is not in place (missing or outdated, or no account to hold it).
    /// Providers that inject their hooks per session, or have none, are not reported here.
    pub(crate) fn unguarded(&self, s: &Session) -> bool {
        use meninx_agents::IntegrationStatus as S;
        if s.kind != SessionKind::Agent {
            return false;
        }
        let Ok(p) = self.provider(&s.provider) else {
            return false;
        };
        let caps = p.hook_caps();
        if !p.capabilities().integration
            || (caps.guard_edits == meninx_agents::Support::Unsupported
                && caps.guard_shell == meninx_agents::Support::Unsupported)
        {
            return false;
        }
        let targets = self.bridge.hook_targets();
        if targets.program.is_none() {
            // No bridge: no hooks for any provider.
            return false;
        }
        let Some(slug) = &s.account else {
            return true;
        };
        let Some(acct) = self.accounts.lock().unwrap().get(&s.provider, slug).cloned() else {
            return false;
        };
        matches!(p.integration_status(&acct, &targets), S::Missing | S::Outdated)
    }

    /// Where `s` writes without asking.
    fn write_scope(&self, s: &Session) -> Scope {
        let repos = self.repos.lock().unwrap().clone();
        let mut free: Vec<PathBuf> = vec![
            s.root.clone(),
            self.paths.session_dir(s.id.as_str()),
            // The agents' own state (memory, plans, settings) lives in their account folders.
            self.paths.root().join("accounts"),
            std::env::temp_dir(),
        ];
        if s.account.is_none()
            && let Ok(p) = self.provider(&s.provider)
        {
            // No account: the CLI keeps its state in its default config home.
            free.extend(p.default_state_dirs());
        }
        let mut scope_repos = Vec::new();
        for w in &s.worktrees {
            if !w.path.starts_with(&s.root) {
                // A legacy worktree outside the session folder.
                free.push(w.path.clone());
            }
            scope_repos.push(ScopeRepo {
                path: w.path.clone(),
                branch: s.is_workspace().then(|| w.branch.clone()),
                repo_path: repos.get(&w.repo).map_or_else(|| w.path.clone(), |r| r.path.clone()),
            });
        }
        Scope {
            free,
            repos: scope_repos,
        }
    }

    /// The writes of a tool call that leave `s`'s scope (blocking: git and the disk).
    fn write_needs(&self, s: &Session, event: &AgentHookEvent) -> Vec<Need> {
        let writes: Vec<Write> = match event {
            AgentHookEvent::PreEdit { paths, .. } => paths
                .iter()
                .map(|p| {
                    let p = Path::new(p);
                    Write::Path(if p.is_absolute() { p.to_path_buf() } else { s.root.join(p) })
                })
                .collect(),
            AgentHookEvent::PreShell { command, cwd, .. } => {
                let cwd = cwd.as_deref().map(PathBuf::from).unwrap_or_else(|| s.root.clone());
                shell_writes(command, Some(&cwd), home_dir().as_deref())
            }
            _ => return Vec::new(),
        };
        if writes.is_empty() {
            return Vec::new();
        }
        needs(&self.write_scope(s), &writes)
    }

    /// The write guard of a tool hook: `Some(reason)` refuses the tool call. Writes outside
    /// the session ask the user per folder (a kept grant on the folder or one above answers).
    pub(crate) async fn write_refusal(self: &Arc<Self>, session: &str, event: &AgentHookEvent) -> Option<String> {
        let s = self.resolve_session(session).ok()?;
        if s.kind != SessionKind::Agent {
            return None;
        }
        let (this, ev, sc) = (self.clone(), event.clone(), s.clone());
        let found = tokio::task::spawn_blocking(move || this.write_needs(&sc, &ev))
            .await
            .unwrap_or_default();
        for n in found {
            if self
                .grants
                .lock()
                .unwrap()
                .allows_folder(CAP_WRITE, session, &n.folder)
            {
                continue;
            }
            let folder = folder_key(&n.folder);
            if let Err(why) = self.approve(session, CAP_WRITE, &folder, &n.what).await {
                self.audit("agent.write_refused", &format!("{session} {folder}"));
                return Some(format!(
                    "Meninx refused this: it writes outside this session ({what}). This session may \
                     write freely only inside its workspace `{root}` and on its own branches; \
                     anything else (another checkout, another branch such as main, another \
                     folder) needs the user's approval for the folder `{folder}`, and {why}. Do \
                     not work around this with other commands or tools: tell the user what you \
                     wanted to do there and why, and let them do it or approve it.",
                    what = n.what.replace('\n', "; "),
                    root = folder_key(&s.root),
                ));
            }
        }
        None
    }
}
