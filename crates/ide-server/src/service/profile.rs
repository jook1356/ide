//! Profile tools (`docs/profile-editing.md`): a session's agent reads and changes the profile
//! (configuration) it was started with through `ide_profile_get` / `ide_profile_edit`. Edits
//! are applied at once under the configuration store's lock, checked against the files as they
//! are then (`ide_core::profile_files`), recorded in the profile's history and announced to
//! the user (`Notify::ConfigurationEdited`). Also `configuration.history` / `.restore`.

use std::sync::Arc;

use ide_core::profile_files::{EditError, EditOp, INSTRUCTIONS_FILE};
use ide_core::{ChangeNote, ChangeSource, HistoryEntry};
use ide_proto::{AgentToolResult, ConfigurationChange, Notify, ProtoError};

use super::UserCtx;
use crate::bridge::{TOOL_PROFILE_EDIT, TOOL_PROFILE_GET};

/// Most operations in one `ide_profile_edit` call.
const MAX_OPS: usize = 50;

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

fn str_arg<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// The operations of an `ide_profile_edit` call.
fn parse_ops(args: &serde_json::Value) -> Result<Vec<EditOp>, String> {
    let ops = args
        .get("ops")
        .and_then(|o| o.as_array())
        .filter(|o| !o.is_empty())
        .ok_or("`ops` is required: a list of {op, path, ...}")?;
    if ops.len() > MAX_OPS {
        return Err(format!("at most {MAX_OPS} operations per call"));
    }
    ops.iter()
        .enumerate()
        .map(|(i, o)| {
            let n = i + 1;
            let path = str_arg(o, "path")
                .filter(|p| !p.trim().is_empty())
                .ok_or(format!("operation {n}: `path` is required"))?
                .to_owned();
            let need = |key: &str| {
                str_arg(o, key)
                    .map(str::to_owned)
                    .ok_or(format!("operation {n} ({path}): `{key}` is required"))
            };
            match str_arg(o, "op") {
                Some("patch") => Ok(EditOp::Patch {
                    old: need("old")?,
                    new: need("new")?,
                    path: path.clone(),
                }),
                Some("write") => Ok(EditOp::Write {
                    content: need("content")?,
                    rev: str_arg(o, "rev").filter(|r| !r.trim().is_empty()).map(str::to_owned),
                    path: path.clone(),
                }),
                Some("delete") => Ok(EditOp::Delete {
                    rev: need("rev")?,
                    path: path.clone(),
                }),
                _ => Err(format!("operation {n}: `op` is patch, write or delete")),
            }
        })
        .collect()
}

fn change_info(e: HistoryEntry) -> ConfigurationChange {
    ConfigurationChange {
        id: e.id,
        time: e.time,
        source: e.source.id().to_owned(),
        summary: e.summary,
        session: e.session,
        session_title: e.session_title,
        files: e.files,
    }
}

