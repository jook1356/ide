//! Window layout: navigation rail | page, where the work page is session list | terminal |
//! divider | detail; other pages (accounts, configurations, repos, settings) cover the work page,
//! which stays in the widget tree. Modal dialogs on top.

use iced::widget::{
    Column, Row, button, center, checkbox, column, container, mouse_area, opaque, pick_list, popover,
    row, scrollable, space, stack, text, text_input, tooltip,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Widget as _, mouse};
use ide_client::{AccountState, BrowseEntry};
use ide_i18n::LanguagePref;
use ide_term::TerminalView;

use crate::app::{
    ALL_CONFIGURATIONS, App, Choice, Conn, DIVIDER_W, Live, Message,
    Modal, NEW_TITLE_ID, NewForm, PICK_LIST_ID, PICK_PATH_ID, PickRootForm, QuickForm, RAIL_W,
    RunState, SSH_HOST_ID, SessionReposForm, SettingsForm, SortChange,
    TERM_ID,
};
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::session_sort::{SortDir, SortItem, SortKey};
use crate::style;
use crate::ui_state::{CloseAction, LOGIN_GROUP, Page, ServerPref};

type El<'a> = Element<'a, Message>;

// Symbols (Segoe UI Symbol / system fallback fonts on Windows).
const I_PLUS: &str = "+";
// U+25B8: U+25B6 would get the emoji presentation from Segoe UI Emoji.
const I_PLAY: &str = "▸";
const I_RESTART: &str = "↻";
const I_EDIT: &str = "✎";
const I_DELETE: &str = "✕";
const I_DOT: &str = "●";

/// Icon and colour of a provider (from the server's provider list).
pub fn provider_glyph(app: &App, id: &str) -> (String, Color) {
    match app.provider(id) {
        Some(p) => (
            p.glyph.clone(),
            p.color.map_or(style::DIM, |c| {
                Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
            }),
        ),
        None => ("●".into(), style::DIM),
    }
}

pub(crate) fn tip<'a>(content: impl Into<El<'a>>, label: String) -> El<'a> {
    tooltip(
        content.into(),
        container(text(label).size(12))
            .padding([3, 7])
            .style(style::modal_card),
    )
    .position(tooltip::Position::Bottom)
    .boxed()
}

pub(crate) fn icon_button<'a>(glyph: &'a str, msg: Option<Message>, label: String) -> El<'a> {
    let b = button(
        text(glyph)
            .size(13)
            .width(Length::Fixed(14.0))
            .align_x(Alignment::Center),
    )
    .padding([3, 5])
    .style(style::icon)
    .on_press_maybe(msg);
    tip(b.boxed(), label)
}

fn label<'a>(s: String) -> El<'a> {
    text(s).size(11).color(style::DIM).boxed()
}

