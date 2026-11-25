//! Text helpers shared by the language packs (port of Hermes Desk `langpacks/util.ts`).
//! Offsets are byte offsets into `&str`; columns are 1-based char counts.

use std::sync::LazyLock;

use fancy_regex::Regex;

/// Compile a pattern known to be valid.
pub fn re(src: &str) -> Regex {
    Regex::new(src).unwrap_or_else(|e| panic!("bad regex {src:?}: {e}"))
}

/// `fancy_regex::escape`.
pub fn esc(s: &str) -> String {
    fancy_regex::escape(s).into_owned()
}

/// Kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Top-level binding.
    Top,
    /// Method / property function (weaker).
    Member,
}

/// A place where a name is declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    /// 1-based line.
    pub line: u32,
    /// 1-based char column of the name.
    pub column: u32,
    /// The trimmed line.
    pub preview: String,
    /// Kind.
    pub kind: Kind,
}

/// Identifier byte for JS/TS (`[A-Za-z0-9_$]`, plus any non-ASCII byte so Unicode
/// identifiers stay whole).
pub fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Identifier byte for Python (`[A-Za-z0-9_]` plus non-ASCII).
pub fn is_py_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Posix-style normalization of a `/` path; `None` if `..` climbs above the start.
pub fn normalize_path(p: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    let p = p.replace('\\', "/");
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            x => out.push(x),
        }
    }
    Some(out.join("/"))
}

/// Directory part (`""` for a bare name).
pub fn dirname(p: &str) -> &str {
    p.rfind('/').map_or("", |i| &p[..i])
}

/// Join non-empty parts and normalize.
pub fn join_path(parts: &[&str]) -> Option<String> {
    let joined: Vec<&str> = parts.iter().copied().filter(|s| !s.is_empty()).collect();
    normalize_path(&joined.join("/"))
}

/// 1-based line of a byte offset.
pub fn line_at(text: &str, offset: usize) -> u32 {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count() as u32
        + 1
}

/// Start byte of the line containing `offset`.
pub fn line_start(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1)
}

/// End byte (exclusive, before `\n`) of the line containing `offset`.
pub fn line_end(text: &str, offset: usize) -> usize {
    let start = offset.min(text.len());
    text.as_bytes()[start..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(text.len(), |i| start + i)
}

/// 1-based char column of byte `offset`.
pub fn column_at(text: &str, offset: usize) -> u32 {
    let s = line_start(text, offset);
    char_count(text, s, offset) + 1
}

/// Chars in `text[a..b]` (bounds clamped to char boundaries).
pub fn char_count(text: &str, a: usize, b: usize) -> u32 {
    let a = floor_boundary(text, a);
    let b = floor_boundary(text, b.max(a));
    text[a..b].chars().count() as u32
}

/// Largest char boundary `<= i`.
pub fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary `>= i`.
pub fn ceil_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// A [`Decl`] at byte `offset` of `text` (`kind` Top).
pub fn decl_at(text: &str, offset: usize) -> Decl {
    let s = line_start(text, offset);
    let e = line_end(text, offset);
    Decl {
        line: line_at(text, offset),
        column: char_count(text, s, offset) + 1,
        preview: text[s..e].trim().to_owned(),
        kind: Kind::Top,
    }
}

/// A word and its byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The word.
    pub word: String,
    /// Start byte.
    pub start: usize,
    /// End byte (exclusive).
    pub end: usize,
}

/// The identifier around `offset` (the caret may sit right after it). Not a number.
pub fn word_at(text: &str, offset: usize, ident: fn(u8) -> bool) -> Option<Word> {
    let b = text.as_bytes();
    let at = |i: usize| b.get(i).copied().is_some_and(ident);
    let mut start = offset.min(b.len());
    if !at(start) && start > 0 && at(start - 1) {
        start -= 1;
    }
    if !at(start) {
        return None;
    }
    let mut end = start;
    while start > 0 && at(start - 1) {
        start -= 1;
    }
    while end < b.len() && at(end) {
        end += 1;
    }
    let word = &text[start..end];
    if word.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    Some(Word {
        word: word.to_owned(),
        start,
        end,
    })
}

/// Lines of `text` without their `\r`, with byte offsets of their starts.
pub fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut pos = 0;
    text.split('\n').map(move |l| {
        let start = pos;
        pos += l.len() + 1;
        (start, l.strip_suffix('\r').unwrap_or(l))
    })
}

/// A declaration on 1-based `line` of `text`, at the first whole-word `name` (or column 1).
pub fn declaration_on_line(text: &str, line: u32, name: Option<&str>) -> Decl {
    let src = lines(text)
        .nth(line.saturating_sub(1) as usize)
        .map_or("", |(_, l)| l);
    let mut column = 1;
    if let Some(n) = name {
        let r = re(&format!(r"(^|[^\w$]){}(?![\w$])", esc(n)));
        if let Ok(Some(c)) = r.captures(src) {
            let m0 = c.get(0).unwrap();
            let g1 = c.get(1).unwrap();
            column = char_count(src, 0, m0.start() + g1.as_str().len()) + 1;
        }
    }
    Decl {
        line,
        column,
        preview: src.trim().to_owned(),
        kind: Kind::Top,
    }
}

