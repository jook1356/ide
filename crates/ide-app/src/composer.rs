//! The input box under an agent's terminal (Hermes Desk's `Composer.tsx`, for CLI agents):
//! a text editor (Korean IME, multi-line editing, selection, copy / paste) whose ↑ pastes the
//! text into the terminal and presses Enter.
//!
//! - Enter sends, Shift+Enter breaks the line (Desk); an Enter the IME uses to commit a
//!   composition never sends (the key arrives as `Process` then).
//! - The text goes in as one bracketed paste when the program asked for it (Claude Code,
//!   codex: a multi-line prompt stays one prompt), then Enter a moment later, so the program
//!   has taken the paste before the Enter arrives.
//! - Per session: the draft (kept while switching sessions) and the sent history (↑ on the
//!   first line / ↓ on the last line walk it, as in a shell).
//! - Attachments are chips in the text (Desk's TipTap nodes): a short label between two
//!   invisible marks (U+2068 … U+2069) that the editor draws as a pill; the label's path is
//!   kept beside the text. Cursor moves and Backspace / Delete take a chip as one piece. Sent,
//!   a chip becomes its path, quoted when it has spaces (as terminals paste dropped files):
//!   every agent understands a path, and Claude Code turns image paths into `[Image #n]`.
//! - The box grows with its text to about 10 lines, then scrolls (`ide_views::CodeEditor`'s
//!   plain look); Desk's round ↑ sits inside it, bottom right.
//! - Shown under agent sessions while `ui.composer_open` (the header's 입력창 toggle); typing in
//!   the terminal itself still works.

use std::sync::Arc;
use std::time::Duration;

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{self, Binding, Cursor, Edit, KeyPress, Motion};
use iced::widget::{button, container, operation, row, text};
use iced::{Alignment, Element, Fill, Task, Widget as _};
use ide_views::editor::{CodeEditor, Content, Match};

use crate::app::{App, Message};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::style;
use crate::view::tip;

type El<'a> = Element<'a, Message>;

/// Widget id of the input box (focused when it opens and after a send).
pub const COMPOSER_ID: &str = "ide-composer";
/// One line of 13 px text: room for a chip's pill around descenders (g, y, p).
const LINE_H: f32 = 20.0;
/// The round send button, and its gap to the box's edges (the same above it on one line).
const SEND: f32 = 30.0;
const GAP: f32 = 8.0;
/// Text padding: one line stays just under the button plus its two gaps (a text column
/// taller than the button's would make the gap above the button the larger one).
const PAD_Y: f32 = 12.5;
/// Height the text grows to before it scrolls (about 10 lines).
const MAX_H: f32 = 10.0 * LINE_H + 2.0 * PAD_Y;
const RADIUS: f32 = 14.0;
/// Sent prompts remembered per session.
const HISTORY: usize = 100;
/// Between the paste and its Enter: long enough for the program to finish the paste.
const ENTER_AFTER: Duration = Duration::from_millis(80);
/// Marks around a chip in the text.
const OPEN: char = '\u{2068}';
const CLOSE: char = '\u{2069}';
/// A chip's room in the text: no-break spaces (a chip never wraps across lines) as wide as
/// its label at [`CHIP_SIZE`], which the editor draws over the pill; one more on each side.
const PAD: char = '\u{00A0}';
/// The slot of a chip's × button: a no-break figure space the editor draws the × over
/// (centred; the `×` character sits low in the font).
const CLOSE_MARK: char = '\u{2007}';
/// Fills a chip's label room: the blank braille cell (drawn empty, not white space, so the
/// chip stays one word for the line wrap).
const FILL: char = '\u{2800}';
/// A chip's id right after its open mark: hex digits as variation selectors (U+FE00 +
/// digit; invisible, no width).
const ID_BASE: u32 = 0xFE00;
/// Label size inside a chip (the text around it is 13 px).
const CHIP_SIZE: f32 = 11.5;
const TEXT_SIZE: f32 = 13.0;

/// An attachment chip: the label drawn in its pill and the path it stands for.
#[derive(Debug, Clone)]
struct Chip {
    id: u32,
    label: String,
    path: String,
}

/// Width of `s` in the app's font at `size`.
fn text_width(s: &str, size: f32) -> f32 {
    use iced::advanced::text::{self as t, Paragraph as _};
    iced::advanced::graphics::text::Paragraph::with_text(t::Text {
        content: s,
        bounds: iced::Size::INFINITE,
        size: iced::Pixels(size),
        line_height: t::LineHeight::default(),
        font: iced::Font::DEFAULT,
        align_x: t::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: t::Shaping::Advanced,
        wrapping: t::Wrapping::None,
        ellipsis: t::Ellipsis::None,
        hint_factor: None,
    })
    .min_width()
}

