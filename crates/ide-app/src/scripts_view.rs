//! The Changes panel's 스크립트 section (Hermes Desk's `ScriptsSection`): every script file of the
//! picked repo's worktree (`scripts.list`) as a tree, tech stack → file → scripts, VS Code
//! "NPM Scripts" style; ▶ (or a double click) runs one in a user terminal in its file's folder
//! (`uterm.rs`), a running one opens its terminal instead. Docked under the Changes panel with
//! a draggable top edge; folded or not and its height are kept in `ui.json` (Desk's
//! `scripts-collapsed` / `scripts-height`).

use std::collections::HashMap;

use iced::widget::{
    Column, Row, button, column, container, hover, mouse_area, operation, row, scrollable, space,
    text, text_input,
};
use iced::{Alignment, Border, Element, Fill, Length, Padding, Task, Widget as _, mouse};
use ide_client::{GitStatus, ProtoError, ScriptGroup, ScriptItem, SessionState};

use crate::app::{App, Live, Message};
use crate::detail::{DetailMsg, ItemKey};
use crate::i18n::{self, tr};
use crate::icons::{Icon, icon};
use crate::style;
use crate::uterm::UTermMsg;
use crate::view::tip;

type El<'a> = Element<'a, Message>;
/// A tech stack shown: its order, label, icon file name, and its files with their scripts.
type Stack<'g> = (usize, &'g str, &'g str, Vec<(&'g ScriptGroup, Vec<&'g ScriptItem>)>);

/// Height of the open section (Desk: 240, dragged within 80..640).
pub const DEFAULT_H: f32 = 240.0;
const MIN_H: f32 = 80.0;
const MAX_H: f32 = 640.0;
/// Width the ▶ button takes at a script row's end (kept while hidden, like Desk's
/// `visibility: hidden`).
const RUN_W: f32 = 24.0;
const SEARCH_ID: &str = "ide-scripts-search";

/// Tech stack of each script file kind (Desk's `STACKS`): label and a file name whose icon
/// represents it, in Desk's order.
const STACKS: [(&str, &str, &str); 14] = [
    ("npm", "Node.js", "package.json"),
    ("deno", "Deno", "deno.json"),
    ("dotnet", ".NET", "a.csproj"),
    ("python", "Python", "pyproject.toml"),
    ("go", "Go", "go.mod"),
    ("cargo", "Rust", "Cargo.toml"),
    ("gradle", "Gradle", "build.gradle"),
    ("maven", "Maven", "pom.xml"),
    ("composer", "PHP", "composer.json"),
    ("rake", "Ruby", "Rakefile"),
    ("make", "Make", "Makefile"),
    ("just", "just", "justfile"),
    ("task", "Task", "Taskfile.yml"),
    ("compose", "Docker Compose", "docker-compose.yml"),
];

/// The section's state (per session).
#[derive(Default)]
pub struct ScriptsUi {
    /// The repo the list is of, and the list (`Err`: why it could not be read).
    pub list: Option<(String, Result<Vec<ScriptGroup>, String>)>,
    pub loading: bool,
    /// The search box is shown (Desk's search toggle) and its text.
    pub searching: bool,
    pub query: String,
    /// Open / closed stacks (`stack:<label>`) and files (their path) per repo; unset = the
    /// default (stacks and root files open).
    pub open: HashMap<String, HashMap<String, bool>>,
    /// The changed script inputs (Desk's `scriptChangeKey`) of the repo's last status, and what
    /// the list was loaded for: a difference lists the scripts again.
    pub input_key: String,
    pub loaded_key: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum ScriptsMsg {
    /// Fold / unfold the section (every session: `ui.json`).
    ToggleSection,
    ToggleSearch,
    CloseSearch,
    Query(String),
    Refresh,
    Loaded(String, String, Result<Vec<ScriptGroup>, ProtoError>),
    /// Set the open state of stacks / files.
    SetOpen(Vec<(String, bool)>),
    /// Run a script of a file (its folder) in a new terminal, or open its running terminal.
    Run { folder: String, name: String, command: String },
    /// The top edge is pressed (height drag).
    ResizePressed,
}

fn sm(sid: &str, m: ScriptsMsg) -> Message {
    Message::Detail(sid.to_owned(), DetailMsg::Scripts(m))
}

/// Desk's `SCRIPT_INPUTS`: files whose change changes what scripts there are (lock files and
/// wrappers pick the package manager / Gradle command).
fn is_script_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(
        name.as_str(),
        "package.json"
            | "composer.json"
            | "deno.json"
            | "deno.jsonc"
            | "makefile"
            | "gnumakefile"
            | "justfile"
            | ".justfile"
            | "taskfile.yml"
            | "taskfile.yaml"
            | "rakefile"
            | "pyproject.toml"
            | "cargo.toml"
            | "go.mod"
            | "build.gradle"
            | "build.gradle.kts"
            | "gradlew"
            | "gradlew.bat"
            | "pom.xml"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | "compose.yml"
            | "compose.yaml"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lockb"
            | "bun.lock"
    ) || [".csproj", ".fsproj", ".vbproj", ".sln"]
        .iter()
        .any(|e| name.ends_with(e))
}

