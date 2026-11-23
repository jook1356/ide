//! Widgets in the app (`docs/widgets.md`, Hermes Desk's mods): a registered widget is a
//! browser tab of the detail area whose page comes from `ide-widget://<widget>.<session>/...`
//! (`session`: the session showing it; a `global` widget is one 공용 tab). The app answers that
//! scheme (CEF asks through `ide_web::set_scheme_handler`): files through the server
//! (`widget.file`), the page's `window.ide` API (`/__ide/ide.js`, injected into every HTML
//! page) through `widget.call` / `widget.data`. Only the tab the app opened for a widget gets
//! its files and API; API calls must come from the widget's own origin. An agent's request to
//! register a widget is asked here (the 위젯 등록 dialog, then `widget.register`); open tabs are
//! remembered by the server (`widget.set_open`) and restored at start. The 위젯 page and the
//! status bar's launcher are in `widgets_page.rs`.

use std::collections::HashSet;
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
    /// The session showing it (the one it was opened from, for a 공용 tab).
    pub session: String,
    pub id: String,
    pub name: String,
}

/// The registered widgets and the app's widget UI state.
#[derive(Default)]
pub struct WidgetsState {
    /// As last listed (`widget.list`).
    pub list: Vec<WidgetInfo>,
    /// Open tabs (widget, session) already restored or opened in this run: a listing that
    /// still has a tab the user just closed does not bring it back.
    seen: HashSet<(String, String)>,
    /// The status bar's launcher is open.
    pub launcher: bool,
    /// The 위젯 page: the picked widget, its delete armed, the last error.
    pub selected: Option<String>,
    pub delete_armed: bool,
    pub error: Option<String>,
}

impl WidgetsState {
    pub fn get(&self, id: &str) -> Option<&WidgetInfo> {
        self.list.iter().find(|w| w.id == id)
    }
}

/// A `global` widget: one 공용 tab, its data shared by every session.
pub fn is_global(w: &WidgetInfo) -> bool {
    w.scope == "global"
}

