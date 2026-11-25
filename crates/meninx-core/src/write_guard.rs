//! Write guard (`docs/write-guard.md`): what an agent's tool call writes, and which of those
//! writes leave the session's own area (its folder and worktrees, the current branch of its
//! repositories) and so need the user's approval per folder.
//!
//! [`shell_writes`] reads a shell command the way a person skimming it would: it follows
//! `cd` / `pushd` / `Set-Location`, finds git commands that change a repository (`-C <dir>`
//! included) and the branches they write, redirects, and the usual file commands of POSIX
//! shells and PowerShell, also inside `bash -c` / `powershell -Command` / `cmd /c`. It cannot
//! be complete (variables, scripts, programs that write files themselves): it is a guard
//! against the common cases, not a sandbox. [`needs`] then checks each write against a
//! [`Scope`].

use std::path::{Component, Path, PathBuf};

use crate::repo::normalize_path;

/// Something a tool call writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// A file or folder created, changed or removed.
    Path(PathBuf),
    /// A git command that changes the repository around `dir`.
    Git {
        /// Where it runs (cwd plus `-C`).
        dir: PathBuf,
        /// The command, for the question shown to the user.
        command: String,
        /// Branches it writes or moves the worktree to: `(name, certain)`. Not certain = an
        /// argument that is a branch only if such a local branch exists (`git checkout x`).
        branches: Vec<(String, bool)>,
    },
}

/// Where a session may write without asking.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Folders written freely (the session folder, its scratch dirs, the agent's account).
    pub free: Vec<PathBuf>,
    /// The session's repositories: git writes there are free on `branch` only.
    pub repos: Vec<ScopeRepo>,
}

/// One repository of a session.
#[derive(Debug, Clone)]
pub struct ScopeRepo {
    /// The worktree (original-repo sessions: the repository itself).
    pub path: PathBuf,
    /// The branch the session works on; `None` = whatever is checked out (original-repo
    /// sessions work on the repository's current branch).
    pub branch: Option<String>,
    /// The registered repository's own folder (asked for when another branch is written).
    pub repo_path: PathBuf,
}

/// A write that needs the user's approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    /// The folder to approve (a repository's top level, else the nearest existing folder).
    pub folder: PathBuf,
    /// What is written, in a line.
    pub what: String,
}

/// Does the approved `folder` cover `path` (the folder itself or anything below it)?
pub fn covers(folder: &Path, path: &Path) -> bool {
    let f = normalize_path(folder);
    let p = normalize_path(path);
    !f.is_empty() && (p == f || p.starts_with(&format!("{f}/")))
}

/// The writes of a shell command run in `cwd` (`home` for `~`).
pub fn shell_writes(command: &str, cwd: Option<&Path>, home: Option<&Path>) -> Vec<Write> {
    let mut out = Vec::new();
    let mut cwd = cwd.map(Path::to_path_buf);
    scan(command, &mut cwd, home, &mut out, 0);
    out
}

fn scan(command: &str, cwd: &mut Option<PathBuf>, home: Option<&Path>, out: &mut Vec<Write>, depth: u8) {
    for seg in split(command) {
        for r in &seg.redirects {
            if !is_null_device(r)
                && let Some(p) = resolve(cwd.as_deref(), home, r)
            {
                out.push(Write::Path(p));
            }
        }
        segment(&seg.words, cwd, home, out, depth);
    }
}

/// One simple command: words and output redirect targets.
#[derive(Debug, Default)]
struct Segment {
    words: Vec<String>,
    redirects: Vec<String>,
}

