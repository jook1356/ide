//! App script steps for widgets (`widgets_ui.rs`, `widgets_page.rs`, `docs/widgets.md`): a
//! draft written into the session folder is registered after the 위젯 등록 dialog (the agent's
//! `register` as the server hands it over); the 위젯 page, the status bar's launcher and
//! "+ 새로 열기" list it; opened, its page (real CEF, served through `ide-widget://`) gets
//! `window.ide`, calls a declared tool, keeps data, is refused an undeclared tool and files
//! outside the widget; the open tab is remembered by the server; the agent's `open` reloads the
//! tab and `close` closes it; the page deletes it. Screenshots `widget_*` (Korean `ko_widget_*`).
//! A child module of `script.rs`: it uses the script's helpers.

use super::*;
use crate::widgets_ui::{WidgetMsg, console_lines};

#[derive(Debug, Clone)]
pub(super) enum WStep {
    /// Write the demo widget's draft into the P2 session's folder.
    WriteFixture,
    /// The registration dialog shows the demo widget's tools.
    WaitRegister(u64),
    /// The demo widget is registered (`true`) or not.
    WaitListed(bool, u64),
    /// Open it as the launcher / menu do.
    Open,
    /// One widget tab of the demo widget is open (`true`) or none.
    WaitTab(bool, u64),
    /// The server remembers the demo widget open in P2 (`true`) or not.
    WaitRemembered(bool, u64),
    /// The page logged `WIDGET DONE`; then its lines are checked.
    WaitPage(u64),
    /// Forget the page's lines.
    ClearConsole,
    /// The daemon asks for an approval of this capability (as it hands it over), as this request.
    AskApproval(&'static str, u64),
    /// The daemon stopped waiting for this request (timed out: `true`).
    ApprovalEnded(u64, bool),
    /// Settings -> 권한 shows this wait (seconds, `None`: no limit).
    WaitApprovalWait(Option<u64>, u64),
    /// The approval dialog shows this capability (`None`: no dialog), with this many waiting.
    ApprovalShown(Option<&'static str>, usize),
    /// The agent's `ide_widget` (`register` / `open` / `close`) as the server hands it over.
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

fn wm(m: WidgetMsg) -> Step {
    Step::App(Message::Widget(m))
}

fn steps() -> Vec<Step> {
    use Step::{App as A, FillNew, GoPage, Log, MarkP2, Pause, SetLang, Shot, Submit, Wait, WaitCreated, WaitText};
    vec![
        Log("--- widgets: register (dialog), page, launcher, menu"),
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
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        w(WStep::Agent("register")),
        w(WStep::WaitRegister(10_000)),
        Wait(300),
        Shot("ko_widget_01_register"),
        Wait(300),
        Pause("ko_widget_01_register"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        Wait(200),
        Shot("widget_01_register"),
        Wait(300),
        wm(WidgetMsg::Allow),
        w(WStep::WaitListed(true, 10_000)),
        // The 위젯 page: the list beside the rail and the picked widget.
        GoPage(Page::Widgets),
        wm(WidgetMsg::Select(WIDGET.into())),
        Wait(500),
        Shot("widget_02_page"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(300),
        Shot("ko_widget_02_page"),
        Pause("ko_widget_02_page"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        GoPage(Page::Work),
        // The status bar's launcher.
        wm(WidgetMsg::Launcher(true)),
        Wait(500),
        Shot("widget_03_launcher"),
        Wait(300),
        wm(WidgetMsg::Launcher(false)),
        // 열린 항목 -> "+ 새로 열기": the registered widgets are listed under the web part.
        A(Message::ToggleOpenItems),
        A(Message::UTerm(crate::uterm::UTermMsg::NewMenu(true))),
        Wait(800),
        Shot("widget_00_menu"),
        Wait(300),
        A(Message::UTerm(crate::uterm::UTermMsg::NewMenu(false))),
        A(Message::CloseOpenItems),
        Log("--- widgets: open, window.ide, remembered, agent open / close, delete"),
        w(WStep::Open),
        w(WStep::WaitTab(true, 10_000)),
        w(WStep::WaitPage(30_000)),
        w(WStep::WaitRemembered(true, 10_000)),
        Wait(500),
        Shot("widget_04_tab"),
        Wait(300),
        Pause("widget_04_tab"),
        w(WStep::ClearConsole),
        w(WStep::Agent("open")),
        w(WStep::WaitPage(30_000)),
        w(WStep::WaitTab(true, 5_000)),
        w(WStep::Agent("close")),
        w(WStep::WaitTab(false, 5_000)),
        w(WStep::WaitRemembered(false, 10_000)),
        // Deleting on the page closes its tabs and unregisters it.
        w(WStep::Open),
        w(WStep::WaitTab(true, 10_000)),
        GoPage(Page::Widgets),
        wm(WidgetMsg::Select(WIDGET.into())),
        wm(WidgetMsg::DeleteArm(true)),
        Wait(300),
        Shot("widget_05_delete"),
        wm(WidgetMsg::Delete(WIDGET.into())),
        w(WStep::WaitListed(false, 10_000)),
        w(WStep::WaitTab(false, 5_000)),
        GoPage(Page::Work),
        // Approvals (`approvals_ui.rs`, docs/approvals.md): two questions, one at a time.
        Log("--- approvals: the question, its queue, ended questions, closing refuses, the settings list"),
        w(WStep::AskApproval("session.steer", 1)),
        w(WStep::AskApproval("session.interrupt", 2)),
        w(WStep::AskApproval("folder.write", 3)),
        w(WStep::ApprovalShown(Some("session.steer"), 2)),
        // A waiting question the daemon stopped waiting for leaves the queue.
        w(WStep::ApprovalEnded(3, true)),
        w(WStep::ApprovalShown(Some("session.steer"), 1)),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(300),
        Shot("ko_approval_01_question"),
        Pause("ko_approval_01_question"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        Wait(200),
        Shot("approval_01_question"),
        A(Message::Approval(crate::approvals_ui::ApprovalMsg::Answer("once"))),
        w(WStep::ApprovalShown(Some("session.interrupt"), 0)),
        // The shown question timed out: it closes (a toast says so), the next one shows.
        w(WStep::AskApproval("folder.write", 4)),
        w(WStep::ApprovalShown(Some("session.interrupt"), 1)),
        w(WStep::ApprovalEnded(2, true)),
        w(WStep::ApprovalShown(Some("folder.write"), 0)),
        Wait(300),
        Shot("approval_03_timed_out"),
        // Ending another request changes nothing; closing refuses.
        w(WStep::ApprovalEnded(1, false)),
        w(WStep::ApprovalShown(Some("folder.write"), 0)),
        A(Message::CloseModal),
        w(WStep::ApprovalShown(None, 0)),
        GoPage(Page::Settings),
        w(WStep::WaitApprovalWait(Some(300), 5_000)),
        Wait(500),
        Shot("approval_02_settings"),
        A(Message::Approval(crate::approvals_ui::ApprovalMsg::SetWait(None))),
        w(WStep::WaitApprovalWait(None, 5_000)),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        Wait(500),
        Shot("ko_approval_02_settings_no_limit"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        A(Message::Approval(crate::approvals_ui::ApprovalMsg::SetWait(Some(300)))),
        w(WStep::WaitApprovalWait(Some(300), 5_000)),
        GoPage(Page::Work),
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
        WStep::WaitRegister(ms) => {
            let tools = match &app.modal {
                Some(Modal::WidgetRegister(f)) if f.widget == WIDGET => Some(f.tools.clone()),
                _ => None,
            };
            wait!(
                tools.as_deref() == Some(&["ide_session".to_owned()][..]),
                ms,
                format!("the registration dialog asks for ide_session ({tools:?})")
            )
        }
        WStep::WaitListed(want, ms) => {
            let listed = app.widgets.get(WIDGET).is_some();
            wait!(
                listed == want,
                ms,
                format!("demo widget registered: {listed} (want {want})")
            )
        }
        WStep::Open => {
            task = app.update(Message::Widget(WidgetMsg::Open {
                session: sid.clone(),
                widget: WIDGET.into(),
            }));
            true
        }
        WStep::WaitTab(open, ms) => {
            let n = tabs(app, &sid);
            wait!(
                if open { n == 1 } else { n == 0 },
                ms,
                format!("demo widget tabs: {n} (want {})", u8::from(open))
            )
        }
        WStep::WaitRemembered(want, ms) => {
            let open = app
                .widgets
                .get(WIDGET)
                .is_some_and(|w| w.open.contains(&sid));
            wait!(
                open == want,
                ms,
                format!("the server remembers the tab open: {open} (want {want})")
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
        WStep::AskApproval(capability, request) => {
            let args = serde_json::json!({
                "capability": capability,
                "requester": "p1",
                "requester_title": "planner",
                "target": sid,
                "target_title": "widgets",
                "detail": "Stop here: the API changed, rebase on the parent first.",
            });
            // The answer goes to an unknown request: ignored by the server.
            task = app.on_agent_request(u64::MAX - request, sid.clone(), ide_client::APPROVAL_REQUEST.into(), args.to_string());
            true
        }
        WStep::ApprovalEnded(request, timed_out) => {
            task = app.agent_request_ended(u64::MAX - request, timed_out);
            true
        }
        WStep::WaitApprovalWait(want, ms) => {
            let got = app.approvals.wait;
            wait!(got == Some(want), ms, format!("approval wait {got:?} (want {want:?})"))
        }
        WStep::ApprovalShown(want, waiting) => {
            let shown = match &app.modal {
                Some(Modal::Approval(f)) => Some(f.capability.clone()),
                _ => None,
            };
            let n = app.approvals.queue.len();
            sc.check(
                shown.as_deref() == want && n == waiting,
                &format!("approval dialog {shown:?} with {n} waiting (want {want:?}, {waiting})"),
            );
            true
        }
        WStep::Agent(action) => {
            let args = if action == "register" {
                // What the server sends for a new widget.
                serde_json::json!({
                    "action": "register",
                    "widget": WIDGET,
                    "name": "Demo widget",
                    "description": "Script check of window.ide",
                    "tools": ["ide_session"],
                    "scope": "session",
                    "ask": "new",
                    "by": "",
                })
            } else {
                serde_json::json!({ "action": action, "widget": WIDGET })
            };
            // As the server hands it over (the reply goes to an unknown request: ignored).
            task = app.on_agent_request(u64::MAX, sid.clone(), "ide_widget".into(), args.to_string());
            true
        }
    };
    (next, task)
}
