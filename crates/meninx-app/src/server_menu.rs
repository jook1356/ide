//! The server list in one app (`docs/multi-server.md`): the title bar's server menu, the
//! settings page's server list, and what an `App` does when the shell shows or hides it.

use iced::widget::{Column, button, column, container, mouse_area, pin, row, space, stack, text, text_input};
use iced::{Alignment, Color, Element, Fill, Task, Widget as _};
use meninx_client::AgentActivity;

use crate::app::{App, Conn, Message, SSH_HOST_ID};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::shell::{LinkState, ServerBrief};
use crate::style;
use crate::ui_state::{LOCAL_SERVER, ServerEntry, ServerPref, new_server_id};

type El<'a> = Element<'a, Message>;

const DOT: &str = "●";
const MENU_W: f32 = 300.0;
/// Where the menu opens: under the title bar, below its button.
const MENU_X: f32 = 64.0;

fn state_color(s: LinkState) -> Color {
    match s {
        LinkState::Connected => style::GREEN,
        LinkState::Connecting => style::AMBER,
        LinkState::Lost => style::RED,
        LinkState::Off => style::FAINT,
    }
}

impl App {
    /// The server's name: Local's in the UI language (it follows a change), else the list's.
    pub fn display_name(&self) -> String {
        if self.server_id == LOCAL_SERVER && self.target.local {
            tr::conn_local()
        } else {
            self.server_name.clone()
        }
    }

    /// This server's line in the server menu.
    pub fn brief(&self) -> ServerBrief {
        let state = match &self.conn {
            Conn::Connected(_) => LinkState::Connected,
            Conn::Connecting | Conn::Reconnecting => LinkState::Connecting,
            Conn::Disconnected(_) if self.link.is_some() => LinkState::Lost,
            Conn::Disconnected(_) => LinkState::Off,
        };
        let count = |a: AgentActivity| {
            self.sessions
                .iter()
                .filter(|l| l.state.is_alive() && l.info.activity == a)
                .count()
        };
        ServerBrief {
            id: self.server_id.clone(),
            name: self.display_name(),
            detail: self.server_label(),
            state,
            working: count(AgentActivity::Working),
            waiting: count(AgentActivity::NeedsInput),
            fresh: self.new_answers.len(),
        }
    }

    /// Web view `view` is one of this server's tabs (its events and requests come here).
    pub fn owns_view(&self, view: meninx_web::ViewId) -> bool {
        self.sessions
            .iter()
            .flat_map(|l| l.browsers.iter())
            .chain(self.global_browsers.iter())
            .any(|b| b.view.as_ref().is_some_and(|v| v.id() == view))
    }

    /// Another server is shown: close menus, hide the web views, close the detail window
    /// (it stays detached in the UI state and reopens when this server is shown again).
    pub fn suspend(&mut self) -> Task<Message> {
        self.shown = false;
        self.server_menu = false;
        self.session_menu = None;
        self.item_menu = None;
        self.focused = false;
        self.sync_browsers(None);
        match self.detail_window.take() {
            Some(id) => {
                self.detail_window_ready = false;
                crate::detail_window::save_placement(id).chain(iced::window::close(id))
            }
            None => Task::none(),
        }
    }

    /// This server is shown again (after [`App::suspend`]): its web view, detail area and
    /// detail window.
    pub fn resume(&mut self) -> Task<Message> {
        self.shown = true;
        let shown = self.shown_browser();
        self.sync_browsers(shown);
        for l in &mut self.sessions {
            l.detail.stale = true;
        }
        Task::batch([self.sync_detail(), self.sync_detail_window(), self.after_select()])
    }