/// Split a command line into simple commands (`&&`, `||`, `;`, `|`, `&`, newlines,
/// parentheses), honouring quotes and skipping heredoc bodies. Backslashes stay as they are
/// (Windows paths).
fn split(s: &str) -> Vec<Segment> {
    let chars: Vec<char> = s.chars().collect();
    let mut segs = Vec::new();
    let mut seg = Segment::default();
    let mut word = String::new();
    let mut in_word = false;
    let mut heredocs: Vec<String> = Vec::new();
    let mut i = 0;
    // Reads one word starting at `i` (quotes joined), for redirect targets and heredoc tags.
    let read_word = |i: &mut usize| -> String {
        while *i < chars.len() && (chars[*i] == ' ' || chars[*i] == '\t') {
            *i += 1;
        }
        let mut w = String::new();
        while *i < chars.len() {
            let c = chars[*i];
            match c {
                '\'' | '"' => {
                    *i += 1;
                    while *i < chars.len() && chars[*i] != c {
                        w.push(chars[*i]);
                        *i += 1;
                    }
                    *i += 1;
                }
                ' ' | '\t' | '\n' | '\r' | ';' | '|' | '&' | '(' | ')' | '<' | '>' => break,
                _ => {
                    w.push(c);
                    *i += 1;
                }
            }
        }
        w
    };
    let end_word = |word: &mut String, in_word: &mut bool, seg: &mut Segment| {
        if *in_word {
            seg.words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && matches!(chars.get(i + 1), Some('"' | '\\' | '$' | '`')) {
                        i += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            ' ' | '\t' | '\r' => {
                end_word(&mut word, &mut in_word, &mut seg);
                i += 1;
            }
            '\n' | ';' | '|' | '&' | '(' | ')' => {
                end_word(&mut word, &mut in_word, &mut seg);
                // `&>` / `&>>`: a redirect of both streams.
                if c == '&' && chars.get(i + 1) == Some(&'>') {
                    i += 1;
                    continue;
                }
                if !seg.words.is_empty() || !seg.redirects.is_empty() {
                    segs.push(std::mem::take(&mut seg));
                }
                i += 1;
                if c == '\n' && !heredocs.is_empty() {
                    // Skip heredoc bodies up to their tags.
                    for tag in std::mem::take(&mut heredocs) {
                        loop {
                            let start = i;
                            while i < chars.len() && chars[i] != '\n' {
                                i += 1;
                            }
                            let line: String = chars[start..i].iter().collect();
                            i = (i + 1).min(chars.len());
                            if line.trim() == tag || i >= chars.len() {
                                break;
                            }
                        }
                    }
                }
            }
            '>' => {
                // A file descriptor right before (`2>`) is not a word.
                if in_word && word.chars().all(|d| d.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                }
                end_word(&mut word, &mut in_word, &mut seg);
                i += 1;
                if chars.get(i) == Some(&'>') {
                    i += 1;
                }
                if chars.get(i) == Some(&'&') {
                    // `>&2`: a descriptor copy.
                    i += 1;
                    let _ = read_word(&mut i);
                    continue;
                }
                let target = read_word(&mut i);
                if !target.is_empty() {
                    seg.redirects.push(target);
                }
            }
            '<' => {
                end_word(&mut word, &mut in_word, &mut seg);
                i += 1;
                if chars.get(i) == Some(&'<') {
                    i += 1;
                    if chars.get(i) == Some(&'<') {
                        // `<<<` here-string.
                        i += 1;
                        let _ = read_word(&mut i);
                        continue;
                    }
                    if chars.get(i) == Some(&'-') {
                        i += 1;
                    }
                    let tag = read_word(&mut i);
                    if !tag.is_empty() {
                        heredocs.push(tag);
                    }
                } else {
                    let _ = read_word(&mut i);
                }
            }
            _ => {
                in_word = true;
                word.push(c);
                i += 1;
            }
        }
    }
    end_word(&mut word, &mut in_word, &mut seg);
    if !seg.words.is_empty() || !seg.redirects.is_empty() {
        segs.push(seg);
    }
    segs
}

fn is_null_device(t: &str) -> bool {
    matches!(
        t.to_ascii_lowercase().as_str(),
        "/dev/null" | "nul" | "$null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "con"
    )
}

/// A word as a path (`None`: unknown, e.g. a variable or a relative path without a cwd).
/// Globs stand for their folder.
fn resolve(cwd: Option<&Path>, home: Option<&Path>, word: &str) -> Option<PathBuf> {
    let w = word.trim();
    if w.is_empty() || w.contains(['$', '%', '`']) || w.contains("://") {
        return None;
    }
    let w = match w.find(['*', '?', '[']) {
        Some(g) => match w[..g].rfind(['/', '\\']) {
            Some(s) => &w[..s.max(1)],
            None => ".",
        },
        None => w,
    };
    let p = if w == "~" || w.starts_with("~/") || w.starts_with("~\\") {
        home?.join(w[1..].trim_start_matches(['/', '\\']))
    } else if let Some(p) = msys_drive(w) {
        p
    } else {
        let p = PathBuf::from(w);
        if p.is_absolute() {
            p
        } else {
            cwd?.join(p)
        }
    };
    Some(lexical(&p))
}

/// Git Bash's `/c/Users/...` as `C:/Users/...` (Windows only).
fn msys_drive(w: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let b = w.as_bytes();
    if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
        let rest = if b.len() > 2 { &w[3..] } else { "" };
        return Some(PathBuf::from(format!("{}:/{rest}", (b[1] as char).to_ascii_uppercase())));
    }
    None
}

