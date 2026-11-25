//! Drawing of the Changes / History tab actions (`changes_actions.rs`): the Changes tab body
//! (select-all + `N changed files` + 임시 보관 button and popover, rows with checkboxes, hover
//! buttons and the right-click menu, the stash section), the `repo / branch` picker, the History
//! commit menu, the reveal button of commit file rows and the dialogs.

use iced::widget::{
    Column, Row, button, column, container, hover, mouse_area, popover, row, scrollable,
    space, text, text_input,
};
use iced::{Alignment, Color, Element, Fill, Length, Padding, Point, Widget as _};
use ide_client::{ChangeKind, CommitInfo, StashAction, SwitchMode};
use ide_views::{ChangeEntry, ChangeGroup, icons as file_icons};

use crate::app::{App, Live, Message};
use crate::changes_actions::{
    ActMsg, BRANCH_QUERY_ID, ChangesActs, ChangesModal, CommitKind, MAX_ROWS, MODAL_INPUT_ID,
    ModalKind, STASH_MEMO_ID, branch_rows, session_of, stash_title,
};
use crate::detail::{DetailMsg, file_name, view_status};
use crate::history;
use crate::i18n::{self, tr};
use crate::icons::{Icon, icon};
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Space the hover buttons of a change row leave for the kind box on the right.
const KIND_W: f32 = 12.0 + 16.0 + 8.0;

/// `push` for element columns / rows that boxes its argument (widgets or elements).
trait Add<'a> {
    fn add(self, x: impl iced::Widget<Message> + 'a) -> Self;
}

impl<'a> Add<'a> for Column<El<'a>> {
    fn add(self, x: impl iced::Widget<Message> + 'a) -> Self {
        self.push(x.boxed())
    }
}

impl<'a> Add<'a> for Row<El<'a>> {
    fn add(self, x: impl iced::Widget<Message> + 'a) -> Self {
        self.push(x.boxed())
    }
}

/// `content` as it is when its estimated height fits `cap`, else scrolled in `cap` px (Desk's
/// `ScrollArea.Autosize mah`).
fn capped<'a>(content: impl iced::Widget<Message> + 'a, est: f32, cap: f32) -> El<'a> {
    if est <= cap {
        content.boxed()
    } else {
        container(scrollable(content.boxed()).height(Fill))
            .height(cap)
            .boxed()
    }
}

fn act(sid: &str, m: ActMsg) -> Message {
    Message::Detail(sid.to_owned(), DetailMsg::Acts(m))
}

/// Mantine-like checkbox: `Some(true)` checked, `Some(false)` empty, `None` indeterminate.
pub fn check_box<'a>(state: Option<bool>, on_press: Message) -> El<'a> {
    let on = state != Some(false);
    let mark: El<'a> = match state {
        Some(true) => icon(Icon::Check, 12.0, Color::WHITE),
        None => icon(Icon::Minus, 12.0, Color::WHITE),
        Some(false) => space().width(12).boxed(),
    };
    button(container(mark).center(16).style(move |_| {
        container::Style {
            background: Some(
                if on {
                    style::ACCENT
                } else {
                    Color::from_rgb8(0x16, 0x17, 0x1a)
                }
                .into(),
            ),
            border: iced::Border {
                color: if on {
                    style::ACCENT
                } else {
                    Color::from_rgb8(0x5c, 0x5f, 0x66)
                },
                width: 1.0,
                radius: 3.0.into(),
            },
            ..Default::default()
        }
    }))
    .padding(0)
    .style(|_, _| button::Style::default())
    .on_press(on_press)
    .boxed()
}

/// Small icon-only button with a tooltip (Desk's `ActionIcon size="xs" variant="subtle"`).
fn action_icon<'a>(i: Icon, color: Color, label: String, msg: Option<Message>) -> El<'a> {
    let enabled = msg.is_some();
    tip(
        button(icon(i, 14.0, if enabled { color } else { style::FAINT }))
            .padding(3)
            .style(style::icon)
            .on_press_maybe(msg)
            .boxed(),
        label,
    )
}

/// Desk's `.row-reveal`: buttons that appear over the right end of a row while it is hovered;
/// `right` keeps the row's own trailing content (the kind box) visible.
pub fn row_actions<'a>(base: El<'a>, actions: Vec<El<'a>>, right: f32) -> El<'a> {
    hover(
        base,
        container(row(actions).spacing(2).align_y(Alignment::Center))
            .padding(Padding {
                right,
                ..Padding::ZERO
            })
            .width(Fill)
            .height(Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::Center),
    )
    .boxed()
}

/// The `RevealButton` (탐색기에서 보기) of a file row.
pub fn reveal_button<'a>(sid: &str, path: &str) -> El<'a> {
    action_icon(
        Icon::ArrowRight,
        style::DIM,
        tr::chg_reveal(),
        Some(act(sid, ActMsg::Reveal(path.to_owned()))),
    )
}

/// The change-kind letter box (Changes list / commit file rows).
fn kind_box<'a>(st: ide_views::GitStatus) -> El<'a> {
    tip(
        container(
            text(st.letter().to_string())
                .size(10)
                .font(ide_views::theme::mono_bold())
                .line_height(1.0),
        )
        .center(16)
        .style(ide_views::theme::status_box(st.color()))
        .boxed(),
        st.label(),
    )
}

