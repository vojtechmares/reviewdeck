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

use gpui::{App, KeyBinding, actions};

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
}
