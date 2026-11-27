//! Several servers in one window (`docs/multi-server.md`): one [`App`] per server (Local
//! first, then the list of `ui.json`), all connected (unless auto-connect is off), one shown.
//!
//! Every `App` message is tagged with its `App`'s serial, so replies reach the `App` that asked
//! also after a switch, and a rebuilt `App` (new serial) ignores the old one's. Web view events
//! and widget / preview requests are subscribed to once here and go to the `App` that owns the
//! view. Window and keyboard events reach only the shown `App` (its foreground
//! subscriptions). The PC-wide UI fields (`UiState::copy_globals`) are copied from the `App`
//! that changed them to the others; Local's `ui.json` is their record.

use std::path::PathBuf;

use iced::{Size, Subscription, Task, Widget as _, window};
use meninx_web::{ViewId, WebRuntime, WebViewEvent};

use crate::app::{App, Message, build_target};
use crate::i18n::tr;
use crate::ui_state::{CloseAction, LOCAL_SERVER, ServerEntry, ServerPref, UiState};

/// A server's connection, for the server menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Connected,
    /// Connecting or reconnecting.
    Connecting,
    /// Disconnected with a client (it retries, or the retry button does).
    Lost,
    /// No client: auto-connect is off, or the server was stopped.
    Off,
}

/// One line of the server menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerBrief {
    pub id: String,
    pub name: String,
    /// `Local` or `ssh <host>`.
    pub detail: String,
    pub state: LinkState,
    /// Running agent sessions working / waiting for the user, and new answers not seen yet.
    pub working: usize,
    pub waiting: usize,
    pub fresh: usize,
}

#[derive(Debug, Clone)]
pub enum ShellMsg {
    /// A message of the `App` with this serial.
    App(u64, Message),
    Web(WebViewEvent),
    /// A widget or preview page's request (`Message::Widget` / `Message::Preview`).
    Routed(Message),
    WindowOpened(window::Id),
}

struct Slot {
    serial: u64,
    /// The list entry (`None` for Local, or the one server of `MENINX_REMOTE`).
    entry: Option<ServerEntry>,
    app: App,
}

pub struct Shell {
    data_dir: PathBuf,
    runtime: WebRuntime,
    /// Local first, then the list's order.
    slots: Vec<Slot>,
    /// The shown one.
    cur: usize,
    next_serial: u64,
    /// `MENINX_REMOTE`: that server alone; the list is not used.
    single: bool,
    /// The PC-wide UI fields as last shared.
    globals: UiState,
}

impl Shell {
    pub fn boot(data_dir: PathBuf, runtime: WebRuntime, remote: Option<String>) -> (Self, Task<ShellMsg>) {
        crate::widgets_ui::install();
        crate::previews_ui::install();
        // Self-update: a run that started this app learns it came up.
        crate::self_update::confirm_started(&data_dir);
        let ui = UiState::load(&data_dir);
        meninx_i18n::apply(ui.language);
        let mut shell = Self {
            data_dir,
            runtime,
            slots: Vec::new(),
            cur: 0,
            next_serial: 0,
            single: remote.is_some(),
            globals: ui.clone(),
        };
        if let Some(r) = &remote {
            let target = build_target(&ServerPref::Local, Some(r));
            let name = target.remote.clone();
            let slot = shell.new_slot(None, (LOCAL_SERVER.into(), name), target, true);
            shell.slots.push(slot);
        } else {
            let local = build_target(&ServerPref::Local, None);
            let slot = shell.new_slot(None, (LOCAL_SERVER.into(), tr::conn_local()), local, true);
            shell.slots.push(slot);
            for e in &ui.servers {
                let connect = e.auto_connect || e.id == ui.current_server;
                let target = build_target(&e.pref, None);
                let slot = shell.new_slot(Some(e.clone()), (e.id.clone(), e.name.clone()), target, connect);
                shell.slots.push(slot);
            }
            shell.cur = shell.index_of(&ui.current_server).unwrap_or(0);
        }
        shell.mark_shown();
        shell.refresh_briefs();
        // The app window (a daemon opens none by itself, frameless like the detail window);
        // closing it is handled by the shown `App`.
        let (_, open) = window::open(crate::frame::frameless(window::Settings {
            size: Size::new(1440.0, 880.0),
            min_size: Some(Size::new(640.0, 400.0)),
            exit_on_close_request: false,
            ..window::Settings::default()
        }));
        (shell, open.map(ShellMsg::WindowOpened))
    }

