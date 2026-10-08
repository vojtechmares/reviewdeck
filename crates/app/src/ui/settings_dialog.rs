//! Port of src/renderer/src/components/SettingsDialog.tsx: every setting, with its
//! control and its copy.
//!
//! Each control saves as it changes, through [`AppState::set_settings`] (the TSX called
//! `updateSettings` on every change). There is no "apply" step, and "Done" only closes.

use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder,
};
use reviewdeck_core::model::{DEFAULT_AGENT_COMMAND, DiffViewMode, Settings, ThemeMode};

use crate::platform::login_item;
use crate::state::{AppState, GlobalState};
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant};
use crate::ui::components::dialog::Dialog;
use crate::ui::components::glass::GlassExt;
use crate::ui::components::input::{TextInput, TextInputEvent};
use crate::ui::components::select::{Select, SelectEvent, SelectOption};
use crate::ui::components::switch::Switch;
use crate::ui::components::toast::ToastKind;
use crate::ui::theme::{ActiveTheme, UI_FONT, radius, rpx};
use crate::ui::thread_view::{probe, say};

/// The dialogs one can open another from. `AppView` renders at most one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Accounts,
    Settings,
    Schedule,
}

/// A request from a dialog to swap itself for another: "the schedule replaces settings
/// rather than stacking" (App.tsx). The owner closes the sender and opens the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogEvent {
    Open(DialogKind),
}

/// `SettingsDialog`. Render it as the last child of a `size_full` root, while open; it
/// emits [`DismissEvent`] to close and [`DialogEvent::Open`] to hand over to the schedule.
pub struct SettingsDialog {
    state: Entity<AppState>,
    focus: FocusHandle,
    poll_interval: Entity<Select>,
    check_poll_interval: Entity<Select>,
    theme: Entity<Select>,
    diff_view: Entity<Select>,
    agent_command: Entity<TextInput>,
    /// Why "Launch at login" did not take, when the system refused.
    login_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for SettingsDialog {}
impl EventEmitter<DialogEvent> for SettingsDialog {}

impl Focusable for SettingsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The poll intervals offered, in seconds, and how they read.
const POLL_INTERVALS: [(&str, &str); 5] = [
    ("60", "1 minute"),
    ("180", "3 minutes"),
    ("300", "5 minutes"),
    ("900", "15 minutes"),
    ("1800", "30 minutes"),
];

const CHECK_INTERVALS: [(&str, &str); 4] = [
    ("20", "20 seconds"),
    ("45", "45 seconds"),
    ("90", "90 seconds"),
    ("180", "3 minutes"),
];

const THEMES: [(&str, &str); 3] = [
    ("system", "Match macOS"),
    ("light", "Light"),
    ("dark", "Dark"),
];

const DIFF_VIEWS: [(&str, &str); 2] = [("split", "Side by side"), ("unified", "Unified")];

fn options(pairs: &[(&'static str, &'static str)]) -> Vec<SelectOption> {
    pairs
        .iter()
        .map(|&(value, label)| SelectOption::new(value, label))
        .collect()
}

fn theme_value(theme: ThemeMode) -> &'static str {
    match theme {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

/// The theme a select value stands for.
fn theme_from_value(value: &str) -> ThemeMode {
    match value {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::System,
    }
}

/// The diff layout a select value stands for.
fn diff_view_from_value(value: &str) -> DiffViewMode {
    match value {
        "unified" => DiffViewMode::Unified,
        _ => DiffViewMode::Split,
    }
}

fn diff_view_value(mode: DiffViewMode) -> &'static str {
    match mode {
        DiffViewMode::Split => "split",
        DiffViewMode::Unified => "unified",
    }
}

impl SettingsDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        let focus = cx.focus_handle();
        focus.focus(window);
        let settings = state.read(cx).settings();

        let poll_interval = cx.new(|cx| {
            Select::new(
                options(&POLL_INTERVALS),
                &settings.poll_interval.to_string(),
                cx,
            )
            .width(160.)
        });
        let check_poll_interval = cx.new(|cx| {
            Select::new(
                options(&CHECK_INTERVALS),
                &settings.check_poll_interval.to_string(),
                cx,
            )
            .width(160.)
        });
        let theme =
            cx.new(|cx| Select::new(options(&THEMES), theme_value(settings.theme), cx).width(160.));
        let diff_view = cx.new(|cx| {
            Select::new(
                options(&DIFF_VIEWS),
                diff_view_value(settings.diff_view),
                cx,
            )
            .width(160.)
        });
        let agent_command = cx.new(|cx| {
            TextInput::new(cx)
                .placeholder(DEFAULT_AGENT_COMMAND)
                .with_text(settings.agent_command.clone())
        });

