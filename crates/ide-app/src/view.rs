//! Window layout: navigation rail | page, where the work page is session list | terminal |
//! divider | detail; other pages (accounts, configurations, repos, settings) cover the work page,
//! which stays in the widget tree. Modal dialogs on top.

use iced::widget::{
    Column, button, center, checkbox, column, container, mouse_area, opaque, pick_list, row,
    scrollable, space, stack, text, text_editor, text_input, tooltip,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Widget as _, mouse};
use ide_client::{AccountState, BrowseEntry, Support};
use ide_i18n::LanguagePref;
use ide_term::TerminalView;

use crate::app::{
    App, CONFIG_NAME_ID, Choice, Conn, DIVIDER_W, Live, McpField, Message, Modal, NEW_REPO_ID,
    NewForm, PICK_LIST_ID, PickRootForm, QuickForm, RAIL_W, RENAME_ID, ROOT_INPUT_ID, RunState,
    SIDEBAR_W, SSH_HOST_ID, SettingsForm, TERM_ID, path_name, same_path,
};
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::style;
use crate::ui_state::{CloseAction, Page, ServerPref};

type El<'a> = Element<'a, Message>;

// Symbols (Segoe UI Symbol / system fallback fonts on Windows).
const I_PLUS: &str = "+";
// U+25B8: U+25B6 would get the emoji presentation from Segoe UI Emoji.
const I_PLAY: &str = "▸";
const I_RESTART: &str = "↻";
const I_STOP: &str = "■";
const I_EDIT: &str = "✎";
const I_DELETE: &str = "✕";
const I_DOT: &str = "●";
const I_RING: &str = "○";

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

