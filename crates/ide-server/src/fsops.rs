//! File operations of a session worktree (`fs.*`) and repository browsing (`browse.*`).
//! Every path goes through the [`RootJail`] / [`PathJail`]; mutations are audited by the
//! caller.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ide_proto::{
    BrowseEntry, BrowseListing, ChangeKind, EntryKind, FileContent, FsEntry, FsStat, ProtoError,
    TextEncoding,
};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::jail::{PathJail, RootJail, canonicalize, is_git_internal, normalize_rel};

/// Default `fs.read` limit.
pub const DEFAULT_MAX_READ: u64 = 2 * 1024 * 1024;
/// Largest `fs.read` `max_bytes` honoured (RPC messages are limited to 16 MiB).
pub const MAX_READ_LIMIT: u64 = 8 * 1024 * 1024;
/// Files up to this size get a content-hash version; larger ones a size+mtime version.
const HASH_LIMIT: u64 = 16 * 1024 * 1024;
/// Bytes checked for NUL to classify a file as binary (same as git and Hermes Desk).
const BINARY_PROBE: usize = 8000;

/// Map an I/O error on `rel` to a wire error.
pub fn io_error(rel: &str, e: &std::io::Error) -> ProtoError {
    use std::io::ErrorKind as K;
    let id = match e.kind() {
        K::NotFound => "fs-not-found",
        K::AlreadyExists => "fs-exists",
        K::PermissionDenied => "fs-permission",
        K::DirectoryNotEmpty => "fs-dir-not-empty",
        K::NotADirectory => "fs-not-a-dir",
        K::IsADirectory => "fs-is-a-dir",
        _ => {
            return ProtoError::new("fs-io")
                .arg("path", rel)
                .arg("detail", e.to_string());
        }
    };
    ProtoError::new(id).arg("path", rel)
}

fn err(id: &str, rel: &str) -> ProtoError {
    ProtoError::new(id).arg("path", rel)
}

fn mtime_ms(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

fn kind_of(ft: &std::fs::FileType) -> EntryKind {
    if ft.is_symlink() || is_junction(ft) {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}

#[cfg(windows)]
fn is_junction(ft: &std::fs::FileType) -> bool {
    use std::os::windows::fs::FileTypeExt;
    ft.is_symlink_dir() || ft.is_symlink_file()
}

#[cfg(not(windows))]
fn is_junction(_: &std::fs::FileType) -> bool {
    false
}

/// Is `p` a link (symlink or Windows junction / mount point)?
fn is_link(p: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return false;
    };
    if m.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------
// Versions and encodings

/// FNV-1a 64.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Content version of `bytes` (opaque string).
pub fn version_of(bytes: &[u8]) -> String {
    format!("h{:x}-{:016x}", bytes.len(), fnv1a(bytes))
}

/// Version of the file on disk (`None` if it does not exist or is not a file).
pub fn disk_version(abs: &Path) -> Result<Option<String>, std::io::Error> {
    let m = match std::fs::metadata(abs) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !m.is_file() {
        return Ok(None);
    }
    if m.len() > HASH_LIMIT {
        let ns = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        return Ok(Some(format!("m{:x}-{ns:x}", m.len())));
    }
    Ok(Some(version_of(&std::fs::read(abs)?)))
}

/// Decode file bytes. `truncated` = `bytes` is a prefix of the file (an incomplete trailing
/// character is dropped instead of becoming U+FFFD). Returns `(text, encoding, binary)`.
pub fn decode(bytes: &[u8], truncated: bool) -> (String, TextEncoding, bool) {
    let enc = |label: &str, bom: bool| TextEncoding {
        label: label.into(),
        bom,
    };
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return (utf8_prefix(rest, truncated), enc("UTF-8", true), false);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return (utf16(rest, true), enc("UTF-16LE", true), false);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return (utf16(rest, false), enc("UTF-16BE", true), false);
    }
    if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        return (String::new(), enc("UTF-8", false), true);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => return (s.to_owned(), enc("UTF-8", false), false),
        Err(e) if truncated && e.error_len().is_none() && bytes.len() - e.valid_up_to() < 4 => {
            return (
                String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
                enc("UTF-8", false),
                false,
            );
        }
        Err(_) => {}
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, !truncated);
    let guess = det.guess(None, true);
    let (text, _) = guess.decode_without_bom_handling(bytes);
    (text.into_owned(), enc(guess.name(), false), false)
}

