//! Widgets (`docs/widgets.md`, Hermes Desk's mods): small web apps an agent drafts in its
//! session folder (`<root>/.ide/widgets/<id>/` with a `widget.json`) and registers once the
//! user allowed it: the registry (`<data>/widgets/registry.json`) keeps a copy of each
//! registered widget, which every session can open as a tab of its detail area, and the open
//! tabs. This module reads drafts (manifests, files inside a widget only), keeps the registry
//! and the widgets' key-value data per scope.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, read_json, write_json};

/// Widget folders, relative to the session folder.
pub const WIDGETS_DIR: &str = ".ide/widgets";
/// A widget's manifest file.
pub const MANIFEST: &str = "widget.json";
/// Longest widget id.
pub const MAX_ID: usize = 40;
/// Largest file a widget serves.
pub const MAX_FILE: u64 = 8 * 1024 * 1024;
/// Largest data file of one widget and scope (all its keys together).
pub const MAX_DATA: usize = 1024 * 1024;
/// Longest data key.
pub const MAX_KEY: usize = 200;

/// Why a widget request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WidgetError {
    #[error("no widget `{0}`")]
    NotFound(String),
    #[error("widget id `{0}` is not valid: 1-40 of a-z, 0-9 and -, not starting or ending with -")]
    BadId(String),
    #[error("widget `{id}`: {reason}")]
    Invalid { id: String, reason: String },
    #[error("no file `{0}` in the widget")]
    NoFile(String),
    #[error("file `{0}` is larger than 8 MiB")]
    TooLarge(String),
    #[error("data: {0}")]
    Data(String),
    #[error("{0}")]
    Io(String),
}

/// Where a widget's data is kept (`widget.json` `scope`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This session (the folder's owner: shared children see the same data).
    #[default]
    Session,
    /// The session family: the root session and every session below it.
    Family,
    /// Every session of this user.
    Global,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Session => "session",
            Scope::Family => "family",
            Scope::Global => "global",
        }
    }
}

/// `widget.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Name shown on the tab.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The page to open, relative to the widget folder.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// ide tools (`ide_*`) the widget calls through `window.ide.call`; the user approves them
    /// before the widget first runs.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub scope: Scope,
}

fn default_entry() -> String {
    "index.html".into()
}

/// A widget folder: its id and its manifest (or why it cannot be used).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Result<Manifest, String>,
}

/// A widget id: 1-40 of `[a-z0-9-]`, not starting or ending with `-` (it is a URL host label).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<root>/.ide/widgets`.
pub fn widgets_dir(root: &Path) -> PathBuf {
    root.join(WIDGETS_DIR)
}

/// A relative path inside a widget: no root, prefix, `..`; `/` or `\` separated.
fn safe_relative(rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches(['/', '\\']);
    if rel.is_empty() {
        return None;
    }
    let p = Path::new(rel);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Read and check a manifest.
fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join(MANIFEST);
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {MANIFEST}: {e}"))?;
    let m: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("{MANIFEST} is not valid: {e}"))?;
    if m.name.trim().is_empty() {
        return Err(format!("{MANIFEST}: `name` is empty"));
    }
    if safe_relative(&m.entry).is_none() {
        return Err(format!("{MANIFEST}: `entry` must be a path inside the widget folder"));
    }
    if !dir.join(safe_relative(&m.entry).unwrap()).is_file() {
        return Err(format!("{MANIFEST}: the entry file `{}` does not exist", m.entry));
    }
    for t in &m.tools {
        if !t.starts_with("ide_") {
            return Err(format!("{MANIFEST}: `tools` lists `{t}`, not an ide tool (ide_*)"));
        }
    }
    Ok(m)
}

