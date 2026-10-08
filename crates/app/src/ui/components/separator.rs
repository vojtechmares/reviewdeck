//! The hairline separator: `border-t border-border` and `border-l border-border` as elements.

use gpui::{App, Div, Styled, div, px};

use crate::ui::theme::ActiveTheme;

/// A full-width 1px rule in the border colour.
pub fn horizontal(cx: &App) -> Div {
    div()
        .w_full()
        .flex_none()
        .h(px(1.))
        .bg(cx.theme().colors.border)
}

/// A full-height 1px rule in the border colour.
pub fn vertical(cx: &App) -> Div {
    div()
        .h_full()
        .flex_none()
        .w(px(1.))
        .bg(cx.theme().colors.border)
}
