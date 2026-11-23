//! Shared building blocks for providers (composition, not inheritance).

use std::path::Path;

use ide_core::{Error, IoAction, LaunchSpec, McpServer, Result};

use crate::{AgentProvider, HostEnv, LaunchPlan, Skill};

/// Fallback for CLIs without a skills mechanism: list the skills (name, description, path of
/// `SKILL.md`) in the instructions and tell the agent to read a skill's file when it applies.
pub fn skills_as_instructions(plan: &mut LaunchPlan, skills: &[Skill]) {
    if skills.is_empty() {
        return;
    }
    let mut text = String::from(
        "## Skills\nThe following skills are available. Each one is a folder with a SKILL.md \
         file. When a task matches a skill's description, read its SKILL.md first and follow \
         it; files it mentions are relative to the skill's folder.\n",
    );
    for s in skills {
        text.push_str(&format!(
            "- {}: {} (file: {})\n",
            s.name,
            if s.description.is_empty() {
                "-"
            } else {
                &s.description
            },
            s.skill_md().display()
        ));
    }
    plan.extra_instructions.push(text);
}

/// Record that `count` items of `feature` were not passed on.
pub fn unsupported(plan: &mut LaunchPlan, feature: &str, count: usize) {
    if count > 0 {
        plan.notes.push(format!(
            "{feature}: not supported by this provider ({count} item(s) not passed on)"
        ));
    }
}

/// The standard MCP JSON (`{"mcpServers": {name: {type, command, args, env}}}`), as read by
/// Claude Code's `--mcp-config` and plugin `.mcp.json` files.
pub fn mcp_json(servers: &[McpServer]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for s in servers {
        let env: serde_json::Map<String, serde_json::Value> = s
            .env
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        map.insert(
            s.name.clone(),
            serde_json::json!({
                "type": "stdio",
                "command": s.command,
                "args": s.args,
                "env": env,
            }),
        );
    }
    serde_json::json!({ "mcpServers": map })
}

/// A single-line TOML basic string (`"..."`; quotes, backslashes and control characters
/// escaped), safe as a `-c key=<value>` override argument.
pub fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Default [`AgentProvider::finish`]: resolve the program, wrap batch files (Windows npm
/// shims run through `cmd.exe /d /s /c`).
pub fn finish<P: AgentProvider + ?Sized>(
    p: &P,
    plan: LaunchPlan,
    env: &HostEnv,
) -> Result<LaunchSpec> {
    let program = if plan.program == p.program() {
        p.program_path(env)?
    } else {
        ide_core::resolve_program(&plan.program, env)?
    };
    let (program, args) = ide_core::wrap_batch_file(program, plan.args, env)?;
    Ok(LaunchSpec {
        program,
        args,
        cwd: plan.cwd,
        env: plan.env,
        env_remove: plan.env_remove,
    })
}

/// Copy a folder recursively (`.git` folders and symlinks are skipped).
pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    ide_core::ensure_dir(to)?;
    let rd = std::fs::read_dir(from).map_err(|e| Error::io(IoAction::Reading, from, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| Error::io(IoAction::Reading, from, e))?;
        let ty = entry
            .file_type()
            .map_err(|e| Error::io(IoAction::Reading, entry.path(), e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if ty.is_dir() {
            if entry.file_name() != ".git" {
                copy_dir(&src, &dst)?;
            }
        } else if ty.is_file() {
            std::fs::copy(&src, &dst).map_err(|e| Error::io(IoAction::Writing, &dst, e))?;
        }
    }
    Ok(())
}

/// Copy `skills` into `dir`, one folder each named after the skill folder (made unique with
/// `-2`, `-3`...). Returns the folder names.
pub fn copy_skills(skills: &[Skill], dir: &Path) -> Result<Vec<String>> {
    let mut used: Vec<String> = Vec::new();
    for s in skills {
        let base = s
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| s.name.clone());
        let mut folder = base.clone();
        let mut n = 2;
        while used.contains(&folder) {
            folder = format!("{base}-{n}");
            n += 1;
        }
        copy_dir(&s.dir, &dir.join(&folder))?;
        used.push(folder);
    }
    Ok(used)
}

/// `--add-dir <dir>` per directory (Claude Code and Codex use the same flag).
pub fn add_dirs(plan: &mut LaunchPlan, dirs: &[std::path::PathBuf]) {
    for d in dirs {
        plan.arg("--add-dir").arg(d.display().to_string());
    }
}
