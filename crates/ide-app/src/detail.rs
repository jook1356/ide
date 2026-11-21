//! The per-session detail area (Hermes Desk `DetailTabs` + `ChangesPanel`): open items (the
//! session's web page, files, diffs) shown one at a time under a header with an "open items"
//! switcher, and a side panel with Changes / History / Explore (Explore has the file-name and
//! text search on top of its tree; History is in [`crate::history`]).
//!
//! All state lives in [`Detail`], one per session ([`crate::app::Live::detail`]); switching
//! sessions switches the detail area. Everything on disk is reached through the server
//! (`fs.*`, `git.*`, `search.*`, `def.lookup`); the visible session is watched (`fs.watch`) and
//! refreshed from `FsChanged` / `GitChanged`.

use std::collections::{BTreeSet, HashSet};

use crate::history::{CommitFiles, HistoryState};
use std::time::{Duration, Instant};

use iced::Task;
use ide_client::{
    ChangeKind, Client, CommitInfo, DefResult, DiffAgainst, EntryKind, FileContent, FileMatch,
    FsEntry, GitChange, GitLog, ProtoError, SearchEvent, SearchMatch, SearchStream, SearchSummary, TextEncoding, TextSearch,
};
use ide_views::{
    ChangeEntry, ChangeGroup, ChangesEvent, ChangesList, ChangesMessage, CodeEvent, CodeMessage,
    CodeView, DiffMode, DiffView, DiffViewEvent, DiffViewMessage, FileTree, TreeEntry, TreeEvent,
    TreeMessage,
};

use crate::app::{App, Message, Modal};
use crate::i18n::{self, tr};
use crate::ui_state::DetailUi;

/// Width of the side panel (Changes / History / Explore).
pub const SIDE_W: f32 = 290.0;
/// How long a transient message stays.
pub const TOAST_FOR: Duration = Duration::from_secs(5);
/// Widget ids (`SEARCH_ID`: Explore's search input).
pub const SEARCH_ID: &str = "ide-search";
pub const QUICK_ID: &str = "ide-quick-open";
/// Most quick-open results.
const QUICK_MAX: u32 = 60;
/// Most file-name matches shown in Explore.
const FIND_FILES_MAX: u32 = 200;
/// Typing pause before Explore's text search starts (file names are matched at once).
const SEARCH_DELAY: Duration = Duration::from_millis(250);
/// Largest file the editor opens (bytes; bigger files show the first part read-only).
const READ_MAX: u64 = 4 * 1024 * 1024;

/// Side panel tab, in tab-bar order (Hermes Desk: Changes | History | Explore). A new tab: add
/// the variant here in Desk order, its `code` / `parse` arm (the `ui.json` value), and its
/// `tab(..)` + content arm in `detail_view.rs::side_panel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideTab {
    Changes,
    History,
    Explore,
}

impl SideTab {
    pub fn code(self) -> &'static str {
        match self {
            SideTab::Changes => "changes",
            SideTab::History => "history",
            SideTab::Explore => "explore",
        }
    }

    /// `ui.json` value; the old `"search"` tab is now part of Explore.
    fn parse(s: &str) -> Self {
        match s {
            "changes" => SideTab::Changes,
            "history" => SideTab::History,
            _ => SideTab::Explore,
        }
    }
}

/// Identity of an open item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemKey {
    /// The session's web page (always open).
    Web,
    /// A file in the editor ([`qualify`]d path).
    File(String),
    /// A changed file's diff (`staged` = index vs HEAD, else working tree vs index;
    /// [`qualify`]d path).
    Diff { staged: bool, path: String },
    /// A file on disk compared with the unsaved buffer (save conflict; [`qualify`]d path).
    Compare(String),
    /// A commit of `repo` (History) compared with its first parent: the whole commit, or one
    /// root-relative `path` of it.
    Commit {
        repo: String,
        sha: String,
        path: Option<String>,
    },
}

impl ItemKey {
    /// Stable string for `ui.json` (`Compare` is not remembered).
    pub fn encode(&self) -> Option<String> {
        Some(match self {
            ItemKey::Web => "web".into(),
            ItemKey::File(p) => format!("file:{p}"),
            ItemKey::Diff { staged: true, path } => format!("diff:s:{path}"),
            ItemKey::Diff {
                staged: false,
                path,
            } => format!("diff:w:{path}"),
            ItemKey::Compare(_) => return None,
            ItemKey::Commit { repo, sha, path } => {
                format!("commit:{repo}:{sha}:{}", path.as_deref().unwrap_or_default())
            }
        })
    }

    pub fn decode(s: &str) -> Option<Self> {
        if s == "web" {
            return Some(ItemKey::Web);
        }
        if let Some(rest) = s.strip_prefix("commit:") {
            // `commit:<repo>:<sha>:<path>` (empty path = the whole commit).
            let (repo, rest) = rest.split_once(':')?;
            let (sha, path) = rest.split_once(':')?;
            let hex = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_hexdigit());
            return (hex(repo) && hex(sha)).then(|| ItemKey::Commit {
                repo: repo.into(),
                sha: sha.into(),
                path: (!path.is_empty()).then(|| path.into()),
            });
        }
        if let Some(p) = s.strip_prefix("file:") {
            return (!p.is_empty()).then(|| ItemKey::File(p.into()));
        }
        let (staged, p) = if let Some(p) = s.strip_prefix("diff:s:") {
            (true, p)
        } else {
            (false, s.strip_prefix("diff:w:")?)
        };
        (!p.is_empty()).then(|| ItemKey::Diff {
            staged,
            path: p.into(),
        })
    }

    /// The key with a [`qualify`]d path: plain paths (saved before multi-repo sessions) get
    /// `repo`; `None` when there is none.
    pub fn qualified(self, repo: Option<&str>) -> Option<Self> {
        let q = |p: String| -> Option<String> {
            if !split_q(&p).0.is_empty() {
                Some(p)
            } else {
                repo.map(|r| qualify(r, &p))
            }
        };
        Some(match self {
            ItemKey::Web => ItemKey::Web,
            ItemKey::File(p) => ItemKey::File(q(p)?),
            ItemKey::Diff { staged, path } => ItemKey::Diff {
                staged,
                path: q(path)?,
            },
            ItemKey::Compare(p) => ItemKey::Compare(q(p)?),
            k @ ItemKey::Commit { .. } => k,
        })
    }

    /// The repo of a file / diff key.
    pub fn repo(&self) -> Option<&str> {
        match self {
            ItemKey::Web => None,
            ItemKey::File(p) | ItemKey::Diff { path: p, .. } | ItemKey::Compare(p) => {
                Some(split_q(p).0)
            }
            ItemKey::Commit { repo, .. } => Some(repo),
        }
    }
}

/// A bar over an open file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileBar {
    /// Saving failed: the file changed on disk since it was read.
    Conflict,
    /// The file changed on disk while the buffer has unsaved edits.
    ChangedOnDisk,
    /// The file was deleted on disk.
    Deleted,
}

/// An open file.
pub struct FileItem {
    pub path: String,
    pub view: Option<CodeView>,
    pub loading: bool,
    pub error: Option<String>,
    pub binary: bool,
    pub truncated: bool,
    pub size: u64,
    /// Version the buffer was read / saved at (`fs.write` conflict check).
    pub version: Option<String>,
    pub encoding: Option<TextEncoding>,
    pub bar: Option<FileBar>,
    pub saving: bool,
    /// Close the item once the save in flight succeeded.
    pub close_after_save: bool,
    /// Go to (line, column) once loaded.
    pub pending_goto: Option<(usize, usize)>,
}

impl FileItem {
    fn new(path: &str) -> Self {
        Self {
            path: path.into(),
            view: None,
            loading: false,
            error: None,
            binary: false,
            truncated: false,
            size: 0,
            version: None,
            encoding: None,
            bar: None,
            saving: false,
            close_after_save: false,
            pending_goto: None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.view.as_ref().is_some_and(CodeView::is_dirty)
    }
}

/// An open diff (or a disk-vs-buffer comparison).
pub struct DiffItem {
    pub view: Option<DiffView>,
    pub loading: bool,
    pub error: Option<String>,
    /// The commit of an [`ItemKey::Commit`] (title / subtitle), when known.
    pub commit: Option<CommitInfo>,
    /// Script mode only: the added lines of the loaded diff (checked by the app script).
    pub script_adds: Vec<String>,
}

impl DiffItem {
    fn empty() -> Self {
        Self {
            view: None,
            loading: false,
            error: None,
            commit: None,
            script_adds: Vec::new(),
        }
    }
}

// A handful of items per session: boxing the file variant would only add indirection.
#[allow(clippy::large_enum_variant)]
pub enum Item {
    Web,
    File(FileItem),
    Diff(DiffItem),
}

/// Explore's search (Hermes Desk's 파일 찾기 plus the text search): a non-empty query shows
/// the file-name matches of the shown repo and the text matches of every repo instead of the
/// tree.
#[derive(Default)]
pub struct SearchState {
    pub query: String,
    /// File-name matches in the shown repo (`search.files`), best first.
    pub files: Vec<FileMatch>,
    pub files_error: Option<String>,
    /// Incremented per file-name query; older answers are dropped.
    files_seq: u64,
    /// Incremented per typed query; only the last one's pause starts the text search.
    typed: u64,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Results grouped by file, in arrival order.
    pub groups: Vec<(String, Vec<SearchMatch>)>,
    pub collapsed: HashSet<String>,
    pub running: bool,
    pub summary: Option<SearchSummary>,
    pub error: Option<String>,
    /// Incremented per search; events of older searches are dropped.
    pub generation: u64,
    handle: Option<iced::task::Handle>,
}

impl SearchState {
    pub fn hit_count(&self) -> usize {
        self.groups.iter().map(|(_, h)| h.len()).sum()
    }

    /// The results replace Explore's tree.
    pub fn active(&self) -> bool {
        !self.query.is_empty()
    }

    /// Empty query: back to the tree.
    fn clear(&mut self) {
        self.stop();
        self.query.clear();
        self.generation += 1;
        self.files_seq += 1;
        self.typed += 1;
        self.groups.clear();
        self.collapsed.clear();
        self.files.clear();
        self.files_error = None;
        self.summary = None;
        self.error = None;
    }