fn utf8_prefix(bytes: &[u8], truncated: bool) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(e) if truncated && e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn utf16(bytes: &[u8], le: bool) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|c| {
        if le {
            u16::from_le_bytes(*c)
        } else {
            u16::from_be_bytes(*c)
        }
    });
    char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Encode text for writing.
pub fn encode(text: &str, enc: &TextEncoding, rel: &str) -> Result<Vec<u8>, ProtoError> {
    let label = enc.label.trim();
    let lower = label.to_ascii_lowercase();
    let mut out = Vec::with_capacity(text.len() + 3);
    match lower.as_str() {
        "utf-8" | "utf8" | "" => {
            if enc.bom {
                out.extend_from_slice(b"\xEF\xBB\xBF");
            }
            out.extend_from_slice(text.as_bytes());
        }
        "utf-16le" | "utf-16" => {
            if enc.bom {
                out.extend_from_slice(b"\xFF\xFE");
            }
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        "utf-16be" => {
            if enc.bom {
                out.extend_from_slice(b"\xFE\xFF");
            }
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_be_bytes());
            }
        }
        _ => {
            let e = encoding_rs::Encoding::for_label(label.as_bytes())
                .ok_or_else(|| ProtoError::new("fs-bad-encoding").arg("encoding", label))?;
            let (bytes, _, unmappable) = e.encode(text);
            if unmappable {
                return Err(ProtoError::new("fs-unencodable")
                    .arg("path", rel)
                    .arg("encoding", e.name()));
            }
            out.extend_from_slice(&bytes);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Ignore rules

/// `.gitignore` rules relevant to one directory of a worktree: every `.gitignore` from the
/// root down to the directory, `info/exclude` of the repository and the global excludes file.
pub struct IgnoreStack {
    /// Deepest first.
    levels: Vec<Gitignore>,
}

impl IgnoreStack {
    /// Rules for entries of `dir` (absolute, inside `root`).
    pub fn for_dir(root: &Path, dir: &Path) -> Self {
        let mut levels = Vec::new();
        let mut d = Some(dir);
        while let Some(cur) = d {
            let gi = cur.join(".gitignore");
            if gi.is_file() {
                let mut b = GitignoreBuilder::new(cur);
                b.add(&gi);
                if let Ok(g) = b.build() {
                    levels.push(g);
                }
            }
            if cur == root {
                break;
            }
            d = cur.parent().filter(|p| p.starts_with(root));
        }
        if let Some(common) = git_common_dir(root) {
            let ex = common.join("info").join("exclude");
            if ex.is_file() {
                let mut b = GitignoreBuilder::new(root);
                b.add(ex);
                if let Ok(g) = b.build() {
                    levels.push(g);
                }
            }
        }
        let (global, _) = GitignoreBuilder::new(root).build_global();
        levels.push(global);
        IgnoreStack { levels }
    }

    /// Is `path` (absolute, under the root) ignored? An ignored parent directory makes
    /// everything below it ignored.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        for g in &self.levels {
            let m = g.matched_path_or_any_parents(path, is_dir);
            if m.is_ignore() {
                return true;
            }
            if m.is_whitelist() {
                return false;
            }
        }
        false
    }
}

/// The repository's common git dir for a work tree root (`.git` dir, or the `commondir` of
/// a linked worktree's `gitdir`).
pub fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let gitdir = per_worktree_git_dir(root)?;
    match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) => {
            let c = PathBuf::from(c.trim());
            Some(if c.is_absolute() { c } else { gitdir.join(c) })
        }
        Err(_) => Some(gitdir),
    }
}