/// A context-menu item (Mantine `Menu.Item`).
fn menu_item<'a>(i: Option<Icon>, label: String, color: Color, msg: Message) -> El<'a> {
    menu_item_maybe(i, label, color, Some(msg))
}

/// A menu item that is greyed out without a message.
fn menu_item_maybe<'a>(i: Option<Icon>, label: String, color: Color, msg: Option<Message>) -> El<'a> {
    let color = if msg.is_some() { color } else { style::FAINT };
    let mut r = Row::<El<'a>>::new().spacing(8).align_y(Alignment::Center);
    if let Some(i) = i {
        r = r.add(icon(i, 14.0, color));
    }
    r = r.add(text(label).size(12).color(color));
    button(r)
        .padding([5, 10])
        .width(Fill)
        .style(style::row(false))
        .on_press_maybe(msg)
        .boxed()
}

fn menu_card<'a>(items: Column<El<'a>>, width: f32) -> El<'a> {
    container(items.spacing(1))
        .padding(4)
        .width(width)
        .style(style::modal_card)
        .boxed()
}

fn menu_divider<'a>() -> El<'a> {
    container(space())
        .height(1)
        .width(Fill)
        .style(style::bg(style::BORDER))
        .padding([4, 0])
        .boxed()
}

fn count_badge<'a>(n: usize) -> El<'a> {
    container(text(n.to_string()).size(10).font(style::bold()))
        .padding([0, 5])
        .style(|_| container::Style {
            background: Some(style::BORDER.into()),
            text_color: Some(style::TEXT),
            border: iced::Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .boxed()
}

fn error_text<'a>(e: &str) -> El<'a> {
    container(text(e.to_owned()).size(12))
        .padding(8)
        .width(Fill)
        .style(style::error_box)
        .boxed()
}

fn dialog_buttons<'a>(cancel: Option<Message>, ok: El<'a>) -> El<'a> {
    row![
        space().width(Fill),
        button(text(tr::cancel()).size(13))
            .padding([6, 14])
            .style(style::secondary)
            .on_press_maybe(cancel),
        ok,
    ]
    .spacing(8)
    .boxed()
}

fn busy_label(busy: bool, label: String) -> String {
    if busy { format!("{label}…") } else { label }
}

impl App {
    // ------------------------------------------------------------------ Changes tab

