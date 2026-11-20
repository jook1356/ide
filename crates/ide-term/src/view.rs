//! `TerminalView`: iced (master, advanced `Widget` API) widget showing a [`TermModel`].
//!
//! The widget never writes to a PTY: input bytes (keys, committed IME text, paste, focus
//! reports, alternate-scroll arrows) go out through [`TerminalView::on_input`] and grid size
//! changes through [`TerminalView::on_resize`]; the app forwards both to the server.
//!
//! Rendering: per row, runs of ASCII cells with the same style are one `fill_text` (Basic shaping)
//! at x = col * cell_width; every other char (Hangul, emoji, symbols) is drawn alone at its cell
//! with Advanced shaping (system font fallback), so wide chars stay on the 2-cell grid whatever the
//! fallback font's advance. Backgrounds, underline, strike, cursor and selection are quads (iced
//! draws a layer's quads below its text). Box-drawing/block chars are pixel-snapped quads.
//! IME: `Shell::request_input_method` with the cursor cell rect on every redraw while focused;
//! the preedit is drawn on-the-spot at the cursor in a top layer; keys are ignored while composing.
use std::time::Instant;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AColor, CursorShape};
use iced::advanced::clipboard;
use iced::advanced::mouse::{Click, click};
use iced::advanced::input_method::{self, InputMethod, Purpose};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{self, Id, Tree, Widget, operation, tree};
use iced::advanced::{Renderer as _, Shell};
use iced::keyboard::{self, Key, key::Named};
use iced::{Background, Color, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme, alignment, mouse, window};
use unicode_width::UnicodeWidthStr;

use crate::config::{Palette, TermConfig};
use crate::glyphs::builtin_glyph;
use crate::keys::key_bytes;
use crate::model::{ModelId, TermModel};

type Renderer = iced::Renderer;
type P = <Renderer as text::Renderer>::Paragraph;

/// Messages the widget can emit (through [`TerminalView::on_event`]).
#[derive(Debug, Clone, PartialEq)]
pub enum ViewEvent {
    /// Text was put on the clipboard (Ctrl+C with a selection / Ctrl+Shift+C).
    Copied(String),
    /// The widget gained keyboard focus (click inside or a focus operation).
    Focused,
    /// The widget lost keyboard focus (click outside or a focus operation).
    Unfocused,
}

/// Cell metrics in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Metrics {
    cw: f32,
    ch: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct MetricsKey {
    family: String,
    size: f32,
    line_height: f32,
    hint: Option<f32>,
}

struct State {
    model: ModelId,
    focused: bool,
    window_focused: bool,
    preedit: String,
    dragging: bool,
    paste_pending: bool,
    last_click: Option<Click>,
    metrics: Option<(MetricsKey, Metrics)>,
    last_ime: Option<Rectangle>,
    /// Grid size from the last layout, and the size last reported through `on_resize`.
    grid: Option<(u16, u16)>,
    reported: Option<(u16, u16)>,
}

impl State {
    fn new(model: ModelId, focused: bool) -> Self {
        State {
            model,
            focused,
            window_focused: true,
            preedit: String::new(),
            dragging: false,
            paste_pending: false,
            last_click: None,
            metrics: None,
            last_ime: None,
            grid: None,
            reported: None,
        }
    }
    fn m(&self) -> Option<Metrics> {
        self.metrics.as_ref().map(|(_, m)| *m)
    }
}

impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }
    fn focus(&mut self) {
        self.focused = true;
    }
    fn unfocus(&mut self) {
        self.focused = false;
        self.preedit.clear();
    }
}

/// The terminal widget. Create one per frame in `view`:
/// `TerminalView::new(&model, &config).on_input(Message::Input).on_resize(Message::Resize)`.
pub struct TerminalView<'a, Message> {
    model: TermModel,
    config: &'a TermConfig,
    on_input: Option<Box<dyn Fn(Vec<u8>) -> Message + 'a>>,
    on_resize: Option<Box<dyn Fn(u16, u16) -> Message + 'a>>,
    on_event: Option<Box<dyn Fn(ViewEvent) -> Message + 'a>>,
    autofocus: bool,
    id: Option<Id>,
    width: Length,
    height: Length,
}

