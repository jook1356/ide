//! Widgets (`docs/widgets.md`, Hermes Desk's mods): drafts in a session folder's
//! `.ide/widgets/<id>/`, registered (copied) into `<data>/widgets/` once the user allowed it,
//! then served to every session (`widget.*`); and the agent's `ide_widget` tool.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ide_core::widget::{self, Ask, Class, Manifest, Registry, WidgetError};
use ide_core::{Session, SessionId};
use ide_proto::{AgentToolResult, Notify, ProtoError, Response, WidgetDataOp, WidgetInfo};

use super::UserCtx;

/// Serializes writes of widget data files.
static DATA_LOCK: Mutex<()> = Mutex::new(());
/// Serializes changes of the registry (read, change, save).
static REG_LOCK: Mutex<()> = Mutex::new(());

fn proto(e: WidgetError) -> ProtoError {
    ProtoError::new("widget").arg("detail", e.to_string())
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

/// A `ProtoError`'s text for the agent.
fn detail(e: &ProtoError) -> String {
    e.get("detail").map_or_else(|| e.to_string(), str::to_owned)
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

    /// A registered widget.
    fn widget_class(&self, id: &str) -> Result<Class, ProtoError> {
        Registry::load(&self.paths)
            .classes
            .remove(id)
            .ok_or_else(|| proto(WidgetError::NotFound(id.to_owned())))
    }

    /// A usable draft of the session's folder.
    fn widget_draft(&self, s: &Session, id: &str) -> Result<Manifest, ProtoError> {
        let m = widget::load(&s.root, id).map_err(proto)?;
        check_tools(&m).map_err(|reason| {
            proto(WidgetError::Invalid {
                id: id.to_owned(),
                reason,
            })
        })?;
        Ok(m)
    }

    fn session_title(&self, id: &str) -> String {
        self.resolve_session(id).map(|s| s.title).unwrap_or_default()
    }

    /// `widget.list`.
    pub(crate) fn widget_list(&self) -> Vec<WidgetInfo> {
        let reg = Registry::load(&self.paths);
        reg.classes
            .iter()
            .map(|(id, c)| WidgetInfo {
                id: id.clone(),
                name: c.name.clone(),
                description: c.description.clone(),
                tools: c.tools.clone(),
                scope: c.scope.as_str().to_owned(),
                version: c.version,
                created_by: c.created_by.clone(),
                created_by_title: self.session_title(&c.created_by),
                updated: c.updated,
                open: reg
                    .open
                    .iter()
                    .filter(|t| t.widget == *id)
                    .map(|t| t.session.clone())
                    .collect(),
            })
            .collect()
    }

    /// `widget.file`.
    pub(crate) fn widget_file(&self, session: &str, id: &str, path: &str) -> Result<Response, ProtoError> {
        self.resolve_session(session)?;
        let c = self.widget_class(id)?;
        let (data, mime) = widget::read_file(&widget::source_dir(&self.paths, id), &c.entry, path).map_err(proto)?;
        Ok(Response::WidgetFile {
            mime: mime.to_owned(),
            data,
        })
    }

    /// Register the session's draft (the user allowed it, or nothing needed asking).
    fn widget_do_register(&self, s: &Session, id: &str) -> Result<Class, ProtoError> {
        let c = {
            let _g = REG_LOCK.lock().unwrap();
            let mut reg = Registry::load(&self.paths);
            let c = reg.register(&self.paths, &s.root, id, s.id.as_str()).map_err(proto)?;
            reg.save(&self.paths).map_err(proto)?;
            c
        };
        self.audit(
            "widget.register",
            &format!("{} {id} v{} {}", s.id.as_str(), c.version, c.tools.join(",")),
        );
        self.notify(Notify::WidgetsChanged);
        Ok(c)
    }

    /// `widget.register`: the user allowed registering the draft with `tools` (as the dialog
    /// showed them).
    pub(crate) fn widget_register(&self, session: &str, id: &str, tools: &[String]) -> Result<Response, ProtoError> {
        let s = self.resolve_session(session)?;
        let m = self.widget_draft(&s, id)?;
        if !widget::same_tools(&m.tools, tools) {
            return Err(ProtoError::new("widget-changed").arg("widget", id));
        }
        self.widget_do_register(&s, id)?;
        Ok(Response::Unit)
    }

    /// `widget.delete`.
    pub(crate) fn widget_delete(&self, id: &str) -> Result<Response, ProtoError> {
        let gone = {
            let _g = REG_LOCK.lock().unwrap();
            let mut reg = Registry::load(&self.paths);
            let gone = reg.unregister(&self.paths, id).map_err(proto)?;
            reg.save(&self.paths).map_err(proto)?;
            gone
        };
        if !gone {
            return Err(proto(WidgetError::NotFound(id.to_owned())));
        }
        self.audit("widget.delete", id);
        self.notify(Notify::WidgetsChanged);
        Ok(Response::Unit)
    }

    /// `widget.set_open`.
    pub(crate) fn widget_set_open(&self, session: &str, id: &str, open: bool) -> Result<Response, ProtoError> {
        let s = self.resolve_session(session)?;
        let changed = {
            let _g = REG_LOCK.lock().unwrap();
            let mut reg = Registry::load(&self.paths);
            let changed = reg.set_open(id, s.id.as_str(), open);
            if changed {
                reg.save(&self.paths).map_err(proto)?;
            }
            changed
        };
        if changed {
            self.notify(Notify::WidgetsChanged);
        }
        Ok(Response::Unit)
    }

    /// A session was deleted: its widget tabs are forgotten.
    pub(crate) fn widget_forget_session(&self, session: &str) {
        let _g = REG_LOCK.lock().unwrap();
        let mut reg = Registry::load(&self.paths);
        if reg.forget_session(session) {
            let _ = reg.save(&self.paths);
        }
    }

    /// `widget.call`: a widget calls an ide tool the user allowed when it was registered;
    /// carried out like the session agent's own call.
    pub(crate) async fn widget_call(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        tool: &str,
        args: &str,
    ) -> Result<AgentToolResult, ProtoError> {
        let c = {
            let (me, s, i) = (self.clone(), session.to_owned(), id.to_owned());
            tokio::task::spawn_blocking(move || {
                me.resolve_session(&s)?;
                me.widget_class(&i)
            })
            .await
            .map_err(ProtoError::internal)??
        };
        if !c.tools.iter().any(|t| t == tool) {
            return Err(ProtoError::new("widget-not-allowed").arg("tool", tool));
        }
        self.audit("widget.call", &format!("{session} {id} {tool}"));
        Ok(self.agent_call(session, tool, args).await)
    }

    /// `widget.data`: in the widget's scope, seen from `session`.
    pub(crate) fn widget_data(&self, session: &str, id: &str, op: WidgetDataOp) -> Result<Response, ProtoError> {
        let s = self.resolve_session(session)?;
        let c = self.widget_class(id)?;
        let owner = self.folder_owner(&s);
        let root = self.family_root(&s);
        let file = widget::data_file(&self.paths, c.scope, owner.as_str(), root.as_str(), id);
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

    /// `ide_widget {action: list}`: registered widgets and the session's drafts.
    fn widget_tool_list(&self, s: &Session) -> serde_json::Value {
        let registered = self.widget_list();
        let drafts: Vec<serde_json::Value> = widget::list(&s.root)
            .into_iter()
            .map(|e| {
                let problem = e
                    .manifest
                    .and_then(|m| check_tools(&m))
                    .err();
                serde_json::json!({
                    "id": e.id,
                    "registered": registered.iter().any(|w| w.id == e.id),
                    "problem": problem,
                })
            })
            .collect();
        serde_json::json!({
            "drafts_folder": widget::widgets_dir(&s.root),
            "drafts": drafts,
            "registered": registered.iter().map(|w| serde_json::json!({
                "id": w.id,
                "name": w.name,
                "description": w.description,
                "tools": w.tools,
                "scope": w.scope,
                "version": w.version,
                "registered_by_this_session": w.created_by == s.id.as_str(),
                "open_here": w.open.iter().any(|o| o == s.id.as_str()),
            })).collect::<Vec<_>>(),
        })
    }

    /// The agent's `ide_widget` tool.
    pub(crate) async fn widget_tool(self: &Arc<Self>, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let id = args.get("widget").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
        let me = self.clone();
        let s = session.to_owned();
        let (action2, id2) = (action.clone(), id.clone());
        // What the daemon answers itself (blocking: files, the registry); `Ok(None)`: ask an app
        // with these arguments.
        let pre = tokio::task::spawn_blocking(move || -> Result<Result<AgentToolResult, (serde_json::Value, Option<Duration>)>, AgentToolResult> {
            let s = me.resolve_session(&s).map_err(|e| err(format!("session not available: {e}")))?;
            let need_id = || {
                if id2.is_empty() {
                    Err(err("`widget` is required"))
                } else {
                    Ok(())
                }
            };
            match action2.as_str() {
                "guide" => Ok(Ok(AgentToolResult {
                    text: guide(&widget::widgets_dir(&s.root).display().to_string()),
                    is_error: false,
                    image: None,
                })),
                "list" => Ok(Ok(ok(me.widget_tool_list(&s)))),
                "register" => {
                    need_id()?;
                    let m = me.widget_draft(&s, &id2).map_err(|e| err(detail(&e)))?;
                    let ask = Registry::load(&me.paths).ask_for(&id2, s.id.as_str(), &m);
                    let Some(ask) = ask else {
                        let c = me.widget_do_register(&s, &id2).map_err(|e| err(detail(&e)))?;
                        return Ok(Ok(ok(serde_json::json!({
                            "registered": id2,
                            "version": c.version,
                            "next": "open it with {\"action\":\"open\",\"widget\":\"<id>\"} (reloads an open tab)",
                        }))));
                    };
                    let (kind, by) = match ask {
                        Ask::New => ("new", String::new()),
                        Ask::OtherSession { by } => ("other", me.session_title(&by)),
                        Ask::ToolsChanged => ("tools", String::new()),
                    };
                    Ok(Err((
                        serde_json::json!({
                            "action": "register",
                            "widget": id2,
                            "name": m.name,
                            "description": m.description,
                            "tools": m.tools,
                            "scope": m.scope.as_str(),
                            "ask": kind,
                            "by": by,
                        }),
                        me.ask_wait(),
                    )))
                }
                "unregister" => {
                    need_id()?;
                    let c = me.widget_class(&id2).map_err(|e| err(detail(&e)))?;
                    if c.created_by != s.id.as_str() {
                        return Err(err(
                            "another session registered this widget; the user can delete it on the ide's 위젯 page",
                        ));
                    }
                    me.widget_delete(&id2).map_err(|e| err(detail(&e)))?;
                    Ok(Ok(ok(serde_json::json!({ "unregistered": id2 }))))
                }
                "open" | "close" => {
                    need_id()?;
                    if action2 == "open" {
                        me.widget_class(&id2).map_err(|_| {
                            err(format!(
                                "`{id2}` is not registered: register the draft first ({{\"action\":\"register\",\"widget\":\"{id2}\"}})"
                            ))
                        })?;
                    }
                    Ok(Err((
                        serde_json::json!({ "action": action2, "widget": id2 }),
                        Some(crate::bridge::APP_TIMEOUT),
                    )))
                }
                other => Err(err(format!(
                    "unknown action `{other}` (guide, list, register, unregister, open, close)"
                ))),
            }
        })
        .await;
        let (args, timeout) = match pre {
            Ok(Ok(Ok(r))) | Ok(Err(r)) => return r,
            Ok(Ok(Err(x))) => x,
            Err(_) => return err("the tool failed"),
        };
        crate::bridge::ask_app_within(
            &self.bridge,
            session,
            crate::bridge::TOOL_WIDGET,
            args.to_string(),
            || self.subscriber_count() > 0,
            |n| self.notify(n),
            timeout,
        )
        .await
    }
}

/// `ide_widget {action: guide}`: how to build a widget.
fn guide(folder: &str) -> String {
    format!(
        r#"# ide widgets

A widget is a small web app (HTML, CSS, JavaScript; any framework or none) that ide shows as a
tab in the detail area, next to the terminal. Use one when a page shows something better than
text: a board, a form, a chart, a dashboard, a small tool. Like an app on a phone, a widget is
registered once and then every session can open it (the user finds them on the 위젯 page of
the navigation rail and in the 위젯 button of the status bar).

## Draft, then register
1. Write the draft into `{folder}/<id>/`; `<id>` is 1-40 of a-z, 0-9 and `-` (e.g. `todo-board`).
2. `ide_widget {{"action":"register","widget":"<id>"}}` copies it into ide. A new widget, one
   another session registered, or a changed `tools` list asks the user first (the call waits
   for the answer, up to 5 minutes). Updating your own widget with the same tools does not ask.
3. `ide_widget {{"action":"open","widget":"<id>"}}` opens it here (or reloads the open tab).
   After changing the draft, register again, then open again: the tab shows the registered
   copy, not the draft.

## Files
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
  - `tools`: ide tools the page calls (`ide_*`, see your tool list); the user allows them when
    the widget is registered. Leave it out if none.
  - `scope`: where `ide.data` lives, and so what a tab shows:
    - `session` (default): each session has its own data (sessions sharing a folder share it).
    - `family`: a parent session and all its children share the data.
    - `global`: one data set for every session; the widget is one 공용 tab shown in every
      session.
- Everything else in the folder is served as is: `./app.js`, `./style.css`, images. Paths
  outside the folder are not reachable. Files up to 8 MiB, 64 MiB in all.

## The page
- Served from its own origin (`ide-widget://<id>.<session>/`). Relative URLs work. The page
  may load scripts from the web (e.g. React from a CDN as ES modules with exact versions); there
  is no build step, so write plain JS or modules the browser runs as they are.
- ide injects `window.ide` into every HTML page (before the page's own scripts):
  - `await ide.call(tool, args)`: call a declared ide tool, as the session showing the tab;
    resolves to the result (parsed JSON when the tool answers JSON, else text), rejects with an
    Error on failure.
  - `await ide.data.get(key)`, `ide.data.set(key, value)` (any JSON value),
    `ide.data.delete(key)`, `ide.data.keys()`: the widget's stored data in its scope (at most
    1 MiB).
  - `ide.widget` = `{{ id, session }}`: `session` is the session showing the tab.
- Keep it compact and readable on a dark background (ide's UI is dark: background #1e1f22,
  text #d4d6db); the tab may be narrow (from ~360px).

## Other actions
- `list`: registered widgets (and whether this session registered them / shows them) and this
  session's drafts with their problems (e.g. an invalid widget.json).
- `close` {{widget}}: closes its tab here. `unregister` {{widget}}: removes a widget this session
  registered (its tabs and global data go too).
"#
    )
}
