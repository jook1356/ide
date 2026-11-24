//! Previews (`docs/previews.md`): the agent's `ide_preview` tool and `preview.file`. The
//! daemon checks and stores what a preview shows (`ide_core::preview`: HTML / Markdown text
//! under `<data>/sessions/<id>/previews/`, a file inside the session folder, a web address);
//! an app opens or reloads the preview tab (`Notify::AgentRequest`) and serves its page from
//! `ide-preview://<id>.<session>/` through `preview.file`.

use std::sync::Arc;

use ide_core::preview::{self, Kind, PreviewError};
use ide_proto::{AgentToolResult, ProtoError, Response};

use super::UserCtx;

fn proto(e: PreviewError) -> ProtoError {
    ProtoError::new("preview").arg("detail", e.to_string())
}

fn ok(v: serde_json::Value) -> AgentToolResult {
    AgentToolResult {
        text: serde_json::to_string_pretty(&v).expect("json"),
        is_error: false,
        image: None,
    }
}

fn err(t: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: t.into(),
        is_error: true,
        image: None,
    }
}

/// The page URL of a session's preview (as the app serves it).
pub(crate) fn preview_url(session: &str, id: &str) -> String {
    format!("ide-preview://{id}.{session}/")
}

impl UserCtx {
    /// `preview.file`.
    pub(crate) fn preview_file(&self, session: &str, id: &str, path: &str) -> Result<Response, ProtoError> {
        let s = self.resolve_session(session)?;
        let (data, mime) = preview::read(&self.paths, s.id.as_str(), &s.root, id, path).map_err(proto)?;
        Ok(Response::WidgetFile {
            mime: mime.to_owned(),
            data,
        })
    }

    /// The agent's `ide_preview` tool.
    pub(crate) async fn preview_tool(self: &Arc<Self>, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let me = self.clone();
        let (s, a) = (session.to_owned(), args.clone());
        // What the daemon answers itself (files); `Err(args)`: ask an app with these.
        let pre = tokio::task::spawn_blocking(move || me.preview_pre(&s, &a)).await;
        let (app_args, answer) = match pre {
            Ok(Ok(r)) => return r,
            Ok(Err(x)) => x,
            Err(_) => return err("the tool failed"),
        };
        let apps = self.subscriber_count() > 0;
        let r = crate::bridge::ask_app(
            &self.bridge,
            session,
            crate::bridge::TOOL_PREVIEW,
            app_args.to_string(),
            apps,
            |n| self.notify(n),
        )
        .await;
        if r.is_error {
            return r;
        }
        let mut answer = answer;
        answer["tab"] = serde_json::Value::String(r.text);
        ok(answer)
    }

