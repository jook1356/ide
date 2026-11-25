//! File viewer / editor (Hermes Desk `FileEditor`): syntax highlighting, line numbers, current
//! line, find bar (Ctrl+F; Enter / Shift+Enter / F3), go to line (Ctrl+G), read-only mode, dirty
//! tracking with a Revert / Save bar (Ctrl+S), wrap toggle (Alt+Z), a large-file guard (plain
//! text above [`HIGHLIGHT_LIMIT`]), Ctrl/Cmd+click for go-to-definition and [`CodeView::go_to`].
//!
//! Line endings: the text is edited with `\n`; [`CodeEvent::Save`] gives it back with the
//! file's original line ending (`\r\n` when the loaded text had any).

use crate::editor::{CodeEditor, Content, Match};
use crate::interact::Interact;
use crate::syntax;
use crate::theme;
use meninx_i18n::tr;
use iced::Widget as _;
use iced::advanced::text::editor::{Action, Binding, KeyPress};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{container, row, space, stack, text, text_input};
use iced::{Alignment, Element, Length, Padding, Task};

/// Files larger than this (bytes) are shown without syntax highlighting.
pub const HIGHLIGHT_LIMIT: usize = 2_000_000;
/// At most this many find matches are highlighted / counted.
const MAX_MATCHES: usize = 10_000;

#[derive(Debug, Clone)]
pub enum CodeMessage {
    Action(Action),
    Save,
    Revert,
    OpenFind,
    FindInput(String),
    FindNext,
    FindPrev,
    CloseFind,
    OpenGoto,
    GotoInput(String),
    GotoSubmit,
    CloseGoto,
    CtrlClick,
    ToggleWrap,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodeEvent {
    /// Ctrl+S or the save button: write `text` (original line endings) to `path`, then call
    /// [`CodeView::mark_saved`] with the same text.
    Save { path: String, text: String },
    /// The edits were dropped (back to the last saved text).
    Reverted,
    /// The unsaved-edits flag changed (tabs show it).
    DirtyChanged(bool),
    /// Ctrl/Cmd+click: go to the definition of the symbol at 1-based `line` / `column`
    /// (column in characters).
    Definition {
        path: String,
        line: usize,
        column: usize,
    },
}

struct Find {
    query: String,
    matches: Vec<Match>,
    current: Option<usize>,
}

/// The view's state; keep one per open file.
pub struct CodeView {
    path: String,
    content: Content,
    /// Last saved text (`\n` line endings).
    saved: String,
    crlf: bool,
    dirty: bool,
    read_only: bool,
    wrap: bool,
    language: Option<String>,
    find: Option<Find>,
    goto: Option<String>,
    editor_id: iced::widget::Id,
    find_id: iced::widget::Id,
    goto_id: iced::widget::Id,
}

impl CodeView {
    /// Opens `text` as the contents of `path` (used for the language and in events).
    pub fn new(path: impl Into<String>, text: &str) -> Self {
        Self::with_highlight_limit(path, text, HIGHLIGHT_LIMIT)
    }

