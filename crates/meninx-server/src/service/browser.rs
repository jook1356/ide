//! Browser control by agents (`docs/browser-control.md`): the `meninx_browser_*` tools. The pages
//! live in the app (CEF), so the app carries the tools out; the daemon checks the arguments,
//! asks the user's approval for the site first (`browser.act` / `browser.read` per origin) and
//! turns a screenshot answer into an image result.

use std::sync::Arc;

use meninx_proto::browser::{
    CAP_ACT, CAP_READ, TAB_SITE, TOOL_ACT, TOOL_LIST, TOOL_OPEN, TOOL_READ, TOOL_SCREENSHOT, origin,
};
use meninx_proto::rpc::AgentImage;
use meninx_proto::{AgentTool, AgentToolResult};
use serde_json::{Value, json};

use super::UserCtx;

/// Longest `wait` (the app answers within the bridge's 60 s).
const WAIT_MAX_MS: u64 = 45_000;
/// Default and largest `max_chars` of `meninx_browser_read`.
const READ_CHARS: u64 = 20_000;
const READ_CHARS_MAX: u64 = 200_000;
/// Default and largest console `limit`.
const CONSOLE_LIMIT: u64 = 100;
const CONSOLE_LIMIT_MAX: u64 = 300;

fn err(t: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: t.into(),
        is_error: true,
        image: None,
    }
}

/// The browser tools of the bridge (`tools/list`).
pub(crate) fn tools() -> Vec<AgentTool> {
    let tool = |name: &str, description: &str, schema: &str| AgentTool {
        name: name.into(),
        description: description.into(),
        input_schema: schema.into(),
    };
    vec![
        tool(
            TOOL_LIST,
            "List the web pages of Meninx's browser this session can use: \
             its browser tabs and the shared (공용) tabs, with id, address, title, whether \
             it is shown and loading. Use the ids with the other meninx_browser_* tools.",
            r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        ),
        tool(
            TOOL_OPEN,
            "Open an address in a new browser tab of this session in Meninx (shown next to the \
             terminal) and return its tab id. The user approves each site (origin) first.",
            r#"{"type":"object","properties":{"url":{"type":"string","description":"An address, e.g. http://localhost:5173 or https://example.com."}},"required":["url"],"additionalProperties":false}"#,
        ),
        tool(
            TOOL_READ,
            "Read a page of Meninx's browser: `text` (visible text, default), `dom` (HTML) of the \
             page or of the first element matching `selector`, or `console` (recent console \
             messages and errors). The user approves reading each site first.",
            r#"{"type":"object","properties":{"tab":{"type":"string","description":"A tab id from meninx_browser_list."},"what":{"type":"string","enum":["text","dom","console"]},"selector":{"type":"string","description":"CSS selector (text / dom)."},"max_chars":{"type":"integer","description":"Longest answer (default 20000)."},"limit":{"type":"integer","description":"Last N console messages (default 100)."}},"required":["tab"],"additionalProperties":false}"#,
        ),
        tool(
            TOOL_ACT,
            "Act on a page of Meninx's browser, inside the browser (no OS input): `navigate` {url}; \
             `click` {selector} or {x, y} (CSS pixels); `type` {text, selector?} (focuses the \
             element first, then inserts the text); `key` {key} (e.g. Enter, Tab, Escape, \
             ArrowDown, ctrl+a); `scroll` {selector} (into view) or {dx?, dy?, x?, y?}; `wait` \
             {selector | text | ms, timeout_ms?}; `evaluate` {expression} (JavaScript, the \
             result as JSON; promises are awaited). The user approves each site first.",
            r#"{"type":"object","properties":{"tab":{"type":"string","description":"A tab id from meninx_browser_list."},"action":{"type":"string","enum":["navigate","click","type","key","scroll","wait","evaluate"]},"url":{"type":"string"},"selector":{"type":"string","description":"CSS selector."},"x":{"type":"number"},"y":{"type":"number"},"text":{"type":"string"},"key":{"type":"string"},"dx":{"type":"number"},"dy":{"type":"number"},"ms":{"type":"integer"},"timeout_ms":{"type":"integer","description":"For wait (default 10000, at most 45000)."},"expression":{"type":"string"}},"required":["tab","action"],"additionalProperties":false}"#,
        ),
        tool(
            TOOL_SCREENSHOT,
            "A PNG screenshot of a page of Meninx's browser (what its viewport shows). The user \
             approves reading each site first.",
            r#"{"type":"object","properties":{"tab":{"type":"string","description":"A tab id from meninx_browser_list."}},"required":["tab"],"additionalProperties":false}"#,
        ),
    ]
}

