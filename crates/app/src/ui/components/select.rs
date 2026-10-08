//! The select from ui/input.tsx, and the filter dropdowns in App.tsx.
//!
//! A native `<select>` has no gpui equivalent, so this is a trigger field and a list in a
//! [`Popover`]. The trigger carries the `FIELD` look with the chevron the TSX draws as a
//! background image. The list is a glass panel, the way the platform menu would be.
//!
//! A `Select` is an entity, because it holds its open state and the chosen option. Create it
//! with `cx.new(|cx| Select::new(options, selected, cx))`, subscribe to [`SelectEvent`] for
//! changes, and render it as a child: `.child(select.clone())`.

use gpui::{
    Context, EventEmitter, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};

use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, radius, rpx};

use super::button::with_alpha;
use super::popover::Popover;

/// One choice: the value the app stores and the label the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    pub value: SharedString,
    pub label: SharedString,
}

impl SelectOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> SelectOption {
        SelectOption {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// Emitted when the user picks a different option. Not emitted by `set_selected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectEvent {
    Changed(SharedString),
}

/// The select's state and view. See the module docs.
pub struct Select {
    options: Vec<SelectOption>,
    selected: Option<usize>,
    open: bool,
    placeholder: SharedString,
    /// Trigger width in CSS pixels. Without one the trigger fills its parent.
    width: Option<f32>,
}

impl EventEmitter<SelectEvent> for Select {}

impl Select {
    /// `selected` is the value to start on; an unknown value starts with nothing chosen.
    pub fn new(options: Vec<SelectOption>, selected: &str, _cx: &mut Context<Self>) -> Select {
        let selected = options
            .iter()
            .position(|option| option.value.as_ref() == selected);
        Select {
            options,
            selected,
            open: false,
            placeholder: SharedString::from(""),
            width: None,
        }
    }

    /// Shown on the trigger while nothing is chosen.
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Select {
        self.placeholder = text.into();
        self
    }

    /// Fixes the trigger's width in CSS pixels.
    pub fn width(mut self, css_px: f32) -> Select {
        self.width = Some(css_px);
        self
    }

    /// The value of the chosen option, if any.
    pub fn selected_value(&self) -> Option<&SharedString> {
        self.selected
            .and_then(|ix| self.options.get(ix))
            .map(|option| &option.value)
    }

    /// Replaces the options. The choice is kept if its value is still there.
    pub fn set_options(&mut self, options: Vec<SelectOption>, cx: &mut Context<Self>) {
        let previous = self.selected_value().cloned();
        self.selected =
            previous.and_then(|value| options.iter().position(|option| option.value == value));
        self.options = options;
        cx.notify();
    }

    /// Chooses by value without emitting `Changed`, for showing stored state.
    pub fn set_selected(&mut self, value: &str, cx: &mut Context<Self>) {
        self.selected = self
            .options
            .iter()
            .position(|option| option.value.as_ref() == value);
        cx.notify();
    }

    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.open = false;
        if self.selected == Some(ix) {
            cx.notify();
            return;
        }
        if let Some(option) = self.options.get(ix) {
            let value = option.value.clone();
            self.selected = Some(ix);
            cx.emit(SelectEvent::Changed(value));
        }
        cx.notify();
    }
}

impl Render for Select {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let shown = self
            .selected
            .and_then(|ix| self.options.get(ix))
            .map(|option| option.label.clone())
            .unwrap_or_else(|| self.placeholder.clone());
        let placeholder_shown = self.selected.is_none();
        let weak = cx.entity().downgrade();
        let dismiss_weak = weak.clone();

        let text_colour = if placeholder_shown {
            with_alpha(colors.muted_foreground, 0.7)
        } else {
            colors.foreground
        };
        let trigger = div()
            .id("select-trigger")
            .flex()
            .items_center()
            .gap(rpx(8.))
            .h(rpx(36.))
            .px(rpx(12.))
            .rounded(rpx(radius::LG))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface_strong)
            .text_size(rpx(13.))
            .text_color(text_colour)
            .cursor_pointer()
            .when_some(self.width, |d, width| d.w(rpx(width)))
            .when(self.width.is_none(), |d| d.w_full())
            .child(div().min_w_0().flex_1().truncate().child(shown))
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(14.)
                    .color(colors.muted_foreground),
            )
            .on_click(move |_, _, cx| {
                weak.update(cx, |select, cx| {
                    select.open = !select.open;
                    cx.notify();
                })
                .ok();
            });

        let mut list = div()
            .flex()
            .flex_col()
            .gap(rpx(2.))
            .p(rpx(4.))
            .min_w(rpx(160.));
        for (ix, option) in self.options.iter().enumerate() {
            let is_selected = self.selected == Some(ix);
            let row_weak = cx.entity().downgrade();
            let row = div()
                .id(SharedString::from(format!("select-option-{ix}")))
                .flex()
                .items_center()
                .gap(rpx(8.))
                .h(rpx(30.))
                .px(rpx(8.))
                .rounded(rpx(radius::MD))
                .text_size(rpx(12.5))
                .cursor_pointer()
                .hover(move |s| s.bg(colors.accent))
                .when(is_selected, |d| d.font_weight(FontWeight::MEDIUM))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .child(option.label.clone()),
                )
                .when(is_selected, |d| {
                    d.child(
                        Icon::new(IconName::Check)
                            .size(14.)
                            .color(colors.foreground),
                    )
                })
                .on_click(move |_, _, cx| {
                    row_weak.update(cx, |select, cx| select.choose(ix, cx)).ok();
                });
            list = list.child(row);
        }

        let open = self.open;
        Popover::new("select-popover")
            .trigger(trigger)
            .open(open)
            .content(list)
            .on_dismiss(move |_, cx| {
                dismiss_weak
                    .update(cx, |select, cx| {
                        select.open = false;
                        cx.notify();
                    })
                    .ok();
            })
    }
}
