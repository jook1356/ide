//! Lazy file tree: directories first, sorted; git status colours and letters; ignored entries
//! dimmed; keyboard navigation; inline rename / create; context menu. Virtualised (fixed 26 px
//! rows), so 10k+ visible entries scroll smoothly.
//!
//! Paths are `/`-separated and relative to the tree root (`""` is the root itself).

use crate::interact::Interact;
use crate::theme;
use crate::vlist::VList;
use crate::{GitStatus, icons};
use ide_i18n::tr;
use iced::Widget as _;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::scrollable::Viewport;
use iced::widget::{
    Column, button, container, mouse_area, row, space, stack, text, text_input,
};
use iced::{Alignment, Element, Length, Padding, Point, Task};
use std::collections::{HashMap, HashSet};

pub const ROW_H: f32 = 26.0;
const INDENT: f32 = 14.0;

/// One directory entry as the caller lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeEntry {
    /// File or folder name (no slashes).
    pub name: String,
    pub is_dir: bool,
    /// Git status of the file (or a summary for a folder); `None` = clean.
    pub status: Option<GitStatus>,
    /// Ignored by git (drawn dimmed).
    pub ignored: bool,
}

impl TreeEntry {
    pub fn file(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_dir: false,
            status: None,
            ignored: false,
        }
    }
    pub fn dir(name: impl Into<String>) -> Self {
        Self {
            is_dir: true,
            ..Self::file(name)
        }
    }
    pub fn with_status(mut self, status: GitStatus) -> Self {
        self.status = Some(status);
        self
    }
    pub fn ignored(mut self) -> Self {
        self.ignored = true;
        self
    }
}

/// What the tree asks its owner to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeEvent {
    /// A folder was expanded: list it and call [`FileTree::set_children`] (also sent when it is
    /// already loaded, so the owner may refresh it).
    Expand(String),
    Collapse(String),
    /// Open a file (click, Enter, or the menu).
    Open(String),
    /// The selection moved (keyboard or click).
    Select(String),
    CreateFile { dir: String, name: String },
    CreateFolder { dir: String, name: String },
    Rename { path: String, new_name: String },
    /// Delete was requested (menu or the Delete key): confirm first, then delete.
    Delete(String),
}

#[derive(Debug, Clone)]
pub enum TreeMessage {
    Click(usize),
    Context(usize),
    /// Right press anywhere in the tree (position for the menu).
    ContextAt(Point),
    Menu(MenuAction),
    Key(Key, Modifiers),
    EditInput(String),
    EditSubmit,
    Scrolled(Viewport),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    Close,
}

#[derive(Debug, Clone)]
enum RowKind {
    Entry(TreeEntry),
    Loading,
    /// Inline name input for a new file / folder inside `path`.
    Create { is_dir: bool },
}

#[derive(Debug, Clone)]
struct Row {
    /// Entry path, or the parent folder for `Loading` / `Create` rows.
    path: String,
    depth: u16,
    kind: RowKind,
}

#[derive(Debug, Clone)]
enum Edit {
    Rename { path: String, value: String },
    Create { dir: String, is_dir: bool, value: String },
}

#[derive(Debug, Clone)]
struct Menu {
    /// Target entry (`None` = the root, from a right-click on empty space).
    path: Option<String>,
    is_dir: bool,
    at: Point,
}

/// The tree's state. Keep it in the owner's state; feed [`TreeMessage`]s back through
/// [`FileTree::update`] and act on the returned [`TreeEvent`]s.
pub struct FileTree {
    dirs: HashMap<String, Vec<TreeEntry>>,
    expanded: HashSet<String>,
    selected: Option<String>,
    rows: Vec<Row>,
    list: VList,
    edit: Option<Edit>,
    menu: Option<Menu>,
    pending_menu_row: Option<usize>,
    focus_id: iced::widget::Id,
    input_id: iced::widget::Id,
}

impl Default for FileTree {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTree {
    /// An empty tree; the root (`""`) is expanded and waits for [`FileTree::set_children`].
    pub fn new() -> Self {
        let mut t = Self {
            dirs: HashMap::new(),
            expanded: HashSet::from([String::new()]),
            selected: None,
            rows: Vec::new(),
            list: VList::default(),
            edit: None,
            menu: None,
            pending_menu_row: None,
            focus_id: iced::widget::Id::unique(),
            input_id: iced::widget::Id::unique(),
        };
        t.rebuild();
        t
    }

