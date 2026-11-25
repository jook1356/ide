//! The app's part of browser control by agents (`docs/browser-control.md`): the
//! `meninx_browser_*` tools on the session's web page and browser tabs, through the DevTools
//! protocol of their CEF views (`meninx_web::DevTools`); input is dispatched inside the browser,
//! never as OS input. The daemon has checked the arguments and the user's approval of the site
//! before a call gets here; the app refuses when the tab shows another site by now.

use std::time::Duration;

use iced::{Size, Task};
use meninx_proto::browser::{TAB_SITE, TOOL_ACT, TOOL_LIST, TOOL_OPEN, TOOL_READ, TOOL_SCREENSHOT, origin};
use meninx_web::{DevTools, WebView};
use serde_json::{Value, json};

use crate::app::{App, Message};

/// The session's own web page.
const PAGE: &str = "page";
/// Browser tabs: `tab-<id>`.
const TAB_PREFIX: &str = "tab-";
/// Longest run of one call (the daemon waits 60 s).
const CALL_LIMIT: Duration = Duration::from_secs(55);
/// How long a page may take to load after open / navigate before the call answers anyway.
const LOAD_WAIT: Duration = Duration::from_secs(15);
/// Poll interval of waits.
const POLL: Duration = Duration::from_millis(150);

/// The app script (`script_browser.rs`) calls the tools with request ids from here up; their
/// answers are kept for it (the server ignores replies to requests it did not send).
pub(crate) const SCRIPT_REQUESTS: u64 = u64::MAX - 1_000_000;
static SCRIPT_REPLIES: std::sync::Mutex<Vec<(u64, Result<String, String>)>> = std::sync::Mutex::new(Vec::new());

/// The answer to a script request, once there.
pub(crate) fn script_reply(request: u64) -> Option<Result<String, String>> {
    let r = SCRIPT_REPLIES.lock().unwrap();
    r.iter().find(|(id, _)| *id == request).map(|(_, a)| a.clone())
}

fn tab_id(id: u32) -> String {
    format!("{TAB_PREFIX}{id}")
}

impl App {
    /// A browser tool call or the daemon's site question (`meninx.browser.site`).
    pub(crate) fn agent_browser(&mut self, request: u64, session: String, tool: String, args: Value) -> Task<Message> {
        crate::script::log(&format!("agent {session}: {tool} {args}"));
        let mut show = Task::none();
        let run: Result<Job, String> = match tool.as_str() {
            TOOL_LIST => Ok(Job::Done(self.browser_list(&session))),
            TAB_SITE => self.browser_site(&session, &args).map(Job::Done),
            TOOL_OPEN => self.browser_open(&session, &args).map(|(job, task)| {
                show = task;
                job
            }),
            TOOL_READ | TOOL_ACT | TOOL_SCREENSHOT => self.browser_page(&session, &args).map(|dt| Job::Run(dt, tool.clone(), args.clone())),
            other => Err(format!("this Meninx window cannot carry out `{other}`")),
        };
        let Some(c) = self.online() else {
            return Task::none();
        };
        let job = run.unwrap_or_else(Job::Failed);
        let reply = Task::perform(
            async move {
                let result = match job {
                    Job::Done(v) => Ok(v),
                    Job::Failed(e) => Err(e),
                    Job::Open(dt, tab) => {
                        let _awake = dt.awake();
                        wait_loaded(&dt).await;
                        Ok(json!({"tab": tab, "url": dt.url(), "loading": dt.is_loading()}).to_string())
                    }
                    Job::Run(dt, tool, args) => match tokio::time::timeout(CALL_LIMIT, run_on_page(dt, &tool, &args)).await {
                        Ok(r) => r,
                        Err(_) => Err("the page did not answer in time".into()),
                    },
                };
                if request >= SCRIPT_REQUESTS {
                    SCRIPT_REPLIES.lock().unwrap().push((request, result.clone()));
                }
                let _ = c.agent_reply(request, result).await;
            },
            |_| (),
        )
        .discard();
        Task::batch([show, reply])
    }