/// `..` and `.` removed without touching the disk.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The program name of a word: file name, lowercase, without `.exe` / `.cmd`.
fn program(w: &str) -> String {
    let base = w.rsplit(['/', '\\']).next().unwrap_or(w).to_ascii_lowercase();
    base.trim_end_matches(".exe").trim_end_matches(".cmd").to_owned()
}

fn positionals(args: &[String]) -> Vec<&String> {
    args.iter().filter(|a| !a.starts_with('-') || a.as_str() == "-").collect()
}

/// PowerShell parameters whose value is a path.
const PS_PATH_PARAMS: [&str; 5] = ["-path", "-literalpath", "-filepath", "-destination", "-name"];

/// PowerShell: the values of path parameters, else the first positional argument (`all`: every
/// positional argument).
fn ps_paths(args: &[String], all: bool) -> Vec<String> {
    let mut named = Vec::new();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with('-') && a.len() > 1 {
            let low = a.to_ascii_lowercase();
            let (name, inline) = match low.split_once(':') {
                Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                None => (low.clone(), None),
            };
            let takes = !matches!(
                name.as_str(),
                "-force" | "-recurse" | "-whatif" | "-confirm" | "-nonewline" | "-append" | "-passthru"
            );
            if PS_PATH_PARAMS.contains(&name.as_str()) {
                if let Some(v) = inline {
                    named.push(v);
                } else if let Some(v) = args.get(i + 1) {
                    named.push(v.clone());
                    i += 1;
                }
            } else if takes && inline.is_none() {
                i += 1;
            }
        } else {
            pos.push(a.clone());
        }
        i += 1;
    }
    if !named.is_empty() {
        return named;
    }
    if all { pos } else { pos.into_iter().take(1).collect() }
}

