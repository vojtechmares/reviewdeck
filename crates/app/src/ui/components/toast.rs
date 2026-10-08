//! Port of src/renderer/src/components/ui/toast.tsx: a stack of glass notes in the bottom-right
//! corner, each with a tone icon, its message and a dismiss button.
//!
//! The stack is an entity the root view creates once and renders as its last child:
//!
//! ```ignore
//! let toasts = cx.new(|_| ToastStack::new());
//! // later, from any view with a context:
//! toasts.update(cx, |stack, cx| stack.push(ToastKind::Bad, "Could not load pull requests", cx));
//! ```
//!
//! As in the TSX provider, at most four toasts are kept (the oldest goes when a fifth
//! arrives). Errors stay for 8 seconds, confirmations for 3.5. Each toast owns its timer,
//! so dismissing one, or dropping the stack, cancels it.

use std::time::Duration;

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, deferred, div, prelude::FluentBuilder, px,
};

use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, radius, rpx};

use super::glass::GlassExt;

/// `ToastTone` on the TSX side: the icon and its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// A check in `text-ok`.
    Ok,
    /// A warning triangle in `text-bad`. Stays longer.
    Bad,
    /// An info mark in `text-info`.
    Info,
}

/// The most toasts on screen at once.
const MAX_TOASTS: usize = 4;

struct Toast {
    id: u64,
    kind: ToastKind,
    message: SharedString,
    /// Dropping the task cancels the auto-dismiss.
    _timer: Task<()>,
}

/// The toast stack. See the module docs.
pub struct ToastStack {
    toasts: Vec<Toast>,
    next_id: u64,
}

impl Default for ToastStack {
    fn default() -> Self {
        ToastStack::new()
    }
}

impl ToastStack {
    pub fn new() -> ToastStack {
        ToastStack {
            toasts: Vec::new(),
            next_id: 0,
        }
    }

    /// Shows a toast and starts its timer.
    pub fn push(
        &mut self,
        kind: ToastKind,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        // "Errors linger; confirmations get out of the way quickly."
        let ttl = match kind {
            ToastKind::Bad => Duration::from_millis(8000),
            ToastKind::Ok | ToastKind::Info => Duration::from_millis(3500),
        };
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ttl).await;
            this.update(cx, |stack, cx| stack.dismiss(id, cx)).ok();
        });
        self.toasts.push(Toast {
            id,
            kind,
            message: message.into(),
            _timer: timer,
        });
        // `[...current.slice(-3), next]`: keep the newest three, then add the new one.
        if self.toasts.len() > MAX_TOASTS {
            let excess = self.toasts.len() - MAX_TOASTS;
            self.toasts.drain(..excess);
        }
        cx.notify();
    }

    /// Removes a toast by id. Unknown ids are ignored.
    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.id != id);
        if self.toasts.len() != before {
            cx.notify();
        }
    }

    /// The number of toasts on screen.
    pub fn len(&self) -> usize {
        self.toasts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }
}

impl Render for ToastStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mut column = div()
            .absolute()
            .right(rpx(16.))
            .bottom(rpx(16.))
            .w(rpx(352.))
            .flex()
            .flex_col()
            .gap(rpx(8.));

        for toast in &self.toasts {
            let (icon, tone) = match toast.kind {
                ToastKind::Ok => (IconName::Check, colors.ok),
                ToastKind::Bad => (IconName::AlertTriangle, colors.bad),
                ToastKind::Info => (IconName::Info, colors.info),
            };
            let id = toast.id;
            let dismiss = cx.listener(move |stack, _: &gpui::ClickEvent, _window, cx| {
                stack.dismiss(id, cx);
            });
            let row = div()
                .id(SharedString::from(format!("toast-{id}")))
                .debug_selector(move || format!("toast-{id}"))
                // `pointer-events-auto` on a `pointer-events-none` stack: the notes catch
                // the mouse, the gaps between them do not.
                .occlude()
                .flex()
                .items_start()
                .gap(rpx(10.))
                .px(rpx(12.))
                .py(rpx(10.))
                .rounded(rpx(radius::LG))
                .glass_overlay(cx)
                .child(
                    div()
                        .mt(px(1.))
                        .flex_none()
                        .child(Icon::new(icon).size(16.).color(tone)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(rpx(12.5))
                        .line_height(rpx(16.))
                        .font_weight(FontWeight::NORMAL)
                        // `break-words`: gpui wraps at the box width and breaks a word
                        // that is wider than it, which is the same thing.
                        .child(toast.message.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("toast-dismiss-{id}")))
                        .debug_selector(move || format!("toast-dismiss-{id}"))
                        .flex_none()
                        .cursor_pointer()
                        .group(SharedString::from(format!("toast-dismiss-group-{id}")))
                        .on_click(dismiss)
                        .child(
                            Icon::new(IconName::X)
                                .size(14.)
                                .color(colors.muted_foreground)
                                // `hover:text-foreground`
                                .hover_group(
                                    SharedString::from(format!("toast-dismiss-group-{id}")),
                                    colors.foreground,
                                ),
                        ),
                );
            column = column.child(row);
        }
        // `z-100`: above dialogs and popovers, so a toast raised while a dialog is open is
        // not hidden behind its scrim.
        deferred(column.when(self.toasts.is_empty(), |d| d.invisible())).with_priority(10)
    }
}