impl UserCtx {
    /// The profile tools; `None` = not one of them.
    pub(crate) async fn profile_tool(
        self: &Arc<Self>,
        session: &str,
        tool: &str,
        args: &str,
    ) -> Option<AgentToolResult> {
        if tool != TOOL_PROFILE_GET && tool != TOOL_PROFILE_EDIT {
            return None;
        }
        let args: serde_json::Value =
            match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
                Ok(v) => v,
                Err(e) => return Some(err(format!("invalid arguments: {e}"))),
            };
        let (me, session, tool) = (self.clone(), session.to_owned(), tool.to_owned());
        // Blocking: file reads and git commits under the store's lock.
        let r = tokio::task::spawn_blocking(move || {
            if tool == TOOL_PROFILE_GET {
                me.profile_get(&session, &args)
            } else {
                me.profile_edit(&session, &args)
            }
        })
        .await;
        Some(r.unwrap_or_else(|_| err("the tool failed")))
    }

    /// `ide_profile_get`.
    fn profile_get(&self, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let s = match self.resolve_session(session) {
            Ok(s) => s,
            Err(e) => return err(format!("session not available: {e}")),
        };
        let store = self.configs.lock().unwrap();
        let Some(c) = store.get(&s.configuration) else {
            return err(format!("this session's profile `{}` no longer exists", s.configuration));
        };
        let dir = match store.folder(&c.slug) {
            Ok(d) => d,
            Err(e) => return err(e.to_string()),
        };
        if let Some(path) = str_arg(args, "path").filter(|p| !p.trim().is_empty()) {
            return match ide_core::profile_files::read(&dir, path) {
                Ok((text, rev)) => ok(serde_json::json!({
                    "path": path,
                    "rev": rev,
                    "content": text,
                })),
                Err(e) => err(e.to_string()),
            };
        }
        let files: Vec<serde_json::Value> = ide_core::profile_files::list(&dir)
            .into_iter()
            .map(|f| serde_json::json!({ "path": f.path, "rev": f.rev, "size": f.size }))
            .collect();
        let instructions = ide_core::profile_files::read(&dir, INSTRUCTIONS_FILE)
            .map(|(text, rev)| serde_json::json!({ "rev": rev, "text": text }))
            .unwrap_or(serde_json::Value::Null);
        let skills: Vec<serde_json::Value> = store
            .skill_list(&c.slug)
            .into_iter()
            .map(|k| {
                serde_json::json!({
                    "name": k.name,
                    "title": k.title,
                    "description": k.description,
                    "enabled": k.enabled,
                })
            })
            .collect();
        let mcp: Vec<&str> = c.mcp_servers.iter().map(|m| m.name.as_str()).collect();
        ok(serde_json::json!({
            "profile": { "slug": c.slug, "name": c.name },
            "instructions": instructions,
            "skills": skills,
            "mcp_servers": mcp,
            "files": files,
            "note": "Changes go through ide_profile_edit and reach sessions when they start next.",
        }))
    }

    /// `ide_profile_edit`.
    fn profile_edit(&self, session: &str, args: &serde_json::Value) -> AgentToolResult {
        let Some(reason) = str_arg(args, "reason").map(str::trim).filter(|r| !r.is_empty()) else {
            return err("`reason` is required: one line saying what you changed and why");
        };
        let ops = match parse_ops(args) {
            Ok(o) => o,
            Err(e) => return err(e),
        };
        let s = match self.resolve_session(session) {
            Ok(s) => s,
            Err(e) => return err(format!("session not available: {e}")),
        };
        let note = ChangeNote {
            source: ChangeSource::Agent,
            summary: reason.to_owned(),
            session: Some(s.id.to_string()),
            session_title: Some(s.title.clone()),
        };
        let (changed, name) = {
            let mut store = self.configs.lock().unwrap();
            let Some(name) = store.get(&s.configuration).map(|c| c.name.clone()) else {
                return err(format!("this session's profile `{}` no longer exists", s.configuration));
            };
            match store.edit_files(&s.configuration, &ops, &note) {
                Ok(Ok(changed)) => (changed, name),
                Ok(Err(e)) => return err(edit_error_text(&e)),
                Err(e) => return err(e.to_string()),
            }
        };
        let slug = s.configuration.clone();
        let files: Vec<String> = changed.iter().map(|c| c.path.clone()).collect();
        self.audit("profile.edit", &format!("{slug}: {}", files.join(", ")));
        self.configuration_changed(&slug);
        self.notify(Notify::ConfigurationEdited {
            slug,
            name: name.clone(),
            session: s.id.to_string(),
            session_title: s.title.clone(),
            summary: reason.to_owned(),
            files,
        });
        let out: Vec<serde_json::Value> = changed
            .into_iter()
            .map(|c| match c.rev {
                Some(rev) => serde_json::json!({ "path": c.path, "rev": rev }),
                None => serde_json::json!({ "path": c.path, "deleted": true }),
            })
            .collect();
        ok(serde_json::json!({
            "profile": name,
            "changed": out,
            "note": "Saved. Sessions using this profile get it when they start next; the user can undo it in the profile's history.",
        }))
    }

    /// `configuration.history`.
    pub(crate) fn configuration_history(
        &self,
        slug: &str,
        limit: u32,
    ) -> Result<Vec<ConfigurationChange>, ProtoError> {
        let store = self.configs.lock().unwrap();
        let log = store
            .history(slug, limit.clamp(1, 500) as usize)
            .map_err(|e| super::core_error(&e))?;
        Ok(log.into_iter().map(change_info).collect())
    }

    /// `configuration.restore`.
    pub(crate) fn configuration_restore(
        &self,
        slug: &str,
        id: &str,
    ) -> Result<ide_proto::ConfigurationInfo, ProtoError> {
        let c = {
            let mut store = self.configs.lock().unwrap();
            let short: String = id.chars().take(7).collect();
            let note = ChangeNote::new(ChangeSource::Restore, format!("Restore version {short}"));
            store.restore(slug, id, &note).map_err(|e| super::core_error(&e))?;
            self.configuration_of(&store, slug)?
        };
        self.audit("configuration.restore", &format!("{slug}@{id}"));
        self.configuration_changed(slug);
        Ok(c)
    }
}

/// What the agent reads when an edit was refused.
fn edit_error_text(e: &EditError) -> String {
    format!("Nothing was changed. {e}")
}
