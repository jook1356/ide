//! Previews (`docs/previews.md`): what a session's agent shows the user in a tab of the detail
//! area with the `ide_preview` tool. HTML or Markdown text is written to
//! `<data>/sessions/<session>/previews/<id>.html` (Markdown rendered to a page); a file
//! preview points at a file inside the session folder (HTML, Markdown, an image); a URL preview
//! is an ordinary web page. `<id>.json` keeps what a preview shows, so the app's
//! `ide-preview://<id>.<session>/` requests can be answered (the entry page and, for pages,
//! the files they load, only from inside the session folder).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{Paths, now_unix, read_json, write_json};
use crate::widget::safe_relative;

/// Longest preview id (it is a URL host label, like a widget id).
pub const MAX_ID: usize = 40;
/// Largest content of an HTML / Markdown preview and largest file a preview serves.
pub const MAX_SIZE: u64 = 8 * 1024 * 1024;
/// Longest title.
pub const MAX_TITLE: usize = 120;

/// Why a preview request failed (the agent reads the text).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreviewError {
    #[error("no preview `{0}` in this session")]
    NotFound(String),
    #[error("preview id `{0}` is not valid: 1-40 of a-z, 0-9 and -, not starting or ending with -")]
    BadId(String),
    #[error("{0}")]
    Invalid(String),
    #[error("no file `{0}` inside the session folder")]
    NoFile(String),
    #[error("`{0}` is larger than 8 MiB")]
    TooLarge(String),
    #[error("{0}")]
    Io(String),
}

/// What a preview shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Html,
    Markdown,
    File,
    Url,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "html" => Self::Html,
            "markdown" | "md" => Self::Markdown,
            "file" => Self::File,
            "url" => Self::Url,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "markdown",
            Self::File => "file",
            Self::Url => "url",
        }
    }
}

/// A preview of a session (`<id>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    /// `File`: the file (canonical path inside the session folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// `Url`: the web address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub updated: u64,
}

/// A preview id: 1-40 of `[a-z0-9-]`, not starting or ending with `-`.
pub fn valid_id(id: &str) -> bool {
    crate::widget::valid_id(id)
}

