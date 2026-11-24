//! App script steps for the new-session dialog with many repositories
//! (`new_session_repos.rs`): twelve scratch repos are registered; the dialog lists them most
//! recently used first, gets a search box and chips, keeps its buttons inside a short window and
//! scrolls its fields instead. Screenshots `newrepos_*` (Korean `ko_newrepos_*`).
//! `IDE_APP_SCRIPT_ONLY=newrepos` runs startup, these steps and the server stop.
//! A child module of `script.rs`: it uses the script's helpers.

use super::*;
use crate::new_session_repos::{self, SEARCH_MIN_REPOS};

/// Folder names of the scratch repos (`Repo::Many(i)`, under `<data>/many-repos`).
pub(super) const NAMES: [&str; 12] = [
    "web-app",
    "api-server",
    "mobile",
    "design-system",
    "infra",
    "docs-site",
    "billing",
    "auth-service",
    "analytics",
    "cli-tools",
    "sdk-js",
    "sdk-python",
];

/// The repo a session is created with, so it is listed first afterwards.
const RECENT: u8 = 7;

#[derive(Debug, Clone)]
pub(super) enum NStep {
    /// Create the scratch repos on disk (one commit on `main` each).
    MakeRepos,
    /// The new-session dialog is open with every scratch repo, search shown.
    ExpectMany,
    /// The first listed repo is this scratch repo (`None`: registry order, no session yet).
    ExpectFirst(Option<u8>),
    /// Check these scratch repos (chips) in the open dialog.
    Check(&'static [u8]),
    /// The search box filter matches exactly these scratch repos.
    Filter(&'static str, &'static [u8]),
    /// Wait until the session list knows a session with this scratch repo (worktrees are
    /// attached in the background after the dialog closes).
    WaitUsed(u8, u64),
    /// Resize the main window.
    Resize(f32, f32),
    /// The main window has this height (± 2 px).
    WaitHeight(f32, u64),
}

fn n(x: NStep) -> Step {
    Step::NewRepos(x)
}

/// `IDE_APP_SCRIPT_ONLY=newrepos`: startup, these steps and the server stop.
pub(super) fn steps_only() -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup (new-session dialog with many repos)"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        WaitReady(30_000),
        MarkDaemon,
        n(NStep::MakeRepos),
        GoPage(Page::Repos),
    ];
    for i in 0..NAMES.len() as u8 {
        s.push(RegisterTyped(Repo::Many(i)));
        // One at a time takes a few seconds each (base detection), sometimes more.
        s.push(WaitRepoRegistered(Repo::Many(i), 90_000));
    }
    s.push(GoPage(Page::Work));
    s.extend(steps());
    s.extend([
        GoPage(Page::Settings),
        AskStopServer,
        ConfirmStopServer,
        WaitDisconnected(15_000),
        WaitDaemonGone(10_000),
        Exit,
    ]);
    s
}

