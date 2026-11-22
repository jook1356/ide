//! The Changes panel's 스크립트 section (Hermes Desk's `ScriptsSection`): the picked repo's
//! runnable scripts (`scripts.list`), grouped by file, each run in a user terminal in its
//! file's folder (`uterm.rs`); a running one opens its terminal instead.

use std::collections::HashSet;

use iced::widget::{Column, button, column, container, row, scrollable, space, text, text_input};
use iced::{Alignment, Element, Fill, Length, Padding, Task, Widget as _};
use ide_client::{ProtoError, ScriptGroup, SessionState};

use crate::app::{App, Live, Message};
use crate::detail::{DetailMsg, ItemKey};
use crate::i18n::{self, tr};
use crate::icons::{Icon, icon};
use crate::style;
use crate::uterm::UTermMsg;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Height of the open section's list.
const LIST_H: f32 = 220.0;

/// The section's state (per session).
#[derive(Default)]
pub struct ScriptsUi {
    pub open: bool,
    /// The repo the list is of, and the list (`Err`: why it could not be read).
    pub list: Option<(String, Result<Vec<ScriptGroup>, String>)>,
    pub loading: bool,
    pub query: String,
    /// Folded files.
    pub folded: HashSet<String>,
}

#[derive(Debug, Clone)]
pub enum ScriptsMsg {
    Toggle,
    Refresh,
    Loaded(String, Result<Vec<ScriptGroup>, ProtoError>),
    Query(String),
    Fold(String),
    /// Run `script` of `file` (its folder) in a new terminal.
    Run { repo: String, folder: String, name: String, command: String },
}

fn sm(sid: &str, m: ScriptsMsg) -> Message {
    Message::Detail(sid.to_owned(), DetailMsg::Scripts(m))
}

impl App {
    fn load_scripts(&mut self, sid: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        if d.repo.is_empty() {
            return Task::none();
        }
        d.scripts.loading = true;
        let (s, repo) = (sid.to_owned(), d.repo.clone());
        Task::perform(
            {
                let (s, repo) = (s.clone(), repo.clone());
                async move { c.scripts_list(&s, &repo).await }
            },
            move |r| sm(&s, ScriptsMsg::Loaded(repo.clone(), r)),
        )
    }

    /// The section is open and lists another repo than the picked one: list it again.
    pub(crate) fn scripts_follow_repo(&mut self, sid: &str) -> Task<Message> {
        let stale = self.detail(sid).is_some_and(|d| {
            d.scripts.open && d.scripts.list.as_ref().is_none_or(|(r, _)| *r != d.repo)
        });
        if stale {
            self.load_scripts(sid)
        } else {
            Task::none()
        }
    }

