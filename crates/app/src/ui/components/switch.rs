//! The switch and checkbox from SettingsDialog.tsx and ScheduleDialog.tsx.
//!
//! Both are stateless: the view passes `checked` and an `on_change` handler, and re-renders
//! with the new value, as a React controlled input does.
//!
//! In the TSX the row is a `<label>` around a visually hidden checkbox input, so the whole
//! row toggles on click, Tab lands on the (hidden) input and Space toggles it, with the ring
//! drawn on the track. Here the row handles the click, and the track (or box) is the keyboard
//! stop that handles Space and shows the ring.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, white,
};

use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, rpx};

use super::FocusRing;

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Makes the box or track the control's keyboard stop, as the native checkbox input is in
/// the TSX (`peer-focus-visible:outline-2`): Tab focuses it, Space toggles it, and the ring
/// shows. A mouse press never focuses it (see [`FocusRing`]); the row handles the click.
fn keyboard_stop(
    element: Stateful<gpui::Div>,
    next: bool,
    handler: Option<ChangeHandler>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let element = element.focus_ring(cx);
    match handler {
        Some(handler) => element.on_key_down(move |event, window, cx| {
            if event.keystroke.key == "space" && !event.is_held {
                handler(next, window, cx);
                cx.stop_propagation();
            }
        }),
        None => element,
    }
}

/// `w-8.5 h-5` track with a 16px thumb: `bg-ok` when on, `bg-muted` when off. The label, if
/// any, sits to the left and takes the spare width, as the settings rows lay it out.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>) -> Switch {
        Switch {
            id: id.into(),
            label: None,
            checked: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Switch {
        self.label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Switch {
        self.checked = checked;
        self
    }

    /// `opacity-45` on the row, and no clicks.
    pub fn disabled(mut self, disabled: bool) -> Switch {
        self.disabled = disabled;
        self
    }

    /// Called with the new value when the row is clicked (or Space is pressed on the track).
    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Switch {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let track_fill = if self.checked {
            colors.ok
        } else {
            colors.muted
        };
        // The thumb sits 1px in from the track's border, at the far end when on.
        let thumb_left = if self.checked { 15. } else { 1. };
        let toggle = self.on_change.clone().filter(|_| !self.disabled);
        let row_selector = format!("switch-{}", self.id);
        let track_selector = format!("switch-track-{}", self.id);

        let mut row = div()
            .id(self.id)
            .debug_selector(move || row_selector)
            .flex()
            .items_center()
            .gap(rpx(12.))
            .when(self.disabled, |d| d.opacity(0.45))
            .when(!self.disabled, |d| d.cursor_pointer());
        if let Some(label) = self.label {
            // `min_w_0` lets a long label wrap inside the row instead of pushing the track out.
            row = row.child(div().flex_1().min_w_0().text_size(rpx(12.5)).child(label));
        }
        let track = div()
            .id("track")
            .debug_selector(move || track_selector)
            .relative()
            .flex_none()
            .w(rpx(34.))
            .h(rpx(20.))
            .rounded_full()
            .border_1()
            .border_color(colors.border)
            .bg(track_fill)
            .child(
                div()
                    .absolute()
                    .top(rpx(1.))
                    .left(rpx(thumb_left))
                    .size(rpx(16.))
                    .rounded_full()
                    .bg(white())
                    .shadow(vec![gpui::BoxShadow {
                        color: colors.overlay_shadow,
                        offset: gpui::point(px(0.), px(1.)),
                        blur_radius: px(2.),
                        spread_radius: px(0.),
                    }]),
            );
        let track = if self.disabled {
            track
        } else {
            keyboard_stop(track, !self.checked, toggle.clone(), cx)
        };
        row = row.child(track);

        if let Some(handler) = toggle {
            let next = !self.checked;
            row = row.on_click(move |_, window, cx| handler(next, window, cx));
        }
        row
    }
}

/// The 16px checkbox with its label, `peer`-style: the box is `bg-primary` when checked.
///
/// The TSX uses the platform's native checkbox in the schedule dialog. gpui has no native
/// control, so this draws the same thing in the theme's colours; it is a few pixels larger
/// than Chromium's 13px box, which is deliberate: the 16px box is easier to hit.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    label: Option<SharedString>,
    checked: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Checkbox {
        Checkbox {
            id: id.into(),
            label: None,
            checked: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Checkbox {
        self.label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Checkbox {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Checkbox {
        self.disabled = disabled;
        self
    }

    /// Called with the new value when the row is clicked (or Space is pressed on the box).
    pub fn on_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Checkbox {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (fill, edge) = if self.checked {
            (colors.primary, colors.primary)
        } else {
            (colors.surface_strong, colors.border_strong)
        };
        let mark: Option<AnyElement> = self.checked.then(|| {
            Icon::new(IconName::Check)
                .size(12.)
                .color(colors.primary_foreground)
                .into_any_element()
        });
        let toggle = self.on_change.clone().filter(|_| !self.disabled);
        let row_selector = format!("checkbox-{}", self.id);
        let box_selector = format!("checkbox-box-{}", self.id);

        let mut row = div()
            .id(self.id)
            .debug_selector(move || row_selector)
            .flex()
            .items_center()
            .gap(rpx(8.))
            .text_size(rpx(13.))
            .when(self.disabled, |d| d.opacity(0.45))
            .when(!self.disabled, |d| d.cursor_pointer());
        let square = div()
            .id("box")
            .debug_selector(move || box_selector)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(rpx(16.))
            .rounded(rpx(4.))
            .border_1()
            .border_color(edge)
            .bg(fill)
            .when_some(mark, |d, mark| d.child(mark));
        let square = if self.disabled {
            square
        } else {
            keyboard_stop(square, !self.checked, toggle.clone(), cx)
        };
        row = row.child(square);
        if let Some(label) = self.label {
            row = row.child(div().min_w_0().font_weight(FontWeight::NORMAL).child(label));
        }
        if let Some(handler) = toggle {
            let next = !self.checked;
            row = row.on_click(move |_, window, cx| handler(next, window, cx));
        }
        row
    }
}
