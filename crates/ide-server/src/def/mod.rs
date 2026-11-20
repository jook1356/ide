//! Go-to-definition (`def.lookup`): a port of Hermes Desk's text-level heuristics
//! (`src/main/definitions.ts` + `src/shared/langpacks`): import specifiers open the module,
//! names resolve to a same-file declaration (nearest above, parameters first), then through
//! imports and re-export chains (max 3 hops), then a repo-wide `git grep` of declarations
//! (max 50 targets, same directory first).
//!
//! Languages: JavaScript, TypeScript (+ tsconfig `paths`/`baseUrl`), Python, Vue SFCs.
//! Offsets are byte offsets; results carry 1-based lines and char columns.

pub mod js;
pub mod py;
pub mod ts;
pub mod util;
pub mod vue;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use ide_proto::{DefResult, DefTarget, ProtoError};

use crate::jail::RootJail;
use util::*;

/// Files above this are not read for definitions (minified bundles, generated data).
const MAX_READ: u64 = 2 * 1024 * 1024;
/// Re-export / alias hops followed from an import to its declaration.
const MAX_DEPTH: u32 = 3;
/// Repo-wide candidates returned at most.
const MAX_TARGETS: usize = 50;

/// One name an import statement binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    /// Local name (`*` for `export * from` / `from x import *`).
    pub local: String,
    /// Name taken from the module: a name, `default`, or `*` (the module object).
    pub imported: String,
    /// The module the binding refers to when not the specifier (Python `import a.b` binds a).
    pub module: Option<String>,
}

/// An import / require / re-export statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRef {
    /// Module text as written.
    pub specifier: String,
    /// Byte range of the specifier in the source.
    pub start: usize,
    /// End (exclusive).
    pub end: usize,
    /// Bindings.
    pub bindings: Vec<ImportBinding>,
    /// Visible to importers of this file (`export ... from`, Python imports).
    pub reexport: bool,
}

/// What `export default` / `module.exports =` points at.
#[derive(Debug, Clone)]
pub struct DefaultExport {
    /// The default export's own line.
    pub decl: Decl,
    /// When the default is just another name, that name.
    pub alias: Option<String>,
}

/// Disk access for packs, relative to the root (`/`-separated).
pub trait Ctx {
    /// `rel` is an existing file.
    fn exists(&self, rel: &str) -> bool;
    /// The file's text (`None` when missing, too large or not a file).
    fn read(&self, rel: &str) -> Option<Rc<String>>;
}

/// Disk context confined to a [`RootJail`]; reads are cached for one lookup.
pub struct DiskCtx<'a> {
    jail: &'a RootJail,
    cache: RefCell<HashMap<String, Option<Rc<String>>>>,
}

impl<'a> DiskCtx<'a> {
    /// Context for `jail`.
    pub fn new(jail: &'a RootJail) -> Self {
        DiskCtx {
            jail,
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn file(&self, rel: &str) -> Option<std::path::PathBuf> {
        let n = normalize_path(rel)?;
        if n.is_empty() {
            return None;
        }
        let p = self.jail.resolve(&n).ok()?;
        p.is_file().then_some(p)
    }
}

impl Ctx for DiskCtx<'_> {
    fn exists(&self, rel: &str) -> bool {
        self.file(rel).is_some()
    }

    fn read(&self, rel: &str) -> Option<Rc<String>> {
        let key = normalize_path(rel)?;
        if let Some(v) = self.cache.borrow().get(&key) {
            return v.clone();
        }
        let v = self.file(&key).and_then(|p| {
            let m = std::fs::metadata(&p).ok()?;
            if m.len() > MAX_READ {
                return None;
            }
            let bytes = std::fs::read(&p).ok()?;
            Some(Rc::new(String::from_utf8_lossy(&bytes).into_owned()))
        });
        self.cache.borrow_mut().insert(key, v.clone());
        v
    }
}

/// In-memory context (tests).
#[derive(Default)]
pub struct MemCtx(HashMap<String, Rc<String>>);

impl MemCtx {
    /// Files by path.
    pub fn new(files: &[(&str, &str)]) -> Self {
        MemCtx(
            files
                .iter()
                .map(|(p, t)| (p.to_string(), Rc::new(t.to_string())))
                .collect(),
        )
    }
}

impl Ctx for MemCtx {
    fn exists(&self, rel: &str) -> bool {
        self.0.contains_key(rel)
    }
    fn read(&self, rel: &str) -> Option<Rc<String>> {
        self.0.get(rel).cloned()
    }
}

/// A language pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pack {
    /// `.js .jsx .mjs .cjs`
    Js,
    /// `.ts .tsx .mts .cts .d.ts`
    Ts,
    /// `.py .pyi`
    Py,
    /// `.vue`
    Vue,
}

