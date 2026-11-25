//! A transparent wrapper that gives a list-like view keyboard focus (click inside / focus
//! operation), forwards key presses its children did not capture, and reports right-clicks with
//! the position relative to itself (for context menus).

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::Focusable;
use iced::advanced::widget::{self, Operation, Tree, Widget, tree};
use iced::advanced::{Layout, Shell};
use iced::keyboard::{self, Key, Modifiers};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector};

type KeyFn<'a, M> = Box<dyn Fn(&Key, Modifiers) -> Option<M> + 'a>;

pub struct Interact<'a, M> {
    content: Element<'a, M>,
    id: Option<widget::Id>,
    on_key: Option<KeyFn<'a, M>>,
    on_right_press: Option<Box<dyn Fn(Point) -> M + 'a>>,
    focus_within: bool,
    intercept: bool,
    absolute: bool,
}

#[derive(Default)]
struct State {
    focused: bool,
    modifiers: Modifiers,
}

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }
    fn focus(&mut self) {
        self.focused = true;
    }
    fn unfocus(&mut self) {
        self.focused = false;
    }
}

impl<'a, M> Interact<'a, M> {
    pub fn new(content: impl Into<Element<'a, M>>) -> Self {
        Self {
            content: content.into(),
            id: None,
            on_key: None,
            on_right_press: None,
            focus_within: false,
            intercept: false,
            absolute: false,
        }
    }

    /// Also counts as focused while any focusable child (e.g. a text input) is focused.
    pub fn focus_within(mut self) -> Self {
        self.focus_within = true;
        self
    }

    /// Offer key presses to `on_key` *before* the children (a handled key never reaches them).
    pub fn intercept(mut self) -> Self {
        self.intercept = true;
        self
    }

    pub fn id(mut self, id: widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Key presses while focused (Escape is delivered even when a child captured it).
    pub fn on_key(mut self, f: impl Fn(&Key, Modifiers) -> Option<M> + 'a) -> Self {
        self.on_key = Some(Box::new(f));
        self
    }

    /// Right button pressed inside; the point is relative to this widget's top-left corner.
    /// [`Interact::on_right_press`] reports the point in window coordinates instead (for menus
    /// drawn at the window's root).
    pub fn absolute(mut self) -> Self {
        self.absolute = true;
        self
    }

    pub fn on_right_press(mut self, f: impl Fn(Point) -> M + 'a) -> Self {
        self.on_right_press = Some(Box::new(f));
        self
    }
}

impl<M> widget::Meta for Interact<'_, M> {}

impl<M> Widget<M, Theme, iced::Renderer> for Interact<'_, M> {
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
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), bounds, state);
        operation.container(self.id.as_ref(), bounds, viewport);
        operation.traverse(&mut |operation| {
            let (layout, tree) = layout.iter_mut(&mut tree.children).next().unwrap();
            self.content
                .operate(tree, layout, viewport, renderer, operation);
        });
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
        if self.intercept
            && let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            && let Some(f) = &self.on_key
        {
            let mut focused = tree.state.downcast_ref::<State>().focused;
            if !focused && self.focus_within {
                let mut op = AnyFocused(false);
                let (layout, child) = layout.iter_mut(&mut tree.children).next().unwrap();
                self.content
                    .operate(child, layout, viewport, renderer, &mut op);
                focused = op.0;
            }
            if focused && let Some(m) = f(key, *modifiers) {
                shell.publish(m);
                shell.capture_event();
                return;
            }
        }
        {
            let (layout, child) = layout.iter_mut(&mut tree.children).next().unwrap();
            self.content
                .update(child, event, layout, cursor, renderer, shell, viewport);
        }
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                state.focused = cursor.is_over(bounds);
                let at = if self.absolute {
                    cursor.position_over(bounds)
                } else {
                    cursor.position_in(bounds)
                };
                if *button == mouse::Button::Right
                    && let Some(p) = at
                    && let Some(f) = &self.on_right_press
                {
                    shell.publish(f(p));
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => state.modifiers = *m,
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if state.focused && !self.intercept =>
            {
                let escape = matches!(key, Key::Named(keyboard::key::Named::Escape));
                if (escape || !shell.is_event_captured())
                    && let Some(f) = &self.on_key
                    && let Some(m) = f(key, *modifiers)
                {
                    shell.publish(m);
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
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

impl<'a, M: 'a> From<Interact<'a, M>> for Element<'a, M> {
    fn from(w: Interact<'a, M>) -> Self {
        Widget::<M, Theme, iced::Renderer>::_boxed(w)
    }
}

/// Finds whether any focusable widget in a subtree is focused.
struct AnyFocused(bool);

impl Operation for AnyFocused {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() {
            self.0 = true;
        }
    }
}
