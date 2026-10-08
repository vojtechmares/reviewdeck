//! A debug-only scene hook: `REVIEWDECK_SCENE` puts the UI into a named state after
//! launch, so the app can be screenshotted in the same states as the Electron one
//! without synthesising input on the desktop.
//!
//! The module only exists in debug builds (`#[cfg(debug_assertions)]` on its
//! declaration and on every call site). The variable is a comma-separated list of
//! steps; see [`parse`]. Each step is applied once, by the view that owns its target,
//! at the moment the target exists: the view asks [`pending`], does what the matching
//! click handler does, and calls [`mark_done`].

use gpui::{App, Global};
use reviewdeck_core::model::{DiffViewMode, ReviewVerdict, Side};

/// The environment variable holding the steps.
pub const ENV: &str = "REVIEWDECK_SCENE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneTab {
    Files,
    Checks,
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneDialog {
    Accounts,
    AccountsAdd,
    Settings,
    Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    RevealDrafts,
    Select(usize),
    Filters,
    Pill,
    Tab(SceneTab),
    Layout(DiffViewMode),
    Comment(Side, u32),
    Scroll(u32),
    Verdict(ReviewVerdict),
    Dialog(SceneDialog),
    DraftEdit,
}

/// What a step is, without its argument: how a hook site asks for "its" step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    RevealDrafts,
    Select,
    Filters,
    Pill,
    Tab,
    Layout,
    Comment,
    Scroll,
    Verdict,
    Dialog,
    DraftEdit,
}

impl Step {
    pub fn kind(&self) -> Kind {
        match self {
            Step::RevealDrafts => Kind::RevealDrafts,
            Step::Select(_) => Kind::Select,
            Step::Filters => Kind::Filters,
            Step::Pill => Kind::Pill,
            Step::Tab(_) => Kind::Tab,
            Step::Layout(_) => Kind::Layout,
            Step::Comment(..) => Kind::Comment,
            Step::Scroll(_) => Kind::Scroll,
            Step::Verdict(_) => Kind::Verdict,
            Step::Dialog(_) => Kind::Dialog,
            Step::DraftEdit => Kind::DraftEdit,
        }
    }
}

fn parse_step(text: &str) -> Option<Step> {
    let (name, arg) = match text.split_once(':') {
        Some((name, arg)) => (name, Some(arg)),
        None => (text, None),
    };
    Some(match (name, arg) {
        ("reveal-drafts", None) => Step::RevealDrafts,
        ("select", Some(n)) => Step::Select(n.parse().ok()?),
        ("filters", None) => Step::Filters,
        ("pill", None) => Step::Pill,
        ("tab", Some("files")) => Step::Tab(SceneTab::Files),
        ("tab", Some("checks")) => Step::Tab(SceneTab::Checks),
        ("tab", Some("conversation")) => Step::Tab(SceneTab::Conversation),
        ("unified", None) => Step::Layout(DiffViewMode::Unified),
        ("split", None) => Step::Layout(DiffViewMode::Split),
        ("comment", Some(rest)) => {
            let (side, line) = rest.split_once(':')?;
            let side = match side {
                "old" => Side::Old,
                "new" => Side::New,
                _ => return None,
            };
            Step::Comment(side, line.parse().ok()?)
        }
        ("scroll", Some(n)) => Step::Scroll(n.parse().ok()?),
        ("verdict", Some("approve")) => Step::Verdict(ReviewVerdict::Approve),
        ("verdict", Some("request_changes")) => Step::Verdict(ReviewVerdict::RequestChanges),
        ("verdict", Some("comment")) => Step::Verdict(ReviewVerdict::Comment),
        ("dialog", Some("accounts")) => Step::Dialog(SceneDialog::Accounts),
        ("dialog", Some("accounts-add")) => Step::Dialog(SceneDialog::AccountsAdd),
        ("dialog", Some("settings")) => Step::Dialog(SceneDialog::Settings),
        ("dialog", Some("schedule")) => Step::Dialog(SceneDialog::Schedule),
        ("draft-edit", None) => Step::DraftEdit,
        _ => return None,
    })
}

