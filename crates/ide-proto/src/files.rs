//! Phase 2 wire types: files, repository browsing, search, git, go-to-definition.
//!
//! Conventions:
//! - Paths inside a session are **root-relative** to the session's worktree, `/`-separated,
//!   without `.`/`..` components (`""` = the root). Only repository browsing uses absolute
//!   paths (server OS syntax).
//! - Lines are 1-based. Columns (`col`) are 1-based and count Unicode scalar values (`char`s)
//!   of the line. Byte ranges are only used for [`SearchMatch::ranges`] (into `line_text`).
//! - Times are Unix milliseconds.

use serde::{Deserialize, Serialize};

use crate::error::ProtoError;

/// Kind of a directory entry. Symlinks (and Windows junctions) are not followed when listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Directory.
    Dir,
    /// Symbolic link or junction (target not resolved).
    Symlink,
}

/// Git change kind (status badge, status lists, diff files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChangeKind {
    /// Content changed.
    Modified,
    /// New (staged or, in a diff, added).
    Added,
    /// Deleted.
    Deleted,
    /// Renamed (see `orig_path`).
    Renamed,
    /// Copied (see `orig_path`).
    Copied,
    /// File type changed (file <-> symlink).
    TypeChanged,
    /// Unmerged (merge conflict).
    Conflicted,
    /// Not tracked by git.
    Untracked,
}

/// One entry of `fs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEntry {
    /// File name (no directory part).
    pub name: String,
    /// Kind.
    pub kind: EntryKind,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Modification time (Unix ms, 0 if unknown).
    pub mtime: u64,
    /// Git status of the file; for a directory `Modified` when anything below it changed.
    pub git: Option<ChangeKind>,
    /// Matched by `.gitignore` / `info/exclude` / the global excludes file.
    pub ignored: bool,
}

/// `fs.stat` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsStat {
    /// Root-relative path.
    pub path: String,
    /// Kind (of the entry itself; a symlink is not followed).
    pub kind: EntryKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time (Unix ms).
    pub mtime: u64,
    /// Read-only attribute.
    pub readonly: bool,
}

/// A text encoding: WHATWG label (`UTF-8`, `UTF-16LE`, `UTF-16BE`, `EUC-KR`, `Shift_JIS`...)
/// plus whether the file starts with a byte order mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEncoding {
    /// Encoding name as understood by `encoding_rs` (`Encoding::for_label`).
    pub label: String,
    /// The file has (and a write keeps) a BOM.
    pub bom: bool,
}

impl TextEncoding {
    /// UTF-8 without BOM (the default for new files).
    pub fn utf8() -> Self {
        TextEncoding {
            label: "UTF-8".into(),
            bom: false,
        }
    }
}

/// `fs.read` / `git.file_at` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    /// Root-relative path.
    pub path: String,
    /// Decoded text (empty when `binary`). Line endings are kept as in the file.
    pub text: String,
    /// Detected encoding (the one `fs.write` should use to keep the file's encoding).
    pub encoding: TextEncoding,
    /// The file looks binary (NUL byte in the first 8000 bytes, no UTF-16 BOM): no text.
    pub binary: bool,
    /// Only the first `max_bytes` were decoded.
    pub truncated: bool,
    /// Size of the whole file in bytes.
    pub size: u64,
    /// Content version of the whole file on disk (opaque; pass to `fs.write`).
    pub version: String,
}

/// `browse.list` entry (directories only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseEntry {
    /// Folder name.
    pub name: String,
    /// Absolute path (server OS syntax).
    pub path: String,
    /// Contains a `.git` dir or file (a repository or worktree root).
    pub is_git_repo: bool,
}

/// `browse.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseListing {
    /// The listed folder (absolute, canonical, server OS syntax).
    pub path: String,
    /// Parent folder if it is still inside a browse root.
    pub parent: Option<String>,
    /// The listed folder itself is a repository root.
    pub is_git_repo: bool,
    /// Sub-folders, sorted by name (case-insensitive).
    pub entries: Vec<BrowseEntry>,
}

/// `browse.config` / `browse.set_roots` result: the folders repositories can be picked from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseConfig {
    /// The user's home dir (always a browse root; `None` if unknown).
    pub home: Option<String>,
    /// Extra roots stored in the server's `server.json` (`browse_roots`), as saved.
    pub roots: Vec<String>,
}

/// `search.text` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearch {
    /// Session id.
    pub session: String,
    /// Search text (a regular expression when `regex`).
    pub query: String,
    /// `query` is a regex (Rust `regex` syntax); otherwise a literal.
    pub regex: bool,
    /// Case-sensitive (false = case-insensitive).
    pub case_sensitive: bool,
    /// Whole words only.
    pub whole_word: bool,
    /// Glob filters (gitignore syntax, relative to the root): `*.rs`, `src/**`; a leading `!`
    /// excludes (`!target/**`). Empty = all files.
    pub globs: Vec<String>,
    /// Also search files ignored by `.gitignore` (hidden files are always searched; `.git` never).
    pub include_ignored: bool,
    /// Stop after this many matching lines (0 = default 2000).
    pub max_results: u32,
}