    /// Like [`CodeView::new`] with another large-file limit (bytes) for syntax highlighting.
    pub fn with_highlight_limit(path: impl Into<String>, text: &str, limit: usize) -> Self {
        let path = path.into();
        let crlf = text.contains("\r\n");
        let normalized = if crlf {
            text.replace("\r\n", "\n")
        } else {
            text.to_owned()
        };
        let language = if normalized.len() > limit {
            None
        } else {
            syntax::language_token(&path)
        };
        Self {
            content: Content::with_text(&normalized),
            saved: normalized,
            path,
            crlf,
            dirty: false,
            read_only: false,
            wrap: false,
            language,
            find: None,
            goto: None,
            editor_id: iced::widget::Id::unique(),
            find_id: iced::widget::Id::unique(),
            goto_id: iced::widget::Id::unique(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The current text with the file's line endings.
    pub fn text(&self) -> String {
        self.with_eol(self.content.text())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
    }

    pub fn wrap(&self) -> bool {
        self.wrap
    }

    /// Whether syntax highlighting is on (off for plain text and above the size limit).
    pub fn is_highlighted(&self) -> bool {
        self.language.is_some()
    }

    pub fn line_count(&self) -> usize {
        self.content.line_count()
    }

    /// Cursor as 1-based (line, column in characters).
    pub fn cursor(&self) -> (usize, usize) {
        let c = self.content.cursor();
        let col = self
            .content
            .line(c.position.line)
            .map(|l| l[..c.position.index.min(l.len())].chars().count())
            .unwrap_or(0);
        (c.position.line + 1, col + 1)
    }

    /// First line (1-based) at the top of the view.
    pub fn top_line(&self) -> usize {
        self.content.top_line() + 1
    }

    /// Find state: (query, match count, current 0-based index).
    pub fn find_state(&self) -> Option<(&str, usize, Option<usize>)> {
        self.find
            .as_ref()
            .map(|f| (f.query.as_str(), f.matches.len(), f.current))
    }

    /// Marks `text` (as given in [`CodeEvent::Save`]) as saved.
    pub fn mark_saved(&mut self, text: &str) -> Option<CodeEvent> {
        self.saved = text.replace("\r\n", "\n");
        self.refresh_dirty()
    }

    /// The file changed on disk: takes `text` unless there are unsaved edits (then the edits
    /// are kept and `false` is returned).
    pub fn reload(&mut self, text: &str) -> bool {
        if self.dirty {
            return false;
        }
        let (line, col) = self.cursor();
        self.crlf = text.contains("\r\n");
        self.saved = text.replace("\r\n", "\n");
        self.content = Content::with_text(&self.saved);
        self.reveal(line, col);
        self.refind();
        true
    }

    /// Puts the cursor at 1-based `line` / `column` (characters; clamped) in the middle of the
    /// view and focuses the editor.
    pub fn go_to<M: Send + 'static>(&mut self, line: usize, column: usize) -> Task<M> {
        self.reveal(line, column);
        self.focus()
    }

    /// Focuses the editor.
    pub fn focus<M: Send + 'static>(&self) -> Task<M> {
        iced::widget::operation::focus(self.editor_id.clone())
    }

    fn reveal(&mut self, line: usize, column: usize) {
        let line0 = line.max(1) - 1;
        let index = self
            .content
            .line(line0.min(self.content.line_count().saturating_sub(1)))
            .map(|l| {
                l.char_indices()
                    .nth(column.max(1) - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(l.len())
            })
            .unwrap_or(0);
        self.content.reveal(line0, index);
    }

    fn with_eol(&self, text: String) -> String {
        if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text
        }
    }

    fn refresh_dirty(&mut self) -> Option<CodeEvent> {
        let dirty = self.content.text() != self.saved;
        (dirty != self.dirty).then(|| {
            self.dirty = dirty;
            CodeEvent::DirtyChanged(dirty)
        })
    }

    fn refind(&mut self) {
        let Some(find) = &mut self.find else {
            return;
        };
        find.matches.clear();
        find.current = None;
        if find.query.is_empty() {
            return;
        }
        let ascii = find.query.is_ascii();
        let needle = if ascii {
            find.query.to_ascii_lowercase()
        } else {
            find.query.clone()
        };
        let matches = &mut find.matches;
        self.content.for_each_line(|line, text| {
            if matches.len() >= MAX_MATCHES {
                return;
            }
            let hay;
            let hay: &str = if ascii {
                hay = text.to_ascii_lowercase();
                &hay
            } else {
                text
            };
            let mut from = 0;
            while let Some(i) = hay[from..].find(&needle) {
                let start = from + i;
                let end = start + needle.len();
                matches.push(Match { line, start, end });
                from = end.max(start + 1);
                if matches.len() >= MAX_MATCHES || from >= hay.len() {
                    break;
                }
            }
        });
        // The first match at or after the cursor.
        let c = self.content.cursor().position;
        let current = find
            .matches
            .iter()
            .position(|m| (m.line, m.start) >= (c.line, c.index))
            .or((!find.matches.is_empty()).then_some(0));
        find.current = current;
    }

    fn select_current(&mut self) {
        if let Some(f) = &self.find
            && let Some(m) = f.current.and_then(|i| f.matches.get(i))
        {
            let m = *m;
            self.content.select(m.line, m.start, m.end);
        }
    }

    pub fn update(&mut self, msg: CodeMessage) -> (Task<CodeMessage>, Option<CodeEvent>) {
        match msg {
            CodeMessage::Action(action) => {
                if self.read_only && action.is_edit() {
                    return (Task::none(), None);
                }
                let edit = action.is_edit();
                self.content.perform(action);
                if edit {
                    let ev = self.refresh_dirty();
                    if self.find.is_some() {
                        self.refind();
                    }
                    return (Task::none(), ev);
                }
                (Task::none(), None)
            }
            CodeMessage::Save => {
                if self.read_only {
                    return (Task::none(), None);
                }
                let text = self.text();
                (
                    Task::none(),
                    Some(CodeEvent::Save {
                        path: self.path.clone(),
                        text,
                    }),
                )
            }
            CodeMessage::Revert => {
                let (line, col) = self.cursor();
                self.content = Content::with_text(&self.saved);
                self.reveal(line, col);
                self.refind();
                let was = self.dirty;
                self.dirty = false;
                (
                    self.focus(),
                    if was {
                        Some(CodeEvent::Reverted)
                    } else {
                        None
                    },
                )
            }
            CodeMessage::OpenFind => {
                self.goto = None;
                let seed = self
                    .content
                    .selection()
                    .filter(|s| !s.contains('\n') && !s.is_empty());
                match &mut self.find {
                    Some(f) => {
                        if let Some(s) = seed {
                            f.query = s;
                        }
                    }
                    None => {
                        self.find = Some(Find {
                            query: seed.unwrap_or_default(),
                            matches: Vec::new(),
                            current: None,
                        })
                    }
                }
                self.refind();
                (
                    Task::batch([
                        iced::widget::operation::focus(self.find_id.clone()),
                        iced::widget::operation::text_input::select_all(self.find_id.clone()),
                    ]),
                    None,
                )
            }
            CodeMessage::FindInput(q) => {
                if let Some(f) = &mut self.find {
                    f.query = q;
                }
                self.refind();
                self.select_current();
                (Task::none(), None)
            }
            CodeMessage::FindNext | CodeMessage::FindPrev => {
                if let Some(f) = &mut self.find
                    && !f.matches.is_empty()
                {
                    let n = f.matches.len();
                    let c = f.current.unwrap_or(0);
                    f.current = Some(if matches!(msg, CodeMessage::FindNext) {
                        (c + 1) % n
                    } else {
                        (c + n - 1) % n
                    });
                }
                self.select_current();
                (Task::none(), None)
            }
            CodeMessage::CloseFind => {
                self.find = None;
                (self.focus(), None)
            }
            CodeMessage::OpenGoto => {
                self.find = None;
                self.goto = Some(String::new());
                (iced::widget::operation::focus(self.goto_id.clone()), None)
            }
            CodeMessage::GotoInput(v) => {
                self.goto = Some(v);
                (Task::none(), None)
            }
            CodeMessage::GotoSubmit => {
                let Some(v) = self.goto.take() else {
                    return (Task::none(), None);
                };
                let mut parts = v.trim().splitn(2, [':', ',']);
                let line = parts.next().and_then(|s| s.trim().parse::<usize>().ok());
                let col = parts
                    .next()
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(1);
                match line {
                    Some(l) => (self.go_to(l, col), None),
                    None => (self.focus(), None),
                }
            }
            CodeMessage::CloseGoto => {
                self.goto = None;
                (self.focus(), None)
            }
            CodeMessage::CtrlClick => {
                let (line, column) = self.cursor();
                (
                    Task::none(),
                    Some(CodeEvent::Definition {
                        path: self.path.clone(),
                        line,
                        column,
                    }),
                )
            }
            CodeMessage::ToggleWrap => {
                self.wrap = !self.wrap;
                (Task::none(), None)
            }
        }
    }

    pub fn view(&self) -> Element<'_, CodeMessage> {
        let find_open = self.find.is_some();
        let (matches, current): (&[Match], Option<usize>) = match &self.find {
            Some(f) => (&f.matches, f.current),
            None => (&[], None),
        };
        let editor = CodeEditor::new(&self.content)
            .id(self.editor_id.clone())
            .language(self.language.clone())
            .wrap(self.wrap)
            .matches(matches, current)
            .on_action(CodeMessage::Action)
            .on_ctrl_click(CodeMessage::CtrlClick)
            .key_binding(move |k| key_binding(k, find_open));

        let mut layers = vec![Element::from(editor)];
        if let Some(card) = self.find_card().or_else(|| self.goto_card()) {
            layers.push(
                container(card)
                    .width(Length::Fill)
                    .align_right(Length::Fill)
                    .padding(Padding {
                        top: 6.0,
                        right: 14.0,
                        ..Padding::ZERO
                    })
                    .boxed(),
            );
        }
        let main = stack(layers).width(Length::Fill).height(Length::Fill);
        // The editor keeps the same place in the widget tree whether or not the save bar is
        // shown (a changed structure would reset its state, e.g. drop focus on the first edit).
        let mut col = iced::widget::Column::new().push(main.boxed());
        if !self.dirty || self.read_only {
            return col.boxed();
        }
        let bar = container(
            row![
                text(format!("● {}", tr::code_modified()))
                    .size(12)
                    .color(theme::GIT_MODIFIED),
                space().width(Length::Fill),
                theme::icon_button("↺", Some(CodeMessage::Revert), tr::code_revert()),
                theme::icon_button("✓", Some(CodeMessage::Save), tr::code_save()),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([3, 10])
        .width(Length::Fill)
        .style(|t| {
            let mut s = theme::header_bar(t);
            s.border = iced::Border {
                color: theme::BORDER,
                width: 1.0,
                radius: 0.0.into(),
            };
            s
        });
        col = col.push(bar.boxed());
        col.boxed()
    }

    fn find_card(&self) -> Option<Element<'_, CodeMessage>> {
        let f = self.find.as_ref()?;
        let count = if f.query.is_empty() {
            String::new()
        } else if f.matches.is_empty() {
            tr::code_find_none()
        } else {
            let more = if f.matches.len() >= MAX_MATCHES { "+" } else { "" };
            format!(
                "{}/{}{more}",
                f.current.map(|c| c + 1).unwrap_or(0),
                f.matches.len()
            )
        };
        let input = text_input(tr::code_find_placeholder(), &f.query)
            .id(self.find_id.clone())
            .on_input(CodeMessage::FindInput)
            .size(12)
            .padding([4, 8])
            .width(Length::Fixed(200.0))
            .font(theme::mono())
            .style(theme::input);
        let has = !f.matches.is_empty();
        let card = container(
            row![
                input,
                text(count)
                    .size(12)
                    .color(if has || f.query.is_empty() {
                        theme::DIM
                    } else {
                        theme::GIT_DELETED
                    })
                    .width(Length::Fixed(64.0))
                    .align_x(Alignment::Center),
                theme::icon_button("↑", has.then_some(CodeMessage::FindPrev), tr::code_find_prev()),
                theme::icon_button("↓", has.then_some(CodeMessage::FindNext), tr::code_find_next()),
                theme::icon_button("✕", Some(CodeMessage::CloseFind), tr::close()),
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        )
        .padding([5, 6])
        .style(theme::card);
        Some(
            Interact::new(card.boxed())
                .focus_within()
                .intercept()
                .on_key(find_key)
                .into(),
        )
    }

    fn goto_card(&self) -> Option<Element<'_, CodeMessage>> {
        let v = self.goto.as_ref()?;
        let input = text_input(tr::code_goto_placeholder(), v)
            .id(self.goto_id.clone())
            .on_input(CodeMessage::GotoInput)
            .on_submit(CodeMessage::GotoSubmit)
            .size(12)
            .padding([4, 8])
            .width(Length::Fixed(220.0))
            .font(theme::mono())
            .style(theme::input);
        let card = container(input).padding(5).style(theme::card);
        Some(
            Interact::new(card.boxed())
                .focus_within()
                .intercept()
                .on_key(|k, _| {
                    matches!(k, Key::Named(Named::Escape)).then_some(CodeMessage::CloseGoto)
                })
                .into(),
        )
    }
}

/// Keys while the find input is focused.
fn find_key(key: &Key, m: Modifiers) -> Option<CodeMessage> {
    match key {
        Key::Named(Named::Escape) => Some(CodeMessage::CloseFind),
        Key::Named(Named::Enter) if m.shift() => Some(CodeMessage::FindPrev),
        Key::Named(Named::Enter) => Some(CodeMessage::FindNext),
        Key::Named(Named::F3) if m.shift() => Some(CodeMessage::FindPrev),
        Key::Named(Named::F3) => Some(CodeMessage::FindNext),
        _ => None,
    }
}

/// Editor keys: the defaults plus save / find / go to line / wrap.
fn key_binding(k: KeyPress, find_open: bool) -> Option<Binding<CodeMessage>> {
    if !k.is_focused {
        return None;
    }
    let cmd = k.modifiers.command();
    let custom = match k.key.to_latin(k.physical_key) {
        Some('s') if cmd => Some(CodeMessage::Save),
        Some('f') if cmd => Some(CodeMessage::OpenFind),
        Some('g') if cmd => Some(CodeMessage::OpenGoto),
        Some('z') if k.modifiers.alt() && !cmd => Some(CodeMessage::ToggleWrap),
        _ => match &k.key {
            Key::Named(Named::F3) if find_open && k.modifiers.shift() => Some(CodeMessage::FindPrev),
            Key::Named(Named::F3) if find_open => Some(CodeMessage::FindNext),
            Key::Named(Named::Escape) if find_open => Some(CodeMessage::CloseFind),
            _ => None,
        },
    };
    if let Some(m) = custom {
        return Some(Binding::Custom(m));
    }
    Binding::from_key_press(k)
}

#[cfg(test)]
mod tests {
    #![allow(unused_must_use)]
    use super::*;
    use iced::advanced::text::editor::Edit;

