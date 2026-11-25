//! 열린 항목 as a vertical tab list in the detail window (Hermes Desk's `DetailRail.tsx`,
//! `lib/detailRail.ts`; settings in `ui.json` `detail_rail`).
//!
//! Pinned, the full list (240 px) sits beside the content under the pane's header. Unpinned it
//! is a 44 px icon rail that, hovered for a moment, widens in place over the content. The list's
//! top has its settings (세로 탭 / 팝오버, left / right, closing idle items) and the pin; in
//! popover mode the header's 열린 항목 popover has a button back to the tab list.

use std::time::Duration;

use iced::widget::{
    Column, button, column, container, mouse_area, popover, row, scrollable, space, stack, text,
};
use iced::{Alignment, Border, Color, Element, Fill, Length, Shadow, Task, Vector, Widget as _};

use crate::app::{App, Live, Message};
use crate::detail::{CloseSet, DetailMsg, Item, ItemKey, file_name};
use crate::detail_view::{I_CLOSE, item_icon, item_names};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::style;
use crate::ui_state::{RailMode, RailSide};
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

/// Icon rail width, and the open list's (Desk's `RAIL` / `WIDE`).
const NARROW: f32 = 44.0;
const WIDE: f32 = 240.0;
/// Hovering this long opens the list (passing over the rail does not).
const OPEN_DELAY: Duration = Duration::from_millis(120);
const ROW_H: f32 = 32.0;
const GROUP_H: f32 = 22.0;
/// An item's icon box, and its inset in a row: (rail inner width 32 - 18) / 2, so the icon does
/// not move when the rail widens.
const ICON_BOX: f32 = 18.0;
const ICON_X: f32 = 7.0;

#[derive(Debug, Clone)]
pub enum RailMsg {
    SetMode(RailMode),
    SetSide(RailSide),
    TogglePinned,
    /// The pointer entered the icon rail: open after [`OPEN_DELAY`] if it stays.
    Enter,
    /// The pointer left the icon rail before it opened.
    LeaveRail,
    /// The delay of the `n`th [`RailMsg::Enter`] is over.
    Arm(u64),
    /// The pointer left the opened list.
    Leave,
    /// The settings menu opened / closed (the opened list stays meanwhile).
    Menu(bool),
}

/// A group of 열린 항목 (the popover's groups) with its colour on the icon rail.
struct Group {
    label: String,
    color: Color,
    is: fn(&ItemKey) -> bool,
}

fn groups() -> [Group; 4] {
    [
        Group {
            label: tr::group_browser(),
            color: style::ORANGE,
            is: |k| matches!(k, ItemKey::Web),
        },
        Group {
            label: tr::side_changes(),
            color: style::YELLOW,
            is: |k| matches!(k, ItemKey::Diff { .. } | ItemKey::Compare(_)),
        },
        Group {
            label: tr::side_history(),
            color: style::TEAL,
            is: |k| matches!(k, ItemKey::Commit { .. }),
        },
        Group {
            label: tr::side_explore(),
            color: style::ACCENT_TEXT,
            is: |k| matches!(k, ItemKey::File(_)),
        },
    ]
}

impl App {
    /// 열린 항목 shows as the tab list in this pane (the detail window in tabs mode).
    pub(crate) fn rail_shown(&self, in_window: bool) -> bool {
        in_window && self.ui.detail_rail.mode == RailMode::Tabs
    }

