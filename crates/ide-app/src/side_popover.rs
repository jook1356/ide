//! A popover that opens to the right of its button, its top level with the button's top.
//! iced's `popover::Position::Right` centres the popup vertically on the button; this wraps
//! such a popover and moves its overlay so the popup's top is level with the button's (the
//! popup is laid out by the popover in a first pass, see `overlay`). The popover's own snapping
//! is off: it snaps into the clip of the enclosing scrollable (the rail list), which pushed
//! the popup back over the button; this one keeps the popup inside the window vertically.

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
    TopAligned {
        inner: inner.boxed(),
    }
    .boxed()
}

/// Shares the popover's tree (like `Widget::map`); only `overlay` differs.
struct TopAligned<'a, Message> {
    inner: Element<'a, Message>,
}

impl<Message> widget::Meta for TopAligned<'_, Message> {}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for TopAligned<'_, Message> {
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
        let popup_h = tree.children.get(1).map_or(0.0, |t| t.size.height);
        let base = layout.bounds() + translation;
        // Where `Position::Right` puts it (centred on the button) and where it should go: the
        // button's top, moved up when the popup would leave the window's bottom.
        let centred = base.y + (base.height - popup_h) / 2.0;
        let top = base.y.min(window.height - popup_h).max(0.0);
        let shifted = Vector::new(translation.x, translation.y + top - centred);
        self.inner
            .overlay(tree, layout, renderer, viewport, shifted, window)
    }
}
