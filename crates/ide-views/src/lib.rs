//! Client-side iced views fed by plain view models (no protocol types): the owner maps its data
//! (e.g. `ide-proto` messages) to these models, keeps each view's state struct, forwards the
//! view's messages to its `update` and acts on the returned events.
//!
//! - [`FileTree`]: lazy file tree ([`TreeEntry`] → [`TreeEvent`]).
//! - [`CodeView`]: file viewer/editor with highlighting, find, go-to-line, save/revert,
//!   Ctrl+click ([`CodeEvent`]).
//! - [`DiffView`]: unified / side-by-side diff of [`DiffFile`]s with word highlights and
//!   collapsible unchanged regions.
//! - [`ChangesList`]: staged / unstaged / untracked groups ([`ChangeEntry`] → [`ChangesEvent`]).
//!
//! Every `update` returns `(Task<Msg>, Option<Event>)`: map the task into your message type and
//! handle the event. All user-visible strings come from `ide_i18n::tr`.

pub mod changes;
pub mod code;
pub mod diff;
pub mod editor;
pub mod icons;
mod interact;
mod reorder;
pub mod syntax;
pub mod theme;
pub mod tree;
mod vlist;

pub use changes::{ChangeEntry, ChangeGroup, ChangesEvent, ChangesList, ChangesMessage};
pub use code::{CodeEvent, CodeMessage, CodeView};
pub use diff::{DiffFile, DiffLine, DiffMode, DiffView, DiffViewEvent, DiffViewMessage, Hunk, LineKind};
pub use interact::Interact;
pub use reorder::Reorder;
pub use tree::{FileTree, MenuAction, TreeEntry, TreeEvent, TreeMessage};

use iced::Color;

/// Git status of a file (tree badge, changes list letter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed,
    Conflicted,
}

impl GitStatus {
    /// One-letter badge (`M A D U R !`).
    pub fn letter(self) -> char {
        match self {
            GitStatus::Modified => 'M',
            GitStatus::Added => 'A',
            GitStatus::Deleted => 'D',
            GitStatus::Untracked => 'U',
            GitStatus::Renamed => 'R',
            GitStatus::Conflicted => '!',
        }
    }

    pub fn color(self) -> Color {
        match self {
            GitStatus::Modified => theme::GIT_MODIFIED,
            GitStatus::Added | GitStatus::Untracked => theme::GIT_ADDED,
            GitStatus::Deleted => theme::GIT_DELETED,
            GitStatus::Renamed => theme::GIT_RENAMED,
            GitStatus::Conflicted => theme::GIT_CONFLICT,
        }
    }

    /// Translated name (tooltips).
    pub fn label(self) -> String {
        use ide_i18n::tr;
        match self {
            GitStatus::Modified => tr::git_modified(),
            GitStatus::Added => tr::git_added(),
            GitStatus::Deleted => tr::git_deleted(),
            GitStatus::Untracked => tr::git_untracked(),
            GitStatus::Renamed => tr::git_renamed(),
            GitStatus::Conflicted => tr::git_conflicted(),
        }
    }
}