/// A section of a page in a bordered card (Hermes Desk's `Paper withBorder p="md"`).
pub(crate) fn card(content: El<'_>) -> El<'_> {
    container(content)
        .padding(16)
        .width(Fill)
        .style(style::card)
        .boxed()
}

/// Frame of a server-side folder list (new-session browser, repos page picker).
fn browser_frame(content: El<'_>) -> El<'_> {
    container(content)
        .padding(8)
        .width(Fill)
        .style(|_| container::Style {
            background: Some(style::SIDEBAR.into()),
            border: iced::Border {
                color: style::BORDER,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .boxed()
}

/// One folder of a server-side folder list: folder icon, name, optional tag (e.g. home) and
/// a `git` badge for repositories.
fn folder_entry_row(e: &BrowseEntry, tag: Option<String>) -> El<'_> {
    let tint = if e.is_git_repo {
        style::GREEN
    } else {
        style::DIM
    };
    let mut r = row![
        ide_views::icons::folder_icon(e.is_git_repo, tint),
        text(e.name.as_str())
            .size(12)
            .width(Fill)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if let Some(t) = tag {
        r = r.push(text(t).size(11).color(style::DIM).boxed());
    }
    if e.is_git_repo {
        r = r.push(tip(
            container(text("git").size(10).font(style::BOLD).color(style::GREEN))
                .padding([0, 5])
                .style(|_| container::Style {
                    border: iced::Border {
                        color: style::GREEN,
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .boxed(),
            tr::browse_git(),
        ));
    }
    r.boxed()
}

/// What the session list sorts a session by.
fn sort_item(l: &Live) -> SortItem<'_> {
    SortItem {
        name: l.info.title.as_str(),
        updated: l.info.last_active,
        created: l.info.created_at,
        // Hermes Desk's 실행·응답 상태: what needs attention first.
        activity: match l.state {
            RunState::Failed => 0,
            RunState::Running => 1,
            RunState::Starting => 2,
            RunState::Exited(_) => 3,
            RunState::Stopped => 4,
        },
    }
}

fn sort_key_label(key: SortKey) -> String {
    match key {
        SortKey::Name => tr::sort_key_name(),
        SortKey::Updated => tr::sort_key_updated(),
        SortKey::Created => tr::sort_key_created(),
        SortKey::Activity => tr::sort_key_activity(),
    }
}

/// Hermes Desk's `AgentDivider`: "▾ name ───── 3", folding the group's rows on click.
/// Desk's `session-archive-toggle`: "보관함" with its count, folding the group's archived rows.
fn archive_toggle<'a>(key: String, count: usize, open: bool) -> El<'a> {
    let body = row![
        icons::icon(
            if open { Icon::ChevronDown } else { Icon::ChevronRight },
            12.0,
            style::DIM
        ),
        icons::icon(Icon::Archive, 14.0, style::DIM),
        text(tr::archive_label())
            .size(11)
            .color(style::DIM)
            .width(Fill),
        text(count.to_string()).size(11).color(style::FAINT),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let b = button(body)
        .padding([4, 6])
        .width(Fill)
        .style(style::row(false))
        .on_press(Message::ToggleArchiveOpen(key));
    tip(
        b.boxed(),
        if open {
            tr::archive_fold()
        } else {
            tr::archive_unfold()
        },
    )
}

fn group_divider<'a>(key: String, name: String, count: usize, folded: bool) -> El<'a> {
    let tooltip = if folded {
        tr::group_unfold(&name)
    } else {
        tr::group_fold(&name)
    };
    let body = row![
        text(if folded { "▸" } else { "▾" })
            .size(11)
            .color(style::DIM)
            .width(10),
        text(name)
            .size(11)
            .font(style::BOLD)
            .color(style::DIM)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End),
        container(container(space()).width(Fill).height(1).style(style::bg(style::BORDER)))
            .width(Fill)
            .padding(iced::Padding {
                left: 4.0,
                right: 4.0,
                ..iced::Padding::ZERO
            }),
        text(count.to_string()).size(11).color(style::FAINT),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let b = button(body)
        .padding(iced::Padding {
            top: 8.0,
            bottom: 3.0,
            left: 4.0,
            right: 6.0,
        })
        .width(Fill)
        .style(style::divider)
        .on_press(Message::ToggleGroupFold(key));
    tip(b.boxed(), tooltip)
}

pub(crate) fn error_box<'a>(e: &'a str) -> El<'a> {
    container(text(e).size(12))
        .padding(8)
        .width(Fill)
        .style(style::error_box)
        .boxed()
}

impl App {
    pub fn view(&self) -> El<'_> {
        // The session list's slot keeps its place when hidden (same child count), so the
        // terminal and the detail area keep their widget state.
        let (list, list_border): (El<'_>, El<'_>) = if self.ui.sessions_open {
            (self.sidebar(), self.list_handle())
        } else {
            (space().width(0).boxed(), space().width(0).boxed())
        };
        let work = row![
            list,
            list_border,
            self.centre(),
            self.divider(),
            self.detail_area(),
            self.side_area(),
        ]
        .height(Fill);
        // Other pages cover the work page (like Hermes Desk, which keeps it mounted).
        let page: El<'_> = if self.on_work_page() {
            space().boxed()
        } else {
            opaque(self.page_view()).into()
        };
        let base = row![self.rail(), stack![work, page].width(Fill).height(Fill),].height(Fill);
        // Desk's app footer under everything (only the self-update control here).
        let base = column![base, self.app_footer()].height(Fill);
        // Always a stack so opening/closing a modal keeps the widget tree (terminal).
        let overlay = self.modal_layer(false);
        let mut root = stack![base, overlay].boxed();
        // Frameless window (`frame.rs`): the title bar stays usable above modals.
        if let Some(id) = self.window {
            let max = self.window_maximized;
            let bar = crate::frame::title_bar(id, "ide".into(), space().boxed(), max);
            // The item menu is placed in window coordinates: a layer over the whole window.
            let window = stack![
                column![bar, root],
                self.item_menu_layer(false),
                self.session_menu_layer(),
                self.drag_ghost(),
                self.side_menu_layer()
            ]
                .width(Fill)
                .height(Fill);
            root = crate::frame::with_grips(id, window.boxed(), max);
        }
        if self.script.is_some() {
            ide_web::testing::Injector::new(root).into()
        } else {
            root
        }
    }

    /// `msg` if connected (server actions are disabled while disconnected).
    fn when_connected(&self, msg: Message) -> Option<Message> {
        self.connected().then_some(msg)
    }

    // ------------------------------------------------------------------ rail

    /// Icon and tooltip of a rail page button. On its own page the work button toggles the
    /// session list: hovered it shows the collapse / expand icon, and its tooltip says so.
    pub fn rail_entry(&self, page: Page) -> (Icon, String) {
        let (icon, label) = match page {
            Page::Work => (Icon::Messages, tr::nav_work()),
            Page::Accounts => (Icon::Key, tr::nav_accounts()),
            Page::Configurations => (Icon::Stack2, tr::nav_configurations()),
            Page::Repos => (Icon::GitBranch, tr::nav_repos()),
            Page::Settings => (Icon::Settings, tr::settings()),
        };
        // On its own page the work / profiles / repos button shows or hides the list beside the rail.
        if page != self.ui.page
            || !matches!(page, Page::Work | Page::Configurations | Page::Repos)
        {
            return (icon, label);
        }
        let open = self.ui.sessions_open;
        let icon = match (self.rail_hover, open) {
            (false, _) => icon,
            (true, true) => Icon::SidebarCollapse,
            (true, false) => Icon::SidebarExpand,
        };
        let label = match (page, open) {
            (Page::Work, true) => tr::sessions_hide(),
            (Page::Work, false) => tr::sessions_show(),
            (Page::Repos, true) => tr::repos_hide(),
            (Page::Repos, false) => tr::repos_show(),
            (_, true) => tr::profiles_hide(),
            (_, false) => tr::profiles_show(),
        };
        (icon, label)
    }

    /// Text of the footer's server indicator: target, state and (connected) server version.
    pub fn server_tooltip(&self) -> String {
        let target = self.server_label();
        match &self.conn {
            Conn::Connected(w) => {
                tr::rail_server_version(&target, &tr::conn_state_connected(), &w.server_version)
            }
            Conn::Connecting => tr::rail_server(&target, &tr::conn_state_connecting()),
            Conn::Reconnecting => tr::rail_server(&target, &tr::conn_state_reconnecting()),
            Conn::Disconnected(_) => tr::rail_server(&target, &tr::conn_state_disconnected()),
        }
    }

    /// Colour of the connection: green connected, amber (re)connecting, red disconnected.
    pub(crate) fn conn_color(&self) -> Color {
        match &self.conn {
            Conn::Connected(_) => style::GREEN,
            Conn::Connecting | Conn::Reconnecting => style::AMBER,
            Conn::Disconnected(_) => style::RED,
        }
    }

    /// Navigation rail (Hermes Desk's `NavRail`): 64 px, 44 px buttons 6 px apart (the server
    /// indicator is in the app footer).
    fn rail(&self) -> El<'_> {
        let link = |icon: Icon, active: bool, msg: Message| {
            let color = if active {
                style::RAIL_ACTIVE_ICON
            } else {
                style::RAIL_ICON
            };
            button(container(icons::icon(icon, 22.0, color)).center(44))
                .width(44)
                .height(44)
                .padding(0)
                .style(style::rail_link(active))
                .on_press(msg)
        };
        let tip = |content: El<'static>, label: String| -> El<'static> {
            tooltip(
                content,
                container(text(label).size(14))
                    .padding([5, 10])
                    .style(style::rail_tip),
            )
            .position(tooltip::Position::Right)
            .gap(8)
            .boxed()
        };
        let mut top: Column<El<'_>> = Column::new().spacing(6).align_x(Alignment::Center);
        for page in [
            Page::Work,
            Page::Accounts,
            Page::Configurations,
            Page::Repos,
            Page::Settings,
        ] {
            let (icon, label) = self.rail_entry(page);
            let mut b: El<'static> = link(icon, self.ui.page == page, Message::Page(page)).boxed();
            if matches!(page, Page::Work | Page::Configurations | Page::Repos) {
                b = mouse_area(b)
                    .on_enter(Message::RailHover(true))
                    .on_exit(Message::RailHover(false))
                    .boxed();
            }
            // The active page also gets an accent bar on the rail's left edge, 11 px in from the
            // button's top and bottom.
            let bar: El<'_> = if self.ui.page == page {
                container(
                    container(space())
                        .width(3)
                        .height(22)
                        .style(style::rail_accent),
                )
                .padding(iced::Padding {
                    top: 11.0,
                    ..iced::Padding::ZERO
                })
                .boxed()
            } else {
                space().boxed()
            };
            top = top.push(
                stack![container(tip(b, label)).center_x(Fill), bar]
                    .width(Fill)
                    .boxed(),
            );
        }
        let col = column![top.height(Fill)]
        .width(Fill)
        .align_x(Alignment::Center)
        .padding(iced::Padding {
            top: 12.0,
            ..iced::Padding::ZERO
        });
        row![
            container(col)
                .width(RAIL_W - 1.0)
                .height(Fill)
                .style(style::bg(style::RAIL)),
            container(space())
                .width(1)
                .height(Fill)
                .style(style::bg(style::RAIL_BORDER)),
        ]
        .boxed()
    }

    // ------------------------------------------------------------------ sidebar

    fn sidebar(&self) -> El<'_> {
        // Hermes Desk's agent select: 전체 (every configuration) or one configuration.
        let mut configs = vec![Choice {
            key: ALL_CONFIGURATIONS.to_owned(),
            label: tr::configurations_all(),
        }];
        configs.extend(self.config_choices());
        let current = if self.ui.sessions_all {
            ALL_CONFIGURATIONS.to_owned()
        } else {
            self.current_configuration()
        };
        let selected = configs.iter().find(|c| c.key == current).cloned();
        let profile = row![
            label(tr::configuration_label()),
            space().width(8),
            pick_list(selected, configs, |c: &Choice| c.label.clone())
                .on_select(|c: Choice| Message::ConfigurationPicked(c.key))
                .text_size(13)
                .padding([4, 8])
                .width(Fill),
        ]
        .align_y(Alignment::Center);
        let sessions_header = row![
            label(tr::sessions_label()),
            space().width(Fill),
            self.sort_control(),
            icon_button(
                I_PLUS,
                self.when_connected(Message::OpenNew),
                tr::new_session()
            ),
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let mut list: Column<El<'_>> = Column::new().spacing(2);
        let groups = self.sidebar_groups();
        let empty = groups.is_empty();
        for (key, name, members) in groups {
            let folded = !key.is_empty() && self.ui.folded_groups.contains(&key);
            // Desk: each block is its live rows, then its 보관함 (archived rows, folded by default).
            let (archived, members): (Vec<&Live>, Vec<&Live>) =
                members.into_iter().partition(|l| l.info.archived);
            if !key.is_empty() {
                list = list.push(group_divider(key.clone(), name, members.len(), folded));
            }
            if !folded {
                // Children under their parent (docs/session-family.md).
                for (l, depth) in self.family_order(members) {
                    list = list.push(self.session_row(l, depth));
                }
                if !archived.is_empty() {
                    let open = self.ui.archive_open.contains(&key);
                    list = list.push(archive_toggle(key, archived.len(), open));
                    if open {
                        for (l, depth) in self.family_order(archived) {
                            list = list.push(self.session_row(l, depth));
                        }
                    }
                }
            }
        }
        if empty && self.loaded {
            list = list.push(
                container(text(tr::no_sessions()).size(12).color(style::DIM))
                    .padding([8, 4])
                    .boxed(),
            );
        }

        let mut col = column![sessions_header, profile, scrollable(list).height(Fill),]
            .spacing(10)
            .padding(10);
        if let Some(b) = &self.banner {
            col = col.push(
                container(text(b.as_str()).size(12))
                    .padding(6)
                    .style(style::error_box)
                    .boxed(),
            );
        }
        container(col)
            .width(self.list_w())
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
            .boxed()
    }

    /// The session / profile list's right edge: a 1 px border that drags the list's width
    /// (like the side panel's left edge).
    pub(crate) fn list_handle(&self) -> El<'_> {
        let color = if self.list_dragging {
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
            .center_x(DIVIDER_W)
            .height(Fill)
            .style(style::bg(style::SIDEBAR)),
        )
        .on_press(Message::ListResizePressed)
        .interaction(mouse::Interaction::ResizingHorizontally)
        .boxed()
    }

    /// The session list's groups in display order: (key, divider label, sessions sorted by
    /// the sort criteria). As Hermes Desk: under 전체 one block per configuration (in the
    /// configurations page's order, empty ones left out); one configuration is a single block
    /// with key "" (no divider). Login terminals always form their own last block.
    pub(crate) fn sidebar_groups(&self) -> Vec<(String, String, Vec<&Live>)> {
        let mut mine: Vec<&Live> = self
            .sessions
            .iter()
            .filter(|l| self.in_filter(&l.info))
            .collect();
        crate::session_sort::sort_by(&mut mine, &self.ui.session_sort, |l| sort_item(l));
        // Desk: pinned sessions first in their block (a stable sort keeps the order otherwise).
        mine.sort_by_key(|l| !l.info.pinned);
        let mut groups: Vec<(String, String, Vec<&Live>)> = Vec::new();
        if self.ui.sessions_all {
            for c in &self.configs {
                groups.push((c.slug.clone(), c.name.clone(), Vec::new()));
            }
        }
        for l in mine {
            let (key, name) = if l.info.kind == ide_client::SessionKind::Login {
                (LOGIN_GROUP.to_owned(), tr::group_login())
            } else if self.ui.sessions_all {
                (l.info.configuration.clone(), l.info.configuration.clone())
            } else {
                (String::new(), String::new())
            };
            match groups.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, _, v)) => v.push(l),
                None => groups.push((key, name, vec![l])),
            }
        }
        groups.retain(|(_, _, v)| !v.is_empty());
        groups.sort_by_key(|(k, _, _)| k == LOGIN_GROUP);
        groups
    }

    /// Hermes Desk's `SessionSortControl`: a header icon (tinted while criteria are set) opening
    /// the criteria list: priority, direction, remove, add, back to the default order.
    fn sort_control(&self) -> El<'_> {
        let crit = &self.ui.session_sort;
        let on = !crit.is_empty();
        let arrow = |d: SortDir| if d == SortDir::Asc { "↑" } else { "↓" };
        let summary = if on {
            crit.iter()
                .map(|c| format!("{} {}", sort_key_label(c.key), arrow(c.dir)))
                .collect::<Vec<_>>()
                .join(" → ")
        } else {
            tr::session_sort_default()
        };
        let icon_color = if on {
            style::ACCENT_TEXT
        } else {
            style::DIM
        };
        let trigger = tip(
            button(icons::icon(Icon::ArrowsSort, 16.0, icon_color))
                .padding(3)
                .style(style::icon_toggle(on))
                .on_press(Message::ToggleSortMenu)
                .boxed(),
            tr::session_sort_tooltip(&summary),
        );
        let popup = self.sort_open.then(|| {
            let mut col: Column<El<'_>> = Column::new().spacing(8);
            col = col.push(
                row![
                    text(tr::session_sort()).size(13).font(style::BOLD),
                    space().width(Fill),
                    button(text(tr::session_sort_reset()).size(11))
                        .padding([2, 8])
                        .style(style::secondary)
                        .on_press_maybe(on.then_some(Message::Sort(SortChange::Reset))),
                ]
                .align_y(Alignment::Center)
                .boxed(),
            );
            if !on {
                col = col.push(
                    text(tr::session_sort_default_note())
                        .size(11)
                        .color(style::DIM)
                        .boxed(),
                );
            }
            let last = crit.len().saturating_sub(1);
            for (i, c) in crit.iter().enumerate() {
                let key = c.key;
                let dir = if c.dir == SortDir::Asc {
                    tr::sort_asc()
                } else {
                    tr::sort_desc()
                };
                col = col.push(
                    row![
                        icon_button(
                            "▲",
                            (i > 0).then_some(Message::Sort(SortChange::Move(key, -1))),
                            tr::sort_move_up()
                        ),
                        icon_button(
                            "▼",
                            (i < last).then_some(Message::Sort(SortChange::Move(key, 1))),
                            tr::sort_move_down()
                        ),
                        text(format!("{}. {}", i + 1, sort_key_label(key)))
                            .size(12)
                            .width(Fill),
                        button(text(format!("{} {}", arrow(c.dir), dir)).size(11))
                            .padding([2, 8])
                            .width(92)
                            .style(style::secondary)
                            .on_press(Message::Sort(SortChange::Flip(key))),
                        icon_button(
                            I_DELETE,
                            Some(Message::Sort(SortChange::Remove(key))),
                            tr::sort_remove()
                        ),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .boxed(),
                );
            }
            let available: Vec<SortKey> = SortKey::ALL
                .into_iter()
                .filter(|k| !crit.iter().any(|c| c.key == *k))
                .collect();
            if !available.is_empty() {
                let mut adds = row![].spacing(4);
                for k in available {
                    adds = adds.push(
                        button(text(format!("+ {}", sort_key_label(k))).size(11))
                            .padding([2, 8])
                            .style(style::secondary)
                            .on_press(Message::Sort(SortChange::Add(k))),
                    );
                }
                col = col.push(
                    column![
                        text(tr::session_sort_add()).size(11).color(style::DIM),
                        adds.wrap(),
                    ]
                    .spacing(4)
                    .boxed(),
                );
            }
            col = col.push(
                column![
                    container(space()).width(Fill).height(1).style(style::bg(style::BORDER)),
                    text(tr::session_sort_help())
                        .size(11)
                        .color(style::DIM)
                        .line_height(1.6),
                ]
                .spacing(10)
                .boxed(),
            );
            container(col)
                .padding(12)
                .width(330)
                .style(style::modal_card)
        });
        popover(trigger, popup)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(Message::CloseSortMenu)
            .boxed()
    }

    /// Connection status (settings page, server section): connected (target + server
    /// version), reconnecting, disconnected with a retry button.
    fn connection_status(&self) -> El<'_> {
        let (msg, retry) = match &self.conn {
            Conn::Connecting => (tr::conn_connecting(), false),
            Conn::Connected(w) => (tr::conn_connected(&w.server_version), false),
            Conn::Reconnecting => (tr::conn_reconnecting(), true),
            Conn::Disconnected(why) => (tr::conn_disconnected(why), true),
        };
        let mut r = row![
            text(I_DOT).size(10).color(self.conn_color()),
            column![
                text(tr::conn_target(&self.server_label()))
                    .size(11)
                    .font(style::BOLD)
                    .color(style::DIM)
                    .wrapping(text::Wrapping::WordOrGlyph),
                text(msg)
                    .size(11)
                    .color(style::DIM)
                    .wrapping(text::Wrapping::WordOrGlyph),
            ]
            .width(Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if retry && self.target.transport.is_ok() {
            r = r.push(
                button(text(tr::conn_retry()).size(12))
                    .padding([4, 10])
                    .style(style::secondary)
                    .on_press(Message::Retry)
                    .boxed(),
            );
        }
        container(r)
            .padding([6, 8])
            .width(Fill)
            .style(style::bg(style::HEADER))
            .boxed()
    }

    /// The server the app talks to: "Local" or "ssh <host>".
    pub fn server_label(&self) -> String {
        if self.target.local {
            tr::conn_local()
        } else {
            self.target.remote.clone()
        }
    }

    fn session_row<'a>(&'a self, live: &'a Live, depth: usize) -> El<'a> {
        let s = &live.info;
        let selected = self.selected.as_deref() == Some(s.id.as_str());
        let (glyph, gcolor) = provider_glyph(self, &s.provider);

        // The terminal title, unless it only repeats the session title (Claude Code gets the
        // title as its session name and shows it back, after a status mark such as ✳ or a
        // spinner).
        let repeats = |t: &str| {
            let t = t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
            t == s.title.trim()
        };
        let subtitle = if s.term_title.is_empty() {
            i18n::status_label(live.state)
        } else if repeats(&s.term_title) {
            String::new()
        } else {
            s.term_title.clone()
        };
        let title = text(s.title.as_str())
            .color(if s.archived { style::DIM } else { style::TEXT })
            .size(13)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End)
            .width(Fill);
        // Desk's phase (답변 중 / 응답 필요 / 새 답변) leads the second line.
        let phase = match s.activity {
            ide_client::AgentActivity::Working => Some((tr::activity_working(), style::ACCENT_TEXT)),
            ide_client::AgentActivity::NeedsInput => {
                Some((tr::activity_needs_input(), style::AMBER))
            }
            _ if self.new_answers.contains(&s.id) => Some((tr::activity_new_answer(), style::GREEN)),
            _ => None,
        };
        let subtitle = match phase {
            Some(_) if !subtitle.is_empty() => format!("· {subtitle}"),
            _ => subtitle,
        };
        // Desk's order: the branch badge (변경 없음, 커밋 있음, ...) first, never wrapped; the
        // phase and the text take what is left (cut with …).
        let mut sub = Row::<El<'a>>::new().spacing(4).align_y(Alignment::Center);
        if let Some(b) = self.branch_badge(&s.id, !s.repos.is_empty()) {
            sub = sub.push(b);
        }
        if let Some((label, color)) = phase {
            sub = sub.push(
                text(label)
                    .size(11)
                    .color(color)
                    .wrapping(text::Wrapping::None)
                    .boxed(),
            );
        }
        let sub = sub.push(
            text(subtitle)
                .size(11)
                .color(style::DIM)
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::End)
                .width(Fill)
                .boxed(),
        );
        // Desk: a pinned row shows a filled pin before the rest.
        let mut content = row![].spacing(6).align_y(Alignment::Center);
        if s.pinned {
            content = content.push(icons::icon(Icon::PinFilled, 12.0, style::ACCENT_TEXT));
        }
        let mut content = content
            .push(text(glyph).size(14).color(gcolor).width(18).boxed())
            .push(column![title, sub].width(Fill).boxed());
        // Add child / pin / delete take room inside the row (pushing the title) only while
        // hovered.
        if self.session_hover.as_deref() == Some(s.id.as_str()) {
            content = content.push(self.session_actions(live));
        }
        // Desk: a parent's fold toggle with its child count, always at the row's right end.
        let kids = self.family_children(&s.id);
        if kids > 0 {
            let folded = self.ui.folded_families.contains(&s.id);
            content = content.push(tip(
                button(
                    row![
                        icons::icon(
                            if folded { Icon::ChevronRight } else { Icon::ChevronDown },
                            11.0,
                            style::DIM
                        ),
                        text(kids.to_string()).size(11).color(style::DIM),
                    ]
                    .spacing(2)
                    .align_y(Alignment::Center),
                )
                .padding([2, 4])
                .style(style::icon)
                .on_press(Message::Family(crate::family_ui::FamilyMsg::ToggleFold(s.id.clone())))
                .boxed(),
                if folded { tr::family_unfold() } else { tr::family_fold() },
            ));
        }
        // Children are indented per level.
        let content = row![space().width((depth * 14) as f32), content].align_y(Alignment::Center);
        let main = button(content)
            .padding([4, 6])
            .width(Fill)
            .style(style::row(selected))
            .on_press(Message::Select(s.id.clone()));
        let main = mouse_area(main)
            .on_enter(Message::SessionHover(s.id.clone(), true))
            .on_exit(Message::SessionHover(s.id.clone(), false))
            .boxed();
        crate::session_menu::with_menu(main, &s.id)
    }

    /// The hovered session row's pin / delete buttons.
    fn session_actions<'a>(&'a self, live: &'a Live) -> El<'a> {
        let s = &live.info;
        // An archived session cannot be pinned (Desk's `canPin`).
        let pin: El<'a> = if s.archived {
            space().width(0).boxed()
        } else {
            let label = if s.pinned { tr::session_unpin() } else { tr::session_pin() };
            tip(
                button(icons::icon(
                    if s.pinned { Icon::PinFilled } else { Icon::Pin },
                    14.0,
                    if s.pinned { style::ACCENT_TEXT } else { style::DIM },
                ))
                .padding([3, 5])
                .style(style::icon)
                .on_press_maybe(
                    self.when_connected(Message::SetSessionFlags(s.id.clone(), Some(!s.pinned), None)),
                )
                .boxed(),
                label,
            )
        };
        let add: El<'a> = if self.can_have_child(&s.id) {
            tip(
                button(icons::icon(Icon::Plus, 14.0, style::DIM))
                    .padding([3, 5])
                    .style(style::icon)
                    .on_press_maybe(self.when_connected(Message::Family(
                        crate::family_ui::FamilyMsg::Open(s.id.clone()),
                    )))
                    .boxed(),
                tr::child_create_menu(),
            )
        } else {
            space().width(0).boxed()
        };
        // Renaming is in 세션 설정; run / stop is in the terminal's footer.
        row![
            add,
            pin,
            icon_button(
                I_DELETE,
                self.when_connected(Message::AskDelete(s.id.clone())),
                tr::delete()
            ),
        ]
        .spacing(1)
        .align_y(Alignment::Center)
        .boxed()
    }

    // ------------------------------------------------------------------ centre

    fn centre(&self) -> El<'_> {
        let portion = (self.ui.split_ratio * 1000.0).round() as u16;
        // The terminal takes the detail pane's room while it is hidden or in its own window.
        let width = if !self.detail_in_app() {
            Fill
        } else {
            FillPortion(portion.max(1))
        };
        let Some(live) = self.selected_live() else {
            return container(
                column![
                    text(tr::no_session_selected()).size(18).color(style::DIM),
                    button(text(tr::new_session()).size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press_maybe(self.when_connected(Message::OpenNew)),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Fill)
            .width(width)
            .style(style::bg(style::BG))
            .boxed();
        };
        let s = &live.info;

        let (glyph, gcolor) = provider_glyph(self, &s.provider);
        let header = container(
            row![
                text(glyph).size(13).color(gcolor),
                text(s.title.as_str()).size(13).font(style::BOLD),
                text(
                    s.repos
                        .first()
                        .map(|r| r.branch.clone())
                        .unwrap_or_default()
                )
                .size(12)
                .color(style::DIM)
                .font(style::MONO),
                text(s.root.as_str())
                    .size(11)
                    .color(style::FAINT)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start)
                    .width(Fill),
                if self.composer_shown_kind(live) {
                    self.composer_toggle()
                } else {
                    space().width(0).boxed()
                },
                self.detail_toggle(live),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .width(Fill)
        .style(style::bg(style::HEADER));

        // Show the terminal whenever there is (or was) output: stopped sessions keep their
        // scrollback on the server.
        let has_term = live.attached || live.state.is_alive() || s.term_end > 0;
        let body: El<'_> = if has_term {
            let id = s.id.clone();
            let mut view = TerminalView::new(&live.term, &self.term_config)
                .id(TERM_ID)
                .on_event(Message::TermView);
            if self.connected() {
                let (a, b) = (id.clone(), id.clone());
                view = view
                    .on_input(move |bytes| Message::TermInput(a.clone(), bytes))
                    .on_resize(move |c, r| Message::TermResize(b.clone(), c, r));
            }
            let mut col = Column::new();
            if !self.connected() {
                col = col.push(
                    container(text(tr::conn_read_only()).size(12).color(style::AMBER))
                        .padding([4, 10])
                        .width(Fill)
                        .style(style::bg(style::HEADER))
                        .boxed(),
                );
            }
            // Like Hermes Desk's chat column: the terminal is at most as wide as the composer
            // box and centred, so wide panes keep side margins (in the terminal's own ground)
            // and lines stay readable.
            let term_bg = self.term_config.palette.background;
            let view = container(view)
                .width(Fill.max(crate::composer::MAX_W))
                .height(Fill);
            // A file dragged in the app may drop on the terminal too (`composer.rs`).
            col = col.push(
                mouse_area(
                    container(view)
                        .center_x(Fill)
                        .height(Fill)
                        .style(move |_| container::Style {
                            background: Some(term_bg.into()),
                            ..container::Style::default()
                        }),
                )
                .on_enter(Message::DropHover(crate::composer::Dest::Terminal, true))
                .on_exit(Message::DropHover(crate::composer::Dest::Terminal, false))
                .boxed(),
            );
            if self.composer_shown(live) && live.state.is_alive() {
                col = col.push(self.composer_view(live));
            }
            let exited = match live.state {
                RunState::Exited(code) => Some(match code {
                    Some(c) => tr::process_exited_code(&c.to_string()),
                    None => tr::process_exited(),
                }),
                RunState::Stopped => Some(tr::session_stopped()),
                RunState::Failed => Some(tr::session_start_failed()),
                _ => None,
            };
            if let Some(msg) = exited {
                let mut bar = column![
                    row![
                        text(msg).size(12).color(style::DIM),
                        space().width(Fill),
                        button(text(format!("{I_RESTART}  {}", tr::restart())).size(12))
                            .padding([3, 10])
                            .style(style::secondary)
                            .on_press_maybe(self.when_connected(Message::Start(id))),
                    ]
                    .align_y(Alignment::Center)
                ]
                .spacing(4);
                if let Some(e) = &live.error {
                    bar = bar.push(error_box(e));
                }
                col = col.push(
                    container(bar)
                        .padding([4, 10])
                        .style(style::bg(style::HEADER))
                        .boxed(),
                );
            }
            col.boxed()
        } else {
            let mut c = column![
                text(match live.state {
                    RunState::Failed => tr::session_start_failed(),
                    _ => tr::session_stopped(),
                })
                .size(16)
                .color(style::DIM),
                button(
                    text(format!(
                        "{I_PLAY}  {}",
                        tr::start_agent(&self.provider_name(&s.provider))
                    ))
                    .size(13),
                )
                .padding([6, 14])
                .style(style::primary)
                .on_press_maybe(self.when_connected(Message::Start(s.id.clone()))),
            ]
            .spacing(12)
            .align_x(Alignment::Center)
            .width(520);
            if let Some(e) = &live.error {
                c = c.push(error_box(e));
            }
            container(c).center(Fill).boxed()
        };

        container(column![header, body])
            .width(width)
            .height(Fill)
            .style(style::bg(style::BG))
            .boxed()
    }

    /// Desk's `DetailToggle` at the right end of the session header: shows / hides the detail
    /// area (its window while detached), with the count of open items.
    fn detail_toggle<'a>(&'a self, live: &'a Live) -> El<'a> {
        let on = !self.ui.detail_collapsed;
        let label = match (self.ui.detail_detached, on) {
            (false, true) => tr::hide_detail(),
            (false, false) => tr::show_detail(),
            (true, true) => tr::detail_window_hide(),
            (true, false) => tr::detail_window_show(),
        };
        let count = live.detail.items.len();
        let mut content = row![text(tr::detail_toggle()).size(12)]
            .spacing(6)
            .align_y(Alignment::Center);
        if count > 0 {
            content = content.push(
                container(text(count.min(99).to_string()).size(10).font(style::BOLD))
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
                    .boxed(),
            );
        }
        let b = button(content)
            .padding([2, 8])
            .style(style::choice(on))
            .on_press(Message::ToggleDetail);
        tip(b.boxed(), label)
    }

    fn divider(&self) -> El<'_> {
        // Hidden or detached detail area: no divider (the side panel's edge is the border).
        if !self.detail_in_app() {
            return space().width(0).boxed();
        }
        let color = if self.dragging {
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
            .center_x(DIVIDER_W)
            .height(Fill),
        )
        .on_press(Message::DividerPressed)
        .interaction(mouse::Interaction::ResizingHorizontally)
        .boxed()
    }

    /// The new-session dialog's "워크스페이스" part (Desk's `NewSessionModal`): "use worktrees"
    /// with a checkbox per registered repo (pre-checked: the configuration's default repos;
    /// none = an empty workspace), or one repo to work in directly.
    fn new_workspace_section<'a>(&'a self, f: &'a NewForm) -> El<'a> {
        let mut c = column![
            label(tr::workspace_label()),
            checkbox(f.use_worktrees)
                .label(tr::use_worktrees())
                .on_toggle(Message::NewUseWorktrees)
                .size(16)
                .text_size(13),
            text(tr::use_worktrees_note()).size(11).color(style::FAINT),
        ]
        .spacing(6);
        if f.use_worktrees {
            c = c.push(text(tr::worktree_repos_label()).size(12).font(style::BOLD).boxed());
            let mut list: Column<El<'_>> = Column::new().spacing(3);
            for r in &self.repos {
                let id = r.id.clone();
                let checked = f.repos.contains(&r.id);
                let mut line = row![
                    checkbox(checked)
                        .label(format!("{}  ·  {}", r.name, r.path))
                        .on_toggle(move |v| Message::NewRepoChecked(id.clone(), v))
                        .size(15)
                        .text_size(12)
                        .width(Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center);
                if checked {
                    // Desk's RepoBaseTable: the repo default, any of origin's branches.
                    let id = r.id.clone();
                    line = line.push(self.base_picker(
                        &r.id,
                        f.bases.get(&r.id).cloned(),
                        r.base.clone(),
                        move |b| Message::NewBase(id.clone(), b),
                    ));
                }
                list = list.push(line.boxed());
            }
            c = c.push(list.boxed());
            c = c.push(
                text(if self.repos.is_empty() {
                    tr::no_repos_empty_workspace()
                } else if f.repos.is_empty() {
                    tr::empty_workspace_note()
                } else {
                    tr::new_base_note()
                })
                .size(11)
                .color(style::DIM)
                .boxed(),
            );
            c = c.push(
                column![
                    checkbox(f.refresh_base)
                        .label(tr::new_refresh_base())
                        .on_toggle_maybe((!f.busy && !f.repos.is_empty()).then_some(Message::NewRefreshBase))
                        .size(15)
                        .text_size(12),
                    text(tr::new_refresh_base_note()).size(11).color(style::FAINT),
                ]
                .spacing(3)
                .boxed(),
            );
            c = c.push(text(tr::worktree_repos_note()).size(11).color(style::FAINT).boxed());
        } else {
            c = c.push(
                container(text(tr::original_repo_warning()).size(11))
                    .padding(8)
                    .width(Fill)
                    .style(style::warn_box)
                    .boxed(),
            );
            c = c.push(text(tr::original_repo_label()).size(12).font(style::BOLD).boxed());
            let choices: Vec<Choice> = self
                .repos
                .iter()
                .map(|r| Choice {
                    key: r.id.clone(),
                    label: format!("{} · {}", r.name, r.path),
                })
                .collect();
            if choices.is_empty() {
                c = c.push(
                    text(tr::no_repos_register_first())
                        .size(12)
                        .color(style::DIM)
                        .boxed(),
                );
            } else {
                let sel = choices
                    .iter()
                    .find(|x| f.original_repo.as_deref() == Some(x.key.as_str()))
                    .cloned();
                c = c.push(
                    pick_list(sel, choices, |c: &Choice| c.label.clone())
                        .on_select(|c: Choice| Message::NewOriginalRepo(c.key))
                        .text_size(13)
                        .padding([5, 8])
                        .width(Fill)
                        .boxed(),
                );
            }
        }
        container(c)
            .padding(10)
            .width(Fill)
            .style(style::card)
            .boxed()
    }

    /// Desk's "세션 설정" repo table: the session's worktrees (each can be detached: an ide
    /// addition) and registered repos to add, attached on "save".
    fn session_repos_card<'a>(&'a self, f: &'a SessionReposForm) -> El<'a> {
        let Some(live) = self.live(&f.session) else {
            return space().boxed();
        };
        let s = &live.info;
        let mut c = column![
            text(tr::session_settings()).size(18).font(style::BOLD),
            column![
                text(tr::session_name_label()).size(12).font(style::BOLD),
                text_input(tr::session_title_placeholder(), &f.title)
                    .on_input(Message::SessionReposTitle)
                    .on_submit_maybe((!f.busy && self.connected()).then_some(Message::SessionReposSave))
                    .size(13)
                    .padding([5, 8])
                    .style(style::input),
            ]
            .spacing(4),
            row![
                text(tr::session_repos_label()).size(12).font(style::BOLD),
                space().width(Fill),
                text(s.root.as_str())
                    .size(11)
                    .font(style::MONO)
                    .color(style::FAINT)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start),
            ]
            .spacing(8)
            .boxed(),
        ]
        .spacing(10);
        let workspace = s.mode == ide_client::SessionMode::Workspace;
        let mut list: Column<El<'_>> = Column::new().spacing(3);
        for r in &s.repos {
            let mut row_el = row![
                icons::icon(Icon::GitBranch, 14.0, style::DIM),
                text(r.name.as_str()).size(13).width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if workspace {
                // Desk's "레포 · 기준 브랜치": the session's own base per repo.
                let id = r.repo.clone();
                row_el = row_el.push(
                    text(format!("{} ←", r.branch))
                        .size(11)
                        .font(style::MONO)
                        .color(style::DIM).boxed(),
                );
                row_el = row_el.push(self.base_picker(
                    &r.repo,
                    f.bases.get(&r.repo).cloned(),
                    r.base.clone(),
                    move |b| Message::SessionRepoBase(id.clone(), b),
                ));
            } else {
                row_el = row_el.push(
                    text(format!("{} ← {}", r.branch, r.base))
                        .size(11)
                        .font(style::MONO)
                        .color(style::DIM).boxed(),
                );
            }
            if workspace {
                row_el = row_el.push(icon_button(
                    I_DELETE,
                    (!f.busy && self.connected())
                        .then(|| Message::SessionRepoDetach(r.repo.clone())),
                    tr::session_repo_detach(),
                ).boxed());
            }
            list = list.push(
                container(row_el)
                    .padding([3, 8])
                    .style(style::bg(style::HEADER))
                    .boxed(),
            );
        }
        if s.repos.is_empty() {
            list = list.push(text(tr::no_repos()).size(12).color(style::DIM).boxed());
        }
        c = c.push(list.boxed());
        if workspace {
            let addable: Vec<&ide_client::RepoInfo> = self
                .repos
                .iter()
                .filter(|r| !s.repos.iter().any(|x| x.repo == r.id))
                .collect();
            if !addable.is_empty() {
                c = c.push(
                    text(tr::session_repos_add_label())
                        .size(12)
                        .font(style::BOLD)
                        .boxed(),
                );
                for r in addable {
                    let id = r.id.clone();
                    let on = f.add.contains(&r.id);
                    let mut line = row![
                        checkbox(on)
                            .label(format!("{}  ·  {}", r.name, r.path))
                            .on_toggle(move |v| Message::SessionRepoAdd(id.clone(), v))
                            .size(15)
                            .text_size(12)
                            .width(Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center);
                    if on {
                        let id = r.id.clone();
                        line = line.push(self.base_picker(
                            &r.id,
                            f.bases.get(&r.id).cloned(),
                            r.base.clone(),
                            move |b| Message::SessionRepoBase(id.clone(), b),
                        ));
                    }
                    c = c.push(line.boxed());
                }
            } else if self.repos.is_empty() {
                c = c.push(
                    text(tr::no_repos_register_first())
                        .size(12)
                        .color(style::DIM)
                        .boxed(),
                );
            }
            c = c.push(text(tr::session_repos_base_note()).size(11).color(style::FAINT).boxed());
            c = c.push(text(tr::session_repos_note()).size(11).color(style::FAINT).boxed());
        } else {
            c = c.push(
                text(tr::session_repos_no_workspace())
                    .size(12)
                    .color(style::DIM)
                    .boxed(),
            );
        }
        if let Some(e) = &f.error {
            c = c.push(error_box(e));
        }
        c = c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                button(text(tr::save()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe(
                        (!f.busy && self.connected()).then_some(Message::SessionReposSave)
                    ),
            ]
            .spacing(8)
            .boxed(),
        );
        c.width(620).boxed()
    }

    /// A base-branch pick list (Desk's `RepoBaseTable` select): `picked` (else `fallback`,
    /// the default) plus origin's branches as of the last fetch.
    fn base_picker<'a>(
        &'a self,
        repo: &str,
        picked: Option<String>,
        fallback: String,
        on: impl Fn(String) -> Message + 'a,
    ) -> El<'a> {
        let mut choices: Vec<String> = Vec::new();
        for b in picked
            .iter()
            .chain(std::iter::once(&fallback))
            .chain(self.origin_branches.get(repo).into_iter().flatten())
        {
            if !b.is_empty() && !choices.contains(b) {
                choices.push(b.clone());
            }
        }
        let sel = picked.or((!fallback.is_empty()).then_some(fallback));
        let placeholder = if choices.is_empty() {
            tr::base_no_origin_branches()
        } else {
            tr::base_pick_label()
        };
        tip(
            pick_list(sel, choices, |b: &String| b.clone())
                .on_select(on)
                .placeholder(placeholder)
                .text_size(12)
                .padding([3, 6])
                .width(200)
                .boxed(),
            tr::base_pick_label(),
        )
    }

    /// The repos page's folder picker (`browse.pick_list`: any folder the server's OS user
    /// can read; server-side, so it also works over SSH).
    fn pick_root_card<'a>(&'a self, f: &'a PickRootForm) -> El<'a> {
        let current = f.listing.as_ref();
        // Up: the parent folder, or the start list from a drive / `/`.
        let up = current
            .filter(|l| !l.path.is_empty())
            .map(|l| Message::PickGo(l.parent.clone()));
        let mut path_row = row![
            button(text(tr::pick_start()).size(12))
                .padding([2, 6])
                .style(style::row(current.is_some_and(|l| l.path.is_empty())))
                .on_press(Message::PickGo(None)),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        if let Some(l) = current {
            let parts = crate::app::crumbs(&l.path);
            let last = parts.len().saturating_sub(1);
            for (i, (label, path)) in parts.into_iter().enumerate() {
                path_row = path_row.push(text("›").size(12).color(style::FAINT).boxed());
                path_row = path_row.push(
                    button(text(label).size(12).wrapping(text::Wrapping::None))
                        .padding([2, 6])
                        .style(style::row(i == last))
                        .on_press(Message::PickGo(Some(path)))
                        .boxed(),
                );
            }
        }
        let head = row![
            icon_button("↑", up, tr::browse_up()),
            scrollable(path_row)
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(3).scroller_width(3)
                ))
                .width(Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let mut list: Column<El<'a>> = Column::new().spacing(1);
        if let Some(l) = current {
            if l.entries.is_empty() {
                list = list.push(
                    text(tr::browse_empty())
                        .size(12)
                        .color(style::FAINT)
                        .boxed(),
                );
            }
            for (i, e) in l.entries.iter().enumerate() {
                let selected = f.selected == Some(i);
                let r = container(folder_entry_row(e, None))
                    .padding([3, 6])
                    .width(Fill)
                    .style(move |_| container::Style {
                        background: selected.then(|| style::SELECTED.into()),
                        border: iced::Border {
                            radius: 3.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                list = list.push(
                    mouse_area(r)
                        .on_press(Message::PickHighlight(i))
                        .on_double_click(Message::PickGo(Some(e.path.clone())))
                        .interaction(mouse::Interaction::Pointer)
                        .boxed(),
                );
            }
        } else if f.loading {
            list = list.push(text(tr::tree_loading()).size(12).color(style::DIM).boxed());
        }
        let list = list.padding(iced::Padding {
            right: 12.0,
            ..iced::Padding::ZERO
        });
        let mut body = column![head, scrollable(list).id(PICK_LIST_ID).height(320)].spacing(6);
        if current.is_some_and(|l| l.truncated) {
            body = body.push(
                text(tr::pick_truncated())
                    .size(11)
                    .color(style::AMBER)
                    .boxed(),
            );
        }
        let target = f.target();
        let mut c = column![
            text(if f.purpose == crate::app::PickPurpose::Terminal {
                tr::pick_term_title()
            } else if matches!(f.purpose, crate::app::PickPurpose::Skill(_)) {
                tr::pick_skill_title()
            } else {
                tr::pick_root_title()
            })
            .size(18)
            .font(style::BOLD),
            text(tr::pick_root_note()).size(12).color(style::DIM),
            text_input(tr::pick_path_placeholder(), &f.path)
                .id(PICK_PATH_ID)
                .on_input(Message::PickPath)
                .on_submit(Message::PickPathGo)
                .size(12)
                .font(style::MONO)
                .padding([5, 8])
                .style(style::input)
                .width(Fill),
            browser_frame(body.boxed()),
        ]
        .spacing(10);
        if let Some(e) = &f.error {
            c = c.push(error_box(e));
        }
        c = c.push(
            row![
                text(target.unwrap_or_default())
                    .size(11)
                    .font(style::MONO)
                    .color(style::DIM)
                    .width(Fill)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::CloseModal),
                button(text(tr::pick_select()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe(
                        target
                            .is_some()
                            .then_some(Message::PickConfirm)
                            .filter(|_| self.connected() && !self.settings.repos_busy)
                    ),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed(),
        );
        c.width(600).boxed()
    }

    /// Ctrl+P: files of the selected session by name.
    fn quick_open_card<'a>(&'a self, f: &'a QuickForm) -> El<'a> {
        let mut list: Column<El<'a>> = Column::new().spacing(1);
        let multi = self.selected_live().is_some_and(|l| l.info.repos.len() > 1);
        for (i, m) in f.results.iter().enumerate() {
            let name = crate::detail::file_name(&m.path);
            let folder = crate::detail::parent(&m.path);
            // Several repos: the folder starts with the repo name.
            let folder = if multi {
                let r = self.repo_name(&m.repo);
                if folder.is_empty() { r } else { format!("{r}/{folder}") }
            } else {
                folder.to_owned()
            };
            list = list.push(
                button(
                    row![
                        ide_views::icons::file_icon(name),
                        text(name).size(13).wrapping(text::Wrapping::None),
                        text(folder)
                            .size(11)
                            .color(style::DIM)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::Start)
                            .width(Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding([4, 8])
                .width(Fill)
                .style(style::row(i == f.selected))
                .on_press(Message::QuickPick(crate::detail::qualify(&m.repo, &m.path)))
                .boxed(),
            );
        }
        if f.results.is_empty() && !f.query.is_empty() {
            list = list.push(
                container(text(tr::quick_open_none()).size(12).color(style::DIM))
                    .padding(8)
                    .boxed(),
            );
        }
        let mut c = column![
            text_input(tr::quick_open_placeholder(), &f.query)
                .id(crate::detail::QUICK_ID)
                .on_input(Message::QuickInput)
                .on_submit(Message::QuickSubmit)
                .size(14)
                .padding([7, 10])
                .style(style::input),
            scrollable(list).height(Length::Shrink),
        ]
        .spacing(8);
        if let Some(e) = &f.error {
            c = c.push(error_box(e));
        }
        container(c).width(560).boxed()
    }

    /// A modal covers the window: the web view shows a dimmed, frozen snapshot (see
    /// [`App::detail`]).
    pub fn web_covered(&self) -> bool {
        // In the detail window only its own dialogs cover it.
        if self.ui.detail_detached {
            return self.modal_in_detail() && !self.modal_web_unsafe;
        }
        (self.modal.is_some() && !self.modal_web_unsafe) || !self.on_work_page()
    }

    /// The open dialog came from the detail pane's content (unsaved-file close, definition
    /// picker) and the pane is in its own window: the dialog is drawn there.
    pub fn modal_in_detail(&self) -> bool {
        self.ui.detail_detached
            && self.detail_window.is_some()
            && matches!(self.modal, Some(Modal::CloseDirty(_) | Modal::DefPicker(_)))
    }

    /// The open dialog over the window (`detail_window`: the detail window's layer), or
    /// `space()` when it belongs to the other window.
    pub(crate) fn modal_layer(&self, detail_window: bool) -> El<'_> {
        match &self.modal {
            Some(m) if self.modal_in_detail() == detail_window => self.modal_view(m),
            _ => space().boxed(),
        }
    }

    // ------------------------------------------------------------------ pages

    /// The shown page other than the work page: a centred column (like Hermes Desk's
    /// `PageLayout`: 960 px wide with 32 px margins, a 40 px title row, 20 px between sections).
    fn page_view(&self) -> El<'_> {
        let f = &self.settings;
        let online = self.connected();
        let (title, sections): (String, Vec<El<'_>>) = match self.ui.page {
            Page::Work => (String::new(), Vec::new()),
            Page::Accounts => (
                tr::nav_accounts(),
                vec![card(self.accounts_section(f, online))],
            ),
            // Its own layout (list beside the rail + sections): `profiles_view.rs`.
            Page::Configurations => return self.profiles_page(),
            // Its own layout (list beside the rail + the picked repository), like profiles.
            Page::Repos => return self.repos_page(),
            Page::Settings => (
                tr::settings(),
                vec![
                    card(self.language_section()),
                    card(self.server_section(f, online)),
                    card(self.on_close_section()),
                ],
            ),
        };
        let mut c = column![
            container(text(title).size(22).font(style::BOLD))
                .height(40)
                .align_y(Alignment::Center),
        ]
        .spacing(20);
        let error = match self.ui.page {
            Page::Accounts | Page::Configurations => f.error.as_deref(),
            _ => None,
        };
        if let Some(e) = error {
            c = c.push(error_box(e));
        }
        for s in sections {
            c = c.push(s);
        }
        let avail = self.window_size.width - RAIL_W;
        let width = (avail - 64.0).clamp(320.0, 896.0);
        container(scrollable(
            container(c.width(width)).center_x(Fill).padding([32, 0]),
        ))
        .width(Fill)
        .height(Fill)
        .style(style::bg(style::BG))
        .boxed()
    }

    /// Settings page: UI language.
    fn language_section(&self) -> El<'_> {
        let languages = LanguagePref::all()
            .into_iter()
            .fold(row![].spacing(6), |r, p| {
                let name = match p {
                    LanguagePref::System => tr::language_system(),
                    LanguagePref::Fixed(l) => l.native_name().to_owned(),
                };
                r.push(
                    button(text(name).size(13))
                        .padding([5, 12])
                        .style(style::choice(self.ui.language == p))
                        .on_press(Message::SetLanguage(p))
                        .boxed(),
                )
            });
        column![label(tr::language_label()), languages]
            .spacing(10)
            .boxed()
    }

    /// Settings page: connection status, local / SSH server, stop server.
    fn server_section<'a>(&'a self, f: &'a SettingsForm, online: bool) -> El<'a> {
        let mut c = column![label(tr::server_label()), self.connection_status()].spacing(10);
        let kinds = row![
            button(text(tr::conn_local()).size(13))
                .padding([5, 12])
                .style(style::choice(!f.ssh))
                .on_press(Message::SettingsServerSsh(false)),
            button(text(tr::server_ssh()).size(13))
                .padding([5, 12])
                .style(style::choice(f.ssh))
                .on_press(Message::SettingsServerSsh(true)),
            space().width(Fill),
            button(text(tr::server_apply()).size(13))
                .padding([5, 12])
                .style(style::secondary)
                .on_press(Message::SettingsApplyServer),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        c = c.push(kinds.boxed());
        if f.ssh {
            c = c.push(
                text_input(tr::ssh_host_placeholder(), &f.ssh_host)
                    .id(SSH_HOST_ID)
                    .on_input(Message::SettingsSshHost)
                    .on_submit(Message::SettingsApplyServer)
                    .size(13)
                    .padding([5, 8])
                    .style(style::input)
                    .boxed(),
            );
            c = c.push(
                text_input(tr::ssh_command_placeholder(), &f.ssh_command)
                    .on_input(Message::SettingsSshCommand)
                    .on_submit(Message::SettingsApplyServer)
                    .size(13)
                    .padding([5, 8])
                    .style(style::input)
                    .boxed(),
            );
        }
        if let Some(v) = &self.remote_override {
            c = c.push(
                text(tr::server_env_override(v))
                    .size(12)
                    .color(style::AMBER)
                    .boxed(),
            );
        }
        if let Some(e) = &f.server_error {
            c = c.push(error_box(e));
        }
        c = c.push(
            row![
                text(tr::server_note())
                    .size(12)
                    .color(style::DIM)
                    .width(Fill),
                button(text(tr::stop_server()).size(13))
                    .padding([5, 12])
                    .style(style::danger)
                    .on_press_maybe(online.then_some(Message::AskStopServer)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed(),
        );
        c.boxed()
    }

    /// Settings page: what closing the window does.
    fn on_close_section(&self) -> El<'_> {
        let on_close = [
            (CloseAction::Ask, tr::on_close_ask()),
            (CloseAction::Keep, tr::on_close_keep()),
            (CloseAction::Stop, tr::on_close_stop()),
        ]
        .into_iter()
        .fold(row![].spacing(6), |r, (a, name)| {
            r.push(
                button(text(name).size(13))
                    .padding([5, 12])
                    .style(style::choice(self.ui.on_close == a))
                    .on_press(Message::SetOnClose(a))
                    .boxed(),
            )
        });
        column![label(tr::on_close_label()), on_close]
            .spacing(10)
            .boxed()
    }

    /// Repos page (Hermes Desk's `RepoList` + `ReposPage`): like the profiles page, the
    /// repository list sits beside the rail (the page's rail button shows / hides it) and the
    /// page shows the picked repository.
    fn repos_page(&self) -> El<'_> {
        let page = self.repo_view();
        if !self.ui.sessions_open {
            return page;
        }
        row![self.repo_list(), self.list_handle(), page]
            .height(Fill)
            .boxed()
    }

    /// Desk's `RepoList`: title and hide button, "레포 추가" (folder picker), a path input,
    /// one row per repository.
    fn repo_list(&self) -> El<'_> {
        let f = &self.settings;
        let online = self.connected();
        let head = row![
            text(tr::nav_repos()).size(13).font(style::BOLD),
            space().width(Fill),
            tip(
                button(icons::icon(Icon::SidebarCollapse, 16.0, style::DIM))
                    .padding(4)
                    .style(style::icon)
                    .on_press(Message::Page(Page::Repos))
                    .boxed(),
                tr::repos_hide(),
            ),
        ]
        .align_y(Alignment::Center);
        let add = button(
            row![
                icons::icon(Icon::FolderOpen, 14.0, style::ON_ACCENT),
                text(tr::repo_add()).size(13),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding([6, 10])
        .style(style::primary)
        .on_press_maybe((online && !f.repos_busy).then_some(Message::PickOpen));
        let mut list: Column<El<'_>> = Column::new().spacing(2);
        for r in &self.repos {
            let sel = f.repo_selected.as_deref() == Some(r.id.as_str());
            list = list.push(
                button(
                    row![
                        icons::icon(
                            Icon::GitBranch,
                            16.0,
                            if sel { style::ACCENT_TEXT } else { style::DIM },
                        ),
                        column![
                            text(r.name.as_str())
                                .size(13)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End),
                            text(r.path.as_str())
                                .size(11)
                                .color(style::DIM)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End),
                        ]
                        .spacing(1)
                        .width(Fill),
                        text(r.sessions.to_string()).size(10).color(style::FAINT),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding([6, 8])
                .style(style::row(sel))
                .on_press(Message::RepoSelect(r.id.clone()))
                .boxed(),
            );
        }
        if self.repos.is_empty() {
            list = list.push(text(tr::repos_none()).size(12).color(style::DIM).boxed());
        }
        container(
            column![head, add, scrollable(list).height(Fill)]
                .spacing(10)
                .padding(10),
        )
        .width(self.list_w())
        .height(Fill)
        .style(style::bg(style::SIDEBAR))
        .boxed()
    }

    /// The picked repository: name, path and "unregister (files kept)" with a confirm step in
    /// the title row, then the base branch and the sessions using it, one card each.
    fn repo_view(&self) -> El<'_> {
        let f = &self.settings;
        let online = self.connected();
        let selected = f
            .repo_selected
            .as_deref()
            .and_then(|id| self.repos.iter().find(|r| r.id == id));
        let mut c = Column::<El<'_>>::new().spacing(20);
        match selected {
            None => {
                c = c.push(
                    container(text(tr::repos_title()).size(22).font(style::BOLD))
                        .height(40)
                        .align_y(Alignment::Center)
                        .boxed(),
                );
                c = c.push(text(tr::repos_note()).size(12).color(style::DIM).boxed());
                if let Some(e) = &f.repos_error {
                    c = c.push(error_box(e));
                }
                c = c.push(text(tr::repos_pick_note()).size(13).color(style::DIM).boxed());
            }
            Some(r) => {
                let users: Vec<&Live> = self
                    .sessions
                    .iter()
                    .filter(|l| l.info.repos.iter().any(|x| x.repo == r.id))
                    .collect();
                let mut sessions: Column<El<'_>> = Column::new().spacing(2);
                for l in &users {
                    let branch = l
                        .info
                        .repos
                        .iter()
                        .find(|x| x.repo == r.id)
                        .map(|x| x.branch.clone())
                        .unwrap_or_default();
                    sessions = sessions.push(
                        row![
                            text(l.info.title.as_str()).size(12).width(Fill),
                            text(branch).size(11).font(style::MONO).color(style::DIM),
                        ]
                        .spacing(8)
                        .boxed(),
                    );
                }
                if users.is_empty() {
                    sessions = sessions
                        .push(text(tr::repo_no_sessions()).size(12).color(style::DIM).boxed());
                }
                let actions: El<'_> = if f.repo_confirm {
                    row![
                        button(text(tr::repo_unregister()).size(12))
                            .padding([5, 12])
                            .style(style::danger)
                            .on_press_maybe(
                                (online && !f.repos_busy)
                                    .then(|| Message::RepoUnregister(r.id.clone()))
                            ),
                        button(text(tr::cancel()).size(12))
                            .padding([5, 12])
                            .style(style::secondary)
                            .on_press(Message::RepoConfirm(false)),
                    ]
                    .spacing(6)
                    .boxed()
                } else {
                    button(text(tr::repo_unregister_keep_files()).size(12))
                        .padding([5, 12])
                        .style(style::secondary)
                        .on_press_maybe(online.then_some(Message::RepoConfirm(true)))
                        .boxed()
                };
                c = c.push(
                    row![
                        column![
                            text(r.name.as_str())
                                .size(22)
                                .font(style::BOLD)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End),
                            text(r.path.as_str())
                                .size(12)
                                .font(style::MONO)
                                .color(style::DIM),
                        ]
                        .spacing(2)
                        .width(Fill),
                        actions,
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .boxed(),
                );
                c = c.push(text(tr::repos_note()).size(12).color(style::DIM).boxed());
                if let Some(e) = &f.repos_error {
                    c = c.push(error_box(e));
                }
                c = c.push(card(self.base_card(r, f, online)));
                c = c.push(card(
                    column![
                        label(tr::repo_sessions_label(i18n::count(users.len()))),
                        sessions,
                    ]
                    .spacing(8)
                    .boxed(),
                ));
            }
        }
        let avail = self.window_size.width - self.left_w();
        let width = (avail - 64.0).clamp(320.0, 896.0);
        container(scrollable(
            container(c.width(width)).center_x(Fill).padding([32, 0]),
        ))
        .width(Fill)
        .height(Fill)
        .style(style::bg(style::BG))
        .boxed()
    }

    /// Desk's 레포 page "기준 브랜치" card: 고정 브랜치 (`FixedBaseSettings`) or 규칙으로 자동
    /// 선택 (`BaseBranchRuleSettings`, without the "AI로 만들기" regex draft).
    fn base_card<'a>(&'a self, r: &'a ide_client::RepoInfo, f: &'a SettingsForm, online: bool) -> El<'a> {
        let Some(b) = f.base.as_ref().filter(|b| b.repo == r.id) else {
            return column![
                label(tr::repo_base_label()),
                text(if r.base.is_empty() { tr::repo_base_unknown() } else { r.base.clone() })
                    .size(13)
                    .font(style::MONO),
            ]
            .spacing(6)
            .boxed();
        };
        let seg = |on: bool, title: String, msg: Message| {
            button(text(title).size(12))
                .padding([4, 12])
                .style(if on { style::primary } else { style::secondary })
                .on_press(msg)
        };
        let mut c = column![
            label(tr::repo_base_label()),
            row![
                seg(!b.rule_mode, tr::base_mode_fixed(), Message::BaseMode(false)),
                seg(b.rule_mode, tr::base_mode_rule(), Message::BaseMode(true)),
            ]
            .spacing(4),
        ]
        .spacing(8);
        if b.saved {
            c = c.push(text(tr::base_saved()).size(12).color(style::GREEN).boxed());
        }
        let small = |s: String| text(s).size(11).color(style::FAINT);
        let can = online && !b.busy && !f.repos_busy;
        if !b.rule_mode {
            if r.base_rule.is_some() {
                c = c.push(
                    container(text(tr::base_rule_active_warning()).size(11))
                        .padding(8)
                        .width(Fill)
                        .style(style::warn_box).boxed(),
                );
            }
            c = c.push(text(tr::base_fixed_note()).size(12).boxed());
            let refs = b.refs.as_deref();
            let missing = b.fixed.as_ref().filter(|x| {
                refs.is_some_and(|refs| {
                    !refs.contains(x)
                        && !(!x.starts_with("refs/")
                            && (refs.contains(&format!("refs/heads/{x}"))
                                || refs.contains(&format!("refs/remotes/{x}"))))
                })
            });
            let auto = Choice {
                key: String::new(),
                label: tr::base_auto(),
            };
            let mut choices = vec![auto.clone()];
            for x in refs.unwrap_or_default() {
                choices.push(Choice {
                    key: x.clone(),
                    label: x.clone(),
                });
            }
            if let Some(x) = &b.fixed
                && !choices.iter().any(|c| &c.key == x)
            {
                choices.push(Choice {
                    key: x.clone(),
                    label: if missing.is_some() { tr::base_missing(x) } else { x.clone() },
                });
            }
            let sel = match &b.fixed {
                None => Some(auto),
                Some(x) => choices.iter().find(|c| &c.key == x).cloned(),
            };
            c = c.push(small(tr::base_fixed_pick_note()).boxed());
            c = c.push(
                pick_list(sel, choices, |c: &Choice| c.label.clone())
                    .on_select(|c: Choice| Message::BaseFixed((!c.key.is_empty()).then_some(c.key)))
                    .placeholder(if refs.is_none() { tr::base_loading() } else { tr::base_auto() })
                    .text_size(12)
                    .padding([5, 8])
                    .width(Fill).boxed(),
            );
            if missing.is_some() {
                c = c.push(text(tr::base_missing_error()).size(11).color(style::RED).boxed());
            }
            c = c.push(small(tr::base_auto_note()).boxed());
            c = c.push(
                row![
                    space().width(Fill),
                    button(text(tr::base_reset_auto()).size(12))
                        .padding([4, 10])
                        .style(style::secondary)
                        .on_press_maybe(can.then_some(Message::BaseFixed(None))),
                    button(text(tr::cancel()).size(12))
                        .padding([4, 10])
                        .style(style::secondary)
                        .on_press_maybe(can.then_some(Message::BaseCancel)),
                    button(text(tr::save()).size(12))
                        .padding([4, 12])
                        .style(style::primary)
                        .on_press_maybe((can && missing.is_none() && refs.is_some()).then_some(Message::BaseFixedSave)),
                ]
                .spacing(6).boxed(),
            );
        } else {
            c = c.push(text(tr::base_rule_note()).size(12).boxed());
            c = c.push(
                row![
                    column![
                        small(tr::base_rule_remote()),
                        text_input("origin", &b.remote)
                            .on_input(Message::BaseRuleRemote)
                            .size(12)
                            .padding([5, 8])
                            .style(style::input),
                    ]
                    .spacing(3)
                    .width(140),
                    column![
                        small(tr::base_rule_pattern()),
                        text_input("release/*", &b.pattern)
                            .on_input(Message::BaseRulePattern)
                            .size(12)
                            .padding([5, 8])
                            .style(style::input),
                        small(tr::base_rule_pattern_note()),
                    ]
                    .spacing(3)
                    .width(Fill),
                ]
                .spacing(8).boxed(),
            );
            c = c.push(small(tr::base_rule_match()).boxed());
            c = c.push(
                text_input(tr::base_rule_match_placeholder(), &b.matches)
                    .on_input(Message::BaseRuleMatch)
                    .size(12)
                    .padding([5, 8])
                    .font(style::MONO)
                    .style(style::input).boxed(),
            );
            c = c.push(small(tr::base_rule_version_note()).boxed());
            let pick: El<'_> = if b.preview_loading {
                text(tr::base_rule_loading()).size(12).color(style::DIM).boxed()
            } else if let Some(e) = &b.preview_error {
                text(e.as_str()).size(12).color(style::RED).boxed()
            } else if let Some(p) = &b.preview {
                let mut col: Column<El<'_>> = Column::new().spacing(2);
                col = col.push(
                    text(p.pick.clone().unwrap_or_else(tr::base_rule_no_pick))
                        .size(13)
                        .font(style::MONO)
                        .color(if p.pick.is_some() { style::TEXT } else { style::RED })
                        .boxed(),
                );
                let last = r
                    .base_rule
                    .as_ref()
                    .and(r.default_base.as_ref())
                    .filter(|l| p.pick.as_ref().is_some_and(|x| x != *l));
                if let (Some(last), Some(pick)) = (last, &p.pick) {
                    col = col.push(small(tr::base_rule_last_pick(last, pick)).boxed());
                }
                for m in p.matched.iter().take(8) {
                    col = col.push(text(format!("✓ {m}")).size(11).font(style::MONO).color(style::DIM).boxed());
                }
                for x in p.excluded.iter().take(8) {
                    col = col.push(
                        text(format!("✕ {x} — {}", tr::base_rule_excluded()))
                            .size(11)
                            .font(style::MONO)
                            .color(style::FAINT)
                            .boxed(),
                    );
                }
                col.boxed()
            } else {
                space().boxed()
            };
            c = c.push(
                row![
                    small(tr::base_rule_current()),
                    space().width(Fill),
                    button(text(tr::base_rule_recheck()).size(11))
                        .padding([3, 8])
                        .style(style::secondary)
                        .on_press_maybe((can && !b.preview_loading).then_some(Message::BaseRuleCheck)),
                ]
                .align_y(Alignment::Center).boxed(),
            );
            c = c.push(container(pick).padding([6, 8]).width(Fill).style(style::bg(style::HEADER)).boxed());
            c = c.push(
                row![
                    space().width(Fill),
                    button(text(tr::base_rule_clear()).size(12))
                        .padding([4, 10])
                        .style(style::secondary)
                        .on_press_maybe((can && r.base_rule.is_some()).then_some(Message::BaseRuleClear)),
                    button(text(tr::cancel()).size(12))
                        .padding([4, 10])
                        .style(style::secondary)
                        .on_press_maybe(can.then_some(Message::BaseCancel)),
                    button(text(tr::save()).size(12))
                        .padding([4, 12])
                        .style(style::primary)
                        .on_press_maybe(can.then_some(Message::BaseRuleSave)),
                ]
                .spacing(6).boxed(),
            );
        }
        container(c)
            .padding(10)
            .width(Fill)
            .style(style::card)
            .boxed()
    }

    // ------------------------------------------------------------------ modals

    /// Configurations as pick-list entries.
    fn config_choices(&self) -> Vec<Choice> {
        self.configs
            .iter()
            .map(|c| Choice {
                key: c.slug.clone(),
                label: c.name.clone(),
            })
            .collect()
    }

    /// An account's integration state and its button (providers that have one; `docs/agent-bridge.md`).
    fn integration_controls<'a>(
        &'a self,
        f: &'a SettingsForm,
        a: &'a ide_client::AccountInfo,
        online: bool,
    ) -> El<'a> {
        use ide_client::{IntegrationAction as A, IntegrationState as S};
        if !self.provider(&a.provider).is_some_and(|p| p.caps.integration) {
            return space().width(0).boxed();
        }
        let key = (a.provider.clone(), a.slug.clone());
        let state = f.acct_integration.iter().find(|(k, _)| *k == key).map(|(_, i)| i.as_ref().map(|i| i.state));
        let (label, action, button_label) = match state {
            None | Some(None) => (tr::integration_checking(), None, None),
            Some(Some(S::NotNeeded)) => return space().width(0).boxed(),
            Some(Some(S::Missing)) => (tr::integration_missing(), Some(A::Install), Some(tr::integration_install())),
            Some(Some(S::Outdated)) => (tr::integration_outdated(), Some(A::Install), Some(tr::integration_update())),
            Some(Some(S::Installed)) => (tr::integration_installed(), Some(A::Uninstall), Some(tr::integration_uninstall())),
        };
        let mut r = row![text(label).size(11).color(style::DIM)]
            .spacing(6)
            .align_y(Alignment::Center);
        if let (Some(action), Some(bl)) = (action, button_label) {
            r = r.push(tip(
                button(text(bl).size(11))
                    .padding([3, 8])
                    .style(style::secondary)
                    .on_press_maybe(online.then(|| {
                        Message::AcctIntegration(a.provider.clone(), a.slug.clone(), action)
                    }))
                    .boxed(),
                tr::integration_tooltip(),
            ));
        }
        r.boxed()
    }

    /// Accounts page: accounts per provider (add = create + open the login command).
    fn accounts_section<'a>(&'a self, f: &'a SettingsForm, online: bool) -> El<'a> {
        let mut list: Column<El<'_>> = Column::new().spacing(4);
        for a in &self.accounts {
            let (g, gc) = provider_glyph(self, &a.provider);
            let can_login = self.provider(&a.provider).is_some_and(|p| p.caps.login);
            let key = (a.provider.clone(), a.slug.clone());
            let status = f
                .acct_status
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, s)| match s {
                    None => tr::account_status_checking(),
                    Some(AccountState::LoggedIn) => tr::account_logged_in(),
                    Some(AccountState::LoggedOut) => tr::account_logged_out(),
                    Some(AccountState::Unknown) => tr::account_status_unknown(),
                })
                .unwrap_or_default();
            let row_el: El<'_> = match &f.acct_renaming {
                Some((p, sl, new)) if *p == a.provider && *sl == a.slug => row![
                    text_input(tr::account_name_placeholder(), new)
                        .on_input(Message::AcctRenameInput)
                        .on_submit(Message::AcctRenameCommit)
                        .size(13)
                        .padding([4, 8])
                        .style(style::input)
                        .width(Fill),
                    button(text(tr::save()).size(12))
                        .padding([4, 10])
                        .style(style::primary)
                        .on_press_maybe(online.then_some(Message::AcctRenameCommit)),
                ]
                .spacing(6)
                .boxed(),
                _ => {
                    let integration = self.integration_controls(f, a, online);
                    row![
                    text(g).size(13).color(gc),
                    text(a.name.as_str()).size(13),
                    text(self.provider_name(&a.provider))
                        .size(11)
                        .color(style::DIM),
                    space().width(Fill),
                    text(status).size(11).color(style::DIM),
                    integration,
                    button(text(tr::account_check()).size(11))
                        .padding([3, 8])
                        .style(style::secondary)
                        .on_press_maybe(
                            online.then(|| Message::AcctStatus(a.provider.clone(), a.slug.clone()))
                        ),
                    button(text(tr::account_login()).size(11))
                        .padding([3, 8])
                        .style(style::secondary)
                        .on_press_maybe(
                            (online && can_login).then(|| {
                                Message::AcctLogin(a.provider.clone(), a.slug.clone())
                            })
                        ),
                    icon_button(
                        I_EDIT,
                        online
                            .then(|| Message::AcctStartRename(a.provider.clone(), a.slug.clone())),
                        tr::rename()
                    ),
                    icon_button(
                        I_DELETE,
                        online.then(|| Message::AcctDelete(a.provider.clone(), a.slug.clone())),
                        tr::account_delete_tooltip()
                    ),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .boxed()
                }
            };
            // After an install: how to approve the hooks in the CLI.
            let key = (a.provider.clone(), a.slug.clone());
            let approve = f
                .acct_integration
                .iter()
                .find(|(k, _)| *k == key)
                .and_then(|(_, i)| i.as_ref())
                .filter(|i| i.state == ide_client::IntegrationState::Installed)
                .and_then(|i| i.approve_with.clone());
            let row_el: El<'_> = match approve {
                Some(cmd) => column![
                    row_el,
                    text(tr::integration_approve(&self.provider_name(&a.provider), &cmd))
                        .size(11)
                        .color(style::AMBER),
                ]
                .spacing(4)
                .boxed(),
                None => row_el,
            };
            list = list.push(
                container(row_el)
                    .padding([4, 8])
                    .style(style::bg(style::HEADER))
                    .boxed(),
            );
        }
        let providers: Vec<Choice> = self
            .providers
            .iter()
            .filter(|p| p.caps.accounts)
            .map(|p| Choice {
                key: p.id.clone(),
                label: p.name.clone(),
            })
            .collect();
        let sel = providers.iter().find(|c| c.key == f.acct_provider).cloned();
        column![
            text(tr::accounts_note()).size(12).color(style::DIM),
            list,
            row![
                pick_list(sel, providers, |c: &Choice| c.label.clone())
                    .on_select(|c: Choice| Message::AcctProvider(c.key))
                    .text_size(13)
                    .padding([5, 8])
                    .width(150),
                text_input(tr::account_name_placeholder(), &f.acct_name)
                    .on_input(Message::AcctName)
                    .on_submit(Message::AcctAdd)
                    .size(13)
                    .padding([5, 8])
                    .style(style::input)
                    .width(Fill),
                button(text(tr::account_add()).size(13))
                    .padding([5, 12])
                    .style(style::secondary)
                    .on_press_maybe((online && !f.busy).then_some(Message::AcctAdd)),
            ]
            .spacing(6),
        ]
        .spacing(8)
        .boxed()
    }

    fn modal_view<'a>(&'a self, modal: &'a Modal) -> El<'a> {
        let card = match modal {
            Modal::NewSession(f) => {
                let providers = self.providers.iter().filter(|p| p.detection.usable).fold(
                    row![].spacing(6),
                    |r, p| {
                        let (g, c) = provider_glyph(self, &p.id);
                        r.push(
                            button(
                                row![text(g).color(c).size(13), text(p.name.as_str()).size(13)]
                                    .spacing(6),
                            )
                            .padding([5, 12])
                            .style(style::choice(f.provider == p.id))
                            .on_press(Message::NewProvider(p.id.clone()))
                            .boxed(),
                        )
                    },
                );
                let configs = self.config_choices();
                let config_sel = configs.iter().find(|c| c.key == f.configuration).cloned();
                let with_accounts = self.provider(&f.provider).is_some_and(|p| p.caps.accounts);
                let accounts: Vec<Choice> = std::iter::once(Choice {
                    key: String::new(),
                    label: tr::account_default(),
                })
                .chain(
                    self.accounts
                        .iter()
                        .filter(|a| a.provider == f.provider)
                        .map(|a| Choice {
                            key: a.slug.clone(),
                            label: a.name.clone(),
                        }),
                )
                .collect();
                let account_sel = accounts
                    .iter()
                    .find(|c| c.key == f.account.clone().unwrap_or_default())
                    .cloned();
                let account_row: El<'_> = if with_accounts {
                    column![
                        label(tr::account_label()),
                        pick_list(account_sel, accounts, |c: &Choice| c.label.clone())
                            .on_select(|c: Choice| Message::NewAccount(c.key))
                            .text_size(13)
                            .padding([5, 8])
                            .width(Fill),
                    ]
                    .spacing(8)
                    .boxed()
                } else {
                    space().height(0).boxed()
                };
                let mut c = column![
                    text(tr::new_session()).size(18).font(style::BOLD),
                    label(tr::agent_label()),
                    providers,
                    account_row,
                    label(tr::configuration_label()),
                    pick_list(config_sel, configs, |c: &Choice| c.label.clone())
                        .on_select(|c: Choice| Message::NewConfiguration(c.key))
                        .text_size(13)
                        .padding([5, 8])
                        .width(Fill),
                    label(tr::title_label()),
                    text_input(tr::title_placeholder(), &f.title)
                        .id(NEW_TITLE_ID)
                        .on_input(Message::NewTitle)
                        .on_submit(Message::NewSubmit)
                        .size(13)
                        .padding([6, 8])
                        .style(style::input),
                    self.new_workspace_section(f),
                ]
                .spacing(8);
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press(Message::CloseModal),
                        button(text(if f.busy { tr::creating() } else { tr::create() }).size(13))
                            .padding([6, 14])
                            .style(style::primary)
                            .on_press_maybe(
                                (!f.busy)
                                    .then_some(Message::NewSubmit)
                                    .filter(|_| self.connected())
                            ),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(520).boxed()
            }
            Modal::Delete(f) => {
                let s = self.live(&f.id).map(|l| &l.info);
                let title = s.map(|s| s.title.as_str()).unwrap_or("?");
                let mut c = column![
                    text(tr::delete_session_title(title))
                        .size(18)
                        .font(style::BOLD),
                    text(tr::delete_session_body()).size(13).color(style::DIM),
                    checkbox(f.remove_worktree)
                        .label(tr::delete_remove_worktree())
                        .on_toggle(Message::DeleteWorktree)
                        .size(16)
                        .text_size(13),
                ]
                .spacing(10);
                if let Some(s) = s {
                    let mut lines = vec![s.root.clone()];
                    lines.extend(
                        s.repos
                            .iter()
                            .map(|r| format!("  {}  ({})", r.path, r.branch)),
                    );
                    c = c.push(
                        text(lines.join("\n"))
                            .size(11)
                            .font(style::MONO)
                            .color(style::FAINT)
                            .boxed(),
                    );
                }
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                        button(text(if f.busy { tr::deleting() } else { tr::delete() }).size(13))
                            .padding([6, 14])
                            .style(style::danger)
                            .on_press_maybe(
                                (!f.busy)
                                    .then_some(Message::DeleteConfirm)
                                    .filter(|_| self.connected())
                            ),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(480).boxed()
            }
            Modal::StopServer(f) => {
                let mut c = column![
                    text(tr::stop_server_title()).size(18).font(style::BOLD),
                    text(tr::stop_server_body()).size(13).color(style::DIM),
                ]
                .spacing(10);
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                        button(
                            text(if f.busy {
                                tr::stopping()
                            } else {
                                tr::stop_server_confirm()
                            })
                            .size(13)
                        )
                        .padding([6, 14])
                        .style(style::danger)
                        .on_press_maybe(
                            (!f.busy)
                                .then_some(Message::StopServerConfirm)
                                .filter(|_| self.connected())
                        ),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(460).boxed()
            }
            Modal::SwitchServer(pref) => {
                let target = match pref {
                    ServerPref::Local => tr::conn_local(),
                    ServerPref::Ssh { host, .. } => format!("ssh {host}"),
                };
                column![
                    text(tr::switch_server_title()).size(18).font(style::BOLD),
                    text(tr::switch_server_body(&target))
                        .size(13)
                        .color(style::DIM),
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press(Message::CloseModal),
                        button(text(tr::switch_server()).size(13))
                            .padding([6, 14])
                            .style(style::primary)
                            .on_press(Message::SwitchServerConfirm),
                    ]
                    .spacing(8),
                ]
                .spacing(10)
                .width(460)
                .boxed()
            }
            Modal::Close(f) => {
                let running = self.sessions.iter().filter(|l| l.state.is_alive()).count();
                let mut c = column![
                    text(tr::close_title()).size(18).font(style::BOLD),
                    text(tr::close_body(i18n::count(running)))
                        .size(13)
                        .color(style::DIM),
                    checkbox(f.remember)
                        .label(tr::close_remember())
                        .on_toggle_maybe((!f.busy).then_some(Message::CloseRemember))
                        .size(16)
                        .text_size(13),
                ]
                .spacing(10);
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c = c.push(
                    row![
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                        space().width(Fill),
                        button(
                            text(if f.busy {
                                tr::stopping()
                            } else {
                                tr::close_stop()
                            })
                            .size(13)
                        )
                        .padding([6, 14])
                        .style(style::danger)
                        .on_press_maybe((!f.busy).then_some(Message::CloseStop)),
                        button(text(tr::close_keep()).size(13))
                            .padding([6, 14])
                            .style(style::primary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseKeep)),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(560).boxed()
            }
            Modal::QuickOpen(f) => self.quick_open_card(f),
            Modal::Changes(f) => self.changes_modal_card(f),
            Modal::Footer(f) => self.footer_modal_card(f),
            Modal::SkillEdit(f) => self.skill_modal_card(f),
            Modal::AddProfile => self.add_profile_card(),
            Modal::NewChild(f) => self.child_card(f),
            Modal::SelfUpdate => self.update_card(),
            Modal::PickRoot(f) => self.pick_root_card(f),
            Modal::SessionRepos(f) => self.session_repos_card(f),
            Modal::InitRepo(f) => {
                // Desk's `ws:pickRepo` question for a plain folder.
                let mut c = column![
                    text(tr::init_repo_title()).size(18).font(style::BOLD),
                    text(f.path.as_str()).size(12).font(style::MONO).color(style::DIM),
                    text(tr::init_repo_message()).size(13),
                    text(tr::init_repo_detail()).size(12).color(style::DIM),
                    text(tr::init_repo_no_remote()).size(12).color(style::DIM),
                ]
                .spacing(10);
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                        button(text(tr::init_repo_confirm()).size(13))
                            .padding([6, 14])
                            .style(style::primary)
                            .on_press_maybe(
                                (!f.busy && self.connected()).then_some(Message::RepoInitConfirm)
                            ),
                    ]
                    .spacing(8)
                    .boxed(),
                )
                .width(480)
                .boxed()
            }
            Modal::DefPicker(f) => {
                let mut list = Column::new().spacing(2);
                for (i, t) in f.targets.iter().enumerate() {
                    list = list.push(
                        button(
                            column![
                                row![
                                    ide_views::icons::file_icon(crate::detail::file_name(&t.path)),
                                    text(format!("{}:{}", t.path, t.line)).size(12),
                                ]
                                .spacing(8)
                                .align_y(Alignment::Center),
                                text(t.preview.as_str())
                                    .size(11)
                                    .font(style::MONO)
                                    .color(style::DIM)
                                    .wrapping(text::Wrapping::None)
                                    .ellipsis(text::Ellipsis::End),
                            ]
                            .spacing(2),
                        )
                        .padding([5, 8])
                        .width(Fill)
                        .style(style::row(false))
                        .on_press(Message::DefPick(i))
                        .boxed(),
                    );
                }
                column![
                    text(tr::def_pick_title(&f.name)).size(16).font(style::BOLD),
                    scrollable(list).height(Length::Shrink),
                ]
                .spacing(10)
                .width(560)
                .boxed()
            }
            Modal::DeleteEntry(f) => {
                let name = crate::detail::file_name(&f.path);
                let many = !f.more.is_empty();
                let mut c = column![
                    text(if many {
                        tr::delete_many_title(f.more.len() as i64 + 1)
                    } else {
                        tr::delete_entry_title(name)
                    })
                    .size(18)
                    .font(style::BOLD),
                    text(if many {
                        tr::delete_many_body()
                    } else if f.is_dir {
                        tr::delete_folder_body()
                    } else {
                        tr::delete_entry_body()
                    })
                    .size(13)
                    .color(style::DIM),
                    text(f.path.as_str())
                        .size(11)
                        .font(style::MONO)
                        .color(style::FAINT),
                ]
                .spacing(10);
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                        button(text(if f.busy { tr::deleting() } else { tr::delete() }).size(13))
                            .padding([6, 14])
                            .style(style::danger)
                            .on_press_maybe(
                                (!f.busy)
                                    .then_some(Message::DeleteEntryConfirm)
                                    .filter(|_| self.connected())
                            ),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(460).boxed()
            }
            // One file: "save “x”?"; several items: Desk's 저장하지 않은 변경 with the list.
            Modal::CloseDirty(f) if f.keys.len() == 1 && f.dirty.len() == 1 => column![
                text(tr::unsaved_title(crate::detail::file_name(&f.dirty[0])))
                    .size(18)
                    .font(style::BOLD),
                text(tr::unsaved_body()).size(13).color(style::DIM),
                row![
                    button(text(tr::cancel()).size(13))
                        .padding([6, 14])
                        .style(style::secondary)
                        .on_press(Message::CloseModal),
                    space().width(Fill),
                    button(text(tr::dont_save()).size(13))
                        .padding([6, 14])
                        .style(style::danger)
                        .on_press(Message::CloseDirtyDiscard),
                    button(text(tr::save()).size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press_maybe(self.when_connected(Message::CloseDirtySave)),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .width(460)
            .boxed(),
            Modal::CloseDirty(f) => {
                let mut names = Column::new().spacing(2);
                for p in &f.dirty {
                    names = names.push(text(crate::detail::file_name(p)).size(12).font(style::MONO));
                }
                column![
                    text(tr::close_many_title()).size(18).font(style::BOLD),
                    text(tr::close_many_body()).size(13).color(style::DIM),
                    names,
                    row![
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press(Message::CloseModal),
                        space().width(Fill),
                        button(text(tr::close_many_discard()).size(13))
                            .padding([6, 14])
                            .style(style::danger)
                            .on_press(Message::CloseDirtyDiscard),
                        button(text(tr::close_many_save()).size(13))
                            .padding([6, 14])
                            .style(style::primary)
                            .on_press_maybe(self.when_connected(Message::CloseDirtySave)),
                    ]
                    .spacing(8),
                ]
                .spacing(10)
                .width(460)
                .boxed()
            }
        };
        opaque(
            mouse_area(
                center(opaque(container(card).padding(20).style(style::modal_card)))
                    .style(style::backdrop),
            )
            .on_press(Message::CloseModal),
        )
        .into()
    }
}
