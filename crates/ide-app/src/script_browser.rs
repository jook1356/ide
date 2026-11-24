//! App script steps for browser control (`browser_agent.rs`, `docs/browser-control.md`): the
//! `ide_browser_*` tools as the server hands them over (after its approval), on real CEF
//! pages: open a local fixture page, list, the site question, read text / DOM / console, type,
//! click, key, scroll, wait, evaluate (value, promise, error), a screenshot of the shown tab and
//! of the hidden session page, a page that moved to another site, navigate. Screenshot
//! `browser_01_tab`. A child module of `script.rs`: it uses the script's helpers.

use super::*;
use crate::browser_agent::{SCRIPT_REQUESTS, script_reply};

#[derive(Debug, Clone)]
pub(super) enum BStep {
    /// Write the fixture page into the data dir.
    WriteFixture,
    /// Call a tool as the server would (`$URL`: the fixture's address, `$TAB`: the opened tab).
    Call(u64, &'static str, &'static str),
    /// The answer of call `n`: ok or not, containing this text.
    Expect(u64, bool, &'static str, u64),
    /// Keep the tab id of call `n`'s answer as `$TAB`.
    SaveTab(u64),
}

fn b(x: BStep) -> Step {
    Step::Browser(x)
}

/// The fixture's address and the opened tab.
static STATE: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

const FIXTURE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Browser fixture</title></head>
<body style="font: 16px sans-serif">
<h1>Browser fixture</h1>
<input id="q" placeholder="type here"> <button id="go">Go</button>
<p id="out">ready</p>
<div style="height: 3000px"></div>
<p id="end">end</p>
<script>
const out = document.getElementById('out');
const q = document.getElementById('q');
document.getElementById('go').addEventListener('click', () => {
  out.textContent = 'clicked ' + q.value;
  console.log('FIXTURE clicked');
});
q.addEventListener('keydown', (e) => { if (e.key === 'Enter') out.textContent = 'enter ' + q.value; });
console.warn('FIXTURE loaded');
</script></body></html>
"#;

/// Run the browser steps before the server stop.
pub(super) fn splice(steps: &mut Vec<Step>) {
    let at = steps.iter().position(
        |s| matches!(s, Step::Log(t) if t.starts_with("--- stop the server from the settings page")),
    );
    if let Some(at) = at {
        steps.splice(at..at, self::steps());
    }
}

/// `IDE_APP_SCRIPT_ONLY=browser`: startup, these steps and the server stop.
pub(super) fn steps_only() -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup (browser only)"),
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
    use BStep::*;
    use Step::{FillNew, Log, MarkP2, Shot, Submit, Wait, WaitCreated, WaitText};
    let t = 20_000;
    vec![
        Log("--- browser control: tools on real pages"),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "browser",
        },
        Submit,
        WaitCreated(90_000),
        MarkP2,
        WaitText(P2, ">", 15_000),
        b(WriteFixture),
        b(Call(1, "ide_browser_open", r#"{"url":"$URL"}"#)),
        b(Expect(1, true, "\"tab\":\"tab-", t)),
        b(SaveTab(1)),
        b(Call(2, "ide_browser_list", "{}")),
        b(Expect(2, true, "browser-fixture.html", t)),
        b(Call(3, "ide.browser.site", r#"{"tab":"$TAB"}"#)),
        b(Expect(3, true, "\"origin\":\"file://\"", t)),
        b(Call(4, "ide_browser_read", r#"{"tab":"$TAB","what":"text","max_chars":20000,"origin":"file://"}"#)),
        b(Expect(4, true, "ready", t)),
        b(Call(5, "ide_browser_act", r##"{"tab":"$TAB","action":"type","selector":"#q","text":"hello 한글","origin":"file://"}"##)),
        b(Expect(5, true, "typed 8 characters", t)),
        b(Call(6, "ide_browser_act", r##"{"tab":"$TAB","action":"click","selector":"#go","origin":"file://"}"##)),
        b(Expect(6, true, "clicked #go", t)),
        b(Call(7, "ide_browser_read", r##"{"tab":"$TAB","what":"text","selector":"#out","max_chars":100,"origin":"file://"}"##)),
        b(Expect(7, true, "clicked hello 한글", t)),
        b(Call(8, "ide_browser_act", r##"{"tab":"$TAB","action":"type","selector":"#q","text":"!","origin":"file://"}"##)),
        b(Expect(8, true, "typed", t)),
        b(Call(9, "ide_browser_act", r#"{"tab":"$TAB","action":"key","key":"Enter","origin":"file://"}"#)),
        b(Expect(9, true, "pressed Enter", t)),
        b(Call(10, "ide_browser_act", r#"{"tab":"$TAB","action":"wait","text":"enter hello","timeout_ms":5000,"origin":"file://"}"#)),
        b(Expect(10, true, "found after", t)),
        b(Call(11, "ide_browser_read", r#"{"tab":"$TAB","what":"console","limit":50,"origin":"file://"}"#)),
        b(Expect(11, true, "[info] FIXTURE clicked", t)),
        b(Call(12, "ide_browser_read", r##"{"tab":"$TAB","what":"dom","selector":"#go","max_chars":500,"origin":"file://"}"##)),
        b(Expect(12, true, "<button id=\"go\">Go</button>", t)),
        b(Call(13, "ide_browser_act", r#"{"tab":"$TAB","action":"evaluate","expression":"1 + 2","origin":"file://"}"#)),
        b(Expect(13, true, "3", t)),
        b(Call(14, "ide_browser_act", r#"{"tab":"$TAB","action":"evaluate","expression":"new Promise(r => setTimeout(() => r({late: true}), 100))","origin":"file://"}"#)),
        b(Expect(14, true, "\"late\": true", t)),
        b(Call(15, "ide_browser_act", r#"{"tab":"$TAB","action":"evaluate","expression":"nope()","origin":"file://"}"#)),
        b(Expect(15, false, "JavaScript error: ReferenceError", t)),
        b(Call(16, "ide_browser_act", r##"{"tab":"$TAB","action":"scroll","selector":"#end","origin":"file://"}"##)),
        b(Expect(16, true, "\"scroll\"", t)),
        b(Call(17, "ide_browser_act", r#"{"tab":"$TAB","action":"scroll","dx":0,"dy":-100000,"origin":"file://"}"#)),
        b(Expect(17, true, "\"y\":0", t)),
        b(Call(18, "ide_browser_act", r##"{"tab":"$TAB","action":"wait","selector":"#missing","timeout_ms":500,"origin":"file://"}"##)),
        b(Expect(18, false, "not found within 500 ms", t)),
        b(Call(19, "ide_browser_screenshot", r#"{"tab":"$TAB","origin":"file://"}"#)),
        b(Expect(19, true, "\"data\":\"iVBOR", t)),
        Wait(300),
        Shot("browser_01_tab"),
        // The session's own page is hidden behind the tab: it still answers and paints.
        b(Call(20, "ide_browser_act", r#"{"tab":"page","action":"evaluate","expression":"document.readyState","origin":"about:"}"#)),
        b(Expect(20, true, "complete", t)),
        b(Call(21, "ide_browser_screenshot", r#"{"tab":"page","origin":"about:"}"#)),
        b(Expect(21, true, "\"data\":\"iVBOR", t)),
        // The tab shows another site than the approved one.
        b(Call(22, "ide_browser_read", r#"{"tab":"$TAB","what":"text","max_chars":100,"origin":"https://example.com"}"#)),
        b(Expect(22, false, "another site", t)),
        b(Call(23, "ide_browser_act", r#"{"tab":"$TAB","action":"navigate","url":"data:text/html,<p>moved here</p>"}"#)),
        b(Expect(23, true, "\"url\":\"data:", t)),
        b(Call(24, "ide_browser_read", r#"{"tab":"$TAB","what":"text","max_chars":100,"origin":"data:"}"#)),
        b(Expect(24, true, "moved here", t)),
        b(Call(25, "ide_browser_read", r#"{"tab":"tab-999999","what":"text","origin":"data:"}"#)),
        b(Expect(25, false, "no tab", t)),
    ]
}

fn fill(args: &str) -> String {
    let (url, tab) = STATE.lock().unwrap().clone();
    args.replace("$URL", &url).replace("$TAB", &tab)
}

pub(super) fn run(sc: &mut Script, app: &mut App, st: BStep, el: Duration) -> (bool, Task<Message>) {
    let mut task = Task::none();
    let sid = sc.id(P2).unwrap_or_default();
    let next = match st {
        BStep::WriteFixture => {
            let data = std::env::var("IDE_DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
            let path = data.join("browser-fixture.html");
            let r = std::fs::write(&path, FIXTURE);
            let url = format!("file:///{}", path.display().to_string().replace('\\', "/").trim_start_matches('/'));
            sc.check(r.is_ok(), &format!("fixture page written to {url} ({r:?})"));
            STATE.lock().unwrap().0 = url;
            true
        }
        BStep::Call(n, tool, args) => {
            let args = fill(args);
            task = app.on_agent_request(SCRIPT_REQUESTS + n, sid.clone(), tool.into(), args);
            true
        }
        BStep::Expect(n, ok, needle, ms) => match script_reply(SCRIPT_REQUESTS + n) {
            Some(r) => {
                let (got_ok, text) = match &r {
                    Ok(t) => (true, t.as_str()),
                    Err(t) => (false, t.as_str()),
                };
                let shown: String = text.chars().take(300).collect();
                sc.check(
                    got_ok == ok && text.contains(needle),
                    &format!("browser call {n}: ok {got_ok} (want {ok}), has {needle:?}: {shown}"),
                );
                true
            }
            None if el > Duration::from_millis(ms) => {
                sc.check(false, &format!("timeout: browser call {n} never answered"));
                true
            }
            None => false,
        },
        BStep::SaveTab(n) => {
            let tab = script_reply(SCRIPT_REQUESTS + n)
                .and_then(Result::ok)
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v["tab"].as_str().map(str::to_owned))
                .unwrap_or_default();
            sc.check(!tab.is_empty(), &format!("opened tab {tab}"));
            STATE.lock().unwrap().1 = tab;
            true
        }
    };
    (next, task)
}
