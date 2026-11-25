//! `search.text` (ripgrep's `ignore` walker + `grep-searcher`) and `search.files` (fuzzy
//! file names with `nucleo-matcher`).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks};
use meninx_proto::{
    ByteRange, FileMatch, PathMatch, PathMatches, ProtoError, SearchMatch, SearchSummary, TextSearch,
};
use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};

/// Default `max_results` of `search.text`.
pub const DEFAULT_MAX_TEXT: u32 = 2000;
/// Default `max_results` of `search.files`.
pub const DEFAULT_MAX_FILES: u32 = 100;
/// Files larger than this are not searched.
const MAX_FILE_SIZE: u64 = 8 * 1024 * 1024;
/// File-name search looks at no more files than this.
const MAX_WALK_FILES: usize = 300_000;
/// `line_text` is cut after this many bytes.
const MAX_LINE: usize = 1000;
/// A hit batch is sent at this size or after [`BATCH_TIME`].
const BATCH: usize = 100;
const BATCH_TIME: Duration = Duration::from_millis(100);

/// A validated query: the compiled matcher and glob overrides.
pub struct Prepared {
    matcher: RegexMatcher,
    globs: Vec<String>,
    include_ignored: bool,
    max: u32,
}

/// Validate and compile a query (errors: `search-empty`, `search-bad-regex` (`detail`),
/// `search-bad-glob` (`glob`, `detail`)).
pub fn prepare(root: &Path, q: &TextSearch) -> Result<Prepared, ProtoError> {
    if q.query.is_empty() {
        return Err(ProtoError::new("search-empty"));
    }
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!q.case_sensitive)
        .word(q.whole_word)
        .fixed_strings(!q.regex)
        .line_terminator(Some(b'\n'))
        .build(&q.query)
        .map_err(|e| ProtoError::new("search-bad-regex").arg("detail", e.to_string()))?;
    Ok(Prepared {
        matcher,
        globs: {
            // Validated once here; built per searched root in `run_text`.
            overrides(root, &q.globs)?;
            q.globs.clone()
        },
        include_ignored: q.include_ignored,
        max: if q.max_results == 0 {
            DEFAULT_MAX_TEXT
        } else {
            q.max_results
        },
    })
}

fn overrides(root: &Path, globs: &[String]) -> Result<Override, ProtoError> {
    let mut b = OverrideBuilder::new(root);
    for g in globs {
        let g = g.trim();
        if g.is_empty() {
            continue;
        }
        b.add(g).map_err(|e| {
            ProtoError::new("search-bad-glob")
                .arg("glob", g)
                .arg("detail", e.to_string())
        })?;
    }
    b.build().map_err(|e| {
        ProtoError::new("search-bad-glob")
            .arg("glob", globs.join(" "))
            .arg("detail", e.to_string())
    })
}

fn walker(root: &Path, include_ignored: bool, ov: Override) -> WalkBuilder {
    let mut w = WalkBuilder::new(root);
    w.hidden(false)
        .parents(true)
        .ignore(!include_ignored)
        .git_ignore(!include_ignored)
        .git_exclude(!include_ignored)
        .git_global(!include_ignored)
        .follow_links(false)
        .max_filesize(Some(MAX_FILE_SIZE))
        .overrides(ov)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| e.file_name() != ".git");
    w
}

/// A worktree's `.git` *file* (`gitdir: ...`): `ignore` 0.4.34 skips `filter_entry` for files
/// when `max_filesize` is set, so it is checked again per entry.
fn is_dot_git(e: &ignore::DirEntry) -> bool {
    e.file_name() == ".git"
}

