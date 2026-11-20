//! [`WebViewWidget`]: draws a [`WebView`] with iced's wgpu primitive pipeline and forwards
//! mouse, keyboard, focus and IME input to CEF.

use cef::{
    CompositionUnderline, CompositionUnderlineStyle, CursorType, ImplBrowserHost as _,
    MouseButtonType, MouseEvent, Range,
};
use iced::advanced::input_method::{self, InputMethod};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget, tree};
use iced::advanced::{Shell, mouse};
use iced::keyboard::{self, Modifiers};
use iced::widget::shader::{self, Shader};
use iced::{Element, Event, Length, Point, Rectangle, Size, window};
use std::sync::Arc;
use std::sync::atomic::Ordering::*;
use std::time::{Duration, Instant};

use crate::gpu::{PendingResize, PhysRect, WebPrimitive, dip_for, px_for};
use crate::input::{self, EF_LEFT_MOUSE, EF_MIDDLE_MOUSE, EF_RIGHT_MOUSE};
use crate::view::{ViewShared, WebView};

/// An iced widget showing one [`WebView`]. Build it in `view()` every frame (it is cheap):
/// `WebViewWidget::new(&self.web).width(Fill).height(Fill)`.
///
/// Behaviour: click focuses the page, a click outside blurs it; the wheel scrolls the page while
/// the cursor is over it (the event is captured so outer scrollables don't also scroll); keys and
/// IME go to the page while it is focused. The texture is clipped by parent scrollables and
/// covered by later `stack` layers like any iced primitive.
///
/// The frame is drawn pixel-exact: CEF renders exactly the widget's physical pixel rectangle at
/// iced's scale factor and every screen pixel shows one frame pixel (no resampling). While a
/// resize is in flight the previous frame keeps its size (anchored top-left, the rest filled
/// with [`crate::ViewOptions::background`]) instead of being stretched.
pub struct WebViewWidget {
    view: Arc<ViewShared>,
    program: WebProgram,
    width: Length,
    height: Length,
    dimmed: bool,
}

/// Default colour multiplier of [`WebViewWidget::dimmed`].
pub const DEFAULT_DIM: f32 = 0.45;

impl WebViewWidget {
    pub fn new(view: &WebView) -> Self {
        let view = view.shared().clone();
        Self {
            program: WebProgram {
                view: view.clone(),
                dim: 1.0,
            },
            view,
            width: Length::Fill,
            height: Length::Fill,
            dimmed: false,
        }
    }