        let mut subscriptions = vec![cx.observe(&state, |_, _, cx| cx.notify())];
        subscriptions.push(cx.subscribe(&poll_interval, |this, _, event, cx| {
            let SelectEvent::Changed(value) = event;
            if let Ok(seconds) = value.parse::<u32>() {
                this.apply(cx, move |s| s.poll_interval = seconds);
            }
        }));
        subscriptions.push(cx.subscribe(&check_poll_interval, |this, _, event, cx| {
            let SelectEvent::Changed(value) = event;
            if let Ok(seconds) = value.parse::<u32>() {
                this.apply(cx, move |s| s.check_poll_interval = seconds);
            }
        }));
        subscriptions.push(cx.subscribe(&theme, |this, _, event, cx| {
            let SelectEvent::Changed(value) = event;
            let theme = theme_from_value(value);
            this.apply(cx, move |s| s.theme = theme);
        }));
        subscriptions.push(cx.subscribe(&diff_view, |this, _, event, cx| {
            let SelectEvent::Changed(value) = event;
            let mode = diff_view_from_value(value);
            this.apply(cx, move |s| s.diff_view = mode);
        }));
        subscriptions.push(
            cx.subscribe(&agent_command, |this, input, event, cx| match event {
                // The command is saved as it is typed, as the TSX's `onChange` did.
                TextInputEvent::Changed => {
                    let command = input.read(cx).text().to_string();
                    this.apply(cx, move |s| s.agent_command = command);
                }
                TextInputEvent::Cancel => cx.emit(DismissEvent),
                TextInputEvent::Submit => {}
            }),
        );

        SettingsDialog {
            state,
            focus,
            poll_interval,
            check_poll_interval,
            theme,
            diff_view,
            agent_command,
            login_error: None,
            _subscriptions: subscriptions,
        }
    }

    /// `updateSettings`: applies and saves a patch. A failure to save is shown in a toast
    /// (the TSX left it as an unhandled rejection).
    fn apply(&mut self, cx: &mut Context<Self>, patch: impl FnOnce(&mut Settings)) {
        let result = self
            .state
            .update(cx, |state, cx| state.set_settings(patch, cx));
        if let Err(error) = result {
            say(cx, ToastKind::Bad, error.to_string());
        }
        cx.notify();
    }

    /// "Launch Reviewdeck at login". `AppState::set_settings` registers the login item but
    /// only logs a failure, so this asks again: registering what is already registered is
    /// a no-op that succeeds, and a refusal comes back as the sentence to show. When the
    /// system refuses, the switch goes back off rather than claiming something untrue.
    fn set_launch_at_login(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.apply(cx, move |s| s.launch_at_login = enabled);
        self.login_error = match login_item::set_launch_at_login(enabled) {
            Ok(()) => None,
            Err(error) => {
                if enabled {
                    self.apply(cx, |s| s.launch_at_login = false);
                }
                Some(error.into())
            }
        };
        cx.notify();
    }

    fn close_handler(cx: &mut Context<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        let weak = cx.entity().downgrade();
        move |_, cx| {
            weak.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
        }
    }
}

/// `Field`: a label that takes the width and a 160px control.
fn field(label: &'static str, tag: &'static str, control: impl IntoElement) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .gap(rpx(12.))
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.5))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(div().w(rpx(160.)).flex_none().child(probe(tag, control)))
        .into_any_element()
}

/// The `text-[11px] text-muted-foreground` paragraph under a control.
fn note(text: &'static str, cx: &App) -> gpui::AnyElement {
    div()
        .text_size(rpx(11.))
        .line_height(rpx(16.))
        .text_color(cx.theme().colors.muted_foreground)
        .child(text)
        .into_any_element()
}

