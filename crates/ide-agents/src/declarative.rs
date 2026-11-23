//! Declarative providers: an agent CLI described by a TOML file in `<data>/providers/*.toml`
//! instead of code. See `docs/providers/example.toml`.
//!
//! ```toml
//! id = "gemini"                 # slug; must not clash with a built-in provider
//! name = "Gemini CLI"
//! program = "gemini"            # searched on PATH
//! args = []                     # always passed; placeholders allowed (see below)
//! version_args = ["--version"]  # [] = do not probe the version
//! min_version = "0.1.0"         # optional
//! glyph = "✦"                   # optional
//! account_env = "GEMINI_CLI_HOME"   # optional: the account dir goes into this variable;
//!                                   # without it the provider has no accounts
//! login_args = ["auth", "login"]    # optional: login command (program + these args)
//!
//! [env]                         # optional extra env vars; placeholders allowed
//! GEMINI_TELEMETRY = "off"
//!
//! [instructions]                # how configuration instructions (and skills, as a list)
//! mechanism = "flag"            # "flag" | "env" | "file" | "none"
//! flag = "--system-prompt-file" # flag: the instructions file path follows this flag
//! # env = "GEMINI_SYSTEM_MD"    # env: the file path goes into this variable
//! # file = "GEMINI.md"          # file: name of the file in the scratch dir; reference it
//! #                             #       with {instructions_file} in args/env
//!
//! [mcp]
//! mechanism = "unsupported"     # MCP servers are not passed on (noted at launch)
//!
//! [prompt]                      # optional: how `session.send_prompt` submits a prompt
//! mode = "paste"                # "paste" (TUI: bracketed paste + Enter) | "lines" (default)
//! submit_delay_ms = 300         # paste only: pause before Enter
//! ```
//!
//! Placeholders in `args` and `[env]` values: `{account_dir}` (empty without an account),
//! `{scratch_dir}`, `{session_id}`, `{title}`, `{instructions_file}` (the generated file;
//! always written for `mechanism = "file"`, else only with instructions).

use std::collections::BTreeMap;
use std::path::Path;

use ide_core::{Result, validate_slug};
use serde::Deserialize;

use crate::{AccountDir, AgentProvider, Caps, LaunchPlan, Support};

/// How a declarative provider receives instructions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstructionsMechanism {
    /// `<flag> <file>`.
    Flag,
    /// `<env>=<file>`.
    Env,
    /// A file in the scratch dir referenced through `{instructions_file}`.
    File,
    /// Not supported.
    #[default]
    None,
}

/// `[instructions]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstructionsSpec {
    /// Mechanism.
    #[serde(default)]
    pub mechanism: InstructionsMechanism,
    /// For `flag`.
    pub flag: Option<String>,
    /// For `env`.
    pub env: Option<String>,
    /// For `file` (default `instructions.md`).
    pub file: Option<String>,
}

/// `[mcp]` table: only `"unsupported"` for now.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpSpec {
    /// Must be `"unsupported"`.
    #[serde(default = "unsupported")]
    pub mechanism: String,
}

impl Default for McpSpec {
    fn default() -> Self {
        McpSpec {
            mechanism: unsupported(),
        }
    }
}

fn unsupported() -> String {
    "unsupported".into()
}

/// A provider file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarativeSpec {
    /// Provider id (slug).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Program.
    pub program: String,
    /// Arguments always passed.
    #[serde(default)]
    pub args: Vec<String>,
    /// Version probe arguments.
    #[serde(default = "default_version_args")]
    pub version_args: Vec<String>,
    /// Oldest supported version.
    pub min_version: Option<String>,
    /// Icon.
    pub glyph: Option<String>,
    /// Env var receiving the account dir.
    pub account_env: Option<String>,
    /// Login command arguments.
    pub login_args: Option<Vec<String>>,
    /// Extra env vars.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Instructions mechanism.
    #[serde(default)]
    pub instructions: InstructionsSpec,
    /// MCP mechanism.
    #[serde(default)]
    pub mcp: McpSpec,
    /// How the CLI takes a prompt (`[prompt] mode = "paste"` (+ `submit_delay_ms`) for a TUI
    /// with bracketed paste, `"lines"` = typed lines, the default).
    #[serde(default)]
    pub prompt: Option<crate::PromptInput>,
}

fn default_version_args() -> Vec<String> {
    vec!["--version".into()]
}

impl DeclarativeSpec {
    /// Parse and validate a provider file's text.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let spec: DeclarativeSpec = toml::from_str(text).map_err(|e| e.to_string())?;
        validate_slug(&spec.id).map_err(|e| e.to_string())?;
        if spec.name.trim().is_empty() || spec.program.trim().is_empty() {
            return Err("`name` and `program` must not be empty".into());
        }
        if spec.mcp.mechanism != "unsupported" {
            return Err(format!(
                "mcp.mechanism {:?} is not supported (only \"unsupported\")",
                spec.mcp.mechanism
            ));
        }
        let i = &spec.instructions;
        match i.mechanism {
            InstructionsMechanism::Flag if i.flag.as_deref().is_none_or(str::is_empty) => {
                return Err("instructions.mechanism = \"flag\" needs `flag`".into());
            }
            InstructionsMechanism::Env if i.env.as_deref().is_none_or(str::is_empty) => {
                return Err("instructions.mechanism = \"env\" needs `env`".into());
            }
            _ => {}
        }
        if let Some(f) = &i.file
            && (f.is_empty() || f.contains(['/', '\\']) || f == "." || f == "..")
        {
            return Err("instructions.file must be a plain file name".into());
        }
        Ok(spec)
    }
}

