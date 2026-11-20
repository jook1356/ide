//! Python language pack (port of Hermes Desk `langpacks/python/*`).

use std::sync::LazyLock;

use fancy_regex::Regex;

use super::util::*;
use super::{Ctx, ImportBinding, ImportRef};

static NAME_LIST_JUNK: LazyLock<Regex> = LazyLock::new(|| re(r"[()\\]"));
static HASH_COMMENT: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)#.*$"));
static NAME_AS: LazyLock<Regex> =
    LazyLock::new(|| re(r"^([A-Za-z_]\w*)(?:\s+as\s+([A-Za-z_]\w*))?$"));

/// `a as b, c` -> bindings.
fn name_list(list: &str) -> Vec<ImportBinding> {
    let mut out = Vec::new();
    let cleaned = NAME_LIST_JUNK.replace_all(list, " ");
    for part in cleaned.split(',') {
        let p = HASH_COMMENT.replace_all(part, "");
        let p = p.trim();
        if p == "*" {
            out.push(ImportBinding {
                local: "*".into(),
                imported: "*".into(),
                module: None,
            });
        }
        if let Ok(Some(m)) = NAME_AS.captures(p) {
            let imported = m.get(1).unwrap().as_str();
            out.push(ImportBinding {
                local: m.get(2).map_or(imported, |g| g.as_str()).to_owned(),
                imported: imported.to_owned(),
                module: None,
            });
        }
    }
    out
}

