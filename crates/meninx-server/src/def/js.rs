//! JavaScript language pack (port of Hermes Desk `langpacks/javascript/*`): declarations,
//! imports, parameters and module resolution. TypeScript and Vue build on it.

use std::collections::HashSet;
use std::sync::LazyLock;

use fancy_regex::Regex;

use super::util::*;
use super::{Ctx, DefaultExport, ImportBinding, ImportRef};

// ---------------------------------------------------------------------------------------
// Declarations

const CONTROL: &[&str] = &[
    "if", "for", "while", "switch", "catch", "function", "return", "with", "do", "else", "try",
    "new", "typeof", "await", "yield", "super", "import", "require",
];

const EXPORT: &str = r"(?:export\s+)?(?:default\s+)?(?:declare\s+)?";
const MODIFIERS: &str =
    r"(?:(?:static|async|get|set|public|private|protected|readonly|override|abstract|declare)\s+)*";
/// `(args)` without nested parens.
const ARGS: &str = r"\s*(?:<[^>()]*>)?\s*\([^()]*\)";
const RETURN_TYPE: &str = r"(?:\s*:\s*[^{;=]+?)?";

static JS_PATTERNS: LazyLock<Vec<(String, String, Kind)>> = LazyLock::new(|| {
    vec![
        (
            format!(r"\s*{EXPORT}(?:async\s+)?function\s*\*?\s*"),
            String::new(),
            Kind::Top,
        ),
        (
            format!(r"\s*{EXPORT}(?:abstract\s+)?class\s+"),
            String::new(),
            Kind::Top,
        ),
        (
            r"\s*(?:export\s+)?(?:declare\s+)?(?:const|let|var)\s+".into(),
            String::new(),
            Kind::Top,
        ),
        (
            r"\s*(?:module\.)?exports\.".into(),
            r"\s*=(?!=)".into(),
            Kind::Top,
        ),
        (
            format!(r"\s*{MODIFIERS}"),
            format!(
                r"\s*(?::|=)\s*(?:async\s+)?(?:function\b|\([^()]*\){RETURN_TYPE}\s*=>|[\w$]+\s*=>)"
            ),
            Kind::Member,
        ),
    ]
});

/// Compiled JS declaration matcher for one name.
pub struct JsDecls {
    patterns: Vec<(Regex, Kind)>,
    destructure: Regex,
    method: Option<Regex>,
}