fn status_glyph(state: RunState) -> (&'static str, Color) {
    match state {
        RunState::Stopped => (I_RING, style::FAINT),
        RunState::Starting => (I_DOT, style::AMBER),
        RunState::Running => (I_DOT, style::GREEN),
        RunState::Exited(_) => (I_DOT, style::FAINT),
        RunState::Failed => (I_DOT, style::RED),
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
fn card(content: El<'_>) -> El<'_> {
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

fn error_box<'a>(e: &'a str) -> El<'a> {
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
            (
                self.sidebar(),
                container(space())
                    .width(1)
                    .height(Fill)
                    .style(style::bg(style::BORDER))
                    .boxed(),
            )
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
        // Always a stack so opening/closing a modal keeps the widget tree (terminal).
        let overlay: El<'_> = match &self.modal {
            Some(m) => self.modal_view(m),
            None => space().boxed(),
        };
        let root = stack![base, overlay].boxed();
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
        if page != Page::Work || !self.on_work_page() {
            return (icon, label);
        }
        let open = self.ui.sessions_open;
        let icon = match (self.rail_hover, open) {
            (false, _) => icon,
            (true, true) => Icon::SidebarCollapse,
            (true, false) => Icon::SidebarExpand,
        };
        let label = if open {
            tr::sessions_hide()
        } else {
            tr::sessions_show()
        };
        (icon, label)
    }

    /// Tooltip of the rail's server indicator: target, state and (connected) server version.
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
    fn conn_color(&self) -> Color {
        match &self.conn {
            Conn::Connected(_) => style::GREEN,
            Conn::Connecting | Conn::Reconnecting => style::AMBER,
            Conn::Disconnected(_) => style::RED,
        }
    }

    /// Navigation rail (Hermes Desk's `NavRail`): 64 px, 44 px buttons 6 px apart, the server
    /// indicator at the bottom.
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
            if page == Page::Work {
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
        // Server indicator: pulse icon with a status dot (Mantine `Indicator`: 9 px, 7 px in
        // from the top right corner).
        let server = stack![
            link(
                Icon::ActivityHeartbeat,
                false,
                Message::Page(Page::Settings)
            ),
            container(
                container(space())
                    .width(9)
                    .height(9)
                    .style(style::dot(self.conn_color())),
            )
            .padding(iced::Padding {
                top: 2.5,
                left: 32.5,
                ..iced::Padding::ZERO
            }),
        ]
        .width(44)
        .height(44)
        .boxed();
        let col = column![
            top.height(Fill),
            container(tip(server, self.server_tooltip())).padding(iced::Padding {
                bottom: 10.0,
                ..iced::Padding::ZERO
            }),
        ]
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
        let configs = self.config_choices();
        let current = self.current_configuration();
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
            icon_button(
                I_PLUS,
                self.when_connected(Message::OpenNew),
                tr::new_session()
            ),
        ]
        .align_y(Alignment::Center);

        let mut list: Column<El<'_>> = Column::new().spacing(2);
        let mine: Vec<&Live> = self
            .sessions
            .iter()
            .filter(|l| self.in_filter(&l.info))
            .collect();
        // Group by repository, in order of first appearance.
        let mut groups: Vec<(&Live, Vec<&Live>)> = Vec::new();
        for l in &mine {
            match groups
                .iter_mut()
                .find(|(first, _)| same_path(&first.info.repo_path, &l.info.repo_path))
            {
                Some((_, v)) => v.push(l),
                None => groups.push((l, vec![l])),
            }
        }
        for (first, members) in groups {
            let repo_name = path_name(&first.info.repo_path);
            list = list.push(
                container(
                    text(repo_name.to_uppercase())
                        .size(11)
                        .font(style::BOLD)
                        .color(style::DIM),
                )
                .padding([6, 4])
                .boxed(),
            );
            for l in members {
                list = list.push(self.session_row(l));
            }
        }
        if mine.is_empty() && self.loaded {
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
            .width(SIDEBAR_W)
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
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

    fn session_row<'a>(&'a self, live: &'a Live) -> El<'a> {
        let s = &live.info;
        let selected = self.selected.as_deref() == Some(s.id.as_str());
        let (glyph, gcolor) = provider_glyph(self, &s.provider);
        let (dot, dcolor) = status_glyph(live.state);

        if let Some((id, value)) = &self.rename
            && id == &s.id
        {
            return text_input(tr::session_title_placeholder(), value)
                .id(RENAME_ID)
                .on_input(Message::RenameInput)
                .on_submit(Message::RenameCommit)
                .size(13)
                .padding([5, 8])
                .style(style::input)
                .boxed();
        }

        let subtitle = if !s.term_title.is_empty() {
            s.term_title.clone()
        } else {
            i18n::status_label(live.state)
        };
        let main = button(
            row![
                text(glyph).size(14).color(gcolor).width(18),
                column![
                    text(s.title.as_str())
                        .size(13)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                    text(subtitle)
                        .size(11)
                        .color(style::DIM)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                ]
                .width(Fill),
                text(dot).size(10).color(dcolor),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([4, 6])
        .width(Fill)
        .style(style::row(selected))
        .on_press(Message::Select(s.id.clone()));
        let main = mouse_area(main).on_right_press(Message::StartRename(s.id.clone()));

        let run: El<'a> = match live.state {
            RunState::Starting | RunState::Running => icon_button(
                I_STOP,
                self.when_connected(Message::Stop(s.id.clone())),
                tr::stop(),
            ),
            RunState::Stopped => icon_button(
                I_PLAY,
                self.when_connected(Message::Start(s.id.clone())),
                tr::start(),
            ),
            RunState::Exited(_) | RunState::Failed => icon_button(
                I_RESTART,
                self.when_connected(Message::Start(s.id.clone())),
                tr::restart(),
            ),
        };
        row![
            main,
            run,
            icon_button(
                I_EDIT,
                self.when_connected(Message::StartRename(s.id.clone())),
                tr::rename()
            ),
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
        let width = if self.ui.detail_collapsed {
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
                text(s.branch.as_str())
                    .size(12)
                    .color(style::DIM)
                    .font(style::MONO),
                text(s.worktree_path.as_str())
                    .size(11)
                    .color(style::FAINT)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start)
                    .width(Fill),
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
            col = col.push(view.boxed());
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

    fn divider(&self) -> El<'_> {
        if self.ui.detail_collapsed {
            return container(space())
                .width(1)
                .height(Fill)
                .style(style::bg(style::BORDER))
                .boxed();
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

    /// The new-session dialog's folder browser (folders on the server).
    fn folder_browser<'a>(&'a self, f: &'a NewForm) -> El<'a> {
        let Some(b) = &f.browser else {
            return space().height(0).boxed();
        };
        let current = b.listing.as_ref();
        let up = current.and_then(|l| l.parent.clone());
        let roots = pick_list(
            // The innermost root containing the listed folder.
            current.and_then(|l| {
                b.roots
                    .iter()
                    .filter(|r| l.path.starts_with(r.as_str()))
                    .max_by_key(|r| r.len())
                    .cloned()
            }),
            b.roots.clone(),
            |s: &String| s.clone(),
        )
        .on_select(Message::BrowseGo)
        .text_size(12)
        .padding([4, 8])
        .width(Fill);
        let head = row![
            icon_button("↑", up.map(Message::BrowseGo), tr::browse_up()),
            roots,
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let mut list: Column<El<'a>> = Column::new().spacing(1);
        if let Some(l) = current {
            list = list.push(
                text(l.path.as_str())
                    .size(11)
                    .font(style::MONO)
                    .color(if l.is_git_repo {
                        style::GREEN
                    } else {
                        style::DIM
                    })
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start)
                    .boxed(),
            );
            if l.entries.is_empty() {
                list = list.push(
                    text(tr::browse_empty())
                        .size(12)
                        .color(style::FAINT)
                        .boxed(),
                );
            }
            for e in &l.entries {
                let r = folder_entry_row(e, None);
                list = list.push(
                    button(r)
                        .padding([3, 6])
                        .width(Fill)
                        .style(style::row(same_path(&e.path, &f.repo)))
                        .on_press(Message::BrowseGo(e.path.clone()))
                        .boxed(),
                );
            }
        } else if b.loading {
            list = list.push(text(tr::tree_loading()).size(12).color(style::DIM).boxed());
        }
        let list = list.padding(iced::Padding {
            right: 12.0,
            ..iced::Padding::ZERO
        });
        let mut c = column![head, scrollable(list).height(200)].spacing(6);
        if let Some(e) = &b.error {
            c = c.push(error_box(e));
        }
        browser_frame(c.boxed())
    }

    /// The repos page's folder picker (`browse.pick_list`: any folder the server's OS user
    /// can read; server-side, so it also works over SSH).
    fn pick_root_card<'a>(&'a self, f: &'a PickRootForm) -> El<'a> {
        let current = f.listing.as_ref();
        let home = self.settings.roots.as_ref().and_then(|c| c.home.as_deref());
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
                let tag = (l.path.is_empty()
                    && home.is_some_and(|h| crate::app::same_path(h, &e.path)))
                .then(tr::browse_home);
                let selected = f.selected == Some(i);
                let r = container(folder_entry_row(e, tag))
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
            text(tr::pick_root_title()).size(18).font(style::BOLD),
            text(tr::pick_root_note()).size(12).color(style::DIM),
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
                            .filter(|_| self.connected() && !self.settings.roots_busy)
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
        for (i, m) in f.results.iter().enumerate() {
            let name = crate::detail::file_name(&m.path);
            list = list.push(
                button(
                    row![
                        ide_views::icons::file_icon(name),
                        text(name).size(13).wrapping(text::Wrapping::None),
                        text(crate::detail::parent(&m.path).to_owned())
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
                .on_press(Message::QuickPick(m.path.clone()))
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
        (self.modal.is_some() && !self.modal_web_unsafe) || !self.on_work_page()
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
            Page::Configurations => (
                tr::nav_configurations(),
                vec![card(self.configurations_section(f, online))],
            ),
            Page::Repos => (tr::nav_repos(), vec![card(self.roots_section(f, online))]),
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

    /// Repos page: folders the new-session dialog can browse on the server.
    fn roots_section<'a>(&'a self, f: &'a SettingsForm, online: bool) -> El<'a> {
        let mut c = column![
            label(tr::browse_roots_label()),
            text(tr::browse_roots_note()).size(12).color(style::DIM),
        ]
        .spacing(10);
        if let Some(cfg) = &f.roots {
            let mut list: Column<El<'_>> = Column::new().spacing(4);
            if let Some(h) = &cfg.home {
                list = list.push(
                    container(
                        row![
                            ide_views::icons::folder_icon(false, style::DIM),
                            text(h.as_str()).size(12).width(Fill),
                            text(tr::browse_home()).size(11).color(style::DIM),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding([5, 8])
                    .style(style::bg(style::HEADER))
                    .boxed(),
                );
            }
            for (i, r) in cfg.roots.iter().enumerate() {
                list = list.push(
                    container(
                        row![
                            ide_views::icons::folder_icon(false, style::DIM),
                            text(r.as_str()).size(12).width(Fill),
                            icon_button(
                                I_DELETE,
                                (online && !f.roots_busy).then_some(Message::RootRemove(i)),
                                tr::remove()
                            ),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding([2, 8])
                    .style(style::bg(style::HEADER))
                    .boxed(),
                );
            }
            c = c.push(list.boxed());
        }
        c = c.push(
            row![
                text_input(tr::browse_root_placeholder(), &f.root_input)
                    .id(ROOT_INPUT_ID)
                    .on_input(Message::RootInput)
                    .on_submit(Message::RootAdd)
                    .size(13)
                    .padding([5, 8])
                    .style(style::input)
                    .width(Fill),
                tip(
                    button(icons::icon(Icon::FolderOpen, 16.0, style::DIM))
                        .padding([5, 7])
                        .style(style::secondary)
                        .on_press_maybe((online && !f.roots_busy).then_some(Message::PickOpen))
                        .boxed(),
                    tr::pick_root_open()
                ),
                button(text(tr::add()).size(13))
                    .padding([5, 12])
                    .style(style::secondary)
                    .on_press_maybe(
                        (online && !f.roots_busy && !f.root_input.trim().is_empty())
                            .then_some(Message::RootAdd)
                    ),
            ]
            .spacing(6)
            .boxed(),
        );
        if let Some(e) = &f.roots_error {
            c = c.push(error_box(e));
        }
        c.boxed()
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
                _ => row![
                    text(g).size(13).color(gc),
                    text(a.name.as_str()).size(13),
                    text(self.provider_name(&a.provider))
                        .size(11)
                        .color(style::DIM),
                    space().width(Fill),
                    text(status).size(11).color(style::DIM),
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
                .boxed(),
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

    /// Configurations page: configurations (list, add, delete) and a plain editor for one of them.
    fn configurations_section<'a>(&'a self, f: &'a SettingsForm, online: bool) -> El<'a> {
        let mut list: Column<El<'_>> = Column::new().spacing(4);
        for c in &self.configs {
            let (total, running) = self.sessions_in_configuration(&c.slug);
            let editing = f.cfg_edit.as_ref().is_some_and(|e| e.base.slug == c.slug);
            let row_el = row![
                text(c.name.as_str()).size(13).width(Fill),
                text(tr::configuration_sessions(
                    i18n::count(total),
                    i18n::count(running)
                ))
                .size(11)
                .color(style::DIM),
                icon_button(
                    I_EDIT,
                    (online && !editing).then(|| Message::CfgEdit(c.slug.clone())),
                    tr::configuration_edit()
                ),
                icon_button(
                    I_DELETE,
                    online.then(|| Message::CfgDelete(c.slug.clone())),
                    tr::configuration_delete_tooltip()
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            list = list.push(
                container(row_el)
                    .padding([4, 8])
                    .style(style::bg(style::HEADER))
                    .boxed(),
            );
            if editing && let Some(e) = &f.cfg_edit {
                list = list.push(self.config_editor(e, f.busy, online));
            }
        }
        column![
            text(tr::configurations_note()).size(12).color(style::DIM),
            list,
            row![
                text_input(tr::configuration_name_placeholder(), &f.cfg_name)
                    .id(CONFIG_NAME_ID)
                    .on_input(Message::CfgName)
                    .on_submit(Message::CfgAdd)
                    .size(13)
                    .padding([5, 8])
                    .style(style::input)
                    .width(Fill),
                button(text(tr::add()).size(13))
                    .padding([5, 12])
                    .style(style::secondary)
                    .on_press_maybe((online && !f.busy).then_some(Message::CfgAdd)),
            ]
            .spacing(6),
        ]
        .spacing(8)
        .boxed()
    }

    fn config_editor<'a>(
        &'a self,
        e: &'a crate::app::ConfigEdit,
        busy: bool,
        online: bool,
    ) -> El<'a> {
        let small = |s: String| text(s).size(11).color(style::DIM);
        let input = |ph: String, v: &'a str| {
            text_input(ph, v)
                .size(12)
                .padding([4, 6])
                .style(style::input)
        };
        // Which features the providers pass on (native / fallback / not supported).
        let support = self
            .providers
            .iter()
            .filter(|p| p.caps.accounts)
            .map(|p| {
                let s = |x: Support| match x {
                    Support::Native => "✓",
                    Support::Fallback => "~",
                    Support::Unsupported => "✕",
                };
                tr::provider_support(
                    &p.name,
                    s(p.caps.skills),
                    s(p.caps.mcp),
                    s(p.caps.instructions),
                )
            })
            .collect::<Vec<_>>()
            .join("   ");
        let mut skills: Column<El<'_>> = Column::new().spacing(4);
        for (i, sk) in e.skills.iter().enumerate() {
            skills = skills.push(
                row![
                    input(tr::configuration_skill_placeholder(), sk)
                        .on_input(move |v| Message::CfgSkill(i, v))
                        .width(Fill),
                    icon_button(I_DELETE, Some(Message::CfgSkillRemove(i)), tr::remove()),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }
        let mut mcp: Column<El<'_>> = Column::new().spacing(4);
        for (i, m) in e.mcp.iter().enumerate() {
            mcp = mcp.push(
                row![
                    input(tr::mcp_name_placeholder(), &m.name)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Name, v))
                        .width(90),
                    input(tr::mcp_command_placeholder(), &m.command)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Command, v))
                        .width(90),
                    input(tr::mcp_args_placeholder(), &m.args)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Args, v))
                        .width(Fill),
                    input(tr::mcp_env_placeholder(), &m.env)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Env, v))
                        .width(110),
                    icon_button(I_DELETE, Some(Message::CfgMcpRemove(i)), tr::remove()),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }
        container(
            column![
                small(tr::configuration_name_label()),
                input(tr::configuration_name_placeholder(), &e.name).on_input(Message::CfgEditName),
                small(tr::configuration_instructions()),
                text_editor(&e.instructions)
                    .on_action(Message::CfgInstructions)
                    .size(12)
                    .height(90),
                row![
                    small(tr::configuration_skills()),
                    space().width(Fill),
                    button(text(tr::add()).size(11))
                        .padding([2, 8])
                        .style(style::secondary)
                        .on_press(Message::CfgSkillAdd),
                ]
                .align_y(Alignment::Center),
                skills,
                row![
                    small(tr::configuration_mcp()),
                    space().width(Fill),
                    button(text(tr::add()).size(11))
                        .padding([2, 8])
                        .style(style::secondary)
                        .on_press(Message::CfgMcpAdd),
                ]
                .align_y(Alignment::Center),
                mcp,
                text(support).size(10).color(style::FAINT),
                row![
                    space().width(Fill),
                    button(text(tr::cancel()).size(12))
                        .padding([4, 12])
                        .style(style::secondary)
                        .on_press(Message::CfgCancel),
                    button(text(tr::save()).size(12))
                        .padding([4, 12])
                        .style(style::primary)
                        .on_press_maybe((online && !busy).then_some(Message::CfgSave)),
                ]
                .spacing(6),
            ]
            .spacing(6),
        )
        .padding(8)
        .style(style::bg(style::BG))
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
                    label(tr::repo_folder_label()),
                    row![
                        text_input(tr::repo_placeholder(), &f.repo)
                            .id(NEW_REPO_ID)
                            .on_input(Message::NewRepo)
                            .on_submit(Message::NewSubmit)
                            .size(13)
                            .padding([6, 8])
                            .style(style::input)
                            .width(Fill),
                        tip(
                            button(ide_views::icons::folder_icon(
                                f.browser.is_some(),
                                style::DIM
                            ))
                            .padding([7, 8])
                            .style(style::choice(f.browser.is_some()))
                            .on_press_maybe(self.when_connected(Message::NewBrowse))
                            .boxed(),
                            tr::browse()
                        ),
                    ]
                    .spacing(6),
                    self.folder_browser(f),
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
                        .on_input(Message::NewTitle)
                        .on_submit(Message::NewSubmit)
                        .size(13)
                        .padding([6, 8])
                        .style(style::input),
                    label(tr::base_ref_label()),
                    text_input(tr::base_ref_placeholder(), &f.base_ref)
                        .on_input(Message::NewBase)
                        .on_submit(Message::NewSubmit)
                        .size(13)
                        .padding([6, 8])
                        .style(style::input),
                    text(tr::worktree_note()).size(11).color(style::FAINT),
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
                    c = c.push(
                        text(format!("{}  ({})", s.worktree_path, s.branch))
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
            Modal::PickRoot(f) => self.pick_root_card(f),
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
                let mut c = column![
                    text(tr::delete_entry_title(name))
                        .size(18)
                        .font(style::BOLD),
                    text(if f.is_dir {
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
            Modal::CloseDirty(f) => column![
                text(tr::unsaved_title(crate::detail::file_name(&f.path)))
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