    /// Sets (or refreshes) the children of folder `dir` (`""` = root). Sorted here: folders
    /// first, then case-insensitive by name.
    pub fn set_children(&mut self, dir: &str, mut entries: Vec<TreeEntry>) {
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        self.dirs.insert(dir.to_owned(), entries);
        self.rebuild();
    }

    /// Updates the git status of one entry (no-op when its folder is not loaded).
    pub fn set_status(&mut self, path: &str, status: Option<GitStatus>) {
        let (dir, name) = split(path);
        if let Some(e) = self
            .dirs
            .get_mut(dir)
            .and_then(|v| v.iter_mut().find(|e| e.name == name))
        {
            e.status = status;
            self.rebuild();
        }
    }

    /// Forgets the listing of `dir` and everything below it (e.g. after the folder was deleted).
    pub fn forget(&mut self, dir: &str) {
        let prefix = format!("{dir}/");
        self.dirs
            .retain(|k, _| k != dir && !(dir.is_empty() || k.starts_with(&prefix)));
        self.rebuild();
    }

    pub fn is_expanded(&self, dir: &str) -> bool {
        self.expanded.contains(dir)
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Number of rows currently shown (expanded entries).
    pub fn visible_rows(&self) -> usize {
        self.rows.len()
    }

    /// Selects `path` (expanding its loaded ancestors) and scrolls it into view. Returns the
    /// ancestor folders that still need [`FileTree::set_children`] as `Expand` events.
    pub fn reveal(&mut self, path: &str) -> (Task<TreeMessage>, Vec<TreeEvent>) {
        let mut events = Vec::new();
        let mut acc = String::new();
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            if self.expanded.insert(acc.clone()) && !self.dirs.contains_key(&acc) {
                events.push(TreeEvent::Expand(acc.clone()));
            }
        }
        self.selected = Some(path.to_owned());
        self.rebuild();
        let task = match self.row_of(path) {
            Some(i) => self.list.center(i, ROW_H),
            None => Task::none(),
        };
        (task, events)
    }

    /// Gives the tree keyboard focus.
    pub fn focus<M: Send + 'static>(&self) -> Task<M> {
        iced::widget::operation::focus(self.focus_id.clone())
    }

