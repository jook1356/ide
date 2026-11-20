# Agent providers

Status: agreed direction; implementation in progress. Verified CLI facts are marked (v) with the
version checked; everything else must be verified during implementation.

## Goal
Any agent CLI (Claude Code, Codex, later Gemini CLI, opencode, ...) plugs into one common contract.
The IDE only deals with provider-agnostic data (accounts, configurations: skills, MCP servers,
instructions, hooks); each provider translates that into its own CLI flags, env vars and files.

## Concepts
| Concept | What it is | Stored |
|---|---|---|
| **Provider** | An agent CLI implementation (`claude`, `codex`, declarative ones). Code (`impl AgentProvider`) or a TOML file. | server: registry |
| **Account** | One login for one provider (credentials only). E.g. "Claude - work", "Codex - personal". | server: `accounts/<provider>/<slug>/` (the CLI's own config home for that login) |
| **Configuration** | Provider-agnostic bundle: skills, MCP servers, instructions, hook policy. Replaces today's "profile". | server: `configs/<slug>/` |
| **Session** | provider + account + configuration + repo/worktree | server: `sessions.json` |

Today's `Profile` (one Claude dir + one Codex home) migrates to: one Claude account + one Codex
account (same dirs, no copying) + one configuration with the same name.

## The trait (server side, crate `ide-agents`)
```rust
pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &'static str;                 // "claude"
    fn display_name(&self) -> &str;
    fn detect(&self, env: &HostEnv) -> Detection; // installed? path, version, min-version check
    fn capabilities(&self) -> Caps;               // skills, mcp, instructions, hooks, resume, session_name

    // Accounts
    fn account_env(&self, acct: &AccountDir) -> Vec<(String, String)>;   // e.g. CLAUDE_CONFIG_DIR
    fn login_command(&self, acct: &AccountDir) -> Option<LaunchSpec>;   // run in a terminal tab
    fn account_status(&self, acct: &AccountDir) -> AccountStatus;      // logged in? (best effort, no network)

    // Session assembly: each step has a default (fallback) implementation
    fn apply_skills(&self, plan: &mut LaunchPlan, skills: &[Skill]) { fallback::skills_as_instructions(plan, skills) }
    fn apply_mcp(&self, plan: &mut LaunchPlan, servers: &[McpServer]);
    fn apply_instructions(&self, plan: &mut LaunchPlan, text: &str);
    fn apply_hooks(&self, plan: &mut LaunchPlan, hooks: &HookTargets) {}  // status: working/idle/needs-input
    fn finish(&self, plan: LaunchPlan) -> LaunchSpec;                     // program, args, env, cwd
}
```
- `LaunchPlan` collects args/env plus a per-session **scratch dir** (`sessions/<id>/agent/`) for
  generated files (MCP JSON, instruction file, plugin dirs). Providers never write into the worktree.
- Shared helpers live in plain functions/structs (composition), e.g. `fallback::*`, `mcp_json()`.
- Registry: `Providers::builtin()` + declarative providers loaded from `providers/*.toml`.

## Injection rules
1. **Never pollute the repo.** Inject through flags, env vars and the session scratch dir. If a CLI
   only reads a file inside the worktree, add it to `.git/info/exclude` (never `.gitignore`).
2. **Accounts are not modified** except by the CLI's own login flow. Configuration is layered on
   top per session, so one configuration works with any account.
3. **Unsupported capability -> documented fallback** (e.g. skills become instructions) or the UI
   disables it; never silently drop.
4. **Provider-specific details stay inside the provider.** No `match provider` outside `ide-agents`.

## Provider mapping
| Common | Claude Code (v 2.1.318) | Codex (v 0.160.1) |
|---|---|---|
| Account | `CLAUDE_CONFIG_DIR=<account dir>` | `CODEX_HOME=<account dir>` |
| Login | `claude` then `/login` in a terminal tab (verify a direct login command) | `codex login` |
| MCP | (v) `--mcp-config <file>` (+ `--strict-mcp-config` optional) | (v) `-c mcp_servers.<name>.*=...` overrides |
| Instructions | (v) `--append-system-prompt-file <file>` | `-c` key for developer/extra instructions (verify name) |
| Skills | (v) `--plugin-dir <generated plugin with skills/>` | skills support exists (feature flags `skill_*`); verify location/override |
| Hooks (status) | (v) `--settings <json>` with hooks | (v) `hooks` feature stable; verify config shape |
| Session name | (v) `-n/--name`, (v) `--session-id <uuid>` | — |
| Daemon | — | (v) `--no-daemon` avoids clashing with the Codex desktop app's daemon |

## UI
- Settings -> **Accounts**: per provider, list accounts with status (logged in / not), add (creates
  the account dir and opens the login command in a terminal tab), rename, remove.
- Settings -> **Configurations**: skills (folders), MCP servers (name, command, args, env), instructions
  (text), per configuration.
- New session: provider (only detected ones), account (of that provider), configuration, repo.
- Sidebar rows show the provider icon and account name.

## Phases
1. `ide-agents` crate with the trait, `LaunchPlan`, registry, Claude + Codex + declarative providers,
   migration from profiles, unit tests that assert the generated `LaunchSpec` per provider.
2. Protocol + server: accounts/configurations RPCs, session create with (provider, account, config),
   provider list with detection/capabilities.
3. App: settings sections + new-session dialog.
4. Verification against the real CLIs without sending prompts: `--help`/config validation,
   `codex --strict-config`, MCP server listing inside the CLI (`/mcp` in Claude, `codex mcp list`),
   instruction presence via a dry inspection where possible.
