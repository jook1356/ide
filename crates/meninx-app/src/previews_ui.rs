//! Previews in the app (`docs/previews.md`): an agent's `meninx_preview` opens or reloads a
//! browser tab of the session's detail area marked as a preview (`Browser.preview`). HTML /
//! Markdown / file previews come from `meninx-preview://<id>.<session>/...`, a scheme the app
//! answers through the server (`preview.file`) for the preview's own tab only; unlike widget
//! pages they get no Meninx API. URL previews are ordinary web pages in such a tab (same id =
//! the same tab). No approval: a preview only shows something in the user's own window.

use std::sync::{Arc, Mutex};

use iced::widget::{row, text};
use iced::{Alignment, Element, Fill, Subscription, Task, Widget as _};
use meninx_web::{Responder, SchemeRequest, SchemeResponse};

use crate::app::{App, Message};
use crate::browser::Browser;
use crate::detail::ItemKey;
use crate::i18n::tr;
use crate::style;
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

/// The URL scheme of preview pages.
pub const SCHEME: &str = "meninx-preview";

/// What a preview tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTab {
    /// The session whose agent showed it.
    pub session: String,
    pub id: String,
    pub title: String,
    /// `html`, `markdown`, `file` or `url`.
    pub kind: String,
}

impl PreviewTab {
    /// A page served by Meninx (not a web address): the header shows its title, not an address.
    pub fn is_page(&self) -> bool {
        self.kind != "url"
    }
}

#[derive(Debug, Clone)]
pub enum PreviewMsg {
    /// A request of a preview page (CEF).
    Request(SchemeRequest, Responder),
    Reload(u32),
    DevTools(u32),
}

/// The URL of a preview's page.
pub fn preview_url(session: &str, id: &str) -> String {
    format!("{SCHEME}://{id}.{session}/")
}

/// Console lines of preview pages, kept for the app script (`script_previews.rs`).
static CONSOLE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A preview page logged a line (script mode only).
pub(crate) fn console(id: &str, line: &str) {
    if crate::script::enabled() {
        crate::script::log(&format!("preview {id} console: {line}"));
        CONSOLE.lock().unwrap().push(line.to_owned());
    }
}

pub(crate) fn console_lines() -> Vec<String> {
    CONSOLE.lock().unwrap().clone()
}

pub(crate) fn clear_console() {
    CONSOLE.lock().unwrap().clear();
}

// ---------------------------------------------------------------- scheme requests -> iced

type Item = (SchemeRequest, Responder);
static SUBSCRIBERS: Mutex<Vec<iced::futures::channel::mpsc::UnboundedSender<Item>>> = Mutex::new(Vec::new());

/// Route `meninx-preview://` requests to the app (once, at boot).
pub fn install() {
    meninx_web::set_scheme_handler(
        SCHEME,
        Arc::new(|req, resp: Responder| {
            let mut subs = SUBSCRIBERS.lock().unwrap();
            subs.retain(|tx| !tx.is_closed());
            match subs.first() {
                Some(tx) => {
                    let _ = tx.unbounded_send((req, resp));
                }
                None => resp.respond(SchemeResponse::text(503, "Meninx is starting")),
            }
        }),
    );
}

/// Preview page requests as messages.
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
    .map(|(req, resp)| Message::Preview(PreviewMsg::Request(req, resp)))
}