    fn row_of(&self, path: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r.kind, RowKind::Entry(_)) && r.path == path)
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        self.push_dir("", 0, &mut rows);
        self.rows = rows;
    }

    fn push_dir(&self, dir: &str, depth: u16, rows: &mut Vec<Row>) {
        if let Some(Edit::Create { dir: d, is_dir, .. }) = &self.edit
            && d == dir
        {
            rows.push(Row {
                path: dir.to_owned(),
                depth,
                kind: RowKind::Create { is_dir: *is_dir },
            });
        }
        let Some(entries) = self.dirs.get(dir) else {
            rows.push(Row {
                path: dir.to_owned(),
                depth,
                kind: RowKind::Loading,
            });
            return;
        };
        for e in entries {
            let path = join(dir, &e.name);
            let open = e.is_dir && self.expanded.contains(&path);
            rows.push(Row {
                path: path.clone(),
                depth,
                kind: RowKind::Entry(e.clone()),
            });
            if open {
                self.push_dir(&path, depth + 1, rows);
            }
        }
    }

    fn entry_at(&self, i: usize) -> Option<(&str, &TreeEntry)> {
        match self.rows.get(i) {
            Some(Row {
                path,
                kind: RowKind::Entry(e),
                ..
            }) => Some((path, e)),
            _ => None,
        }
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected.as_deref().and_then(|p| self.row_of(p))
    }

    fn toggle(&mut self, path: &str) -> Option<TreeEvent> {
        if self.expanded.remove(path) {
            self.rebuild();
            Some(TreeEvent::Collapse(path.to_owned()))
        } else {
            self.expanded.insert(path.to_owned());
            self.rebuild();
            Some(TreeEvent::Expand(path.to_owned()))
        }
    }

    fn select_index(&mut self, i: usize) -> (Task<TreeMessage>, Option<TreeEvent>) {
        let Some((path, _)) = self.entry_at(i) else {
            return (Task::none(), None);
        };
        let path = path.to_owned();
        self.selected = Some(path.clone());
        (self.list.ensure_visible(i, ROW_H), Some(TreeEvent::Select(path)))
    }

    fn start_edit(&mut self, edit: Edit) -> Task<TreeMessage> {
        self.edit = Some(edit);
        self.rebuild();
        Task::batch([
            iced::widget::operation::focus(self.input_id.clone()),
            iced::widget::operation::text_input::select_all(self.input_id.clone()),
        ])
    }

    fn cancel_edit(&mut self) -> Task<TreeMessage> {
        if self.edit.take().is_some() {
            self.rebuild();
        }
        self.focus()
    }

    pub fn update(&mut self, msg: TreeMessage) -> (Task<TreeMessage>, Option<TreeEvent>) {
        match msg {
            TreeMessage::Scrolled(v) => {
                self.list.on_scroll(&v);
                (Task::none(), None)
            }
            TreeMessage::Click(i) => {
                self.menu = None;
                let Some((path, e)) = self.entry_at(i) else {
                    return (Task::none(), None);
                };
                let (path, is_dir) = (path.to_owned(), e.is_dir);
                self.selected = Some(path.clone());
                if is_dir {
                    (Task::none(), self.toggle(&path))
                } else {
                    (Task::none(), Some(TreeEvent::Open(path)))
                }
            }
            TreeMessage::Context(i) => {
                if let Some((path, _)) = self.entry_at(i) {
                    self.selected = Some(path.to_owned());
                }
                self.pending_menu_row = Some(i);
                (Task::none(), None)
            }
            TreeMessage::ContextAt(at) => {
                let target = self.pending_menu_row.take().and_then(|i| self.entry_at(i));
                self.menu = Some(match target {
                    Some((path, e)) => Menu {
                        path: Some(path.to_owned()),
                        is_dir: e.is_dir,
                        at,
                    },
                    None => Menu {
                        path: None,
                        is_dir: true,
                        at,
                    },
                });
                (Task::none(), None)
            }
            TreeMessage::Menu(action) => {
                let Some(menu) = self.menu.take() else {
                    return (Task::none(), None);
                };
                let path = menu.path.clone().unwrap_or_default();
                match action {
                    MenuAction::Close => (Task::none(), None),
                    MenuAction::Open if menu.is_dir => (Task::none(), self.toggle(&path)),
                    MenuAction::Open => (Task::none(), Some(TreeEvent::Open(path))),
                    MenuAction::NewFile | MenuAction::NewFolder => {
                        // Create inside the folder (or next to the file).
                        let dir = if menu.is_dir {
                            path
                        } else {
                            split(&path).0.to_owned()
                        };
                        let is_dir = action == MenuAction::NewFolder;
                        let mut event = None;
                        if !dir.is_empty() && self.expanded.insert(dir.clone()) {
                            event = Some(TreeEvent::Expand(dir.clone()));
                        }
                        let task = self.start_edit(Edit::Create {
                            dir,
                            is_dir,
                            value: String::new(),
                        });
                        (task, event)
                    }
                    MenuAction::Rename => match menu.path {
                        Some(p) => {
                            let value = split(&p).1.to_owned();
                            (self.start_edit(Edit::Rename { path: p, value }), None)
                        }
                        None => (Task::none(), None),
                    },
                    MenuAction::Delete => (Task::none(), menu.path.map(TreeEvent::Delete)),
                }
            }
            TreeMessage::EditInput(v) => {
                match &mut self.edit {
                    Some(Edit::Rename { value, .. } | Edit::Create { value, .. }) => *value = v,
                    None => {}
                }
                (Task::none(), None)
            }
            TreeMessage::EditSubmit => {
                let Some(edit) = self.edit.take() else {
                    return (Task::none(), None);
                };
                self.rebuild();
                let event = match edit {
                    Edit::Rename { path, value } => {
                        let value = value.trim().to_owned();
                        (!value.is_empty() && value != split(&path).1).then_some(
                            TreeEvent::Rename {
                                path,
                                new_name: value,
                            },
                        )
                    }
                    Edit::Create { dir, is_dir, value } => {
                        let name = value.trim().to_owned();
                        (!name.is_empty()).then_some(if is_dir {
                            TreeEvent::CreateFolder { dir, name }
                        } else {
                            TreeEvent::CreateFile { dir, name }
                        })
                    }
                };
                (self.focus(), event)
            }
            TreeMessage::Key(key, mods) => self.on_key(key, mods),
        }
    }

    fn on_key(&mut self, key: Key, mods: Modifiers) -> (Task<TreeMessage>, Option<TreeEvent>) {
        if let Key::Named(Named::Escape) = key {
            self.menu = None;
            return (self.cancel_edit(), None);
        }
        if self.edit.is_some() {
            return (Task::none(), None);
        }
        let n = self.rows.len();
        let cur = self.selected_index();
        let Key::Named(named) = key else {
            return (Task::none(), None);
        };
        if mods.control() || mods.alt() {
            return (Task::none(), None);
        }
        match named {
            Named::ArrowDown => {
                let start = cur.map(|c| c + 1).unwrap_or(0);
                match (start..n).find(|&i| self.entry_at(i).is_some()) {
                    Some(i) => self.select_index(i),
                    None => (Task::none(), None),
                }
            }
            Named::ArrowUp => {
                let end = cur.unwrap_or(n);
                match (0..end).rev().find(|&i| self.entry_at(i).is_some()) {
                    Some(i) => self.select_index(i),
                    None => (Task::none(), None),
                }
            }
            Named::Home => match (0..n).find(|&i| self.entry_at(i).is_some()) {
                Some(i) => self.select_index(i),
                None => (Task::none(), None),
            },
            Named::End => match (0..n).rev().find(|&i| self.entry_at(i).is_some()) {
                Some(i) => self.select_index(i),
                None => (Task::none(), None),
            },
            Named::PageDown | Named::PageUp => {
                let page = (self.list.height() / ROW_H).floor().max(1.0) as usize;
                let c = cur.unwrap_or(0);
                let target = if named == Named::PageDown {
                    (c + page).min(n.saturating_sub(1))
                } else {
                    c.saturating_sub(page)
                };
                let found = (target..n)
                    .chain((0..target).rev())
                    .find(|&i| self.entry_at(i).is_some());
                match found {
                    Some(i) => self.select_index(i),
                    None => (Task::none(), None),
                }
            }
            Named::ArrowRight => {
                let Some(c) = cur else {
                    return (Task::none(), None);
                };
                let (path, is_dir) = {
                    let (p, e) = self.entry_at(c).unwrap();
                    (p.to_owned(), e.is_dir)
                };
                if !is_dir {
                    return (Task::none(), None);
                }
                if !self.expanded.contains(&path) {
                    (Task::none(), self.toggle(&path))
                } else if self.entry_at(c + 1).is_some() && self.rows[c + 1].depth > self.rows[c].depth
                {
                    self.select_index(c + 1)
                } else {
                    (Task::none(), None)
                }
            }
            Named::ArrowLeft => {
                let Some(c) = cur else {
                    return (Task::none(), None);
                };
                let (path, is_dir) = {
                    let (p, e) = self.entry_at(c).unwrap();
                    (p.to_owned(), e.is_dir)
                };
                if is_dir && self.expanded.contains(&path) {
                    (Task::none(), self.toggle(&path))
                } else {
                    let parent = split(&path).0;
                    match self.row_of(parent) {
                        Some(i) if !parent.is_empty() => self.select_index(i),
                        _ => (Task::none(), None),
                    }
                }
            }
            Named::Enter => {
                let Some((path, e)) = cur.and_then(|c| self.entry_at(c)) else {
                    return (Task::none(), None);
                };
                let (path, is_dir) = (path.to_owned(), e.is_dir);
                if is_dir {
                    (Task::none(), self.toggle(&path))
                } else {
                    (Task::none(), Some(TreeEvent::Open(path)))
                }
            }
            Named::F2 => match self.selected.clone() {
                Some(p) => {
                    let value = split(&p).1.to_owned();
                    (self.start_edit(Edit::Rename { path: p, value }), None)
                }
                None => (Task::none(), None),
            },
            Named::Delete => (Task::none(), self.selected.clone().map(TreeEvent::Delete)),
            _ => (Task::none(), None),
        }
    }

    pub fn view(&self) -> Element<'_, TreeMessage> {
        let list = self.list.view(
            self.rows.len(),
            ROW_H,
            None,
            move |i| self.row_view(i),
            TreeMessage::Scrolled,
        );
        let base = Interact::new(list)
            .id(self.focus_id.clone())
            .on_key(|k, m| Some(TreeMessage::Key(k.clone(), m)))
            .on_right_press(TreeMessage::ContextAt);
        // Always a stack with the list first, so opening the menu keeps the list's scroll state.
        let Some(menu) = &self.menu else {
            return container(stack![base]).style(theme::bg(theme::PANEL)).boxed();
        };
        // Click anywhere else closes the menu.
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill))
            .on_press(TreeMessage::Menu(MenuAction::Close))
            .on_right_press(TreeMessage::Menu(MenuAction::Close));
        let item = |label: String, action: MenuAction| {
            button(text(label).size(theme::UI_SIZE))
                .width(Length::Fill)
                .padding([4, 10])
                .style(theme::menu_item(action == MenuAction::Delete))
                .on_press(TreeMessage::Menu(action))
                .boxed()
        };
        let mut items = Column::new().spacing(1).width(Length::Fixed(170.0));
        if menu.path.is_some() {
            items = items.push(item(tr::tree_open(), MenuAction::Open));
        }
        items = items
            .push(item(tr::tree_new_file(), MenuAction::NewFile))
            .push(item(tr::tree_new_folder(), MenuAction::NewFolder));
        if menu.path.is_some() {
            items = items
                .push(item(tr::rename(), MenuAction::Rename))
                .push(item(tr::delete(), MenuAction::Delete));
        }
        let card = container(items).padding(4).style(theme::card);
        let placed = container(card).padding(Padding {
            top: menu.at.y,
            left: menu.at.x,
            ..Padding::ZERO
        });
        container(stack![base, backdrop, placed])
            .style(theme::bg(theme::PANEL))
            .boxed()
    }

    fn row_view(&self, i: usize) -> Element<'_, TreeMessage> {
        let r = &self.rows[i];
        let pad = 8.0 + r.depth as f32 * INDENT;
        match &r.kind {
            RowKind::Loading => row![
                space().width(pad + 16.0),
                text(tr::tree_loading()).size(12).color(theme::FAINT)
            ]
            .height(ROW_H)
            .align_y(Alignment::Center)
            .boxed(),
            RowKind::Create { is_dir } => {
                let icon = if *is_dir {
                    icons::folder_icon(false, theme::DIM)
                } else {
                    icons::file_icon("")
                };
                let value = match &self.edit {
                    Some(Edit::Create { value, .. }) => value.as_str(),
                    _ => "",
                };
                row![
                    space().width(pad + 16.0),
                    icon,
                    self.input(value),
                ]
                .spacing(6)
                .padding(Padding {
                    right: 8.0,
                    ..Padding::ZERO
                })
                .height(ROW_H)
                .align_y(Alignment::Center)
                .boxed()
            }
            RowKind::Entry(e) => {
                let open = e.is_dir && self.expanded.contains(&r.path);
                let renaming = matches!(&self.edit, Some(Edit::Rename { path, .. }) if *path == r.path);
                let color = if e.ignored {
                    theme::FAINT
                } else {
                    e.status.map(GitStatus::color).unwrap_or(theme::TEXT)
                };
                let mut line = row![space().width(pad)].spacing(6).align_y(Alignment::Center);
                if e.is_dir {
                    line = line.push(
                        text(if open { "▾" } else { "▸" })
                            .size(12)
                            .color(theme::DIM)
                            .width(10)
                            .boxed(),
                    );
                    line = line.push(icons::folder_icon(open, if e.ignored { theme::FAINT } else { theme::DIM }));
                } else {
                    line = line.push(space().width(10).boxed());
                    line = line.push(icons::file_icon(&e.name));
                }
                if renaming {
                    let value = match &self.edit {
                        Some(Edit::Rename { value, .. }) => value.as_str(),
                        _ => "",
                    };
                    return line
                        .push(self.input(value))
                        .padding(Padding {
                            right: 8.0,
                            ..Padding::ZERO
                        })
                        .height(ROW_H)
                        .boxed();
                }
                line = line.push(
                    text(e.name.as_str())
                        .size(theme::UI_SIZE)
                        .color(color)
                        .wrapping(text::Wrapping::None)
                        .width(Length::Fill)
                        .boxed(),
                );
                if let Some(s) = e.status {
                    line = line.push(
                        text(s.letter().to_string())
                            .size(11)
                            .font(theme::MONO_BOLD)
                            .color(if e.ignored { theme::FAINT } else { s.color() })
                            .boxed(),
                    );
                }
                let selected = self.selected.as_deref() == Some(r.path.as_str());
                let b = button(
                    line.padding(Padding {
                        right: 10.0,
                        ..Padding::ZERO
                    })
                    .height(ROW_H),
                )
                .padding(0)
                .width(Length::Fill)
                .style(theme::row(selected))
                .on_press(TreeMessage::Click(i));
                mouse_area(b).on_right_press(TreeMessage::Context(i)).boxed()
            }
        }
    }

    fn input<'a>(&self, value: &'a str) -> Element<'a, TreeMessage> {
        text_input(tr::tree_name_placeholder(), value)
            .id(self.input_id.clone())
            .size(theme::UI_SIZE)
            .padding([2, 6])
            .style(theme::input)
            .on_input(TreeMessage::EditInput)
            .on_submit(TreeMessage::EditSubmit)
            .boxed()
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// `("a/b", "c")` for `"a/b/c"`; `("", "c")` for `"c"`.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