impl Render for SettingsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (settings, info) = {
            let state = self.state.read(cx);
            (state.settings(), state.app_info())
        };
        let window_count = settings.review_windows.len();

        let toggle = |id: &'static str,
                      label: &'static str,
                      checked: bool,
                      disabled: bool,
                      patch: fn(&mut Settings, bool),
                      cx: &mut Context<Self>| {
            let weak = cx.entity().downgrade();
            // The kit's labelled Switch cannot wrap a long label (its label has no
            // `min-w-0`), so the row is laid out here and only the track is the kit's.
            let row = div()
                .id(id)
                .flex()
                .items_center()
                .gap(rpx(12.))
                .when(disabled, |d| d.opacity(0.45))
                .when(!disabled, |d| d.cursor_pointer())
                .child(div().min_w_0().flex_1().text_size(rpx(12.5)).child(label))
                .child(Switch::new(SharedString::from(format!("{id}-track"))).checked(checked))
                .when(!disabled, |d| {
                    d.on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.apply(cx, |s| patch(s, !checked)))
                            .ok();
                    })
                });
            probe(id, row)
        };

        // Each section is a caption and a glass card; the card's children are listed here.
        let sections = div()
            .flex()
            .flex_col()
            .gap(rpx(16.))
            .child(titled(
                "Syncing",
                vec![
                    field(
                        "Check for new reviews every",
                        "poll-interval",
                        self.poll_interval.clone(),
                    ),
                    field(
                        "Re-poll running checks every",
                        "check-poll-interval",
                        self.check_poll_interval.clone(),
                    ),
                ],
                cx,
            ))
            .child(titled(
                "Notifications",
                vec![
                    toggle(
                        "notifications",
                        "Notify me about new review requests",
                        settings.notifications_enabled,
                        false,
                        |s, v| s.notifications_enabled = v,
                        cx,
                    ),
                    toggle(
                        "play-sound",
                        "Play a sound",
                        settings.play_sound,
                        !settings.notifications_enabled,
                        |s, v| s.play_sound = v,
                        cx,
                    ),
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(12.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(rpx(12.5))
                                .child("Review schedule"),
                        )
                        .child(
                            probe(
                                "open-schedule",
                                Button::new("open-schedule")
                                    .size(ButtonSize::Sm)
                                    .disabled(!settings.notifications_enabled)
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(DialogEvent::Open(DialogKind::Schedule))
                                    }))
                                    .child(schedule_label(window_count)),
                            ),
                        )
                        .into_any_element(),
                    note(
                        "Windows keep the interruption to the part of the day you actually review in: outside them Reviewdeck stays quiet and rolls everything up when the next one opens. With none set it notifies whenever it finds something.",
                        cx,
                    ),
                ],
                cx,
            ))
            .child(titled(
                "Appearance",
                vec![
                    field("Theme", "theme", self.theme.clone()),
                    field("Diff layout", "diff-view", self.diff_view.clone()),
                    toggle(
                        "hide-approved",
                        "Hide pull requests I already approved",
                        settings.hide_approved,
                        false,
                        |s, v| s.hide_approved = v,
                        cx,
                    ),
                    toggle(
                        "hide-fully-approved",
                        "Hide pull requests that already have every required approval",
                        settings.hide_fully_approved,
                        false,
                        |s, v| s.hide_fully_approved = v,
                        cx,
                    ),
                    note(
                        "Only where the host says what it requires and that it has it. A branch with no rule, or one the token cannot read, is never hidden.",
                        cx,
                    ),
                    toggle(
                        "hide-drafts",
                        "Hide draft pull requests",
                        settings.hide_drafts,
                        false,
                        |s, v| s.hide_drafts = v,
                        cx,
                    ),
                    note(
                        "Drafts the host marks as such, plus anything titled Draft:, WIP: or [WIP] - some hosts have no draft flag at all. The deck says how many it is holding back, and one click shows them.",
                        cx,
                    ),
                    toggle(
                        "menu-bar-count",
                        "Show the review count in the menu bar",
                        settings.show_menu_bar_count,
                        false,
                        |s, v| s.show_menu_bar_count = v,
                        cx,
                    ),
                    note(
                        "Off leaves just the Reviewdeck icon up there. The count is still in the menu itself, one click away.",
                        cx,
                    ),
                ],
                cx,
            ))
            .child(titled(
                "Agent handoff",
                vec![
                    field("Command to copy", "agent-command", self.agent_command.clone()),
                    note(
                        "Used by “Copy Claude prompt”. A shell alias works, because it is your own shell that runs it - Reviewdeck only copies the text. Any account can override this.",
                        cx,
                    ),
                ],
                cx,
            ))
            .child(titled(
                "System",
                {
                    let weak = cx.entity().downgrade();
                    let launch = settings.launch_at_login;
                    let row = div()
                            .id("launch-at-login")
                            .flex()
                            .items_center()
                            .gap(rpx(12.))
                            .cursor_pointer()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_size(rpx(12.5))
                                    .child("Launch Reviewdeck at login"),
                            )
                            .child(Switch::new("launch-at-login-track").checked(settings.launch_at_login))
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| this.set_launch_at_login(!launch, cx))
                                    .ok();
                            });
                    let mut rows = vec![probe("launch-at-login", row)];
                    if let Some(error) = &self.login_error {
                        rows.push(
                            div()
                                .text_size(rpx(11.))
                                .line_height(rpx(16.))
                                .text_color(colors.bad)
                                .child(error.clone())
                                .into_any_element(),
                        );
                    }
                    rows
                },
                cx,
            ));

        Dialog::new("Settings", self.focus.clone())
            .width(448.)
            .on_close(Self::close_handler(cx))
            .footer(
                div()
                    .flex_1()
                    .text_size(rpx(11.))
                    .text_color(colors.muted_foreground)
                    .child(format!("Reviewdeck {}", info.version)),
            )
            .footer(probe(
                "settings-done",
                Button::new("settings-done")
                    .variant(ButtonVariant::Default)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
                    .child("Done"),
            ))
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(rpx(13.))
                    .line_height(rpx(20.))
                    .text_color(colors.foreground)
                    .child(sections),
            )
    }
}

/// The schedule button: how many windows are set, or the invitation to set one up.
fn schedule_label(window_count: usize) -> String {
    match window_count {
        0 => "Set up…".to_string(),
        1 => "1 window…".to_string(),
        n => format!("{n} windows…"),
    }
}

/// `Section`: an uppercase caption over a glass card holding the rows.
fn titled(title: &'static str, rows: Vec<gpui::AnyElement>, cx: &App) -> gpui::Div {
    let colors = cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .mb(rpx(8.))
                .text_size(rpx(11.))
                .line_height(rpx(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(10.))
                .p(rpx(12.))
                .rounded(rpx(radius::LG))
                .glass(cx)
                .children(rows),
        )
}

#[cfg(test)]
#[path = "settings_dialog_tests.rs"]
mod tests;
