//! Browser tabs of the detail area beyond the session's own web page (Hermes Desk's
//! `BrowserPane`, `main/browsers.ts`), opened from "+ 새로 열기" (address or 즐겨찾기) or by a
//! page's "open in a new window"; 공용 ones belong to no session. And the 즐겨찾기 (bookmarks,
//! `ui.json`, shared by every session), toggled with ☆ in a page's header.

use iced::widget::{button, container, row, text, text_input};
use iced::{Alignment, Element, Fill, Size, Task, Widget as _};
use ide_web::{WebView, WebViewWidget};

use crate::app::{App, Live, Message, URL_ID};
use crate::detail::ItemKey;
use crate::i18n::tr;
use crate::style;
use crate::ui_state::Bookmark;
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

/// Google search for what is not an address (Desk's `SEARCH_URL`).
const SEARCH_URL: &str = "https://www.google.com/search?q=";

/// One browser tab.
pub struct Browser {
    pub id: u32,
    /// Created when first shown.
    pub view: Option<WebView>,
    pub url_input: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub error: Option<String>,
    /// The tab shows a widget (`widgets_ui.rs`): its page is served by ide, the header shows
    /// its name instead of the address.
    pub widget: Option<crate::widgets_ui::WidgetTab>,
}

impl Browser {
    fn new(id: u32, url: String) -> Self {
        Self {
            id,
            view: None,
            url_input: url,
            title: String::new(),
            loading: false,
            can_back: false,
            can_forward: false,
            error: None,
            widget: None,
        }
    }

    /// Name in 열린 항목 (Desk: title, else the address, else 새 탭; a widget's name).
    pub fn label(&self) -> String {
        if let Some(w) = &self.widget {
            w.name.clone()
        } else if !self.title.trim().is_empty() {
            self.title.clone()
        } else if !self.url_input.trim().is_empty() {
            self.url_input.clone()
        } else {
            tr::browser_new_tab()
        }
    }
}

#[derive(Debug, Clone)]
pub enum BrowserMsg {
    /// Open `url` in a new tab of the session (`global`: 공용).
    Open { global: bool, url: String },
    /// "+ 새로 열기"'s address field and its submit.
    Address(String),
    AddressSubmit,
    /// ☆ of a page: add / remove the bookmark.
    BookmarkToggle { url: String, title: String },
    BookmarkRemove(String),
    /// Rename a bookmark: start, typing, done.
    BookmarkEdit(String),
    BookmarkEditInput(String),
    BookmarkEditDone,
}

