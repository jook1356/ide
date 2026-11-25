# Profile editing and history

A session's agent can save what it learned into the profile (configuration) it was started
with, so later sessions using that profile get it: a new skill, a fix to a skill, a project
rule in the instructions, an MCP server. Every change of a profile, from an agent or from the
profiles page, is kept as a version the user can go back to.

## Storage (`meninx-core/src/configuration.rs`, `profile_history.rs`)

```text
configurations.json                 the slugs, in order: {"configurations":[{"slug":"default"}]}
configurations/<slug>/              one folder per profile, a git repository of its own
  profile.json                      name, created_at, default_repos, disabled_skills
  instructions.md                   the instructions (text as saved)
  mcp/<name>.json                   one MCP server: {"command": "...", "args": [...], "env": [["K", "v"]]}
  skills/<name>/SKILL.md ...        one skill (`skill.rs`)
```

- One file per item, so edits of different items never touch the same file. MCP servers are
  listed by name (their files' order); names must be unique ignoring case (one file each).
- Older `configurations.json` files held the profiles whole: `ConfigurationStore::open` moves
  each into its folder (an entry without a `profile.json` in its folder is read the old way).
  `migrate_profiles` still writes the old shape; it is moved on the next open.
- **History**: every change is one commit in the profile's folder (`git add -A`, author
  `Meninx <meninx@localhost>`, `--no-verify --no-gpg-sign`: an internal store, not the user's
  repository). The summary line is the agent's reason or a fixed English text; trailers keep
  who made it: `Meninx-Source: ui | agent | restore | import`, `Meninx-Session: <id>`,
  `Meninx-Session-Title: <title>`. A folder without `.git` gets `git init` and a first change
  (`import`) on open. Git never runs in a folder without its own `.git` (it would act on an
  enclosing repository). History is best effort: without git, profiles still work (a warning
  is logged).
- Restore (`profile_history::restore`): `git read-tree -u --reset <id>` (the folder becomes that
  version; files added since are removed), committed as a new change (`restore`). The store
  then reads the profile again.
- MCP `env` values are committed with the rest. The repositories are local only (no remote);
  they hold what `configurations.json` held before. A separate secret store is a to-do.

## Agent tools (`meninx-server/src/service/profile.rs`)

Both act on the session's own profile (`Session::configuration`).

