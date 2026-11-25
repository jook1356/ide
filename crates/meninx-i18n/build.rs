//! Generates the type-safe `tr::` API and the `Lang` enum from the Fluent files in `locales/`.
//!
//! - Every directory `locales/<code>/` is a language; it must contain `meninx.ftl`. `en` is the
//!   reference (fallback) locale: the API is generated from it.
//! - One function per message (`new-session` -> `tr::new_session()`) and per attribute
//!   (`new-session.tooltip` -> `tr::new_session_tooltip()`).
//! - Parameters come from the variables the English pattern uses, in order of first use.
//!   A variable is `i64` if the English pattern uses it as a plural selector (variant keys are
//!   numbers or CLDR plural categories) or passes it to `NUMBER()`; otherwise it is `&str`.
//! - The build fails on Fluent syntax errors, on a locale whose message/attribute/variable set
//!   differs from English, on message references (keep each message self-contained), and on
//!   function-name collisions.
//! - The message `language-name` (the language's name in itself) is reserved: required in every
//!   locale, exposed as `Lang::native_name()`, not as a `tr::` function.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use fluent_syntax::ast;

const REFERENCE: &str = "en";
const FILE: &str = "meninx.ftl";
const LANGUAGE_NAME: &str = "language-name";
const PLURAL_CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Str,
    Num,
}

/// One translatable unit: a message value or a message attribute.
#[derive(Debug, Clone)]
struct Unit {
    id: String,
    attr: Option<String>,
    /// Variables in order of first use, with their type in this locale.
    vars: Vec<(String, Kind)>,
    /// Plain-text rendering for the doc comment.
    preview: String,
    /// The pattern is plain text (no placeables).
    plain: Option<String>,
}

impl Unit {
    fn key(&self) -> String {
        match &self.attr {
            Some(a) => format!("{}.{}", self.id, a),
            None => self.id.clone(),
        }
    }
}

struct Locale {
    code: String,
    path: PathBuf,
    units: BTreeMap<String, Unit>,
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("locales");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut errors = Vec::new();
    let mut locales = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let code = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path().join(FILE);
        println!("cargo:rerun-if-changed={}", path.display());
        match load(&code, &path) {
            Ok(l) => locales.push(l),
            Err(mut e) => errors.append(&mut e),
        }
    }
    fail_if(&errors);

    // Reference first, then the others alphabetically.
    let Some(pos) = locales.iter().position(|l| l.code == REFERENCE) else {
        panic!("missing reference locale {}", dir.join(REFERENCE).display());
    };
    let reference = locales.remove(pos);
    locales.insert(0, reference);
    for l in &locales {
        errors.extend(check_parity(&locales[0], l));
        if !l.units.contains_key(LANGUAGE_NAME) {
            errors.push(format!(
                "{}: missing reserved message `{LANGUAGE_NAME}` (the language's own name)",
                l.path.display()
            ));
        } else if l.units[LANGUAGE_NAME].plain.is_none() {
            errors.push(format!(
                "{}: `{LANGUAGE_NAME}` must be plain text",
                l.path.display()
            ));
        }
    }
    fail_if(&errors);

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let tr = gen_tr(&locales[0], &mut errors);
    fail_if(&errors);
    std::fs::write(out.join("tr.rs"), tr).unwrap();
    std::fs::write(out.join("lang.rs"), gen_lang(&locales)).unwrap();
}

fn fail_if(errors: &[String]) {
    if !errors.is_empty() {
        panic!(
            "\n\nmeninx-i18n: {} translation error(s):\n  - {}\n\n",
            errors.len(),
            errors.join("\n  - ")
        );
    }
}