static DESTRUCT_HEAD: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(\s*(?:export\s+)?(?:const|let|var)\s*)[{\[]"));
static DESTRUCT_TAIL: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(?::[^=]*)?="));
static DEFAULTS: LazyLock<Regex> = LazyLock::new(|| re(r"=[^,}\]]*"));

impl JsDecls {
    /// Compile for `name`.
    pub fn new(name: &str) -> Self {
        let n = esc(name);
        JsDecls {
            patterns: JS_PATTERNS
                .iter()
                .map(|(p, s, k)| (re(&format!(r"^({p}){n}(?![\w$]){s}")), *k))
                .collect(),
            destructure: re(&format!(r"(^|[,{{\s:\[]){n}(?![\w$])(?!\s*:)")),
            method: (!CONTROL.contains(&name)).then(|| {
                re(&format!(
                    r"^(\s*{MODIFIERS}\*?\s*){n}{ARGS}{RETURN_TYPE}\s*\{{\s*(?:\}}\s*)?(?://.*)?$"
                ))
            }),
        }
    }

    /// The column of the name when `line` declares it.
    pub fn column(&self, line: &str) -> Option<(u32, Kind)> {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            return None;
        }
        for (r, k) in &self.patterns {
            if let Ok(Some(c)) = r.captures(line) {
                return Some((char_count(line, 0, c.get(1).unwrap().end()) + 1, *k));
            }
        }
        if let Some(b) = self.destructured(line) {
            return Some((char_count(line, 0, b) + 1, Kind::Top));
        }
        let m = self.method.as_ref()?.captures(line).ok()??;
        Some((
            char_count(line, 0, m.get(1).unwrap().end()) + 1,
            Kind::Member,
        ))
    }

    /// `const { a, b: c, d = 1 } = ...` / `const [a, b] = ...`: byte offset of the bound name.
    fn destructured(&self, line: &str) -> Option<usize> {
        let m = DESTRUCT_HEAD.captures(line).ok()??;
        let open = m.get(1).unwrap().end();
        let b = line.as_bytes();
        let mut depth = 0i32;
        let mut close = None;
        for (i, &c) in b.iter().enumerate().skip(open) {
            if c == b'{' || c == b'[' {
                depth += 1;
            } else if c == b'}' || c == b']' {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
        }
        let close = close?;
        if !DESTRUCT_TAIL.is_match(&line[close + 1..]).unwrap_or(false) {
            return None;
        }
        let inner = blank_matches(&DEFAULTS, &line[open + 1..close]);
        let hit = self.destructure.captures(&inner).ok()??;
        Some(open + 1 + hit.get(0).unwrap().start() + hit.get(1).unwrap().as_str().len())
    }
}

static DEF_FN: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^\s*export\s+default\s+(?:async\s+)?(?:function\s*\*?|(?:abstract\s+)?class)\s*([\w$]*)")
});
static DEF_NAME: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:export\s+default|module\.exports\s*=(?!=))\s*([\w$]+)\s*;?\s*$"));
static DEF_ANY: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:export\s+default\b|module\.exports\s*=(?!=))"));
static DEF_BRACE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*export\s*\{[^}]*?\b([\w$]+)\s+as\s+default\b"));

/// `export default ...` / `module.exports = ...` / `export { x as default }`.
pub fn default_export(text: &str) -> Option<DefaultExport> {
    for (i, (_, line)) in lines(text).enumerate() {
        let ln = i as u32 + 1;
        if let Ok(Some(m)) = DEF_FN.captures(line) {
            let n = m.get(1).map_or("", |g| g.as_str());
            let name = if n.is_empty() { "default" } else { n };
            return Some(DefaultExport {
                decl: declaration_on_line(text, ln, Some(name)),
                alias: None,
            });
        }
        if let Ok(Some(m)) = DEF_NAME.captures(line) {
            let n = m.get(1).unwrap().as_str();
            return Some(DefaultExport {
                decl: declaration_on_line(text, ln, Some(n)),
                alias: Some(n.to_owned()),
            });
        }
        if DEF_ANY.is_match(line).unwrap_or(false) {
            let name = if line.contains("module") {
                "module"
            } else {
                "default"
            };
            return Some(DefaultExport {
                decl: declaration_on_line(text, ln, Some(name)),
                alias: None,
            });
        }
        if let Ok(Some(m)) = DEF_BRACE.captures(line) {
            let n = m.get(1).unwrap().as_str();
            return Some(DefaultExport {
                decl: declaration_on_line(text, ln, Some(n)),
                alias: Some(n.to_owned()),
            });
        }
    }
    None
}

static EXPORT_BRACES: LazyLock<Regex> = LazyLock::new(|| re(r"\bexport\s*\{([^}]*)\}(?!\s*from)"));
static EXPORT_AS: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:type\s+)?([\w$]+)\s+as\s+([\w$]+)\s*$"));