static FROM: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?m)^[ \t]*from[ \t]+(\.*[\w.]*)[ \t]+import[ \t]+(\([^)]*\)|[^\n#;]*)"));
static PLAIN: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^[ \t]*import[ \t]+([^\n#;]+)"));
static PLAIN_PART: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(\s*)([A-Za-z_][\w.]*)(?:\s+as\s+([A-Za-z_]\w*))?\s*$"));

/// `import a.b as c, d`, `from x import a as b, c`, `from . import x`, `from .m import (a, b)`.
/// Every module-level import is a re-export candidate.
pub fn parse_imports(text: &str) -> Vec<ImportRef> {
    let mut out = Vec::new();
    for m in FROM.captures_iter(text).flatten() {
        let m0 = m.get(0).unwrap();
        let spec = m.get(1).unwrap().as_str();
        let after_from = m0.as_str().find("from").map_or(0, |i| i + 4);
        let start = m0.start() + after_from + m0.as_str()[after_from..].find(spec).unwrap_or(0);
        out.push(ImportRef {
            specifier: spec.to_owned(),
            start,
            end: start + spec.len(),
            bindings: name_list(m.get(2).unwrap().as_str()),
            reexport: true,
        });
    }
    for m in PLAIN.captures_iter(text).flatten() {
        let list = m.get(1).unwrap();
        let mut cursor = list.start();
        for part in list.as_str().split(',') {
            if let Ok(Some(p)) = PLAIN_PART.captures(part) {
                let start = cursor + p.get(1).unwrap().as_str().len();
                let module = p.get(2).unwrap().as_str();
                let top = module.split('.').next().unwrap_or(module);
                let binding = match p.get(3) {
                    Some(alias) => ImportBinding {
                        local: alias.as_str().to_owned(),
                        imported: "*".into(),
                        module: None,
                    },
                    None => ImportBinding {
                        local: top.to_owned(),
                        imported: "*".into(),
                        module: (top != module).then(|| top.to_owned()),
                    },
                };
                out.push(ImportRef {
                    specifier: module.to_owned(),
                    start,
                    end: start + module.len(),
                    bindings: vec![binding],
                    reexport: true,
                });
            }
            cursor += part.len() + 1;
        }
    }
    out.sort_by_key(|r| r.start);
    out
}

fn module_candidates(dir: &str, dotted: &str) -> Vec<String> {
    let rel = dotted.split('.').collect::<Vec<_>>().join("/");
    let Some(base) = join_path(&[dir, &rel]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if !rel.is_empty() {
        out.push(format!("{base}.py"));
        out.push(format!("{base}.pyi"));
    }
    if base.is_empty() {
        out.push("__init__.py".into());
        out.push("__init__.pyi".into());
    } else {
        out.push(format!("{base}/__init__.py"));
        out.push(format!("{base}/__init__.pyi"));
    }
    out
}

/// Source roots for absolute imports: repo root, common layouts, then the file's ancestors.
fn roots_for(from: &str) -> Vec<String> {
    let mut roots: Vec<String> = ["", "src", "lib", "python", "app"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut dir = dirname(from);
    while !dir.is_empty() {
        if !roots.iter().any(|r| r == dir) {
            roots.push(dir.to_owned());
        }
        dir = dirname(dir);
    }
    roots
}

/// Resolve a Python module specifier from `from`.
pub fn resolve(spec: &str, from: &str, ctx: &dyn Ctx) -> Option<String> {
    let dots = spec.bytes().take_while(|b| *b == b'.').count();
    let rest = &spec[dots..];
    let dirs: Vec<String> = if dots > 0 {
        let mut dir: Option<String> = Some(dirname(from).to_owned());
        for _ in 1..dots {
            dir = match dir {
                Some(d) if !d.is_empty() => Some(dirname(&d).to_owned()),
                _ => None,
            };
        }
        vec![dir?]
    } else {
        if rest.is_empty() {
            return None;
        }
        roots_for(from)
    };
    for dir in &dirs {
        for c in module_candidates(dir, rest) {
            if ctx.exists(&c) {
                return Some(c);
            }
        }
    }
    None
}

/// Compiled Python declaration matcher for one name.
pub struct PyDecls(Compiled);

impl PyDecls {
    /// Compile for `name`.
    pub fn new(name: &str) -> Self {
        PyDecls(Compiled::new(
            &[
                pat(r"[ \t]*(?:async[ \t]+)?def[ \t]+", "", Kind::Top),
                pat(r"[ \t]*class[ \t]+", "", Kind::Top),
                pat("", r"[ \t]*(?::[^=]+)?=(?!=)", Kind::Top),
                pat("", r"[ \t]*:[^=]+$", Kind::Top),
            ],
            name,
        ))
    }

    /// The column of the name when `line` declares it.
    pub fn column(&self, line: &str) -> Option<(u32, Kind)> {
        if line.trim_start().starts_with('#') {
            return None;
        }
        self.0.first(line)
    }
}

/// `from pkg import mod` -> `pkg.mod`; `from . import x` -> `.x`.
pub fn member_module(spec: &str, name: &str) -> String {
    if spec.ends_with('.') {
        format!("{spec}{name}")
    } else {
        format!("{spec}.{name}")
    }
}

// ---------------------------------------------------------------------------------------
// Parameters

/// Comments and string contents (incl. triple-quoted) blanked out; offsets kept.
fn mask(text: &str) -> String {
    let t = text.as_bytes();
    let n = t.len();
    let mut out = t.to_vec();
    let mut i = 0;
    while i < n {
        let c = t[i];
        if c == b'#' {
            let end = text[i..].find('\n').map_or(n, |e| i + e);
            blank_range(&mut out, i, end);
            i = end;
        } else if c == b'"' || c == b'\'' {
            let triple = t.get(i..i + 3) == Some(&[c, c, c][..]);
            let ql = if triple { 3 } else { 1 };
            let mut j = i + ql;
            let at_quote = |j: usize| {
                if triple {
                    t.get(j..j + 3) == Some(&[c, c, c][..])
                } else {
                    t.get(j) == Some(&c)
                }
            };
            while j < n && !at_quote(j) && (triple || t[j] != b'\n') {
                j += if t[j] == b'\\' { 2 } else { 1 };
            }
            blank_range(&mut out, i + ql, j.min(n));
            i = j + ql;
        } else {
            i += 1;
        }
    }
    bytes_to_string(out)
}

fn indent_of(s: &[u8], line_start: usize) -> usize {
    s[line_start.min(s.len())..]
        .iter()
        .take_while(|b| **b == b' ' || **b == b'\t')
        .count()
}

/// The end of a `def` body whose header ends with the `:` at `colon`.
fn def_body_end(src: &str, def_line: usize, colon: usize) -> usize {
    let s = src.as_bytes();
    let Some(eol) = src
        .get(colon..)
        .and_then(|r| r.find('\n'))
        .map(|e| colon + e)
    else {
        return s.len();
    };
    if !src[(colon + 1).min(eol)..eol].trim().is_empty() {
        return eol;
    }
    let indent = indent_of(s, def_line);
    let mut end = eol;
    let mut st = eol + 1;
    while st < s.len() {
        let line_end = src[st..].find('\n').map_or(s.len(), |e| st + e);
        if !src[st..line_end].trim().is_empty() {
            if indent_of(s, st) <= indent {
                break;
            }
            end = line_end;
        }
        st = line_end + 1;
    }
    end
}

static LEAD: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s*]*"));

fn param_offset(
    src: &str,
    m: &[Option<usize>],
    from: usize,
    to: usize,
    name: &str,
) -> Option<usize> {
    let s = src.as_bytes();
    let r = re(&format!(r"^{}(?![\w])", esc(name)));
    let mut start = from;
    let mut i = from;
    while i <= to {
        let c = s.get(i).copied().unwrap_or(0);
        if matches!(c, b'(' | b'{' | b'[')
            && let Some(p) = m[i].filter(|&p| p > i && p < to)
        {
            i = p + 1;
            continue;
        }
        if c != b',' && i != to {
            i += 1;
            continue;
        }
        let end = i.min(s.len());
        let seg = &src[start.min(end)..end];
        let lead = LEAD.find(seg).ok().flatten().map_or(0, |x| x.end());
        let st = start + lead;
        if st <= end && r.is_match(&src[st..end]).unwrap_or(false) {
            return Some(st);
        }
        start = i + 1;
        i += 1;
    }
    None
}

static DEF: LazyLock<Regex> =
    LazyLock::new(|| re(r"(^|[^\w])def\s+[A-Za-z_]\w*\s*(?:\[[^\]]*\]\s*)?\("));
static LAMBDA: LazyLock<Regex> = LazyLock::new(|| re(r"(^|[^\w])lambda(?![\w])"));

/// The parameter `name` of the innermost `def` / `lambda` around `offset`.
pub fn parameter_at(text: &str, offset: usize, name: &str) -> Option<Decl> {
    if !text.contains(name) {
        return None;
    }
    let src = mask(text);
    let s = src.as_bytes();
    let (m, depth) = partners(s);
    let mut best: Option<usize> = None;
    let mut best_start: Option<usize> = None;
    let mut consider = |start: usize, end: usize, at: Option<usize>| {
        if let Some(at) = at
            && start <= offset
            && offset <= end
            && best_start.is_none_or(|b| start > b)
        {
            best_start = Some(start);
            best = Some(at);
        }
    };
    for c in DEF.captures_iter(&src).flatten() {
        let m0 = c.get(0).unwrap();
        if m0.start() > offset {
            break;
        }
        let open = m0.end() - 1;
        let Some(close) = m[open] else { continue };
        let mut colon = close + 1;
        while colon < s.len() && !(s[colon] == b':' && depth[colon] == depth[open]) {
            colon += 1;
        }
        let line_start = s[..=m0.start()]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        consider(
            open,
            def_body_end(&src, line_start, colon),
            param_offset(&src, &m, open + 1, close, name),
        );
    }
    for c in LAMBDA.captures_iter(&src).flatten() {
        let m0 = c.get(0).unwrap();
        if m0.start() > offset {
            break;
        }
        let from = m0.end();
        if from >= s.len() {
            continue;
        }
        let mut colon = from;
        while colon < s.len() && !(s[colon] == b':' && depth[colon] == depth[from]) {
            colon += 1;
        }
        let mut end = colon + 1;
        while end < s.len() {
            let ch = s[end];
            if matches!(ch, b'(' | b'{' | b'[')
                && let Some(p) = m[end].filter(|&p| p > end)
            {
                end = p;
            } else if matches!(ch, b')' | b'}' | b']' | b',') || (ch == b'\n' && depth[end] == 0) {
                break;
            }
            end += 1;
        }
        consider(from, end, param_offset(&src, &m, from, colon, name));
    }
    best.map(|b| decl_at(text, b))
}

#[cfg(test)]
mod tests {
    use super::super::{MemCtx, Pack, find_declarations};
    use super::*;

    #[test]
    fn py_imports() {
        let src = [
            "import os",
            "import a.b as c, d",
            "import pkg.sub",
            "from x import a as b, c",
            "from . import sibling",
            "from .mod import y",
            "from ..up import (",
            "    one,",
            "    two as deux,",
            ")",
            "from star import *",
        ]
        .join("\n");
        let refs = parse_imports(&src);
        let got: Vec<(String, String)> = refs
            .iter()
            .map(|r| {
                (
                    r.specifier.clone(),
                    r.bindings
                        .iter()
                        .map(|b| format!("{}>{}", b.imported, b.local))
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })
            .collect();
        let want: Vec<(String, String)> = [
            ("os", "*>os"),
            ("a.b", "*>c"),
            ("d", "*>d"),
            ("pkg.sub", "*>pkg"),
            ("x", "a>b,c>c"),
            (".", "sibling>sibling"),
            (".mod", "y>y"),
            ("..up", "one>one,two>deux"),
            ("star", "*>*"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(got, want);
        assert_eq!(refs[3].bindings[0].module.as_deref(), Some("pkg"));
        for r in &refs {
            assert_eq!(&src[r.start..r.end], r.specifier);
        }
    }

    #[test]
    fn py_resolution() {
        let ctx = MemCtx::new(&[
            ("pkg/__init__.py", ""),
            ("pkg/mod.py", ""),
            ("pkg/sub/__init__.py", ""),
            ("pkg/sub/deep.py", ""),
            ("src/app/core.py", ""),
            ("src/app/__init__.py", ""),
        ]);
        let p = Pack::Py;
        let r = |s: &str, f: &str| p.resolve(s, f, &ctx);
        // TS normalizes `pkg/sub/../x.py` first; our callers pass normalized paths.
        assert_eq!(r(".mod", "pkg/x.py").as_deref(), Some("pkg/mod.py"));
        assert_eq!(r(".", "pkg/mod.py").as_deref(), Some("pkg/__init__.py"));
        assert_eq!(r("..mod", "pkg/sub/deep.py").as_deref(), Some("pkg/mod.py"));
        assert_eq!(
            r(".deep", "pkg/sub/__init__.py").as_deref(),
            Some("pkg/sub/deep.py")
        );
        assert_eq!(
            r("pkg.sub.deep", "tools/run.py").as_deref(),
            Some("pkg/sub/deep.py")
        );
        assert_eq!(r("pkg", "tools/run.py").as_deref(), Some("pkg/__init__.py"));
        assert_eq!(
            r("app.core", "tests/t.py").as_deref(),
            Some("src/app/core.py")
        );
        assert_eq!(r("numpy", "tests/t.py"), None);
        assert_eq!(member_module(".", "x"), ".x");
        assert_eq!(member_module("pkg", "x"), "pkg.x");
    }

    #[test]
    fn py_declarations() {
        let src = [
            "def top(a):",
            "    inner = 1",
            "async def coro():",
            "class Model(Base):",
            "    def method(self):",
            "CONSTANT = 3",
            "typed: int = 4",
            "compare == 1",
            "# def commented():",
        ]
        .join("\n");
        let lines = |n: &str| -> Vec<u32> {
            find_declarations(Pack::Py, &src, n)
                .iter()
                .map(|d| d.line)
                .collect()
        };
        assert_eq!(lines("top"), [1]);
        assert!(lines("inner").is_empty());
        assert_eq!(lines("coro"), [3]);
        assert_eq!(lines("Model"), [4]);
        assert_eq!(lines("method"), [5]);
        assert_eq!(lines("CONSTANT"), [6]);
        assert_eq!(lines("typed"), [7]);
        assert!(lines("compare").is_empty());
        assert!(lines("commented").is_empty());
    }
}