fn load(code: &str, path: &Path) -> Result<Locale, Vec<String>> {
    let src =
        std::fs::read_to_string(path).map_err(|e| vec![format!("{}: {e}", path.display())])?;
    let res = match fluent_syntax::parser::parse(src.as_str()) {
        Ok(r) => r,
        Err((_, errs)) => {
            return Err(errs
                .iter()
                .map(|e| {
                    let line = src[..e.pos.start.min(src.len())].lines().count().max(1);
                    format!(
                        "{}:{line}: Fluent syntax error: {:?}",
                        path.display(),
                        e.kind
                    )
                })
                .collect());
        }
    };
    let mut errors = Vec::new();
    let mut units = BTreeMap::new();
    for entry in &res.body {
        let ast::Entry::Message(m) = entry else {
            if let ast::Entry::Junk { content } = entry {
                errors.push(format!("{}: unparsable entry: {content:?}", path.display()));
            }
            continue;
        };
        let id = m.id.name.to_owned();
        let mut push = |attr: Option<String>, pattern: &ast::Pattern<&str>| {
            let mut c = Collect::default();
            c.pattern(pattern);
            for r in &c.message_refs {
                errors.push(format!(
                    "{}: `{id}` references message `{r}`; message references are not supported \
                     (keep each message self-contained, use a -term for shared words)",
                    path.display()
                ));
            }
            let unit = Unit {
                id: id.clone(),
                attr,
                vars: c.vars.iter().map(|v| (v.clone(), c.kind(v))).collect(),
                preview: c.preview.trim().to_owned(),
                plain: c.plain.then(|| c.preview.clone()),
            };
            if units.insert(unit.key(), unit.clone()).is_some() {
                errors.push(format!(
                    "{}: duplicate message `{}`",
                    path.display(),
                    unit.key()
                ));
            }
        };
        if let Some(v) = &m.value {
            push(None, v);
        }
        for a in &m.attributes {
            push(Some(a.id.name.to_owned()), &a.value);
        }
    }
    if errors.is_empty() {
        Ok(Locale {
            code: code.to_owned(),
            path: path.to_owned(),
            units,
        })
    } else {
        Err(errors)
    }
}

#[derive(Default)]
struct Collect {
    vars: Vec<String>,
    numeric: BTreeSet<String>,
    message_refs: Vec<String>,
    preview: String,
    plain: bool,
    depth: usize,
}

impl Collect {
    fn kind(&self, v: &str) -> Kind {
        if self.numeric.contains(v) {
            Kind::Num
        } else {
            Kind::Str
        }
    }

    fn var(&mut self, name: &str) {
        if !self.vars.iter().any(|v| v == name) {
            self.vars.push(name.to_owned());
        }
    }

    fn pattern(&mut self, p: &ast::Pattern<&str>) {
        if self.depth == 0 {
            self.plain = true;
        }
        for el in &p.elements {
            match el {
                ast::PatternElement::TextElement { value } => self.preview.push_str(value),
                ast::PatternElement::Placeable { expression } => {
                    self.plain = false;
                    self.expression(expression);
                }
            }
        }
    }

    fn expression(&mut self, e: &ast::Expression<&str>) {
        match e {
            ast::Expression::Inline(i) => {
                self.preview.push_str("{ ");
                self.inline(i);
                self.preview.push_str(" }");
            }
            ast::Expression::Select { selector, variants } => {
                if let ast::InlineExpression::VariableReference { id } = selector {
                    let plural = variants.iter().all(|v| match &v.key {
                        ast::VariantKey::NumberLiteral { .. } => true,
                        ast::VariantKey::Identifier { name } => PLURAL_CATEGORIES.contains(name),
                    });
                    if plural {
                        self.numeric.insert(id.name.to_owned());
                    }
                }
                self.preview.push_str("{ ");
                self.inline(selector);
                self.preview.push_str(" -> ");
                // Preview shows the default variant only.
                self.depth += 1;
                for v in variants {
                    if v.default {
                        let start = self.preview.len();
                        self.pattern(&v.value);
                        let shown = self.preview[start..].trim().to_owned();
                        self.preview.truncate(start);
                        self.preview.push_str(&shown);
                    } else {
                        // Still collect variables from the other variants.
                        let len = self.preview.len();
                        self.pattern(&v.value);
                        self.preview.truncate(len);
                    }
                }
                self.depth -= 1;
                self.preview.push_str(" }");
            }
        }
    }