/// The widgets of a session folder, by id (folders without a manifest are not widgets).
pub fn list(root: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(widgets_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(MANIFEST).is_file())
        .map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let dir = e.path();
            let manifest = if valid_id(&id) {
                read_manifest(&dir)
            } else {
                Err(WidgetError::BadId(id.clone()).to_string())
            };
            Entry { id, dir, manifest }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// One widget's manifest.
pub fn load(root: &Path, id: &str) -> Result<Manifest, WidgetError> {
    if !valid_id(id) {
        return Err(WidgetError::BadId(id.to_owned()));
    }
    let dir = widgets_dir(root).join(id);
    if !dir.join(MANIFEST).is_file() {
        return Err(WidgetError::NotFound(id.to_owned()));
    }
    read_manifest(&dir).map_err(|reason| WidgetError::Invalid {
        id: id.to_owned(),
        reason,
    })
}

/// A file of a registered widget (`dir`: its copy, `entry`: its entry page; `rel` empty or
/// `/` = the entry page): its bytes and media type. Only files inside `dir` (links leading out
/// of it are refused).
pub fn read_file(dir: &Path, entry: &str, rel: &str) -> Result<(Vec<u8>, &'static str), WidgetError> {
    let rel = if rel.trim_matches(['/', '\\']).is_empty() {
        entry
    } else {
        rel
    };
    let no = || WidgetError::NoFile(rel.to_owned());
    let sub = safe_relative(rel).ok_or_else(no)?;
    let path = dir.join(&sub);
    let (Ok(real_dir), Ok(real)) = (canon(dir), canon(&path)) else {
        return Err(no());
    };
    if !real.starts_with(&real_dir) || !real.is_file() {
        return Err(no());
    }
    let len = std::fs::metadata(&real).map(|m| m.len()).map_err(|_| no())?;
    if len > MAX_FILE {
        return Err(WidgetError::TooLarge(rel.to_owned()));
    }
    let bytes = std::fs::read(&real).map_err(|e| WidgetError::Io(e.to_string()))?;
    Ok((bytes, mime(&sub)))
}

fn canon(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p)
}

/// Media type by extension (what a widget usually ships; else `application/octet-stream`).
pub fn mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "jsx" | "tsx" | "ts" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------- data

/// The data file of `widget` in a scope. `owner`: the session folder's owner (session scope),
/// `family_root`: the family's root session (family scope).
pub fn data_file(paths: &Paths, scope: Scope, owner: &str, family_root: &str, widget: &str) -> PathBuf {
    let name = format!("{widget}.json");
    match scope {
        Scope::Session => paths.session_dir(owner).join("widgets").join(name),
        Scope::Family => paths.session_dir(family_root).join("widgets-family").join(name),
        Scope::Global => home(paths).join("data").join(name),
    }
}

type Data = BTreeMap<String, serde_json::Value>;

fn read_data(file: &Path) -> Result<Data, WidgetError> {
    read_json(file)
        .map(Option::unwrap_or_default)
        .map_err(|e| WidgetError::Io(e.to_string()))
}

fn check_key(key: &str) -> Result<(), WidgetError> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(WidgetError::Data(format!("a key is 1-{MAX_KEY} bytes")));
    }
    Ok(())
}

/// A data value (`None`: not set).
pub fn data_get(file: &Path, key: &str) -> Result<Option<serde_json::Value>, WidgetError> {
    check_key(key)?;
    Ok(read_data(file)?.remove(key))
}

/// Every key, sorted.
pub fn data_keys(file: &Path) -> Result<Vec<String>, WidgetError> {
    Ok(read_data(file)?.into_keys().collect())
}

/// Set (`Some`) or delete (`None`) a value. The caller serializes writes of one file.
pub fn data_put(file: &Path, key: &str, value: Option<serde_json::Value>) -> Result<(), WidgetError> {
    check_key(key)?;
    let mut d = read_data(file)?;
    match value {
        Some(v) => {
            d.insert(key.to_owned(), v);
        }
        None => {
            if d.remove(key).is_none() {
                return Ok(());
            }
        }
    }
    let size = serde_json::to_vec(&d).map(|v| v.len()).unwrap_or(usize::MAX);
    if size > MAX_DATA {
        return Err(WidgetError::Data(format!(
            "a widget keeps at most {} KiB per scope",
            MAX_DATA / 1024
        )));
    }
    write_json(file, &d).map_err(|e| WidgetError::Io(e.to_string()))
}

