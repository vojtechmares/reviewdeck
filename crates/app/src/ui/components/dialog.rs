//! Port of src/renderer/src/components/ui/dialog.tsx: a modal over a scrim, with a title,
//! an optional description, a scrolling body and an optional footer.
//!
//! The view owns the open state: render the dialog only while it is open, as the last child
//! of its `size_full()` root, so the dialog's absolute layer covers the window and paints
//! over everything before it.
//!
//! The dialog is deliberately not a `deferred` element: gpui 0.2.2 cannot draw a deferred
//! element inside another one ("cannot call defer_draw during deferred drawing"), and a
//! select or popover opened inside a dialog is one. Painted last in the root instead, the
//! dialog is on top of the window's content, and popovers opened from inside it defer
//! normally and land on top of the dialog.
//!
//! Behaviour, as in the TSX dialog:
//! - Escape (`Dismiss` in the `Dialog` key context) and a click on the scrim call `on_close`.
//!   A click inside the panel does not reach the scrim. Escape pressed inside a text field
//!   closes the dialog too (the field emits `Cancel` and lets the key travel on, and the
//!   panel handles the text field's `Escape` action), as the TSX capture-phase listener does.
//! - When the dialog first appears, focus lands on its first control other than the close
//!   button, as the TSX `first?.focus()` does. Restoring the previous focus on close is the
//!   view's job, because a dialog that is simply no longer rendered cannot run code.
//! - Focus is trapped. The panel takes a [`FocusHandle`] the view keeps: Tab and Shift-Tab
//!   move through the controls inside it, and when focus would leave, it is pulled back to
//!   the panel. The view should `window.focus(&handle)` when it opens the dialog, and
//!   restore its own focus when it closes.
//! - The close button sits in the header, as the TSX `X` does.

use std::rc::Rc;

use gpui::{
    AnyElement, App, FocusHandle, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, relative,
};

use crate::ui::icons::IconName;
use crate::ui::theme::{ActiveTheme, radius, rpx};

use super::button::{Button, ButtonSize, ButtonVariant};
use super::glass::{GlassExt, scrim};
use super::input;
use super::{Dismiss, FocusNext, FocusPrev};

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A modal dialog. Build it, then return it from the view's `render` while open.
#[derive(IntoElement)]
pub struct Dialog {
    title: SharedString,
    description: Option<SharedString>,
    focus: FocusHandle,
    width: f32,
    children: Vec<AnyElement>,
    footer: Vec<AnyElement>,
    on_close: Option<CloseHandler>,
}

impl Dialog {
    /// `focus` is a handle the view creates with `cx.focus_handle()` and keeps.
    pub fn new(title: impl Into<SharedString>, focus: FocusHandle) -> Dialog {
        Dialog {
            title: title.into(),
            description: None,
            focus,
            // `max-w-lg`
            width: 512.,
            children: Vec::new(),
            footer: Vec::new(),
            on_close: None,
        }
    }

    /// The muted line under the title.
    pub fn description(mut self, description: impl Into<SharedString>) -> Dialog {
        self.description = Some(description.into());
        self
    }

    /// The panel's width in CSS pixels (`max-w-lg` is 512).
    pub fn width(mut self, css_px: f32) -> Dialog {
        self.width = css_px;
        self
    }

    /// Adds an element to the footer, which is laid out to the right with `gap-2`.
    pub fn footer(mut self, element: impl IntoElement) -> Dialog {
        self.footer.push(element.into_any_element());
        self
    }

    /// Called on Escape, a click on the scrim or the close button. The view should close.
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Dialog {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let close = self.on_close.clone();

        // First appearance: move focus to the first control that is not the close button.
        let opened = window.use_keyed_state(
            SharedString::from(format!("dialog-opened-{}", self.title)),
            cx,
            |_, _| false,
        );
        if !*opened.read(cx) {
            opened.update(cx, |value, _| *value = true);
            let panel = self.focus.clone();
            window.defer(cx, move |window, cx| {
                window.focus(&panel);
                // The close button is the first stop; step over it. With nothing else in
                // the dialog the second step would leave the panel, so stay on the button.
                window.focus_next();
                window.focus_next();
                if !panel.contains_focused(window, cx) {
                    window.focus(&panel);
                    window.focus_next();
                }
            });
        }

        // The header: title and description on the left, the close button on the right.
        let mut heading = div().min_w_0().flex_1().child(
            div()
                .text_size(rpx(15.))
                .line_height(rpx(19.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(self.title.clone()),
        );
        if let Some(description) = self.description {
            heading = heading.child(
                div()
                    .mt(rpx(4.))
                    .text_size(rpx(12.5))
                    .line_height(rpx(20.))
                    .text_color(colors.muted_foreground)
                    .child(description),
            );
        }
        let close_button = {
            let close = close.clone();
            let button = Button::new("dialog-close")
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Icon)
                .icon_only(IconName::X)
                .on_click(move |_, window, cx| {
                    if let Some(handler) = &close {
                        handler(window, cx);
                    }
                });
            // `-mt-1 -mr-1`
            div().mt(rpx(-4.)).mr(rpx(-4.)).flex_none().child(button)
        };
        let header = div()
            .flex()
            .items_start()
            .gap(rpx(12.))
            .px(rpx(20.))
            .pt(rpx(18.))
            .pb(rpx(12.))
            .child(heading)
            .child(close_button);

        let body = div()
            .id("dialog-body")
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .px(rpx(20.))
            .pb(rpx(16.))
            .children(self.children);

        let footer = (!self.footer.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(rpx(8.))
                .border_t_1()
                .border_color(colors.border)
                .px(rpx(20.))
                .py(rpx(14.))
                .children(self.footer)
        });

        // Focus trap: a Tab that leaves the panel brings focus back to the panel, so the
        // next Tab enters the dialog's controls again.
        let focus_next = self.focus.clone();
        let focus_prev = self.focus.clone();
        let dismiss = close.clone();
        let dismiss_from_input = close.clone();
        let panel = div()
            .id("dialog-panel")
            .debug_selector(|| "dialog-panel".to_string())
            .key_context("Dialog")
            .track_focus(&self.focus)
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .max_w(relative(1.))
            .max_h(relative(1.))
            .w(rpx(self.width))
            .rounded(rpx(radius::XL))
            .glass_overlay(cx)
            .on_action(move |_: &Dismiss, window, cx| {
                if let Some(handler) = &dismiss {
                    handler(window, cx);
                }
            })
            .on_action(move |_: &input::Escape, window, cx| {
                if let Some(handler) = &dismiss_from_input {
                    handler(window, cx);
                }
            })
            .on_action(move |_: &FocusNext, window, cx| {
                window.focus_next();
                if !focus_next.contains_focused(window, cx) {
                    focus_next.focus(window);
                }
            })
            .on_action(move |_: &FocusPrev, window, cx| {
                window.focus_prev();
                if !focus_prev.contains_focused(window, cx) {
                    focus_prev.focus(window);
                }
            })
            .child(header)
            .child(body)
            .when_some(footer, |d, footer| d.child(footer));

        div()
            .id("dialog-backdrop")
            .debug_selector(|| "dialog-backdrop".to_string())
            .absolute()
            .top(rpx(0.))
            .left(rpx(0.))
            .right(rpx(0.))
            .bottom(rpx(0.))
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(rpx(32.))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(handler) = &close {
                    handler(window, cx);
                }
            })
            .child(scrim(cx))
            .child(panel)
    }
}