/// Width of one [`PAD`] at the text size (a chip label's inset), measured once.
fn pad_width() -> f32 {
    static W: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *W.get_or_init(|| text_width(&PAD.to_string(), TEXT_SIZE))
}

/// Width of one [`FILL`] at the text size, measured once.
fn fill_width() -> f32 {
    static W: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *W.get_or_init(|| text_width(&FILL.to_string(), TEXT_SIZE))
}

/// The id written after a chip's open mark.
fn id_marks(id: u32) -> String {
    format!("{id:x}")
        .chars()
        .filter_map(|d| char::from_u32(ID_BASE + d.to_digit(16)?))
        .collect()
}

/// The id of a chip from the text between its marks.
fn chip_id(inner: &str) -> Option<u32> {
    let digits: String = inner
        .chars()
        .take_while(|c| (ID_BASE..ID_BASE + 16).contains(&(*c as u32)))
        .filter_map(|c| char::from_digit(c as u32 - ID_BASE, 16))
        .collect();
    u32::from_str_radix(&digits, 16).ok()
}

/// The input box state of one session.
pub struct Composer {
    pub content: Content,
    /// Sent texts (with their chips), oldest first.
    history: Vec<String>,
    /// The history entry shown (`None` = the draft).
    pos: Option<usize>,
    /// The draft while a history entry is shown.
    draft: String,
    /// Every chip made in this session (history entries keep theirs).
    chips: Vec<Chip>,
    next_id: u32,
    /// Byte spans of the chips in the text, for the pills.
    spans: Vec<Match>,
    /// The span of each chip's close mark (`×`), in the same order.
    closes: Vec<Match>,
    /// Each chip's label, in the same order (drawn by the editor).
    labels: Vec<String>,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            content: Content::with_text(""),
            history: Vec::new(),
            pos: None,
            draft: String::new(),
            chips: Vec::new(),
            next_id: 1,
            spans: Vec::new(),
            closes: Vec::new(),
            labels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ComposerMsg {
    Edit(String, text_editor::Action),
    Send(String),
    /// ↑ (`true`) / ↓ through the sent history.
    History(String, bool),
    /// Backspace (`true`) / Delete next to a chip: remove the whole chip.
    ChipDelete(String, bool),
    /// ← / → (`true`) next to a chip: jump over it.
    ChipSkip(String, bool),
    /// A chip's × pressed (its index among the chips in the text).
    ChipClose(String, usize),
    /// The header's 입력창 toggle.
    Toggle,
    /// Ctrl/Cmd+V in the box: files copied in the file manager, else an image (screenshot),
    /// else text.
    PasteKey(String),
    /// What the clipboard held, for the box or the terminal.
    PasteFiles(String, Dest, Vec<std::path::PathBuf>),
    PasteImage(String, Dest, iced::clipboard::Image),
    PasteText(String, Option<String>),
    /// An attachment stored on the server: its path (or why not).
    Uploaded(String, Dest, Result<String, ide_client::ProtoError>),
}

/// Where an attachment goes: a chip in the input box, or its path typed into the agent's own
/// input (the input box hidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Composer,
    Terminal,
}

/// A path as a terminal pastes a dropped file: quoted when it has white space.
pub fn quote_path(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        format!("\"{path}\"")
    } else {
        path.to_owned()
    }
}

/// The chip spans (`OPEN` .. `CLOSE`, both included) of one line.
fn line_spans(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut open = None;
    for (i, c) in line.char_indices() {
        match c {
            OPEN => open = Some(i),
            CLOSE => {
                if let Some(s) = open.take() {
                    out.push((s, i + CLOSE.len_utf8()));
                }
            }
            _ => {}
        }
    }
    out
}

impl Composer {
    pub(crate) fn set_text(&mut self, t: &str) {
        self.content = Content::with_text(t);
        // Cursor at the end, so a further ↓ / ↑ keeps walking and typing goes on.
        self.content
            .perform(text_editor::Action::Move(Motion::DocumentEnd));
        self.refresh();
    }