/// What has to happen before the app may carry a call out.
#[derive(Debug, PartialEq)]
pub(crate) enum Plan {
    /// Nothing (`meninx_browser_list`).
    Direct,
    /// Approve acting on the site of an address the page goes to.
    Site { origin: String, detail: String },
    /// Look up which site the tab shows, approve `cap` for it.
    Tab {
        tab: String,
        cap: &'static str,
        detail: String,
    },
}

/// Checks and normalises a browser tool's arguments (what the app gets) and says what to
/// approve.
pub(crate) fn plan(tool: &str, args: &Value) -> Result<(Value, Plan), String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("");
    let need = |k: &str| {
        let v = text(k);
        if v.is_empty() { Err(format!("`{k}` is required")) } else { Ok(v.to_owned()) }
    };
    let num = |k: &str| args.get(k).and_then(Value::as_f64);
    let uint = |k: &str, default: u64, max: u64| {
        args.get(k).and_then(Value::as_u64).unwrap_or(default).clamp(1, max)
    };
    let short = |s: &str, n: usize| -> String {
        let mut out: String = s.chars().take(n).collect();
        if s.chars().count() > n {
            out.push('…');
        }
        out
    };
    match tool {
        TOOL_LIST => Ok((json!({}), Plan::Direct)),
        TOOL_OPEN => {
            let url = web_address(&need("url")?)?;
            let detail = format!("open {url}");
            Ok((json!({"url": url}), Plan::Site { origin: origin(&url), detail }))
        }
        TOOL_READ => {
            let tab = need("tab")?;
            let what = match text("what") {
                "" | "text" => "text",
                "dom" => "dom",
                "console" => "console",
                other => return Err(format!("unknown `what` `{other}` (text, dom, console)")),
            };
            let mut out = json!({"tab": tab, "what": what});
            if what == "console" {
                out["limit"] = json!(uint("limit", CONSOLE_LIMIT, CONSOLE_LIMIT_MAX));
            } else {
                out["max_chars"] = json!(uint("max_chars", READ_CHARS, READ_CHARS_MAX));
                if !text("selector").is_empty() {
                    out["selector"] = json!(text("selector"));
                }
            }
            let detail = format!("read the page's {what}");
            Ok((out, Plan::Tab { tab, cap: CAP_READ, detail }))
        }
        TOOL_SCREENSHOT => {
            let tab = need("tab")?;
            Ok((json!({"tab": tab}), Plan::Tab { tab, cap: CAP_READ, detail: "screenshot".into() }))
        }
        TOOL_ACT => {
            let tab = need("tab")?;
            let action = need("action")?;
            let mut out = json!({"tab": tab, "action": action});
            let detail = match action.as_str() {
                "navigate" => {
                    let url = web_address(&need("url")?)?;
                    out["url"] = json!(url);
                    let detail = format!("navigate to {url}");
                    return Ok((out, Plan::Site { origin: origin(&url), detail }));
                }
                "click" => {
                    let selector = text("selector");
                    if !selector.is_empty() {
                        out["selector"] = json!(selector);
                        format!("click {}", short(selector, 200))
                    } else if let (Some(x), Some(y)) = (num("x"), num("y")) {
                        out["x"] = json!(x);
                        out["y"] = json!(y);
                        format!("click at {x}, {y}")
                    } else {
                        return Err("click needs `selector` or `x` and `y`".into());
                    }
                }
                "type" => {
                    let t = args.get("text").and_then(Value::as_str).unwrap_or("");
                    if t.is_empty() {
                        return Err("`text` is required".into());
                    }
                    out["text"] = json!(t);
                    let selector = text("selector");
                    if selector.is_empty() {
                        format!("type “{}”", short(t, 200))
                    } else {
                        out["selector"] = json!(selector);
                        format!("type “{}” into {}", short(t, 200), short(selector, 200))
                    }
                }
                "key" => {
                    let key = need("key")?;
                    out["key"] = json!(key);
                    format!("press {key}")
                }
                "scroll" => {
                    let selector = text("selector");
                    if !selector.is_empty() {
                        out["selector"] = json!(selector);
                        format!("scroll to {}", short(selector, 200))
                    } else {
                        let (dx, dy) = (num("dx").unwrap_or(0.0), num("dy").unwrap_or(0.0));
                        let dy = if dx == 0.0 && dy == 0.0 { 600.0 } else { dy };
                        out["dx"] = json!(dx);
                        out["dy"] = json!(dy);
                        if let (Some(x), Some(y)) = (num("x"), num("y")) {
                            out["x"] = json!(x);
                            out["y"] = json!(y);
                        }
                        format!("scroll by {dx}, {dy}")
                    }
                }
                "wait" => {
                    let (selector, t) = (text("selector"), text("text"));
                    let ms = args.get("ms").and_then(Value::as_u64);
                    if selector.is_empty() && t.is_empty() && ms.is_none() {
                        return Err("wait needs `selector`, `text` or `ms`".into());
                    }
                    if !selector.is_empty() {
                        out["selector"] = json!(selector);
                    }
                    if !t.is_empty() {
                        out["text"] = json!(t);
                    }
                    if let Some(ms) = ms {
                        out["ms"] = json!(ms.min(WAIT_MAX_MS));
                    }
                    out["timeout_ms"] = json!(uint("timeout_ms", 10_000, WAIT_MAX_MS));
                    "wait".into()
                }
                "evaluate" => {
                    let e = args.get("expression").and_then(Value::as_str).unwrap_or("");
                    if e.trim().is_empty() {
                        return Err("`expression` is required".into());
                    }
                    out["expression"] = json!(e);
                    format!("run JavaScript: {}", short(e.trim(), 400))
                }
                other => {
                    return Err(format!(
                        "unknown action `{other}` (navigate, click, type, key, scroll, wait, evaluate)"
                    ));
                }
            };
            // Waiting only watches the page: reading it is enough.
            let cap = if action == "wait" { CAP_READ } else { CAP_ACT };
            Ok((out, Plan::Tab { tab, cap, detail }))
        }
        other => Err(format!("unknown tool `{other}`")),
    }
}