    /// The tabs `session` can use: its folder's web page and browser tabs, and the 공용 ones.
    fn browser_list(&self, session: &str) -> String {
        let owner = self.workspace_owner(session);
        let mut rows = Vec::new();
        let shown = self.shown_live().map(|l| l.info.id.clone());
        if let Some(live) = self.live(&owner) {
            let page_shown = shown.as_deref() == Some(owner.as_str()) && self.page_shown(live);
            let url = live.web.as_ref().map(WebView::url).unwrap_or_else(|| live.url_input.clone());
            rows.push(json!({
                "tab": PAGE,
                "kind": "session page",
                "url": url,
                "title": live.web_title,
                "shown": page_shown,
                "loading": live.web.as_ref().is_some_and(WebView::is_loading),
            }));
            let shown_tab = self.shown_browser();
            let tabs = live
                .browsers
                .iter()
                .map(|b| (b, false))
                .chain(self.global_browsers.iter().map(|b| (b, true)));
            for (b, global) in tabs {
                let kind = match (&b.widget, global) {
                    (Some(_), _) => "widget",
                    (None, true) => "shared tab",
                    (None, false) => "tab",
                };
                rows.push(json!({
                    "tab": tab_id(b.id),
                    "kind": kind,
                    "url": b.view.as_ref().map(WebView::url).unwrap_or_else(|| b.url_input.clone()),
                    "title": b.label(),
                    "shown": shown_tab == Some(b.id),
                    "loading": b.loading,
                }));
            }
        }
        serde_json::to_string_pretty(&json!({ "tabs": rows })).expect("json")
    }

    /// The view of a tab of `session`, created when it has none yet (a tab never shown).
    fn browser_view(&mut self, session: &str, tab: &str) -> Result<WebView, String> {
        let owner = self.workspace_owner(session);
        let runtime = self.runtime.clone();
        let make = |url: &str| {
            WebView::new(&runtime, url, Size::new(800.0, 600.0), 1.0).map_err(|e| format!("could not open the page: {e:#}"))
        };
        let view = if tab == PAGE {
            let live = self.live_mut(&owner).ok_or("this session is not open in the Meninx window")?;
            match &live.web {
                Some(v) => v.clone(),
                None => {
                    let url = if live.url_input.trim().is_empty() {
                        crate::app::DEFAULT_URL.to_owned()
                    } else {
                        live.url_input.clone()
                    };
                    let v = make(&url)?;
                    live.web = Some(v.clone());
                    v
                }
            }
        } else {
            let id: u32 = tab
                .strip_prefix(TAB_PREFIX)
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| format!("no tab `{tab}` (see meninx_browser_list)"))?;
            let reachable = self.global_browsers.iter().any(|b| b.id == id)
                || self.live(&owner).is_some_and(|l| l.browsers.iter().any(|b| b.id == id));
            if !reachable {
                return Err(format!("no tab `{tab}` (see meninx_browser_list)"));
            }
            let b = self.browser_mut(id).ok_or_else(|| format!("no tab `{tab}`"))?;
            match &b.view {
                Some(v) => v.clone(),
                None => {
                    let url = if b.widget.is_some() {
                        b.url_input.clone()
                    } else {
                        crate::browser::address_to_url(&b.url_input).unwrap_or_else(|| crate::app::DEFAULT_URL.into())
                    };
                    let v = make(&url)?;
                    b.view = Some(v.clone());
                    v
                }
            }
        };
        // A new view starts shown: hide what is not on screen.
        self.sync_web();
        Ok(view)
    }

    /// `meninx.browser.site`: which site a tab shows.
    fn browser_site(&mut self, session: &str, args: &Value) -> Result<String, String> {
        let tab = args.get("tab").and_then(Value::as_str).unwrap_or("");
        let v = self.browser_view(session, tab)?;
        let url = v.url();
        Ok(json!({"origin": origin(&url), "url": url, "title": v.title()}).to_string())
    }

    /// The page of a read / act / screenshot call, if it still shows the approved site
    /// (`navigate` goes elsewhere: its destination was approved).
    fn browser_page(&mut self, session: &str, args: &Value) -> Result<DevTools, String> {
        let tab = args.get("tab").and_then(Value::as_str).unwrap_or("");
        let v = self.browser_view(session, tab)?;
        let navigate = args.get("action").and_then(Value::as_str) == Some("navigate");
        let approved = args.get("origin").and_then(Value::as_str);
        let now = origin(&v.url());
        if !navigate && approved != Some(now.as_str()) {
            return Err(format!("the tab shows another site now ({now}); call again"));
        }
        Ok(v.devtools())
    }

    /// `meninx_browser_open`: a new tab of the session, shown in its detail area.
    fn browser_open(&mut self, session: &str, args: &Value) -> Result<(Job, Task<Message>), String> {
        let url = args.get("url").and_then(Value::as_str).unwrap_or("").to_owned();
        if url.is_empty() {
            return Err("`url` is required".into());
        }
        if self.live(session).is_none() {
            return Err("this session is not open in the Meninx window".into());
        }
        let show = self.open_browser(Some(session.to_owned()), false, url);
        let id = self.next_browser;
        let tab = tab_id(id);
        let view = self.browser_view(session, &tab)?;
        Ok((Job::Open(view.devtools(), tab), show))
    }
}