    /// The labels of the chips in the text, in order.
    pub(crate) fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Recompute the chip spans after the text changed.
    fn refresh(&mut self) {
        let mut spans = Vec::new();
        let mut closes = Vec::new();
        let mut labels = Vec::new();
        let chips = &self.chips;
        self.content.for_each_line(|line, t| {
            for (start, end) in line_spans(t) {
                spans.push(Match { line, start, end });
                let id = chip_id(&t[start + OPEN.len_utf8()..]);
                labels.push(
                    chips
                        .iter()
                        .find(|c| Some(c.id) == id)
                        .map(|c| c.label.clone())
                        .unwrap_or_default(),
                );
                // The × is the chip's last mark before its closing pad.
                if let Some(x) = t[start..end].rfind(CLOSE_MARK) {
                    let s = start + x;
                    closes.push(Match {
                        line,
                        start: s,
                        end: s + CLOSE_MARK.len_utf8(),
                    });
                }
            }
        });
        self.spans = spans;
        self.closes = closes;
        self.labels = labels;
    }

    /// Keep the cursor and the selection off a chip's inside (Desk: a chip is one piece): a
    /// cursor inside goes to the nearer edge, a selection end inside takes the whole chip.
    fn snap(&mut self) {
        use iced::advanced::text::Position;
        let inside = |p: Position| {
            self.spans
                .iter()
                .find(|m| m.line == p.line && m.start < p.index && p.index < m.end)
                .copied()
        };
        let cur = self.content.cursor();
        let at = |m: Match, index: usize| Position {
            line: m.line,
            index,
        };
        let next = match cur.selection {
            None => inside(cur.position).map(|m| {
                let near = if cur.position.index - m.start <= m.end - cur.position.index {
                    m.start
                } else {
                    m.end
                };
                Cursor {
                    position: at(m, near),
                    selection: None,
                }
            }),
            Some(anchor) => {
                let forward =
                    (anchor.line, anchor.index) <= (cur.position.line, cur.position.index);
                let a = inside(anchor).map(|m| at(m, if forward { m.start } else { m.end }));
                let p = inside(cur.position).map(|m| at(m, if forward { m.end } else { m.start }));
                (a.is_some() || p.is_some()).then(|| Cursor {
                    position: p.unwrap_or(cur.position),
                    selection: Some(a.unwrap_or(anchor)),
                })
            }
        };
        if let Some(c) = next {
            self.content.move_to(c);
        }
    }

    /// Remove chip `k` (in text order), as its × does.
    fn close_chip(&mut self, k: usize) {
        if let Some(m) = self.spans.get(k).copied() {
            self.content.select(m.line, m.start, m.end);
            self.content.perform(text_editor::Action::Edit(Edit::Delete));
            self.refresh();
        }
    }

