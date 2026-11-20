//! Window layout: sidebar | terminal | divider | detail, plus modal dialogs.

use iced::widget::{
    Column, button, center, checkbox, column, container, mouse_area, opaque, pick_list, row,
    scrollable, space, stack, text, text_input, tooltip,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Widget as _, mouse};
use ide_client::Agent;
use ide_i18n::LanguagePref;
use ide_term::TerminalView;
use ide_web::WebViewWidget;

use crate::app::{
    AGENTS, App, Conn, DIVIDER_W, Live, Message, Modal, NEW_REPO_ID, PROFILE_NAME_ID, RENAME_ID,
    RunState, SIDEBAR_W, SSH_HOST_ID, TERM_ID, URL_ID, path_name, same_path,
};
use crate::i18n::{self, tr};
use crate::style;
use crate::ui_state::{CloseAction, ServerPref};

type El<'a> = Element<'a, Message>;

// Symbols (Segoe UI Symbol / system fallback fonts on Windows).
const I_PLUS: &str = "+";
const I_GEAR: &str = "⚙";
// U+25B8: U+25B6 would get the emoji presentation from Segoe UI Emoji.
const I_PLAY: &str = "▸";
const I_RESTART: &str = "↻";
const I_STOP: &str = "■";
const I_EDIT: &str = "✎";
const I_DELETE: &str = "✕";
const I_BACK: &str = "←";
const I_FORWARD: &str = "→";
const I_RELOAD: &str = "⟳";
const I_COLLAPSE: &str = "»";
const I_EXPAND: &str = "«";
const I_DOT: &str = "●";
const I_RING: &str = "○";

pub fn agent_glyph(agent: Agent) -> (&'static str, Color) {
    match agent {
        Agent::Claude => ("✻", style::CLAUDE),
        Agent::Codex => ("◆", style::CODEX),
        Agent::Shell => ("❯", style::DIM),
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

fn tip<'a>(content: impl Into<El<'a>>, label: String) -> El<'a> {
    tooltip(
        content.into(),
        container(text(label).size(12))
            .padding([3, 7])
            .style(style::modal_card),
    )
    .position(tooltip::Position::Bottom)
    .boxed()
}

fn icon_button<'a>(glyph: &'a str, msg: Option<Message>, label: String) -> El<'a> {
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

fn error_box<'a>(e: &'a str) -> El<'a> {
    container(text(e).size(12))
        .padding(8)
        .width(Fill)
        .style(style::error_box)
        .boxed()
}

impl App {
    pub fn view(&self) -> El<'_> {
        let base = row![
            self.sidebar(),
            container(space())
                .width(1)
                .height(Fill)
                .style(style::bg(style::BORDER)),
            self.centre(),
            self.divider(),
            self.detail(),
        ]
        .height(Fill);
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

    // ------------------------------------------------------------------ sidebar