    pub(crate) fn on_rail(&mut self, m: RailMsg) -> Task<Message> {
        match m {
            RailMsg::SetMode(mode) => {
                self.ui.detail_rail.mode = mode;
                self.rail_menu = false;
                self.rail_open = false;
                self.open_items = false;
                self.save_ui();
            }
            RailMsg::SetSide(side) => {
                self.ui.detail_rail.side = side;
                self.rail_menu = false;
                self.save_ui();
            }
            RailMsg::TogglePinned => {
                self.ui.detail_rail.pinned = !self.ui.detail_rail.pinned;
                self.rail_open = false;
                self.save_ui();
            }
            RailMsg::Enter => {
                if self.ui.detail_rail.pinned || self.rail_open {
                    return Task::none();
                }
                self.rail_arm += 1;
                let n = self.rail_arm;
                return Task::perform(tokio::time::sleep(OPEN_DELAY), move |()| {
                    Message::Rail(RailMsg::Arm(n))
                });
            }
            RailMsg::LeaveRail => {
                if !self.rail_open {
                    self.rail_arm += 1;
                }
            }
            RailMsg::Arm(n) => {
                if n == self.rail_arm && !self.ui.detail_rail.pinned {
                    self.rail_open = true;
                }
            }
            RailMsg::Leave => {
                if !self.rail_menu {
                    self.rail_open = false;
                }
            }
            RailMsg::Menu(on) => self.rail_menu = on,
        }
        Task::none()
    }