/// What is left to do after the synchronous part.
enum Job {
    Done(String),
    Failed(String),
    Open(DevTools, String),
    Run(DevTools, String, Value),
}

/// Waits until the page exists and stopped loading (at most [`LOAD_WAIT`]).
async fn wait_loaded(dt: &DevTools) {
    let start = std::time::Instant::now();
    // Loading starts a moment after a navigation is asked for.
    tokio::time::sleep(POLL).await;
    while start.elapsed() < LOAD_WAIT {
        if dt.is_created() && !dt.is_loading() {
            match evaluate(dt, "document.readyState").await {
                Ok(Value::String(s)) if s == "complete" => return,
                _ => {}
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Waits until the CEF browser of a just created view exists.
async fn wait_created(dt: &DevTools) -> Result<(), String> {
    let start = std::time::Instant::now();
    while !dt.is_created() {
        if start.elapsed() > LOAD_WAIT {
            return Err("the page did not start".into());
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(())
}

/// `Runtime.evaluate` (by value, promises awaited): the value, or the exception as text.
async fn evaluate(dt: &DevTools, expression: &str) -> Result<Value, String> {
    let r = dt
        .call(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true}),
        )
        .await?;
    eval_value(&r)
}

/// The value of a `Runtime.evaluate` result.
fn eval_value(r: &Value) -> Result<Value, String> {
    if let Some(ex) = r.get("exceptionDetails") {
        let text = ex
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| ex.get("text").and_then(Value::as_str))
            .unwrap_or("exception");
        return Err(format!("JavaScript error: {text}"));
    }
    let res = r.get("result").cloned().unwrap_or(Value::Null);
    Ok(match res.get("value") {
        Some(v) => v.clone(),
        None => match res.get("type").and_then(Value::as_str) {
            Some("undefined") | None => Value::Null,
            // Not serialisable (functions, symbols, DOM nodes): its description.
            Some(_) => res.get("description").cloned().unwrap_or(Value::Null),
        },
    })
}

/// A JS string literal.
fn js_str(s: &str) -> String {
    serde_json::to_string(s).expect("json")
}

/// JS: the element `selector` matches (or the body / document), scrolled into view; its centre
/// in viewport CSS pixels, or null.
fn js_center(selector: &str) -> String {
    format!(
        "(() => {{ const e = document.querySelector({}); if (!e) return null; \
         e.scrollIntoView({{block: 'center', inline: 'center'}}); const r = e.getBoundingClientRect(); \
         return {{x: r.left + r.width / 2, y: r.top + r.height / 2}}; }})()",
        js_str(selector)
    )
}

/// JS: text or HTML of the element `selector` matches (whole page without one), or null.
fn js_read(what: &str, selector: Option<&str>) -> String {
    let el = match selector {
        Some(s) => format!("document.querySelector({})", js_str(s)),
        None if what == "dom" => "document.documentElement".into(),
        None => "document.body".into(),
    };
    let prop = if what == "dom" { "outerHTML" } else { "innerText" };
    format!("(() => {{ const e = {el}; return e ? e.{prop} : null; }})()")
}

/// At most `max` characters, with a note when cut.
fn clip(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(&format!("\n… ({} more characters; use selector or max_chars)", n - max));
    out
}

/// Console messages as lines (`[level] message (source:line)`), the last `limit`.
fn console_lines(entries: &[meninx_web::ConsoleEntry], limit: usize) -> String {
    if entries.is_empty() {
        return "(no console messages)".into();
    }
    let skip = entries.len().saturating_sub(limit);
    entries[skip..]
        .iter()
        .map(|e| {
            let level = match e.level {
                meninx_web::ConsoleLevel::Debug => "debug",
                meninx_web::ConsoleLevel::Info => "info",
                meninx_web::ConsoleLevel::Warning => "warning",
                meninx_web::ConsoleLevel::Error => "error",
            };
            if e.source.is_empty() {
                format!("[{level}] {}", e.message)
            } else {
                format!("[{level}] {} ({}:{})", e.message, e.source, e.line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn dispatch_mouse(dt: &DevTools, events: Vec<Value>) -> Result<(), String> {
    for e in events {
        dt.call("Input.dispatchMouseEvent", e).await?;
    }
    Ok(())
}

/// Carries out a read / act / screenshot call on a page.
async fn run_on_page(dt: DevTools, tool: &str, args: &Value) -> Result<String, String> {
    wait_created(&dt).await?;
    // Hidden tabs paint and take input while an agent works on them.
    let _awake = dt.awake();
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| args.get(k).and_then(Value::as_f64);
    let u = |k: &str, d: u64| args.get(k).and_then(Value::as_u64).unwrap_or(d);
    let selector = args.get("selector").and_then(Value::as_str);
    match tool {
        TOOL_SCREENSHOT => {
            let r = dt.screenshot().await?;
            let data = r.get("data").and_then(Value::as_str).unwrap_or("");
            if data.is_empty() {
                return Err("the page gave no screenshot".into());
            }
            Ok(json!({"text": format!("screenshot of {}", dt.url()), "image": {"data": data, "mime_type": "image/png"}}).to_string())
        }
        TOOL_READ => {
            let what = s("what");
            if what == "console" {
                return Ok(console_lines(&dt.console(), u("limit", 100) as usize));
            }
            match evaluate(&dt, &js_read(what, selector)).await? {
                Value::String(t) => Ok(clip(&t, u("max_chars", 20_000) as usize)),
                Value::Null => Err(match selector {
                    Some(sel) => format!("no element matches `{sel}`"),
                    None => "the page has no content yet".into(),
                }),
                other => Ok(other.to_string()),
            }
        }
        TOOL_ACT => match s("action") {
            "navigate" => {
                let r = dt.call("Page.navigate", json!({"url": s("url")})).await?;
                if let Some(e) = r.get("errorText").and_then(Value::as_str).filter(|e| !e.is_empty()) {
                    return Err(format!("navigation failed: {e}"));
                }
                wait_loaded(&dt).await;
                Ok(json!({"url": dt.url(), "loading": dt.is_loading()}).to_string())
            }
            "click" => {
                let (x, y, what) = match selector {
                    Some(sel) => {
                        let c = evaluate(&dt, &js_center(sel)).await?;
                        let (Some(x), Some(y)) = (c.get("x").and_then(Value::as_f64), c.get("y").and_then(Value::as_f64)) else {
                            return Err(format!("no element matches `{sel}`"));
                        };
                        (x, y, sel.to_owned())
                    }
                    None => {
                        let (x, y) = (n("x").unwrap_or(0.0), n("y").unwrap_or(0.0));
                        (x, y, "the point".to_owned())
                    }
                };
                dispatch_mouse(&dt, meninx_web::cdp::click(x, y, "left", 1)).await?;
                Ok(format!("clicked {what} at {x:.0}, {y:.0}"))
            }
            "type" => {
                if let Some(sel) = selector {
                    let focus = format!(
                        "(() => {{ const e = document.querySelector({}); if (!e) return false; e.scrollIntoView({{block: 'center'}}); e.focus(); return true; }})()",
                        js_str(sel)
                    );
                    if evaluate(&dt, &focus).await? != Value::Bool(true) {
                        return Err(format!("no element matches `{sel}`"));
                    }
                }
                dt.call("Input.insertText", json!({"text": s("text")})).await?;
                Ok(format!("typed {} characters", s("text").chars().count()))
            }
            "key" => {
                let key = meninx_web::cdp::parse_key(s("key"))?;
                for e in key.events() {
                    dt.call("Input.dispatchKeyEvent", e).await?;
                }
                Ok(format!("pressed {}", s("key")))
            }
            "scroll" => {
                if let Some(sel) = selector {
                    if evaluate(&dt, &js_center(sel)).await?.is_null() {
                        return Err(format!("no element matches `{sel}`"));
                    }
                } else {
                    let (x, y) = match (n("x"), n("y")) {
                        (Some(x), Some(y)) => (x, y),
                        _ => {
                            let c = evaluate(&dt, "[innerWidth / 2, innerHeight / 2]").await?;
                            (c[0].as_f64().unwrap_or(100.0), c[1].as_f64().unwrap_or(100.0))
                        }
                    };
                    let wheel = meninx_web::cdp::wheel(x, y, n("dx").unwrap_or(0.0), n("dy").unwrap_or(0.0));
                    dt.call("Input.dispatchMouseEvent", wheel).await?;
                    // The wheel scrolls asynchronously.
                    tokio::time::sleep(POLL).await;
                }
                let pos = evaluate(&dt, "({x: scrollX, y: scrollY})").await?;
                Ok(json!({"scroll": pos}).to_string())
            }
            "wait" => {
                if let Some(ms) = args.get("ms").and_then(Value::as_u64) {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
                let text = s("text");
                if selector.is_none() && text.is_empty() {
                    return Ok("waited".into());
                }
                let mut checks = Vec::new();
                if let Some(sel) = selector {
                    checks.push(format!("!!document.querySelector({})", js_str(sel)));
                }
                if !text.is_empty() {
                    checks.push(format!("!!document.body && document.body.innerText.includes({})", js_str(text)));
                }
                let check = checks.join(" && ");
                let limit = Duration::from_millis(u("timeout_ms", 10_000));
                let start = std::time::Instant::now();
                loop {
                    if evaluate(&dt, &check).await.ok() == Some(Value::Bool(true)) {
                        return Ok(format!("found after {} ms", start.elapsed().as_millis()));
                    }
                    if start.elapsed() >= limit {
                        return Err(format!("not found within {} ms", limit.as_millis()));
                    }
                    tokio::time::sleep(POLL).await;
                }
            }
            "evaluate" => {
                let v = evaluate(&dt, s("expression")).await?;
                Ok(serde_json::to_string_pretty(&v).expect("json"))
            }
            other => Err(format!("unknown action `{other}`")),
        },
        other => Err(format!("unknown tool `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_results() {
        assert_eq!(eval_value(&json!({"result": {"type": "number", "value": 2}})).unwrap(), json!(2));
        assert_eq!(eval_value(&json!({"result": {"type": "undefined"}})).unwrap(), Value::Null);
        assert_eq!(
            eval_value(&json!({"result": {"type": "function", "description": "function f() {}"}})).unwrap(),
            json!("function f() {}")
        );
        let e = eval_value(&json!({"result": {}, "exceptionDetails": {"text": "Uncaught", "exception": {"description": "ReferenceError: x is not defined"}}}));
        assert_eq!(e.unwrap_err(), "JavaScript error: ReferenceError: x is not defined");
    }

    #[test]
    fn scripts_quote_selectors() {
        let js = js_center("a[title=\"x\"]'");
        assert!(js.contains(r#"document.querySelector("a[title=\"x\"]'")"#));
        assert_eq!(js_read("text", None), "(() => { const e = document.body; return e ? e.innerText : null; })()");
        assert!(js_read("dom", None).contains("document.documentElement"));
        assert!(js_read("dom", Some("#m")).contains(r##"document.querySelector("#m")"##));
    }

    #[test]
    fn clipping_and_console() {
        assert_eq!(clip("héllo", 10), "héllo");
        assert!(clip("héllo", 2).starts_with("hé\n… (3 more characters"));
        let e = |level, m: &str, src: &str| meninx_web::ConsoleEntry {
            level,
            message: m.into(),
            source: src.into(),
            line: 3,
            url: String::new(),
        };
        let all = [e(meninx_web::ConsoleLevel::Info, "a", ""), e(meninx_web::ConsoleLevel::Error, "b", "x.js")];
        assert_eq!(console_lines(&all, 10), "[info] a\n[error] b (x.js:3)");
        assert_eq!(console_lines(&all, 1), "[error] b (x.js:3)");
        assert_eq!(console_lines(&[], 1), "(no console messages)");
    }
}
