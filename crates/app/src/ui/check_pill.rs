//! Port of src/renderer/src/components/CheckPill.tsx.

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder,
};
use reviewdeck_core::model::{CheckStatus, CheckSummary};

use crate::ui::components::badge::{Badge, BadgeTone};
use crate::ui::components::popover::Popover;
use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, Colors, rpx};

/// `META` in the TSX: the tone and the word a status is shown as.
fn meta(status: CheckStatus) -> (BadgeTone, &'static str) {
    match status {
        CheckStatus::Passed => (BadgeTone::Ok, "Passed"),
        CheckStatus::Failed => (BadgeTone::Bad, "Failed"),
        CheckStatus::Running => (BadgeTone::Busy, "Running"),
        CheckStatus::Unknown => (BadgeTone::Neutral, "Unknown"),
    }
}

/// The text colour of a badge tone (the `text-ok` etc. half of the badge's classes).
pub fn tone_color(tone: BadgeTone, cx: &App) -> Hsla {
    let colors = cx.theme().colors;
    match tone {
        BadgeTone::Neutral => colors.muted_foreground,
        BadgeTone::Ok => colors.ok,
        BadgeTone::Bad => colors.bad,
        BadgeTone::Busy => colors.busy,
        BadgeTone::Info => colors.info,
    }
}

/// `CHECK_TONE`: the colour a status is drawn in wherever a run is listed rather
/// than rolled up.
pub fn check_tone(status: CheckStatus, colors: &Colors) -> Hsla {
    match status {
        CheckStatus::Passed => colors.ok,
        CheckStatus::Failed => colors.bad,
        CheckStatus::Running => colors.busy,
        CheckStatus::Unknown => colors.muted_foreground,
    }
}

/// `CheckIcon`: the glyph for a status, `size` CSS pixels (the TSX default is 14).
pub fn check_icon(status: CheckStatus, size: f32, color: Hsla) -> Icon {
    let (name, spin) = match status {
        CheckStatus::Passed => (IconName::CheckCircle2, false),
        CheckStatus::Failed => (IconName::XCircle, false),
        CheckStatus::Running => (IconName::Loader2, true),
        CheckStatus::Unknown => (IconName::CircleDashed, false),
    };
    let icon = Icon::new(name).size(size).color(color);
    if spin { icon.spin() } else { icon }
}

/// The words in the badge: `2 failed`, `1 running`, `5/6`, or the status itself when the
/// host reported no runs at all.
fn pill_text(checks: &CheckSummary) -> String {
    if checks.total > 0 {
        match checks.status {
            CheckStatus::Failed => format!("{} failed", checks.failed),
            CheckStatus::Running => format!("{} running", checks.running),
            _ => format!("{}/{}", checks.passed, checks.total),
        }
    } else {
        meta(checks.status).1.to_string()
    }
}

/// What a pill remembers between frames: whether its panel is open, and the focus
/// handle that makes it a tab stop.
struct PillState {
    open: bool,
    focus: FocusHandle,
}

/// Compact CI badge; opening it lists the runs behind the roll-up.
///
/// The badge is the trigger rather than a button wrapping it, because one of the two
/// places this appears is inside the button a review card already is, and a button
/// cannot contain another one. So the click stops there instead of selecting the
/// card, and the pill is a focusable element of its own, which gpui opens on Enter
/// and Space when it has focus - what the role, the tab stop and the key handler put
/// back in the TSX.
///
/// The open state lives in gpui keyed state under `id`, so give each pill on screen
/// its own id.
#[derive(IntoElement)]
pub struct CheckPill {
    id: SharedString,
    checks: CheckSummary,
}

impl CheckPill {
    pub fn new(id: impl Into<SharedString>, checks: &CheckSummary) -> CheckPill {
        CheckPill {
            id: id.into(),
            checks: checks.clone(),
        }
    }
}

/// `<CheckPill checks={..} />` for the one place there is a single pill on screen.
/// Use [`CheckPill::new`] with a distinct id where several can be.
pub fn check_pill(checks: &CheckSummary, _cx: &App) -> CheckPill {
    CheckPill::new("check-pill", checks)
}

impl RenderOnce for CheckPill {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let checks = self.checks;
        let (tone, label) = meta(checks.status);
        let state: Entity<PillState> = window.use_keyed_state(
            SharedString::from(format!("{}-state", self.id)),
            cx,
            |_, cx: &mut Context<PillState>| PillState {
                open: false,
                focus: cx.focus_handle().tab_stop(true),
            },
        );
        // Debug scene: the first deck card's pill opens as if clicked. Cards are drawn
        // top down, so the first pill to render with a card id is the first card's.
        #[cfg(debug_assertions)]
        if self.id.starts_with("pill-")
            && !crate::scene::deck_pending(cx)
            && crate::scene::pending(cx, crate::scene::Kind::Pill).is_some()
        {
            crate::scene::mark_done(cx, crate::scene::Kind::Pill);
            state.update(cx, |state, _| state.open = true);
        }
        let (open, focus) = {
            let state = state.read(cx);
            (state.open, state.focus.clone())
        };