- `meninx_profile_get {path?}`: without `path` the profile's name, the instructions text and its
  revision, the skills (name, title, description, enabled), the MCP server names and every
  editable file with its revision (`rev`) and size. With `path` that file's text and `rev`.
  `profile.json` is not offered (name and switches are the user's).
- `meninx_profile_edit {reason, ops: [...]}`: applied at once, no approval. `reason` (one line) is
  shown to the user and kept as the change's summary. Operations (`profile_files.rs`):

  | `op` | fields | applies when |
  |---|---|---|
  | `patch` | `path`, `old`, `new` | `old` occurs exactly once in the file now |
  | `write` | `path`, `content`, `rev?` | no `rev`: the file does not exist (create); `rev`: the file's revision is `rev` |
  | `delete` | `path`, `rev` | the file's revision is `rev`; `skills/<name>` deletes the whole skill (`rev` of its `SKILL.md`) |

  Paths: `instructions.md`, `mcp/<name>.json`, `skills/<name>/<file...>` (lowercase skill folder
  names, no hidden or `..` parts). One operation per file per call; at most 50 operations;
  files up to 256 KiB. The end state must be valid: an MCP file parses and has a command, a
  skill folder that keeps files has a `SKILL.md`, `instructions.md` is never deleted.

### The profile is the only place for an agent's setup

Meninx launches the agent CLI with the profile's MCP servers, skills and instructions; the CLI's
own config (`claude mcp add`, `~/.claude.json`, a project `.mcp.json`, `~/.codex/config.toml`)
is outside Meninx: Meninx neither shows nor manages it, it reaches only that CLI, and it also reaches
the CLI when it runs outside Meninx. So the bridge instructions (`bridge.rs::INSTRUCTIONS`) and
both tool descriptions tell the agent to add, change or remove an MCP server, a skill or a
standing instruction only through `meninx_profile_edit`, never with the CLI's commands or files,
and to say so and ask when the profile cannot hold what is asked. The instructions used to
offer the profile only for "something future sessions should know", and an agent asked to add
an MCP server ran `claude mcp add --scope user` instead (2026-11-25).

A profile's MCP server is a local (stdio) program (`McpServer`: command, args, env). A remote
(HTTP) server is reached through a local proxy, e.g.
`{"command": "npx", "args": ["-y", "mcp-remote", "https://host/mcp/", "--header", "Authorization:${AUTH}"], "env": [["AUTH", "Bearer <token>"]]}`
(the value goes in `env` so no space sits inside an argument). Native remote servers (url +
headers in `mcp/<name>.json`, mapped to each CLI's own format) are a to-do.

### Concurrency

Several sessions may use one profile and edit it at the same time. Edits are **optimistic**:
nothing is locked while an agent reads and thinks. The check happens when the edit is applied,
under the configuration store's mutex, so check and write are one step and two edits never
interleave.

- A patch carries its own base: the exact text it replaces. If another session changed that
  text in between, the patch no longer finds it and is refused; a patch elsewhere in the same
  file still applies.
- A whole-file write or delete names the revision it was based on (`rev` = FNV-1a 64 of the
  bytes, 16 hex digits; compares versions of one file, not a security hash).
- A refused call applies nothing (all operations or none) and tells the agent why, with the
  file's current text and revision for `Stale` / `NoMatch`: the agent re-applies its change to
  that text and calls again. Nobody arbitrates: the agent that made the change redoes it, the
  user can undo anything from the history.
- A whole-profile lock (an "edit mode" other sessions wait for) was considered and rejected:
  locks left behind by ended sessions, agents waiting while another one thinks, and unrelated
  edits serialized, for a case that is rare (profile edits are few) and that the check above
  already handles.
- The profiles page still saves the whole profile (`configuration.update`, last writer wins).
  It shows a newer saved version at once when nothing is edited there; with unsaved edits it
  keeps them and marks the profile 다른 곳에서 바뀜 / Changed elsewhere (saving overwrites the
  other change, 되돌리기 loads it). Both versions stay in the history.

### After an edit

- The change is a commit (`agent`, with the session's id and title and the reason), audited
  (`profile.edit <slug>: <files>`).
- `configuration_changed`: workspace sessions using the profile get their root files (copied
  skills, guidance) regenerated; agents see the rest (instructions, MCP) on their next start.
- `Notify::ConfigurationsChanged`, then `Notify::ConfigurationEdited {slug, name, session,
  session_title, summary, files}`: the app shows a notice (세션이 “…” 프로필을 바꿨어요).

## Protocol

| Request | Response |
|---|---|
| `configuration.history { slug, limit }` | `ConfigurationHistory(Vec<ConfigurationChange { id, time, source, summary, session, session_title, files }>)`, newest first (limit 1..=500) |
| `configuration.restore { slug, id }` | `Configuration` (the restored profile) |

## UI (`meninx-app/src/profile_history_ui.rs`)

The profiles page has a 변경 이력 / Change history section: the last 30 changes (who: 세션 “title”,
프로필 화면, 이전 버전으로 되돌림, 기록 시작; when; an agent's reason; the changed files). Every row
but the newest has 이 버전으로 with an inline confirm. The list reloads when the page or the
picked profile changes and on every configuration change.

## Not done yet

- Approval before an agent's edit applies (e.g. only for deletes or whole-file rewrites):
  proposals were discussed and left out; history + undo is the safety net.
- Item locks (`lock` a skill or instruction against agent edits).
- Diff view of a change in the history (only the changed file names are shown).
- Secrets of MCP servers outside the committed files.