#[cfg(test)]
mod tests {
    #![allow(unused_must_use)]
    use super::*;

    fn names(t: &FileTree) -> Vec<String> {
        t.rows
            .iter()
            .map(|r| match &r.kind {
                RowKind::Entry(_) => r.path.clone(),
                RowKind::Loading => format!("<loading {}>", r.path),
                RowKind::Create { .. } => format!("<create {}>", r.path),
            })
            .collect()
    }

    #[test]
    fn sorts_dirs_first_and_expands_lazily() {
        let mut t = FileTree::new();
        assert_eq!(names(&t), ["<loading >"]);
        t.set_children(
            "",
            vec![
                TreeEntry::file("b.rs"),
                TreeEntry::dir("src"),
                TreeEntry::file("A.md"),
                TreeEntry::dir("assets"),
            ],
        );
        assert_eq!(names(&t), ["assets", "src", "A.md", "b.rs"]);
        let (_, ev) = t.update(TreeMessage::Click(1));
        assert_eq!(ev, Some(TreeEvent::Expand("src".into())));
        assert_eq!(names(&t), ["assets", "src", "<loading src>", "A.md", "b.rs"]);
        t.set_children("src", vec![TreeEntry::file("main.rs")]);
        assert_eq!(names(&t), ["assets", "src", "src/main.rs", "A.md", "b.rs"]);
    }

