//! The code editor widget behind [`crate::CodeView`]: iced's text editor core (cosmic-text
//! buffer, `text::editor::State` for keys / mouse / IME / clipboard) drawn with a line-number
//! gutter, a current-line highlight and find-match highlights, plus Ctrl/Cmd+click reporting,
//! a vertical scrollbar and auto-scroll while a selection is dragged past the top or bottom.
//!
//! It is a fork of `iced::widget::text_editor` (same `Action`s and `Binding`s) specialised to
//! `iced::Renderer`, because the stock widget exposes neither the scroll position (needed for
//! the gutter) nor the laid-out lines (needed for match highlights).

use crate::syntax;
use crate::theme;
use iced::advanced::graphics::text::Editor as GEditor;
use iced::advanced::graphics::text::cosmic_text;
use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::text::editor::{self as core_editor, Action, Binding, Cursor, KeyPress};
use iced::advanced::text::{self, Editor as _, LineHeight, Position, Renderer as _, Wrapping};
use iced::advanced::widget::{self, Operation, Tree, Widget, tree};
use iced::advanced::{Layout, Shell, clipboard};
use iced::keyboard::{self, Modifiers};
use iced::time::{Duration, Instant};
use iced::{Code, Element, Event, Length, Pixels, Point, Rectangle, Size, Theme, alignment, window};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

const PAD_LEFT: f32 = 8.0;
const PAD_TOP: f32 = 4.0;
const GUTTER_PAD: f32 = 12.0;
/// The scrollbar along the right edge (drawn over the text, like VS Code's).
const SCROLLBAR_W: f32 = 10.0;
const THUMB_MIN: f32 = 24.0;
/// While a selection is dragged past the text area: one scroll step this often, of up to
/// [`AUTOSCROLL_MAX`] lines (more the farther the pointer is).
const AUTOSCROLL_EVERY: Duration = Duration::from_millis(40);
const AUTOSCROLL_MAX: i32 = 8;

static GENERATION: AtomicU64 = AtomicU64::new(1);

/// The × of a chip's close mark (the colour comes from the draw call).
static CLOSE_ICON: std::sync::LazyLock<iced::advanced::svg::Handle> = std::sync::LazyLock::new(|| {
    iced::advanced::svg::Handle::from_memory(
        br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path d="M1.5 1.5l7 7m0-7l-7 7" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round"/></svg>"##.as_slice(),
    )
});

/// A find match: byte range `start..end` on 0-based `line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Text buffer of a [`CodeEditor`].
pub struct Content {
    editor: RefCell<GEditor>,
    generation: u64,
    /// Cursor to put in the middle of the view at the next layout (`reveal`).
    pending_reveal: Cell<Option<Position>>,
}

impl Content {
    pub fn with_text(text: &str) -> Self {
        Self {
            editor: RefCell::new(load(text)),
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            pending_reveal: Cell::new(None),
        }
    }

    pub fn perform(&mut self, action: Action) {
        self.editor.get_mut().perform(action);
    }

    pub fn text(&self) -> String {
        self.editor.borrow().text()
    }

    pub fn line_count(&self) -> usize {
        self.editor.borrow().line_count()
    }

    pub fn line(&self, i: usize) -> Option<String> {
        self.editor.borrow().line(i).map(|l| l.text.into_owned())
    }

