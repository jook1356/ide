//! Widgets in the app (`docs/widgets.md`): a widget is a browser tab of the detail area whose
//! page comes from `ide-widget://<widget>.<session>/...`. The app answers that scheme (CEF
//! asks through `ide_web::set_scheme_handler`): files through the server (`widget.file`),
//! the page's `window.ide` API (`/__ide/ide.js`, injected into every HTML page) through
//! `widget.call` / `widget.data`. Only the tab the app opened for a widget gets its files and
//! API; API calls must come from the widget's own origin. A widget that declares ide tools is
//! opened only after the user approved them (`widget.grant`).

use std::sync::{Arc, Mutex};

use iced::widget::{button, column, container, row, space, text};
use iced::{Alignment, Element, Fill, Subscription, Task, Widget as _};
use ide_client::{AGENT_TOOL_WIDGET, ProtoError, WidgetDataOp, WidgetInfo};
use ide_web::{Responder, SchemeRequest, SchemeResponse};

use crate::app::{App, Message, Modal};
use crate::browser::Browser;
use crate::detail::ItemKey;
use crate::i18n::tr;
use crate::style;
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

/// The URL scheme of widget pages.
pub const SCHEME: &str = "ide-widget";
/// Path prefix of ide's own endpoints on a widget's origin.
const API: &str = "/__ide/";

/// What a widget tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetTab {
    pub session: String,
    pub id: String,
    pub name: String,
}

/// The approval dialog of a widget's ide tools.
pub struct GrantForm {
    pub session: String,
    pub widget: String,
    pub name: String,
    pub tools: Vec<String>,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WidgetMsg {
    /// The widgets of a session (for "+ 새로 열기").
    Listed(String, Result<Vec<WidgetInfo>, ProtoError>),
    /// Open (or reload) a widget from the menu.
    Open { session: String, widget: String },
    /// A widget's info arrived for opening; `request`: the agent request to answer.
    Info {
        session: String,
        widget: String,
        request: Option<u64>,
        result: Result<Vec<WidgetInfo>, ProtoError>,
    },
    /// The approval dialog's 허용.
    Allow,
    Granted(Result<(), ProtoError>),
    /// A request of a widget page (CEF).
    Request(SchemeRequest, Responder),
    DevTools(u32),
    Reload(u32),
}

/// Console lines of widget pages, kept for the app script (`script_widgets.rs`).
static CONSOLE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A widget page logged a line (script mode: the log and [`console_lines`]).
pub(crate) fn console(widget: &str, line: &str) {
    if crate::script::enabled() {
        crate::script::log(&format!("widget {widget} console: {line}"));
        CONSOLE.lock().unwrap().push(line.to_owned());
    }
}

pub(crate) fn console_lines() -> Vec<String> {
    CONSOLE.lock().unwrap().clone()
}

pub(crate) fn clear_console() {
    CONSOLE.lock().unwrap().clear();
}

/// The URL of a widget's page.
pub fn widget_url(session: &str, widget: &str) -> String {
    format!("{SCHEME}://{widget}.{session}/")
}

// ---------------------------------------------------------------- scheme requests -> iced

type Item = (SchemeRequest, Responder);
static SUBSCRIBERS: Mutex<Vec<iced::futures::channel::mpsc::UnboundedSender<Item>>> =
    Mutex::new(Vec::new());

/// Route `ide-widget://` requests to the app (once, at boot).
pub fn install() {
    ide_web::set_scheme_handler(
        SCHEME,
        Arc::new(|req, resp: Responder| {
            let mut subs = SUBSCRIBERS.lock().unwrap();
            subs.retain(|tx| !tx.is_closed());
            match subs.first() {
                Some(tx) => {
                    let _ = tx.unbounded_send((req, resp));
                }
                None => resp.respond(SchemeResponse::text(503, "ide is starting")),
            }
        }),
    );
}

/// Widget page requests as messages.
pub fn subscription() -> Subscription<Message> {
    use iced::futures::stream::{self, StreamExt as _};
    Subscription::run(|| {
        stream::once(async {
            let (tx, rx) = iced::futures::channel::mpsc::unbounded();
            SUBSCRIBERS.lock().unwrap().push(tx);
            rx
        })
        .flatten()
    })
    .map(|(req, resp)| Message::Widget(WidgetMsg::Request(req, resp)))
}

/// `ide-widget://<widget>.<session>/<path>?q#f` -> (widget, session, decoded path with `/`).
fn parse_url(url: &str) -> Option<(String, String, String)> {
    let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let (widget, session) = host.split_once('.')?;
    if widget.is_empty() || session.is_empty() {
        return None;
    }
    Some((widget.to_owned(), session.to_owned(), percent_decode(path)?))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The origin (`scheme://host`) of a URL.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    match url[after..].find('/') {
        Some(i) => &url[..after + i],
        None => url,
    }
}

/// The script tag of `window.ide`, put at the start of the page's `<head>` (or of the page)
/// so it runs before the page's own scripts.
fn inject_bridge(html: &str) -> String {
    const TAG: &str = "<script src=\"/__ide/ide.js\"></script>";
    let lower = html.to_ascii_lowercase();
    let after_tag = |name: &str| -> Option<usize> {
        let mut from = 0;
        while let Some(i) = lower[from..].find(name) {
            let at = from + i;
            let next = lower.as_bytes().get(at + name.len()).copied();
            if matches!(next, Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')) {
                return lower[at..].find('>').map(|j| at + j + 1);
            }
            from = at + name.len();
        }
        None
    };
    let at = after_tag("<head")
        .or_else(|| after_tag("<html"))
        .or_else(|| after_tag("<!doctype"))
        .unwrap_or(0);
    let mut out = String::with_capacity(html.len() + TAG.len());
    out.push_str(&html[..at]);
    out.push_str(TAG);
    out.push_str(&html[at..]);
    out
}

/// `window.ide` (served as `/__ide/ide.js`).
const IDE_JS: &str = r#"(() => {
  if (window.ide) return;
  const dot = location.hostname.indexOf('.');
  const id = location.hostname.slice(0, dot), session = location.hostname.slice(dot + 1);
  async function post(path, body) {
    const r = await fetch('/__ide/' + path, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    });
    let j;
    try { j = await r.json(); } catch { throw new Error('ide: HTTP ' + r.status); }
    if (!j.ok) throw new Error(j.error || ('ide: HTTP ' + r.status));
    return j;
  }
  const parse = (t) => { try { return JSON.parse(t); } catch { return t; } };
  window.ide = Object.freeze({
    widget: Object.freeze({ id, session }),
    async call(tool, args) { return parse((await post('call', { tool, args: args ?? {} })).text); },
    data: Object.freeze({
      async get(key) { const v = (await post('data', { op: 'get', key })).value; return v == null ? undefined : JSON.parse(v); },
      async set(key, value) { await post('data', { op: 'set', key, value: JSON.stringify(value === undefined ? null : value) }); },
      async delete(key) { await post('data', { op: 'delete', key }); },
      async keys() { return JSON.parse((await post('data', { op: 'keys' })).value); },
    }),
  });
})();
"#;

