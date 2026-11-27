//! The server list in one app (`docs/multi-server.md`): the title bar's server menu, the
//! settings page's server list, and what an `App` does when the shell shows or hides it.

use iced::widget::{Column, button, column, container, mouse_area, pin, row, space, stack, text, text_input};
use iced::{Alignment, Color, Element, Fill, Task, Widget as _};
use meninx_client::AgentActivity;

use crate::app::{App, Conn, Message, Modal, SSH_HOST_ID};
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
            Message::ServerEdit(None) => {
                if matches!(self.modal, Some(Modal::Server(_))) {
                    self.modal = None;
                }
            }
            Message::ServerEdit(Some(id)) => {
                let entry = self.ui.servers.iter().find(|e| e.id == id);
                self.modal = Some(Modal::Server(ServerForm::new(id.clone(), entry)));
                return iced::widget::operation::focus(SSH_HOST_ID);
            }
            Message::ServerRemove(id) => {
                crate::script::log(&format!("server list: remove {id}"));
                self.ui.servers.retain(|e| e.id != id);
                if self.ui.current_server == id {
                    self.ui.current_server = LOCAL_SERVER.to_owned();
                }
                self.save_ui();
            }
            Message::ServerAutoConnect(id, on) => {
                if let Some(e) = self.ui.servers.iter_mut().find(|e| e.id == id) {
                    e.auto_connect = on;
                }
                self.save_ui();
            }
            Message::ServerTested(n, t) => {
                if let Some(Modal::Server(f)) = &mut self.modal
                    && f.test.run() == Some(n)
                {
                    f.test = t;
                }
            }
            m => return self.on_server_form(m),
        }
        Task::none()
    }

    /// The server dialog's own messages (it is open).
    fn on_server_form(&mut self, m: Message) -> Task<Message> {
        let Some(Modal::Server(f)) = &mut self.modal else {
            return Task::none();
        };
        match m {
            Message::SettingsSshName(v) => f.name = v,
            Message::SettingsSshHost(v) => {
                f.host = v;
                f.error = None;
                f.test = ServerTest::Idle;
            }
            Message::SettingsSshCommand(v) => {
                f.command = v;
                f.test = ServerTest::Idle;
            }
            Message::ServerFormAdvanced => f.advanced = !f.advanced,
            Message::ServerFormAuto(on) => f.auto_connect = on,
            Message::ServerTest | Message::ServerInstall => {
                let pref = match f.pref() {
                    Ok(p) => p,
                    Err(e) => {
                        f.error = Some(e);
                        return Task::none();
                    }
                };
                let n = next_test();
                let install = match (&m, &f.test) {
                    (Message::ServerInstall, ServerTest::Missing { platform, .. }) => Some((platform.clone(), false)),
                    (Message::ServerInstall, ServerTest::Differs { platform, incompatible, .. }) => {
                        Some((platform.clone(), *incompatible))
                    }
                    (Message::ServerInstall, _) => return Task::none(),
                    _ => None,
                };
                f.test = if install.is_some() { ServerTest::Installing(n) } else { ServerTest::Running(n) };
                return Task::perform(check_server(pref, install), move |t| Message::ServerTested(n, t));
            }
            Message::SettingsApplyServer => {
                let pref = match f.pref() {
                    Ok(p) => p,
                    Err(e) => {
                        f.error = Some(e);
                        return Task::none();
                    }
                };
                let name = match f.name.trim() {
                    "" => f.host.trim().to_owned(),
                    n => n.to_owned(),
                };
                let (id, auto_connect) = (f.id.clone(), f.auto_connect);
                crate::script::log(&format!("server list: {id:?} = {name} {pref:?}"));
                if id.is_empty() {
                    let id = new_server_id(&name, &self.ui.servers);
                    self.ui.servers.push(ServerEntry {
                        id,
                        name,
                        pref,
                        auto_connect,
                    });
                } else if let Some(e) = self.ui.servers.iter_mut().find(|e| e.id == id) {
                    e.name = name;
                    e.pref = pref;
                    e.auto_connect = auto_connect;
                }
                self.modal = None;
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
        c = c.push(
            row![
                space().width(Fill),
                small(tr::server_add(), style::secondary, Message::ServerEdit(Some(String::new()))),
            ]
            .boxed(),
        );
        c.boxed()
    }

    /// The server dialog (A안): host and name with labels and hints, the server command under
    /// 고급, auto-connect, a connection test, then 추가 / 저장.
    pub(crate) fn server_dialog<'a>(&'a self, f: &'a ServerForm) -> El<'a> {
        let field = |label: String, input: El<'a>, hint: Option<String>| -> El<'a> {
            let mut c = column![text(label).size(12).color(style::DIM), input].spacing(4);
            if let Some(h) = hint {
                c = c.push(
                    text(h)
                        .size(11)
                        .color(style::FAINT)
                        .wrapping(text::Wrapping::WordOrGlyph)
                        .boxed(),
                );
            }
            c.boxed()
        };
        let input = |placeholder: String, value: &'a str, on: fn(String) -> Message| {
            text_input(placeholder, value)
                .on_input(on)
                .on_submit(Message::SettingsApplyServer)
                .size(13)
                .padding([6, 9])
                .style(style::input)
        };
        let new = f.id.is_empty();
        let host_placeholder = tr::ssh_host_placeholder();
        let name_placeholder = match f.host.trim() {
            "" => tr::server_name_placeholder(),
            h => h.to_owned(),
        };
        let mut c = column![
            column![
                text(if new { tr::server_add() } else { tr::server_edit_title() })
                    .size(18)
                    .font(style::bold()),
                text(tr::server_dialog_sub()).size(12).color(style::DIM),
            ]
            .spacing(2),
            field(
                tr::server_host_label(),
                input(host_placeholder, &f.host, Message::SettingsSshHost)
                    .id(SSH_HOST_ID)
                    .boxed(),
                Some(tr::server_host_hint()),
            ),
            field(
                tr::server_name_label(),
                input(name_placeholder, &f.name, Message::SettingsSshName).boxed(),
                Some(tr::server_name_hint()),
            ),
            button(
                row![
                    icons::icon(
                        if f.advanced { Icon::ChevronDown } else { Icon::ChevronRight },
                        12.0,
                        style::DIM
                    ),
                    text(tr::server_advanced()).size(12).color(style::DIM),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([2, 0])
            .style(style::ghost_choice(false))
            .on_press(Message::ServerFormAdvanced),
        ]
        .spacing(12);
        if f.advanced {
            c = c.push(field(
                tr::server_command_label(),
                input(tr::ssh_command_placeholder(), &f.command, Message::SettingsSshCommand).boxed(),
                None,
            ));
        }
        c = c.push(
            iced::widget::checkbox(f.auto_connect)
                .label(tr::server_auto_connect_hint())
                .on_toggle(Message::ServerFormAuto)
                .size(15)
                .text_size(13)
                .boxed(),
        );
        if let Some(e) = &f.error {
            c = c.push(crate::view::error_box(e));
        }
        // The check's result, and 설치 / 업데이트 when it offers one.
        let (color, status, action) = match &f.test {
            ServerTest::Idle => (style::FAINT, String::new(), None),
            ServerTest::Running(_) => (style::AMBER, tr::server_testing(), None),
            ServerTest::Installing(_) => (style::AMBER, tr::server_installing(), None),
            ServerTest::Ok(v) => (style::GREEN, tr::conn_connected(v), None),
            ServerTest::Missing { platform, bundled: true } => {
                (style::AMBER, tr::server_missing(platform), Some(tr::server_install()))
            }
            ServerTest::Missing { platform, bundled: false } => {
                (style::RED, tr::server_no_build(platform), None)
            }
            ServerTest::Differs { version, incompatible: false, .. } => {
                (style::AMBER, tr::server_differs(version), Some(tr::server_update()))
            }
            ServerTest::Differs { version, incompatible: true, .. } => {
                (style::RED, tr::server_incompatible(version), Some(tr::server_update()))
            }
            ServerTest::Failed(e) => (style::RED, tr::server_test_failed(e), None),
        };
        let testing = f.test.run().is_some();
        let mut test_row = row![].spacing(8).align_y(Alignment::Center);
        if !status.is_empty() {
            test_row = test_row.push(text(DOT).size(10).color(color).boxed());
        }
        c = c.push(
            container(space()).height(1).width(Fill).style(style::bg(style::BORDER)).boxed(),
        );
        c = c.push(
            test_row
                .push(
                    text(status)
                        .size(12)
                        .color(style::DIM)
                        .wrapping(text::Wrapping::WordOrGlyph)
                        .width(Fill)
                        .boxed(),
                )
                .push(
                    button(text(tr::server_test()).size(12))
                        .padding([4, 10])
                        .style(style::secondary)
                        .on_press_maybe((!testing).then_some(Message::ServerTest))
                        .boxed(),
                )
                .push(match action {
                    Some(label) => button(text(label).size(12))
                        .padding([4, 10])
                        .style(style::primary)
                        .on_press(Message::ServerInstall)
                        .boxed(),
                    None => space().boxed(),
                })
                .boxed(),
        );
        c = c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::ServerEdit(None)),
                button(text(if new { tr::add() } else { tr::save() }).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press(Message::SettingsApplyServer),
            ]
            .spacing(8)
            .boxed(),
        );
        c.width(440).boxed()
    }
}

