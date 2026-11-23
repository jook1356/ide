//! Right-click menu of an open item (Hermes Desk's `DetailItemMenu`), opened from the detail
//! window's tab list or the 열린 항목 popover, drawn at the pointer at the root of the window
//! that shows the detail pane.

use iced::widget::{Column, button, container, mouse_area, pin, space, stack, text};
use iced::{Element, Fill, Point, Task, Widget as _};

use crate::app::{App, Message};
use crate::detail::{CloseSet, DetailMsg, ItemKey, SideTab, split_q};
use crate::detail_view::commit_info;
use crate::i18n::tr;
use crate::style;

type El<'a> = Element<'a, Message>;

const MENU_W: f32 = 240.0;
const ITEM_H: f32 = 28.0;

/// An open item's menu: where it was asked for (window coordinates of the detail pane's window).
#[derive(Debug, Clone)]
pub struct ItemMenu {
    pub session: String,
    pub key: ItemKey,
    pub at: Point,
}

#[derive(Debug, Clone)]
pub enum ItemAct {
    CopyPath,
    CopyRelPath,
    RevealExplore,
    RevealOs,
    OpenVsCode,
    CopySha,
    CopySubject,
    CopyUrl,
    OpenExternal,
    CopyCommand,
    /// 공통 탭으로 고정 / 공통 해제.
    ToggleCommon,
}

/// Dragging an open item within its group (Desk's `useItemDrag`).
#[derive(Debug, Clone)]
pub enum DragMsg {
    Start(String, ItemKey),
    /// The pointer is over this item's top (`true`) or bottom half.
    Over(ItemKey, bool),
    End,
}

#[derive(Debug, Clone)]
pub struct ItemDrag {
    pub session: String,
    pub key: ItemKey,
    pub over: Option<(ItemKey, bool)>,
}

/// The 열린 항목 group of an item (rows move only within their group).
fn group(k: &ItemKey) -> u8 {
    match k {
        ItemKey::Web | ItemKey::Browser(_) => 0,
        ItemKey::Diff { .. } | ItemKey::Compare(_) => 1,
        ItemKey::Commit { .. } => 2,
        ItemKey::File(_) => 3,
        ItemKey::Terminal(_) => 4,
    }
}

impl App {
    /// An item row of the tab list or the popover: right-click menu, drag within its group
    /// (when the group has more than one item) and the drop line above / below it.
    pub(crate) fn item_row<'a>(&self, live: &crate::app::Live, row: El<'a>, key: &ItemKey) -> El<'a> {
        let sid = live.info.id.as_str();
        let drag = self.item_drag.as_ref().filter(|d| d.session == sid);
        let active = drag.is_some_and(|d| group(&d.key) == group(key));
        let mark = drag
            .filter(|d| active && d.key != *key)
            .and_then(|d| d.over.as_ref())
            .filter(|(k, _)| k == key)
            .map(|(_, top)| *top);
        let line: El<'a> = match mark {
            Some(top) => container(
                container(space())
                    .height(2)
                    .width(Fill)
                    .style(style::bg(style::ACCENT)),
            )
            .height(Fill)
            .align_y(if top {
                iced::Alignment::Start
            } else {
                iced::Alignment::End
            })
            .boxed(),
            None => space().boxed(),
        };
        let k = key.clone();
        let mut r = ide_views::Reorder::new(stack![row, line].boxed())
            .active(active)
            .on_over(move |top| Message::ItemDrag(DragMsg::Over(k.clone(), top)))
            .on_end(Message::ItemDrag(DragMsg::End));
        let peers = live.detail.items.iter().filter(|(k, _)| group(k) == group(key)).count();
        if peers > 1 {
            r = r.on_start(Message::ItemDrag(DragMsg::Start(sid.to_owned(), key.clone())));
        }
        with_menu(r.into(), sid, key)
    }

    pub(crate) fn on_item_drag(&mut self, m: DragMsg) -> Task<Message> {
        match m {
            DragMsg::Start(session, key) => {
                self.item_menu = None;
                self.item_drag = Some(ItemDrag {
                    session,
                    key,
                    over: None,
                });
            }
            DragMsg::Over(k, top) => {
                if let Some(d) = &mut self.item_drag {
                    d.over = Some((k, top));
                }
            }
            DragMsg::End => {
                let Some(d) = self.item_drag.take() else {
                    return Task::none();
                };
                let Some((target, top)) = d.over else {
                    return Task::none();
                };
                if target == d.key || group(&target) != group(&d.key) {
                    return Task::none();
                }
                let Some(det) = self.detail_mut(&d.session) else {
                    return Task::none();
                };
                let Some(from) = det.items.iter().position(|(k, _)| *k == d.key) else {
                    return Task::none();
                };
                let item = det.items.remove(from);
                let Some(at) = det.items.iter().position(|(k, _)| *k == target) else {
                    det.items.insert(from, item);
                    return Task::none();
                };
                det.items.insert(if top { at } else { at + 1 }, item);
                self.persist_detail(&d.session);
            }
        }
        Task::none()
    }
}