fn rel_of(root: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(root).ok()?;
    let parts: Vec<String> = r
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// Cut `s` to at most `max` bytes at a char boundary.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Run a prepared search under every `(repo id, root)` in order (one session's worktrees);
/// `max_results` counts over all of them. Hits are delivered in batches through `emit`
/// (return false to stop: the receiver is gone). Checks `cancel` between files and lines.
pub fn run_text(
    roots: &[(String, &Path)],
    p: &Prepared,
    cancel: &AtomicBool,
    mut emit: impl FnMut(Vec<SearchMatch>) -> bool,
) -> SearchSummary {
    let mut sum = SearchSummary {
        matches: 0,
        files: 0,
        searched: 0,
        truncated: false,
        cancelled: false,
        error: None,
    };
    let mut batch: Vec<SearchMatch> = Vec::new();
    let mut last_flush = Instant::now();
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\0'))
        .build();
    'roots: for (repo, root) in roots {
    let root: &Path = root;
    let Ok(ov) = overrides(root, &p.globs) else {
        continue;
    };
    let walk = walker(root, p.include_ignored, ov).build();
    for ent in walk {
        if cancel.load(Ordering::Relaxed) {
            sum.cancelled = true;
            break 'roots;
        }
        let Ok(ent) = ent else { continue };
        if !ent.file_type().is_some_and(|t| t.is_file()) || is_dot_git(&ent) {
            continue;
        }
        let Some(rel) = rel_of(root, ent.path()) else {
            continue;
        };
        sum.searched += 1;
        let mut file_hits = 0u32;
        let mut stop = false;
        let res = searcher.search_path(
            &p.matcher,
            ent.path(),
            sinks::Bytes(|lnum, line| {
                if cancel.load(Ordering::Relaxed) || sum.matches >= p.max {
                    stop = true;
                    return Ok(false);
                }
                let line = line.strip_suffix(b"\n").unwrap_or(line);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                let mut raw = Vec::new();
                let _ = p.matcher.find_iter(line, |m| {
                    raw.push((m.start(), m.end()));
                    raw.len() < 100
                });
                let full = String::from_utf8_lossy(line);
                let text = cut(&full, MAX_LINE).to_owned();
                // Byte offsets into the lossily decoded text.
                let at =
                    |b: usize| String::from_utf8_lossy(&line[..b.min(line.len())]).len() as u32;
                let ranges: Vec<ByteRange> = raw
                    .iter()
                    .map(|&(s, e)| ByteRange {
                        start: at(s),
                        end: at(e),
                    })
                    .filter(|r| (r.end as usize) <= text.len())
                    .collect();
                let first = raw.first().map_or(0, |&(s, _)| s);
                let col = String::from_utf8_lossy(&line[..first.min(line.len())])
                    .chars()
                    .count() as u32
                    + 1;
                batch.push(SearchMatch {
                    repo: repo.clone(),
                    path: rel.clone(),
                    line: lnum as u32,
                    col,
                    line_text: text,
                    ranges,
                });
                sum.matches += 1;
                file_hits += 1;
                Ok(true)
            }),
        );
        if let Err(e) = res {
            tracing::debug!("search {}: {e}", ent.path().display());
        }
        if file_hits > 0 {
            sum.files += 1;
        }
        if !batch.is_empty() && (batch.len() >= BATCH || last_flush.elapsed() >= BATCH_TIME) {
            if !emit(std::mem::take(&mut batch)) {
                sum.cancelled = true;
                break 'roots;
            }
            last_flush = Instant::now();
        }
        if stop {
            if cancel.load(Ordering::Relaxed) {
                sum.cancelled = true;
            } else {
                sum.truncated = true;
            }
            break 'roots;
        }
        if sum.matches >= p.max {
            sum.truncated = true;
            break 'roots;
        }
    }
    }
    if !batch.is_empty() && !emit(batch) {
        sum.cancelled = true;
    }
    sum
}

/// `search.files`: fuzzy match of `query` against root-relative paths of non-ignored files
/// under every `(repo id, root)`. An empty query returns the first paths in name order.
pub fn files(roots: &[(String, &Path)], query: &str, max: u32) -> Vec<FileMatch> {
    use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
    use nucleo_matcher::{Config, Matcher, Utf32Str};
    let max = if max == 0 { DEFAULT_MAX_FILES } else { max } as usize;
    let mut paths: Vec<(String, String)> = Vec::new();
    'roots: for (repo, root) in roots {
        let root: &Path = root;
        for ent in walker(root, false, Override::empty()).build().flatten() {
            if ent.file_type().is_some_and(|t| t.is_file())
                && !is_dot_git(&ent)
                && let Some(rel) = rel_of(root, ent.path())
            {
                paths.push((repo.clone(), rel));
                if paths.len() >= MAX_WALK_FILES {
                    break 'roots;
                }
            }
        }
    }
    let query = query.trim();
    if query.is_empty() {
        paths.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        return paths
            .into_iter()
            .take(max)
            .map(|(repo, path)| FileMatch {
                repo,
                path,
                score: 0,
                indices: Vec::new(),
            })
            .collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, String, String)> = paths
        .into_iter()
        .filter_map(|(repo, p)| {
            let s = pattern.score(Utf32Str::new(&p, &mut buf), &mut matcher)?;
            Some((s, repo, p))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.len().cmp(&b.2.len()))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.truncate(max);
    scored
        .into_iter()
        .map(|(score, repo, path)| {
            let mut idx = Vec::new();
            pattern.indices(Utf32Str::new(&path, &mut buf), &mut matcher, &mut idx);
            idx.sort_unstable();
            idx.dedup();
            FileMatch {
                repo,
                path,
                score,
                indices: idx,
            }
        })
        .collect()
}