    /// The Changes tab under the picker: the head line, the rows and the stash section.
    pub fn changes_tab<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.clone();
        let s2 = sid.clone();
        let list = d.changes.view_with(
            move |g, k, e, selected| self.change_row(live, g, k, e, selected),
            move |m| Message::Detail(s2.clone(), DetailMsg::Changes(m)),
        );
        let filter = container(
            text_input(tr::chg_filter(), d.changes.filter())
                .on_input({
                    let sid = sid.clone();
                    move |v| act(&sid, ActMsg::Filter(v))
                })
                .size(12)
                .padding([4, 8])
                .style(style::input),
        )
        .padding(Padding {
            top: 8.0,
            right: 10.0,
            bottom: 0.0,
            left: 10.0,
        });
        column![
            filter,
            self.changes_head(live),
            list,
            self.stash_section(live),
        ]
        .boxed()
    }

    /// Select-all, `N changed files`, the 임시 보관 button with its popover.
    fn changes_head<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let a = &d.acts;
        let sid = live.info.id.as_str();
        let total = d.changed_count();
        let checked = d.checked_changes().len();
        let mut r = Row::<El<'a>>::new().spacing(8).align_y(Alignment::Center);
        if d.status_error.is_none() && total > 0 {
            let state = if checked == total {
                Some(true)
            } else if checked == 0 {
                Some(false)
            } else {
                None
            };
            r = r.add(tip(
                check_box(state, act(sid, ActMsg::CheckAll(checked != total))),
                tr::chg_select_all(),
            ));
        }
        r = r.add(
            text(match &d.status_error {
                Some(e) => e.clone(),
                None => tr::changes_count(i18n::count(total)),
            })
            .size(11)
            .color(if d.status_error.is_some() {
                style::RED
            } else {
                style::DIM
            })
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End)
            .width(Fill),
        );
        let partial = checked < total;
        let enabled = d.status_error.is_none() && checked > 0;
        let label = if partial {
            tr::chg_stash_checked_tip(i18n::count(checked))
        } else {
            tr::chg_stash_all_tip()
        };
        let trigger = tip(
            button(icon(
                Icon::Archive,
                14.0,
                if enabled { style::DIM } else { style::FAINT },
            ))
            .padding(3)
            .style(style::icon_toggle(a.stash.open))
            .on_press_maybe(enabled.then(|| act(sid, ActMsg::StashToggle)))
            .boxed(),
            label,
        );
        let popup = a.stash.open.then(|| {
            let body = if partial {
                tr::chg_stash_checked_body(i18n::count(checked))
            } else {
                tr::chg_stash_all_body(i18n::count(checked))
            };
            let busy = a.stash.busy;
            container(
                column![
                    text(body).size(12),
                    text_input(tr::chg_stash_memo(), &a.stash.memo)
                        .id(STASH_MEMO_ID)
                        .on_input({
                            let sid = sid.to_owned();
                            move |v| act(&sid, ActMsg::StashMemo(v))
                        })
                        .on_submit(act(sid, ActMsg::StashSubmit))
                        .size(12)
                        .padding([5, 8])
                        .style(style::input),
                    text(tr::chg_stash_untracked()).size(11).color(style::DIM),
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(12))
                            .padding([4, 10])
                            .style(style::secondary)
                            .on_press_maybe((!busy).then(|| act(sid, ActMsg::StashClose))),
                        button(text(busy_label(busy, tr::chg_stash_confirm())).size(12))
                            .padding([4, 10])
                            .style(style::primary)
                            .on_press_maybe((!busy).then(|| act(sid, ActMsg::StashSubmit))),
                    ]
                    .spacing(6),
                ]
                .spacing(8),
            )
            .padding(10)
            .width(260)
            .style(style::modal_card)
        });
        r = r.add(
            popover(trigger, popup)
                .position(iced::widget::popover::Position::Bottom)
                .gap(4)
                .on_close(act(sid, ActMsg::StashClose)),
        );
        container(r)
            .padding(Padding {
                top: 8.0,
                right: 10.0,
                bottom: 6.0,
                left: 10.0,
            })
            .width(Fill)
            .boxed()
    }

    /// A changed file: checkbox, icon, path, hover buttons (변경 취소, 탐색기에서 보기), kind;
    /// click opens the diff, right-click the menu.
    fn change_row<'a>(
        &'a self,
        live: &'a Live,
        g: ChangeGroup,
        k: usize,
        e: &'a ChangeEntry,
        selected: bool,
    ) -> El<'a> {
        let a = &live.detail.acts;
        let sid = live.info.id.as_str();
        let path = e.path.clone();
        let label = match &e.old_path {
            Some(o) => format!("{o} → {}", e.path),
            None => e.path.clone(),
        };
        let line = row![
            check_box(
                Some(a.is_checked(&e.path)),
                act(sid, ActMsg::Check(path.clone(), !a.is_checked(&e.path)))
            ),
            file_icons::file_icon(&e.path),
            text(label)
                .size(12)
                .color(if e.status == ide_views::GitStatus::Deleted {
                    style::DIM
                } else {
                    style::TEXT
                })
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::Start)
                .width(Fill),
            kind_box(e.status),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .height(ide_views::changes::ROW_H)
        .padding(Padding {
            left: 18.0,
            right: 12.0,
            ..Padding::ZERO
        });
        let base = button(line)
            .padding(0)
            .width(Fill)
            .style(style::row(selected))
            .on_press(Message::Detail(
                sid.to_owned(),
                DetailMsg::Changes(ide_views::ChangesMessage::Open(g, k)),
            ))
            .boxed();
        let actions = vec![
            action_icon(
                Icon::ArrowBackUp,
                style::RED,
                tr::chg_discard_tip(),
                Some(act(sid, ActMsg::Discard(vec![path.clone()]))),
            ),
            reveal_button(sid, &path),
        ];
        let el = mouse_area(row_actions(base, actions, KIND_W))
            .on_right_press(act(sid, ActMsg::Menu(Some((g, path.clone())))));
        at_pointer(el.boxed())
    }

    /// Desk's `StashSection`: this branch's stashes (all on request) with pop / apply / drop;
    /// nothing while the repository has no stash.
    fn stash_section<'a>(&'a self, live: &'a Live) -> El<'a> {
        let a: &'a ChangesActs = &live.detail.acts;
        let sid = live.info.id.as_str();
        let Some(list) = a.stash.list.as_ref().filter(|l| !l.is_empty()) else {
            return space().height(0).boxed();
        };
        let branch = a.branch_name().unwrap_or("");
        let mine: Vec<_> = list.iter().filter(|s| s.branch == branch).collect();
        let others = list.len() - mine.len();
        let shown: Vec<_> = if a.stash.show_all {
            list.iter().collect()
        } else {
            mine
        };
        let open = a.stash.section_open;
        let mut head = row![
            icon(
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                12.0,
                style::DIM
            ),
            icon(Icon::Archive, 14.0, style::DIM),
            text(tr::chg_stash_title()).size(12).color(style::DIM),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if !shown.is_empty() {
            head = head.add(count_badge(shown.len()));
        }
        let mut col = Column::<El<'a>>::new().add(
            button(head)
                .padding([6, 12])
                .width(Fill)
                .style(style::row(false))
                .on_press(act(sid, ActMsg::StashSection)),
        );
        if open {
            let now = history::now_secs();
            let mut rows = Column::<El<'a>>::new();
            // Estimated height (rows ~30 px, file rows ~26 px).
            let mut est = 0.0f32;
            for s in shown {
                est += 30.0;
                let expanded = a.stash.expanded.contains(&s.sha);
                let busy_on = |x: StashAction| {
                    a.stash
                        .acting
                        .as_ref()
                        .is_some_and(|(sha, act)| *sha == s.sha && *act == x)
                };
                let idle = a.stash.acting.is_none();
                let btn = |i: Icon, color: Color, label: String, x: StashAction| -> El<'a> {
                    if busy_on(x) {
                        return text("…").size(12).color(style::DIM).width(20).boxed();
                    }
                    action_icon(
                        i,
                        color,
                        label,
                        idle.then(|| act(sid, ActMsg::StashAct(s.sha.clone(), x))),
                    )
                };
                let mut meta = tr::chg_stash_meta(
                    i18n::count(s.files.len()),
                    &history::relative_time(s.date, now),
                );
                if s.branch != branch {
                    let b = if s.branch.is_empty() {
                        tr::chg_no_branch()
                    } else {
                        s.branch.clone()
                    };
                    meta = format!("{meta} · {b}");
                }
                let title = stash_title(s);
                rows = rows.add(
                    button(
                        row![
                            icon(
                                if expanded {
                                    Icon::ChevronDown
                                } else {
                                    Icon::ChevronRight
                                },
                                12.0,
                                style::DIM
                            ),
                            tip(
                                text(title.clone())
                                    .size(12)
                                    .wrapping(text::Wrapping::None)
                                    .ellipsis(text::Ellipsis::End)
                                    .width(Fill)
                                    .boxed(),
                                title,
                            ),
                            text(meta)
                                .size(11)
                                .color(style::DIM)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End)
                                .width(Length::Shrink),
                            btn(
                                Icon::PackageExport,
                                style::DIM,
                                tr::chg_stash_pop_tip(),
                                StashAction::Pop
                            ),
                            btn(
                                Icon::CopyCheck,
                                style::DIM,
                                tr::chg_stash_apply_tip(),
                                StashAction::Apply
                            ),
                            btn(
                                Icon::Trash,
                                style::RED,
                                tr::chg_stash_drop_tip(),
                                StashAction::Drop
                            ),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding(Padding {
                        top: 4.0,
                        right: 12.0,
                        bottom: 4.0,
                        left: 20.0,
                    })
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(act(sid, ActMsg::StashExpand(s.sha.clone()))),
                );
                if expanded {
                    est += s.files.len() as f32 * 26.0;
                    for f in &s.files {
                        let label = match &f.orig_path {
                            Some(o) => format!("{o} → {}", f.path),
                            None => f.path.clone(),
                        };
                        rows = rows.add(
                            container(
                                row![
                                    file_icons::file_icon(file_name(&f.path)),
                                    tip(
                                        text(f.path.clone())
                                            .size(12)
                                            .wrapping(text::Wrapping::None)
                                            .ellipsis(text::Ellipsis::Start)
                                            .width(Fill)
                                            .boxed(),
                                        label,
                                    ),
                                    kind_box(view_status(f.kind)),
                                ]
                                .spacing(8)
                                .align_y(Alignment::Center),
                            )
                            .padding(Padding {
                                top: 4.0,
                                right: 12.0,
                                bottom: 4.0,
                                left: 40.0,
                            })
                            .width(Fill),
                        );
                    }
                }
            }
            if others > 0 {
                est += 26.0;
                rows = rows.add(
                    button(
                        text(if a.stash.show_all {
                            tr::chg_stash_mine()
                        } else {
                            tr::chg_stash_others(i18n::count(others))
                        })
                        .size(12)
                        .color(style::DIM),
                    )
                    .padding(Padding {
                        top: 4.0,
                        right: 12.0,
                        bottom: 6.0,
                        left: 20.0,
                    })
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(act(sid, ActMsg::StashShowAll)),
                );
            }
            col = col.add(capped(rows, est, 220.0));
        }
        column![
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER)),
            container(col).width(Fill).style(style::bg(style::SIDEBAR)),
        ]
        .boxed()
    }

    // ------------------------------------------------------------------ repo / branch picker

    /// Desk's `BranchSelect` button: `repo / branch`; the popover lists the session's repos and
    /// the repo's branches (local, remote), with 새 브랜치 and a search box.
    pub fn repo_branch_picker<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let a = &d.acts;
        let sid = live.info.id.as_str();
        let repo = live.info.repos.iter().find(|r| r.repo == d.repo);
        let repo_name = repo.map(|r| r.name.clone()).unwrap_or_default();
        let current = a.branch_name().map(str::to_owned);
        let branch_text = current.clone().unwrap_or_else(|| "—".to_owned());
        let title = format!(
            "{repo_name} / {}",
            current.clone().unwrap_or_else(tr::chg_no_branch)
        );
        let refresh = tip(
            button(icon(Icon::Refresh, 14.0, style::DIM))
                .padding(3)
                .style(style::icon)
                .on_press(act(sid, ActMsg::Refresh))
                .boxed(),
            tr::chg_refresh(),
        );
        let btn = tip(
            button(
                row![
                    icon(Icon::GitBranch, 14.0, style::DIM),
                    text(repo_name)
                        .size(12)
                        .font(style::bold())
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End)
                        .width(Length::Shrink),
                    text("/").size(12).color(style::DIM),
                    text(branch_text)
                        .size(12)
                        .color(Color::from_rgb8(0xce, 0xd4, 0xda))
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End)
                        .width(Fill),
                    refresh,
                    icon(Icon::ChevronDown, 14.0, style::DIM),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding(Padding {
                top: 2.0,
                right: 8.0,
                bottom: 2.0,
                left: 10.0,
            })
            .height(30)
            .width(Fill)
            .style(move |_, status| button::Style {
                background: Some(
                    match status {
                        button::Status::Hovered | button::Status::Pressed => style::HOVER,
                        _ => Color::from_rgb8(0x16, 0x17, 0x1a),
                    }
                    .into(),
                ),
                text_color: style::TEXT,
                border: iced::Border {
                    color: if a.branch.open {
                        style::ACCENT
                    } else {
                        Color::from_rgb8(0x37, 0x3a, 0x40)
                    },
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
            .on_press(act(sid, ActMsg::BranchToggle))
            .boxed(),
            title,
        );
        let popup = a.branch.open.then(|| self.branch_popup(live));
        popover(btn, popup)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(act(sid, ActMsg::BranchClose))
            .boxed()
    }

    fn branch_popup<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let a = &d.acts;
        let sid = live.info.id.as_str();
        let section = |label: String| -> El<'a> {
            container(text(label).size(11).font(style::bold()).color(style::DIM))
                .padding(Padding {
                    top: 8.0,
                    right: 12.0,
                    bottom: 4.0,
                    left: 12.0,
                })
                .boxed()
        };
        let group = |label: String, extra: Option<&'static str>| -> El<'a> {
            let mut r = row![text(label).size(11).color(style::FAINT)].spacing(6);
            if let Some(x) = extra {
                r = r.add(text(x).size(11).color(style::DIM));
            }
            container(r)
                .padding(Padding {
                    top: 6.0,
                    right: 8.0,
                    bottom: 2.0,
                    left: 8.0,
                })
                .boxed()
        };
        let note = |t: String| -> El<'a> {
            container(text(t).size(11).color(style::DIM))
                .padding([4, 10])
                .boxed()
        };
        // Repos.
        let mut repos = Column::<El<'a>>::new();
        for r in &live.info.repos {
            repos = repos.add(picker_row(
                r.name.clone(),
                r.repo == d.repo,
                false,
                None,
                Some(act(sid, ActMsg::PickRepo(r.repo.clone()))),
            ));
        }
        // Branches.
        let q = a.branch.query.trim().to_lowercase();
        let current = a.branch_name();
        let mut list = Column::<El<'a>>::new();
        let (locals, remotes) = match &a.branch.list {
            Some(b) => (
                branch_rows(b),
                b.remote.iter().filter(|r| !r.has_local).collect::<Vec<_>>(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let locals: Vec<_> = locals
            .into_iter()
            .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q))
            .collect();
        let remotes: Vec<_> = remotes
            .into_iter()
            .filter(|r| q.is_empty() || r.short_ref.to_lowercase().contains(&q))
            .collect();
        // Rows of 28 px, plus group labels and notes.
        let list_est =
            (locals.len().min(MAX_ROWS) + remotes.len().min(MAX_ROWS)) as f32 * 28.0 + 80.0;
        let has_remote = a
            .branch
            .list
            .as_ref()
            .is_some_and(|b| b.remote.iter().any(|r| !r.has_local));
        if has_remote {
            list = list.add(group(tr::chg_local(), None));
        }
        for crate::changes_actions::BranchRow {
            name,
            disabled,
            holder,
        } in locals.iter().take(MAX_ROWS)
        {
            let active = Some(name.as_str()) == current;
            let badge = holder.as_ref().filter(|_| !active).map(|(path, main)| {
                let (i, label) = if *main {
                    (Icon::Folder, tr::chg_in_use_main(path))
                } else if let Some(s) = session_of(path) {
                    (Icon::Messages, tr::chg_in_use_session(&s))
                } else {
                    (Icon::Folder, tr::chg_in_use_path(path))
                };
                tip(icon(i, 14.0, style::DIM), label)
            });
            let disabled = *disabled && !active;
            list = list.add(picker_row(
                name.clone(),
                active,
                disabled,
                badge,
                (!disabled && !active).then(|| act(sid, ActMsg::PickBranch(name.clone(), None))),
            ));
        }
        if locals.len() > MAX_ROWS {
            list = list.add(note(tr::chg_more_rows(i18n::count(
                locals.len() - MAX_ROWS,
            ))));
        }
        if locals.is_empty() {
            list = list.add(note(tr::chg_no_branch()));
        }
        if !remotes.is_empty() || a.branch.fetching {
            list = list.add(group(tr::chg_remote(), a.branch.fetching.then_some("…")));
        }
        for r in remotes.iter().take(MAX_ROWS) {
            list = list.add(picker_row(
                r.short_ref.clone(),
                false,
                false,
                Some(tip(
                    icon(Icon::Cloud, 14.0, style::DIM),
                    tr::chg_remote_tip(),
                )),
                Some(act(
                    sid,
                    ActMsg::PickBranch(r.branch.clone(), Some(r.remote.clone())),
                )),
            ));
        }
        if remotes.len() > MAX_ROWS {
            list = list.add(note(tr::chg_more_rows(i18n::count(
                remotes.len() - MAX_ROWS,
            ))));
        }
        if let Some(e) = &a.branch.fetch_failed {
            let n = note(tr::chg_fetch_failed());
            list = list.add(if e.is_empty() { n } else { tip(n, e.clone()) });
        }
        let branches_head = container(
            row![
                text(tr::chg_picker_branches())
                    .size(11)
                    .font(style::bold())
                    .color(style::DIM),
                space().width(Fill),
                button(
                    row![
                        icon(Icon::Plus, 12.0, style::ACCENT_TEXT),
                        text(tr::chg_new_branch()).size(11).color(style::ACCENT_TEXT),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([2, 6])
                .style(style::icon)
                .on_press(act(sid, ActMsg::NewBranch)),
            ]
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            top: 6.0,
            right: 8.0,
            bottom: 4.0,
            left: 12.0,
        });
        let search = container(
            text_input(tr::chg_branch_search(), &a.branch.query)
                .id(BRANCH_QUERY_ID)
                .on_input({
                    let sid = sid.to_owned();
                    move |v| act(&sid, ActMsg::BranchQuery(v))
                })
                .size(12)
                .padding([4, 8])
                .style(style::input),
        )
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 4.0,
            left: 8.0,
        });
        container(column![
            section(tr::chg_picker_repos()),
            capped(
                container(repos).padding([0, 4]),
                live.info.repos.len() as f32 * 28.0,
                160.0
            ),
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER)),
            branches_head,
            search,
            capped(
                container(list).padding(Padding {
                    top: 0.0,
                    right: 4.0,
                    bottom: 6.0,
                    left: 4.0,
                }),
                list_est,
                240.0
            ),
        ])
        .width(self.side_w() - 16.0)
        .style(style::modal_card)
        .boxed()
    }

    // ------------------------------------------------------------------ History

    /// A History commit row with Desk's right-click `CommitContextMenu`.
    pub fn commit_menu_wrap<'a>(
        &'a self,
        live: &'a Live,
        c: &'a CommitInfo,
        row_el: El<'a>,
    ) -> El<'a> {
        let sid = live.info.id.as_str();
        at_pointer(
            mouse_area(row_el)
                .on_right_press(act(sid, ActMsg::CommitMenu(Some(c.sha.clone()))))
                .boxed(),
        )
    }

    /// The open Changes-row or commit menu, drawn at the pointer over the app window
    /// (`space()` when none is open).
    pub fn side_menu_layer(&self) -> El<'_> {
        let Some(live) = self.shown_live() else {
            return space().boxed();
        };
        let a = &live.detail.acts;
        let sid = live.info.id.as_str();
        let (card, width, rows) = if let Some((_, path)) = &a.menu {
            let checked: Vec<String> = live
                .detail
                .checked_changes()
                .into_iter()
                .map(|f| f.path)
                .collect();
            let mut items = Column::<El<'_>>::new().add(menu_item(
                Some(Icon::ArrowBackUp),
                tr::chg_discard(),
                style::RED,
                act(sid, ActMsg::Discard(vec![path.clone()])),
            ));
            let mut rows = 3.0;
            if checked.len() > 1 {
                rows += 1.0;
                items = items.add(menu_item(
                    Some(Icon::ArrowBackUp),
                    tr::chg_discard_checked(i18n::count(checked.len())),
                    style::RED,
                    act(sid, ActMsg::Discard(checked)),
                ));
            }
            items = items.add(menu_divider()).add(menu_item(
                Some(Icon::ArrowRight),
                tr::chg_reveal(),
                style::TEXT,
                act(sid, ActMsg::Reveal(path.clone())),
            ));
            (menu_card(items, 250.0), 250.0, rows)
        } else if let Some(sha) = &a.commit_menu {
            let attach = self
                .can_attach(sid)
                .then(|| act(sid, ActMsg::Attach(sha.clone())));
            let mut items = Column::<El<'_>>::new()
                .add(menu_item_maybe(None, tr::attach_to_input(), style::TEXT, attach))
                .add(menu_divider())
                .add(menu_item(
                    None,
                    tr::chg_copy_sha(),
                    style::TEXT,
                    act(sid, ActMsg::Copy(sha.clone(), false)),
                ))
                .add(menu_item(
                    None,
                    tr::chg_copy_subject(),
                    style::TEXT,
                    act(sid, ActMsg::Copy(sha.clone(), true)),
                ))
                .add(menu_divider());
            for k in CommitKind::ALL {
                items = items.add(menu_item(
                    None,
                    k.label(),
                    style::TEXT,
                    act(sid, ActMsg::CommitPick(sha.clone(), k)),
                ));
            }
            (menu_card(items, 270.0), 270.0, 5.0 + CommitKind::ALL.len() as f32)
        } else {
            return space().boxed();
        };
        // Where the row was right-clicked; scripted menus open beside the side panel.
        let at = self.side_menu_at.unwrap_or(Point::new(
            self.window_size.width - self.side_w(),
            120.0,
        ));
        let h = rows * 30.0 + 12.0;
        let x = at.x.min(self.window_size.width - width - 4.0).max(0.0);
        let y = at.y.min(self.window_size.height - h - 4.0).max(0.0);
        let backdrop = mouse_area(space().width(Fill).height(Fill))
            .on_press(act(sid, ActMsg::CloseMenus))
            .on_right_press(act(sid, ActMsg::CloseMenus));
        iced::widget::stack![backdrop, iced::widget::pin(card).x(x).y(y).width(Fill).height(Fill)]
            .width(Fill)
            .height(Fill)
            .boxed()
    }

    // ------------------------------------------------------------------ dialogs

    pub fn changes_modal_card<'a>(&'a self, f: &'a ChangesModal) -> El<'a> {
        let sid = f.session.as_str();
        let cancel = (!f.busy).then_some(Message::CloseModal);
        let online = self.connected();
        let title = |t: String| -> El<'a> { text(t).size(18).font(style::bold()).boxed() };
        let mut c = Column::<El<'a>>::new().spacing(10);
        let width;
        match &f.kind {
            ModalKind::Discard(files) => {
                width = 480.0;
                let mut list = Column::<El<'a>>::new().spacing(2);
                for file in files {
                    list = list.add(
                        row![
                            kind_box(view_status(file.kind)),
                            text(file.path.clone())
                                .size(12)
                                .font(style::mono())
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::Start),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    );
                }
                c = c
                    .add(title(tr::chg_discard()))
                    .add(text(tr::chg_discard_body()).size(13))
                    .add(capped(list, files.len() as f32 * 22.0, 240.0));
                if files
                    .iter()
                    .any(|x| matches!(x.kind, ChangeKind::Untracked | ChangeKind::Added))
                {
                    c = c.add(
                        container(text(tr::chg_discard_new()).size(12))
                            .padding([6, 10])
                            .width(Fill)
                            .style(style::warn_box),
                    );
                }
                if let Some(e) = &f.error {
                    c = c.add(error_text(e));
                }
                c = c.add(dialog_buttons(
                    cancel,
                    button(text(busy_label(f.busy, tr::chg_discard())).size(13))
                        .padding([6, 14])
                        .style(style::danger)
                        .on_press_maybe((!f.busy && online).then(|| act(sid, ActMsg::ModalConfirm)))
                        .boxed(),
                ));
            }
            ModalKind::StashDrop(s) => {
                width = 460.0;
                c = c.add(title(tr::chg_stash_drop_title())).add(
                    text(tr::chg_stash_drop_body(
                        i18n::count(s.files.len()),
                        &stash_title(s),
                    ))
                    .size(13),
                );
                if let Some(e) = &f.error {
                    c = c.add(error_text(e));
                }
                c = c.add(dialog_buttons(
                    cancel,
                    button(text(busy_label(f.busy, tr::chg_stash_drop())).size(13))
                        .padding([6, 14])
                        .style(style::danger)
                        .on_press_maybe((!f.busy && online).then(|| act(sid, ActMsg::ModalConfirm)))
                        .boxed(),
                ));
            }
            ModalKind::Commit {
                kind,
                commit,
                name,
                target,
            } => {
                width = 520.0;
                c = c
                    .add(title(kind.label()))
                    .add(text(kind.description()).size(13))
                    .add(text(commit.sha.clone()).size(12).font(style::mono()))
                    .add(text(commit.subject.clone()).size(13))
                    .add(
                        text(tr::chg_commit_target(target))
                            .size(12)
                            .color(style::DIM),
                    );
                if *kind == CommitKind::Branch {
                    c = c.add(
                        column![
                            text(tr::chg_commit_branch_name()).size(12),
                            text_input("", name)
                                .id(MODAL_INPUT_ID)
                                .on_input({
                                    let sid = sid.to_owned();
                                    move |v| act(&sid, ActMsg::ModalInput(v))
                                })
                                .on_submit(act(sid, ActMsg::ModalConfirm))
                                .size(13)
                                .padding([6, 8])
                                .style(style::input),
                        ]
                        .spacing(4),
                    );
                }
                c = c.add(
                    container(text(tr::chg_commit_warning()).size(12))
                        .padding([8, 10])
                        .width(Fill)
                        .style(style::warn_box),
                );
                if let Some(e) = &f.error {
                    c = c.add(error_text(e));
                }
                let ready =
                    !f.busy && online && (*kind != CommitKind::Branch || !name.trim().is_empty());
                c = c.add(dialog_buttons(
                    cancel,
                    button(text(busy_label(f.busy, tr::chg_commit_run())).size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press_maybe(ready.then(|| act(sid, ActMsg::ModalConfirm)))
                        .boxed(),
                ));
            }
            ModalKind::Switch {
                name,
                remote,
                from,
                dirty,
                mode,
            } => {
                width = 500.0;
                c = c.add(title(tr::chg_switch_title())).add(
                    text(match remote {
                        Some(r) => tr::chg_switch_remote_body(r, name, from),
                        None => tr::chg_switch_body(from, name),
                    })
                    .size(13),
                );
                if *dirty > 0 {
                    c = c.add(changes_choice(sid, *dirty, *mode));
                }
                c = c.add(text(tr::chg_switch_agent_note()).size(12).color(style::DIM));
                if f.busy
                    && let Some(r) = remote
                {
                    c = c.add(
                        text(tr::chg_switch_fetching(r, name))
                            .size(12)
                            .color(style::DIM),
                    );
                }
                if let Some(e) = &f.error {
                    c = c.add(error_text(e));
                }
                c = c.add(dialog_buttons(
                    cancel,
                    button(text(busy_label(f.busy, tr::chg_switch())).size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press_maybe((!f.busy && online).then(|| act(sid, ActMsg::ModalConfirm)))
                        .boxed(),
                ));
            }
            ModalKind::Create {
                name,
                base,
                dirty,
                mode,
                base_open,
                base_query,
            } => {
                width = 480.0;
                let branches = self
                    .detail(sid)
                    .filter(|d| d.repo == f.repo)
                    .and_then(|d| d.acts.branch.list.as_ref());
                let trimmed = name.trim();
                let taken = branches.is_some_and(|b| b.local.iter().any(|l| l.name == trimmed));
                let mut name_col = column![
                    text(tr::chg_branch_name()).size(12),
                    text_input("feature/…", name)
                        .id(MODAL_INPUT_ID)
                        .on_input({
                            let sid = sid.to_owned();
                            move |v| act(&sid, ActMsg::ModalInput(v))
                        })
                        .on_submit(act(sid, ActMsg::ModalConfirm))
                        .size(13)
                        .padding([6, 8])
                        .style(style::input),
                ]
                .spacing(4);
                if taken {
                    name_col =
                        name_col.add(text(tr::chg_branch_taken()).size(12).color(style::RED));
                }
                c = c.add(title(tr::chg_new_branch())).add(name_col).add(
                    column![
                        text(tr::chg_base_branch()).size(12),
                        base_select(sid, branches, base.as_deref(), *base_open, base_query),
                    ]
                    .spacing(4),
                );
                if *dirty > 0 {
                    c = c.add(changes_choice(sid, *dirty, *mode));
                }
                if let Some(e) = &f.error {
                    c = c.add(error_text(e));
                }
                let ready = !f.busy && online && !trimmed.is_empty() && base.is_some() && !taken;
                c = c.add(dialog_buttons(
                    cancel,
                    button(text(busy_label(f.busy, tr::create())).size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press_maybe(ready.then(|| act(sid, ActMsg::ModalConfirm)))
                        .boxed(),
                ));
            }
        }
        c.width(width).boxed()
    }
}

/// Desk's `ChangesChoice`: carry the uncommitted changes along or stash them on the branch
/// being left, with one line saying what it does.
fn changes_choice<'a>(sid: &str, dirty: usize, mode: SwitchMode) -> El<'a> {
    let seg = |label: String, m: SwitchMode| -> El<'a> {
        button(container(text(label).size(12)).center_x(Fill))
            .padding([5, 8])
            .width(Fill)
            .style(style::choice(mode == m))
            .on_press(act(sid, ActMsg::ModalMode(m)))
            .boxed()
    };
    column![
        text(tr::chg_dirty(i18n::count(dirty))).size(13),
        row![
            seg(tr::chg_mode_carry(), SwitchMode::Carry),
            seg(tr::chg_mode_stash(), SwitchMode::Stash),
        ]
        .spacing(4),
        text(if mode == SwitchMode::Carry {
            tr::chg_mode_carry_note()
        } else {
            tr::chg_mode_stash_note()
        })
        .size(12)
        .color(style::DIM),
    ]
    .spacing(6)
    .boxed()
}

/// One picker row (Desk's `PickerRow`): a check when current, the label, an optional badge.
fn picker_row<'a>(
    label: String,
    active: bool,
    disabled: bool,
    badge: Option<El<'a>>,
    msg: Option<Message>,
) -> El<'a> {
    let check: El<'a> = if active {
        icon(Icon::Check, 13.0, style::ACCENT_TEXT)
    } else {
        space().width(13).boxed()
    };
    let mut r = row![
        container(check).width(16).center_x(16),
        text(label)
            .size(12)
            .color(if disabled { style::FAINT } else { style::TEXT })
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End)
            .width(Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if let Some(b) = badge {
        r = r.add(b);
    }
    button(r)
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 4.0,
        })
        .height(28)
        .width(Fill)
        .style(style::row(active))
        .on_press_maybe(msg)
        .boxed()
}