    fn inline(&mut self, i: &ast::InlineExpression<&str>) {
        match i {
            ast::InlineExpression::StringLiteral { value } => {
                let _ = write!(self.preview, "\"{value}\"");
            }
            ast::InlineExpression::NumberLiteral { value } => self.preview.push_str(value),
            ast::InlineExpression::FunctionReference { id, arguments } => {
                if id.name == "NUMBER" {
                    for a in &arguments.positional {
                        if let ast::InlineExpression::VariableReference { id } = a {
                            self.numeric.insert(id.name.to_owned());
                        }
                    }
                }
                let _ = write!(self.preview, "{}(", id.name);
                for a in &arguments.positional {
                    self.inline(a);
                }
                for a in &arguments.named {
                    self.inline(&a.value);
                }
                self.preview.push(')');
            }
            ast::InlineExpression::MessageReference { id, .. } => {
                self.message_refs.push(id.name.to_owned());
            }
            ast::InlineExpression::TermReference { id, arguments, .. } => {
                let _ = write!(self.preview, "-{}", id.name);
                if let Some(args) = arguments {
                    for a in &args.positional {
                        self.inline(a);
                    }
                    for a in &args.named {
                        self.inline(&a.value);
                    }
                }
            }
            ast::InlineExpression::VariableReference { id } => {
                self.var(id.name);
                let _ = write!(self.preview, "${}", id.name);
            }
            ast::InlineExpression::Placeable { expression } => self.expression(expression),
        }
    }
}

fn check_parity(reference: &Locale, other: &Locale) -> Vec<String> {
    let mut errors = Vec::new();
    if reference.code == other.code {
        return errors;
    }
    let p = other.path.display();
    let missing: Vec<_> = reference
        .units
        .keys()
        .filter(|k| !other.units.contains_key(*k))
        .cloned()
        .collect();
    let extra: Vec<_> = other
        .units
        .keys()
        .filter(|k| !reference.units.contains_key(*k))
        .cloned()
        .collect();
    if !missing.is_empty() {
        errors.push(format!(
            "{p}: missing {} message(s) present in {REFERENCE}: {}",
            missing.len(),
            missing.join(", ")
        ));
    }
    if !extra.is_empty() {
        errors.push(format!(
            "{p}: {} extra message(s) not in {REFERENCE}: {}",
            extra.len(),
            extra.join(", ")
        ));
    }
    for (key, r) in &reference.units {
        let Some(o) = other.units.get(key) else {
            continue;
        };
        let rv: BTreeSet<_> = r.vars.iter().map(|(n, _)| n.as_str()).collect();
        let ov: BTreeSet<_> = o.vars.iter().map(|(n, _)| n.as_str()).collect();
        if rv != ov {
            errors.push(format!(
                "{p}: `{key}` uses variables {ov:?} but {REFERENCE} uses {rv:?}"
            ));
        }
        for (n, k) in &o.vars {
            let rk = r.vars.iter().find(|(rn, _)| rn == n).map(|(_, k)| *k);
            if *k == Kind::Num && rk == Some(Kind::Str) {
                errors.push(format!(
                    "{p}: `{key}` uses ${n} as a number, but it is a string in {REFERENCE} \
                     (use NUMBER(${n}) or a plural selector in {REFERENCE} too)"
                ));
            }
        }
    }
    errors
}

