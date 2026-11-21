# Session workspace abc

This folder is this session's private workspace. Each subfolder is a git worktree of one repository:

- (no repositories are currently connected to this workspace)

Rules:
- Work only inside this folder: create and change files here only.
- Session output that is not part of a repository's change (analysis notes, reports, plans, scratch files, e.g. `analysis/`) goes in this folder's root, beside the worktrees, written by absolute path: `/analysis/…`.
- The original repositories are never changed; use the worktrees here. To change several repositories, edit their worktrees here; they can all be modified freely.
- Commit to each worktree's branch (`ide/abc`). Do not check out branches that are already checked out elsewhere (git will refuse).
- Before working in a repository, read its own AGENTS.md / CLAUDE.md / README if present.
- If the task needs another registered repository, ask the user to attach it in the app (repository picker of the session's detail area); it then appears in this list.
