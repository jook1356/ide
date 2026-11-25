//! The 위젯 page (Hermes Desk's 모드 page: `ModsNavList` + `ModDetail`) and the status bar's
//! widget launcher (Desk's `ModLauncher`). Widgets are registered once and every session can
//! open them; their `scope` decides where the data lives and so how they group here.

use iced::widget::{Column, button, column, container, popover, row, scrollable, space, text};
use iced::{Alignment, Element, Fill, Widget as _};
use ide_client::WidgetInfo;

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::style;
use crate::ui_state::Page;
use crate::view::{card, error_box, tip};
use crate::widgets_ui::WidgetMsg;

type El<'a> = Element<'a, Message>;

fn wm(m: WidgetMsg) -> Message {
    Message::Widget(m)
}

/// What a scope means, for the page and the registration dialog.
pub(crate) fn scope_text(scope: &str) -> String {
    match scope {
        "global" => tr::widget_scope_global(),
        "family" => tr::widget_scope_family(),
        _ => tr::widget_scope_session(),
    }
}

/// The page's groups, in Desk's order: per family, per session, shared.
fn groups() -> [(&'static str, String); 3] {
    [
        ("family", tr::widget_group_family()),
        ("session", tr::widget_group_session()),
        ("global", tr::widget_group_global()),
    ]
}

impl App {
    /// The widget is open in the selected session (its 공용 tab for a global one).
    fn widget_open_here(&self, w: &WidgetInfo) -> bool {
        self.selected
            .as_deref()
            .is_some_and(|s| self.widget_tab_id(s, &w.id).is_some())
    }

    /// Open / close `w` in the selected session (`None`: no session to open it in).
    fn widget_toggle_msg(&self, w: &WidgetInfo) -> Option<Message> {
        let session = self.selected.clone()?;
        let widget = w.id.clone();
        Some(if self.widget_open_here(w) {
            wm(WidgetMsg::Close { session, widget })
        } else {
            wm(WidgetMsg::Open { session, widget })
        })
    }

    /// The 위젯 page: the list beside the rail (hidden with the rail button) and the picked widget.
    pub(crate) fn widgets_page(&self) -> El<'_> {
        let page = self.widget_view();
        if !self.ui.sessions_open {
            return page;
        }
        row![self.widget_list(), self.list_handle(), page]
            .height(Fill)
            .boxed()
    }

    /// Desk's `ModsNavList`: the widgets grouped by data scope.
    fn widget_list(&self) -> El<'_> {
        let head = row![
            text(tr::nav_widgets()).size(13).font(style::bold()),
            space().width(Fill),
            tip(
                button(icons::icon(Icon::SidebarCollapse, 16.0, style::DIM))
                    .padding(4)
                    .style(style::icon)
                    .on_press(Message::Page(Page::Widgets))
                    .boxed(),
                tr::widgets_hide(),
            ),
        ]
        .align_y(Alignment::Center);
        let mut list: Column<El<'_>> = Column::new().spacing(2);
        for (scope, title) in groups() {
            let mut members = self.widgets.list.iter().filter(|w| w.scope == scope).peekable();
            if members.peek().is_none() {
                continue;
            }
            list = list.push(
                container(text(title).size(11).color(style::DIM))
                    .padding(iced::Padding {
                        top: 8.0,
                        bottom: 2.0,
                        left: 4.0,
                        right: 4.0,
                    })
                    .boxed(),
            );
            for w in members {
                let sel = self.widgets.selected.as_deref() == Some(w.id.as_str());
                let open = self.widget_open_here(w);
                list = list.push(
                    button(
                        row![
                            icons::icon(Icon::Puzzle, 16.0, if sel { style::ACCENT_TEXT } else { style::DIM }),
                            text(w.name.as_str())
                                .size(13)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End)
                                .width(Fill),
                            open_dot(open),
                            text(tr::widget_version(&w.version.to_string()))
                                .size(10)
                                .color(style::FAINT),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .width(Fill)
                    .padding([6, 8])
                    .style(style::row(sel))
                    .on_press(wm(WidgetMsg::Select(w.id.clone())))
                    .boxed(),
                );
            }
        }
        if self.widgets.list.is_empty() {
            list = list.push(text(tr::widgets_none()).size(12).color(style::DIM).boxed());
        }
        container(column![head, scrollable(list).height(Fill)].spacing(10).padding(10))
            .width(self.list_w())
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
            .boxed()
    }

    /// Desk's `ModDetail`: what the picked widget is, where it is open, open / close / delete.
    fn widget_view(&self) -> El<'_> {
        let online = self.connected();
        let selected = self
            .widgets
            .selected
            .as_deref()
            .and_then(|id| self.widgets.get(id));
        let mut c = Column::<El<'_>>::new().spacing(20);
        let Some(w) = selected else {
            c = c.push(
                container(text(tr::nav_widgets()).size(22).font(style::bold()))
                    .height(40)
                    .align_y(Alignment::Center)
                    .boxed(),
            );
            c = c.push(text(tr::widgets_note()).size(12).color(style::DIM).boxed());
            if let Some(e) = &self.widgets.error {
                c = c.push(error_box(e));
            }
            let hint = if self.widgets.list.is_empty() {
                tr::widgets_hint()
            } else {
                tr::widgets_pick()
            };
            c = c.push(text(hint).size(13).color(style::DIM).boxed());
            return self.page_frame(c);
        };
        let open_here = self.widget_open_here(w);
        let toggle = button(
            text(if open_here { tr::widget_close_here() } else { tr::widget_open_here() }).size(12),
        )
        .padding([5, 12])
        .style(if open_here { style::secondary } else { style::primary })
        .on_press_maybe(self.widget_toggle_msg(w).filter(|_| online));
        let toggle: El<'_> = if self.selected.is_none() {
            tip(toggle.boxed(), tr::widget_launcher_no_session())
        } else {
            toggle.boxed()
        };
        let delete: El<'_> = if self.widgets.delete_armed {
            row![
                button(text(tr::widget_delete()).size(12))
                    .padding([5, 12])
                    .style(style::danger)
                    .on_press_maybe(online.then(|| wm(WidgetMsg::Delete(w.id.clone())))),
                button(text(tr::cancel()).size(12))
                    .padding([5, 12])
                    .style(style::secondary)
                    .on_press(wm(WidgetMsg::DeleteArm(false))),
            ]
            .spacing(6)
            .boxed()
        } else {
            tip(
                button(icons::icon(Icon::Trash, 16.0, style::DIM))
                    .padding(6)
                    .style(style::icon)
                    .on_press_maybe(online.then_some(wm(WidgetMsg::DeleteArm(true))))
                    .boxed(),
                tr::widget_delete(),
            )
        };
        c = c.push(
            row![
                icons::icon(Icon::Puzzle, 26.0, style::ACCENT_TEXT),
                column![
                    text(w.name.as_str())
                        .size(22)
                        .font(style::bold())
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                    text(w.id.as_str()).size(12).font(style::mono()).color(style::DIM),
                ]
                .spacing(2)
                .width(Fill),
                toggle,
                delete,
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .boxed(),
        );
        if self.widgets.delete_armed {
            c = c.push(text(tr::widget_delete_note()).size(12).color(style::DIM).boxed());
        }
        if let Some(e) = &self.widgets.error {
            c = c.push(error_box(e));
        }
        // Desk's info table.
        let line = |k: String, v: El<'static>| -> El<'static> {
            row![text(k).size(12).color(style::DIM).width(110), v]
                .spacing(12)
                .align_y(Alignment::Center)
                .boxed()
        };
        let value = |s: String| -> El<'static> { text(s).size(13).width(Fill).boxed() };
        let creator: El<'static> = match self.live(&w.created_by) {
            Some(l) => button(text(l.info.title.clone()).size(13).color(style::ACCENT_TEXT))
                .padding(0)
                .style(style::icon)
                .on_press(Message::Select(w.created_by.clone()))
                .boxed(),
            None if !w.created_by_title.is_empty() => value(w.created_by_title.clone()),
            None => value(tr::widget_session_gone()),
        };
        let tools = self.widget_tools(w, online);
        let mut info = column![
            line(
                tr::widget_label_description(),
                value(if w.description.is_empty() { "—".to_owned() } else { w.description.clone() })
            ),
            line(tr::widget_label_data(), value(scope_text(&w.scope))),
            line(tr::widget_label_tools(), tools),
            line(tr::widget_label_created_by(), creator),
            line(tr::widget_label_version(), value(tr::widget_version(&w.version.to_string()))),
            line(
                tr::widget_label_updated(),
                value(crate::history::full_date(i64::try_from(w.updated).unwrap_or(0)))
            ),
        ]
        .spacing(10);
        info = info.push(line(tr::widget_label_open_in(), self.widget_open_in(w)));
        c = c.push(card(info.boxed()));
        self.page_frame(c)
    }

    /// The tools and features a widget declared, each with 차단 / 허용 (the user's block policy,
    /// `widget.set_blocked`).
    fn widget_tools(&self, w: &WidgetInfo, online: bool) -> El<'static> {
        if w.tools.is_empty() {
            return text(tr::widget_tools_none()).size(13).boxed();
        }
        let mut col: Column<El<'static>> = Column::new().spacing(4);
        for t in &w.tools {
            let blocked = w.blocked.iter().any(|b| b == t);
            let what = match t.as_str() {
                "*" => Some(tr::widget_feature_all()),
                "exec" => Some(tr::widget_feature_exec()),
                "fs" => Some(tr::widget_feature_fs()),
                "terminal" => Some(tr::widget_feature_terminal()),
                "sessions" => Some(tr::widget_feature_sessions()),
                _ => None,
            };
            let mut r: iced::widget::Row<El<'static>> =
                row![text(t.clone()).size(12).font(style::mono()).boxed()]
                    .spacing(8)
                    .align_y(Alignment::Center);
            if let Some(what) = what {
                r = r.push(text(what).size(12).color(style::DIM).boxed());
            }
            r = r.push(space().width(Fill).boxed());
            if blocked {
                r = r.push(text(tr::widget_feature_blocked()).size(11).color(style::RED).boxed());
            }
            r = r.push(
                button(text(if blocked { tr::widget_unblock() } else { tr::widget_block() }).size(11))
                    .padding([2, 8])
                    .style(style::secondary)
                    .on_press_maybe(online.then(|| {
                        wm(WidgetMsg::SetBlocked {
                            widget: w.id.clone(),
                            name: t.clone(),
                            blocked: !blocked,
                        })
                    }))
                    .boxed(),
            );
            col = col.push(r.boxed());
        }
        col = col.push(text(tr::widget_block_note()).size(11).color(style::FAINT).boxed());
        col.width(Fill).boxed()
    }

    /// The sessions showing a widget (a global one: 공용).
    fn widget_open_in(&self, w: &WidgetInfo) -> El<'static> {
        if w.open.is_empty() {
            return text(tr::widget_open_none()).size(13).color(style::DIM).boxed();
        }
        if crate::widgets_ui::is_global(w) {
            return text(tr::widget_open_common()).size(13).boxed();
        }
        let mut col: Column<El<'static>> = Column::new().spacing(4);
        for s in &w.open {
            let title = self
                .live(s)
                .map_or_else(tr::widget_session_gone, |l| l.info.title.clone());
            col = col.push(
                button(text(title).size(13).color(style::ACCENT_TEXT))
                    .padding(0)
                    .style(style::icon)
                    .on_press(Message::Select(s.clone()))
                    .boxed(),
            );
        }
        col.boxed()
    }

    /// The status bar's widget button and its popup (Desk's `ModLauncher`): every widget with
    /// whether it is open in the selected session; a click opens or closes it there.
    pub(crate) fn widget_launcher(&self) -> El<'_> {
        let open = self.widgets.launcher;
        let b = tip(
            button(icons::icon(Icon::Puzzle, 13.0, if open { style::ACCENT_TEXT } else { style::DIM }))
                .padding(3)
                .style(style::icon)
                .on_press(wm(WidgetMsg::Launcher(!open)))
                .boxed(),
            tr::nav_widgets(),
        );
        let popup = open.then(|| self.widget_launcher_panel());
        popover(b, popup)
            .position(popover::Position::Top)
            .gap(6)
            .on_close(wm(WidgetMsg::Launcher(false)))
            .boxed()
    }

    fn widget_launcher_panel(&self) -> El<'_> {
        let online = self.connected();
        let mut col: Column<El<'_>> = Column::new().spacing(2);
        col = col.push(
            row![
                text(tr::nav_widgets()).size(13).font(style::bold()),
                space().width(Fill),
                button(text(tr::widget_manage()).size(11).color(style::ACCENT_TEXT))
                    .padding([2, 4])
                    .style(style::icon)
                    .on_press(Message::Page(Page::Widgets)),
            ]
            .align_y(Alignment::Center)
            .boxed(),
        );
        if self.selected.is_none() && !self.widgets.list.is_empty() {
            col = col.push(text(tr::widget_launcher_no_session()).size(11).color(style::DIM).boxed());
        }
        // Desk's sections: 공용, then the ones whose data follows the session.
        let sections = [
            (tr::widget_launcher_common(), true),
            (tr::widget_launcher_session(), false),
        ];
        for (title, global) in sections {
            let mut members = self
                .widgets
                .list
                .iter()
                .filter(|w| crate::widgets_ui::is_global(w) == global)
                .peekable();
            if members.peek().is_none() {
                continue;
            }
            col = col.push(
                container(text(title).size(11).color(style::DIM))
                    .padding(iced::Padding {
                        top: 6.0,
                        bottom: 2.0,
                        left: 2.0,
                        right: 2.0,
                    })
                    .boxed(),
            );
            for w in members {
                let here = self.widget_open_here(w);
                let state = if here { tr::widget_state_open() } else { tr::widget_state_closed() };
                let r = button(
                    row![
                        open_dot(here),
                        text(w.name.as_str())
                            .size(13)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::End)
                            .width(Fill),
                        text(state).size(11).color(style::DIM),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding([5, 8])
                .style(style::row(false))
                .on_press_maybe(self.widget_toggle_msg(w).filter(|_| online));
                col = col.push(r.boxed());
            }
        }
        if self.widgets.list.is_empty() {
            col = col.push(text(tr::widgets_none()).size(12).color(style::DIM).boxed());
        }
        container(scrollable(col.padding(8)).height(iced::Length::Shrink))
            .width(280)
            .style(style::modal_card)
            .boxed()
    }
}

/// Green when open, faint when not.
fn open_dot<'a>(open: bool) -> El<'a> {
    container(space().width(7).height(7))
        .style(style::dot(if open { style::GREEN } else { style::FAINT }))
        .boxed()
}

impl App {
    /// A page's content column, centred and scrollable like the repos page.
    fn page_frame<'a>(&self, c: Column<El<'a>>) -> El<'a> {
        let avail = self.window_size.width - self.left_w();
        let width = (avail - 64.0).clamp(320.0, 896.0);
        container(scrollable(container(c.width(width)).center_x(Fill).padding([32, 0])))
            .width(Fill)
            .height(Fill)
            .style(style::bg(style::BG))
            .boxed()
    }
}
