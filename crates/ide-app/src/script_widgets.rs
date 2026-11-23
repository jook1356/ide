//! App script steps for widgets (`widgets_ui.rs`, `docs/widgets.md`): a widget written into
//! the session folder opens from "+ 새로 열기" after the tool approval dialog; its page (real
//! CEF, served through `ide-widget://`) gets `window.ide`, calls a declared tool, keeps data,
//! is refused an undeclared tool and files outside the widget; the agent's `open` reloads the
//! tab and `close` closes it. Screenshots `widget_*` (Korean dialog `ko_widget_01_grant`). A
//! child module of `script.rs`: it uses the script's helpers.

use super::*;
use crate::widgets_ui::{WidgetMsg, console_lines};

#[derive(Debug, Clone)]
pub(super) enum WStep {
    /// Write the demo widget into the P2 session's folder.
    WriteFixture,
    /// The "+ 새로 열기" menu lists the demo widget (after opening the menu).
    WaitListed(u64),
    /// Open it as the menu does.
    Open,
    /// The approval dialog shows the demo widget's tools.
    WaitGrant(u64),
    /// One widget tab of the demo widget is open (`true`) or none.
    WaitTab(bool, u64),
    /// The page logged `WIDGET DONE`; then its lines are checked.
    WaitPage(u64),
    /// Forget the page's lines.
    ClearConsole,
    /// The agent's `ide_widget` (`open` / `close`) as the server hands it over.
    Agent(&'static str),
}

fn w(x: WStep) -> Step {
    Step::Widget(x)
}

const WIDGET: &str = "demo";

const MANIFEST: &str = r#"{
  "name": "Demo widget",
  "description": "Script check of window.ide",
  "tools": ["ide_session"]
}
"#;

const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>demo</title><link rel="stylesheet" href="style.css"></head>
<body><h1>Demo widget</h1><ul id="out"></ul>
<script type="module">
const out = (t) => {
  const li = document.createElement('li');
  li.textContent = t;
  document.getElementById('out').append(li);
  console.log('WIDGET ' + t);
};
try {
  out('ide ' + typeof window.ide + ' ' + ide.widget.id);
  const s = await ide.call('ide_session', {});
  out('session ' + s.id);
  await ide.data.set('count', { n: 41 });
  out('data ' + (await ide.data.get('count')).n);
  out('keys ' + (await ide.data.keys()).join(','));
  try { await ide.call('ide_show_message', { message: 'x' }); out('undeclared allowed'); }
  catch (e) { out('undeclared refused'); }
  out('css ' + (await fetch('./style.css').then((r) => r.text())).length);
  out('outside ' + (await fetch('/..%5C..%5C..%5Cwidget.json').then((r) => r.status)));
  out('DONE');
} catch (e) { out('FAIL ' + e.message); }
</script></body></html>
"#;

const CSS: &str = "body { background: #1e1f22; color: #d4d6db; font: 14px sans-serif; }\n";

/// Run the widget steps after the detail window passes (before the server stop).
pub(super) fn splice(steps: &mut Vec<Step>) {
    let at = steps.iter().position(
        |s| matches!(s, Step::Log(t) if t.starts_with("--- stop the server from the settings page")),
    );
    if let Some(at) = at {
        steps.splice(at..at, self::steps());
    }
}

/// `IDE_APP_SCRIPT_ONLY=widget`: startup, these steps and the server stop.
pub(super) fn steps_only() -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup (widgets only)"),
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
    use Step::{App as A, FillNew, Log, MarkP2, Pause, SetLang, Shot, Submit, Wait, WaitCreated, WaitText};
    vec![
        Log("--- widgets: open from the menu, approve, window.ide, agent open / close"),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "widgets",
        },
        Submit,
        WaitCreated(30_000),
        MarkP2,
        WaitText(P2, ">", 15_000),
        w(WStep::WriteFixture),
        // 열린 항목 -> "+ 새로 열기": the session's widgets are listed under the web part.
        A(Message::ToggleOpenItems),
        A(Message::UTerm(crate::uterm::UTermMsg::NewMenu(true))),
        w(WStep::WaitListed(10_000)),
        Wait(500),
        Shot("widget_00_menu"),
        Wait(300),
        A(Message::UTerm(crate::uterm::UTermMsg::NewMenu(false))),
        A(Message::CloseOpenItems),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        w(WStep::Open),
        w(WStep::WaitGrant(10_000)),
        Wait(300),
        Shot("ko_widget_01_grant"),
        Wait(300),
        Pause("ko_widget_01_grant"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        Wait(200),
        Shot("widget_01_grant"),
        Wait(300),
        A(Message::Widget(WidgetMsg::Allow)),
        w(WStep::WaitTab(true, 10_000)),
        w(WStep::WaitPage(30_000)),
        Wait(500),
        Shot("widget_02_tab"),
        Wait(300),
        Pause("widget_02_tab"),
        Log("--- widgets: the agent's open reloads the tab, close closes it"),
        w(WStep::ClearConsole),
        w(WStep::Agent("open")),
        w(WStep::WaitPage(30_000)),
        w(WStep::WaitTab(true, 5_000)),
        w(WStep::Agent("close")),
        w(WStep::WaitTab(false, 5_000)),
    ]
}

