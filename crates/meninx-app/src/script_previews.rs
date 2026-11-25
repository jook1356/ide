//! App script steps for previews (`previews_ui.rs`, `docs/previews.md`): previews the server
//! stored (written here as the daemon writes them: `<data>/sessions/<id>/previews/`) open as
//! tabs when the agent's `meninx_preview` arrives as the server hands it over; the HTML page (real
//! CEF, served through `meninx-preview://`) has no `window.meninx`, loads files of the session folder,
//! is refused files outside it, other origins and writes; the same id reloads the tab; a
//! Markdown file is rendered; `close` closes the tab. Screenshots `preview_*` (Korean
//! `ko_preview_*`). A child module of `script.rs`: it uses the script's helpers.

use super::*;
use crate::previews_ui::console_lines;

#[derive(Debug, Clone)]
pub(super) enum PStep {
    /// Write the previews (records and pages) and the session folder's files.
    WriteFixture,
    /// The agent's `meninx_preview` (`show` of a fixture preview / `close`) as the server hands it over.
    Agent(&'static str, &'static str),
    /// This many preview tabs of the P2 session are open, one of them showing this id (or none).
    WaitTabs(usize, Option<&'static str>, u64),
    /// The HTML page logged `PREVIEW DONE`; then its lines are checked.
    WaitPage(u64),
    /// The Markdown page's script logged its heading.
    WaitMarkdown(u64),
    /// Forget the pages' lines.
    ClearConsole,
}

fn p(x: PStep) -> Step {
    Step::Preview(x)
}

const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>page</title><link rel="stylesheet" href="preview-assets/a.css"></head>
<body><h1>Preview page</h1><ul id="out"></ul>
<script type="module">
const out = (t) => {
  const li = document.createElement('li');
  li.textContent = t;
  document.getElementById('out').append(li);
  console.log('PREVIEW ' + t);
};
try {
  out('Meninx ' + typeof window.meninx);
  out('css ' + (await fetch('preview-assets/a.css').then((r) => r.text())).length);
  out('outside ' + (await fetch('/..%5C..%5Csecret.txt').then((r) => r.status)));
  out('post ' + (await fetch('preview-assets/a.css', { method: 'POST', body: 'x' }).then((r) => r.status)));
  const session = location.hostname.slice(location.hostname.indexOf('.') + 1);
  out('other ' + (await fetch('meninx-preview://other.' + session + '/').then((r) => r.status, () => 'blocked')));
  out('DONE');
} catch (e) { out('FAIL ' + e.message); }
</script></body></html>
"#;

const CSS: &str = "body { background: #1e1f22; color: #d4d6db; font: 14px sans-serif; }\n";

const MARKDOWN: &str = "# Release notes\n\n| Item | State |\n|---|---|\n| Previews | done |\n\n- [x] html\n- [x] markdown\n- [ ] more\n\n```rust\nfn main() {}\n```\n\n<script>console.log('PREVIEW MD ' + document.querySelector('h1').textContent)</script>\n";

/// Run the preview steps after the widget steps (before the server stop).
pub(super) fn splice(steps: &mut Vec<Step>) {
    let at = steps.iter().position(
        |s| matches!(s, Step::Log(t) if t.starts_with("--- stop the server from the settings page")),
    );
    if let Some(at) = at {
        steps.splice(at..at, self::steps());
    }
}

/// `MENINX_APP_SCRIPT_ONLY=preview`: startup, these steps and the server stop.
pub(super) fn steps_only() -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup (previews only)"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        WaitReady(30_000),
        MarkDaemon,
        EnsureRepo,
        GoPage(Page::Repos),
        RegisterTyped(Repo::Test),
        WaitRepoRegistered(Repo::Test, 15_000),
        GoPage(Page::Work),
    ];
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
    use Step::{FillNew, Log, MarkP2, Pause, SetLang, Shot, Submit, Wait, WaitCreated, WaitText};
    vec![
        Log("--- previews: html page, reload, markdown file, close"),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "previews",
        },
        Submit,
        WaitCreated(30_000),
        MarkP2,
        WaitText(P2, ">", 15_000),
        p(PStep::WriteFixture),
        p(PStep::ClearConsole),
        p(PStep::Agent("show", "page")),
        p(PStep::WaitTabs(1, Some("page"), 10_000)),
        p(PStep::WaitPage(30_000)),
        Wait(500),
        Shot("preview_01_page"),
        // The same id: the tab reloads, no second tab.
        p(PStep::ClearConsole),
        p(PStep::Agent("show", "page")),
        p(PStep::WaitPage(30_000)),
        p(PStep::WaitTabs(1, Some("page"), 5_000)),
        // A Markdown file of the session folder, rendered.
        p(PStep::Agent("show", "notes")),
        p(PStep::WaitTabs(2, Some("notes"), 10_000)),
        p(PStep::WaitMarkdown(30_000)),
        Wait(500),
        Shot("preview_02_markdown"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(300),
        Shot("ko_preview_02_markdown"),
        Pause("ko_preview_02_markdown"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        p(PStep::Agent("close", "page")),
        p(PStep::WaitTabs(1, None, 5_000)),
        p(PStep::Agent("close", "notes")),
        p(PStep::WaitTabs(0, None, 5_000)),
    ]
}