    /// Calls `f` with every line (0-based index, text) without copying the buffer.
    pub fn for_each_line(&self, mut f: impl FnMut(usize, &str)) {
        let ed = self.editor.borrow();
        for (i, line) in ed.buffer().lines.iter().enumerate() {
            f(i, line.text());
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.editor.borrow().cursor()
    }

    /// Selected text, if any.
    pub fn selection(&self) -> Option<String> {
        self.editor.borrow().copy()
    }

    pub fn move_to(&mut self, cursor: Cursor) {
        self.editor.get_mut().move_to(cursor);
    }

    /// Moves the cursor to `line` / byte `index` (clamped) and scrolls it to the middle of the
    /// view at the next layout.
    pub fn reveal(&mut self, line: usize, index: usize) {
        let ed = self.editor.get_mut();
        let line = line.min(ed.line_count().saturating_sub(1));
        let len = ed.line(line).map(|l| l.text.len()).unwrap_or(0);
        let position = Position {
            line,
            index: index.min(len),
        };
        ed.move_to(Cursor {
            position,
            selection: None,
        });
        self.pending_reveal.set(Some(position));
    }

    /// Selects `start..end` on `line` (bytes) and reveals it.
    pub fn select(&mut self, line: usize, start: usize, end: usize) {
        self.reveal(line, end);
        self.editor.get_mut().move_to(Cursor {
            position: Position { line, index: end },
            selection: Some(Position { line, index: start }),
        });
    }

    /// Buffer line at the top of the view.
    pub fn top_line(&self) -> usize {
        self.editor.borrow().buffer().scroll().line
    }

    /// Number of (visual) lines that fit in the view.
    pub fn visible_lines(&self) -> usize {
        let ed = self.editor.borrow();
        let lh = ed.buffer().metrics().line_height.max(1.0);
        (ed.bounds().height * ed.hint_factor().unwrap_or(1.0) / lh).floor() as usize
    }

    fn apply_reveal(&self) {
        let Some(p) = self.pending_reveal.take() else {
            return;
        };
        let mut ed = self.editor.borrow_mut();
        if ed.bounds().height <= 0.0 {
            self.pending_reveal.set(Some(p));
            return;
        }
        let lh = ed.buffer().metrics().line_height.max(1.0);
        let window = (ed.bounds().height * ed.hint_factor().unwrap_or(1.0) / lh) as i64;
        let top = ed.buffer().scroll().line as i64;
        let want_top = (p.line as i64 - window / 2).max(0);
        let delta = want_top - top;
        if delta != 0 {
            ed.perform(Action::Scroll {
                lines: delta as i32,
            });
        }
    }
}

/// Texts up to this size go straight to `Editor::with_text`, which shapes every line (the
/// buffer has no height yet): ~50 µs per line, so 1 s for 20 000 lines.
const DIRECT_LOAD_BYTES: usize = 64 * 1024;
/// Lines per paste when loading a large text (cosmic-text inserts the lines of one paste one by
/// one in the middle of its line vector: quadratic in the paste size).
const LOAD_CHUNK_LINES: usize = 128;

/// Builds the editor buffer for `text`. Large texts are pasted in chunks into an editor that is
/// one line tall, so only the lines in view are ever shaped.
fn load(text: &str) -> GEditor {
    if text.len() <= DIRECT_LOAD_BYTES {
        return GEditor::with_text(text);
    }
    let mut ed = GEditor::with_text("");
    ed.update(
        Size::new(1000.0, theme::CODE_LINE),
        theme::mono(),
        Pixels(theme::CODE_SIZE),
        LineHeight::Absolute(Pixels(theme::CODE_LINE)),
        Wrapping::None,
        text::Alignment::Default,
        None,
        &mut Parser::Plain(0),
    );
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest
            .match_indices('\n')
            .nth(LOAD_CHUNK_LINES - 1)
            .map(|(i, _)| i + 1)
            .unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(end);
        ed.perform(Action::Move(core_editor::Motion::DocumentEnd));
        ed.perform(Action::Edit(core_editor::Edit::Paste(std::sync::Arc::new(chunk.to_owned()))));
        rest = tail;
    }
    ed.perform(Action::Move(core_editor::Motion::DocumentStart));
    ed
}

/// Lines the parser may run ahead of the view before it restarts near the view instead (a jump
/// far down a large file would otherwise parse every line above it first; syntect manages only
/// a few thousand lines per second).
const PARSE_JUMP_LINES: usize = 300;
/// Lines parsed above the view after such a restart, so that most constructs have settled.
const PARSE_LEAD_LINES: usize = 60;

/// Syntax parser of the editor: iced's syntect parser, or nothing (plain text / large files).
///
/// The syntect parser starts at line `base` (0 unless the view jumped far ahead, see
/// [`Parser::follow_view`]): highlighting after a restart assumes the restart line is at the top
/// level of the grammar, like editors that highlight only the viewport.
pub enum Parser {
    Syntax {
        inner: iced::highlighter::Parser,
        token: String,
        base: usize,
    },
    Plain(usize),
}

impl Parser {
    fn restart(&mut self, at: usize) {
        if let Parser::Syntax { inner, token, base } = self {
            *inner = iced::highlighter::Parser::new(&iced::highlighter::Settings { token: token.clone() });
            *base = at;
        }
    }

    /// Called before highlighting with the first line in view: restarts the parser near the view
    /// when it is far behind it (jump down) or the view is above where it started (jump back up).
    pub fn follow_view(&mut self, top: usize) {
        let Parser::Syntax { inner, base, .. } = self else {
            return;
        };
        let current = *base + inner.current_line();
        if top > current + PARSE_JUMP_LINES || top < *base {
            let at = top.saturating_sub(PARSE_LEAD_LINES);
            // Back to the start when it is close anyway (exact highlighting).
            let at = if at <= PARSE_JUMP_LINES { 0 } else { at };
            self.restart(at);
        }
    }
}

impl text::Parser for Parser {
    type Settings = Option<String>;
    type Output = Code;
    type Iterator<'a> = Box<dyn Iterator<Item = (Range<usize>, Code)> + 'a>;

    fn new(settings: &Self::Settings) -> Self {
        match settings {
            Some(token) => Parser::Syntax {
                inner: iced::highlighter::Parser::new(&iced::highlighter::Settings { token: token.clone() }),
                token: token.clone(),
                base: 0,
            },
            None => Parser::Plain(0),
        }
    }

    fn update(&mut self, settings: &Self::Settings) {
        *self = <Self as text::Parser>::new(settings);
    }

    fn change_line(&mut self, line: usize) {
        match self {
            Parser::Syntax { inner, base, .. } => {
                if line >= *base {
                    inner.change_line(line - *base);
                } else {
                    // An edit above the restart point: restart there.
                    let at = if line <= PARSE_JUMP_LINES { 0 } else { line };
                    self.restart(at);
                }
            }
            Parser::Plain(l) => *l = line,
        }
    }