    fn stop(&mut self) {
        // Aborting drops the `SearchStream`, which cancels the search on the server.
        if let Some(h) = self.handle.take() {
            h.abort();
        }
        self.running = false;
    }

    /// Group hits by file (key: [`qualify`]d path); files of one repo stay together (the view
    /// shows a repo header above each repo's files).
    fn add(&mut self, hits: Vec<SearchMatch>) {
        for h in hits {
            let key = qualify(&h.repo, &h.path);
            match self.groups.iter_mut().rev().find(|(p, _)| *p == key) {
                Some((_, v)) => v.push(h),
                None => {
                    let at = self
                        .groups
                        .iter()
                        .rposition(|(p, _)| split_q(p).0 == h.repo)
                        .map_or(self.groups.len(), |i| i + 1);
                    self.groups.insert(at, (key, vec![h]));
                }
            }
        }
    }
}

/// The detail area of one session.
pub struct Detail {
    /// The repo shown by Changes and Explore (Hermes Desk's per-tab Changes folder); `""` =
    /// none (a session without repos).
    pub repo: String,
    pub items: Vec<(ItemKey, Item)>,
    pub active: ItemKey,
    pub side: SideTab,
    pub tree: FileTree,
    /// Folders listed into the tree (re-listed on changes).
    pub loaded_dirs: BTreeSet<String>,
    pub root_loading: bool,
    pub tree_error: Option<String>,
    pub changes: ChangesList,
    pub status_loaded: bool,
    pub status_error: Option<String>,
    pub search: SearchState,
    /// History of the picked repo.
    pub history: HistoryState,
    pub diff_mode: DiffMode,
    /// Repos with `fs.watch` requested on the current client.
    pub watched: BTreeSet<String>,
    /// Shown state may be outdated (not watched for a while, reconnect): refresh when shown.
    pub stale: bool,
    pub toast: Option<(String, Instant)>,
}

impl Detail {
    /// `first_repo`: the session's first repo; files saved before multi-repo sessions (plain
    /// paths) belong to it.
    pub fn new(saved: Option<&DetailUi>, first_repo: Option<&str>) -> Self {
        let mut items: Vec<(ItemKey, Item)> = vec![(ItemKey::Web, Item::Web)];
        let mut active = ItemKey::Web;
        let mut side = SideTab::Explore;
        if let Some(ui) = saved {
            let fix = |k: ItemKey| k.qualified(first_repo);
            for k in ui.open.iter().filter_map(|s| ItemKey::decode(s)).filter_map(fix) {
                if items.iter().any(|(x, _)| *x == k) {
                    continue;
                }
                let item = match &k {
                    ItemKey::Web | ItemKey::Compare(_) => continue,
                    ItemKey::File(p) => Item::File(FileItem::new(p)),
                    ItemKey::Diff { .. } | ItemKey::Commit { .. } => Item::Diff(DiffItem::empty()),
                };
                items.push((k, item));
            }
            if let Some(a) = ui.active.as_deref().and_then(ItemKey::decode).and_then(fix)
                && items.iter().any(|(k, _)| *k == a)
            {
                active = a;
            }
            side = SideTab::parse(&ui.side);
        }
        Self {
            repo: first_repo.unwrap_or_default().to_owned(),
            items,
            active,
            side,
            tree: FileTree::new(),
            loaded_dirs: BTreeSet::new(),
            root_loading: false,
            tree_error: None,
            changes: ChangesList::new(),
            status_loaded: false,
            status_error: None,
            search: SearchState::default(),
            history: HistoryState::default(),
            diff_mode: DiffMode::Unified,
            watched: BTreeSet::new(),
            stale: false,
            toast: None,
        }
    }

    pub fn to_ui(&self) -> DetailUi {
        DetailUi {
            open: self.items.iter().filter_map(|(k, _)| k.encode()).collect(),
            active: self.active.encode(),
            side: self.side.code().into(),
        }
    }

    pub fn item(&self, key: &ItemKey) -> Option<&Item> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, i)| i)
    }

    pub fn item_mut(&mut self, key: &ItemKey) -> Option<&mut Item> {
        self.items
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, i)| i)
    }

    pub fn file(&self, path: &str) -> Option<&FileItem> {
        match self.item(&ItemKey::File(path.into())) {
            Some(Item::File(f)) => Some(f),
            _ => None,
        }
    }

    pub fn file_mut(&mut self, path: &str) -> Option<&mut FileItem> {
        match self.item_mut(&ItemKey::File(path.into())) {
            Some(Item::File(f)) => Some(f),
            _ => None,
        }
    }

    pub fn active_item(&self) -> Option<&Item> {
        self.item(&self.active)
    }

    /// Show another repo in Changes / Explore: its tree and status are loaded afresh.
    pub fn set_repo(&mut self, repo: String) {
        if self.repo == repo {
            return;
        }
        self.repo = repo;
        self.tree = FileTree::new();
        self.loaded_dirs.clear();
        self.root_loading = false;
        self.tree_error = None;
        self.changes = ChangesList::new();
        self.status_loaded = false;
        self.status_error = None;
        self.history = HistoryState::default();
    }

    /// Show a message for a few seconds.
    pub fn toast(&mut self, msg: String) {
        self.toast = Some((msg, Instant::now()));
    }
}

/// What a file read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadReason {
    /// Opening the item.
    Open,
    /// The file may have changed on disk.
    Refresh,
    /// Drop the buffer and take the disk content ("reload" on a bar).
    Force,
    /// Compare the disk content with the buffer.
    Compare,
}

/// A finished `fs.*` mutation from the tree.
#[derive(Debug, Clone)]
pub enum FsOp {
    Created { path: String, is_dir: bool },
    Renamed { from: String, to: String },
    Deleted { path: String },
}

/// One item of the search task's stream.
#[derive(Debug, Clone)]
pub enum SearchItem {
    Event(SearchEvent),
    Failed(ProtoError),
    /// The connection dropped before the search ended.
    Lost,
}

/// Messages of the detail area of session `.0` of [`Message::Detail`].
#[derive(Debug, Clone)]
pub enum DetailMsg {
    Activate(ItemKey),
    /// Close an item (asks first when a file has unsaved edits).
    Close(ItemKey),
    Side(SideTab),
    Tree(TreeMessage),
    /// The Changes / Explore repo picked (Hermes Desk's repo dropdown).
    PickRepo(String),
    /// `(repo, dir, entries)`.
    Listed(String, String, Result<Vec<FsEntry>, ProtoError>),
    Changes(ChangesMessage),
    Status(String, Result<ide_client::GitStatus, ProtoError>),
    Code(String, CodeMessage),
    Diff(ItemKey, DiffViewMessage),
    FileLoaded(String, LoadReason, Result<FileContent, ProtoError>),
    Saved(String, String, Result<String, ProtoError>),
    BarReload(String),
    BarOverwrite(String),
    BarCompare(String),
    BarDismiss(String),
    DiffLoaded(
        ItemKey,
        Result<(Vec<ide_client::DiffFile>, Option<String>), ProtoError>,
    ),
    /// A tree operation in repo `.0` finished.
    FsDone(String, FsOp, Result<(), ProtoError>),
    /// Explore's search input changed (file names at once, text after [`SEARCH_DELAY`]).
    SearchInput(String),
    /// The typing pause of query `.0` ended.
    SearchTyped(u64),
    /// File-name matches of query `.0`.
    Files(u64, Result<Vec<FileMatch>, ProtoError>),
    SearchCase,
    SearchWord,
    SearchRegex,
    SearchSubmit,
    Search(u64, SearchItem),
    SearchToggleFile(String),
    /// Open `path` ([`qualify`]d) at 1-based line / column (search hit, definition, quick open).
    OpenAt(String, u32, u32),
    /// A definition lookup in repo `.0` answered.
    Def(String, Result<DefResult, ProtoError>),
    Watched(String, Result<(), ProtoError>),
    /// History's first page(s) of repo `.0` for refresh `.1` (+ the unpushed marks).
    HistoryLoaded(String, u64, Result<GitLog, ProtoError>),
    Unpushed(String, u64, Result<Vec<String>, ProtoError>),
    /// History's "more" (button or scrolled to the end).
    HistoryMore,
    /// History was scrolled: pixels left to its end.
    HistoryScrolled(f32),
    HistoryMoreLoaded(String, u64, Result<GitLog, ProtoError>),
    /// Fold / unfold a commit's files.
    ToggleCommit(String),
    /// Files of commit `.1` in repo `.0`.
    CommitFilesLoaded(String, String, Result<Vec<GitChange>, ProtoError>),
    /// Open a commit's diff (whole commit, or one of its files).
    OpenCommit(String, Option<String>),
}

// ---------------------------------------------------------------------------------- helpers

/// A file of one attached repo as used by item keys and file items: `<repo id>:<path>`
/// (repo ids are 8 hex digits, so the first `:` separates them). Tree, changes and RPC paths
/// are plain root-relative paths of one repo.
pub fn qualify(repo: &str, path: &str) -> String {
    format!("{repo}:{path}")
}

/// `(repo id, root-relative path)` of a [`qualify`]d path (`("", path)` for a plain one).
pub fn split_q(q: &str) -> (&str, &str) {
    match q.split_once(':') {
        Some((r, p)) if r.len() == 8 && r.bytes().all(|b| b.is_ascii_hexdigit()) => (r, p),
        _ => ("", q),
    }
}