/// An id made from a title (`"Login page v2"` -> `login-page-v2`; nothing usable: `preview`).
pub fn id_from_title(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= MAX_ID {
            break;
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() { "preview".into() } else { out }
}

/// `<data>/sessions/<session>/previews`.
pub fn previews_dir(paths: &Paths, session: &str) -> PathBuf {
    paths.session_dir(session).join("previews")
}

fn record_file(paths: &Paths, session: &str, id: &str) -> PathBuf {
    previews_dir(paths, session).join(format!("{id}.json"))
}

fn page_file(paths: &Paths, session: &str, id: &str) -> PathBuf {
    previews_dir(paths, session).join(format!("{id}.html"))
}

fn io(e: impl std::fmt::Display) -> PreviewError {
    PreviewError::Io(e.to_string())
}

fn check(id: &str, title: &str) -> Result<String, PreviewError> {
    if !valid_id(id) {
        return Err(PreviewError::BadId(id.to_owned()));
    }
    let title = title.trim();
    if title.is_empty() {
        return Err(PreviewError::Invalid("`title` is required".into()));
    }
    Ok(title.chars().take(MAX_TITLE).collect())
}

fn save(paths: &Paths, session: &str, p: &Preview) -> Result<(), PreviewError> {
    write_json(&record_file(paths, session, &p.id), p).map_err(io)
}

/// Show HTML or Markdown text (`kind` `Html` / `Markdown`): written as the preview's page
/// (the same id replaces it).
pub fn put_content(
    paths: &Paths,
    session: &str,
    id: &str,
    title: &str,
    kind: Kind,
    content: &str,
) -> Result<Preview, PreviewError> {
    let title = check(id, title)?;
    if content.len() as u64 > MAX_SIZE {
        return Err(PreviewError::TooLarge("content".into()));
    }
    let html = match kind {
        Kind::Html => content.to_owned(),
        Kind::Markdown => markdown_page(&title, content),
        _ => return Err(PreviewError::Invalid("content previews are html or markdown".into())),
    };
    let page = page_file(paths, session, id);
    crate::paths::ensure_dir(page.parent().expect("previews dir")).map_err(io)?;
    std::fs::write(&page, html).map_err(io)?;
    let p = Preview {
        id: id.to_owned(),
        title,
        kind,
        file: None,
        url: None,
        updated: now_unix(),
    };
    save(paths, session, &p)?;
    Ok(p)
}

/// The file kinds a file preview shows.
fn showable(path: &Path) -> bool {
    let ext = ext(path);
    matches!(
        ext.as_str(),
        "html" | "htm" | "md" | "markdown" | "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "txt"
    )
}

fn ext(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_markdown(path: &Path) -> bool {
    matches!(ext(path).as_str(), "md" | "markdown")
}

/// `path` (relative to `root`, or absolute) as a canonical file inside `root`.
fn inside(root: &Path, path: &Path, shown: &str) -> Result<PathBuf, PreviewError> {
    let no = || PreviewError::NoFile(shown.to_owned());
    let real_root = std::fs::canonicalize(root).map_err(|_| no())?;
    let full = if path.is_absolute() { path.to_owned() } else { root.join(path) };
    let real = std::fs::canonicalize(&full).map_err(|_| no())?;
    if !real.starts_with(&real_root) || !real.is_file() {
        return Err(no());
    }
    let len = std::fs::metadata(&real).map_err(|_| no())?.len();
    if len > MAX_SIZE {
        return Err(PreviewError::TooLarge(shown.to_owned()));
    }
    Ok(real)
}

/// Show a file inside the session folder `root` (`path`: relative to it, or absolute).
pub fn put_file(
    paths: &Paths,
    session: &str,
    root: &Path,
    id: &str,
    title: &str,
    path: &str,
) -> Result<Preview, PreviewError> {
    let title = check(id, title)?;
    let path = path.trim();
    if path.is_empty() {
        return Err(PreviewError::Invalid("`path` is required for a file preview".into()));
    }
    let real = inside(root, Path::new(path), path)?;
    if !showable(&real) {
        return Err(PreviewError::Invalid(format!(
            "`{path}`: a file preview shows HTML, Markdown, text or an image (png, jpg, gif, webp, svg, ico, bmp)"
        )));
    }
    let _ = std::fs::remove_file(page_file(paths, session, id));
    let p = Preview {
        id: id.to_owned(),
        title,
        kind: Kind::File,
        file: Some(real),
        url: None,
        updated: now_unix(),
    };
    save(paths, session, &p)?;
    Ok(p)
}

/// Show a web page (`http(s)://` only).
pub fn put_url(paths: &Paths, session: &str, id: &str, title: &str, url: &str) -> Result<Preview, PreviewError> {
    let title = check(id, title)?;
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) || url.len() <= "https://".len() {
        return Err(PreviewError::Invalid(
            "a url preview needs an http:// or https:// address in `path` (or `content`)".into(),
        ));
    }
    let _ = std::fs::remove_file(page_file(paths, session, id));
    let p = Preview {
        id: id.to_owned(),
        title,
        kind: Kind::Url,
        file: None,
        url: Some(url.to_owned()),
        updated: now_unix(),
    };
    save(paths, session, &p)?;
    Ok(p)
}

/// A preview of the session.
pub fn load(paths: &Paths, session: &str, id: &str) -> Result<Preview, PreviewError> {
    if !valid_id(id) {
        return Err(PreviewError::BadId(id.to_owned()));
    }
    read_json(&record_file(paths, session, id))
        .ok()
        .flatten()
        .ok_or_else(|| PreviewError::NotFound(id.to_owned()))
}

/// The session's previews (newest first).
pub fn list(paths: &Paths, session: &str) -> Vec<Preview> {
    let Ok(rd) = std::fs::read_dir(previews_dir(paths, session)) else {
        return Vec::new();
    };
    let mut out: Vec<Preview> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_json::<Preview>(&e.path()).ok().flatten())
        .collect();
    out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    out
}