    /// ↑ / ↓: show the previous / next sent text; past the newest the draft comes back.
    fn walk(&mut self, up: bool) {
        let n = self.history.len();
        let next = match (self.pos, up) {
            (_, true) if n == 0 => return,
            (None, true) => {
                self.draft = self.content.text();
                Some(n - 1)
            }
            (Some(i), true) => Some(i.saturating_sub(1)),
            (None, false) => return,
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(_), false) => None,
        };
        self.pos = next;
        let t = match next {
            Some(i) => self.history[i].clone(),
            None => std::mem::take(&mut self.draft),
        };
        self.set_text(&t);
    }

    /// Insert a chip for `path` at the cursor (label: its file name).
    pub fn insert_chip(&mut self, path: &str) {
        let label = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());
        let id = self.next_id;
        self.next_id += 1;
        // Room for the label at the chip size, in blank cells of the text size (not spaces: a
        // word of only spaces may be split by the line wrap).
        let room = (text_width(&label, CHIP_SIZE) / fill_width().max(1.0)).ceil() as usize;
        let fill: String = std::iter::repeat_n(FILL, room.max(1)).collect();
        self.chips.push(Chip {
            id,
            label,
            path: path.to_owned(),
        });
        let marks = id_marks(id);
        self.content
            .perform(text_editor::Action::Edit(Edit::Paste(Arc::new(format!(
                "{OPEN}{marks}{PAD}{fill}{PAD}{CLOSE_MARK}{PAD}{PAD}{CLOSE} "
            )))));
        self.refresh();
    }

    /// The text as sent: each chip replaced by its (quoted) path, stray marks dropped.
    fn serialize(&self, raw: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(o) = rest.find(OPEN) {
            out.push_str(&rest[..o]);
            let after = &rest[o + OPEN.len_utf8()..];
            match after.find(CLOSE) {
                Some(c) => {
                    let id = chip_id(&after[..c]);
                    if let Some(chip) = self.chips.iter().find(|x| Some(x.id) == id) {
                        out.push_str(&quote_path(&chip.path));
                    }
                    rest = &after[c + CLOSE.len_utf8()..];
                }
                None => {
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out.replace([OPEN, CLOSE], "")
    }

    /// Take the text to send (chips as paths, trailing white space dropped) and remember it.
    fn take(&mut self) -> Option<String> {
        let raw = self.content.text();
        let raw = raw.trim_end().to_owned();
        if raw.trim().is_empty() {
            return None;
        }
        if self.history.last() != Some(&raw) {
            self.history.push(raw.clone());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        self.pos = None;
        self.draft.clear();
        let sent = self.serialize(&raw);
        self.set_text("");
        Some(sent)
    }

    /// The chip span touching the cursor: the one ending at it (`back`) or starting at it.
    fn chip_at_cursor(&self, back: bool) -> Option<Match> {
        let p = self.content.cursor().position;
        self.spans
            .iter()
            .find(|m| m.line == p.line && if back { m.end == p.index } else { m.start == p.index })
            .copied()
    }

    fn delete_chip(&mut self, back: bool) {
        if let Some(m) = self.chip_at_cursor(back) {
            self.content.select(m.line, m.start, m.end);
            self.content.perform(text_editor::Action::Edit(Edit::Delete));
            self.refresh();
        }
    }

    fn skip_chip(&mut self, forward: bool) {
        if let Some(m) = self.chip_at_cursor(!forward) {
            let index = if forward { m.end } else { m.start };
            self.content.move_to(Cursor {
                position: iced::advanced::text::Position {
                    line: m.line,
                    index,
                },
                selection: None,
            });
        }
    }
}

impl App {
    pub(crate) fn on_composer(&mut self, m: ComposerMsg) -> Task<Message> {
        match m {
            ComposerMsg::Edit(sid, a) => {
                if let Some(l) = self.live_mut(&sid) {
                    let edits = matches!(a, text_editor::Action::Edit(_));
                    l.composer.content.perform(a);
                    if edits {
                        l.composer.refresh();
                    }
                    // Clicks, drags, word / line selection and arrow moves never stop inside
                    // a chip.
                    l.composer.snap();
                }
                Task::none()
            }
            ComposerMsg::ChipClose(sid, k) => {
                if let Some(l) = self.live_mut(&sid) {
                    l.composer.close_chip(k);
                }
                operation::focus(COMPOSER_ID)
            }
            ComposerMsg::History(sid, up) => {
                if let Some(l) = self.live_mut(&sid) {
                    l.composer.walk(up);
                }
                Task::none()
            }
            ComposerMsg::ChipDelete(sid, back) => {
                if let Some(l) = self.live_mut(&sid) {
                    l.composer.delete_chip(back);
                }
                Task::none()
            }
            ComposerMsg::ChipSkip(sid, forward) => {
                if let Some(l) = self.live_mut(&sid) {
                    l.composer.skip_chip(forward);
                }
                Task::none()
            }
            ComposerMsg::Send(sid) => {
                if !self.connected() {
                    return Task::none();
                }
                let Some(l) = self.live_mut(&sid) else {
                    return Task::none();
                };
                let Some(text) = l.composer.take() else {
                    return Task::none();
                };
                let bytes = l.term.paste_bytes(&text);
                l.term.scroll_to_bottom();
                crate::script::log(&format!("composer: send {} chars to {sid}", text.chars().count()));
                let paste = self.update(Message::TermInput(sid.clone(), bytes));
                let enter = Task::perform(tokio::time::sleep(ENTER_AFTER), |()| ())
                    .map(move |()| Message::TermInput(sid.clone(), b"\r".to_vec()));
                Task::batch([paste, enter])
            }
            ComposerMsg::Toggle => {
                self.ui.composer_open = !self.ui.composer_open;
                self.save_ui();
                if self.ui.composer_open {
                    operation::focus(COMPOSER_ID)
                } else {
                    Task::none()
                }
            }
            ComposerMsg::PasteKey(sid) => paste_from_clipboard(sid, Dest::Composer),
            ComposerMsg::PasteFiles(sid, dest, files) => {
                Task::batch(files.into_iter().map(|p| self.attach_path(&sid, p, dest)))
            }
            ComposerMsg::PasteImage(sid, dest, image) => self.attach_screenshot(&sid, image, dest),
            ComposerMsg::PasteText(sid, text) => {
                if let (Some(t), Some(l)) = (text, self.live_mut(&sid)) {
                    l.composer
                        .content
                        .perform(text_editor::Action::Edit(Edit::Paste(Arc::new(t))));
                    l.composer.refresh();
                }
                Task::none()
            }
            ComposerMsg::Uploaded(sid, dest, r) => match r {
                Ok(path) => self.deliver(&sid, &path, dest),
                Err(e) => {
                    crate::script::log(&format!("attachment upload failed: {e:?}"));
                    let msg = crate::i18n::proto_error(&e);
                    if let Some(l) = self.live_mut(&sid) {
                        l.detail.toast(msg);
                    }
                    Task::none()
                }
            },
        }
    }

    /// A file of this machine for session `sid`: its own path when the server runs here, else
    /// a copy sent to the server (a folder cannot be sent).
    fn attach_path(&mut self, sid: &str, path: std::path::PathBuf, dest: Dest) -> Task<Message> {
        if self.target.local {
            return self.deliver(sid, &path.display().to_string(), dest);
        }
        let Some(c) = self.online() else {
            return Task::none();
        };
        if path.is_dir() {
            if let Some(l) = self.live_mut(sid) {
                l.detail.toast(tr::attach_no_folder_remote());
            }
            return Task::none();
        }
        let (s, s2) = (sid.to_owned(), sid.to_owned());
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Task::perform(
            async move {
                let data = tokio::task::spawn_blocking(move || std::fs::read(&path))
                    .await
                    .map_err(|e| ide_client::ProtoError::new("io").arg("detail", e.to_string()))?
                    .map_err(|e| ide_client::ProtoError::new("io").arg("detail", e.to_string()))?;
                c.attach_file(&s, &name, &data).await
            },
            move |r| Message::Composer(ComposerMsg::Uploaded(s2.clone(), dest, r)),
        )
    }

    /// A clipboard image: encoded as PNG and stored on the server as `screenshot-<time>.png`.
    fn attach_screenshot(
        &mut self,
        sid: &str,
        image: iced::clipboard::Image,
        dest: Dest,
    ) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        let (s, s2) = (sid.to_owned(), sid.to_owned());
        let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let name = format!(
            "screenshot-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        crate::script::log(&format!(
            "clipboard image {}x{} -> {name}",
            image.size.width, image.size.height
        ));
        Task::perform(
            async move {
                let png = tokio::task::spawn_blocking(move || encode_png(&image))
                    .await
                    .map_err(|e| ide_client::ProtoError::new("io").arg("detail", e.to_string()))?
                    .map_err(|e| ide_client::ProtoError::new("io").arg("detail", e))?;
                c.attach_file(&s, &name, &png).await
            },
            move |r| Message::Composer(ComposerMsg::Uploaded(s2.clone(), dest, r)),
        )
    }

    /// Put an attachment's path where it goes: a chip in the box, or typed into the agent's
    /// own input as a terminal pastes a dropped file.
    pub(crate) fn deliver(&mut self, sid: &str, path: &str, dest: Dest) -> Task<Message> {
        crate::script::log(&format!("attachment for {sid} ({dest:?}): {path}"));
        match dest {
            Dest::Composer => {
                if let Some(l) = self.live_mut(sid) {
                    l.composer.insert_chip(path);
                }
                operation::focus(COMPOSER_ID)
            }
            Dest::Terminal => {
                let Some(l) = self.live(sid) else {
                    return Task::none();
                };
                let bytes = l.term.paste_bytes(&format!("{} ", quote_path(path)));
                self.update(Message::TermInput(sid.to_owned(), bytes))
            }
        }
    }

    /// A file dropped on the window (from the OS): a chip in the input box when it is shown,
    /// else its path typed into the agent's own input (where its cursor is).
    pub(crate) fn on_file_dropped(&mut self, path: std::path::PathBuf) -> Task<Message> {
        let Some(sid) = self.selected.clone() else {
            return Task::none();
        };
        crate::script::log(&format!("file dropped: {}", path.display()));
        let dest = self.attach_dest(&sid);
        self.attach_path(&sid, path, dest)
    }

    /// An in-app file drag ended: over the input box it becomes a chip; over the terminal it
    /// goes where attachments of the session go now (the box, or the agent's own input).
    pub(crate) fn end_file_drag(&mut self) -> Task<Message> {
        let drag = self.file_drag.take();
        let target = self.drop_target.take();
        self.drag_pointer = None;
        let (Some((sid, paths)), Some(target)) = (drag, target) else {
            return Task::none();
        };
        let dest = match target {
            Dest::Composer => Dest::Composer,
            Dest::Terminal => self.attach_dest(&sid),
        };
        crate::script::log(&format!("drop {paths:?} on {target:?} -> {dest:?}"));
        Task::batch(
            paths
                .iter()
                .map(|p| self.deliver(&sid, p, dest))
                .collect::<Vec<_>>(),
        )
    }

    /// The label following the pointer during an in-app file drag.
    pub(crate) fn drag_ghost(&self) -> El<'_> {
        let (Some((_, paths)), Some(p)) = (&self.file_drag, self.drag_pointer) else {
            return iced::widget::space().boxed();
        };
        let first = paths.first().map(String::as_str).unwrap_or_default();
        let name = std::path::Path::new(first)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Several entries: the first one's name and how many more.
        let name = match paths.len() {
            0 | 1 => name,
            n => format!("{name}  +{}", n - 1),
        };
        let label = container(text(name).size(CHIP_SIZE).color(style::TEXT))
            .padding([2, 8])
            .style(|_| container::Style {
                background: Some(style::accent_alpha(0.3).into()),
                border: iced::Border {
                    color: style::accent_alpha(0.7),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..container::Style::default()
            });
        iced::widget::pin(label)
            .position(iced::Point::new(p.x + 14.0, p.y + 10.0))
            .width(Fill)
            .height(Fill)
            .boxed()
    }

    /// Where an attachment for session `sid` goes now.
    pub(crate) fn attach_dest(&self, sid: &str) -> Dest {
        let shown = self
            .live(sid)
            .is_some_and(|l| self.composer_shown(l) && l.state.is_alive());
        if shown { Dest::Composer } else { Dest::Terminal }
    }

    /// Ctrl+V in the terminal found no text (the input box hidden): files or an image from the
    /// clipboard go in as paths.
    pub(crate) fn terminal_paste_not_text(&self, sid: &str) -> Task<Message> {
        paste_from_clipboard(sid.to_owned(), Dest::Terminal)
    }

    /// Whether session `live` can have the input box (agent sessions; login terminals not).
    pub(crate) fn composer_shown_kind(&self, live: &crate::app::Live) -> bool {
        live.info.kind == ide_client::SessionKind::Agent
    }

    /// Whether session `live` shows the input box (agent sessions, while the toggle is on).
    pub(crate) fn composer_shown(&self, live: &crate::app::Live) -> bool {
        self.ui.composer_open && self.composer_shown_kind(live)
    }

    /// The input box of session `live`, under its terminal.
    pub(crate) fn composer_view<'a>(&'a self, live: &'a crate::app::Live) -> El<'a> {
        let sid = live.info.id.clone();
        let c = &live.composer;
        let cursor = c.content.cursor();
        let lines = c.content.line_count().max(1);
        let first = cursor.position.line == 0;
        let last = cursor.position.line + 1 >= lines;
        let line = c.content.line(cursor.position.line).unwrap_or_default();
        let at = cursor.position.index.min(line.len());
        let before = line[..at].chars().next_back();
        let after = line[at..].chars().next();
        let online = self.connected();
        let keys = {
            let sid = sid.clone();
            move |k: KeyPress| -> Option<Binding<Message>> {
                if !k.is_focused {
                    return None;
                }
                let plain = !k.modifiers.shift()
                    && !k.modifiers.control()
                    && !k.modifiers.alt()
                    && !k.modifiers.logo();
                let msg = |m: ComposerMsg| Some(Binding::Custom(Message::Composer(m)));
                match &k.key {
                    Key::Named(Named::Enter) if plain => msg(ComposerMsg::Send(sid.clone())),
                    Key::Named(Named::Enter) if k.modifiers.shift() => Some(Binding::Enter),
                    // Paste: files and images become chips (`paste_from_clipboard`).
                    Key::Character(c) if k.modifiers.command() && !k.modifiers.alt() && c.as_str() == "v" => {
                        msg(ComposerMsg::PasteKey(sid.clone()))
                    }
                    Key::Named(Named::Insert) if k.modifiers.shift() => {
                        msg(ComposerMsg::PasteKey(sid.clone()))
                    }
                    Key::Named(Named::Backspace) if before == Some(CLOSE) => {
                        msg(ComposerMsg::ChipDelete(sid.clone(), true))
                    }
                    Key::Named(Named::Delete) if after == Some(OPEN) => {
                        msg(ComposerMsg::ChipDelete(sid.clone(), false))
                    }
                    Key::Named(Named::ArrowLeft) if plain && before == Some(CLOSE) => {
                        msg(ComposerMsg::ChipSkip(sid.clone(), false))
                    }
                    Key::Named(Named::ArrowRight) if plain && after == Some(OPEN) => {
                        msg(ComposerMsg::ChipSkip(sid.clone(), true))
                    }
                    Key::Named(Named::ArrowUp) if plain && first => {
                        msg(ComposerMsg::History(sid.clone(), true))
                    }
                    Key::Named(Named::ArrowDown) if plain && last => {
                        msg(ComposerMsg::History(sid.clone(), false))
                    }
                    _ => Binding::from_key_press(k),
                }
            }
        };
        let edit_sid = sid.clone();
        let editor = CodeEditor::new(&c.content)
            .id(iced::widget::Id::new(COMPOSER_ID))
            .plain(iced::Font::DEFAULT, TEXT_SIZE, LINE_H)
            .padding(iced::Padding {
                top: PAD_Y,
                right: 10.0,
                bottom: PAD_Y,
                left: 16.0,
            })
            .max_height(MAX_H)
            .placeholder(tr::composer_placeholder())
            .colors(style::TEXT, style::FAINT)
            .chips(&c.spans, style::accent_alpha(0.16), style::accent_alpha(0.45))
            .chip_labels(&c.labels, CHIP_SIZE, pad_width())
            .chip_close(&c.closes, {
                let sid = sid.clone();
                move |k| Message::Composer(ComposerMsg::ChipClose(sid.clone(), k))
            })
            .on_action(move |a| Message::Composer(ComposerMsg::Edit(edit_sid.clone(), a)))
            .key_binding(keys);
        // Desk's send button: a round ↑ inside the box, bottom right.
        let can_send = online && !c.content.text().trim().is_empty();
        let send = button(
            container(icons::icon(Icon::ArrowUp, 16.0, style::ON_ACCENT)).center(SEND),
        )
        .width(SEND)
        .height(SEND)
        .padding(0)
        .style(move |_: &iced::Theme, status| button::Style {
            background: Some(
                match (can_send, status) {
                    (false, _) => style::ACCENT_DISABLED,
                    (true, button::Status::Hovered | button::Status::Pressed) => {
                        style::ACCENT_HOVER
                    }
                    _ => style::ACCENT,
                }
                .into(),
            ),
            text_color: style::ON_ACCENT,
            border: iced::Border {
                radius: (SEND / 2.0).into(),
                ..iced::Border::default()
            },
            shadow: iced::Shadow::default(),
            snap: true,
        })
        .on_press_maybe(can_send.then(|| Message::Composer(ComposerMsg::Send(sid.clone()))));
        let dragging = self.file_drag.is_some();
        let over = dragging && self.drop_target == Some(Dest::Composer);
        let boxed = container(
            row![
                container(editor).width(Fill),
                // The same gap above (single line), right of and below the button.
                container(tip(send.boxed(), tr::composer_send_tip())).padding(GAP),
            ]
            .align_y(Alignment::End),
        )
        .width(Fill)
        .style(move |_| container::Style {
            background: Some(style::INPUT.into()),
            border: iced::Border {
                // A file dragged in the app: the box is where it drops.
                color: match (dragging, over) {
                    (true, true) => style::ACCENT,
                    (true, false) => style::accent_alpha(0.5),
                    _ => style::BORDER,
                },
                width: if dragging { 1.5 } else { 1.0 },
                radius: RADIUS.into(),
            },
            ..container::Style::default()
        });
        let boxed = iced::widget::mouse_area(boxed)
            .on_enter(Message::DropHover(Dest::Composer, true))
            .on_exit(Message::DropHover(Dest::Composer, false));
        container(boxed)
            .padding([8, 10])
            .width(Fill)
            .style(style::bg(style::HEADER))
            .boxed()
    }

    /// The header's 입력창 toggle.
    pub(crate) fn composer_toggle(&self) -> El<'_> {
        let on = self.ui.composer_open;
        let b = button(text(tr::composer_toggle()).size(12))
            .padding([2, 8])
            .style(style::choice(on))
            .on_press(Message::Composer(ComposerMsg::Toggle));
        tip(
            b.boxed(),
            if on {
                tr::composer_hide()
            } else {
                tr::composer_show()
            },
        )
    }
}