/// Parent folder of a root-relative (or qualified) path (`""` for top-level entries).
pub fn parent(path: &str) -> &str {
    let path = split_q(path).1;
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Last component of a root-relative (or qualified) path.
pub fn file_name(path: &str) -> &str {
    let path = split_q(path).1;
    path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path)
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// `path` is `prefix` or inside it.
fn under(path: &str, prefix: &str) -> bool {
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

pub(crate) fn view_status(k: ChangeKind) -> ide_views::GitStatus {
    use ide_views::GitStatus as V;
    match k {
        ChangeKind::Modified | ChangeKind::TypeChanged => V::Modified,
        ChangeKind::Added | ChangeKind::Copied => V::Added,
        ChangeKind::Deleted => V::Deleted,
        ChangeKind::Renamed => V::Renamed,
        ChangeKind::Conflicted => V::Conflicted,
        ChangeKind::Untracked => V::Untracked,
    }
}

fn tree_entry(e: &FsEntry) -> TreeEntry {
    let mut t = if e.kind == EntryKind::Dir {
        TreeEntry::dir(e.name.clone())
    } else {
        TreeEntry::file(e.name.clone())
    };
    t.status = e.git.map(view_status);
    t.ignored = e.ignored;
    t
}

fn change_entry(c: &ide_client::GitChange) -> ChangeEntry {
    let mut e = ChangeEntry::new(c.path.clone(), view_status(c.kind));
    e.old_path = c.orig_path.clone();
    e
}

/// Server diff -> view model (`new_text` = the whole new side, for expanding gaps).
pub fn view_diff(f: &ide_client::DiffFile, new_text: Option<&str>) -> ide_views::DiffFile {
    use ide_client::LineKind as P;
    use ide_views::LineKind as V;
    ide_views::DiffFile {
        old_path: f.old_path.clone(),
        new_path: f.path.clone(),
        binary: f.binary,
        hunks: f
            .hunks
            .iter()
            .map(|h| ide_views::Hunk {
                old_start: h.old_start,
                old_count: h.old_lines,
                new_start: h.new_start,
                new_count: h.new_lines,
                section: h.header.clone(),
                lines: h
                    .lines
                    .iter()
                    .map(|l| ide_views::DiffLine {
                        kind: match l.kind {
                            P::Context => V::Context,
                            P::Add => V::Add,
                            P::Del => V::Del,
                        },
                        old_no: l.old_line,
                        new_no: l.new_line,
                        text: l.text.clone(),
                        no_newline: l.no_newline,
                    })
                    .collect(),
            })
            .collect(),
        new_lines: new_text.map(|t| {
            t.lines()
                .map(|l| l.trim_end_matches('\r').to_owned())
                .collect()
        }),
    }
}

/// Disk content (old) vs the buffer (new) of `path` as a diff.
pub fn compare_diff(path: &str, disk: &str, buffer: &str) -> Vec<ide_views::DiffFile> {
    let disk = disk.replace("\r\n", "\n");
    let buffer = buffer.replace("\r\n", "\n");
    let text = format!(
        "diff --git a/{path} b/{path}\n{}",
        similar::TextDiff::from_lines(&disk, &buffer)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
    );
    let mut files = ide_views::DiffFile::parse_unified(&text);
    if files.is_empty() {
        files.push(ide_views::DiffFile {
            new_path: path.into(),
            ..Default::default()
        });
    }
    for f in &mut files {
        f.new_lines = Some(buffer.lines().map(str::to_owned).collect());
    }
    files
}

/// Streams a text search's events (dropping the stream cancels the search).
fn search_stream(
    c: Client,
    q: TextSearch,
) -> impl futures::Stream<Item = SearchItem> + Send + 'static {
    enum St {
        Start(Client, TextSearch),
        Run(SearchStream),
        Done,
    }
    futures::stream::unfold(St::Start(c, q), |st| async move {
        let mut s = match st {
            St::Done => return None,
            St::Start(c, q) => match c.search_text(q).await {
                Ok(s) => s,
                Err(e) => return Some((SearchItem::Failed(e), St::Done)),
            },
            St::Run(s) => s,
        };
        match s.next().await {
            Some(e @ SearchEvent::Done(_)) => Some((SearchItem::Event(e), St::Done)),
            Some(e) => Some((SearchItem::Event(e), St::Run(s))),
            None => Some((SearchItem::Lost, St::Done)),
        }
    })
}

fn msg(sid: &str, m: DetailMsg) -> Message {
    Message::Detail(sid.to_owned(), m)
}

// ---------------------------------------------------------------------------------- logic

impl App {
    pub fn detail(&self, sid: &str) -> Option<&Detail> {
        self.live(sid).map(|l| &l.detail)
    }

    pub fn detail_mut(&mut self, sid: &str) -> Option<&mut Detail> {
        self.sessions
            .iter_mut()
            .find(|l| l.info.id == sid)
            .map(|l| &mut l.detail)
    }

    /// The session whose detail area is on screen.
    pub fn shown_detail(&self) -> Option<String> {
        (!self.ui.detail_collapsed)
            .then(|| self.selected.clone())
            .flatten()
    }

    /// Remember the session's open items in `ui.json`.
    pub fn persist_detail(&mut self, sid: &str) {
        let Some(d) = self.detail(sid) else { return };
        let ui = d.to_ui();
        if self.ui.detail.get(sid) != Some(&ui) {
            self.ui.detail.insert(sid.to_owned(), ui);
            self.ui.save(&self.data_dir);
        }
    }

    /// Watch every repo of the shown session (and only it), load what it shows and refresh it
    /// when stale.
    pub fn sync_detail(&mut self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let shown = self.shown_detail();
        let mut tasks = Vec::new();
        // Unwatch other sessions and repos no longer attached.
        let mut unwatch: Vec<(String, String)> = Vec::new();
        for l in &mut self.sessions {
            let keep: Vec<String> = if Some(&l.info.id) == shown.as_ref() {
                l.info.repos.iter().map(|r| r.repo.clone()).collect()
            } else {
                Vec::new()
            };
            let gone: Vec<String> = l
                .detail
                .watched
                .iter()
                .filter(|r| !keep.contains(r))
                .cloned()
                .collect();
            if !gone.is_empty() {
                l.detail.stale = true;
            }
            for r in gone {
                l.detail.watched.remove(&r);
                unwatch.push((l.info.id.clone(), r));
            }
        }
        for (id, repo) in unwatch {
            let c = c.clone();
            tasks.push(
                Task::future(async move {
                    let _ = c.fs_unwatch(&id, &repo).await;
                })
                .discard(),
            );
        }
        let Some(sid) = shown else {
            return Task::batch(tasks);
        };
        let Some(live) = self.live(&sid) else {
            return Task::batch(tasks);
        };
        let repos: Vec<String> = live.info.repos.iter().map(|r| r.repo.clone()).collect();
        let d = self.detail_mut(&sid).expect("session");
        // The picked repo was detached (or none picked yet): show the first one.
        if !repos.contains(&d.repo) {
            let first = repos.first().cloned().unwrap_or_default();
            d.set_repo(first);
        }
        for r in repos {
            if d.watched.insert(r.clone()) {
                let (c, s, rr) = (c.clone(), sid.clone(), r.clone());
                tasks.push(Task::perform(async move { c.fs_watch(&s, &rr).await }, {
                    let sid = sid.clone();
                    move |res| msg(&sid, DetailMsg::Watched(r.clone(), res))
                }));
            }
        }
        let d = self.detail_mut(&sid).expect("session");
        let stale = std::mem::take(&mut d.stale);
        if d.repo.is_empty() {
            d.tree_error = None;
            d.status_error = None;
        } else if d.loaded_dirs.is_empty() && !d.root_loading {
            d.root_loading = true;
            tasks.push(self.list_dirs(&sid, vec![String::new()]));
        } else if stale {
            let dirs: Vec<String> = d.loaded_dirs.iter().cloned().collect();
            tasks.push(self.list_dirs(&sid, dirs));
        }
        let d = self.detail_mut(&sid).expect("session");
        if !d.repo.is_empty() && (!d.status_loaded || stale) {
            d.status_loaded = true;
            tasks.push(self.load_status(&sid));
        }
        let d = self.detail_mut(&sid).expect("session");
        // History (also for the tab's unpushed badge while another tab is shown, like Desk).
        if !d.repo.is_empty() && (!d.history.requested || stale) {
            tasks.push(self.load_history(&sid));
        }
        if stale {
            tasks.push(self.check_files(&sid, None));
            tasks.push(self.reload_diffs(&sid, None));
        }
        tasks.push(self.ensure_active_loaded(&sid));
        Task::batch(tasks)
    }

    /// (Re)load History of the picked repo: every commit shown so far (at least a page) and
    /// the unpushed marks (Desk's `loadRepositoryHistory`).
    fn load_history(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(d) = self.detail_mut(sid).filter(|d| !d.repo.is_empty()) else {
            return Task::none();
        };
        let h = &mut d.history;
        h.requested = true;
        h.seq += 1;
        // A refresh replaces the pages: a "more" in flight is outdated.
        h.loading_more = false;
        let (seq, limit, repo) = (h.seq, h.refresh_limit(), d.repo.clone());
        let log = {
            let (c, s, r) = (c.clone(), sid.to_owned(), repo.clone());
            let (sid, repo) = (sid.to_owned(), repo.clone());
            Task::perform(async move { c.git_log(&s, &r, 0, limit).await }, move |res| {
                msg(&sid, DetailMsg::HistoryLoaded(repo.clone(), seq, res))
            })
        };
        let marks = {
            let (s, r) = (sid.to_owned(), repo.clone());
            let sid = sid.to_owned();
            Task::perform(async move { c.git_unpushed(&s, &r).await }, move |res| {
                msg(&sid, DetailMsg::Unpushed(repo.clone(), seq, res))
            })
        };
        Task::batch([log, marks])
    }

    /// The next page of History.
    fn load_more_history(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(d) = self.detail_mut(sid).filter(|d| !d.repo.is_empty()) else {
            return Task::none();
        };
        let h = &mut d.history;
        if !h.loaded || !h.has_more || h.loading_more {
            return Task::none();
        }
        h.loading_more = true;
        let (seq, skip, repo) = (h.seq, h.commits.len() as u32, d.repo.clone());
        let (s, r, sid) = (sid.to_owned(), repo.clone(), sid.to_owned());
        Task::perform(async move { c.git_log(&s, &r, skip, 0).await }, move |res| {
            msg(&sid, DetailMsg::HistoryMoreLoaded(repo.clone(), seq, res))
        })
    }

    /// List folders of the picked repo's tree.
    fn list_dirs(&self, sid: &str, dirs: Vec<String>) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(repo) = self.detail(sid).map(|d| d.repo.clone()).filter(|r| !r.is_empty())
        else {
            return Task::none();
        };
        Task::batch(dirs.into_iter().map(|dir| {
            let (c, s, d, r) = (c.clone(), sid.to_owned(), dir.clone(), repo.clone());
            let (sid, repo) = (sid.to_owned(), repo.clone());
            Task::perform(async move { c.fs_list(&s, &r, &d, true).await }, move |res| {
                msg(&sid, DetailMsg::Listed(repo.clone(), dir.clone(), res))
            })
        }))
    }

    /// Git status of the picked repo (the Changes list).
    fn load_status(&self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(repo) = self.detail(sid).map(|d| d.repo.clone()).filter(|r| !r.is_empty())
        else {
            return Task::none();
        };
        let (s, sid, r) = (sid.to_owned(), sid.to_owned(), repo.clone());
        Task::perform(async move { c.git_status(&s, &r, None).await }, move |res| {
            msg(&sid, DetailMsg::Status(repo.clone(), res))
        })
    }

    /// Read `path` ([`qualify`]d).
    fn read_file(&self, sid: &str, path: &str, reason: LoadReason) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let (repo, rel) = split_q(path);
        let (s, r, p) = (sid.to_owned(), repo.to_owned(), rel.to_owned());
        let (sid, path) = (sid.to_owned(), path.to_owned());
        Task::perform(async move { c.fs_read(&s, &r, &p, READ_MAX).await }, move |r| {
            msg(&sid, DetailMsg::FileLoaded(path.clone(), reason, r))
        })
    }

    /// Re-read open files (all, or those in `paths`) to pick up changes on disk.
    fn check_files(&mut self, sid: &str, paths: Option<&[String]>) -> Task<Message> {
        let Some(d) = self.detail(sid) else {
            return Task::none();
        };
        let files: Vec<String> = d
            .items
            .iter()
            .filter_map(|(_, i)| match i {
                Item::File(f)
                    if f.view.is_some()
                        && !f.saving
                        && paths.is_none_or(|p| p.contains(&f.path)) =>
                {
                    Some(f.path.clone())
                }
                _ => None,
            })
            .collect();
        Task::batch(
            files
                .iter()
                .map(|p| self.read_file(sid, p, LoadReason::Refresh)),
        )
    }

    /// Reload diffs (all, or those of `paths`) that were loaded.
    fn reload_diffs(&mut self, sid: &str, paths: Option<&[String]>) -> Task<Message> {
        let Some(d) = self.detail(sid) else {
            return Task::none();
        };
        let keys: Vec<ItemKey> = d
            .items
            .iter()
            .filter(|(k, i)| {
                matches!(k, ItemKey::Diff { path, .. } if paths.is_none_or(|p| p.iter().any(|x| x == path)))
                    && matches!(i, Item::Diff(x) if x.view.is_some() || x.error.is_some())
            })
            .map(|(k, _)| k.clone())
            .collect();
        Task::batch(keys.into_iter().map(|k| self.load_diff(sid, k)))
    }

    fn load_diff(&mut self, sid: &str, key: ItemKey) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        if let ItemKey::Commit { repo, sha, path } = key.clone() {
            if let Some(Item::Diff(x)) = self.detail_mut(sid).and_then(|d| d.item_mut(&key)) {
                x.loading = true;
            }
            let s = sid.to_owned();
            let sid = sid.to_owned();
            return Task::perform(
                async move {
                    let files = c.git_commit_diff(&s, &repo, &sha, path.clone()).await?;
                    // One file: its whole new side lets the view expand unchanged regions.
                    let new_text = match &path {
                        Some(p) => c
                            .git_file_at(&s, &repo, p, ide_client::GitRev::Rev(sha.clone()), None)
                            .await
                            .ok()
                            .flatten()
                            .filter(|f| !f.binary)
                            .map(|f| f.text),
                        None => None,
                    };
                    Ok((files, new_text))
                },
                move |r| msg(&sid, DetailMsg::DiffLoaded(key.clone(), r)),
            );
        }
        let ItemKey::Diff { staged, path } = key.clone() else {
            return Task::none();
        };
        let (repo, path) = {
            let (r, p) = split_q(&path);
            (r.to_owned(), p.to_owned())
        };
        if let Some(Item::Diff(x)) = self.detail_mut(sid).and_then(|d| d.item_mut(&key)) {
            x.loading = true;
        }
        let s = sid.to_owned();
        let sid = sid.to_owned();
        Task::perform(
            async move {
                let against = if staged {
                    DiffAgainst::Index
                } else {
                    DiffAgainst::WorkingTree
                };
                let files = c.git_diff(&s, &repo, Some(&path), against, None).await?;
                // The whole new side lets the view expand unchanged regions.
                let new_text = if staged {
                    c.git_file_at(&s, &repo, &path, ide_client::GitRev::Index, None)
                        .await
                        .ok()
                        .flatten()
                        .filter(|f| !f.binary)
                        .map(|f| f.text)
                } else {
                    c.fs_read(&s, &repo, &path, 0)
                        .await
                        .ok()
                        .filter(|f| !f.binary && !f.truncated)
                        .map(|f| f.text)
                };
                Ok((files, new_text))
            },
            move |r| msg(&sid, DetailMsg::DiffLoaded(key.clone(), r)),
        )
    }

    /// Load the active item if it was never loaded (restored from `ui.json`, or opened while
    /// offline).
    fn ensure_active_loaded(&mut self, sid: &str) -> Task<Message> {
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        let key = d.active.clone();
        match d.item_mut(&key) {
            Some(Item::File(f)) if f.view.is_none() && !f.loading && !f.binary => {
                f.loading = true;
                f.error = None;
                let p = f.path.clone();
                self.read_file(sid, &p, LoadReason::Open)
            }
            Some(Item::Diff(x)) if x.view.is_none() && !x.loading && x.error.is_none() => {
                self.load_diff(sid, key)
            }
            _ => Task::none(),
        }
    }

    /// Show `key` (opening it if needed) and give it focus.
    pub fn activate(&mut self, sid: &str, key: ItemKey) -> Task<Message> {
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        if d.item(&key).is_none() {
            let item = match &key {
                ItemKey::Web => Item::Web,
                ItemKey::File(p) => Item::File(FileItem::new(p)),
                ItemKey::Diff { .. } | ItemKey::Compare(_) | ItemKey::Commit { .. } => {
                    Item::Diff(DiffItem::empty())
                }
            };
            d.items.push((key.clone(), item));
        }
        d.active = key.clone();
        let same_repo = key.repo() == Some(d.repo.as_str());
        if let ItemKey::Diff { staged, path } = &key
            && same_repo
        {
            let path = &split_q(path).1.to_owned();
            let g = if *staged {
                ChangeGroup::Staged
            } else if d
                .changes
                .entries(ChangeGroup::Untracked)
                .iter()
                .any(|e| e.path == *path)
            {
                ChangeGroup::Untracked
            } else {
                ChangeGroup::Unstaged
            };
            d.changes.set_selected(Some((g, path.clone())));
        } else {
            d.changes.set_selected(None);
        }
        let focus = match d.item(&key) {
            Some(Item::File(f)) => f.view.as_ref().map(|v| v.focus()),
            _ => None,
        };
        self.persist_detail(sid);
        self.sync_web();
        Task::batch([
            self.ensure_active_loaded(sid),
            focus.unwrap_or_else(Task::none),
        ])
    }

    /// Open `path` (1-based line/column; 0 = keep) in the editor.
    pub fn open_file_at(&mut self, sid: &str, path: &str, line: u32, col: u32) -> Task<Message> {
        let key = ItemKey::File(path.to_owned());
        let act = self.activate(sid, key);
        let goto = (line > 0).then_some((line as usize, col.max(1) as usize));
        let Some(f) = self.detail_mut(sid).and_then(|d| d.file_mut(path)) else {
            return act;
        };
        let jump = match (goto, &mut f.view) {
            (Some((l, c)), Some(v)) => v.go_to::<Message>(l, c),
            (Some(g), None) => {
                f.pending_goto = Some(g);
                Task::none()
            }
            _ => Task::none(),
        };
        Task::batch([act, jump])
    }

    fn close_item(&mut self, sid: &str, key: &ItemKey) -> Task<Message> {
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        if *key == ItemKey::Web {
            return Task::none();
        }
        let Some(i) = d.items.iter().position(|(k, _)| k == key) else {
            return Task::none();
        };
        d.items.remove(i);
        if d.active == *key {
            d.active = d
                .items
                .get(i.min(d.items.len().saturating_sub(1)))
                .map(|(k, _)| k.clone())
                .unwrap_or(ItemKey::Web);
        }
        let next = d.active.clone();
        self.activate(sid, next)
    }

    /// Files changed in `repo`: refresh the tree / Changes when it is the picked repo, and
    /// open files / diffs of that repo.
    fn after_fs_change(
        &mut self,
        sid: &str,
        repo: &str,
        paths: &[String],
        rescan: bool,
    ) -> Task<Message> {
        let Some(d) = self.detail(sid) else {
            return Task::none();
        };
        let shown = d.repo == repo;
        let dirs: Vec<String> = if !shown {
            Vec::new()
        } else if rescan {
            d.loaded_dirs.iter().cloned().collect()
        } else {
            // The folder of each changed path and its ancestors (folder badges), and the
            // path itself when it is a listed folder.
            let mut set = BTreeSet::new();
            for p in paths {
                let mut dir = p.as_str();
                if d.loaded_dirs.contains(dir) {
                    set.insert(dir.to_owned());
                }
                loop {
                    dir = parent(dir);
                    if d.loaded_dirs.contains(dir) {
                        set.insert(dir.to_owned());
                    }
                    if dir.is_empty() {
                        break;
                    }
                }
            }
            set.into_iter().collect()
        };
        let qualified: Vec<String> = paths.iter().map(|p| qualify(repo, p)).collect();
        let p = if rescan {
            // Everything of this repo.
            self.repo_items(sid, repo)
        } else {
            qualified
        };
        Task::batch([
            self.list_dirs(sid, dirs),
            if shown {
                self.load_status(sid)
            } else {
                Task::none()
            },
            self.check_files(sid, Some(&p)),
            self.reload_diffs(sid, Some(&p)),
        ])
    }

    /// Qualified paths of the open files / diffs of `repo`.
    fn repo_items(&self, sid: &str, repo: &str) -> Vec<String> {
        self.detail(sid)
            .map(|d| {
                d.items
                    .iter()
                    .filter(|(k, _)| k.repo() == Some(repo))
                    .filter_map(|(k, _)| match k {
                        ItemKey::File(p) | ItemKey::Diff { path: p, .. } => Some(p.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn after_git_change(&mut self, sid: &str, repo: &str) -> Task<Message> {
        let Some(d) = self.detail(sid) else {
            return Task::none();
        };
        let shown = d.repo == repo;
        let dirs: Vec<String> = if shown {
            d.loaded_dirs.iter().cloned().collect()
        } else {
            Vec::new()
        };
        let p = self.repo_items(sid, repo);
        Task::batch([
            self.list_dirs(sid, dirs),
            if shown {
                // Commits, checkouts, pushes: History follows the git state like Changes.
                Task::batch([self.load_status(sid), self.load_history(sid)])
            } else {
                Task::none()
            },
            self.reload_diffs(sid, Some(&p)),
        ])
    }

    /// `Notify::FsChanged` / `GitChanged` of a session's repo.
    pub fn on_fs_notify(
        &mut self,
        sid: &str,
        repo: &str,
        paths: Option<(Vec<String>, bool)>,
    ) -> Task<Message> {
        if !self.detail(sid).is_some_and(|d| d.watched.contains(repo)) {
            return Task::none();
        }
        crate::script::log(&format!("watch {sid}/{repo}: {paths:?}"));
        match paths {
            Some((p, rescan)) => self.after_fs_change(sid, repo, &p, rescan),
            None => self.after_git_change(sid, repo),
        }
    }

    pub fn on_detail(&mut self, sid: String, m: DetailMsg) -> Task<Message> {
        let sid = sid.as_str();
        if self.live(sid).is_none() {
            return Task::none();
        }
        match m {
            DetailMsg::Activate(k) => {
                self.open_items = false;
                self.activate(sid, k)
            }
            DetailMsg::Close(k) => {
                let dirty = match self.detail(sid).and_then(|d| d.item(&k)) {
                    Some(Item::File(f)) => f.dirty(),
                    _ => false,
                };
                if dirty && let ItemKey::File(p) = &k {
                    self.open_items = false;
                    self.modal = Some(Modal::CloseDirty(crate::app::CloseDirtyForm {
                        session: sid.to_owned(),
                        path: p.clone(),
                    }));
                    return Task::none();
                }
                let t = self.close_item(sid, &k);
                self.persist_detail(sid);
                t
            }
            DetailMsg::Side(tab) => {
                let d = self.detail_mut(sid).expect("session");
                let opened = d.side != tab && tab == SideTab::History;
                d.side = tab;
                self.persist_detail(sid);
                // Opening History shows the current commits.
                if opened && self.detail(sid).is_some_and(|d| d.history.requested) {
                    self.load_history(sid)
                } else {
                    Task::none()
                }
            }
            DetailMsg::Tree(tm) => {
                let d = self.detail_mut(sid).expect("session");
                let (task, ev) = d.tree.update(tm);
                let sid2 = sid.to_owned();
                let task = task.map(move |m| msg(&sid2, DetailMsg::Tree(m)));
                match ev {
                    Some(ev) => Task::batch([task, self.on_tree_event(sid, ev)]),
                    None => task,
                }
            }
            DetailMsg::PickRepo(repo) => {
                let d = self.detail_mut(sid).expect("session");
                d.set_repo(repo.clone());
                crate::script::log(&format!("detail {sid}: repo {repo}"));
                // File names are matched in the shown repo.
                Task::batch([self.sync_detail(), self.find_files(sid)])
            }
            DetailMsg::Listed(repo, dir, r) => {
                let d = self.detail_mut(sid).expect("session");
                if d.repo != repo {
                    return Task::none();
                }
                if dir.is_empty() {
                    d.root_loading = false;
                }
                match r {
                    Ok(entries) => {
                        d.tree
                            .set_children(&dir, entries.iter().map(tree_entry).collect());
                        if dir.is_empty() {
                            d.tree_error = None;
                        }
                        d.loaded_dirs.insert(dir);
                    }
                    Err(e) if dir.is_empty() => d.tree_error = Some(i18n::proto_error(&e)),
                    Err(_) => {
                        // The folder is gone (deleted / renamed): forget it.
                        d.tree.forget(&dir);
                        let prefix = format!("{dir}/");
                        d.loaded_dirs
                            .retain(|x| *x != dir && !x.starts_with(&prefix));
                    }
                }
                Task::none()
            }
            DetailMsg::Changes(cm) => {
                let d = self.detail_mut(sid).expect("session");
                let (task, ev) = d.changes.update(cm);
                let sid2 = sid.to_owned();
                let task = task.map(move |m| msg(&sid2, DetailMsg::Changes(m)));
                match ev {
                    Some(ChangesEvent::OpenDiff { group, path }) => {
                        let repo = self.detail(sid).expect("session").repo.clone();
                        let key = ItemKey::Diff {
                            staged: group == ChangeGroup::Staged,
                            path: qualify(&repo, &path),
                        };
                        // Re-opening a diff shows the current state.
                        let reload = matches!(
                            self.detail(sid).and_then(|d| d.item(&key)),
                            Some(Item::Diff(x)) if x.view.is_some()
                        );
                        let act = self.activate(sid, key.clone());
                        let r = if reload {
                            self.load_diff(sid, key)
                        } else {
                            Task::none()
                        };
                        Task::batch([task, act, r])
                    }
                    None => task,
                }
            }
            DetailMsg::Status(repo, r) => {
                let d = self.detail_mut(sid).expect("session");
                if d.repo != repo {
                    return Task::none();
                }
                match r {
                    Ok(s) => {
                        let mut unstaged: Vec<ChangeEntry> =
                            s.conflicted.iter().map(change_entry).collect();
                        unstaged.extend(s.unstaged.iter().map(change_entry));
                        d.changes.set_all(
                            s.staged.iter().map(change_entry).collect(),
                            unstaged,
                            s.untracked
                                .iter()
                                .map(|p| {
                                    ChangeEntry::new(p.clone(), ide_views::GitStatus::Untracked)
                                })
                                .collect(),
                        );
                        d.status_error = None;
                    }
                    Err(e) => d.status_error = Some(i18n::proto_error(&e)),
                }
                Task::none()
            }
            DetailMsg::Code(path, cm) => {
                let Some(f) = self.detail_mut(sid).and_then(|d| d.file_mut(&path)) else {
                    return Task::none();
                };
                let Some(v) = f.view.as_mut() else {
                    return Task::none();
                };
                let (task, ev) = v.update(cm);
                let (sid2, p2) = (sid.to_owned(), path.clone());
                let task = task.map(move |m| msg(&sid2, DetailMsg::Code(p2.clone(), m)));
                match ev {
                    Some(ev) => Task::batch([task, self.on_code_event(sid, &path, ev)]),
                    None => task,
                }
            }
            DetailMsg::Diff(key, dm) => {
                let d = self.detail_mut(sid).expect("session");
                let mut mode = None;
                let task = match d.item_mut(&key) {
                    Some(Item::Diff(DiffItem { view: Some(v), .. })) => {
                        let (task, ev) = v.update(dm);
                        if let Some(DiffViewEvent::ModeChanged(m)) = ev {
                            mode = Some(m);
                        }
                        task
                    }
                    _ => Task::none(),
                };
                if let Some(m) = mode {
                    d.diff_mode = m;
                }
                let sid2 = sid.to_owned();
                task.map(move |m| msg(&sid2, DetailMsg::Diff(key.clone(), m)))
            }
            DetailMsg::FileLoaded(path, reason, r) => self.on_file_loaded(sid, path, reason, r),
            DetailMsg::Saved(path, text, r) => self.on_saved(sid, &path, &text, r),
            DetailMsg::BarReload(path) => {
                if let Some(f) = self.detail_mut(sid).and_then(|d| d.file_mut(&path)) {
                    f.loading = true;
                }
                self.read_file(sid, &path, LoadReason::Force)
            }
            DetailMsg::BarOverwrite(path) => self.save_file(sid, &path, None, false),
            DetailMsg::BarCompare(path) => self.read_file(sid, &path, LoadReason::Compare),
            DetailMsg::BarDismiss(path) => {
                if let Some(f) = self.detail_mut(sid).and_then(|d| d.file_mut(&path)) {
                    f.bar = None;
                }
                Task::none()
            }
            DetailMsg::DiffLoaded(key, r) => {
                let d = self.detail_mut(sid).expect("session");
                let mode = d.diff_mode;
                if let Some(Item::Diff(x)) = d.item_mut(&key) {
                    x.loading = false;
                    match r {
                        Ok((files, new_text)) => {
                            if crate::script::enabled() {
                                x.script_adds = files
                                    .iter()
                                    .flat_map(|f| &f.hunks)
                                    .flat_map(|h| &h.lines)
                                    .filter(|l| l.kind == ide_client::LineKind::Add)
                                    .map(|l| l.text.clone())
                                    .collect();
                            }
                            let files: Vec<_> = files
                                .iter()
                                .map(|f| view_diff(f, new_text.as_deref()))
                                .collect();
                            match &mut x.view {
                                Some(v) => v.set_files(files),
                                None => {
                                    let mut v = DiffView::new(files);
                                    v.set_mode(mode);
                                    x.view = Some(v);
                                }
                            }
                            x.error = None;
                        }
                        Err(e) => x.error = Some(i18n::proto_error(&e)),
                    }
                }
                Task::none()
            }
            DetailMsg::FsDone(repo, op, r) => self.on_fs_done(sid, &repo, op, r),
            DetailMsg::SearchInput(q) => {
                let s = &mut self.detail_mut(sid).expect("session").search;
                if q.is_empty() {
                    s.clear();
                    crate::script::log("explore search cleared");
                    return Task::none();
                }
                s.query = q;
                s.typed += 1;
                let typed = s.typed;
                let sid2 = sid.to_owned();
                let pause = Task::perform(tokio::time::sleep(SEARCH_DELAY), move |_| {
                    msg(&sid2, DetailMsg::SearchTyped(typed))
                });
                Task::batch([self.find_files(sid), pause])
            }
            DetailMsg::SearchTyped(typed) => {
                if self.detail(sid).expect("session").search.typed != typed {
                    return Task::none();
                }
                self.start_search(sid)
            }
            DetailMsg::Files(seq, r) => {
                let s = &mut self.detail_mut(sid).expect("session").search;
                if seq != s.files_seq {
                    return Task::none();
                }
                match r {
                    Ok(files) => {
                        crate::script::log(&format!("file-name matches: {}", files.len()));
                        s.files = files;
                        s.files_error = None;
                    }
                    Err(e) => {
                        s.files.clear();
                        s.files_error = Some(i18n::proto_error(&e));
                    }
                }
                Task::none()
            }
            DetailMsg::SearchCase | DetailMsg::SearchWord | DetailMsg::SearchRegex => {
                let s = &mut self.detail_mut(sid).expect("session").search;
                match m {
                    DetailMsg::SearchCase => s.case_sensitive = !s.case_sensitive,
                    DetailMsg::SearchWord => s.whole_word = !s.whole_word,
                    _ => s.regex = !s.regex,
                }
                if s.query.is_empty() {
                    Task::none()
                } else {
                    self.start_search(sid)
                }
            }
            DetailMsg::SearchSubmit => {
                // Enter: the text search starts without waiting for the typing pause.
                self.detail_mut(sid).expect("session").search.typed += 1;
                self.start_search(sid)
            }
            DetailMsg::Search(generation, item) => {
                let s = &mut self.detail_mut(sid).expect("session").search;
                if generation != s.generation {
                    return Task::none();
                }
                match item {
                    SearchItem::Event(SearchEvent::Hits(h)) => s.add(h),
                    SearchItem::Event(SearchEvent::Done(sum)) => {
                        crate::script::log(&format!(
                            "search done: {} match(es) in {} file(s)",
                            sum.matches, sum.files
                        ));
                        s.error = sum.error.as_ref().map(i18n::proto_error);
                        s.summary = Some(sum);
                        s.running = false;
                        s.handle = None;
                    }
                    SearchItem::Failed(e) => {
                        s.error = Some(i18n::proto_error(&e));
                        s.running = false;
                        s.handle = None;
                    }
                    SearchItem::Lost => {
                        s.error = Some(tr::err_disconnected());
                        s.running = false;
                        s.handle = None;
                    }
                }
                Task::none()
            }
            DetailMsg::SearchToggleFile(p) => {
                let s = &mut self.detail_mut(sid).expect("session").search;
                if !s.collapsed.remove(&p) {
                    s.collapsed.insert(p);
                }
                Task::none()
            }
            DetailMsg::OpenAt(path, line, col) => {
                self.modal = None;
                self.open_file_at(sid, &path, line, col)
            }
            DetailMsg::Def(repo, r) => match r {
                Ok(DefResult::One(t)) => {
                    crate::script::log(&format!("definition -> {}:{}:{}", t.path, t.line, t.col));
                    self.open_file_at(sid, &qualify(&repo, &t.path), t.line, t.col)
                }
                Ok(DefResult::Many { name, targets }) => {
                    crate::script::log(&format!("definition of {name}: {} targets", targets.len()));
                    self.modal = Some(Modal::DefPicker(crate::app::DefPickerForm {
                        session: sid.to_owned(),
                        repo,
                        name,
                        targets,
                    }));
                    Task::none()
                }
                Ok(DefResult::None { reason }) => {
                    crate::script::log(&format!("definition: none ({reason})"));
                    let text = i18n::proto_error(&reason);
                    self.detail_mut(sid).expect("session").toast(text);
                    Task::none()
                }
                Err(e) => {
                    let text = i18n::proto_error(&e);
                    self.detail_mut(sid).expect("session").toast(text);
                    Task::none()
                }
            },
            DetailMsg::Watched(repo, r) => {
                if let Err(e) = r {
                    crate::script::log(&format!("fs.watch {sid}/{repo}: {e}"));
                    if e.id != "disconnected" {
                        let text = i18n::proto_error(&e);
                        let d = self.detail_mut(sid).expect("session");
                        d.watched.remove(&repo);
                        d.toast(text);
                    }
                }
                Task::none()
            }
            DetailMsg::HistoryLoaded(repo, seq, r) => {
                let d = self.detail_mut(sid).expect("session");
                if d.repo != repo || d.history.seq != seq {
                    return Task::none();
                }
                let h = &mut d.history;
                h.loaded = true;
                match r {
                    Ok(log) => {
                        crate::script::log(&format!(
                            "history {repo}: {} commit(s), more {}",
                            log.commits.len(),
                            log.has_more
                        ));
                        h.commits = log.commits;
                        h.has_more = log.has_more;
                    }
                    Err(e) => {
                        let text = tr::history_load_failed(&i18n::proto_error(&e));
                        d.toast(text);
                    }
                }
                Task::none()
            }
            DetailMsg::Unpushed(repo, seq, r) => {
                let d = self.detail_mut(sid).expect("session");
                if d.repo != repo || d.history.seq != seq {
                    return Task::none();
                }
                // Desk: a failed query marks nothing.
                let shas = r.unwrap_or_default();
                crate::script::log(&format!("history {repo}: {} unpushed", shas.len()));
                d.history.unpushed = shas.into_iter().collect();
                Task::none()
            }
            DetailMsg::HistoryMore => self.load_more_history(sid),
            DetailMsg::HistoryScrolled(left) => {
                // Desk's `onBottomReached`.
                if left < 24.0 {
                    self.load_more_history(sid)
                } else {
                    Task::none()
                }
            }
            DetailMsg::HistoryMoreLoaded(repo, seq, r) => {
                let d = self.detail_mut(sid).expect("session");
                if d.repo != repo || d.history.seq != seq {
                    return Task::none();
                }
                d.history.loading_more = false;
                match r {
                    Ok(log) => d.history.append(log.commits, log.has_more),
                    Err(e) => {
                        let text = tr::history_load_failed(&i18n::proto_error(&e));
                        d.toast(text);
                    }
                }
                Task::none()
            }
            DetailMsg::ToggleCommit(sha) => {
                let d = self.detail_mut(sid).expect("session");
                if !d.history.toggle(&sha) {
                    return Task::none();
                }
                let Some(c) = self.online() else {
                    let d = self.detail_mut(sid).expect("session");
                    d.history
                        .files
                        .insert(sha, CommitFiles::Failed(tr::err_disconnected()));
                    return Task::none();
                };
                let repo = self.detail(sid).expect("session").repo.clone();
                let (s, r, h) = (sid.to_owned(), repo.clone(), sha.clone());
                let sid = sid.to_owned();
                Task::perform(async move { c.git_commit_files(&s, &r, &h).await }, move |res| {
                    msg(
                        &sid,
                        DetailMsg::CommitFilesLoaded(repo.clone(), sha.clone(), res),
                    )
                })
            }
            DetailMsg::CommitFilesLoaded(repo, sha, r) => {
                let d = self.detail_mut(sid).expect("session");
                if d.repo != repo {
                    return Task::none();
                }
                let files = match r {
                    Ok(f) => {
                        crate::script::log(&format!("commit {sha}: {} file(s)", f.len()));
                        CommitFiles::Loaded(f)
                    }
                    Err(e) => CommitFiles::Failed(i18n::proto_error(&e)),
                };
                d.history.files.insert(sha, files);
                Task::none()
            }
            DetailMsg::OpenCommit(sha, path) => {
                let d = self.detail(sid).expect("session");
                let info = d.history.commit(&sha).cloned();
                let key = ItemKey::Commit {
                    repo: d.repo.clone(),
                    sha,
                    path,
                };
                self.open_items = false;
                let t = self.activate(sid, key.clone());
                if let Some(Item::Diff(x)) = self.detail_mut(sid).and_then(|d| d.item_mut(&key))
                    && info.is_some()
                {
                    x.commit = info;
                }
                t
            }
        }
    }

    fn on_tree_event(&mut self, sid: &str, ev: TreeEvent) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let repo = self.detail(sid).map(|d| d.repo.clone()).unwrap_or_default();
        let (s, r, sid_owned) = (sid.to_owned(), repo.clone(), sid.to_owned());
        let done = move |op: FsOp| {
            let (sid, repo) = (sid_owned.clone(), repo.clone());
            move |res: Result<(), ProtoError>| {
                msg(&sid, DetailMsg::FsDone(repo.clone(), op.clone(), res))
            }
        };
        match ev {
            TreeEvent::Expand(dir) => self.list_dirs(sid, vec![dir]),
            TreeEvent::Collapse(_) | TreeEvent::Select(_) => Task::none(),
            TreeEvent::Open(path) => self.open_file_at(sid, &qualify(&r, &path), 0, 0),
            TreeEvent::CreateFile { dir, name } => {
                let path = join(&dir, &name);
                let op = FsOp::Created {
                    path: path.clone(),
                    is_dir: false,
                };
                Task::perform(
                    async move { c.fs_create_file(&s, &r, &path, "").await.map(drop) },
                    done(op),
                )
            }
            TreeEvent::CreateFolder { dir, name } => {
                let path = join(&dir, &name);
                let op = FsOp::Created {
                    path: path.clone(),
                    is_dir: true,
                };
                Task::perform(async move { c.fs_create_dir(&s, &r, &path).await }, done(op))
            }
            TreeEvent::Rename { path, new_name } => {
                let to = join(parent(&path), &new_name);
                if to == path {
                    return Task::none();
                }
                let op = FsOp::Renamed {
                    from: path.clone(),
                    to: to.clone(),
                };
                Task::perform(
                    async move { c.fs_rename(&s, &r, &path, &to).await },
                    done(op),
                )
            }
            TreeEvent::Delete(path) => {
                let is_dir = self
                    .detail(sid)
                    .is_some_and(|d| d.loaded_dirs.contains(&path) || d.tree.is_expanded(&path));
                self.modal = Some(Modal::DeleteEntry(crate::app::DeleteEntryForm {
                    session: sid.to_owned(),
                    repo: r,
                    path,
                    is_dir,
                    error: None,
                    busy: false,
                }));
                Task::none()
            }
        }
    }

    /// The delete confirm was accepted.
    pub fn delete_entry(&mut self) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(Modal::DeleteEntry(f)) = &mut self.modal else {
            return Task::none();
        };
        f.busy = true;
        f.error = None;
        let (s, repo, path) = (f.session.clone(), f.repo.clone(), f.path.clone());
        let (sid, r) = (s.clone(), repo.clone());
        let op = FsOp::Deleted { path: path.clone() };
        Task::perform(
            async move { c.fs_delete(&s, &r, &path, true).await },
            move |res| msg(&sid, DetailMsg::FsDone(repo.clone(), op.clone(), res)),
        )
    }

    fn on_fs_done(
        &mut self,
        sid: &str,
        repo: &str,
        op: FsOp,
        r: Result<(), ProtoError>,
    ) -> Task<Message> {
        crate::script::log(&format!("fs op {op:?}: {r:?}"));
        if let Err(e) = r {
            let text = i18n::proto_error(&e);
            if let Some(Modal::DeleteEntry(f)) = &mut self.modal {
                f.busy = false;
                f.error = Some(text);
            } else if let Some(d) = self.detail_mut(sid) {
                d.toast(text);
            }
            return Task::none();
        }
        if matches!(self.modal, Some(Modal::DeleteEntry(_))) {
            self.modal = None;
        }
        // The watcher reports these too; refresh now so the tree follows at once.
        let (dirs, then): (Vec<String>, Task<Message>) = match &op {
            FsOp::Created { path, is_dir } => {
                let t = if *is_dir {
                    Task::none()
                } else {
                    self.open_file_at(sid, &qualify(repo, path), 0, 0)
                };
                (vec![parent(path).to_owned()], t)
            }
            FsOp::Renamed { from, to } => {
                // Open items of the old path follow the rename.
                if let Some(d) = self.detail_mut(sid) {
                    let (qf, qt) = (qualify(repo, from), qualify(repo, to));
                    for (k, i) in &mut d.items {
                        if let ItemKey::File(p) = k
                            && under(p, &qf)
                        {
                            let np = format!("{qt}{}", &p[qf.len()..]);
                            // The view keeps its buffer (and unsaved edits); events are routed
                            // by the item's path, not the view's.
                            if let Item::File(f) = i {
                                f.path = np.clone();
                            }
                            if d.active == *k {
                                d.active = ItemKey::File(np.clone());
                            }
                            *k = ItemKey::File(np);
                        }
                    }
                    if d.repo == repo {
                        d.tree.forget(from);
                    }
                    let prefix = format!("{from}/");
                    d.loaded_dirs
                        .retain(|x| x != from && !x.starts_with(&prefix));
                }
                self.persist_detail(sid);
                (
                    vec![parent(from).to_owned(), parent(to).to_owned()],
                    Task::none(),
                )
            }
            FsOp::Deleted { path } => {
                let keys: Vec<ItemKey> = self
                    .detail(sid)
                    .map(|d| {
                        d.items
                            .iter()
                            .filter(|(k, _)| {
                                matches!(k, ItemKey::File(p) if under(p, &qualify(repo, path)))
                            })
                            .map(|(k, _)| k.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut tasks = Vec::new();
                for k in keys {
                    tasks.push(self.close_item(sid, &k));
                }
                if let Some(d) = self.detail_mut(sid)
                    && d.repo == repo
                {
                    d.tree.forget(path);
                    let prefix = format!("{path}/");
                    d.loaded_dirs
                        .retain(|x| x != path && !x.starts_with(&prefix));
                }
                self.persist_detail(sid);
                (vec![parent(path).to_owned()], Task::batch(tasks))
            }
        };
        let mut dirs = dirs;
        dirs.dedup();
        if self.detail(sid).is_none_or(|d| d.repo != repo) {
            return then;
        }
        Task::batch([self.list_dirs(sid, dirs), self.load_status(sid), then])
    }

    fn on_code_event(&mut self, sid: &str, path: &str, ev: CodeEvent) -> Task<Message> {
        match ev {
            CodeEvent::Save { text, .. } => self.save_file(sid, path, Some(text), false),
            CodeEvent::DirtyChanged(_) | CodeEvent::Reverted => Task::none(),
            CodeEvent::Definition { line, column, .. } => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                // The unsaved buffer: the server looks names up in what is on screen.
                let text = self
                    .detail(sid)
                    .and_then(|d| d.file(path))
                    .and_then(|f| f.view.as_ref())
                    .map(|v| v.text());
                crate::script::log(&format!("go to definition at {path}:{line}:{column}"));
                let (repo, rel) = split_q(path);
                let (s, r, p) = (sid.to_owned(), repo.to_owned(), rel.to_owned());
                let (sid, repo) = (sid.to_owned(), repo.to_owned());
                Task::perform(
                    async move {
                        c.def_lookup(&s, &r, &p, line as u32, column as u32, text)
                            .await
                    },
                    move |res| msg(&sid, DetailMsg::Def(repo.clone(), res)),
                )
            }
        }
    }

    /// Write the buffer of `path` (`text` = from the save event, else the current buffer).
    /// `checked` = with the version the buffer was read at (not after "overwrite").
    pub fn save_file(
        &mut self,
        sid: &str,
        path: &str,
        text: Option<String>,
        close_after: bool,
    ) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let overwrite = text.is_none() && !close_after;
        let Some(f) = self.detail_mut(sid).and_then(|d| d.file_mut(path)) else {
            return Task::none();
        };
        let Some(v) = &f.view else {
            return Task::none();
        };
        let text = text.unwrap_or_else(|| v.text());
        f.saving = true;
        f.close_after_save |= close_after;
        let expected = if overwrite { None } else { f.version.clone() };
        let encoding = f.encoding.clone();
        crate::script::log(&format!(
            "save {path} ({} bytes, expected version {expected:?})",
            text.len()
        ));
        let (repo, rel) = split_q(path);
        let (s, r, p, t) = (sid.to_owned(), repo.to_owned(), rel.to_owned(), text.clone());
        let (sid, path) = (sid.to_owned(), path.to_owned());
        Task::perform(
            async move {
                c.fs_write(&s, &r, &p, t, expected.as_deref(), encoding)
                    .await
            },
            move |r| msg(&sid, DetailMsg::Saved(path.clone(), text.clone(), r)),
        )
    }

    fn on_saved(
        &mut self,
        sid: &str,
        path: &str,
        text: &str,
        r: Result<String, ProtoError>,
    ) -> Task<Message> {
        crate::script::log(&format!("saved {path}: {r:?}"));
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        let Some(f) = d.file_mut(path) else {
            return Task::none();
        };
        f.saving = false;
        match r {
            Ok(version) => {
                f.version = Some(version);
                f.bar = None;
                if let Some(v) = &mut f.view {
                    v.mark_saved(text);
                }
                let close = std::mem::take(&mut f.close_after_save);
                // A comparison of this file is outdated now.
                let cmp = ItemKey::Compare(path.to_owned());
                let mut tasks = Vec::new();
                if d.item(&cmp).is_some() {
                    tasks.push(self.close_item(sid, &cmp));
                }
                if close {
                    tasks.push(self.close_item(sid, &ItemKey::File(path.to_owned())));
                    self.persist_detail(sid);
                }
                Task::batch(tasks)
            }
            Err(e) => {
                f.close_after_save = false;
                if e.id == "fs-conflict" {
                    f.bar = Some(FileBar::Conflict);
                } else {
                    let text = i18n::proto_error(&e);
                    d.toast(text);
                }
                Task::none()
            }
        }
    }

    fn on_file_loaded(
        &mut self,
        sid: &str,
        path: String,
        reason: LoadReason,
        r: Result<FileContent, ProtoError>,
    ) -> Task<Message> {
        if reason == LoadReason::Compare {
            return match r {
                Ok(fc) => {
                    let buffer = self
                        .detail(sid)
                        .and_then(|d| d.file(&path))
                        .and_then(|f| f.view.as_ref())
                        .map(|v| v.text())
                        .unwrap_or_default();
                    let files = compare_diff(&path, &fc.text, &buffer);
                    let key = ItemKey::Compare(path);
                    let d = self.detail_mut(sid).expect("session");
                    let mut v = DiffView::new(files);
                    v.set_mode(d.diff_mode);
                    let item = Item::Diff(DiffItem {
                        view: Some(v),
                        ..DiffItem::empty()
                    });
                    match d.items.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, i)) => *i = item,
                        None => d.items.push((key.clone(), item)),
                    }
                    self.activate(sid, key)
                }
                Err(e) => {
                    let text = i18n::proto_error(&e);
                    self.detail_mut(sid).expect("session").toast(text);
                    Task::none()
                }
            };
        }
        let Some(f) = self.detail_mut(sid).and_then(|d| d.file_mut(&path)) else {
            return Task::none();
        };
        f.loading = false;
        let fc = match r {
            Ok(fc) => fc,
            Err(e) => {
                if reason == LoadReason::Open {
                    f.error = Some(i18n::proto_error(&e));
                } else if e.id == "fs-not-found" && f.view.is_some() {
                    f.bar = Some(FileBar::Deleted);
                }
                return Task::none();
            }
        };
        match reason {
            LoadReason::Open => {
                f.binary = fc.binary;
                f.truncated = fc.truncated;
                f.size = fc.size;
                f.version = Some(fc.version);
                f.encoding = Some(fc.encoding);
                f.error = None;
                if fc.binary {
                    f.view = None;
                    return Task::none();
                }
                let mut v = CodeView::new(path.clone(), &fc.text);
                // Only part of the file was read: saving it would cut the file.
                v.set_read_only(fc.truncated);
                let goto = f.pending_goto.take();
                let task = match goto {
                    Some((l, c)) => v.go_to::<Message>(l, c),
                    None => v.focus(),
                };
                f.view = Some(v);
                task
            }
            LoadReason::Refresh => {
                if f.version.as_deref() == Some(fc.version.as_str()) {
                    if f.bar == Some(FileBar::Deleted) {
                        f.bar = None;
                    }
                    return Task::none();
                }
                let Some(v) = &mut f.view else {
                    return Task::none();
                };
                if fc.binary || fc.truncated != f.truncated {
                    return Task::none();
                }
                if v.reload(&fc.text) {
                    crate::script::log(&format!("reloaded clean buffer {path} from disk"));
                    f.version = Some(fc.version);
                    f.encoding = Some(fc.encoding);
                    f.bar = None;
                } else {
                    crate::script::log(&format!("{path} changed on disk while dirty"));
                    f.bar = Some(FileBar::ChangedOnDisk);
                }
                Task::none()
            }
            LoadReason::Force => {
                if let Some(v) = &mut f.view {
                    let _ = v.update(CodeMessage::Revert);
                    v.reload(&fc.text);
                }
                f.version = Some(fc.version);
                f.encoding = Some(fc.encoding);
                f.bar = None;
                let cmp = ItemKey::Compare(path);
                if self.detail(sid).and_then(|d| d.item(&cmp)).is_some() {
                    return self.close_item(sid, &cmp);
                }
                Task::none()
            }
            LoadReason::Compare => Task::none(),
        }
    }

    fn start_search(&mut self, sid: &str) -> Task<Message> {
        let c = self.online();
        let d = self.detail_mut(sid).expect("session");
        let s = &mut d.search;
        s.stop();
        s.generation += 1;
        s.groups.clear();
        s.collapsed.clear();
        s.summary = None;
        s.error = None;
        if s.query.is_empty() {
            return Task::none();
        }
        let Some(c) = c else {
            s.error = Some(tr::err_disconnected());
            return Task::none();
        };
        // Every repo of the session (results grouped by repo).
        let q = TextSearch {
            session: sid.to_owned(),
            repo: None,
            query: s.query.clone(),
            regex: s.regex,
            case_sensitive: s.case_sensitive,
            whole_word: s.whole_word,
            globs: Vec::new(),
            include_ignored: false,
            max_results: 0,
        };
        crate::script::log(&format!("search {:?}", q.query));
        s.running = true;
        let generation = s.generation;
        let sid2 = sid.to_owned();
        let (task, handle) = Task::run(search_stream(c, q), move |item| {
            msg(&sid2, DetailMsg::Search(generation, item))
        })
        .abortable();
        s.handle = Some(handle);
        task
    }

    /// Explore's file-name matches for the current query in the shown repo (Hermes Desk's
    /// 파일 찾기; the server matches fuzzily).
    fn find_files(&mut self, sid: &str) -> Task<Message> {
        let c = self.online();
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        let s = &mut d.search;
        s.files_seq += 1;
        s.files.clear();
        s.files_error = None;
        if s.query.is_empty() || d.repo.is_empty() {
            return Task::none();
        }
        let Some(c) = c else {
            s.files_error = Some(tr::err_disconnected());
            return Task::none();
        };
        let (seq, q, repo, sid2) = (s.files_seq, s.query.clone(), d.repo.clone(), sid.to_owned());
        Task::perform(
            async move {
                c.search_files(&sid2, Some(&repo), &q, FIND_FILES_MAX)
                    .await
            },
            {
                let sid = sid.to_owned();
                move |r| msg(&sid, DetailMsg::Files(seq, r))
            },
        )
    }

    /// Select `path` ([`qualify`]d) in the tree, expanding (and listing) its folders; shows its
    /// repo first. Like Hermes Desk, revealing clears Explore's search so the tree is shown.
    pub fn tree_reveal(&mut self, sid: &str, path: &str) -> Task<Message> {
        let (repo, rel) = split_q(path);
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        if d.search.active() {
            d.search.clear();
        }
        let mut switched = Task::none();
        if !repo.is_empty() && d.repo != repo {
            d.set_repo(repo.to_owned());
            switched = self.sync_detail();
        }
        let d = self.detail_mut(sid).expect("session");
        let (task, events) = d.tree.reveal(rel);
        let sid2 = sid.to_owned();
        let mut tasks = vec![switched, task.map(move |m| msg(&sid2, DetailMsg::Tree(m)))];
        for ev in events {
            tasks.push(self.on_tree_event(sid, ev));
        }
        Task::batch(tasks)
    }

    // ------------------------------------------------------------------ quick open

    pub fn open_quick(&mut self) -> Task<Message> {
        if !self.connected() || self.selected.is_none() {
            return Task::none();
        }
        self.open_items = false;
        self.modal = Some(Modal::QuickOpen(crate::app::QuickForm::default()));
        Task::batch([
            iced::widget::operation::focus(QUICK_ID),
            self.quick_query(String::new()),
        ])
    }

    pub fn quick_query(&mut self, q: String) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(sid) = self.selected.clone() else {
            return Task::none();
        };
        let Some(Modal::QuickOpen(f)) = &mut self.modal else {
            return Task::none();
        };
        f.query = q.clone();
        f.seq += 1;
        let seq = f.seq;
        Task::perform(
            async move { c.search_files(&sid, None, &q, QUICK_MAX).await },
            move |r| Message::QuickResults(seq, r),
        )
    }

    /// Ctrl+Shift+F: show Explore with its search input focused.
    pub fn open_search(&mut self) -> Task<Message> {
        let Some(sid) = self.selected.clone() else {
            return Task::none();
        };
        if self.modal.is_some() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        if self.ui.detail_collapsed {
            self.ui.detail_collapsed = false;
            self.save_ui();
            self.sync_web();
            tasks.push(self.sync_detail());
        }
        tasks.push(self.on_detail(sid, DetailMsg::Side(SideTab::Explore)));
        tasks.push(iced::widget::operation::focus(SEARCH_ID));
        Task::batch(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for k in [
            ItemKey::Web,
            ItemKey::File("src/한글.ts".into()),
            ItemKey::Diff {
                staged: true,
                path: "a/b.rs".into(),
            },
            ItemKey::Diff {
                staged: false,
                path: "c".into(),
            },
            ItemKey::Commit {
                repo: "0123abcd".into(),
                sha: "89abcdef0123".into(),
                path: None,
            },
            ItemKey::Commit {
                repo: "0123abcd".into(),
                sha: "89abcdef0123".into(),
                path: Some("src/a:b 한글.ts".into()),
            },
        ] {
            assert_eq!(ItemKey::decode(&k.encode().unwrap()), Some(k));
        }
        assert_eq!(
            ItemKey::Commit {
                repo: "0123abcd".into(),
                sha: "89ab".into(),
                path: Some("x/y.rs".into()),
            }
            .encode()
            .as_deref(),
            Some("commit:0123abcd:89ab:x/y.rs")
        );
        assert_eq!(ItemKey::Compare("x".into()).encode(), None);
        assert_eq!(ItemKey::decode("file:"), None);
        assert_eq!(ItemKey::decode("diff:x:y"), None);
        assert_eq!(ItemKey::decode("commit:0123abcd:zz:a"), None);
        assert_eq!(ItemKey::decode("commit:0123abcd"), None);
        // A restored commit item keeps its repo.
        let k = ItemKey::decode("commit:0123abcd:89ab:").unwrap();
        assert_eq!(k.repo(), Some("0123abcd"));
        assert_eq!(k.clone().qualified(Some("ffffffff")), Some(k));
    }

    #[test]
    fn qualified_paths() {
        let q = qualify("0123abcd", "src/a.rs");
        assert_eq!(q, "0123abcd:src/a.rs");
        assert_eq!(split_q(&q), ("0123abcd", "src/a.rs"));
        assert_eq!(split_q("src/a.rs"), ("", "src/a.rs"));
        assert_eq!(split_q("notahex1:x"), ("", "notahex1:x"));
        assert_eq!((file_name(&q), parent(&q)), ("a.rs", "src"));
        assert_eq!(ItemKey::File(q.clone()).repo(), Some("0123abcd"));
        assert_eq!(
            ItemKey::decode(&ItemKey::File(q.clone()).encode().unwrap()),
            Some(ItemKey::File(q))
        );
    }

    #[test]
    fn path_helpers() {
        assert_eq!(parent("a/b/c.txt"), "a/b");
        assert_eq!(parent("c.txt"), "");
        assert_eq!(file_name("a/b/c.txt"), "c.txt");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("a", "x"), "a/x");
        assert!(under("a/b/c", "a/b") && under("a/b", "a/b") && !under("a/bc", "a/b"));
    }

    #[test]
    fn restore_from_ui() {
        let ui = DetailUi {
            open: vec![
                "file:src/main.ts".into(),
                "diff:w:README.md".into(),
                "bogus".into(),
                "file:src/main.ts".into(),
            ],
            active: Some("diff:w:README.md".into()),
            side: "changes".into(),
        };
        let d = Detail::new(Some(&ui), Some("0123abcd"));
        assert_eq!(d.items.len(), 3);
        assert_eq!(d.side, SideTab::Changes);
        assert_eq!(
            d.active,
            ItemKey::Diff {
                staged: false,
                path: "0123abcd:README.md".into()
            }
        );
        // Plain paths (saved before multi-repo sessions) belong to the first repo.
        assert_eq!(
            d.to_ui().open,
            vec![
                "web",
                "file:0123abcd:src/main.ts",
                "diff:w:0123abcd:README.md"
            ]
        );
        assert_eq!(d.repo, "0123abcd");
        // Without a repo they cannot be opened.
        assert_eq!(Detail::new(Some(&ui), None).items.len(), 1);
    }

    #[test]
    fn old_search_tab_opens_explore() {
        for (saved, side) in [
            ("search", SideTab::Explore),
            ("explore", SideTab::Explore),
            ("changes", SideTab::Changes),
            ("history", SideTab::History),
            ("", SideTab::Explore),
        ] {
            let ui = DetailUi {
                side: saved.into(),
                ..DetailUi::default()
            };
            assert_eq!(Detail::new(Some(&ui), None).side, side, "{saved:?}");
        }
        assert_eq!(SideTab::parse(SideTab::Changes.code()), SideTab::Changes);
        assert_eq!(SideTab::parse(SideTab::Explore.code()), SideTab::Explore);
        assert_eq!(SideTab::parse(SideTab::History.code()), SideTab::History);
    }

    #[test]
    fn clearing_the_query_shows_the_tree() {
        let mut s = SearchState {
            query: "greet".into(),
            ..SearchState::default()
        };
        s.files.push(FileMatch {
            repo: "0123abcd".into(),
            path: "src/greet.ts".into(),
            score: 1,
            indices: Vec::new(),
        });
        assert!(s.active());
        let (g, f, t) = (s.generation, s.files_seq, s.typed);
        s.clear();
        assert!(!s.active() && s.files.is_empty() && s.groups.is_empty());
        // Late answers of the old query are dropped.
        assert!(s.generation > g && s.files_seq > f && s.typed > t);
    }

    #[test]
    fn compare_is_a_diff() {
        let files = compare_diff("a.txt", "one\ntwo\n", "one\nTWO\nthree\n");
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].added(), files[0].removed()), (2, 1));
        let same = compare_diff("a.txt", "x\r\n", "x\n");
        assert!(same[0].hunks.is_empty());
    }
}