// ---------------------------------------------------------------- registry

/// Largest widget (all its files) that can be registered.
pub const MAX_TOTAL: u64 = 64 * 1024 * 1024;

/// `<data>/widgets`: the registry, the registered copies and the global data.
pub fn home(paths: &Paths) -> PathBuf {
    paths.root().join("widgets")
}

/// `<data>/widgets/registry.json`.
pub fn registry_file(paths: &Paths) -> PathBuf {
    home(paths).join("registry.json")
}

/// The registered copy of a widget (`<data>/widgets/sources/<id>/`): what the app serves.
pub fn source_dir(paths: &Paths, id: &str) -> PathBuf {
    home(paths).join("sources").join(id)
}

/// A registered widget (Desk's mod class): every session can open it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Class {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub entry: String,
    /// The ide tools the user allowed when it was registered.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub scope: Scope,
    /// 1 at the first registration, +1 at each update.
    pub version: u32,
    /// The session that registered it first.
    pub created_by: String,
    /// The session that registered the current version.
    #[serde(default)]
    pub updated_by: String,
    pub created: u64,
    pub updated: u64,
}

/// An open widget tab, remembered across restarts. `session`: the session showing it (a
/// `global` widget is one 공용 tab; `session` is then the session it was opened from).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTab {
    pub widget: String,
    pub session: String,
}

/// `<data>/widgets/registry.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub classes: BTreeMap<String, Class>,
    #[serde(default)]
    pub open: Vec<OpenTab>,
}

/// Why registering asks the user first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Not registered yet.
    New,
    /// Registered by another session.
    OtherSession { by: String },
    /// The ide tools it uses changed.
    ToolsChanged,
}

fn sorted(tools: &[String]) -> Vec<String> {
    let mut t = tools.to_vec();
    t.sort();
    t.dedup();
    t
}

/// The same tool lists (order and repeats aside)?
pub fn same_tools(a: &[String], b: &[String]) -> bool {
    sorted(a) == sorted(b)
}