/// An agent asks to register a widget (the 위젯 등록 dialog).
pub struct RegisterForm {
    /// The agent request to answer.
    pub request: u64,
    pub session: String,
    pub widget: String,
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub scope: String,
    /// Why it asks: `new`, `other` (another session registered it), `tools` (they changed).
    pub ask: String,
    /// The registering session's title (`other`).
    pub by: String,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WidgetMsg {
    /// The registered widgets.
    Listed(Result<Vec<WidgetInfo>, ProtoError>),
    /// Open (or reload) a widget in a session (menus, launcher, page).
    Open { session: String, widget: String },
    /// Close its tab in a session.
    Close { session: String, widget: String },
    /// The list arrived for opening; `request`: the agent request to answer.
    Fetched {
        session: String,
        widget: String,
        request: Option<u64>,
        result: Result<Vec<WidgetInfo>, ProtoError>,
    },
    /// The registration dialog's 허용.
    Allow,
    Registered(Result<(), ProtoError>),
    /// A request of a widget page (CEF).
    Request(SchemeRequest, Responder),
    DevTools(u32),
    Reload(u32),
    /// The status bar's launcher.
    Launcher(bool),
    /// The 위젯 page: pick a widget, arm / disarm its delete, delete it.
    Select(String),
    DeleteArm(bool),
    Delete(String),
    Deleted(Result<(), ProtoError>),
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
    /// Every browser tab (sessions' and 공용).
    fn all_browsers(&self) -> impl Iterator<Item = &Browser> {
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .chain(self.global_browsers.iter())
    }

    /// The widget tab showing web view `view`.
    fn widget_tab_of(&self, view: ide_web::ViewId) -> Option<&Browser> {
        self.all_browsers()
            .find(|b| b.widget.is_some() && b.view.as_ref().is_some_and(|v| v.id() == view))
    }

    /// The tab of a widget in a session: the 공용 one of a global widget.
    pub(crate) fn widget_tab_id(&self, session: &str, widget: &str) -> Option<u32> {
        if let Some(b) = self
            .global_browsers
            .iter()
            .find(|b| b.widget.as_ref().is_some_and(|w| w.id == widget))
        {
            return Some(b.id);
        }
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .find(|b| b.widget.as_ref().is_some_and(|w| w.session == session && w.id == widget))
            .map(|b| b.id)
    }

    /// Load the registered widgets.
    pub(crate) fn load_widgets(&self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        Task::perform(async move { c.widget_list().await }, |r| Message::Widget(WidgetMsg::Listed(r)))
    }

    /// Fetch the list, then open `widget` (`request`: the agent request to answer).
    fn fetch_and_open(&self, session: String, widget: String, request: Option<u64>) -> Task<Message> {
        let Some(c) = self.online() else {
            return self.reply_agent(request, Err("ide is not connected to its server".into()));
        };
        Task::perform(async move { c.widget_list().await }, move |result| {
            Message::Widget(WidgetMsg::Fetched {
                session,
                widget,
                request,
                result,
            })
        })
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

    /// Tell the server a widget tab was opened / closed (remembered across restarts).
    fn persist_open(&self, session: &str, widget: &str, open: bool) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let (s, w) = (session.to_owned(), widget.to_owned());
        Task::perform(
            async move {
                let _ = c.widget_set_open(&s, &w, open).await;
            },
            |_| (),
        )
        .discard()
    }

    /// The user (or the agent) closed a widget tab.
    pub(crate) fn widget_closed(&self, w: &WidgetTab) -> Task<Message> {
        self.persist_open(&w.session, &w.id, false)
    }

    /// Show a widget in a session: reload its tab when open, else open a new one (a global
    /// widget's is 공용).
    fn show_widget(&mut self, session: &str, w: &WidgetInfo) -> Task<Message> {
        self.widgets.seen.insert((w.id.clone(), session.to_owned()));
        self.widgets.launcher = false;
        let persist = self.persist_open(session, &w.id, true);
        if let Some(id) = self.widget_tab_id(session, &w.id) {
            if let Some(b) = self.browser_mut(id) {
                let url = b
                    .widget
                    .as_mut()
                    .map(|t| {
                        t.name = w.name.clone();
                        widget_url(&t.session, &t.id)
                    })
                    .unwrap_or_default();
                if let Some(v) = &b.view {
                    v.navigate(&url);
                }
            }
            let owner = self.workspace_owner(session);
            self.new_menu = false;
            self.ui.detail_collapsed = false;
            if self.ui.page != crate::ui_state::Page::Work {
                self.ui.page = crate::ui_state::Page::Work;
            }
            let show = self.activate(&owner, ItemKey::Browser(id));
            return Task::batch([show, self.sync_detail(), persist]);
        }
        let tab = WidgetTab {
            session: session.to_owned(),
            id: w.id.clone(),
            name: w.name.clone(),
        };
        self.ui.page = crate::ui_state::Page::Work;
        let open = self.open_browser_tab(
            Some(session.to_owned()),
            is_global(w),
            widget_url(session, &w.id),
            Some(tab),
        );
        Task::batch([open, persist])
    }

    /// Close a widget's tab in a session (its 공용 tab for a global widget).
    fn close_widget(&mut self, session: &str, widget: &str) -> Option<Task<Message>> {
        let id = self.widget_tab_id(session, widget)?;
        let w = self.close_browser(id)?;
        Some(self.widget_closed(&w))
    }

    /// Bring back the tabs open when ide last ran (once per tab and run), quietly.
    fn restore_widget_tabs(&mut self) {
        if !self.loaded {
            return;
        }
        let mut todo = Vec::new();
        for w in &self.widgets.list {
            for s in &w.open {
                let key = (w.id.clone(), s.clone());
                if self.widgets.seen.contains(&key) {
                    continue;
                }
                // A 공용 tab opened from a session that went: shown from another one.
                let session = if self.live(s).is_some() {
                    s.clone()
                } else if is_global(w) {
                    match self.selected.clone().or_else(|| self.sessions.first().map(|l| l.info.id.clone())) {
                        Some(x) => x,
                        None => continue,
                    }
                } else {
                    continue;
                };
                todo.push((key, session, w.clone()));
            }
        }
        for (key, session, w) in todo {
            self.widgets.seen.insert(key);
            if self.widget_tab_id(&session, &w.id).is_some() {
                continue;
            }
            crate::script::log(&format!("widget {} restored in {session}", w.id));
            let tab = WidgetTab {
                session: session.clone(),
                id: w.id.clone(),
                name: w.name.clone(),
            };
            self.add_browser_tab_quietly(&session, is_global(&w), widget_url(&session, &w.id), Some(tab));
        }
    }

    /// Tabs of widgets that are no longer registered close.
    fn drop_unregistered_tabs(&mut self) {
        let gone: Vec<u32> = self
            .all_browsers()
            .filter(|b| {
                b.widget
                    .as_ref()
                    .is_some_and(|w| self.widgets.get(&w.id).is_none())
            })
            .map(|b| b.id)
            .collect();
        for id in gone {
            self.close_browser(id);
        }
    }

    /// The agent's `ide_widget` `register` / `open` / `close` (the server checked the widget).
    pub(crate) fn agent_widget(&mut self, request: u64, session: String, args: &serde_json::Value) -> Task<Message> {
        let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let widget = s("widget");
        crate::script::log(&format!("agent {session}: widget {args}"));
        if self.live(&session).is_none() {
            return self.reply_agent(Some(request), Err("this session is not open in the ide window".into()));
        }
        match s("action").as_str() {
            "register" => {
                if self.modal.is_some() {
                    return self.reply_agent(
                        Some(request),
                        Err("the user is busy with another ide dialog; try again in a moment".into()),
                    );
                }
                let tools = args
                    .get("tools")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                self.modal = Some(Modal::WidgetRegister(RegisterForm {
                    request,
                    session,
                    widget,
                    name: s("name"),
                    description: s("description"),
                    tools,
                    scope: s("scope"),
                    ask: s("ask"),
                    by: s("by"),
                    busy: false,
                    error: None,
                }));
                Task::none()
            }
            "open" => self.fetch_and_open(session, widget, Some(request)),
            "close" => {
                let r = match self.close_widget(&session, &widget) {
                    Some(t) => (Ok("closed".to_owned()), t),
                    None => (Ok("it was not open".to_owned()), Task::none()),
                };
                Task::batch([r.1, self.reply_agent(Some(request), r.0)])
            }
            _ => self.reply_agent(Some(request), Err(format!("{AGENT_TOOL_WIDGET}: unknown action"))),
        }
    }

    /// The registration dialog closes without 허용: the agent is told the user refused.
    pub(crate) fn widget_register_closed(&self) -> Task<Message> {
        match &self.modal {
            Some(Modal::WidgetRegister(f)) => self.reply_agent(
                Some(f.request),
                Err("the user did not allow registering the widget".into()),
            ),
            _ => Task::none(),
        }
    }

    pub(crate) fn on_widget(&mut self, m: WidgetMsg) -> Task<Message> {
        match m {
            WidgetMsg::Listed(r) => {
                if let Ok(list) = r {
                    self.widgets.list = list;
                    if self
                        .widgets
                        .selected
                        .as_deref()
                        .is_some_and(|s| self.widgets.get(s).is_none())
                    {
                        self.widgets.selected = None;
                        self.widgets.delete_armed = false;
                    }
                    self.drop_unregistered_tabs();
                    self.restore_widget_tabs();
                    return self.sync_detail();
                }
                Task::none()
            }
            WidgetMsg::Open { session, widget } => {
                self.new_menu = false;
                match self.widgets.get(&widget).cloned() {
                    Some(w) => self.show_widget(&session, &w),
                    None => self.fetch_and_open(session, widget, None),
                }
            }
            WidgetMsg::Close { session, widget } => {
                self.widgets.launcher = false;
                self.close_widget(&session, &widget).unwrap_or_else(Task::none)
            }
            WidgetMsg::Fetched {
                session,
                widget,
                request,
                result,
            } => {
                let list = match result {
                    Ok(l) => l,
                    Err(e) => return self.reply_agent(request, Err(e.to_string())),
                };
                self.widgets.list = list;
                let Some(w) = self.widgets.get(&widget).cloned() else {
                    if request.is_none()
                        && let Some(d) = self.detail_mut(&session)
                    {
                        d.toast(crate::notices::Tone::Error, tr::widget_gone(&widget));
                    }
                    return self.reply_agent(request, Err(format!("no registered widget `{widget}`")));
                };
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
                let Some(Modal::WidgetRegister(f)) = &mut self.modal else {
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let (s, w, t) = (f.session.clone(), f.widget.clone(), f.tools.clone());
                Task::perform(async move { c.widget_register(&s, &w, t).await }, |r| {
                    Message::Widget(WidgetMsg::Registered(r))
                })
            }
            WidgetMsg::Registered(r) => {
                let Some(Modal::WidgetRegister(f)) = &mut self.modal else {
                    return Task::none();
                };
                match r {
                    Ok(()) => {
                        let (request, widget) = (f.request, f.widget.clone());
                        self.modal = None;
                        let text = format!(
                            "registered; every session can open it now. Show it here with {{\"action\":\"open\",\"widget\":\"{widget}\"}}"
                        );
                        Task::batch([self.reply_agent(Some(request), Ok(text)), self.load_widgets()])
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
            WidgetMsg::Launcher(open) => {
                self.widgets.launcher = open;
                if open { self.load_widgets() } else { Task::none() }
            }
            WidgetMsg::Select(id) => {
                self.widgets.selected = Some(id);
                self.widgets.delete_armed = false;
                self.widgets.error = None;
                Task::none()
            }
            WidgetMsg::DeleteArm(on) => {
                self.widgets.delete_armed = on;
                Task::none()
            }
            WidgetMsg::Delete(id) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                self.widgets.delete_armed = false;
                Task::perform(async move { c.widget_delete(&id).await }, |r| {
                    Message::Widget(WidgetMsg::Deleted(r))
                })
            }
            WidgetMsg::Deleted(r) => {
                if let Err(e) = r {
                    self.widgets.error = Some(crate::i18n::proto_error(&e));
                }
                self.load_widgets()
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

    /// The menu rows of the registered widgets ("+ 새로 열기"), opened in `session`.
    pub(crate) fn widget_menu_rows<'a>(&'a self, session: &str) -> Vec<El<'a>> {
        if self.widgets.list.is_empty() {
            return Vec::new();
        }
        let mut rows: Vec<El<'a>> = vec![
            container(text(tr::new_widgets()).size(11).color(style::DIM))
                .padding([6, 10])
                .boxed(),
        ];
        for w in &self.widgets.list {
            let label = row![
                crate::icons::icon(crate::icons::Icon::Puzzle, 13.0, style::DIM),
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
                .on_press(Message::Widget(WidgetMsg::Open {
                    session: session.to_owned(),
                    widget: w.id.clone(),
                }));
            let hint = if w.description.is_empty() {
                w.id.clone()
            } else {
                w.description.clone()
            };
            rows.push(tip(b.boxed(), hint));
        }
        rows
    }

    /// The registration dialog (Desk's "모드 등록 (모든 세션에서 사용)" ask).
    pub(crate) fn widget_register_card<'a>(&'a self, f: &'a RegisterForm) -> El<'a> {
        let who = self
            .live(&f.session)
            .map_or_else(|| f.session.clone(), |l| l.info.title.clone());
        let body = match f.ask.as_str() {
            "other" if !f.by.is_empty() => tr::widget_register_other(&who, &f.name, &f.by),
            "other" => tr::widget_register_other(&who, &f.name, &tr::widget_session_gone()),
            "tools" => tr::widget_register_tools(&who, &f.name),
            _ => tr::widget_register_new(&who, &f.name),
        };
        let mut c = column![
            text(tr::widget_register_title()).size(18).font(style::bold()).boxed(),
            text(body).size(13).boxed(),
        ]
        .spacing(10);
        if !f.description.is_empty() {
            c = c.push(text(f.description.clone()).size(12).color(style::DIM).boxed());
        }
        c = c.push(
            row![
                text(tr::widget_label_data()).size(12).color(style::DIM).width(90),
                text(crate::widgets_page::scope_text(&f.scope)).size(12),
            ]
            .spacing(8)
            .boxed(),
        );
        if f.tools.is_empty() {
            c = c.push(text(tr::widget_register_no_tools()).size(12).color(style::DIM).boxed());
        } else {
            let mut tools = column![].spacing(4);
            for t in &f.tools {
                tools = tools.push(text(t.clone()).size(12).font(style::mono()));
            }
            c = c.push(text(tr::widget_register_uses()).size(13).boxed());
            c = c.push(container(tools).padding([6, 10]).style(style::card).boxed());
            c = c.push(text(tr::widget_grant_note()).size(12).color(style::DIM).boxed());
        }
        if let Some(e) = &f.error {
            c = c.push(crate::view::error_box(e));
        }
        c.push(
            row![
                space().width(Fill),
                button(text(tr::widget_register_refuse()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(text(tr::widget_register_allow()).size(13))
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