    fn type_str(v: &mut CodeView, s: &str) -> Vec<CodeEvent> {
        s.chars()
            .filter_map(|c| v.update(CodeMessage::Action(Action::Edit(Edit::Insert(c)))).1)
            .collect()
    }

    #[test]
    fn dirty_save_revert_keep_crlf() {
        let mut v = CodeView::new("a.rs", "fn main() {}\r\nlet x = 1;\r\n");
        assert_eq!(type_str(&mut v, "// "), [CodeEvent::DirtyChanged(true)]);
        let (_, ev) = v.update(CodeMessage::Save);
        let Some(CodeEvent::Save { text, .. }) = ev else {
            panic!("no save")
        };
        assert_eq!(text, "// fn main() {}\r\nlet x = 1;\r\n");
        assert_eq!(v.mark_saved(&text), Some(CodeEvent::DirtyChanged(false)));
        type_str(&mut v, "x");
        assert!(v.is_dirty());
        let (_, ev) = v.update(CodeMessage::Revert);
        assert_eq!(ev, Some(CodeEvent::Reverted));
        assert_eq!(v.text(), "// fn main() {}\r\nlet x = 1;\r\n");
    }

    #[test]
    fn read_only_drops_edits() {
        let mut v = CodeView::new("a.txt", "hello");
        v.set_read_only(true);
        assert!(type_str(&mut v, "x").is_empty());
        assert_eq!(v.text(), "hello");
        assert_eq!(v.update(CodeMessage::Save).1, None);
    }