    /// `body` (the pane under its header) with the tab list on its side.
    pub(crate) fn with_rail<'a>(&'a self, live: &'a Live, body: El<'a>) -> El<'a> {
        let prefs = self.ui.detail_rail;
        let open = !prefs.pinned && self.rail_open;
        // The settings menu belongs to the list on show: the opened one while it is up.
        let beside = self.rail_list(live, prefs.pinned, !open);
        let mut slot = mouse_area(
            container(beside)
                .width(if prefs.pinned { WIDE } else { NARROW })
                .height(Fill)
                .style(style::bg(style::SIDEBAR)),
        );
        if !prefs.pinned {
            slot = slot
                .on_enter(Message::Rail(RailMsg::Enter))
                .on_exit(Message::Rail(RailMsg::LeaveRail));
        }
        let line = || {
            container(space())
                .width(1)
                .height(Fill)
                .style(style::bg(style::BORDER))
        };
        let base: El<'a> = match prefs.side {
            RailSide::Left => row![slot, line(), body].boxed(),
            RailSide::Right => row![body, line(), slot].boxed(),
        };
        let overlay: El<'a> = if open {
            let list = mouse_area(
                container(self.rail_list(live, true, true))
                    .width(WIDE)
                    .height(Fill)
                    .style(|_| container::Style {
                        background: Some(style::SIDEBAR.into()),
                        shadow: Shadow {
                            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
                            offset: Vector::new(0.0, 4.0),
                            blur_radius: 18.0,
                        },
                        ..Default::default()
                    }),
            )
            .on_exit(Message::Rail(RailMsg::Leave));
            match prefs.side {
                RailSide::Left => row![list, line(), space().width(Fill)].boxed(),
                RailSide::Right => row![space().width(Fill), line(), list].boxed(),
            }
        } else {
            space().boxed()
        };
        // Always a stack, so opening the list keeps the content's widget tree.
        stack![base, overlay].width(Fill).height(Fill).boxed()
    }

    /// The list: settings and pin on top, then the groups. `wide`: names, labels and close
    /// buttons (else icons and coloured group lines). `menu`: this one shows the settings menu.
    fn rail_list<'a>(&'a self, live: &'a Live, wide: bool, menu: bool) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.as_str();
        let prefs = self.ui.detail_rail;
        let settings = button(
            container(icons::icon(Icon::ChevronDown, 16.0, style::DIM)).center(28),
        )
        .width(28)
        .height(28)
        .padding(0)
        .style(style::icon_toggle(self.rail_menu && menu))
        .on_press(Message::Rail(RailMsg::Menu(!self.rail_menu)));
        let popup = (self.rail_menu && menu).then(|| self.rail_menu_view(sid));
        let settings = popover(tip(settings.boxed(), tr::rail_settings()), popup)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(Message::Rail(RailMsg::Menu(false)));
        let mut top = row![settings].align_y(Alignment::Center).height(ROW_H);
        if wide {
            let pinned = prefs.pinned;
            let pin = button(
                container(icons::icon(
                    if pinned { Icon::PinFilled } else { Icon::Pin },
                    15.0,
                    if pinned { style::ACCENT_TEXT } else { style::DIM },
                ))
                .center(28),
            )
            .width(28)
            .height(28)
            .padding(0)
            .style(style::icon_toggle(pinned))
            .on_press(Message::Rail(RailMsg::TogglePinned));
            let label = if pinned { tr::rail_unpin() } else { tr::rail_pin() };
            top = top.push(space().width(Fill).boxed()).push(tip(pin.boxed(), label));
        }

        let mut list = Column::new();
        let shared = self.shared_sections(live, wide);
        let mut listed = !shared.is_empty();
        for e in shared {
            list = list.push(e);
        }
        for g in groups() {
            let mut rows: Vec<(ItemKey, bool)> = d
                .items
                .iter()
                .filter(|(k, _)| (g.is)(k) && !self.is_common(sid, k))
                .map(|(k, item)| (k.clone(), matches!(item, Item::File(f) if f.dirty())))
                .collect();
            // The session's browser tabs follow its web page.
            if (g.is)(&ItemKey::Web) {
                rows.extend(
                    live.browsers
                        .iter()
                        .map(|b| (ItemKey::Browser(b.id), false))
                        .filter(|(k, _)| !self.is_common(sid, k)),
                );
            }
            if rows.is_empty() {
                continue;
            }
            listed = true;
            list = list.push(group_heading(g.label, rows.len(), g.color, wide));
            for (k, dirty) in &rows {
                list = list.push(self.item_row(
                    live,
                    rail_row(self, sid, live, k, *k == d.active, *dirty, wide),
                    k,
                ));
            }
            list = list.push(space().height(4).boxed());
        }
        // The shown session's terminals (`uterm.rs`), then Desk's "+ 새로 열기" row.
        let term_live = self.term_live(live);
        let terms: Vec<ItemKey> = term_live
            .uterms
            .iter()
            .map(|t| ItemKey::Terminal(t.info.id.clone()))
            .filter(|k| !self.is_common(term_live.id(), k))
            .collect();
        if !terms.is_empty() {
            listed = true;
            list = list.push(group_heading(
                tr::group_terminal(),
                terms.len(),
                crate::uterm::GROUP_COLOR,
                wide,
            ));
            for k in &terms {
                let row = rail_row(self, sid, live, k, *k == d.active, false, wide);
                list = list.push(self.item_row(live, row, k));
            }
            list = list.push(space().height(4).boxed());
        }
        list = list.push(self.new_item_button(live, wide));
        if !listed && wide {
            list = list.push(
                container(text(tr::rail_empty()).size(12).color(style::DIM))
                    .padding(8)
                    .boxed(),
            );
        }
        column![
            container(top).padding([0, 2]),
            scrollable(list).height(Fill),
        ]
        .spacing(4)
        .padding(6)
        .width(Fill)
        .height(Fill)
        .boxed()
    }

    /// The list's settings menu (Desk's 열린 항목 설정).
    fn rail_menu_view<'a>(&'a self, sid: &str) -> El<'a> {
        let prefs = self.ui.detail_rail;
        let label = |s: String| {
            container(text(s).size(11).color(style::DIM))
                .padding([4, 8])
                .boxed()
        };
        let item = |s: String, on: bool, msg: Message| {
            let check: El<'a> = if on {
                icons::icon(Icon::Check, 14.0, style::ACCENT_TEXT)
            } else {
                space().width(14).boxed()
            };
            button(
                row![text(s).size(13).width(Fill), check]
                    .spacing(8)
                    .align_y(Alignment::Center),
            )
            .padding([5, 8])
            .width(Fill)
            .style(style::row(false))
            .on_press(msg)
            .boxed()
        };
        let rule = || {
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER))
                .boxed()
        };
        let rail = |m| Message::Rail(m);
        container(
            column![
                label(tr::rail_show_as()),
                item(
                    tr::rail_tabs(),
                    prefs.mode == RailMode::Tabs,
                    rail(RailMsg::SetMode(RailMode::Tabs))
                ),
                item(
                    tr::rail_popover(),
                    prefs.mode == RailMode::Popover,
                    rail(RailMsg::SetMode(RailMode::Popover))
                ),
                rule(),
                label(tr::rail_side()),
                item(
                    tr::rail_left(),
                    prefs.side == RailSide::Left,
                    rail(RailMsg::SetSide(RailSide::Left))
                ),
                item(
                    tr::rail_right(),
                    prefs.side == RailSide::Right,
                    rail(RailMsg::SetSide(RailSide::Right))
                ),
                rule(),
                item(
                    tr::close_all(),
                    false,
                    Message::Detail(sid.to_owned(), DetailMsg::CloseSome(CloseSet::All))
                ),
                item(
                    tr::close_idle(),
                    false,
                    Message::Detail(sid.to_owned(), DetailMsg::CloseSome(CloseSet::Idle))
                ),
            ]
            .spacing(2),
        )
        .padding(6)
        .width(220)
        .style(style::modal_card)
        .boxed()
    }
}

