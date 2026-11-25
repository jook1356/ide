//! The profiles page (Hermes Desk's agents page: `AgentList.tsx` + `AgentsPage.tsx`): the
//! profile list sits beside the rail where the work page has its session list (picked there,
//! added with "프로필 추가"; the page's rail button shows / hides it), and the page shows the
//! picked profile in separate sections (기본, 지침, 스킬, MCP 서버, 기본 레포) with 저장 /
//! 되돌리기 in its title row. Skills apply at once (`skills_ui.rs`); the rest waits for 저장.

use iced::widget::{
    Column, button, checkbox, column, container, row, scrollable, space, text, text_editor,
    text_input,
};
use iced::{Alignment, Element, Fill, Widget as _};

use ide_client::Support;

use crate::app::{App, CONFIG_NAME_ID, ConfigEdit, McpField, Message};
use crate::i18n::{self, tr};
use crate::list_edit::ListKind;
use crate::icons::{self, Icon};
use crate::style;
use crate::view::{card, error_box, icon_button, tip};

type El<'a> = Element<'a, Message>;

impl App {
    /// The page layer of the profiles page: the list (when shown), then the profile.
    pub(crate) fn profiles_page(&self) -> El<'_> {
        let page = self.profile_view();
        if !self.ui.sessions_open {
            return page;
        }
        row![self.profile_list(), self.list_handle(), page,]
        .height(Fill)
        .boxed()
    }

    /// Desk's `AgentList`: title and hide button, "프로필 추가", one row per profile.
    fn profile_list(&self) -> El<'_> {
        let online = self.connected();
        let shown = self.profile_shown().map(|c| c.slug.as_str());
        let head = row![
            text(tr::nav_configurations()).size(13).font(style::bold()),
            space().width(Fill),
            self.list_edit_toggle(ListKind::Profiles),
            tip(
                button(icons::icon(Icon::SidebarCollapse, 16.0, style::DIM))
                    .padding(4)
                    .style(style::icon)
                    .on_press(Message::Page(crate::ui_state::Page::Configurations))
                    .boxed(),
                tr::profiles_hide(),
            ),
        ]
        .align_y(Alignment::Center);
        let add = button(
            row![
                icons::icon(Icon::Plus, 14.0, style::ON_ACCENT),
                text(tr::profile_add()).size(13),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Fill)
        .padding([6, 10])
        .style(style::primary)
        .on_press_maybe(online.then_some(Message::ProfileAdding(true)));
        let editing = self.list_editing(ListKind::Profiles);
        let mut list: Column<El<'_>> = Column::new().spacing(2);
        for c in &self.configs {
            let (total, _) = self.sessions_in_configuration(&c.slug);
            let selected = shown == Some(c.slug.as_str());
            list = list.push(
                button(
                    row![
                        editing.then(|| self.list_row_box(&c.slug)),
                        icons::icon(
                            Icon::Stack2,
                            16.0,
                            if selected { style::ACCENT_TEXT } else { style::DIM },
                        ),
                        column![
                            text(c.name.as_str())
                                .size(13)
                                .wrapping(text::Wrapping::None)
                                .ellipsis(text::Ellipsis::End),
                            text(tr::profile_row_detail(
                                i18n::count(total),
                                i18n::count(c.skills.len())
                            ))
                            .size(11)
                            .color(style::DIM)
                            .wrapping(text::Wrapping::None)
                            .ellipsis(text::Ellipsis::End),
                        ]
                        .spacing(1)
                        .width(Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Fill)
                .padding([6, 8])
                .style(style::row(self.list_row_lit(ListKind::Profiles, &c.slug, selected)))
                .on_press(self.list_row_press(
                    ListKind::Profiles,
                    &c.slug,
                    Message::CfgEdit(c.slug.clone()),
                ))
                .boxed(),
            );
        }
        let bar = editing.then(|| self.list_edit_bar(ListKind::Profiles));
        container(
            column![head, add, bar, scrollable(list).height(Fill)]
                .spacing(10)
                .padding(10),
        )
        .width(self.list_w())
        .height(Fill)
        .style(style::bg(style::SIDEBAR))
        .boxed()
    }

    /// The picked profile: title row (name, 저장 안 됨, 삭제 / 되돌리기 / 저장), then one card per
    /// section.
    fn profile_view(&self) -> El<'_> {
        let f = &self.settings;
        let online = self.connected();
        let avail = self.window_size.width - self.left_w();
        let width = (avail - 64.0).clamp(320.0, 896.0);
        let mut c = Column::<El<'_>>::new().spacing(16);
        match (self.profile_shown(), &f.cfg_edit) {
            (Some(p), Some(e)) => {
                c = c.push(self.profile_title(p.slug.as_str(), e, online));
                let (total, running) = self.sessions_in_configuration(&p.slug);
                c = c.push(
                    column![
                        text(tr::configuration_sessions(
                            i18n::count(total),
                            i18n::count(running)
                        ))
                        .size(12)
                        .color(style::TEXT),
                        text(tr::configurations_note()).size(12).color(style::DIM),
                    ]
                    .spacing(4)
                    .boxed(),
                );
                if let Some(err) = &f.error {
                    c = c.push(error_box(err));
                }
                for s in self.profile_sections(e, online) {
                    c = c.push(s);
                }
                c = c.push(text(self.provider_support()).size(10).color(style::FAINT).boxed());
            }
            _ => {
                c = c.push(
                    container(text(tr::nav_configurations()).size(22).font(style::bold()))
                        .height(40)
                        .align_y(Alignment::Center)
                        .boxed(),
                );
                c = c.push(text(tr::profiles_none()).size(13).color(style::DIM).boxed());
            }
        }
        container(scrollable(
            container(c.width(width)).center_x(Fill).padding([32, 0]),
        ))
        .width(Fill)
        .height(Fill)
        .style(style::bg(style::BG))
        .boxed()
    }

    fn profile_title<'a>(&'a self, slug: &'a str, e: &'a ConfigEdit, online: bool) -> El<'a> {
        let f = &self.settings;
        let dirty = e.dirty();
        let can = online && !f.busy;
        let mut title = row![
            text(e.base.name.as_str())
                .size(22)
                .font(style::bold())
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::End),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if dirty {
            title = title.push(
                container(text(tr::profile_unsaved()).size(11).color(style::AMBER))
                    .padding([2, 8])
                    .style(style::card)
                    .boxed(),
            );
        }
        if dirty && e.outdated {
            title = title.push(tip(
                container(text(tr::profile_outdated()).size(11).color(style::RED))
                    .padding([2, 8])
                    .style(style::card)
                    .boxed(),
                tr::profile_outdated_tooltip(),
            ));
        }
        let actions: El<'_> = if f.profile_delete_armed {
            row![
                text(tr::profile_delete_question()).size(12).color(style::DIM),
                button(text(tr::delete()).size(12))
                    .padding([5, 12])
                    .style(style::danger)
                    .on_press_maybe(can.then(|| Message::CfgDelete(slug.to_owned()))),
                button(text(tr::cancel()).size(12))
                    .padding([5, 12])
                    .style(style::secondary)
                    .on_press(Message::ProfileDeleteArm(false)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .boxed()
        } else {
            row![
                tip(
                    button(icons::icon(Icon::Trash, 15.0, style::DIM))
                        .padding([6, 8])
                        .style(style::icon)
                        .on_press_maybe(can.then_some(Message::ProfileDeleteArm(true)))
                        .boxed(),
                    tr::configuration_delete_tooltip(),
                ),
                button(text(tr::profile_revert()).size(12))
                    .padding([5, 12])
                    .style(style::secondary)
                    .on_press_maybe(dirty.then_some(Message::CfgCancel)),
                button(text(tr::save()).size(12))
                    .padding([5, 12])
                    .style(style::primary)
                    .on_press_maybe((can && dirty).then_some(Message::CfgSave)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .boxed()
        };
        container(
            row![container(title).width(Fill), actions]
                .spacing(12)
                .align_y(Alignment::Center),
        )
        .height(40)
        .align_y(Alignment::Center)
        .boxed()
    }

    /// The sections of the editor, each a card with a title and a note.
    fn profile_sections<'a>(&'a self, e: &'a ConfigEdit, online: bool) -> Vec<El<'a>> {
        let input = |ph: String, v: &'a str| {
            text_input(ph, v)
                .size(12)
                .padding([5, 8])
                .style(style::input)
        };
        let add_button = |msg: Message| {
            button(text(tr::add()).size(11))
                .padding([3, 10])
                .style(style::secondary)
                .on_press(msg)
                .boxed()
        };

        let basic = section(
            tr::profile_section_basic(),
            None,
            None,
            column![
                text(tr::configuration_name_label()).size(11).color(style::DIM),
                input(tr::configuration_name_placeholder(), &e.name)
                    .on_input(Message::CfgEditName),
            ]
            .spacing(4)
            .boxed(),
        );

        let instructions = section(
            tr::configuration_instructions(),
            Some(tr::profile_instructions_note()),
            None,
            text_editor(&e.instructions)
                .on_action(Message::CfgInstructions)
                .size(12)
                .height(160)
                .boxed(),
        );

        let skills = section(
            tr::configuration_skills(),
            Some(tr::skills_note()),
            None,
            self.skills_editor(e, online),
        );

        let mut mcp: Column<El<'_>> = Column::new().spacing(6);
        for (i, m) in e.mcp.iter().enumerate() {
            mcp = mcp.push(
                row![
                    input(tr::mcp_name_placeholder(), &m.name)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Name, v))
                        .width(110),
                    input(tr::mcp_command_placeholder(), &m.command)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Command, v))
                        .width(110),
                    input(tr::mcp_args_placeholder(), &m.args)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Args, v))
                        .width(Fill),
                    input(tr::mcp_env_placeholder(), &m.env)
                        .on_input(move |v| Message::CfgMcp(i, McpField::Env, v))
                        .width(140),
                    icon_button("✕", Some(Message::CfgMcpRemove(i)), tr::remove()),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }
        if e.mcp.is_empty() {
            mcp = mcp.push(text(tr::mcp_none()).size(11).color(style::FAINT).boxed());
        }
        let mcp = section(
            tr::configuration_mcp(),
            Some(tr::profile_mcp_note()),
            Some(add_button(Message::CfgMcpAdd)),
            mcp.boxed(),
        );

        // Desk's "기본 레포": pre-checked in the new-session dialog.
        let registered: Vec<String> = self.repos.iter().map(|r| r.id.clone()).collect();
        let defaults = crate::app::default_repos_for(e.default_repos.as_ref(), &registered);
        let mut repos: Column<El<'_>> = Column::new().spacing(4);
        for r in &self.repos {
            let id = r.id.clone();
            repos = repos.push(
                checkbox(defaults.contains(&r.id))
                    .label(format!("{}  ·  {}", r.name, r.path))
                    .on_toggle(move |v| Message::CfgDefaultRepo(id.clone(), v))
                    .size(14)
                    .text_size(12)
                    .boxed(),
            );
        }
        if self.repos.is_empty() {
            repos = repos.push(text(tr::repos_none()).size(11).color(style::FAINT).boxed());
        }
        let repos = section(
            tr::configuration_default_repos(),
            Some(tr::configuration_default_repos_note()),
            None,
            repos.boxed(),
        );

        let history = section(
            tr::profile_history(),
            Some(tr::profile_history_note()),
            None,
            self.profile_history_view(&e.base.slug, online),
        );

        vec![basic, instructions, skills, mcp, repos, history]
    }

    /// Which features the providers pass on, per session mode (✓ native, ~ fallback, ✕ not).
    fn provider_support(&self) -> String {
        self.providers
            .iter()
            .filter(|p| p.caps.accounts)
            .map(|p| {
                let s = |x: Support| match x {
                    Support::Native => "✓",
                    Support::Fallback => "~",
                    Support::Unsupported => "✕",
                };
                tr::provider_support_modes(
                    &p.name,
                    &format!(
                        "{}{}{}",
                        s(p.caps.skills),
                        s(p.caps.mcp),
                        s(p.caps.instructions)
                    ),
                    &format!(
                        "{}{}{}",
                        s(p.caps.original_repo.skills),
                        s(p.caps.original_repo.mcp),
                        s(p.caps.original_repo.instructions)
                    ),
                )
            })
            .collect::<Vec<_>>()
            .join("   ")
    }

    /// Desk's "에이전트 추가" dialog: the new profile's name.
    pub(crate) fn add_profile_card(&self) -> El<'_> {
        let f = &self.settings;
        let can = self.connected() && !f.busy && !f.cfg_name.trim().is_empty();
        let mut c = column![
            text(tr::profile_add()).size(18).font(style::bold()),
            text(tr::configurations_note()).size(12).color(style::DIM),
            text_input(tr::configuration_name_placeholder(), &f.cfg_name)
                .id(CONFIG_NAME_ID)
                .on_input(Message::CfgName)
                .on_submit_maybe(can.then_some(Message::CfgAdd))
                .size(13)
                .padding([6, 8])
                .style(style::input),
        ]
        .spacing(12);
        if let Some(e) = &f.error {
            c = c.push(error_box(e));
        }
        c.push(
            row![
                space().width(Fill),
                button(text(tr::cancel()).size(13))
                    .padding([6, 14])
                    .style(style::secondary)
                    .on_press(Message::ProfileAdding(false)),
                button(text(tr::profile_add()).size(13))
                    .padding([6, 14])
                    .style(style::primary)
                    .on_press_maybe(can.then_some(Message::CfgAdd)),
            ]
            .spacing(8)
            .boxed(),
        )
        .width(440)
        .boxed()
    }
}

/// A section of the profile: a card with its title (and an action on the right), a note, then
/// the content.
fn section<'a>(title: String, note: Option<String>, action: Option<El<'a>>, body: El<'a>) -> El<'a> {
    let mut head = iced::widget::Row::<El<'a>>::new()
        .push(text(title).size(15).font(style::bold()).boxed())
        .push(space().width(Fill).boxed())
        .align_y(Alignment::Center);
    if let Some(a) = action {
        head = head.push(a);
    }
    let mut c = Column::<El<'a>>::new().push(head.boxed()).spacing(10);
    if let Some(n) = note {
        c = c.push(text(n).size(11).color(style::DIM).boxed());
    }
    card(c.push(body).boxed())
}