    fn parse_line(&mut self, line: &str) -> Self::Iterator<'_> {
        match self {
            Parser::Syntax { inner, .. } => inner.parse_line(line),
            Parser::Plain(l) => {
                *l += 1;
                Box::new(std::iter::empty())
            }
        }
    }

    fn current_line(&self) -> usize {
        match self {
            Parser::Syntax { inner, base, .. } => *base + inner.current_line(),
            Parser::Plain(l) => *l,
        }
    }
}

type KeyBindingFn<'a, M> = Box<dyn Fn(KeyPress) -> Option<Binding<M>> + 'a>;

/// The editor widget. Build it in `view` from a [`Content`] kept in your state.
pub struct CodeEditor<'a, M> {
    content: &'a Content,
    id: Option<widget::Id>,
    language: Option<String>,
    wrap: bool,
    size: f32,
    line_height: f32,
    on_action: Option<Box<dyn Fn(Action) -> M + 'a>>,
    key_binding: Option<KeyBindingFn<'a, M>>,
    on_ctrl_click: Option<M>,
    matches: &'a [Match],
    current_match: Option<usize>,
    /// Chat-input look ([`CodeEditor::plain`]): no gutter, current-line band or background.
    plain: bool,
    font: iced::Font,
    padding: iced::Padding,
    /// Grow with the text up to this height, then scroll (`None` = fill the space).
    max_height: Option<f32>,
    placeholder: Option<String>,
    text_color: iced::Color,
    placeholder_color: iced::Color,
    /// Spans drawn as pills behind their text (attachments in a chat input), and the pill's
    /// fill and border.
    chips: &'a [Match],
    chip_colors: (iced::Color, iced::Color),
    /// The chips' close marks (one span each, the `×` in the chip's text): pressed, they
    /// publish `on_chip_close(index)` instead of moving the cursor.
    chip_close: &'a [Match],
    on_chip_close: Option<Box<dyn Fn(usize) -> M + 'a>>,
    /// Each chip's label (same order as `chips`), drawn over its pill at `chip_label_size`,
    /// `chip_label_inset` from the pill's left edge (the chip's text is only its room).
    chip_labels: &'a [String],
    chip_label_size: f32,
    chip_label_inset: f32,
}

struct State {
    editor: core_editor::State,
    parser: RefCell<Parser>,
    settings: Option<String>,
    generation: u64,
    modifiers: Modifiers,
    /// The left button went down in the text: a selection may be dragged.
    selecting: bool,
    /// Last pointer position (window coordinates), also outside the widget.
    pointer: Option<Point>,
    /// Next auto-scroll step while selecting past the edge.
    next_autoscroll: Option<Instant>,
    /// The scrollbar thumb is dragged, grabbed this far below its top.
    thumb_grab: Option<f32>,
    /// A scroll the scrollbar asked for that may not be applied yet: (top line when asked,
    /// requested top line). Pointer moves in one batch then do not add up.
    scroll_ask: Option<(usize, usize)>,
}

/// The scrollbar's geometry for the current scroll position.
struct Scrollbar {
    track: Rectangle,
    thumb: Rectangle,
    max_top: usize,
}

impl Scrollbar {
    /// The top line that puts the thumb's top at `y`.
    fn top_at(&self, y: f32) -> usize {
        let room = (self.track.height - self.thumb.height).max(1.0);
        let f = ((y - self.track.y) / room).clamp(0.0, 1.0);
        (f * self.max_top as f32).round() as usize
    }
}

impl<'a, M: Clone> CodeEditor<'a, M> {
    pub fn new(content: &'a Content) -> Self {
        Self {
            content,
            id: None,
            language: None,
            wrap: false,
            size: theme::CODE_SIZE,
            line_height: theme::CODE_LINE,
            on_action: None,
            key_binding: None,
            on_ctrl_click: None,
            matches: &[],
            current_match: None,
            plain: false,
            font: theme::mono(),
            padding: iced::Padding {
                top: PAD_TOP,
                left: PAD_LEFT,
                right: 0.0,
                bottom: 0.0,
            },
            max_height: None,
            placeholder: None,
            text_color: theme::CODE_TEXT,
            placeholder_color: theme::LINE_NO,
            chips: &[],
            chip_colors: (theme::FIND_MATCH, theme::FIND_MATCH_BORDER),
            chip_close: &[],
            on_chip_close: None,
            chip_labels: &[],
            chip_label_size: 12.0,
            chip_label_inset: 4.0,
        }
    }

    /// Labels drawn over the chips' pills (same order as the chips), at `size`, `inset` from
    /// a pill's left edge.
    pub fn chip_labels(mut self, labels: &'a [String], size: f32, inset: f32) -> Self {
        self.chip_labels = labels;
        self.chip_label_size = size;
        self.chip_label_inset = inset;
        self
    }

    /// The chips' close marks (spans of their `×`) and what pressing one publishes.
    pub fn chip_close(mut self, marks: &'a [Match], f: impl Fn(usize) -> M + 'a) -> Self {
        self.chip_close = marks;
        self.on_chip_close = Some(Box::new(f));
        self
    }