/// Open preview tabs of the session and whether one shows `id`.
fn tabs(app: &App, sid: &str, id: Option<&str>) -> (usize, bool) {
    let all: Vec<&crate::previews_ui::PreviewTab> = app
        .sessions
        .iter()
        .flat_map(|l| l.browsers.iter())
        .filter_map(|b| b.preview.as_ref())
        .filter(|p| p.session == sid)
        .collect();
    (all.len(), id.is_none_or(|id| all.iter().any(|p| p.id == id)))
}

pub(super) fn run(sc: &mut Script, app: &mut App, st: PStep, el: Duration) -> (bool, Task<Message>) {
    let timeout = |ms: u64| el > Duration::from_millis(ms);
    let mut task = Task::none();
    macro_rules! wait {
        ($ok:expr, $ms:expr, $what:expr) => {{
            if $ok {
                sc.check(true, &$what);
                true
            } else if timeout($ms) {
                sc.check(false, &format!("timeout: {}", $what));
                true
            } else {
                false
            }
        }};
    }
    let sid = sc.id(P2).unwrap_or_default();
    let next = match st {
        PStep::WriteFixture => {
            let root = app.live(&sid).map(|l| PathBuf::from(&l.info.root)).unwrap_or_default();
            let dir = app.data_dir.join("sessions").join(&sid).join("previews");
            let notes = root.join("PREVIEW-notes.md");
            let r = (|| -> std::io::Result<()> {
                std::fs::create_dir_all(root.join("preview-assets"))?;
                std::fs::create_dir_all(&dir)?;
                std::fs::write(root.join("preview-assets").join("a.css"), CSS)?;
                std::fs::write(&notes, MARKDOWN)?;
                // What the daemon writes for `meninx_preview` (`meninx_core::preview`).
                std::fs::write(dir.join("page.html"), PAGE)?;
                let rec = |v: serde_json::Value| serde_json::to_vec_pretty(&v).expect("json");
                std::fs::write(
                    dir.join("page.json"),
                    rec(serde_json::json!({"id": "page", "title": "Preview page", "kind": "html", "updated": 1})),
                )?;
                let file = std::fs::canonicalize(&notes)?;
                std::fs::write(
                    dir.join("notes.json"),
                    rec(serde_json::json!({"id": "notes", "title": "Release notes", "kind": "file", "file": file, "updated": 1})),
                )?;
                Ok(())
            })();
            sc.check(r.is_ok(), &format!("previews written to {} ({r:?})", dir.display()));
            true
        }
        PStep::Agent(action, id) => {
            let title = if id == "page" { "Preview page" } else { "Release notes" };
            let kind = if id == "page" { "html" } else { "file" };
            let args = serde_json::json!({
                "action": action,
                "preview": id,
                "title": title,
                "kind": kind,
                "url": crate::previews_ui::preview_url(&sid, id),
            });
            // As the server hands it over (the reply goes to an unknown request: ignored).
            task = app.on_agent_request(u64::MAX, sid.clone(), "meninx_preview".into(), args.to_string());
            true
        }
        PStep::WaitTabs(n, id, ms) => {
            let (got, shows) = tabs(app, &sid, id);
            wait!(
                got == n && shows,
                ms,
                format!("preview tabs: {got} (want {n}), showing {id:?}: {shows}")
            )
        }
        PStep::WaitPage(ms) => {
            let lines = console_lines();
            let done = lines.iter().any(|l| l == "PREVIEW DONE" || l.starts_with("PREVIEW FAIL"));
            if done {
                let has = |t: &str| lines.iter().any(|l| l == t);
                let css = lines.iter().any(|l| l.starts_with("PREVIEW css ") && l != "PREVIEW css 0");
                sc.check(has("PREVIEW Meninx undefined"), "a preview page gets no window.meninx");
                sc.check(css, "files of the session folder are served");
                sc.check(has("PREVIEW outside 404"), "files outside the session folder are not served");
                sc.check(has("PREVIEW post 405"), "a preview is read only");
                sc.check(
                    lines.iter().any(|l| l == "PREVIEW other blocked" || l == "PREVIEW other 403"),
                    "another preview's origin is not reachable",
                );
                sc.check(has("PREVIEW DONE"), &format!("the page finished: {lines:?}"));
                true
            } else {
                wait!(false, ms, format!("the preview page never finished: {lines:?}"))
            }
        }
        PStep::WaitMarkdown(ms) => {
            let lines = console_lines();
            wait!(
                lines.iter().any(|l| l == "PREVIEW MD Release notes"),
                ms,
                format!("the Markdown file is rendered: {lines:?}")
            )
        }
        PStep::ClearConsole => {
            crate::previews_ui::clear_console();
            true
        }
    };
    (next, task)
}