impl<'a, Message> TerminalView<'a, Message> {
    pub fn new(model: &TermModel, config: &'a TermConfig) -> Self {
        TerminalView {
            model: model.clone(),
            config,
            on_input: None,
            on_resize: None,
            on_event: None,
            autofocus: true,
            id: None,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// Bytes for the terminal's input (keys, committed IME text, paste incl. bracketed-paste
    /// markers when the program enabled them, focus reports, Ctrl+C = 0x03, ...). Without it the
    /// widget is view-only.
    pub fn on_input(mut self, f: impl Fn(Vec<u8>) -> Message + 'a) -> Self {
        self.on_input = Some(Box::new(f));
        self
    }

    /// The widget's grid size (cols, rows) changed (also emitted once for a new widget state, e.g.
    /// on first layout or after switching models). The local model is already resized; forward
    /// it to the PTY owner.
    pub fn on_resize(mut self, f: impl Fn(u16, u16) -> Message + 'a) -> Self {
        self.on_resize = Some(Box::new(f));
        self
    }

    /// Receive [`ViewEvent`]s (copy, focus changes).
    pub fn on_event(mut self, f: impl Fn(ViewEvent) -> Message + 'a) -> Self {
        self.on_event = Some(Box::new(f));
        self
    }

    /// Whether the widget starts focused when its state is created (default: true).
    pub fn autofocus(mut self, focused: bool) -> Self {
        self.autofocus = focused;
        self
    }

    /// Widget id, for `iced::widget::operation::focus(id)`.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }

    fn emit(&self, shell: &mut Shell<'_, Message>, ev: ViewEvent) {
        if let Some(f) = &self.on_event {
            shell.publish(f(ev));
        }
    }

    fn input(&self, shell: &mut Shell<'_, Message>, bytes: Vec<u8>) {
        if let Some(f) = &self.on_input {
            shell.publish(f(bytes));
        }
    }

    fn font(&self, bold: bool, italic: bool) -> Font {
        Font {
            family: iced::font::Family::name(&self.config.font_family),
            weight: if bold { iced::font::Weight::Bold } else { iced::font::Weight::Normal },
            style: if italic { iced::font::Style::Italic } else { iced::font::Style::Normal },
            ..Font::DEFAULT
        }
    }

    fn measure(&self, key: &MetricsKey) -> Metrics {
        let s = "M".repeat(40);
        let p = P::with_text(text::Text {
            content: s.as_str(),
            bounds: Size::INFINITE,
            size: Pixels(key.size),
            line_height: text::LineHeight::Absolute(Pixels(key.line_height)),
            font: self.font(false, false),
            align_x: text::Alignment::Left,
            align_y: alignment::Vertical::Top,
            shaping: text::Shaping::Basic,
            wrapping: text::Wrapping::None,
            ellipsis: text::Ellipsis::None,
            hint_factor: key.hint,
        });
        let b = p.min_bounds();
        Metrics { cw: b.width / 40.0, ch: key.line_height.max(b.height.round()) }
    }

    fn grid_point(&self, m: Metrics, bounds: Rectangle, p: Point) -> (GPoint, Side) {
        let pad = self.config.padding;
        let g = self.model.grid_size();
        let x = (p.x - bounds.x - pad).max(0.0);
        let y = (p.y - bounds.y - pad).max(0.0);
        let col = ((x / m.cw) as usize).min(g.cols as usize - 1);
        let row = ((y / m.ch) as usize).min(g.rows as usize - 1);
        let side = if x - col as f32 * m.cw < m.cw / 2.0 { Side::Left } else { Side::Right };
        let offset = self.model.lock_term().grid().display_offset() as i32;
        (GPoint::new(Line(row as i32 - offset), Column(col)), side)
    }

    fn cursor_rect(&self, m: Metrics, bounds: Rectangle, preedit_cols: usize) -> Rectangle {
        let pad = self.config.padding;
        let term = self.model.lock_term();
        let c = term.grid().cursor.point;
        let offset = term.grid().display_offset() as i32;
        let row = (c.line.0 + offset).clamp(0, term.screen_lines() as i32 - 1);
        Rectangle::new(
            Point::new(bounds.x + pad + c.column.0 as f32 * m.cw, bounds.y + pad + row as f32 * m.ch),
            Size::new(m.cw * preedit_cols.max(1) as f32, m.ch),
        )
    }

    fn copy_selection(&self, shell: &mut Shell<'_, Message>) {
        let text = {
            let mut term = self.model.lock_term();
            let t = term.selection_to_string();
            term.selection = None;
            t
        };
        if let Some(t) = text.filter(|t| !t.is_empty()) {
            self.model.diagnostics().copies.push(t.clone());
            shell.write_clipboard(clipboard::Content::Text(t.clone()));
            self.emit(shell, ViewEvent::Copied(t));
        }
    }

    fn set_focus(&self, state: &mut State, focused: bool, shell: &mut Shell<'_, Message>) {
        if state.focused == focused {
            return;
        }
        state.focused = focused;
        if !focused {
            state.preedit.clear();
        }
        if self.model.lock_term().mode().contains(TermMode::FOCUS_IN_OUT) {
            self.input(shell, if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
        self.emit(shell, if focused { ViewEvent::Focused } else { ViewEvent::Unfocused });
        shell.request_redraw();
    }

    fn handle(&self, state: &mut State, event: &Event, bounds: Rectangle, cursor: mouse::Cursor, shell: &mut Shell<'_, Message>) {
        let Some(m) = state.m() else { return };
        match event {
            Event::Window(window::Event::RedrawRequested(_)) => {
                if state.focused {
                    let w = UnicodeWidthStr::width(state.preedit.as_str());
                    let rect = self.cursor_rect(m, bounds, w);
                    if state.last_ime != Some(rect) {
                        state.last_ime = Some(rect);
                        self.model.diagnostics().ime_rects.push(rect);
                    }
                    shell.request_input_method(&InputMethod::Enabled {
                        cursor: rect,
                        purpose: Purpose::Terminal,
                        preedit: None::<input_method::Preedit<&str>>,
                    });
                }
            }
            Event::Window(window::Event::Focused) => {
                state.window_focused = true;
                if state.focused && self.model.lock_term().mode().contains(TermMode::FOCUS_IN_OUT) {
                    self.input(shell, b"\x1b[I".to_vec());
                }
            }
            Event::Window(window::Event::Unfocused) => {
                // The preedit is not cleared here: the OS IME commits or cancels it and reports
                // that through `input_method::Event`s.
                state.window_focused = false;
                if state.focused && self.model.lock_term().mode().contains(TermMode::FOCUS_IN_OUT) {
                    self.input(shell, b"\x1b[O".to_vec());
                }
            }
            Event::InputMethod(ev) if state.focused => {
                match ev {
                    input_method::Event::Opened => {}
                    input_method::Event::Preedit(s, _) => state.preedit = s.clone(),
                    input_method::Event::Commit(s) => {
                        state.preedit.clear();
                        self.model.lock_term().scroll_display(Scroll::Bottom);
                        self.input(shell, s.as_bytes().to_vec());
                    }
                    input_method::Event::Closed => state.preedit.clear(),
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Clipboard(clipboard::Event::Read(result)) if state.paste_pending => {
                state.paste_pending = false;
                if let Ok(content) = result
                    && let clipboard::Content::Text(t) = content.as_ref()
                {
                    self.model.lock_term().scroll_display(Scroll::Bottom);
                    let bytes = self.model.paste_bytes(t);
                    self.input(shell, bytes);
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, text, .. }) if state.focused => {
                if !state.preedit.is_empty() {
                    // The IME owns keys while composing (Backspace edits the preedit, not the PTY).
                    shell.capture_event();
                    return;
                }
                let ctrl = modifiers.control();
                let shift = modifiers.shift();
                if let Key::Character(c) = key {
                    let c = c.to_lowercase();
                    if ctrl && c == "c" && (shift || self.model.lock_term().selection.is_some()) {
                        self.copy_selection(shell);
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                    if ctrl && c == "v" && !modifiers.alt() {
                        state.paste_pending = true;
                        shell.read_clipboard(clipboard::Kind::Text);
                        shell.capture_event();
                        return;
                    }
                }
                if shift && matches!(key, Key::Named(Named::PageUp | Named::PageDown)) {
                    let up = matches!(key, Key::Named(Named::PageUp));
                    self.model.lock_term().scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
                let app_cursor = self.model.lock_term().mode().contains(TermMode::APP_CURSOR);
                if let Some(bytes) = key_bytes(key, *modifiers, text.as_deref(), app_cursor) {
                    {
                        let mut term = self.model.lock_term();
                        term.selection = None;
                        term.scroll_display(Scroll::Bottom);
                    }
                    self.input(shell, bytes);
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(bounds) {
                    self.set_focus(state, true, shell);
                    let click = Click::new(p, mouse::Button::Left, state.last_click);
                    state.last_click = Some(click);
                    let (pt, side) = self.grid_point(m, bounds, p);
                    let ty = match click.kind() {
                        click::Kind::Single => SelectionType::Simple,
                        click::Kind::Double => SelectionType::Semantic,
                        click::Kind::Triple => SelectionType::Lines,
                    };
                    let mut sel = Selection::new(ty, pt, side);
                    if ty != SelectionType::Simple {
                        // Semantic/line selections cover the clicked word/line right away.
                        sel.update(pt, side);
                    }
                    self.model.lock_term().selection = Some(sel);
                    state.dragging = true;
                    shell.capture_event();
                    shell.request_redraw();
                } else if state.focused {
                    self.set_focus(state, false, shell);
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(p) = cursor.position() {
                    let (pt, side) = self.grid_point(m, bounds, p);
                    if let Some(sel) = self.model.lock_term().selection.as_mut() {
                        sel.update(pt, side);
                    }
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                let mut term = self.model.lock_term();
                if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                    term.selection = None;
                }
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y * self.config.wheel_lines,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / m.ch,
                };
                let n = lines.round() as i32;
                if n != 0 {
                    let mut term = self.model.lock_term();
                    let mode = *term.mode();
                    if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                        drop(term);
                        let seq: &[u8] = match (n > 0, mode.contains(TermMode::APP_CURSOR)) {
                            (true, true) => b"\x1bOA",
                            (true, false) => b"\x1b[A",
                            (false, true) => b"\x1bOB",
                            (false, false) => b"\x1b[B",
                        };
                        self.input(shell, seq.repeat(n.unsigned_abs() as usize));
                    } else {
                        term.scroll_display(Scroll::Delta(n));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Convenience constructor: `terminal_view(&model, &config)`.
pub fn terminal_view<'a, Message>(model: &TermModel, config: &'a TermConfig) -> TerminalView<'a, Message> {
    TerminalView::new(model, config)
}

fn dim(c: Color) -> Color {
    Color { r: c.r * 0.66, g: c.g * 0.66, b: c.b * 0.66, a: c.a }
}

fn resolve(c: &AColor, colors: &Colors, p: &Palette) -> Color {
    let rgb = |v: alacritty_terminal::vte::ansi::Rgb| Color::from_rgb8(v.r, v.g, v.b);
    match c {
        AColor::Spec(v) => rgb(*v),
        AColor::Indexed(i) => colors[*i as usize].map(rgb).unwrap_or_else(|| p.indexed(*i)),
        AColor::Named(n) => {
            let idx = *n as usize;
            if let Some(v) = colors[idx] {
                return rgb(v);
            }
            match idx {
                0..=15 => p.ansi[idx],
                256 => p.foreground,
                257 => p.background,
                258 => p.foreground,
                259..=266 => dim(p.ansi[idx - 259]),
                267 => p.ansi[15],
                268 => dim(p.foreground),
                _ => p.foreground,
            }
        }
    }
}

struct Run {
    col: usize,
    end: usize,
    text: String,
    fg: Color,
    bold: bool,
    italic: bool,
}

impl<Message> widget::Meta for TerminalView<'_, Message> {}

impl<Message> Widget<Message, Theme, Renderer> for TerminalView<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::new(self.model.id(), self.autofocus))
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.children.clear();
        // The same tree slot may show another model after a switch: start fresh (this also
        // re-reports the grid size for the new model).
        let state = tree.state.downcast_mut::<State>();
        if state.model != self.model.id() {
            let focused = state.focused;
            *state = State::new(self.model.id(), focused);
        }
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let key = MetricsKey {
            family: self.config.font_family.clone(),
            size: self.config.font_size,
            line_height: self.config.line_height,
            hint: renderer.hint_factor(),
        };
        let state = tree.state.downcast_mut::<State>();
        let m = match &state.metrics {
            Some((k, m)) if *k == key => *m,
            _ => {
                let m = self.measure(&key);
                state.metrics = Some((key, m));
                m
            }
        };
        let pad = self.config.padding;
        let intrinsic = Size::new(80.0 * m.cw + 2.0 * pad, 24.0 * m.ch + 2.0 * pad);
        let mut size = limits.resolve(self.width, self.height, intrinsic);
        if !size.width.is_finite() {
            size.width = intrinsic.width;
        }
        if !size.height.is_finite() {
            size.height = intrinsic.height;
        }
        let cols = ((size.width - 2.0 * pad) / m.cw).floor().max(2.0) as u16;
        let rows = ((size.height - 2.0 * pad) / m.ch).floor().max(1.0) as u16;
        self.model.resize_cells(cols, rows, m.cw, m.ch);
        state.grid = Some((cols, rows));
        tree.size = size;
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout, _viewport: &Rectangle, _renderer: &Renderer, operation: &mut dyn widget::Operation) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), layout.bounds(), state);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        // `layout` cannot publish messages: report a changed grid size on the next event (a
        // relayout is always followed by a redraw).
        if state.grid != state.reported
            && let Some((cols, rows)) = state.grid
            && let Some(f) = &self.on_resize
        {
            state.reported = state.grid;
            shell.publish(f(cols, rows));
        }
        let injected: Vec<_> = self.model.0.inject.lock().unwrap().drain(..).collect();
        for inj in injected {
            let (ev, cur) = match inj.pos {
                Some(p) => {
                    let abs = Point::new(bounds.x + p.x, bounds.y + p.y);
                    let ev = match inj.event {
                        Event::Mouse(mouse::Event::CursorMoved { .. }) => Event::Mouse(mouse::Event::CursorMoved { position: abs }),
                        e => e,
                    };
                    (ev, mouse::Cursor::Available(abs))
                }
                None => (inj.event, cursor),
            };
            self.handle(state, &ev, bounds, cur, shell);
        }
        self.handle(state, event, bounds, cursor, shell);
    }

    fn mouse_interaction(&self, _t: &Tree, layout: Layout, cursor: mouse::Cursor, _v: &Rectangle, _r: &Renderer) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) { mouse::Interaction::Text } else { mouse::Interaction::None }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let t0 = Instant::now();
        let state = tree.state.downcast_ref::<State>();
        let Some(m) = state.m() else { return };
        let cfg = self.config;
        let pal = &cfg.palette;
        let bounds = layout.bounds();
        let clip = bounds;
        let scale = renderer.scale().map(|s| s.total()).unwrap_or(1.0);
        let hint = state.metrics.as_ref().and_then(|(k, _)| k.hint);
        renderer.fill_quad(Quad { bounds, ..Quad::default() }, Background::Color(pal.background));
        let ox = bounds.x + cfg.padding;
        let oy = bounds.y + cfg.padding;
        let fonts = [self.font(false, false), self.font(true, false), self.font(false, true), self.font(true, true)];
        let font_of = |bold: bool, italic: bool| fonts[bold as usize + 2 * italic as usize];
        let probe = {
            let d = self.model.diagnostics();
            if d.probe_hit.is_none() { d.probe.clone() } else { None }
        };
        let mut probe_hit = false;
        let mut row_text = String::new();

        self.model.mark_drawn();
        let term = self.model.lock_term();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let sel = content.selection;
        let cursor = content.cursor;

        let draw_text = |renderer: &mut Renderer, s: String, col: usize, row: usize, cols: usize, fg: Color, bold: bool, italic: bool, advanced: bool| {
            renderer.fill_text(
                text::Text {
                    content: s,
                    bounds: Size::new(m.cw * (cols as f32 + 1.0), m.ch),
                    size: Pixels(cfg.font_size),
                    line_height: text::LineHeight::Absolute(Pixels(m.ch)),
                    font: font_of(bold, italic),
                    align_x: text::Alignment::Left,
                    align_y: alignment::Vertical::Top,
                    shaping: if advanced { text::Shaping::Advanced } else { text::Shaping::Basic },
                    wrapping: text::Wrapping::None,
                    ellipsis: text::Ellipsis::None,
                    hint_factor: hint,
                },
                Point::new(ox + col as f32 * m.cw, oy + row as f32 * m.ch),
                fg,
                clip,
            );
        };
        let flush = |renderer: &mut Renderer, run: &mut Option<Run>, row: usize| {
            if let Some(r) = run.take() {
                let t = r.text.trim_end().to_string();
                if !t.is_empty() {
                    let n = t.chars().count();
                    draw_text(renderer, t, r.col, row, n, r.fg, r.bold, r.italic, false);
                }
            }
        };
        let fill = |renderer: &mut Renderer, x: f32, y: f32, w: f32, h: f32, c: Color| {
            renderer.fill_quad(Quad { bounds: Rectangle::new(Point::new(x, y), Size::new(w, h)), ..Quad::default() }, Background::Color(c));
        };
        let fill_cells = |renderer: &mut Renderer, col: usize, row: usize, ncols: usize, c: Color| {
            fill(renderer, ox + col as f32 * m.cw, oy + row as f32 * m.ch, ncols as f32 * m.cw, m.ch, c);
        };

        let mut cur_row = usize::MAX;
        let mut run: Option<Run> = None;
        let mut bg_run: Option<(usize, usize, Color)> = None;
        for cell in content.display_iter {
            let row = (cell.point.line.0 + offset) as usize;
            if row != cur_row {
                flush(renderer, &mut run, cur_row);
                if let Some((a, b, c)) = bg_run.take() {
                    fill_cells(renderer, a, cur_row, b - a, c);
                }
                if let Some(p) = &probe
                    && row_text.contains(p.as_str())
                {
                    probe_hit = true;
                }
                row_text.clear();
                cur_row = row;
            }
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let col = cell.point.column.0;
            let w = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            let mut fg = resolve(&cell.fg, colors, pal);
            let mut bg = resolve(&cell.bg, colors, pal);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg = dim(fg);
            }
            let selected = sel.is_some_and(|s| {
                s.contains(cell.point) || (w == 2 && s.contains(GPoint::new(cell.point.line, cell.point.column + 1)))
            });
            if selected {
                bg = pal.selection;
            }
            let want_bg = if bg != pal.background { Some(bg) } else { None };
            match (&mut bg_run, want_bg) {
                (Some((_, end, c)), Some(b)) if *end == col && *c == b => *end = col + w,
                (_, wb) => {
                    if let Some((a, b, c)) = bg_run.take() {
                        fill_cells(renderer, a, row, b - a, c);
                    }
                    if let Some(b) = wb {
                        bg_run = Some((col, col + w, b));
                    }
                }
            }
            if probe.is_some() {
                row_text.push(cell.c);
            }
            let x0 = ox + col as f32 * m.cw;
            let y0 = oy + row as f32 * m.ch;
            if flags.intersects(Flags::ALL_UNDERLINES) {
                fill(renderer, x0, y0 + m.ch - 2.0, w as f32 * m.cw, 1.0, fg);
            }
            if flags.contains(Flags::STRIKEOUT) {
                fill(renderer, x0, y0 + 0.5 * m.ch, w as f32 * m.cw, 1.0, fg);
            }
            let c = cell.c;
            let hidden = flags.contains(Flags::HIDDEN);
            let bold = flags.contains(Flags::BOLD);
            let italic = flags.contains(Flags::ITALIC);
            if !hidden && let Some(rects) = builtin_glyph(c, m.cw, m.ch) {
                flush(renderer, &mut run, row);
                let cwid = w as f32 * m.cw;
                let snap = |v: f32| (v * scale).round() / scale;
                for (fx, fy, fw, fh, a) in rects {
                    // Snap both edges to physical pixels: neighbours share an edge => no seams and
                    // no double-blended overlap for shades.
                    let x = snap(x0 + fx * cwid);
                    let y = snap(y0 + fy * m.ch);
                    let ww = (snap(x0 + (fx + fw) * cwid) - x).max(1.0 / scale);
                    let hh = (snap(y0 + (fy + fh) * m.ch) - y).max(1.0 / scale);
                    fill(renderer, x, y, ww, hh, Color { a: fg.a * a, ..fg });
                }
                continue;
            }
            if c == ' ' || c == '\t' || hidden {
                if let Some(r) = run.as_mut()
                    && r.end == col
                {
                    r.text.push(' ');
                    r.end += 1;
                }
                continue;
            }
            let simple = c.is_ascii() && cell.zerowidth().is_none();
            if simple {
                if let Some(r) = run.as_mut()
                    && r.end == col
                    && r.fg == fg
                    && r.bold == bold
                    && r.italic == italic
                {
                    r.text.push(c);
                    r.end += 1;
                    continue;
                }
                flush(renderer, &mut run, row);
                run = Some(Run { col, end: col + 1, text: c.to_string(), fg, bold, italic });
            } else {
                flush(renderer, &mut run, row);
                let mut s = c.to_string();
                if let Some(z) = cell.zerowidth() {
                    s.extend(z.iter());
                }
                draw_text(renderer, s, col, row, w, fg, bold, italic, true);
            }
        }
        flush(renderer, &mut run, cur_row);
        if let Some((a, b, c)) = bg_run.take() {
            fill_cells(renderer, a, cur_row, b - a, c);
        }
        if let Some(p) = &probe
            && row_text.contains(p.as_str())
        {
            probe_hit = true;
        }

        // Cursor (hidden while composing: the preedit marks the position).
        let crow = cursor.point.line.0 + offset;
        if state.preedit.is_empty() && cursor.shape != CursorShape::Hidden && crow >= 0 {
            let x = ox + cursor.point.column.0 as f32 * m.cw;
            let y = oy + crow as f32 * m.ch;
            let wide = term.grid()[cursor.point].flags.contains(Flags::WIDE_CHAR);
            let cw = if wide { 2.0 * m.cw } else { m.cw };
            let focused = state.focused && state.window_focused;
            match (cursor.shape, focused) {
                (CursorShape::Beam, _) => fill(renderer, x, y, 2.0, m.ch, pal.cursor),
                (CursorShape::Underline, _) => fill(renderer, x, y + m.ch - 2.0, cw, 2.0, pal.cursor),
                (_, true) => fill(renderer, x, y, cw, m.ch, pal.cursor),
                (_, false) => {
                    // Unfocused: hollow block.
                    fill(renderer, x, y, cw, 1.0, pal.cursor);
                    fill(renderer, x, y + m.ch - 1.0, cw, 1.0, pal.cursor);
                    fill(renderer, x, y, 1.0, m.ch, pal.cursor);
                    fill(renderer, x + cw - 1.0, y, 1.0, m.ch, pal.cursor);
                }
            }
        }
        drop(term);

        // IME preedit (on-the-spot) in a layer above the grid text.
        if !state.preedit.is_empty() && crow >= 0 {
            let x = ox + cursor.point.column.0 as f32 * m.cw;
            let y = oy + crow as f32 * m.ch;
            let w = UnicodeWidthStr::width(state.preedit.as_str()) as f32 * m.cw;
            let r = Rectangle::new(Point::new(x, y), Size::new(w + 2.0, m.ch));
            renderer.with_layer(r, |renderer| {
                fill(renderer, r.x, r.y, r.width, r.height, pal.preedit_background);
                fill(renderer, x, y + m.ch - 2.0, w, 2.0, pal.preedit_underline);
                renderer.fill_text(
                    text::Text {
                        content: state.preedit.clone(),
                        bounds: Size::new(w + m.cw, m.ch),
                        size: Pixels(cfg.font_size),
                        line_height: text::LineHeight::Absolute(Pixels(m.ch)),
                        font: font_of(false, false),
                        align_x: text::Alignment::Left,
                        align_y: alignment::Vertical::Top,
                        shaping: text::Shaping::Advanced,
                        wrapping: text::Wrapping::None,
                        ellipsis: text::Ellipsis::None,
                        hint_factor: hint,
                    },
                    Point::new(x, y),
                    pal.foreground,
                    r,
                );
            });
        }

        let mut diag = self.model.diagnostics();
        if probe_hit && diag.probe_hit.is_none() {
            diag.probe_hit = Some(Instant::now());
        }
        if diag.measure {
            diag.frames.push((t0, t0.elapsed().as_secs_f32() * 1000.0));
        }
    }
}