/// `export { local as name }` (no `from`): the local behind an exported name.
pub fn export_alias(text: &str, name: &str) -> Option<String> {
    for m in EXPORT_BRACES.captures_iter(text).flatten() {
        for part in m.get(1).unwrap().as_str().split(',') {
            if let Ok(Some(a)) = EXPORT_AS.captures(part)
                && a.get(2).unwrap().as_str() == name
            {
                return Some(a.get(1).unwrap().as_str().to_owned());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------
// Imports

static COMMENTS: LazyLock<Regex> = LazyLock::new(|| re(r"//[^\n]*|/\*[\s\S]*?\*/"));
static TYPE_PREFIX: LazyLock<Regex> = LazyLock::new(|| re(r"^type\s+"));
static DEFAULT_VALUE: LazyLock<Regex> = LazyLock::new(|| re(r"\s*=[\s\S]*$"));
static AS_BINDING: LazyLock<Regex> = LazyLock::new(|| re(r"^([\w$]+)(?:\s+as\s+([\w$]+))?$"));
static COLON_BINDING: LazyLock<Regex> = LazyLock::new(|| re(r"^([\w$]+)(?:\s*:\s*([\w$]+))?$"));

fn binding(local: &str, imported: &str) -> ImportBinding {
    ImportBinding {
        local: local.to_owned(),
        imported: imported.to_owned(),
        module: None,
    }
}

/// `{ a, b as c, type d }` (import/export braces) or `{ a, b: c }` (destructuring).
fn brace_bindings(inner: &str, colon: bool) -> Vec<ImportBinding> {
    let mut out = Vec::new();
    for part in inner.split(',') {
        let p = COMMENTS.replace_all(part, "");
        let p = p.trim();
        let p = TYPE_PREFIX.replace(p, "");
        let mut p = p.into_owned();
        if p.is_empty() || p.starts_with("...") {
            continue;
        }
        if colon {
            p = DEFAULT_VALUE.replace(&p, "").into_owned();
        }
        let r = if colon { &COLON_BINDING } else { &AS_BINDING };
        if let Ok(Some(m)) = r.captures(&p) {
            let imported = m.get(1).unwrap().as_str();
            let local = m.get(2).map_or(imported, |g| g.as_str());
            out.push(binding(local, imported));
        }
    }
    out
}

static IDENT_ONLY: LazyLock<Regex> = LazyLock::new(|| re(r"^([\w$]+)$"));

/// `const { a, b: c } = ` / `const x = ` target of a require / dynamic import.
fn assigned_bindings(target: &str, member: Option<&str>) -> Vec<ImportBinding> {
    let t = target.trim();
    if let Some(rest) = t.strip_prefix('{') {
        let inner = rest.rfind('}').map_or(rest, |c| &rest[..c]);
        if member.is_some() {
            return Vec::new();
        }
        return brace_bindings(inner, true);
    }
    match IDENT_ONLY.captures(t) {
        Ok(Some(m)) => vec![binding(m.get(1).unwrap().as_str(), member.unwrap_or("*"))],
        _ => Vec::new(),
    }
}

static CLAUSE_DEFAULT: LazyLock<Regex> = LazyLock::new(|| re(r"^([\w$]+)\s*(,|$)"));
static CLAUSE_NS: LazyLock<Regex> = LazyLock::new(|| re(r"^\*\s*as\s+([\w$]+)"));
static CLAUSE_BRACE: LazyLock<Regex> = LazyLock::new(|| re(r"\{([^}]*)\}"));

/// `def, { a as b }` / `* as ns` / `def, * as ns` / `{ a }`.
fn clause_bindings(clause: &str) -> Vec<ImportBinding> {
    let mut out = Vec::new();
    let c = clause.trim();
    let mut rest = TYPE_PREFIX.replace(c, "").into_owned();
    if let Ok(Some(d)) = CLAUSE_DEFAULT.captures(&rest) {
        out.push(binding(d.get(1).unwrap().as_str(), "default"));
        rest = rest[d.get(0).unwrap().end()..].trim().to_owned();
    }
    if let Ok(Some(ns)) = CLAUSE_NS.captures(&rest) {
        out.push(binding(ns.get(1).unwrap().as_str(), "*"));
    }
    if let Ok(Some(b)) = CLAUSE_BRACE.captures(&rest) {
        out.extend(brace_bindings(b.get(1).unwrap().as_str(), false));
    }
    out
}

static STATIC_IMPORT: LazyLock<Regex> =
    LazyLock::new(|| re(r#"\bimport\s+((?:type\s+)?[\w$*{}\s,]+?)\s*from\s*(['"])([^'"\n]+)\2"#));
static SIDE_EFFECT: LazyLock<Regex> = LazyLock::new(|| re(r#"\bimport\s*(['"])([^'"\n]+)\1"#));
static REEXPORT: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"\bexport\s+(?:type\s+)?(\*(?:\s*as\s+[\w$]+)?|\{[^}]*\})\s*from\s*(['"])([^'"\n]+)\2"#)
});
static REEXPORT_NS: LazyLock<Regex> = LazyLock::new(|| re(r"as\s+([\w$]+)"));
static CALL: LazyLock<Regex> =
    LazyLock::new(|| re(r#"\b(require|import)\s*\(\s*(['"])([^'"\n]+)\2\s*\)"#));
static MEMBER_AFTER: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\.\s*([\w$]+)"));
static ASSIGN_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?:\b(?:const|let|var)|\bimport)\s+([\w$]+|\{[^}]*\})\s*=\s*(?:await\s+)?$")
});

/// Every import-like statement of a JS/TS file: `import ... from`, side-effect `import 'x'`,
/// `export ... from`, dynamic `import('x')`, `require('x')` and `import x = require('x')`.
pub fn parse_imports(text: &str) -> Vec<ImportRef> {
    let mut out: Vec<ImportRef> = Vec::new();
    let mut seen = HashSet::new();
    let mut add =
        |m0: fancy_regex::Match<'_>, spec: &str, bindings: Vec<ImportBinding>, re: bool| {
            let idx = m0.as_str().rfind(spec).unwrap_or(0);
            let start = m0.start() + idx;
            if !seen.insert(start) {
                return;
            }
            out.push(ImportRef {
                specifier: spec.to_owned(),
                start,
                end: start + spec.len(),
                bindings,
                reexport: re,
            });
        };
    for m in STATIC_IMPORT.captures_iter(text).flatten() {
        let spec = m.get(3).unwrap().as_str();
        add(
            m.get(0).unwrap(),
            spec,
            clause_bindings(m.get(1).unwrap().as_str()),
            false,
        );
    }
    for m in SIDE_EFFECT.captures_iter(text).flatten() {
        add(
            m.get(0).unwrap(),
            m.get(2).unwrap().as_str(),
            Vec::new(),
            false,
        );
    }
    for m in REEXPORT.captures_iter(text).flatten() {
        let what = m.get(1).unwrap().as_str();
        let bindings = if what.starts_with('{') {
            brace_bindings(&what[1..what.len() - 1], false)
        } else {
            match REEXPORT_NS.captures(what) {
                Ok(Some(ns)) => vec![binding(ns.get(1).unwrap().as_str(), "*")],
                _ => vec![binding("*", "*")],
            }
        };
        add(
            m.get(0).unwrap(),
            m.get(3).unwrap().as_str(),
            bindings,
            true,
        );
    }
    for m in CALL.captures_iter(text).flatten() {
        let m0 = m.get(0).unwrap();
        let before = &text[floor_boundary(text, m0.start().saturating_sub(2000))..m0.start()];
        let after = &text[m0.end()..ceil_boundary(text, m0.end() + 80)];
        let member = MEMBER_AFTER
            .captures(after)
            .ok()
            .flatten()
            .map(|c| c.get(1).unwrap().as_str().to_owned());
        let bindings = match ASSIGN_BEFORE.captures(before) {
            Ok(Some(a)) => assigned_bindings(a.get(1).unwrap().as_str(), member.as_deref()),
            _ => Vec::new(),
        };
        add(m0, m.get(3).unwrap().as_str(), bindings, false);
    }
    out.sort_by_key(|r| r.start);
    out
}

// ---------------------------------------------------------------------------------------
// Parameters

const NOT_FUNCTION: [&str; 18] = [
    "if",
    "while",
    "switch",
    "with",
    "return",
    "typeof",
    "await",
    "yield",
    "new",
    "case",
    "do",
    "else",
    "in",
    "of",
    "instanceof",
    "void",
    "delete",
    "throw",
];

static MODIFIER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^\s*(?:(?:\.\.\.|(?:public|private|protected|readonly|override|const|let|var)\s+)\s*)*")
});
static BEFORE_PAREN: LazyLock<Regex> = LazyLock::new(|| re(r"([\w$]+)\s*(?:<[^<>()]*>)?\s*$"));
static KEY_COLON: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*:"));

/// Comments and string / template contents blanked out; offsets and newlines kept.
pub fn mask(text: &str) -> String {
    let t = text.as_bytes();
    let mut out = t.to_vec();
    let n = t.len();
    let mut i = 0;
    while i < n {
        let c = t[i];
        if c == b'/' && matches!(t.get(i + 1), Some(b'/' | b'*')) {
            let line = t[i + 1] == b'/';
            let end = if line {
                text[i + 2..].find('\n').map_or(n, |e| i + 2 + e)
            } else {
                text[i + 2..].find("*/").map_or(n, |e| i + 2 + e + 2)
            };
            blank_range(&mut out, i, end);
            i = end;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let mut j = i + 1;
            while j < n && t[j] != c && (c == b'`' || t[j] != b'\n') {
                j += if t[j] == b'\\' { 2 } else { 1 };
            }
            blank_range(&mut out, i + 1, j.min(n));
            i = j + 1;
        } else {
            i += 1;
        }
    }
    bytes_to_string(out)
}

fn is_ws(b: u8) -> bool {
    b.is_ascii_whitespace()
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_ws(s[i]) {
        i += 1;
    }
    i
}

type Partners = Vec<Option<usize>>;

fn partner_after(m: &Partners, i: usize) -> Option<usize> {
    m.get(i).copied().flatten().filter(|&p| p > i)
}

/// End of the body starting at `i`: a `{}` block, else the expression up to `;`/`,`/a close.
fn body_end(s: &[u8], m: &Partners, i: usize) -> usize {
    let mut i = skip_ws(s, i);
    if s.get(i) == Some(&b'{') {
        return m[i].unwrap_or(s.len());
    }
    while i < s.len() {
        let c = s[i];
        if matches!(c, b'(' | b'{' | b'[')
            && let Some(p) = partner_after(m, i)
        {
            i = p;
        } else if matches!(c, b')' | b'}' | b']' | b';' | b',') {
            return i;
        }
        i += 1;
    }
    s.len()
}

/// After a parameter list's `)`: the offset of `=>` / `{` (skipping a return type).
fn after_params(s: &[u8], m: &Partners, close: usize) -> Option<usize> {
    let mut p = skip_ws(s, close + 1);
    if s.get(p) == Some(&b':') {
        p = skip_ws(s, p + 1);
        if s.get(p) == Some(&b'{')
            && let Some(q) = partner_after(m, p)
        {
            p = q + 1;
        }
        let limit = p + 300;
        while p < s.len() && p < limit {
            let c = s[p];
            if c == b'{' || (c == b'=' && s.get(p + 1) == Some(&b'>')) {
                break;
            }
            if matches!(c, b'(' | b'[')
                && let Some(q) = partner_after(m, p)
            {
                p = q;
            } else if matches!(c, b';' | b',' | b')' | b']' | b'}' | b'=') {
                return None;
            }
            p += 1;
        }
    }
    let ok = s.get(p) == Some(&b'{') || (s.get(p) == Some(&b'=') && s.get(p + 1) == Some(&b'>'));
    ok.then_some(p)
}

/// The offset where `name` is bound in the parameter list between `open` and `close`.
fn param_offset(src: &str, m: &Partners, open: usize, close: usize, name: &str) -> Option<usize> {
    let s = src.as_bytes();
    let word = re(&format!(r"(^|[^\w$]){}(?![\w$])", esc(name)));
    let plain = re(&format!(r"^{}(?![\w$])", esc(name)));
    let mut start = open + 1;
    let mut i = open + 1;
    while i <= close {
        let c = s[i];
        if matches!(c, b'(' | b'{' | b'[')
            && let Some(p) = partner_after(m, i)
            && p < close
        {
            i = p + 1;
            continue;
        }
        if c != b',' && c != b';' && i != close {
            i += 1;
            continue;
        }
        let seg = &src[start..i];
        let lead = MODIFIER.find(seg).ok().flatten().map_or(0, |x| x.end());
        let st = start + lead;
        if matches!(s.get(st), Some(b'{' | b'[')) {
            let end = partner_after(m, st).unwrap_or(i);
            let pattern = blank_matches(&DEFAULTS, &src[st..end]);
            for c in word.captures_iter(&pattern).flatten() {
                let at = c.get(0).unwrap().start() + c.get(1).unwrap().as_str().len();
                let rest = &pattern[(at + name.len()).min(pattern.len())..];
                if !KEY_COLON.is_match(rest).unwrap_or(false) {
                    return Some(st + at);
                }
            }
        } else if st <= i && plain.is_match(&src[st..i]).unwrap_or(false) {
            return Some(st);
        }
        start = i + 1;
        i += 1;
    }
    None
}

/// The parameter (or `catch` / `for` binding, or `x =>` arrow parameter) named `name` of the
/// innermost function around `offset`. Text-level: comments and strings are ignored.
pub fn parameter_at(text: &str, offset: usize, name: &str) -> Option<Decl> {
    if !text.contains(name) {
        return None;
    }
    let src = mask(text);
    let s = src.as_bytes();
    let (m, _) = partners(s);
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
    let mut open_pos = 0;
    while let Some(rel) = src[open_pos..].find('(') {
        let open = open_pos + rel;
        open_pos = open + 1;
        if open > offset {
            break;
        }
        let Some(close) = m[open] else { continue };
        let Some(hit) = src[open..].find(name).map(|h| open + h) else {
            continue;
        };
        if hit > close {
            continue;
        }
        let window = &src[floor_boundary(&src, open.saturating_sub(200))..open];
        let kw = BEFORE_PAREN
            .captures(window)
            .ok()
            .flatten()
            .map(|c| c.get(1).unwrap().as_str().to_owned());
        let body = if matches!(kw.as_deref(), Some("for" | "catch")) {
            close + 1
        } else {
            let Some(p) = after_params(s, &m, close) else {
                continue;
            };
            if s[p] == b'=' {
                p + 2
            } else if kw.as_deref().is_some_and(|k| !NOT_FUNCTION.contains(&k)) {
                p
            } else {
                continue;
            }
        };
        consider(
            open,
            body_end(s, &m, body),
            param_offset(&src, &m, open, close, name),
        );
    }
    let arrow = re(&format!(r"(^|[^\w$.]){}\s*=>", esc(name)));
    for c in arrow.captures_iter(&src).flatten() {
        let m0 = c.get(0).unwrap();
        if m0.start() > offset {
            break;
        }
        let at = m0.start() + c.get(1).unwrap().as_str().len();
        consider(at, body_end(s, &m, m0.end()), Some(at));
    }
    best.map(|b| decl_at(text, b))
}

// ---------------------------------------------------------------------------------------
// Resolution

const TS_EXTENSIONS: [&str; 5] = ["ts", "tsx", "d.ts", "mts", "cts"];
const JS_EXTENSIONS: [&str; 4] = ["js", "jsx", "mjs", "cjs"];

/// Extensions tried for an extension-less specifier, TS first.
pub fn source_extensions() -> Vec<&'static str> {
    TS_EXTENSIONS
        .iter()
        .chain(JS_EXTENSIONS.iter())
        .chain(["vue"].iter())
        .copied()
        .collect()
}