/// The server dialog's state (`Modal::Server`).
#[derive(Debug, Default)]
pub struct ServerForm {
    /// The entry edited (`""` = a new one).
    pub id: String,
    pub name: String,
    pub host: String,
    pub command: String,
    pub auto_connect: bool,
    /// 고급 (the server command) is unfolded.
    pub advanced: bool,
    pub error: Option<String>,
    pub test: ServerTest,
}

/// The dialog's connection check (`docs/remote-install.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ServerTest {
    #[default]
    Idle,
    /// Check `n` runs (an older check's result is ignored).
    Running(u64),
    /// Install / update `n` runs, then the check.
    Installing(u64),
    /// Connected to this app's build (or a server not installed by Meninx): its version.
    Ok(String),
    /// No server in `~/.meninx/bin` and none answered; `bundled`: the app has a build for
    /// `platform`.
    Missing { platform: String, bundled: bool },
    /// Another build is installed: `incompatible` when it does not speak this app's protocol
    /// (updating restarts the daemon: its sessions stop).
    Differs {
        platform: String,
        version: String,
        incompatible: bool,
    },
    Failed(String),
}

impl ServerTest {
    /// The check or install that runs.
    pub fn run(&self) -> Option<u64> {
        match self {
            ServerTest::Running(n) | ServerTest::Installing(n) => Some(*n),
            _ => None,
        }
    }
}

