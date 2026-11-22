//! Changed files grouped as staged / unstaged / untracked (Hermes Desk `ChangesPanel` rows):
//! collapsible group headers with counts, rows with a file-type icon, the path and a boxed
//! status letter. Clicking a row asks the owner to open its diff. Virtualised (fixed rows).

use crate::theme;
use crate::vlist::VList;
use crate::{GitStatus, icons};
use ide_i18n::tr;
use iced::Widget as _;
use iced::widget::scrollable::Viewport;
use iced::widget::{button, container, row, space, text};
use iced::{Alignment, Element, Length, Padding, Task};

pub const ROW_H: f32 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeGroup {
    Staged,
    Unstaged,
    Untracked,
}

impl ChangeGroup {
    pub const ALL: [ChangeGroup; 3] = [ChangeGroup::Staged, ChangeGroup::Unstaged, ChangeGroup::Untracked];

    pub fn label(self) -> String {
        match self {
            ChangeGroup::Staged => tr::changes_staged(),
            ChangeGroup::Unstaged => tr::changes_unstaged(),
            ChangeGroup::Untracked => tr::changes_untracked(),
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One changed file.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeEntry {
    /// Repo-relative `/`-separated path (the new path for renames).
    pub path: String,
    /// Previous path of a rename.
    pub old_path: Option<String>,
    pub status: GitStatus,
}

impl ChangeEntry {
    pub fn new(path: impl Into<String>, status: GitStatus) -> Self {
        Self {
            path: path.into(),
            old_path: None,
            status,
        }
    }

    pub fn renamed(old: impl Into<String>, new: impl Into<String>) -> Self {
        Self {
            path: new.into(),
            old_path: Some(old.into()),
            status: GitStatus::Renamed,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChangesMessage {
    /// Collapse / expand a group.
    ToggleGroup(ChangeGroup),
    /// Click on entry `index` of `group`.
    Open(ChangeGroup, usize),
    Scrolled(Viewport),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangesEvent {
    /// Open the diff of `path` (staged diff for [`ChangeGroup::Staged`]).
    OpenDiff { group: ChangeGroup, path: String },
}

#[derive(Debug, Clone, Copy)]
enum Row {
    Header(ChangeGroup),
    Entry(ChangeGroup, usize),
}

/// The changes list's state.
pub struct ChangesList {
    groups: [Vec<ChangeEntry>; 3],
    collapsed: [bool; 3],
    selected: Option<(ChangeGroup, String)>,
    rows: Vec<Row>,
    list: VList,
    /// Shown rows: paths containing this (case-insensitive); empty = all.
    filter: String,
}

impl Default for ChangesList {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangesList {
    pub fn new() -> Self {
        Self {
            groups: Default::default(),
            collapsed: [false; 3],
            selected: None,
            rows: Vec::new(),
            list: VList::default(),
            filter: String::new(),
        }
    }

    /// Shows only the rows whose path contains `filter` (case-insensitive; Desk's Changes
    /// `Filter`). Groups without such rows are hidden.
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.rebuild();
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Indexes of `group`'s entries that pass the filter.
    fn shown(&self, g: ChangeGroup) -> Vec<usize> {
        let q = self.filter.trim().to_lowercase();
        self.groups[g.index()]
            .iter()
            .enumerate()
            .filter(|(_, e)| q.is_empty() || e.path.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    /// Replaces the entries of `group` (kept in the given order).
    pub fn set_group(&mut self, group: ChangeGroup, entries: Vec<ChangeEntry>) {
        self.groups[group.index()] = entries;
        self.rebuild();
    }

    /// Replaces all three groups.
    pub fn set_all(&mut self, staged: Vec<ChangeEntry>, unstaged: Vec<ChangeEntry>, untracked: Vec<ChangeEntry>) {
        self.groups = [staged, unstaged, untracked];
        self.rebuild();
    }

    pub fn entries(&self, group: ChangeGroup) -> &[ChangeEntry] {
        &self.groups[group.index()]
    }

    /// Total number of changed entries over all groups.
    pub fn total(&self) -> usize {
        self.groups.iter().map(Vec::len).sum()
    }

    /// Highlights the row of `path` in `group` (e.g. the diff that is open), or nothing.
    pub fn set_selected(&mut self, sel: Option<(ChangeGroup, String)>) {
        self.selected = sel;
    }

    pub fn selected(&self) -> Option<(ChangeGroup, &str)> {
        self.selected.as_ref().map(|(g, p)| (*g, p.as_str()))
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        for g in ChangeGroup::ALL {
            let shown = self.shown(g);
            if shown.is_empty() {
                continue;
            }
            rows.push(Row::Header(g));
            if !self.collapsed[g.index()] {
                rows.extend(shown.into_iter().map(|i| Row::Entry(g, i)));
            }
        }
        self.rows = rows;
    }

    pub fn update(&mut self, msg: ChangesMessage) -> (Task<ChangesMessage>, Option<ChangesEvent>) {
        match msg {
            ChangesMessage::ToggleGroup(g) => {
                self.collapsed[g.index()] ^= true;
                self.rebuild();
                (Task::none(), None)
            }
            ChangesMessage::Open(g, i) => {
                let Some(e) = self.groups[g.index()].get(i) else {
                    return (Task::none(), None);
                };
                self.selected = Some((g, e.path.clone()));
                (
                    Task::none(),
                    Some(ChangesEvent::OpenDiff {
                        group: g,
                        path: e.path.clone(),
                    }),
                )
            }
            ChangesMessage::Scrolled(v) => {
                self.list.on_scroll(&v);
                (Task::none(), None)
            }
        }
    }

    pub fn view(&self) -> Element<'_, ChangesMessage> {
        if self.rows.is_empty() {
            return container(text(tr::changes_empty()).size(12).color(theme::DIM))
                .padding([10, 12])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::bg(theme::PANEL))
                .boxed();
        }
        container(self.list.view(
            self.rows.len(),
            ROW_H,
            None,
            move |i| self.row_view(i),
            ChangesMessage::Scrolled,
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::bg(theme::PANEL))
        .boxed()
    }

    /// Like [`Self::view`], but the owner draws the file rows (`entry(group, index, entry,
    /// selected)`, e.g. with checkboxes and row actions in its own message type); group headers
    /// and scrolling stay here and are mapped with `map`.
    pub fn view_with<'a, M: 'static>(
        &'a self,
        entry: impl Fn(ChangeGroup, usize, &'a ChangeEntry, bool) -> Element<'a, M> + 'a,
        map: impl Fn(ChangesMessage) -> M + Clone + 'a,
    ) -> Element<'a, M> {
        if self.rows.is_empty() {
            return self.view().map(map).boxed();
        }
        let on_scroll = {
            let map = map.clone();
            move |v| map(ChangesMessage::Scrolled(v))
        };
        container(self.list.view(
            self.rows.len(),
            ROW_H,
            None,
            move |i| match self.rows[i] {
                Row::Header(_) => self.row_view(i).map(map.clone()).boxed(),
                Row::Entry(g, k) => {
                    let e = &self.groups[g.index()][k];
                    let selected = self
                        .selected
                        .as_ref()
                        .is_some_and(|(sg, p)| *sg == g && *p == e.path);
                    iced::widget::column![
                        entry(g, k, e, selected),
                        container(space()).width(Length::Fill).height(1).style(theme::bg(theme::BORDER))
                    ]
                    .boxed()
                }
            },
            on_scroll,
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::bg(theme::PANEL))
        .boxed()
    }

    fn row_view(&self, i: usize) -> Element<'_, ChangesMessage> {
        match self.rows[i] {
            Row::Header(g) => {
                let open = !self.collapsed[g.index()];
                let n = self.shown(g).len();
                button(
                    row![
                        text(if open { "▾" } else { "▸" }).size(12).color(theme::DIM).width(10),
                        text(g.label()).size(12).font(theme::BOLD).color(theme::DIM),
                        container(text(n.to_string()).size(10).font(theme::BOLD))
                            .padding([0, 5])
                            .style(|_| container::Style {
                                background: Some(theme::HOVER.into()),
                                text_color: Some(theme::TEXT),
                                border: iced::Border {
                                    radius: 8.0.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .height(ROW_H)
                    .padding(Padding {
                        left: 10.0,
                        right: 10.0,
                        ..Padding::ZERO
                    }),
                )
                .padding(0)
                .width(Length::Fill)
                .style(theme::row(false))
                .on_press(ChangesMessage::ToggleGroup(g))
                .boxed()
            }
            Row::Entry(g, k) => {
                let e = &self.groups[g.index()][k];
                let selected = self
                    .selected
                    .as_ref()
                    .is_some_and(|(sg, p)| *sg == g && *p == e.path);
                let label = match &e.old_path {
                    Some(o) => format!("{o} → {}", e.path),
                    None => e.path.clone(),
                };
                let c = e.status.color();
                let letter = theme::tip(
                    container(
                        text(e.status.letter().to_string())
                            .size(10)
                            .font(theme::MONO_BOLD)
                            .line_height(1.0),
                    )
                    .center(16)
                    .style(theme::status_box(c))
                    .boxed(),
                    e.status.label(),
                );
                let line = row![
                    icons::file_icon(&e.path),
                    text(label)
                        .size(12)
                        .color(if e.status == GitStatus::Deleted { theme::DIM } else { theme::TEXT })
                        .wrapping(text::Wrapping::None)
                        .width(Length::Fill),
                    letter,
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .height(ROW_H)
                .padding(Padding {
                    left: 22.0,
                    right: 12.0,
                    ..Padding::ZERO
                });
                let b = button(line)
                    .padding(0)
                    .width(Length::Fill)
                    .style(theme::row(selected))
                    .on_press(ChangesMessage::Open(g, k));
                iced::widget::column![
                    b,
                    container(space()).width(Length::Fill).height(1).style(theme::bg(theme::BORDER))
                ]
                .boxed()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_must_use)]
    use super::*;

    #[test]
    fn groups_counts_and_open() {
        let mut c = ChangesList::new();
        c.set_all(
            vec![ChangeEntry::new("a.rs", GitStatus::Added)],
            vec![
                ChangeEntry::new("b.rs", GitStatus::Modified),
                ChangeEntry::renamed("c.rs", "d.rs"),
            ],
            vec![],
        );
        assert_eq!(c.total(), 3);
        // 2 headers + 3 rows (empty untracked group hidden).
        assert_eq!(c.rows.len(), 5);
        let (_, ev) = c.update(ChangesMessage::Open(ChangeGroup::Unstaged, 1));
        assert_eq!(
            ev,
            Some(ChangesEvent::OpenDiff {
                group: ChangeGroup::Unstaged,
                path: "d.rs".into()
            })
        );
        assert_eq!(c.selected(), Some((ChangeGroup::Unstaged, "d.rs")));
        c.update(ChangesMessage::ToggleGroup(ChangeGroup::Unstaged));
        assert_eq!(c.rows.len(), 3);
    }
}
