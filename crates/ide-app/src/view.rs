//! Window layout: sidebar | terminal | divider | detail, plus modal dialogs.

use iced::widget::{
    Column, button, center, checkbox, column, container, mouse_area, opaque, pick_list, row,
    scrollable, space, stack, text, text_input, tooltip,
};
use iced::{Alignment, Color, Element, Fill, FillPortion, Length, Widget as _, mouse};
use ide_core::{AgentKind, Session, same_path};
use ide_i18n::LanguagePref;
use ide_term::TerminalView;
use ide_web::WebViewWidget;

use crate::app::{
    App, DIVIDER_W, Live, Message, Modal, NEW_REPO_ID, PROFILE_NAME_ID, RENAME_ID, RunState,
    SIDEBAR_W, TERM_ID, URL_ID,
};
use crate::i18n::{self, tr};
use crate::style;

type El<'a> = Element<'a, Message>;

// Symbols (Segoe UI Symbol / system fallback fonts on Windows).
const I_PLUS: &str = "+";
const I_GEAR: &str = "⚙";
// U+25B8: U+25B6 would get the emoji presentation from Segoe UI Emoji.
const I_PLAY: &str = "▸";
const I_RESTART: &str = "↻";
const I_STOP: &str = "■";
const I_EDIT: &str = "✎";
const I_DELETE: &str = "✕";
const I_BACK: &str = "←";
const I_FORWARD: &str = "→";
const I_RELOAD: &str = "⟳";
const I_COLLAPSE: &str = "»";
const I_EXPAND: &str = "«";
const I_DOT: &str = "●";
const I_RING: &str = "○";

pub fn agent_glyph(agent: AgentKind) -> (&'static str, Color) {
    match agent {
        AgentKind::Claude => ("✻", style::CLAUDE),
        AgentKind::Codex => ("◆", style::CODEX),
        AgentKind::Shell => ("❯", style::DIM),
    }
}

fn status_glyph(state: RunState) -> (&'static str, Color) {
    match state {
        RunState::Stopped => (I_RING, style::FAINT),
        RunState::Starting => (I_DOT, style::AMBER),
        RunState::Running => (I_DOT, style::GREEN),
        RunState::Exited(_) => (I_DOT, style::FAINT),
        RunState::Failed => (I_DOT, style::RED),
    }
}

fn tip<'a>(content: impl Into<El<'a>>, label: String) -> El<'a> {
    tooltip(
        content.into(),
        container(text(label).size(12))
            .padding([3, 7])
            .style(style::modal_card),
    )
    .position(tooltip::Position::Bottom)
    .boxed()
}

fn icon_button<'a>(glyph: &'a str, msg: Option<Message>, label: String) -> El<'a> {
    let b = button(
        text(glyph)
            .size(13)
            .width(Length::Fixed(14.0))
            .align_x(Alignment::Center),
    )
    .padding([3, 5])
    .style(style::icon)
    .on_press_maybe(msg);
    tip(b.boxed(), label)
}

fn label<'a>(s: String) -> El<'a> {
    text(s).size(11).color(style::DIM).boxed()
}