/// A `^(prefix)NAME(?![\w$])suffix` declaration pattern.
pub struct DeclPattern<'a> {
    /// Regex source of the part before the name.
    pub prefix: &'a str,
    /// Regex source of the part after the name.
    pub suffix: &'a str,
    /// Kind of declaration.
    pub kind: Kind,
}

/// Shorthand for a [`DeclPattern`].
pub const fn pat<'a>(prefix: &'a str, suffix: &'a str, kind: Kind) -> DeclPattern<'a> {
    DeclPattern {
        prefix,
        suffix,
        kind,
    }
}

/// Compiled patterns for one name.
pub struct Compiled(Vec<(Regex, Kind)>);

impl Compiled {
    /// Compile `patterns` for `name`.
    pub fn new(patterns: &[DeclPattern<'_>], name: &str) -> Self {
        let n = esc(name);
        Compiled(
            patterns
                .iter()
                .map(|p| {
                    (
                        re(&format!(r"^({}){n}(?![\w$]){}", p.prefix, p.suffix)),
                        p.kind,
                    )
                })
                .collect(),
        )
    }

    /// The column (chars) right after the prefix group, if a pattern matches.
    pub fn first(&self, line: &str) -> Option<(u32, Kind)> {
        for (r, k) in &self.0 {
            if let Ok(Some(c)) = r.captures(line) {
                let g1 = c.get(1).unwrap();
                return Some((char_count(line, 0, g1.end()) + 1, *k));
            }
        }
        None
    }
}

/// Removes `//` and `/* */` comments outside strings and trailing commas, then parses JSON.
pub fn parse_jsonc(text: &str) -> Option<serde_json::Value> {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            out.extend_from_slice(&b[i..(j + 1).min(b.len())]);
            i = j + 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
        } else {
            out.push(c);
            i += 1;
        }
    }
    static TRAILING: LazyLock<Regex> = LazyLock::new(|| re(r",(\s*[}\]])"));
    let s = String::from_utf8_lossy(&out).into_owned();
    let s = TRAILING.replace_all(&s, "$1");
    serde_json::from_str(&s).ok()
}

/// Replace every match of `r` in `s` by the same number of spaces (byte offsets kept).
pub fn blank_matches(r: &Regex, s: &str) -> String {
    let mut out = s.as_bytes().to_vec();
    for m in r.find_iter(s).flatten() {
        for b in &mut out[m.start()..m.end()] {
            *b = b' ';
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Blank bytes `[a, b)` of `out` except line breaks.
pub fn blank_range(out: &mut [u8], a: usize, b: usize) {
    let n = out.len();
    for x in &mut out[a.min(n)..b.min(n)] {
        if *x != b'\n' && *x != b'\r' {
            *x = b' ';
        }
    }
}

/// Bytes back to a string; a multi-byte char cut by blanking becomes spaces (offsets kept).
pub fn bytes_to_string(mut out: Vec<u8>) -> String {
    loop {
        match std::str::from_utf8(&out) {
            Ok(_) => return String::from_utf8(out).unwrap(),
            Err(e) => {
                let s = e.valid_up_to();
                let n = e.error_len().unwrap_or(out.len() - s);
                for b in &mut out[s..s + n] {
                    *b = b' ';
                }
            }
        }
    }
}

/// For every bracket, the offset of its partner (`None` when unbalanced). With `depth`, also
/// the bracket depth at every offset.
pub fn partners(src: &[u8]) -> (Vec<Option<usize>>, Vec<u32>) {
    let mut m = vec![None; src.len()];
    let mut depth = vec![0u32; src.len()];
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..src.len() {
        let c = src[i];
        depth[i] = stack.len() as u32;
        if matches!(c, b'(' | b'{' | b'[') {
            stack.push(i);
        } else if matches!(c, b')' | b'}' | b']') {
            let open = match c {
                b')' => b'(',
                b'}' => b'{',
                _ => b'[',
            };
            while stack.last().is_some_and(|&o| src[o] != open) {
                stack.pop();
            }
            if let Some(o) = stack.pop() {
                m[o] = Some(i);
                m[i] = Some(o);
            }
        }
    }
    (m, depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(normalize_path("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize_path("../x"), None);
        assert_eq!(join_path(&["", "src", "./a"]).as_deref(), Some("src/a"));
        let s = "foo.barBaz(1)";
        assert_eq!(word_at(s, 6, is_word_char).unwrap().word, "barBaz");
        assert_eq!(word_at(s, 10, is_word_char).unwrap().word, "barBaz");
        assert_eq!(word_at(s, 0, is_word_char).unwrap().word, "foo");
        assert_eq!(word_at(s, 11, is_word_char), None);
        assert_eq!(
            word_at("한글_이름()", 3, is_word_char).unwrap().word,
            "한글_이름"
        );
        assert_eq!(column_at("ab\n가나다", 9), 3);
        let v = parse_jsonc("{\n // c \"q\"\n \"a\": /* b */ [1, 2,],\n}").unwrap();
        assert_eq!(v["a"][1], 2);
        let d = declaration_on_line("x\n  const 이름 = foo", 2, Some("foo"));
        assert_eq!((d.column, d.preview.as_str()), (14, "const 이름 = foo"));
    }
}
