//! Drawing of the detail area: header (document identity or web navigation, "open items",
//! collapse), the shown item, and the side panel (Changes / Explore / Search).

use iced::widget::{
    Column, button, column, container, pick_list, popover, row, scrollable, space, stack, text,
    text_input,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Padding, Widget as _};
use ide_views::icons;
use ide_web::WebViewWidget;

use crate::app::{App, Choice, Live, Message, URL_ID};
use crate::detail::{
    DetailMsg, FileBar, FileItem, Item, ItemKey, SEARCH_ID, SIDE_W, SideTab, file_name, parent,
};
use crate::i18n::{self, tr};
use crate::style;
use crate::view::{icon_button, tip};

type El<'a> = Element<'a, Message>;

const I_BACK: &str = "←";
const I_FORWARD: &str = "→";
const I_RELOAD: &str = "⟳";
const I_COLLAPSE: &str = "»";
const I_EXPAND: &str = "«";
const I_CLOSE: &str = "✕";
const I_CHEVRON: &str = "▾";
const I_GLOBE: &str = "◍";
const I_DIFF: &str = "±";
const I_RELOAD_DISK: &str = "↻";
const I_OVERWRITE: &str = "⤓";
const I_COMPARE: &str = "⇄";
const I_SETTINGS: &str = "⚙";
/// Height of the header row.
const HEADER_H: f32 = 40.0;

/// Human-readable byte size.
pub fn size_text(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1} KB", f / K)
    } else {
        format!("{:.1} MB", f / K / K)
    }
}

fn hline<'a>() -> El<'a> {
    container(space())
        .height(1)
        .width(Fill)
        .style(style::bg(style::BORDER))
        .boxed()
}

fn vline<'a>() -> El<'a> {
    container(space())
        .width(1)
        .height(Fill)
        .style(style::bg(style::BORDER))
        .boxed()
}

/// Title and folder of an item (header, open items list).
fn item_names(key: &ItemKey, live: &Live) -> (String, String) {
    match key {
        ItemKey::Web => {
            let t = if live.web_title.trim().is_empty() {
                tr::tab_web()
            } else {
                live.web_title.clone()
            };
            (t, live.url_input.clone())
        }
        ItemKey::File(p) | ItemKey::Diff { path: p, .. } | ItemKey::Compare(p) => {
            let folder = parent(p);
            // Several repos: the folder starts with the repo's name.
            let folder = if live.info.repos.len() > 1 {
                let repo = crate::detail::split_q(p).0;
                let name = live
                    .info
                    .repos
                    .iter()
                    .find(|r| r.repo == repo)
                    .map_or(repo, |r| r.name.as_str());
                if folder.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name}/{folder}")
                }
            } else {
                folder.to_owned()
            };
            (file_name(p).to_owned(), folder)
        }
    }
}

fn item_kind(key: &ItemKey) -> String {
    match key {
        ItemKey::Web => tr::tab_web(),
        ItemKey::File(_) => tr::kind_file(),
        ItemKey::Diff { staged: true, .. } => tr::kind_staged(),
        ItemKey::Diff { staged: false, .. } => tr::kind_changed(),
        ItemKey::Compare(_) => tr::kind_compare(),
    }
}

fn item_icon<'a>(key: &ItemKey) -> El<'a> {
    match key {
        ItemKey::Web => text(I_GLOBE).size(14).color(style::DIM).boxed(),
        ItemKey::File(p) => icons::file_icon(file_name(p)),
        ItemKey::Diff { path, .. } | ItemKey::Compare(path) => row![
            icons::file_icon(file_name(path)),
            text(I_DIFF).size(10).color(style::AMBER),
        ]
        .spacing(1)
        .align_y(Alignment::Center)
        .boxed(),
    }
}