    fn new_slot(&mut self, entry: Option<ServerEntry>, server: (String, String), target: crate::app::Target, connect: bool) -> Slot {
        let mut app = App::start(self.data_dir.clone(), self.runtime.clone(), server, target, connect);
        // Local's has the window when another server is shown too (`reconcile`).
        if let Some(first) = self.slots.first() {
            take_window(&first.app, &mut app);
            app.focused = false;
        }
        self.next_serial += 1;
        Slot {
            serial: self.next_serial,
            entry,
            app,
        }
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.app.server_id == id)
    }

    fn owner(&self, view: ViewId) -> usize {
        self.slots
            .iter()
            .position(|s| s.app.owns_view(view))
            .unwrap_or(self.cur)
    }

    pub fn update(&mut self, m: ShellMsg) -> Task<ShellMsg> {
        match m {
            ShellMsg::WindowOpened(id) => {
                for s in &mut self.slots {
                    s.app.window = Some(id);
                }
                self.deliver(self.cur, Message::WindowOpened(Some(id)))
            }
            ShellMsg::Web(ev) => {
                let k = self.owner(ev.view);
                self.deliver(k, Message::Web(ev))
            }
            ShellMsg::Routed(msg) => {
                let view = match &msg {
                    Message::Widget(crate::widgets_ui::WidgetMsg::Request(req, _))
                    | Message::Preview(crate::previews_ui::PreviewMsg::Request(req, _)) => req.view,
                    _ => None,
                };
                let k = view.map_or(self.cur, |v| self.owner(v));
                self.deliver(k, msg)
            }
            ShellMsg::App(serial, msg) => {
                // A rebuilt or removed server's late reply.
                let Some(k) = self.slots.iter().position(|s| s.serial == serial) else {
                    return Task::none();
                };
                match &msg {
                    Message::PickServer(id) if !self.single => {
                        let close = self.deliver(k, Message::ServerMenu(false));
                        let Some(j) = self.index_of(id) else { return close };
                        return Task::batch([close, self.switch_to(j)]);
                    }
                    Message::ServerRetry(id) if *id != self.slots[k].app.server_id => {
                        let close = self.deliver(k, Message::ServerMenu(false));
                        let Some(j) = self.index_of(id) else { return close };
                        return Task::batch([close, self.deliver(j, Message::Retry)]);
                    }
                    Message::WindowCloseRequest(w)
                        if k == self.cur && k != 0 && self.slots[k].app.window == Some(*w) =>
                    {
                        // Local's running sessions: its question (or stop) needs it shown.
                        let local = &self.slots[0].app;
                        let running = local.target.local
                            && local.connected()
                            && local.sessions.iter().any(|l| l.state.is_alive());
                        if running && local.ui.on_close != CloseAction::Keep {
                            let shown = self.switch_to(0);
                            return Task::batch([shown, self.deliver(0, msg)]);
                        }
                    }
                    _ => {}
                }
                self.deliver(k, msg)
            }
        }
    }

    /// Run `msg` in `App` `k`, then share what it changed (globals, the list, the shown one).
    fn deliver(&mut self, k: usize, msg: Message) -> Task<ShellMsg> {
        let serial = self.slots[k].serial;
        let task = self.slots[k].app.update(msg).map(move |m| ShellMsg::App(serial, m));
        let after = self.after_update(k);
        Task::batch([task, after])
    }

    fn after_update(&mut self, k: usize) -> Task<ShellMsg> {
        let mut tasks = Vec::new();
        if !self.single && !self.slots[k].app.ui.globals_eq(&self.globals) {
            self.globals.copy_globals(&self.slots[k].app.ui);
            self.share_globals(k);
            if self.globals.current_server != self.slots[self.cur].app.server_id {
                let j = self.index_of(&self.globals.current_server).unwrap_or(0);
                tasks.push(self.switch_to(j));
            }
            tasks.push(self.reconcile());
        }
        self.refresh_briefs();
        Task::batch(tasks)
    }

    /// Copy the globals to every `App` but `from` and record them in Local's `ui.json`.
    fn share_globals(&mut self, from: usize) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            if i != from {
                s.app.ui.copy_globals(&self.globals);
            }
        }
        self.slots[0].app.save_ui();
    }

    fn mark_shown(&mut self) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            s.app.shown = i == self.cur;
        }
    }

    fn refresh_briefs(&mut self) {
        let briefs: Vec<ServerBrief> = self.slots.iter().map(|s| s.app.brief()).collect();
        for s in &mut self.slots {
            if s.app.servers_brief != briefs {
                s.app.servers_brief = briefs.clone();
            }
        }
    }

    /// Show `App` `k`: the old one hides its views and detail window, the new one gets the
    /// window and is connected if it was not.
    fn switch_to(&mut self, k: usize) -> Task<ShellMsg> {
        if k == self.cur || k >= self.slots.len() {
            return Task::none();
        }
        let old = self.cur;
        let focused = self.slots[old].app.focused;
        let (so, sk) = (self.slots[old].serial, self.slots[k].serial);
        let hidden = self.slots[old].app.suspend().map(move |m| ShellMsg::App(so, m));
        let (from, to) = if old < k {
            let (a, b) = self.slots.split_at_mut(k);
            (&a[old].app, &mut b[0].app)
        } else {
            let (a, b) = self.slots.split_at_mut(old);
            (&b[0].app, &mut a[k].app)
        };
        take_window(from, to);
        to.focused = focused;
        crate::script::log(&format!("show server {}", to.server_id));
        if to.link.is_none() && to.target.transport.is_ok() {
            to.connect();
        }
        let shown = to.resume().map(move |m| ShellMsg::App(sk, m));
        self.cur = k;
        if !self.single {
            self.globals.current_server = self.slots[k].app.server_id.clone();
            self.slots[k].app.ui.current_server = self.globals.current_server.clone();
            self.share_globals(k);
            self.slots[k].app.save_ui();
        }
        self.refresh_briefs();
        Task::batch([hidden, shown])
    }

    /// Follow the server list: start `App`s for new entries, rebuild changed ones, drop
    /// removed ones (their sessions keep running on their servers), keep the list's order.
    fn reconcile(&mut self) -> Task<ShellMsg> {
        if self.single {
            return Task::none();
        }
        let want = self.globals.servers.clone();
        let mut tasks = Vec::new();
        // A removed shown server: Local is shown first.
        let cur_id = self.slots[self.cur].app.server_id.clone();
        if cur_id != LOCAL_SERVER && !want.iter().any(|e| e.id == cur_id) {
            tasks.push(self.switch_to(0));
        }
        if self.cur != 0 {
            let (a, b) = self.slots.split_at_mut(self.cur);
            take_window(&b[0].app, &mut a[0].app);
        }
        let cur_id = self.slots[self.cur].app.server_id.clone();
        let mut old: Vec<Slot> = self.slots.drain(1..).collect();
        for e in &want {
            match old.iter().position(|s| s.app.server_id == e.id) {
                Some(p) if old[p].entry.as_ref().is_some_and(|x| x.pref == e.pref) => {
                    let mut s = old.remove(p);
                    s.app.server_name = e.name.clone();
                    let turned_on = e.auto_connect && !s.entry.as_ref().is_some_and(|x| x.auto_connect);
                    if turned_on && s.app.link.is_none() && s.app.target.transport.is_ok() {
                        s.app.connect();
                    }
                    s.entry = Some(e.clone());
                    self.slots.push(s);
                }
                found => {
                    // New, or another host / command: a new `App` (the old one is dropped,
                    // which disconnects it).
                    let mut was = found.map(|p| old.remove(p));
                    let shown = e.id == cur_id;
                    if shown && let Some(w) = &mut was {
                        let serial = w.serial;
                        tasks.push(w.app.suspend().map(move |m| ShellMsg::App(serial, m)));
                    }
                    let connect = e.auto_connect || shown;
                    let target = build_target(&e.pref, None);
                    let mut s = self.new_slot(Some(e.clone()), (e.id.clone(), e.name.clone()), target, connect);
                    if let Some(w) = &was {
                        take_window(&w.app, &mut s.app);
                        s.app.focused = w.app.focused;
                    }
                    let serial = s.serial;
                    self.slots.push(s);
                    if shown {
                        let app = &mut self.slots.last_mut().expect("pushed").app;
                        tasks.push(app.resume().map(move |m| ShellMsg::App(serial, m)));
                    }
                }
            }
        }
        // `old` now holds removed servers: dropped here.
        drop(old);
        self.cur = self.index_of(&cur_id).unwrap_or(0);
        self.mark_shown();
        self.refresh_briefs();
        Task::batch(tasks)
    }

    pub fn view(&self, window: window::Id) -> iced::Element<'_, ShellMsg> {
        let s = &self.slots[self.cur];
        let serial = s.serial;
        s.app.view_window(window).map(move |m| ShellMsg::App(serial, m)).boxed()
    }

    pub fn title(&self, window: window::Id) -> String {
        self.slots[self.cur].app.title(window)
    }

    pub fn theme(&self, window: window::Id) -> iced::Theme {
        self.slots[self.cur].app.theme(window)
    }

    pub fn subscription(&self) -> Subscription<ShellMsg> {
        let mut subs = vec![
            meninx_web::events().map(ShellMsg::Web),
            crate::widgets_ui::subscription().map(ShellMsg::Routed),
            crate::previews_ui::subscription().map(ShellMsg::Routed),
        ];
        for (i, s) in self.slots.iter().enumerate() {
            subs.push(
                s.app
                    .subscription(i == self.cur)
                    .with(s.serial)
                    .map(|(serial, m)| ShellMsg::App(serial, m)),
            );
        }
        Subscription::batch(subs)
    }
}

/// The window as `from` has it (id, size, maximised / minimised, modifiers).
fn take_window(from: &App, to: &mut App) {
    to.window = from.window;
    to.window_size = from.window_size;
    to.window_maximized = from.window_maximized;
    to.window_minimized = from.window_minimized;
    to.modifiers = from.modifiers;
}
