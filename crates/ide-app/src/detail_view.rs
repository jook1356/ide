//! Drawing of the detail area: header (document identity or web navigation, "open items",
//! collapse), the shown item, and the side panel (Changes / History / Explore; Explore's search
//! on top).

use iced::widget::{
    Column, button, checkbox, column, container, hover, mouse_area, popover, row, scrollable,
    space, stack,
    text, text_input,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Padding, Widget as _, mouse};
use ide_views::icons;
use ide_web::WebViewWidget;

use crate::app::{App, Live, Message, URL_ID};
use crate::detail::{
    CloseSet, DetailMsg, FileBar, FileItem, Item, ItemKey, SEARCH_ID, SideTab, file_name, parent,
};
use crate::history::{self, CommitFiles};
use crate::i18n::{self, tr};
use crate::icons::Icon;
use crate::style;
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

const I_BACK: &str = "←";
const I_FORWARD: &str = "→";
const I_RELOAD: &str = "⟳";
const I_COLLAPSE: &str = "»";
pub(crate) const I_CLOSE: &str = "✕";
const I_CHEVRON: &str = "▾";
const I_GLOBE: &str = "◍";
const I_DIFF: &str = "±";
const I_RELOAD_DISK: &str = "↻";
const I_OVERWRITE: &str = "⤓";
const I_COMPARE: &str = "⇄";
const I_SETTINGS: &str = "⚙";
/// Height of the header row.
const HEADER_H: f32 = 40.0;

/// Human-readable byte size.
pub fn size_text(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1} KB", f / K)
    } else {
        format!("{:.1} MB", f / K / K)
    }
}

fn hline<'a>() -> El<'a> {
    container(space())
        .height(1)
        .width(Fill)
        .style(style::bg(style::BORDER))
        .boxed()
}


/// Desk's "새 창으로 분리" (`IconWindowMaximize` at the right end of the detail header).
fn popout_button<'a>() -> El<'a> {
    let b = button(
        container(crate::icons::icon(Icon::WindowMaximize, 16.0, style::DIM)).center(28),
    )
    .width(28)
    .height(28)
    .padding(0)
    .style(style::icon)
    .on_press(Message::DetailWindow(
        crate::detail_window::WinMsg::Popout,
    ));
    tip(b.boxed(), tr::detail_popout())
}

/// The commit of a commit item: remembered when it was opened, else from History.
pub(crate) fn commit_info<'a>(key: &ItemKey, live: &'a Live) -> Option<&'a ide_client::CommitInfo> {
    let ItemKey::Commit { sha, .. } = key else {
        return None;
    };
    match live.detail.item(key) {
        Some(Item::Diff(x)) if x.commit.is_some() => x.commit.as_ref(),
        _ => live.detail.history.commit(sha),
    }
}

/// Title and folder of an item (header, open items list).
pub(crate) fn item_names(key: &ItemKey, live: &Live) -> (String, String) {
    match key {
        ItemKey::Browser(id) => match live.browsers.iter().find(|b| b.id == *id) {
            Some(b) => (b.label(), b.url_input.clone()),
            None => (tr::browser_new_tab(), String::new()),
        },
        ItemKey::Terminal(id) => match live.uterm(id) {
            Some(t) => (t.info.title.clone(), t.info.cwd.clone()),
            None => (id.clone(), String::new()),
        },
        // Desk's `tabLabel` / `docSubtitle`: `file @short` under `short subject`, or
        // `short subject` under `author · date`.
        ItemKey::Commit { sha, path, .. } => {
            let info = commit_info(key, live);
            let short = info.map_or_else(
                || sha.chars().take(7).collect::<String>(),
                |c| c.short.clone(),
            );
            let headline = match info {
                Some(c) => format!("{short} {}", c.subject),
                None => short.clone(),
            };
            match path {
                Some(p) => (format!("{} @{short}", file_name(p)), headline),
                None => (
                    headline,
                    info.map(|c| format!("{} · {}", c.author, history::full_date(c.date)))
                        .unwrap_or_default(),
                ),
            }
        }
        ItemKey::Web => {
            let t = if live.web_title.trim().is_empty() {
                tr::tab_web()
            } else {
                live.web_title.clone()
            };
            (t, live.url_input.clone())
        }
        ItemKey::File(p) | ItemKey::Diff { path: p, .. } | ItemKey::Compare(p) => {
            let folder = parent(p);
            // Several repos: the folder starts with the repo's name.
            let folder = if live.info.repos.len() > 1 {
                let repo = crate::detail::split_q(p).0;
                let name = live
                    .info
                    .repos
                    .iter()
                    .find(|r| r.repo == repo)
                    .map_or(repo, |r| r.name.as_str());
                if folder.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name}/{folder}")
                }
            } else {
                folder.to_owned()
            };
            (file_name(p).to_owned(), folder)
        }
    }
}

fn item_kind(key: &ItemKey) -> String {
    match key {
        ItemKey::Web => tr::tab_web(),
        ItemKey::File(_) => tr::kind_file(),
        ItemKey::Diff { staged: true, .. } => tr::kind_staged(),
        ItemKey::Diff { staged: false, .. } => tr::kind_changed(),
        ItemKey::Compare(_) => tr::kind_compare(),
        ItemKey::Commit { .. } => tr::kind_commit(),
        ItemKey::Terminal(_) => tr::group_terminal(),
        ItemKey::Browser(_) => tr::group_browser(),
    }
}