/// `search.paths` (Hermes Desk's Explore 파일 찾기): files and folders under every
/// `(repo id, root)` whose root-relative path contains `query` (case-insensitive), sorted by
/// path, at most `max`, with the total count. Gitignored entries are left out unless
/// `include_ignored`; then an ignored folder is listed once (not what is inside it), like
/// Desk's `listIgnored`.
pub fn paths(roots: &[(String, &Path)], query: &str, max: u32, include_ignored: bool) -> PathMatches {
    let max = if max == 0 { DEFAULT_MAX_FILES } else { max } as usize;
    let q = query.trim().to_lowercase();
    let mut all: Vec<PathMatch> = Vec::new();
    'roots: for (repo, root) in roots {
        let root: &Path = root;
        let mut kept: std::collections::HashSet<std::path::PathBuf> = Default::default();
        let mut dirs: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
        let mut w = WalkBuilder::new(root);
        w.hidden(false)
            .parents(true)
            .follow_links(false)
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(|e| e.file_name() != ".git");
        for ent in w.build().flatten() {
            if ent.depth() == 0 || is_dot_git(&ent) {
                continue;
            }
            let dir = ent.file_type().is_some_and(|t| t.is_dir());
            if include_ignored {
                kept.insert(ent.path().to_path_buf());
                if dir {
                    dirs.push(ent.path().to_path_buf());
                }
            }
            if let Some(rel) = rel_of(root, ent.path()) {
                all.push(PathMatch {
                    repo: repo.clone(),
                    path: rel,
                    dir,
                });
            }
            if all.len() >= MAX_WALK_FILES {
                break 'roots;
            }
        }
        if include_ignored {
            // What the walk left out of each listed folder is ignored: listed, not entered.
            for d in &dirs {
                let Ok(rd) = std::fs::read_dir(d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if e.file_name() == ".git" || kept.contains(&p) {
                        continue;
                    }
                    if let Some(rel) = rel_of(root, &p) {
                        all.push(PathMatch {
                            repo: repo.clone(),
                            path: rel,
                            dir: e.file_type().is_ok_and(|t| t.is_dir()),
                        });
                    }
                }
            }
        }
    }
    let mut hits: Vec<PathMatch> = all
        .into_iter()
        .filter(|m| q.is_empty() || m.path.to_lowercase().contains(&q))
        .collect();
    hits.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.repo.cmp(&b.repo)));
    let total = hits.len() as u32;
    hits.truncate(max);
    PathMatches {
        matches: hits,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jail::testutil::*;

    fn q(query: &str) -> TextSearch {
        TextSearch {
            session: String::new(),
            repo: None,
            query: query.into(),
            regex: false,
            case_sensitive: false,
            whole_word: false,
            globs: Vec::new(),
            include_ignored: false,
            max_results: 0,
        }
    }

    fn all(root: &Path, query: &TextSearch) -> (Vec<SearchMatch>, SearchSummary) {
        let p = prepare(root, query).unwrap();
        let mut hits = Vec::new();
        let sum = run_text(&[("r1".into(), root)], &p, &AtomicBool::new(false), |b| {
            hits.extend(b);
            true
        });
        (hits, sum)
    }

    #[test]
    fn text_search() {
        let t = tmp("search");
        let root = t.join("r");
        init_repo(
            &root,
            &[
                (".gitignore", "target/\n"),
                (
                    "src/main.rs",
                    "fn main() {\n    let 이름 = \"Hello hello\";\r\n}\n",
                ),
                ("src/lib.rs", "pub fn helloWorld() {}\n"),
                ("docs/한글.md", "안녕 hello 세계\n"),
                ("bin.dat", "hello\0binary"),
            ],
        );
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/out.txt"), "hello").unwrap();
        std::fs::write(root.join(".hidden"), "hello").unwrap();

        let (h, s) = all(&root, &q("hello"));
        let paths: Vec<_> = h.iter().map(|m| (m.path.as_str(), m.line)).collect();
        assert_eq!(
            paths,
            [
                (".hidden", 1),
                ("docs/한글.md", 1),
                ("src/lib.rs", 1),
                ("src/main.rs", 2)
            ]
        );
        assert_eq!(
            (s.matches, s.files, s.truncated, s.cancelled),
            (4, 4, false, false)
        );
        let m = h.iter().find(|m| m.path == "src/main.rs").unwrap();
        assert_eq!(m.line_text, "    let 이름 = \"Hello hello\";");
        assert_eq!(m.ranges.len(), 2);
        let r = m.ranges[0];
        assert_eq!(&m.line_text[r.start as usize..r.end as usize], "Hello");
        assert_eq!(m.col, 15);
        let k = h.iter().find(|m| m.path == "docs/한글.md").unwrap();
        assert_eq!(k.col, 4);
        assert_eq!(
            &k.line_text[k.ranges[0].start as usize..k.ranges[0].end as usize],
            "hello"
        );

        // Case-sensitive, whole word, regex, globs, ignored files.
        let mut cs = q("Hello");
        cs.case_sensitive = true;
        assert_eq!(all(&root, &cs).0.len(), 1);
        let mut ww = q("hello");
        ww.whole_word = true;
        assert!(!all(&root, &ww).0.iter().any(|m| m.path == "src/lib.rs"));
        let mut re = q(r"fn \w+\(");
        re.regex = true;
        assert_eq!(all(&root, &re).0.len(), 2);
        let mut gl = q("hello");
        gl.globs = vec!["*.rs".into()];
        assert_eq!(all(&root, &gl).0.len(), 2);
        gl.globs = vec!["!src/**".into()];
        assert!(
            all(&root, &gl)
                .0
                .iter()
                .all(|m| !m.path.starts_with("src/"))
        );
        let mut ig = q("hello");
        ig.include_ignored = true;
        assert!(all(&root, &ig).0.iter().any(|m| m.path == "target/out.txt"));
        let mut lim = q("hello");
        lim.max_results = 2;
        let (h, s) = all(&root, &lim);
        assert_eq!((h.len(), s.truncated), (2, true));
        // Errors.
        let mut bad = q("(");
        bad.regex = true;
        assert_eq!(prepare(&root, &bad).err().unwrap().id, "search-bad-regex");
        assert_eq!(prepare(&root, &q("")).err().unwrap().id, "search-empty");
        let mut bg = q("x");
        bg.globs = vec!["a{".into()];
        assert_eq!(prepare(&root, &bg).err().unwrap().id, "search-bad-glob");
        // Cancel before start.
        let p = prepare(&root, &q("hello")).unwrap();
        let s = run_text(&[("r1".into(), root.as_path())], &p, &AtomicBool::new(true), |_| true);
        assert!(s.cancelled && s.matches == 0);
        // Receiver gone.
        let s = run_text(&[("r1".into(), root.as_path())], &p, &AtomicBool::new(false), |_| false);
        assert!(s.cancelled);

        // File names.
        let f = files(&[("r1".into(), root.as_path())], "mainrs", 10);
        assert_eq!(f[0].path, "src/main.rs");
        assert!(!f[0].indices.is_empty());
        assert!(
            files(&[("r1".into(), root.as_path())], "out", 10)
                .iter()
                .all(|m| !m.path.starts_with("target"))
        );
        assert_eq!(files(&[("r1".into(), root.as_path())], "한글", 10)[0].path, "docs/한글.md");
        assert_eq!(files(&[("r1".into(), root.as_path())], "", 2).len(), 2);
        // Several repos: hits carry their repo, max_results counts over all of them.
        let other = t.join("o");
        init_repo(&other, &[("x.md", "hello\n")]);
        let p = prepare(&root, &q("hello")).unwrap();
        let mut hits = Vec::new();
        let both = [("r1".to_owned(), root.as_path()), ("r2".to_owned(), other.as_path())];
        let s = run_text(&both, &p, &AtomicBool::new(false), |b| {
            hits.extend(b);
            true
        });
        assert_eq!(s.matches, 5);
        assert_eq!(hits.last().map(|h| (h.repo.as_str(), h.path.as_str())), Some(("r2", "x.md")));
        let f = files(&both, "x.md", 10);
        assert_eq!((f[0].repo.as_str(), f[0].path.as_str()), ("r2", "x.md"));
        assert!(files(&[("r1".into(), root.as_path())], "zzzzqqq", 10).is_empty());

        // Explore 파일 찾기: substring of the path (any case), folders too, path order, total.
        let r1 = [("r1".to_owned(), root.as_path())];
        let p = super::paths(&r1, "SRC", 10, false);
        let got: Vec<_> = p.matches.iter().map(|m| (m.path.as_str(), m.dir)).collect();
        assert_eq!(got, [("src", true), ("src/lib.rs", false), ("src/main.rs", false)]);
        assert_eq!(p.total, 3);
        let p = super::paths(&r1, "src", 1, false);
        assert_eq!((p.matches.len(), p.total), (1, 3));
        // The ignored folder: left out, or listed once (not its contents).
        assert!(super::paths(&r1, "target", 10, false).matches.is_empty());
        let p = super::paths(&r1, "target", 10, true);
        let got: Vec<_> = p.matches.iter().map(|m| (m.path.as_str(), m.dir)).collect();
        assert_eq!(got, [("target", true)]);
        let g = super::paths(&r1, ".git", 10, true);
        assert!(g.matches.iter().all(|m| m.path != ".git" && !m.path.starts_with(".git/")));
        let _ = std::fs::remove_dir_all(&t);
    }
}