/// The app's Linux build of the server for `platform` (`linux-x86_64`, ...):
/// `<app folder>/remote/<platform>/meninx-server`, or under `MENINX_REMOTE_SERVERS`.
pub fn bundled_server(platform: &str) -> Option<std::path::PathBuf> {
    let base = match std::env::var_os("MENINX_REMOTE_SERVERS").filter(|v| !v.is_empty()) {
        Some(d) => std::path::PathBuf::from(d),
        None => std::env::current_exe().ok()?.parent()?.join("remote"),
    };
    let p = base.join(platform).join("meninx-server");
    p.is_file().then_some(p)
}

/// Check server `pref` (installing first when `install` names the platform, restarting the
/// daemon too when the installed one was incompatible): what is installed, and whether this
/// app can talk to it.
async fn check_server(pref: ServerPref, install: Option<(String, bool)>) -> ServerTest {
    use meninx_client::remote_install as ri;
    let ServerPref::Ssh { host, command } = &pref else {
        return ServerTest::Failed(String::new());
    };
    let ssh = crate::app::ssh_for(host, command);
    if let Some((platform, restart)) = install {
        let Some(path) = bundled_server(&platform) else {
            return ServerTest::Failed(tr::server_no_build(&platform));
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return ServerTest::Failed(format!("{}: {e}", path.display())),
        };
        if let Err(e) = ri::install(&ssh, bytes).await {
            return ServerTest::Failed(e.to_string());
        }
        if restart && let Err(e) = ri::restart(&ssh).await {
            return ServerTest::Failed(e.to_string());
        }
    }
    // A custom server command is the user's: only the connection is checked.
    let probe = if command.is_empty() {
        match ri::probe(&ssh).await {
            Ok(p) => Some(p),
            Err(e) => return ServerTest::Failed(e.to_string()),
        }
    } else {
        None
    };
    let same_build = |p: &ri::Probe| {
        let Some(i) = &p.installed else { return true };
        match bundled_server(&p.platform).and_then(|b| std::fs::read(b).ok()) {
            Some(bytes) => ri::cksum(&bytes) == i.cksum,
            // No build to compare with: nothing to offer.
            None => true,
        }
    };
    let opts = meninx_client::ClientOptions {
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        ..meninx_client::ClientOptions::default()
    };
    // Dropped right after: the check keeps no connection.
    let connected = meninx_client::Client::connect(meninx_client::Switchable::new(ssh.clone()), opts)
        .await
        .map(|c| c.welcome().map(|w| w.server_version).unwrap_or_default());
    match (probe, connected) {
        (Some(p), Ok(v)) if !same_build(&p) => ServerTest::Differs {
            platform: p.platform,
            version: v,
            incompatible: false,
        },
        // Ours, or a server the user installed (`meninx-server` on `PATH`).
        (_, Ok(v)) => ServerTest::Ok(v),
        (Some(p), Err(_)) if p.installed.is_none() => {
            let bundled = bundled_server(&p.platform).is_some();
            ServerTest::Missing {
                platform: p.platform,
                bundled,
            }
        }
        (Some(p), Err(e)) if e.id == "proto-version" => ServerTest::Differs {
            version: p.installed.map(|i| i.version).unwrap_or_default(),
            platform: p.platform,
            incompatible: true,
        },
        (_, Err(e)) => ServerTest::Failed(crate::i18n::proto_error(&e)),
    }
}

impl ServerForm {
    fn new(id: String, entry: Option<&ServerEntry>) -> Self {
        let mut f = ServerForm {
            id,
            auto_connect: true,
            ..Default::default()
        };
        if let Some(e) = entry {
            f.name = e.name.clone();
            f.auto_connect = e.auto_connect;
            if let ServerPref::Ssh { host, command } = &e.pref {
                f.host = host.clone();
                f.command = command.clone();
                f.advanced = !command.is_empty();
            }
        }
        f
    }

    /// The edited server (`Err` = localized validation message).
    fn pref(&self) -> Result<ServerPref, String> {
        let host = self.host.trim();
        if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
            return Err(tr::err_ssh_host());
        }
        Ok(ServerPref::Ssh {
            host: host.to_owned(),
            command: self.command.trim().to_owned(),
        })
    }
}

fn next_test() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}