/// Forget a preview (its page and record).
pub fn remove(paths: &Paths, session: &str, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    let a = std::fs::remove_file(record_file(paths, session, id)).is_ok();
    let _ = std::fs::remove_file(page_file(paths, session, id));
    a
}

/// A file of preview `id` of `session`, whose folder is `root`: `rel` empty = the preview's
/// page (Markdown rendered); else a file the page loads, relative to the shown file's folder
/// (file previews) or to the session folder (HTML / Markdown text), never outside the session
/// folder. Bytes and media type.
pub fn read(
    paths: &Paths,
    session: &str,
    root: &Path,
    id: &str,
    rel: &str,
) -> Result<(Vec<u8>, &'static str), PreviewError> {
    let p = load(paths, session, id)?;
    let rel = rel.trim_start_matches(['/', '\\']);
    const HTML: &str = "text/html; charset=utf-8";
    if rel.is_empty() {
        return match p.kind {
            Kind::Html | Kind::Markdown => {
                let page = page_file(paths, session, id);
                Ok((std::fs::read(&page).map_err(|_| PreviewError::NotFound(id.to_owned()))?, HTML))
            }
            Kind::File => {
                let file = p.file.as_deref().ok_or_else(|| PreviewError::NotFound(id.to_owned()))?;
                // Checked again: the file may have moved or grown since.
                let real = inside(root, file, &file.display().to_string())?;
                let bytes = std::fs::read(&real).map_err(io)?;
                if is_markdown(&real) {
                    let text = String::from_utf8_lossy(&bytes);
                    Ok((markdown_page(&p.title, &text).into_bytes(), HTML))
                } else {
                    Ok((bytes, crate::widget::mime(&real)))
                }
            }
            Kind::Url => Err(PreviewError::NoFile(String::new())),
        };
    }
    let base = match p.kind {
        Kind::File => p
            .file
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_owned)
            .ok_or_else(|| PreviewError::NoFile(rel.to_owned()))?,
        Kind::Html | Kind::Markdown => root.to_owned(),
        Kind::Url => return Err(PreviewError::NoFile(rel.to_owned())),
    };
    let sub = safe_relative(rel).ok_or_else(|| PreviewError::NoFile(rel.to_owned()))?;
    let real = inside(root, &base.join(sub), rel)?;
    let bytes = std::fs::read(&real).map_err(io)?;
    Ok((bytes, crate::widget::mime(&real)))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// No scripts in rendered Markdown: a file from a repository (e.g. a third party's README) may
/// carry raw HTML, and a script there could read the session's files through the preview's
/// origin. Styles inline, images from anywhere.
const MARKDOWN_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src * data:; font-src *";

/// Markdown as a whole HTML page (CommonMark with tables, task lists, strikethrough,
/// footnotes), styled for ide's dark detail area; scripts are blocked ([`MARKDOWN_CSP`]).
pub fn markdown_page(title: &str, markdown: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut body = String::with_capacity(markdown.len() * 3 / 2);
    html::push_html(&mut body, Parser::new_ext(markdown, opts));
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{MARKDOWN_CSP}\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{}</title><style>{MARKDOWN_CSS}</style></head>\n<body><main>\n{body}</main></body></html>\n",
        escape(title)
    )
}