/// An item's icon in a fixed box at the same place on the icon rail and in the open list (the
/// popover's badges for diffs and commit files are left out: the group says what it is).
fn rail_icon<'a>(app: &App, live: &Live, k: &ItemKey) -> El<'a> {
    let icon = match k {
        ItemKey::Terminal(id) => match app.find_uterm(live, id) {
            Some(t) => crate::uterm::dot(t.info.state),
            None => item_icon(k),
        },
        ItemKey::File(p)
        | ItemKey::Diff { path: p, .. }
        | ItemKey::Compare(p)
        | ItemKey::Commit { path: Some(p), .. } => ide_views::icons::file_icon(file_name(p)),
        _ => item_icon(k),
    };
    container(icon).center(ICON_BOX).boxed()
}

/// One item of the list: its icon (with an unsaved dot on the icon rail), and wide its name
/// and close button.
fn rail_row<'a>(
    app: &'a App,
    sid: &str,
    live: &'a Live,
    k: &ItemKey,
    active: bool,
    dirty: bool,
    wide: bool,
) -> El<'a> {
    let (title, folder) = match k {
        ItemKey::Terminal(id) => app
            .find_uterm(live, id)
            .map(|t| (t.info.title.clone(), t.info.cwd.clone()))
            .unwrap_or_else(|| item_names(k, live)),
        ItemKey::Browser(id) => app
            .browser(live, *id)
            .map(|b| (b.label(), b.url_input.clone()))
            .unwrap_or_else(|| item_names(k, live)),
        _ => item_names(k, live),
    };
    let activate = Message::Detail(sid.to_owned(), DetailMsg::Activate(k.clone()));
    if !wide {
        let mut icon = stack![rail_icon(app, live, k)];
        if dirty {
            icon = icon.push(
                container(
                    container(space().width(6).height(6)).style(style::dot(style::ACCENT_TEXT)),
                )
                .width(ICON_BOX)
                .height(ICON_BOX)
                .align_x(Alignment::End)
                .align_y(Alignment::End)
                .boxed(),
            );
        }
        let b = button(container(icon).center_y(ROW_H))
            .padding([0.0, ICON_X])
            .width(Fill)
            .height(ROW_H)
            .style(style::row(active))
            .on_press(activate);
        let label = if folder.is_empty() || folder == title {
            title
        } else {
            format!("{title}\n{folder}")
        };
        return tip(b.boxed(), label);
    }
    let mut name = row![
        text(title)
            .size(12)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End)
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Fill);
    if dirty {
        name = name.push(text("●").size(9).color(style::ACCENT_TEXT).boxed());
    }
    let main = button(
        row![rail_icon(app, live, k), name]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([0.0, ICON_X])
    .width(Fill)
    .height(ROW_H)
    .style(style::row(active))
    .on_press(activate);
    // A terminal is stopped and removed (Desk's 종료하고 목록에서 지우기 / 목록에서 지우기).
    let close_tip = match k {
        ItemKey::Terminal(id)
            if app
                .find_uterm(live, id)
                .is_some_and(|t| t.info.state == ide_client::SessionState::Running) =>
        {
            tr::uterm_stop_remove()
        }
        ItemKey::Terminal(_) => tr::uterm_remove(),
        _ => tr::close(),
    };
    let close: El<'a> = icon_button(
        I_CLOSE,
        Some(Message::Detail(sid.to_owned(), DetailMsg::Close(k.clone()))),
        close_tip,
    );
    row![main, close]
        .spacing(2)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .boxed()
}

