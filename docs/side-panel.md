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
- History omits (not ported): the commit context menu (checkout, branch, revert, cherry-pick, copy
  sha), dragging a commit into the chat, the reveal-in-Explore button on commit file rows, the
  stash section and the AI commit / sync / PR bar under Changes and History. Desk's merge commits
  list nothing; ide shows them against the first parent (`docs/git-history.md`).

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
