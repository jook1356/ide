//! User terminals of a session in the detail area (Hermes Desk's `UserTerminalPane`,
//! `NewTerminalMenu`, `lib/terminals.ts`): shells or commands the server runs in the session's
//! folder (`uterm.*`). Their output, input and size go through the same `term.*` stream as
//! the agent's terminal, keyed by the terminal's id (`<session>:t<n>`). They show in 열린
//! 항목's Terminal group and in the detail pane; the server keeps them while it runs.

use iced::widget::{Column, button, column, container, row, space, text, text_input};
use iced::{Alignment, Color, Element, Fill, Task, Widget as _};
use ide_client::{PortInfo, ProtoError, SessionState, ShellInfo, TermChunk, UserTerm};
use ide_term::{TermModel, TerminalView};

use crate::app::{App, Live, Message};
use crate::detail::ItemKey;
use crate::i18n::{self, tr};
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// A user terminal as the app keeps it.
pub struct UTermView {
    pub info: UserTerm,
    pub term: TermModel,
    /// `term.attach` was requested on the current client.
    pub attached: bool,
}

#[derive(Debug, Clone)]
pub enum UTermMsg {
    Loaded(String, Result<Vec<UserTerm>, ProtoError>),
    /// "+ 새로 열기" → a terminal of `shell` (`None` = the default) in the picked folder, for
    /// session `.0` or 공용 (the switch, or Alt held).
    Open(String, Option<String>),
    /// The shells of the server, and the default one (☆, `ui.json`).
    Shells(Result<Vec<ShellInfo>, ProtoError>),
    SetDefaultShell(String),
    /// The folder a new terminal starts in.
    Dir(DirChoice),
    /// A 공용 start folder picked with 폴더 선택….
    PickedDir(String),
    /// 실행 중인 서버: ask again (while the menu is open), and the answer for a scope.
    PortsTick,
    Ports(String, Result<Vec<PortInfo>, ProtoError>),
    Opened(String, Result<UserTerm, ProtoError>),
    Stop(String),
    Rerun(String),
    /// Stop (if running) and forget (Desk's 종료하고 목록에서 지우기 / 목록에서 지우기).
    Remove(String),
    Done(String, Result<(), ProtoError>),
    Attached(String, Result<(), ProtoError>),
    /// "+ 새로 열기"'s 이 세션 (false) | 공용 (true).
    Scope(bool),
    /// The "+ 새로 열기" menu opened / closed.
    NewMenu(bool),
}

/// The session of a user terminal id (`""` for a 공용 one, `g:t<n>`).
pub fn session_of(id: &str) -> Option<&str> {
    if id.starts_with("g:t") {
        return Some("");
    }
    id.rsplit_once(":t").map(|(s, _)| s)
}

fn alive(s: SessionState) -> bool {
    matches!(s, SessionState::Running)
}

/// The status dot of a terminal (Desk's `process-dot`: running / failed / done).
pub fn dot<'a>(state: SessionState) -> El<'a> {
    let c = match state {
        SessionState::Running => style::GREEN,
        SessionState::Exited(Some(0)) | SessionState::Stopped => style::FAINT,
        _ => style::RED,
    };
    container(space().width(8).height(8))
        .style(style::dot(c))
        .boxed()
}

impl Live {
    pub fn uterm(&self, id: &str) -> Option<&UTermView> {
        self.uterms.iter().find(|t| t.info.id == id)
    }
}

impl App {
    /// A terminal by id: `live`'s, the session its id names (`<session>:t<n>`, the detail
    /// area of a folder shows its sessions' terminals) or a 공용 one.
    pub fn find_uterm<'a>(&'a self, live: &'a Live, id: &str) -> Option<&'a UTermView> {
        live.uterm(id)
            .or_else(|| {
                let (sid, _) = id.rsplit_once(":t")?;
                self.live(sid)?.uterm(id)
            })
            .or_else(|| self.global_uterms.iter().find(|t| t.info.id == id))
    }

    /// The terminals of session `sid` (`""`: the 공용 ones).
    fn uterm_list(&self, sid: &str) -> Option<&Vec<UTermView>> {
        if sid.is_empty() {
            Some(&self.global_uterms)
        } else {
            self.live(sid).map(|l| &l.uterms)
        }
    }

    fn uterm_list_mut(&mut self, sid: &str) -> Option<&mut Vec<UTermView>> {
        if sid.is_empty() {
            Some(&mut self.global_uterms)
        } else {
            self.live_mut(sid).map(|l| &mut l.uterms)
        }
    }