/// Compiled declaration matcher of a pack for one name.
pub enum DeclMatcher {
    /// JS.
    Js(js::JsDecls),
    /// TS / Vue.
    Ts(ts::TsDecls),
    /// Python.
    Py(py::PyDecls),
}

impl DeclMatcher {
    /// The column of the name when `line` declares it.
    pub fn column(&self, line: &str) -> Option<(u32, Kind)> {
        match self {
            DeclMatcher::Js(d) => d.column(line),
            DeclMatcher::Ts(d) => d.column(line),
            DeclMatcher::Py(d) => d.column(line),
        }
    }
}

const JS_GLOBS: [&str; 4] = ["*.js", "*.jsx", "*.mjs", "*.cjs"];
const TS_GLOBS: [&str; 4] = ["*.ts", "*.tsx", "*.mts", "*.cts"];

impl Pack {
    const ALL: [Pack; 4] = [Pack::Ts, Pack::Js, Pack::Py, Pack::Vue];

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Pack::Js => &["js", "jsx", "mjs", "cjs"],
            Pack::Ts => &["ts", "tsx", "mts", "cts", "d.ts"],
            Pack::Py => &["py", "pyi"],
            Pack::Vue => &["vue"],
        }
    }

    /// The pack for a path by extension (longest match: `x.d.ts` -> TS).
    pub fn for_path(path: &str) -> Option<Pack> {
        let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
        let mut best: Option<(Pack, usize)> = None;
        for p in Pack::ALL {
            for ext in p.extensions() {
                if name.ends_with(&format!(".{ext}")) && best.is_none_or(|(_, l)| ext.len() > l) {
                    best = Some((p, ext.len()));
                }
            }
        }
        best.map(|(p, _)| p)
    }

    /// `git grep` pathspecs for the repo-wide search.
    pub fn grep_globs(self) -> Vec<&'static str> {
        match self {
            Pack::Js => JS_GLOBS.to_vec(),
            Pack::Ts => TS_GLOBS.iter().chain(JS_GLOBS.iter()).copied().collect(),
            Pack::Py => vec!["*.py", "*.pyi"],
            Pack::Vue => TS_GLOBS
                .iter()
                .chain(JS_GLOBS.iter())
                .copied()
                .chain(["*.vue"])
                .collect(),
        }
    }

    /// Identifier byte test.
    pub fn ident(self) -> fn(u8) -> bool {
        match self {
            Pack::Py => is_py_ident,
            _ => is_word_char,
        }
    }

    /// Every import-like statement.
    pub fn parse_imports(self, text: &str) -> Vec<ImportRef> {
        match self {
            Pack::Js | Pack::Ts => js::parse_imports(text),
            Pack::Vue => js::parse_imports(&vue::script_only(text)),
            Pack::Py => py::parse_imports(text),
        }
    }

    /// The root-relative file a specifier names from `from`.
    pub fn resolve(self, spec: &str, from: &str, ctx: &dyn Ctx) -> Option<String> {
        match self {
            Pack::Js => js::resolve(spec, from, ctx, true),
            Pack::Ts => ts::resolve(spec, from, ctx),
            Pack::Vue => vue::resolve(spec, from, ctx),
            Pack::Py => py::resolve(spec, from, ctx),
        }
    }

    /// A bare package the pack does not look into.
    pub fn is_external(self, spec: &str) -> bool {
        match self {
            Pack::Py => !spec.starts_with('.'),
            _ => !js::is_relative(spec) && !spec.starts_with('/'),
        }
    }

    /// Compile the declaration matcher for `name`.
    pub fn decls(self, name: &str) -> DeclMatcher {
        match self {
            Pack::Js => DeclMatcher::Js(js::JsDecls::new(name)),
            Pack::Ts | Pack::Vue => DeclMatcher::Ts(ts::TsDecls::new(name)),
            Pack::Py => DeclMatcher::Py(py::PyDecls::new(name)),
        }
    }

    /// The parameter `name` in scope at `offset`.
    pub fn parameter_at(self, text: &str, offset: usize, name: &str) -> Option<Decl> {
        match self {
            Pack::Js | Pack::Ts => js::parameter_at(text, offset, name),
            Pack::Vue => vue::in_script(text, offset)
                .then(|| js::parameter_at(&vue::script_only(text), offset, name))
                .flatten()
                .map(|mut d| {
                    // The masked text has blanks for the template: preview from the real line.
                    d.preview = declaration_on_line(text, d.line, None).preview;
                    d
                }),
            Pack::Py => py::parameter_at(text, offset, name),
        }
    }

    /// `export default` / `module.exports =`.
    pub fn default_export(self, text: &str) -> Option<DefaultExport> {
        match self {
            Pack::Js | Pack::Ts => js::default_export(text),
            Pack::Vue => js::default_export(&vue::script_only(text)).or_else(|| {
                Some(DefaultExport {
                    decl: declaration_on_line(text, 1, None),
                    alias: None,
                })
            }),
            Pack::Py => None,
        }
    }

    /// `export { local as name }`: the local behind `name`.
    pub fn export_alias(self, text: &str, name: &str) -> Option<String> {
        match self {
            Pack::Js | Pack::Ts => js::export_alias(text, name),
            Pack::Vue => js::export_alias(&vue::script_only(text), name),
            Pack::Py => None,
        }
    }

    /// `name` as a sub-module of `spec` (Python `from . import mod`).
    pub fn member_module(self, spec: &str, name: &str) -> Option<String> {
        match self {
            Pack::Py => Some(py::member_module(spec, name)),
            _ => None,
        }
    }

    /// The name to look up at `offset` when it is not a plain identifier.
    pub fn name_at(self, text: &str, offset: usize) -> Option<Word> {
        match self {
            Pack::Vue => vue::name_at(text, offset),
            _ => None,
        }
    }
}

