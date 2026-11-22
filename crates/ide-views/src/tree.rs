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
    /// Delete was requested (menu or the Delete key) for the selection: confirm first.
    Delete(Vec<String>),
    /// Rows were dragged (pressed and moved): their paths. Released over a folder of the tree
    /// they become [`TreeEvent::Move`]; anywhere else [`TreeEvent::DragEnd`] (the owner may
    /// drop them elsewhere, e.g. into an input box).
    DragStart(Vec<String>),
    DragEnd,
    /// Move these entries into the folder `to_dir` (`""` = the root).
    Move { paths: Vec<String>, to_dir: String },
    /// Hermes Desk's Explore menu: 채팅에 첨부, 잘라내기 / 복사 / 붙여넣기 (into a folder),
    /// 탐색기에서 보기 (`None` = the root), 경로 복사 / 상대 경로 복사 / 원격 파일 URL 복사.
    Attach(Vec<String>),
    Cut(Vec<String>),
    Copy(Vec<String>),
    Paste(String),
    Reveal(Option<String>),
    CopyPath(String),
    CopyRelPath(String),
    CopyRemoteUrl(String),
}

#[derive(Debug, Clone)]
pub enum TreeMessage {
    /// A row was pressed and moved (a drag), the pointer is over a row during a drag, it left
    /// the tree, and the drag ended.
    DragStart(usize),
    DragOver(usize),
    DragLeave,
    DragEnd,
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
    NewFile,
    NewFolder,
    Attach,
    Cut,
    Copy,
    Paste,
    Reveal,
    CopyPath,
    CopyRelPath,
    CopyRemoteUrl,
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
    /// The selected entries (Ctrl+click toggles, Shift+click takes a range from `selected`,
    /// the anchor); `selected` alone when empty.
    selection: Vec<String>,
    /// Keyboard modifiers at the last click (set by the owner, see [`FileTree::set_modifiers`]).
    modifiers: Modifiers,
    /// The entries being dragged, and the folder they would move into.
    drag: Option<Vec<String>>,
    drop_dir: Option<String>,
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
            selection: Vec::new(),
            modifiers: Modifiers::empty(),
            drag: None,
            drop_dir: None,
        };
        t.rebuild();
        t
    }

    /// The keyboard modifiers for the next click (Ctrl toggles, Shift takes a range). The
    /// owner tracks them (the tree only sees key presses).
    pub fn set_modifiers(&mut self, m: Modifiers) {
        self.modifiers = m;
    }

    /// Whether `path` is selected.
    fn is_selected(&self, path: &str) -> bool {
        if self.selection.is_empty() {
            self.selected.as_deref() == Some(path)
        } else {
            self.selection.iter().any(|p| p == path)
        }
    }

    /// The selected paths, without those inside an also-selected folder (moved / deleted once
    /// with it; Desk's `topmost`).
    pub fn selected_paths(&self) -> Vec<String> {
        let all: Vec<String> = if self.selection.is_empty() {
            self.selected.clone().into_iter().collect()
        } else {
            self.selection.clone()
        };
        all.iter()
            .filter(|p| !all.iter().any(|o| o != *p && p.starts_with(&format!("{o}/"))))
            .cloned()
            .collect()
    }

    /// Select only `path` (it becomes the anchor).
    fn select_only(&mut self, path: &str) {
        self.selected = Some(path.to_owned());
        self.selection = vec![path.to_owned()];
    }

    /// The folder an entry stands for as a target: itself, or a file's folder.
    fn folder_for(&self, i: usize) -> Option<String> {
        let (p, e) = self.entry_at(i)?;
        Some(if e.is_dir {
            p.to_owned()
        } else {
            split(p).0.to_owned()
        })
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
        self.select_only(path);
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

    /// The visible row showing `path`, if any (scripts drive drags by row).
    pub fn row_of(&self, path: &str) -> Option<usize> {
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
        self.select_only(&path);
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
            TreeMessage::DragStart(i) => {
                let Some((path, _)) = self.entry_at(i) else {
                    return (Task::none(), None);
                };
                let path = path.to_owned();
                // The whole selection when the dragged row is in it, else that row alone.
                if !self.is_selected(&path) {
                    self.select_only(&path);
                }
                let paths = self.selected_paths();
                self.drag = Some(paths.clone());
                self.drop_dir = None;
                (Task::none(), Some(TreeEvent::DragStart(paths)))
            }
            TreeMessage::DragOver(i) => {
                if self.drag.is_some() {
                    self.drop_dir = self.folder_for(i);
                }
                (Task::none(), None)
            }
            TreeMessage::DragLeave => {
                self.drop_dir = None;
                (Task::none(), None)
            }
            TreeMessage::DragEnd => {
                let drag = self.drag.take();
                let target = self.drop_dir.take();
                match (drag, target) {
                    // Released over the tree: move what may move (not into itself, not where
                    // it already is; Desk).
                    (Some(paths), Some(to_dir)) => {
                        let movable: Vec<String> = paths
                            .into_iter()
                            .filter(|p| {
                                *p != to_dir
                                    && split(p).0 != to_dir
                                    && !to_dir.starts_with(&format!("{p}/"))
                            })
                            .collect();
                        let ev = (!movable.is_empty()).then_some(TreeEvent::Move {
                            paths: movable,
                            to_dir,
                        });
                        (Task::none(), ev.or(Some(TreeEvent::DragEnd)))
                    }
                    _ => (Task::none(), Some(TreeEvent::DragEnd)),
                }
            }
            TreeMessage::Click(i) => {
                self.menu = None;
                let Some((path, e)) = self.entry_at(i) else {
                    return (Task::none(), None);
                };
                let (path, is_dir) = (path.to_owned(), e.is_dir);
                let m = self.modifiers;
                if m.command() {
                    // Ctrl+click: toggle in the selection.
                    if self.selection.is_empty()
                        && let Some(s) = self.selected.clone()
                    {
                        self.selection.push(s);
                    }
                    if let Some(k) = self.selection.iter().position(|p| *p == path) {
                        self.selection.remove(k);
                    } else {
                        self.selection.push(path.clone());
                    }
                    self.selected = Some(path);
                    return (Task::none(), None);
                }
                if m.shift()
                    && let Some(a) = self.selected.clone().and_then(|s| self.row_of(&s))
                {
                    // Shift+click: the entries from the anchor to here.
                    let (lo, hi) = (a.min(i), a.max(i));
                    self.selection = (lo..=hi)
                        .filter_map(|k| self.entry_at(k).map(|(p, _)| p.to_owned()))
                        .collect();
                    return (Task::none(), None);
                }
                self.select_only(&path);
                if is_dir {
                    (Task::none(), self.toggle(&path))
                } else {
                    (Task::none(), Some(TreeEvent::Open(path)))
                }
            }
            TreeMessage::Context(i) => {
                // Right-clicking outside the selection targets just that row (VS Code, Desk).
                if let Some((path, _)) = self.entry_at(i) {
                    let path = path.to_owned();
                    if !self.is_selected(&path) {
                        self.select_only(&path);
                    }
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
                // The folder the menu targets: the row's folder, or the root.
                let folder = if menu.is_dir {
                    path.clone()
                } else {
                    split(&path).0.to_owned()
                };
                let some = |e: TreeEvent| (Task::none(), Some(e));
                match action {
                    MenuAction::Close => (Task::none(), None),
                    MenuAction::Attach => some(TreeEvent::Attach(self.selected_paths())),
                    MenuAction::Cut => some(TreeEvent::Cut(self.selected_paths())),
                    MenuAction::Copy => some(TreeEvent::Copy(self.selected_paths())),
                    MenuAction::Paste => some(TreeEvent::Paste(folder)),
                    MenuAction::Reveal => some(TreeEvent::Reveal(menu.path.clone())),
                    MenuAction::CopyPath => some(TreeEvent::CopyPath(path)),
                    MenuAction::CopyRelPath => some(TreeEvent::CopyRelPath(path)),
                    MenuAction::CopyRemoteUrl => some(TreeEvent::CopyRemoteUrl(path)),
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
                    MenuAction::Delete => (
                        Task::none(),
                        menu.path.map(|_| TreeEvent::Delete(self.selected_paths())),
                    ),
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
        // Ctrl/Cmd+C / X / V: copy, cut, paste (into the selected row's folder).
        if mods.command()
            && !mods.alt()
            && let Key::Character(c) = &key
        {
            let ev = match c.as_str() {
                "c" => Some(TreeEvent::Copy(self.selected_paths())),
                "x" => Some(TreeEvent::Cut(self.selected_paths())),
                "v" => Some(TreeEvent::Paste(
                    cur.and_then(|c| self.folder_for(c)).unwrap_or_default(),
                )),
                _ => None,
            };
            let ev = ev.filter(|e| !matches!(e, TreeEvent::Copy(p) | TreeEvent::Cut(p) if p.is_empty()));
            return (Task::none(), ev);
        }
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
            Named::Delete => {
                let paths = self.selected_paths();
                (Task::none(), (!paths.is_empty()).then_some(TreeEvent::Delete(paths)))
            }
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
        // A drag that leaves the tree no longer moves anything (it may drop elsewhere).
        let base = mouse_area(base).on_exit(TreeMessage::DragLeave);
        // Always a stack with the list first, so opening the menu keeps the list's scroll state.
        let Some(menu) = &self.menu else {
            return container(stack![base]).style(theme::bg(theme::PANEL)).boxed();
        };
        // Click anywhere else closes the menu.
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill))
            .on_press(TreeMessage::Menu(MenuAction::Close))
            .on_right_press(TreeMessage::Menu(MenuAction::Close));
        // Hermes Desk's Explore menu (`ExploreTree.tsx`): the same items, order and shortcuts.
        let item = |label: String, keys: &'static str, action: MenuAction| {
            button(
                row![
                    text(label).size(theme::UI_SIZE).width(Length::Fill),
                    text(keys).size(11).color(theme::FAINT),
                ]
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([4, 10])
            .style(theme::menu_item(action == MenuAction::Delete))
            .on_press(TreeMessage::Menu(action))
            .boxed()
        };
        let divider = || {
            container(space())
                .height(1)
                .width(Length::Fill)
                .style(theme::bg(theme::BORDER))
                .boxed()
        };
        let on_row = menu.path.is_some();
        let count = self.selected_paths().len();
        let single = on_row && count <= 1;
        let mut items = Column::new().spacing(1).width(Length::Fixed(220.0));
        items = items
            .push(item(tr::tree_new_file(), "", MenuAction::NewFile))
            .push(item(tr::tree_new_folder(), "", MenuAction::NewFolder))
            .push(divider());
        if on_row {
            let attach = if count > 1 {
                tr::tree_attach_many(count as i64)
            } else {
                tr::tree_attach()
            };
            items = items
                .push(item(attach, "", MenuAction::Attach))
                .push(divider())
                .push(item(tr::tree_cut(), "Ctrl+X", MenuAction::Cut))
                .push(item(tr::tree_copy(), "Ctrl+C", MenuAction::Copy));
        }
        items = items
            .push(item(tr::tree_paste(), "Ctrl+V", MenuAction::Paste))
            .push(divider())
            .push(item(reveal_os_label(), "", MenuAction::Reveal));
        if on_row {
            // Path items are for one entry only (VS Code's single-selection menu, Desk).
            if single {
                items = items
                    .push(divider())
                    .push(item(tr::item_copy_path(), "", MenuAction::CopyPath))
                    .push(item(tr::item_copy_rel(), "", MenuAction::CopyRelPath));
                if !menu.is_dir {
                    items = items.push(item(tr::tree_copy_remote_url(), "", MenuAction::CopyRemoteUrl));
                }
            }
            items = items.push(divider());
            if single {
                items = items.push(item(tr::rename(), "F2", MenuAction::Rename));
            }
            items = items.push(item(tr::delete(), "Del", MenuAction::Delete));
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
                let selected = self.is_selected(&r.path);
                // During a drag the folder it would move into is marked (Desk).
                let drop_here = e.is_dir && self.drop_dir.as_deref() == Some(r.path.as_str());
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
                let b: Element<'_, TreeMessage> = if drop_here {
                    container(b)
                        .style(|_| container::Style {
                            border: iced::Border {
                                color: theme::ACCENT,
                                width: 1.0,
                                radius: 3.0.into(),
                            },
                            ..container::Style::default()
                        })
                        .boxed()
                } else {
                    b.boxed()
                };
                // Pressing and moving a row drags it (the selection): over a folder of the
                // tree it moves there, elsewhere it goes to the owner (the input box). A plain
                // click still opens it.
                crate::Reorder::new(mouse_area(b).on_right_press(TreeMessage::Context(i)).boxed())
                    .on_start(TreeMessage::DragStart(i))
                    .on_end(TreeMessage::DragEnd)
                    .active(self.drag.is_some())
                    .on_over(move |_| TreeMessage::DragOver(i))
                    .boxed()
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

/// "<file manager> 에서 보기" in this OS's words.
fn reveal_os_label() -> String {
    if cfg!(windows) {
        tr::item_reveal_windows()
    } else if cfg!(target_os = "macos") {
        tr::item_reveal_finder()
    } else {
        tr::item_reveal_files()
    }
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
        assert_eq!(ev, Some(TreeEvent::Delete(vec!["src".into()])));
    }

    #[test]
    fn multi_selection_drag_move_and_menu() {
        let mut t = FileTree::new();
        t.set_children(
            "",
            vec![
                TreeEntry::dir("src"),
                TreeEntry::file("a.txt"),
                TreeEntry::file("b.txt"),
                TreeEntry::file("c.txt"),
            ],
        );
        // Rows: 0 src (collapsed), 1 a.txt, 2 b.txt, 3 c.txt.
        t.update(TreeMessage::Click(1));
        t.set_modifiers(Modifiers::SHIFT);
        t.update(TreeMessage::Click(3));
        assert_eq!(t.selected_paths(), ["a.txt", "b.txt", "c.txt"]);
        t.set_modifiers(Modifiers::CTRL);
        let (_, ev) = t.update(TreeMessage::Click(2));
        assert_eq!(ev, None, "a Ctrl+click only selects");
        assert_eq!(t.selected_paths(), ["a.txt", "c.txt"]);
        t.set_modifiers(Modifiers::empty());
        // Dragging a selected row drags the selection; over the folder and released: a move.
        let (_, ev) = t.update(TreeMessage::DragStart(3));
        assert_eq!(ev, Some(TreeEvent::DragStart(vec!["a.txt".into(), "c.txt".into()])));
        t.update(TreeMessage::DragOver(0));
        let (_, ev) = t.update(TreeMessage::DragEnd);
        assert_eq!(
            ev,
            Some(TreeEvent::Move {
                paths: vec!["a.txt".into(), "c.txt".into()],
                to_dir: "src".into()
            })
        );
        // Released outside the tree: the owner gets it.
        t.update(TreeMessage::DragStart(1));
        t.update(TreeMessage::DragOver(0));
        t.update(TreeMessage::DragLeave);
        let (_, ev) = t.update(TreeMessage::DragEnd);
        assert_eq!(ev, Some(TreeEvent::DragEnd));
        // Dropped where it already is: nothing moves.
        t.update(TreeMessage::DragStart(1));
        t.update(TreeMessage::DragOver(2));
        let (_, ev) = t.update(TreeMessage::DragEnd);
        assert_eq!(ev, Some(TreeEvent::DragEnd));
        // The menu on an unselected row targets it alone; paste goes into the row's folder.
        t.update(TreeMessage::Context(2));
        t.update(TreeMessage::ContextAt(Point::new(5.0, 5.0)));
        let (_, ev) = t.update(TreeMessage::Menu(MenuAction::Copy));
        assert_eq!(ev, Some(TreeEvent::Copy(vec!["b.txt".into()])));
        t.update(TreeMessage::Context(0));
        t.update(TreeMessage::ContextAt(Point::new(5.0, 5.0)));
        let (_, ev) = t.update(TreeMessage::Menu(MenuAction::Paste));
        assert_eq!(ev, Some(TreeEvent::Paste("src".into())));
        // Ctrl+C on the selection.
        let (_, ev) = t.update(TreeMessage::Key(Key::Character("c".into()), Modifiers::CTRL));
        assert_eq!(ev, Some(TreeEvent::Copy(vec!["src".into()])));
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
