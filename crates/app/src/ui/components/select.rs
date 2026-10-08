//! The select from ui/input.tsx, and the filter dropdowns in App.tsx.
//!
//! A native `<select>` has no gpui equivalent, so this is a trigger field and a list in a
//! [`Popover`]. The trigger carries the `FIELD` look with the chevron the TSX draws as a
//! background image. The list is a glass panel, the way the platform menu would be.
//!
//! A `Select` is an entity, because it holds its open state and the chosen option. Create it
//! with `cx.new(|cx| Select::new(options, selected, cx))`, subscribe to [`SelectEvent`] for
//! changes, and render it as a child: `.child(select.clone())`.
//!
//! # Keyboard
//!
//! The trigger is a tab stop and keeps focus while the list is open, so one set of keys does
//! it all, as a native select on macOS:
//!
//! - closed: Enter, Space, Up and Down open the list on the current choice; typing a letter
//!   chooses the next option that starts with what was typed (type-ahead);
//! - open: Up and Down move the highlight, Home and End jump to the ends, Enter or Space
//!   chooses the highlighted option, Escape closes the list (and only the list: Escape on a
//!   closed select travels on, so a dialog around it still closes), typing moves the
//!   highlight by prefix;
//! - the list closes when focus leaves the trigger.
//!
//! The keys are actions in the `Select` key context ([`bind_keys`], called by the kit's own
//! `bind_keys`), not key listeners: gpui runs bound actions before listeners, so a plain
//! listener would lose Up and Down to any ancestor that binds them. For the same reason a
//! window-level `j`/`k` binding should exclude this context (`!Select`), or typing those
//! letters into the select would move the window's selection instead.

use std::time::Duration;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, actions, div, prelude::FluentBuilder,
};

use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, radius, rpx};

use super::button::with_alpha;
use super::popover::Popover;

actions!(
    select,
    [Cancel, Confirm, MoveUp, MoveDown, MoveFirst, MoveLast]
);

/// The key context of the trigger.
pub const KEY_CONTEXT: &str = "Select";

/// How long a pause ends a type-ahead word.
const TYPEAHEAD_RESET: Duration = Duration::from_millis(1000);

/// Registers the select's keys. Called by [`super::bind_keys`].
pub fn bind_keys(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("enter", Confirm, c),
        KeyBinding::new("space", Confirm, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("home", MoveFirst, c),
        KeyBinding::new("end", MoveLast, c),
    ]);
}

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
    /// The row the keyboard (or the pointer) is on while the list is open.
    highlighted: usize,
    placeholder: SharedString,
    /// Trigger width in CSS pixels. Without one the trigger fills its parent.
    width: Option<f32>,
    /// The `h-7 rounded-md px-2 text-[11.5px]` look of the filter bar's selects.
    compact: bool,
    focus: FocusHandle,
    /// What has been typed for type-ahead, and when the last letter came.
    typeahead: String,
    /// Ends the type-ahead word after a pause; replaced (and so cancelled) on every key.
    typeahead_reset: Option<Task<()>>,
}

impl EventEmitter<SelectEvent> for Select {}

impl Focusable for Select {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Select {
    /// `selected` is the value to start on; an unknown value starts with nothing chosen.
    pub fn new(options: Vec<SelectOption>, selected: &str, cx: &mut Context<Self>) -> Select {
        let selected = options
            .iter()
            .position(|option| option.value.as_ref() == selected);
        Select {
            options,
            selected,
            open: false,
            highlighted: selected.unwrap_or(0),
            placeholder: SharedString::from(""),
            width: None,
            compact: false,
            focus: cx.focus_handle().tab_stop(true),
            typeahead: String::new(),
            typeahead_reset: None,
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

    /// The small filter-bar look: `h-7 rounded-md px-2 text-[11.5px]` instead of `h-9`.
    pub fn compact(mut self) -> Select {
        self.compact = true;
        self
    }

    pub fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        self.compact = compact;
        cx.notify();
    }

    /// The value of the chosen option, if any.
    pub fn selected_value(&self) -> Option<&SharedString> {
        self.selected
            .and_then(|ix| self.options.get(ix))
            .map(|option| &option.value)
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The index of the highlighted row while the list is open.
    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    /// Replaces the options. The choice is kept if its value is still there.
    pub fn set_options(&mut self, options: Vec<SelectOption>, cx: &mut Context<Self>) {
        let previous = self.selected_value().cloned();
        self.selected =
            previous.and_then(|value| options.iter().position(|option| option.value == value));
        self.options = options;
        self.highlighted = self
            .selected
            .unwrap_or(0)
            .min(self.options.len().saturating_sub(1));
        cx.notify();
    }

    /// Chooses by value without emitting `Changed`, for showing stored state.
    pub fn set_selected(&mut self, value: &str, cx: &mut Context<Self>) {
        self.selected = self
            .options
            .iter()
            .position(|option| option.value.as_ref() == value);
        if let Some(ix) = self.selected {
            self.highlighted = ix;
        }
        cx.notify();
    }

    fn open_list(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.highlighted = self
            .selected
            .unwrap_or(0)
            .min(self.options.len().saturating_sub(1));
        cx.notify();
    }

    fn close_list(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
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
            self.highlighted = ix;
            cx.emit(SelectEvent::Changed(value));
        }
        cx.notify();
    }

    // ---- keyboard ----

    fn on_cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close_list(cx);
        } else {
            // Not ours: let a dialog (or anything else bound to Escape) have it.
            cx.propagate();
        }
    }