fn steps() -> Vec<Step> {
    use Step::{App as A, FillNew, Log, Pause, SetLang, Shot, Submit, Wait, WaitCreated};
    vec![
        Log("--- new-session dialog: many repos, tall window"),
        n(NStep::Resize(1400.0, 900.0)),
        n(NStep::WaitHeight(900.0, 5_000)),
        A(Message::OpenNew),
        Wait(300),
        n(NStep::ExpectMany),
        n(NStep::ExpectFirst(None)),
        n(NStep::Check(&[1, 4, 9])),
        Wait(300),
        Shot("newrepos_tall"),
        Pause("newrepos_tall"),
        n(NStep::Filter("sdk", &[10, 11])),
        Wait(300),
        Shot("newrepos_filter"),
        Pause("newrepos_filter"),
        n(NStep::Filter("zzz", &[])),
        Wait(300),
        Shot("newrepos_filter_none"),
        n(NStep::Filter("", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])),
        Log("--- new-session dialog: short window keeps the buttons"),
        n(NStep::Resize(1100.0, 560.0)),
        n(NStep::WaitHeight(560.0, 5_000)),
        Wait(500),
        Shot("newrepos_short"),
        Pause("newrepos_short"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(300),
        Shot("ko_newrepos_short"),
        Pause("ko_newrepos_short"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        A(Message::CloseModal),
        n(NStep::Resize(1400.0, 900.0)),
        n(NStep::WaitHeight(900.0, 5_000)),
        Log("--- new-session dialog: the last used repo comes first"),
        FillNew {
            repo: Repo::Many(RECENT),
            provider: "shell",
            title: "recent repo",
        },
        Submit,
        WaitCreated(90_000),
        n(NStep::WaitUsed(RECENT, 90_000)),
        A(Message::OpenNew),
        Wait(300),
        n(NStep::ExpectFirst(Some(RECENT))),
        Shot("newrepos_recent"),
        A(Message::CloseModal),
    ]
}

/// Ids of the scratch repos, by index (`None` where one is not registered).
fn ids(sc: &Script, app: &App, which: &[u8]) -> Vec<Option<String>> {
    which.iter().map(|&i| registered(sc, app, Repo::Many(i))).collect()
}

fn make_repos(root: &Path) -> Result<(), String> {
    for name in NAMES {
        let repo = root.join(name);
        if repo.join(".git").exists() {
            continue;
        }
        std::fs::create_dir_all(&repo).map_err(|e| e.to_string())?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        std::fs::write(repo.join("README.md"), format!("# {name}\n")).map_err(|e| e.to_string())?;
        git(&repo, &["add", "."])?;
        git(&repo, &["commit", "-q", "-m", "init"])?;
    }
    Ok(())
}

pub(super) fn run(sc: &mut Script, app: &mut App, st: NStep, el: Duration) -> (bool, Task<Message>) {
    let timeout = |ms: u64| el > Duration::from_millis(ms);
    let mut task = Task::none();
    let next = match st {
        NStep::MakeRepos => {
            let root = sc.data.join("many-repos");
            let r = make_repos(&root);
            sc.check(r.is_ok(), &format!("{} scratch repos under {} {r:?}", NAMES.len(), root.display()));
            true
        }
        NStep::ExpectMany => {
            let open = matches!(app.modal, Some(Modal::NewSession(_)));
            let all = ids(sc, app, &(0..NAMES.len() as u8).collect::<Vec<_>>());
            sc.check(
                open && all.iter().all(Option::is_some) && app.repos.len() >= SEARCH_MIN_REPOS,
                &format!(
                    "new-session dialog open with {} repos (search from {SEARCH_MIN_REPOS})",
                    app.repos.len()
                ),
            );
            true
        }
        NStep::ExpectFirst(want) => {
            let first = app.repos_by_recent().first().map(|r| r.id.clone());
            let want_id = match want {
                Some(i) => registered(sc, app, Repo::Many(i)),
                None => app.repos.first().map(|r| r.id.clone()),
            };
            sc.check(
                first.is_some() && first == want_id,
                &format!("first listed repo {first:?}, expected {want:?} ({want_id:?})"),
            );
            true
        }
        NStep::Check(which) => {
            let want: Vec<String> = ids(sc, app, which).into_iter().flatten().collect();
            let mut tasks = Vec::new();
            for id in app.repos.iter().map(|r| r.id.clone()).collect::<Vec<_>>() {
                tasks.push(app.update(Message::NewRepoChecked(id.clone(), want.contains(&id))));
            }
            task = Task::batch(tasks);
            let checked = match &app.modal {
                Some(Modal::NewSession(f)) => f.repos.clone(),
                _ => Vec::new(),
            };
            let mut sorted = want.clone();
            let order: Vec<&String> = app.repos.iter().map(|r| &r.id).collect();
            sorted.sort_by_key(|id| order.iter().position(|o| *o == id));
            sc.check(
                checked == sorted,
                &format!("checked {checked:?} (chips, registry order {sorted:?})"),
            );
            true
        }
        NStep::Filter(q, which) => {
            task = app.update(Message::NewRepoFilter(q.into()));
            let filter = match &app.modal {
                Some(Modal::NewSession(f)) => f.repo_filter.clone(),
                _ => String::new(),
            };
            let shown: Vec<String> = app
                .repos_by_recent()
                .into_iter()
                .filter(|r| new_session_repos::matches(&filter, r))
                .filter(|r| r.path.contains("many-repos"))
                .map(|r| r.id.clone())
                .collect();
            let mut want: Vec<String> = ids(sc, app, which).into_iter().flatten().collect();
            let mut got = shown.clone();
            want.sort();
            got.sort();
            sc.check(
                filter == q && got == want,
                &format!("search {q:?} lists {} scratch repos (expected {})", got.len(), which.len()),
            );
            true
        }
        NStep::Resize(w, h) => {
            if let Some(id) = app.window {
                log(&format!("resize the main window to {w}x{h}"));
                task = window::resize(id, iced::Size::new(w, h));
            }
            true
        }
        NStep::WaitUsed(i, ms) => {
            let id = registered(sc, app, Repo::Many(i));
            let used = id.as_ref().is_some_and(|id| {
                app.sessions.iter().any(|l| l.info.repos.iter().any(|r| &r.repo == id))
            });
            if used {
                sc.check(true, &format!("a session has Many({i}) ({id:?})"));
                true
            } else if timeout(ms) {
                sc.check(false, &format!("no session has Many({i}) ({id:?})"));
                true
            } else {
                false
            }
        }
        NStep::WaitHeight(h, ms) => {
            let now = app.window_size.height;
            if (now - h).abs() <= 2.0 {
                sc.check(true, &format!("main window height {now}"));
                true
            } else if timeout(ms) {
                sc.check(false, &format!("main window height {now}, expected {h}"));
                true
            } else {
                false
            }
        }
    };
    (next, task)
}