fn json(v: serde_json::Value) -> SchemeResponse {
    SchemeResponse::new(200, "application/json; charset=utf-8", v.to_string().into_bytes())
}

fn api_error(e: impl Into<String>) -> SchemeResponse {
    json(serde_json::json!({ "ok": false, "error": e.into() }))
}

impl App {
    /// The widget tab showing web view `view`.
    fn widget_tab_of(&self, view: ide_web::ViewId) -> Option<&Browser> {
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .find(|b| b.widget.is_some() && b.view.as_ref().is_some_and(|v| v.id() == view))
    }

    /// The tab of a widget, if open.
    fn widget_tab_id(&self, session: &str, widget: &str) -> Option<u32> {
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .find(|b| b.widget.as_ref().is_some_and(|w| w.session == session && w.id == widget))
            .map(|b| b.id)
    }

    /// Load a session's widgets (for "+ 새로 열기").
    pub(crate) fn load_widgets(&self, session: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let s = session.to_owned();
        Task::perform(async move { (s.clone(), c.widget_list(&s).await) }, |(s, r)| {
            Message::Widget(WidgetMsg::Listed(s, r))
        })
    }

    /// Fetch a widget's info, then open it (`request`: the agent request to answer).
    fn fetch_and_open(&self, session: String, widget: String, request: Option<u64>) -> Task<Message> {
        let Some(c) = self.online() else {
            return self.reply_agent(request, Err("ide is not connected to its server".into()));
        };
        Task::perform(
            async move {
                let r = c.widget_list(&session).await;
                (session, widget, r)
            },
            move |(session, widget, result)| {
                Message::Widget(WidgetMsg::Info {
                    session,
                    widget,
                    request,
                    result,
                })
            },
        )
    }

