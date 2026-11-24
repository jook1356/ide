# Agent providers

Status: phases 1-2 implemented, phase 3 minimal (plain settings/new-session UI, to be
redesigned), phase 4 verification done for Claude Code 2.1.329 and Codex 0.160.1 (see
[Verification](#verification)). Marks: (v) checked against the real CLI with the version given;
(d) taken from the official docs, not exercised; (a) assumption.

## Goal
Any agent CLI (Claude Code, Codex, later Gemini CLI, opencode, ...) plugs into one common contract.
The IDE only deals with provider-agnostic data (accounts, configurations: skills, MCP servers,
instructions, hooks); each provider translates that into its own CLI flags, env vars and files.

## Concepts
| Concept | What it is | Stored |
|---|---|---|
| **Provider** | An agent CLI implementation (`claude`, `codex`, `shell`, declarative ones). Code (`impl AgentProvider`) or a TOML file. | server: `ide_agents::Providers` (built-ins + `<data>/providers/*.toml`) |
| **Account** | One login for one provider (credentials only). E.g. "Claude - work", "Codex - personal". | server: `accounts.json`; dir `accounts/<provider>/<slug>/` (the CLI's own config home for that login) |
| **Configuration** (UI: 프로필 / profile) | Provider-agnostic bundle: skills it owns (folders `<data>/configurations/<slug>/skills/<name>/` with a `SKILL.md`; `disabled_skills` lists the ones switched off; `ide-core/src/skill.rs`), MCP servers (stdio: name, command, args, env), instructions (text). Replaces the old "profile"; the UI calls it 프로필 again. Older files listed skill folders anywhere: they are copied in once on open. | server: `configurations.json` + `configurations/<slug>/` |
| **Session** | provider + account (optional) + configuration + repo/worktree; or a *login* session (provider + account, no repo) | server: `sessions.json` (`kind`, `provider`, `account`, `configuration`) |

Every agent session also gets the **agent bridge** (`docs/agent-bridge.md`): the `ide-bridge` MCP server,
its env vars and base instructions, added by the server through `SessionInput::bridge`.

Generated per-session files live in `<data>/sessions/<id>/agent/` (regenerated on every start,
removed with the session). Nothing is written into the worktree.

## Crate `ide-agents` (server side, UI-free)
```
crates/ide-agents/src/
  lib.rs          trait AgentProvider, Caps/Support, AccountStatus, HookTargets,
                  SessionInput, assemble(), login_spec()
  plan.rs         LaunchPlan (args, env, scratch dir, extra_instructions, notes)
  detect.rs       Detection (path, version via --version, min-version check), run_capture
  fallback.rs     skills_as_instructions, unsupported, mcp_json, toml_string, finish, copy_dir
  skills.rs       Skill (SKILL.md front matter: name, description)
  claude.rs codex.rs shell.rs declarative.rs   the providers
  registry.rs     Providers::builtin() / ::load(<data>/providers)
  tests.rs        LaunchSpec tests per provider, declarative parsing, registry, real-CLI checks
```
```rust
pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &str;                                  // "claude"
    fn display_name(&self) -> &str;
    fn glyph(&self) -> &str { "●" }                         // list icon
    fn color(&self) -> Option<u32> { None }                 // icon colour 0xRRGGBB
    fn program(&self) -> &str;
    fn version_args(&self) -> Vec<String> { vec!["--version".into()] }
    fn min_version(&self) -> Option<&str> { None }
    fn program_path(&self, env: &HostEnv) -> Result<PathBuf>;     // default: PATH lookup
    fn detect(&self, env: &HostEnv) -> Detection;                 // default: path + --version
    fn capabilities(&self) -> Caps;
    fn prompt_input(&self) -> PromptInput { Lines }        // session.send_prompt: Paste{..} for TUIs
                                                           // (claude, codex), see changes-actions.md
    // Accounts
    fn account_env(&self, acct: &AccountDir) -> Vec<(String, String)> { vec![] }
    fn login_args(&self) -> Option<Vec<String>> { None }          // see login_spec()
    fn account_status(&self, acct: &AccountDir, env: &HostEnv) -> AccountStatus { Unknown }
    fn account_home(&self, acct: &AccountDir, ctx: &SessionContext) -> Result<PathBuf>; // default: acct.dir
    fn after_exit(&self, ctx: &SessionContext, acct: Option<&AccountDir>) {}   // default: nothing
    // Conversations (only with Caps::resume, see "Resume and restart" below)
    fn new_conversation(&self, plan) -> Option<String>;               // default: None
    fn resume_conversation(&self, plan, id: &str, acct: Option<&AccountDir>) -> bool; // default: false
    // Session assembly (each step has a default)
    fn prepare_root(&self, plan, ctx: &SessionContext, config: &Configuration, skills: &[Skill])
        -> Result<RootPrepared>;                           // default: nothing (workspace only)
    fn apply_dirs(&self, plan, dirs: &[PathBuf]) -> Result<()>;   // default: nothing
    fn begin(&self, plan: &mut LaunchPlan, acct: Option<&AccountDir>);   // account env, base args
    fn apply_skills(&self, plan, skills: &[Skill]) -> Result<()>;  // default: skills_as_instructions
    fn apply_mcp(&self, plan, servers: &[McpServer]) -> Result<()>;  // default: note "unsupported"
    fn apply_instructions(&self, plan, text: &str) -> Result<()>;   // default: note "unsupported"
    fn apply_hooks(&self, plan, hooks: &HookTargets) -> Result<()> { Ok(()) }
    fn finish(&self, plan: LaunchPlan, env: &HostEnv) -> Result<LaunchSpec>; // resolve + .cmd wrap
}
```
Deviations from the first draft: `login_command` became `login_args()` + the shared
`login_spec(provider, account, cwd, env)` (program resolution and batch-file wrapping are the same
for every provider); the apply steps return `Result` (they write files); `begin()` was added
(base args such as `--no-daemon`, `-n <title>`); `id()` returns `&str` (declarative ids are
data). `assemble()` runs begin -> skills -> MCP -> instructions (configuration text + fallback
text) -> hooks -> `TERM`/`COLORTERM` -> finish and returns the spec plus `notes` (parts not passed
on; the server logs them and writes `session.note` audit records). Errors are `ide_core::Error`
(new ids: `account-not-found`, `account-in-use`, `configuration-not-found`,
`configuration-in-use`, `last-configuration`, `provider-not-found`, `provider-no-login`,
`invalid-mcp-name`).

No `match` on provider ids outside `ide-agents`; the only other place that names `claude`/`codex`
is the profile migration in `ide-core::migrate` (the legacy format had exactly those two dirs).

## Injection rules
1. **Never pollute the repo.** Inject through flags, env vars and the session scratch dir. If a CLI
   only reads a file inside the worktree, add it to `.git/info/exclude` (never `.gitignore`). (No
   current provider needs this.)
2. **Accounts are not modified** except by the CLI's own login flow. Configuration is layered on
   top per session, so one configuration works with any account.
3. **Unsupported capability -> documented fallback** (skills become instructions) or a launch note
   + the UI shows the support level; never silently dropped.
4. **Provider-specific details stay inside the provider.** No `match provider` outside `ide-agents`;
   mode-dependent behaviour goes through `SessionContext` + the trait (`prepare_root`, `apply_dirs`,
   `account_home`, `after_exit`, `Caps::mode`), never through server branches.
5. **Workspace sessions** may get generated files in the session folder (outside every repo);
   **original-repo sessions** never get a file in their cwd.

## Provider mapping
| Common | Claude Code (v 2.1.329) | Codex (v 0.160.1) | Shell | Declarative (TOML) |
|---|---|---|---|---|
| Detection | (v) `claude --version` -> `2.1.329 (Claude Code)`; min 2.1.318 | (v) `codex --version` -> `codex-cli 0.160.1`; min 0.160.0 | `COMSPEC`/`$SHELL` found | `version_args`, `min_version` |
| Account | (v) `CLAUDE_CONFIG_DIR=<account dir>` | (v) `CODEX_HOME=<account dir>` | none | `account_env = "VAR"` |
| Login | (v, help only) `claude auth login` | (v, help only) `codex login` | — | `login_args` |
| Status | (v) `claude auth status --json` -> `loggedIn` (exit 1 + `false` when logged out) | (v) `codex login status` (exit 1 + `Not logged in`) | — | unknown |
| Skills | (v) generated plugin `<scratch>/plugin/{.claude-plugin/plugin.json, skills/<folder>/...}` (copied) via `--plugin-dir`; skill ids `ide-<config>:<folder>` | (v) fallback: listed in the developer instructions (name, description, SKILL.md path) | ignored | fallback if instructions are supported |
| MCP | (v) `<scratch>/mcp.json` `{"mcpServers":{name:{type:"stdio",command,args,env}}}` via `--mcp-config` | (v) `-c mcp_servers.<name>={command="..", args=[..], env={"K"="v"}}` | ignored | unsupported (noted) |
| Instructions | (v) `<scratch>/instructions.md` via `--append-system-prompt-file` | (v) `-c developer_instructions="..."` (single-line TOML string) | ignored | `flag` / `env` / `file` |
| Hooks (status, edit guard) | (d) `--settings <scratch>/settings.json`, exec-form entries running `ide-server hook` (`docs/agent-bridge.md`); background agents (`SubagentStart/Stop`) -> `HookEvent::Background*` (optional for a provider: report a start only with its end) | (d) account integration: ide's entries in `CODEX_HOME/hooks.json` on the user's request, trusted once with `/hooks` (`docs/agent-bridge.md`) | — | — |
| Session name | (v) `-n <title>` | — | — | — |
| Resume | fresh `--session-id <uuid>`, next start `--resume <uuid>` when `<config home>/projects/*/<uuid>.jsonl` exists; `SessionStart` hook `session_id` (a) | — (not yet: `codex resume <id>` + hook id, unverified) | — | — |
| Inherited env | removed: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, ... (v 2.1.330, see below) | — | — | — |
| Daemon | — | (v) `--no-daemon` always | — | — |

Notes:
- Codex skills: Codex discovers skills only in `$CWD/.agents/skills` (up to the repo root),
  `$HOME/.agents/skills`, `/etc/codex/skills` and `$CODEX_HOME/skills` (system skills); the
  `skills.config` entries (`path`/`name` + `enabled`) only enable/disable discovered skills (d, and
  (v): a `skills.config` entry pointing at an outside SKILL.md did not add it to the model input).
  Writing into the worktree or the account is not allowed, so skills use the fallback.
- Codex instructions: `developer_instructions` is added as its own developer message (v);
  `model_instructions_file` would *replace* the built-in instructions (d) and is not used. A
  `developer_instructions` in the account's own `config.toml` is overridden for IDE sessions (a).
- Claude plugin name must not start with `claude-`/`anthropic-` (d); we use `ide-<config slug>`.
- `--append-system-prompt-file` has no own line in `claude --help` (listed as
  `--append-system-prompt[-file]`), documented in the CLI reference (d); accepted at launch (v).
- Windows: both CLIs are `.exe` here, so arguments go straight to `CreateProcess`. An npm `.cmd`
  shim would run through `cmd.exe /d /s /c`, where `&|<>^` inside the instructions text could be
  interpreted by cmd (a; not hit with the current installs).

### Session context, modes and the new trait methods
Every assembly step gets a `SessionContext` (in `SessionInput::ctx` / `LaunchPlan`):
```rust
pub enum SessionMode { Workspace, OriginalRepo }          // ide_core::SessionMode
pub struct RepoInfo { name: String, path: PathBuf /* worktree or repo */, git_dir: Option<PathBuf> }
pub struct SessionContext {
    mode: SessionMode,
    root: PathBuf,        // workspace: <data>/workspaces/<id> (the cwd, outside every repo);
                          // original repo: the repository itself
    repos: Vec<RepoInfo>,
    scratch: PathBuf,     // <data>/sessions/<id>/agent (generated files, never in a repo)
    guidance: Option<String>, // session guidance text (Hermes Desk's HERMES.md role)
}
```
- `prepare_root` materializes project-level files into `ctx.root`, where the CLI discovers them
  from its cwd. `assemble()` only calls it for `Workspace`; writes go through
  `LaunchPlan::write_root_file` / `root_dir`, which return an error in `OriginalRepo` mode, so a
  provider cannot write into a repository even by mistake. It returns `RootPrepared { skills,
  guidance }`: what it covered (the skills / guidance are then not passed again by the apply steps).
  The files are regenerated on every start (a configuration change applies on the next start,
  attach/detach regenerates the guidance right away); the previous generated `skills` folder and
  guidance file are removed first (`LaunchPlan::clear_root_path`).
- `apply_dirs` gets `ctx.outside_dirs()`: worktrees outside the root (migrated legacy sessions)
  and every repo's shared git dir (commits on the worktree branch write there). Claude and Codex
  pass them with `--add-dir` (both listed in `analysis/*-help.txt`). Codex takes them only with a
  writable sandbox: a session folder is no trusted project, so it would start `read-only` and
  refuse them ("the effective permissions do not allow additional writable roots", (v) 0.172.0);
  `begin()` asks for `--sandbox workspace-write` unless the account's `config.toml` sets
  `sandbox_mode`.
- `account_home` returns the config home for the session (default: the account dir). The server
  never builds account env itself; `begin()` gets the account with the returned dir.
- `after_exit` runs when a session's process exited or was stopped (server: `Service::after_exit`).
  Default no-op; no built-in provider needs it now (see "Codex overlay" below).
- `Caps` reports per mode: the top-level `skills`/`mcp`/`instructions` are for workspace
  sessions, `original_repo: ModeSupport { skills, mcp, instructions }` for original-repo
  sessions; `Caps::mode(m)` picks one. Sent to the UI as `ProviderCaps { .., original_repo:
  ModeCaps }` and shown on the configurations page (two support lines per provider).

| | Claude workspace | Claude original repo | Codex workspace | Codex original repo |
|---|---|---|---|---|
| Skills | native: `<root>/.claude/skills/<folder>/` (copied) | native: plugin via `--plugin-dir` (scratch) | native: `<root>/.agents/skills/<folder>/` (copied) | fallback: listed in `developer_instructions` |
| MCP | native: `--mcp-config <scratch>/mcp.json` | same | native: `-c mcp_servers.*` | same |
| Instructions + session guidance | `<root>/CLAUDE.md` (guidance) + `--append-system-prompt-file` (configuration text) | `--append-system-prompt-file` (both) | `<root>/AGENTS.md` (guidance) + `-c developer_instructions` | `-c developer_instructions` (both) |

Verified facts behind this table (scratch homes under `ti-probe/`, no login, no prompts):
- (v) Codex 0.160.1, cwd = a non-git folder: `codex debug prompt-input hello` lists skills from
  `<cwd>/.agents/skills/` and `<cwd>/.codex/skills/` and includes `<cwd>/AGENTS.md`.
- (v) Codex ignores a project `<cwd>/.codex/config.toml` there (its `developer_instructions` did
  not appear, `codex mcp list --json` printed `[]`), so MCP and instructions stay on `-c`.
- (v) Claude 2.1.329: a project `<cwd>/.mcp.json` shows as `Pending approval (run claude to
  approve)` in `claude mcp list` / `claude mcp get` -> it would stop at an approval prompt, so MCP
  stays on `--mcp-config` (no prompt).
- (d, not verified) Claude loads `<cwd>/.claude/skills/` and `<cwd>/CLAUDE.md` from a non-git cwd:
  documented behaviour; checking needs a logged-in session (`/skills`, `/memory`).
- Instructions stay on `--append-system-prompt-file` / `developer_instructions` in both modes
  (no prompt, no file in a repo); only the generated guidance is a root file in workspace mode.

#### Codex overlay (evaluated, not used)
Native skills for Codex original-repo sessions would need a per-session `CODEX_HOME` overlay
(`$CODEX_HOME/skills`). Probe: `auth.json` hard-linked into an overlay home (Python `os.link`;
`cmd /c mklink /H` failed here) kept the same inode and link count 2 after `codex login
--with-api-key` through either home (v), so in-place auth writes are shared. Not adopted because
(a) OAuth refreshes were not observable without a real login (an atomic rename would silently
split the link and the next session would use a stale token), (b) the overlay must also mirror
`config.toml`, history, sessions and logs of the account, (c) a symlink/junction alternative
needs admin or developer mode on Windows. Codex original-repo skills therefore use the
documented fallback (`Caps.original_repo.skills = Fallback`) and `account_home` / `after_exit`
keep their defaults. The trait methods stay so an overlay can be added inside `codex.rs` without
touching the server.

### Resume and restart
A session keeps the CLI's own id of its conversation (`Session::conversation` in
`sessions.json`, opaque to everything outside the provider) and whether its process was
running when the server went away (`Session::autostart`).
- `assemble()` (providers with `Caps::resume` only): a stored id goes to
  `resume_conversation`; when it returns `false` (or nothing is stored) `new_conversation` may
  pick one. `Assembled::conversation` is what the launch uses; the server stores it at start.
- The CLI's `SessionStart` hook may report a different id (`HookEvent::SessionStart {
  conversation }`: after `/clear`, or `/resume` of another conversation inside the TUI); the
  server stores that one, so the next start continues what the user saw last.
- `autostart` is set when a resumable session starts, cleared by `session.stop` (the user) and
  when the process exits by itself (`Exited`); a server shutdown (close dialog, stop server,
  crash) keeps it. When a user context opens (first client after a server start) the server
  restarts those sessions (`session.autostart` audit record); a failed start clears the mark.
- Claude: `resume_conversation` checks for the transcript first: `--resume` with an unknown id
  ends the CLI. Ids from hooks are used only if they are UUIDs.
- (v 2.1.330) A server started from a Claude Code tool shell (an agent that builds and runs
  ide) inherits that Claude Code's session markers (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`,
  `CLAUDE_CODE_CHILD_SESSION`, messaging socket, ...). A session's `claude` started with them
  runs as a child session and writes no transcript (`projects/<cwd>/` stayed empty, `/resume`
  listed nothing). Claude's `begin()` removes them (`LaunchPlan::remove_env` ->
  `LaunchSpec::env_remove`, applied by the runner before `env`); user settings such as
  `CLAUDE_CONFIG_DIR` or `CLAUDE_CODE_USE_BEDROCK` are kept.