impl App {
    pub fn view(&self) -> El<'_> {
        let base = row![
            self.sidebar(),
            container(space())
                .width(1)
                .height(Fill)
                .style(style::bg(style::BORDER)),
            self.centre(),
            self.divider(),
            self.detail(),
        ]
        .height(Fill);
        // Always a stack so opening/closing a modal keeps the widget tree (terminal, web view).
        let overlay: El<'_> = match &self.modal {
            Some(m) => self.modal_view(m),
            None => space().boxed(),
        };
        let root = stack![base, overlay].boxed();
        if self.script.is_some() {
            ide_web::testing::Injector::new(root).into()
        } else {
            root
        }
    }

    // ------------------------------------------------------------------ sidebar

    fn sidebar(&self) -> El<'_> {
        let names: Vec<String> = self
            .profiles
            .list()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let header = row![
            text("ide").size(15).font(style::BOLD),
            space().width(Fill),
            icon_button(I_GEAR, Some(Message::OpenSettings), tr::settings()),
        ]
        .align_y(Alignment::Center);
        let profile = row![
            label(tr::profile_label()),
            space().width(8),
            pick_list(Some(self.current_profile()), names, |s: &String| s.clone())
                .on_select(Message::ProfilePicked)
                .text_size(13)
                .padding([4, 8])
                .width(Fill),
        ]
        .align_y(Alignment::Center);
        let sessions_header = row![
            label(tr::sessions_label()),
            space().width(Fill),
            icon_button(I_PLUS, Some(Message::OpenNew), tr::new_session()),
        ]
        .align_y(Alignment::Center);

        let mut list: Column<El<'_>> = Column::new().spacing(2);
        let profile_name = self.current_profile();
        let mine: Vec<&Session> = self
            .store
            .list()
            .iter()
            .filter(|s| s.profile == profile_name)
            .collect();
        // Group by repository, in order of first appearance.
        let mut groups: Vec<(&Session, Vec<&Session>)> = Vec::new();
        for s in &mine {
            match groups
                .iter_mut()
                .find(|(first, _)| same_path(&first.repo_path, &s.repo_path))
            {
                Some((_, v)) => v.push(s),
                None => groups.push((s, vec![s])),
            }
        }
        for (first, members) in groups {
            let repo_name = first
                .repo_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| first.repo_path.display().to_string());
            list = list.push(
                container(
                    text(repo_name.to_uppercase())
                        .size(11)
                        .font(style::BOLD)
                        .color(style::DIM),
                )
                .padding([6, 4])
                .boxed(),
            );
            for s in members {
                if let Some(live) = self.live(&s.id) {
                    list = list.push(self.session_row(s, live));
                }
            }
        }
        if mine.is_empty() {
            list = list.push(
                container(text(tr::no_sessions()).size(12).color(style::DIM))
                    .padding([8, 4])
                    .boxed(),
            );
        }

        let mut col = column![
            header,
            profile,
            container(space())
                .height(1)
                .width(Fill)
                .style(style::bg(style::BORDER)),
            sessions_header,
            scrollable(list).height(Fill),
        ]
        .spacing(10)
        .padding(10);
        if let Some(b) = &self.banner {
            col = col.push(
                container(text(b.as_str()).size(12))
                    .padding(6)
                    .style(style::error_box)
                    .boxed(),
            );
        }
        container(col)
            .width(SIDEBAR_W)
            .height(Fill)
            .style(style::bg(style::SIDEBAR))
            .boxed()
    }

    fn session_row<'a>(&'a self, s: &'a Session, live: &'a Live) -> El<'a> {
        let selected = self.selected.as_ref() == Some(&s.id);
        let (glyph, gcolor) = agent_glyph(s.agent);
        let (dot, dcolor) = status_glyph(live.state);

        if let Some((id, value)) = &self.rename
            && id == &s.id
        {
            return text_input(tr::session_title_placeholder(), value)
                .id(RENAME_ID)
                .on_input(Message::RenameInput)
                .on_submit(Message::RenameCommit)
                .size(13)
                .padding([5, 8])
                .style(style::input)
                .boxed();
        }

        let subtitle = if !live.term_title.is_empty() {
            live.term_title.clone()
        } else {
            i18n::status_label(live.state)
        };
        let main = button(
            row![
                text(glyph).size(14).color(gcolor).width(18),
                column![
                    text(s.title.as_str())
                        .size(13)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                    text(subtitle)
                        .size(11)
                        .color(style::DIM)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                ]
                .width(Fill),
                text(dot).size(10).color(dcolor),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([4, 6])
        .width(Fill)
        .style(style::row(selected))
        .on_press(Message::Select(s.id.clone()));
        let main = mouse_area(main).on_right_press(Message::StartRename(s.id.clone()));

        let run: El<'a> = match live.state {
            RunState::Starting | RunState::Running => {
                icon_button(I_STOP, Some(Message::Stop(s.id.clone())), tr::stop())
            }
            RunState::Stopped => {
                icon_button(I_PLAY, Some(Message::Start(s.id.clone())), tr::start())
            }
            RunState::Exited(_) | RunState::Failed => {
                icon_button(I_RESTART, Some(Message::Start(s.id.clone())), tr::restart())
            }
        };
        row![
            main,
            run,
            icon_button(
                I_EDIT,
                Some(Message::StartRename(s.id.clone())),
                tr::rename()
            ),
            icon_button(
                I_DELETE,
                Some(Message::AskDelete(s.id.clone())),
                tr::delete()
            ),
        ]
        .spacing(1)
        .align_y(Alignment::Center)
        .boxed()
    }

    // ------------------------------------------------------------------ centre

    fn centre(&self) -> El<'_> {
        let portion = (self.ui.split_ratio * 1000.0).round() as u16;
        let width = if self.ui.detail_collapsed {
            Fill
        } else {
            FillPortion(portion.max(1))
        };
        let Some((s, live)) = self
            .selected
            .as_ref()
            .and_then(|id| Some((self.session(id)?, self.live(id)?)))
        else {
            return container(
                column![
                    text(tr::no_session_selected()).size(18).color(style::DIM),
                    button(text(tr::new_session()).size(13))
                        .padding([6, 14])
                        .style(style::primary)
                        .on_press(Message::OpenNew),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Fill)
            .width(width)
            .style(style::bg(style::BG))
            .boxed();
        };

        let (glyph, gcolor) = agent_glyph(s.agent);
        let header = container(
            row![
                text(glyph).size(13).color(gcolor),
                text(s.title.as_str()).size(13).font(style::BOLD),
                text(s.branch.as_str())
                    .size(12)
                    .color(style::DIM)
                    .font(style::MONO),
                text(s.worktree_path.display().to_string())
                    .size(11)
                    .color(style::FAINT)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::Start)
                    .width(Fill),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .width(Fill)
        .style(style::bg(style::HEADER));

        let body: El<'_> = match &live.term {
            Some(term) => {
                let mut col = Column::new().push(
                    TerminalView::new(term, &self.term_config)
                        .id(TERM_ID)
                        .on_event(Message::TermView)
                        .boxed(),
                );
                if let RunState::Exited(code) = live.state {
                    col = col.push(
                        container(
                            row![
                                text(match code {
                                    Some(c) => tr::process_exited_code(&c.to_string()),
                                    None => tr::process_exited(),
                                })
                                .size(12)
                                .color(style::DIM),
                                space().width(Fill),
                                button(text(format!("{I_RESTART}  {}", tr::restart())).size(12))
                                    .padding([3, 10])
                                    .style(style::secondary)
                                    .on_press(Message::Start(s.id.clone())),
                            ]
                            .align_y(Alignment::Center),
                        )
                        .padding([4, 10])
                        .style(style::bg(style::HEADER))
                        .boxed(),
                    );
                }
                col.boxed()
            }
            None => {
                let mut c = column![
                    text(match live.state {
                        RunState::Failed => tr::session_start_failed(),
                        _ => tr::session_stopped(),
                    })
                    .size(16)
                    .color(style::DIM),
                    button(
                        text(format!(
                            "{I_PLAY}  {}",
                            tr::start_agent(&i18n::agent_label(s.agent))
                        ))
                        .size(13),
                    )
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press(Message::Start(s.id.clone())),
                ]
                .spacing(12)
                .align_x(Alignment::Center)
                .width(520);
                if let Some(e) = &live.error {
                    c = c.push(
                        container(text(e.as_str()).size(12))
                            .padding(8)
                            .style(style::error_box)
                            .boxed(),
                    );
                }
                container(c).center(Fill).boxed()
            }
        };

        container(column![header, body])
            .width(width)
            .height(Fill)
            .style(style::bg(style::BG))
            .boxed()
    }

    fn divider(&self) -> El<'_> {
        if self.ui.detail_collapsed {
            return container(space())
                .width(1)
                .height(Fill)
                .style(style::bg(style::BORDER))
                .boxed();
        }
        let color = if self.dragging {
            style::ACCENT
        } else {
            style::BORDER
        };
        mouse_area(
            container(
                container(space())
                    .width(1)
                    .height(Fill)
                    .style(style::bg(color)),
            )
            .center_x(DIVIDER_W)
            .height(Fill),
        )
        .on_press(Message::DividerPressed)
        .interaction(mouse::Interaction::ResizingHorizontally)
        .boxed()
    }

    // ------------------------------------------------------------------ detail

    fn detail(&self) -> El<'_> {
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
        let tabs = container(
            row![
                button(text(tr::tab_web()).size(12))
                    .padding([5, 14])
                    .style(style::tab(true)),
                space().width(Fill),
                icon_button(I_COLLAPSE, Some(Message::ToggleDetail), tr::hide_detail()),
            ]
            .align_y(Alignment::End),
        )
        .padding(iced::Padding {
            top: 4.0,
            right: 6.0,
            bottom: 0.0,
            left: 6.0,
        })
        .width(Fill)
        .style(style::bg(style::HEADER));

        let content: El<'_> = match self.selected_live() {
            Some(live) => {
                let bar = row![
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
                .padding([6, 8])
                .align_y(Alignment::Center);
                let page: El<'_> = match &live.web {
                    Some(w) => WebViewWidget::new(w).width(Fill).height(Fill).into(),
                    None => container(text(live.error.as_deref().unwrap_or("")).size(12))
                        .center(Fill)
                        .boxed(),
                };
                column![bar, page].boxed()
            }
            None => container(text(tr::no_session()).size(13).color(style::DIM))
                .center(Fill)
                .boxed(),
        };
        container(column![tabs, content])
            .width(FillPortion(portion.max(1)))
            .height(Fill)
            .style(style::bg(style::PANEL))
            .boxed()
    }

    // ------------------------------------------------------------------ modals

    fn modal_view<'a>(&'a self, modal: &'a Modal) -> El<'a> {
        let card = match modal {
            Modal::NewSession(f) => {
                let names: Vec<String> = self
                    .profiles
                    .list()
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                let agents = AgentKind::ALL.iter().fold(row![].spacing(6), |r, a| {
                    let (g, c) = agent_glyph(*a);
                    r.push(
                        button(
                            row![
                                text(g).color(c).size(13),
                                text(i18n::agent_label(*a)).size(13)
                            ]
                            .spacing(6),
                        )
                        .padding([5, 12])
                        .style(style::choice(f.agent == *a))
                        .on_press(Message::NewAgent(*a))
                        .boxed(),
                    )
                });
                let mut c = column![
                    text(tr::new_session()).size(18).font(style::BOLD),
                    label(tr::repo_folder_label()),
                    row![
                        text_input(tr::repo_placeholder(), &f.repo)
                            .id(NEW_REPO_ID)
                            .on_input(Message::NewRepo)
                            .on_submit(Message::NewSubmit)
                            .size(13)
                            .padding([6, 8])
                            .style(style::input)
                            .width(Fill),
                        button(text(tr::browse()).size(13))
                            .padding([6, 12])
                            .style(style::secondary)
                            .on_press(Message::NewBrowse),
                    ]
                    .spacing(6),
                    label(tr::profile_label()),
                    pick_list(Some(f.profile.clone()), names, |s: &String| s.clone())
                        .on_select(Message::NewProfile)
                        .text_size(13)
                        .padding([5, 8])
                        .width(Fill),
                    label(tr::agent_label()),
                    agents,
                    label(tr::title_label()),
                    text_input(tr::title_placeholder(), &f.title)
                        .on_input(Message::NewTitle)
                        .on_submit(Message::NewSubmit)
                        .size(13)
                        .padding([6, 8])
                        .style(style::input),
                    label(tr::base_ref_label()),
                    text_input(tr::base_ref_placeholder(), &f.base_ref)
                        .on_input(Message::NewBase)
                        .on_submit(Message::NewSubmit)
                        .size(13)
                        .padding([6, 8])
                        .style(style::input),
                    text(tr::worktree_note()).size(11).color(style::FAINT),
                ]
                .spacing(8);
                if let Some(e) = &f.error {
                    c = c.push(
                        container(text(e.as_str()).size(12))
                            .padding(8)
                            .width(Fill)
                            .style(style::error_box)
                            .boxed(),
                    );
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press(Message::CloseModal),
                        button(text(if f.busy { tr::creating() } else { tr::create() }).size(13))
                            .padding([6, 14])
                            .style(style::primary)
                            .on_press_maybe((!f.busy).then_some(Message::NewSubmit)),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(520).boxed()
            }
            Modal::Delete(f) => {
                let s = self.session(&f.id);
                let title = s.map(|s| s.title.as_str()).unwrap_or("?");
                let mut c = column![
                    text(tr::delete_session_title(title))
                        .size(18)
                        .font(style::BOLD),
                    text(tr::delete_session_body()).size(13).color(style::DIM),
                    checkbox(f.remove_worktree)
                        .label(tr::delete_remove_worktree())
                        .on_toggle(Message::DeleteWorktree)
                        .size(16)
                        .text_size(13),
                ]
                .spacing(10);
                if let Some(s) = s {
                    c = c.push(
                        text(format!("{}  ({})", s.worktree_path.display(), s.branch))
                            .size(11)
                            .font(style::MONO)
                            .color(style::FAINT)
                            .boxed(),
                    );
                }
                if let Some(e) = &f.error {
                    c = c.push(
                        container(text(e.as_str()).size(12))
                            .padding(8)
                            .width(Fill)
                            .style(style::error_box)
                            .boxed(),
                    );
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::cancel()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press_maybe((!f.busy).then_some(Message::CloseModal)),
                        button(text(if f.busy { tr::deleting() } else { tr::delete() }).size(13))
                            .padding([6, 14])
                            .style(style::danger)
                            .on_press_maybe((!f.busy).then_some(Message::DeleteConfirm)),
                    ]
                    .spacing(8)
                    .boxed(),
                );
                c.width(480).boxed()
            }
            Modal::Settings(f) => {
                let mut list: Column<El<'_>> = Column::new().spacing(4);
                for p in self.profiles.list() {
                    let (total, running) = self.sessions_in_profile(&p.name);
                    let row_el: El<'_> = match &f.renaming {
                        Some((old, new)) if old == &p.name => row![
                            text_input(tr::profile_rename_placeholder(), new)
                                .on_input(Message::SettingsRenameInput)
                                .on_submit(Message::SettingsRenameCommit)
                                .size(13)
                                .padding([4, 8])
                                .style(style::input)
                                .width(Fill),
                            button(text(tr::save()).size(12))
                                .padding([4, 10])
                                .style(style::primary)
                                .on_press(Message::SettingsRenameCommit),
                        ]
                        .spacing(6)
                        .boxed(),
                        _ => row![
                            text(p.name.as_str()).size(13).width(Fill),
                            text(tr::profile_sessions(
                                i18n::count(total),
                                i18n::count(running)
                            ))
                            .size(11)
                            .color(style::DIM),
                            icon_button(
                                I_EDIT,
                                Some(Message::SettingsStartRename(p.name.clone())),
                                tr::rename()
                            ),
                            icon_button(
                                I_DELETE,
                                Some(Message::SettingsDelete(p.name.clone())),
                                tr::profile_delete_tooltip()
                            ),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .boxed(),
                    };
                    list = list.push(
                        container(row_el)
                            .padding([4, 8])
                            .style(style::bg(style::HEADER))
                            .boxed(),
                    );
                }
                let languages = LanguagePref::all()
                    .into_iter()
                    .fold(row![].spacing(6), |r, p| {
                        let name = match p {
                            LanguagePref::System => tr::language_system(),
                            LanguagePref::Fixed(l) => l.native_name().to_owned(),
                        };
                        r.push(
                            button(text(name).size(13))
                                .padding([5, 12])
                                .style(style::choice(self.ui.language == p))
                                .on_press(Message::SetLanguage(p))
                                .boxed(),
                        )
                    });
                let mut c = column![
                    text(tr::settings()).size(18).font(style::BOLD),
                    label(tr::language_label()),
                    languages,
                    space().height(4),
                    label(tr::profiles_label()),
                    text(tr::profiles_note()).size(12).color(style::DIM),
                    list,
                    row![
                        text_input(tr::profile_name_placeholder(), &f.new_name)
                            .id(PROFILE_NAME_ID)
                            .on_input(Message::SettingsNewName)
                            .on_submit(Message::SettingsCreate)
                            .size(13)
                            .padding([5, 8])
                            .style(style::input)
                            .width(Fill),
                        button(text(tr::add()).size(13))
                            .padding([5, 12])
                            .style(style::secondary)
                            .on_press(Message::SettingsCreate),
                    ]
                    .spacing(6),
                ]
                .spacing(10);
                if let Some(e) = &f.error {
                    c = c.push(
                        container(text(e.as_str()).size(12))
                            .padding(8)
                            .width(Fill)
                            .style(style::error_box)
                            .boxed(),
                    );
                }
                c = c.push(
                    row![
                        space().width(Fill),
                        button(text(tr::close()).size(13))
                            .padding([6, 14])
                            .style(style::secondary)
                            .on_press(Message::CloseModal),
                    ]
                    .boxed(),
                );
                c.width(460).boxed()
            }
        };
        opaque(
            mouse_area(
                center(opaque(container(card).padding(20).style(style::modal_card)))
                    .style(style::backdrop),
            )
            .on_press(Message::CloseModal),
        )
        .into()
    }
}