/// The per-worktree git dir (`.git` dir, or the `gitdir:` target of a `.git` file).
pub fn per_worktree_git_dir(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let p = text.trim().strip_prefix("gitdir:")?.trim();
    let p = PathBuf::from(p);
    Some(if p.is_absolute() { p } else { root.join(p) })
}

// ---------------------------------------------------------------------------------------
// Operations

/// `fs.list`.
pub fn list(
    jail: &RootJail,
    dir: &str,
    include_ignored: bool,
    badges: &HashMap<String, ChangeKind>,
) -> Result<Vec<FsEntry>, ProtoError> {
    let abs = jail.resolve(dir)?;
    let rd = std::fs::read_dir(&abs).map_err(|e| io_error(dir, &e))?;
    let ignore = IgnoreStack::for_dir(jail.root(), &abs);
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case(".git") {
            continue;
        }
        let Ok(ft) = ent.file_type() else { continue };
        let kind = kind_of(&ft);
        let path = ent.path();
        // Size/mtime of the entry itself (links are not followed).
        let meta = std::fs::symlink_metadata(&path).ok();
        let is_dir = kind == EntryKind::Dir || (kind == EntryKind::Symlink && path.is_dir());
        let ignored = ignore.is_ignored(&path, is_dir);
        if ignored && !include_ignored {
            continue;
        }
        out.push(FsEntry {
            git: badges.get(&name).copied(),
            size: if kind == EntryKind::File {
                meta.as_ref().map_or(0, |m| m.len())
            } else {
                0
            },
            mtime: meta.as_ref().map_or(0, mtime_ms),
            name,
            kind,
            ignored,
        });
    }
    out.sort_by(|a, b| {
        (a.kind != EntryKind::Dir)
            .cmp(&(b.kind != EntryKind::Dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// `fs.stat` (of the entry itself; a link is reported as `Symlink`).
pub fn stat(jail: &RootJail, rel: &str) -> Result<FsStat, ProtoError> {
    let norm = normalize_rel(rel)?;
    let abs = if norm.is_empty() {
        jail.root().to_path_buf()
    } else {
        jail.resolve_entry(&norm)?
    };
    let m = std::fs::symlink_metadata(&abs).map_err(|e| io_error(&norm, &e))?;
    Ok(FsStat {
        kind: kind_of(&m.file_type()),
        size: if m.is_file() { m.len() } else { 0 },
        mtime: mtime_ms(&m),
        readonly: m.permissions().readonly(),
        path: norm,
    })
}

/// `fs.read`.
pub fn read(jail: &RootJail, rel: &str, max_bytes: u64) -> Result<FileContent, ProtoError> {
    let norm = normalize_rel(rel)?;
    let abs = jail.resolve(&norm)?;
    let m = std::fs::metadata(&abs).map_err(|e| io_error(&norm, &e))?;
    if m.is_dir() {
        return Err(err("fs-is-a-dir", &norm));
    }
    let max = if max_bytes == 0 {
        DEFAULT_MAX_READ
    } else {
        max_bytes.min(MAX_READ_LIMIT)
    };
    let size = m.len();
    let (bytes, version) = if size <= HASH_LIMIT {
        let all = std::fs::read(&abs).map_err(|e| io_error(&norm, &e))?;
        let v = version_of(&all);
        (all, v)
    } else {
        let mut buf = Vec::new();
        std::fs::File::open(&abs)
            .and_then(|f| f.take(max).read_to_end(&mut buf))
            .map_err(|e| io_error(&norm, &e))?;
        let v = disk_version(&abs)
            .map_err(|e| io_error(&norm, &e))?
            .unwrap_or_default();
        (buf, v)
    };
    let truncated = (bytes.len() as u64) > max || size > bytes.len() as u64;
    let cut = &bytes[..bytes.len().min(max as usize)];
    let (text, encoding, binary) = decode(cut, truncated);
    Ok(FileContent {
        path: norm,
        text,
        encoding,
        binary,
        truncated,
        size,
        version,
    })
}

/// Build a [`FileContent`] from bytes that are not on disk (a git blob).
pub fn content_from_bytes(rel: &str, bytes: &[u8], max_bytes: u64) -> FileContent {
    let max = if max_bytes == 0 {
        DEFAULT_MAX_READ
    } else {
        max_bytes
    } as usize;
    let truncated = bytes.len() > max;
    let (text, encoding, binary) = decode(&bytes[..bytes.len().min(max)], truncated);
    FileContent {
        path: rel.to_owned(),
        text,
        encoding,
        binary,
        truncated,
        size: bytes.len() as u64,
        version: version_of(bytes),
    }
}

fn deny_git_internal(rel: &str) -> Result<(), ProtoError> {
    if is_git_internal(rel) {
        Err(crate::jail::denied(rel))
    } else {
        Ok(())
    }
}

/// `fs.write`: returns the new version.
pub fn write(
    jail: &RootJail,
    rel: &str,
    content: &str,
    expected_version: Option<&str>,
    encoding: Option<&TextEncoding>,
) -> Result<String, ProtoError> {
    let norm = normalize_rel(rel)?;
    deny_git_internal(&norm)?;
    let abs = jail.resolve(&norm)?;
    if abs == jail.root() {
        return Err(err("fs-is-a-dir", &norm));
    }
    if abs.is_dir() {
        return Err(err("fs-is-a-dir", &norm));
    }
    if let Some(expected) = expected_version {
        let actual = disk_version(&abs)
            .map_err(|e| io_error(&norm, &e))?
            .unwrap_or_default();
        if actual != expected {
            return Err(ProtoError::new("fs-conflict")
                .arg("path", &norm)
                .arg("expected", expected)
                .arg("actual", actual));
        }
    }
    let bytes = encode(content, encoding.unwrap_or(&TextEncoding::utf8()), &norm)?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(&norm, &e))?;
    }
    std::fs::write(&abs, &bytes).map_err(|e| io_error(&norm, &e))?;
    Ok(version_of(&bytes))
}

/// `fs.create_file`: returns the version.
pub fn create_file(jail: &RootJail, rel: &str, content: &str) -> Result<String, ProtoError> {
    let norm = normalize_rel(rel)?;
    deny_git_internal(&norm)?;
    let abs = jail.resolve_entry(&norm)?;
    // Follow-mode check too: the parent chain must not leave the root.
    jail.resolve(&norm)?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(&norm, &e))?;
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&abs)
        .map_err(|e| io_error(&norm, &e))?;
    f.write_all(content.as_bytes())
        .map_err(|e| io_error(&norm, &e))?;
    Ok(version_of(content.as_bytes()))
}