/// Wraps an item row: a right-click opens its menu at the pointer.
pub fn with_menu<'a>(row: El<'a>, sid: &str, key: &ItemKey) -> El<'a> {
    let (sid, key) = (sid.to_owned(), key.clone());
    ide_views::Interact::new(row)
        .absolute()
        .on_right_press(move |at| {
            Message::ItemMenu(Some(ItemMenu {
                session: sid.clone(),
                key: key.clone(),
                at,
            }))
        })
        .into()
}

impl App {
    pub(crate) fn on_item_menu(&mut self, m: Option<ItemMenu>) -> Task<Message> {
        if m.is_some() {
            // Drawn at the window's root, under an open popover: the popover goes.
            self.open_items = false;
            self.rail_menu = false;
        }
        self.item_menu = m;
        Task::none()
    }

    pub(crate) fn on_item_act(&mut self, act: ItemAct) -> Task<Message> {
        let Some(m) = self.item_menu.take() else {
            return Task::none();
        };
        let sid = m.session.as_str();
        let Some(live) = self.live(sid) else {
            return Task::none();
        };
        let path = match &m.key {
            ItemKey::File(p)
            | ItemKey::Diff { path: p, .. }
            | ItemKey::Compare(p)
            | ItemKey::Commit { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        };
        // The file on the server's disk: the repo's worktree + the repo-relative path.
        let abs = path.as_deref().and_then(|q| {
            let (repo, rel) = split_q(q);
            live.info
                .repos
                .iter()
                .find(|r| r.repo == repo)
                .map(|r| native(&format!("{}/{rel}", r.path.trim_end_matches(['/', '\\']))))
        });
        let rel = path.as_deref().map(|q| split_q(q).1.to_owned());
        let (sha, subject) = match &m.key {
            ItemKey::Commit { sha, .. } => (
                Some(sha.clone()),
                commit_info(&m.key, live).map(|c| c.subject.clone()),
            ),
            _ => (None, None),
        };
        let url = match &m.key {
            ItemKey::Browser(id) => self
                .browser(live, *id)
                .map(|b| b.url_input.clone())
                .unwrap_or_default(),
            _ => live.url_input.clone(),
        };
        let copy = |this: &mut Self, s: Option<String>, done: String| -> Task<Message> {
            let Some(s) = s else {
                return Task::none();
            };
            if let Some(d) = this.detail_mut(sid) {
                d.toast(done);
            }
            iced::clipboard::write(s).discard()
        };
        match act {
            ItemAct::CopyPath => copy(self, abs, tr::item_copied_path()),
            ItemAct::CopyRelPath => copy(self, rel, tr::item_copied_rel()),
            ItemAct::CopySha => copy(self, sha, tr::item_copied_sha()),
            ItemAct::CopySubject => copy(self, subject, tr::item_copied_subject()),
            ItemAct::CopyUrl => copy(self, Some(url), tr::item_copied_url()),
            ItemAct::ToggleCommon => {
                self.toggle_common(m.session.clone(), m.key.clone());
                Task::none()
            }
            ItemAct::CopyCommand => {
                let cmd = match &m.key {
                    ItemKey::Terminal(id) => self
                        .live(sid)
                        .and_then(|l| self.find_uterm(l, id))
                        .map(|t| t.info.command.clone()),
                    _ => None,
                };
                copy(self, cmd, tr::item_copied_command())
            }
            ItemAct::RevealExplore => {
                let Some(q) = path else {
                    return Task::none();
                };
                let side = self.on_detail(m.session.clone(), DetailMsg::Side(SideTab::Explore));
                Task::batch([side, self.tree_reveal(sid, &q)])
            }
            ItemAct::RevealOs | ItemAct::OpenVsCode | ItemAct::OpenExternal => {
                let r = match act {
                    ItemAct::RevealOs => abs.as_deref().map(reveal_in_file_manager),
                    ItemAct::OpenVsCode => abs.as_deref().map(open_in_vscode),
                    _ => Some(open_url(&url)),
                };
                if let Some(Err(e)) = r {
                    let msg = match act {
                        ItemAct::RevealOs => tr::item_reveal_os_failed(&e),
                        ItemAct::OpenVsCode => tr::item_vscode_failed(&e),
                        _ => tr::item_external_failed(&e),
                    };
                    if let Some(d) = self.detail_mut(sid) {
                        d.toast(msg);
                    }
                }
                Task::none()
            }
        }
    }

    /// The open item menu as a layer over window content: `space()` when it is not shown in
    /// this window (`detail_window`: the layer of the detail window, else of the app window).
    pub(crate) fn item_menu_layer(&self, detail_window: bool) -> El<'_> {
        let Some(m) = &self.item_menu else {
            return space().boxed();
        };
        if detail_window != self.ui.detail_detached {
            return space().boxed();
        }
        let Some(live) = self.live(&m.session) else {
            return space().boxed();
        };
        let sid = m.session.as_str();
        let local = self.target.local;
        let entry = |label: String, msg: Option<Message>| -> El<'_> {
            let fg = if msg.is_some() { style::TEXT } else { style::FAINT };
            button(text(label).size(13).color(fg))
                .padding([5, 10])
                .height(ITEM_H)
                .width(Fill)
                .style(style::row(false))
                .on_press_maybe(msg)
                .boxed()
        };
        let act = |a: ItemAct| Some(Message::ItemAct(a));
        let rule = || -> El<'_> {
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER))
                .boxed()
        };
        let close = |set: CloseSet| Some(Message::Detail(sid.to_owned(), DetailMsg::CloseSome(set)));
        let mut items: Vec<El<'_>> = Vec::new();
        // 공용 items are everyone's already; others can be pinned for every session.
        let global = match &m.key {
            ItemKey::Browser(id) => self.global_browsers.iter().any(|b| b.id == *id),
            ItemKey::Terminal(id) => id.starts_with("g:t"),
            _ => false,
        };
        if !global {
            let pinned = self.is_common(&m.session, &m.key);
            items.push(entry(
                if pinned { tr::common_unpin() } else { tr::common_pin() },
                act(ItemAct::ToggleCommon),
            ));
            items.push(rule());
        }
        match &m.key {
            ItemKey::Terminal(id) => {
                items.push(entry(tr::uterm_copy_command(), act(ItemAct::CopyCommand)));
                if self.find_uterm(live, id).is_some_and(|t| t.info.state == ide_client::SessionState::Running) {
                    items.push(entry(
                        tr::uterm_stop(),
                        Some(Message::UTerm(crate::uterm::UTermMsg::Stop(id.clone()))),
                    ));
                }
            }
            ItemKey::Browser(_) => {
                items.push(entry(tr::item_copy_url(), act(ItemAct::CopyUrl)));
                items.push(entry(tr::item_open_external(), act(ItemAct::OpenExternal)));
            }
            ItemKey::Web => {
                items.push(entry(tr::web_reload(), Some(Message::WebReload)));
                items.push(entry(tr::item_copy_url(), act(ItemAct::CopyUrl)));
                items.push(entry(tr::item_open_external(), act(ItemAct::OpenExternal)));
            }
            ItemKey::Commit { path: None, .. } => {
                items.push(entry(tr::item_copy_sha(), act(ItemAct::CopySha)));
                items.push(entry(tr::item_copy_subject(), act(ItemAct::CopySubject)));
            }
            ItemKey::Commit { path: Some(_), .. } => {
                items.push(entry(tr::item_copy_sha(), act(ItemAct::CopySha)));
                items.push(entry(tr::item_copy_subject(), act(ItemAct::CopySubject)));
                items.push(rule());
                items.push(entry(tr::item_copy_rel(), act(ItemAct::CopyRelPath)));
                items.push(entry(tr::chg_reveal(), act(ItemAct::RevealExplore)));
            }
            _ => {
                items.push(entry(tr::item_copy_path(), act(ItemAct::CopyPath)));
                items.push(entry(tr::item_copy_rel(), act(ItemAct::CopyRelPath)));
                items.push(entry(tr::chg_reveal(), act(ItemAct::RevealExplore)));
                // The server's disk is this computer's only with a local server.
                items.push(entry(reveal_os_label(), local.then(|| Message::ItemAct(ItemAct::RevealOs))));
                items.push(entry(tr::item_open_vscode(), local.then(|| Message::ItemAct(ItemAct::OpenVsCode))));
            }
        }
        let others = live.detail.items.iter().any(|(k, _)| *k != m.key);
        let any = !live.detail.items.is_empty();
        items.push(rule());
        if let ItemKey::Terminal(id) = &m.key {
            let running = live
                .uterm(id)
                .is_some_and(|t| t.info.state == ide_client::SessionState::Running);
            items.push(entry(
                if running { tr::uterm_stop_remove() } else { tr::uterm_remove() },
                Some(Message::UTerm(crate::uterm::UTermMsg::Remove(id.clone()))),
            ));
        } else {
            items.push(entry(
                tr::close(),
                Some(Message::Detail(sid.to_owned(), DetailMsg::Close(m.key.clone()))),
            ));
        }
        items.push(entry(tr::close_others(), others.then(|| close(CloseSet::Others(m.key.clone()))).flatten()));
        items.push(entry(tr::close_idle(), any.then(|| close(CloseSet::Idle)).flatten()));
        items.push(entry(tr::close_all(), any.then(|| close(CloseSet::All)).flatten()));

        let rows = items.len() as f32;
        let card = container(Column::with_children(items).spacing(1))
            .padding(4)
            .width(MENU_W)
            .style(style::modal_card);
        // Kept inside the window.
        let win = if detail_window {
            self.detail_window_size
        } else {
            self.window_size
        };
        let h = rows * (ITEM_H + 1.0) + 8.0;
        let x = m.at.x.min(win.width - MENU_W - 4.0).max(0.0);
        let y = m.at.y.min(win.height - h - 4.0).max(0.0);
        let backdrop = mouse_area(space().width(Fill).height(Fill))
            .on_press(Message::ItemMenu(None))
            .on_right_press(Message::ItemMenu(None));
        stack![backdrop, pin(card).x(x).y(y).width(Fill).height(Fill)]
            .width(Fill)
            .height(Fill)
            .boxed()
    }
}

/// `path` with the platform's separators.
fn native(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_owned()
    }
}

/// Show `path` selected in the OS file manager.
pub(crate) fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    let r = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{path}"))
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").args(["-R", path]).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let r = std::process::Command::new("xdg-open")
        .arg(std::path::Path::new(path).parent().unwrap_or(std::path::Path::new(path)))
        .spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// Open `path` in VS Code (its `code` command on the PATH).
fn open_in_vscode(path: &str) -> Result<(), String> {
    // `code` is a batch file on Windows: Rust quotes the argument for cmd.exe safely.
    let program = if cfg!(windows) { "code.cmd" } else { "code" };
    let mut cmd = std::process::Command::new(program);
    cmd.arg(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Open an http(s) `url` in the default browser.
fn open_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(url.to_owned());
    }
    #[cfg(windows)]
    let r = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// "<OS file manager>에서 보기" of the computer the app runs on (the local server's).
fn reveal_os_label() -> String {
    if cfg!(windows) {
        tr::item_reveal_windows()
    } else if cfg!(target_os = "macos") {
        tr::item_reveal_finder()
    } else {
        tr::item_reveal_files()
    }
}
