//! Widgets (`docs/widgets.md`): the session folder's `.ide/widgets/<id>/`, served to the app
//! (`widget.*`), and the agent's `ide_widget` tool.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ide_core::widget::{self, Manifest, WidgetError};
use ide_core::{Session, SessionId};
use ide_proto::{AgentToolResult, ProtoError, Response, WidgetDataOp, WidgetInfo};

use super::UserCtx;

/// Serializes writes of widget data files.
static DATA_LOCK: Mutex<()> = Mutex::new(());

fn proto(e: WidgetError) -> ProtoError {
    ProtoError::new("widget").arg("detail", e.to_string())
}

fn ok(v: serde_json::Value) -> AgentToolResult {
    AgentToolResult {
        text: serde_json::to_string_pretty(&v).expect("json"),
        is_error: false,
    }
}

fn err(t: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        text: t.into(),
        is_error: true,
    }
}

/// The ide tools a widget may declare: the bridge's.
fn known_tool(t: &str) -> bool {
    crate::bridge::tools().iter().any(|x| x.name == t)
}

/// A manifest's problem the core does not see: tools the bridge does not have.
fn check_tools(m: &Manifest) -> Result<(), String> {
    match m.tools.iter().find(|t| !known_tool(t)) {
        Some(t) => Err(format!("widget.json: `tools` lists `{t}`, which ide does not have")),
        None => Ok(()),
    }
}

impl UserCtx {
    /// The session, its folder and the folder's owner (whose data the session scope uses).
    fn widget_home(&self, session: &str) -> Result<(Session, PathBuf, SessionId), ProtoError> {
        let s = self.resolve_session(session)?;
        let owner = self.folder_owner(&s);
        let root = s.root.clone();
        Ok((s, root, owner))
    }

