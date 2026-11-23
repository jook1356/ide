//! Right-click menu of a session row (Hermes Desk's `SessionSidebar` `rowMenu`), drawn at the
//! pointer at the root of the app window.
//!
//! Desk's items this app has no counterpart for are left out: 채팅에 첨부 / 마스터 세션에 첨부 (no
//! session attachments, no master session).

use iced::widget::{Column, button, container, mouse_area, pin, space, stack, text};
use iced::{Element, Fill, Point, Task, Widget as _};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::style;

type El<'a> = Element<'a, Message>;

const MENU_W: f32 = 200.0;
const ITEM_H: f32 = 28.0;

/// A session row's menu: the session and where it was asked for (app window coordinates).
#[derive(Debug, Clone)]
pub struct SessionMenu {
    pub session: String,
    pub at: Point,
}

#[derive(Debug, Clone)]
pub enum SessionAct {
    OpenFolder,
    CopyFolder,
    CopyId,
}

/// Wraps a session row: a right-click opens its menu at the pointer.
pub fn with_menu<'a>(row: El<'a>, sid: &str) -> El<'a> {
    let sid = sid.to_owned();
    ide_views::Interact::new(row)
        .absolute()
        .on_right_press(move |at| {
            Message::SessionMenu(Some(SessionMenu {
                session: sid.clone(),
                at,
            }))
        })
        .into()
}

impl App {
    pub(crate) fn on_session_act(&mut self, act: SessionAct) -> Task<Message> {
        let Some(m) = self.session_menu.take() else {
            return Task::none();
        };
        let Some(live) = self.live(&m.session) else {
            return Task::none();
        };
        let root = live.info.root.clone();
        // Shown in the selected session's detail pane (the one on screen).
        let shown = self.selected.clone().unwrap_or_else(|| m.session.clone());
        let toast = |this: &mut Self, tone: crate::notices::Tone, msg: crate::notices::Text| {
            if let Some(d) = this.detail_mut(&shown) {
                d.toast(tone, msg);
            }
        };
        match act {
            SessionAct::CopyFolder => {
                toast(self, crate::notices::Tone::Success, crate::notices::Text::tr(tr::session_copied_folder));
                iced::clipboard::write(root).discard()
            }
            SessionAct::CopyId => {
                toast(self, crate::notices::Tone::Success, crate::notices::Text::tr(tr::session_copied_id));
                iced::clipboard::write(m.session).discard()
            }
            SessionAct::OpenFolder => {
                if let Err(e) = open_folder(&root) {
                    toast(self, crate::notices::Tone::Error, tr::session_open_folder_failed(&e).into());
                }
                Task::none()
            }
        }
    }

    /// The session menu as a layer over the app window (`space()` when it is not open).
    pub(crate) fn session_menu_layer(&self) -> El<'_> {
        let Some(m) = &self.session_menu else {
            return space().boxed();
        };
        let Some(live) = self.live(&m.session) else {
            return space().boxed();
        };
        let sid = m.session.as_str();
        let online = self.connected();
        let has_root = !live.info.root.is_empty();
        let entry = |label: String, msg: Option<Message>, danger: bool| -> El<'_> {
            let fg = match (msg.is_some(), danger) {
                (false, _) => style::FAINT,
                (true, true) => style::RED,
                (true, false) => style::TEXT,
            };
            button(text(label).size(13).color(fg))
                .padding([5, 10])
                .height(ITEM_H)
                .width(Fill)
                .style(style::row(false))
                .on_press_maybe(msg)
                .boxed()
        };
        let act = |a: SessionAct| Some(Message::SessionAct(a));
        let rule = || -> El<'_> {
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER))
                .boxed()
        };
        // The working folder is on this computer's disk only with a local server.
        let local = self.target.local && has_root;
        let s = &live.info;
        let flags = |pinned: Option<bool>, archived: Option<bool>| {
            online.then(|| {
                Message::SessionMenuThen(Box::new(Message::SetSessionFlags(
                    sid.to_owned(),
                    pinned,
                    archived,
                )))
            })
        };
        let mut items: Vec<El<'_>> = Vec::new();
        // An archived session cannot be pinned (Desk's `canPin`).
        if !s.archived {
            items.push(entry(
                if s.pinned { tr::session_unpin() } else { tr::session_pin() },
                flags(Some(!s.pinned), None),
                false,
            ));
        }
        items.push(entry(
            if s.archived { tr::session_unarchive() } else { tr::session_archive() },
            flags(None, Some(!s.archived)),
            false,
        ));
        items.push(rule());
        // Session family (docs/session-family.md).
        let family = |m: crate::family_ui::FamilyMsg| {
            online.then(|| Message::SessionMenuThen(Box::new(Message::Family(m))))
        };
        if self.can_have_child(sid) {
            items.push(entry(
                tr::child_create_menu(),
                family(crate::family_ui::FamilyMsg::Open(sid.to_owned())),
                false,
            ));
        }
        if s.parent.is_some() && !s.shared {
            items.push(entry(
                tr::child_merge(),
                family(crate::family_ui::FamilyMsg::Merge(sid.to_owned())),
                false,
            ));
            items.push(entry(
                tr::child_sync(),
                family(crate::family_ui::FamilyMsg::Sync(sid.to_owned())),
                false,
            ));
        }
        if self.can_have_child(sid) || (s.parent.is_some() && !s.shared) {
            items.push(rule());
        }
        items.extend([
            entry(
                tr::session_open_folder(),
                local.then(|| Message::SessionAct(SessionAct::OpenFolder)),
                false,
            ),
            entry(
                tr::session_copy_folder(),
                has_root.then(|| act(SessionAct::CopyFolder)).flatten(),
                false,
            ),
            entry(tr::session_copy_id(), act(SessionAct::CopyId), false),
            rule(),
            entry(
                tr::session_settings(),
                online.then(|| Message::SessionMenuThen(Box::new(Message::SessionRepos(sid.to_owned())))),
                false,
            ),
            entry(
                tr::delete(),
                online.then(|| Message::SessionMenuThen(Box::new(Message::AskDelete(sid.to_owned())))),
                true,
            ),
        ]);
        let rows = items.len() as f32;
        let card = container(Column::with_children(items).spacing(1))
            .padding(4)
            .width(MENU_W)
            .style(style::modal_card);
        // Kept inside the window.
        let win = self.window_size;
        let h = rows * (ITEM_H + 1.0) + 8.0;
        let x = m.at.x.min(win.width - MENU_W - 4.0).max(0.0);
        let y = m.at.y.min(win.height - h - 4.0).max(0.0);
        let backdrop = mouse_area(space().width(Fill).height(Fill))
            .on_press(Message::SessionMenu(None))
            .on_right_press(Message::SessionMenu(None));
        stack![backdrop, pin(card).x(x).y(y).width(Fill).height(Fill)]
            .width(Fill)
            .height(Fill)
            .boxed()
    }
}

/// Open `path` (a folder) in the OS file manager.
fn open_folder(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    let r = std::process::Command::new("explorer.exe").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(path).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(path).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}
