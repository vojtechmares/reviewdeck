//! Port of src/renderer/src/components/ScheduleDialog.tsx: the review windows - the
//! times of day Reviewdeck may interrupt - and the editor for one of them.
//!
//! The windows live in the settings (`reviewWindows`) and are saved through
//! [`AppState::set_settings`] the moment a window is saved or removed. The window being
//! edited is a draft held here until Save; `core::review_window::window_problem` decides
//! whether it can be saved and says why not.

use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder,
    relative,
};
use reviewdeck_core::model::{Account, ReviewWindow};
use reviewdeck_core::review_window::{
    covers_nothing, day_name, describe_window, minutes_of_day, window_problem,
};

use crate::state::{AppState, GlobalState};
use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::components::dialog::Dialog;
use crate::ui::components::glass::GlassExt;
use crate::ui::components::input::{TextInput, TextInputEvent, label};
use crate::ui::components::switch::Checkbox;
use crate::ui::components::toast::ToastKind;
use crate::ui::icons::IconName;
use crate::ui::theme::{ActiveTheme, UI_FONT, radius, rpx};
use crate::ui::thread_view::{probe, say};

/// Monday first, because that is how a working week is read.
const WEEK: [u32; 7] = [1, 2, 3, 4, 5, 6, 0];

const WEEKDAYS: [u32; 5] = [1, 2, 3, 4, 5];

/// A new window: weekday mornings, half an hour, one review, every account.
fn blank_window() -> ReviewWindow {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    ReviewWindow {
        id: format!("window-{millis:x}-{serial:x}"),
        enabled: true,
        days: WEEKDAYS.to_vec(),
        start: "09:00".into(),
        end: "09:30".into(),
        minimum: 1,
        // Empty is every account, now and in future.
        accounts: Vec::new(),
    }
}

/// "9:30" and " 09:30 " as "09:30"; anything `minutes_of_day` cannot read as it was.
fn normalise_time(text: &str) -> String {
    match minutes_of_day(text) {
        Some(minutes) => format!("{:02}:{:02}", minutes / 60, minutes % 60),
        None => text.to_string(),
    }
}

/// `Math.trunc(Number(text))`. What cannot be read as a number is 0, which
/// `window_problem` then refuses with its own message.
fn parse_minimum(text: &str) -> i64 {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .map_or(0, |number| number.trunc() as i64)
}

/// `ScheduleDialog`. Render it as the last child of a `size_full` root, while open; it
/// emits [`DismissEvent`] when it wants to close.
pub struct ScheduleDialog {
    state: Entity<AppState>,
    focus: FocusHandle,
    accounts: Vec<Account>,
    /// The window being edited, or `None` while the list shows.
    editing: Option<ReviewWindow>,
    start: Entity<TextInput>,
    end: Entity<TextInput>,
    minimum: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for ScheduleDialog {}

impl Focusable for ScheduleDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScheduleDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        let focus = cx.focus_handle();
        focus.focus(window);

        let start = cx.new(|cx| TextInput::new(cx).placeholder("09:00"));
        let end = cx.new(|cx| TextInput::new(cx).placeholder("09:30"));
        let minimum = cx.new(|cx| TextInput::new(cx).placeholder("1"));

        let mut subscriptions = vec![cx.observe(&state, |this, state, cx| {
            this.accounts = state.read(cx).accounts();
            cx.notify();
        })];
        // `patch({ start })`, `patch({ end })`, `patch({ minimum })` on every keystroke.
        for (input, field) in [
            (&start, Field::Start),
            (&end, Field::End),
            (&minimum, Field::Minimum),
        ] {
            subscriptions.push(cx.subscribe(
                input,
                move |this, input, event: &TextInputEvent, cx| {
                    match event {
                        TextInputEvent::Changed => {
                            let text = input.read(cx).text().to_string();
                            this.patch(cx, |window| match field {
                                Field::Start => window.start = text,
                                Field::End => window.end = text,
                                Field::Minimum => window.minimum = parse_minimum(&text),
                            });
                        }
                        // Escape in a field travels on to the Dialog, which closes.
                        TextInputEvent::Cancel | TextInputEvent::Submit => {}
                    }
                },
            ));
        }