    #[test]
    fn keyboard_navigation() {
        let mut t = FileTree::new();
        t.set_children("", vec![TreeEntry::dir("src"), TreeEntry::file("x.txt")]);
        t.set_children("src", vec![TreeEntry::file("lib.rs")]);
        let key = |n| TreeMessage::Key(Key::Named(n), Modifiers::empty());
        let (_, ev) = t.update(key(Named::ArrowDown));
        assert_eq!(ev, Some(TreeEvent::Select("src".into())));
        let (_, ev) = t.update(key(Named::ArrowRight));
        assert_eq!(ev, Some(TreeEvent::Expand("src".into())));
        let (_, ev) = t.update(key(Named::ArrowRight));
        assert_eq!(ev, Some(TreeEvent::Select("src/lib.rs".into())));
        let (_, ev) = t.update(key(Named::Enter));
        assert_eq!(ev, Some(TreeEvent::Open("src/lib.rs".into())));
        let (_, ev) = t.update(key(Named::ArrowLeft));
        assert_eq!(ev, Some(TreeEvent::Select("src".into())));
        let (_, ev) = t.update(key(Named::ArrowLeft));
        assert_eq!(ev, Some(TreeEvent::Collapse("src".into())));
        let (_, ev) = t.update(key(Named::Delete));
        assert_eq!(ev, Some(TreeEvent::Delete("src".into())));
    }

