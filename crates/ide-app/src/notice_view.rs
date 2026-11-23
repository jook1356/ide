//! Notification centre UI (Hermes Desk's `NoticeCenter.tsx`): the rail's bell with the unread
//! count, the list it opens, and the toasts. The store is `notices.rs`; docs/notifications.md.

use std::time::{Instant, UNIX_EPOCH};

use iced::widget::{Column, button, column, container, mouse_area, popover, row, scrollable, space, stack, text, tooltip};
use iced::{Alignment, Border, Color, Element, Fill, Theme, Widget as _};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::notices::{ActionStyle, Notice, NoticeKey, Notices, TaskState, Text, Tone};
use crate::style;

type El<'a> = Element<'a, Message>;

/// The app's notice store.
pub type AppNotices = Notices<Message>;
pub type AppNotice = Notice<Message>;

const PANEL_W: f32 = 360.0;
const PANEL_H: f32 = 520.0;
const TOAST_W: f32 = 340.0;
/// Most toasts on screen at once (the newest).
const MAX_TOASTS: usize = 3;

#[derive(Debug, Clone)]
pub enum NoticeMsg {
    /// The bell: open / close the list.
    Toggle,
    Close,
    /// A notice (or its toast) was clicked.
    Open(u64),
    /// Button `.1` of notice `.0`.
    Action(u64, usize),
    /// The session chip of a notice.
    GoSession(String),
    Read(u64),
    ReadAll,
    Dismiss(u64),
    ClearFinished,
    /// × of a toast.
    HideToast(u64),
}

pub fn tone_color(t: Tone) -> Color {
    match t {
        Tone::Info => style::ACCENT_TEXT,
        Tone::Success => style::GREEN,
        Tone::Warning => style::ORANGE,
        Tone::Error => style::RED,
    }
}

fn tone_icon(t: Tone) -> Icon {
    match t {
        Tone::Info => Icon::InfoCircle,
        Tone::Success => Icon::CircleCheck,
        Tone::Warning => Icon::AlertTriangle,
        Tone::Error => Icon::CircleX,
    }
}

fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// A page that did not load (CEF `LoadError`), unless it was only cancelled (a redirect or a
/// new navigation: `ERR_ABORTED`). Not a toast: the page shows its own error.
pub fn load_error_notice(url: &str, error: &str) -> Option<AppNotice> {
    if error.contains("ABORTED") || error.trim() == "-3" {
        return None;
    }
    Some(Notice::warning(Text::tr(tr::notice_load_failed)).body(format!("{url}\n{error}")))
}

/// Local `HH:MM` of a notice.
fn clock(n: &AppNotice) -> String {
    let secs = n.at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(secs) else {
        return String::new();
    };
    let t = time::UtcOffset::current_local_offset().map_or(t, |o| t.to_offset(o));
    format!("{:02}:{:02}", t.hour(), t.minute())
}