/// Every declaration of `name` in `text`.
pub fn find_declarations(pack: Pack, text: &str, name: &str) -> Vec<Decl> {
    if name.is_empty() || !text.contains(name) {
        return Vec::new();
    }
    let m = pack.decls(name);
    let mut out = Vec::new();
    for (i, (_, line)) in lines(text).enumerate() {
        if !line.contains(name) {
            continue;
        }
        if let Some((column, kind)) = m.column(line) {
            out.push(Decl {
                line: i as u32 + 1,
                column,
                preview: line.trim().to_owned(),
                kind,
            });
        }
    }
    out
}

/// Internal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing (reason id + args).
    None(ProtoError),
    /// One target.
    One(DefTarget),
    /// Several.
    Many(String, Vec<DefTarget>),
}

impl From<Outcome> for DefResult {
    fn from(o: Outcome) -> Self {
        match o {
            Outcome::None(reason) => DefResult::None { reason },
            Outcome::One(t) => DefResult::One(t),
            Outcome::Many(name, targets) => DefResult::Many { name, targets },
        }
    }
}

fn none(id: &str) -> Outcome {
    Outcome::None(ProtoError::new(id))
}

fn none_spec(id: &str, spec: &str) -> Outcome {
    Outcome::None(ProtoError::new(id).arg("spec", spec))
}

fn target(path: &str, d: &Decl) -> DefTarget {
    DefTarget {
        path: path.to_owned(),
        line: d.line,
        col: d.column,
        preview: d.preview.clone(),
    }
}

/// The preferred declaration among several in one module: top-level, least indented, first.
fn best(decls: &[Decl]) -> Option<&Decl> {
    let top: Vec<&Decl> = decls.iter().filter(|d| d.kind == Kind::Top).collect();
    let pool: Vec<&Decl> = if top.is_empty() {
        decls.iter().collect()
    } else {
        top
    };
    let mut b: Option<&Decl> = None;
    for d in pool {
        if b.is_none_or(|x| d.column < x.column) {
            b = Some(d);
        }
    }
    b
}

struct Lookup<'a> {
    ctx: &'a dyn Ctx,
    /// Pack of the clicked file (fallback for files no pack claims).
    pack: Pack,
}