const MARKDOWN_CSS: &str = "\
:root{color-scheme:dark}\
body{margin:0;background:#1e1f22;color:#d4d6db;font:14px/1.6 system-ui,-apple-system,'Segoe UI',sans-serif}\
main{max-width:860px;margin:0 auto;padding:20px 24px 48px}\
h1,h2,h3,h4{color:#eceef2;line-height:1.3;margin:1.4em 0 .5em}\
h1{font-size:1.7em;border-bottom:1px solid #3a3d44;padding-bottom:.3em}\
h2{font-size:1.35em;border-bottom:1px solid #3a3d44;padding-bottom:.25em}\
a{color:#6ea8fe}\
code{font:12.5px/1.5 'Cascadia Mono',Consolas,'SF Mono',monospace;background:#2b2d31;padding:.1em .35em;border-radius:4px}\
pre{background:#2b2d31;padding:12px 14px;border-radius:6px;overflow:auto}\
pre code{background:none;padding:0}\
blockquote{margin:0;padding:0 1em;color:#9da1aa;border-left:3px solid #3a3d44}\
table{border-collapse:collapse;margin:1em 0}\
th,td{border:1px solid #3a3d44;padding:5px 10px}\
th{background:#2b2d31}\
img{max-width:100%}\
hr{border:0;border-top:1px solid #3a3d44}\
li>input[type=checkbox]{margin-right:.4em}";

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> crate::paths::TempDir {
        crate::paths::TempDir::new(tag)
    }

    #[test]
    fn ids_from_titles() {
        assert_eq!(id_from_title("Login page v2"), "login-page-v2");
        assert_eq!(id_from_title("  --Chart: Q3!! "), "chart-q3");
        assert_eq!(id_from_title("미리보기"), "preview");
        assert_eq!(id_from_title(&"a".repeat(80)).len(), MAX_ID);
        assert!(valid_id(&id_from_title(&"ab-".repeat(30))));
        assert!(!valid_id("Bad"));
        assert_eq!(Kind::parse("md"), Some(Kind::Markdown));
        assert_eq!(Kind::parse("pdf"), None);
    }

    #[test]
    fn content_previews_replace_and_serve() {
        let t = tmp("preview-content");
        let paths = Paths::new(t.0.join("data"));
        let root = t.0.join("ws");
        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("img/a.png"), b"PNG").unwrap();
        let p = put_content(&paths, "s1", "page", " Page ", Kind::Html, "<p>one</p>").unwrap();
        assert_eq!(p.title, "Page");
        assert_eq!(read(&paths, "s1", &root, "page", "/").unwrap().0, b"<p>one</p>");
        put_content(&paths, "s1", "page", "Page", Kind::Html, "<p>two</p>").unwrap();
        let (body, mime) = read(&paths, "s1", &root, "page", "").unwrap();
        assert_eq!(body, b"<p>two</p>");
        assert!(mime.starts_with("text/html"));
        // Rendered Markdown runs no scripts (raw HTML in it is kept, the policy blocks it).
        let md = markdown_page("t", "# Hi\n<script>alert(1)</script>");
        assert!(md.contains("Content-Security-Policy") && md.contains("default-src 'none'"));
        assert!(!md.contains("script-src"), "no script source is allowed");
        assert!(previews_dir(&paths, "s1").join("page.html").is_file());
        // Relative files come from the session folder, never from outside it.
        let (img, mime) = read(&paths, "s1", &root, "page", "/img/a.png").unwrap();
        assert_eq!((img.as_slice(), mime), (&b"PNG"[..], "image/png"));
        assert!(matches!(read(&paths, "s1", &root, "page", "/../data/x"), Err(PreviewError::NoFile(_))));
        assert!(matches!(read(&paths, "s1", &root, "page", "/nope.css"), Err(PreviewError::NoFile(_))));
        // Another session does not see it.
        assert!(matches!(read(&paths, "s2", &root, "page", ""), Err(PreviewError::NotFound(_))));

        let md = put_content(&paths, "s1", "notes", "Notes <1>", Kind::Markdown, "# Hi\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n").unwrap();
        assert_eq!(md.kind, Kind::Markdown);
        let html = String::from_utf8(read(&paths, "s1", &root, "notes", "").unwrap().0).unwrap();
        assert!(html.contains("<h1>Hi</h1>"), "{html}");
        assert!(html.contains("<table>"));
        assert!(html.contains("checkbox"));
        assert!(html.contains("<title>Notes &lt;1&gt;</title>"));

        assert_eq!(list(&paths, "s1").len(), 2);
        assert!(matches!(
            put_content(&paths, "s1", "Bad id", "x", Kind::Html, ""),
            Err(PreviewError::BadId(_))
        ));
        assert!(matches!(
            put_content(&paths, "s1", "x", "  ", Kind::Html, ""),
            Err(PreviewError::Invalid(_))
        ));
        assert!(remove(&paths, "s1", "page"));
        assert!(matches!(load(&paths, "s1", "page"), Err(PreviewError::NotFound(_))));
    }

    #[test]
    fn file_previews_stay_in_the_session_folder() {
        let t = tmp("preview-file");
        let paths = Paths::new(t.0.join("data"));
        let root = t.0.join("ws");
        std::fs::create_dir_all(root.join("site/css")).unwrap();
        std::fs::write(root.join("site/index.html"), "<link href=\"css/a.css\">").unwrap();
        std::fs::write(root.join("site/css/a.css"), "p{}").unwrap();
        std::fs::write(root.join("README.md"), "# Readme").unwrap();
        std::fs::write(root.join("main.rs"), "fn main(){}").unwrap();
        std::fs::write(t.0.join("secret.html"), "x").unwrap();

        put_file(&paths, "s1", &root, "site", "Site", "site/index.html").unwrap();
        let (b, mime) = read(&paths, "s1", &root, "site", "").unwrap();
        assert_eq!((b.as_slice(), mime), (&b"<link href=\"css/a.css\">"[..], "text/html; charset=utf-8"));
        // Relative to the file's folder.
        assert_eq!(read(&paths, "s1", &root, "site", "/css/a.css").unwrap().0, b"p{}");
        // `..` is refused even when it stays inside the folder.
        assert!(read(&paths, "s1", &root, "site", "/../README.md").is_err());

        // Absolute paths inside the folder are fine; Markdown files are rendered.
        let abs = root.join("README.md").display().to_string();
        put_file(&paths, "s1", &root, "readme", "Readme", &abs).unwrap();
        let html = String::from_utf8(read(&paths, "s1", &root, "readme", "").unwrap().0).unwrap();
        assert!(html.contains("<h1>Readme</h1>"));

        // Outside the folder, missing, or not something a preview shows.
        let out = t.0.join("secret.html").display().to_string();
        assert!(matches!(put_file(&paths, "s1", &root, "x", "X", &out), Err(PreviewError::NoFile(_))));
        assert!(matches!(put_file(&paths, "s1", &root, "x", "X", "../secret.html"), Err(PreviewError::NoFile(_))));
        assert!(matches!(put_file(&paths, "s1", &root, "x", "X", "nope.html"), Err(PreviewError::NoFile(_))));
        assert!(matches!(put_file(&paths, "s1", &root, "x", "X", "main.rs"), Err(PreviewError::Invalid(_))));
        assert!(matches!(put_file(&paths, "s1", &root, "x", "X", ""), Err(PreviewError::Invalid(_))));

        // A file that went away later.
        std::fs::remove_file(root.join("README.md")).unwrap();
        assert!(matches!(read(&paths, "s1", &root, "readme", ""), Err(PreviewError::NoFile(_))));
    }

    #[test]
    fn url_previews() {
        let t = tmp("preview-url");
        let paths = Paths::new(t.0.join("data"));
        let p = put_url(&paths, "s1", "docs", "Docs", " https://example.com/a ").unwrap();
        assert_eq!(p.url.as_deref(), Some("https://example.com/a"));
        assert!(put_url(&paths, "s1", "x", "X", "javascript:alert(1)").is_err());
        assert!(put_url(&paths, "s1", "x", "X", "file:///c:/x").is_err());
        assert!(put_url(&paths, "s1", "x", "X", "https://").is_err());
        assert!(read(&paths, "s1", &t.0, "docs", "").is_err());
    }
}