/// `fs.create_dir`.
pub fn create_dir(jail: &RootJail, rel: &str) -> Result<(), ProtoError> {
    let norm = normalize_rel(rel)?;
    deny_git_internal(&norm)?;
    let abs = jail.resolve_entry(&norm)?;
    jail.resolve(&norm)?;
    if std::fs::symlink_metadata(&abs).is_ok() {
        return Err(err("fs-exists", &norm));
    }
    std::fs::create_dir_all(&abs).map_err(|e| io_error(&norm, &e))
}

/// `fs.rename` (the entries themselves; links are moved, not followed).
pub fn rename(jail: &RootJail, from: &str, to: &str) -> Result<(), ProtoError> {
    let nf = normalize_rel(from)?;
    let nt = normalize_rel(to)?;
    deny_git_internal(&nf)?;
    deny_git_internal(&nt)?;
    let a = jail.resolve_entry(&nf)?;
    let b = jail.resolve_entry(&nt)?;
    if std::fs::symlink_metadata(&a).is_err() {
        return Err(err("fs-not-found", &nf));
    }
    // Case-only renames on case-insensitive file systems name the same entry.
    let same_entry = a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase();
    if std::fs::symlink_metadata(&b).is_ok() && !same_entry {
        return Err(err("fs-exists", &nt));
    }
    if b.starts_with(&a) && !same_entry {
        return Err(crate::jail::invalid(&nt));
    }
    if let Some(parent) = b.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(&nt, &e))?;
    }
    std::fs::rename(&a, &b).map_err(|e| io_error(&nf, &e))
}