/// A group's heading: its name and count on the open list, a short line in its colour on the
/// icon rail (one height, so rows stay put when the rail widens).
fn group_heading<'a>(label: String, count: usize, color: Color, wide: bool) -> El<'a> {
    let inner: El<'a> = if wide {
        row![
            text(label).size(11).font(style::bold()).color(style::DIM),
            text(count.to_string()).size(11).color(style::FAINT),
        ]
        .spacing(6)
        .boxed()
    } else {
        let c = Color { a: 0.6, ..color };
        container(space())
            .height(2)
            .width(Fill)
            .style(move |_| container::Style {
                background: Some(c.into()),
                border: Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .boxed()
    };
    container(inner)
        .height(GROUP_H)
        .padding([0, 8])
        .center_y(GROUP_H)
        .boxed()
}

impl App {
    /// The item is a 공통 tab of its session.
    pub(crate) fn is_common(&self, sid: &str, k: &ItemKey) -> bool {
        self.common.iter().any(|(s, x)| s == sid && x == k)
    }

    /// A 공통 tab still exists in its session.
    fn common_alive(&self, owner: &Live, k: &ItemKey) -> bool {
        match k {
            ItemKey::Terminal(id) => owner.uterm(id).is_some(),
            ItemKey::Browser(id) => owner.browsers.iter().any(|b| b.id == *id),
            _ => owner.detail.item(k).is_some(),
        }
    }

    /// The top of 열린 항목 (Desk): 공용 items (no session), then the 공통 tabs grouped by
    /// the session they come from ("현재" for this one). Rows as on the tab list.
    pub(crate) fn shared_sections<'a>(&'a self, live: &'a Live, wide: bool) -> Vec<El<'a>> {
        let mut out: Vec<El<'a>> = Vec::new();
        let cur = live.info.id.as_str();
        let active = &live.detail.active;
        let global: Vec<ItemKey> = self
            .global_browsers
            .iter()
            .map(|b| ItemKey::Browser(b.id))
            .chain(self.global_uterms.iter().map(|t| ItemKey::Terminal(t.info.id.clone())))
            .collect();
        if !global.is_empty() {
            out.push(group_heading(tr::new_scope_global(), global.len(), style::FAINT, wide));
            for k in &global {
                let row = rail_row(self, cur, live, k, k == active, false, wide);
                out.push(tip(row, tr::global_item_tip()));
            }
            out.push(space().height(4).boxed());
        }
        let mut owners: Vec<&str> = Vec::new();
        for (s, _) in &self.common {
            if !owners.contains(&s.as_str()) {
                owners.push(s);
            }
        }
        for owner in owners {
            let Some(ol) = self.live(owner) else {
                continue;
            };
            let keys: Vec<&ItemKey> = self
                .common
                .iter()
                .filter(|(s, k)| s == owner && self.common_alive(ol, k))
                .map(|(_, k)| k)
                .collect();
            if keys.is_empty() {
                continue;
            }
            let label = if owner == cur {
                format!("{} · {}", ol.info.title, tr::common_current())
            } else {
                ol.info.title.clone()
            };
            out.push(group_heading(label, keys.len(), style::ACCENT_TEXT, wide));
            for k in keys {
                let on = owner == cur && k == active;
                out.push(self.item_row(ol, rail_row(self, owner, ol, k, on, false, wide), k));
            }
            out.push(space().height(4).boxed());
        }
        out
    }

    /// 공통 탭으로 고정 / 공통 해제.
    pub(crate) fn toggle_common(&mut self, sid: String, k: ItemKey) {
        match self.common.iter().position(|(s, x)| *s == sid && *x == k) {
            Some(i) => {
                self.common.remove(i);
            }
            None => self.common.push((sid, k)),
        }
    }
}
