//! Widgets (`docs/widgets.md`, Hermes Desk's mods): drafts in a session folder's
//! `.meninx/widgets/<id>/`, registered (copied) into `<data>/widgets/` once the user allowed it,
//! then served to every session (`widget.*`); and the agent's `meninx_widget` tool.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meninx_core::widget::{self, Ask, Class, Manifest, Policy, Registry, WidgetError};
use meninx_core::{Session, SessionId};
use meninx_proto::{AgentToolResult, Notify, ProtoError, Response, WidgetDataOp, WidgetInfo};

use super::UserCtx;

/// Serializes writes of widget data files.
static DATA_LOCK: Mutex<()> = Mutex::new(());
/// Serializes changes of the registry (read, change, save).
static REG_LOCK: Mutex<()> = Mutex::new(());
/// Serializes changes of the block policy.
static POLICY_LOCK: Mutex<()> = Mutex::new(());

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

/// What a widget may declare: the bridge's tools, the host features, `*` (everything).
fn known_tool(t: &str) -> bool {
    t == widget::ALL || widget::FEATURES.contains(&t) || crate::bridge::tools().iter().any(|x| x.name == t)
}

/// A manifest's problem the core does not see: tools the bridge does not have.
fn check_tools(m: &Manifest) -> Result<(), String> {
    match m.tools.iter().find(|t| !known_tool(t)) {
        Some(t) => Err(format!("widget.json: `tools` lists `{t}`, which Meninx does not have")),
        None => Ok(()),
    }
}