pub(crate) fn item_icon<'a>(key: &ItemKey) -> El<'a> {
    match key {
        ItemKey::Web | ItemKey::Browser(_) => text(I_GLOBE).size(14).color(style::DIM).boxed(),
        ItemKey::Terminal(_) => text(">_").size(11).font(style::MONO).color(style::DIM).boxed(),
        ItemKey::File(p) => icons::file_icon(file_name(p)),
        ItemKey::Diff { path, .. } | ItemKey::Compare(path) => row![
            icons::file_icon(file_name(path)),
            text(I_DIFF).size(10).color(style::AMBER),
        ]
        .spacing(1)
        .align_y(Alignment::Center)
        .boxed(),
        // Desk's `TabIcon`: the file with a commit badge, or the commit icon.
        ItemKey::Commit { path: Some(p), .. } => row![
            icons::file_icon(file_name(p)),
            crate::icons::icon(Icon::GitCommit, 11.0, style::DIM),
        ]
        .spacing(1)
        .align_y(Alignment::Center)
        .boxed(),
        ItemKey::Commit { path: None, .. } => crate::icons::icon(Icon::GitCommit, 14.0, style::DIM),
    }
}

impl App {
    pub fn detail_area(&self) -> El<'_> {
        // Hidden or in its own window: no room taken (the session header's toggle shows it).
        if !self.detail_in_app() {
            return space().width(0).boxed();
        }
        let portion = 1000u16.saturating_sub((self.ui.split_ratio * 1000.0).round() as u16);
        let body: El<'_> = match self.shown_live() {
            Some(live) => self.pane(live, false),
            None => column![
                container(
                    row![
                        space().width(Fill),
                        icon_button(I_COLLAPSE, Some(Message::ToggleDetail), tr::hide_detail()),
                        popout_button(),
                    ]
                    .align_y(Alignment::Center)
                )
                .height(HEADER_H)
                .padding([0, 6])
                .style(style::bg(style::HEADER)),
                container(text(tr::no_session()).size(13).color(style::DIM)).center(Fill),
            ]
            .boxed(),
        };
        container(body)
            .width(FillPortion(portion.max(1)))
            .height(Fill)
            .style(style::bg(style::PANEL))
            .boxed()
    }

    /// The side panel right of the detail pane (fixed width, outside the split).
    pub fn side_area(&self) -> El<'_> {
        match self.shown_live() {
            Some(live) if !self.ui.detail_collapsed => {
                row![self.side_handle(), self.side_panel(live)].boxed()
            }
            _ => space().width(0).boxed(),
        }
    }

    /// The side panel's left edge: a 1 px border that resizes the panel when dragged (Desk's
    /// `PanelResizer` beside the Changes panel).
    fn side_handle(&self) -> El<'_> {
        let color = if self.side_dragging {
            style::ACCENT
        } else {
            style::BORDER
        };
        mouse_area(
            container(
                container(space())
                    .width(1)
                    .height(Fill)
                    .style(style::bg(color)),
            )
            .center_x(crate::app::DIVIDER_W)
            .height(Fill),
        )
        .on_press(Message::SideResizePressed)
        .interaction(mouse::Interaction::ResizingHorizontally)
        .boxed()
    }

    // ------------------------------------------------------------------ pane

    /// The detail pane of a session: in the app window (`in_window` false: with the hide and
    /// "새 창으로 분리" buttons) or in the detail window (neither: the window's own close button
    /// hides it).
    pub(crate) fn pane<'a>(&'a self, live: &'a Live, in_window: bool) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.clone();
        // Another session's terminal is the folder's active item: show the page instead.
        let active = if self.item_shown(live, &d.active) {
            d.active.clone()
        } else {
            ItemKey::Web
        };
        let identity: El<'a> = match &active {
            ItemKey::Terminal(id) => self.uterm_identity(live, id),
            ItemKey::Browser(id) => self.browser_identity(live, *id),
            // Nothing open (the page is not open by default).
            ItemKey::Web if d.item(&ItemKey::Web).is_none() => space().width(Fill).boxed(),
            ItemKey::Web => row![
                icon_button(
                    I_BACK,
                    live.can_back.then_some(Message::WebBack),
                    tr::web_back()
                ),
                icon_button(
                    I_FORWARD,
                    live.can_forward.then_some(Message::WebForward),
                    tr::web_forward()
                ),
                icon_button(I_RELOAD, Some(Message::WebReload), tr::web_reload()),
                text_input(tr::url_placeholder(), &live.url_input)
                    .id(URL_ID)
                    .on_input(Message::UrlInput)
                    .on_submit(Message::UrlSubmit)
                    .size(12)
                    .padding([4, 8])
                    .style(style::input)
                    .width(Fill),
                self.bookmark_star(&live.url_input, &live.web_title),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .boxed(),
            key => {
                let (title, folder) = item_names(key, live);
                let dirty = matches!(d.active_item(), Some(Item::File(f)) if f.dirty());
                let subtitle = if folder.is_empty() {
                    item_kind(key)
                } else {
                    format!("{} · {folder}", item_kind(key))
                };
                let mut title_row = row![
                    text(title)
                        .size(13)
                        .font(style::BOLD)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                ]
                .spacing(6)
                .align_y(Alignment::Center);
                if dirty {
                    title_row = title_row.push(text("●").size(10).color(style::TEXT).boxed());
                }
                row![
                    item_icon(key),
                    column![
                        title_row,
                        text(subtitle)
                            .size(11)
                            .color(style::DIM)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::Start),
                    ]
                    .spacing(1)
                    .width(Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .boxed()
            }
        };
        // In the detail window's tabs mode 열린 항목 is the tab list beside the body instead.
        let rail = self.rail_shown(in_window);
        let mut header_row = row![container(identity).width(Fill)]
            .spacing(6)
            .align_y(Alignment::Center);
        if !rail {
            header_row = header_row
                .push(vline_short())
                .push(self.open_items_button(live, in_window));
        }
        if !in_window {
            header_row = header_row
                .push(icon_button(
                    I_COLLAPSE,
                    Some(Message::ToggleDetail),
                    tr::hide_detail(),
                ))
                .push(popout_button());
        }
        let header = container(header_row)
        .height(HEADER_H)
        .padding([0, 8])
        .align_y(Alignment::Center)
        .width(Fill)
        .style(style::bg(style::HEADER));

        let body: El<'a> = if let ItemKey::Browser(id) = &active {
            self.browser_body(live, *id)
        } else if let ItemKey::Terminal(id) = &active {
            self.uterm_body(live, id)
        } else {
            // `None` (the fallback above) shows the page.
            let item = if active == d.active { d.active_item() } else { None };
            match item {
            None if d.item(&ItemKey::Web).is_none() => message_body(tr::rail_empty(), false),
            Some(Item::Web) | None => match &live.web {
                // Under a modal the page stays visible as a frozen, dimmed snapshot of its
                // last frame (no input reaches it, CEF stops rendering); the backdrop and the
                // dialog are drawn above it (see docs, "modal z-order").
                Some(w) => WebViewWidget::new(w)
                    .width(Fill)
                    .height(Fill)
                    .dimmed(self.web_covered())
                    .boxed(),
                None => container(text(live.error.as_deref().unwrap_or("")).size(12))
                    .center(Fill)
                    .boxed(),
            },
            Some(Item::File(f)) => self.file_body(&sid, f),
            Some(Item::Diff(x)) => {
                let key = d.active.clone();
                match (&x.view, &x.error) {
                    (_, Some(e)) => message_body(e.clone(), true),
                    (Some(v), None) => {
                        let sid = sid.clone();
                        v.view()
                            .map(move |m| {
                                Message::Detail(sid.clone(), DetailMsg::Diff(key.clone(), m))
                            })
                            .boxed()
                    }
                    (None, None) => message_body(tr::tree_loading(), false),
                }
            }
            }
        };
        let layers = vec![body];
        let body = stack(layers).width(Fill).height(Fill).boxed();
        let body = if rail {
            self.with_rail(live, body)
        } else {
            body
        };
        container(column![header, hline(), body])
        .width(Fill)
        .height(Fill)
        .boxed()
    }

    fn file_body<'a>(&'a self, sid: &str, f: &'a FileItem) -> El<'a> {
        if let Some(e) = &f.error {
            return message_body(e.clone(), true);
        }
        if f.binary {
            return message_body(tr::file_binary(&size_text(f.size)), false);
        }
        let Some(v) = &f.view else {
            return message_body(tr::tree_loading(), false);
        };
        let (s, p) = (sid.to_owned(), f.path.clone());
        let editor = v
            .view()
            .map(move |m| Message::Detail(s.clone(), DetailMsg::Code(p.clone(), m)))
            .boxed();
        let mut col = Column::new();
        if let Some(bar) = f.bar {
            col = col.push(file_bar(sid, &f.path, bar));
        }
        if f.truncated {
            col = col.push(
                container(text(tr::file_truncated(&size_text(f.size))).size(12))
                    .padding([5, 10])
                    .width(Fill)
                    .style(style::bg(style::HEADER))
                    .boxed(),
            );
        }
        col.push(editor).boxed()
    }

    /// "Open items": every item of the session, grouped like Hermes Desk's list. In the detail
    /// window the list ends with Desk's "세로 탭으로 보기" (back to the tab list).
    fn open_items_button<'a>(&'a self, live: &'a Live, in_window: bool) -> El<'a> {
        let d = &live.detail;
        let total = d.items.len();
        let trigger = button(
            row![
                text(I_CHEVRON).size(11).color(style::DIM),
                text(tr::open_items()).size(12),
                container(text(total.to_string()).size(10).font(style::BOLD))
                    .padding([0, 5])
                    .style(|_| container::Style {
                        background: Some(style::BORDER.into()),
                        text_color: Some(style::TEXT),
                        border: iced::Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(style::choice(self.open_items))
        .on_press(Message::ToggleOpenItems);
        let popup = self.open_items.then(|| {
            let sid = live.info.id.clone();
            let mut list = Column::new().spacing(2);
            for e in self.shared_sections(live, true) {
                list = list.push(e);
            }
            type Group = (String, fn(&ItemKey) -> bool);
            let groups: [Group; 4] = [
                (tr::group_browser(), |k| matches!(k, ItemKey::Web)),
                (tr::side_changes(), |k| {
                    matches!(k, ItemKey::Diff { .. } | ItemKey::Compare(_))
                }),
                (tr::side_history(), |k| matches!(k, ItemKey::Commit { .. })),
                (tr::side_explore(), |k| matches!(k, ItemKey::File(_))),
            ];
            for (label, is) in groups {
                let mut rows: Vec<(ItemKey, bool)> = d
                    .items
                    .iter()
                    .filter(|(k, _)| is(k) && !self.is_common(&live.info.id, k))
                    .map(|(k, item)| (k.clone(), matches!(item, Item::File(f) if f.dirty())))
                    .collect();
                // The session's browser tabs follow its web page.
                if is(&ItemKey::Web) {
                    rows.extend(
                        live.browsers
                            .iter()
                            .map(|b| (ItemKey::Browser(b.id), false))
                            .filter(|(k, _)| !self.is_common(&live.info.id, k)),
                    );
                }
                if rows.is_empty() {
                    continue;
                }
                list = list.push(
                    row![
                        text(label).size(11).font(style::BOLD).color(style::DIM),
                        text(rows.len().to_string()).size(11).color(style::FAINT),
                    ]
                    .spacing(6)
                    .padding([4, 8])
                    .boxed(),
                );
                for (k, dirty) in &rows {
                    let (k, dirty) = (k, *dirty);
                    let (title, folder) = item_names(k, live);
                    let mut names = row![
                        text(title.clone())
                            .size(12)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::End),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);
                    if dirty {
                        names = names.push(text("●").size(9).boxed());
                    }
                    if !folder.is_empty() && folder != title {
                        names = names.push(
                            text(folder)
                                .size(11)
                                .color(style::FAINT)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::Start)
                                .boxed(),
                        );
                    }
                    let main = button(
                        row![item_icon(k), names]
                            .spacing(8)
                            .align_y(Alignment::Center),
                    )
                    .padding([4, 8])
                    .width(Fill)
                    .style(style::row(*k == d.active))
                    .on_press(Message::Detail(sid.clone(), DetailMsg::Activate(k.clone())));
                    let close: El<'_> = icon_button(
                        I_CLOSE,
                        Some(Message::Detail(sid.clone(), DetailMsg::Close(k.clone()))),
                        tr::close(),
                    );
                    list = list.push(self.item_row(
                        live,
                        row![main, close]
                            .spacing(2)
                            .align_y(Alignment::Center)
                            .boxed(),
                        k,
                    ));
                }
            }
            // The shown session's terminals (`uterm.rs`; the folder's other sessions keep theirs).
            let term_live = self.term_live(live);
            let shown_terms: Vec<&crate::uterm::UTermView> = term_live
                .uterms
                .iter()
                .filter(|t| !self.is_common(term_live.id(), &ItemKey::Terminal(t.info.id.clone())))
                .collect();
            if !shown_terms.is_empty() {
                list = list.push(
                    row![
                        text(tr::group_terminal()).size(11).font(style::BOLD).color(style::DIM),
                        text(shown_terms.len().to_string()).size(11).color(style::FAINT),
                    ]
                    .spacing(6)
                    .padding([4, 8])
                    .boxed(),
                );
                for t in shown_terms {
                    let k = ItemKey::Terminal(t.info.id.clone());
                    let running = t.info.state == ide_client::SessionState::Running;
                    let main = button(
                        row![
                            crate::uterm::dot(t.info.state),
                            text(t.info.title.clone())
                                .size(12)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding([4, 8])
                    .width(Fill)
                    .style(style::row(k == d.active))
                    .on_press(Message::Detail(sid.clone(), DetailMsg::Activate(k.clone())));
                    let close = icon_button(
                        I_CLOSE,
                        Some(Message::Detail(sid.clone(), DetailMsg::Close(k.clone()))),
                        if running {
                            tr::uterm_stop_remove()
                        } else {
                            tr::uterm_remove()
                        },
                    );
                    let r = row![main, close].spacing(2).align_y(Alignment::Center);
                    list = list.push(self.item_row(live, r.boxed(), &k));
                }
            }
            list = list.push(container(self.new_item_button(live, true)).padding([4, 0]).boxed());
            if total <= 1 {
                list = list.push(
                    container(text(tr::open_items_none()).size(12).color(style::DIM))
                        .padding([4, 8])
                        .boxed(),
                );
            }
            // Desk's foot: 탭 모두 닫기 · 작업 중 아닌 항목 닫기, and in the detail window the
            // way back to the tab list.
            let link = |label: String, tip_text: String, set: CloseSet, on: bool| {
                let b = button(text(label).size(11))
                    .padding([2, 4])
                    .style(style::icon)
                    .on_press_maybe(
                        on.then(|| Message::Detail(sid.clone(), DetailMsg::CloseSome(set))),
                    );
                tip(b.boxed(), tip_text)
            };
            let mut foot = row![
                link(tr::close_all(), tr::close_all_tip(), CloseSet::All, total > 1),
                link(tr::close_idle(), tr::close_idle_tip(), CloseSet::Idle, total > 1),
                space().width(Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if in_window {
                let to_tabs = button(
                    container(crate::icons::icon(Icon::SidebarExpand, 14.0, style::DIM))
                        .center(24),
                )
                .width(24)
                .height(24)
                .padding(0)
                .style(style::icon)
                .on_press(Message::Rail(crate::detail_rail::RailMsg::SetMode(
                    crate::ui_state::RailMode::Tabs,
                )));
                foot = foot.push(tip(to_tabs.boxed(), tr::rail_to_tabs()));
            }
            container(column![
                scrollable(list).height(Length::Shrink),
                container(space()).height(1).width(Fill).style(style::bg(style::BORDER)),
                container(foot).padding([4, 6]),
            ])
            .padding(6)
            .width(360)
            .style(style::modal_card)
        });
        popover(trigger, popup)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(Message::CloseOpenItems)
            .boxed()
    }

    // ------------------------------------------------------------------ side panel

    fn side_panel<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.clone();
        // `badge`: the count badge's text and its tooltip.
        let tab = |t: SideTab, label: String, badge: Option<(String, Option<String>)>| -> El<'a> {
            let active = d.side == t;
            let mut r = row![text(label).size(12)]
                .spacing(5)
                .align_y(Alignment::Center);
            if let Some((n, tooltip)) = badge {
                let b = container(text(n).size(10).font(style::BOLD))
                    .padding([0, 5])
                    .style(|_| container::Style {
                        background: Some(style::BORDER.into()),
                        text_color: Some(style::TEXT),
                        border: iced::Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .boxed();
                r = r.push(match tooltip {
                    Some(t) => tip(b, t),
                    None => b,
                });
            }
            column![
                button(container(r).center_x(Fill))
                    .padding([8, 6])
                    .width(Fill)
                    .style(style::tab_flat(active))
                    .on_press(Message::Detail(sid.clone(), DetailMsg::Side(t))),
                container(space())
                    .height(2)
                    .width(Fill)
                    .style(style::bg(if active {
                        style::ACCENT
                    } else {
                        style::HEADER
                    })),
            ]
            .width(Fill)
            .boxed()
        };
        // Hermes Desk order: Changes | History | Explore. History's badge: commits not pushed
        // to a remote yet.
        let changed = d.changed_count();
        let tabs = container(row![
            tab(
                SideTab::Changes,
                tr::side_changes(),
                (changed > 0).then(|| (changed.to_string(), None))
            ),
            tab(
                SideTab::History,
                tr::side_history(),
                d.history.badge().map(|b| (
                    b,
                    Some(tr::history_unpushed_count(i18n::count(
                        d.history.unpushed.len()
                    )))
                ))
            ),
            tab(SideTab::Explore, tr::side_explore(), None),
        ])
        .style(style::bg(style::HEADER));
        let content: El<'a> = match d.side {
            _ if live.info.repos.is_empty() => column![
                self.repo_selector(live),
                message_body(tr::no_repos_in_session(), false),
            ]
            .boxed(),
            // Checkboxes, stash, discard, reveal: `changes_view.rs`.
            SideTab::Changes => column![self.repo_selector(live), self.changes_tab(live)].boxed(),
            SideTab::History => column![self.repo_selector(live), self.history_list(live)].boxed(),
            SideTab::Explore => column![
                self.repo_selector(live),
                self.explore_search_input(live),
                match &d.tree_error {
                    _ if d.search.active() => self.explore_results(live),
                    Some(e) => message_body(e.clone(), true),
                    None => {
                        let s = sid.clone();
                        d.tree
                            .view()
                            .map(move |m| Message::Detail(s.clone(), DetailMsg::Tree(m)))
                            .boxed()
                    }
                },
            ]
            .boxed(),
        };
        // Commit / sync / PR footer under Changes and History (`footer_view.rs`), then (Desk) the
        // 스크립트 section at the very bottom under every tab.
        let scripts: El<'a> = if live.info.repos.is_empty() {
            space().height(0).boxed()
        } else {
            self.scripts_section(live)
        };
        container(column![
            tabs,
            hline(),
            container(content).height(Fill),
            self.footer_section(live),
            scripts,
        ])
            .width(self.side_w())
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
            .boxed()
    }

    /// Hermes Desk's Changes-panel header (`BranchSelect`): `repo / branch` of the shown repo,
    /// a dropdown of the session's repos, and the session's repo settings (⚙: attach / detach).
    fn repo_selector<'a>(&'a self, live: &'a Live) -> El<'a> {
        let sid = live.info.id.clone();
        // Desk's `repo / branch` button and popover (repos, branches): `changes_view.rs`.
        let picker: El<'a> = if live.info.repos.is_empty() {
            row![
                crate::icons::icon(crate::icons::Icon::GitBranch, 14.0, style::DIM),
                text(tr::no_repos()).size(12).color(style::DIM).width(Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .boxed()
        } else {
            self.repo_branch_picker(live)
        };
        container(
            row![
                picker,
                icon_button(
                    I_SETTINGS,
                    self.connected()
                        .then(|| Message::SessionRepos(sid.clone())),
                    tr::session_repos_open()
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([5, 8])
        .width(Fill)
        .style(style::bg(style::HEADER))
        .boxed()
    }

    /// Hermes Desk's Explore 파일 찾기 input: a search icon on the left, the text search's
    /// case / word / regex toggles on the right (where Desk has its settings button).
    /// Desk's 파일 찾기 설정 (gear in the search box): gitignore 파일 숨기기.
    fn explore_settings_button(&self) -> El<'_> {
        let open = self.explore_settings;
        let trigger = tip(
            button(
                container(crate::icons::icon(Icon::Settings, 13.0, style::DIM)).center(18),
            )
            .width(22)
            .height(20)
            .padding(0)
            .style(style::choice(open))
            .on_press(Message::ExploreSettings(!open))
            .boxed(),
            tr::explore_settings(),
        );
        let popup = open.then(|| {
            container(tip(
                checkbox(self.ui.explore_hide_ignored)
                    .label(tr::explore_hide_ignored())
                    .on_toggle(Message::ExploreHideIgnored)
                    .size(14)
                    .text_size(12)
                    .boxed(),
                tr::explore_hide_ignored_tip(),
            ))
            .padding(10)
            .style(style::modal_card)
        });
        popover(trigger, popup)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(Message::ExploreSettings(false))
            .boxed()
    }

    fn explore_search_input<'a>(&'a self, live: &'a Live) -> El<'a> {
        let s = &live.detail.search;
        let sid = live.info.id.clone();
        let m = |x: DetailMsg| Message::Detail(sid.clone(), x);
        let toggle = |glyph: &'static str, on: bool, msg: Message, label: String| -> El<'a> {
            tip(
                button(
                    text(glyph)
                        .size(11)
                        .font(style::MONO)
                        .width(18)
                        .align_x(Alignment::Center),
                )
                .padding([2, 2])
                .style(style::choice(on))
                .on_press(msg)
                .boxed(),
                label,
            )
        };
        let input = text_input(tr::explore_find_placeholder(), &s.query)
            .id(SEARCH_ID)
            .on_input({
                let sid = sid.clone();
                move |v| Message::Detail(sid.clone(), DetailMsg::SearchInput(v))
            })
            .on_submit(m(DetailMsg::SearchSubmit))
            .size(12)
            .padding(Padding {
                top: 5.0,
                right: 100.0,
                bottom: 5.0,
                left: 28.0,
            })
            .style(style::input)
            .width(Fill);
        let icon = container(crate::icons::icon(
            crate::icons::Icon::Search,
            14.0,
            style::DIM,
        ))
        .padding(Padding {
            left: 9.0,
            ..Padding::ZERO
        })
        .height(Fill)
        .align_y(Alignment::Center);
        let toggles = container(
            row![
                toggle(
                    "Aa",
                    s.case_sensitive,
                    m(DetailMsg::SearchCase),
                    tr::search_case()
                ),
                toggle(
                    "ab",
                    s.whole_word,
                    m(DetailMsg::SearchWord),
                    tr::search_word()
                ),
                toggle(".*", s.regex, m(DetailMsg::SearchRegex), tr::search_regex()),
                self.explore_settings_button(),
            ]
            .spacing(1)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            right: 4.0,
            ..Padding::ZERO
        })
        .width(Fill)
        .height(Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::Center);
        container(stack![input, icon, toggles])
            .padding([8, 10])
            .width(Fill)
            .boxed()
    }

    /// Explore with a query (instead of the tree): file-name matches of the shown repo like
    /// Desk's filtered tree, then the text matches of every repo under a section label.
    fn explore_results<'a>(&'a self, live: &'a Live) -> El<'a> {
        let s = &live.detail.search;
        let sid = live.info.id.clone();
        let mut list = Column::new().spacing(0).padding([0, 0]);
        for f in &s.files {
            let label = f.path.trim_end_matches('/').to_owned();
            // A folder shows in the tree (Desk); a file opens.
            let open = if f.dir {
                Message::Detail(
                    sid.clone(),
                    DetailMsg::Acts(crate::changes_actions::ActMsg::Reveal(label.clone())),
                )
            } else {
                Message::Detail(
                    sid.clone(),
                    DetailMsg::OpenAt(crate::detail::qualify(&f.repo, &f.path), 0, 0),
                )
            };
            let glyph: El<'a> = if f.dir {
                icons::folder_icon(false, style::DIM)
            } else {
                icons::file_icon(file_name(&label))
            };
            list = list.push(
                tip(
                    button(
                        row![
                            glyph,
                            text(label.clone())
                                .size(12)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::Start)
                                .width(Fill),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding(Padding {
                        top: 3.0,
                        right: 8.0,
                        bottom: 3.0,
                        left: 24.0,
                    })
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(open)
                    .boxed(),
                    label,
                ),
            );
        }
        let more = s.files_total as usize - s.files.len().min(s.files_total as usize);
        if more > 0 {
            list = list.push(
                container(
                    text(tr::explore_more(i18n::count(more)))
                        .size(11)
                        .color(style::DIM),
                )
                .padding([4, 24])
                .boxed(),
            );
        }
        if let Some(e) = &s.files_error {
            list = list.push(
                container(text(e.clone()).size(11).color(style::RED))
                    .padding([4, 10])
                    .boxed(),
            );
        }
        let status: Option<(String, Color)> = if let Some(e) = &s.error {
            Some((e.clone(), style::RED))
        } else if s.running {
            Some((tr::search_running(), style::DIM))
        } else {
            s.summary.as_ref().map(|sum| {
                let mut t = if sum.matches == 0 {
                    tr::search_none()
                } else {
                    tr::search_summary(sum.matches as i64, sum.files as i64)
                };
                if sum.truncated {
                    t = format!("{t} {}", tr::search_truncated());
                }
                (t, style::DIM)
            })
        };
        let mut label = row![
            text(tr::explore_contents())
                .size(11)
                .font(style::BOLD)
                .color(style::DIM)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some((t, c)) = status {
            label = label.push(
                text(t)
                    .size(11)
                    .color(c)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::End)
                    .width(Fill)
                    .boxed(),
            );
        }
        list = list.push(
            container(label)
                .padding(Padding {
                    top: if s.files.is_empty() { 2.0 } else { 10.0 },
                    right: 10.0,
                    bottom: 4.0,
                    left: 10.0,
                })
                .width(Fill)
                .boxed(),
        );

        let mut last_repo: Option<&str> = None;
        let multi = live.info.repos.len() > 1;
        for (path, hits) in &s.groups {
            // Results are grouped by repo: a header above each repo's files.
            let repo = crate::detail::split_q(path).0;
            if multi && last_repo != Some(repo) {
                last_repo = Some(repo);
                let name = live
                    .info
                    .repos
                    .iter()
                    .find(|r| r.repo == repo)
                    .map_or(repo, |r| r.name.as_str());
                list = list.push(
                    container(
                        row![
                            crate::icons::icon(crate::icons::Icon::GitBranch, 12.0, style::DIM),
                            text(name.to_owned()).size(11).font(style::BOLD).color(style::DIM),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding([4, 8])
                    .boxed(),
                );
            }
            let collapsed = s.collapsed.contains(path);
            let sid2 = live.info.id.clone();
            list = list.push(
                button(
                    row![
                        text(if collapsed { "▸" } else { "▾" })
                            .size(10)
                            .color(style::DIM)
                            .width(10),
                        icons::file_icon(file_name(path)),
                        text(file_name(path))
                            .size(12)
                            .wrapping(text::Wrapping::None),
                        text(parent(path).to_owned())
                            .size(11)
                            .color(style::FAINT)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::Start)
                            .width(Fill),
                        text(hits.len().to_string()).size(10).color(style::DIM),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([3, 8])
                .width(Fill)
                .style(style::row(false))
                .on_press(Message::Detail(
                    sid2.clone(),
                    DetailMsg::SearchToggleFile(path.clone()),
                ))
                .boxed(),
            );
            if collapsed {
                continue;
            }
            for h in hits {
                list = list.push(
                    button(
                        row![
                            text(h.line.to_string())
                                .size(11)
                                .font(style::MONO)
                                .color(style::FAINT)
                                .width(34)
                                .align_x(Alignment::End),
                            hit_text(&h.line_text, &h.ranges),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(Padding {
                        top: 2.0,
                        right: 8.0,
                        bottom: 2.0,
                        left: 14.0,
                    })
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(Message::Detail(
                        sid2.clone(),
                        DetailMsg::OpenAt(crate::detail::qualify(&h.repo, &h.path), h.line, h.col),
                    ))
                    .boxed(),
                );
            }
        }
        scrollable(list.push(space().height(24).boxed()))
            .height(Fill)
            .boxed()
    }

    /// Hermes Desk's History list: commits newest first, each unfolding into its files, and
    /// "더 보기" at the end (also loaded when scrolled to the end).
    fn history_list<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let h = &d.history;
        let sid = live.info.id.clone();
        if !h.loaded {
            return container(text(tr::tree_loading()).size(12).color(style::DIM))
                .padding(10)
                .width(Fill)
                .height(Fill)
                .style(style::bg(style::PANEL))
                .boxed();
        }
        let now = history::now_secs();
        let m = |x: DetailMsg| Message::Detail(sid.clone(), x);
        let commit_key = |sha: &str, path: Option<&str>| ItemKey::Commit {
            repo: d.repo.clone(),
            sha: sha.to_owned(),
            path: path.map(str::to_owned),
        };
        let mut list = Column::new();
        if h.commits.is_empty() {
            list = list.push(
                container(text(tr::history_empty()).size(12).color(style::DIM))
                    .padding(10)
                    .boxed(),
            );
        }
        for c in &h.commits {
            let expanded = h.expanded.contains(&c.sha);
            let row_el = commit_row(
                c,
                h.unpushed.contains(&c.sha),
                expanded,
                d.active == commit_key(&c.sha, None),
                now,
                m(DetailMsg::OpenCommit(c.sha.clone(), None)),
                m(DetailMsg::ToggleCommit(c.sha.clone())),
            );
            // Right-click: Desk's commit context menu (`changes_view.rs`).
            list = list.push(self.commit_menu_wrap(live, c, row_el));
            if !expanded {
                continue;
            }
            match h.files.get(&c.sha) {
                Some(CommitFiles::Loaded(files)) => {
                    for f in files {
                        let file_row = commit_file_row(
                            f,
                            d.active == commit_key(&c.sha, Some(&f.path)),
                            m(DetailMsg::OpenCommit(c.sha.clone(), Some(f.path.clone()))),
                        );
                        // Desk: a reveal button on hover, except for deleted files.
                        list = list.push(if f.kind == ide_client::ChangeKind::Deleted {
                            file_row
                        } else {
                            crate::changes_view::row_actions(
                                file_row,
                                vec![crate::changes_view::reveal_button(&sid, &f.path)],
                                12.0 + 16.0 + 8.0,
                            )
                        });
                    }
                }
                status => {
                    let (t, color) = match status {
                        Some(CommitFiles::Failed(e)) => (e.clone(), style::RED),
                        _ => (tr::tree_loading(), style::DIM),
                    };
                    list = list.push(
                        container(text(t).size(12).color(color))
                            .padding([4, 10])
                            .width(Fill)
                            .style(style::bg(COMMIT_FILES_BG))
                            .boxed(),
                    );
                }
            }
            // The band of files ends with a gap (Desk: `margin-bottom: 6px`).
            list = list.push(space().height(6).boxed());
        }
        if h.has_more {
            let more: El<'a> = if h.loading_more {
                text(tr::tree_loading()).size(12).color(style::DIM).boxed()
            } else {
                button(text(tr::history_more()).size(12).color(style::DIM))
                    .padding([2, 8])
                    .style(style::icon)
                    .on_press(m(DetailMsg::HistoryMore))
                    .boxed()
            };
            list = list.push(container(more).center_x(Fill).padding([8, 0]).boxed());
        }
        let s = sid.clone();
        // A gap under the picker, like Desk's `marginTop: 8`.
        container(
            scrollable(list)
                .height(Fill)
                .on_scroll(move |v| {
                    Message::Detail(
                        s.clone(),
                        DetailMsg::HistoryScrolled(v.viewport.distance_to_end().y),
                    )
                }),
        )
        .padding(Padding {
            top: 8.0,
            ..Padding::ZERO
        })
        .width(Fill)
        .height(Fill)
        .style(style::bg(style::PANEL))
        .boxed()
    }
}

/// Desk's `.commit-files-list` band under an unfolded commit (`rgb(0 0 0 / 12%)`).
const COMMIT_FILES_BG: Color = ide_views::theme::over(Color::BLACK, 0.12, style::PANEL);
/// Desk's `.unpushed-mark`: accent text on a 25 % accent circle.
const UNPUSHED_FG: Color = style::ACCENT_TEXT;
const UNPUSHED_BG: Color = ide_views::theme::over(style::ACCENT, 0.25, style::PANEL);

/// A 16 px circle (avatar, unpushed mark).
fn circle(c: Color) -> impl Fn(&iced::Theme) -> container::Style {
    move |_| container::Style {
        background: Some(c.into()),
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Desk's `CommitRow`: bold subject (+ ↑ when not pushed), avatar and `author · time ago`;
/// the chevron (shown on hover, kept while unfolded) folds the files; the row opens the commit.
fn commit_row<'a>(
    c: &'a ide_client::CommitInfo,
    unpushed: bool,
    expanded: bool,
    active: bool,
    now: i64,
    open: Message,
    toggle: Message,
) -> El<'a> {
    let mut top = row![
        text(c.subject.as_str())
            .size(12)
            .font(style::BOLD)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End)
            .width(Fill)
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if unpushed {
        top = top.push(tip(
            container(crate::icons::icon(Icon::ArrowUp, 10.0, UNPUSHED_FG))
                .center(16)
                .style(circle(UNPUSHED_BG))
                .boxed(),
            tr::history_unpushed_mark(),
        ));
    }
    let avatar = container(
        text(history::initials(&c.author))
            .size(8)
            .font(style::BOLD)
            .color(Color::WHITE)
            .line_height(1.0),
    )
    .center(16)
    .style(circle(history::avatar_color(&c.author, &c.email)))
    .boxed();
    let who = if c.email.is_empty() {
        c.author.clone()
    } else {
        format!("{} <{}>", c.author, c.email)
    };
    let meta = row![
        tip(avatar, who),
        tip(
            text(format!(
                "{} · {}",
                c.author,
                history::relative_time(c.date, now)
            ))
            .size(12)
            .color(style::DIM)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End)
            .boxed(),
            format!("{} · {}", c.short, history::full_date(c.date)),
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let chevron = tip(
        button(crate::icons::icon(
            if expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            14.0,
            style::DIM,
        ))
        .padding(4)
        .style(style::icon)
        .on_press(toggle)
        .boxed(),
        if expanded {
            tr::history_hide_files()
        } else {
            tr::history_show_files()
        },
    );
    const TOGGLE_W: f32 = 22.0;
    let body = column![top, meta].spacing(2).width(Fill);
    let main = |toggle: El<'a>| {
        button(row![body, toggle].spacing(8).align_y(Alignment::Center))
            .padding([6, 12])
            .width(Fill)
            .style(style::row(active))
            .on_press(open)
            .boxed()
    };
    let row_el = if expanded {
        main(chevron)
    } else {
        hover(
            main(space().width(TOGGLE_W).boxed()),
            container(chevron)
                .padding(Padding {
                    right: 12.0,
                    ..Padding::ZERO
                })
                .width(Fill)
                .height(Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Center),
        )
        .boxed()
    };
    column![row_el, hline()].boxed()
}

/// Desk's `CommitFileRow`: file icon, path (cut from the start), change kind; opens the file's
/// diff in the commit.
fn commit_file_row<'a>(f: &'a ide_client::GitChange, active: bool, open: Message) -> El<'a> {
    let st = crate::detail::view_status(f.kind);
    let kind = tip(
        container(
            text(st.letter().to_string())
                .size(10)
                .font(ide_views::theme::MONO_BOLD)
                .line_height(1.0),
        )
        .center(16)
        .style(ide_views::theme::status_box(st.color()))
        .boxed(),
        st.label(),
    );
    let title = match &f.orig_path {
        Some(o) => format!("{o} → {}", f.path),
        None => f.path.clone(),
    };
    let line = row![
        icons::file_icon(file_name(&f.path)),
        tip(
            text(f.path.as_str())
                .size(12)
                .color(if st == ide_views::GitStatus::Deleted {
                    style::DIM
                } else {
                    style::TEXT
                })
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::Start)
                .width(Fill)
                .boxed(),
            title,
        ),
        kind,
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let b = button(line)
        .padding(Padding {
            top: 6.0,
            right: 12.0,
            bottom: 6.0,
            left: 22.0,
        })
        .width(Fill)
        .style(move |_, status| {
            let bg = match (active, status) {
                (true, _) => style::SELECTED,
                (false, button::Status::Hovered | button::Status::Pressed) => style::HOVER,
                _ => COMMIT_FILES_BG,
            };
            button::Style {
                background: Some(bg.into()),
                text_color: style::TEXT,
                ..Default::default()
            }
        })
        .on_press(open);
    column![b, hline()].boxed()
}

/// A search hit's line with the matches highlighted (leading whitespace trimmed).
fn hit_text<'a>(line: &str, ranges: &[ide_client::ByteRange]) -> El<'a> {
    let trim = line.len() - line.trim_start().len();
    let mut parts = row![].align_y(Alignment::Center);
    let mut at = trim;
    let piece = |s: &str| {
        text(s.to_owned())
            .size(12)
            .font(style::MONO)
            .wrapping(text::Wrapping::None)
    };
    for r in ranges {
        let (a, b) = (r.start as usize, r.end as usize);
        if a < at || b > line.len() || !line.is_char_boundary(a) || !line.is_char_boundary(b) {
            continue;
        }
        if a > at {
            parts = parts.push(piece(&line[at..a]).boxed());
        }
        parts = parts.push(
            container(piece(&line[a..b]).color(style::TEXT))
                .style(|_| container::Style {
                    background: Some(ide_views::theme::FIND_MATCH.into()),
                    border: iced::Border {
                        color: ide_views::theme::FIND_MATCH_BORDER,
                        width: 1.0,
                        radius: 2.0.into(),
                    },
                    ..Default::default()
                })
                .boxed(),
        );
        at = b;
    }
    if at < line.len() {
        parts = parts.push(piece(&line[at..]).color(style::DIM).boxed());
    }
    container(parts).width(Fill).clip(true).boxed()
}

fn vline_short<'a>() -> El<'a> {
    container(space())
        .width(1)
        .height(20)
        .style(style::bg(style::BORDER))
        .boxed()
}

/// A centred message in the pane (loading, errors, binary files).
fn message_body<'a>(msg: String, error: bool) -> El<'a> {
    container(
        text(msg)
            .size(13)
            .color(if error { style::RED } else { style::DIM }),
    )
    .center(Fill)
    .padding(20)
    .boxed()
}

/// The bar over a file: save conflict, changed or deleted on disk.
fn file_bar<'a>(sid: &str, path: &str, bar: FileBar) -> El<'a> {
    let m = |x: DetailMsg| Some(Message::Detail(sid.to_owned(), x));
    let p = || path.to_owned();
    let msg = match bar {
        FileBar::Conflict => tr::file_conflict(),
        FileBar::ChangedOnDisk => tr::file_changed_on_disk(),
        FileBar::Deleted => tr::file_deleted_on_disk(),
    };
    let mut r = row![
        text("⚠").size(13).color(style::AMBER),
        text(msg)
            .size(12)
            .wrapping(text::Wrapping::WordOrGlyph)
            .width(Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if bar != FileBar::Deleted {
        r = r.push(icon_button(
            I_RELOAD_DISK,
            m(DetailMsg::BarReload(p())),
            tr::bar_reload(),
        ));
    }
    r = r.push(icon_button(
        I_OVERWRITE,
        m(DetailMsg::BarOverwrite(p())),
        tr::bar_overwrite(),
    ));
    if bar != FileBar::Deleted {
        r = r.push(icon_button(
            I_COMPARE,
            m(DetailMsg::BarCompare(p())),
            tr::bar_compare(),
        ));
    }
    r = r.push(icon_button(
        I_CLOSE,
        m(DetailMsg::BarDismiss(p())),
        tr::close(),
    ));
    container(r)
        .padding([4, 10])
        .width(Fill)
        .style(|_| container::Style {
            background: Some(ide_views::theme::over(style::AMBER, 0.14, style::PANEL).into()),
            text_color: Some(style::TEXT),
            border: iced::Border {
                color: ide_views::theme::over(style::AMBER, 0.45, style::PANEL),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .boxed()
}