    /// The horizontal scroll that applies to drawing: none in the (wrapping) plain look.
    fn h_scroll(&self, horizontal: f32) -> f32 {
        if self.plain { 0.0 } else { horizontal }
    }

    /// Screen rectangles of `spans` in the laid-out text: (index in `spans`, rectangle), one per
    /// visual segment.
    fn span_rects(&self, ed: &GEditor, text: Rectangle, spans: &[Match]) -> Vec<(usize, Rectangle)> {
        let buffer = ed.buffer();
        let hf = ed.hint_factor().unwrap_or(1.0);
        let scroll = buffer.scroll();
        let mut out = Vec::new();
        for run in buffer.layout_runs() {
            for (k, m) in spans.iter().enumerate().filter(|(_, m)| m.line == run.line_i) {
                let a = cosmic_text::Cursor::new(m.line, m.start);
                let b = cosmic_text::Cursor::new(m.line, m.end);
                for (x, w) in run.highlight(a, b) {
                    out.push((
                        k,
                        Rectangle::new(
                            Point::new(
                                text.x + (x - self.h_scroll(scroll.horizontal)) / hf,
                                text.y + run.line_top / hf,
                            ),
                            Size::new(w / hf, run.line_height / hf),
                        ),
                    ));
                }
            }
        }
        out
    }

    /// The close mark under `p`, if any.
    fn close_at(&self, bounds: Rectangle, p: Point) -> Option<usize> {
        if self.chip_close.is_empty() {
            return None;
        }
        let ed = self.content.editor.borrow();
        self.span_rects(&ed, self.text_bounds(bounds), self.chip_close)
            .into_iter()
            .find(|(_, r)| r.expand(2.0).contains(p))
            .map(|(k, _)| k)
    }

    /// The chat-input look: no gutter, no current-line band, no background, text in `font` at
    /// `size` / `line_height`, soft-wrapped.
    pub fn plain(mut self, font: iced::Font, size: f32, line_height: f32) -> Self {
        self.plain = true;
        self.font = font;
        self.size = size;
        self.line_height = line_height;
        self.wrap = true;
        self
    }

