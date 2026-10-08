//! A vertical scroll area with the slim scrollbar from index.css.
//!
//! The CSS scrollbar is 10px wide with a 4px thumb, drawn with a 3px transparent border, in
//! `border-strong`. gpui has no native scrollbar, so the thumb is drawn over the content
//! and is visual only: it follows the wheel, but it cannot be dragged. The offset comes from
//! the handle, so the thumb is one frame behind a fast wheel scroll, which the eye does not
//! see.
//!
//! The view keeps a `ScrollHandle` and passes it in. Use `handle.scroll_to_bottom()` and
//! friends to move the view; they take effect at the next layout.

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    ScrollHandle, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::ui::theme::ActiveTheme;

/// The thumb's visible width (10px track minus the 3px border on each side).
const THUMB_WIDTH: f32 = 4.;
/// The thumb's inset from the right edge of the track.
const THUMB_RIGHT: f32 = 3.;
/// The shortest thumb, so a long list keeps something to grab onto.
const MIN_THUMB: f32 = 24.;

/// A scrolling column of children with the slim thumb over its right edge.
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    handle: ScrollHandle,
    children: Vec<AnyElement>,
}

impl ScrollArea {
    /// `id` must be unique in the window. `handle` is kept by the view.
    pub fn new(id: impl Into<ElementId>, handle: &ScrollHandle) -> ScrollArea {
        ScrollArea {
            id: id.into(),
            handle: handle.clone(),
            children: Vec::new(),
        }
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let thumb_colour = cx.theme().colors.border_strong;
        let offset_y = self.handle.offset().y;
        let max_y = self.handle.max_offset().height;
        let viewport_h = self.handle.bounds().size.height;
        let thumb = thumb(offset_y, max_y, viewport_h).map(|(top, height)| {
            div()
                .absolute()
                .top(top)
                .right(px(THUMB_RIGHT))
                .w(px(THUMB_WIDTH))
                .h(height)
                .rounded_full()
                .bg(thumb_colour)
        });

        div()
            .relative()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .id(self.id)
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.handle)
                    .children(self.children),
            )
            .when_some(thumb, |d, thumb| d.child(thumb))
    }
}

/// The thumb's `(top, height)` for a scroll position, or `None` when nothing scrolls.
///
/// `offset_y` is at most zero when scrolled (gpui's convention) and `max_y` is how far it
/// can go; the thumb's height is the visible share of the content, and its top is the
/// scrolled share of the track.
pub fn thumb(offset_y: Pixels, max_y: Pixels, viewport_h: Pixels) -> Option<(Pixels, Pixels)> {
    let max = f32::from(max_y);
    let viewport = f32::from(viewport_h);
    if max <= 0. || viewport <= 0. {
        return None;
    }
    let content = viewport + max;
    let height = (viewport * (viewport / content)).max(MIN_THUMB);
    let progress = (-f32::from(offset_y) / max).clamp(0., 1.);
    let top = (viewport - height) * progress;
    Some((px(top), px(height)))
}
