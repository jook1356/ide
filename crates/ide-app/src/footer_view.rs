//! View of the Changes / History footer and its dialogs (state and logic in `footer.rs`).
//! Layout follows Hermes Desk's `.ai-actions` row: round `xl` buttons — 커밋 맡기기 (violet
//! light, sparkles, count badge when only some files are checked), the sync split button
//! (`SyncButton`) and the round "…" menu at the top end — with the PR strips and the PR job
//! notice above it.

use std::sync::LazyLock;

use iced::widget::{
    Column, button, checkbox, column, container, pick_list, popover, row, scrollable, space, svg,
    text, text_input,
};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length, Shadow, Theme, Widget as _, color,
};
use ide_client::{LeftoverMatch, PrMergeMethod, SyncStatus};

use crate::app::{App, Live, Message};
use crate::footer::{
    ClosePrForm, ContinueForm, FooterMenu, FooterModal, FooterMsg, JobAction, JobStage, Load,
    MergeForm, MoreKind, PrForm, PrJob, RecheckForm, RepoFooter, SyncConfirmForm, SyncOp, Tone,
    primary_op,
};
use crate::i18n::{self, tr};
use crate::style;
use crate::view::tip;

/// `push` for element columns / rows that boxes its argument (widgets or elements).
trait Pb<'a> {
    fn pb(self, x: impl iced::Widget<Message> + 'a) -> Self;
}

impl<'a> Pb<'a> for Column<El<'a>> {
    fn pb(self, x: impl iced::Widget<Message> + 'a) -> Self {
        self.push(x.boxed())
    }
}

impl<'a> Pb<'a> for iced::widget::Row<El<'a>> {
    fn pb(self, x: impl iced::Widget<Message> + 'a) -> Self {
        self.push(x.boxed())
    }
}

fn tipw<'a>(w: impl iced::Widget<Message> + 'a, label: String) -> El<'a> {
    tip(w.boxed(), label)
}

type El<'a> = Element<'a, Message>;

/// Widget id of the PR dialog's title input.
pub const PR_TITLE_ID: &str = "ide-pr-title";
/// Widget id of the branch-continue dialog's name input.
pub const CONTINUE_NAME_ID: &str = "ide-continue-name";

// ---------------------------------------------------------------------------- icons

/// Tabler icons of the footer (`assets/icons/`, unmodified files from tabler-icons), drawn like
/// `crate::icons` with `stroke={1.5}` (the count arrows with 2.5, like Desk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FIcon {
    Sparkles,
    Dots,
    CloudUpload,
    CloudDownload,
    CloudOff,
    Refresh,
    ChevronDown,
    GitMerge,
    GitPullRequest,
    GitPullRequestClosed,
    ExternalLink,
    AlertTriangle,
    GitBranch,
    ArrowUp,
    ArrowDown,
    Search,
}

const SOURCES: [&str; 16] = [
    include_str!("../assets/icons/sparkles.svg"),
    include_str!("../assets/icons/dots.svg"),
    include_str!("../assets/icons/cloud-upload.svg"),
    include_str!("../assets/icons/cloud-download.svg"),
    include_str!("../assets/icons/cloud-off.svg"),
    include_str!("../assets/icons/refresh.svg"),
    include_str!("../assets/icons/chevron-down.svg"),
    include_str!("../assets/icons/git-merge.svg"),
    include_str!("../assets/icons/git-pull-request.svg"),
    include_str!("../assets/icons/git-pull-request-closed.svg"),
    include_str!("../assets/icons/external-link.svg"),
    include_str!("../assets/icons/alert-triangle.svg"),
    include_str!("../assets/icons/git-branch.svg"),
    include_str!("../assets/icons/arrow-up.svg"),
    include_str!("../assets/icons/arrow-down.svg"),
    include_str!("../assets/icons/search.svg"),
];

static HANDLES: LazyLock<Vec<svg::Handle>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let bold = i == FIcon::ArrowUp as usize || i == FIcon::ArrowDown as usize;
            let w = if bold { "2.5" } else { "1.5" };
            let s = s.replace("stroke-width=\"2\"", &format!("stroke-width=\"{w}\""));
            svg::Handle::from_memory(s.into_bytes())
        })
        .collect()
});

fn ic<'a>(i: FIcon, size: f32, c: Color) -> El<'a> {
    svg(HANDLES[i as usize].clone())
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(c) })
        .boxed()
}

// ---------------------------------------------------------------------------- styles

/// Mantine violet (`violet-6` / `violet-4`).
const VIOLET: Color = color!(0x7950f2);
const VIOLET_TEXT: Color = color!(0xb197fc);
const ORANGE: Color = color!(0xfd7e14);
const YELLOW: Color = color!(0xfab005);
const TEAL: Color = color!(0x20c997);

fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