impl App {
    pub fn detail_area(&self) -> El<'_> {
        if self.ui.detail_collapsed {
            return container(
                column![icon_button(
                    I_EXPAND,
                    Some(Message::ToggleDetail),
                    tr::show_detail()
                )]
                .padding([6, 2]),
            )
            .width(30)
            .height(Fill)
            .style(style::bg(style::HEADER))
            .boxed();
        }
        let portion = 1000u16.saturating_sub((self.ui.split_ratio * 1000.0).round() as u16);
        let body: El<'_> = match self.selected_live() {
            Some(live) => self.pane(live),
            None => column![
                container(
                    row![
                        space().width(Fill),
                        icon_button(I_COLLAPSE, Some(Message::ToggleDetail), tr::hide_detail()),
                    ]
                    .align_y(Alignment::Center)
                )
                .height(HEADER_H)
                .padding([0, 6])
                .style(style::bg(style::HEADER)),
                container(text(tr::no_session()).size(13).color(style::DIM)).center(Fill),
            ]
            .boxed(),
        };
        container(body)
            .width(FillPortion(portion.max(1)))
            .height(Fill)
            .style(style::bg(style::PANEL))
            .boxed()
    }

    /// The side panel right of the detail pane (fixed width, outside the split).
    pub fn side_area(&self) -> El<'_> {
        match self.selected_live() {
            Some(live) if !self.ui.detail_collapsed => row![vline(), self.side_panel(live)].boxed(),
            _ => space().width(0).boxed(),
        }
    }

    // ------------------------------------------------------------------ pane

    fn pane<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.clone();
        let identity: El<'a> = match &d.active {
            ItemKey::Web => row![
                icon_button(
                    I_BACK,
                    live.can_back.then_some(Message::WebBack),
                    tr::web_back()
                ),
                icon_button(
                    I_FORWARD,
                    live.can_forward.then_some(Message::WebForward),
                    tr::web_forward()
                ),
                icon_button(I_RELOAD, Some(Message::WebReload), tr::web_reload()),
                text_input(tr::url_placeholder(), &live.url_input)
                    .id(URL_ID)
                    .on_input(Message::UrlInput)
                    .on_submit(Message::UrlSubmit)
                    .size(12)
                    .padding([4, 8])
                    .style(style::input)
                    .width(Fill),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .boxed(),
            key => {
                let (title, folder) = item_names(key, live);
                let dirty = matches!(d.active_item(), Some(Item::File(f)) if f.dirty());
                let subtitle = if folder.is_empty() {
                    item_kind(key)
                } else {
                    format!("{} · {folder}", item_kind(key))
                };
                let mut title_row = row![
                    text(title)
                        .size(13)
                        .font(style::BOLD)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                ]
                .spacing(6)
                .align_y(Alignment::Center);
                if dirty {
                    title_row = title_row.push(text("●").size(10).color(style::TEXT).boxed());
                }
                row![
                    item_icon(key),
                    column![
                        title_row,
                        text(subtitle)
                            .size(11)
                            .color(style::DIM)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::Start),
                    ]
                    .spacing(1)
                    .width(Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .boxed()
            }
        };
        let header = container(
            row![
                container(identity).width(Fill),
                vline_short(),
                self.open_items_button(live),
                icon_button(I_COLLAPSE, Some(Message::ToggleDetail), tr::hide_detail()),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .height(HEADER_H)
        .padding([0, 8])
        .align_y(Alignment::Center)
        .width(Fill)
        .style(style::bg(style::HEADER));

        let body: El<'a> = match d.active_item() {
            Some(Item::Web) | None => match &live.web {
                // Under a modal the page stays visible as a frozen, dimmed snapshot of its
                // last frame (no input reaches it, CEF stops rendering); the backdrop and the
                // dialog are drawn above it (see docs, "modal z-order").
                Some(w) => WebViewWidget::new(w)
                    .width(Fill)
                    .height(Fill)
                    .dimmed(self.web_covered())
                    .boxed(),
                None => container(text(live.error.as_deref().unwrap_or("")).size(12))
                    .center(Fill)
                    .boxed(),
            },
            Some(Item::File(f)) => self.file_body(&sid, f),
            Some(Item::Diff(x)) => {
                let key = d.active.clone();
                match (&x.view, &x.error) {
                    (_, Some(e)) => message_body(e.clone(), true),
                    (Some(v), None) => {
                        let sid = sid.clone();
                        v.view()
                            .map(move |m| {
                                Message::Detail(sid.clone(), DetailMsg::Diff(key.clone(), m))
                            })
                            .boxed()
                    }
                    (None, None) => message_body(tr::tree_loading(), false),
                }
            }
        };
        let mut layers = vec![body];
        if let Some((t, _)) = &d.toast {
            layers.push(
                container(
                    container(text(t.as_str()).size(12))
                        .padding([6, 10])
                        .style(style::modal_card),
                )
                .width(Fill)
                .height(Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::End)
                .padding(14)
                .boxed(),
            );
        }
        container(column![
            header,
            hline(),
            stack(layers).width(Fill).height(Fill)
        ])
        .width(Fill)
        .height(Fill)
        .boxed()
    }

    fn file_body<'a>(&'a self, sid: &str, f: &'a FileItem) -> El<'a> {
        if let Some(e) = &f.error {
            return message_body(e.clone(), true);
        }
        if f.binary {
            return message_body(tr::file_binary(&size_text(f.size)), false);
        }
        let Some(v) = &f.view else {
            return message_body(tr::tree_loading(), false);
        };
        let (s, p) = (sid.to_owned(), f.path.clone());
        let editor = v
            .view()
            .map(move |m| Message::Detail(s.clone(), DetailMsg::Code(p.clone(), m)))
            .boxed();
        let mut col = Column::new();
        if let Some(bar) = f.bar {
            col = col.push(file_bar(sid, &f.path, bar));
        }
        if f.truncated {
            col = col.push(
                container(text(tr::file_truncated(&size_text(f.size))).size(12))
                    .padding([5, 10])
                    .width(Fill)
                    .style(style::bg(style::HEADER))
                    .boxed(),
            );
        }
        col.push(editor).boxed()
    }

    /// "Open items": every item of the session, grouped like Hermes Desk's list.
    fn open_items_button<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let total = d.items.len();
        let trigger = button(
            row![
                text(I_CHEVRON).size(11).color(style::DIM),
                text(tr::open_items()).size(12),
                container(text(total.to_string()).size(10).font(style::BOLD))
                    .padding([0, 5])
                    .style(|_| container::Style {
                        background: Some(style::BORDER.into()),
                        text_color: Some(style::TEXT),
                        border: iced::Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(style::choice(self.open_items))
        .on_press(Message::ToggleOpenItems);
        let popup = self.open_items.then(|| {
            let sid = live.info.id.clone();
            let mut list = Column::new().spacing(2);
            type Group = (String, fn(&ItemKey) -> bool);
            let groups: [Group; 3] = [
                (tr::group_browser(), |k| matches!(k, ItemKey::Web)),
                (tr::side_changes(), |k| {
                    matches!(k, ItemKey::Diff { .. } | ItemKey::Compare(_))
                }),
                (tr::side_explore(), |k| matches!(k, ItemKey::File(_))),
            ];
            for (label, is) in groups {
                let rows: Vec<&(ItemKey, Item)> = d.items.iter().filter(|(k, _)| is(k)).collect();
                if rows.is_empty() {
                    continue;
                }
                list = list.push(
                    row![
                        text(label).size(11).font(style::BOLD).color(style::DIM),
                        text(rows.len().to_string()).size(11).color(style::FAINT),
                    ]
                    .spacing(6)
                    .padding([4, 8])
                    .boxed(),
                );
                for (k, item) in rows {
                    let (title, folder) = item_names(k, live);
                    let dirty = matches!(item, Item::File(f) if f.dirty());
                    let mut names = row![
                        text(title.clone())
                            .size(12)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::End),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);
                    if dirty {
                        names = names.push(text("●").size(9).boxed());
                    }
                    if !folder.is_empty() && folder != title {
                        names = names.push(
                            text(folder)
                                .size(11)
                                .color(style::FAINT)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::Start)
                                .boxed(),
                        );
                    }
                    let main = button(
                        row![item_icon(k), names]
                            .spacing(8)
                            .align_y(Alignment::Center),
                    )
                    .padding([4, 8])
                    .width(Fill)
                    .style(style::row(*k == d.active))
                    .on_press(Message::Detail(sid.clone(), DetailMsg::Activate(k.clone())));
                    let close: El<'_> = if *k == ItemKey::Web {
                        space().width(24).boxed()
                    } else {
                        icon_button(
                            I_CLOSE,
                            Some(Message::Detail(sid.clone(), DetailMsg::Close(k.clone()))),
                            tr::close(),
                        )
                    };
                    list = list.push(
                        row![main, close]
                            .spacing(2)
                            .align_y(Alignment::Center)
                            .boxed(),
                    );
                }
            }
            container(scrollable(list).height(Length::Shrink))
                .padding(6)
                .width(360)
                .style(style::modal_card)
        });
        popover(trigger, popup)
            .position(iced::widget::popover::Position::Bottom)
            .gap(4)
            .on_close(Message::CloseOpenItems)
            .boxed()
    }

    // ------------------------------------------------------------------ side panel

    fn side_panel<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.clone();
        let tab = |t: SideTab, label: String, count: Option<usize>| -> El<'a> {
            let active = d.side == t;
            let mut r = row![text(label).size(12)]
                .spacing(5)
                .align_y(Alignment::Center);
            if let Some(n) = count.filter(|n| *n > 0) {
                r = r.push(
                    container(text(n.to_string()).size(10).font(style::BOLD))
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
                        .boxed(),
                );
            }
            column![
                button(container(r).center_x(Fill))
                    .padding([8, 6])
                    .width(Fill)
                    .style(style::tab_flat(active))
                    .on_press(Message::Detail(sid.clone(), DetailMsg::Side(t))),
                container(space())
                    .height(2)
                    .width(Fill)
                    .style(style::bg(if active {
                        style::ACCENT
                    } else {
                        style::HEADER
                    })),
            ]
            .width(Fill)
            .boxed()
        };
        let tabs = container(row![
            tab(
                SideTab::Changes,
                tr::side_changes(),
                Some(d.changes.total())
            ),
            tab(SideTab::Explore, tr::side_explore(), None),
            tab(SideTab::Search, tr::side_search(), None),
        ])
        .style(style::bg(style::HEADER));
        let content: El<'a> = match d.side {
            SideTab::Changes | SideTab::Explore if live.info.repos.is_empty() => column![
                self.repo_selector(live),
                message_body(tr::no_repos_in_session(), false),
            ]
            .boxed(),
            SideTab::Changes => {
                let s = sid.clone();
                let head = container(
                    text(match &d.status_error {
                        Some(e) => e.clone(),
                        None => tr::changes_count(i18n::count(d.changes.total())),
                    })
                    .size(11)
                    .color(if d.status_error.is_some() {
                        style::RED
                    } else {
                        style::DIM
                    }),
                )
                .padding([6, 10]);
                column![
                    self.repo_selector(live),
                    head,
                    d.changes
                        .view()
                        .map(move |m| Message::Detail(s.clone(), DetailMsg::Changes(m)))
                        .boxed(),
                ]
                .boxed()
            }
            SideTab::Explore => column![
                self.repo_selector(live),
                match &d.tree_error {
                    Some(e) => message_body(e.clone(), true),
                    None => {
                        let s = sid.clone();
                        d.tree
                            .view()
                            .map(move |m| Message::Detail(s.clone(), DetailMsg::Tree(m)))
                            .boxed()
                    }
                },
            ]
            .boxed(),
            SideTab::Search => self.search_panel(live),
        };
        container(column![tabs, hline(), content])
            .width(SIDE_W)
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
            .boxed()
    }

    /// Hermes Desk's Changes-panel header (`BranchSelect`): `repo / branch` of the shown repo,
    /// a dropdown of the session's repos, and the session's repo settings (⚙: attach / detach).
    fn repo_selector<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let sid = live.info.id.clone();
        let choices: Vec<Choice> = live
            .info
            .repos
            .iter()
            .map(|r| Choice {
                key: r.repo.clone(),
                label: format!("{} / {}", r.name, r.branch),
            })
            .collect();
        let selected = choices.iter().find(|c| c.key == d.repo).cloned();
        let picker: El<'a> = if choices.is_empty() {
            text(tr::no_repos())
                .size(12)
                .color(style::DIM)
                .width(Fill)
                .boxed()
        } else {
            let s = sid.clone();
            pick_list(selected, choices, |c: &Choice| c.label.clone())
                .on_select(move |c: Choice| Message::Detail(s.clone(), DetailMsg::PickRepo(c.key)))
                .text_size(12)
                .padding([3, 6])
                .width(Fill)
                .boxed()
        };
        container(
            row![
                crate::icons::icon(crate::icons::Icon::GitBranch, 14.0, style::DIM),
                picker,
                icon_button(
                    I_SETTINGS,
                    self.connected()
                        .then(|| Message::SessionRepos(sid.clone())),
                    tr::session_repos_open()
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([5, 8])
        .width(Fill)
        .style(style::bg(style::HEADER))
        .boxed()
    }

    fn search_panel<'a>(&'a self, live: &'a Live) -> El<'a> {
        let s = &live.detail.search;
        let sid = live.info.id.clone();
        let m = |x: DetailMsg| Message::Detail(sid.clone(), x);
        let toggle = |glyph: &'static str, on: bool, msg: Message, label: String| -> El<'a> {
            tip(
                button(
                    text(glyph)
                        .size(12)
                        .font(style::MONO)
                        .width(20)
                        .align_x(Alignment::Center),
                )
                .padding([3, 3])
                .style(style::choice(on))
                .on_press(msg)
                .boxed(),
                label,
            )
        };
        let input = row![
            text_input(tr::search_placeholder(), &s.query)
                .id(SEARCH_ID)
                .on_input({
                    let sid = sid.clone();
                    move |v| Message::Detail(sid.clone(), DetailMsg::SearchInput(v))
                })
                .on_submit(m(DetailMsg::SearchSubmit))
                .size(12)
                .padding([5, 8])
                .style(style::input)
                .width(Fill),
            toggle(
                "Aa",
                s.case_sensitive,
                m(DetailMsg::SearchCase),
                tr::search_case()
            ),
            toggle(
                "ab",
                s.whole_word,
                m(DetailMsg::SearchWord),
                tr::search_word()
            ),
            toggle(".*", s.regex, m(DetailMsg::SearchRegex), tr::search_regex()),
        ]
        .spacing(3)
        .align_y(Alignment::Center);
        let status: Option<(String, Color)> = if let Some(e) = &s.error {
            Some((e.clone(), style::RED))
        } else if s.running {
            Some((tr::search_running(), style::DIM))
        } else {
            s.summary.as_ref().map(|sum| {
                let mut t = if sum.matches == 0 {
                    tr::search_none()
                } else {
                    tr::search_summary(sum.matches as i64, sum.files as i64)
                };
                if sum.truncated {
                    t = format!("{t} {}", tr::search_truncated());
                }
                (t, style::DIM)
            })
        };
        let mut col = column![container(input).padding([8, 8])];
        if let Some((t, c)) = status {
            col = col.push(
                container(text(t).size(11).color(c))
                    .padding([0, 10])
                    .boxed(),
            );
        }
        let mut list = Column::new().spacing(0).padding([4, 0]);
        let mut last_repo: Option<&str> = None;
        let multi = live.info.repos.len() > 1;
        for (path, hits) in &s.groups {
            // Results are grouped by repo: a header above each repo's files.
            let repo = crate::detail::split_q(path).0;
            if multi && last_repo != Some(repo) {
                last_repo = Some(repo);
                let name = live
                    .info
                    .repos
                    .iter()
                    .find(|r| r.repo == repo)
                    .map_or(repo, |r| r.name.as_str());
                list = list.push(
                    container(
                        row![
                            crate::icons::icon(crate::icons::Icon::GitBranch, 12.0, style::DIM),
                            text(name.to_owned()).size(11).font(style::BOLD).color(style::DIM),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding([4, 8])
                    .boxed(),
                );
            }
            let collapsed = s.collapsed.contains(path);
            let sid2 = live.info.id.clone();
            list = list.push(
                button(
                    row![
                        text(if collapsed { "▸" } else { "▾" })
                            .size(10)
                            .color(style::DIM)
                            .width(10),
                        icons::file_icon(file_name(path)),
                        text(file_name(path))
                            .size(12)
                            .wrapping(text::Wrapping::None),
                        text(parent(path).to_owned())
                            .size(11)
                            .color(style::FAINT)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::Start)
                            .width(Fill),
                        text(hits.len().to_string()).size(10).color(style::DIM),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([3, 8])
                .width(Fill)
                .style(style::row(false))
                .on_press(Message::Detail(
                    sid2.clone(),
                    DetailMsg::SearchToggleFile(path.clone()),
                ))
                .boxed(),
            );
            if collapsed {
                continue;
            }
            for h in hits {
                list = list.push(
                    button(
                        row![
                            text(h.line.to_string())
                                .size(11)
                                .font(style::MONO)
                                .color(style::FAINT)
                                .width(34)
                                .align_x(Alignment::End),
                            hit_text(&h.line_text, &h.ranges),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(Padding {
                        top: 2.0,
                        right: 8.0,
                        bottom: 2.0,
                        left: 14.0,
                    })
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(Message::Detail(
                        sid2.clone(),
                        DetailMsg::OpenAt(crate::detail::qualify(&h.repo, &h.path), h.line, h.col),
                    ))
                    .boxed(),
                );
            }
        }
        col.push(scrollable(list).height(Fill).boxed()).boxed()
    }
}

/// A search hit's line with the matches highlighted (leading whitespace trimmed).
fn hit_text<'a>(line: &str, ranges: &[ide_client::ByteRange]) -> El<'a> {
    let trim = line.len() - line.trim_start().len();
    let mut parts = row![].align_y(Alignment::Center);
    let mut at = trim;
    let piece = |s: &str| {
        text(s.to_owned())
            .size(12)
            .font(style::MONO)
            .wrapping(text::Wrapping::None)
    };
    for r in ranges {
        let (a, b) = (r.start as usize, r.end as usize);
        if a < at || b > line.len() || !line.is_char_boundary(a) || !line.is_char_boundary(b) {
            continue;
        }
        if a > at {
            parts = parts.push(piece(&line[at..a]).boxed());
        }
        parts = parts.push(
            container(piece(&line[a..b]).color(style::TEXT))
                .style(|_| container::Style {
                    background: Some(ide_views::theme::FIND_MATCH.into()),
                    border: iced::Border {
                        color: ide_views::theme::FIND_MATCH_BORDER,
                        width: 1.0,
                        radius: 2.0.into(),
                    },
                    ..Default::default()
                })
                .boxed(),
        );
        at = b;
    }
    if at < line.len() {
        parts = parts.push(piece(&line[at..]).color(style::DIM).boxed());
    }
    container(parts).width(Fill).clip(true).boxed()
}

fn vline_short<'a>() -> El<'a> {
    container(space())
        .width(1)
        .height(20)
        .style(style::bg(style::BORDER))
        .boxed()
}

/// A centred message in the pane (loading, errors, binary files).
fn message_body<'a>(msg: String, error: bool) -> El<'a> {
    container(
        text(msg)
            .size(13)
            .color(if error { style::RED } else { style::DIM }),
    )
    .center(Fill)
    .padding(20)
    .boxed()
}

/// The bar over a file: save conflict, changed or deleted on disk.
fn file_bar<'a>(sid: &str, path: &str, bar: FileBar) -> El<'a> {
    let m = |x: DetailMsg| Some(Message::Detail(sid.to_owned(), x));
    let p = || path.to_owned();
    let msg = match bar {
        FileBar::Conflict => tr::file_conflict(),
        FileBar::ChangedOnDisk => tr::file_changed_on_disk(),
        FileBar::Deleted => tr::file_deleted_on_disk(),
    };
    let mut r = row![
        text("⚠").size(13).color(style::AMBER),
        text(msg)
            .size(12)
            .wrapping(text::Wrapping::WordOrGlyph)
            .width(Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if bar != FileBar::Deleted {
        r = r.push(icon_button(
            I_RELOAD_DISK,
            m(DetailMsg::BarReload(p())),
            tr::bar_reload(),
        ));
    }
    r = r.push(icon_button(
        I_OVERWRITE,
        m(DetailMsg::BarOverwrite(p())),
        tr::bar_overwrite(),
    ));
    if bar != FileBar::Deleted {
        r = r.push(icon_button(
            I_COMPARE,
            m(DetailMsg::BarCompare(p())),
            tr::bar_compare(),
        ));
    }
    r = r.push(icon_button(
        I_CLOSE,
        m(DetailMsg::BarDismiss(p())),
        tr::close(),
    ));
    container(r)
        .padding([4, 10])
        .width(Fill)
        .style(|_| container::Style {
            background: Some(ide_views::theme::over(style::AMBER, 0.14, style::PANEL).into()),
            text_color: Some(style::TEXT),
            border: iced::Border {
                color: ide_views::theme::over(style::AMBER, 0.45, style::PANEL),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .boxed()
}