fn item_style(tone: Tone, unread: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let edge = match tone {
            Tone::Error | Tone::Warning => alpha(tone_color(tone), 0.45),
            _ => style::BORDER,
        };
        // Unread: lighter ground and an accent edge; read: plain and dimmer text (Desk).
        let edge = if unread && edge == style::BORDER { alpha(style::ACCENT, 0.55) } else { edge };
        container::Style {
            background: Some(if unread { style::HEADER } else { style::PANEL }.into()),
            text_color: Some(if unread { style::TEXT } else { style::DIM }),
            border: Border {
                color: edge,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    }
}

fn action_btn(st: ActionStyle) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, s| {
        let fg = match st {
            ActionStyle::Primary | ActionStyle::Normal => style::ACCENT_TEXT,
            ActionStyle::Danger => style::RED,
        };
        let bg = match (st, s) {
            (_, button::Status::Hovered | button::Status::Pressed) => Some(style::HOVER),
            (ActionStyle::Primary, _) => Some(style::accent_alpha(0.16)),
            _ => None,
        };
        button::Style {
            background: bg.map(Into::into),
            text_color: if s == button::Status::Disabled { style::FAINT } else { fg },
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

fn nm(m: NoticeMsg) -> Message {
    Message::Notice(m)
}

impl App {
    /// Adds a notice (a toast too when it has `.toast()`). Returns its id.
    pub fn notify(&mut self, n: AppNotice) -> u64 {
        crate::script::log(&format!("notice {:?}: {}", n.tone, n.title.get()));
        self.notices.push(n)
    }

    /// Moves the toasts of the detail panes (`Detail::toast`) into the centre: successes and
    /// plain information are history only (read), warnings and errors count as unread.
    pub(crate) fn take_detail_toasts(&mut self) {
        let mut out = Vec::new();
        for l in &mut self.sessions {
            for (tone, text) in l.detail.toasts.drain(..) {
                out.push((l.info.id.clone(), tone, text));
            }
        }
        for (sid, tone, text) in out {
            let mut n = Notice::new(tone, text).session(sid).toast();
            if matches!(tone, Tone::Success | Tone::Info) {
                n = n.quiet();
            }
            self.notify(n);
        }
    }

    /// Notices that follow other state: each session's PR job and PR status error (footer.rs),
    /// and the self-update result. A notice changes only when what it shows changes.
    pub(crate) fn sync_state_notices(&mut self) {
        use crate::footer::{FooterMsg, JobAction};
        let mut want: Vec<(NoticeKey, String, Option<AppNotice>)> = Vec::new();
        for l in &self.sessions {
            let sid = l.info.id.as_str();
            for (repo, f) in &l.detail.footer.repos {
                let key = NoticeKey::PrJob {
                    session: sid.to_owned(),
                    repo: repo.clone(),
                };
                if let Some(j) = &f.job {
                    let running = j.running();
                    let sig = format!("{:?}|{}|{}|{:?}|{running}", j.tone, j.title, j.message, j.actions);
                    let title_changed = self
                        .notice_sigs
                        .get(&key)
                        .is_none_or(|old| !old.starts_with(&format!("{:?}|{}|", j.tone, j.title)));
                    let mut n = Notice::new(j.tone, j.title.clone())
                        .session(sid)
                        .repo(repo.clone())
                        .key(key.clone())
                        .open(Message::Notice(NoticeMsg::GoSession(sid.to_owned())))
                        .task(match (running, j.tone) {
                            (true, _) => TaskState::Running,
                            (false, Tone::Error) => TaskState::Failed,
                            (false, _) => TaskState::Done,
                        });
                    if !j.message.is_empty() {
                        n = n.body(j.message.clone());
                    }
                    for a in &j.actions {
                        let (label, st): (fn() -> String, ActionStyle) = match a {
                            JobAction::Retry => (tr::foot_job_retry, ActionStyle::Primary),
                            JobAction::Cancel => (tr::cancel, ActionStyle::Danger),
                            JobAction::CreateNow => (tr::foot_job_create_now, ActionStyle::Primary),
                            JobAction::Recheck => (tr::foot_recheck_strip_action, ActionStyle::Primary),
                            JobAction::Open(_) => (tr::foot_job_open, ActionStyle::Normal),
                        };
                        n = n.action(
                            Text::tr(label),
                            Message::Footer(sid.to_owned(), FooterMsg::JobAction(repo.clone(), a.clone())),
                            st,
                        );
                    }
                    // Progress is quiet; a result or a question for the user is not, and pops up
                    // when its title changes.
                    let needs_user = !running || j.tone != Tone::Info;
                    if !needs_user {
                        n = n.quiet();
                    } else if title_changed {
                        n = n.toast();
                    }
                    want.push((key, sig, Some(n)));
                } else if self.notice_sigs.contains_key(&key) {
                    // The card was dismissed: the notice stays in the list.
                    want.push((key, String::new(), None));
                }
                let key = NoticeKey::PrInfo {
                    session: sid.to_owned(),
                    repo: repo.clone(),
                };
                match &f.pr_error {
                    Some(e) => {
                        let n = Notice::warning(Text::tr(tr::notice_pr_status_failed))
                            .body(e.clone())
                            .session(sid)
                            .repo(repo.clone())
                            .key(key.clone())
                            .quiet();
                        want.push((key, e.clone(), Some(n)));
                    }
                    None if self.notice_sigs.contains_key(&key) => {
                        want.push((key, String::new(), None));
                    }
                    None => {}
                }
            }
        }
        if let Some((sig, n)) = self.update_notice() {
            want.push((NoticeKey::Update, sig, Some(n.key(NoticeKey::Update))));
        }
        for (key, sig, n) in want {
            match n {
                Some(n) => {
                    if self.notice_sigs.get(&key) == Some(&sig) {
                        continue;
                    }
                    self.notice_sigs.insert(key, sig);
                    self.notify(n);
                }
                None => {
                    self.notice_sigs.remove(&key);
                    if matches!(key, NoticeKey::PrInfo { .. }) {
                        self.notices.remove_key(&key);
                    }
                }
            }
        }
    }

    /// The user looks at session `sid` (selected on the work page of the focused window).
    fn seeing(&self, sid: &str) -> bool {
        self.focused && self.on_work_page() && self.selected.as_deref() == Some(sid)
    }

    /// Flashes the app's taskbar button while the window is not focused (an agent notice).
    fn attention(&self) -> iced::Task<Message> {
        match self.window {
            Some(id) if !self.focused => iced::window::request_user_attention(
                id,
                Some(iced::window::UserAttention::Informational),
            ),
            _ => iced::Task::none(),
        }
    }

    fn attention_notice(&mut self, sid: &str, n: AppNotice) -> iced::Task<Message> {
        self.notify(
            n.session(sid)
                .key(NoticeKey::Attention {
                    session: sid.to_owned(),
                })
                .open(Message::Notice(NoticeMsg::GoSession(sid.to_owned())))
                .toast(),
        );
        self.attention()
    }

    /// The agent of `sid` went from `was` to `now` (Desk `onAttention`): it waits for the user
    /// or finished its answer while the user looks elsewhere.
    pub(crate) fn activity_notice(
        &mut self,
        sid: &str,
        was: ide_client::AgentActivity,
        now: ide_client::AgentActivity,
    ) -> iced::Task<Message> {
        use ide_client::AgentActivity as A;
        if self.seeing(sid) || was == now {
            return iced::Task::none();
        }
        match (was, now) {
            (_, A::NeedsInput) => self.attention_notice(sid, Notice::warning(Text::tr(tr::notice_needs_input))),
            (A::Working, A::Idle) => self.attention_notice(sid, Notice::success(Text::tr(tr::notice_answered))),
            _ => iced::Task::none(),
        }
    }

    /// The process of `sid` exited by itself: a failure (non-zero code) is announced like an
    /// agent notice, a clean exit is history only.
    pub(crate) fn exit_notice(&mut self, sid: &str, code: Option<u32>) -> iced::Task<Message> {
        match code {
            Some(c) if c != 0 => {
                let c = c.to_string();
                self.attention_notice(sid, Notice::error(Text::tr(move || tr::process_exited_code(&c))))
            }
            _ => {
                self.notify(
                    Notice::info(Text::tr(tr::process_exited))
                        .session(sid)
                        .open(Message::Notice(NoticeMsg::GoSession(sid.to_owned())))
                        .quiet(),
                );
                iced::Task::none()
            }
        }
    }

    /// The terminal of `sid` rang its bell (BEL) while the user looks elsewhere.
    pub(crate) fn bell_notice(&mut self, sid: &str) -> iced::Task<Message> {
        if self.live(sid).is_none() || self.seeing(sid) {
            return iced::Task::none();
        }
        self.notify(
            Notice::info(Text::tr(tr::notice_bell_rang))
                .session(sid)
                .key(NoticeKey::Bell {
                    session: sid.to_owned(),
                })
                .open(Message::Notice(NoticeMsg::GoSession(sid.to_owned()))),
        );
        self.attention()
    }

    /// Connection to the server lost / back (keyed: one notice follows it).
    pub(crate) fn conn_notice(&mut self) {
        use crate::app::Conn;
        let shown = self.notices.find_key(&NoticeKey::Conn).map(|n| n.tone);
        let n = match &self.conn {
            Conn::Connected(_) => {
                // Only after a lost connection was reported.
                if !matches!(shown, Some(Tone::Warning | Tone::Error)) {
                    return;
                }
                Notice::success(Text::tr(tr::notice_conn_back)).quiet().toast()
            }
            Conn::Reconnecting => Notice::warning(Text::tr(tr::conn_reconnecting)).toast(),
            Conn::Disconnected(d) => {
                let d = d.clone();
                let n = Notice::error(Text::tr(move || tr::conn_disconnected(&d)));
                if self.server_outdated {
                    n.action(
                        Text::tr(tr::old_server_confirm),
                        Message::OldServer(crate::old_server::OldServerMsg::Ask),
                        ActionStyle::Primary,
                    )
                } else {
                    n.action(Text::tr(tr::conn_retry), Message::Retry, ActionStyle::Primary)
                }
                .toast()
            }
            Conn::Connecting => return,
        };
        self.notify(n.key(NoticeKey::Conn));
    }

    /// Text of the newest toast about `sid` still up (test script).
    pub(crate) fn toast_text(&self, sid: &str) -> Option<String> {
        let now = Instant::now();
        self.notices
            .toasts(now)
            .filter(|n| n.session.as_deref() == Some(sid))
            .last()
            .map(|n| n.title.get())
    }

    /// The bell at the bottom of the navigation rail.
    pub(crate) fn notice_bell(&self) -> El<'_> {
        let unread = self.notices.unread();
        let running = self.notices.running();
        let open = self.notice_open;
        let color = if open { style::RAIL_ACTIVE_ICON } else { style::RAIL_ICON };
        let mut layers: Vec<El<'_>> = vec![container(icons::icon(Icon::Bell, 22.0, color)).center(44).boxed()];
        if unread > 0 {
            let label = if unread > 9 { "9+".to_owned() } else { unread.to_string() };
            layers.push(
                container(
                    container(text(label).size(10).color(Color::WHITE).font(style::bold()))
                        .padding([0, 4])
                        .style(|_: &Theme| container::Style {
                            background: Some(style::RED.into()),
                            border: Border {
                                radius: 8.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                )
                .width(44)
                .height(44)
                .align_x(Alignment::End)
                .padding(iced::Padding {
                    top: 5.0,
                    right: 4.0,
                    ..iced::Padding::ZERO
                })
                .boxed(),
            );
        } else if running > 0 {
            layers.push(
                container(container(space().width(7).height(7)).style(style::dot(style::ACCENT)))
                    .width(44)
                    .height(44)
                    .align_x(Alignment::End)
                    .padding(iced::Padding {
                        top: 8.0,
                        right: 9.0,
                        ..iced::Padding::ZERO
                    })
                    .boxed(),
            );
        }
        let b = button(stack(layers).width(44).height(44))
            .width(44)
            .height(44)
            .padding(0)
            .style(style::rail_link(open))
            .on_press(nm(NoticeMsg::Toggle));
        let label = if running > 0 {
            tr::notice_bell_running(crate::i18n::count(running))
        } else {
            tr::notice_bell()
        };
        let b: El<'_> = if open {
            b.boxed()
        } else {
            tooltip(
                b,
                container(text(label).size(14)).padding([5, 10]).style(style::rail_tip),
            )
            .position(tooltip::Position::Right)
            .gap(8)
            .boxed()
        };
        let popup = open.then(|| self.notice_panel());
        popover(b, popup)
            .position(popover::Position::Right)
            .gap(12)
            .on_close(nm(NoticeMsg::Close))
            .boxed()
    }

    fn notice_panel(&self) -> El<'_> {
        let running = self.notices.running();
        let unread = self.notices.unread();
        let head_btn = |i: Icon, tip: String, msg: Option<Message>| -> El<'_> {
            tooltip(
                button(container(icons::icon(i, 16.0, if msg.is_some() { style::DIM } else { style::FAINT })).center(26))
                    .width(26)
                    .height(26)
                    .padding(0)
                    .style(style::icon)
                    .on_press_maybe(msg),
                container(text(tip).size(12)).padding([4, 8]).style(style::rail_tip),
            )
            .position(tooltip::Position::Bottom)
            .boxed()
        };
        let mut head = row![text(tr::notice_title()).size(14).font(style::bold())]
            .spacing(6)
            .align_y(Alignment::Center);
        if running > 0 {
            head = head.push(
                container(text(tr::notice_running(crate::i18n::count(running))).size(11).color(style::ACCENT_TEXT))
                    .padding([1, 6])
                    .style(|_: &Theme| container::Style {
                        background: Some(style::accent_alpha(0.16).into()),
                        border: Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .boxed(),
            );
        }
        head = head
            .push(space().width(Fill).boxed())
            .push(head_btn(Icon::Checks, tr::notice_read_all(), (unread > 0).then(|| nm(NoticeMsg::ReadAll))))
            .push(head_btn(
                Icon::Trash,
                tr::notice_clear(),
                self.notices.iter().any(|n| !n.running()).then(|| nm(NoticeMsg::ClearFinished)),
            ))
            .push(head_btn(Icon::X, tr::close(), Some(nm(NoticeMsg::Close))));
        let head = container(head).padding([8, 12]).height(42);
        let body: El<'_> = if self.notices.is_empty() {
            container(text(tr::notice_empty()).size(12).color(style::DIM))
                .center(Fill)
                .boxed()
        } else {
            let mut list = Column::new().spacing(6).padding(8);
            for n in self.notices.iter() {
                list = list.push(self.notice_item(n));
            }
            scrollable(list).height(Fill).boxed()
        };
        let h = PANEL_H.min((self.window_size.height - 80.0).max(200.0));
        container(column![
            head,
            container(space()).height(1).width(Fill).style(style::bg(style::BORDER)),
            body
        ])
        .width(PANEL_W)
        .height(h)
        .style(style::modal_card)
        .boxed()
    }

    fn session_name(&self, sid: &str) -> Option<String> {
        self.live(sid).map(|l| {
            let t = l.info.title.trim();
            if t.is_empty() { sid.to_owned() } else { t.to_owned() }
        })
    }

    fn notice_item<'a>(&'a self, n: &'a AppNotice) -> El<'a> {
        let c = tone_color(n.tone);
        let icon: El<'_> = if n.task == Some(TaskState::Running) {
            text("…").size(14).color(style::ACCENT_TEXT).boxed()
        } else {
            icons::icon(tone_icon(n.tone), 16.0, c)
        };
        let unread = !n.read || n.running();
        let mut top = row![
            text(n.title.get()).size(12).font(style::bold()).width(Fill),
            text(clock(n)).size(10).color(style::FAINT),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if !n.read {
            top = top.push(
                button(icons::icon(Icon::Check, 12.0, style::DIM))
                    .padding(2)
                    .style(style::icon)
                    .on_press(nm(NoticeMsg::Read(n.id)))
                    .boxed(),
            );
        }
        if !n.running() {
            top = top.push(
                button(icons::icon(Icon::X, 12.0, style::DIM))
                    .padding(2)
                    .style(style::icon)
                    .on_press(nm(NoticeMsg::Dismiss(n.id)))
                    .boxed(),
            );
        }
        let mut col = column![top].spacing(4);
        if let Some(sid) = &n.session
            && let Some(name) = self.session_name(sid)
        {
            let mut chip = row![icons::icon(Icon::Messages, 12.0, style::DIM), text(name).size(11)]
                .spacing(4)
                .align_y(Alignment::Center);
            if let Some(repo) = &n.repo {
                chip = chip.push(text(repo.as_str()).size(11).color(style::FAINT).boxed());
            }
            col = col.push(
                button(chip)
                    .padding([1, 6])
                    .style(style::divider)
                    .on_press(nm(NoticeMsg::GoSession(sid.clone())))
                    .boxed(),
            );
        }
        if let Some(b) = &n.body {
            let b = b.get();
            if !b.is_empty() {
                col = col.push(text(b).size(11).color(style::DIM).boxed());
            }
        }
        if !n.actions.is_empty() {
            let mut acts = row![].spacing(4);
            for (i, a) in n.actions.iter().enumerate() {
                acts = acts.push(
                    button(text(a.label.get()).size(11))
                        .padding([2, 8])
                        .style(action_btn(a.style))
                        .on_press(nm(NoticeMsg::Action(n.id, i)))
                        .boxed(),
                );
            }
            col = col.push(acts.boxed());
        }
        let content = row![container(icon).padding(iced::Padding { top: 1.0, ..iced::Padding::ZERO }), col.width(Fill)]
            .spacing(8);
        let card = container(content)
            .padding([8, 10])
            .width(Fill)
            .style(item_style(n.tone, unread));
        let card: El<'_> = if n.open.is_some() {
            mouse_area(card)
                .on_press(nm(NoticeMsg::Open(n.id)))
                .interaction(iced::mouse::Interaction::Pointer)
                .boxed()
        } else {
            card.boxed()
        };
        card
    }

    /// Toasts of window `detail` (the detail window: notices of the session it shows; the
    /// app window: the rest), bottom right.
    pub(crate) fn toast_layer(&self, detail: bool) -> El<'_> {
        let now = Instant::now();
        let in_detail = self.detail_window.and(self.shown_detail());
        let mine = |n: &&AppNotice| {
            let here = n.session.is_some() && n.session == in_detail;
            here == detail
        };
        let ups: Vec<&AppNotice> = self.notices.toasts(now).filter(mine).collect();
        if ups.is_empty() {
            return space().boxed();
        }
        let mut col = Column::new().spacing(8).align_x(Alignment::End);
        for n in ups.iter().skip(ups.len().saturating_sub(MAX_TOASTS)) {
            col = col.push(self.toast_card(n));
        }
        container(col)
            .width(Fill)
            .height(Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::End)
            .padding(iced::Padding {
                right: 16.0,
                bottom: 40.0,
                ..iced::Padding::ZERO
            })
            .boxed()
    }

    fn toast_card<'a>(&'a self, n: &'a AppNotice) -> El<'a> {
        let c = tone_color(n.tone);
        let mut col = column![text(n.title.get()).size(12).font(style::bold())].spacing(2);
        if let Some(b) = &n.body {
            let b = b.get();
            if !b.is_empty() {
                col = col.push(text(b).size(11).color(style::DIM).boxed());
            }
        }
        let content = row![
            icons::icon(tone_icon(n.tone), 16.0, c),
            col.width(Fill),
            button(icons::icon(Icon::X, 12.0, style::DIM))
                .padding(2)
                .style(style::icon)
                .on_press(nm(NoticeMsg::HideToast(n.id))),
        ]
        .spacing(8);
        let card = container(content)
            .padding([8, 10])
            .width(TOAST_W)
            .style(move |t: &Theme| {
                let mut s = style::modal_card(t);
                s.border.color = alpha(c, 0.5);
                s
            });
        // Clicking the toast does what clicking the notice does, or just hides it.
        let msg = if n.open.is_some() {
            NoticeMsg::Open(n.id)
        } else {
            NoticeMsg::HideToast(n.id)
        };
        mouse_area(card)
            .on_press(nm(msg))
            .interaction(iced::mouse::Interaction::Pointer)
            .boxed()
    }

    pub(crate) fn on_notice(&mut self, m: NoticeMsg) -> iced::Task<Message> {
        match m {
            NoticeMsg::Toggle => {
                self.notice_open = !self.notice_open;
                crate::script::log(&format!("notice list open: {}", self.notice_open));
            }
            NoticeMsg::Close => self.notice_open = false,
            NoticeMsg::Open(id) => {
                let Some(msg) = self.notices.get(id).and_then(|n| n.open.clone()) else {
                    self.notices.mark_read(id);
                    self.notices.hide_toast(id);
                    return iced::Task::none();
                };
                self.notices.mark_read(id);
                self.notices.hide_toast(id);
                self.notice_open = false;
                return self.update(msg);
            }
            NoticeMsg::Action(id, i) => {
                let Some(a) = self.notices.get(id).and_then(|n| n.actions.get(i).cloned()) else {
                    return iced::Task::none();
                };
                self.notices.mark_read(id);
                self.notices.hide_toast(id);
                return self.update(a.msg);
            }
            NoticeMsg::GoSession(sid) => {
                self.notice_open = false;
                return self.go_session(sid);
            }
            NoticeMsg::Read(id) => self.notices.mark_read(id),
            NoticeMsg::ReadAll => self.notices.mark_all_read(),
            NoticeMsg::Dismiss(id) => self.notices.remove(id),
            NoticeMsg::ClearFinished => self.notices.clear_finished(),
            NoticeMsg::HideToast(id) => self.notices.hide_toast(id),
        }
        iced::Task::none()
    }

    /// Shows the work page with `sid` selected.
    pub(crate) fn go_session(&mut self, sid: String) -> iced::Task<Message> {
        if self.live(&sid).is_none() {
            return iced::Task::none();
        }
        let page = if self.ui.page == crate::ui_state::Page::Work {
            iced::Task::none()
        } else {
            self.update(Message::Page(crate::ui_state::Page::Work))
        };
        page.chain(self.update(Message::Select(sid)))
    }

    /// The user looks at `sid`: its attention notices are read.
    pub(crate) fn notices_seen(&mut self, sid: &str) {
        self.notices.mark_key_read(&NoticeKey::Attention {
            session: sid.to_owned(),
        });
        self.notices.mark_key_read(&NoticeKey::Bell {
            session: sid.to_owned(),
        });
    }
}
