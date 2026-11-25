//! In-app input injection for tests and scripted demos.
//!
//! Wrap the window content in [`Injector`] and push synthetic iced events with [`inject`].
//! The injector feeds them through the *real* widget tree (scrollable offsets, `stack` layering,
//! `opaque` overlays, focus), with the synthetic cursor position given in window coordinates.
//! Nothing touches the OS mouse or keyboard.

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Operation, Tree, Widget, tree};
use iced::advanced::{Layout, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};
use std::collections::VecDeque;
use std::sync::Mutex;

struct Queue {
    events: VecDeque<(Event, Option<Point>)>,
    cursor: Option<Point>,
    waker: Option<iced::advanced::shell::Waker>,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    events: VecDeque::new(),
    cursor: None,
    waker: None,
});

/// Queues `event` for the [`Injector`]. `cursor` (window logical coordinates) moves the synthetic
/// cursor before the event is delivered; `None` keeps the previous synthetic position.
pub fn inject(event: Event, cursor: Option<Point>) {
    let mut q = QUEUE.lock().unwrap();
    q.events.push_back((event, cursor));
    if let Some(w) = &q.waker {
        w.wake();
    }
}

/// Convenience: move + press + release the left button at `p` (window coordinates).
pub fn click(p: Point) {
    inject(
        Event::Mouse(mouse::Event::CursorMoved { position: p }),
        Some(p),
    );
    inject(
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Some(p),
    );
    inject(
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        Some(p),
    );
}

/// Number of injected events not yet delivered.
pub fn pending() -> usize {
    QUEUE.lock().unwrap().events.len()
}

/// Root wrapper that delivers injected events to its content.
pub struct Injector<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> Injector<'a, Message, Theme, Renderer> {
    pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

impl<Message, Theme, Renderer> widget::Meta for Injector<'_, Message, Theme, Renderer> {}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Injector<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        self.content.tag()
    }

    fn state(&self) -> tree::State {
        self.content.state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content.layout(&mut tree.children[0], renderer, limits);
        tree.size = tree.children[0].size;
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
        let (layout, tree) = layout.iter(&tree.children).next().unwrap();
        self.content
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let (layout, tree) = layout.iter_mut(&mut tree.children).next().unwrap();
        self.content
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        // Deliver at most one injected event per real event so the app processes the messages
        // (and re-lays out) between injected events; request another pass if more are queued.
        let next = {
            let mut q = QUEUE.lock().unwrap();
            if q.waker.is_none() {
                q.waker = Some(shell.waker().clone());
            }
            let next = q.events.pop_front();
            if let Some((_, Some(p))) = &next {
                q.cursor = Some(*p);
            }
            next.map(|(e, _)| (e, q.cursor, !q.events.is_empty()))
        };
        if let Some((event, cursor, more)) = next {
            let synthetic = cursor
                .map(mouse::Cursor::Available)
                .unwrap_or(mouse::Cursor::Unavailable);
            let mut local = iced::advanced::shell::Bus::new();
            let mut sub = shell.local(&mut local);
            self.content.update(
                tree, &event, layout, synthetic, renderer, &mut sub, viewport,
            );
            shell.merge(sub, std::convert::identity);
            shell.request_redraw();
            if more {
                shell.waker().wake();
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let (layout, tree) = layout.iter(&tree.children).next().unwrap();
        self.content
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        let (layout, tree) = layout.iter_mut(&mut tree.children).next().unwrap();
        self.content
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<Injector<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(i: Injector<'a, Message, Theme, Renderer>) -> Self {
        Widget::<Message, Theme, Renderer>::_boxed(i)
    }
}
