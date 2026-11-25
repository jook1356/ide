//! Virtualised vertical list with fixed row heights: only the rows in view (plus a margin) are
//! built, between two spacers, inside a normal `scrollable` (so the scrollbar, wheel and smooth
//! scrolling behave as everywhere else).

use iced::Widget as _;
use iced::widget::operation::{self, scrollable::AbsoluteOffset};
use iced::widget::scrollable::{self, Direction, Scrollbar, Viewport};
use iced::widget::{Column, column, container, responsive, scrollable as scrollable_w, space};
use iced::{Element, Length, Task, Theme};
use std::cell::Cell;

/// Rows built above and below the visible ones.
const OVERSCAN: usize = 8;

/// Scroll state of one virtual list (keep it in the owning view's state).
#[derive(Debug)]
pub struct VList {
    pub id: iced::widget::Id,
    offset: f32,
    x: f32,
    height: Cell<f32>,
}

impl Default for VList {
    fn default() -> Self {
        Self {
            id: iced::widget::Id::unique(),
            offset: 0.0,
            x: 0.0,
            height: Cell::new(800.0),
        }
    }
}

impl VList {
    pub fn on_scroll(&mut self, v: &Viewport) {
        let o = v.absolute_offset();
        self.offset = o.y;
        self.x = o.x;
        self.height.set(v.bounds.height);
    }

    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// Visible height from the last layout.
    pub fn height(&self) -> f32 {
        self.height.get()
    }

    /// Scrolls so that row `index` is in view (no-op when it already is).
    pub fn ensure_visible<M: Send + 'static>(&mut self, index: usize, row_h: f32) -> Task<M> {
        let top = index as f32 * row_h;
        let h = self.height.get();
        let target = if top < self.offset {
            top
        } else if top + row_h > self.offset + h {
            top + row_h - h
        } else {
            return Task::none();
        };
        self.scroll_to(target)
    }

    /// Scrolls so that row `index` is in the middle of the view.
    pub fn center<M: Send + 'static>(&mut self, index: usize, row_h: f32) -> Task<M> {
        let target = (index as f32 * row_h - self.height.get() / 2.0 + row_h / 2.0).max(0.0);
        self.scroll_to(target)
    }

    pub fn scroll_to<M: Send + 'static>(&mut self, y: f32) -> Task<M> {
        self.offset = y.max(0.0);
        operation::scrollable::scroll_to(
            self.id.clone(),
            AbsoluteOffset {
                x: None,
                y: Some(self.offset),
            },
            operation::Animation::Instant,
        )
    }

    /// Like [`VList::view`] with variable row heights: `offsets[i]` is the top of row `i` and
    /// `offsets[count]` the total height (so `offsets.len() == count + 1`).
    pub fn view_var<'a, M: 'a>(
        &'a self,
        offsets: &'a [f32],
        width: Option<f32>,
        row: impl Fn(usize) -> Element<'a, M> + 'a,
        on_scroll: impl Fn(Viewport) -> M + 'a + Clone,
    ) -> Element<'a, M> {
        responsive(move |size| {
            self.height.set(size.height);
            let count = offsets.len().saturating_sub(1);
            let total = offsets.last().copied().unwrap_or(0.0);
            let first = offsets
                .partition_point(|&o| o <= self.offset)
                .saturating_sub(1)
                .saturating_sub(OVERSCAN)
                .min(count);
            let bottom = self.offset + size.height;
            let last = (offsets.partition_point(|&o| o < bottom) + OVERSCAN).min(count);
            let mut col = Column::new().push(space().height(offsets[first]).boxed());
            for i in first..last {
                let h = offsets[i + 1] - offsets[i];
                col = col.push(container(row(i)).height(h).clip(true).boxed());
            }
            col = col.push(space().height(total - offsets[last]).boxed());
            let col = match width {
                Some(w) => col.width(Length::Fixed(w.max(size.width))),
                None => col.width(Length::Fill),
            };
            let dir = if width.is_some() {
                Direction::Both {
                    vertical: bar(),
                    horizontal: bar(),
                }
            } else {
                Direction::Vertical(bar())
            };
            let on_scroll = on_scroll.clone();
            scrollable_w(column![col])
                .id(self.id.clone())
                .direction(dir)
                .on_scroll(move |s| on_scroll(s.viewport))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(scroll_style)
                .boxed()
        })
        .boxed()
    }

    /// The list: `count` rows of `row_h` px built by `row(index)`. `width` is the content width
    /// (`Some` for a horizontally scrollable list of long lines).
    pub fn view<'a, M: 'a>(
        &'a self,
        count: usize,
        row_h: f32,
        width: Option<f32>,
        row: impl Fn(usize) -> Element<'a, M> + 'a,
        on_scroll: impl Fn(Viewport) -> M + 'a + Clone,
    ) -> Element<'a, M> {
        responsive(move |size| {
            self.height.set(size.height);
            let first = ((self.offset / row_h).floor().max(0.0) as usize).saturating_sub(OVERSCAN);
            let visible = (size.height / row_h).ceil() as usize + 2 * OVERSCAN + 1;
            let first = first.min(count);
            let last = (first + visible).min(count);
            let mut col = Column::new().push(space().height(first as f32 * row_h).boxed());
            for i in first..last {
                col = col.push(container(row(i)).height(row_h).clip(true).boxed());
            }
            col = col.push(space().height((count - last) as f32 * row_h).boxed());
            let col = match width {
                Some(w) => col.width(Length::Fixed(w.max(size.width))),
                None => col.width(Length::Fill),
            };
            let dir = if width.is_some() {
                Direction::Both {
                    vertical: bar(),
                    horizontal: bar(),
                }
            } else {
                Direction::Vertical(bar())
            };
            let on_scroll = on_scroll.clone();
            scrollable_w(column![col])
                .id(self.id.clone())
                .direction(dir)
                .on_scroll(move |s| on_scroll(s.viewport))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(scroll_style)
                .boxed()
        })
        .boxed()
    }
}

fn bar() -> Scrollbar {
    Scrollbar::new().width(8).scroller_width(6).margin(1)
}

pub fn scroll_style(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let mut s = scrollable::default(theme, status);
    let handle = |c: f32| iced::Color::from_rgba(1.0, 1.0, 1.0, c);
    for rail in [&mut s.vertical_rail, &mut s.horizontal_rail] {
        rail.background = None;
        rail.border = iced::Border::default();
        rail.scroller.background = handle(0.16).into();
        rail.scroller.border = iced::Border {
            radius: 3.0.into(),
            ..Default::default()
        };
    }
    if let scrollable::Status::Hovered {
        is_vertical_scrollbar_hovered: true,
        ..
    }
    | scrollable::Status::Dragged {
        is_vertical_scrollbar_dragged: true,
        ..
    } = status
    {
        s.vertical_rail.scroller.background = handle(0.28).into();
    }
    s
}