    /// Answer an agent request (none: nothing to do).
    fn reply_agent(&self, request: Option<u64>, result: Result<String, String>) -> Task<Message> {
        let (Some(request), Some(c)) = (request, self.online()) else {
            return Task::none();
        };
        Task::perform(
            async move {
                let _ = c.agent_reply(request, result).await;
            },
            |_| (),
        )
        .discard()
    }

    /// Show a widget: reload its tab when open, else open a new one.
    fn show_widget(&mut self, session: &str, w: &WidgetInfo) -> Task<Message> {
        if let Some(id) = self.widget_tab_id(session, &w.id) {
            if let Some(b) = self.browser_mut(id) {
                if let Some(t) = &mut b.widget {
                    t.name = w.name.clone();
                }
                if let Some(v) = &b.view {
                    v.navigate(&widget_url(session, &w.id));
                }
            }
            let owner = self.workspace_owner(session);
            self.new_menu = false;
            self.ui.detail_collapsed = false;
            let show = self.activate(&owner, ItemKey::Browser(id));
            return Task::batch([show, self.sync_detail()]);
        }
        let tab = WidgetTab {
            session: session.to_owned(),
            id: w.id.clone(),
            name: w.name.clone(),
        };
        self.open_browser_tab(Some(session.to_owned()), false, widget_url(session, &w.id), Some(tab))
    }

    /// The agent's `ide_widget` `open` / `close` (the server checked the widget).
    pub(crate) fn agent_widget(&mut self, request: u64, session: String, args: &serde_json::Value) -> Task<Message> {
        let widget = args.get("widget").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        crate::script::log(&format!("agent {session}: widget {args}"));
        if self.live(&session).is_none() {
            return self.reply_agent(Some(request), Err("this session is not open in the ide window".into()));
        }
        match args.get("action").and_then(|v| v.as_str()) {
            Some("open") => self.fetch_and_open(session, widget, Some(request)),
            Some("close") => {
                let r = match self.widget_tab_id(&session, &widget) {
                    Some(id) => {
                        self.close_browser(id);
                        Ok("closed".to_owned())
                    }
                    None => Ok("it was not open".to_owned()),
                };
                self.reply_agent(Some(request), r)
            }
            _ => self.reply_agent(Some(request), Err(format!("{AGENT_TOOL_WIDGET}: unknown action"))),
        }
    }

