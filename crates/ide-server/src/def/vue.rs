//! Vue single-file components (port of Hermes Desk `langpacks/vue`): the `<script>` blocks are
//! TS/JS; template names (`{{ foo }}`, `@click="onClick"`, `<MyButton>`, `<my-button>`) are
//! looked up in them.

use std::sync::LazyLock;

use fancy_regex::Regex;

use super::util::*;
use super::{Ctx, js, ts};

static SCRIPT_OPEN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<script\b[^>]*>"));

/// Content ranges of the `<script>` / `<script setup>` blocks (end exclusive).
pub fn script_blocks(text: &str) -> Vec<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= text.len() {
        let Ok(Some(m)) = SCRIPT_OPEN.find_from_pos(text, pos) else {
            break;
        };
        let start = m.end();
        let end = lower[start..]
            .find("</script")
            .map_or(text.len(), |c| start + c);
        out.push((start, end));
        if end <= pos {
            break;
        }
        pos = end;
    }
    out
}

/// The SFC with everything outside its script blocks blanked (byte offsets and newlines kept).
pub fn script_only(text: &str) -> String {
    let mut out = text.as_bytes().to_vec();
    let mut pos = 0;
    for (s, e) in script_blocks(text) {
        blank_range(&mut out, pos, s);
        pos = e;
    }
    blank_range(&mut out, pos, text.len());
    // Blanking whole bytes of multi-byte chars outside scripts keeps valid UTF-8 inside them.
    bytes_to_string(out)
}

/// `offset` lies in a script block.
pub fn in_script(text: &str, offset: usize) -> bool {
    script_blocks(text)
        .iter()
        .any(|&(s, e)| offset >= s && offset <= e)
}

fn tag_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'-')
}

/// `my-button` -> `MyButton`.
fn pascal(tag: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in tag.chars() {
        if c == '-' {
            up = true;
            continue;
        }
        if up && (c.is_ascii_lowercase() || c.is_ascii_digit()) {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
        up = false;
    }
    out
}

/// A kebab-case component tag under the caret in the template (`<my-button>` -> `MyButton`).
pub fn name_at(text: &str, offset: usize) -> Option<Word> {
    if in_script(text, offset) {
        return None;
    }
    let b = text.as_bytes();
    let at = |i: usize| b.get(i).copied().is_some_and(tag_char);
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
    let tag = &text[start..end];
    let prev = |k: usize| start.checked_sub(k).and_then(|i| b.get(i)).copied();
    let is_tag = prev(1) == Some(b'<') || (prev(1) == Some(b'/') && prev(2) == Some(b'<'));
    if !is_tag || !tag.contains('-') || !tag.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    Some(Word {
        word: pascal(tag),
        start,
        end,
    })
}

/// TS resolution, then the Vite / Vue CLI `@/x` -> `src/x` convention.
pub fn resolve(spec: &str, from: &str, ctx: &dyn Ctx) -> Option<String> {
    if let Some(r) = ts::resolve(spec, from, ctx) {
        return Some(r);
    }
    let rest = spec
        .strip_prefix("@/")
        .or_else(|| spec.strip_prefix("~/"))?;
    if rest.is_empty() {
        return None;
    }
    js::first_existing(
        &js::file_candidates(&format!("src/{rest}"), &js::source_extensions()),
        ctx,
    )
}

#[cfg(test)]
mod tests {
    use super::super::{MemCtx, Pack};
    use super::*;

    #[test]
    fn vue_pack() {
        assert_eq!(Pack::for_path("src/App.vue"), Some(Pack::Vue));
        let src = [
            "<template>",
            "  <p>import x from './not-an-import'</p>",
            "  <my-card />",
            "</template>",
            "<script setup lang=\"ts\">",
            "import MyCard from './MyCard.vue'",
            "</script>",
        ]
        .join("\n");
        let refs = Pack::Vue.parse_imports(&src);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].specifier, "./MyCard.vue");
        assert_eq!(refs[0].bindings[0].local, "MyCard");
        assert_eq!(&src[refs[0].start..refs[0].end], "./MyCard.vue");
        let tag = name_at(&src, src.find("my-card").unwrap() + 4).unwrap();
        assert_eq!(tag.word, "MyCard");
        assert_eq!(&src[tag.start..tag.end], "my-card");
        assert_eq!(name_at(&src, src.find("MyCard from").unwrap()), None);
        assert_eq!(Pack::Vue.default_export(&src).unwrap().decl.line, 1);
        let ctx = MemCtx::new(&[("src/MyCard.vue", ""), ("src/Plain.vue", "")]);
        let r = |s: &str| Pack::Vue.resolve(s, "src/App.vue", &ctx);
        assert_eq!(r("./MyCard.vue").as_deref(), Some("src/MyCard.vue"));
        assert_eq!(r("./Plain").as_deref(), Some("src/Plain.vue"));
        assert_eq!(r("@/MyCard.vue").as_deref(), Some("src/MyCard.vue"));
        assert_eq!(
            Pack::Ts
                .resolve("./MyCard.vue", "src/main.ts", &ctx)
                .as_deref(),
            Some("src/MyCard.vue")
        );
        assert_eq!(pascal("my-button"), "MyButton");
    }
}