/// Desk's `scriptChangeKey`: the changed script inputs of a status, sorted.
pub fn input_key(s: &GitStatus) -> String {
    let mut keys: Vec<String> = s
        .staged
        .iter()
        .chain(&s.unstaged)
        .chain(&s.conflicted)
        .filter(|c| is_script_input(&c.path) || c.orig_path.as_deref().is_some_and(is_script_input))
        .map(|c| format!("{}|{:?}|{}", c.path, c.kind, c.orig_path.as_deref().unwrap_or("")))
        .chain(s.untracked.iter().filter(|p| is_script_input(p)).map(|p| format!("{p}|new|")))
        .collect();
    keys.sort();
    keys.join("\n")
}

/// The stack (label, icon file name) of a script file kind.
fn stack_of(kind: &str) -> (usize, &'static str, &'static str) {
    STACKS
        .iter()
        .enumerate()
        .find(|(_, (k, _, _))| *k == kind)
        .map_or((STACKS.len(), "", ""), |(i, (_, l, f))| (i, *l, *f))
}

/// Server paths compared loosely (separators, trailing ones, case).
fn same_dir(a: &str, b: &str) -> bool {
    let n = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    n(a) == n(b)
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
        let (s, repo, key) = (sid.to_owned(), d.repo.clone(), d.scripts.input_key.clone());
        Task::perform(
            {
                let (s, repo) = (s.clone(), repo.clone());
                async move { c.scripts_list(&s, &repo).await }
            },
            move |r| sm(&s, ScriptsMsg::Loaded(repo.clone(), key.clone(), r)),
        )
    }

    /// List the picked repo's scripts again when they are of another repo, or its script
    /// files changed since (Desk revalidates on the repository's changes).
    pub(crate) fn scripts_follow_repo(&mut self, sid: &str) -> Task<Message> {
        let stale = self.detail(sid).is_some_and(|d| {
            let s = &d.scripts;
            !d.repo.is_empty()
                && !s.loading
                && s.loaded_key.as_ref().is_none_or(|(r, k)| *r != d.repo || *k != s.input_key)
        });
        if stale {
            self.load_scripts(sid)
        } else {
            Task::none()
        }
    }

    pub(crate) fn on_scripts(&mut self, sid: &str, m: ScriptsMsg) -> Task<Message> {
        if let ScriptsMsg::ToggleSection = m {
            self.ui.scripts_collapsed = !self.ui.scripts_collapsed;
            self.save_ui();
            return Task::none();
        }
        if let ScriptsMsg::ResizePressed = m {
            self.scripts_drag = Some((None, self.ui.scripts_height));
            return Task::none();
        }
        let Some(d) = self.detail_mut(sid) else {
            return Task::none();
        };
        let repo = d.repo.clone();
        let s = &mut d.scripts;
        match m {
            ScriptsMsg::ToggleSection | ScriptsMsg::ResizePressed => Task::none(),
            ScriptsMsg::ToggleSearch => {
                s.searching = !s.searching;
                s.query.clear();
                if s.searching {
                    operation::focus(SEARCH_ID)
                } else {
                    Task::none()
                }
            }
            ScriptsMsg::CloseSearch => {
                s.searching = false;
                s.query.clear();
                Task::none()
            }
            ScriptsMsg::Query(q) => {
                s.query = q;
                Task::none()
            }
            ScriptsMsg::Refresh => self.load_scripts(sid),
            ScriptsMsg::Loaded(repo, key, r) => {
                s.loading = false;
                s.list = Some((repo.clone(), r.map_err(|e| i18n::proto_error(&e))));
                s.loaded_key = Some((repo, key));
                // The repo or its script files changed while listing.
                self.scripts_follow_repo(sid)
            }
            ScriptsMsg::SetOpen(entries) => {
                let map = s.open.entry(repo).or_default();
                for (k, v) in entries {
                    map.insert(k, v);
                }
                Task::none()
            }
            ScriptsMsg::Run {
                folder,
                name,
                command,
            } => {
                // Scripts run in a terminal of the shown session (the folder's other sessions
                // keep theirs).
                let tsid = self.term_session(sid);
                let live = self.live(&tsid);
                if let Some(id) = live.and_then(|l| running(l, &repo, &folder, &command)) {
                    return self.on_detail(sid.to_owned(), DetailMsg::Activate(ItemKey::Terminal(id)));
                }
                let Some(c) = self.online() else {
                    return Task::none();
                };
                // Desk: "<folder> · <script>" for a script of a sub-folder.
                let title = match folder.rsplit('/').next() {
                    Some(f) if !f.is_empty() => format!("{f} · {name}"),
                    _ => name,
                };
                let sid = tsid;
                Task::perform(
                    {
                        let sid = sid.clone();
                        async move {
                            c.uterm_open(ide_client::UTermOpen {
                                session: sid.clone(),
                                repo: Some(repo),
                                folder: Some(folder),
                                command: Some(command),
                                title: Some(title),
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

    /// The section's height while its top edge is dragged (`y`: the pointer).
    pub(crate) fn scripts_drag_to(&mut self, y: f32) {
        let Some((start, h)) = self.scripts_drag else {
            return;
        };
        match start {
            None => self.scripts_drag = Some((Some(y), h)),
            Some(y0) => self.ui.scripts_height = (h + y0 - y).round().clamp(MIN_H, MAX_H),
        }
    }

    /// The section under the Changes list: its top edge, header, and when open the tree.
    pub fn scripts_section<'a>(&'a self, live: &'a Live) -> El<'a> {
        let d = &live.detail;
        let s = &d.scripts;
        let sid = live.info.id.as_str();
        let collapsed = self.ui.scripts_collapsed;
        let groups: Option<&[ScriptGroup]> = match &s.list {
            Some((r, Ok(g))) if *r == d.repo => Some(g),
            _ => None,
        };
        let error = match &s.list {
            Some((r, Err(e))) if *r == d.repo => Some(e.as_str()),
            _ => None,
        };
        let total: usize = groups.map_or(0, |g| g.iter().map(|g| g.scripts.len()).sum());

        // Header: fold toggle with the title and count, then the tools (open only).
        let mut title = Row::<El<'a>>::new()
            .push(icon(
                if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                14.0,
                style::DIM,
            ))
            .push(text(tr::scripts_title()).size(11).font(style::bold()).color(style::DIM).boxed())
        .spacing(6)
        .align_y(Alignment::Center);
        if total > 0 {
            title = title.push(count_badge(total));
        }
        let mut head = Row::<El<'a>>::new()
            .spacing(2)
            .align_y(Alignment::Center)
            .padding(Padding {
                top: 4.0,
                right: 6.0,
                bottom: 4.0,
                left: 4.0,
            })
            .push(
                button(title)
                    .padding([2, 4])
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(sm(sid, ScriptsMsg::ToggleSection))
                    .boxed(),
            );
        let q = s.query.trim().to_lowercase();
        let shown: Vec<(&ScriptGroup, Vec<&ScriptItem>)> = groups
            .unwrap_or(&[])
            .iter()
            .filter_map(|g| {
                let hits: Vec<&ScriptItem> = g
                    .scripts
                    .iter()
                    .filter(|x| {
                        q.is_empty()
                            || x.name.to_lowercase().contains(&q)
                            || g.file.to_lowercase().contains(&q)
                            || x.detail.as_deref().unwrap_or("").to_lowercase().contains(&q)
                    })
                    .collect();
                (!hits.is_empty()).then_some((g, hits))
            })
            .collect();
        // Tech stacks in Desk's order, each with its files.
        let mut stacks: Vec<Stack> = Vec::new();
        for (g, hits) in shown {
            let (order, label, file) = stack_of(&g.kind);
            let label = if label.is_empty() { g.kind.as_str() } else { label };
            match stacks.iter_mut().find(|st| st.1 == label) {
                Some(st) => st.3.push((g, hits)),
                None => stacks.push((order, label, file, vec![(g, hits)])),
            }
        }
        stacks.sort_by_key(|st| st.0);
        let open_map = s.open.get(&d.repo);
        let is_open = |key: &str, default: bool| -> bool {
            !q.is_empty() || open_map.and_then(|m| m.get(key)).copied().unwrap_or(default)
        };
        // Expand / collapse stacks and every file in them.
        let fold_all = |list: &[&Stack], open: bool| {
            let mut e = Vec::new();
            for st in list {
                e.push((format!("stack:{}", st.1), open));
                e.extend(st.3.iter().map(|(g, _)| (g.file.clone(), open)));
            }
            sm(sid, ScriptsMsg::SetOpen(e))
        };
        if !collapsed {
            let all: Vec<_> = stacks.iter().collect();
            let any = !all.is_empty();
            head = head
                .push(tool(
                    Icon::Search,
                    tr::scripts_search_toggle(),
                    Some(sm(sid, ScriptsMsg::ToggleSearch)),
                    s.searching,
                ))
                .push(tool(
                    Icon::ChevronsDown,
                    tr::scripts_expand_all(),
                    any.then(|| fold_all(&all, true)),
                    false,
                ))
                .push(tool(
                    Icon::ChevronsUp,
                    tr::scripts_collapse_all(),
                    any.then(|| fold_all(&all, false)),
                    false,
                ))
                .push(tool(
                    Icon::Refresh,
                    tr::scripts_refresh(),
                    (!s.loading).then(|| sm(sid, ScriptsMsg::Refresh)),
                    false,
                ));
        }

        let edge = self.scripts_edge(sid, collapsed);
        if collapsed {
            return column![edge, head.boxed()].boxed();
        }

        // The tree.
        let repo_path = live
            .info
            .repos
            .iter()
            .find(|r| r.repo == d.repo)
            .map(|r| r.path.as_str())
            .unwrap_or("");
        let mut list = Column::<El<'a>>::new();
        let group_rows = |list: Column<El<'a>>, g: &'a ScriptGroup, hits: &[&'a ScriptItem], level: f32| -> Column<El<'a>> {
            let mut list = list;
            let open = is_open(&g.file, g.folder.is_empty());
            let name = g.file.rsplit('/').next().unwrap_or(&g.file).to_owned();
            let mut r = Row::<El<'a>>::new()
                .push(icon(
                    if open { Icon::ChevronDown } else { Icon::ChevronRight },
                    12.0,
                    style::DIM,
                ))
                .push(ide_views::icons::file_icon(&name))
                .push(text(name).size(12).font(style::bold()).wrapping(text::Wrapping::None).boxed())
            .spacing(6)
            .align_y(Alignment::Center);
            if !g.folder.is_empty() {
                r = r.push(
                    text(g.folder.clone())
                        .size(12)
                        .color(style::DIM)
                        .wrapping(text::Wrapping::None)
                        .ellipsis(text::Ellipsis::Start)
                        .width(Fill)
                        .boxed(),
                );
            }
            list = list.push(tip(
                button(r)
                    .padding(Padding {
                        top: 3.0,
                        right: 8.0,
                        bottom: 3.0,
                        left: 8.0 + level * 14.0,
                    })
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(sm(sid, ScriptsMsg::SetOpen(vec![(g.file.clone(), !open)])))
                    .boxed(),
                g.file.clone(),
            ));
            if open {
                for x in hits {
                    let cwd = if g.folder.is_empty() {
                        repo_path.to_owned()
                    } else {
                        format!("{}/{}", repo_path.trim_end_matches(['/', '\\']), g.folder)
                    };
                    let term = running_at(self.term_live(live), &cwd, &x.run);
                    list = list.push(script_row(sid, g, x, term, level));
                }
            }
            list
        };
        if let Some(e) = error {
            list = list.push(
                container(text(e.to_owned()).size(12).color(style::RED)).padding([4, 12]).boxed(),
            );
        } else if groups.is_some() && stacks.is_empty() {
            let note = if q.is_empty() { tr::scripts_none() } else { tr::scripts_no_match() };
            list = list.push(container(text(note).size(12).color(style::DIM)).padding([6, 12]).boxed());
        } else if stacks.len() == 1 {
            for (g, hits) in &stacks[0].3 {
                list = group_rows(list, g, hits, 0.0);
            }
        } else {
            for st in &stacks {
                let key = format!("stack:{}", st.1);
                let open = is_open(&key, true);
                let count = st.3.len();
                let mut r = Row::<El<'a>>::new()
                    .push(icon(
                        if open { Icon::ChevronDown } else { Icon::ChevronRight },
                        12.0,
                        style::DIM,
                    ))
                    .push(ide_views::icons::file_icon(st.2))
                    .push(text(st.1.to_owned()).size(12).font(style::bold()).wrapping(text::Wrapping::None).boxed())
                .spacing(6)
                .align_y(Alignment::Center);
                r = r.push(
                    text(if count > 1 { tr::scripts_stack_files(count as i64) } else { String::new() })
                        .size(12)
                        .color(style::DIM)
                        .width(Fill)
                        .boxed(),
                );
                let base = button(r)
                    .padding([3, 8])
                    .width(Fill)
                    .style(style::row(false))
                    .on_press(sm(sid, ScriptsMsg::SetOpen(vec![(key.clone(), !open)])))
                    .boxed();
                let collapse_files: Vec<(String, bool)> =
                    st.3.iter().map(|(g, _)| (g.file.clone(), false)).collect();
                list = list.push(crate::changes_view::row_actions(
                    base,
                    vec![
                        tool(
                            Icon::ChevronsDown,
                            tr::scripts_stack_expand(st.1),
                            Some(fold_all(&[st], true)),
                            false,
                        ),
                        tool(
                            Icon::ChevronsUp,
                            tr::scripts_stack_collapse(st.1),
                            Some(sm(sid, ScriptsMsg::SetOpen(collapse_files))),
                            false,
                        ),
                    ],
                    6.0,
                ));
                if open {
                    for (g, hits) in &st.3 {
                        list = group_rows(list, g, hits, 1.0);
                    }
                }
            }
        }

        let mut col = Column::<El<'a>>::new().push(edge).push(head.boxed());
        if s.searching {
            let input = text_input(tr::scripts_search(), &s.query)
                .id(SEARCH_ID)
                .on_input({
                    let sid = sid.to_owned();
                    move |v| sm(&sid, ScriptsMsg::Query(v))
                })
                .size(12)
                .padding([3, 8])
                .style(style::input);
            let sid2 = sid.to_owned();
            let input = ide_views::Interact::new(input.boxed())
                .focus_within()
                .on_key(move |k, _| {
                    matches!(k, iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape))
                        .then(|| sm(&sid2, ScriptsMsg::CloseSearch))
                });
            col = col.push(
                container(El::from(input))
                    .padding(Padding {
                        top: 0.0,
                        right: 10.0,
                        bottom: 4.0,
                        left: 10.0,
                    })
                    .boxed(),
            );
        }
        col.push(scrollable(list).height(Fill).boxed())
            .height(Length::Fixed(self.ui.scripts_height.clamp(MIN_H, MAX_H)))
            .boxed()
    }

    /// The section's top border; while open it drags the height (Desk's `scripts-resizer`).
    fn scripts_edge(&self, sid: &str, collapsed: bool) -> El<'_> {
        let line = |c| container(space()).height(1).width(Fill).style(style::bg(c));
        if collapsed {
            return line(style::BORDER).boxed();
        }
        let color = if self.scripts_drag.is_some() { style::ACCENT } else { style::BORDER };
        mouse_area(container(line(color)).center_y(5).width(Fill))
            .on_press(sm(sid, ScriptsMsg::ResizePressed))
            .interaction(mouse::Interaction::ResizingVertically)
            .boxed()
    }
}

/// The running terminal of a script: in the file's folder, running its command line.
fn running_at(live: &Live, cwd: &str, command: &str) -> Option<String> {
    live.uterms
        .iter()
        .find(|t| {
            t.info.command == command
                && t.info.state == SessionState::Running
                && same_dir(&t.info.cwd, cwd)
        })
        .map(|t| t.info.id.clone())
}

fn running(live: &Live, repo: &str, folder: &str, command: &str) -> Option<String> {
    let path = live.info.repos.iter().find(|r| r.repo == repo)?.path.as_str();
    let cwd = if folder.is_empty() {
        path.to_owned()
    } else {
        format!("{}/{folder}", path.trim_end_matches(['/', '\\']))
    };
    running_at(live, &cwd, command)
}

/// A script row: bullet (a pulsing dot while running), name, what it does, ▶ (shown on hover
/// or while running). A double click runs it; a click opens its running terminal.
fn script_row<'a>(
    sid: &str,
    g: &ScriptGroup,
    x: &ScriptItem,
    term: Option<String>,
    level: f32,
) -> El<'a> {
    let run_msg = sm(
        sid,
        ScriptsMsg::Run {
            folder: g.folder.clone(),
            name: x.name.clone(),
            command: x.run.clone(),
        },
    );
    let live = term.is_some();
    let open_msg = term.map(|id| {
        Message::Detail(sid.to_owned(), DetailMsg::Activate(ItemKey::Terminal(id)))
    });
    let tooltip = match &x.detail {
        Some(d) => format!("{}\n{d}", x.run),
        None => x.run.clone(),
    };
    let layer = |hovered: bool| -> El<'a> {
        let mark: El<'a> = if live {
            container(space())
                .width(8)
                .height(8)
                .style(style::dot(style::TEAL))
                .boxed()
        } else {
            container(space())
                .width(8)
                .height(8)
                .style(|_| container::Style {
                    border: Border {
                        color: style::FAINT,
                        width: 1.0,
                        radius: 100.0.into(),
                    },
                    ..Default::default()
                })
                .boxed()
        };
        let run: El<'a> = if live || hovered {
            tip(
                button(icon(
                    Icon::PlayerPlayFilled,
                    12.0,
                    if live { style::TEAL } else { style::DIM },
                ))
                .padding(4)
                .width(RUN_W)
                .style(style::icon)
                .on_press(run_msg.clone())
                .boxed(),
                if live { tr::scripts_running() } else { tr::scripts_run() },
            )
        } else {
            space().width(RUN_W).boxed()
        };
        let body = row![
            mark,
            text(x.name.clone()).size(12).wrapping(text::Wrapping::None).boxed(),
            text(x.detail.clone().unwrap_or_default())
                .size(12)
                .color(style::DIM)
                .wrapping(text::Wrapping::None)
                .ellipsis(text::Ellipsis::End)
                .width(Fill)
                .boxed(),
            run,
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let c = container(body)
            .padding(Padding {
                top: 1.0,
                right: 6.0,
                bottom: 1.0,
                left: 28.0 + level * 14.0,
            })
            .width(Fill);
        let c = if hovered { c.style(style::bg(style::HOVER)) } else { c };
        let mut m = mouse_area(c).on_double_click(run_msg.clone());
        if let Some(open) = &open_msg {
            m = m.on_press(open.clone());
        }
        m.boxed()
    };
    tip(hover(layer(false), layer(true)).boxed(), tooltip)
}

/// A header tool button (Desk's `ActionIcon size="sm" variant="subtle"`).
fn tool<'a>(i: Icon, label: String, msg: Option<Message>, on: bool) -> El<'a> {
    let color = if msg.is_some() { style::DIM } else { style::FAINT };
    tip(
        button(icon(i, 13.0, color))
            .padding(4)
            .style(style::icon_toggle(on))
            .on_press_maybe(msg)
            .boxed(),
        label,
    )
}

/// Desk's `count-badge`: the number of scripts in a filled grey pill.
fn count_badge<'a>(n: usize) -> El<'a> {
    container(text(n.to_string()).size(10).color(style::TEXT))
        .padding([0, 5])
        .style(|_| container::Style {
            background: Some(style::BORDER.into()),
            border: Border {
                radius: 100.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .boxed()
}
