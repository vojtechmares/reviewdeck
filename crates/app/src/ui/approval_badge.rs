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
    let (tone, count, state) = describe(approvals);
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

/// The tone, the count and the sentence the tooltip carries for an approval summary.
fn describe(approvals: &ApprovalSummary) -> (BadgeTone, String, String) {
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
    (tone, count, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(given: u32, required: Option<u32>, outcome: ApprovalOutcome) -> ApprovalSummary {
        ApprovalSummary {
            given,
            required,
            outcome,
        }
    }

    #[test]
    fn a_host_that_asks_for_none_is_grey_and_shows_the_bare_count() {
        let (tone, count, state) = describe(&summary(1, None, ApprovalOutcome::NoneRequired));
        assert_eq!(tone, BadgeTone::Neutral);
        assert_eq!(count, "1");
        assert_eq!(
            state,
            "1 approval given. The host requires none, or will not say what it requires."
        );
        let (_, count, state) = describe(&summary(0, Some(0), ApprovalOutcome::NoneRequired));
        assert_eq!(count, "0", "a requirement of zero is no requirement");
        assert!(state.starts_with("0 approvals given."));
    }

    #[test]
    fn a_host_still_waiting_is_amber_over_what_it_asks_for() {
        let (tone, count, state) = describe(&summary(1, Some(2), ApprovalOutcome::Pending));
        assert_eq!(tone, BadgeTone::Busy);
        assert_eq!(count, "1/2");
        assert_eq!(
            state,
            "1 of 2 required approvals given. Still waiting on somebody."
        );
        let (_, count, state) = describe(&summary(2, None, ApprovalOutcome::Pending));
        assert_eq!(count, "2");
        assert_eq!(
            state,
            "2 approvals given. Still short of what the host requires."
        );
    }

    #[test]
    fn a_host_with_what_it_wants_is_green() {
        let (tone, count, state) = describe(&summary(2, Some(2), ApprovalOutcome::Satisfied));
        assert_eq!(tone, BadgeTone::Ok);
        assert_eq!(count, "2/2");
        assert_eq!(
            state,
            "2 approvals given. Every approval the host requires is there."
        );
    }
}