        let accounts = state.read(cx).accounts();
        ScheduleDialog {
            state,
            focus,
            accounts,
            editing: None,
            start,
            end,
            minimum,
            _subscriptions: subscriptions,
        }
    }

    fn windows(&self, cx: &App) -> Vec<ReviewWindow> {
        self.state.read(cx).settings().review_windows
    }

    /// Opens the editor on `window`, filling the fields from it.
    fn edit(&mut self, window: ReviewWindow, cx: &mut Context<Self>) {
        let (start, end, minimum) = (
            window.start.clone(),
            window.end.clone(),
            window.minimum.to_string(),
        );
        self.start.update(cx, |input, cx| input.set_text(start, cx));
        self.end.update(cx, |input, cx| input.set_text(end, cx));
        self.minimum
            .update(cx, |input, cx| input.set_text(minimum, cx));
        self.editing = Some(window);
        cx.notify();
    }

    /// `patch`: changes the window being edited.
    fn patch(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ReviewWindow)) {
        if let Some(window) = &mut self.editing {
            change(window);
            cx.notify();
        }
    }

    /// `save`: adds the window or replaces the one with its id, and goes back to the list.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(mut editing) = self.editing.clone() else {
            return;
        };
        if window_problem(&editing).is_some() {
            return;
        }
        // A `type="time"` field only ever hands over `HH:MM`; a text field also lets
        // "9:30" and " 09:30" through the check, and the summary would print them as
        // typed. Store what the browser would have.
        editing.start = normalise_time(&editing.start);
        editing.end = normalise_time(&editing.end);
        let mut next = self.windows(cx);
        match next.iter_mut().find(|window| window.id == editing.id) {
            Some(existing) => *existing = editing,
            None => next.push(editing),
        }
        self.store(next, cx);
        self.editing = None;
        cx.notify();
    }

    /// `remove`.
    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut next = self.windows(cx);
        next.retain(|window| window.id != id);
        self.store(next, cx);
        cx.notify();
    }

    fn store(&mut self, windows: Vec<ReviewWindow>, cx: &mut Context<Self>) {
        let result = self.state.update(cx, |state, cx| {
            state.set_settings(|settings| settings.review_windows = windows, cx)
        });
        if let Err(error) = result {
            say(cx, ToastKind::Bad, error.to_string());
        }
    }

    fn toggle_account(&mut self, account_id: &str, cx: &mut Context<Self>) {
        self.patch(cx, |window| {
            if window.accounts.iter().any(|entry| entry == account_id) {
                window.accounts.retain(|entry| entry != account_id);
            } else {
                window.accounts.push(account_id.to_string());
            }
        });
    }

    fn toggle_day(&mut self, day: u32, cx: &mut Context<Self>) {
        self.patch(cx, |window| {
            if window.days.contains(&day) {
                window.days.retain(|entry| *entry != day);
            } else {
                window.days.push(day);
            }
        });
    }

    /// A [`Checkbox`] change handler that runs against the dialog.
    fn checked_handler(
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
    ) -> impl Fn(bool, &mut Window, &mut App) + 'static {
        let weak = cx.entity().downgrade();
        move |value, _, cx| {
            weak.update(cx, |this, cx| handler(this, value, cx)).ok();
        }
    }

    fn close_handler(cx: &mut Context<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        let weak = cx.entity().downgrade();
        move |_, cx| {
            weak.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
        }
    }
}

#[derive(Clone, Copy)]
enum Field {
    Start,
    End,
    Minimum,
}

fn hint(text: &'static str, cx: &App) -> gpui::Div {
    div()
        .text_size(rpx(11.))
        .text_color(cx.theme().colors.muted_foreground)
        .child(text)
}