    /// The root session of `s`'s family.
    fn family_root(&self, s: &Session) -> SessionId {
        let sessions = self.sessions.lock().unwrap();
        let mut cur = s.clone();
        for _ in 0..=ide_core::family::MAX_DEPTH + 1 {
            match cur.parent.as_ref().and_then(|p| sessions.get(p)) {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        cur.id
    }

    /// A usable widget of the session: its manifest (core checks and known tools).
    fn widget_manifest(&self, session: &str, id: &str) -> Result<(Session, SessionId, Manifest), ProtoError> {
        let (s, root, owner) = self.widget_home(session)?;
        let m = widget::load(&root, id).map_err(proto)?;
        check_tools(&m).map_err(|reason| {
            proto(WidgetError::Invalid {
                id: id.to_owned(),
                reason,
            })
        })?;
        Ok((s, owner, m))
    }

    /// `widget.list`.
    pub(crate) fn widget_list(&self, session: &str) -> Result<Vec<WidgetInfo>, ProtoError> {
        let (_, root, owner) = self.widget_home(session)?;
        let grants = widget::grants_file(&self.paths, owner.as_str());
        Ok(widget::list(&root)
            .into_iter()
            .map(|e| {
                let m = e.manifest.and_then(|m| check_tools(&m).map(|_| m));
                match m {
                    Ok(m) => WidgetInfo {
                        granted: widget::granted(&grants, &e.id, &m.tools),
                        id: e.id,
                        name: m.name,
                        description: m.description,
                        tools: m.tools,
                        scope: m.scope.as_str().to_owned(),
                        error: None,
                    },
                    Err(reason) => WidgetInfo {
                        name: e.id.clone(),
                        id: e.id,
                        description: String::new(),
                        tools: Vec::new(),
                        scope: String::new(),
                        granted: false,
                        error: Some(reason),
                    },
                }
            })
            .collect())
    }

    /// `widget.file`.
    pub(crate) fn widget_file(&self, session: &str, id: &str, path: &str) -> Result<Response, ProtoError> {
        let (_, root, _) = self.widget_home(session)?;
        let (data, mime) = widget::read_file(&root, id, path).map_err(proto)?;
        Ok(Response::WidgetFile {
            mime: mime.to_owned(),
            data,
        })
    }

    /// `widget.grant`: the user approved `tools` for the widget (as the dialog showed them).
    pub(crate) fn widget_grant(&self, session: &str, id: &str, tools: &[String]) -> Result<Response, ProtoError> {
        let (_, owner, m) = self.widget_manifest(session, id)?;
        let mut a = m.tools.clone();
        let mut b = tools.to_vec();
        a.sort();
        a.dedup();
        b.sort();
        b.dedup();
        if a != b {
            return Err(ProtoError::new("widget-changed").arg("widget", id));
        }
        widget::grant(&widget::grants_file(&self.paths, owner.as_str()), id, &m.tools).map_err(proto)?;
        self.audit("widget.grant", &format!("{session} {id} {}", m.tools.join(",")));
        Ok(Response::Unit)
    }

    /// `widget.call`: a widget calls an ide tool it declared and the user approved; carried out
    /// like the session agent's own call.
    pub(crate) async fn widget_call(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        tool: &str,
        args: &str,
    ) -> Result<AgentToolResult, ProtoError> {
        let (_, owner, m) = {
            let (me, s, i) = (self.clone(), session.to_owned(), id.to_owned());
            tokio::task::spawn_blocking(move || me.widget_manifest(&s, &i))
                .await
                .map_err(ProtoError::internal)??
        };
        if !m.tools.iter().any(|t| t == tool) {
            return Err(ProtoError::new("widget-not-allowed").arg("tool", tool));
        }
        if !widget::granted(&widget::grants_file(&self.paths, owner.as_str()), id, &m.tools) {
            return Err(ProtoError::new("widget-not-allowed").arg("tool", tool));
        }
        self.audit("widget.call", &format!("{session} {id} {tool}"));
        Ok(self.agent_call(session, tool, args).await)
    }

    /// `widget.data`.
    pub(crate) fn widget_data(&self, session: &str, id: &str, op: WidgetDataOp) -> Result<Response, ProtoError> {
        let (s, owner, m) = self.widget_manifest(session, id)?;
        let root = self.family_root(&s);
        let file = widget::data_file(&self.paths, m.scope, owner.as_str(), root.as_str(), id);
        let out = match op {
            WidgetDataOp::Get { key } => widget::data_get(&file, &key)
                .map_err(proto)?
                .map(|v| v.to_string()),
            WidgetDataOp::Keys => Some(serde_json::Value::from(widget::data_keys(&file).map_err(proto)?).to_string()),
            WidgetDataOp::Set { key, value } => {
                let v: serde_json::Value = serde_json::from_str(&value)
                    .map_err(|e| proto(WidgetError::Data(format!("not JSON: {e}"))))?;
                let _g = DATA_LOCK.lock().unwrap();
                widget::data_put(&file, &key, Some(v)).map_err(proto)?;
                None
            }
            WidgetDataOp::Delete { key } => {
                let _g = DATA_LOCK.lock().unwrap();
                widget::data_put(&file, &key, None).map_err(proto)?;
                None
            }
        };
        Ok(Response::WidgetData(out))
    }

    /// The agent's `ide_widget` tool: `guide` and `list` answered here, `open` / `close`
    /// carried out by an app after the widget was checked.
    pub(crate) async fn widget_tool(self: &Arc<Self>, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let id = args.get("widget").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
        let me = self.clone();
        let s = session.to_owned();
        let action_owned = action.to_owned();
        let id2 = id.clone();
        // Blocking part: the folder, the manifests.
        let pre = tokio::task::spawn_blocking(move || -> Result<Option<AgentToolResult>, AgentToolResult> {
            let (_, root, _) = me.widget_home(&s).map_err(|e| err(format!("session not available: {e}")))?;
            let folder = widget::widgets_dir(&root);
            match action_owned.as_str() {
                "guide" => Ok(Some(AgentToolResult {
                    text: guide(&folder.display().to_string()),
                    is_error: false,
                })),
                "list" => {
                    let list = me.widget_list(&s).map_err(|e| err(e.to_string()))?;
                    Ok(Some(ok(serde_json::json!({
                        "folder": folder,
                        "widgets": list.iter().map(|w| serde_json::json!({
                            "id": w.id,
                            "name": w.name,
                            "description": w.description,
                            "tools": w.tools,
                            "scope": w.scope,
                            "approved_by_user": w.granted,
                            "problem": w.error,
                        })).collect::<Vec<_>>(),
                    }))))
                }
                "open" | "close" => {
                    if id2.is_empty() {
                        return Err(err("`widget` is required"));
                    }
                    if action_owned == "open" {
                        me.widget_manifest(&s, &id2).map_err(|e| {
                            err(match e.get("detail") {
                                Some(d) => d.to_owned(),
                                None => e.to_string(),
                            })
                        })?;
                    }
                    Ok(None)
                }
                other => Err(err(format!("unknown action `{other}` (guide, list, open, close)"))),
            }
        })
        .await;
        match pre {
            Ok(Ok(Some(r))) => return r,
            Ok(Err(r)) => return r,
            Ok(Ok(None)) => {}
            Err(_) => return err("the tool failed"),
        }
        let apps = self.subscriber_count() > 0;
        let args = serde_json::json!({ "action": action, "widget": id }).to_string();
        crate::bridge::ask_app(&self.bridge, session, crate::bridge::TOOL_WIDGET, args, apps, |n| self.notify(n)).await
    }
}

/// `ide_widget {action: guide}`: how to build a widget.
fn guide(folder: &str) -> String {
    format!(
        r#"# ide widgets

A widget is a small web app (HTML, CSS, JavaScript; any framework or none) that ide shows as a
tab in this session's detail area, next to the terminal. Use one when a page shows something
better than text: a board, a form, a chart, a dashboard, a small tool.

## Files
- Folder: `{folder}/<id>/` where `<id>` is 1-40 of a-z, 0-9 and `-` (e.g. `todo-board`).
- `widget.json` (required):
  ```json
  {{
    "name": "Todo board",
    "description": "Tasks of this session",
    "entry": "index.html",
    "tools": ["ide_session"],
    "scope": "session"
  }}
  ```
  - `name`: the tab title. `entry`: the page (default `index.html`), relative to the folder.
  - `tools`: ide tools the page calls (`ide_*`, see your tool list). The user approves them
    before the widget first runs; a changed list asks again. Leave it out if none.
  - `scope`: where `ide.data` lives: `session` (default; this session and the sessions sharing
    its folder), `family` (the parent session and all its children), `global` (every session).
- Everything else in the folder is served as is: `./app.js`, `./style.css`, images. Paths
  outside the folder are not reachable. Files up to 8 MiB.

## The page
- Served from its own origin (`ide-widget://<id>.<session>/`). Relative URLs work. The page
  may load scripts from the web (e.g. React from a CDN as ES modules with exact versions); there
  is no build step, so write plain JS or modules the browser runs as they are.
- ide injects `window.ide` into every HTML page (before the page's own scripts):
  - `await ide.call(tool, args)`: call a declared ide tool; resolves to the result (parsed
    JSON when the tool answers JSON, else text), rejects with an Error on failure.
  - `await ide.data.get(key)`, `ide.data.set(key, value)` (any JSON value),
    `ide.data.delete(key)`, `ide.data.keys()`: the widget's stored data (at most 1 MiB).
  - `ide.widget` = `{{ id, session }}`.
- Keep it compact and readable on a dark background (ide's UI is dark: background #1e1f22,
  text #d4d6db); the tab may be narrow (from ~360px).

## Show it
- `ide_widget {{"action":"open","widget":"<id>"}}` opens the tab (or reloads it after you changed
  files); `close` closes it; `list` shows every widget and its problems (e.g. an invalid
  widget.json). The user can also reopen widgets from the "+" menu of the detail area.
"#
    )
}
