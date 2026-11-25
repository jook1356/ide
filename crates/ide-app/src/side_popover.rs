//! Popovers aligned to an edge of their button. iced's `popover` centres the popup on the
//! button (`Position::Right`: vertically, `Position::Bottom`: horizontally); this wraps such a
//! popover and moves its overlay (the popup is laid out by the popover in a first pass, see
//! `overlay`):
//! - `side_popover`: right of the button, its top level with the button's top;
//! - `end_popover`: below the button, its right edge level with the button's right edge.
//!
//! The popover's own snapping is off and its clip is the window: both are the enclosing
//! scrollable's (the rail list) otherwise, which pushes the popup back over the button or hides
//! it outside the rail. These keep the popup inside the window.
//!
//! Moving the overlay also moves the popover's idea of where the button is, so a press on the
//! button itself counts as outside and sends `on_close`: give the button no `on_press` while
//! the popup is open, or its release would open it again.

use iced::advanced::Widget;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Operation, Tree};
use iced::advanced::{Shell, mouse, overlay, renderer};
use iced::widget::popover::Position;
use iced::{Element, Event, Length, Rectangle, Size, Vector, Widget as _};

/// `popover(trigger, popup)` opening right of `trigger`, top-aligned with it.
pub fn side_popover<'a, Message: Clone + 'a>(
    trigger: impl Widget<Message, iced::Theme, iced::Renderer> + 'a,
    popup: Option<impl Widget<Message, iced::Theme, iced::Renderer> + 'a>,
    gap: f32,
    on_close: Message,
) -> Element<'a, Message> {
    let inner = iced::widget::popover(trigger, popup)
        .position(Position::Right)
        .gap(gap)
        .snap_within_viewport(false)
        .on_close(on_close);
    Aligned {
        inner: inner.boxed(),
        align: Align::SideTop,
    }
    .boxed()
}

/// `popover(trigger, popup)` opening below `trigger`, right-aligned with it.
pub fn end_popover<'a, Message: Clone + 'a>(
    trigger: impl Widget<Message, iced::Theme, iced::Renderer> + 'a,
    popup: Option<impl Widget<Message, iced::Theme, iced::Renderer> + 'a>,
    gap: f32,
    on_close: Message,
) -> Element<'a, Message> {
    let inner = iced::widget::popover(trigger, popup)
        .position(Position::Bottom)
        .gap(gap)
        .snap_within_viewport(false)
        .on_close(on_close);
    Aligned {
        inner: inner.boxed(),
        align: Align::BottomEnd(gap),
    }
    .boxed()
}

#[derive(Clone, Copy)]
enum Align {
    /// Right of the button, top-aligned.
    SideTop,
    /// Below the button (with the popover's gap), right-aligned.
    BottomEnd(f32),
}

/// Shares the popover's tree (like `Widget::map`); only `overlay` differs.
struct Aligned<'a, Message> {
    inner: Element<'a, Message>,
    align: Align,
}

impl<Message> widget::Meta for Aligned<'_, Message> {}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Aligned<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.inner.size()
    }

    fn tag(&self) -> widget::tree::Tag {
        self.inner.tag()
    }

    fn state(&self) -> widget::tree::State {
        self.inner.state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.inner.diff(tree);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.inner.layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.inner
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.inner
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.inner
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.inner
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        // First pass: the popover lays its popup out (its tree's second child) when needed.
        drop(
            self.inner
                .overlay(tree, layout, renderer, viewport, translation, window),
        );
        let popup = tree.children.get(1).map_or(Size::ZERO, |t| t.size);
        let base = layout.bounds() + translation;
        let shift = match self.align {
            Align::SideTop => {
                // Where `Position::Right` puts it (centred on the button) and where it should
                // go: the button's top, moved up when the popup would leave the window's bottom.
                let centred = base.y + (base.height - popup.height) / 2.0;
                let top = base.y.min(window.height - popup.height).max(0.0);
                Vector::new(0.0, top - centred)
            }
            Align::BottomEnd(gap) => {
                // `Position::Bottom` centres it on the button: move it so its right edge is the
                // button's, to the right when it would leave the window's left edge, and up
                // when it would leave the bottom.
                let centred = base.x + (base.width - popup.width) / 2.0;
                let left = (base.x + base.width - popup.width).min(window.width - popup.width).max(0.0);
                let below = base.y + base.height + gap;
                let top = below.min(window.height - popup.height).max(0.0);
                Vector::new(left - centred, top - below)
            }
        };
        let shifted = translation + shift;
        // The popover draws its popup clipped to `viewport + translation`: make that the whole
        // window, not the enclosing scrollable's clip (which hid the popup outside the rail).
        let whole = Rectangle {
            x: -shifted.x,
            y: -shifted.y,
            width: window.width,
            height: window.height,
        };
        self.inner
            .overlay(tree, layout, renderer, &whole, shifted, window)
    }
}