impl UserCtx {
    /// The root session of `s`'s family.
    fn family_root(&self, s: &Session) -> SessionId {
        let sessions = self.sessions.lock().unwrap();
        let mut cur = s.clone();
        for _ in 0..=meninx_core::family::MAX_DEPTH + 1 {
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
        let policy = Policy::load(&self.paths);
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
                blocked: policy.blocked(id),
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
        {
            let _g = POLICY_LOCK.lock().unwrap();
            let mut p = Policy::load(&self.paths);
            if p.forget(id) {
                let _ = p.save(&self.paths);
            }
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

    /// `widget.call`: a widget calls a Meninx tool the user allowed when it was registered;
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
        self.widget_may(&c, id, tool)?;
        self.audit("widget.call", &format!("{session} {id} {tool}"));
        Ok(self.agent_call(session, tool, args).await)
    }

    /// May widget `id` (registered as `c`) use `name` (a tool or a feature)? Declared, and not
    /// blocked by the user.
    fn widget_may(&self, c: &Class, id: &str, name: &str) -> Result<(), ProtoError> {
        if !widget::grants(&c.tools, name) {
            return Err(ProtoError::new("widget-not-allowed").arg("tool", name));
        }
        if Policy::load(&self.paths).blocks(id, name) {
            return Err(ProtoError::new("widget-blocked").arg("tool", name));
        }
        Ok(())
    }

    /// `widget.host`: a widget uses a Meninx feature directly (`widget_host.rs`).
    pub(crate) async fn widget_host(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        method: &str,
        args: &str,
    ) -> Result<AgentToolResult, ProtoError> {
        let Some(feature) = widget::feature_of(method) else {
            return Ok(err(format!(
                "unknown method `{method}` (exec, fs.read/write/list/stat/delete/mkdir,                  term.open/list/read/write/stop/rerun/remove, sessions.list/get)"
            )));
        };
        let c = {
            let (me, s, i) = (self.clone(), session.to_owned(), id.to_owned());
            tokio::task::spawn_blocking(move || {
                me.resolve_session(&s)?;
                me.widget_class(&i)
            })
            .await
            .map_err(ProtoError::internal)??
        };
        self.widget_may(&c, id, feature)?;
        if method != "term.list" && method != "term.read" && method != "fs.read" && method != "fs.stat" && method != "fs.list" {
            self.audit("widget.host", &format!("{session} {id} {method}"));
        }
        Ok(self.host_call(session, method, args).await)
    }

    /// `widget.set_blocked`.
    pub(crate) fn widget_set_blocked(&self, id: &str, name: &str, blocked: bool) -> Result<Response, ProtoError> {
        if !known_tool(name) {
            return Err(proto(WidgetError::Data(format!("unknown tool or feature `{name}`"))));
        }
        let changed = {
            let _g = POLICY_LOCK.lock().unwrap();
            let mut p = Policy::load(&self.paths);
            let changed = p.set(id, name, blocked);
            if changed {
                p.save(&self.paths).map_err(proto)?;
            }
            changed
        };
        if changed {
            self.audit("widget.set_blocked", &format!("{id} {name} {blocked}"));
            self.notify(Notify::WidgetsChanged);
        }
        Ok(Response::Unit)
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

    /// `meninx_widget {action: list}`: registered widgets and the session's drafts.
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

    /// The agent's `meninx_widget` tool.
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
                            "another session registered this widget; the user can delete it on Meninx's 위젯 page",
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

/// `meninx_widget {action: guide}`: how to build a widget.
fn guide(folder: &str) -> String {
    format!(
        r#"# Meninx widgets

A widget is a small web app (HTML, CSS, JavaScript; any framework or none) that Meninx shows as a
tab in the detail area, next to the terminal. Use one when a page shows something better than
text: a board, a form, a chart, a dashboard, a small tool. Like an app on a phone, a widget is
registered once and then every session can open it (the user finds them on the 위젯 page of
the navigation rail and in the 위젯 button of the status bar).

## Draft, then register
1. Write the draft into `{folder}/<id>/`; `<id>` is 1-40 of a-z, 0-9 and `-` (e.g. `todo-board`).
2. `meninx_widget {{"action":"register","widget":"<id>"}}` copies it into Meninx. A new widget, one
   another session registered, or a changed `tools` list asks the user first (the call waits
   for the answer, up to 5 minutes). Updating your own widget with the same tools does not ask.
3. `meninx_widget {{"action":"open","widget":"<id>"}}` opens it here (or reloads the open tab).
   After changing the draft, register again, then open again: the tab shows the registered
   copy, not the draft.

## Files
- `widget.json` (required):
  ```json
  {{
    "name": "Todo board",
    "description": "Tasks of this session",
    "entry": "index.html",
    "tools": ["meninx_session"],
    "scope": "session"
  }}
  ```
  - `name`: the tab title. `entry`: the page (default `index.html`), relative to the folder.
  - `tools`: what the page uses; the user allows them when the widget is registered. Leave it
    out if none. Entries: Meninx tools (`meninx_*`, see your tool list) for `meninx.call`; the features
    `exec`, `fs`, `terminal`, `sessions` of the host API (below); or `*` for everything.
  - `scope`: where `meninx.data` lives, and so what a tab shows:
    - `session` (default): each session has its own data (sessions sharing a folder share it).
    - `family`: a parent session and all its children share the data.
    - `global`: one data set for every session; the widget is one 공용 tab shown in every
      session.
- Everything else in the folder is served as is: `./app.js`, `./style.css`, images. Paths
  outside the folder are not reachable. Files up to 8 MiB, 64 MiB in all.

## The page
- Served from its own origin (`meninx-widget://<id>.<session>/`). Relative URLs work. The page
  may load scripts from the web (e.g. React from a CDN as ES modules with exact versions); there
  is no build step, so write plain JS or modules the browser runs as they are.
- Meninx injects `window.meninx` into every HTML page (before the page's own scripts):
  - `await meninx.call(tool, args)`: call a declared Meninx tool, as the session showing the tab;
    resolves to the result (parsed JSON when the tool answers JSON, else text), rejects with an
    Error on failure.
  - `await meninx.data.get(key)`, `meninx.data.set(key, value)` (any JSON value),
    `meninx.data.delete(key)`, `meninx.data.keys()`: the widget's stored data in its scope (at most
    1 MiB).
  - `meninx.widget` = `{{ id, session }}`: `session` is the session showing the tab.
  - `await meninx.context()` -> `{{ selected, session }}`: `selected` is the session the user has
    selected now (a `global` widget's one tab is shown in every session: follow it by polling).
  - `await meninx.openBrowser(url, {{session}}?)`: opens an http(s) page in Meninx's browser, in
    `session` (default: the selected one); needs `meninx_browser_open` in `tools`.
- Host API (the widget runs as the user, like a script of theirs; relative paths are under
  the showing session's folder; results are JSON, errors reject):
  - `exec`: `await meninx.exec({{program, args, cwd, stdin, env, timeout_ms}})` or
    `meninx.exec({{command}})` (a command line for `cmd /c` / `sh -c`) -> `{{code, stdout, stderr,
    timed_out, truncated, ms}}`. Default timeout 120 s, at most 3600 s. No console window.
  - `fs`: `meninx.fs.read(path, {{encoding:'base64'}}?)` -> `{{text | base64, size}}`,
    `meninx.fs.readText(path)`, `meninx.fs.write(path, text, {{append, mkdirs}}?)` (whole files are
    replaced atomically; `{{base64}}` in the options for bytes), `meninx.fs.list(path)` ->
    `{{entries:[{{name, dir, size, modified}}]}}`, `meninx.fs.stat(path)` -> `{{exists, dir, size,
    modified}}`, `meninx.fs.delete(path, {{recursive}}?)`, `meninx.fs.mkdir(path)`.
  - `terminal` (user terminals of the terminal panel; output stays visible to the user):
    `meninx.term.open({{command, cwd, title, shell, session}})` -> `{{id, pid, ...}}`,
    `meninx.term.list({{session}})` (`"*"`: every session) -> `{{terminals:[{{id, session, title,
    command, folder, state, running, pid, ports:[{{port, process}}]}}]}}`,
    `meninx.term.read(id, lines)` -> `{{text, state, running}}`, `meninx.term.write(id, text, keys)`,
    `meninx.term.stop(id)`, `meninx.term.rerun(id)`, `meninx.term.remove(id)`.
  - `sessions`: `meninx.sessions.list()` -> `[{{id, title, folder, parent, shared, archived,
    repositories:[{{name, repo, folder, branch, base}}]}}]`, `meninx.sessions.get(id?)`.
  - `meninx.host(method, args)` is the generic form (`'fs.read'`, `'term.open'`, ...).
  - The user can block any tool or feature per widget on the 위젯 page (calls then fail with
    "blocked").
- Keep it compact and readable on a dark background (Meninx's UI is dark: background #1e1f22,
  text #d4d6db); the tab may be narrow (from ~360px).

## Other actions
- `list`: registered widgets (and whether this session registered them / shows them) and this
  session's drafts with their problems (e.g. an invalid widget.json).
- `close` {{widget}}: closes its tab here. `unregister` {{widget}}: removes a widget this session
  registered (its tabs and global data go too).
"#
    )
}
