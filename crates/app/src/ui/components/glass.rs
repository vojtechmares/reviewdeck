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

use gpui::{
    AbsoluteLength, Bounds, BoxShadow, Div, Hsla, IntoElement, ParentElement, Styled, canvas, div,
    hsla, point, px, size,
};

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
            .child(outer_shadow(
                vec![BoxShadow {
                    color: glass_shadow_colour(),
                    offset: point(px(0.), px(8.)),
                    blur_radius: px(28.),
                    spread_radius: px(-14.),
                }],
                corner,
            ))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
    }

    /// `.glass-overlay`: the near-opaque film dialogs, popovers, tooltips and toasts use,
    /// with the overlay border and its two drop shadows.
    fn glass_overlay(mut self, cx: &gpui::App) -> Self {
        let colors = cx.theme().colors;
        let corner = current_radius(&mut self);
        self.child(top_highlight_with(cx, corner))
            .child(outer_shadow(
                vec![
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
                ],
                corner,
            ))
            // The CSS film is 0.95 over a 44px blur. Without the blur the app's own text
            // shows through the 5% as a ghost, so the film is a little thicker here.
            .bg(Hsla {
                a: colors.overlay.a.max(0.985),
                ..colors.overlay
            })
            .border_1()
            .border_color(colors.overlay_border)
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

/// A CSS `box-shadow` that is only painted outside the box.
///
/// CSS clips an outer shadow away beneath the element's own box; gpui paints it under the
/// whole box, so on a translucent film (every glass surface here) it shows through as a dark
/// wash: a white `.glass` card came out 20 levels too grey. gpui cannot clip a shadow out
/// either: a content mask only works if it overlaps the shadow's *unblurred* rectangle, and
/// with the negative spreads used here that rectangle lies wholly inside the box.
///
/// So the blur is drawn by hand: bands of 2px quads on the four sides of the box, each with
/// the alpha a gaussian-blurred rectangle has at that distance (the same maths the shader
/// uses). Nothing lands inside the box. Corners are squared off, and the shadow's ends are
/// not blurred along the edge, which at these spreads is not visible. It fills the parent,
/// like [`top_highlight`].
pub fn outer_shadow(shadows: Vec<BoxShadow>, _corner: AbsoluteLength) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for shadow in &shadows {
                paint_outer_shadow(window, bounds, shadow);
            }
        },
    )
    .absolute()
    .top(px(0.))
    .left(px(0.))
    .right(px(0.))
    .bottom(px(0.))
}

/// `erfc`, to about 1e-7 (Numerical Recipes' Chebyshev fit).
fn erfc(x: f32) -> f32 {
    let z = x.abs();
    let t = 1. / (1. + 0.5 * z);
    let poly = -z * z - 1.265_512_2
        + t * (1.000_023_7
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_204
                                + t * (1.488_515_9 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let r = t * poly.exp();
    if x >= 0. { r } else { 2. - r }
}

fn paint_outer_shadow(window: &mut gpui::Window, bounds: Bounds<gpui::Pixels>, shadow: &BoxShadow) {
    let value = |v: gpui::Pixels| f32::from(v);
    // The shadow's own rectangle, in window pixels.
    let rect = (bounds + shadow.offset).dilate(shadow.spread_radius);
    let (left, top) = (value(rect.left()), value(rect.top()));
    let (right, bottom) = (value(rect.right()), value(rect.bottom()));
    let sigma = (value(shadow.blur_radius) / 2.).max(0.5);
    // How far past the box the shadow can still be seen.
    let reach = sigma * 2.6;
    let step = 2.;
    // Alpha of an edge blurred over `sigma`, `outside` pixels beyond it.
    let falloff = |outside: f32| 0.5 * erfc(outside / (sigma * std::f32::consts::SQRT_2));
    // The shadow fades out along an edge over about a sigma, so the bands run a little
    // further than the rectangle itself.
    let slack = sigma * 0.5;
    let box_left = value(bounds.left());
    let box_right = value(bounds.right());
    let box_top = value(bounds.top());
    let box_bottom = value(bounds.bottom());
    let colour = shadow.color;

    let mut paint = |x: f32, y: f32, w: f32, h: f32, alpha: f32| {
        if alpha > 0.002 && w > 0. && h > 0. {
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
                Hsla {
                    a: colour.a * alpha,
                    ..colour
                },
            ));
        }
    };

    let mut d = 0.;
    while d < reach {
        let mid = d + step / 2.;
        // Below.
        let a = falloff(box_bottom + mid - bottom);
        paint(
            left - slack,
            box_bottom + d,
            right - left + 2. * slack,
            step,
            a,
        );
        // Above.
        let a = falloff(top - (box_top - mid));
        paint(
            left - slack,
            box_top - d - step,
            right - left + 2. * slack,
            step,
            a,
        );
        // Left and right, along the box's own height.
        let a = falloff(left - (box_left - mid));
        paint(
            box_left - d - step,
            top - slack,
            step,
            bottom - top + 2. * slack,
            a,
        );
        let a = falloff(box_right + mid - right);
        paint(
            box_right + d,
            top - slack,
            step,
            bottom - top + 2. * slack,
            a,
        );
        d += step;
    }
}

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

/// Tailwind's `shadow-sm`: `0 1px 3px 0 rgb(0 0 0 / .1), 0 1px 2px -1px rgb(0 0 0 / .1)`.
/// `fade` multiplies the alpha (a disabled control fades its shadow with the rest).
pub fn shadow_sm(fade: f32) -> Vec<BoxShadow> {
    let colour = hsla(0., 0., 0., 0.1 * fade);
    vec![
        BoxShadow {
            color: colour,
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
        },
        BoxShadow {
            color: colour,
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(-1.),
        },
    ]
}