        let text = pill_text(&checks);

        let toggle = state.clone();
        let trigger = div()
            .id(self.id.clone())
            .track_focus(&focus)
            .cursor_pointer()
            .hover(|s| s.opacity(0.8))
            .on_click(move |_: &ClickEvent, _window, cx| {
                // The card around the pill must not also take the click.
                cx.stop_propagation();
                toggle.update(cx, |state, cx| {
                    state.open = !state.open;
                    cx.notify();
                });
            })
            .child(
                Badge::new()
                    .tone(tone)
                    .child(check_icon(checks.status, 12., tone_color(tone, cx)))
                    .child(text),
            );
        #[cfg(test)]
        let trigger = trigger.debug_selector({
            let id = self.id.clone();
            move || format!("{id}")
        });

        let panel = if checks.total == 0 {
            div()
                .w(rpx(288.))
                .my(rpx(-4.))
                .px(rpx(12.))
                .py(rpx(10.))
                .text_size(rpx(11.5))
                .line_height(rpx(15.8125))
                .text_color(colors.muted_foreground)
                .child("No status checks reported for this branch.")
        } else {
            let header = div()
                .flex()
                .items_center()
                .gap(rpx(8.))
                .border_b_1()
                .border_color(colors.border)
                .px(rpx(12.))
                .py(rpx(8.))
                .child(
                    div()
                        .flex_none()
                        .mr(rpx(8.))
                        .size(rpx(14.))
                        .child(check_icon(
                            checks.status,
                            14.,
                            check_tone(checks.status, &colors),
                        )),
                )
                .child(
                    div()
                        .text_size(rpx(12.))
                        .line_height(rpx(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .child(
                    div()
                        .ml_auto()
                        .text_size(rpx(11.))
                        .line_height(rpx(16.5))
                        .text_color(colors.muted_foreground)
                        .child(format!("{}/{} passed", checks.passed, checks.total)),
                );
            let rows = checks.runs.iter().enumerate().map(|(ix, run)| {
                div()
                    .flex()
                    .items_start()
                    .gap(rpx(8.))
                    .px(rpx(12.))
                    .py(rpx(6.))
                    .when(ix > 0, |d| d.border_t_1().border_color(colors.border))
                    .child(div().mt(rpx(2.)).flex_none().child(check_icon(
                        run.status,
                        14.,
                        check_tone(run.status, &colors),
                    )))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(rpx(12.))
                                    .line_height(rpx(16.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(run.name.clone()),
                            )
                            .when_some(
                                run.description.clone().filter(|d| !d.is_empty()),
                                |d, description| {
                                    d.child(
                                        div()
                                            .truncate()
                                            .text_size(rpx(11.))
                                            .line_height(rpx(15.125))
                                            .text_color(colors.muted_foreground)
                                            .child(description),
                                    )
                                },
                            ),
                    )
            });
            div().w(rpx(288.)).my(rpx(-4.)).child(header).child(
                div()
                    .id(SharedString::from(format!("{}-runs", self.id)))
                    .max_h(rpx(320.))
                    .overflow_y_scroll()
                    .children(rows),
            )
        };

        #[cfg(test)]
        let panel = panel.debug_selector({
            let id = self.id.clone();
            move || format!("{id}-panel")
        });

        let dismiss = state.clone();
        Popover::new(SharedString::from(format!("{}-popover", self.id)))
            .trigger(trigger)
            .open(open)
            .content(panel)
            .on_dismiss(move |_, cx| {
                dismiss.update(cx, |state, cx| {
                    state.open = false;
                    cx.notify();
                });
            })
    }
}

#[cfg(test)]
mod tests {
    use reviewdeck_core::model::CheckRun;

    use super::*;

    fn summary(status: CheckStatus, passed: u32, failed: u32, running: u32) -> CheckSummary {
        CheckSummary {
            status,
            total: passed + failed + running,
            passed,
            failed,
            running,
            runs: Vec::<CheckRun>::new(),
        }
    }

    #[test]
    fn the_badge_counts_what_matters_for_the_status() {
        assert_eq!(
            pill_text(&summary(CheckStatus::Failed, 3, 2, 0)),
            "2 failed"
        );
        assert_eq!(
            pill_text(&summary(CheckStatus::Running, 3, 0, 1)),
            "1 running"
        );
        assert_eq!(pill_text(&summary(CheckStatus::Passed, 4, 0, 0)), "4/4");
        assert_eq!(pill_text(&summary(CheckStatus::Unknown, 1, 0, 0)), "1/1");
    }

    #[test]
    fn a_pill_with_no_runs_says_the_status() {
        assert_eq!(
            pill_text(&summary(CheckStatus::Unknown, 0, 0, 0)),
            "Unknown"
        );
        assert_eq!(pill_text(&summary(CheckStatus::Passed, 0, 0, 0)), "Passed");
        assert_eq!(pill_text(&summary(CheckStatus::Failed, 0, 0, 0)), "Failed");
        assert_eq!(
            pill_text(&summary(CheckStatus::Running, 0, 0, 0)),
            "Running"
        );
    }
}