    pub(crate) fn on_scripts(&mut self, sid: &str, m: ScriptsMsg) -> Task<Message> {
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        let s = &mut d.scripts;
        match m {
            ScriptsMsg::Toggle => {
                s.open = !s.open;
                self.scripts_follow_repo(sid)
            }
            ScriptsMsg::Refresh => self.load_scripts(sid),
            ScriptsMsg::Loaded(repo, r) => {
                s.loading = false;
                s.list = Some((repo, r.map_err(|e| i18n::proto_error(&e))));
                Task::none()
            }
            ScriptsMsg::Query(q) => {
                s.query = q;
                Task::none()
            }
            ScriptsMsg::Fold(file) => {
                if !s.folded.remove(&file) {
                    s.folded.insert(file);
                }
                Task::none()
            }
            ScriptsMsg::Run {
                repo,
                folder,
                name,
                command,
            } => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let sid = sid.to_owned();
                Task::perform(
                    {
                        let sid = sid.clone();
                        async move {
                            c.uterm_open(ide_client::UTermOpen {
                                session: sid.clone(),
                                repo: Some(repo),
                                folder: Some(folder),
                                command: Some(command),
                                title: Some(name),
                                ..Default::default()
                            })
                                .await
                        }
                    },
                    move |r| Message::UTerm(UTermMsg::Opened(sid.clone(), r)),
                )
            }
        }
    }

    /// The section under the Changes list: its header, and when open the scripts.
    pub fn scripts_section<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let s = &d.scripts;
        let sid = live.info.id.as_str();
        let groups: &[ScriptGroup] = match &s.list {
            Some((r, Ok(g))) if *r == d.repo => g,
            _ => &[],
        };
        let count: usize = groups.iter().map(|g| g.scripts.len()).sum();
        let mut head = row![
            text(if s.open { "▾" } else { "▸" }).size(12).color(style::DIM).width(10),
            text(tr::scripts_title()).size(12).font(style::BOLD).color(style::DIM),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if count > 0 {
            head = head.push(text(count.to_string()).size(11).color(style::FAINT).boxed());
        }
        let header = button(head.width(Fill))
            .padding([5, 10])
            .width(Fill)
            .style(style::row(false))
            .on_press(sm(sid, ScriptsMsg::Toggle));
        let refresh = tip(
            button(icon(Icon::Refresh, 13.0, style::DIM))
                .padding(4)
                .style(style::icon)
                .on_press(sm(sid, ScriptsMsg::Refresh))
                .boxed(),
            tr::scripts_refresh(),
        );
        let rule = || {
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER))
        };
        if !s.open {
            return column![rule(), header].boxed();
        }
        let q = s.query.trim().to_lowercase();
        let hit = |g: &ScriptGroup, name: &str| {
            q.is_empty() || name.to_lowercase().contains(&q) || g.file.to_lowercase().contains(&q)
        };
        let mut list = Column::<El<'a>>::new().spacing(1);
        let mut shown = 0;
        for g in groups {
            let scripts: Vec<_> = g.scripts.iter().filter(|x| hit(g, &x.name)).collect();
            if scripts.is_empty() {
                continue;
            }
            let folded = s.folded.contains(&g.file) && q.is_empty();
            list = list.push(
                button(
                    row![
                        text(if folded { "▸" } else { "▾" }).size(11).color(style::DIM).width(10),
                        text(g.file.clone())
                            .size(11)
                            .color(style::DIM)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::Start)
                            .width(Fill),
                        text(g.kind.clone()).size(10).color(style::FAINT),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([3, 10])
                .width(Fill)
                .style(style::row(false))
                .on_press(sm(sid, ScriptsMsg::Fold(g.file.clone())))
                .boxed(),
            );
            if folded {
                continue;
            }
            for x in scripts {
                shown += 1;
                let running = live.uterms.iter().find(|t| {
                    t.info.title == x.name
                        && t.info.command == x.run
                        && t.info.state == SessionState::Running
                });
                let (glyph, color, tip_text, msg) = match running {
                    Some(t) => (
                        "●",
                        style::GREEN,
                        tr::scripts_running(),
                        Message::Detail(
                            sid.to_owned(),
                            DetailMsg::Activate(ItemKey::Terminal(t.info.id.clone())),
                        ),
                    ),
                    None => (
                        "▶",
                        style::ACCENT_TEXT,
                        tr::scripts_run(),
                        sm(
                            sid,
                            ScriptsMsg::Run {
                                repo: d.repo.clone(),
                                folder: g.folder.clone(),
                                name: x.name.clone(),
                                command: x.run.clone(),
                            },
                        ),
                    ),
                };
                let run = tip(
                    button(text(glyph).size(11).color(color))
                        .padding([2, 6])
                        .style(style::icon)
                        .on_press(msg)
                        .boxed(),
                    tip_text,
                );
                let detail = x.detail.clone().unwrap_or_else(|| x.run.clone());
                list = list.push(tip(
                    row![
                        text(x.name.clone())
                            .size(12)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::End),
                        text(detail)
                            .size(11)
                            .color(style::FAINT)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::End)
                            .width(Fill),
                        run,
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .padding(Padding {
                        left: 26.0,
                        right: 6.0,
                        top: 1.0,
                        bottom: 1.0,
                    })
                    .boxed(),
                    x.run.clone(),
                ));
            }
        }
        let note = match &s.list {
            _ if s.loading && groups.is_empty() => Some(tr::scripts_loading()),
            Some((_, Err(e))) => Some(e.clone()),
            _ if groups.is_empty() => Some(tr::scripts_none()),
            _ if shown == 0 && !q.is_empty() => Some(tr::scripts_no_match()),
            _ => None,
        };
        if let Some(n) = note {
            list = list.push(container(text(n).size(11).color(style::DIM)).padding([4, 10]).boxed());
        }
        let search = text_input(tr::scripts_search(), &s.query)
            .on_input({
                let sid = sid.to_owned();
                move |v| sm(&sid, ScriptsMsg::Query(v))
            })
            .size(12)
            .padding([3, 8])
            .style(style::input);
        column![
            rule(),
            row![header, refresh].align_y(Alignment::Center),
            container(search).padding([2, 10]),
            scrollable(list).height(Length::Fixed(LIST_H)),
        ]
        .boxed()
    }
}