    #[test]
    fn inline_rename_and_create() {
        let mut t = FileTree::new();
        t.set_children("", vec![TreeEntry::dir("src"), TreeEntry::file("x.txt")]);
        t.update(TreeMessage::Context(1));
        t.update(TreeMessage::ContextAt(Point::new(10.0, 40.0)));
        t.update(TreeMessage::Menu(MenuAction::Rename));
        t.update(TreeMessage::EditInput("y.txt".into()));
        let (_, ev) = t.update(TreeMessage::EditSubmit);
        assert_eq!(
            ev,
            Some(TreeEvent::Rename {
                path: "x.txt".into(),
                new_name: "y.txt".into()
            })
        );
        t.update(TreeMessage::Context(0));
        t.update(TreeMessage::ContextAt(Point::new(10.0, 10.0)));
        let (_, ev) = t.update(TreeMessage::Menu(MenuAction::NewFile));
        assert_eq!(ev, Some(TreeEvent::Expand("src".into())));
        assert_eq!(names(&t)[..3], ["src", "<create src>", "<loading src>"]);
        t.update(TreeMessage::EditInput("new.rs".into()));
        let (_, ev) = t.update(TreeMessage::EditSubmit);
        assert_eq!(
            ev,
            Some(TreeEvent::CreateFile {
                dir: "src".into(),
                name: "new.rs".into()
            })
        );
    }
}
