//! Git status / diff / file-at-revision for a session worktree, through the git CLI
//! (`meninx_core::git_raw`: `-c core.quotepath=false`, no prompts). Not routed through the
//! `Runner` yet (see docs/server-architecture.md).

use std::collections::HashMap;
use std::path::Path;

use meninx_proto::{
    AheadBehind, ChangeKind, CommitInfo, DiffAgainst, DiffFile, DiffHunk, DiffLine, GIT_LOG_PAGE,
    GIT_UNPUSHED_MAX, GitChange, GitLog, GitRev, GitStatus, LineKind, ProtoError,
};

use crate::service::core_error;

/// Run git; exit code 0 required. Returns stdout bytes.
pub fn run(root: &Path, args: &[&str]) -> Result<Vec<u8>, ProtoError> {
    let out = meninx_core::git_raw(root, args, &[("GIT_OPTIONAL_LOCKS", "0")])
        .map_err(|e| core_error(&e))?;
    if out.code == Some(0) {
        Ok(out.stdout)
    } else {
        Err(core_error(&out.error(args)))
    }
}

/// Run git and return trimmed stdout, or `None` on a non-zero exit.
fn try_text(root: &Path, args: &[&str]) -> Option<String> {
    let out = meninx_core::git_raw(root, args, &[("GIT_OPTIONAL_LOCKS", "0")]).ok()?;
    (out.code == Some(0)).then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `git-bad-rev`: a revision/branch argument that could be read as an option or is malformed.
fn check_rev(rev: &str) -> Result<(), ProtoError> {
    let bad = rev.is_empty()
        || rev.starts_with('-')
        || rev.contains(':')
        || rev.contains("..")
        || rev.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad {
        Err(ProtoError::new("git-bad-rev").arg("rev", rev))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Status

/// Parsed `git status --porcelain=v2 --branch -z`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StatusV2 {
    /// `branch.oid` (empty for `(initial)`).
    pub head: String,
    /// `branch.head` (`None` when detached).
    pub branch: Option<String>,
    /// `branch.upstream`.
    pub upstream: Option<String>,
    /// `branch.ab` (ahead, behind).
    pub ab: Option<(u32, u32)>,
    /// Index vs HEAD.
    pub staged: Vec<GitChange>,
    /// Worktree vs index.
    pub unstaged: Vec<GitChange>,
    /// Untracked.
    pub untracked: Vec<String>,
    /// Unmerged.
    pub conflicted: Vec<GitChange>,
}

fn letter_kind(c: u8) -> Option<ChangeKind> {
    Some(match c {
        b'M' => ChangeKind::Modified,
        b'T' => ChangeKind::TypeChanged,
        b'A' => ChangeKind::Added,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        b'U' => ChangeKind::Conflicted,
        _ => return None,
    })
}

/// The rest of `record` after `n` space-separated fields.
fn after_fields(record: &str, n: usize) -> &str {
    let mut rest = record;
    for _ in 0..n {
        match rest.find(' ') {
            Some(i) => rest = &rest[i + 1..],
            None => return "",
        }
    }
    rest
}

/// Parse porcelain v2 (`-z`) output.
pub fn parse_status_v2(out: &[u8]) -> StatusV2 {
    let text = String::from_utf8_lossy(out);
    let records: Vec<&str> = text.split('\0').collect();
    let mut s = StatusV2::default();
    let mut i = 0;
    while i < records.len() {
        let r = records[i];
        i += 1;
        if let Some(h) = r.strip_prefix("# ") {
            let (key, value) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => s.head = if value == "(initial)" { "" } else { value }.into(),
                "branch.head" => {
                    s.branch = (value != "(detached)").then(|| value.to_owned());
                }
                "branch.upstream" => s.upstream = Some(value.to_owned()),
                "branch.ab" => {
                    let mut it = value.split(' ');
                    let a = it
                        .next()
                        .and_then(|v| v.trim_start_matches('+').parse().ok());
                    let b = it
                        .next()
                        .and_then(|v| v.trim_start_matches('-').parse().ok());
                    if let (Some(a), Some(b)) = (a, b) {
                        s.ab = Some((a, b));
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some(t) = r.bytes().next() else { continue };
        match t {
            b'?' => s.untracked.push(r[2..].to_owned()),
            b'1' | b'2' => {
                let xy = r.as_bytes().get(2..4).unwrap_or(b"..");
                let path = after_fields(r, if t == b'1' { 8 } else { 9 }).to_owned();
                let orig = if t == b'2' {
                    let o = records.get(i).map(|o| o.to_string());
                    i += 1;
                    o
                } else {
                    None
                };
                if let Some(k) = letter_kind(xy[0]) {
                    let renamed = matches!(k, ChangeKind::Renamed | ChangeKind::Copied);
                    s.staged.push(GitChange {
                        path: path.clone(),
                        orig_path: if renamed { orig.clone() } else { None },
                        kind: k,
                    });
                }
                if let Some(k) = letter_kind(xy[1]) {
                    let renamed = matches!(k, ChangeKind::Renamed | ChangeKind::Copied);
                    s.unstaged.push(GitChange {
                        path,
                        orig_path: if renamed { orig } else { None },
                        kind: k,
                    });
                }
            }
            b'u' => s.conflicted.push(GitChange {
                path: after_fields(r, 10).to_owned(),
                orig_path: None,
                kind: ChangeKind::Conflicted,
            }),
            _ => {}
        }
    }
    for v in [&mut s.staged, &mut s.unstaged, &mut s.conflicted] {
        v.sort_by(|a, b| a.path.cmp(&b.path));
    }
    s.untracked.sort();
    s
}

fn status_raw(root: &Path, pathspec: Option<&str>) -> Result<StatusV2, ProtoError> {
    let mut args = vec![
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=all",
        "--find-renames",
    ];
    if let Some(p) = pathspec {
        args.push("--");
        args.push(p);
    }
    Ok(parse_status_v2(&run(root, &args)?))
}

/// The base branch: `base` if given, else the branch checked out in `main_repo` (the
/// session's repository), else `main`/`master` if they exist. Returns `(ref, merge_base)`.
pub fn resolve_base(
    root: &Path,
    main_repo: Option<&Path>,
    base: Option<&str>,
) -> Result<Option<(String, String)>, ProtoError> {
    let candidates: Vec<String> = match base {
        Some(b) => {
            check_rev(b)?;
            vec![b.to_owned()]
        }
        None => {
            let own = try_text(root, &["symbolic-ref", "--short", "-q", "HEAD"]);
            let mut c = Vec::new();
            if let Some(m) = main_repo
                && let Some(b) = try_text(m, &["symbolic-ref", "--short", "-q", "HEAD"])
                && !b.is_empty()
                && Some(&b) != own.as_ref()
            {
                c.push(b);
            }
            c.extend(["main".to_owned(), "master".to_owned()]);
            c
        }
    };
    for b in candidates {
        if try_text(
            root,
            &[
                "rev-parse",
                "--verify",
                "-q",
                "--end-of-options",
                &format!("{b}^{{commit}}"),
            ],
        )
        .is_none()
        {
            continue;
        }
        if let Some(mb) = try_text(root, &["merge-base", &b, "HEAD"]) {
            return Ok(Some((b, mb)));
        }
    }
    Ok(None)
}

fn ahead_behind(root: &Path, reference: &str) -> Option<AheadBehind> {
    let out = try_text(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{reference}...HEAD"),
        ],
    )?;
    let mut it = out.split_whitespace();
    let behind = it.next()?.parse().ok()?;
    let ahead = it.next()?.parse().ok()?;
    Some(AheadBehind {
        reference: reference.to_owned(),
        ahead,
        behind,
    })
}

/// `git.status`.
pub fn status(
    root: &Path,
    main_repo: Option<&Path>,
    base: Option<&str>,
) -> Result<GitStatus, ProtoError> {
    let s = status_raw(root, None)?;
    let upstream = s.upstream.as_ref().map(|u| {
        let (ahead, behind) = s.ab.unwrap_or((0, 0));
        AheadBehind {
            reference: u.clone(),
            ahead,
            behind,
        }
    });
    let (base_ab, merge_base) = if s.head.is_empty() {
        (None, None)
    } else {
        match resolve_base(root, main_repo, base)? {
            Some((b, mb)) => (ahead_behind(root, &b), Some(mb)),
            None => (None, None),
        }
    };
    Ok(GitStatus {
        branch: s.branch,
        head: s.head,
        upstream,
        base: base_ab,
        merge_base,
        staged: s.staged,
        unstaged: s.unstaged,
        untracked: s.untracked,
        conflicted: s.conflicted,
    })
}

/// Status badges for the children of `dir` (root-relative, `""` = root): child name ->
/// kind. A directory gets `Modified` when anything below it changed. Empty if not a repo.
pub fn badges(root: &Path, dir: &str) -> HashMap<String, ChangeKind> {
    let spec = if dir.is_empty() {
        ".".to_owned()
    } else {
        format!(":(literal){dir}")
    };
    let Ok(s) = status_raw(root, Some(&spec)) else {
        return HashMap::new();
    };
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut out = HashMap::new();
    let mut put = |path: &str, kind: ChangeKind| {
        let Some(rest) = path.strip_prefix(&prefix) else {
            return;
        };
        match rest.split_once('/') {
            None => {
                // The file's own kind; a conflict or an unstaged change wins over staged.
                out.insert(rest.to_owned(), kind);
            }
            Some((child, _)) => {
                out.entry(child.to_owned()).or_insert(ChangeKind::Modified);
            }
        }
    };
    for c in &s.staged {
        put(&c.path, c.kind);
    }
    for c in &s.unstaged {
        put(&c.path, c.kind);
    }
    for p in &s.untracked {
        put(p, ChangeKind::Untracked);
    }
    for c in &s.conflicted {
        put(&c.path, ChangeKind::Conflicted);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Diff

/// Undo git's C-style quoting (`"a\tb\303\251"`); unquoted input is returned as is.
pub fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return s.to_owned();
    };
    let b = inner.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let c = b[i + 1];
        i += 2;
        out.push(match c {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'0'..=b'7' => {
                let mut v = u32::from(c - b'0');
                for _ in 0..2 {
                    if i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                        v = v * 8 + u32::from(b[i] - b'0');
                        i += 1;
                    }
                }
                v as u8
            }
            other => other,
        });
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A `---`/`+++` path: `None` for `/dev/null`, else without the `a/`/`b/` prefix.
fn side_path(raw: &str, prefix: &str) -> Option<String> {
    let raw = raw.trim_end_matches(['\t', '\r']);
    if raw == "/dev/null" {
        return None;
    }
    let p = unquote(raw);
    Some(p.strip_prefix(prefix).map(str::to_owned).unwrap_or(p))
}

/// Paths from `diff --git a/X b/Y` when the names are unquoted and equal.
fn header_paths(rest: &str) -> Option<String> {
    if let Some(body) = rest.strip_prefix('"') {
        // `"a/x" "b/x"`: take the first quoted string.
        let end = body.find("\" ")? + 1;
        let a = unquote(&rest[..=end]);
        return a.strip_prefix("a/").map(str::to_owned);
    }
    let n = rest.len();
    if n.is_multiple_of(2) {
        return None;
    }
    let half = (n - 1) / 2;
    let (a, b) = (rest.get(..half)?, rest.get(half + 1..)?);
    match (a.strip_prefix("a/"), b.strip_prefix("b/")) {
        (Some(x), Some(y)) if x == y => Some(x.to_owned()),
        _ => None,
    }
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    // @@ -a[,b] +c[,d] @@ header
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, header) = rest.split_once(" @@").unwrap_or((rest, ""));
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: header.trim().to_owned(),
        lines: Vec::new(),
    })
}

/// Parse `git diff` patch output (`--no-color`, default `a/` `b/` prefixes) into files.
pub fn parse_diff(out: &[u8]) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let mut cur: Option<DiffFile> = None;
    // Remaining old/new lines of the current hunk; both 0 = in a header.
    let (mut old_left, mut new_left) = (0u32, 0u32);
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut minus: Option<Option<String>> = None;
    let finish = |f: Option<DiffFile>, files: &mut Vec<DiffFile>| {
        if let Some(f) = f {
            files.push(f);
        }
    };
    for raw in out.split(|b| *b == b'\n') {
        let line_owned = String::from_utf8_lossy(raw);
        let line: &str = &line_owned;
        if old_left > 0 || new_left > 0 {
            let Some(f) = cur.as_mut() else { break };
            let Some(h) = f.hunks.last_mut() else { break };
            let (kind, text) = match line.as_bytes().first() {
                Some(b' ') => (LineKind::Context, &line[1..]),
                Some(b'+') => (LineKind::Add, &line[1..]),
                Some(b'-') => (LineKind::Del, &line[1..]),
                Some(b'\\') => {
                    if let Some(l) = h.lines.last_mut() {
                        l.no_newline = true;
                    }
                    continue;
                }
                // An empty line inside a hunk: a context line whose space was stripped.
                None => (LineKind::Context, ""),
                _ => {
                    old_left = 0;
                    new_left = 0;
                    continue;
                }
            };
            let text = text.strip_suffix('\r').unwrap_or(text).to_owned();
            let (o, n) = match kind {
                LineKind::Context => {
                    old_no += 1;
                    new_no += 1;
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    (Some(old_no), Some(new_no))
                }
                LineKind::Add => {
                    new_no += 1;
                    new_left = new_left.saturating_sub(1);
                    (None, Some(new_no))
                }
                LineKind::Del => {
                    old_no += 1;
                    old_left = old_left.saturating_sub(1);
                    (Some(old_no), None)
                }
            };
            h.lines.push(DiffLine {
                kind,
                old_line: o,
                new_line: n,
                text,
                no_newline: false,
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish(cur.take(), &mut files);
            minus = None;
            cur = Some(DiffFile {
                path: header_paths(rest).unwrap_or_default(),
                old_path: None,
                kind: ChangeKind::Modified,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if line.starts_with("@@ ") {
            if let Some(h) = parse_hunk_header(line) {
                old_left = h.old_lines;
                new_left = h.new_lines;
                old_no = h.old_start.saturating_sub(1);
                new_no = h.new_start.saturating_sub(1);
                // `@@ -0,0 +1 @@`: start 0 means no lines on that side.
                if h.old_lines == 0 {
                    old_no = h.old_start;
                }
                if h.new_lines == 0 {
                    new_no = h.new_start;
                }
                f.hunks.push(h);
            }
        } else if line.starts_with("\\") {
            if let Some(l) = f.hunks.last_mut().and_then(|h| h.lines.last_mut()) {
                l.no_newline = true;
            }
        } else if line.starts_with("new file mode") {
            f.kind = ChangeKind::Added;
        } else if line.starts_with("deleted file mode") {
            f.kind = ChangeKind::Deleted;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            f.old_path = Some(unquote(p));
            f.kind = ChangeKind::Renamed;
        } else if let Some(p) = line.strip_prefix("rename to ") {
            f.path = unquote(p);
        } else if let Some(p) = line.strip_prefix("copy from ") {
            f.old_path = Some(unquote(p));
            f.kind = ChangeKind::Copied;
        } else if let Some(p) = line.strip_prefix("copy to ") {
            f.path = unquote(p);
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            f.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            minus = Some(side_path(p, "a/"));
        } else if let Some(p) = line.strip_prefix("+++ ") {
            let plus = side_path(p, "b/");
            match (&minus, plus) {
                (_, Some(n)) => f.path = n,
                (Some(Some(o)), None) => f.path = o.clone(),
                _ => {}
            }
        }
    }
    finish(cur.take(), &mut files);
    files
}

fn is_tracked(root: &Path, rel: &str) -> bool {
    try_text(root, &["ls-files", "-z", "--", &format!(":(literal){rel}")])
        .is_some_and(|o| !o.is_empty())
}

fn has_head(root: &Path) -> bool {
    try_text(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_some()
}

/// `git.diff`.
pub fn diff(
    root: &Path,
    main_repo: Option<&Path>,
    path: Option<&str>,
    against: DiffAgainst,
    base: Option<&str>,
) -> Result<Vec<DiffFile>, ProtoError> {
    // An untracked file diffs as all-added (except against the index).
    if let Some(p) = path
        && against != DiffAgainst::Index
        && !is_tracked(root, p)
        && root.join(p).is_file()
    {
        let out = meninx_core::git_raw(
            root,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--no-index",
                "--",
                "/dev/null",
                p,
            ],
            &[],
        )
        .map_err(|e| core_error(&e))?;
        if out.code != Some(0) && out.code != Some(1) {
            return Err(core_error(&out.error(&["diff", "--no-index", p])));
        }
        let mut files = parse_diff(&out.stdout);
        for f in &mut files {
            f.kind = ChangeKind::Untracked;
            f.path = p.to_owned();
        }
        return Ok(files);
    }
    let mut args: Vec<String> = [
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]
    .map(String::from)
    .to_vec();
    let mut against_rev: Option<String> = None;
    match against {
        DiffAgainst::WorkingTree => {}
        DiffAgainst::Index => args.push("--cached".into()),
        DiffAgainst::Head => {
            // No commit yet: everything is relative to the index.
            if has_head(root) {
                against_rev = Some("HEAD".into());
            }
        }
        DiffAgainst::Base => match resolve_base(root, main_repo, base)? {
            Some((_, mb)) => against_rev = Some(mb),
            None => {
                return Err(ProtoError::new("git-no-base").arg("base", base.unwrap_or_default()));
            }
        },
    }
    if let Some(r) = &against_rev {
        args.push(r.clone());
    }
    if let Some(p) = path {
        // A renamed file: include its old path so rename detection pairs them.
        let mut specs = vec![format!(":(literal){p}")];
        if against != DiffAgainst::WorkingTree {
            let mut ns: Vec<String> = ["diff", "--name-status", "-z", "--find-renames"]
                .map(String::from)
                .to_vec();
            if against == DiffAgainst::Index {
                ns.push("--cached".into());
            }
            if let Some(r) = &against_rev {
                ns.push(r.clone());
            }
            let ns_ref: Vec<&str> = ns.iter().map(String::as_str).collect();
            if let Ok(out) = run(root, &ns_ref) {
                let text = String::from_utf8_lossy(&out);
                let parts: Vec<&str> = text.split('\0').collect();
                let mut i = 0;
                while i < parts.len() {
                    let st = parts[i];
                    if st.starts_with('R') || st.starts_with('C') {
                        if parts.get(i + 2) == Some(&p) {
                            specs.push(format!(":(literal){}", parts[i + 1]));
                        }
                        i += 3;
                    } else {
                        i += 2;
                    }
                }
            }
        }
        args.push("--".into());
        args.extend(specs);
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let files = parse_diff(&run(root, &refs)?);
    Ok(match path {
        Some(p) => files
            .into_iter()
            .filter(|f| f.path == p || f.old_path.as_deref() == Some(p))
            .collect(),
        None => files,
    })
}

/// `git.file_at`: the blob bytes of `rel` at `rev`, `None` if it does not exist there.
pub fn file_at(
    root: &Path,
    main_repo: Option<&Path>,
    rel: &str,
    rev: &GitRev,
    base: Option<&str>,
) -> Result<Option<Vec<u8>>, ProtoError> {
    let spec = match rev {
        GitRev::Head => {
            if !has_head(root) {
                return Ok(None);
            }
            format!("HEAD:{rel}")
        }
        GitRev::Index => format!(":0:{rel}"),
        GitRev::Base => match resolve_base(root, main_repo, base)? {
            Some((_, mb)) => format!("{mb}:{rel}"),
            None => {
                return Err(ProtoError::new("git-no-base").arg("base", base.unwrap_or_default()));
            }
        },
        GitRev::Rev(r) => {
            check_rev(r)?;
            if try_text(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    "--end-of-options",
                    &format!("{r}^{{commit}}"),
                ],
            )
            .is_none()
            {
                return Err(ProtoError::new("git-bad-rev").arg("rev", r.as_str()));
            }
            format!("{r}:{rel}")
        }
    };
    let ty = try_text(root, &["cat-file", "-t", &spec]);
    match ty.as_deref() {
        None => Ok(None),
        Some("blob") => Ok(Some(run(root, &["cat-file", "blob", &spec])?)),
        Some(_) => Err(ProtoError::new("fs-is-a-dir").arg("path", rel)),
    }
}

// ---------------------------------------------------------------------------------------
// History (Hermes Desk's History tab: `changes.ts` commitLog/commitFiles/commitDiff,
// `github.ts` historyLocalCommits)

/// Most commits one `git.log` page returns (bounds the reply size).
const LOG_MAX: u32 = 10_000;

/// `git log` record format of Hermes Desk's `commitLog` (unit/record separators).
const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%s%x1f%an%x1f%at%x1f%ae%x1e";

/// Parse `git log` output in [`LOG_FORMAT`].
pub fn parse_log(out: &[u8]) -> Vec<CommitInfo> {
    String::from_utf8_lossy(out)
        .split('\x1e')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let mut f = r.split('\x1f');
            let sha = f.next()?.to_owned();
            let short = f.next()?.to_owned();
            let subject = f.next()?.to_owned();
            let author = f.next()?.to_owned();
            let date = f.next()?.parse().unwrap_or(0);
            let email = f.next().unwrap_or_default().to_owned();
            Some(CommitInfo {
                sha,
                short,
                subject,
                author,
                email,
                date,
            })
        })
        .collect()
}

/// `git.log`: a page of HEAD's history, newest first (`limit` 0 = [`GIT_LOG_PAGE`]).
pub fn log(root: &Path, skip: u32, limit: u32) -> Result<GitLog, ProtoError> {
    let limit = if limit == 0 {
        GIT_LOG_PAGE
    } else {
        limit.min(LOG_MAX)
    };
    if !has_head(root) {
        return Ok(GitLog {
            commits: Vec::new(),
            has_more: false,
        });
    }
    // One extra commit tells whether another page follows.
    let skip_arg = format!("--skip={skip}");
    let n = (limit + 1).to_string();
    let out = run(
        root,
        &[
            "log",
            "--no-color",
            LOG_FORMAT,
            &skip_arg,
            "-n",
            &n,
            "HEAD",
            "--",
        ],
    )?;
    let mut commits = parse_log(&out);
    let has_more = commits.len() > limit as usize;
    commits.truncate(limit as usize);
    Ok(GitLog { commits, has_more })
}

/// `git.unpushed`: Hermes Desk's `historyLocalCommits` (GitHub Desktop's count): a branch with
/// an upstream marks `upstream..HEAD`; one whose upstream is gone marks nothing (it is to be
/// published again); a branch tracking nothing (or a detached HEAD) marks the commits no
/// remote-tracking branch has. At most [`GIT_UNPUSHED_MAX`]. Empty without a remote, and on
/// any git failure (as in Desk).
pub fn unpushed(root: &Path) -> Vec<String> {
    if try_text(root, &["remote"]).is_none_or(|r| r.is_empty()) || !has_head(root) {
        return Vec::new();
    }
    let branch = try_text(root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let track = if !branch.is_empty() && branch != "HEAD" {
        try_text(
            root,
            &[
                "for-each-ref",
                "--format=%(upstream)|%(upstream:track)",
                &format!("refs/heads/{branch}"),
            ],
        )
        .unwrap_or_default()
    } else {
        String::new()
    };
    let (upstream, state) = track.split_once('|').unwrap_or((track.as_str(), ""));
    if !upstream.is_empty() && state.contains("gone") {
        return Vec::new();
    }
    let max = format!("--max-count={GIT_UNPUSHED_MAX}");
    let range = format!("{upstream}..HEAD");
    let mut args = vec!["rev-list", max.as_str()];
    if upstream.is_empty() {
        args.extend(["HEAD", "--not", "--remotes"]);
    } else {
        args.push(&range);
    }
    try_text(root, &args)
        .map(|o| o.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// A commit id from the client: 4-64 hex digits (Hermes Desk's `commitDetail` check) naming an
/// existing commit, else `git-bad-rev`. Returns the full id.
fn resolve_commit(root: &Path, sha: &str) -> Result<String, ProtoError> {
    let bad = || ProtoError::new("git-bad-rev").arg("rev", sha);
    if !(4..=64).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    try_text(
        root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            "--end-of-options",
            &format!("{sha}^{{commit}}"),
        ],
    )
    .filter(|s| !s.is_empty())
    .ok_or_else(bad)
}

/// `diff-tree` arguments comparing `commit` with its first parent, or with the empty tree for
/// a root commit (`--root`).
fn commit_range(root: &Path, commit: &str) -> Vec<String> {
    match try_text(
        root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("{commit}^1^{{commit}}"),
        ],
    ) {
        Some(parent) if !parent.is_empty() => vec![parent, commit.to_owned()],
        _ => vec!["--root".into(), "--no-commit-id".into(), commit.to_owned()],
    }
}

/// Parse `diff-tree --name-status -z` output.
pub fn parse_name_status(out: &[u8]) -> Vec<GitChange> {
    let text = String::from_utf8_lossy(out);
    let parts: Vec<&str> = text.split('\0').filter(|p| !p.is_empty()).collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let st = parts[i];
        i += 1;
        let letter = st.bytes().next().unwrap_or(b'M');
        if letter == b'R' || letter == b'C' {
            let (Some(orig), Some(path)) = (parts.get(i), parts.get(i + 1)) else {
                break;
            };
            i += 2;
            files.push(GitChange {
                path: (*path).to_owned(),
                orig_path: Some((*orig).to_owned()),
                kind: letter_kind(letter).unwrap_or(ChangeKind::Renamed),
            });
        } else {
            let Some(path) = parts.get(i) else { break };
            i += 1;
            files.push(GitChange {
                path: (*path).to_owned(),
                orig_path: None,
                kind: letter_kind(letter).unwrap_or(ChangeKind::Modified),
            });
        }
    }
    files
}

/// `git.commit_files`: files `sha` changed against its first parent (root: the empty tree),
/// renames detected, in git's order.
pub fn commit_files(root: &Path, sha: &str) -> Result<Vec<GitChange>, ProtoError> {
    let commit = resolve_commit(root, sha)?;
    let mut args: Vec<String> = ["diff-tree", "-r", "-M", "--name-status", "-z"]
        .map(String::from)
        .to_vec();
    args.extend(commit_range(root, &commit));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    Ok(parse_name_status(&run(root, &refs)?))
}

/// `git.commit_diff`: patch of `sha` against its first parent (root: the empty tree); `path`
/// = only that file (a rename's old path matches too).
pub fn commit_diff(
    root: &Path,
    sha: &str,
    path: Option<&str>,
) -> Result<Vec<DiffFile>, ProtoError> {
    let commit = resolve_commit(root, sha)?;
    let range = commit_range(root, &commit);
    let mut args: Vec<String> = [
        "diff-tree",
        "-r",
        "-p",
        "-M",
        "--no-color",
        "--no-ext-diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]
    .map(String::from)
    .to_vec();
    args.extend(range.iter().cloned());
    if let Some(p) = path {
        // A renamed file: include the other side's path so rename detection pairs them.
        let mut specs = vec![format!(":(literal){p}")];
        let mut ns: Vec<String> = ["diff-tree", "-r", "-M", "--name-status", "-z"]
            .map(String::from)
            .to_vec();
        ns.extend(range.iter().cloned());
        let ns_ref: Vec<&str> = ns.iter().map(String::as_str).collect();
        for c in parse_name_status(&run(root, &ns_ref)?) {
            match c.orig_path {
                Some(o) if c.path == p => specs.push(format!(":(literal){o}")),
                Some(o) if o == p => specs.push(format!(":(literal){}", c.path)),
                _ => {}
            }
        }
        args.push("--".into());
        args.extend(specs);
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let files = parse_diff(&run(root, &refs)?);
    Ok(match path {
        Some(p) => files
            .into_iter()
            .filter(|f| f.path == p || f.old_path.as_deref() == Some(p))
            .collect(),
        None => files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jail::testutil::*;

    #[test]
    fn porcelain_v2_parsing() {
        let out = b"# branch.oid 1234\0# branch.head feature\0# branch.upstream origin/feature\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 A. N... 000000 100644 100644 000 ccc new file.txt\0\
2 R. N... 100644 100644 100644 ddd ddd R100 \xed\x95\x9c\xea\xb8\x80.txt\0old name.txt\0\
1 MD N... 100644 100644 000000 eee fff both.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? untracked dir/x.txt\0! ignored.log\0";
        let s = parse_status_v2(out);
        assert_eq!(s.head, "1234");
        assert_eq!(s.branch.as_deref(), Some("feature"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feature"));
        assert_eq!(s.ab, Some((2, 1)));
        let staged: Vec<_> = s
            .staged
            .iter()
            .map(|c| (c.path.as_str(), c.kind, c.orig_path.as_deref()))
            .collect();
        assert_eq!(
            staged,
            [
                ("both.rs", ChangeKind::Modified, None),
                ("new file.txt", ChangeKind::Added, None),
                ("한글.txt", ChangeKind::Renamed, Some("old name.txt")),
            ]
        );
        let unstaged: Vec<_> = s
            .unstaged
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            unstaged,
            [
                ("both.rs", ChangeKind::Deleted),
                ("src/a.rs", ChangeKind::Modified)
            ]
        );
        assert_eq!(s.untracked, ["untracked dir/x.txt"]);
        assert_eq!(s.conflicted[0].path, "conflict.rs");
        let s = parse_status_v2(b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!((s.head.as_str(), s.branch), ("", None));
    }

    #[test]
    fn diff_parsing() {
        let out = [
            "diff --git a/src/a.rs b/src/a.rs",
            "index 111..222 100644",
            "--- a/src/a.rs",
            "+++ b/src/a.rs",
            "@@ -1,3 +1,4 @@ fn main() {",
            " one",
            "-two",
            "+TWO",
            "+-- not a header",
            " three",
            "@@ -10 +11,0 @@",
            "-gone",
            "\\ No newline at end of file",
            "diff --git a/old.txt b/new name.txt",
            "similarity index 90%",
            "rename from old.txt",
            "rename to new name.txt",
            "diff --git a/img.png b/img.png",
            "new file mode 100644",
            "index 0000000..abc",
            "Binary files /dev/null and b/img.png differ",
            "diff --git \"a/tab\\there.txt\" \"b/tab\\there.txt\"",
            "deleted file mode 100644",
            "--- \"a/tab\\there.txt\"",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-x\r",
            "",
        ]
        .join("\n");
        let f = parse_diff(out.as_bytes());
        assert_eq!(f.len(), 4);
        assert_eq!(
            (f[0].path.as_str(), f[0].kind),
            ("src/a.rs", ChangeKind::Modified)
        );
        let h = &f[0].hunks;
        assert_eq!(h.len(), 2);
        assert_eq!(
            (
                h[0].old_start,
                h[0].old_lines,
                h[0].new_start,
                h[0].new_lines
            ),
            (1, 3, 1, 4)
        );
        assert_eq!(h[0].header, "fn main() {");
        let l: Vec<_> = h[0]
            .lines
            .iter()
            .map(|l| (l.kind, l.old_line, l.new_line, l.text.as_str()))
            .collect();
        assert_eq!(
            l,
            [
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Del, Some(2), None, "two"),
                (LineKind::Add, None, Some(2), "TWO"),
                (LineKind::Add, None, Some(3), "-- not a header"),
                (LineKind::Context, Some(3), Some(4), "three"),
            ]
        );
        assert_eq!(h[1].lines[0].old_line, Some(10));
        assert!(h[1].lines[0].no_newline);
        assert_eq!(
            (f[1].path.as_str(), f[1].old_path.as_deref(), f[1].kind),
            ("new name.txt", Some("old.txt"), ChangeKind::Renamed)
        );
        assert!(f[1].hunks.is_empty());
        assert!(f[2].binary && f[2].kind == ChangeKind::Added && f[2].path == "img.png");
        assert_eq!(
            (f[3].path.as_str(), f[3].kind),
            ("tab\there.txt", ChangeKind::Deleted)
        );
        assert_eq!(f[3].hunks[0].lines[0].text, "x");
        assert_eq!(unquote("\"\\355\\225\\234\""), "한");
    }

    #[test]
    fn status_diff_file_at_on_a_repo() {
        let t = tmp("git");
        let repo = t.join("repo");
        init_repo(
            &repo,
            &[
                ("a.txt", "one\ntwo\nthree\n"),
                ("한글 이름.txt", "가\n"),
                ("ren.txt", "rename me\nline 2\nline 3\n"),
                ("del.txt", "bye\n"),
                ("img.bin", "\0\x01\x02"),
            ],
        );
        // Session worktree on its own branch with one commit ahead of main.
        let wt = t.join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "meninx/s1",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::write(wt.join("a.txt"), "one\nTWO\nthree\nfour").unwrap();
        git(&wt, &["commit", "-qam", "edit a"]);
        // main moves on too.
        std::fs::write(repo.join("main-only.txt"), "m\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "main"]);
        // Uncommitted: staged rename, unstaged edit, deletion, untracked, binary edit.
        git(&wt, &["mv", "ren.txt", "renamed.txt"]);
        std::fs::write(wt.join("한글 이름.txt"), "가\n나\n").unwrap();
        std::fs::remove_file(wt.join("del.txt")).unwrap();
        std::fs::write(wt.join("new.txt"), "new\n").unwrap();
        std::fs::write(wt.join("img.bin"), "\0\x01\x03").unwrap();

        let s = status(&wt, Some(&repo), None).unwrap();
        assert_eq!(s.branch.as_deref(), Some("meninx/s1"));
        let base = s.base.clone().unwrap();
        assert_eq!(
            (base.reference.as_str(), base.ahead, base.behind),
            ("main", 1, 1)
        );
        assert!(s.merge_base.is_some());
        assert_eq!(s.staged.len(), 1);
        assert_eq!(
            (s.staged[0].kind, s.staged[0].orig_path.as_deref()),
            (ChangeKind::Renamed, Some("ren.txt"))
        );
        let unstaged: Vec<_> = s
            .unstaged
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            unstaged,
            [
                ("del.txt", ChangeKind::Deleted),
                ("img.bin", ChangeKind::Modified),
                ("한글 이름.txt", ChangeKind::Modified)
            ]
        );
        assert_eq!(s.untracked, ["new.txt"]);
        // Explicit base / bad base.
        assert!(status(&wt, None, Some("main")).unwrap().base.is_some());
        assert!(status(&wt, None, Some("nope")).unwrap().base.is_none());
        assert_eq!(
            status(&wt, None, Some("--exec=x")).unwrap_err().id,
            "git-bad-rev"
        );

        // Diffs.
        let d = diff(
            &wt,
            Some(&repo),
            Some("한글 이름.txt"),
            DiffAgainst::WorkingTree,
            None,
        )
        .unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].hunks[0].lines.last().unwrap().text, "나");
        let d = diff(
            &wt,
            Some(&repo),
            Some("renamed.txt"),
            DiffAgainst::Index,
            None,
        )
        .unwrap();
        assert_eq!(
            (d[0].kind, d[0].old_path.as_deref()),
            (ChangeKind::Renamed, Some("ren.txt"))
        );
        let d = diff(&wt, Some(&repo), Some("new.txt"), DiffAgainst::Head, None).unwrap();
        assert_eq!(
            (d[0].kind, d[0].path.as_str()),
            (ChangeKind::Untracked, "new.txt")
        );
        assert_eq!(d[0].hunks[0].lines[0].kind, LineKind::Add);
        let d = diff(&wt, Some(&repo), Some("img.bin"), DiffAgainst::Head, None).unwrap();
        assert!(d[0].binary);
        // Against the base: a.txt's committed edit (no newline at the end).
        let d = diff(&wt, Some(&repo), Some("a.txt"), DiffAgainst::Base, None).unwrap();
        let last = d[0].hunks[0].lines.last().unwrap();
        assert_eq!((last.text.as_str(), last.no_newline), ("four", true));
        assert!(
            diff(&wt, Some(&repo), Some("a.txt"), DiffAgainst::Head, None)
                .unwrap()
                .is_empty()
        );
        let all = diff(&wt, Some(&repo), None, DiffAgainst::Head, None).unwrap();
        let mut names: Vec<_> = all.iter().map(|f| (f.path.as_str(), f.kind)).collect();
        names.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            names,
            [
                ("del.txt", ChangeKind::Deleted),
                ("img.bin", ChangeKind::Modified),
                ("renamed.txt", ChangeKind::Renamed),
                ("한글 이름.txt", ChangeKind::Modified)
            ]
        );
        // main-only.txt is not part of the session's base diff (merge base, not main's tip).
        let all = diff(&wt, Some(&repo), None, DiffAgainst::Base, None).unwrap();
        assert!(!all.iter().any(|f| f.path == "main-only.txt"));

        // file_at.
        assert_eq!(
            file_at(&wt, Some(&repo), "a.txt", &GitRev::Head, None)
                .unwrap()
                .unwrap(),
            b"one\nTWO\nthree\nfour"
        );
        assert_eq!(
            file_at(&wt, Some(&repo), "a.txt", &GitRev::Base, None)
                .unwrap()
                .unwrap(),
            b"one\ntwo\nthree\n"
        );
        assert_eq!(
            file_at(&wt, Some(&repo), "renamed.txt", &GitRev::Index, None)
                .unwrap()
                .unwrap(),
            b"rename me\nline 2\nline 3\n"
        );
        assert!(
            file_at(&wt, Some(&repo), "new.txt", &GitRev::Head, None)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            file_at(
                &wt,
                Some(&repo),
                "한글 이름.txt",
                &GitRev::Rev("main".into()),
                None
            )
            .unwrap()
            .unwrap(),
            "가\n".as_bytes()
        );
        assert_eq!(
            file_at(&wt, None, "a.txt", &GitRev::Rev("-p".into()), None)
                .unwrap_err()
                .id,
            "git-bad-rev"
        );
        assert_eq!(
            file_at(&wt, None, "a.txt", &GitRev::Rev("nosuch".into()), None)
                .unwrap_err()
                .id,
            "git-bad-rev"
        );

        // Badges.
        std::fs::create_dir_all(wt.join("dir/sub")).unwrap();
        std::fs::write(wt.join("dir/sub/x.txt"), "x").unwrap();
        let b = badges(&wt, "");
        assert_eq!(b.get("dir"), Some(&ChangeKind::Modified));
        assert_eq!(b.get("new.txt"), Some(&ChangeKind::Untracked));
        assert_eq!(b.get("del.txt"), Some(&ChangeKind::Deleted));
        assert_eq!(b.get("a.txt"), None);
        let b = badges(&wt, "dir");
        assert_eq!(b.get("sub"), Some(&ChangeKind::Modified));
        assert!(badges(&t, "").is_empty());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn history_log_files_diff_unpushed() {
        let t = tmp("git-history");
        let repo = t.join("repo");
        init_repo(
            &repo,
            &[
                ("a.txt", "one\ntwo\n"),
                ("old.txt", "rename me\nline 2\nline 3\n"),
            ],
        );
        let head = |r: &Path| git(r, &["rev-parse", "HEAD"]).trim().to_owned();
        let root_sha = head(&repo);
        std::fs::write(repo.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "edit a, add b"]);
        let edit_sha = head(&repo);
        git(&repo, &["mv", "old.txt", "새 이름.txt"]);
        git(&repo, &["commit", "-qm", "rename"]);
        let ren_sha = head(&repo);
        // A merge commit: its files/diff are against the first parent.
        git(&repo, &["checkout", "-q", "-b", "side"]);
        std::fs::write(repo.join("side.txt"), "s\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "side"]);
        git(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("b.txt"), "b\nmain\n").unwrap();
        git(&repo, &["commit", "-qam", "main edit"]);
        git(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
        );
        let merge_sha = head(&repo);

        // Log paging: init, edit, rename, side, main edit, merge = 6 commits.
        let all = log(&repo, 0, 0).unwrap();
        assert_eq!((all.commits.len(), all.has_more), (6, false));
        let c = &all.commits[0];
        assert_eq!(
            (
                c.sha.as_str(),
                c.subject.as_str(),
                c.author.as_str(),
                c.email.as_str()
            ),
            (merge_sha.as_str(), "merge side", "t", "t@example.com")
        );
        assert!(c.sha.starts_with(&c.short) && c.short.len() >= 4 && c.date > 1_600_000_000);
        assert_eq!(all.commits[5].sha, root_sha);
        let p1 = log(&repo, 0, 2).unwrap();
        assert_eq!((p1.commits.len(), p1.has_more), (2, true));
        let p3 = log(&repo, 4, 2).unwrap();
        assert_eq!((p3.commits.len(), p3.has_more), (2, false));
        assert_eq!(p3.commits[1].sha, root_sha);
        let p4 = log(&repo, 5, 2).unwrap();
        assert_eq!((p4.commits.len(), p4.has_more), (1, false));
        assert!(log(&repo, 50, 2).unwrap().commits.is_empty());

        // Commit files.
        let kinds = |v: Vec<GitChange>| -> Vec<(String, Option<String>, ChangeKind)> {
            v.into_iter()
                .map(|c| (c.path, c.orig_path, c.kind))
                .collect()
        };
        assert_eq!(
            kinds(commit_files(&repo, &root_sha).unwrap()),
            [
                ("a.txt".into(), None, ChangeKind::Added),
                ("old.txt".into(), None, ChangeKind::Added)
            ]
        );
        assert_eq!(
            kinds(commit_files(&repo, &edit_sha[..7]).unwrap()),
            [
                ("a.txt".into(), None, ChangeKind::Modified),
                ("b.txt".into(), None, ChangeKind::Added)
            ]
        );
        assert_eq!(
            kinds(commit_files(&repo, &ren_sha).unwrap()),
            [(
                "새 이름.txt".into(),
                Some("old.txt".into()),
                ChangeKind::Renamed
            )]
        );
        assert_eq!(
            kinds(commit_files(&repo, &merge_sha).unwrap()),
            [("side.txt".into(), None, ChangeKind::Added)]
        );

        // Commit diffs.
        let d = commit_diff(&repo, &edit_sha, Some("a.txt")).unwrap();
        assert_eq!(d.len(), 1);
        let l: Vec<_> = d[0].hunks[0]
            .lines
            .iter()
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(
            l,
            [
                (LineKind::Context, "one"),
                (LineKind::Del, "two"),
                (LineKind::Add, "TWO")
            ]
        );
        let d = commit_diff(&repo, &edit_sha, None).unwrap();
        assert_eq!(
            d.iter()
                .map(|f| (f.path.as_str(), f.kind))
                .collect::<Vec<_>>(),
            [
                ("a.txt", ChangeKind::Modified),
                ("b.txt", ChangeKind::Added)
            ]
        );
        let d = commit_diff(&repo, &root_sha, None).unwrap();
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|f| f.kind == ChangeKind::Added));
        assert_eq!(d[1].hunks[0].lines.len(), 3);
        for p in ["새 이름.txt", "old.txt"] {
            let d = commit_diff(&repo, &ren_sha, Some(p)).unwrap();
            assert_eq!(
                (
                    d.len(),
                    d[0].kind,
                    d[0].path.as_str(),
                    d[0].old_path.as_deref()
                ),
                (1, ChangeKind::Renamed, "새 이름.txt", Some("old.txt"))
            );
        }
        let d = commit_diff(&repo, &merge_sha, None).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "side.txt");
        assert!(
            commit_diff(&repo, &edit_sha, Some("nope.txt"))
                .unwrap()
                .is_empty()
        );

        // Invalid commit ids.
        for bad in [
            "-p",
            "--output=x",
            "HEAD",
            "main",
            "abc",
            "zzzzzzz",
            "deadbeefdeadbeef",
            "",
        ] {
            assert_eq!(
                commit_files(&repo, bad).unwrap_err().id,
                "git-bad-rev",
                "{bad}"
            );
            assert_eq!(
                commit_diff(&repo, bad, None).unwrap_err().id,
                "git-bad-rev",
                "{bad}"
            );
        }

        // Unpushed: nothing without a remote.
        assert!(unpushed(&repo).is_empty());
        let origin = t.join("origin.git");
        git(&t, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        assert!(unpushed(&repo).is_empty());
        std::fs::write(repo.join("c.txt"), "c\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "c1"]);
        let c1 = head(&repo);
        std::fs::write(repo.join("c.txt"), "c2\n").unwrap();
        git(&repo, &["commit", "-qam", "c2"]);
        let c2 = head(&repo);
        assert_eq!(unpushed(&repo), [c2.clone(), c1.clone()]);
        // A branch tracking nothing: the commits no remote branch has.
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "f\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "f"]);
        let f = head(&repo);
        assert_eq!(unpushed(&repo), [f.clone(), c2.clone(), c1.clone()]);
        // Published, then the remote branch deleted (merged PR): upstream gone -> none.
        git(&repo, &["push", "-q", "-u", "origin", "feature"]);
        assert!(unpushed(&repo).is_empty());
        git(&repo, &["push", "-q", "origin", "--delete", "feature"]);
        git(&repo, &["fetch", "-q", "--prune", "origin"]);
        assert!(unpushed(&repo).is_empty());

        // No commit yet: empty page, nothing unpushed.
        let empty = t.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        let l = log(&empty, 0, 0).unwrap();
        assert!(l.commits.is_empty() && !l.has_more);
        assert!(unpushed(&empty).is_empty());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn log_and_name_status_parsing() {
        let out = "aaaa\x1faa\x1fsubject: x\x1fme\x1f1700000000\x1fm@x\x1e\nbbbb\x1fbb\x1f\x1fyou\x1fbad\x1f\x1e\n";
        let c = parse_log(out.as_bytes());
        assert_eq!(c.len(), 2);
        assert_eq!(
            (c[0].subject.as_str(), c[0].date, c[0].email.as_str()),
            ("subject: x", 1_700_000_000, "m@x")
        );
        assert_eq!(c[1].subject, "");
        assert_eq!((c[1].date, c[1].email.as_str()), (0, ""));
        let n = parse_name_status(b"M\0a.txt\0R087\0old\0new\0T\0link\0C100\0x\0y\0D\0gone\0");
        let k: Vec<_> = n
            .iter()
            .map(|c| (c.path.as_str(), c.orig_path.as_deref(), c.kind))
            .collect();
        assert_eq!(
            k,
            [
                ("a.txt", None, ChangeKind::Modified),
                ("new", Some("old"), ChangeKind::Renamed),
                ("link", None, ChangeKind::TypeChanged),
                ("y", Some("x"), ChangeKind::Copied),
                ("gone", None, ChangeKind::Deleted)
            ]
        );
    }
}