fn js_first() -> Vec<&'static str> {
    JS_EXTENSIONS
        .iter()
        .chain(TS_EXTENSIONS.iter())
        .chain(["vue"].iter())
        .copied()
        .collect()
}

/// `.`, `..`, `./x`, `../x`.
pub fn is_relative(spec: &str) -> bool {
    spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../")
}

static SRC_EXT: LazyLock<Regex> = LazyLock::new(|| re(r"\.([cm]?[jt]sx?)$"));

/// File candidates for a repo-relative base path: as-is, + extension, /index + extension.
pub fn file_candidates(base: &str, exts: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(Some(ext)) = SRC_EXT.captures(base) {
        out.push(base.to_owned());
        let stem = &base[..base.len() - ext.get(0).unwrap().as_str().len()];
        let twins: &[&str] = match ext.get(1).unwrap().as_str() {
            "js" => &["ts", "tsx", "d.ts"],
            "jsx" => &["tsx"],
            "mjs" => &["mts"],
            "cjs" => &["cts"],
            _ => &[],
        };
        for t in twins {
            out.push(format!("{stem}.{t}"));
        }
    }
    if !base.is_empty() && !base.ends_with(".json") {
        for e in exts {
            out.push(format!("{base}.{e}"));
        }
    }
    for e in exts {
        out.push(if base.is_empty() {
            format!("index.{e}")
        } else {
            format!("{base}/index.{e}")
        });
    }
    if base.ends_with(".json") || base.ends_with(".vue") {
        out.insert(0, base.to_owned());
    }
    out
}