    fn uterm_mut(&mut self, id: &str) -> Option<&mut UTermView> {
        let sid = session_of(id)?.to_owned();
        self.uterm_list_mut(&sid)?
            .iter_mut()
            .find(|t| t.info.id == id)
    }

    /// Ask the server for a session's terminals (`""`: the 공용 ones).
    pub(crate) fn load_uterms(&self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let sid = sid.to_owned();
        Task::perform(
            {
                let sid = sid.clone();
                async move { c.uterm_list(&sid).await }
            },
            move |r| Message::UTerm(UTermMsg::Loaded(sid.clone(), r)),
        )
    }

    /// Attach the terminals of `sid` that are not attached on this client yet.
    fn attach_uterms(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(list) = self.uterm_list_mut(sid) else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for t in list {
            if t.attached {
                continue;
            }
            t.attached = true;
            let off = t.term.last_offset();
            let from = (off > 0).then_some(off);
            let (c, id) = (c.clone(), t.info.id.clone());
            tasks.push(Task::perform(
                {
                    let id = id.clone();
                    async move { c.attach(&id, from).await.map(drop) }
                },
                move |r| Message::UTerm(UTermMsg::Attached(id.clone(), r)),
            ));
        }
        Task::batch(tasks)
    }

    /// The server's list replaces the one of `sid` (output kept per id); a pane showing a
    /// terminal that is gone shows the web page.
    fn set_uterms(&mut self, sid: &str, list: Vec<UserTerm>) {
        let config = self.term_config.clone();
        let Some(cur) = self.uterm_list_mut(sid) else {
            return;
        };
        let mut old = std::mem::take(cur);
        for info in list {
            match old.iter().position(|t| t.info.id == info.id) {
                Some(i) => {
                    let mut t = old.remove(i);
                    t.info = info;
                    cur.push(t);
                }
                None => cur.push(UTermView {
                    info,
                    term: TermModel::new(120, 30, &config),
                    attached: false,
                }),
            }
        }
        let global: Vec<String> = self.global_uterms.iter().map(|t| t.info.id.clone()).collect();
        for l in &mut self.sessions {
            if let ItemKey::Terminal(id) = &l.detail.active
                && l.uterm(id).is_none()
                && !global.contains(id)
            {
                l.detail.active = ItemKey::Web;
            }
        }
    }