    #[test]
    fn find_counts_case_insensitive_and_cycles() {
        let mut v = CodeView::new("a.txt", "Foo foo\nbar FOO\n한글 foo");
        v.update(CodeMessage::OpenFind);
        v.update(CodeMessage::FindInput("foo".into()));
        let (q, n, cur) = v.find_state().unwrap();
        assert_eq!((q, n, cur), ("foo", 4, Some(0)));
        v.update(CodeMessage::FindPrev);
        assert_eq!(v.find_state().unwrap().2, Some(3));
        // The last match is after Korean text: byte offsets.
        assert_eq!(v.cursor(), (3, 7));
        v.update(CodeMessage::FindInput("한글".into()));
        assert_eq!(v.find_state().unwrap().1, 1);
    }

    #[test]
    fn go_to_line_and_ctrl_click() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let mut v = CodeView::new("a.txt", &text);
        v.update(CodeMessage::OpenGoto);
        v.update(CodeMessage::GotoInput("42:3".into()));
        v.update(CodeMessage::GotoSubmit);
        assert_eq!(v.cursor(), (42, 3));
        let (_, ev) = v.update(CodeMessage::CtrlClick);
        assert_eq!(
            ev,
            Some(CodeEvent::Definition {
                path: "a.txt".into(),
                line: 42,
                column: 3
            })
        );
    }

    #[test]
    fn large_files_are_plain() {
        let big = "x".repeat(100);
        assert!(!CodeView::with_highlight_limit("a.rs", &big, 50).is_highlighted());
        assert!(CodeView::with_highlight_limit("a.rs", &big, 500).is_highlighted());
        assert!(!CodeView::new("notes.txt", "hi").is_highlighted());
    }
}