/// The first candidate that exists.
pub fn first_existing(candidates: &[String], ctx: &dyn Ctx) -> Option<String> {
    candidates.iter().find(|c| ctx.exists(c)).cloned()
}

/// Relative specifiers only; bare packages are not looked into.
pub fn resolve(spec: &str, from: &str, ctx: &dyn Ctx, js_first_order: bool) -> Option<String> {
    if !is_relative(spec) {
        return None;
    }
    let base = join_path(&[dirname(from), spec])?;
    let exts = if js_first_order {
        js_first()
    } else {
        source_extensions()
    };
    first_existing(&file_candidates(&base, &exts), ctx)
}

#[cfg(test)]
mod tests {
    use super::super::{MemCtx, Pack, find_declarations};
    use super::*;

    fn bindings(refs: &[ImportRef]) -> Vec<(String, String)> {
        refs.iter()
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
            .collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn js_imports() {
        let src = [
            "import def, { a, b as c } from './m1'",
            "import * as ns from './m2'",
            "import './side'",
            "const x = require('./r1')",
            "const { p, q: r } = require('./r2')",
            "const only = require('./r3').member",
            "const lazy = await import('./d1')",
            "foo(require('./inline'))",
        ]
        .join("\n");
        let refs = parse_imports(&src);
        assert_eq!(
            bindings(&refs),
            pairs(&[
                ("./m1", "default>def,a>a,b>c"),
                ("./m2", "*>ns"),
                ("./side", ""),
                ("./r1", "*>x"),
                ("./r2", "p>p,q>r"),
                ("./r3", "member>only"),
                ("./d1", "*>lazy"),
                ("./inline", ""),
            ])
        );
        for r in &refs {
            assert_eq!(&src[r.start..r.end], r.specifier);
        }
    }

    #[test]
    fn ts_imports() {
        let src = [
            "import type { T, U as V } from './types'",
            "export { x, y as z } from './xy'",
            "export * from './all'",
            "export * as space from './sp'",
            "import legacy = require('./legacy')",
            "import {",
            "  multi,",
            "  type Other",
            "} from '@/lib/multi'",
        ]
        .join("\n");
        let refs = parse_imports(&src);
        assert_eq!(
            bindings(&refs),
            pairs(&[
                ("./types", "T>T,U>V"),
                ("./xy", "x>x,y>z"),
                ("./all", "*>*"),
                ("./sp", "*>space"),
                ("./legacy", "*>legacy"),
                ("@/lib/multi", "multi>multi,Other>Other"),
            ])
        );
        assert_eq!(
            refs.iter().map(|r| r.reexport).collect::<Vec<_>>(),
            [false, true, true, true, false, false]
        );
    }

    #[test]
    fn js_resolution() {
        let ctx = MemCtx::new(&[
            ("src/a.js", ""),
            ("src/lib/index.js", ""),
            ("src/c.mjs", ""),
            ("src/t.ts", ""),
        ]);
        let p = Pack::Js;
        assert_eq!(
            p.resolve("./a", "src/main.js", &ctx).as_deref(),
            Some("src/a.js")
        );
        assert_eq!(
            p.resolve("./lib", "src/main.js", &ctx).as_deref(),
            Some("src/lib/index.js")
        );
        assert_eq!(p.resolve("../src/c.mjs", "src/x/main.js", &ctx), None);
        assert_eq!(
            p.resolve("../c.mjs", "src/x/main.js", &ctx).as_deref(),
            Some("src/c.mjs")
        );
        assert_eq!(
            p.resolve("./t.js", "src/main.js", &ctx).as_deref(),
            Some("src/t.ts")
        );
        assert_eq!(p.resolve("react", "src/main.js", &ctx), None);
        assert!(p.is_external("react"));
        assert!(!p.is_external("./a"));
        assert_eq!(p.resolve("../../../etc/passwd", "src/main.js", &ctx), None);
    }

    fn decl_lines(p: Pack, src: &str, name: &str) -> Vec<u32> {
        find_declarations(p, src, name)
            .iter()
            .map(|d| d.line)
            .collect()
    }

    #[test]
    fn js_declarations() {
        let src = [
            "function plain() {}",
            "export async function asyncFn(a) {}",
            "function* gen() {}",
            "export default class Klass {}",
            "const { a, b: renamed, c = 1 } = obj",
            "let counter = 0",
            "exports.viaExports = 1",
            "module.exports.viaModule = () => 1",
            "class X {",
            "  method(a, b) {",
            "  static async other() {",
            "  arrow = (x) => x",
            "}",
            "method(1, 2)",
            "if (cond) {",
            "describe(\"x\", () => {",
            "const o = { prop: function () {}, }",
            "  handler: async (e) => e,",
        ]
        .join("\n");
        let js = Pack::Js;
        for (name, want) in [
            ("plain", vec![1]),
            ("asyncFn", vec![2]),
            ("gen", vec![3]),
            ("Klass", vec![4]),
            ("a", vec![5]),
            ("renamed", vec![5]),
            ("b", vec![]),
            ("c", vec![5]),
            ("counter", vec![6]),
            ("viaExports", vec![7]),
            ("viaModule", vec![8]),
            ("method", vec![10]),
            ("other", vec![11]),
            ("arrow", vec![12]),
            ("if", vec![]),
            ("describe", vec![]),
            ("handler", vec![18]),
        ] {
            assert_eq!(decl_lines(js, &src, name), want, "{name}");
        }
        let d = &find_declarations(js, &src, "renamed")[0];
        assert_eq!(d.column, "const { a, b: ".len() as u32 + 1);
        assert_eq!(find_declarations(js, &src, "method")[0].kind, Kind::Member);
    }

    #[test]
    fn js_default_export_and_alias() {
        assert_eq!(
            default_export("const a = 1\nexport default a\n")
                .unwrap()
                .alias
                .as_deref(),
            Some("a")
        );
        assert_eq!(
            default_export("export default function named() {}")
                .unwrap()
                .decl
                .column,
            25
        );
        assert_eq!(
            default_export("x\nmodule.exports = {\n a }")
                .unwrap()
                .decl
                .line,
            2
        );
        assert_eq!(
            default_export("function f() {}\nmodule.exports = f")
                .unwrap()
                .alias
                .as_deref(),
            Some("f")
        );
        assert!(default_export("const x = 1").is_none());
        assert_eq!(
            export_alias("const inner = 1\nexport { inner as outer }", "outer").as_deref(),
            Some("inner")
        );
        assert_eq!(
            export_alias("export { inner as outer } from './x'", "outer"),
            None
        );
    }
}