/// A provider described by a [`DeclarativeSpec`].
#[derive(Debug, Clone)]
pub struct DeclarativeProvider {
    spec: DeclarativeSpec,
}

impl DeclarativeProvider {
    /// Wrap a parsed spec.
    pub fn new(spec: DeclarativeSpec) -> Self {
        Self { spec }
    }

    /// Read and parse a provider file.
    pub fn from_file(path: &Path) -> std::result::Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        DeclarativeSpec::parse(&text).map(Self::new)
    }

    /// The spec.
    pub fn spec(&self) -> &DeclarativeSpec {
        &self.spec
    }

    fn instructions_file_name(&self) -> &str {
        self.spec
            .instructions
            .file
            .as_deref()
            .unwrap_or("instructions.md")
    }

    fn expand(&self, s: &str, plan: &LaunchPlan, account_dir: &str) -> String {
        let instr = plan.scratch.join(self.instructions_file_name());
        s.replace("{account_dir}", account_dir)
            .replace("{scratch_dir}", &plan.scratch.display().to_string())
            .replace("{session_id}", &plan.session_id)
            .replace("{title}", &plan.title)
            .replace("{instructions_file}", &instr.display().to_string())
    }
}

impl AgentProvider for DeclarativeProvider {
    fn id(&self) -> &str {
        &self.spec.id
    }
    fn display_name(&self) -> &str {
        &self.spec.name
    }
    fn glyph(&self) -> &str {
        self.spec.glyph.as_deref().unwrap_or("●")
    }
    fn program(&self) -> &str {
        &self.spec.program
    }
    fn version_args(&self) -> Vec<String> {
        self.spec.version_args.clone()
    }
    fn min_version(&self) -> Option<&str> {
        self.spec.min_version.as_deref()
    }
    fn prompt_input(&self) -> crate::PromptInput {
        self.spec.prompt.unwrap_or(crate::PromptInput::Lines)
    }
    fn capabilities(&self) -> Caps {
        let instructions = match self.spec.instructions.mechanism {
            InstructionsMechanism::None => Support::Unsupported,
            _ => Support::Native,
        };
        let skills = if instructions == Support::Native {
            Support::Fallback
        } else {
            Support::Unsupported
        };
        Caps {
            accounts: self.spec.account_env.is_some(),
            login: self.spec.account_env.is_some() && self.spec.login_args.is_some(),
            skills,
            mcp: Support::Unsupported,
            instructions,
            hooks: Support::Unsupported,
            resume: false,
            session_name: false,
            integration: false,
            original_repo: crate::ModeSupport {
                skills,
                mcp: Support::Unsupported,
                instructions,
            },
        }
    }
    fn account_env(&self, acct: &AccountDir) -> Vec<(String, String)> {
        self.spec
            .account_env
            .iter()
            .map(|k| (k.clone(), acct.dir.display().to_string()))
            .collect()
    }
    fn login_args(&self) -> Option<Vec<String>> {
        self.spec.account_env.as_ref()?;
        self.spec.login_args.clone()
    }

    fn begin(&self, plan: &mut LaunchPlan, acct: Option<&AccountDir>) {
        let account_dir = acct
            .map(|a| a.dir.display().to_string())
            .unwrap_or_default();
        if let Some(a) = acct {
            plan.env.extend(self.account_env(a));
        }
        if self.spec.instructions.mechanism == InstructionsMechanism::File {
            // Always present so `{instructions_file}` references stay valid.
            let _ = plan.write_file(self.instructions_file_name(), b"");
        }
        let args: Vec<String> = self
            .spec
            .args
            .iter()
            .map(|a| self.expand(a, plan, &account_dir))
            .collect();
        plan.args.extend(args);
        let env: Vec<(String, String)> = self
            .spec
            .env
            .iter()
            .map(|(k, v)| (k.clone(), self.expand(v, plan, &account_dir)))
            .collect();
        for (k, v) in env {
            plan.set_env(&k, v);
        }
    }

    fn apply_instructions(&self, plan: &mut LaunchPlan, text: &str) -> Result<()> {
        let i = &self.spec.instructions;
        if i.mechanism == InstructionsMechanism::None {
            crate::fallback::unsupported(plan, "instructions", 1);
            return Ok(());
        }
        let path = plan.write_file(self.instructions_file_name(), text.as_bytes())?;
        match i.mechanism {
            InstructionsMechanism::Flag => {
                let flag = i.flag.clone().unwrap_or_default();
                plan.arg(flag).arg(path.display().to_string());
            }
            InstructionsMechanism::Env => {
                let key = i.env.clone().unwrap_or_default();
                plan.set_env(&key, path.display().to_string());
            }
            InstructionsMechanism::File | InstructionsMechanism::None => {}
        }
        Ok(())
    }
}