    pub(crate) fn on_widget(&mut self, m: WidgetMsg) -> Task<Message> {
        match m {
            WidgetMsg::Listed(s, r) => {
                match r {
                    Ok(list) => {
                        self.widgets.insert(s, list);
                    }
                    Err(_) => {
                        self.widgets.remove(&s);
                    }
                }
                Task::none()
            }
            WidgetMsg::Open { session, widget } => {
                self.new_menu = false;
                self.fetch_and_open(session, widget, None)
            }
            WidgetMsg::Info {
                session,
                widget,
                request,
                result,
            } => {
                let list = match result {
                    Ok(l) => l,
                    Err(e) => return self.reply_agent(request, Err(e.to_string())),
                };
                self.widgets.insert(session.clone(), list.clone());
                let Some(w) = list.into_iter().find(|w| w.id == widget) else {
                    return self.reply_agent(request, Err(format!("no widget `{widget}`")));
                };
                if let Some(e) = &w.error {
                    if request.is_none()
                        && let Some(d) = self.detail_mut(&session)
                    {
                        d.toast(tr::widget_broken_toast(&w.name, e));
                    }
                    return self.reply_agent(request, Err(e.clone()));
                }
                if !w.granted {
                    self.modal = Some(Modal::WidgetGrant(GrantForm {
                        session,
                        widget: w.id.clone(),
                        name: w.name.clone(),
                        tools: w.tools.clone(),
                        busy: false,
                        error: None,
                    }));
                    return self.reply_agent(
                        request,
                        Ok("the user is asked to allow the widget's ide tools; it opens once they do".into()),
                    );
                }
                let reloaded = self.widget_tab_id(&session, &w.id).is_some();
                let show = self.show_widget(&session, &w);
                let answer = self.reply_agent(
                    request,
                    Ok(if reloaded { "reloaded" } else { "opened" }.to_owned()),
                );
                Task::batch([show, answer])
            }
            WidgetMsg::Allow => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let Some(Modal::WidgetGrant(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let (s, w, t) = (f.session.clone(), f.widget.clone(), f.tools.clone());
                Task::perform(async move { c.widget_grant(&s, &w, t).await }, |r| {
                    Message::Widget(WidgetMsg::Granted(r))
                })
            }
            WidgetMsg::Granted(r) => {
                let Some(Modal::WidgetGrant(f)) = &mut self.modal else {
                    return Task::none();
                };
                match r {
                    Ok(()) => {
                        let (session, widget) = (f.session.clone(), f.widget.clone());
                        self.modal = None;
                        self.fetch_and_open(session, widget, None)
                    }
                    Err(e) => {
                        f.busy = false;
                        f.error = Some(crate::i18n::proto_error(&e));
                        Task::none()
                    }
                }
            }
            WidgetMsg::Request(req, resp) => self.serve_widget(req, resp),
            WidgetMsg::DevTools(id) => {
                if let Some(v) = self.browser_mut(id).and_then(|b| b.view.as_ref()) {
                    v.show_dev_tools();
                }
                Task::none()
            }
            WidgetMsg::Reload(id) => {
                if let Some(b) = self.browser_mut(id)
                    && let (Some(v), Some(w)) = (&b.view, &b.widget)
                {
                    v.navigate(&widget_url(&w.session, &w.id));
                }
                Task::none()
            }
        }
    }

    /// Answer a widget page's request: only for the tab the app opened for that widget.
    fn serve_widget(&mut self, req: SchemeRequest, resp: Responder) -> Task<Message> {
        let Some((widget, session, path)) = parse_url(&req.url) else {
            resp.respond(SchemeResponse::text(400, "bad widget URL"));
            return Task::none();
        };
        let tab = req.view.and_then(|v| self.widget_tab_of(v)).and_then(|b| b.widget.clone());
        if tab.as_ref().is_none_or(|t| t.id != widget || t.session != session) {
            resp.respond(SchemeResponse::text(403, "not a widget tab of ide"));
            return Task::none();
        }
        let Some(c) = self.online() else {
            resp.respond(SchemeResponse::text(503, "ide is not connected to its server"));
            return Task::none();
        };
        if let Some(endpoint) = path.strip_prefix(API) {
            if endpoint == "ide.js" {
                resp.respond(SchemeResponse::new(200, "text/javascript; charset=utf-8", IDE_JS));
                return Task::none();
            }
            // The API answers the widget's own page only (not pages it embeds).
            if req.method != "POST" || origin(&req.frame_url) != origin(&req.url) {
                resp.respond(SchemeResponse::text(403, "ide API: POST from the widget's page only"));
                return Task::none();
            }
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
            let text = |k: &str| body.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
            match endpoint {
                "call" => {
                    let tool = text("tool");
                    let args = body.get("args").cloned().unwrap_or_else(|| serde_json::json!({})).to_string();
                    crate::script::log(&format!("widget {widget}: call {tool}"));
                    return Task::perform(
                        async move {
                            resp.respond(match c.widget_call(&session, &widget, &tool, &args).await {
                                Ok(r) if !r.is_error => json(serde_json::json!({ "ok": true, "text": r.text })),
                                Ok(r) => api_error(r.text),
                                Err(e) => api_error(crate::i18n::proto_error(&e)),
                            });
                        },
                        |_| (),
                    )
                    .discard();
                }
                "data" => {
                    let key = text("key");
                    let op = match text("op").as_str() {
                        "get" => WidgetDataOp::Get { key },
                        "set" => WidgetDataOp::Set { key, value: text("value") },
                        "delete" => WidgetDataOp::Delete { key },
                        "keys" => WidgetDataOp::Keys,
                        _ => {
                            resp.respond(api_error("ide.data: unknown op"));
                            return Task::none();
                        }
                    };
                    return Task::perform(
                        async move {
                            resp.respond(match c.widget_data(&session, &widget, op).await {
                                Ok(v) => json(serde_json::json!({ "ok": true, "value": v })),
                                Err(e) => api_error(crate::i18n::proto_error(&e)),
                            });
                        },
                        |_| (),
                    )
                    .discard();
                }
                _ => {
                    resp.respond(SchemeResponse::text(404, "unknown ide endpoint"));
                    return Task::none();
                }
            }
        }
        Task::perform(
            async move {
                resp.respond(match c.widget_file(&session, &widget, &path).await {
                    Ok((mime, data)) if mime.starts_with("text/html") => {
                        let html = String::from_utf8_lossy(&data);
                        SchemeResponse::new(200, mime, inject_bridge(&html).into_bytes())
                    }
                    Ok((mime, data)) => SchemeResponse::new(200, mime, data),
                    Err(e) => SchemeResponse::text(404, e.get("detail").unwrap_or(&e.to_string()).to_owned()),
                });
            },
            |_| (),
        )
        .discard()
    }

    /// The pane's header for a widget tab: its name, reload, developer tools.
    pub(crate) fn widget_identity<'a>(&'a self, b: &'a Browser, w: &'a WidgetTab) -> El<'a> {
        row![
            text("◇").size(13).color(style::DIM),
            text(w.name.clone())
                .size(13)
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::End)
                .width(Fill),
            icon_button("⟳", Some(Message::Widget(WidgetMsg::Reload(b.id))), tr::web_reload()),
            icon_button("{}", Some(Message::Widget(WidgetMsg::DevTools(b.id))), tr::widget_devtools()),
        ]
        .spacing(6)
        .padding([0, 4])
        .align_y(Alignment::Center)
        .boxed()
    }

    /// The menu rows of the session's widgets ("+ 새로 열기").
    pub(crate) fn widget_menu_rows<'a>(&'a self, session: &str) -> Vec<El<'a>> {
        let Some(list) = self.widgets.get(session).filter(|l| !l.is_empty()) else {
            return Vec::new();
        };
        let mut rows: Vec<El<'a>> = vec![
            container(text(tr::new_widgets()).size(11).color(style::DIM))
                .padding([6, 10])
                .boxed(),
        ];
        for w in list {
            let label = row![
                text("◇").size(12).color(style::DIM),
                text(w.name.clone())
                    .size(13)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::End)
                    .width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            let b = button(label)
                .padding([5, 10])
                .width(Fill)
                .style(style::row(false))
                .on_press_maybe(w.error.is_none().then(|| {
                    Message::Widget(WidgetMsg::Open {
                        session: session.to_owned(),
                        widget: w.id.clone(),
                    })
                }));
            let hint = match &w.error {
                Some(e) => format!("{}\n{e}", tr::widget_broken()),
                None if !w.description.is_empty() => w.description.clone(),
                None => w.id.clone(),
            };
            rows.push(tip(b.boxed(), hint));
        }
        rows
    }

    /// The approval dialog.
    pub(crate) fn widget_grant_card<'a>(&'a self, f: &'a GrantForm) -> El<'a> {
        let mut tools = column![].spacing(4);
        for t in &f.tools {
            tools = tools.push(text(t.clone()).size(12).font(style::MONO));
        }
        let mut c = column![
            text(tr::widget_grant_title()).size(18).font(style::BOLD),
            text(tr::widget_grant_body(&f.name)).size(13),
            container(tools).padding([6, 10]).style(style::card),
            text(tr::widget_grant_note()).size(12).color(style::DIM),
        ]
        .spacing(10);
        if let Some(e) = &f.error {
            c = c.push(crate::view::error_box(e));
        }
        c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::CloseModal),
                button(text(tr::widget_grant_allow()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe((!f.busy).then_some(Message::Widget(WidgetMsg::Allow))),
            ]
            .spacing(8)
            .boxed(),
        )
        .width(480)
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(widget_url("s1", "board"), "ide-widget://board.s1/");
        assert_eq!(
            parse_url("ide-widget://board.s1/js/a%20b.js?x=1#y"),
            Some(("board".into(), "s1".into(), "/js/a b.js".into()))
        );
        assert_eq!(parse_url("ide-widget://board.s1"), Some(("board".into(), "s1".into(), "/".into())));
        assert_eq!(parse_url("https://board.s1/"), None);
        assert_eq!(parse_url("ide-widget://board/"), None);
        assert_eq!(parse_url("ide-widget://board.s1/%zz"), None);
        assert_eq!(origin("ide-widget://board.s1/a/b"), "ide-widget://board.s1");
        assert_eq!(origin("ide-widget://board.s1"), "ide-widget://board.s1");
        assert_ne!(origin("https://evil.example/x"), origin("ide-widget://board.s1/"));
    }

    #[test]
    fn bridge_goes_first_in_head() {
        let tag = "<script src=\"/__ide/ide.js\"></script>";
        assert_eq!(
            inject_bridge("<!DOCTYPE html><html lang=\"en\"><HEAD><title>x</title></HEAD></html>"),
            format!("<!DOCTYPE html><html lang=\"en\"><HEAD>{tag}<title>x</title></HEAD></html>")
        );
        // `<header>` is not `<head>`.
        assert_eq!(
            inject_bridge("<html><body><header>h</header></body></html>"),
            format!("<html>{tag}<body><header>h</header></body></html>")
        );
        assert_eq!(inject_bridge("<!doctype html><p>x"), format!("<!doctype html>{tag}<p>x"));
        assert_eq!(inject_bridge("<p>x</p>"), format!("{tag}<p>x</p>"));
    }
}