### Declarative providers
`<data>/providers/*.toml`, sample [`docs/providers/example.toml`](providers/example.toml):
`id`, `name`, `program`, `args`, `version_args`, `min_version`, `glyph`, `account_env`,
`login_args`, `[env]`, `[instructions] mechanism = "flag" | "env" | "file" | "none"` (+ `flag` /
`env` / `file`), `[mcp] mechanism = "unsupported"`. Placeholders in `args`/`[env]`:
`{account_dir}`, `{scratch_dir}`, `{session_id}`, `{title}`, `{instructions_file}`. Invalid files
and ids that clash with a built-in are skipped (logged). The in-app test script installs one
(`fake`: `cmd.exe` whose "login" echoes a marker).

## Migration (profiles -> accounts + configurations)
Runs when the server opens a user's data (`ide_core::migrate_profiles`), before the stores:
- only if neither `accounts.json` nor `configurations.json` exists and `profiles.json` does: every
  profile `p` becomes account `claude/p` (dir `profiles/p/claude`, not copied), account `codex/p`
  (dir `profiles/p/codex`) and an empty configuration `p` (same slug, `created_at` kept);
- sessions without `provider` (always, idempotent): `provider` = old `agent`, `account` = the
  migrated account of their profile for that provider (none for the shell or a vanished profile),
  `configuration` = the profile (else the first configuration). Legacy fields are read, not written;