/// Read the clipboard for an attachment: file paths (copied in the file manager) first, then an
/// image (a screenshot); the input box then takes plain text as a normal paste.
fn paste_from_clipboard(sid: String, dest: Dest) -> Task<Message> {
    use iced::clipboard;
    clipboard::read_files().then(move |files| {
        let sid = sid.clone();
        match files {
            Ok(f) if !f.is_empty() => Task::done(Message::Composer(ComposerMsg::PasteFiles(
                sid,
                dest,
                f.to_vec(),
            ))),
            _ => clipboard::read_image().then(move |image| {
                let sid = sid.clone();
                match image {
                    Ok(img) => Task::done(Message::Composer(ComposerMsg::PasteImage(sid, dest, img))),
                    Err(_) if dest == Dest::Composer => clipboard::read_text().map(move |t| {
                        Message::Composer(ComposerMsg::PasteText(
                            sid.clone(),
                            t.ok().map(|t| t.to_string()),
                        ))
                    }),
                    Err(_) => Task::none(),
                }
            }),
        }
    })
}

/// A clipboard image as PNG bytes.
fn encode_png(image: &iced::clipboard::Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, image.size.width, image.size.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&image.rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_and_walk_history() {
        let mut c = Composer::default();
        assert!(c.take().is_none(), "empty box sends nothing");
        c.set_text("first\n");
        assert_eq!(c.take().as_deref(), Some("first"));
        c.set_text("second\nline");
        assert_eq!(c.take().as_deref(), Some("second\nline"));
        c.set_text("draft");
        c.walk(true);
        assert_eq!(c.content.text(), "second\nline");
        c.walk(true);
        assert_eq!(c.content.text(), "first");
        c.walk(true);
        assert_eq!(c.content.text(), "first", "stays at the oldest");
        c.walk(false);
        c.walk(false);
        assert_eq!(c.content.text(), "draft", "past the newest the draft comes back");
        c.walk(false);
        assert_eq!(c.content.text(), "draft");
    }

    #[test]
    fn chips_are_sent_as_paths() {
        let mut c = Composer::default();
        c.set_text("look at ");
        c.insert_chip(r"C:\shots\a b.png");
        c.content
            .perform(text_editor::Action::Edit(Edit::Paste(Arc::new("and ".into()))));
        c.insert_chip(r"C:\src\main.rs");
        c.refresh();
        assert_eq!(c.spans.len(), 2);
        assert_eq!(
            c.take().as_deref(),
            Some(r#"look at "C:\shots\a b.png" and C:\src\main.rs"#)
        );
        // Chips are told apart by id (two of the same name, other folders); labels are drawn.
        c.insert_chip(r"C:\x\main.rs");
        c.insert_chip(r"C:\y\main.rs");
        assert_eq!(c.labels, ["main.rs", "main.rs"]);
        assert_eq!(c.take().as_deref(), Some(r"C:\x\main.rs C:\y\main.rs"));
        assert_eq!(chip_id(&id_marks(0x2a)), Some(0x2a));
    }

    #[test]
    fn cursor_and_selection_never_stop_inside_a_chip() {
        use iced::advanced::text::Position;
        let mut c = Composer::default();
        c.set_text("ab ");
        c.insert_chip(r"C:\long-name.png");
        let m = c.spans[0];
        let to = |c: &mut Composer, index: usize, anchor: Option<usize>| {
            c.content.move_to(Cursor {
                position: Position { line: 0, index },
                selection: anchor.map(|i| Position { line: 0, index: i }),
            });
            c.snap();
            let cur = c.content.cursor();
            (cur.position.index, cur.selection.map(|s| s.index))
        };
        // A click just inside either end goes to that end.
        assert_eq!(to(&mut c, m.start + 2, None), (m.start, None));
        assert_eq!(to(&mut c, m.end - 2, None), (m.end, None));
        // A drag from before the chip into it takes the whole chip; so does one backwards.
        assert_eq!(to(&mut c, m.start + 3, Some(0)), (m.end, Some(0)));
        let len = c.content.text().len();
        assert_eq!(to(&mut c, m.end - 3, Some(len)), (m.start, Some(len)));
        // Sent text and label survive the close mark.
        assert_eq!(c.closes.len(), 1);
        // The × removes the chip.
        c.close_chip(0);
        assert!(c.spans.is_empty() && c.closes.is_empty());
        assert_eq!(c.content.text(), "ab  ");
    }

    #[test]
    fn a_chip_is_deleted_and_skipped_whole() {
        let mut c = Composer::default();
        c.insert_chip(r"C:\a.png");
        let text = c.content.text();
        assert!(text.starts_with(OPEN) && text.ends_with(' '));
        // Cursor after the chip's space: step back over the space, then Backspace.
        c.content
            .perform(text_editor::Action::Move(Motion::Left));
        c.skip_chip(false);
        assert_eq!(c.content.cursor().position.index, 0);
        c.skip_chip(true);
        c.delete_chip(true);
        assert_eq!(c.content.text(), " ");
        assert!(c.spans.is_empty());
    }
}