    fn sidebar(&self) -> El<'_> {
        let names: Vec<String> = self.profiles.iter().map(|p| p.name.clone()).collect();
        let header = row![
            text("ide").size(15).font(style::BOLD),
            space().width(Fill),
            icon_button(I_GEAR, Some(Message::OpenSettings), tr::settings()),
        ]
        .align_y(Alignment::Center);
        let profile = row![
            label(tr::profile_label()),
            space().width(8),
            pick_list(Some(self.current_profile()), names, |s: &String| s.clone())
                .on_select(Message::ProfilePicked)
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
        let profile_name = self.current_profile();
        let mine: Vec<&Live> = self
            .sessions
            .iter()
            .filter(|l| l.info.profile == profile_name)
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

        let mut col = column![
            header,
            profile,
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER)),
            sessions_header,
            scrollable(list).height(Fill),
        ]
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
        col = col.push(self.connection_footer());
        container(col)
            .width(SIDEBAR_W)
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
            .boxed()
    }

    /// Connection status: connected (target + server version), reconnecting, disconnected with
    /// a retry button.
    fn connection_footer(&self) -> El<'_> {
        let (dot, color, msg, retry) = match &self.conn {
            Conn::Connecting => (I_DOT, style::AMBER, tr::conn_connecting(), false),
            Conn::Connected(w) => (
                I_DOT,
                style::GREEN,
                tr::conn_connected(&w.server_version),
                false,
            ),
            Conn::Reconnecting => (I_DOT, style::AMBER, tr::conn_reconnecting(), true),
            Conn::Disconnected(why) => (I_DOT, style::RED, tr::conn_disconnected(why), true),
        };
        let mut r = row![
            text(dot).size(10).color(color),
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
            r = r.push(icon_button(
                I_RESTART,
                Some(Message::Retry),
                tr::conn_retry(),
            ));
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
        let (glyph, gcolor) = agent_glyph(s.agent);
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

        let (glyph, gcolor) = agent_glyph(s.agent);
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
                        tr::start_agent(&i18n::agent_label(s.agent))
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

    // ------------------------------------------------------------------ detail

    fn detail(&self) -> El<'_> {
        if self.ui.detail_collapsed {
            return container(
                column![icon_button(
                    I_EXPAND,
                    Some(Message::ToggleDetail),
                    tr::show_detail()
                )]
                .padding([6, 2]),
            )
            .width(30)
            .height(Fill)
            .style(style::bg(style::HEADER))
            .boxed();
        }
        let portion = 1000u16.saturating_sub((self.ui.split_ratio * 1000.0).round() as u16);
        let tabs = container(
            row![
                button(text(tr::tab_web()).size(12))
                    .padding([5, 14])
                    .style(style::tab(true)),
                space().width(Fill),
                icon_button(I_COLLAPSE, Some(Message::ToggleDetail), tr::hide_detail()),
            ]
            .align_y(Alignment::End),
        )
        .padding(iced::Padding {
            top: 4.0,
            right: 6.0,
            bottom: 0.0,
            left: 6.0,
        })
        .width(Fill)
        .style(style::bg(style::HEADER));

        let content: El<'_> = match self.selected_live() {
            Some(live) => {
                let bar = row![
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
                ]
                .spacing(4)
                .padding([6, 8])
                .align_y(Alignment::Center);
                let page: El<'_> = match &live.web {
                    // Under a modal the page stays visible as a frozen, dimmed snapshot of its
                    // last frame (no input reaches it, CEF stops rendering); the backdrop and
                    // the dialog are drawn above it (see docs, "modal z-order").
                    Some(w) => WebViewWidget::new(w)
                        .width(Fill)
                        .height(Fill)
                        .dimmed(self.web_covered())
                        .into(),
                    None => container(text(live.error.as_deref().unwrap_or("")).size(12))
                        .center(Fill)
                        .boxed(),
                };
                column![bar, page].boxed()
            }
            None => container(text(tr::no_session()).size(13).color(style::DIM))
                .center(Fill)
                .boxed(),
        };
        container(column![tabs, content])
            .width(FillPortion(portion.max(1)))
            .height(Fill)
            .style(style::bg(style::PANEL))
            .boxed()
    }

    /// A modal covers the window: the web view shows a dimmed, frozen snapshot (see
    /// [`App::detail`]).
    pub fn web_covered(&self) -> bool {
        self.modal.is_some() && !self.modal_web_unsafe
    }

    // ------------------------------------------------------------------ modals

    fn modal_view<'a>(&'a self, modal: &'a Modal) -> El<'a> {
        let card = match modal {
            Modal::NewSession(f) => {
                let names: Vec<String> = self.profiles.iter().map(|p| p.name.clone()).collect();
                let agents = AGENTS.iter().fold(row![].spacing(6), |r, a| {
                    let (g, c) = agent_glyph(*a);
                    r.push(
                        button(
                            row![
                                text(g).color(c).size(13),
                                text(i18n::agent_label(*a)).size(13)
                            ]
                            .spacing(6),
                        )
                        .padding([5, 12])
                        .style(style::choice(f.agent == *a))
                        .on_press(Message::NewAgent(*a))
                        .boxed(),
                    )
                });
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
                        button(text(tr::browse()).size(13))
                            .padding([6, 12])
                            .style(style::secondary)
                            .on_press(Message::NewBrowse),
                    ]
                    .spacing(6),
                    label(tr::profile_label()),
                    pick_list(Some(f.profile.clone()), names, |s: &String| s.clone())
                        .on_select(Message::NewProfile)
                        .text_size(13)
                        .padding([5, 8])
                        .width(Fill),
                    label(tr::agent_label()),
                    agents,
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
            Modal::Settings(f) => {
                let online = self.connected();
                let mut list: Column<El<'_>> = Column::new().spacing(4);
                for p in &self.profiles {
                    let (total, running) = self.sessions_in_profile(&p.name);
                    let row_el: El<'_> = match &f.renaming {
                        Some((old, new)) if old == &p.name => row![
                            text_input(tr::profile_rename_placeholder(), new)
                                .on_input(Message::SettingsRenameInput)
                                .on_submit(Message::SettingsRenameCommit)
                                .size(13)
                                .padding([4, 8])
                                .style(style::input)
                                .width(Fill),
                            button(text(tr::save()).size(12))
                                .padding([4, 10])
                                .style(style::primary)
                                .on_press_maybe(online.then_some(Message::SettingsRenameCommit)),
                        ]
                        .spacing(6)
                        .boxed(),
                        _ => row![
                            text(p.name.as_str()).size(13).width(Fill),
                            text(tr::profile_sessions(
                                i18n::count(total),
                                i18n::count(running)
                            ))
                            .size(11)
                            .color(style::DIM),
                            icon_button(
                                I_EDIT,
                                online.then(|| Message::SettingsStartRename(p.name.clone())),
                                tr::rename()
                            ),
                            icon_button(
                                I_DELETE,
                                online.then(|| Message::SettingsDelete(p.name.clone())),
                                tr::profile_delete_tooltip()
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
                let mut c = column![
                    text(tr::settings()).size(18).font(style::BOLD),
                    label(tr::language_label()),
                    languages,
                    space().height(4),
                    label(tr::profiles_label()),
                    text(tr::profiles_note()).size(12).color(style::DIM),
                    list,
                    row![
                        text_input(tr::profile_name_placeholder(), &f.new_name)
                            .id(PROFILE_NAME_ID)
                            .on_input(Message::SettingsNewName)
                            .on_submit(Message::SettingsCreate)
                            .size(13)
                            .padding([5, 8])
                            .style(style::input)
                            .width(Fill),
                        button(text(tr::add()).size(13))
                            .padding([5, 12])
                            .style(style::secondary)
                            .on_press_maybe((online && !f.busy).then_some(Message::SettingsCreate)),
                    ]
                    .spacing(6),
                ]
                .spacing(10);
                if let Some(e) = &f.error {
                    c = c.push(error_box(e));
                }
                c = c.push(space().height(4).boxed());
                c = c.push(label(tr::server_label()));
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
                c = c.push(label(tr::on_close_label()));
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
                c = c.push(on_close.boxed());
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::close()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press(Message::CloseModal),
                    ]
                    .boxed(),
                );
                c.width(480).boxed()
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