- `profiles.json` and the profile dirs stay in place; account dirs inside the data dir are stored
  relative (`profiles/default/claude`), so a moved data dir keeps working;
- a fresh data dir gets one empty configuration `default` and no accounts (a session without an
  account uses the CLI's default login);
- the app's `ui.json` `profile` key is read as the sidebar's configuration (`#[serde(alias)]`).

## Protocol (PROTO_VERSION 3, see repos-and-workspaces.md for repos/sessions)
| Method | Result |
|---|---|
| `provider.list { refresh }` | `Providers(Vec<ProviderInfo { id, name, glyph, color, detection { path, version, min_version, usable, error }, caps { accounts, login, skills, mcp, instructions, hooks: Support, resume, session_name, original_repo { skills, mcp, instructions } } }>)` (detection cached per server until `refresh`) |
| `account.list` / `account.create { provider, name }` / `account.rename { provider, slug, name }` / `account.delete { provider, slug, remove_files }` | `Accounts` / `Account` / `Account` / `Unit` (`account-in-use` while sessions use it) |
| `account.status { provider, slug }` | `AccountState(LoggedIn / LoggedOut / Unknown)` (runs the CLI's status command) |
| `account.login { provider, slug, cols, rows }` | `Session`: a started *login session* (`kind: Login`, cwd `sessions/<id>/cwd`, no repo/branch) running the login command; the app shows it like a session tab; delete it when done |
| `configuration.list` / `.create { name }` / `.update(ConfigurationInfo)` / `.delete { slug }` | `Configurations` / `Configuration` / `Configuration` / `Unit` (`configuration-in-use`, `last-configuration`) |
| `session.create { provider, account?, configuration, repos, original_repo?, ... }` | `SessionCreated { session, failures }` (`SessionInfo` has `kind`, `provider`, `account`, `configuration`, `mode`, `root`, `repos`) |

Notifications `AccountsChanged`, `ConfigurationsChanged` replace `ProfilesChanged`; Welcome
capabilities `providers`, `accounts`, `configurations` replace `profiles`.

## UI (minimal, to be redesigned)
- Accounts page (navigation rail -> 계정): **Accounts**: all accounts (provider icon, name, provider), Status (asks the CLI),
  Log in (opens a login session), rename (display name), delete (with its dir). Add = provider +
  name -> creates the account and opens its login command in a terminal tab (settings close).
- Profiles page (navigation rail -> 프로필; `profiles_view.rs`), like Hermes Desk's agents page:
  the profile list sits beside the rail where the work page has its session list (rows: name,
  "세션 N개 · 스킬 M개"; "프로필 추가" opens a name dialog and shows the new profile; the page's
  rail button or the list's collapse button hides / shows it, sharing `sessions_open`). The page
  shows the picked profile: title row (name, 저장 안 됨, delete with an inline confirm, 되돌리기,
  저장), session counts, then one card per section: 기본 (name), 지침 (multiline), 스킬 (own skills,
  applied at once: `skills_ui.rs`), MCP 서버 (name, command, args split on spaces, env as
  `KEY=value` items), 기본 레포; a per-provider support line (✓ native, ~ fallback, ✕ not passed)
  at the bottom. Deviations from Desk: sections are cards on one page instead of Desk's tabs
  (기본 / MCP / 스킬 / 학습), so unsaved edits stay in sight; no 학습 tab; "프로필 추가" has no
  "<agent> 설정 복제" choice.
- New session: provider (only usable ones), account (of that provider, or "default login"; hidden
  for providers without accounts), configuration. Sidebar filter = configuration; login sessions
  are always listed. Session rows show the provider icon (not yet the account name).

## Verification
All runs without prompts and without logging in, with account dirs inside the workspace
(`IDE_TEST_TMP`), never the user's `~/.claude`/`~/.codex`.
1. `IDE_REAL_CLI=1 cargo test -p ide-agents real_cli -- --ignored --nocapture`:
   - `claude plugin validate --json <scratch>/plugin` -> success, provides skill
     `ide-frontend:lint-fix` (v 2.1.329);
   - the generated `mcp.json` copied as a probe plugin's `.mcp.json` -> `claude plugin validate`
     success, provides MCP server `docs` (same schema as `--mcp-config`) (v);
   - `claude --help` lists `--plugin-dir`, `--mcp-config`, `--append-system-prompt[-file]`,
     `--name` (v); `claude auth status --json` on the empty account -> `loggedIn: false` (v);
   - `codex mcp list --json` + generated `-c` overrides (scratch `CODEX_HOME`) -> server `docs`
     with command, args (incl. `C:\a b`) and env (v 0.160.1);
   - `codex debug prompt-input` + generated overrides renders the model input locally (nothing is
     sent): our `developer_instructions` appear verbatim as a developer message, incl. quotes,
     newlines and the skills list (v);
   - `codex features list` runs with the overrides (`hooks`, `skill_search` stable) (v);
     `codex login status` on the empty account -> logged out (v).
   - `--strict-config` is only accepted by the interactive `codex` (not by `debug`/`mcp`/
     `features`) (v), so overrides were checked through `mcp list` / `prompt-input` instead; the IDE
     does not pass `--strict-config` (an account's own config may hold keys of another version).
2. `IDE_REAL_CLI=1 cargo test -p ide-server --test real_cli -- --ignored --nocapture`: Claude and
   Codex sessions created through the client with a configuration (skill, MCP server,
   instructions with `"` and `&`) and a fresh account, started in a ConPTY by the server, screen
   read for 12 s, stopped and deleted, nothing typed. Claude 2.1.329 showed its first-run theme
   picker, Codex 0.160.1 its sign-in screen; both still running, no option/config errors (v). No
   claude/codex process was left (tasklist before/after).
3. Not verified: that Claude loads the plugin skills / MCP server / appended prompt inside a
   logged-in session (`/mcp`, `/skills` need a login); Codex MCP server start-up in a session;
   the real `claude auth login` / `codex login` flows (only their `--help`).

## Phases
1. done: `ide-agents` crate with the trait, `LaunchPlan`, registry, Claude + Codex + Shell +
   declarative providers, migration from profiles, unit tests per provider.
2. done: protocol + server (accounts/configurations RPCs, session create with (provider, account,
   configuration), provider list with detection/capabilities, login sessions).
3. minimal: settings sections + new-session dialog (plain widgets; redesign pending).
4. done for the checks above; hooks (status) are the next step.