/// An address an agent may open: web, local file and inert pages; a bare host gets a scheme
/// (`http` for local hosts and IPs, `https` else). `javascript:` and other schemes are refused.
pub(crate) fn web_address(input: &str) -> Result<String, String> {
    let url = input.trim();
    let lower = url.to_ascii_lowercase();
    if ["http://", "https://", "file:", "data:", "about:"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return Ok(url.to_owned());
    }
    let host = lower.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, p)| {
        if p.chars().all(|c| c.is_ascii_digit()) { h } else { host }
    });
    if host.is_empty() || host.contains(':') || url.chars().any(char::is_whitespace) {
        return Err(format!("not a web address: `{url}` (use http://, https://, file: or data:)"));
    }
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.starts_with('[')
        || host.split('.').count() == 4 && host.split('.').all(|p| p.parse::<u8>().is_ok());
    Ok(format!("{}://{url}", if local { "http" } else { "https" }))
}

/// A screenshot answer of the app (`{"text", "image": {"data", "mime_type"}}`) as a result
/// with an image.
fn screenshot_result(r: AgentToolResult) -> AgentToolResult {
    if r.is_error {
        return r;
    }
    let v: Value = serde_json::from_str(&r.text).unwrap_or(Value::Null);
    let data = v.pointer("/image/data").and_then(Value::as_str).unwrap_or("");
    if data.is_empty() {
        return err("the Meninx window sent no image");
    }
    AgentToolResult {
        text: v.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
        is_error: false,
        image: Some(AgentImage {
            data: data.to_owned(),
            mime_type: v
                .pointer("/image/mime_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_owned(),
        }),
    }
}

impl UserCtx {
    async fn ask_browser(self: &Arc<Self>, session: &str, tool: &str, args: &Value) -> AgentToolResult {
        let apps = self.subscriber_count() > 0;
        crate::bridge::ask_app(&self.bridge, session, tool, args.to_string(), apps, |n| self.notify(n)).await
    }

    /// An `meninx_browser_*` call of `session`'s agent (or of a widget shown in it).
    pub(crate) async fn browser_tool(self: &Arc<Self>, session: &str, tool: &str, args: &str) -> AgentToolResult {
        let args: Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
            Ok(v) => v,
            Err(e) => return err(format!("invalid arguments: {e}")),
        };
        let (mut args, plan) = match plan(tool, &args) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
        let me = match self.resolve_session(session) {
            Ok(s) => s.id.as_str().to_owned(),
            Err(e) => return err(format!("session not available: {e}")),
        };
        match plan {
            Plan::Direct => {}
            Plan::Site { origin, detail } => {
                if let Err(why) = self.approve(&me, CAP_ACT, &origin, &detail).await {
                    return err(why);
                }
            }
            Plan::Tab { tab, cap, detail } => {
                let site = self.ask_browser(&me, TAB_SITE, &json!({"tab": tab})).await;
                if site.is_error {
                    return site;
                }
                let site: Value = serde_json::from_str(&site.text).unwrap_or(Value::Null);
                let Some(origin) = site.get("origin").and_then(Value::as_str).map(str::to_owned) else {
                    return err("the Meninx window did not say which site the tab shows");
                };
                let url = site.get("url").and_then(Value::as_str).unwrap_or("");
                // Acting on a site includes reading it.
                let kept = cap == CAP_READ && self.grants.lock().unwrap().allows(CAP_ACT, &me, &origin);
                if !kept && let Err(why) = self.approve(&me, cap, &origin, &format!("{detail}\n{url}")).await {
                    return err(why);
                }
                // The app refuses when the tab went to another site meanwhile.
                args["origin"] = json!(origin);
            }
        }
        let r = self.ask_browser(&me, tool, &args).await;
        if tool == TOOL_SCREENSHOT { screenshot_result(r) } else { r }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(tool: &str, args: Value) -> Result<(Value, Plan), String> {
        plan(tool, &args)
    }

    #[test]
    fn addresses() {
        assert_eq!(web_address("localhost:5173").unwrap(), "http://localhost:5173");
        assert_eq!(web_address("127.0.0.1:8080/x").unwrap(), "http://127.0.0.1:8080/x");
        assert_eq!(web_address("example.com").unwrap(), "https://example.com");
        assert_eq!(web_address(" https://a.b/c ").unwrap(), "https://a.b/c");
        assert_eq!(web_address("data:text/html,hi").unwrap(), "data:text/html,hi");
        assert!(web_address("javascript:alert(1)").is_err());
        assert!(web_address("two words").is_err());
        assert!(web_address("").is_err());
    }

    #[test]
    fn plans() {
        assert_eq!(p(TOOL_LIST, json!({"x": 1})).unwrap(), (json!({}), Plan::Direct));

        let (a, plan) = p(TOOL_OPEN, json!({"url": "localhost:3000/app"})).unwrap();
        assert_eq!(a, json!({"url": "http://localhost:3000/app"}));
        assert_eq!(plan, Plan::Site { origin: "http://localhost:3000".into(), detail: "open http://localhost:3000/app".into() });
        assert!(p(TOOL_OPEN, json!({})).is_err());

        let (a, plan) = p(TOOL_READ, json!({"tab": "tab-1"})).unwrap();
        assert_eq!(a, json!({"tab": "tab-1", "what": "text", "max_chars": 20000}));
        assert!(matches!(plan, Plan::Tab { cap: CAP_READ, .. }));
        let (a, _) = p(TOOL_READ, json!({"tab": "tab-2", "what": "console", "limit": 9999})).unwrap();
        assert_eq!(a["limit"], 300);
        assert!(p(TOOL_READ, json!({"tab": "tab-1", "what": "cookies"})).is_err());
        assert!(p(TOOL_READ, json!({"what": "text"})).is_err());

        let (a, plan) = p(TOOL_ACT, json!({"tab": "tab-1", "action": "navigate", "url": "https://x.dev/a"})).unwrap();
        assert_eq!(a["url"], "https://x.dev/a");
        assert!(matches!(plan, Plan::Site { origin, .. } if origin == "https://x.dev"));

        let (a, plan) = p(TOOL_ACT, json!({"tab": "tab-1", "action": "click", "selector": "#go"})).unwrap();
        assert_eq!(a, json!({"tab": "tab-1", "action": "click", "selector": "#go"}));
        assert_eq!(plan, Plan::Tab { tab: "tab-1".into(), cap: CAP_ACT, detail: "click #go".into() });
        let (a, _) = p(TOOL_ACT, json!({"tab": "t", "action": "click", "x": 3, "y": 4.5})).unwrap();
        assert_eq!((a["x"].as_f64(), a["y"].as_f64()), (Some(3.0), Some(4.5)));
        assert!(p(TOOL_ACT, json!({"tab": "t", "action": "click", "x": 3})).is_err());

        let (a, _) = p(TOOL_ACT, json!({"tab": "t", "action": "type", "text": " hi ", "selector": "#q"})).unwrap();
        assert_eq!(a["text"], " hi ");
        assert!(p(TOOL_ACT, json!({"tab": "t", "action": "type"})).is_err());
        assert!(p(TOOL_ACT, json!({"tab": "t", "action": "key"})).is_err());

        let (a, _) = p(TOOL_ACT, json!({"tab": "t", "action": "scroll"})).unwrap();
        assert_eq!((a["dx"].as_f64(), a["dy"].as_f64()), (Some(0.0), Some(600.0)));

        let (a, plan) = p(TOOL_ACT, json!({"tab": "t", "action": "wait", "ms": 999999, "timeout_ms": 999999})).unwrap();
        assert_eq!((a["ms"].as_u64(), a["timeout_ms"].as_u64()), (Some(45000), Some(45000)));
        assert!(matches!(plan, Plan::Tab { cap: CAP_READ, .. }));
        assert!(p(TOOL_ACT, json!({"tab": "t", "action": "wait"})).is_err());

        let (_, plan) = p(TOOL_ACT, json!({"tab": "t", "action": "evaluate", "expression": "1+1"})).unwrap();
        assert!(matches!(plan, Plan::Tab { cap: CAP_ACT, detail, .. } if detail == "run JavaScript: 1+1"));
        assert!(p(TOOL_ACT, json!({"tab": "t", "action": "hover"})).is_err());

        let (_, plan) = p(TOOL_SCREENSHOT, json!({"tab": "tab-1"})).unwrap();
        assert!(matches!(plan, Plan::Tab { cap: CAP_READ, .. }));
    }

    #[test]
    fn screenshots() {
        let ok = |t: &str| AgentToolResult { text: t.into(), is_error: false, image: None };
        let r = screenshot_result(ok(r#"{"text":"800x600","image":{"data":"iVBO","mime_type":"image/png"}}"#));
        assert_eq!(r.text, "800x600");
        let mcp = crate::bridge::mcp_content(r.clone());
        assert_eq!(
            mcp,
            json!({"content": [{"type": "text", "text": "800x600"}, {"type": "image", "data": "iVBO", "mimeType": "image/png"}], "isError": false})
        );
        assert_eq!(r.image.unwrap().data, "iVBO");
        assert!(screenshot_result(ok("{}")).is_error);
        assert!(screenshot_result(err("no")).is_error);
    }
}