    fn uterm_infos(&self, sid: &str) -> Vec<UserTerm> {
        self.uterm_list(sid)
            .map(|l| l.iter().map(|x| x.info.clone()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn on_uterm(&mut self, m: UTermMsg) -> Task<Message> {
        match m {
            UTermMsg::Loaded(sid, Ok(list)) => {
                self.set_uterms(&sid, list);
                self.sync_web();
                self.attach_uterms(&sid)
            }
            UTermMsg::Loaded(_, Err(e)) => {
                tracing::debug!("uterm.list: {e}");
                Task::none()
            }
            UTermMsg::Open(sid, shell) => {
                // The detail area is the folder's; the terminal is the shown session's.
                let sid = if sid.is_empty() { sid } else { self.term_session(&sid) };
                self.new_menu = false;
                self.rail_open = false;
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let global = self.new_global || self.modifiers.alt();
                let shell = shell.or_else(|| self.ui.default_shell.clone());
                let mut o = ide_client::UTermOpen {
                    shell,
                    ..Default::default()
                };
                if global {
                    o.cwd = self.new_dir_global.clone();
                    if let Some(d) = &o.cwd {
                        let r = &mut self.ui.recent_term_dirs;
                        r.retain(|x| x != d);
                        r.insert(0, d.clone());
                        r.truncate(RECENT_DIRS);
                        self.save_ui();
                    }
                } else {
                    o.session = sid.clone();
                    o.repo = match &self.new_dir_repo {
                        Some(r) => r.clone(),
                        None => self
                            .detail(&sid)
                            .map(|d| d.repo.clone())
                            .filter(|r| !r.is_empty()),
                    };
                }
                let target = o.session.clone();
                Task::perform(async move { c.uterm_open(o).await }, move |r| {
                    Message::UTerm(UTermMsg::Opened(target.clone(), r))
                })
            }
            UTermMsg::Shells(r) => {
                match r {
                    Ok(s) => self.shells = Some(s),
                    Err(e) => tracing::debug!("uterm.shells: {e}"),
                }
                Task::none()
            }
            UTermMsg::SetDefaultShell(id) => {
                self.ui.default_shell = Some(id);
                self.save_ui();
                Task::none()
            }
            UTermMsg::Dir(choice) => {
                match choice {
                    DirChoice::Repo(id, _) => self.new_dir_repo = Some(Some(id)),
                    DirChoice::SessionFolder => self.new_dir_repo = Some(None),
                    DirChoice::Home => self.new_dir_global = None,
                    DirChoice::Path(p) => self.new_dir_global = Some(p),
                    DirChoice::Pick => {
                        self.new_menu = false;
                        return self.update(Message::PickTermFolder);
                    }
                }
                Task::none()
            }
            UTermMsg::PickedDir(p) => {
                self.new_dir_global = Some(p);
                self.new_global = true;
                self.new_menu = true;
                Task::none()
            }
            UTermMsg::PortsTick => self.load_ports(),
            UTermMsg::Ports(scope, r) => {
                if scope == self.ports_scope() {
                    self.ports = r.unwrap_or_default();
                }
                Task::none()
            }
            UTermMsg::Opened(sid, Ok(t)) => {
                let id = t.id.clone();
                let mut list = self.uterm_infos(&sid);
                if !list.iter().any(|x| x.id == id) {
                    list.push(t);
                }
                self.set_uterms(&sid, list);
                // Shown in the selected session's detail area, opened if hidden.
                let Some(show_in) = (if sid.is_empty() {
                    self.selected.clone()
                } else {
                    Some(sid.clone())
                }) else {
                    return self.attach_uterms(&sid);
                };
                self.ui.detail_collapsed = false;
                let show = self.activate(&show_in, ItemKey::Terminal(id));
                Task::batch([show, self.attach_uterms(&sid), self.sync_detail()])
            }
            UTermMsg::Opened(sid, Err(e)) | UTermMsg::Done(sid, Err(e)) => {
                let target = if sid.is_empty() {
                    self.selected.clone().unwrap_or_default()
                } else {
                    sid
                };
                if let Some(d) = self.detail_mut(&target) {
                    d.toast(crate::notices::Tone::Error, i18n::proto_error(&e));
                }
                Task::none()
            }
            UTermMsg::Done(_, Ok(())) => Task::none(),
            UTermMsg::Stop(id) => self.uterm_act(0, id),
            UTermMsg::Rerun(id) => self.uterm_act(1, id),
            UTermMsg::Remove(id) => self.uterm_act(2, id),
            UTermMsg::Attached(id, r) => {
                if r.is_err()
                    && let Some(t) = self.uterm_mut(&id)
                {
                    t.attached = false;
                }
                Task::none()
            }
            UTermMsg::Scope(global) => {
                self.new_global = global;
                self.ports.clear();
                self.load_ports()
            }
            UTermMsg::NewMenu(open) => {
                self.new_menu = open;
                self.bookmark_edit = None;
                if open {
                    Task::batch([self.load_shells(), self.load_ports()])
                } else {
                    Task::none()
                }
            }
        }
    }

    /// 중지 (0), 다시 실행 (1) or 목록에서 지우기 (2) of a user terminal.
    fn uterm_act(&mut self, what: u8, id: String) -> Task<Message> {
        let Some(sid) = session_of(&id).map(str::to_owned) else {
            return Task::none();
        };
        let Some(c) = self.online() else {
            return Task::none();
        };
        if what == 2 {
            // Gone at once; the server's list follows.
            let mut list = self.uterm_infos(&sid);
            list.retain(|t| t.id != id);
            self.set_uterms(&sid, list);
            self.sync_web();
        }
        Task::perform(
            async move {
                match what {
                    0 => c.uterm_stop(&id).await,
                    1 => c.uterm_rerun(&id).await,
                    _ => c.uterm_remove(&id).await,
                }
            },
            move |r| Message::UTerm(UTermMsg::Done(sid.clone(), r)),
        )
    }

    /// Output of a user terminal (`false`: not one).
    pub(crate) fn uterm_chunk(&mut self, chunk: &TermChunk) -> bool {
        let Some(t) = self.uterm_mut(&chunk.session) else {
            return false;
        };
        if chunk.reset {
            t.term.reset();
        }
        if !chunk.bytes.is_empty() {
            t.term.feed(chunk.offset, &chunk.bytes);
        }
        true
    }

    /// A user terminal's state or title changed (`false`: not one).
    pub(crate) fn uterm_notify(&mut self, id: &str, state: Option<SessionState>, title: Option<String>) -> bool {
        let Some(t) = self.uterm_mut(id) else {
            return false;
        };
        if let Some(s) = state {
            t.info.state = s;
        }
        if let Some(x) = title.filter(|x| !x.is_empty()) {
            t.info.title = x;
        }
        true
    }

    /// The pane's header for a terminal: status, title, folder; 다시 실행 / 중지.
    pub(crate) fn uterm_identity<'a>(&'a self, live: &'a Live, id: &str) -> El<'a> {
        let Some(t) = self.find_uterm(live, id) else {
            return space().boxed();
        };
        let running = alive(t.info.state);
        let status = match t.info.state {
            SessionState::Running => tr::uterm_running(),
            SessionState::Exited(Some(0)) | SessionState::Stopped => tr::uterm_done(),
            SessionState::Exited(Some(c)) => tr::uterm_failed(&c.to_string()),
            _ => tr::uterm_done(),
        };
        let action = if running {
            button(text(tr::uterm_stop()).size(12).color(style::RED))
                .padding([3, 10])
                .style(style::secondary)
                .on_press(Message::UTerm(UTermMsg::Stop(t.info.id.clone())))
        } else {
            button(text(tr::uterm_rerun()).size(12))
                .padding([3, 10])
                .style(style::secondary)
                .on_press(Message::UTerm(UTermMsg::Rerun(t.info.id.clone())))
        };
        row![
            dot(t.info.state),
            column![
                row![
                    text(t.info.title.clone())
                        .size(13)
                        .font(style::BOLD)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                    text(status).size(11).color(style::DIM),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text(t.info.cwd.clone())
                    .size(11)
                    .color(style::DIM)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start),
            ]
            .spacing(1)
            .width(Fill),
            action,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .boxed()
    }

    /// The pane's body for a terminal.
    pub(crate) fn uterm_body<'a>(&'a self, live: &'a Live, id: &str) -> El<'a> {
        let Some(t) = self.find_uterm(live, id) else {
            return space().boxed();
        };
        let (a, b) = (t.info.id.clone(), t.info.id.clone());
        let mut view = TerminalView::new(&t.term, &self.term_config).on_event(Message::TermView);
        if self.connected() {
            view = view
                .on_input(move |bytes| Message::TermInput(a.clone(), bytes))
                .on_resize(move |c, r| Message::TermResize(b.clone(), c, r));
        }
        container(view)
            .width(Fill)
            .height(Fill)
            .style(style::bg(style::BG))
            .boxed()
    }

    /// Desk's "+ 새로 열기" (`NewTerminalMenu`): 이 세션 | 공용 on top, a terminal (in one of
    /// the session's repos or its folder; 공용: the home folder), then the web: 즐겨찾기 (open,
    /// rename, delete) and an address field. `wide`: with its label.
    pub(crate) fn new_item_button<'a>(&'a self, live: &'a Live, wide: bool) -> El<'a> {
        use crate::browser::{BOOKMARK_EDIT_ID, BrowserMsg};
        let sid = live.info.id.as_str();
        let global = self.new_global;
        let label: El<'a> = if wide {
            row![text("+").size(14), text(tr::new_item()).size(12)]
                .spacing(6)
                .align_y(Alignment::Center)
                .boxed()
        } else {
            container(text("+").size(15)).center_x(Fill).boxed()
        };
        let trigger = button(label)
            .padding([5, 8])
            .width(Fill)
            .style(style::choice(self.new_menu))
            .on_press(Message::UTerm(UTermMsg::NewMenu(!self.new_menu)));
        let popup = self.new_menu.then(|| {
            let row_btn = |content: El<'a>, msg: Message| -> El<'a> {
                button(content)
                    .padding([5, 10])
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(msg)
                    .boxed()
            };
            let label = |s: String| -> El<'a> {
                container(text(s).size(11).color(style::DIM))
                    .padding([6, 10])
                    .boxed()
            };
            let scope = |on: bool, s: String| {
                button(container(text(s).size(12)).center_x(Fill))
                    .padding([4, 8])
                    .width(Fill)
                    .style(style::choice(global == on))
                    .on_press(Message::UTerm(UTermMsg::Scope(on)))
            };
            let mut col = Column::<El<'a>>::new().spacing(1);
            col = col.push(
                row![
                    scope(false, tr::new_scope_session()),
                    scope(true, tr::new_scope_global())
                ]
                .spacing(4)
                .boxed(),
            );
            col = col.push(
                container(
                    text(if global {
                        tr::new_scope_global_hint()
                    } else {
                        tr::new_scope_session_hint()
                    })
                    .size(11)
                    .color(style::FAINT),
                )
                .padding([4, 4])
                .boxed(),
            );
            // Terminal: the shells (click opens one, ☆ makes it the default), then the
            // folder it starts in.
            col = col.push(label(tr::uterm_new_title()));
            let default = self
                .ui
                .default_shell
                .clone()
                .or_else(|| self.shells.as_ref().and_then(|s| s.first()).map(|s| s.id.clone()));
            match &self.shells {
                Some(list) => {
                    for s in list {
                        let is_default = default.as_deref() == Some(s.id.as_str());
                        let open = row_btn(
                            row![
                                text(">_").size(11).font(style::MONO).color(style::DIM),
                                text(s.name.clone()).size(13),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .boxed(),
                            Message::UTerm(UTermMsg::Open(sid.to_owned(), Some(s.id.clone()))),
                        );
                        let star = tip(
                            button(
                                text(if is_default { "★" } else { "☆" })
                                    .size(12)
                                    .color(if is_default { style::YELLOW } else { style::FAINT }),
                            )
                            .padding([3, 6])
                            .style(style::icon)
                            .on_press(Message::UTerm(UTermMsg::SetDefaultShell(s.id.clone())))
                            .boxed(),
                            if is_default {
                                tr::uterm_default_shell()
                            } else {
                                tr::uterm_make_default()
                            },
                        );
                        col = col.push(
                            tip(
                                row![open, star].align_y(Alignment::Center).boxed(),
                                s.path.clone(),
                            ),
                        );
                    }
                }
                None => {
                    col = col.push(row_btn(
                        text(tr::uterm_new_title()).size(13).boxed(),
                        Message::UTerm(UTermMsg::Open(sid.to_owned(), None)),
                    ));
                }
            }
            let (options, picked): (Vec<DirChoice>, DirChoice) = if global {
                let mut o = vec![DirChoice::Home];
                o.extend(self.ui.recent_term_dirs.iter().cloned().map(DirChoice::Path));
                if let Some(p) = &self.new_dir_global
                    && !self.ui.recent_term_dirs.contains(p)
                {
                    o.push(DirChoice::Path(p.clone()));
                }
                o.push(DirChoice::Pick);
                let picked = match &self.new_dir_global {
                    Some(p) => DirChoice::Path(p.clone()),
                    None => DirChoice::Home,
                };
                (o, picked)
            } else {
                let mut o: Vec<DirChoice> = live
                    .info
                    .repos
                    .iter()
                    .map(|r| DirChoice::Repo(r.repo.clone(), r.name.clone()))
                    .collect();
                o.push(DirChoice::SessionFolder);
                let chosen = match &self.new_dir_repo {
                    Some(r) => r.clone(),
                    None => Some(live.detail.repo.clone()).filter(|r| !r.is_empty()),
                };
                let picked = chosen
                    .and_then(|id| o.iter().find(|c| matches!(c, DirChoice::Repo(x, _) if *x == id)).cloned())
                    .unwrap_or(DirChoice::SessionFolder);
                (o, picked)
            };
            col = col.push(
                container(
                    row![
                        text(tr::uterm_start_in()).size(11).color(style::DIM),
                        iced::widget::pick_list(Some(picked), options, |c: &DirChoice| c.to_string())
                            .on_select(|c| Message::UTerm(UTermMsg::Dir(c)))
                            .text_size(12)
                            .padding([3, 8])
                            .width(Fill),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([2, 10])
                .boxed(),
            );
            // 실행 중인 서버 (ports listening in this session's / the 공용 terminals).
            if !self.ports.is_empty() {
                col = col.push(label(tr::new_running_servers(i18n::count(self.ports.len()))));
                for p in &self.ports {
                    let url = format!("http://localhost:{}", p.port);
                    col = col.push(row_btn(
                        row![
                            text("●").size(9).color(style::GREEN),
                            text(format!("localhost:{}", p.port)).size(13).width(Fill),
                            text(p.process.clone()).size(11).color(style::FAINT),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .boxed(),
                        Message::Browser(BrowserMsg::Open { global, url }),
                    ));
                }
            }
            // Web: 즐겨찾기, then the address.
            col = col.push(label(tr::new_web()));
            for b in &self.ui.bookmarks {
                let editing = self
                    .bookmark_edit
                    .as_ref()
                    .filter(|(u, _)| *u == b.url)
                    .map(|(_, t)| t.as_str());
                let r: El<'a> = match editing {
                    Some(t) => container(
                        text_input(&b.title, t)
                            .id(BOOKMARK_EDIT_ID)
                            .on_input(|v| Message::Browser(BrowserMsg::BookmarkEditInput(v)))
                            .on_submit(Message::Browser(BrowserMsg::BookmarkEditDone))
                            .size(12)
                            .padding([4, 8])
                            .style(style::input),
                    )
                    .padding([2, 6])
                    .boxed(),
                    None => {
                        let open = row_btn(
                            row![
                                text("★").size(12).color(style::YELLOW),
                                text(b.title.clone())
                                    .size(13)
                                    .wrapping(text::Wrapping::None)
                                    .ellipsis(text::Ellipsis::End),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .boxed(),
                            Message::Browser(BrowserMsg::Open {
                                global,
                                url: b.url.clone(),
                            }),
                        );
                        let small = |glyph: &'static str, tip_text: String, msg: Message| {
                            tip(
                                button(text(glyph).size(11).color(style::DIM))
                                    .padding([3, 6])
                                    .style(style::icon)
                                    .on_press(msg)
                                    .boxed(),
                                tip_text,
                            )
                        };
                        tip(
                            row![
                                open,
                                small(
                                    "✎",
                                    tr::bookmark_rename(),
                                    Message::Browser(BrowserMsg::BookmarkEdit(b.url.clone()))
                                ),
                                small(
                                    "✕",
                                    tr::bookmark_remove(),
                                    Message::Browser(BrowserMsg::BookmarkRemove(b.url.clone()))
                                ),
                            ]
                            .align_y(Alignment::Center)
                            .boxed(),
                            b.url.clone(),
                        )
                    }
                };
                col = col.push(r);
            }
            let go = tip(
                button(text("→").size(13))
                    .padding([4, 8])
                    .style(style::secondary)
                    .on_press(Message::Browser(BrowserMsg::AddressSubmit))
                    .boxed(),
                tr::new_web_open(),
            );
            col = col.push(
                container(
                    row![
                        text_input(tr::new_web_placeholder(), &self.new_address)
                            .on_input(|v| Message::Browser(BrowserMsg::Address(v)))
                            .on_submit(Message::Browser(BrowserMsg::AddressSubmit))
                            .size(12)
                            .padding([5, 8])
                            .style(style::input)
                            .width(Fill),
                        go,
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([4, 6])
                .boxed(),
            );
            container(col)
                .padding(6)
                .width(290)
                .style(style::modal_card)
        });
        tip(
            crate::side_popover::side_popover(
                trigger,
                popup,
                4.0,
                Message::UTerm(UTermMsg::NewMenu(false)),
            ),
            tr::new_item_tip(),
        )
    }
}

/// The Terminal group's colour on the icon rail.
pub const GROUP_COLOR: Color = style::CYAN;

/// 공용 start folders remembered (Desk's recent folders).
const RECENT_DIRS: usize = 5;

/// A choice of the folder picker of "+ 새로 열기".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirChoice {
    /// A repo of the session (id, name).
    Repo(String, String),
    SessionFolder,
    Home,
    Path(String),
    /// 폴더 선택…
    Pick,
}

impl std::fmt::Display for DirChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirChoice::Repo(_, name) => f.write_str(name),
            DirChoice::SessionFolder => f.write_str(&tr::uterm_in_session()),
            DirChoice::Home => f.write_str(&tr::uterm_in_home()),
            DirChoice::Path(p) => f.write_str(p),
            DirChoice::Pick => f.write_str(&tr::uterm_pick_folder()),
        }
    }
}

impl App {
    fn load_shells(&self) -> Task<Message> {
        if self.shells.is_some() {
            return Task::none();
        }
        let Some(c) = self.online() else {
            return Task::none();
        };
        Task::perform(async move { c.uterm_shells().await }, |r| {
            Message::UTerm(UTermMsg::Shells(r))
        })
    }

    /// Whose servers the menu lists: the selected session's, or `""` for 공용.
    fn ports_scope(&self) -> String {
        if self.new_global {
            String::new()
        } else {
            self.selected.clone().unwrap_or_default()
        }
    }

    pub(crate) fn load_ports(&self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let scope = self.ports_scope();
        Task::perform(
            {
                let scope = scope.clone();
                async move { c.uterm_ports(&scope).await }
            },
            move |r| Message::UTerm(UTermMsg::Ports(scope.clone(), r)),
        )
    }
}
