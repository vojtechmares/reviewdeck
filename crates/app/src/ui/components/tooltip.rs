//! Port of src/renderer/src/components/ui/tooltip.tsx.
//!
//! The TSX tooltip is positioned with plain CSS under its trigger. Here it is gpui's own
//! tooltip: `.tooltip(tooltip(text))` on any element with an id, shown after the standard
//! hover delay and placed at the pointer. It takes the `glass-overlay` surface and the
//! `text-[11.5px] font-medium leading-snug` type.

use gpui::{
    AnyView, App, AppContext, Context, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, div,
};

use crate::ui::theme::{ActiveTheme, radius, rpx};

use super::glass::GlassExt;

/// The view a tooltip shows.
pub struct TooltipView {
    text: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .font_family(crate::ui::theme::UI_FONT)
            .text_size(rpx(11.5))
            .line_height(rpx(15.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(colors.foreground)
            .max_w(rpx(320.))
            .px(rpx(8.))
            .py(rpx(4.))
            .rounded(rpx(radius::MD))
            .glass_overlay(cx)
            .child(self.text.clone())
    }
}

/// The builder `Stateful::tooltip` takes: `.tooltip(tooltip("Refresh (⌘R)"))`.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_window, cx| {
        let text = text.clone();
        cx.new(|_| TooltipView { text }).into()
    }
}