    /// Shows a frozen, dimmed snapshot of the page instead of the live page, e.g. under a modal
    /// dialog: the last frame is drawn darkened ([`DEFAULT_DIM`]), input is not forwarded (an
    /// IME composition is finished and CEF loses focus), and CEF is told it is hidden so it
    /// stops rendering. `dimmed(false)` resumes the live page (and gives focus back if the page
    /// had it).
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self.program.dim = if dimmed { DEFAULT_DIM } else { 1.0 };
        self
    }

    /// Like [`WebViewWidget::dimmed`] with a custom colour multiplier (0 = black, 1 = undimmed).
    pub fn dimmed_by(mut self, factor: f32) -> Self {
        self.dimmed = true;
        self.program.dim = factor.clamp(0.0, 1.0);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

/// Shorthand for [`WebViewWidget::new`].
pub fn web_view(view: &WebView) -> WebViewWidget {
    WebViewWidget::new(view)
}

/// Draws through iced's shader widget so the primitive reaches iced's wgpu renderer through a
/// public bound (`Shader: Widget`), without naming iced_wgpu's renderer trait.
pub(crate) struct WebProgram {
    view: Arc<ViewShared>,
    dim: f32,
}

impl<Message> shader::Program<Message> for WebProgram {
    type State = State;
    type Primitive = WebPrimitive;

    fn draw(&self, _state: &State, _cursor: mouse::Cursor, _bounds: Rectangle) -> WebPrimitive {
        WebPrimitive {
            view: self.view.clone(),
            dim: self.dim,
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    hovered: bool,
    /// CEF button flags currently held (pressed inside the view).
    buttons: u32,
    modifiers: Modifiers,
    last_pos: Point,
    last_click: Option<(Instant, Point, mouse::Button, i32)>,
    /// The view this state belongs to. The app may show another view in the same tree slot
    /// (e.g. one web view per session in one detail area): the state is then reset and the
    /// new view gets the waker.
    view: Option<crate::ViewId>,
    window_focused: bool,
}

fn button_info(b: mouse::Button) -> Option<(MouseButtonType, u32)> {
    match b {
        mouse::Button::Left => Some((MouseButtonType::LEFT, EF_LEFT_MOUSE)),
        mouse::Button::Middle => Some((MouseButtonType::MIDDLE, EF_MIDDLE_MOUSE)),
        mouse::Button::Right => Some((MouseButtonType::RIGHT, EF_RIGHT_MOUSE)),
        _ => None,
    }
}

fn mouse_event(p: Point, flags: u32) -> MouseEvent {
    MouseEvent {
        x: p.x.floor() as i32,
        y: p.y.floor() as i32,
        modifiers: flags,
    }
}

/// Gives/removes CEF focus; removing it finishes an active composition (commits it, like Chrome).
pub(crate) fn set_focus(sh: &ViewShared, focused: bool) {
    let mut input = sh.input.lock().unwrap();
    if input.focused == focused {
        return;
    }
    input.focused = focused;
    let host = sh.host();
    if !focused {
        if input.composing.take().is_some() {
            if let Some(h) = &host {
                h.ime_finish_composing_text(0);
            }
            input.ime_reset = true;
        }
        input.ime_caret = None;
    }
    drop(input);
    if let Some(h) = host {
        h.set_focus(focused as _);
    }
    sh.wake();
}

/// Called from `Primitive::prepare` (the frame that first shows the bounds) with the widget's
/// physical rectangle and iced's scale factor: sizes CEF's view so its frame is exactly
/// `rect`'s pixel size (up to < 1 DIP of overflow when `px / scale` is fractional).
///
/// `legacy`: the logical bounds when the diagnostic pre-fix presentation is on (the view is then
/// sized to the rounded logical bounds, as before).
pub(crate) fn sync_geometry(
    sh: &ViewShared,
    rect: PhysRect,
    scale: f32,
    frame: u64,
    legacy: Option<&iced::Rectangle>,
) {
    let scale = scale.max(0.25);
    sh.widget_px
        .store(((rect.width as u64) << 32) | rect.height as u64, Relaxed);
    let scale_changed = sh.scale_bits.swap(scale.to_bits(), Relaxed) != scale.to_bits();
    let (w, h) = match legacy {
        Some(b) => (
            b.width.round().max(1.0) as u32,
            b.height.round().max(1.0) as u32,
        ),
        None => (dip_for(rect.width, scale), dip_for(rect.height, scale)),
    };
    let size_changed = (sh.width.swap(w, Relaxed), sh.height.swap(h, Relaxed)) != (w, h);
    if !(scale_changed || size_changed) {
        return;
    }
    {
        let mut pending = sh.resize_pending.lock().unwrap();
        if pending.len() >= 32 {
            pending.remove(0);
        }
        pending.push(PendingResize {
            at: Instant::now(),
            frame,
            expect_px: (px_for(w, scale), px_for(h, scale)),
        });
    }
    if let Some(host) = sh.host() {
        if scale_changed {
            host.notify_screen_info_changed();
        }
        host.was_resized();
    }
}

/// Enters/leaves the frozen (dimmed snapshot) state of [`WebViewWidget::dimmed`].
fn sync_frozen(sh: &ViewShared, frozen: bool) {
    if sh.frozen.swap(frozen, SeqCst) == frozen {
        return;
    }
    let host = sh.host();
    let mut input = sh.input.lock().unwrap();
    if frozen {
        if input.composing.take().is_some()
            && let Some(h) = &host
        {
            h.ime_finish_composing_text(0);
        }
        input.ime_caret = None;
        drop(input);
        if let Some(h) = &host {
            h.set_focus(0);
        }
    } else {
        let focused = input.focused;
        drop(input);
        if focused && let Some(h) = &host {
            h.set_focus(1);
        }
    }
    sh.apply_hidden();
    sh.wake();
}

fn set_composition(host: &cef::BrowserHost, text: &str, selection: Option<std::ops::Range<usize>>) {
    let n = text.encode_utf16().count() as u32;
    // iced gives a byte range into `text`; CEF wants UTF-16 offsets.
    let to_u16 = |byte: usize| {
        text.get(..byte)
            .map(|s| s.encode_utf16().count() as u32)
            .unwrap_or(n)
    };
    let (from, to) = selection
        .map(|r| (to_u16(r.start), to_u16(r.end)))
        .unwrap_or((n, n));
    let underline = CompositionUnderline {
        range: Range { from: 0, to: n },
        color: 0xFF00_0000,
        background_color: 0,
        thick: 0,
        style: CompositionUnderlineStyle::SOLID,
        ..Default::default()
    };
    host.ime_set_composition(
        Some(&text.into()),
        Some(&[underline]),
        Some(&Range {
            from: u32::MAX,
            to: u32::MAX,
        }),
        Some(&Range { from, to }),
    );
}

impl WebViewWidget {
    fn handle(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, impl Sized>,
    ) {
        let sh = &self.view;
        match event {
            Event::Waken => {
                if sh.frame_seq.load(Acquire) != sh.drawn_seq.load(Acquire)
                    || sh.dump_requested.load(Acquire)
                    || sh.input.lock().unwrap().focused
                {
                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::RedrawRequested(_)) => {
                // (The view is sized to the widget in `Primitive::prepare`.)
                let mut input = sh.input.lock().unwrap();
                if input.ime_reset {
                    // Leave the IME disabled for this frame so the OS drops its composition.
                    input.ime_reset = false;
                    shell.request_redraw();
                } else if input.focused && state.window_focused {
                    let caret = if input.composing.is_some() {
                        input.ime_caret.or(input.probe_caret)
                    } else {
                        input.probe_caret.or(input.ime_caret)
                    }
                    .unwrap_or(Rectangle {
                        x: state.last_pos.x,
                        y: state.last_pos.y,
                        width: 1.0,
                        height: 18.0,
                    });
                    let cursor = Rectangle {
                        x: bounds.x + caret.x,
                        y: bounds.y + caret.y,
                        ..caret
                    };
                    shell.request_input_method(&InputMethod::<&str>::Enabled {
                        cursor,
                        purpose: input_method::Purpose::Normal,
                        // CEF draws the composition inline; iced must not draw its own preedit.
                        preedit: None,
                    });
                }
            }
            Event::Window(window::Event::Focused) => {
                state.window_focused = true;
                if sh.input.lock().unwrap().focused
                    && let Some(h) = sh.host()
                {
                    h.set_focus(1);
                }
            }
            Event::Window(window::Event::Unfocused) => {
                state.window_focused = false;
                let mut input = sh.input.lock().unwrap();
                if input.composing.take().is_some()
                    && let Some(h) = sh.host()
                {
                    h.ime_finish_composing_text(0);
                }
                drop(input);
                if let Some(h) = sh.host() {
                    h.set_focus(0);
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let held = state.buttons & (EF_LEFT_MOUSE | EF_MIDDLE_MOUSE | EF_RIGHT_MOUSE) != 0;
                if let Some(p) = cursor.position_in(bounds) {
                    state.hovered = true;
                    state.last_pos = p;
                    if let Some(h) = sh.host() {
                        h.send_mouse_move_event(
                            Some(&mouse_event(
                                p,
                                input::modifier_flags(state.modifiers) | state.buttons,
                            )),
                            0,
                        );
                    }
                } else if held {
                    // Dragging (text selection) continues outside the view.
                    if let Some(p) = cursor.observe().position_from(bounds.position()) {
                        state.last_pos = p;
                        if let Some(h) = sh.host() {
                            h.send_mouse_move_event(
                                Some(&mouse_event(
                                    p,
                                    input::modifier_flags(state.modifiers) | state.buttons,
                                )),
                                0,
                            );
                        }
                    }
                } else if state.hovered {
                    state.hovered = false;
                    if let Some(h) = sh.host() {
                        h.send_mouse_move_event(
                            Some(&mouse_event(
                                state.last_pos,
                                input::modifier_flags(state.modifiers),
                            )),
                            1,
                        );
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if state.hovered && state.buttons == 0 {
                    state.hovered = false;
                    if let Some(h) = sh.host() {
                        h.send_mouse_move_event(Some(&mouse_event(state.last_pos, 0)), 1);
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let Some(p) = cursor.position_in(bounds) else {
                    if sh.input.lock().unwrap().focused {
                        set_focus(sh, false);
                    }
                    return;
                };
                let Some((btn, flag)) = button_info(*button) else {
                    return;
                };
                state.last_pos = p;
                {
                    // A click during composition commits it where it is (Chrome behaviour) and
                    // resets the OS IME; its late commit of the same text is dropped.
                    let mut input = sh.input.lock().unwrap();
                    if let Some(text) = input.composing.take() {
                        if let Some(h) = sh.host() {
                            h.ime_finish_composing_text(0);
                        }
                        input.drop_commit = Some(text);
                        input.ime_reset = true;
                    }
                }
                set_focus(sh, true);
                let now = Instant::now();
                let count = match state.last_click {
                    Some((t, lp, lb, c))
                        if lb == *button
                            && now - t < Duration::from_millis(500)
                            && (lp.x - p.x).abs() < 4.0
                            && (lp.y - p.y).abs() < 4.0 =>
                    {
                        (c % 3) + 1
                    }
                    _ => 1,
                };
                state.last_click = Some((now, p, *button, count));
                state.buttons |= flag;
                if let Some(h) = sh.host() {
                    h.send_mouse_click_event(
                        Some(&mouse_event(
                            p,
                            input::modifier_flags(state.modifiers) | state.buttons,
                        )),
                        btn,
                        0,
                        count,
                    );
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                let Some((btn, flag)) = button_info(*button) else {
                    return;
                };
                if state.buttons & flag == 0 {
                    return;
                }
                state.buttons &= !flag;
                let p = cursor
                    .observe()
                    .position_from(bounds.position())
                    .unwrap_or(state.last_pos);
                let count = state.last_click.map(|c| c.3).unwrap_or(1);
                if let Some(h) = sh.host() {
                    h.send_mouse_click_event(
                        Some(&mouse_event(
                            p,
                            input::modifier_flags(state.modifiers) | state.buttons,
                        )),
                        btn,
                        1,
                        count,
                    );
                }
                if cursor.is_over(bounds) {
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(p) = cursor.position_in(bounds) else {
                    return;
                };
                let (dx, dy) = match delta {
                    // One notch = WHEEL_DELTA (120) like native Windows wheel messages.
                    mouse::ScrollDelta::Lines { x, y } => (x * 120.0, y * 120.0),
                    mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                if let Some(h) = sh.host() {
                    h.send_mouse_wheel_event(
                        Some(&mouse_event(p, input::modifier_flags(state.modifiers))),
                        dx as i32,
                        dy as i32,
                    );
                }
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = *m;
            }
            Event::Keyboard(
                ev @ (keyboard::Event::KeyPressed { .. } | keyboard::Event::KeyReleased { .. }),
            ) => {
                let input = sh.input.lock().unwrap();
                if !input.focused {
                    return;
                }
                let key = match ev {
                    keyboard::Event::KeyPressed { key, .. }
                    | keyboard::Event::KeyReleased { key, .. } => key,
                    _ => unreachable!(),
                };
                if input::is_ime_processed(key) {
                    shell.capture_event();
                    return;
                }
                drop(input);
                if let keyboard::Event::KeyPressed { modifiers, .. } = ev {
                    state.modifiers = *modifiers;
                }
                if let Some(h) = sh.host() {
                    input::send_key(&h, ev);
                }
                shell.capture_event();
            }
            Event::InputMethod(ime) => {
                let mut input = sh.input.lock().unwrap();
                if !input.focused {
                    return;
                }
                let Some(host) = sh.host() else { return };
                match ime {
                    input_method::Event::Opened => {}
                    input_method::Event::Preedit(text, selection) => {
                        if text.is_empty() {
                            if input.composing.take().is_some() {
                                host.ime_cancel_composition();
                            }
                        } else {
                            input.drop_commit = None;
                            set_composition(&host, text, selection.clone());
                            input.composing = Some(text.clone());
                        }
                    }
                    input_method::Event::Commit(text) => {
                        if input.drop_commit.take().as_deref() == Some(text.as_str()) {
                            tracing::debug!(
                                "dropping late IME commit {text:?} (already committed by click)"
                            );
                        } else {
                            host.ime_commit_text(
                                Some(&text.as_str().into()),
                                Some(&Range {
                                    from: u32::MAX,
                                    to: u32::MAX,
                                }),
                                0,
                            );
                        }
                        input.composing = None;
                    }
                    input_method::Event::Closed => {
                        if input.composing.take().is_some() {
                            host.ime_finish_composing_text(0);
                        }
                    }
                }
                shell.capture_event();
                shell.request_redraw();
            }
            _ => {}
        }
    }
}

impl widget::Meta for WebViewWidget {}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for WebViewWidget
where
    Renderer: renderer::Renderer,
    for<'x> Shader<Message, &'x WebProgram>: Widget<Message, Theme, Renderer>,
{
    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            window_focused: true,
            ..State::default()
        })
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) {
        tree.size = layout::atomic(limits, self.width, self.height);
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
        if state.view != Some(self.view.id) {
            *state = State {
                view: Some(self.view.id),
                window_focused: state.window_focused,
                ..State::default()
            };
            *self.view.waker.lock().unwrap() = Some(shell.waker().clone());
        }
        sync_frozen(&self.view, self.dimmed);
        if self.dimmed {
            // Frozen snapshot: nothing reaches the page; a drag in progress ends.
            match event {
                Event::Waken
                    if self.view.frame_seq.load(Acquire) != self.view.drawn_seq.load(Acquire)
                        || self.view.dump_requested.load(Acquire) =>
                {
                    shell.request_redraw()
                }
                Event::Window(window::Event::Focused) => state.window_focused = true,
                Event::Window(window::Event::Unfocused) => state.window_focused = false,
                _ => {}
            }
            if state.buttons != 0 || state.hovered {
                if let Some(h) = self.view.host() {
                    for (btn, flag) in [
                        (MouseButtonType::LEFT, EF_LEFT_MOUSE),
                        (MouseButtonType::MIDDLE, EF_MIDDLE_MOUSE),
                        (MouseButtonType::RIGHT, EF_RIGHT_MOUSE),
                    ] {
                        if state.buttons & flag != 0 {
                            h.send_mouse_click_event(
                                Some(&mouse_event(state.last_pos, 0)),
                                btn,
                                1,
                                1,
                            );
                        }
                    }
                    h.send_mouse_move_event(Some(&mouse_event(state.last_pos, 0)), 1);
                }
                state.buttons = 0;
                state.hovered = false;
            }
            return;
        }
        self.handle(state, event, layout.bounds(), cursor, shell);
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if !self.dimmed && cursor.is_over(layout.bounds()) {
            interaction(self.view.cursor.load(Relaxed))
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // `Shader::draw` reads `tree.state` as `Program::State` (= our `State`) and calls
        // `renderer.draw_primitive(bounds, program.draw(..))`.
        *self.view.last_bounds.lock().unwrap() = Some(layout.bounds());
        sync_frozen(&self.view, self.dimmed);
        let shader = Shader::new(&self.program);
        Widget::<Message, Theme, Renderer>::draw(
            &shader, tree, renderer, theme, style, layout, cursor, viewport,
        );
    }
}

impl<'a, Message, Theme, Renderer> From<WebViewWidget> for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
    for<'x> Shader<Message, &'x WebProgram>: Widget<Message, Theme, Renderer>,
{
    fn from(w: WebViewWidget) -> Self {
        Widget::<Message, Theme, Renderer>::_boxed(w)
    }
}

// ------------------------------------------------------------- cursor shapes

const CURSORS: &[(CursorType, mouse::Interaction)] = &[
    (CursorType::POINTER, mouse::Interaction::Idle),
    (CursorType::HAND, mouse::Interaction::Pointer),
    (CursorType::IBEAM, mouse::Interaction::Text),
    (CursorType::CROSS, mouse::Interaction::Crosshair),
    (CursorType::WAIT, mouse::Interaction::Wait),
    (CursorType::HELP, mouse::Interaction::Help),
    (CursorType::PROGRESS, mouse::Interaction::Progress),
    (CursorType::MOVE, mouse::Interaction::Move),
    (CursorType::NOTALLOWED, mouse::Interaction::NotAllowed),
    (CursorType::NODROP, mouse::Interaction::NoDrop),
    (CursorType::GRAB, mouse::Interaction::Grab),
    (CursorType::GRABBING, mouse::Interaction::Grabbing),
    (
        CursorType::EASTWESTRESIZE,
        mouse::Interaction::ResizingHorizontally,
    ),
    (
        CursorType::EASTRESIZE,
        mouse::Interaction::ResizingHorizontally,
    ),
    (
        CursorType::WESTRESIZE,
        mouse::Interaction::ResizingHorizontally,
    ),
    (
        CursorType::NORTHSOUTHRESIZE,
        mouse::Interaction::ResizingVertically,
    ),
    (
        CursorType::NORTHRESIZE,
        mouse::Interaction::ResizingVertically,
    ),
    (
        CursorType::SOUTHRESIZE,
        mouse::Interaction::ResizingVertically,
    ),
    (CursorType::COLUMNRESIZE, mouse::Interaction::ResizingColumn),
    (CursorType::ROWRESIZE, mouse::Interaction::ResizingRow),
    (CursorType::CELL, mouse::Interaction::Cell),
    (CursorType::COPY, mouse::Interaction::Copy),
    (CursorType::ALIAS, mouse::Interaction::Alias),
    (CursorType::ZOOMIN, mouse::Interaction::ZoomIn),
    (CursorType::ZOOMOUT, mouse::Interaction::ZoomOut),
    (CursorType::NONE, mouse::Interaction::Hidden),
];

pub(crate) fn cursor_index(t: CursorType) -> u8 {
    CURSORS.iter().position(|(c, _)| *c == t).unwrap_or(0) as u8
}

fn interaction(i: u8) -> mouse::Interaction {
    CURSORS
        .get(i as usize)
        .map(|c| c.1)
        .unwrap_or(mouse::Interaction::Idle)
}
