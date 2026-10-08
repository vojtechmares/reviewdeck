//! Port of src/renderer/src/components/ApprovalBadge.tsx.

use gpui::{App, ElementId, div, prelude::*};
use reviewdeck_core::model::{ApprovalOutcome, ApprovalSummary};

use crate::ui::check_pill::tone_color;
use crate::ui::components::badge::{Badge, BadgeTone};
use crate::ui::components::tooltip::tooltip;
use crate::ui::icons::{Icon, IconName};

/// Where the pull request stands on approvals, whoever gave them.
///
/// Grey is a host that asks for none, or one that would not say - a branch with no
/// protection and a token that cannot read it look the same from here. Amber is a
/// host still waiting on somebody, green is one that has what it wants; the number
/// is what has been given, over what is asked for when the host puts a figure on it.
///
/// `id` must be unique among the elements around it (the tooltip needs one).
pub fn approval_badge(
    approvals: &ApprovalSummary,
    id: impl Into<ElementId>,
    cx: &App,
) -> impl IntoElement {
    let tone = match approvals.outcome {
        ApprovalOutcome::Satisfied => BadgeTone::Ok,
        ApprovalOutcome::Pending => BadgeTone::Busy,
        _ => BadgeTone::Neutral,
    };
    let count = match approvals.required {
        Some(required) if required > 0 => format!("{}/{}", approvals.given, required),
        _ => approvals.given.to_string(),
    };
    let state = match (approvals.outcome, approvals.required) {
        (ApprovalOutcome::NoneRequired, _) => format!(
            "{} given. The host requires none, or will not say what it requires.",
            plural(approvals.given, "approval")
        ),
        (ApprovalOutcome::Satisfied, _) => format!(
            "{} given. Every approval the host requires is there.",
            plural(approvals.given, "approval")
        ),
        (_, Some(required)) => format!(
            "{} of {} required approvals given. Still waiting on somebody.",
            approvals.given, required
        ),
        (_, None) => format!(
            "{} given. Still short of what the host requires.",
            plural(approvals.given, "approval")
        ),
    };
    div()
        .id(id)
        .flex_none()
        .tooltip(tooltip(format!("Approvals from any reviewer\n{state}")))
        .child(
            Badge::new()
                .tone(tone)
                .child(
                    Icon::new(IconName::UserCheck)
                        .size(12.)
                        .color(tone_color(tone, cx)),
                )
                .child(count),
        )
}

fn plural(count: u32, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}