fn bstyle(bg: Option<Color>, fg: Color, border: Color, radius: f32) -> button::Style {
    button::Style {
        background: bg.map(Background::Color),
        text_color: fg,
        border: Border {
            color: border,
            width: if border.a > 0.0 { 1.0 } else { 0.0 },
            radius: radius.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Desk `variant="light" color="violet" radius="xl"`.
fn pill_violet(_: &Theme, s: button::Status) -> button::Style {
    match s {
        button::Status::Hovered | button::Status::Pressed => bstyle(
            Some(alpha(VIOLET, 0.26)),
            VIOLET_TEXT,
            Color::TRANSPARENT,
            16.0,
        ),
        button::Status::Disabled => bstyle(
            Some(alpha(VIOLET, 0.08)),
            alpha(VIOLET_TEXT, 0.4),
            Color::TRANSPARENT,
            16.0,
        ),
        button::Status::Active => bstyle(
            Some(alpha(VIOLET, 0.16)),
            VIOLET_TEXT,
            Color::TRANSPARENT,
            16.0,
        ),
    }
}

/// Desk `variant="default"` with the given radius.
fn default_btn(radius: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, s| match s {
        button::Status::Hovered | button::Status::Pressed => {
            bstyle(Some(style::HOVER), style::TEXT, style::BORDER, radius)
        }
        button::Status::Disabled => bstyle(Some(style::PANEL), style::FAINT, style::BORDER, radius),
        button::Status::Active => bstyle(Some(style::HEADER), style::TEXT, style::BORDER, radius),
    }
}

fn menu_item(_: &Theme, s: button::Status) -> button::Style {
    match s {
        button::Status::Hovered | button::Status::Pressed => {
            bstyle(Some(style::HOVER), style::TEXT, Color::TRANSPARENT, 4.0)
        }
        button::Status::Disabled => bstyle(None, style::FAINT, Color::TRANSPARENT, 4.0),
        button::Status::Active => bstyle(None, style::TEXT, Color::TRANSPARENT, 4.0),
    }
}

fn link_btn(_: &Theme, s: button::Status) -> button::Style {
    match s {
        button::Status::Hovered | button::Status::Pressed => {
            bstyle(Some(style::HOVER), VIOLET_TEXT, Color::TRANSPARENT, 4.0)
        }
        button::Status::Disabled => bstyle(None, style::FAINT, Color::TRANSPARENT, 4.0),
        button::Status::Active => bstyle(None, VIOLET_TEXT, Color::TRANSPARENT, 4.0),
    }
}

/// `.pr-leftover-strip` (yellow) / `[data-tone='warning']` (orange).
fn strip_style(c: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, s| {
        let bg = match s {
            button::Status::Hovered | button::Status::Pressed => 0.16,
            _ => 0.09,
        };
        bstyle(Some(alpha(c, bg)), style::TEXT, alpha(c, 0.4), 8.0)
    }
}

fn tone_color(t: Tone) -> Color {
    match t {
        Tone::Info => VIOLET_TEXT,
        Tone::Success => style::GREEN,
        Tone::Warning => ORANGE,
        Tone::Error => style::RED,
    }
}

fn card_style(c: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(c, 0.08).into()),
        text_color: Some(style::TEXT),
        border: Border {
            color: alpha(c, 0.35),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn band(_: &Theme) -> container::Style {
    container::Style {
        background: Some(color!(0x17181b).into()),
        text_color: Some(style::TEXT),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn small<'a>(s: String) -> text::Text<'a> {
    text(s).size(11).color(style::DIM)
}

fn title<'a>(s: impl text::IntoFragment<'a>) -> El<'a> {
    text(s).size(17).font(style::BOLD).boxed()
}

fn dot<'a>(c: Color) -> El<'a> {
    container(space().width(7).height(7))
        .style(move |_| container::Style {
            background: Some(c.into()),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .boxed()
}

fn err_box<'a>(t: String) -> El<'a> {
    container(text(t).size(12))
        .padding([6, 10])
        .width(Fill)
        .style(style::error_box)
        .boxed()
}

fn warn<'a>(t: String) -> El<'a> {
    container(text(t).size(12))
        .padding([6, 10])
        .width(Fill)
        .style(style::warn_box)
        .boxed()
}

fn btn<'a>(
    label: String,
    style_fn: fn(&Theme, button::Status) -> button::Style,
    msg: Option<Message>,
) -> El<'a> {
    button(text(label).size(13))
        .padding([6, 14])
        .style(style_fn)
        .on_press_maybe(msg)
        .boxed()
}

fn fm(sid: &str, m: FooterMsg) -> Message {
    Message::Footer(sid.to_owned(), m)
}

fn method_label(m: &PrMergeMethod) -> String {
    match m {
        PrMergeMethod::Squash => "Squash and merge".into(),
        PrMergeMethod::Merge => "Create a merge commit".into(),
        PrMergeMethod::Rebase => "Rebase and merge".into(),
    }
}

const METHODS: [PrMergeMethod; 3] = [
    PrMergeMethod::Squash,
    PrMergeMethod::Merge,
    PrMergeMethod::Rebase,
];

// ---------------------------------------------------------------------------- footer

impl App {
    /// The footer under Changes / History (nothing under Explore or without repos).
    pub fn footer_section<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        if d.repo.is_empty() || !crate::footer::shown_on(d.side) {
            return space().height(0).boxed();
        }
        let sid = live.info.id.as_str();
        let f = d.footer.repos.get(&d.repo);
        let mut col = Column::new().spacing(0);
        if let Some(j) = f.and_then(|f| f.job.as_ref()) {
            col = col.pb(self.job_card(sid, &d.repo, j));
        }
        let open_pr = f.and_then(RepoFooter::open_pr).is_some();
        if f.and_then(RepoFooter::recheck).is_some() {
            col = col.pb(strip(
                ORANGE,
                tr::foot_recheck_strip(),
                tr::foot_recheck_strip_action(),
                tr::foot_recheck_strip_tip(),
                Some(fm(
                    sid,
                    FooterMsg::JobAction(d.repo.clone(), JobAction::Recheck),
                )),
            ));
        } else if f.is_some_and(|f| f.leftover) && self.footer_workspace(sid) && !open_pr {
            col = col.pb(strip(
                YELLOW,
                tr::foot_leftover_strip(),
                tr::foot_leftover_strip_action(),
                tr::foot_leftover_strip_tip(),
                Some(fm(sid, FooterMsg::OpenLeftover)),
            ));
        }
        let bar = self.footer_bar(sid, f);
        let menu = d.footer.menu;
        let popup = menu.map(|m| match m {
            FooterMenu::Sync => self.sync_menu(sid, f),
            FooterMenu::More => self.more_menu(sid),
        });
        col = col.pb(popover(bar, popup)
            .position(iced::widget::popover::Position::Top)
            .gap(4)
            .on_close(fm(sid, FooterMsg::CloseMenu)));
        column![
            container(space().height(1).width(Fill)).style(style::bg(style::BORDER)),
            container(col).padding([6, 8]),
        ]
        .boxed()
    }

    fn footer_bar<'a>(&'a self, sid: &str, f: Option<&'a RepoFooter>) -> El<'a> {
        let (files, total) = self.footer_checked(sid);
        let partial = !files.is_empty() && files.len() < total;
        let hint = if files.is_empty() {
            tr::foot_commit_pick()
        } else if partial {
            tr::foot_commit_partial(i18n::count(files.len()))
        } else {
            tr::foot_commit_all()
        };
        let mut label = row![
            ic(FIcon::Sparkles, 14.0, VIOLET_TEXT),
            text(tr::foot_commit())
                .size(12)
                .font(style::BOLD)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if partial {
            label = label.pb(
                container(text(files.len().to_string()).size(10).font(style::BOLD))
                    .padding([0, 5])
                    .style(|_| container::Style {
                        background: Some(VIOLET.into()),
                        text_color: Some(Color::WHITE),
                        border: Border {
                            radius: 8.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            );
        }
        let commit = tipw(
            button(container(label).center_x(Fill))
                .padding([6, 8])
                .width(Fill)
                .style(pill_violet)
                .on_press_maybe(
                    (!files.is_empty() && self.connected()).then(|| fm(sid, FooterMsg::Commit)),
                ),
            hint,
        );
        let more = tipw(
            button(
                container(ic(FIcon::Dots, 16.0, style::TEXT))
                    .center_x(Fill)
                    .center_y(Fill),
            )
            .width(30)
            .height(30)
            .padding(0)
            .style(default_btn(15.0))
            .on_press(fm(sid, FooterMsg::Menu(FooterMenu::More))),
            tr::foot_more(),
        );
        row![commit, self.sync_button(sid, f), more]
            .spacing(6)
            .align_y(Alignment::Center)
            .boxed()
    }

    /// Desk `SyncButton`: the main action with its counts, ▾ opens the menu.
    fn sync_button<'a>(&'a self, sid: &str, f: Option<&'a RepoFooter>) -> El<'a> {
        let sync = f.and_then(|f| f.sync.as_ref());
        if sync.is_some_and(|s| s.no_remote) {
            return tipw(
                button(
                    row![
                        ic(FIcon::CloudOff, 14.0, style::FAINT),
                        text(tr::foot_no_remote()).size(12)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([6, 10])
                .style(default_btn(16.0)),
                tr::foot_no_remote_tip(),
            );
        }
        let merged = f.and_then(RepoFooter::merged_pr);
        let open_pr = f.and_then(RepoFooter::open_pr).map(|p| p.number);
        let primary = primary_op(sync, merged.is_some());
        let (ahead, behind) = sync.map_or((0, 0), |s| (s.ahead, s.behind));
        let unpublished = sync.is_some_and(|s| s.unpublished);
        let running = f.and_then(|f| f.running);
        let (icon, label) = match primary {
            SyncOp::Publish => (FIcon::CloudUpload, tr::foot_sync_publish()),
            SyncOp::Pull => (FIcon::CloudDownload, tr::foot_sync_pull()),
            SyncOp::Push => (FIcon::CloudUpload, tr::foot_sync_push()),
            _ => (FIcon::Refresh, tr::foot_sync_fetch()),
        };
        let mut content = row![].spacing(5).align_y(Alignment::Center);
        content = content.pb(if running.is_some() {
            text("…").size(12).color(style::DIM).boxed()
        } else {
            ic(icon, 14.0, style::TEXT)
        });
        content = content.pb(text(label).size(12).wrapping(text::Wrapping::None));
        if !unpublished {
            if ahead > 0 {
                content = content.pb(row![
                    ic(FIcon::ArrowUp, 11.0, style::TEXT),
                    text(ahead.to_string()).size(11)
                ]
                .align_y(Alignment::Center));
            }
            if behind > 0 {
                content = content.pb(row![
                    ic(FIcon::ArrowDown, 11.0, style::TEXT),
                    text(behind.to_string()).size(11)
                ]
                .align_y(Alignment::Center));
            }
        }
        let hint = sync_hint(primary, sync, open_pr, merged);
        let main = tipw(
            button(content)
                .padding([6, 10])
                .height(30)
                .style(|t, s| {
                    let mut b = default_btn(16.0)(t, s);
                    b.border.radius = iced::border::Radius {
                        top_left: 16.0,
                        bottom_left: 16.0,
                        top_right: 0.0,
                        bottom_right: 0.0,
                    };
                    b
                })
                .on_press_maybe(
                    (running.is_none() && self.connected())
                        .then(|| fm(sid, FooterMsg::Sync(primary))),
                ),
            hint,
        );
        let chevron = tipw(
            button(
                container(ic(FIcon::ChevronDown, 14.0, style::TEXT))
                    .center_y(Fill)
                    .center_x(Fill),
            )
            .width(26)
            .height(30)
            .padding(0)
            .style(|t, s| {
                let mut b = default_btn(16.0)(t, s);
                b.border.radius = iced::border::Radius {
                    top_left: 0.0,
                    bottom_left: 0.0,
                    top_right: 16.0,
                    bottom_right: 16.0,
                };
                b
            })
            .on_press_maybe(
                running
                    .is_none()
                    .then(|| fm(sid, FooterMsg::Menu(FooterMenu::Sync))),
            ),
            tr::foot_sync_options(),
        );
        row![main, chevron].height(30).boxed()
    }

    fn sync_menu<'a>(&'a self, sid: &str, f: Option<&'a RepoFooter>) -> El<'a> {
        let sync = f.and_then(|f| f.sync.as_ref());
        let published = sync.is_some_and(|s| s.upstream.is_some());
        let unpublished = sync.is_some_and(|s| s.unpublished);
        let (ahead, behind) = sync.map_or((0, 0), |s| (s.ahead, s.behind));
        let item = |icon: FIcon,
                    c: Color,
                    label: String,
                    right: Option<String>,
                    msg: Option<Message>|
         -> El<'a> {
            let mut r = row![ic(icon, 14.0, c), text(label).size(13).color(c).width(Fill)]
                .spacing(8)
                .align_y(Alignment::Center);
            if let Some(x) = right {
                r = r.pb(text(x).size(11).color(style::DIM));
            }
            button(r)
                .padding([6, 8])
                .width(Fill)
                .style(menu_item)
                .on_press_maybe(msg)
                .boxed()
        };
        let col = column![
            item(
                FIcon::Refresh,
                style::TEXT,
                tr::foot_menu_fetch(),
                None,
                Some(fm(sid, FooterMsg::Sync(SyncOp::Fetch)))
            ),
            item(
                FIcon::CloudDownload,
                if published { style::TEXT } else { style::FAINT },
                tr::foot_menu_pull(),
                (behind > 0).then(|| format!("↓{behind}")),
                published.then(|| fm(sid, FooterMsg::Sync(SyncOp::Pull)))
            ),
            item(
                FIcon::GitMerge,
                if published { style::TEXT } else { style::FAINT },
                tr::foot_menu_rebase(),
                None,
                published.then(|| fm(sid, FooterMsg::Sync(SyncOp::Rebase)))
            ),
            item(
                FIcon::CloudUpload,
                style::TEXT,
                if unpublished {
                    tr::foot_sync_publish()
                } else {
                    tr::foot_sync_push()
                },
                (ahead > 0 && !unpublished).then(|| format!("↑{ahead}")),
                Some(fm(
                    sid,
                    FooterMsg::Sync(if unpublished {
                        SyncOp::Publish
                    } else {
                        SyncOp::Push
                    })
                ))
            ),
            container(space().height(1).width(Fill)).style(style::bg(style::BORDER)),
            container(small(tr::foot_menu_caution())).padding([4, 8]),
            item(
                FIcon::CloudUpload,
                if published { ORANGE } else { style::FAINT },
                tr::foot_menu_force_push(),
                None,
                published.then(|| fm(sid, FooterMsg::Sync(SyncOp::ForcePush)))
            ),
            item(
                FIcon::AlertTriangle,
                if published { style::RED } else { style::FAINT },
                tr::foot_menu_reset(),
                None,
                published.then(|| fm(sid, FooterMsg::Sync(SyncOp::Reset)))
            ),
        ]
        .spacing(2);
        container(col)
            .padding(6)
            .width(250)
            .style(style::modal_card)
            .boxed()
    }

    fn more_menu<'a>(&'a self, sid: &str) -> El<'a> {
        let repo = self.detail(sid).map(|d| d.repo.clone()).unwrap_or_default();
        let mut col = Column::new().spacing(2);
        for e in self.footer_more(sid) {
            let (icon, c) = match e.kind {
                MoreKind::Merge => (FIcon::GitMerge, style::TEXT),
                MoreKind::View => (FIcon::ExternalLink, style::TEXT),
                MoreKind::Close => (FIcon::GitPullRequestClosed, style::RED),
                MoreKind::Recheck => (FIcon::AlertTriangle, ORANGE),
                MoreKind::CreatePr => (FIcon::GitPullRequest, style::TEXT),
                MoreKind::Continue => (FIcon::GitBranch, style::TEXT),
            };
            let msg = match e.kind {
                MoreKind::Merge => FooterMsg::OpenMerge,
                MoreKind::View => FooterMsg::ViewPr,
                MoreKind::Close => FooterMsg::OpenClosePr,
                MoreKind::Recheck => FooterMsg::JobAction(repo.clone(), JobAction::Recheck),
                MoreKind::CreatePr => FooterMsg::OpenPr,
                MoreKind::Continue => FooterMsg::OpenContinue,
            };
            if e.kind == MoreKind::Continue {
                col = col
                    .pb(container(space().height(1).width(Fill)).style(style::bg(style::BORDER)));
            }
            let fg = if e.enabled {
                if e.kind == MoreKind::Close {
                    style::RED
                } else {
                    style::TEXT
                }
            } else {
                style::FAINT
            };
            let mut body = column![text(e.title).size(13).color(fg)].spacing(1);
            if let Some(h) = e.hint {
                body = body.pb(small(h).size(11));
            }
            col = col.pb(button(
                row![
                    container(ic(icon, 14.0, if e.enabled { c } else { style::FAINT }))
                        .padding([2, 0]),
                    body.width(Fill)
                ]
                .spacing(8),
            )
            .padding([6, 8])
            .width(Fill)
            .style(menu_item)
            .on_press_maybe(e.enabled.then(|| fm(sid, msg))));
        }
        container(col)
            .padding(6)
            .width(270)
            .style(style::modal_card)
            .boxed()
    }

    /// The PR job's notice (Desk shows it in its notification list).
    fn job_card<'a>(&'a self, sid: &str, repo: &str, j: &'a PrJob) -> El<'a> {
        let c = tone_color(j.tone);
        let mut head = row![
            dot(c),
            text(j.title.as_str())
                .size(12)
                .font(style::BOLD)
                .width(Fill)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if !j.running() {
            head = head.pb(button(text("×").size(12))
                .padding([0, 5])
                .style(style::icon)
                .on_press(fm(sid, FooterMsg::JobDismiss(repo.to_owned()))));
        }
        let mut col = column![head].spacing(4);
        if !j.message.is_empty() {
            col = col.pb(text(j.message.as_str()).size(11).color(style::DIM));
        }
        if matches!(j.stage, JobStage::Writing)
            && j.commit_text.is_none()
            && !j.reuse
            && let Some(p) = &j.pr_file
        {
            col = col.pb(text(tr::foot_job_file(p)).size(10).color(style::FAINT));
        }
        let mut acts = row![].spacing(4);
        for a in &j.actions {
            let (label, color_on) = match a {
                JobAction::Retry => (tr::foot_job_retry(), true),
                JobAction::Cancel => (tr::cancel(), false),
                JobAction::CreateNow => (tr::foot_job_create_now(), true),
                JobAction::Recheck => (tr::foot_recheck_strip_action(), true),
                JobAction::Open(_) => (tr::foot_job_open(), true),
            };
            acts = acts.pb(button(text(label).size(11).color(if color_on {
                VIOLET_TEXT
            } else {
                style::RED
            }))
            .padding([2, 8])
            .style(link_btn)
            .on_press(fm(sid, FooterMsg::JobAction(repo.to_owned(), a.clone()))));
        }
        if !j.actions.is_empty() {
            col = col.pb(acts);
        }
        container(col)
            .padding([6, 10])
            .width(Fill)
            .style(card_style(c))
            .boxed()
    }

    // ------------------------------------------------------------------ dialogs

    pub fn footer_modal_card<'a>(&'a self, m: &'a FooterModal) -> El<'a> {
        match m {
            FooterModal::Pr(f) => self.pr_card(f),
            FooterModal::Merge(f) => merge_card(f),
            FooterModal::ClosePr(f) => close_card(f),
            FooterModal::SyncConfirm(f) => self.sync_confirm_card(f),
            FooterModal::Continue(f) => continue_card(f),
            FooterModal::Recheck(f) => self.recheck_card(f),
        }
    }

    fn sync_confirm_card<'a>(&'a self, f: &'a SyncConfirmForm) -> El<'a> {
        let sync: Option<&SyncStatus> = self
            .footer(&f.session, &f.repo)
            .and_then(|r| r.sync.as_ref());
        let (ahead, behind) = sync.map_or((0, 0), |s| (s.ahead, s.behind));
        let upstream = sync
            .and_then(|s| s.upstream.clone())
            .unwrap_or_else(tr::foot_remote_branch);
        let reset = f.op == SyncOp::Reset;
        let body = if reset {
            tr::foot_reset_body(i18n::count(ahead as usize), &upstream)
        } else {
            tr::foot_force_body(i18n::count(behind as usize))
        };
        column![
            title(if reset {
                tr::foot_reset_title()
            } else {
                tr::foot_force_title()
            }),
            text(body).size(13),
            row![
                space().width(Fill),
                btn(tr::cancel(), style::secondary, Some(Message::CloseModal)),
                btn(
                    if reset {
                        tr::foot_reset_confirm()
                    } else {
                        tr::foot_menu_force_push()
                    },
                    style::danger,
                    Some(fm(&f.session, FooterMsg::SyncConfirmed))
                ),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .width(440)
        .boxed()
    }

    fn pr_card<'a>(&'a self, f: &'a PrForm) -> El<'a> {
        let sid = f.session.as_str();
        if f.asking() {
            return pending_card(f);
        }
        let mut c = Column::new().spacing(12);
        c = c.pb(row![
            ic(FIcon::GitPullRequest, 18.0, style::TEXT),
            title(tr::foot_pr_title(&f.repo_name))
        ]
        .spacing(8)
        .align_y(Alignment::Center));
        if let (Some(p), Some(e)) = (f.pending_work(), f.excluded()) {
            let mut parts = Vec::new();
            if e.commits && p.commit_count > 0 {
                parts.push(tr::foot_excluded_commits(i64::from(p.commit_count)));
            }
            if e.files && !p.files.is_empty() {
                parts.push(tr::foot_excluded_files(i18n::count(p.files.len())));
            }
            if !parts.is_empty() {
                c = c.pb(text(tr::foot_excluded(&parts.join(" · ")))
                    .size(11)
                    .color(VIOLET_TEXT));
            }
        }
        if let Some(e) = &f.request_error {
            c = c.pb(err_box(tr::foot_pr_request_error(e)));
        }
        if let Some(e) = &f.existing {
            c = c.pb(container(
                column![
                    text(tr::foot_pr_existing(
                        &e.number.map(|n| n.to_string()).unwrap_or_default()
                    ))
                    .size(13)
                    .font(style::BOLD)
                    .color(style::GREEN),
                    text(e.title.as_str()).size(12),
                    button(text(tr::foot_pr_open_existing()).size(12))
                        .padding([4, 10])
                        .style(style::primary)
                        .on_press(fm(sid, FooterMsg::PrOpenExisting)),
                ]
                .spacing(6),
            )
            .padding(10)
            .width(Fill)
            .style(card_style(style::GREEN)));
        } else if f.checked {
            c = c.pb(text(tr::foot_pr_none_open()).size(12).color(style::DIM));
        }
        let left_file = f.left.as_ref().and_then(|l| l.file.as_ref());
        if let Some(file) = left_file.filter(|_| !f.from_commits && f.existing.is_none()) {
            c = c.pb(leftover_card(f, file));
        }
        if f.no_changes() && f.existing.is_none() {
            let t = f.target.clone().unwrap_or_default();
            c = c.pb(container(
                column![
                    text(tr::foot_pr_same_as(&t)).size(12).font(style::BOLD),
                    text(tr::foot_pr_same_as_body(&t))
                        .size(11)
                        .color(style::DIM),
                ]
                .spacing(4),
            )
            .padding(10)
            .width(Fill)
            .style(card_style(style::DIM)));
        }
        // 머지 대상 브랜치
        match &f.targets {
            Load::Failed(e) => c = c.pb(err_box(tr::foot_pr_targets_failed(e))),
            Load::Loading => c = c.pb(small(tr::foot_pr_targets_loading())),
            Load::Ready(t) => {
                let mut col =
                    column![text(tr::foot_pr_target()).size(12).font(style::BOLD)].spacing(4);
                col = col.pb(row![
                    ic(FIcon::GitMerge, 16.0, style::DIM),
                    text(format!("{} →", t.current)).size(12).font(style::MONO),
                    pick_list(f.target.clone(), t.branches.clone(), |b: &String| b.clone())
                        .on_select({
                            let sid = sid.to_owned();
                            move |b| fm(&sid, FooterMsg::PrTarget(b))
                        })
                        .text_size(12)
                        .padding([4, 8])
                        .width(Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center));
                col = col.pb(small(tr::foot_pr_target_note()));
                if !f.session_base.is_empty()
                    && f.target.as_deref().is_some_and(|x| x != f.session_base)
                {
                    col = col.pb(text(tr::foot_pr_base_missing(
                        &f.session_base,
                        f.target.as_deref().unwrap_or(""),
                    ))
                    .size(11)
                    .color(YELLOW));
                }
                c = c.pb(col);
            }
        }
        // 커밋 기록으로 작성
        c = c.pb(column![
            checkbox(f.from_commits)
                .label(tr::foot_pr_from_commits())
                .on_toggle_maybe((!f.busy).then(|| {
                    let sid = sid.to_owned();
                    move |v| fm(&sid, FooterMsg::PrFromCommits(v))
                }))
                .size(16)
                .text_size(13),
            small(tr::foot_pr_from_commits_note()),
        ]
        .spacing(2));
        if f.from_commits {
            let mut col = column![small(tr::foot_pr_uncommitted_stay())].spacing(6);
            match &f.preview {
                Some(Load::Failed(e)) => col = col.pb(err_box(tr::foot_pr_preview_failed(e))),
                Some(Load::Ready(p)) => {
                    if p.commits.is_empty() {
                        col = col.pb(warn(tr::foot_pr_no_commits()));
                    }
                    col = col.pb(column![
                        text(tr::foot_pr_title_label()).size(12).font(style::BOLD),
                        small(tr::foot_pr_title_note()),
                        text_input("", &f.title)
                            .id(PR_TITLE_ID)
                            .on_input_maybe((!f.busy).then(|| {
                                let sid = sid.to_owned();
                                move |t| fm(&sid, FooterMsg::PrTitle(t))
                            }))
                            .size(13)
                            .padding([6, 8])
                            .style(style::input),
                    ]
                    .spacing(3));
                    col = col.pb(row![
                        small(tr::foot_pr_body_preview(i18n::count(p.commits.len()))).width(Fill),
                        button(
                            text(if f.show_body {
                                tr::foot_pr_body_hide()
                            } else {
                                tr::foot_pr_body_show()
                            })
                            .size(11)
                        )
                        .padding([2, 6])
                        .style(link_btn)
                        .on_press(fm(sid, FooterMsg::PrToggleBody)),
                    ]
                    .align_y(Alignment::Center));
                    if f.show_body {
                        col = col.pb(container(
                            scrollable(text(p.body.as_str()).size(11).font(style::MONO))
                                .height(160),
                        )
                        .padding([6, 8])
                        .width(Fill)
                        .style(band));
                    }
                }
                _ => col = col.pb(small(tr::foot_pr_preview_loading())),
            }
            c = c.pb(col);
        }
        c = c.pb(checkbox(f.draft)
            .label(tr::foot_pr_draft())
            .on_toggle_maybe((!f.busy).then(|| {
                let sid = sid.to_owned();
                move |v| fm(&sid, FooterMsg::PrDraft(v))
            }))
            .size(16)
            .text_size(13));
        let mut merge_col = column![
            checkbox(f.merge_now)
                .label(tr::foot_pr_merge_now())
                .on_toggle_maybe((!f.busy && !f.draft).then(|| {
                    let sid = sid.to_owned();
                    move |v| fm(&sid, FooterMsg::PrMergeNow(v))
                }))
                .size(16)
                .text_size(13),
            small(if f.draft {
                tr::foot_pr_merge_now_draft()
            } else {
                tr::foot_pr_merge_now_note()
            }),
        ]
        .spacing(2);
        if f.merge_now && !f.draft {
            merge_col = merge_col.pb(container(
                column![
                    small(tr::foot_merge_method()),
                    pick_list(Some(f.method), METHODS, method_label)
                        .on_select({
                            let sid = sid.to_owned();
                            move |m| fm(&sid, FooterMsg::PrMergeNowMethod(m))
                        })
                        .text_size(12)
                        .padding([4, 8]),
                    checkbox(f.delete_branch)
                        .label(tr::foot_merge_delete_branch())
                        .on_toggle({
                            let sid = sid.to_owned();
                            move |v| fm(&sid, FooterMsg::PrMergeNowDelete(v))
                        })
                        .size(14)
                        .text_size(12),
                ]
                .spacing(6),
            )
            .padding(iced::Padding {
                left: 26.0,
                top: 6.0,
                ..Default::default()
            }));
        }
        c = c.pb(merge_col);
        let push = f.push_commits();
        let what = if f.from_commits {
            if push {
                tr::foot_pr_what_commits_push()
            } else {
                tr::foot_pr_what_commits_remote()
            }
        } else if f.excluded().is_some_and(|e| e.commits) {
            tr::foot_pr_what_ai_remote()
        } else if f.excluded().is_some_and(|e| e.files) {
            tr::foot_pr_what_ai_no_files()
        } else {
            tr::foot_pr_what_ai()
        };
        c = c.pb(row![
            small(what).width(Fill),
            tipw(
                button(
                    row![
                        ic(FIcon::Search, 12.0, style::DIM),
                        text(tr::foot_pr_check_existing()).size(11)
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([2, 8])
                .style(style::icon)
                .on_press_maybe((!f.busy).then(|| fm(sid, FooterMsg::PrCheckExisting))),
                tr::foot_pr_check_existing_tip(),
            ),
        ]
        .spacing(8));
        let reuse_file = left_file.filter(|_| !f.from_commits && f.existing.is_none());
        if let Some(file) = reuse_file
            && file.matches == LeftoverMatch::Diverged
            && f.confirm_reuse
        {
            c = c.pb(text(tr::foot_pr_reuse_diverged_confirm())
                .size(11)
                .color(style::RED));
        }
        let can = f.target.is_some() && !f.busy && !f.no_changes();
        let mut buttons = row![
            space().width(Fill),
            btn(tr::cancel(), style::secondary, Some(Message::CloseModal))
        ]
        .spacing(8);
        if let Some(file) = reuse_file {
            let reuse_first = file.matches == LeftoverMatch::Same;
            buttons = buttons.pb(button(
                row![
                    ic(FIcon::Sparkles, 14.0, style::TEXT),
                    text(tr::foot_pr_rewrite()).size(13)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .style(if reuse_first {
                style::secondary
            } else {
                style::primary
            })
            .on_press_maybe(can.then(|| fm(sid, FooterMsg::PrSubmit(false)))));
            buttons = buttons.pb(button(
                row![
                    ic(FIcon::Refresh, 14.0, style::TEXT),
                    text(if file.matches == LeftoverMatch::Diverged {
                        tr::foot_pr_reuse_anyway()
                    } else {
                        tr::foot_pr_reuse()
                    })
                    .size(13)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .style(if file.matches == LeftoverMatch::Diverged {
                style::danger
            } else if reuse_first {
                style::primary
            } else {
                style::secondary
            })
            .on_press_maybe(
                (can && !file.title.is_empty()).then(|| fm(sid, FooterMsg::PrSubmit(true))),
            ));
        } else {
            let merge = f.merge_now && !f.draft;
            let label = match (f.from_commits, merge) {
                (true, true) => tr::foot_pr_submit_merge(),
                (true, false) => tr::foot_create_pr(),
                (false, true) => tr::foot_pr_submit_ai_merge(),
                (false, false) => tr::foot_pr_submit_ai(),
            };
            let blocked = f.existing.is_some() || (f.from_commits && f.commit_blocked());
            buttons = buttons.pb(button(
                row![
                    ic(
                        if merge {
                            FIcon::GitMerge
                        } else {
                            FIcon::GitPullRequest
                        },
                        14.0,
                        Color::WHITE
                    ),
                    text(label).size(13)
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 14])
            .style(style::primary)
            .on_press_maybe((can && !blocked).then(|| fm(sid, FooterMsg::PrSubmit(false)))));
        }
        c = c.pb(buttons);
        scrollable(c.width(540)).height(Length::Shrink).boxed()
    }

    fn recheck_card<'a>(&'a self, f: &'a RecheckForm) -> El<'a> {
        let sid = f.session.as_str();
        let ch = &f.change;
        let name = self
            .live(sid)
            .and_then(|l| l.info.repos.iter().find(|r| r.repo == f.repo))
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let mut c = column![
            title(tr::foot_pending_title()),
            small(tr::foot_recheck_intro(&name)),
        ]
        .spacing(10);
        if ch.rewritten {
            c = c.pb(warn(tr::foot_recheck_rewritten()));
        }
        let commits_in = f.commits_in();
        if !ch.commits.is_empty() && !ch.rewritten {
            let note = match (commits_in, f.push_off) {
                (true, true) => tr::foot_recheck_commits_on_pushoff(),
                (true, false) => tr::foot_recheck_commits_on(),
                (false, true) => tr::foot_recheck_commits_off_pushoff(),
                (false, false) => tr::foot_recheck_commits_off(),
            };
            let items: Vec<String> = ch
                .commits
                .iter()
                .map(|x| format!("{} {}", x.sha, x.subject))
                .collect();
            c = c.pb(group(
                commits_in,
                true,
                style::ACCENT,
                tr::foot_recheck_commits(i18n::count(ch.commits.len())),
                items,
                note,
                fm(sid, FooterMsg::RecheckCommits(!f.commits_on)),
            ));
        }
        if !ch.files.is_empty() {
            let note = if !commits_in {
                tr::foot_pending_files_off_commits()
            } else if f.files_in() {
                tr::foot_recheck_files_on()
            } else {
                tr::foot_pending_files_off()
            };
            let items: Vec<String> = ch
                .files
                .iter()
                .take(8)
                .map(|x| {
                    format!(
                        "{} {}",
                        if x.status.is_empty() { "M" } else { &x.status },
                        x.path
                    )
                })
                .collect();
            c = c.pb(group(
                f.files_in(),
                commits_in,
                YELLOW,
                tr::foot_pending_files(i18n::count(ch.files.len())),
                items,
                note,
                fm(sid, FooterMsg::RecheckFiles(!f.files_on)),
            ));
        }
        if let Some(m) = f.merge {
            c = c.pb(checkbox(m)
                .label(tr::foot_recheck_merge())
                .on_toggle({
                    let sid = sid.to_owned();
                    move |v| fm(&sid, FooterMsg::RecheckMerge(v))
                })
                .size(14)
                .text_size(12));
        }
        c = c.pb(row![
            space().width(Fill),
            btn(
                tr::foot_later(),
                style::secondary,
                Some(Message::CloseModal)
            ),
            button(text(tr::foot_recheck_cancel()).size(13).color(style::RED))
                .padding([6, 12])
                .style(style::icon)
                .on_press(fm(sid, FooterMsg::RecheckCancel)),
            btn(
                if f.files_in() {
                    tr::foot_recheck_commit_go()
                } else {
                    tr::foot_create_pr()
                },
                style::primary,
                Some(fm(sid, FooterMsg::RecheckResolve))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center));
        c.width(560).boxed()
    }
}

/// Desk `syncHint` / `hint`.
fn sync_hint(
    primary: SyncOp,
    sync: Option<&SyncStatus>,
    open_pr: Option<u64>,
    merged: Option<u64>,
) -> String {
    let ahead = sync.map_or(0, |s| s.ahead);
    let behind = sync.map_or(0, |s| s.behind);
    let upstream = sync.and_then(|s| s.upstream.clone()).unwrap_or_default();
    let gone = sync.is_some_and(|s| s.gone);
    if let Some(n) = merged
        && primary == SyncOp::Fetch
    {
        return if gone {
            tr::foot_hint_merged_gone(&n.to_string())
        } else {
            tr::foot_hint_merged(&n.to_string())
        };
    }
    let a = i18n::count(ahead as usize);
    match primary {
        SyncOp::Publish => match open_pr {
            Some(n) => tr::foot_hint_publish_pr(a, &n.to_string()),
            None if gone => tr::foot_hint_publish_gone(a),
            None => tr::foot_hint_publish(a),
        },
        SyncOp::Pull => tr::foot_hint_pull(i18n::count(behind as usize), a),
        SyncOp::Push => match open_pr {
            Some(n) => tr::foot_hint_push_pr(a, &upstream, &n.to_string()),
            None => tr::foot_hint_push(a, &upstream),
        },
        _ => tr::foot_hint_fetch(),
    }
}

/// A strip above the footer (`.pr-leftover-strip`).
fn strip<'a>(
    c: Color,
    label: String,
    action: String,
    tooltip: String,
    msg: Option<Message>,
) -> El<'a> {
    let b = button(
        row![
            dot(c),
            text(label).size(11).width(Fill),
            text(action)
                .size(11)
                .color(if c == ORANGE { ORANGE } else { VIOLET_TEXT }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([6, 10])
    .width(Fill)
    .style(strip_style(c))
    .on_press_maybe(msg);
    container(tipw(b, tooltip))
        .padding(iced::Padding {
            bottom: 6.0,
            ..Default::default()
        })
        .boxed()
}

/// Desk `PendingGroup`: the whole card toggles it.
fn group<'a>(
    on: bool,
    enabled: bool,
    tone: Color,
    head: String,
    items: Vec<String>,
    note: String,
    msg: Message,
) -> El<'a> {
    let mut list = column![].spacing(2);
    for i in items {
        list = list.pb(text(i)
            .size(11)
            .font(style::MONO)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End));
    }
    let body = column![
        row![
            checkbox(on).size(14),
            dot(tone),
            text(head).size(13).font(style::BOLD),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        container(list).padding([4, 8]).width(Fill).style(band),
        text(note)
            .size(11)
            .color(if on { style::DIM } else { ORANGE }),
    ]
    .spacing(6);
    button(body)
        .padding(10)
        .width(Fill)
        .style(move |_, s| {
            let a = match (enabled, s) {
                (false, _) => 0.03,
                (_, button::Status::Hovered) => 0.12,
                _ => 0.07,
            };
            bstyle(
                Some(alpha(if on { tone } else { style::DIM }, a)),
                if enabled { style::TEXT } else { style::FAINT },
                alpha(
                    if on { tone } else { style::BORDER },
                    if on { 0.45 } else { 1.0 },
                ),
                8.0,
            )
        })
        .on_press_maybe(enabled.then_some(msg))
        .boxed()
}

/// \"PR 올리기 전에 확인해 주세요\" (Desk `PrPendingStep`).
fn pending_card(f: &PrForm) -> El<'_> {
    let sid = f.session.as_str();
    let mut c = column![title(tr::foot_pending_title())].spacing(10);
    let Some(p) = f.pending_work() else {
        return c.pb(small(tr::foot_pending_loading())).width(560).boxed();
    };
    c = c.pb(small(tr::foot_pending_commit_mode_note()));
    c = c.pb(small(tr::foot_pending_intro(&f.repo_name, &p.branch)));
    let commits_on = !f.exclude.commits;
    let files_on = !f.exclude.files && !f.exclude.commits;
    if p.commit_count > 0 {
        let head = if p.published {
            tr::foot_pending_commits(i64::from(p.commit_count))
        } else {
            tr::foot_pending_commits_unpublished(i64::from(p.commit_count))
        };
        let note = if !p.published {
            tr::foot_pending_commits_unpublished_note()
        } else if commits_on {
            tr::foot_pending_commits_on()
        } else {
            tr::foot_pending_commits_off(&p.branch)
        };
        let mut items: Vec<String> = p
            .commits
            .iter()
            .map(|x| format!("{} {}", x.sha, x.subject))
            .collect();
        if p.commit_count as usize > p.commits.len() {
            items.push(tr::foot_more_count(i18n::count(
                p.commit_count as usize - p.commits.len(),
            )));
        }
        c = c.pb(group(
            commits_on,
            p.published,
            style::ACCENT,
            head,
            items,
            note,
            fm(sid, FooterMsg::PrExcludeCommits(!commits_on)),
        ));
    }
    if !p.files.is_empty() {
        let note = if f.exclude.commits {
            tr::foot_pending_files_off_commits()
        } else if files_on {
            tr::foot_pending_files_on()
        } else {
            tr::foot_pending_files_off()
        };
        let mut items: Vec<String> = p
            .files
            .iter()
            .take(8)
            .map(|x| {
                format!(
                    "{} {}",
                    if x.status.is_empty() { "M" } else { &x.status },
                    x.path
                )
            })
            .collect();
        if p.files.len() > 8 {
            items.push(tr::foot_more_count(i18n::count(p.files.len() - 8)));
        }
        c = c.pb(group(
            files_on,
            !f.exclude.commits,
            YELLOW,
            tr::foot_pending_files(i18n::count(p.files.len())),
            items,
            note,
            fm(sid, FooterMsg::PrExcludeFiles(!files_on)),
        ));
    }
    let some_out = (p.commit_count > 0 && !commits_on) || (!p.files.is_empty() && !files_on);
    c.pb(row![
        space().width(Fill),
        btn(tr::cancel(), style::secondary, Some(Message::CloseModal)),
        btn(
            if some_out {
                tr::foot_pending_continue_some()
            } else {
                tr::foot_pending_continue()
            },
            style::primary,
            Some(fm(sid, FooterMsg::PrContinue))
        ),
    ]
    .spacing(8))
        .width(560)
        .boxed()
}

/// Desk `LeftoverCard`.
fn leftover_card<'a>(f: &'a PrForm, file: &'a ide_client::PrLeftoverFile) -> El<'a> {
    let (c, line) = match file.matches {
        LeftoverMatch::Same => (TEAL, tr::foot_left_same()),
        LeftoverMatch::Ahead => (
            ORANGE,
            tr::foot_left_ahead(i18n::count(file.new_commits.len())),
        ),
        LeftoverMatch::Diverged => (style::RED, tr::foot_left_diverged()),
        LeftoverMatch::Unknown => (YELLOW, tr::foot_left_unknown()),
    };
    let when = crate::history::full_date(file.written_at as i64);
    let reason = file.reason.as_ref().map(i18n::proto_error);
    let mut col = column![
        text(tr::foot_left_title())
            .size(13)
            .font(style::BOLD)
            .color(c),
        small(match &reason {
            Some(r) => tr::foot_left_written_reason(&when, r),
            None => tr::foot_left_written(&when),
        }),
        text(line).size(11).font(style::BOLD).color(c),
    ]
    .spacing(6);
    if !file.new_commits.is_empty() {
        let mut l = column![].spacing(2);
        for x in file.new_commits.iter().take(5) {
            l = l.pb(text(format!("{} {}", x.sha, x.subject)).size(11));
        }
        if file.new_commits.len() > 5 {
            l = l.pb(small(tr::foot_more_count(i18n::count(
                file.new_commits.len() - 5,
            ))));
        }
        col = col.pb(container(l).padding([6, 8]).width(Fill).style(band));
    }
    let mut t = row![
        column![
            text(tr::foot_left_pr_title()).size(10).color(style::DIM),
            text(if file.title.is_empty() {
                tr::foot_left_empty()
            } else {
                file.title.clone()
            })
            .size(13)
            .wrapping(text::Wrapping::None)
            .ellipsis(text::Ellipsis::End),
        ]
        .width(Fill)
    ]
    .align_y(Alignment::Center);
    if !file.body.is_empty() {
        t = t.pb(button(
            text(if f.show_left_body {
                tr::foot_left_fold()
            } else {
                tr::foot_left_show()
            })
            .size(11),
        )
        .padding([2, 6])
        .style(link_btn)
        .on_press(fm(&f.session, FooterMsg::PrToggleLeftoverBody)));
    }
    col = col.pb(container(t).padding([6, 8]).width(Fill).style(band));
    if f.show_left_body {
        col = col.pb(container(
            scrollable(text(file.body.as_str()).size(11).font(style::MONO)).height(160),
        )
        .padding([6, 8])
        .width(Fill)
        .style(band));
    }
    container(col)
        .padding(10)
        .width(Fill)
        .style(card_style(c))
        .boxed()
}

/// Desk `MergePrModal`.
fn merge_card(f: &MergeForm) -> El<'_> {
    let sid = f.session.as_str();
    let n = f.pr.number.to_string();
    let short: String = f.pr.head_sha.chars().take(7).collect();
    let mut c = column![
        title(tr::foot_merge_title(&n)),
        column![
            text(f.pr.title.as_str()).size(14).font(style::BOLD),
            text(format!("{} → {}", f.pr.head_ref, f.pr.base_ref))
                .size(12)
                .font(style::MONO)
                .color(style::DIM),
        ]
        .spacing(2),
        column![
            text(tr::foot_merge_method()).size(12).font(style::BOLD),
            pick_list(Some(f.method), METHODS, method_label)
                .on_select({
                    let sid = sid.to_owned();
                    move |m| fm(&sid, FooterMsg::MergeMethod(m))
                })
                .text_size(13)
                .padding([5, 8])
                .width(Fill),
        ]
        .spacing(4),
        checkbox(f.delete_branch)
            .label(tr::foot_merge_delete_after())
            .on_toggle_maybe((!f.busy).then(|| {
                let sid = sid.to_owned();
                move |v| fm(&sid, FooterMsg::MergeDelete(v))
            }))
            .size(16)
            .text_size(13),
        small(tr::foot_merge_guard(&short)),
    ]
    .spacing(12);
    if let Some(e) = &f.error {
        c = c.pb(err_box(e.clone()));
    }
    c.pb(row![
        space().width(Fill),
        btn(
            tr::cancel(),
            style::secondary,
            (!f.busy).then_some(Message::CloseModal)
        ),
        button(
            row![
                ic(FIcon::GitMerge, 16.0, Color::WHITE),
                text(if f.busy {
                    tr::foot_merging()
                } else {
                    tr::foot_merge()
                })
                .size(13)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 14])
        .style(style::primary)
        .on_press_maybe((!f.busy).then(|| fm(sid, FooterMsg::MergeConfirm))),
    ]
    .spacing(8))
        .width(480)
        .boxed()
}

fn close_card(f: &ClosePrForm) -> El<'_> {
    let mut c = column![
        title(tr::foot_close_title(&f.number.to_string())),
        text(f.title.as_str()).size(13).font(style::BOLD),
        text(tr::foot_close_body()).size(13).color(style::DIM),
    ]
    .spacing(12);
    if let Some(e) = &f.error {
        c = c.pb(err_box(e.clone()));
    }
    c.pb(row![
        space().width(Fill),
        btn(
            tr::cancel(),
            style::secondary,
            (!f.busy).then_some(Message::CloseModal)
        ),
        btn(
            tr::foot_close_confirm(),
            style::danger,
            (!f.busy).then(|| fm(&f.session, FooterMsg::ClosePrConfirm))
        ),
    ]
    .spacing(8))
        .width(440)
        .boxed()
}

/// Desk `BranchRefreshModal` + `BranchRefreshForm`.
fn continue_card(f: &ContinueForm) -> El<'_> {
    let sid = f.session.as_str();
    let mut c = column![
        row![
            ic(FIcon::GitBranch, 18.0, style::TEXT),
            title(tr::foot_continue())
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    ]
    .spacing(10);
    if let Some(lines) = &f.result {
        let mut l = column![].spacing(3);
        for (i, x) in lines.iter().enumerate() {
            l = l.pb(row![
                text(if i == 0 { "✓" } else { " " })
                    .size(12)
                    .color(style::GREEN)
                    .width(14),
                text(x.as_str())
                    .size(12)
                    .color(if i == 0 { style::GREEN } else { style::TEXT }),
            ]
            .spacing(6));
        }
        return c
            .pb(l)
            .pb(row![
                space().width(Fill),
                btn(tr::close(), style::primary, Some(Message::CloseModal))
            ])
            .width(560)
            .boxed();
    }
    let info = match &f.info {
        Load::Loading => return c.pb(small(tr::foot_continue_loading())).width(560).boxed(),
        Load::Failed(e) => {
            return c
                .pb(err_box(tr::foot_continue_load_failed(e)))
                .pb(row![
                    space().width(Fill),
                    btn(tr::close(), style::secondary, Some(Message::CloseModal))
                ])
                .width(560)
                .boxed();
        }
        Load::Ready(i) => i,
    };
    c = c.pb(small(tr::foot_continue_intro(&info.base, &info.branch)));
    if info.fetch_failed {
        c = c.pb(text(tr::foot_continue_fetch_failed(&info.base))
            .size(11)
            .color(ORANGE));
    }
    let name_ok = crate::footer::branch_name_ok(&f.name);
    let mut name_col = column![
        row![
            text(tr::foot_continue_name()).size(12).font(style::BOLD),
            small(tr::foot_continue_name_placeholder(&info.suggested)),
        ]
        .spacing(8),
        text_input(&info.suggested, &f.name)
            .id(CONTINUE_NAME_ID)
            .on_input_maybe((!f.running).then(|| {
                let sid = sid.to_owned();
                move |t| fm(&sid, FooterMsg::ContinueName(t))
            }))
            .on_submit(fm(sid, FooterMsg::ContinueRun))
            .font(style::MONO)
            .size(12)
            .padding([5, 8])
            .style(style::input),
    ]
    .spacing(4);
    if !name_ok {
        name_col = name_col.pb(text(tr::foot_continue_bad_name())
            .size(11)
            .color(style::RED));
    }
    c = c.pb(name_col);
    let conflict = f
        .check
        .as_ref()
        .and_then(|x| x.ready())
        .and_then(|x| x.conflict.as_ref());
    let skipped: Vec<String> = f
        .check
        .as_ref()
        .and_then(|x| x.ready())
        .map(|x| x.skipped.clone())
        .unwrap_or_default();
    let all = !info.commits.is_empty() && f.picked.len() == info.commits.len();
    let mut head = row![
        text(tr::foot_continue_commits()).size(12).font(style::BOLD),
        small(format!("{}/{}", f.picked.len(), info.commits.len())),
        space().width(Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if info.commits.len() > 1 {
        head = head.pb(button(
            text(if all {
                tr::foot_continue_none()
            } else {
                tr::foot_continue_all()
            })
            .size(11),
        )
        .padding([2, 6])
        .style(link_btn)
        .on_press_maybe((!f.running).then(|| fm(sid, FooterMsg::ContinueAll))));
    }
    let mut commits = column![head].spacing(4);
    if info.commits.is_empty() {
        commits = commits.pb(small(tr::foot_continue_no_commits(&info.base)));
    } else {
        let mut list = column![].spacing(1);
        for cm in &info.commits {
            let bad = conflict.is_some_and(|x| x.sha == cm.full);
            let mut r = row![
                checkbox(f.picked.contains(&cm.full)).size(14),
                text(cm.sha.chars().take(7).collect::<String>())
                    .size(11)
                    .font(style::MONO)
                    .color(style::DIM),
                text(cm.subject.as_str())
                    .size(12)
                    .width(Fill)
                    .wrapping(text::Wrapping::None)
                    .ellipsis(text::Ellipsis::End),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if skipped.contains(&cm.full) {
                r = r.pb(small(tr::foot_continue_no_change()));
            } else if cm.in_base {
                r = r.pb(small(tr::foot_continue_in_base(&info.base)));
            }
            list = list.pb(button(r)
                .padding([4, 6])
                .width(Fill)
                .style(move |t, s| {
                    let mut b = menu_item(t, s);
                    if bad {
                        b.background = Some(alpha(style::RED, 0.12).into());
                    }
                    b
                })
                .on_press_maybe(
                    (!f.running).then(|| fm(sid, FooterMsg::ContinuePick(cm.full.clone()))),
                ));
        }
        commits = commits.pb(container(
            scrollable(list).height(Length::Fixed((info.commits.len() as f32 * 26.0).min(240.0))),
        )
        .width(Fill)
        .style(band));
    }
    if info.more_commits > 0 {
        commits = commits.pb(small(tr::foot_continue_more(i64::from(info.more_commits))));
    }
    commits = commits.pb(small(tr::foot_continue_order()));
    c = c.pb(commits);
    if let Some(x) = conflict {
        let short: String = x.sha.chars().take(7).collect();
        let more = x.files.len().saturating_sub(3);
        let files = x
            .files
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        c = c.pb(err_box(if x.files.is_empty() {
            tr::foot_continue_conflict(&short, &x.subject)
        } else {
            tr::foot_continue_conflict_files(i18n::count(more), &short, &x.subject, &files)
        }));
    }
    if let Some(Load::Failed(e)) = &f.check {
        c = c.pb(text(tr::foot_continue_check_failed(e))
            .size(11)
            .color(style::RED));
    }
    if !info.files.is_empty() {
        let fc = f
            .check
            .as_ref()
            .and_then(|x| x.ready())
            .map_or(0, |x| x.files_conflict.len());
        let names = info
            .files
            .iter()
            .take(6)
            .map(|x| x.path.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let more = info.files.len().saturating_sub(6);
        let note = if !f.move_files {
            tr::foot_continue_files_off(&info.branch)
        } else if fc > 0 {
            tr::foot_continue_files_conflict(i18n::count(fc), &info.branch)
        } else if more > 0 {
            tr::foot_continue_files_list_more(&names, i18n::count(more))
        } else {
            names
        };
        c = c.pb(column![
            checkbox(f.move_files)
                .label(tr::foot_continue_files(i18n::count(info.files.len())))
                .on_toggle_maybe((!f.running).then(|| {
                    let sid = sid.to_owned();
                    move |v| fm(&sid, FooterMsg::ContinueMoveFiles(v))
                }))
                .size(14)
                .text_size(12),
            container(small(note)).padding(iced::Padding {
                left: 22.0,
                ..Default::default()
            }),
        ]
        .spacing(2));
    }
    if let Some((n, t)) = &f.open_pr {
        c = c.pb(column![
            checkbox(f.close_pr)
                .label(tr::foot_continue_close_pr(&n.to_string()))
                .on_toggle_maybe((!f.running).then(|| {
                    let sid = sid.to_owned();
                    move |v| fm(&sid, FooterMsg::ContinueClosePr(v))
                }))
                .size(14)
                .text_size(12),
            container(small(tr::foot_continue_close_pr_note(t))).padding(iced::Padding {
                left: 22.0,
                ..Default::default()
            }),
        ]
        .spacing(2));
    }
    c.pb(row![
        space().width(Fill),
        btn(
            tr::cancel(),
            style::secondary,
            (!f.running).then_some(Message::CloseModal)
        ),
        button(
            row![
                ic(FIcon::GitBranch, 16.0, Color::WHITE),
                text(if f.running {
                    tr::foot_continue_running()
                } else {
                    tr::foot_continue_run()
                })
                .size(13)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6, 14])
        .style(style::primary)
        .on_press_maybe(f.can_run().then(|| fm(sid, FooterMsg::ContinueRun))),
    ]
    .spacing(8))
        .width(560)
        .boxed()
}