impl ScheduleDialog {
    fn render_list(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;
        let windows = self.windows(cx);
        let account_ids: Vec<&str> = self.accounts.iter().map(|a| a.id.as_str()).collect();

        let mut list = div().flex().flex_col().gap(rpx(8.));
        for (index, window) in windows.iter().enumerate() {
            let summary = describe_window(window, &self.accounts);
            let edit_window = window.clone();
            let remove_id = window.id.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(12.))
                    .px(rpx(12.))
                    .py(rpx(10.))
                    .rounded(rpx(radius::LG))
                    .glass(cx)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(rpx(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(summary.clone()),
                            )
                            .when(!window.enabled, |d| {
                                d.child(
                                    div()
                                        .text_size(rpx(11.5))
                                        .text_color(colors.muted_foreground)
                                        .child("Turned off"),
                                )
                            })
                            .when(covers_nothing(window, &account_ids), |d| {
                                d.child(
                                    div()
                                        .text_size(rpx(11.5))
                                        .text_color(colors.bad)
                                        .child("Every account it named has been signed out, so it can never fire."),
                                )
                            }),
                    )
                    .child(probe(
                        format!("edit-window-{index}"),
                        Button::new(SharedString::from(format!("edit-{}", window.id)))
                            .variant(ButtonVariant::Ghost)
                            .icon_only(IconName::Pencil)
                            .tooltip(format!("Edit {summary}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(edit_window.clone(), cx)
                            })),
                    ))
                    .child(probe(
                        format!("remove-window-{index}"),
                        Button::new(SharedString::from(format!("remove-{}", window.id)))
                            .variant(ButtonVariant::Ghost)
                            .icon_only(IconName::Trash2)
                            .tooltip(format!("Remove {summary}"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove(&remove_id, cx)
                            })),
                    )),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(rpx(8.))
            .child(list)
            .when(windows.is_empty(), |d| {
                d.child(
                    div()
                        .py(rpx(32.))
                        .flex()
                        .justify_center()
                        .text_size(rpx(13.))
                        .text_color(colors.muted_foreground)
                        .child("No windows yet, so Reviewdeck notifies you whenever a poll finds something."),
                )
            })
            .child(
                div()
                    .text_size(rpx(11.))
                    .line_height(relative(1.625))
                    .text_color(colors.muted_foreground)
                    .child("A window only fires while Reviewdeck is running. If you want the morning roll-up to be there before you are, turn on “Launch Reviewdeck at login” in Settings."),
            )
    }

    fn render_editor(&self, editing: &ReviewWindow, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;

        let mut days = div().flex().gap(rpx(6.));
        for day in WEEK {
            let active = editing.days.contains(&day);
            days = days.child(probe(
                format!("day-{day}"),
                div()
                    .id(SharedString::from(format!("day-{day}")))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .h(rpx(32.))
                    .rounded(rpx(radius::LG))
                    .border_1()
                    .text_size(rpx(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .map(|d| {
                        if active {
                            d.border_color(colors.border_strong)
                                .bg(colors.surface_strong)
                        } else {
                            d.border_color(colors.border)
                                .text_color(colors.muted_foreground)
                                .hover(move |s| s.bg(colors.accent).text_color(colors.foreground))
                        }
                    })
                    .child(day_name(day))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_day(day, cx))),
            ));
        }

        let mut scope = div().flex().flex_col().gap(rpx(6.)).child(probe(
            "all-accounts",
            Checkbox::new("all-accounts")
                .label("All accounts")
                .checked(editing.accounts.is_empty())
                // Ticking it clears the selection, because empty is what "every account"
                // means - including the ones connected after today.
                .on_change(Self::checked_handler(cx, |this, _, cx| {
                    this.patch(cx, |w| w.accounts.clear())
                })),
        ));
        for account in &self.accounts {
            let id = account.id.clone();
            scope = scope.child(
                div().pl(rpx(16.)).child(probe(
                    format!("account-{}", account.label),
                    Checkbox::new(SharedString::from(format!("account-{}", account.id)))
                        .label(account.label.clone())
                        .checked(editing.accounts.contains(&account.id))
                        .on_change(Self::checked_handler(cx, move |this, _, cx| {
                            this.toggle_account(&id, cx)
                        })),
                )),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(rpx(14.))
            .child(div().child(label("Days", cx)).child(days))
            .child(
                div()
                    .flex()
                    .gap(rpx(12.))
                    .child(
                        div()
                            .flex_1()
                            .child(label("From", cx))
                            .child(self.start.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(label("Until", cx))
                            .child(self.end.clone()),
                    ),
            )
            .child(
                // `-mt-2`: the note belongs to the two time fields above it.
                hint(
                    "A window cannot cross midnight. For 22:00 to 02:00, make two.",
                    cx,
                )
                .mt(rpx(-8.)),
            )
            .child(
                div()
                    .child(label("Only if this many reviews are waiting", cx))
                    .child(self.minimum.clone())
                    .child(
                        hint(
                            "Checked for as long as the window is open, not just as it opens - so a window that starts quiet still fires the moment the count is reached.",
                            cx,
                        )
                        .mt(rpx(4.)),
                    ),
            )
            .child(
                div()
                    .child(label("Accounts", cx))
                    .child(scope)
                    .when(self.accounts.is_empty(), |d| {
                        d.child(
                            hint(
                                "No accounts connected yet, so this window covers whatever you add.",
                                cx,
                            )
                            .mt(rpx(4.)),
                        )
                    }),
            )
            .child(
                probe(
                    "window-enabled",
                    Checkbox::new("window-enabled")
                        .label("Use this window")
                        .checked(editing.enabled)
                        .on_change(Self::checked_handler(cx, |this, value, cx| {
                            this.patch(cx, |w| w.enabled = value)
                        })),
                ),
            )
    }
}

impl Render for ScheduleDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let editing = self.editing.clone();
        let problem = editing.as_ref().and_then(window_problem);

        let footer = match &editing {
            Some(_) => div()
                .flex()
                .flex_1()
                .items_center()
                .gap(rpx(8.))
                .child(
                    div()
                        .flex_1()
                        .text_size(rpx(11.5))
                        .text_color(colors.bad)
                        .children(problem),
                )
                .child(probe(
                    "schedule-cancel",
                    Button::new("schedule-cancel")
                        .variant(ButtonVariant::Ghost)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.editing = None;
                            cx.notify();
                        }))
                        .child("Cancel"),
                ))
                .child(probe(
                    "schedule-save",
                    Button::new("schedule-save")
                        .variant(ButtonVariant::Default)
                        .disabled(problem.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                        .child("Save"),
                )),
            None => div().child(probe(
                "schedule-add",
                Button::new("schedule-add")
                    .variant(ButtonVariant::Default)
                    .icon(IconName::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.edit(blank_window(), cx)))
                    .child("Add window"),
            )),
        };

        let body = match &editing {
            Some(window) => self.render_editor(window, cx),
            None => self.render_list(cx),
        };

        Dialog::new("Review schedule", self.focus.clone())
            .description("While a window is open Reviewdeck may interrupt you. Outside every window it stays quiet and lets reviews collect. No windows means it notifies whenever it finds something.")
            .width(576.)
            .on_close(Self::close_handler(cx))
            .footer(footer)
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(rpx(13.))
                    .line_height(relative(1.5))
                    .text_color(colors.foreground)
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_is_truncated_and_unreadable_text_is_zero() {
        assert_eq!(parse_minimum("3"), 3);
        assert_eq!(parse_minimum(" 2.9 "), 2);
        assert_eq!(parse_minimum("-1.5"), -1);
        assert_eq!(parse_minimum(""), 0);
        assert_eq!(parse_minimum("abc"), 0);
    }

    #[test]
    fn a_blank_window_is_valid_and_ids_differ() {
        let first = blank_window();
        let second = blank_window();
        assert_eq!(window_problem(&first), None);
        assert_ne!(first.id, second.id);
    }
}

#[cfg(test)]
#[path = "schedule_dialog_tests.rs"]
mod dialog_tests;