/// The 새 브랜치 dialog's base branch (Desk's searchable `Select`): the chosen branch, and when
/// open a search box over the 로컬 / 원격 lists.
fn base_select<'a>(
    sid: &str,
    branches: Option<&'a ide_client::GitBranches>,
    base: Option<&str>,
    open: bool,
    query: &'a str,
) -> El<'a> {
    let field = button(
        row![
            text(base.unwrap_or_default().to_owned())
                .size(13)
                .width(Fill)
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::End),
            icon(Icon::ChevronDown, 14.0, style::DIM),
        ]
        .align_y(Alignment::Center),
    )
    .padding([5, 8])
    .width(Fill)
    .style(style::secondary)
    .on_press(act(sid, ActMsg::ModalBaseOpen(!open)));
    if !open {
        return field.boxed();
    }
    let q = query.trim().to_lowercase();
    let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);
    let (local, remote): (Vec<String>, Vec<String>) = match branches {
        Some(b) => (
            b.local.iter().map(|l| l.name.clone()).filter(|n| hit(n)).collect(),
            b.remote
                .iter()
                .map(|r| r.short_ref.clone())
                .filter(|n| hit(n))
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let mut list = Column::<El<'a>>::new().spacing(1);
    let mut any = false;
    for (label, names) in [(tr::chg_local(), local), (tr::chg_remote(), remote)] {
        if names.is_empty() {
            continue;
        }
        list = list.add(
            container(text(label).size(11).font(style::bold()).color(style::DIM)).padding([4, 8]),
        );
        for n in names {
            any = true;
            let on = base == Some(n.as_str());
            list = list.add(
                button(text(n.clone()).size(12))
                    .padding([4, 10])
                    .width(Fill)
                    .style(style::row(on))
                    .on_press(act(sid, ActMsg::ModalBase(n))),
            );
        }
    }
    let empty = !any;
    if empty {
        list = list.add(
            container(text(tr::chg_no_branch()).size(12).color(style::DIM)).padding([6, 8]),
        );
    }
    column![
        field,
        container(
            column![
                text_input(tr::chg_branch_search(), query)
                    .id(crate::changes_actions::BASE_QUERY_ID)
                    .on_input({
                        let sid = sid.to_owned();
                        move |v| act(&sid, ActMsg::ModalBaseQuery(v))
                    })
                    .size(12)
                    .padding([4, 8])
                    .style(style::input),
                scrollable(list).height(Length::Fixed(200.0)),
            ]
            .spacing(4),
        )
        .padding(4)
        .style(style::modal_card),
    ]
    .spacing(2)
    .boxed()
}

/// `row` reports where it was right-clicked (window coordinates) for the side panel's menus.
fn at_pointer(row: El<'_>) -> El<'_> {
    ide_views::Interact::new(row)
        .absolute()
        .on_right_press(Message::SideMenuAt)
        .into()
}
