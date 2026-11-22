//! A transparent row wrapper for drag-to-reorder lists: pressing and moving the pointer a few
//! pixels starts a drag ([`Reorder::on_start`]); while a drag is active (the owner's state,
//! [`Reorder::active`]) each row reports whether the pointer is over its top or bottom half
//! ([`Reorder::on_over`]); releasing ends it ([`Reorder::on_end`]) instead of clicking the row.
//! The content (usually a button) keeps working for plain clicks.

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Operation, Tree, Widget, tree};
use iced::advanced::{Layout, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector};

/// Pointer travel (logical px) that turns a press into a drag.
const THRESHOLD: f32 = 5.0;

pub struct Reorder<'a, M> {
    content: Element<'a, M>,
    on_start: Option<M>,
    on_over: Option<Box<dyn Fn(bool) -> M + 'a>>,
    on_end: Option<M>,
    active: bool,
}

#[derive(Default)]
struct State {
    press: Option<Point>,
    dragging: bool,
    /// Last reported half the pointer is over (`true` = top) while a drag is active.
    over: Option<bool>,
}

impl<'a, M> Reorder<'a, M> {
    pub fn new(content: impl Into<Element<'a, M>>) -> Self {
        Self {
            content: content.into(),
            on_start: None,
            on_over: None,
            on_end: None,
            active: false,
        }
    }

    /// A drag of this row started (none: the row cannot be dragged).
    pub fn on_start(mut self, m: M) -> Self {
        self.on_start = Some(m);
        self
    }

    /// While a drag is active: the pointer is over this row's top (`true`) or bottom half.
    pub fn on_over(mut self, f: impl Fn(bool) -> M + 'a) -> Self {
        self.on_over = Some(Box::new(f));
        self
    }

    /// The drag this row started ended (button released anywhere).
    pub fn on_end(mut self, m: M) -> Self {
        self.on_end = Some(m);
        self
    }

    /// A drag (of any row) is in progress.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

impl<M> widget::Meta for Reorder<'_, M> {}

impl<M: Clone> Widget<M, Theme, iced::Renderer> for Reorder<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content.layout(&mut tree.children[0], renderer, limits);
        tree.size = tree.children[0].size;
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let (layout, tree) = layout.iter(&tree.children).next().unwrap();
        self.content
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let (layout, tree) = layout.iter_mut(&mut tree.children).next().unwrap();
        self.content
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        let mut child_cursor = cursor;
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.press = cursor.position_over(bounds);
                state.dragging = false;
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(p) = state.press
                    && !state.dragging
                    && p.distance(*position) > THRESHOLD
                    && let Some(m) = &self.on_start
                {
                    state.dragging = true;
                    shell.publish(m.clone());
                }
                let over = if self.active {
                    cursor
                        .position_over(bounds)
                        .map(|p| p.y < bounds.center_y())
                } else {
                    None
                };
                if over != state.over {
                    state.over = over;
                    if let (Some(top), Some(f)) = (over, &self.on_over) {
                        shell.publish(f(top));
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.press = None;
                state.over = None;
                if std::mem::take(&mut state.dragging) {
                    if let Some(m) = &self.on_end {
                        shell.publish(m.clone());
                    }
                    // The content sees the release away from it: no click.
                    child_cursor = mouse::Cursor::Unavailable;
                    shell.capture_event();
                }
            }
            _ => {}
        }
        let (layout, child) = layout.iter_mut(&mut tree.children).next().unwrap();
        self.content
            .update(child, event, layout, child_cursor, renderer, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if tree.state.downcast_ref::<State>().dragging {
            return mouse::Interaction::Grabbing;
        }
        let (layout, tree) = layout.iter(&tree.children).next().unwrap();
        self.content
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
    ) -> Vec<overlay::Element<'b, M, Theme, iced::Renderer>> {
        let (layout, tree) = layout.iter_mut(&mut tree.children).next().unwrap();
        self.content
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, M: Clone + 'a> From<Reorder<'a, M>> for Element<'a, M> {
    fn from(w: Reorder<'a, M>) -> Self {
        Widget::<M, Theme, iced::Renderer>::_boxed(w)
    }
}