impl Lookup<'_> {
    fn file_top(&self, path: &str) -> DefTarget {
        let text = self.ctx.read(path).unwrap_or_default();
        target(path, &declaration_on_line(&text, 1, None))
    }

    fn module_of(
        &self,
        pack: Pack,
        r: &ImportRef,
        b: &ImportBinding,
        from: &str,
    ) -> Option<String> {
        pack.resolve(b.module.as_deref().unwrap_or(&r.specifier), from, self.ctx)
    }

    fn submodule(&self, pack: Pack, spec: &str, name: &str, from: &str) -> Option<String> {
        let sub = pack.member_module(spec, name)?;
        pack.resolve(&sub, from, self.ctx)
    }

    /// Where module `file` defines (or re-exports) `name` (`default` = its default export).
    fn find_export(
        &self,
        file: &str,
        name: &str,
        depth: u32,
        seen: &mut HashSet<String>,
    ) -> Option<DefTarget> {
        if !seen.insert(format!("{file}\n{name}")) {
            return None;
        }
        let pack = Pack::for_path(file).unwrap_or(self.pack);
        let text = self.ctx.read(file)?;
        if name == "default" {
            let d = pack.default_export(&text)?;
            if let Some(alias) = &d.alias
                && depth < MAX_DEPTH
                && let Some(via) = self.find_export(file, alias, depth + 1, seen)
            {
                return Some(via);
            }
            return Some(target(file, &d.decl));
        }
        let imports = pack.parse_imports(&text);
        let bound = binding_of(&imports, name);
        let import_line = bound.map(|(r, _)| line_at(&text, r.start));
        let decls: Vec<Decl> = find_declarations(pack, &text, name)
            .into_iter()
            .filter(|d| Some(d.line) != import_line)
            .collect();
        if let Some(own) = best(&decls) {
            return Some(target(file, own));
        }
        if depth >= MAX_DEPTH {
            return None;
        }
        if let Some(alias) = pack.export_alias(&text, name)
            && let Some(via) = self.find_export(file, &alias, depth + 1, seen)
        {
            return Some(via);
        }
        if let Some((r, b)) = bound
            && let Some(m) = self.module_of(pack, r, b, file)
        {
            if b.imported == "*" {
                return Some(self.file_top(&m));
            }
            if let Some(via) = self.find_export(&m, &b.imported, depth + 1, seen) {
                return Some(via);
            }
            let sub = self.submodule(pack, &r.specifier, &b.imported, file);
            return Some(self.file_top(sub.as_deref().unwrap_or(&m)));
        }
        for r in &imports {
            if !r.reexport || !r.bindings.iter().any(|b| b.local == "*") {
                continue;
            }
            if let Some(m) = pack.resolve(&r.specifier, file, self.ctx)
                && let Some(via) = self.find_export(&m, name, depth + 1, seen)
            {
                return Some(via);
            }
        }
        None
    }
}

fn binding_of<'a>(
    imports: &'a [ImportRef],
    local: &str,
) -> Option<(&'a ImportRef, &'a ImportBinding)> {
    imports
        .iter()
        .find_map(|r| r.bindings.iter().find(|b| b.local == local).map(|b| (r, b)))
}

/// Where an import statement binds `local`: the whole-word occurrence on the specifier's
/// line, else the last one before it (multi-line `{ }`).
fn binding_site(
    rel: &str,
    text: &str,
    r: &ImportRef,
    local: &str,
    ident: fn(u8) -> bool,
) -> Option<DefTarget> {
    let b = text.as_bytes();
    let ls = line_start(text, r.start);
    let le = line_end(text, r.start);
    let is_word_at = |i: usize| {
        let before = i.checked_sub(1).and_then(|j| b.get(j)).copied();
        let after = b.get(i + local.len()).copied();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    };
    let mut at = None;
    let mut from = ls;
    while let Some(i) = text[from..].find(local).map(|i| from + i) {
        if i >= le {
            break;
        }
        if is_word_at(i) {
            at = Some(i);
            break;
        }
        from = i + 1;
        while !text.is_char_boundary(from) {
            from += 1;
        }
    }
    if at.is_none() {
        let mut upto = ls;
        while let Some(i) = text[..upto].rfind(local) {
            if is_word_at(i) {
                at = Some(i);
                break;
            }
            upto = i;
        }
    }
    let at = at?;
    Some(target(rel, &decl_at(text, at)))
}

/// Normalize a root-relative path for lookups (`None` if it escapes).
fn safe_rel(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') || path.starts_with(['/', '\\']) || path.contains(':')
    {
        return None;
    }
    let p = path.replace('\\', "/");
    if p.split('/').any(|s| s == "..") {
        return None;
    }
    normalize_path(&p).filter(|n| !n.is_empty())
}