    fn on_confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            let ix = self.highlighted;
            self.choose(ix, cx);
        } else if !self.options.is_empty() {
            self.open_list(cx);
        }
    }

    fn on_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            if !self.options.is_empty() {
                self.open_list(cx);
            }
        } else {
            self.highlighted = self.highlighted.saturating_sub(1);
            cx.notify();
        }
    }

    fn on_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            if !self.options.is_empty() {
                self.open_list(cx);
            }
        } else {
            let last = self.options.len().saturating_sub(1);
            self.highlighted = (self.highlighted + 1).min(last);
            cx.notify();
        }
    }

    fn on_move_first(&mut self, _: &MoveFirst, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlighted = 0;
            cx.notify();
        }
    }

    fn on_move_last(&mut self, _: &MoveLast, _: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.highlighted = self.options.len().saturating_sub(1);
            cx.notify();
        }
    }

    /// Type-ahead: the typed prefix picks the first option (after the current one when the
    /// same letter is repeated) whose label starts with it, ignoring case.
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        if stroke.modifiers.control || stroke.modifiers.platform || stroke.modifiers.alt {
            return;
        }
        let Some(typed) = stroke
            .key_char
            .as_deref()
            .filter(|c| c.chars().all(|ch| !ch.is_control()) && !c.trim().is_empty())
        else {
            return;
        };
        self.typeahead.push_str(&typed.to_lowercase());
        self.typeahead_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPEAHEAD_RESET).await;
            this.update(cx, |select, _| select.typeahead.clear()).ok();
        }));

        let count = self.options.len();
        if count == 0 {
            return;
        }
        // One repeated letter cycles through the options with that initial.
        let repeated = self
            .typeahead
            .chars()
            .all(|ch| self.typeahead.starts_with(ch));
        let prefix: String = if repeated {
            self.typeahead.chars().take(1).collect()
        } else {
            self.typeahead.clone()
        };
        // The row to start from: the open list's highlight, else the chosen option, else
        // nothing (start at the top). A single letter, or the same letter again, moves on to
        // the next match after it; a longer prefix may keep the row it is on.
        let current = if self.open {
            Some(self.highlighted)
        } else {
            self.selected
        };
        let start = match current {
            None => 0,
            Some(ix) if prefix.chars().count() == 1 => ix + 1,
            Some(ix) => ix,
        };
        let found = (0..count)
            .map(|offset| (start + offset) % count)
            .find(|&ix| self.options[ix].label.to_lowercase().starts_with(&prefix));
        let Some(ix) = found else {
            return;
        };
        if self.open {
            self.highlighted = ix;
            cx.notify();
        } else {
            self.choose(ix, cx);
        }
    }
}

impl Render for Select {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let focused = self.focus.is_focused(window);
        // The list belongs to the focus: it closes when the trigger loses it.
        if self.open && !focused {
            self.open = false;
        }
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
        let (height, corner, pad_x, font) = if self.compact {
            (28., radius::MD, 8., 11.5)
        } else {
            (36., radius::LG, 12., 13.)
        };
        // `pr-8` reserves room for the chevron, which sits `0.65rem` from the right edge.
        let pad_right = if self.compact { 24. } else { 32. };
        let chevron_right = if self.compact { 6. } else { 10.4 } - 1.;
        let chevron_size = 14.;
        let trigger = div()
            .id("select-trigger")
            .debug_selector(|| "select-trigger".to_string())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .flex()
            .items_center()
            .h(rpx(height))
            .pl(rpx(pad_x))
            .pr(rpx(pad_right))
            .rounded(rpx(corner))
            .border_1()
            .border_color(if focused {
                colors.border_strong
            } else {
                colors.border
            })
            .bg(colors.surface_strong)
            .text_size(rpx(font))
            .text_color(text_colour)
            .cursor_pointer()
            .when_some(self.width, |d, width| d.w(rpx(width)))
            .when(self.width.is_none(), |d| d.w_full())
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_move_up))
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_move_first))
            .on_action(cx.listener(Self::on_move_last))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(div().min_w_0().flex_1().truncate().child(shown))
            .child(
                div()
                    .absolute()
                    .top(rpx((height - 2. - chevron_size) / 2.))
                    .right(rpx(chevron_right))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(chevron_size)
                            .color(colors.muted_foreground),
                    ),
            )
            // Mouse down, not click: a keyboard "click" would reopen the list that Enter
            // has just closed. The press also focuses the trigger.
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                weak.update(cx, |select, cx| {
                    if select.open {
                        select.close_list(cx);
                    } else {
                        select.open_list(cx);
                    }
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
            let is_highlighted = self.highlighted == ix;
            let row_weak = cx.entity().downgrade();
            let hover_weak = cx.entity().downgrade();
            let row = div()
                .id(SharedString::from(format!("select-option-{ix}")))
                .debug_selector(move || format!("select-option-{ix}"))
                .flex()
                .items_center()
                .gap(rpx(8.))
                .h(rpx(30.))
                .px(rpx(8.))
                .rounded(rpx(radius::MD))
                .text_size(rpx(12.5))
                .text_color(colors.foreground)
                .cursor_pointer()
                .when(is_highlighted, |d| d.bg(colors.accent))
                .when(is_selected, |d| d.font_weight(FontWeight::MEDIUM))
                // The pointer moves the highlight too, so keys continue from where it is.
                .on_hover(move |hovered, _, cx| {
                    if *hovered {
                        hover_weak
                            .update(cx, |select, cx| {
                                if select.highlighted != ix {
                                    select.highlighted = ix;
                                    cx.notify();
                                }
                            })
                            .ok();
                    }
                })
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
            // The trigger keeps focus: the keys above are its own.
            .autofocus(false)
            .on_dismiss(move |_, cx| {
                dismiss_weak
                    .update(cx, |select, cx| select.close_list(cx))
                    .ok();
            })
    }
}