/// Parses the variable's value into steps and the entries it did not understand.
pub fn parse(spec: &str) -> (Vec<Step>, Vec<String>) {
    let mut steps = Vec::new();
    let mut unknown = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match parse_step(part) {
            Some(step) => steps.push(step),
            None => unknown.push(part.to_string()),
        }
    }
    (steps, unknown)
}

/// The parsed steps and whether each has been applied.
struct Scene {
    steps: Vec<(Step, bool)>,
}

impl Global for Scene {}

/// Reads the variable and, when it is set, installs the scene. Unknown steps are
/// reported once, here.
pub fn init(cx: &mut App) {
    let Ok(spec) = std::env::var(ENV) else {
        return;
    };
    let (steps, unknown) = parse(&spec);
    for step in unknown {
        eprintln!("{ENV}: ignoring unknown step {step:?}");
    }
    cx.set_global(Scene {
        steps: steps.into_iter().map(|step| (step, false)).collect(),
    });
}

/// The first step of this kind that has not been applied yet.
pub fn pending(cx: &App, kind: Kind) -> Option<Step> {
    cx.try_global::<Scene>()?
        .steps
        .iter()
        .find(|(step, done)| !done && step.kind() == kind)
        .map(|(step, _)| *step)
}

/// Marks the first unapplied step of this kind as applied.
pub fn mark_done(cx: &mut App, kind: Kind) {
    if cx.has_global::<Scene>() {
        let scene = cx.global_mut::<Scene>();
        if let Some((_, done)) = scene
            .steps
            .iter_mut()
            .find(|(step, done)| !*done && step.kind() == kind)
        {
            *done = true;
        }
    }
}

/// Whether a step that changes which pull request is open, or which cards the deck
/// shows, is still waiting. The views below the deck hold back until it is applied,
/// or they would act on the pull request that is selected before it.
pub fn deck_pending(cx: &App) -> bool {
    pending(cx, Kind::RevealDrafts).is_some() || pending(cx, Kind::Select).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_parse_in_order_with_their_arguments() {
        let (steps, unknown) = parse(
            "reveal-drafts, select:2,filters,pill,tab:checks,split,comment:old:12,scroll:40,\
             verdict:request_changes,dialog:accounts-add,draft-edit",
        );
        assert!(unknown.is_empty());
        assert_eq!(
            steps,
            vec![
                Step::RevealDrafts,
                Step::Select(2),
                Step::Filters,
                Step::Pill,
                Step::Tab(SceneTab::Checks),
                Step::Layout(DiffViewMode::Split),
                Step::Comment(Side::Old, 12),
                Step::Scroll(40),
                Step::Verdict(ReviewVerdict::RequestChanges),
                Step::Dialog(SceneDialog::AccountsAdd),
                Step::DraftEdit,
            ]
        );
    }

    #[test]
    fn unknown_and_malformed_steps_are_reported_and_skipped() {
        let (steps, unknown) = parse("unified,,bogus,select:x,comment:left:3,tab:nope,select");
        assert_eq!(steps, vec![Step::Layout(DiffViewMode::Unified)]);
        assert_eq!(
            unknown,
            ["bogus", "select:x", "comment:left:3", "tab:nope", "select"]
        );
    }

    #[gpui::test]
    fn a_step_is_pending_until_it_is_marked_done(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Scene {
                steps: vec![(Step::Filters, false), (Step::Select(1), false)],
            });
            assert_eq!(pending(cx, Kind::Filters), Some(Step::Filters));
            mark_done(cx, Kind::Filters);
            assert_eq!(pending(cx, Kind::Filters), None);
            assert!(deck_pending(cx));
            mark_done(cx, Kind::Select);
            assert!(!deck_pending(cx));
        });
    }
}