/// `fs.delete` (a link is removed, never its target).
pub fn delete(jail: &RootJail, rel: &str, recursive: bool) -> Result<(), ProtoError> {
    let norm = normalize_rel(rel)?;
    deny_git_internal(&norm)?;
    let abs = jail.resolve_entry(&norm)?;
    let m = std::fs::symlink_metadata(&abs).map_err(|e| io_error(&norm, &e))?;
    if is_link(&abs) {
        return std::fs::remove_file(&abs)
            .or_else(|_| std::fs::remove_dir(&abs))
            .map_err(|e| io_error(&norm, &e));
    }
    if m.is_dir() {
        if recursive {
            remove_tree(&abs).map_err(|e| io_error(&norm, &e))
        } else {
            std::fs::remove_dir(&abs).map_err(|e| {
                if std::fs::read_dir(&abs).is_ok_and(|mut d| d.next().is_some()) {
                    err("fs-dir-not-empty", &norm)
                } else {
                    io_error(&norm, &e)
                }
            })
        }
    } else {
        std::fs::remove_file(&abs).map_err(|e| io_error(&norm, &e))
    }
}

/// Recursive delete that removes links (junctions included) without descending into them.
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    for ent in std::fs::read_dir(dir)? {
        let p = ent?.path();
        if is_link(&p) {
            std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p))?;
        } else if p.is_dir() {
            remove_tree(&p)?;
        } else {
            if let Ok(m) = std::fs::metadata(&p)
                && m.permissions().readonly()
            {
                let mut perm = m.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(&p, perm);
            }
            std::fs::remove_file(&p)?;
        }
    }
    std::fs::remove_dir(dir)
}

// ---------------------------------------------------------------------------------------
// Browsing

fn has_git(p: &Path) -> bool {
    std::fs::symlink_metadata(p.join(".git")).is_ok()
}

