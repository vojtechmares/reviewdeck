//! The glass surfaces and scrim from src/renderer/src/index.css: `.glass`, `.glass-overlay`,
//! `.glass-quiet` and `.scrim`.
//!
//! What gpui 0.2.2 can and cannot do here:
//! - `backdrop-filter: blur(..) saturate(..)` has no equivalent. gpui has no backdrop blur,
//!   so the films are as opaque as the CSS film colour alone. The window's own vibrancy
//!   (`WindowBackgroundAppearance::Blurred`) still shows through the thin `.glass` film.
//! - `box-shadow: inset 0 1px 0 var(--highlight)` cannot be drawn: `BoxShadow` has no
//!   inset. The top highlight is approximated with a 1px `highlight` line along the top edge.
//!   Call [`GlassExt::glass`] for the film and border, and [`top_highlight`] for the line.

use gpui::{BoxShadow, Div, Hsla, Styled, div, hsla, point, px};

use crate::ui::theme::ActiveTheme;

/// Shadows the CSS `oklch(0.2 0.02 265 / 0.35)` colour. It is the same hue family as the
/// palette's graphite, so an approximation in hsla is close enough.
fn glass_shadow_colour() -> Hsla {
    hsla(230. / 360., 0.2, 0.2, 0.35)
}

/// Adds the glass recipes to any styled element.
pub trait GlassExt: Styled + Sized {
    /// `.glass`: the thin window film, a hairline border and a soft drop shadow.
    fn glass(self, cx: &gpui::App) -> Self {
        let colors = cx.theme().colors;
        self.bg(colors.surface)
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
    fn glass_overlay(self, cx: &gpui::App) -> Self {
        let colors = cx.theme().colors;
        self.bg(colors.overlay)
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

impl<T: Styled + Sized> GlassExt for T {}

/// The `inset 0 1px 0 var(--highlight)` edge: a 1px line along the top of a glass panel,
/// to be placed as an absolutely positioned child of a `relative()` panel.
pub fn top_highlight(cx: &gpui::App) -> Div {
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(px(1.))
        .bg(cx.theme().colors.highlight)
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