    /// Server menu and settings -> servers messages (the shell takes `PickServer` and
    /// `ServerRetry` for other servers first).
    pub(crate) fn on_server_msg(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::ServerMenu(open) => self.server_menu = open,
            Message::PickServer(_) => self.server_menu = false,
            Message::ServerRetry(id) => {
                self.server_menu = false;
                if id == self.server_id {
                    return self.update(Message::Retry);
                }
            }
            Message::ServerManage => {
                self.server_menu = false;
                return self.update(Message::Page(crate::ui_state::Page::Settings));
            }
            Message::ServerEdit(id) => {
                let entry = id
                    .as_deref()
                    .and_then(|id| self.ui.servers.iter().find(|e| e.id == id))
                    .cloned();
                let focus = id.is_some();
                self.settings.edit_server(id, entry.as_ref());
                if focus {
                    return iced::widget::operation::focus(SSH_HOST_ID);
                }
            }
            Message::SettingsSshName(v) => self.settings.ssh_name = v,
            Message::SettingsSshHost(v) => self.settings.ssh_host = v,
            Message::SettingsSshCommand(v) => self.settings.ssh_command = v,
            Message::SettingsApplyServer => {
                let Some(id) = self.settings.server_edit.clone() else {
                    return Task::none();
                };
                let pref = match self.settings.server_pref() {
                    Ok(p) => p,
                    Err(e) => {
                        self.settings.server_error = Some(e);
                        return Task::none();
                    }
                };
                let name = match self.settings.ssh_name.trim() {
                    "" => self.settings.ssh_host.trim().to_owned(),
                    n => n.to_owned(),
                };
                crate::script::log(&format!("server list: {id:?} = {name} {pref:?}"));
                if id.is_empty() {
                    let id = new_server_id(&name, &self.ui.servers);
                    self.ui.servers.push(ServerEntry {
                        id,
                        name,
                        pref,
                        auto_connect: true,
                    });
                } else if let Some(e) = self.ui.servers.iter_mut().find(|e| e.id == id) {
                    e.name = name;
                    e.pref = pref;
                }
                self.settings.edit_server(None, None);
                self.save_ui();
            }
            Message::ServerRemove(id) => {
                crate::script::log(&format!("server list: remove {id}"));
                self.ui.servers.retain(|e| e.id != id);
                if self.ui.current_server == id {
                    self.ui.current_server = LOCAL_SERVER.to_owned();
                }
                if self.settings.server_edit.as_deref() == Some(id.as_str()) {
                    self.settings.edit_server(None, None);
                }
                self.save_ui();
            }
            Message::ServerAutoConnect(id, on) => {
                if let Some(e) = self.ui.servers.iter_mut().find(|e| e.id == id) {
                    e.auto_connect = on;
                }
                self.save_ui();
            }
            _ => {}
        }
        Task::none()
    }

    // ------------------------------------------------------------------ title bar

    /// The title bar's button: the shown server's name and connection, a dot when another
    /// server has a session waiting for the user or a new answer.
    pub(crate) fn server_button(&self) -> El<'_> {
        let others = self
            .servers_brief
            .iter()
            .filter(|b| b.id != self.server_id)
            .any(|b| b.waiting > 0 || b.fresh > 0);
        let mut r = row![
            text(DOT).size(9).color(self.conn_color()),
            text(self.display_name())
                .size(12)
                .color(style::TEXT)
                .wrapping(text::Wrapping::None),
            icons::icon(Icon::ChevronDown, 12.0, style::DIM),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if others {
            r = r.push(text(DOT).size(8).color(style::AMBER).boxed());
        }
        button(r)
            .padding([3, 8])
            .style(style::row(self.server_menu))
            .on_press(Message::ServerMenu(!self.server_menu))
            .boxed()
    }

    /// The open server menu, in window coordinates.
    pub(crate) fn server_menu_layer(&self) -> El<'_> {
        if !self.server_menu {
            return space().boxed();
        }
        let mut items: Vec<El<'_>> = Vec::new();
        for b in &self.servers_brief {
            let shown = b.id == self.server_id;
            let status = match b.state {
                LinkState::Connected if b.working + b.waiting + b.fresh > 0 => {
                    tr::server_counts(b.working as i64, b.waiting as i64, b.fresh as i64)
                }
                LinkState::Connected => tr::conn_state_connected(),
                LinkState::Connecting => tr::conn_state_connecting(),
                LinkState::Lost => tr::conn_state_disconnected(),
                LinkState::Off => tr::server_state_off(),
            };
            let mut line = row![
                text(DOT).size(10).color(state_color(b.state)),
                column![
                    text(b.name.as_str())
                        .size(13)
                        .font(if shown { style::bold() } else { iced::Font::default() })
                        .wrapping(text::Wrapping::None),
                    text(status).size(11).color(if b.waiting > 0 {
                        style::AMBER
                    } else {
                        style::DIM
                    }),
                ]
                .spacing(1)
                .width(Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if b.state == LinkState::Lost {
                line = line.push(
                    button(text(tr::conn_retry()).size(11))
                        .padding([2, 8])
                        .style(style::secondary)
                        .on_press(Message::ServerRetry(b.id.clone()))
                        .boxed(),
                );
            }
            items.push(
                button(line)
                    .padding([5, 10])
                    .width(Fill)
                    .style(style::row(shown))
                    .on_press(Message::PickServer(b.id.clone()))
                    .boxed(),
            );
        }
        items.push(
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER))
                .boxed(),
        );
        items.push(
            button(text(tr::server_manage()).size(13))
                .padding([6, 10])
                .width(Fill)
                .style(style::row(false))
                .on_press(Message::ServerManage)
                .boxed(),
        );
        let card = container(Column::with_children(items).spacing(1))
            .padding(4)
            .width(MENU_W)
            .style(style::modal_card);
        let backdrop = mouse_area(space().width(Fill).height(Fill))
            .on_press(Message::ServerMenu(false))
            .on_right_press(Message::ServerMenu(false));
        stack![
            backdrop,
            pin(card).x(MENU_X).y(crate::frame::BAR_H).width(Fill).height(Fill)
        ]
        .width(Fill)
        .height(Fill)
        .boxed()
    }

    // ------------------------------------------------------------------ settings page

    /// Settings -> server: Local, then the SSH servers (auto-connect, edit, remove), and the
    /// add / edit form.
    pub(crate) fn server_list<'a>(&'a self) -> El<'a> {
        let f = &self.settings;
        let state_of = |id: &str| {
            self.servers_brief
                .iter()
                .find(|b| b.id == id)
                .map_or(LinkState::Off, |b| b.state)
        };
        let entry_row = |id: &str, name: String, detail: String, actions: El<'static>| -> El<'_> {
            row![
                text(DOT).size(10).color(state_color(state_of(id))),
                column![
                    text(name).size(13).wrapping(text::Wrapping::WordOrGlyph),
                    text(detail)
                        .size(11)
                        .color(style::DIM)
                        .wrapping(text::Wrapping::WordOrGlyph),
                ]
                .spacing(1)
                .width(Fill),
                actions,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed()
        };
        let small = |label: String, style: fn(&iced::Theme, button::Status) -> button::Style, msg: Message| {
            button(text(label).size(12)).padding([4, 10]).style(style).on_press(msg)
        };
        let mut c = column![
            text(tr::server_list_note())
                .size(12)
                .color(style::DIM)
                .wrapping(text::Wrapping::WordOrGlyph),
            entry_row(LOCAL_SERVER, tr::conn_local(), tr::server_local_detail(), space().boxed()),
        ]
        .spacing(8);
        // `MENINX_REMOTE` replaces the list (its one `App` is not Local): nothing to edit.
        if self.server_id == LOCAL_SERVER && !self.target.local {
            return c
                .push(
                    text(tr::server_env_override(&self.target.remote))
                        .size(12)
                        .color(style::AMBER)
                        .boxed(),
                )
                .boxed();
        }
        for e in &self.ui.servers {
            let ServerPref::Ssh { host, .. } = &e.pref else { continue };
            let actions = row![
                button(text(tr::server_auto_connect()).size(12))
                    .padding([4, 10])
                    .style(style::choice(e.auto_connect))
                    .on_press(Message::ServerAutoConnect(e.id.clone(), !e.auto_connect)),
                small(tr::server_edit(), style::secondary, Message::ServerEdit(Some(e.id.clone()))),
                small(tr::remove(), style::danger, Message::ServerRemove(e.id.clone())),
            ]
            .spacing(6)
            .boxed();
            c = c.push(entry_row(&e.id, e.name.clone(), format!("ssh {host}"), actions));
        }
        match &f.server_edit {
            None => {
                c = c.push(row![
                    space().width(Fill),
                    small(tr::server_add(), style::secondary, Message::ServerEdit(Some(String::new()))),
                ].boxed());
            }
            Some(_) => {
                let input = |placeholder: String, value: &'a str, on: fn(String) -> Message| {
                    text_input(placeholder, value)
                        .on_input(on)
                        .on_submit(Message::SettingsApplyServer)
                        .size(13)
                        .padding([5, 8])
                        .style(style::input)
                };
                c = c.push(
                    column![
                        input(tr::ssh_host_placeholder(), &f.ssh_host, Message::SettingsSshHost)
                            .id(SSH_HOST_ID),
                        input(tr::server_name_placeholder(), &f.ssh_name, Message::SettingsSshName),
                        input(tr::ssh_command_placeholder(), &f.ssh_command, Message::SettingsSshCommand),
                        row![
                            space().width(Fill),
                            small(tr::cancel(), style::secondary, Message::ServerEdit(None)),
                            small(tr::save(), style::primary, Message::SettingsApplyServer),
                        ]
                        .spacing(6),
                    ]
                    .spacing(6)
                    .boxed(),
                );
                if let Some(e) = &f.server_error {
                    c = c.push(crate::view::error_box(e));
                }
            }
        }
        c.boxed()
    }
}