/// `browse.list`: sub-folders of an absolute folder inside the browse roots.
pub fn browse(jail: &PathJail, abs_dir: &str) -> Result<BrowseListing, ProtoError> {
    let dir = if abs_dir.trim().is_empty() {
        jail.roots()
            .first()
            .cloned()
            .ok_or_else(|| crate::jail::denied(abs_dir))?
    } else {
        if !Path::new(abs_dir).is_absolute() {
            return Err(crate::jail::invalid(abs_dir));
        }
        jail.resolve(abs_dir)?
    };
    let shown = dir.display().to_string();
    let rd = std::fs::read_dir(&dir).map_err(|e| io_error(&shown, &e))?;
    let mut entries = Vec::new();
    for ent in rd.flatten() {
        let Ok(ft) = ent.file_type() else { continue };
        let p = ent.path();
        // Directories and links to directories that stay inside the roots.
        let is_dir = if ft.is_dir() && !is_link(&p) {
            true
        } else if ft.is_symlink() || is_junction(&ft) || is_link(&p) {
            canonicalize(&p).is_ok_and(|c| c.is_dir() && jail.contains(&c))
        } else {
            false
        };
        if !is_dir {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        entries.push(BrowseEntry {
            is_git_repo: has_git(&p),
            path: p.display().to_string(),
            name,
        });
    }
    entries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    let parent = dir
        .parent()
        .filter(|p| jail.contains(p))
        .map(|p| p.display().to_string());
    Ok(BrowseListing {
        is_git_repo: has_git(&dir),
        path: shown,
        parent,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jail::testutil::*;

    #[test]
    fn decode_encodings() {
        let (t, e, b) = decode("안녕 hi".as_bytes(), false);
        assert_eq!(
            (t.as_str(), e.label.as_str(), e.bom, b),
            ("안녕 hi", "UTF-8", false, false)
        );
        let (t, e, _) = decode(b"\xEF\xBB\xBFx", false);
        assert_eq!((t.as_str(), e.bom), ("x", true));
        // UTF-16LE with BOM (contains NULs but is text).
        let mut u = vec![0xFF, 0xFE];
        for c in "가a".encode_utf16() {
            u.extend_from_slice(&c.to_le_bytes());
        }
        let (t, e, b) = decode(&u, false);
        assert_eq!(
            (t.as_str(), e.label.as_str(), b),
            ("가a", "UTF-16LE", false)
        );
        // Binary.
        assert!(decode(b"ab\0cd", false).2);
        // A cut in the middle of a UTF-8 sequence is dropped when truncated.
        let s = "가나".as_bytes();
        assert_eq!(decode(&s[..4], true).0, "가");
        // EUC-KR (CP949) text is detected.
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("한글 파일입니다. 안녕하세요, 반갑습니다.");
        let (t, e, _) = decode(&bytes, false);
        assert_eq!(e.label, "EUC-KR");
        assert!(t.starts_with("한글 파일"));
        // Round trips.
        for enc in [
            TextEncoding::utf8(),
            TextEncoding {
                label: "UTF-8".into(),
                bom: true,
            },
            TextEncoding {
                label: "UTF-16LE".into(),
                bom: true,
            },
            TextEncoding {
                label: "UTF-16BE".into(),
                bom: true,
            },
        ] {
            let b = encode("한글 text\r\n", &enc, "x").unwrap();
            let (t, e, _) = decode(&b, false);
            assert_eq!(t, "한글 text\r\n", "{enc:?}");
            assert_eq!(e.bom, enc.bom);
        }
        // Legacy encodings are written with encoding_rs (detection of short text is a guess).
        let euc = TextEncoding {
            label: "EUC-KR".into(),
            bom: false,
        };
        assert_eq!(
            encode("한글 text", &euc, "x").unwrap(),
            encoding_rs::EUC_KR.encode("한글 text").0.into_owned()
        );
        assert_eq!(
            encode(
                "x",
                &TextEncoding {
                    label: "nope".into(),
                    bom: false
                },
                "a"
            )
            .unwrap_err()
            .id,
            "fs-bad-encoding"
        );
        assert_eq!(
            encode(
                "😀",
                &TextEncoding {
                    label: "EUC-KR".into(),
                    bom: false
                },
                "a"
            )
            .unwrap_err()
            .id,
            "fs-unencodable"
        );
    }

    #[test]
    fn ops_and_conflicts() {
        let t = tmp("fsops");
        let root = t.join("wt");
        init_repo(
            &root,
            &[
                (".gitignore", "target/\n*.log\n!keep.log\n"),
                ("src/a.rs", "fn a() {}\n"),
            ],
        );
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("x.log"), "l").unwrap();
        std::fs::write(root.join("keep.log"), "k").unwrap();
        std::fs::create_dir_all(root.join("src/한글")).unwrap();
        std::fs::write(root.join("src/한글/파일.txt"), "내용\n").unwrap();
        let j = RootJail::new(&root).unwrap();
        let none = HashMap::new();

        let names = |v: &[FsEntry]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        let l = list(&j, "", false, &none).unwrap();
        // Directories first, then files (case-insensitive by name).
        assert_eq!(names(&l), ["src", ".gitignore", "keep.log"]);
        let all = list(&j, "", true, &none).unwrap();
        let ign: Vec<_> = all
            .iter()
            .filter(|e| e.ignored)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(ign, ["target", "x.log"]);
        assert!(!all.iter().any(|e| e.name == ".git"));
        assert_eq!(all[0].kind, EntryKind::Dir);
        // Inside an ignored directory everything is ignored.
        let inner = list(&j, "target", true, &none).unwrap();
        assert!(inner.iter().all(|e| e.ignored) && !inner.is_empty());
        assert!(list(&j, "target", false, &none).unwrap().is_empty());
        assert_eq!(
            list(&j, "nope", false, &none).unwrap_err().id,
            "fs-not-found"
        );
        assert_eq!(
            list(&j, "../x", false, &none).unwrap_err().id,
            "path-denied"
        );

        // Read + write with versions.
        let f = read(&j, "src/한글/파일.txt", 0).unwrap();
        assert_eq!(
            (f.text.as_str(), f.binary, f.truncated, f.size),
            ("내용\n", false, false, 7)
        );
        let v2 = write(&j, "src/한글/파일.txt", "새 내용\n", Some(&f.version), None).unwrap();
        assert_ne!(v2, f.version);
        let e = write(&j, "src/한글/파일.txt", "stale", Some(&f.version), None).unwrap_err();
        assert_eq!(e.id, "fs-conflict");
        assert_eq!(e.get("actual"), Some(v2.as_str()));
        assert_eq!(read(&j, "src/한글/파일.txt", 0).unwrap().version, v2);
        // Changed on disk by someone else -> conflict.
        std::fs::write(root.join("src/a.rs"), "fn b() {}\n").unwrap();
        let e = write(&j, "src/a.rs", "x", Some(&version_of(b"fn a() {}\n")), None).unwrap_err();
        assert_eq!(e.id, "fs-conflict");
        // Deleted on disk -> conflict with an empty actual version.
        std::fs::write(root.join("gone.txt"), "g").unwrap();
        let gv = read(&j, "gone.txt", 0).unwrap().version;
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        let e = write(&j, "gone.txt", "x", Some(&gv), None).unwrap_err();
        assert_eq!((e.id.as_str(), e.get("actual")), ("fs-conflict", Some("")));
        // Unconditional write creates.
        write(&j, "new/dir/n.txt", "n", None, None).unwrap();
        assert_eq!(read(&j, "new/dir/n.txt", 0).unwrap().text, "n");
        // Truncated read.
        write(&j, "big.txt", &"가".repeat(100), None, None).unwrap();
        let b = read(&j, "big.txt", 10).unwrap();
        assert!(b.truncated);
        assert_eq!(b.text, "가가가");
        assert_eq!(b.size, 300);
        assert_eq!(b.version, read(&j, "big.txt", 0).unwrap().version);
        std::fs::write(root.join("bin.dat"), b"\x00\x01\x02").unwrap();
        assert!(read(&j, "bin.dat", 0).unwrap().binary);
        assert_eq!(read(&j, "src", 0).unwrap_err().id, "fs-is-a-dir");

        // Create, rename, delete.
        create_file(&j, "c/new.txt", "hi").unwrap();
        assert_eq!(
            create_file(&j, "c/new.txt", "x").unwrap_err().id,
            "fs-exists"
        );
        create_dir(&j, "d1/d2").unwrap();
        assert_eq!(create_dir(&j, "d1").unwrap_err().id, "fs-exists");
        rename(&j, "c/new.txt", "d1/moved.txt").unwrap();
        assert!(root.join("d1/moved.txt").is_file());
        assert_eq!(rename(&j, "c/none", "d1/x").unwrap_err().id, "fs-not-found");
        create_file(&j, "d1/other.txt", "").unwrap();
        assert_eq!(
            rename(&j, "d1/moved.txt", "d1/other.txt").unwrap_err().id,
            "fs-exists"
        );
        assert_eq!(
            rename(&j, "d1", "d1/d2/inside").unwrap_err().id,
            "path-invalid"
        );
        rename(&j, "d1/moved.txt", "d1/Moved.txt").unwrap();
        assert_eq!(delete(&j, "d1", false).unwrap_err().id, "fs-dir-not-empty");
        delete(&j, "d1", true).unwrap();
        assert!(!root.join("d1").exists());
        assert_eq!(delete(&j, "", true).unwrap_err().id, "path-denied");
        assert_eq!(delete(&j, ".git", true).unwrap_err().id, "path-denied");
        assert_eq!(
            write(&j, ".git/config", "x", None, None).unwrap_err().id,
            "path-denied"
        );
        assert_eq!(create_file(&j, "con", "x").unwrap_err().id, "path-invalid");

        // Stat.
        let s = stat(&j, "src/a.rs").unwrap();
        assert_eq!(
            (s.kind, s.size, s.path.as_str()),
            (EntryKind::File, 10, "src/a.rs")
        );
        assert_eq!(stat(&j, "").unwrap().kind, EntryKind::Dir);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn links_are_not_followed_out() {
        let t = tmp("fslinks");
        let root = t.join("wt");
        std::fs::create_dir_all(&root).unwrap();
        let outside = t.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        assert!(make_dir_link(&outside, &root.join("link")));
        let j = RootJail::new(&root).unwrap();
        let l = list(&j, "", false, &HashMap::new()).unwrap();
        assert_eq!(l[0].kind, EntryKind::Symlink);
        assert_eq!(
            read(&j, "link/secret.txt", 0).unwrap_err().id,
            "path-denied"
        );
        assert_eq!(
            write(&j, "link/secret.txt", "x", None, None)
                .unwrap_err()
                .id,
            "path-denied"
        );
        assert_eq!(
            create_file(&j, "link/new.txt", "x").unwrap_err().id,
            "path-denied"
        );
        assert_eq!(
            list(&j, "link", false, &HashMap::new()).unwrap_err().id,
            "path-denied"
        );
        assert_eq!(stat(&j, "link").unwrap().kind, EntryKind::Symlink);
        // Deleting the link removes the link only.
        delete(&j, "link", true).unwrap();
        assert!(!root.join("link").exists());
        assert!(outside.join("secret.txt").is_file());
        // A recursive delete of a folder containing a link keeps the link's target.
        std::fs::create_dir_all(root.join("dir")).unwrap();
        assert!(make_dir_link(&outside, &root.join("dir").join("l2")));
        delete(&j, "dir", true).unwrap();
        assert!(outside.join("secret.txt").is_file());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn browse_roots() {
        let t = tmp("browse");
        let home = t.join("홈");
        init_repo(&home.join("repo1"), &[("a", "a")]);
        std::fs::create_dir_all(home.join("plain/deeper")).unwrap();
        std::fs::write(home.join("file.txt"), "f").unwrap();
        std::fs::create_dir_all(t.join("outside")).unwrap();
        assert!(make_dir_link(&t.join("outside"), &home.join("out-link")));
        let jail = PathJail::new([home.clone()]);
        let l = browse(&jail, "").unwrap();
        let names: Vec<_> = l
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_git_repo))
            .collect();
        assert_eq!(names, [("plain", false), ("repo1", true)]);
        assert!(l.parent.is_none());
        let sub = browse(&jail, &l.entries[0].path).unwrap();
        assert_eq!(sub.entries[0].name, "deeper");
        assert!(sub.parent.is_some());
        assert_eq!(
            browse(&jail, &t.display().to_string()).unwrap_err().id,
            "path-denied"
        );
        assert_eq!(browse(&jail, "relative").unwrap_err().id, "path-invalid");
        assert_eq!(
            browse(&jail, &home.join("out-link").display().to_string())
                .unwrap_err()
                .id,
            "path-denied"
        );
        remove_dir_link(&home.join("out-link"));
        let _ = std::fs::remove_dir_all(&t);
    }
}