/// Tabs of the demo widget.
fn tabs(app: &App, sid: &str) -> usize {
    app.sessions
        .iter()
        .flat_map(|l| l.browsers.iter())
        .filter(|b| b.widget.as_ref().is_some_and(|t| t.session == sid && t.id == WIDGET))
        .count()
}

pub(super) fn run(sc: &mut Script, app: &mut App, st: WStep, el: Duration) -> (bool, Task<Message>) {
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
        WStep::WriteFixture => {
            let root = app.live(&sid).map(|l| PathBuf::from(&l.info.root)).unwrap_or_default();
            let dir = root.join(".ide").join("widgets").join(WIDGET);
            let r = std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::write(dir.join("widget.json"), MANIFEST))
                .and_then(|_| std::fs::write(dir.join("index.html"), PAGE))
                .and_then(|_| std::fs::write(dir.join("style.css"), CSS));
            sc.check(r.is_ok(), &format!("demo widget written to {} ({r:?})", dir.display()));
            true
        }
        WStep::WaitListed(ms) => {
            let listed = app
                .widgets
                .get(&sid)
                .is_some_and(|l| l.iter().any(|w| w.id == WIDGET && w.error.is_none() && !w.granted));
            wait!(listed, ms, format!("the menu lists the demo widget (not approved yet): {:?}", app.widgets.get(&sid)))
        }
        WStep::Open => {
            task = app.update(Message::Widget(WidgetMsg::Open {
                session: sid.clone(),
                widget: WIDGET.into(),
            }));
            true
        }
        WStep::WaitGrant(ms) => {
            let tools = match &app.modal {
                Some(Modal::WidgetGrant(f)) if f.widget == WIDGET => Some(f.tools.clone()),
                _ => None,
            };
            wait!(
                tools.as_deref() == Some(&["ide_session".to_owned()][..]),
                ms,
                format!("the approval dialog asks for ide_session ({tools:?})")
            )
        }
        WStep::WaitTab(open, ms) => {
            let n = tabs(app, &sid);
            wait!(
                if open { n == 1 } else { n == 0 },
                ms,
                format!("demo widget tabs: {n} (want {})", u8::from(open))
            )
        }
        WStep::WaitPage(ms) => {
            let lines = console_lines();
            let done = lines.iter().any(|l| l == "WIDGET DONE" || l.starts_with("WIDGET FAIL"));
            if done {
                let has = |t: &str| lines.iter().any(|l| l == t);
                let css = lines.iter().any(|l| l.starts_with("WIDGET css ") && l != "WIDGET css 0");
                sc.check(has(&format!("WIDGET ide object {WIDGET}")), "window.ide is injected");
                sc.check(has(&format!("WIDGET session {sid}")), "ide.call('ide_session') answers this session");
                sc.check(has("WIDGET data 41"), "ide.data.set / get");
                sc.check(has("WIDGET keys count"), "ide.data.keys");
                sc.check(has("WIDGET undeclared refused"), "an undeclared tool is refused");
                sc.check(css, "relative files of the widget are served");
                sc.check(has("WIDGET outside 404"), "files outside the widget are not served");
                sc.check(has("WIDGET DONE"), &format!("the page finished: {lines:?}"));
                true
            } else {
                wait!(false, ms, format!("the widget page never finished: {lines:?}"))
            }
        }
        WStep::ClearConsole => {
            crate::widgets_ui::clear_console();
            true
        }
        WStep::Agent(action) => {
            let args = serde_json::json!({ "action": action, "widget": WIDGET }).to_string();
            // As the server hands it over (the reply goes to an unknown request: ignored).
            task = app.on_agent_request(u64::MAX, sid.clone(), "ide_widget".into(), args);
            true
        }
    };
    (next, task)
}
