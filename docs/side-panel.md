# Side panel (Changes / Explore)

The side panel right of the detail pane follows Hermes Desk's `ChangesPanel`
(`powered_ide/src/renderer/src/components/ChangesPanel.tsx`) and its Explore tab
(`ExploreTree.tsx`).

## Hermes Desk behaviour -> ide file

| Hermes Desk | ide |
|---|---|
| Tabs `Changes \| History \| Explore` (`ChangesPanel.tsx`, `Tabs.List grow`), Changes with a count badge | `detail_view.rs::side_panel` builds the tab row with `tab(SideTab, label, count)`; tabs are `detail.rs::SideTab` (order = Desk order). History is not built yet. |
| Selected tab remembered | `DetailUi.side` in `ui.json` (`SideTab::code` / `SideTab::parse`); the old value `"search"` opens Explore. |
| `repo / branch` picker under the tabs (`BranchSelect`) | `detail_view.rs::repo_selector`, shown under the tabs by every tab |
| Explore: `파일 찾기` input with a search icon on the left (`ExploreTree.tsx`, `TextInput leftSection=<IconSearch>`) | `detail_view.rs::explore_search_input` (Tabler `search` icon, `icons::Icon::Search`) |
| Settings button on the input's right (gitignore 숨기기 popover) | The text search's case / word / regex toggles sit there instead (`Aa`, `ab`, `.*`) |
| Empty filter: the tree | `side_panel`, `SideTab::Explore` arm: `d.tree` when `SearchState::active()` is false |
| Typing filters as you type; matching paths replace the tree as flat rows (file icon, full path, truncated at the start), click opens the file | `DetailMsg::SearchInput` -> `App::find_files` (`search.files` for the shown repo, at most 200) -> `DetailMsg::Files`; rows in `detail_view.rs::explore_results` (`DetailMsg::OpenAt(path, 0, 0)`) |
| Revealing a file in the tree clears the filter | `App::tree_reveal` clears `SearchState` |
| (no content search in Desk's Explore) | Text search of every repo below the file names under the `내용` / `Contents` label, with the status (`검색 중…`, counts, errors) next to it; starts 250 ms after the last keystroke (`DetailMsg::SearchTyped`) or at once on Enter (`SearchSubmit`); results grouped by repo and file as before |
| Ctrl+Shift+F (none in Desk) | `Message::FindInFiles` -> `App::open_search`: shows Explore and focuses its input (`SEARCH_ID`) |

## Deviations from Desk

- File names are matched by the server's fuzzy matcher (`search.files`, best first), not Desk's
  case-insensitive substring filter, and only files are listed (Desk also lists matching folders).
- At most 200 file-name matches; Desk shows 500 and a "N개 더 있음" line.
- No gitignore setting: `search.files` and the text search skip ignored files.
- Explore also holds the text search (the former Search tab) under a section label.
- Matched characters are not highlighted in file-name rows (Desk does not either); text matches are
  highlighted as before.

## Adding a tab (History)

1. `detail.rs`: add `SideTab::History` between `Changes` and `Explore`, with `code()` `"history"`
   and a `parse` arm.
2. `detail_view.rs::side_panel`: add `tab(SideTab::History, tr::side_history(), badge)` between the
   two existing `tab(..)` calls, and a `SideTab::History => column![self.repo_selector(live), ...]`
   content arm (the `live.info.repos.is_empty()` arm covers sessions without repos).
3. Add `side-history` to every locale.