fn snake(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '-' || c == '.' {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

fn camel(code: &str) -> String {
    code.split(['-', '_'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            let first = c.next().unwrap().to_ascii_uppercase();
            std::iter::once(first)
                .chain(c.map(|c| c.to_ascii_lowercase()))
                .collect::<String>()
        })
        .collect()
}

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

fn ident(s: &str) -> String {
    let s = snake(s);
    if KEYWORDS.contains(&s.as_str()) {
        format!("r#{s}")
    } else {
        s
    }
}

fn gen_tr(reference: &Locale, errors: &mut Vec<String>) -> String {
    let mut out = String::new();
    out.push_str("// @generated by meninx-i18n/build.rs from locales/en/meninx.ftl. Do not edit.\n\n");
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut meta = String::new();
    for (key, u) in &reference.units {
        if key == LANGUAGE_NAME {
            continue;
        }
        let name = snake(key);
        if let Some(prev) = seen.insert(name.clone(), key.clone()) {
            errors.push(format!(
                "`{key}` and `{prev}` both map to tr::{name}(); rename one"
            ));
        }
        let params: Vec<String> = u
            .vars
            .iter()
            .map(|(v, k)| {
                let ty = match k {
                    Kind::Num => "i64",
                    Kind::Str => "&str",
                };
                format!("{}: {ty}", ident(v))
            })
            .collect();
        let preview = u.preview.replace('\n', " ");
        let _ = writeln!(out, "/// `{key}`: {preview}");
        let _ = writeln!(out, "#[must_use]");
        let _ = writeln!(
            out,
            "pub fn {}({}) -> String {{",
            ident(&name),
            params.join(", ")
        );
        let attr = match &u.attr {
            Some(a) => format!("Some({a:?})"),
            None => "None".into(),
        };
        if u.vars.is_empty() {
            let _ = writeln!(out, "    crate::format({:?}, {attr}, None)", u.id);
        } else {
            let _ = writeln!(
                out,
                "    let mut __fluent_args = ::fluent_bundle::FluentArgs::new();"
            );
            for (v, _) in &u.vars {
                let _ = writeln!(out, "    __fluent_args.set({v:?}, {});", ident(v));
            }
            let _ = writeln!(
                out,
                "    crate::format({:?}, {attr}, Some(&__fluent_args))",
                u.id
            );
        }
        out.push_str("}\n\n");
        let args: Vec<String> = u
            .vars
            .iter()
            .map(|(v, k)| format!("({v:?}, crate::ArgKind::{k:?})"))
            .collect();
        let _ = writeln!(
            meta,
            "    crate::MessageInfo {{ function: {name:?}, id: {:?}, attribute: {attr}, args: &[{}] }},",
            u.id,
            args.join(", ")
        );
    }
    let _ = writeln!(
        out,
        "/// Every generated function, for tests and tooling.\npub const MESSAGES: &[crate::MessageInfo] = &[\n{meta}];"
    );
    out
}

fn gen_lang(locales: &[Locale]) -> String {
    let mut out = String::new();
    out.push_str(
        "// @generated by meninx-i18n/build.rs from the locales/ directories. Do not edit.\n\n",
    );
    out.push_str(
        "/// A UI language (one per `locales/<code>/` directory).\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]\npub enum Lang {\n",
    );
    for l in locales {
        let name = l.units[LANGUAGE_NAME].plain.clone().unwrap_or_default();
        let _ = writeln!(out, "    /// {} (`{}`)", name.trim(), l.code);
        let _ = writeln!(out, "    {},", camel(&l.code));
    }
    out.push_str("}\n\nimpl Lang {\n");
    let all: Vec<String> = locales
        .iter()
        .map(|l| format!("Lang::{}", camel(&l.code)))
        .collect();
    let _ = writeln!(
        out,
        "    /// All languages, the fallback ({REFERENCE}) first.\n    pub const ALL: &'static [Lang] = &[{}];",
        all.join(", ")
    );
    let _ = writeln!(
        out,
        "    /// The reference language every other one falls back to.\n    pub const FALLBACK: Lang = Lang::{};",
        camel(REFERENCE)
    );
    out.push_str("\n    /// BCP 47 code (`\"en\"`, `\"ko\"`).\n    pub fn code(self) -> &'static str {\n        match self {\n");
    for l in locales {
        let _ = writeln!(out, "            Lang::{} => {:?},", camel(&l.code), l.code);
    }
    out.push_str("        }\n    }\n\n    /// The language's name in itself (`English`, `한국어`).\n    pub fn native_name(self) -> &'static str {\n        match self {\n");
    for l in locales {
        let name = l.units[LANGUAGE_NAME].plain.clone().unwrap_or_default();
        let _ = writeln!(
            out,
            "            Lang::{} => {:?},",
            camel(&l.code),
            name.trim()
        );
    }
    out.push_str("        }\n    }\n\n    pub(crate) fn index(self) -> usize {\n        self as usize\n    }\n}\n\n");
    out.push_str("/// `(code, Fluent source)` per language, in `Lang::ALL` order.\npub(crate) const SOURCES: &[(&str, &str)] = &[\n");
    for l in locales {
        let _ = writeln!(
            out,
            "    ({:?}, include_str!({:?})),",
            l.code,
            l.path.display().to_string()
        );
    }
    out.push_str("];\n");
    out
}