/// Go to definition in `path` at byte `offset` of `text` (the editor's current content).
/// `grep_root` = the work tree for the repo-wide `git grep` step (`None` skips it).
pub fn find_definition(
    ctx: &dyn Ctx,
    grep_root: Option<&Path>,
    path: &str,
    text: &str,
    offset: usize,
) -> Outcome {
    let Some(rel) = safe_rel(path) else {
        return none("def-invalid-position");
    };
    let rel = rel.as_str();
    let Some(pack) = Pack::for_path(rel) else {
        return Outcome::None(ProtoError::new("def-unsupported-file").arg("path", rel));
    };
    let lk = Lookup { ctx, pack };
    let imports = pack.parse_imports(text);

    // 1. The module string of an import / require.
    if let Some(r) = imports
        .iter()
        .find(|r| offset + 1 >= r.start && offset <= r.end + 1)
    {
        if let Some(file) = pack.resolve(&r.specifier, rel, ctx) {
            return Outcome::One(lk.file_top(&file));
        }
        return none_spec(
            if pack.is_external(&r.specifier) {
                "def-external-module"
            } else {
                "def-module-not-found"
            },
            &r.specifier,
        );
    }

    // 2. The name under the caret; `ns.member` of a module import looks in that module.
    let ident = pack.ident();
    let Some(w) = pack
        .name_at(text, offset)
        .or_else(|| word_at(text, offset, ident))
    else {
        return none("def-no-name");
    };
    let name = w.word.as_str();
    let click_line = line_at(text, w.start);
    let click_col = column_at(text, w.start);
    let is_member = w.start > 0 && text.as_bytes()[w.start - 1] == b'.';
    if is_member && w.start >= 2 {
        let ns = word_at(text, w.start - 2, ident);
        let b = ns
            .filter(|n| n.end == w.start - 1)
            .and_then(|n| binding_of(&imports, &n.word));
        if let Some((r, b)) = b {
            let m = if b.imported == "*" {
                lk.module_of(pack, r, b, rel)
            } else {
                lk.submodule(pack, &r.specifier, &b.imported, rel)
            };
            if let Some(m) = m {
                if let Some(t) = lk.find_export(&m, name, 0, &mut HashSet::new()) {
                    return Outcome::One(t);
                }
                let spec = b.module.as_deref().unwrap_or(&r.specifier);
                let sub = lk.submodule(pack, spec, name, rel);
                return Outcome::One(lk.file_top(sub.as_deref().unwrap_or(&m)));
            }
            let spec = b.module.as_deref().unwrap_or(&r.specifier);
            return none_spec(
                if pack.is_external(spec) {
                    "def-external-name"
                } else {
                    "def-module-not-found"
                },
                spec,
            );
        }
    }

    // 3. Declared in this file: the nearest one above the click; parameters first.
    let bound = if is_member {
        None
    } else {
        binding_of(&imports, name)
    };
    let import_line = bound.map(|(r, _)| line_at(text, r.start));
    let decls: Vec<Decl> = find_declarations(pack, text, name)
        .into_iter()
        .filter(|d| Some(d.line) != import_line)
        .collect();
    let param = if is_member {
        None
    } else {
        pack.parameter_at(text, w.start, name)
    };
    if let Some(p) = param {
        let inner: Vec<&Decl> = decls
            .iter()
            .filter(|d| d.line > p.line && d.line <= click_line)
            .collect();
        return Outcome::One(target(rel, inner.last().copied().unwrap_or(&p)));
    }
    let pool: Vec<&Decl> = if bound.is_some() {
        decls.iter().filter(|d| d.kind == Kind::Top).collect()
    } else {
        decls.iter().collect()
    };
    if !pool.is_empty() {
        let above: Vec<&&Decl> = pool.iter().filter(|d| d.line <= click_line).collect();
        let d = above.last().map(|d| **d).unwrap_or(pool[0]);
        return Outcome::One(target(rel, d));
    }

    // 4. Bound by an import: the declaration in that module (following re-exports).
    if let Some((r, b)) = bound {
        if let Some(m) = lk.module_of(pack, r, b, rel) {
            if b.imported != "*" {
                if let Some(t) = lk.find_export(&m, &b.imported, 0, &mut HashSet::new()) {
                    return Outcome::One(t);
                }
                if let Some(sub) = lk.submodule(pack, &r.specifier, &b.imported, rel) {
                    return Outcome::One(lk.file_top(&sub));
                }
            }
            return Outcome::One(lk.file_top(&m));
        }
        // External / unresolvable module: the import binding itself.
        if let Some(site) = binding_site(rel, text, r, name, ident)
            && !(site.line == click_line && site.col == click_col)
        {
            return Outcome::One(site);
        }
        let spec = b.module.as_deref().unwrap_or(&r.specifier);
        if pack.is_external(spec) {
            return none_spec("def-external-name", spec);
        }
    }

    // 5. Repo-wide: declarations of the name in the pack's file types.
    let targets = grep_root
        .map(|root| grep_declarations(root, pack, name, rel, click_line, click_col))
        .unwrap_or_default();
    match targets.len() {
        0 => Outcome::None(ProtoError::new("def-not-found").arg("name", name)),
        1 => Outcome::One(targets.into_iter().next().unwrap()),
        _ => Outcome::Many(name.to_owned(), targets),
    }
}