fn segment(words: &[String], cwd: &mut Option<PathBuf>, home: Option<&Path>, out: &mut Vec<Write>, depth: u8) {
    // Leading `VAR=value` assignments and `sudo` / `command` / `env` wrappers.
    let mut w = words;
    while let Some(first) = w.first() {
        let is_assign = first.contains('=') && !first.starts_with('-') && !first.starts_with('=');
        if is_assign || matches!(program(first).as_str(), "sudo" | "command" | "env" | "exec" | "time" | "nohup") {
            w = &w[1..];
        } else {
            break;
        }
    }
    let Some(first) = w.first() else { return };
    let name = program(first);
    let args = &w[1..];
    let mut paths = |list: Vec<&str>| {
        for a in list {
            if let Some(p) = resolve(cwd.as_deref(), home, a) {
                out.push(Write::Path(p));
            }
        }
    };
    match name.as_str() {
        "cd" | "chdir" | "pushd" | "set-location" | "sl" | "push-location" => {
            let target = args.iter().find(|a| {
                let l = a.to_ascii_lowercase();
                !(l.starts_with('-') && l.len() > 1) && l != "/d"
            });
            *cwd = match target {
                None => home.map(Path::to_path_buf),
                Some(t) if t == "-" => None,
                Some(t) => resolve(cwd.as_deref(), home, t),
            };
        }
        "popd" | "pop-location" => *cwd = None,
        "git" => git(args, cwd, home, out),
        "bash" | "sh" | "zsh" | "dash" | "powershell" | "pwsh" | "cmd" if depth < 4 => {
            let flag = args.iter().position(|a| {
                let l = a.to_ascii_lowercase();
                l == "-c" || l == "-command" || l == "/c" || l == "/k" || l == "-lc"
            });
            if let Some(i) = flag {
                let inner = args[i + 1..].join(" ");
                let mut inner_cwd = cwd.clone();
                scan(&inner, &mut inner_cwd, home, out, depth + 1);
            }
        }
        "rm" | "rmdir" | "unlink" | "mkdir" | "touch" | "truncate" | "shred" | "tee" => {
            paths(positionals(args).into_iter().map(String::as_str).collect());
        }
        "mv" | "move" | "ren" | "rename" | "del" | "erase" | "rd" | "md" => {
            paths(positionals(args).into_iter().map(String::as_str).collect());
        }
        "cp" | "copy" | "xcopy" | "robocopy" | "install" | "ln" => {
            let pos = positionals(args);
            // The destination: the last argument (robocopy: the second).
            let dst = if name == "robocopy" { pos.get(1) } else { pos.last().filter(|_| pos.len() > 1) };
            if let Some(d) = dst {
                paths(vec![d.as_str()]);
            }
        }
        "sed" | "perl" if args.iter().any(|a| a.starts_with("-i") || a == "--in-place") => {
            let has_script_flag = args.iter().any(|a| a == "-e" || a == "-f" || a == "--expression");
            let pos = positionals(args);
            let files = if has_script_flag { &pos[..] } else { pos.get(1..).unwrap_or(&[]) };
            paths(files.iter().map(|s| s.as_str()).collect());
        }
        "remove-item" | "ri" | "rm-item" | "move-item" | "mi" | "rename-item" | "rni" => {
            let p = ps_paths(args, true);
            paths(p.iter().map(String::as_str).collect());
        }
        "new-item" | "ni" | "set-content" | "add-content" | "ac" | "clear-content" | "clc" | "out-file" => {
            let p = ps_paths(args, false);
            paths(p.iter().map(String::as_str).collect());
        }
        "copy-item" | "cpi" => {
            let dst = args
                .iter()
                .position(|a| a.eq_ignore_ascii_case("-destination"))
                .and_then(|i| args.get(i + 1))
                .cloned()
                .or_else(|| ps_paths(args, true).get(1).cloned());
            if let Some(d) = dst {
                paths(vec![d.as_str()]);
            }
        }
        _ => {}
    }
}

/// Git subcommands that never change a repository (beyond fetched refs and caches).
const GIT_READ: [&str; 32] = [
    "status", "log", "diff", "show", "rev-parse", "ls-files", "ls-tree", "blame", "grep", "fetch",
    "ls-remote", "describe", "shortlog", "cat-file", "for-each-ref", "merge-base", "rev-list",
    "name-rev", "check-ignore", "check-attr", "var", "help", "version", "whatchanged", "range-diff",
    "cherry", "count-objects", "fsck", "show-ref", "show-branch", "difftool", "annotate",
];