    pub fn padding(mut self, padding: impl Into<iced::Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Grow with the text up to `max` (then scroll) instead of filling the space.
    pub fn max_height(mut self, max: f32) -> Self {
        self.max_height = Some(max);
        self
    }

    /// Shown while the text is empty.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Text and placeholder colours.
    pub fn colors(mut self, text: iced::Color, placeholder: iced::Color) -> Self {
        self.text_color = text;
        self.placeholder_color = placeholder;
        self
    }

    /// Spans (sorted by position) drawn as pills behind their text, with their fill and border.
    pub fn chips(mut self, spans: &'a [Match], fill: iced::Color, border: iced::Color) -> Self {
        self.chips = spans;
        self.chip_colors = (fill, border);
        self
    }

    pub fn id(mut self, id: widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Syntax token (see [`syntax::language_token`]); `None` = plain text.
    pub fn language(mut self, token: Option<String>) -> Self {
        self.language = token;
        self
    }

    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn on_action(mut self, f: impl Fn(Action) -> M + 'a) -> Self {
        self.on_action = Some(Box::new(f));
        self
    }

    pub fn key_binding(mut self, f: impl Fn(KeyPress) -> Option<Binding<M>> + 'a) -> Self {
        self.key_binding = Some(Box::new(f));
        self
    }

    /// Published right after the click action of a Ctrl/Cmd+click (the cursor is already at the
    /// clicked position when it arrives).
    pub fn on_ctrl_click(mut self, message: M) -> Self {
        self.on_ctrl_click = Some(message);
        self
    }

    pub fn matches(mut self, matches: &'a [Match], current: Option<usize>) -> Self {
        self.matches = matches;
        self.current_match = current;
        self
    }

    fn gutter_width(&self) -> f32 {
        if self.plain {
            return 0.0;
        }
        let digits = self.content.line_count().max(1).ilog10() as f32 + 1.0;
        (digits.max(3.0) * self.size * 0.6 + 2.0 * GUTTER_PAD).ceil()
    }

    fn text_bounds(&self, bounds: Rectangle) -> Rectangle {
        let p = self.padding;
        let g = self.gutter_width() + p.left;
        Rectangle {
            x: bounds.x + g,
            y: bounds.y + p.top,
            width: (bounds.width - g - p.right).max(0.0),
            height: (bounds.height - p.top - p.bottom).max(0.0),
        }
    }

    /// The scrollbar, when the text does not fit (buffer lines; with wrapping an estimate).
    fn scrollbar(&self, bounds: Rectangle) -> Option<Scrollbar> {
        let total = self.content.line_count();
        let visible = self.content.visible_lines().max(1);
        if total <= visible || bounds.height <= 0.0 {
            return None;
        }
        let max_top = total - visible;
        let track = Rectangle {
            x: bounds.x + bounds.width - SCROLLBAR_W,
            // The plain look keeps the bar off a rounded box's corners (Desk: 6 px).
            y: bounds.y + if self.plain { 6.0 } else { 0.0 },
            width: SCROLLBAR_W,
            height: bounds.height - if self.plain { 12.0 } else { 0.0 },
        };
        let h = (track.height * visible as f32 / total as f32)
            .max(THUMB_MIN)
            .min(track.height);
        let top = self.content.top_line().min(max_top);
        let y = track.y + (track.height - h) * top as f32 / max_top as f32;
        Some(Scrollbar {
            track,
            thumb: Rectangle {
                y,
                height: h,
                ..track
            },
            max_top,
        })
    }

    /// Scrolls so that `target` is the top line (relative to what is already asked for).
    fn scroll_to(&self, state: &mut State, target: usize, shell: &mut Shell<'_, M>) {
        let Some(on_action) = self.on_action.as_ref() else {
            return;
        };
        let actual = self.content.top_line();
        let base = match state.scroll_ask {
            Some((seen, asked)) if seen == actual => asked,
            _ => actual,
        };
        let delta = target as i64 - base as i64;
        if delta != 0 {
            shell.publish(on_action(Action::Scroll {
                lines: delta as i32,
            }));
            state.scroll_ask = Some((actual, target));
        }
    }
}

impl<M> widget::Meta for CodeEditor<'_, M> {}

impl<M: Clone> Widget<M, Theme, iced::Renderer> for CodeEditor<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            editor: core_editor::State::new(),
            parser: RefCell::new(<Parser as text::Parser>::new(&self.language)),
            settings: self.language.clone(),
            generation: self.content.generation,
            modifiers: Modifiers::empty(),
            selecting: false,
            pointer: None,
            next_autoscroll: None,
            thumb_grab: None,
            scroll_ask: None,
        })
    }

    fn size(&self) -> Size<Length> {
        let h = if self.max_height.is_some() {
            Length::Shrink
        } else {
            Length::Fill
        };
        Size::new(Length::Fill, h)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let state = tree.state.downcast_mut::<State>();
        let parser = state.parser.get_mut();
        if state.settings != self.language {
            text::Parser::update(parser, &self.language);
            state.settings = self.language.clone();
        }
        if state.generation != self.content.generation {
            text::Parser::change_line(parser, 0);
            state.generation = self.content.generation;
        }
        let mut size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        if let Some(max) = self.max_height {
            size.height = max.min(limits.max.height);
        }
        let text = self.text_bounds(Rectangle::new(Point::ORIGIN, size));
        self.content.editor.borrow_mut().update(
            text.size(),
            self.font,
            Pixels(self.size),
            LineHeight::Absolute(Pixels(self.line_height)),
            if self.wrap {
                Wrapping::WordOrGlyph
            } else {
                Wrapping::None
            },
            text::Alignment::Default,
            renderer.hint_factor(),
            parser,
        );
        self.content.apply_reveal();
        // Growing: as tall as the laid-out lines (at least one), up to the cap; scrolled
        // means the text is taller than the cap.
        if self.max_height.is_some() {
            let ed = self.content.editor.borrow();
            let hf = ed.hint_factor().unwrap_or(1.0);
            let buffer = ed.buffer();
            let used = buffer
                .layout_runs()
                .map(|r| (r.line_top + r.line_height) / hf)
                .fold(self.line_height, f32::max);
            if buffer.scroll().line == 0 {
                let p = self.padding;
                size.height = (used + p.top + p.bottom).min(size.height);
            }
        }
        tree.size = size;
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let Some(on_action) = self.on_action.as_ref() else {
            return;
        };
        let state = tree.state.downcast_mut::<State>();
        if let Event::Keyboard(keyboard::Event::ModifiersChanged(m)) = event {
            state.modifiers = *m;
        }
        let bounds = layout.bounds();
        let text_bounds = self.text_bounds(bounds);
        if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
            state.pointer = Some(*position);
            // The close marks' hover look.
            if !self.chip_close.is_empty() {
                shell.request_redraw();
            }
        }

        // A chip's close mark: remove the chip, the cursor stays where it is.
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && let (Some(f), Some(p)) = (&self.on_chip_close, cursor.position_over(bounds))
            && let Some(k) = self.close_at(bounds, p)
        {
            shell.publish(f(k));
            shell.capture_event();
            return;
        }

        // Scrollbar: drag the thumb, or press the track to jump there (then drag on).
        let bar = self.scrollbar(bounds);
        if state.thumb_grab.is_some() {
            match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    if let (Some(b), Some(grab)) = (&bar, state.thumb_grab) {
                        self.scroll_to(state, b.top_at(position.y - grab), shell);
                    }
                    shell.capture_event();
                    return;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    state.thumb_grab = None;
                    state.scroll_ask = None;
                    shell.request_redraw();
                    shell.capture_event();
                    return;
                }
                _ => {}
            }
        }
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && let Some(b) = &bar
            && let Some(p) = cursor.position_over(b.track)
        {
            let grab = if b.thumb.contains(p) {
                p.y - b.thumb.y
            } else {
                let grab = b.thumb.height / 2.0;
                self.scroll_to(state, b.top_at(p.y - grab), shell);
                grab
            };
            state.thumb_grab = Some(grab);
            shell.request_redraw();
            shell.capture_event();
            return;
        }

        // Selecting: past the top / bottom of the text the view scrolls, and the selection
        // follows to the edge line, while the button is held.
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.selecting = cursor.is_over(text_bounds);
                state.next_autoscroll = None;
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.selecting = false;
                state.next_autoscroll = None;
            }
            _ => {}
        }
        if state.selecting
            && let Some(p) = state.pointer
        {
            let bottom = text_bounds.y + text_bounds.height;
            let past = if p.y < text_bounds.y {
                p.y - text_bounds.y
            } else if p.y > bottom {
                p.y - bottom
            } else {
                0.0
            };
            if past != 0.0 {
                match event {
                    Event::Window(window::Event::RedrawRequested(now)) => {
                        if state.next_autoscroll.is_none_or(|t| *now >= t) {
                            let step = (1 + (past.abs() / self.line_height) as i32)
                                .min(AUTOSCROLL_MAX);
                            let edge = if past < 0.0 {
                                0.0
                            } else {
                                (text_bounds.height - 1.0).max(0.0)
                            };
                            let x = (p.x - text_bounds.x).clamp(0.0, text_bounds.width);
                            shell.publish(on_action(Action::Scroll {
                                lines: if past < 0.0 { -step } else { step },
                            }));
                            shell.publish(on_action(Action::Drag(Point::new(x, edge))));
                            state.next_autoscroll = Some(*now + AUTOSCROLL_EVERY);
                        }
                        if let Some(t) = state.next_autoscroll {
                            shell.request_redraw_at(t);
                        }
                    }
                    Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                        shell.request_redraw();
                        shell.capture_event();
                        return;
                    }
                    _ => {}
                }
            } else {
                state.next_autoscroll = None;
            }
        }

        // The wheel scrolls over the gutter too.
        let cursor = match (event, cursor.position()) {
            (Event::Mouse(mouse::Event::WheelScrolled { .. }), Some(p))
                if bounds.contains(p) && !text_bounds.contains(p) =>
            {
                mouse::Cursor::Available(Point::new(text_bounds.x + 1.0, p.y))
            }
            _ => cursor,
        };
        let ctrl_click = matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) && state.modifiers.command()
            && cursor.is_over(text_bounds);

        let editor = self.content.editor.borrow();
        let key_binding = self
            .key_binding
            .as_deref()
            .unwrap_or(&Binding::from_key_press as _);
        if let Some(update) = state.editor.update(
            &*editor,
            event,
            text_bounds,
            iced::Padding::ZERO,
            cursor,
            key_binding,
        ) {
            apply_update(update, shell, on_action.as_ref());
            if ctrl_click && let Some(m) = &self.on_ctrl_click {
                shell.publish(m.clone());
            }
        }
        if let Event::Window(window::Event::RedrawRequested(_)) = event {
            shell.request_input_method(
                &state
                    .editor
                    .input_method(&*editor, text_bounds.position()),
            );
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();
        // Read before the buffer is borrowed for drawing.
        let bar = self.scrollbar(bounds);
        let text_bounds = self.text_bounds(bounds);
        let gutter_w = self.gutter_width();

        let mut ed = self.content.editor.borrow_mut();
        {
            let mut parser = state.parser.borrow_mut();
            parser.follow_view(ed.buffer().scroll().line);
            ed.highlight(self.font, &mut *parser, syntax::code_style);
        }

        if !self.plain {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    ..Default::default()
                },
                theme::BG,
            );
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: bounds.x + gutter_w - 1.0,
                        width: 1.0,
                        ..bounds
                    },
                    ..Default::default()
                },
                theme::HOVER,
            );
        }

        let hf = ed.hint_factor().unwrap_or(1.0);
        let cur = ed.cursor();
        let focused = state.editor.is_focused();
        let clip = bounds.intersection(viewport).unwrap_or(bounds);
        let text_clip = text_bounds
            .expand(iced::Padding {
                left: self.padding.left,
                ..Default::default()
            })
            .intersection(viewport)
            .unwrap_or(text_bounds);
        {
            let buffer = ed.buffer();
            let scroll = buffer.scroll();
            let mut last_line = usize::MAX;
            // Gutter and current-line band (not in the plain look).
            renderer.with_layer(clip, |renderer| {
                if self.plain {
                    return;
                }
                for run in buffer.layout_runs() {
                    let y = text_bounds.y + run.line_top / hf;
                    let h = run.line_height / hf;
                    let is_cursor_line = run.line_i == cur.position.line;
                    if is_cursor_line && cur.selection.is_none() {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle::new(
                                    Point::new(bounds.x + gutter_w, y),
                                    Size::new(bounds.width - gutter_w, h),
                                ),
                                ..Default::default()
                            },
                            theme::ACTIVE_LINE,
                        );
                    }
                    if run.line_i != last_line {
                        last_line = run.line_i;
                        renderer.fill_text(
                            text::Text {
                                content: (run.line_i + 1).to_string(),
                                bounds: Size::new(gutter_w, h),
                                size: Pixels(self.size),
                                line_height: LineHeight::Absolute(Pixels(self.line_height)),
                                font: theme::mono(),
                                align_x: text::Alignment::Right,
                                align_y: alignment::Vertical::Top,
                                shaping: text::Shaping::Basic,
                                wrapping: Wrapping::None,
                                ellipsis: text::Ellipsis::None,
                                hint_factor: renderer.hint_factor(),
                            },
                            Point::new(bounds.x + gutter_w - GUTTER_PAD, y),
                            if is_cursor_line && focused {
                                theme::LINE_NO_ACTIVE
                            } else {
                                theme::LINE_NO
                            },
                            clip,
                        );
                    }
                }
            });
            // Find matches, clipped to the text area.
            if !self.matches.is_empty() {
                renderer.with_layer(text_clip, |renderer| {
                    for run in buffer.layout_runs() {
                        let first = self.matches.partition_point(|m| m.line < run.line_i);
                        for (k, m) in self.matches[first..]
                            .iter()
                            .enumerate()
                            .take_while(|(_, m)| m.line == run.line_i)
                        {
                            let current = self.current_match == Some(first + k);
                            let a = cosmic_text::Cursor::new(m.line, m.start);
                            let b = cosmic_text::Cursor::new(m.line, m.end);
                            for (x, w) in run.highlight(a, b) {
                                let r = Rectangle::new(
                                    Point::new(
                                        text_bounds.x + (x - self.h_scroll(scroll.horizontal)) / hf,
                                        text_bounds.y + run.line_top / hf,
                                    ),
                                    Size::new(w / hf, run.line_height / hf),
                                );
                                renderer.fill_quad(
                                    renderer::Quad {
                                        bounds: r,
                                        border: iced::Border {
                                            color: theme::FIND_MATCH_BORDER,
                                            width: 1.0,
                                            radius: 2.0.into(),
                                        },
                                        ..Default::default()
                                    },
                                    if current {
                                        theme::FIND_CURRENT
                                    } else {
                                        theme::FIND_MATCH
                                    },
                                );
                            }
                        }
                    }
                });
            }
            // Chips: a pill behind each span (its padding is in the text), clipped to the
            // whole widget so the pill's ends are not cut at the text's edges.
            if !self.chips.is_empty() {
                let (fill, border) = self.chip_colors;
                renderer.with_layer(clip, |renderer| {
                    for run in buffer.layout_runs() {
                        let first = self.chips.partition_point(|m| m.line < run.line_i);
                        for m in self.chips[first..].iter().take_while(|m| m.line == run.line_i) {
                            let a = cosmic_text::Cursor::new(m.line, m.start);
                            let b = cosmic_text::Cursor::new(m.line, m.end);
                            for (x, w) in run.highlight(a, b) {
                                let h = run.line_height / hf;
                                let r = Rectangle::new(
                                    Point::new(
                                        text_bounds.x + (x - self.h_scroll(scroll.horizontal)) / hf,
                                        text_bounds.y + run.line_top / hf + 1.0,
                                    ),
                                    Size::new(w / hf, h - 2.0),
                                );
                                renderer.fill_quad(
                                    renderer::Quad {
                                        bounds: r,
                                        border: iced::Border {
                                            color: border,
                                            width: 1.0,
                                            radius: 4.0.into(),
                                        },
                                        ..Default::default()
                                    },
                                    fill,
                                );
                            }
                        }
                    }
                });
            }
        }

        // Chip labels: over each chip's first pill segment, vertically centred.
        if !self.chip_labels.is_empty() {
            let pills = self.span_rects(&ed, text_bounds, self.chips);
            renderer.with_layer(clip, |renderer| {
                let mut seen = Vec::new();
                for (k, r) in pills {
                    if seen.contains(&k) {
                        continue;
                    }
                    seen.push(k);
                    let Some(label) = self.chip_labels.get(k).filter(|l| !l.is_empty()) else {
                        continue;
                    };
                    renderer.fill_text(
                        text::Text {
                            content: label.clone(),
                            bounds: Size::new(r.width, r.height),
                            size: Pixels(self.chip_label_size),
                            line_height: LineHeight::Absolute(Pixels(r.height)),
                            font: self.font,
                            align_x: text::Alignment::Default,
                            align_y: alignment::Vertical::Center,
                            shaping: text::Shaping::Advanced,
                            wrapping: Wrapping::None,
                            ellipsis: text::Ellipsis::None,
                            hint_factor: renderer.hint_factor(),
                        },
                        Point::new(r.x + self.chip_label_inset, r.center_y()),
                        self.text_color,
                        clip,
                    );
                }
            });
        }

        // The close marks: an × centred in each mark's slot (the font's `×` sits low), with a
        // round spot behind the hovered one.
        if !self.chip_close.is_empty() {
            use iced::advanced::svg::{Renderer as _, Svg};
            let over = cursor.position_over(bounds);
            let rects = self.span_rects(&ed, text_bounds, self.chip_close);
            renderer.with_layer(clip, |renderer| {
                for (_, r) in rects {
                    let c = r.center();
                    let hovered = over.is_some_and(|p| r.expand(2.0).contains(p));
                    if hovered {
                        let d = (r.height - 4.0).min(r.width + 8.0);
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: Rectangle::new(
                                    Point::new(c.x - d / 2.0, c.y - d / 2.0),
                                    Size::new(d, d),
                                ),
                                border: iced::Border {
                                    radius: (d / 2.0).into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            self.chip_colors.1,
                        );
                    }
                    let s = 7.0;
                    let color = if hovered {
                        self.text_color
                    } else {
                        iced::Color {
                            a: 0.65,
                            ..self.text_color
                        }
                    };
                    renderer.draw_svg(
                        Svg::new(CLOSE_ICON.clone()).color(color),
                        Rectangle::new(Point::new(c.x - s / 2.0, c.y - s / 2.0), Size::new(s, s)),
                        clip,
                    );
                }
            });
        }

        // Placeholder of an empty text.
        // (`ed` is borrowed: read the buffer through it, not through `self.content`.)
        if let Some(p) = &self.placeholder
            && ed.buffer().lines.len() <= 1
            && ed.buffer().lines.first().is_none_or(|l| l.text().is_empty())
        {
            renderer.fill_text(
                text::Text {
                    content: p.clone(),
                    bounds: text_bounds.size(),
                    size: Pixels(self.size),
                    line_height: LineHeight::Absolute(Pixels(self.line_height)),
                    font: self.font,
                    align_x: text::Alignment::Default,
                    align_y: alignment::Vertical::Top,
                    shaping: text::Shaping::Advanced,
                    wrapping: Wrapping::None,
                    ellipsis: text::Ellipsis::End,
                    hint_factor: renderer.hint_factor(),
                },
                text_bounds.position(),
                self.placeholder_color,
                text_clip,
            );
        }

        renderer.with_layer(text_clip, |renderer| {
            state.editor.draw(
                &*ed,
                renderer,
                // The plain look wraps: iced's editor still nudges the view sideways (up to
                // its cursor width) at a line's right edge; undo that, it only clips the text.
                text_bounds.position()
                    + iced::Vector::new(
                        if self.plain { ed.buffer().scroll().horizontal / ed.hint_factor().unwrap_or(1.0) } else { 0.0 },
                        0.0,
                    ),
                text_clip,
                core_editor::Style {
                    value: self.text_color,
                    selection: theme::SELECTION,
                },
            );
        });

        if let Some(b) = bar {
            let color = if state.thumb_grab.is_some() {
                theme::SCROLL_THUMB_DRAG
            } else if cursor.is_over(b.track) {
                theme::SCROLL_THUMB_HOVER
            } else {
                theme::SCROLL_THUMB
            };
            renderer.with_layer(clip, |renderer| {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: b.thumb.shrink(iced::Padding {
                            left: 2.0,
                            right: 2.0,
                            top: 1.0,
                            bottom: 1.0,
                        }),
                        border: iced::Border {
                            radius: 3.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    color,
                );
            });
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let on_bar = self
            .scrollbar(layout.bounds())
            .is_some_and(|b| cursor.is_over(b.track));
        let on_close = cursor
            .position_over(layout.bounds())
            .is_some_and(|p| self.close_at(layout.bounds(), p).is_some());
        if on_bar || state.thumb_grab.is_some() {
            mouse::Interaction::default()
        } else if on_close {
            mouse::Interaction::Pointer
        } else if cursor.is_over(self.text_bounds(layout.bounds())) {
            if state.modifiers.command() && self.on_ctrl_click.is_some() {
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::Text
            }
        } else {
            mouse::Interaction::default()
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), layout.bounds(), &mut state.editor);
    }
}

