# Side panel (Changes / History / Explore)

The side panel right of the detail pane follows Hermes Desk's `ChangesPanel`
(`powered_ide/src/renderer/src/components/ChangesPanel.tsx`), its History list and its Explore
tab (`ExploreTree.tsx`).

## Hermes Desk behaviour -> ide file

| Hermes Desk | ide |
|---|---|
| Tabs `Changes \| History \| Explore` (`ChangesPanel.tsx`, `Tabs.List grow`), Changes with a count badge, History with the unpushed count (`500+` at the cap, tooltip `원격에 올라가지 않은 커밋 {n}개`) | `detail_view.rs::side_panel` builds the tab row with `tab(SideTab, label, badge)` (badge = text + tooltip); tabs are `detail.rs::SideTab` (order = Desk order); History's badge is `HistoryState::badge`. |
| Selected tab remembered | `DetailUi.side` in `ui.json` (`SideTab::code` / `SideTab::parse`); the old value `"search"` opens Explore. |
| `repo / branch` picker under the tabs (`BranchSelect`) | `detail_view.rs::repo_selector`, shown under the tabs by every tab |
| Explore: `파일 찾기` input with a search icon on the left (`ExploreTree.tsx`, `TextInput leftSection=<IconSearch>`) | `detail_view.rs::explore_search_input` (Tabler `search` icon, `icons::Icon::Search`) |
| Settings button on the input's right (gitignore 숨기기 popover) | The text search's case / word / regex toggles sit there instead (`Aa`, `ab`, `.*`) |
| Empty filter: the tree | `side_panel`, `SideTab::Explore` arm: `d.tree` when `SearchState::active()` is false |
| Typing filters as you type; matching paths replace the tree as flat rows (file icon, full path, truncated at the start), click opens the file | `DetailMsg::SearchInput` -> `App::find_files` (`search.files` for the shown repo, at most 200) -> `DetailMsg::Files`; rows in `detail_view.rs::explore_results` (`DetailMsg::OpenAt(path, 0, 0)`) |
| Revealing a file in the tree clears the filter | `App::tree_reveal` clears `SearchState` |
| (no content search in Desk's Explore) | Text search of every repo below the file names under the `내용` / `Contents` label, with the status (`검색 중…`, counts, errors) next to it; starts 250 ms after the last keystroke (`DetailMsg::SearchTyped`) or at once on Enter (`SearchSubmit`); results grouped by repo and file as before |
| Ctrl+Shift+F (none in Desk) | `Message::FindInFiles` -> `App::open_search`: shows Explore and focuses its input (`SEARCH_ID`) |

## History (Desk `ChangesPanel` History, `CommitRow`, `CommitFileRow`)

| Hermes Desk | ide |
|---|---|
| `loadRepositoryHistory`: `changes:log(dir, 0, max(200, shown))` + `github:historyLocal`, again whenever the repo's version changes (watcher / after git actions), also while another tab is shown (badge) | `App::load_history` (`git.log` + `git.unpushed`, `HistoryState::refresh_limit`) from `sync_detail` (first show of the repo, stale after a reconnect), `after_git_change` (`GitChanged` of the shown repo: commits, checkouts, pushes) and opening the History tab; answers of older refreshes are dropped (`seq`) |
| Picking another repo shows its history | `Detail::set_repo` resets `HistoryState`; `sync_detail` loads it |
| `커밋이 없습니다.` when the list is empty | `detail_view.rs::history_list` (`history-empty`) |
| `ScrollArea onBottomReached={loadMore}` + `더 보기` button (loader while loading) | `DetailMsg::HistoryScrolled` (24 px from the end) / `HistoryMore` -> `App::load_more_history` (`git.log` with `skip` = shown commits); `HistoryState::append` skips commits already listed |
| `CommitRow`: bold subject truncated, ↑ in a violet circle when unpushed (tooltip `아직 원격에 올라가지 않은 커밋`), `AuthorAvatar` + `author · relativeTime(date)` (title `short · full date`) | `detail_view.rs::commit_row`; ↑ = Tabler `arrow-up` drawn with stroke 2.5 like Desk; avatar = `history::initials` / `history::avatar_color` (`hsl(hash % 360, 45%, 38%)` of the e-mail, Desk's hash); `history::relative_time` mirrors `lib/relativeTime.ts` (`방금`, `N분 전`, `N시간 전`, `어제`, `N일 전`, `지난달`, `N개월 전`, `작년`, `N년 전`); full date `YYYY-MM-DD HH:MM` in local time |
| Chevron (Tabler `chevron-right`, turned down when open) on the right, visible on hover and while open, tooltip `변경된 파일 보기` / `파일 목록 접기`; click the row = whole-commit diff | iced `hover` layer with the chevron while folded, in the row while unfolded (`chevron-right` / `chevron-down`); `DetailMsg::ToggleCommit` / `OpenCommit(sha, None)` |
| Unfolded commit: `불러오는 중…` / error, then `CommitFileRow`s (file icon, path truncated at the start, title `orig → path`, change-kind box) on a darker band with a 6 px gap below | `HistoryState::files` (`CommitFiles::Loading/Loaded/Failed`, cached per commit) from `git.commit_files`; `commit_file_row` (change-kind box = the Changes list's letter box); `COMMIT_FILES_BG` = black 12 % over the panel |
| Click a file: that file's diff in the commit; the active row is highlighted | `ItemKey::Commit { repo, sha, path }` (`ui.json`: `commit:<repo>:<sha>:<path>`, empty path = whole commit) shown by the existing `DiffView`; loaded with `git.commit_diff` (+ `git.file_at` at the commit for one file, to expand unchanged lines) |
| Tab label `file @short` / `short subject`, subtitle `short subject` / `author · date`, kind `커밋`, `IconGitCommit` (badge on the file icon) | `detail_view.rs::item_names` / `item_kind` / `item_icon`; open items list group `히스토리` |

## Deviations from Desk

- File names are matched by the server's fuzzy matcher (`search.files`, best first), not Desk's
  case-insensitive substring filter, and only files are listed (Desk also lists matching folders).
- At most 200 file-name matches; Desk shows 500 and a "N개 더 있음" line.
- No gitignore setting: `search.files` and the text search skip ignored files.
- Explore also holds the text search (the former Search tab) under a section label.
- Matched characters are not highlighted in file-name rows (Desk does not either); text matches are
  highlighted as before.
- History: commit diffs open as normal items (the ide has no preview tab that the next diff
  replaces, same as Changes); the list shows `불러오는 중…` until the first page arrived (Desk shows
  `커밋이 없습니다.` meanwhile); a failed `git.log` shows `커밋 목록을 불러오지 못했습니다: …` as a toast
  (Desk: notification); the history is reloaded when the repo is picked again instead of kept per
  repo; the full date is `YYYY-MM-DD HH:MM` (Desk: the browser's `toLocaleString`).
- History omits (not ported): dragging a commit into the chat and the commit menu's 채팅에 첨부
  (the ide has no chat composer). Desk's merge commits list nothing; ide shows them against the
  first parent (`docs/git-history.md`). The commit menu, the reveal buttons and the stash section
  are described under "Changes / History actions" below; the AI commit / sync / PR footer is
  the footer's (`footer.rs`).

## Changes / History actions (Desk `ChangesPanel`, `StashSection`, `CommitContextMenu`, `BranchSelect`)

State `changes_actions.rs` (`ChangesActs` in `Detail::acts`, messages `ActMsg` via
`DetailMsg::Acts`, dialogs `ChangesModal` in `Modal::Changes`), drawing `changes_view.rs`,
backend calls as listed in `docs/changes-actions.md` ("What the UI does per Desk control").

| Hermes Desk | ide |
|---|---|
| `snapshot.files` (one row per path) and `unchecked` (every file starts checked) | `ChangesActs::files` from `git.status` (`snapshot_files`: one entry per path, kind against HEAD: conflict > staged > unstaged > untracked), `ChangesActs::unchecked` (paths no longer changed are forgotten) |
| Select-all `Checkbox` (checked / indeterminate) + `N changed files` / the status error | `changes_view.rs::changes_head`: `check_box(Some(true) / None / Some(false))`, `ActMsg::CheckAll`; text `changes-count` |
| Row `Checkbox` | `change_row` (drawn through `ChangesList::view_with`, which keeps the group headers and virtual scrolling of `ide-views`), `ActMsg::Check` |
| 임시 보관 `ActionIcon` (`IconArchive`, tooltip 변경 임시 보관 / 체크한 N개 임시 보관, disabled with nothing checked) + popover (text, 메모 input, `새 파일(추적 안 됨)도 함께 보관돼요`, 취소 / 임시 보관; Enter submits) | `changes_head` (popover under the button, memo focused on open); `ActMsg::StashSubmit` -> `git_stash_push(memo.trim(), None` when all checked, else checked paths + rename sources`)`; toast `변경 N개를 임시 보관했어요` / `임시 보관 실패: …` |
| `StashSection` (only with stashes; this branch's, `다른 브랜치 임시 보관 N개 보기` / `이 브랜치 것만 보기`; row: chevron, title, `N개 · time (· branch)`, pop / apply / drop buttons; unfolds into the files; drop asks 임시 보관 버리기) | `changes_view.rs::stash_section` (docked under the rows, scrolls past 220 px); `git_stash_list` after every `git.status` answer; `ActMsg::StashAct` / `StashDone` (conflicts -> Desk's warning, stash kept); drop -> `ModalKind::StashDrop` |
| Row right-click `Menu` at the pointer: 변경 취소, 체크한 파일 모두 변경 취소 (n개) (more than one checked), Explore에서 보기 | `mouse_area::on_right_press` -> `ActMsg::Menu`; popover **under the row** (iced has no pointer-anchored menu); 탐색기에서 보기 |
| Hover buttons `DiscardButton` (`IconArrowBackUp`, red) and `RevealButton` (`IconArrowRight`) | `row_actions` (iced `hover` layer left of the kind box) |
| 변경 취소 `Modal` (file list with kind boxes, `새로 만든 파일은 삭제됩니다.` for untracked / added, 취소 / 변경 취소) | `ModalKind::Discard`; `git_discard` per file, failures collected into one toast (`일부 파일을 되돌리지 못했습니다:` + lines) or `N개 파일의 변경을 취소했습니다` |
| Reveal (switches to Explore, selects the file) | `ActMsg::Reveal` -> `DetailMsg::Side(Explore)` + `App::tree_reveal` |
| `CommitFileRow` reveal button (not for deleted files) | `detail_view.rs::history_list` wraps `commit_file_row` in `row_actions(.., reveal_button)` |
| `CommitContextMenu` (right-click a commit): SHA 복사, 커밋 제목 복사, 4 actions; each action -> confirm modal (label, description, sha, subject, `대상: dir`, branch name input, the warning alert, error alert, 취소 / 확인 후 실행); refresh after success and failure | `commit_menu_wrap` (popover under the row); copies via `iced::clipboard::write`; `ModalKind::Commit` -> `git_commit_action`; server errors (`git-dirty`, `git-in-progress`, ...) shown in the dialog; `App::refresh_repo` either way |
| `BranchSelect` `repo / branch` button (`IconGitBranch`, bold repo, `/`, branch, refresh `IconRefresh` 저장소 상태 새로고침, chevron) | `changes_view.rs::repo_branch_picker` inside `detail_view.rs::repo_selector` (the ⚙ session repos button stays right of it); branch = live `git.status` branch (`—` detached) |
| Popover: 레포 list (check on the shown one), 브랜치 + `+ 새 브랜치`, 브랜치 검색, 로컬 / 원격 groups (200 rows, `N개 더 있어요 — 검색으로 좁혀 보세요`), in-use icons with Desk's tooltips (other worktree: disabled), remote rows with `IconCloud`, fetch loader, `원격 목록을 가져오지 못했어요` | `branch_popup`: opening runs `git_branches` and `git_fetch_remotes` (`git_branches` again if `fetched`); holders from `BranchCheckout` (`Worktree` disabled, `MainRepo` switchable); session key via `session_of` |
| 브랜치 전환 modal (local / remote text, `ChangesChoice` 가져가기 / 현재 브랜치에 임시 보관 when dirty, the agent note, fetching line, 취소 / 전환); toast texts | `ModalKind::Switch` -> `git_switch_branch(name, remote, mode)` (carry when clean); modal closes either way, result as toast |
| 새 브랜치 modal (name, `이미 있는 브랜치예요`, 베이스 브랜치 select (local + remote, default current), `ChangesChoice`, 만들기) | `ModalKind::Create` -> `git_create_branch(name, base, mode)`; bases from `git.branches` (`local[].name` + `remote[].short_ref`) |

**API for the footer** (top of `changes_actions.rs`): `Detail::checked_changes() -> Vec<GitChange>`,
`Detail::changed_count() -> usize`, `Detail::all_checked() -> bool`, `Detail::git_status() ->
Option<&GitStatus>`, `App::refresh_repo(sid) -> Task<Message>`.

### Deviations

- Context menus open under the right-clicked row, not at the pointer (iced popovers anchor to a
  widget).
- The list keeps the ide's staged / changes / untracked groups (Desk: one flat list); a path in
  two groups has one checkbox state, and the tab badge / `N changed files` count paths.
- The checkbox is drawn (iced's checkbox has no indeterminate state).
- Desk's notifications are the detail pane's toast; a failed 새 브랜치 shows its error in the
  dialog (Desk: a notification, dialog kept open).
- The 베이스 브랜치 picker is a plain list (local, then remote refs) without Desk's group labels
  and type-to-search.
- No `Filter` input above the Changes rows (not part of this port; the list is grouped).
- Popover heights are capped from an estimate (`capped`), as iced has no max-height container.

### Omissions

- 채팅에 첨부 in the commit menu and dragging commits (no chat composer in the ide).
- Ctrl / middle / double click "keep the tab" variants of rows (the ide has no preview tab).

### Tests

`changes_actions.rs` unit tests (`snapshot_files` kinds and order, stash paths of renames,
checks and the footer API, branch rows / holders / `session_of`). App script
(`script_changes.rs`, phase 2, spliced before the Korean pass): checks / unchecks and select-all,
stashes one checked untracked file with a memo (`60_changes_checkboxes_stash_popover`,
`61_stash_section`) and pops it, the row menu (`62_change_context_menu`) and the discard modal for
a modified + an untracked file (`63_discard_modal`), reveal in Explore, a commit action refused on
a dirty tree (error in the dialog), stash of everything, branch from a commit
(`64_commit_menu`, `65_commit_branch_confirm`) and its revert (`66_history_after_revert`), the
branch picker (`67_branch_picker`) and a switch back carrying an untracked file
(`68_switch_modal`), a new branch leaving the change stashed (`69_new_branch_modal`), then back
and both stashes popped; Korean screenshots `ko_chg_01`..`ko_chg_09` (popover, menu, discard
modal, stash section, drop dialog, commit menu and dialog, branch picker, new branch).

## Changes / History footer (Desk `AiActionBar`)

Shown under the Changes and History tabs (not Explore): 커밋 맡기기, the sync button and the
`…` menu, with the PR strips / job notice above it. State in `footer.rs` (per session and repo),
view in `footer_view.rs`, script steps in `script_footer.rs`. Desk mapping, calls, deviations
and tests: `docs/changes-actions.md` ("Footer UI").

## Adding a tab

1. `detail.rs`: add the `SideTab` variant in Desk's order, with its `code()` (the `ui.json` value)
   and a `parse` arm.
2. `detail_view.rs::side_panel`: add `tab(SideTab::X, tr::side_x(), badge)` in the tab row and a
   `SideTab::X => column![self.repo_selector(live), ...]` content arm (the first arm covers
   sessions without repos).
3. Add `side-x` to every locale.

## Tests

`history.rs` unit tests (relative time buckets, initials, avatar colour, paging / unfold / badge),
`detail.rs` (`commit:` item keys, `history` tab value). App script (`script.rs`): phase 2 shows
History (`init`), commits `src/history.ts` in the session worktree and waits for the refresh
(newest first), no unpushed marks without a remote, unfolds the commit (its file), opens the file
diff and the whole-commit diff (added line checked, item remembered in `ui.json`); screenshots
`52_history_expanded`, `53_history_file_diff`, `54_history_commit_diff`, `ko_hist_01_history`; the
base-branch pass (local bare origin) commits once more and checks the ↑ mark and badge `1`
(`ko_hist_02_unpushed`).