    /// `ide_preview` checks and files; `Err((app args, answer))` when an app must act.
    #[allow(clippy::result_large_err)]
    fn preview_pre(
        &self,
        session: &str,
        args: &serde_json::Value,
    ) -> Result<AgentToolResult, (serde_json::Value, serde_json::Value)> {
        let s = match self.resolve_session(session) {
            Ok(s) => s,
            Err(e) => return Ok(err(format!("session not available: {e}"))),
        };
        let field = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let title = field("title").trim();
        let id = match field("id").trim() {
            "" => preview::id_from_title(title),
            id => id.to_owned(),
        };
        match field("action") {
            "guide" => return Ok(AgentToolResult {
                text: GUIDE.to_owned(),
                is_error: false,
                image: None,
            }),
            "list" => {
                let list: Vec<serde_json::Value> = preview::list(&self.paths, s.id.as_str())
                    .into_iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id,
                            "title": p.title,
                            "kind": p.kind.as_str(),
                            "file": p.file,
                            "url": p.url,
                        })
                    })
                    .collect();
                return Ok(ok(serde_json::json!({ "previews": list })));
            }
            "close" => {
                if field("id").trim().is_empty() {
                    return Ok(err("`id` is required to close a preview"));
                }
                preview::remove(&self.paths, s.id.as_str(), &id);
                return Err((
                    serde_json::json!({ "action": "close", "preview": id }),
                    serde_json::json!({ "closed": id }),
                ));
            }
            "" | "show" => {}
            other => return Ok(err(format!("unknown action `{other}` (show, guide, list, close)"))),
        }
        let Some(kind) = Kind::parse(field("kind")) else {
            return Ok(err("`kind` must be html, markdown, file or url"));
        };
        let sid = s.id.as_str();
        let r = match kind {
            Kind::Html | Kind::Markdown => {
                let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
                    return Ok(err(format!("`content` is required for a {} preview", kind.as_str())));
                };
                preview::put_content(&self.paths, sid, &id, title, kind, content)
            }
            Kind::File => preview::put_file(&self.paths, sid, &s.root, &id, title, field("path")),
            Kind::Url => {
                let url = [field("url"), field("path"), field("content")]
                    .into_iter()
                    .find(|u| !u.trim().is_empty())
                    .unwrap_or("");
                preview::put_url(&self.paths, sid, &id, title, url)
            }
        };
        let p = match r {
            Ok(p) => p,
            Err(e) => return Ok(err(e.to_string())),
        };
        self.audit("preview.show", &format!("{sid} {} {}", p.id, p.kind.as_str()));
        let url = match &p.url {
            Some(u) => u.clone(),
            None => preview_url(sid, &p.id),
        };
        let mut answer = serde_json::json!({ "id": p.id, "kind": p.kind.as_str() });
        if let Some(f) = &p.file {
            answer["file"] = serde_json::Value::String(f.display().to_string());
        }
        Err((
            serde_json::json!({
                "action": "show",
                "preview": p.id,
                "title": p.title,
                "kind": p.kind.as_str(),
                "url": url,
            }),
            answer,
        ))
    }
}

/// `ide_preview {action: guide}`.
const GUIDE: &str = r#"# ide previews

`ide_preview` shows the user something in a tab of ide's detail area, next to your terminal:
a page you wrote, a rendered Markdown document, a file of this session (an HTML page, an image,
a Markdown file) or a web site. Use it when looking beats reading: a UI mock-up, a chart, a
diagram, a report, a screenshot, the app you are building. It needs no approval: it only shows
things in the user's own ide window.

## Show
`{"title": "...", "kind": "...", ...}` (action `show` is the default):

| kind | with | shows |
|---|---|---|
| `html` | `content`: a whole HTML page (or a fragment) | your page |
| `markdown` | `content`: Markdown (GFM tables, task lists, footnotes) | it rendered, ide's dark style |
| `file` | `path`: relative to the session folder, or absolute inside it | `.html` / `.htm`, `.md`, `.txt`, images (`.png`, `.jpg`, `.gif`, `.webp`, `.svg`, `.ico`, `.bmp`) |
| `url` | `url` (or `path`): `http://` or `https://` | a normal browser tab (e.g. your dev server, `http://localhost:5173`) |

- `id` (optional, 1-40 of a-z, 0-9 and `-`): the same id replaces the preview and reloads its
  tab instead of opening another one. Without it the id comes from the title; pass the same
  title (or id) to update a preview, a new one to show something else beside it.
- The answer says `opened` or `reloaded` and the preview's id.
- `content` and files: up to 8 MiB.

## Pages
- Each preview is served from its own origin (`ide-preview://<id>.<session>/`). It has no ide
  API (unlike widgets) and cannot read other previews.
- Relative URLs load files from the session folder: from the shown file's folder for a `file`
  preview, from the session folder itself for `html` / `markdown` content (e.g.
  `<img src="docs/shot.png">`). Paths outside the session folder are not served; `..` is
  refused.
- Pages may load scripts and styles from the web (e.g. a charting library from a CDN with an
  exact version). There is no build step: write plain HTML / JS / CSS.
- Keep it readable on a dark background (ide's UI: background #1e1f22, text #d4d6db); the tab
  may be narrow (from ~360px).
- A page changed on disk (`file` previews) shows the change when shown again (same id) or when
  the user presses reload.

## Other actions
- `list`: this session's previews (id, title, kind, file / url).
- `close` {id}: closes the preview's tab and forgets it.
- For an app that stays and keeps data, or calls ide tools from the page, build a widget
  instead (`ide_widget` action `guide`).
"#;