impl Registry {
    pub fn load(paths: &Paths) -> Registry {
        read_json(&registry_file(paths)).ok().flatten().unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<(), WidgetError> {
        write_json(&registry_file(paths), self).map_err(|e| WidgetError::Io(e.to_string()))
    }

    /// Does registering `m` as `id` from `session` need the user's approval? (Updating your
    /// own widget with the same tools does not.)
    pub fn ask_for(&self, id: &str, session: &str, m: &Manifest) -> Option<Ask> {
        let Some(c) = self.classes.get(id) else {
            return Some(Ask::New);
        };
        if !same_tools(&c.tools, &m.tools) {
            return Some(Ask::ToolsChanged);
        }
        (c.created_by != session).then(|| Ask::OtherSession {
            by: c.created_by.clone(),
        })
    }

    /// Register (or update) the draft `id` of the session folder `root`: its files are copied
    /// to `<data>/widgets/sources/<id>/`. The caller saves the registry.
    pub fn register(&mut self, paths: &Paths, root: &Path, id: &str, session: &str) -> Result<Class, WidgetError> {
        let m = load(root, id)?;
        let from = widgets_dir(root).join(id);
        let to = source_dir(paths, id);
        let mut tmp = to.as_os_str().to_owned();
        tmp.push(".new");
        let tmp = PathBuf::from(tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        let mut total = 0;
        if let Err(e) = copy_widget(&from, &tmp, &mut total) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        if to.exists() {
            std::fs::remove_dir_all(&to).map_err(|e| WidgetError::Io(e.to_string()))?;
        }
        std::fs::rename(&tmp, &to).map_err(|e| WidgetError::Io(e.to_string()))?;
        let now = crate::paths::now_unix();
        let old = self.classes.get(id);
        let c = Class {
            name: m.name,
            description: m.description,
            entry: m.entry,
            tools: sorted(&m.tools),
            scope: m.scope,
            version: old.map_or(1, |c| c.version + 1),
            created_by: old.map_or_else(|| session.to_owned(), |c| c.created_by.clone()),
            updated_by: session.to_owned(),
            created: old.map_or(now, |c| c.created),
            updated: now,
        };
        // A scope change: the tabs follow the new placement when opened again.
        if old.is_some_and(|o| o.scope != c.scope) {
            self.open.retain(|t| t.widget != id);
        }
        self.classes.insert(id.to_owned(), c.clone());
        Ok(c)
    }

    /// Remove a widget: its copy, its open tabs and its global data (`false`: not registered).
    /// Session and family data stay in their sessions' folders until those go.
    pub fn unregister(&mut self, paths: &Paths, id: &str) -> Result<bool, WidgetError> {
        if !valid_id(id) || self.classes.remove(id).is_none() {
            return Ok(false);
        }
        self.open.retain(|t| t.widget != id);
        let dir = source_dir(paths, id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| WidgetError::Io(e.to_string()))?;
        }
        let _ = std::fs::remove_file(data_file(paths, Scope::Global, "", "", id));
        Ok(true)
    }

    /// Remember that `widget` is open (or closed) in `session` (`false`: nothing changed).
    pub fn set_open(&mut self, widget: &str, session: &str, open: bool) -> bool {
        let global = self.classes.get(widget).is_some_and(|c| c.scope == Scope::Global);
        let before = self.open.clone();
        // A global widget is one 공용 tab, wherever it was opened.
        self.open
            .retain(|t| t.widget != widget || (!global && t.session != session));
        if open && self.classes.contains_key(widget) {
            self.open.push(OpenTab {
                widget: widget.to_owned(),
                session: session.to_owned(),
            });
        }
        self.open != before
    }

    /// A session went: forget its tabs (`false`: it had none).
    pub fn forget_session(&mut self, session: &str) -> bool {
        let n = self.open.len();
        self.open.retain(|t| t.session != session);
        self.open.len() != n
    }
}

/// Copy a widget folder (files and folders only; links are left out), at most [`MAX_FILE`]
/// per file and [`MAX_TOTAL`] in all.
fn copy_widget(from: &Path, to: &Path, total: &mut u64) -> Result<(), WidgetError> {
    let io = |e: std::io::Error| WidgetError::Io(e.to_string());
    std::fs::create_dir_all(to).map_err(io)?;
    for e in std::fs::read_dir(from).map_err(io)?.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let dst = to.join(e.file_name());
        if t.is_dir() {
            copy_widget(&e.path(), &dst, total)?;
        } else if t.is_file() {
            let len = e.metadata().map_err(io)?.len();
            if len > MAX_FILE {
                return Err(WidgetError::TooLarge(e.file_name().to_string_lossy().into_owned()));
            }
            *total += len;
            if *total > MAX_TOTAL {
                return Err(WidgetError::Data(format!(
                    "a widget is at most {} MiB in all",
                    MAX_TOTAL / 1024 / 1024
                )));
            }
            std::fs::copy(e.path(), &dst).map_err(io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> crate::paths::TempDir {
        crate::paths::TempDir::new(tag)
    }

    fn widget(root: &Path, id: &str, manifest: &str) -> PathBuf {
        let d = widgets_dir(root).join(id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(MANIFEST), manifest).unwrap();
        d
    }

    #[test]
    fn ids() {
        assert!(valid_id("todo"));
        assert!(valid_id("my-board-2"));
        for bad in ["", "-a", "a-", "A", "a_b", "a.b", "가", &"a".repeat(41)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn listing_and_manifests() {
        let t = tmp("wlist");
        let root = t.0.as_path();
        assert!(list(root).is_empty());
        let d = widget(root, "board", r#"{"name":"Board","tools":["ide_session"],"scope":"family"}"#);
        std::fs::write(d.join("index.html"), "<p>hi</p>").unwrap();
        widget(root, "broken", r#"{"name":"x","bogus":1}"#);
        widget(root, "noentry", r#"{"name":"x"}"#);
        widget(root, "Bad_Id", r#"{"name":"x"}"#);
        widget(root, "bad-tool", r#"{"name":"x","entry":"a.html","tools":["Bash"]}"#);
        std::fs::write(widgets_dir(root).join("bad-tool").join("a.html"), "").unwrap();
        std::fs::create_dir_all(widgets_dir(root).join("plain-dir")).unwrap();

        let l = list(root);
        let ids: Vec<&str> = l.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["Bad_Id", "bad-tool", "board", "broken", "noentry"]);
        let board = l.iter().find(|e| e.id == "board").unwrap().manifest.clone().unwrap();
        assert_eq!(board.entry, "index.html");
        assert_eq!(board.scope, Scope::Family);
        assert_eq!(board.tools, ["ide_session"]);
        for id in ["Bad_Id", "bad-tool", "broken", "noentry"] {
            assert!(l.iter().find(|e| e.id == id).unwrap().manifest.is_err(), "{id}");
        }
        assert!(matches!(load(root, "nope"), Err(WidgetError::NotFound(_))));
        assert!(matches!(load(root, "broken"), Err(WidgetError::Invalid { .. })));
        assert!(matches!(load(root, "../x"), Err(WidgetError::BadId(_))));
    }

    #[test]
    fn files_stay_inside_the_widget() {
        let t = tmp("wfiles");
        let root = t.0.as_path();
        let d = widget(root, "w", r#"{"name":"W","entry":"app/main.html"}"#);
        std::fs::create_dir_all(d.join("app")).unwrap();
        std::fs::write(d.join("app/main.html"), "<b>x</b>").unwrap();
        std::fs::write(d.join("app/s.css"), "b{}").unwrap();
        std::fs::write(root.join("secret.txt"), "no").unwrap();

        let (b, m) = read_file(&d, "app/main.html", "").unwrap();
        assert_eq!((b.as_slice(), m), (&b"<b>x</b>"[..], "text/html; charset=utf-8"));
        assert_eq!(read_file(&d, "app/main.html", "/app/s.css").unwrap().1, "text/css; charset=utf-8");
        assert_eq!(read_file(&d, "app/main.html", "app\\s.css").unwrap().0, b"b{}");
        for bad in ["../../../secret.txt", "app/../../../../secret.txt", "/../x", "C:/Windows/win.ini", "nope.js", "app"] {
            assert!(read_file(&d, "app/main.html", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn registry() {
        let t = tmp("wreg");
        let paths = Paths::new(t.0.join("data"));
        let root = t.0.join("session");
        let d = widget(&root, "board", r#"{"name":"Board","tools":["ide_session"],"scope":"global"}"#);
        std::fs::write(d.join("index.html"), "v1").unwrap();
        std::fs::create_dir_all(d.join("js")).unwrap();
        std::fs::write(d.join("js/a.js"), "a").unwrap();

        let mut r = Registry::load(&paths);
        let m = load(&root, "board").unwrap();
        assert_eq!(r.ask_for("board", "s1", &m), Some(Ask::New));
        let c = r.register(&paths, &root, "board", "s1").unwrap();
        assert_eq!((c.version, c.created_by.as_str(), c.scope), (1, "s1", Scope::Global));
        assert_eq!(read_file(&source_dir(&paths, "board"), &c.entry, "js/a.js").unwrap().0, b"a");
        r.save(&paths).unwrap();
        let mut r = Registry::load(&paths);
        assert!(r.classes.contains_key("board"));

        // The draft changes; the registered copy changes only when registered again.
        std::fs::write(d.join("index.html"), "v2").unwrap();
        assert_eq!(read_file(&source_dir(&paths, "board"), "index.html", "").unwrap().0, b"v1");
        assert_eq!(r.ask_for("board", "s1", &m), None, "own update, same tools");
        assert_eq!(r.ask_for("board", "s2", &m), Some(Ask::OtherSession { by: "s1".into() }));
        let more = Manifest {
            tools: vec!["ide_session".into(), "ide_show_message".into()],
            ..m.clone()
        };
        assert_eq!(r.ask_for("board", "s1", &more), Some(Ask::ToolsChanged));
        let c = r.register(&paths, &root, "board", "s2").unwrap();
        assert_eq!((c.version, c.created_by.as_str(), c.updated_by.as_str()), (2, "s1", "s2"));
        assert_eq!(read_file(&source_dir(&paths, "board"), "index.html", "").unwrap().0, b"v2");

        // A global widget is one tab; others one per session.
        assert!(r.set_open("board", "s1", true));
        assert!(r.set_open("board", "s2", true));
        assert_eq!(r.open, [OpenTab { widget: "board".into(), session: "s2".into() }]);
        assert!(!r.set_open("nope", "s1", true), "not registered");
        let l = widget(&root, "local", r#"{"name":"L"}"#);
        std::fs::write(l.join("index.html"), "").unwrap();
        r.register(&paths, &root, "local", "s1").unwrap();
        r.set_open("local", "s1", true);
        r.set_open("local", "s2", true);
        assert_eq!(r.open.len(), 3);
        assert!(r.set_open("local", "s1", false));
        assert!(r.forget_session("s2"));
        assert!(r.open.is_empty());

        // Unregister: copy, tabs and global data go.
        let g = data_file(&paths, Scope::Global, "s1", "s1", "board");
        data_put(&g, "k", Some(serde_json::json!(1))).unwrap();
        r.set_open("board", "s1", true);
        assert!(r.unregister(&paths, "board").unwrap());
        assert!(!source_dir(&paths, "board").exists() && !g.exists() && r.open.is_empty());
        assert!(!r.unregister(&paths, "board").unwrap());
        assert!(matches!(r.register(&paths, &root, "nope", "s1"), Err(WidgetError::NotFound(_))));
    }

    #[test]
    fn data() {
        let t = tmp("wdata");
        let paths = Paths::new(t.0.clone());
        let f = data_file(&paths, Scope::Session, "s1", "r", "w");
        assert!(f.ends_with("sessions/s1/widgets/w.json"));
        assert!(data_file(&paths, Scope::Family, "s1", "r", "w").ends_with("sessions/r/widgets-family/w.json"));
        assert!(data_file(&paths, Scope::Global, "s1", "r", "w").ends_with("widgets/data/w.json"));

        assert_eq!(data_get(&f, "a").unwrap(), None);
        data_put(&f, "a", Some(serde_json::json!({"n": 1}))).unwrap();
        data_put(&f, "b", Some(serde_json::json!("x"))).unwrap();
        assert_eq!(data_get(&f, "a").unwrap(), Some(serde_json::json!({"n": 1})));
        assert_eq!(data_keys(&f).unwrap(), ["a", "b"]);
        data_put(&f, "a", None).unwrap();
        assert_eq!(data_keys(&f).unwrap(), ["b"]);
        assert!(data_put(&f, "", Some(serde_json::json!(1))).is_err());
        let big = "x".repeat(MAX_DATA);
        assert!(matches!(data_put(&f, "big", Some(serde_json::json!(big))), Err(WidgetError::Data(_))));
        assert_eq!(data_keys(&f).unwrap(), ["b"], "a refused write changes nothing");
    }
}
