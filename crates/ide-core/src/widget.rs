//! Widgets (`docs/widgets.md`): small web apps an agent writes into its session folder
//! (`<root>/.ide/widgets/<id>/` with a `widget.json`), shown by the app as a tab of the detail
//! area. This module reads the folder (manifests, files inside a widget only), keeps the
//! widgets' key-value data per scope and the user's approvals of the ide tools a widget uses.

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
    #[error("no widget `{0}` in this session")]
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

/// A file of a widget (`rel` empty or `/` = its entry page): its bytes and media type. Only
/// files inside the widget folder (links leading out of it are refused).
pub fn read_file(root: &Path, id: &str, rel: &str) -> Result<(Vec<u8>, &'static str), WidgetError> {
    let m = load(root, id)?;
    let rel = if rel.trim_matches(['/', '\\']).is_empty() {
        m.entry.as_str()
    } else {
        rel
    };
    let no = || WidgetError::NoFile(rel.to_owned());
    let sub = safe_relative(rel).ok_or_else(no)?;
    let dir = widgets_dir(root).join(id);
    let path = dir.join(&sub);
    let (Ok(real_dir), Ok(real)) = (canon(&dir), canon(&path)) else {
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
        Scope::Global => paths.root().join("widgets").join(name),
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

// ---------------------------------------------------------------- approvals

/// The approvals of a session folder's widgets (`<data>/sessions/<owner>/widget-grants.json`).
pub fn grants_file(paths: &Paths, owner: &str) -> PathBuf {
    paths.session_dir(owner).join("widget-grants.json")
}

/// What the user approved per widget: the sorted tool list (a changed list asks again).
type Grants = BTreeMap<String, Vec<String>>;

fn sorted(tools: &[String]) -> Vec<String> {
    let mut t = tools.to_vec();
    t.sort();
    t.dedup();
    t
}

/// Did the user approve exactly these tools for `widget`? (No tools: nothing to approve.)
pub fn granted(file: &Path, widget: &str, tools: &[String]) -> bool {
    if tools.is_empty() {
        return true;
    }
    let g: Grants = read_json(file).ok().flatten().unwrap_or_default();
    g.get(widget).is_some_and(|t| *t == sorted(tools))
}

/// Record the user's approval of `tools` for `widget`.
pub fn grant(file: &Path, widget: &str, tools: &[String]) -> Result<(), WidgetError> {
    let mut g: Grants = read_json(file).ok().flatten().unwrap_or_default();
    g.insert(widget.to_owned(), sorted(tools));
    write_json(file, &g).map_err(|e| WidgetError::Io(e.to_string()))
}

/// Forget the approval of `widget`.
pub fn revoke(file: &Path, widget: &str) -> Result<(), WidgetError> {
    let mut g: Grants = read_json(file).ok().flatten().unwrap_or_default();
    if g.remove(widget).is_some() {
        write_json(file, &g).map_err(|e| WidgetError::Io(e.to_string()))?;
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

        let (b, m) = read_file(root, "w", "").unwrap();
        assert_eq!((b.as_slice(), m), (&b"<b>x</b>"[..], "text/html; charset=utf-8"));
        assert_eq!(read_file(root, "w", "/app/s.css").unwrap().1, "text/css; charset=utf-8");
        assert_eq!(read_file(root, "w", "app\\s.css").unwrap().0, b"b{}");
        for bad in ["../../../secret.txt", "app/../../../../secret.txt", "/../x", "C:/Windows/win.ini", "nope.js", "app"] {
            assert!(read_file(root, "w", bad).is_err(), "{bad}");
        }
        assert!(matches!(read_file(root, "x", ""), Err(WidgetError::NotFound(_))));
    }

    #[test]
    fn data_and_grants() {
        let t = tmp("wdata");
        let paths = Paths::new(t.0.clone());
        let f = data_file(&paths, Scope::Session, "s1", "r", "w");
        assert!(f.ends_with("sessions/s1/widgets/w.json"));
        assert!(data_file(&paths, Scope::Family, "s1", "r", "w").ends_with("sessions/r/widgets-family/w.json"));
        assert!(data_file(&paths, Scope::Global, "s1", "r", "w").ends_with("widgets/w.json"));

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

        let g = grants_file(&paths, "s1");
        let tools = vec!["ide_session".to_owned(), "ide_show_message".to_owned()];
        assert!(granted(&g, "w", &[]));
        assert!(!granted(&g, "w", &tools));
        grant(&g, "w", &[tools[1].clone(), tools[0].clone()]).unwrap();
        assert!(granted(&g, "w", &tools));
        assert!(!granted(&g, "w", &tools[..1]), "another list asks again");
        assert!(!granted(&g, "other", &tools));
        revoke(&g, "w").unwrap();
        assert!(!granted(&g, "w", &tools));
    }
}
