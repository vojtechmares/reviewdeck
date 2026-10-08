//! The UI kit: a port of src/renderer/src/components/ui/*.tsx, plus the primitive controls
//! the views use (switches, checkboxes, selects, segmented toggles, spinners, scroll areas,
//! separators and the glass surfaces from index.css).
//!
//! Views compose these; nothing here knows about pull requests. Colours come from
//! `cx.theme().colors` and lengths from `theme::rpx`, so the kit follows the theme and zoom.
//!
//! Setup: call [`bind_keys`] once at startup, before `cx.set_menus`. It binds Tab and
//! Shift-Tab to the focus-moving actions, and Escape to [`Dismiss`] inside dialogs and
//! popovers. The text field's own bindings come from [`input::bind_keys`], which this calls.
//!
//! # Layout rules learned the hard way (gpui 0.2.2)
//!
//! - `gap` (and `gap_x`/`gap_y`) only does anything on a `flex()` or `grid()` container. A
//!   gpui `div()` is `display: block`, so `div().gap(..).child("text").child(icon)` lays the
//!   children out as blocks and the gap comes out as zero. Every kit container that uses
//!   `gap` calls `.flex()` first, and views must do the same. This is the root cause of the
//!   "gap is zero when a child is plain text" reports: there, the container was never a flex
//!   container (see the `gap_needs_flex` test, which pins both halves of the rule).
//! - Colours set by `hover`/`group_hover` styles reach backgrounds, borders and `svg()` icons
//!   (they are resolved at paint time) but never already-shaped text: text runs take their
//!   colour at layout time. A control whose label changes colour on hover has to be re-rendered
//!   with the new colour, which is what [`button::Button`] does for its ghost variant.
//! - Never toggle `display` from a hover style: use `invisible()`/`visible()`.

// A kit, so its API is broader than what the views happen to call today.
#![allow(dead_code)]

use gpui::{App, BoxShadow, KeyBinding, StatefulInteractiveElement, Styled, actions, point, px};

use crate::ui::theme::ActiveTheme;

pub mod avatar;
pub mod badge;
pub mod button;
pub mod dialog;
pub mod glass;
pub mod input;
pub mod popover;
pub mod scroll;
pub mod segmented;
pub mod select;
pub mod separator;
pub mod spinner;
pub mod switch;
pub mod toast;
pub mod tooltip;

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;

actions!(ui_kit, [Dismiss, FocusNext, FocusPrev]);

/// Registers every key binding the kit's controls need. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
        KeyBinding::new("escape", Dismiss, Some("Dialog")),
        KeyBinding::new("escape", Dismiss, Some("Popover")),
    ]);
    input::bind_keys(cx);
    select::bind_keys(cx);
}

/// Keyboard focus for the clickable controls, the way a browser gives every `<button>` a tab
/// stop and an `outline: 2px solid var(--ring)` while focus came from the keyboard.
///
/// gpui already fires `on_click` for Enter and Space on a focused element. It has no
/// `:focus-visible`, so a mouse press is stopped from moving focus at all (the press still
/// clicks); only Tab focuses the control, and so only Tab shows the ring. The ring is a 2px
/// spread shadow: gpui has no outline offset.
pub trait FocusRing: StatefulInteractiveElement + Styled + Sized {
    fn focus_ring(self, cx: &App) -> Self {
        let ring = cx.theme().colors.ring;
        self.focusable()
            .tab_stop(true)
            .capture_any_mouse_down(|_, window, _| window.prevent_default())
            .focus(move |style| {
                style.shadow(vec![BoxShadow {
                    color: ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                }])
            })
    }
}

impl<T: StatefulInteractiveElement + Styled + Sized> FocusRing for T {}