fn git(args: &[String], cwd: &mut Option<PathBuf>, home: Option<&Path>, out: &mut Vec<Write>) {
    // Global options before the subcommand.
    let mut dir = cwd.clone();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-C" => {
                if let Some(d) = args.get(i + 1) {
                    dir = resolve(dir.as_deref(), home, d);
                }
                i += 2;
            }
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => i += 2,
            _ if a.starts_with("--work-tree=") => {
                dir = resolve(dir.as_deref(), home, &a["--work-tree=".len()..]);
                i += 1;
            }
            _ if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let Some(sub) = args.get(i).map(|s| s.as_str()) else { return };
    let rest = &args[i + 1..];
    let pos = positionals(rest);
    let has = |f: &[&str]| rest.iter().any(|a| f.contains(&a.as_str()));
    let flag_value = |f: &[&str]| {
        rest.iter()
            .position(|a| f.contains(&a.as_str()))
            .and_then(|k| rest.get(k + 1))
            .cloned()
    };
    let read = match sub {
        s if GIT_READ.contains(&s) => true,
        "branch" => {
            pos.is_empty()
                || has(&["-l", "--list", "-a", "-r", "--all", "--remotes", "--contains", "--merged", "--no-merged", "--points-at", "--show-current"])
                    && !has(&["-d", "-D", "-m", "-M", "-c", "-C", "-f", "--force", "--delete", "--move", "--copy"])
        }
        "tag" => pos.is_empty() || has(&["-l", "--list", "-v", "--verify", "--contains", "--points-at"]),
        "stash" => matches!(pos.first().map(|s| s.as_str()), Some("list" | "show")),
        "worktree" => matches!(pos.first().map(|s| s.as_str()), Some("list")),
        "remote" => pos.is_empty() || matches!(pos[0].as_str(), "show" | "get-url"),
        "config" => has(&["--get", "--get-all", "--get-regexp", "-l", "--list"]) || pos.len() == 1,
        "reflog" => !matches!(pos.first().map(|s| s.as_str()), Some("expire" | "delete")),
        "notes" => matches!(pos.first().map(|s| s.as_str()), None | Some("list" | "show")),
        "submodule" => matches!(pos.first().map(|s| s.as_str()), None | Some("status" | "summary")),
        _ => false,
    };
    if read {
        return;
    }
    let mut branches: Vec<(String, bool)> = Vec::new();
    let short = |r: &str| r.trim_start_matches('+').trim_start_matches("refs/heads/").to_owned();
    match sub {
        "push" => {
            if has(&["--all", "--mirror", "--branches"]) {
                branches.push(("*".into(), true));
            }
            for r in pos.iter().skip(1) {
                let dst = r.rsplit_once(':').map_or(r.as_str(), |(_, d)| d);
                let dst = short(dst);
                if !dst.is_empty() && dst != "HEAD" && !dst.starts_with("refs/tags/") {
                    branches.push((dst, true));
                }
            }
        }
        "branch" => {
            if !has(&["-u", "--set-upstream-to", "--unset-upstream", "--edit-description"]) {
                // Delete / move / copy name branches only; create / force take a start point
                // after the name.
                let all = has(&["-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy"]);
                let n = if all { pos.len() } else { 1 };
                for b in pos.iter().take(n) {
                    branches.push((short(b), true));
                }
            }
        }
        "checkout" | "switch" => {
            if let Some(b) = flag_value(&["-b", "-B", "-c", "-C", "--orphan", "--create", "--force-create"]) {
                branches.push((b, true));
            } else if !rest.iter().any(|a| a == "--")
                && !has(&["--detach", "-d"])
                && let Some(b) = pos.first()
            {
                branches.push((short(b), sub == "switch"));
            }
        }
        "update-ref" => {
            if let Some(r) = pos.first().filter(|r| r.starts_with("refs/heads/") || !r.contains('/')) {
                branches.push((short(r), true));
            }
        }
        "worktree" => {
            if pos.first().map(|s| s.as_str()) == Some("add") {
                if let Some(p) = pos.get(1).and_then(|p| resolve(dir.as_deref(), home, p)) {
                    out.push(Write::Path(p));
                }
                if let Some(b) = flag_value(&["-b", "-B"]) {
                    branches.push((b, true));
                } else if let Some(b) = pos.get(2) {
                    branches.push((short(b), false));
                }
            }
        }
        "clone" => {
            // The new folder: the second argument, else the URL's last part.
            let target = pos.get(1).map(|s| s.to_string()).or_else(|| {
                pos.first().map(|u| {
                    u.trim_end_matches('/')
                        .rsplit(['/', ':', '\\'])
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(".git")
                        .to_owned()
                })
            });
            if let Some(p) = target.and_then(|t| resolve(dir.as_deref(), home, &t)) {
                out.push(Write::Path(p));
            }
            return;
        }
        "init" => {
            let p = match pos.first() {
                Some(d) => resolve(dir.as_deref(), home, d),
                None => dir.clone(),
            };
            if let Some(p) = p {
                out.push(Write::Path(p));
            }
            return;
        }
        _ => {}
    }
    let Some(dir) = dir else { return };
    branches.retain(|(b, _)| b != "HEAD" && !b.is_empty());
    let mut command = format!("git {sub}");
    for a in rest.iter().take(6) {
        command.push(' ');
        command.push_str(a);
    }
    out.push(Write::Git { dir, command, branches });
}

