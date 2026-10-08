//! Port of src/renderer/src/components/ChecksPanel.tsx: the Checks tab, one row per run.

use gpui::{
    App, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
};
use reviewdeck_core::model::{CheckRun, CheckStatus, CheckSummary};

use crate::state::GlobalState;
use crate::ui::components::glass::GlassExt;
use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, radius, rpx};

/// `CHECK_TONE`: the colour a status is drawn in wherever a run is listed rather than
/// rolled up.
pub fn check_tone(status: CheckStatus, cx: &App) -> Hsla {
    let colors = cx.theme().colors;
    match status {
        CheckStatus::Passed => colors.ok,
        CheckStatus::Failed => colors.bad,
        CheckStatus::Running => colors.busy,
        CheckStatus::Unknown => colors.muted_foreground,
    }
}

/// `CheckIcon`: a tick for a pass, a cross for a failure, a spinner while running and a
/// dashed circle when the host would not say. Painted in the status's tone.
pub fn check_icon(status: CheckStatus, size: f32, cx: &App) -> Icon {
    let icon = match status {
        CheckStatus::Passed => Icon::new(IconName::CheckCircle2),
        CheckStatus::Failed => Icon::new(IconName::XCircle),
        CheckStatus::Running => Icon::new(IconName::Loader2).spin(),
        CheckStatus::Unknown => Icon::new(IconName::CircleDashed),
    };
    icon.size(size).color(check_tone(status, cx))
}

fn plural(count: u32, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// The headline over the list.
fn headline(checks: &CheckSummary) -> String {
    match checks.status {
        CheckStatus::Running => format!("{} still running", plural(checks.running, "check")),
        CheckStatus::Failed => format!("{} failing", plural(checks.failed, "check")),
        CheckStatus::Passed => "All checks passed".to_string(),
        CheckStatus::Unknown => "Check status unknown".to_string(),
    }
}

fn run_row(index: usize, run: &CheckRun, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let url = run.url.clone().filter(|url| !url.is_empty());
    div()
        .flex()
        .items_center()
        .gap(rpx(10.))
        .px(rpx(14.))
        .py(rpx(8.))
        // `divide-y divide-border`
        .when(index > 0, |row| {
            row.border_t_1().border_color(colors.border)
        })
        .child(check_icon(run.status, 14., cx))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .truncate()
                        .text_size(rpx(12.5))
                        .line_height(rpx(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(run.name.clone()),
                )
                .when_some(
                    run.description.clone().filter(|text| !text.is_empty()),
                    |column, description| {
                        column.child(
                            div()
                                .truncate()
                                .text_size(rpx(11.5))
                                .line_height(rpx(16.))
                                .text_color(colors.muted_foreground)
                                .child(description),
                        )
                    },
                ),
        )
        .when_some(url, |row, url| {
            row.child(
                div()
                    .id(ElementId::from(("check-details", index)))
                    .flex_none()
                    .cursor_pointer()
                    .text_size(rpx(11.5))
                    .text_color(colors.info)
                    .hover(|style| style.opacity(0.75))
                    .on_click(move |_, _, cx| {
                        let state = cx.global::<GlobalState>().0.clone();
                        state.read(cx).open_external(&url, cx);
                    })
                    .child("Details"),
            )
        })
}

/// The Checks tab. Links open through `AppState::open_external`, which only lets
/// http(s) reach the OS.
pub fn checks_panel(checks: &CheckSummary, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    if checks.total == 0 {
        return div()
            .px(rpx(16.))
            .py(rpx(24.))
            .text_center()
            .text_size(rpx(12.5))
            .text_color(colors.muted_foreground)
            .child("No status checks reported for this branch.");
    }

    div().p(rpx(16.)).child(
        div()
            .overflow_hidden()
            .rounded(rpx(radius::LG))
            .glass(cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(12.))
                    .px(rpx(14.))
                    .py(rpx(10.))
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .flex_none()
                            .mr(rpx(12.))
                            .child(check_icon(checks.status, 16., cx)),
                    )
                    .child(
                        div()
                            .text_size(rpx(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(headline(checks)),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_size(rpx(11.5))
                            .text_color(colors.muted_foreground)
                            .child(format!("{}/{} passed", checks.passed, checks.total)),
                    ),
            )
            .child(
                div().children(
                    checks
                        .runs
                        .iter()
                        .enumerate()
                        .map(|(index, run)| run_row(index, run, cx)),
                ),
            ),
    )
}
