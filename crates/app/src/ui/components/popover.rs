//! Port of src/renderer/src/components/ui/popover.tsx.
//!
//! The TSX popover is a Radix portal: it floats above the whole window, so a panel opened
//! from a header is not clipped or hidden under the content below it. Here the panel is a
//! `deferred(anchored(..))`, which paints above everything in the window, and it is snapped
//! back inside the window if it would run off the edge.
//!
//! The popover does not own its open state: the view does, and passes `open(..)` and an
//! `on_dismiss` callback. A click outside the panel calls `on_dismiss` (a full-window
//! backdrop swallows that click, so it does not also re-toggle the trigger), and so does
//! Escape while the panel holds focus (`Dismiss` in the `Popover` key context).

use std::rc::Rc;

use gpui::{
    AnyElement, App, Corner, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, Styled, Window, anchored, deferred, div, point,
    prelude::FluentBuilder, px,
};

use crate::ui::theme::{radius, rpx};

use super::Dismiss;
use super::glass::GlassExt;

type DismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A trigger and, while open, a floating panel under it.
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    trigger: Option<AnyElement>,
    content: Option<AnyElement>,
    open: bool,
    align_end: bool,
    width: Option<f32>,
    on_dismiss: Option<DismissHandler>,
}

impl Popover {
    pub fn new(id: impl Into<ElementId>) -> Popover {
        Popover {
            id: id.into(),
            trigger: None,
            content: None,
            open: false,
            align_end: false,
            width: None,
            on_dismiss: None,
        }
    }

    /// The element the panel anchors to. It is always drawn.
    pub fn trigger(mut self, trigger: impl IntoElement) -> Popover {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    /// The panel's contents. Drawn only while `open` is true.
    pub fn content(mut self, content: impl IntoElement) -> Popover {
        self.content = Some(content.into_any_element());
        self
    }

    pub fn open(mut self, open: bool) -> Popover {
        self.open = open;
        self
    }

    /// Lines the panel's right edge up with the trigger's (Radix `align="end"`). The default
    /// lines up the left edges.
    pub fn align_end(mut self) -> Popover {
        self.align_end = true;
        self
    }

    /// The panel's width in CSS pixels. Without one it sizes to its contents.
    pub fn width(mut self, css_px: f32) -> Popover {
        self.width = Some(css_px);
        self
    }

    /// Called on an outside click, or on Escape in the panel. The view should close.
    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Popover {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let viewport = window.viewport_size();
        let dismiss = self.on_dismiss.clone();
        let open = self.open && self.content.is_some();
        let corner = if self.align_end {
            Corner::TopRight
        } else {
            Corner::TopLeft
        };
        let width = self.width;
        let align_end = self.align_end;

        let mut root = div().id(self.id).relative().flex_none();
        if let Some(trigger) = self.trigger {
            root = root.child(trigger);
        }
        let Some(content) = self.content.filter(|_| open) else {
            return root;
        };

        let backdrop_dismiss = dismiss.clone();
        let panel_dismiss = dismiss;
        let backdrop = deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        if let Some(handler) = &backdrop_dismiss {
                            handler(window, cx);
                        }
                    }),
            ),
        )
        .with_priority(1);

        let mut panel = div()
            .key_context("Popover")
            .occlude()
            .overflow_hidden()
            .rounded(rpx(radius::LG))
            .py(rpx(4.))
            .glass_overlay(cx)
            .on_action(move |_: &Dismiss, window, cx| {
                if let Some(handler) = &panel_dismiss {
                    handler(window, cx);
                }
            })
            .child(content);
        if let Some(width) = width {
            panel = panel.w(rpx(width));
        }

        let positioned = deferred(
            anchored()
                .anchor(corner)
                .offset(point(px(0.), px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        .with_priority(2);

        let holder = div()
            .absolute()
            .top_full()
            .when(align_end, |d| d.right_0())
            .when(!align_end, |d| d.left_0())
            .child(positioned);

        root.child(backdrop).child(holder)
    }
}