/// The repository top level around `p` (the nearest folder with a `.git` entry).
fn toplevel(p: &Path) -> Option<PathBuf> {
    let mut cur = existing(p)?;
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// The nearest existing folder at or above `p`.
fn existing(p: &Path) -> Option<PathBuf> {
    let mut cur = p.to_path_buf();
    loop {
        if cur.is_dir() {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// The folder asked for when `p` is written: its repository's top level, else the nearest
/// existing folder (`p` itself when it is one).
pub fn folder_of(p: &Path) -> PathBuf {
    toplevel(p)
        .or_else(|| existing(p))
        .unwrap_or_else(|| p.parent().map_or_else(|| p.to_path_buf(), Path::to_path_buf))
}

/// The writes that leave `scope`, one per folder (in order of appearance).
pub fn needs(scope: &Scope, writes: &[Write]) -> Vec<Need> {
    let mut out: Vec<Need> = Vec::new();
    let mut push = |folder: PathBuf, what: String| {
        if let Some(n) = out.iter_mut().find(|n| crate::repo::same_path(&n.folder, &folder)) {
            if !n.what.contains(&what) {
                n.what.push('\n');
                n.what.push_str(&what);
            }
        } else {
            out.push(Need { folder, what });
        }
    };
    let free = |p: &Path| scope.free.iter().any(|f| covers(f, p));
    for w in writes {
        match w {
            Write::Path(p) => {
                if !free(p) {
                    push(folder_of(p), p.display().to_string().replace('\\', "/"));
                }
            }
            Write::Git { dir, command, branches } => {
                let Some(top) = toplevel(dir) else { continue };
                let repo = scope.repos.iter().find(|r| crate::repo::same_path(&r.path, &top));
                match repo {
                    Some(r) => {
                        if let Some(b) = other_branch(r, branches) {
                            push(r.repo_path.clone(), format!("{command}  (branch {b})"));
                        }
                    }
                    // A repository inside the session folder that is none of its worktrees
                    // (made by the agent): free.
                    None if free(&top) => {}
                    None => push(top, command.clone()),
                }
            }
        }
    }
    out
}

/// The first branch other than the session's that a git write in `r` touches (its current
/// branch when the worktree was moved off the session's branch).
fn other_branch(r: &ScopeRepo, branches: &[(String, bool)]) -> Option<String> {
    // In the worktree itself (`Repo` always means the main checkout).
    let current = crate::repo::git(&r.path, ["branch", "--show-current"])
        .ok()
        .filter(|b| !b.is_empty());
    let own = r.branch.clone().or_else(|| current.clone())?;
    for (b, certain) in branches {
        if *b == own {
            continue;
        }
        let exists = || {
            crate::repo::git(&r.path, ["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")])
                .is_ok()
        };
        if *certain || exists() {
            return Some(b.clone());
        }
    }
    match current {
        Some(c) if c != own => Some(c),
        None if r.branch.is_some() => Some("(detached HEAD)".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn cwd() -> Option<&'static Path> {
        Some(Path::new(if cfg!(windows) { "C:/ws/s1" } else { "/ws/s1" }))
    }

    fn abs(s: &str) -> PathBuf {
        if cfg!(windows) { p(&format!("C:{s}")) } else { p(s) }
    }

    fn gits(ws: &[Write]) -> Vec<(PathBuf, Vec<(String, bool)>)> {
        ws.iter()
            .filter_map(|w| match w {
                Write::Git { dir, branches, .. } => Some((dir.clone(), branches.clone())),
                _ => None,
            })
            .collect()
    }

    fn paths(ws: &[Write]) -> Vec<PathBuf> {
        ws.iter()
            .filter_map(|w| match w {
                Write::Path(p) => Some(p.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn git_reads_are_not_writes() {
        for c in [
            "git status",
            "git log --oneline -3 main",
            "cd /other && git diff && git branch -a",
            "git -C /other fetch origin",
            "git branch",
            "git branch --show-current",
            "git worktree list",
            "git config user.name",
            "git stash list",
        ] {
            assert!(shell_writes(c, cwd(), None).is_empty(), "{c}");
        }
    }

    #[test]
    fn git_writes_follow_cd_and_dash_c() {
        let w = shell_writes("cd /repo/meninx && git merge --no-ff meninx/x -m 'm' && git push origin main", cwd(), None);
        let g = gits(&w);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0], (abs("/repo/meninx"), vec![]));
        assert_eq!(g[1], (abs("/repo/meninx"), vec![("main".into(), true)]));
        let w = shell_writes("git -C ../other commit -am x", cwd(), None);
        assert_eq!(gits(&w)[0].0, abs("/ws/other"));
        let w = shell_writes("git push origin HEAD:refs/heads/main +meninx/x", cwd(), None);
        assert_eq!(gits(&w)[0].1, vec![("main".into(), true), ("meninx/x".into(), true)]);
        let w = shell_writes("git push", cwd(), None);
        assert_eq!(gits(&w)[0].1, vec![]);
        let w = shell_writes("git checkout -b feat && git switch main && git checkout -- a.txt && git checkout dev", cwd(), None);
        let b: Vec<_> = gits(&w).into_iter().flat_map(|(_, b)| b).collect();
        assert_eq!(b, vec![("feat".into(), true), ("main".into(), true), ("dev".into(), false)]);
        let w = shell_writes("git branch -f main HEAD && git branch -D a b", cwd(), None);
        let b: Vec<_> = gits(&w).into_iter().flat_map(|(_, b)| b).collect();
        assert_eq!(b, vec![("main".into(), true), ("a".into(), true), ("b".into(), true)]);
    }

    #[test]
    fn nested_shells_quotes_and_heredocs() {
        let w = shell_writes("bash -c \"cd /xy && git commit -m 'a; b'\"", cwd(), None);
        assert_eq!(gits(&w)[0].0, abs("/xy"));
        // A heredoc body is text, not commands; the redirect target is written.
        let w = shell_writes("cat > /out/f.txt <<'EOF'\nrm -rf /\ngit push origin main\nEOF\necho done", cwd(), None);
        assert_eq!(paths(&w), vec![abs("/out/f.txt")]);
        assert!(gits(&w).is_empty());
        // Quoted separators stay in their word.
        let w = shell_writes("echo 'a && rm -rf /' > /dev/null 2>&1", cwd(), None);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn file_commands() {
        let w = shell_writes("rm -rf build /tmp/x && mkdir -p ../out && cp a.txt /dst/ && echo x >> log.txt", cwd(), None);
        assert_eq!(
            paths(&w),
            vec![abs("/ws/s1/build"), abs("/tmp/x"), abs("/ws/out"), abs("/dst"), abs("/ws/s1/log.txt")]
        );
        if cfg!(windows) {
            let w = shell_writes(
                "Remove-Item -Recurse -Force C:/a/b; Set-Content -Path x.txt -Value 'hi'; Copy-Item a -Destination D:/d",
                None,
                None,
            );
            let got: Vec<String> = paths(&w).iter().map(|p| p.display().to_string().replace('\\', "/")).collect();
            assert_eq!(got, vec!["C:/a/b", "D:/d"], "relative paths without a cwd are unknown");
        }
        let w = shell_writes("sed -i 's/a/b/' f1 f2", cwd(), None);
        assert_eq!(paths(&w), vec![abs("/ws/s1/f1"), abs("/ws/s1/f2")]);
        let w = shell_writes("rm $TMP/x *.log ~/a", cwd(), Some(&abs("/home/u")));
        assert_eq!(paths(&w), vec![abs("/ws/s1"), abs("/home/u/a")]);
    }

    #[test]
    #[cfg(windows)]
    fn msys_paths() {
        let w = shell_writes("cd /c/Users/u/Desktop/meninx && git commit -m x", None, None);
        assert_eq!(gits(&w)[0].0, p("C:/Users/u/Desktop/meninx"));
    }

    #[test]
    fn covers_folders() {
        assert!(covers(&abs("/a/b"), &abs("/a/b")));
        assert!(covers(&abs("/a/b"), &abs("/a/b/c.txt")));
        assert!(!covers(&abs("/a/b"), &abs("/a/bc")));
        assert!(!covers(Path::new(""), &abs("/a")));
    }

    #[test]
    fn needs_per_folder() {
        use crate::repo::{Repo, test_repo};
        let t = crate::paths::TempDir::new("write-guard");
        let ws = t.0.join("ws");
        let orig = t.0.join("orig");
        test_repo::init(&orig);
        let repo = Repo::open(&orig).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        let wt = ws.join("orig");
        repo.git(["worktree", "add", "-q", "-b", "meninx/s1", &wt.display().to_string()]).unwrap();
        let other = t.0.join("plain");
        std::fs::create_dir_all(&other).unwrap();
        let scope = Scope {
            free: vec![ws.clone()],
            repos: vec![ScopeRepo { path: wt.clone(), branch: Some("meninx/s1".into()), repo_path: orig.clone() }],
        };
        let run = |c: &str, cwd: &Path| needs(&scope, &shell_writes(c, Some(cwd), None));
        // Own worktree, own branch: free.
        let n = run("git commit -am x && git push origin meninx/s1 && echo a > f", &wt);
        assert!(n.is_empty(), "{n:?} {:?}", shell_writes("git commit -am x && git push origin meninx/s1 && echo a > f", Some(&wt), None));
        // Own worktree, another branch: the original repository is asked for.
        let n = run("git push origin HEAD:main", &wt);
        assert_eq!(n.len(), 1);
        assert!(same(&n[0].folder, &orig) && n[0].what.contains("branch main"), "{n:?}");
        // `git checkout main` only counts because a local main exists.
        assert_eq!(run("git checkout main", &wt).len(), 1);
        assert!(run("git checkout src/lib.rs", &wt).is_empty());
        // The original checkout itself: asked, once per folder.
        let n = run("git merge meninx/s1 && git commit -m x && echo > README.md", &orig);
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(same(&n[0].folder, &orig));
        // A plain folder outside: that folder.
        let n = run(&format!("echo hi > {}/new/a.txt", other.display().to_string().replace('\\', "/")), &ws);
        assert!(same(&n[0].folder, &other), "{n:?}");
        // A repo the agent made inside its own folder: free.
        assert!(run("git init scratch && git -C scratch commit --allow-empty -m x", &ws).is_empty());
        // Original-repo sessions: the current branch is free, others are not.
        let scope = Scope {
            free: vec![orig.clone()],
            repos: vec![ScopeRepo { path: orig.clone(), branch: None, repo_path: orig.clone() }],
        };
        let cur = repo.current_branch().unwrap().unwrap();
        let n = needs(&scope, &shell_writes(&format!("git commit -m x && git push origin {cur}"), Some(&orig), None));
        assert!(n.is_empty(), "{n:?}");
        let n = needs(&scope, &shell_writes("git push origin HEAD:release", Some(&orig), None));
        assert_eq!(n.len(), 1);
    }

    fn same(a: &Path, b: &Path) -> bool {
        crate::repo::same_path(a, b)
    }
}