/// What the address field means (Desk's `addressToUrl`): a web address (http for local hosts
/// and IPs, https else) or a Google search.
pub fn address_to_url(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    let search = || format!("{SEARCH_URL}{}", encode(text));
    if text.chars().any(char::is_whitespace) {
        return Some(search());
    }
    let lower = text.to_ascii_lowercase();
    // Web addresses, and the page schemes a browser opens as they are (Desk searches every
    // other scheme; `data:` / `about:` / `file:` pages must still open here).
    if ["http://", "https://", "data:", "about:", "file:", "view-source:"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return Some(text.to_owned());
    }
    // Another scheme (not `host:port`) is searched.
    if let Some((scheme, rest)) = lower.split_once(':')
        && !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
        && !rest.starts_with(|c: char| c.is_ascii_digit())
    {
        return Some(search());
    }
    let host_end = lower.find(['/', '?', '#']).unwrap_or(lower.len());
    let hostport = &lower[..host_end];
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, Some(p)),
        _ => (hostport, None),
    };
    let loopback = matches!(host, "localhost" | "0.0.0.0" | "[::1]");
    let ip = host.starts_with('[')
        || (host.split('.').count() == 4 && host.split('.').all(|p| p.parse::<u8>().is_ok()));
    let labels: Vec<&str> = host.split('.').collect();
    let domain = labels.len() > 1
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|l| l.starts_with(|c: char| c.is_ascii_alphabetic()));
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()));
    if !port_ok || (!loopback && !ip && !domain && port.is_none()) {
        return Some(search());
    }
    let scheme = if loopback || ip { "http" } else { "https" };
    Some(format!("{scheme}://{text}"))
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl App {
    /// A browser tab of `live`'s session or a 공용 one.
    pub fn browser<'a>(&'a self, live: &'a Live, id: u32) -> Option<&'a Browser> {
        live.browsers
            .iter()
            .chain(self.global_browsers.iter())
            .find(|b| b.id == id)
    }

    pub(crate) fn browser_mut(&mut self, id: u32) -> Option<&mut Browser> {
        if let Some(i) = self.global_browsers.iter().position(|b| b.id == id) {
            return self.global_browsers.get_mut(i);
        }
        self.sessions
            .iter_mut()
            .flat_map(|l| l.browsers.iter_mut())
            .find(|b| b.id == id)
    }

    /// The browser tab shown in the selected session's pane, if one is.
    pub(crate) fn shown_browser(&self) -> Option<u32> {
        let live = self.shown_live()?;
        match &live.detail.active {
            ItemKey::Browser(id) if self.browser(live, *id).is_some() => Some(*id),
            _ => None,
        }
    }

    /// Create the view of the shown browser tab and hide every other tab.
    pub(crate) fn sync_browsers(&mut self, shown: Option<u32>) {
        let runtime = self.runtime.clone();
        if let Some(id) = shown
            && let Some(b) = self.browser_mut(id)
            && b.view.is_none()
        {
            let url = if b.widget.is_some() {
                b.url_input.clone()
            } else {
                address_to_url(&b.url_input).unwrap_or_else(|| crate::app::DEFAULT_URL.into())
            };
            match WebView::new(&runtime, &url, Size::new(640.0, 600.0), 1.0) {
                Ok(v) => b.view = Some(v),
                Err(e) => b.error = Some(tr::err_web_view(&format!("{e:#}"))),
            }
        }
        let all = self
            .sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .chain(self.global_browsers.iter());
        for b in all {
            if let Some(v) = &b.view {
                v.set_hidden(Some(b.id) != shown);
            }
        }
    }

    /// Open a new browser tab and show it in the selected session's pane.
    pub(crate) fn open_browser(&mut self, sid: Option<String>, global: bool, url: String) -> Task<Message> {
        self.open_browser_tab(sid, global, url, None)
    }

    /// Open a new browser tab (a widget's when `widget` is set).
    pub(crate) fn open_browser_tab(
        &mut self,
        sid: Option<String>,
        global: bool,
        url: String,
        widget: Option<crate::widgets_ui::WidgetTab>,
    ) -> Task<Message> {
        self.next_browser += 1;
        let id = self.next_browser;
        let mut b = Browser::new(id, url);
        b.widget = widget;
        let Some(sid) = sid.or_else(|| self.selected.clone()) else {
            return Task::none();
        };
        // Browser tabs belong to the folder's detail area.
        let sid = self.workspace_owner(&sid);
        if global {
            self.global_browsers.push(b);
        } else if let Some(l) = self.live_mut(&sid) {
            l.browsers.push(b);
        } else {
            return Task::none();
        }
        self.new_menu = false;
        self.rail_open = false;
        self.ui.detail_collapsed = false;
        let show = self.activate(&sid, ItemKey::Browser(id));
        Task::batch([show, self.sync_detail()])
    }

    /// Close a browser tab (its page goes); the widget it showed, if any.
    pub(crate) fn close_browser(&mut self, id: u32) -> Option<crate::widgets_ui::WidgetTab> {
        let take = |v: &mut Vec<Browser>| v.iter().position(|b| b.id == id).map(|i| v.remove(i));
        let gone = take(&mut self.global_browsers).or_else(|| {
            self.sessions
                .iter_mut()
                .find_map(|l| take(&mut l.browsers))
        });
        let widget = gone.as_ref().and_then(|b| b.widget.clone());
        if let Some(b) = gone
            && let Some(v) = b.view
        {
            v.close();
        }
        for l in &mut self.sessions {
            if l.detail.active == ItemKey::Browser(id) {
                l.detail.active = ItemKey::Web;
            }
        }
        self.sync_web();
        widget
    }

    /// Add a browser tab without showing it (a widget tab restored at start).
    pub(crate) fn add_browser_tab_quietly(
        &mut self,
        sid: &str,
        global: bool,
        url: String,
        widget: Option<crate::widgets_ui::WidgetTab>,
    ) {
        self.next_browser += 1;
        let mut b = Browser::new(self.next_browser, url);
        b.widget = widget;
        let sid = self.workspace_owner(sid);
        if global {
            self.global_browsers.push(b);
        } else if let Some(l) = self.live_mut(&sid) {
            l.browsers.push(b);
        }
    }

    /// A page event of a browser tab (`false`: not one of them).
    pub(crate) fn browser_web_event(&mut self, ev: &ide_web::WebViewEvent) -> bool {
        use ide_web::WebEvent;
        let shown = self.shown_browser();
        let mut new_tab = None;
        let Some(b) = self
            .sessions
            .iter_mut()
            .flat_map(|l| l.browsers.iter_mut())
            .chain(self.global_browsers.iter_mut())
            .find(|b| b.view.as_ref().is_some_and(|v| v.id() == ev.view))
        else {
            return false;
        };
        match &ev.event {
            WebEvent::Created => {
                if let Some(v) = &b.view {
                    v.set_hidden(shown != Some(b.id));
                }
            }
            WebEvent::UrlChanged(u) => b.url_input = u.clone(),
            // A widget's tab keeps its name.
            WebEvent::TitleChanged(t) if b.widget.is_none() => b.title = t.clone(),
            WebEvent::LoadingStateChanged {
                is_loading,
                can_go_back,
                can_go_forward,
            } => {
                b.loading = *is_loading;
                b.can_back = *can_go_back;
                b.can_forward = *can_go_forward;
            }
            WebEvent::NewWindowRequested(url) => new_tab = Some((b.id, url.clone())),
            WebEvent::LoadError { url, error } => {
                tracing::warn!("browser tab load error {url}: {error}");
                if let Some(n) = crate::notice_view::load_error_notice(url, error) {
                    self.notices.push(n);
                }
            }
            WebEvent::Console { message, .. } if b.widget.is_some() => {
                crate::widgets_ui::console(&b.widget.as_ref().map(|w| w.id.clone()).unwrap_or_default(), message);
            }
            _ => {}
        }
        if let Some((from, url)) = new_tab {
            // Desk: a page's new window is a new tab beside it (공용 stays 공용).
            let global = self.global_browsers.iter().any(|b| b.id == from);
            let sid = self
                .sessions
                .iter()
                .find(|l| l.browsers.iter().any(|b| b.id == from))
                .map(|l| l.info.id.clone());
            let _ = self.open_browser(sid, global, url);
        }
        true
    }

    pub(crate) fn on_browser(&mut self, m: BrowserMsg) -> Task<Message> {
        match m {
            BrowserMsg::Open { global, url } => {
                // Alt+click / Alt+Enter opens as 공용 whatever the switch says (Desk).
                let global = global || self.modifiers.alt();
                let Some(url) = address_to_url(&url) else {
                    return Task::none();
                };
                self.open_browser(None, global, url)
            }
            BrowserMsg::Address(v) => {
                self.new_address = v;
                Task::none()
            }
            BrowserMsg::AddressSubmit => {
                let url = std::mem::take(&mut self.new_address);
                let global = self.new_global;
                self.on_browser(BrowserMsg::Open { global, url })
            }
            BrowserMsg::BookmarkToggle { url, title } => {
                let b = &mut self.ui.bookmarks;
                match b.iter().position(|x| x.url == url) {
                    Some(i) => {
                        b.remove(i);
                    }
                    None => b.push(Bookmark {
                        title: if title.trim().is_empty() { url.clone() } else { title },
                        url,
                    }),
                }
                self.save_ui();
                Task::none()
            }
            BrowserMsg::BookmarkRemove(url) => {
                self.ui.bookmarks.retain(|b| b.url != url);
                self.save_ui();
                Task::none()
            }
            BrowserMsg::BookmarkEdit(url) => {
                let title = self
                    .ui
                    .bookmarks
                    .iter()
                    .find(|b| b.url == url)
                    .map(|b| b.title.clone())
                    .unwrap_or_default();
                self.bookmark_edit = Some((url, title));
                iced::widget::operation::focus(BOOKMARK_EDIT_ID)
            }
            BrowserMsg::BookmarkEditInput(v) => {
                if let Some((_, t)) = &mut self.bookmark_edit {
                    *t = v;
                }
                Task::none()
            }
            BrowserMsg::BookmarkEditDone => {
                if let Some((url, title)) = self.bookmark_edit.take()
                    && let Some(b) = self.ui.bookmarks.iter_mut().find(|b| b.url == url)
                    && !title.trim().is_empty()
                {
                    b.title = title.trim().to_owned();
                    self.save_ui();
                }
                Task::none()
            }
        }
    }

    /// ☆ / ★ of a page's header: the address in 즐겨찾기 or not.
    pub(crate) fn bookmark_star<'a>(&self, url: &str, title: &str) -> El<'a> {
        let on = self.ui.bookmarks.iter().any(|b| b.url == url);
        let b = button(
            text(if on { "★" } else { "☆" })
                .size(14)
                .color(if on { style::YELLOW } else { style::DIM }),
        )
        .padding([2, 6])
        .style(style::icon)
        .on_press_maybe((!url.trim().is_empty()).then(|| {
            Message::Browser(BrowserMsg::BookmarkToggle {
                url: url.to_owned(),
                title: title.to_owned(),
            })
        }));
        tip(
            b.boxed(),
            if on {
                tr::bookmark_remove()
            } else {
                tr::bookmark_add()
            },
        )
    }

    /// The pane's header for a browser tab: back, forward, reload, the address, ☆.
    pub(crate) fn browser_identity<'a>(&'a self, live: &'a Live, id: u32) -> El<'a> {
        let Some(b) = self.browser(live, id) else {
            return container(text("")).boxed();
        };
        if let Some(w) = &b.widget {
            return self.widget_identity(b, w);
        }
        row![
            icon_button("←", b.can_back.then_some(Message::WebBack), tr::web_back()),
            icon_button("→", b.can_forward.then_some(Message::WebForward), tr::web_forward()),
            icon_button("⟳", Some(Message::WebReload), tr::web_reload()),
            text_input(tr::url_placeholder(), &b.url_input)
                .id(URL_ID)
                .on_input(Message::UrlInput)
                .on_submit(Message::UrlSubmit)
                .size(12)
                .padding([4, 8])
                .style(style::input)
                .width(Fill),
            self.bookmark_star(&b.url_input, &b.title),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .boxed()
    }

    /// The pane's body for a browser tab.
    pub(crate) fn browser_body<'a>(&'a self, live: &'a Live, id: u32) -> El<'a> {
        match self.browser(live, id) {
            Some(Browser { view: Some(v), .. }) => WebViewWidget::new(v)
                .width(Fill)
                .height(Fill)
                .dimmed(self.web_covered())
                .boxed(),
            Some(Browser { error: Some(e), .. }) => {
                container(text(e.clone()).size(12)).center(Fill).boxed()
            }
            _ => container(text("")).center(Fill).boxed(),
        }
    }
}

/// The bookmark name field of "+ 새로 열기".
pub const BOOKMARK_EDIT_ID: &str = "ide-bookmark-edit";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_like_desk() {
        assert_eq!(address_to_url("localhost:5173").as_deref(), Some("http://localhost:5173"));
        assert_eq!(address_to_url("example.com/a").as_deref(), Some("https://example.com/a"));
        assert_eq!(address_to_url("10.0.0.1:80").as_deref(), Some("http://10.0.0.1:80"));
        assert_eq!(address_to_url("https://x.dev").as_deref(), Some("https://x.dev"));
        assert_eq!(address_to_url("data:text/html,hi").as_deref(), Some("data:text/html,hi"));
        assert_eq!(address_to_url("about:blank").as_deref(), Some("about:blank"));
        assert!(address_to_url("mailto:a@b.c").unwrap().starts_with(SEARCH_URL));
        assert!(address_to_url("rust iced").unwrap().starts_with(SEARCH_URL));
        assert_eq!(address_to_url("file.txt").as_deref(), Some("https://file.txt"));
        assert!(address_to_url("word").unwrap().starts_with(SEARCH_URL));
        assert!(address_to_url("javascript:alert(1)").unwrap().starts_with(SEARCH_URL));
        assert_eq!(address_to_url("  "), None);
    }
}