fn apply_update<M>(
    update: core_editor::Update<M>,
    shell: &mut Shell<'_, M>,
    on_action: &dyn Fn(Action) -> M,
) {
    match update {
        core_editor::Update::Action(action) => {
            shell.publish(on_action(action));
            shell.capture_event();
        }
        core_editor::Update::Release => {}
        core_editor::Update::Custom(message) => {
            shell.publish(message);
            shell.capture_event();
        }
        core_editor::Update::Sequence(updates) => {
            for update in updates {
                apply_update(update, shell, on_action);
            }
        }
        core_editor::Update::Copy(content) => {
            shell.write_clipboard(clipboard::Content::Text(content));
            shell.capture_event();
        }
        core_editor::Update::Paste => {
            shell.read_clipboard(clipboard::Kind::Text);
            shell.capture_event();
        }
        core_editor::Update::RedrawAt(at) => shell.request_redraw_at(at),
        core_editor::Update::Focus
        | core_editor::Update::Unfocus
        | core_editor::Update::InputMethod => shell.request_redraw(),
    }
}

impl<'a, M: Clone + 'a> From<CodeEditor<'a, M>> for Element<'a, M> {
    fn from(w: CodeEditor<'a, M>) -> Self {
        Widget::<M, Theme, iced::Renderer>::_boxed(w)
    }
}