/// `meninx-preview://<id>.<session>/<path>?q#f` -> (id, session, decoded path with `/`).
fn parse_url(url: &str) -> Option<(String, String, String)> {
    let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let (id, session) = host.split_once('.')?;
    if id.is_empty() || session.is_empty() {
        return None;
    }
    Some((id.to_owned(), session.to_owned(), percent_decode(path)?))
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

impl App {
    /// The tab showing a session's preview.
    pub(crate) fn preview_tab_id(&self, session: &str, id: &str) -> Option<u32> {
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .chain(self.global_browsers.iter())
            .find(|b| b.preview.as_ref().is_some_and(|p| p.session == session && p.id == id))
            .map(|b| b.id)
    }

    /// The preview tab showing web view `view`.
    fn preview_tab_of(&self, view: meninx_web::ViewId) -> Option<&PreviewTab> {
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .chain(self.global_browsers.iter())
            .find(|b| b.view.as_ref().is_some_and(|v| v.id() == view))
            .and_then(|b| b.preview.as_ref())
    }

    fn reply_preview(&self, request: u64, result: Result<String, String>) -> Task<Message> {
        let Some(c) = self.online() else {
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

    /// The agent's `meninx_preview` `show` / `close` (the server checked and stored it).
    pub(crate) fn agent_preview(&mut self, request: u64, session: String, args: &serde_json::Value) -> Task<Message> {
        let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        crate::script::log(&format!("agent {session}: preview {args}"));
        if self.live(&session).is_none() {
            return self.reply_preview(request, Err("this session is not open in the Meninx window".into()));
        }
        let id = s("preview");
        match s("action").as_str() {
            "show" => {
                let tab = PreviewTab {
                    session: session.clone(),
                    id,
                    title: s("title"),
                    kind: s("kind"),
                };
                let (show, how) = self.show_preview(tab, s("url"));
                Task::batch([show, self.reply_preview(request, Ok(how.to_owned()))])
            }
            "close" => {
                let r = match self.preview_tab_id(&session, &id) {
                    Some(tab) => {
                        self.close_browser(tab);
                        "closed"
                    }
                    None => "it was not open",
                };
                Task::batch([self.sync_detail(), self.reply_preview(request, Ok(r.to_owned()))])
            }
            _ => self.reply_preview(request, Err(format!("{}: unknown action", meninx_client::AGENT_TOOL_PREVIEW))),
        }
    }

    /// Open a preview's tab, or reload it when open; shows it in the session's pane.
    fn show_preview(&mut self, tab: PreviewTab, url: String) -> (Task<Message>, &'static str) {
        let owner = self.workspace_owner(&tab.session);
        let how = if let Some(id) = self.preview_tab_id(&tab.session, &tab.id) {
            if let Some(b) = self.browser_mut(id) {
                b.preview = Some(tab);
                b.url_input = url.clone();
                b.error = None;
                if let Some(v) = &b.view {
                    v.navigate(&url);
                }
            }
            self.new_menu = false;
            self.ui.detail_collapsed = false;
            if self.ui.page != crate::ui_state::Page::Work {
                self.ui.page = crate::ui_state::Page::Work;
            }
            let show = self.activate(&owner, ItemKey::Browser(id));
            return (Task::batch([show, self.sync_detail()]), "reloaded");
        } else {
            "opened"
        };
        self.next_browser += 1;
        let id = self.next_browser;
        let mut b = Browser::new(id, url);
        b.preview = Some(tab);
        let Some(l) = self.live_mut(&owner) else {
            return (Task::none(), "this session is not open in the Meninx window");
        };
        l.browsers.push(b);
        self.ui.page = crate::ui_state::Page::Work;
        self.new_menu = false;
        self.rail_open = false;
        self.ui.detail_collapsed = false;
        let show = self.activate(&owner, ItemKey::Browser(id));
        (Task::batch([show, self.sync_detail()]), how)
    }

    pub(crate) fn on_preview(&mut self, m: PreviewMsg) -> Task<Message> {
        match m {
            PreviewMsg::Request(req, resp) => self.serve_preview(req, resp),
            PreviewMsg::Reload(id) => {
                if let Some(b) = self.browser_mut(id)
                    && let (Some(v), Some(p)) = (&b.view, &b.preview)
                {
                    v.navigate(&preview_url(&p.session, &p.id));
                }
                Task::none()
            }
            PreviewMsg::DevTools(id) => {
                if let Some(v) = self.browser_mut(id).and_then(|b| b.view.as_ref()) {
                    v.show_dev_tools();
                }
                Task::none()
            }
        }
    }

    /// Answer a preview page's request: only for the tab showing that preview.
    fn serve_preview(&mut self, req: SchemeRequest, resp: Responder) -> Task<Message> {
        let Some((id, session, path)) = parse_url(&req.url) else {
            resp.respond(SchemeResponse::text(400, "bad preview URL"));
            return Task::none();
        };
        let tab = req.view.and_then(|v| self.preview_tab_of(v));
        if tab.is_none_or(|t| t.id != id || t.session != session || !t.is_page()) {
            resp.respond(SchemeResponse::text(403, "not a preview tab of Meninx"));
            return Task::none();
        }
        if req.method != "GET" && req.method != "HEAD" {
            resp.respond(SchemeResponse::text(405, "previews are read only"));
            return Task::none();
        }
        let Some(c) = self.online() else {
            resp.respond(SchemeResponse::text(503, "Meninx is not connected to its server"));
            return Task::none();
        };
        Task::perform(
            async move {
                resp.respond(match c.preview_file(&session, &id, &path).await {
                    Ok((mime, data)) => SchemeResponse::new(200, mime, data),
                    Err(e) => SchemeResponse::text(404, e.get("detail").unwrap_or(&e.to_string()).to_owned()),
                });
            },
            |_| (),
        )
        .discard()
    }

    /// The pane's header for a preview page: its title, reload, developer tools.
    pub(crate) fn preview_identity<'a>(&'a self, b: &'a Browser, p: &'a PreviewTab) -> El<'a> {
        row![
            tip(text("▣").size(13).color(style::DIM).boxed(), tr::preview_tip()),
            text(p.title.clone())
                .size(13)
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::End)
                .width(Fill),
            icon_button("⟳", Some(Message::Preview(PreviewMsg::Reload(b.id))), tr::web_reload()),
            icon_button("{}", Some(Message::Preview(PreviewMsg::DevTools(b.id))), tr::preview_devtools()),
        ]
        .spacing(6)
        .padding([0, 4])
        .align_y(Alignment::Center)
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(preview_url("s1", "chart"), "meninx-preview://chart.s1/");
        assert_eq!(
            parse_url("meninx-preview://chart.s1/img/a%20b.png?x=1#y"),
            Some(("chart".into(), "s1".into(), "/img/a b.png".into()))
        );
        assert_eq!(parse_url("meninx-preview://chart.s1"), Some(("chart".into(), "s1".into(), "/".into())));
        assert_eq!(parse_url("meninx-widget://chart.s1/"), None);
        assert_eq!(parse_url("meninx-preview://chart/"), None);
        assert_eq!(parse_url("meninx-preview://chart.s1/%zz"), None);
        let page = PreviewTab {
            session: "s1".into(),
            id: "chart".into(),
            title: "Chart".into(),
            kind: "html".into(),
        };
        assert!(page.is_page());
        assert!(!PreviewTab { kind: "url".into(), ..page }.is_page());
    }
}