/// `git grep -w name` over the pack's globs (tracked and untracked files), kept where a line
/// declares the name.
fn grep_declarations(
    root: &Path,
    pack: Pack,
    name: &str,
    rel: &str,
    click_line: u32,
    click_col: u32,
) -> Vec<DefTarget> {
    let mut args = vec![
        "grep",
        "-n",
        "-I",
        "-w",
        "-z",
        "--untracked",
        "-e",
        name,
        "--",
    ];
    let globs = pack.grep_globs();
    args.extend(globs.iter().copied());
    let Ok(out) = ide_core::git_raw(root, &args, &[]) else {
        return Vec::new();
    };
    if out.code != Some(0) {
        return Vec::new(); // 1 = no match (or not a repo)
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut matchers: HashMap<Pack, DeclMatcher> = HashMap::new();
    let mut found: Vec<(DefTarget, Kind)> = Vec::new();
    for record in text.split('\n') {
        let mut parts = record.splitn(3, '\0');
        let (Some(file), Some(line_no), Some(content)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let content = content.strip_suffix('\r').unwrap_or(content);
        let Ok(line) = line_no.parse::<u32>() else {
            continue;
        };
        let fp = Pack::for_path(file).unwrap_or(pack);
        let m = matchers.entry(fp).or_insert_with(|| fp.decls(name));
        let Some((col, kind)) = m.column(content) else {
            continue;
        };
        if file == rel && line == click_line && col == click_col {
            continue;
        }
        found.push((
            DefTarget {
                path: file.to_owned(),
                line,
                col,
                preview: content.trim().to_owned(),
            },
            kind,
        ));
    }
    let has_top = found.iter().any(|(_, k)| *k == Kind::Top);
    let here = dirname(rel);
    let mut out: Vec<DefTarget> = found
        .into_iter()
        .filter(|(_, k)| !has_top || *k == Kind::Top)
        .map(|(t, _)| t)
        .collect();
    out.sort_by(|a, b| {
        (dirname(&b.path) == here)
            .cmp(&(dirname(&a.path) == here))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
    out.truncate(MAX_TARGETS);
    out
}

/// `def.lookup` over a session root: `line`/`col` (1-based, `col` in chars) of `text` (or the
/// file's content when `None`).
pub fn lookup(
    jail: &RootJail,
    rel: &str,
    line: u32,
    col: u32,
    text: Option<String>,
) -> Result<DefResult, ProtoError> {
    let norm = crate::jail::normalize_rel(rel)?;
    if Pack::for_path(&norm).is_none() {
        return Ok(
            Outcome::None(ProtoError::new("def-unsupported-file").arg("path", &norm)).into(),
        );
    }
    let text = match text {
        Some(t) => t,
        None => {
            let p = jail.resolve(&norm)?;
            let bytes = std::fs::read(&p).map_err(|e| crate::fsops::io_error(&norm, &e))?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    let Some(offset) = offset_of(&text, line, col) else {
        return Ok(DefResult::None {
            reason: ProtoError::new("def-invalid-position"),
        });
    };
    let ctx = DiskCtx::new(jail);
    Ok(find_definition(&ctx, Some(jail.root()), &norm, &text, offset).into())
}

/// Byte offset of 1-based `line` / char `col` (a column past the line end clamps to it).
pub fn offset_of(text: &str, line: u32, col: u32) -> Option<usize> {
    if line == 0 || col == 0 {
        return None;
    }
    let (start, l) = lines(text).nth(line as usize - 1)?;
    let within = l
        .char_indices()
        .nth(col as usize - 1)
        .map_or(l.len(), |(i, _)| i);
    Some(start + within)
}

#[cfg(test)]
mod tests;
