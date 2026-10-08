//! The glass surfaces and scrim from src/renderer/src/index.css: `.glass`, `.glass-overlay`,
//! `.glass-quiet` and `.scrim`.
//!
//! What gpui 0.2.2 can and cannot do here:
//! - `backdrop-filter: blur(..) saturate(..)` has no equivalent. gpui has no backdrop blur,
//!   so the films are as opaque as the CSS film colour alone. The window's own vibrancy
//!   (`WindowBackgroundAppearance::Blurred`) still shows through the thin `.glass` film.
//! - `box-shadow: inset 0 1px 0 var(--highlight)` cannot be drawn: `BoxShadow` has no
//!   inset. It is reproduced with an absolutely positioned child that has only a 1px top
//!   border in the `highlight` colour and the panel's corner radius: gpui's quad shader
//!   follows the rounded corners, so the line tapers into the curve exactly where an inset
//!   shadow does. [`GlassExt::glass`] and [`GlassExt::glass_overlay`] add that child
//!   themselves, reading the radius from the element when they are called (so call
//!   `.rounded(..)` first; the fallback is `rounded-lg`). `glass_quiet` has no highlight in
//!   the CSS and gets none here.

use gpui::{AbsoluteLength, BoxShadow, Div, Hsla, ParentElement, Styled, div, hsla, point, px};

use crate::ui::theme::{ActiveTheme, radius, rpx};

/// Shadows the CSS `oklch(0.2 0.02 265 / 0.35)` colour. It is the same hue family as the
/// palette's graphite, so an approximation in hsla is close enough.
fn glass_shadow_colour() -> Hsla {
    hsla(230. / 360., 0.2, 0.2, 0.35)
}

/// Adds the glass recipes to any styled element.
pub trait GlassExt: Styled + ParentElement + Sized {
    /// `.glass`: the thin window film, a hairline border and a soft drop shadow.
    fn glass(mut self, cx: &gpui::App) -> Self {
        let colors = cx.theme().colors;
        let corner = current_radius(&mut self);
        self.child(top_highlight_with(cx, corner))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .shadow(vec![BoxShadow {
                color: glass_shadow_colour(),
                offset: point(px(0.), px(8.)),
                blur_radius: px(28.),
                spread_radius: px(-14.),
            }])
    }

    /// `.glass-overlay`: the near-opaque film dialogs, popovers, tooltips and toasts use,
    /// with the overlay border and its two drop shadows.
    fn glass_overlay(mut self, cx: &gpui::App) -> Self {
        let colors = cx.theme().colors;
        let corner = current_radius(&mut self);
        self.child(top_highlight_with(cx, corner))
            .bg(colors.overlay)
            .border_1()
            .border_color(colors.overlay_border)
            .shadow(vec![
                BoxShadow {
                    color: colors.overlay_shadow,
                    offset: point(px(0.), px(4.)),
                    blur_radius: px(12.),
                    spread_radius: px(-6.),
                },
                BoxShadow {
                    color: colors.overlay_shadow,
                    offset: point(px(0.), px(24.)),
                    blur_radius: px(60.),
                    spread_radius: px(-18.),
                },
            ])
    }

    /// `.glass-quiet`: the muted film with a hairline border and no shadow.
    fn glass_quiet(self, cx: &gpui::App) -> Self {
        let colors = cx.theme().colors;
        self.bg(colors.surface_muted)
            .border_1()
            .border_color(colors.border)
    }
}

impl<T: Styled + ParentElement + Sized> GlassExt for T {}

/// The element's top-left corner radius as set so far, or `rounded-lg`.
fn current_radius<T: Styled>(element: &mut T) -> AbsoluteLength {
    element
        .style()
        .corner_radii
        .top_left
        .unwrap_or_else(|| rpx(radius::LG).into())
}

fn top_highlight_with(cx: &gpui::App, corner: AbsoluteLength) -> Div {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .bottom(px(0.))
        .rounded(corner)
        .border_t_1()
        .border_color(cx.theme().colors.highlight)
}

/// The `inset 0 1px 0 var(--highlight)` edge as a standalone child, for panels that do not
/// go through [`GlassExt`]. `corner_css_px` is the panel's radius.
pub fn top_highlight(cx: &gpui::App, corner_css_px: f32) -> Div {
    top_highlight_with(cx, rpx(corner_css_px).into())
}

/// `.scrim`: the dimmed layer a modal lays over everything beneath it. Absolutely fills
/// the nearest positioned ancestor; the blur of the CSS version is not available.
pub fn scrim(cx: &gpui::App) -> Div {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .bottom(px(0.))
        .bg(cx.theme().colors.scrim)
}