/// A byte range (`start..end`) in [`SearchMatch::line_text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    /// Start (inclusive).
    pub start: u32,
    /// End (exclusive).
    pub end: u32,
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    /// Root-relative path.
    pub path: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based char column of the first match (for navigation).
    pub col: u32,
    /// The line (without the line ending; lossily decoded; cut after 1000 bytes).
    pub line_text: String,
    /// Match ranges in `line_text`, in bytes.
    pub ranges: Vec<ByteRange>,
}

/// End of a `search.text` run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSummary {
    /// Matching lines sent.
    pub matches: u32,
    /// Files with at least one match.
    pub files: u32,
    /// Files searched.
    pub searched: u32,
    /// Stopped at `max_results`.
    pub truncated: bool,
    /// Stopped by `search.cancel` (or the client went away).
    pub cancelled: bool,
    /// Failed (e.g. an invalid regex discovered late); results so far were sent.
    pub error: Option<ProtoError>,
}

/// `search.files` result entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMatch {
    /// Root-relative path.
    pub path: String,
    /// Match score (higher is better).
    pub score: u32,
    /// Matched char indices in `path` (sorted, deduplicated) for highlighting.
    pub indices: Vec<u32>,
}

/// One changed path in `git.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChange {
    /// Root-relative path.
    pub path: String,
    /// Previous path of a rename/copy.
    pub orig_path: Option<String>,
    /// Kind.
    pub kind: ChangeKind,
}

/// Commits ahead/behind a reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AheadBehind {
    /// The reference compared with (`main`, `origin/main`).
    pub reference: String,
    /// Commits on HEAD not on `reference`.
    pub ahead: u32,
    /// Commits on `reference` not on HEAD.
    pub behind: u32,
}

/// `git.status` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    /// Checked-out branch (`None` when detached).
    pub branch: Option<String>,
    /// HEAD commit id (empty before the first commit).
    pub head: String,
    /// Upstream tracking state, if the branch has an upstream.
    pub upstream: Option<AheadBehind>,
    /// State against the base branch (the request's `base`, else the branch checked out in
    /// the session's main repository), with its merge base. `None` if it cannot be resolved.
    pub base: Option<AheadBehind>,
    /// Merge base of HEAD and the base branch (commit id).
    pub merge_base: Option<String>,
    /// Index vs HEAD (renames detected).
    pub staged: Vec<GitChange>,
    /// Working tree vs index.
    pub unstaged: Vec<GitChange>,
    /// Untracked files (each file, not collapsed directories).
    pub untracked: Vec<String>,
    /// Unmerged paths.
    pub conflicted: Vec<GitChange>,
}

/// What `git.diff` compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffAgainst {
    /// Working tree vs index (unstaged changes). An untracked `path` diffs as all-added.
    WorkingTree,
    /// Index vs HEAD (staged changes).
    Index,
    /// Working tree vs HEAD (all uncommitted changes). An untracked `path` diffs as all-added.
    Head,
    /// Working tree vs the merge base with the base branch (everything this session changed).
    /// An untracked `path` diffs as all-added.
    Base,
}

/// Kind of a diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    /// Unchanged.
    Context,
    /// Added (new side only).
    Add,
    /// Deleted (old side only).
    Del,
}

/// One diff line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    /// Kind.
    pub kind: LineKind,
    /// 1-based line in the old file (context and deleted lines).
    pub old_line: Option<u32>,
    /// 1-based line in the new file (context and added lines).
    pub new_line: Option<u32>,
    /// Text without the `+`/`-`/` ` prefix and without the line ending (lossy UTF-8).
    pub text: String,
    /// `\ No newline at end of file` followed this line.
    pub no_newline: bool,
}

/// One hunk (`@@ -old_start,old_lines +new_start,new_lines @@ header`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Old start line.
    pub old_start: u32,
    /// Old line count.
    pub old_lines: u32,
    /// New start line.
    pub new_start: u32,
    /// New line count.
    pub new_lines: u32,
    /// Text after the second `@@` (function context), trimmed.
    pub header: String,
    /// Lines.
    pub lines: Vec<DiffLine>,
}

/// One file of a `git.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    /// Root-relative path (the new path of a rename; the old path of a deletion).
    pub path: String,
    /// Old path of a rename/copy.
    pub old_path: Option<String>,
    /// Kind of change.
    pub kind: ChangeKind,
    /// Binary file: no hunks.
    pub binary: bool,
    /// Hunks (empty for binary files, pure renames and mode changes).
    pub hunks: Vec<DiffHunk>,
}

/// Revision for `git.file_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitRev {
    /// The HEAD commit.
    Head,
    /// The index (stage 0).
    Index,
    /// The merge base with the base branch (as in `git.status`).
    Base,
    /// Any commit-ish (`abc123`, `main~2`); must not start with `-`.
    Rev(String),
}

/// A go-to-definition target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefTarget {
    /// Root-relative path.
    pub path: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based char column of the name.
    pub col: u32,
    /// The trimmed line.
    pub preview: String,
}

/// `def.lookup` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefResult {
    /// Nothing to jump to; `reason` is a translatable id (`def-*`) with args.
    None {
        /// Why.
        reason: ProtoError,
    },
    /// One target: jump.
    One(DefTarget),
    /// Several candidates (repo-wide search): let the user pick. At most 50.
    Many {
        /// The name looked up.
        name: String,
        /// Candidates, same directory first.
        targets: Vec<DefTarget>,
    },
}
