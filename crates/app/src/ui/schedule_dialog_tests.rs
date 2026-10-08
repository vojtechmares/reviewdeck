//! Interaction tests for the schedule dialog: adding, editing, validating and removing
//! review windows.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use reviewdeck_core::http::MockResponse;

use super::*;
use crate::ui::thread_view::test_support::{Env, boot, click, open_dialog, sign_in, take_said};

fn rig(
    cx: &mut TestAppContext,
) -> (
    Env,
    Entity<ScheduleDialog>,
    &mut VisualTestContext,
    Rc<RefCell<usize>>,
) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    // The dialog caches the accounts when it opens, so sign them in first where a test
    // needs them: this one has none.
    let (dialog, vcx) = open_dialog(cx, ScheduleDialog::new);
    let dismissed: Rc<RefCell<usize>> = Rc::default();
    let count = dismissed.clone();
    let subscription = vcx.update(|_, cx| {
        cx.subscribe(&dialog, move |_, _: &DismissEvent, _| {
            *count.borrow_mut() += 1
        })
    });
    std::mem::forget(subscription);
    (env, dialog, vcx, dismissed)
}

fn stored(env: &Env) -> Vec<ReviewWindow> {
    env.vault.settings().review_windows
}

fn problem(dialog: &Entity<ScheduleDialog>, vcx: &VisualTestContext) -> Option<&'static str> {
    dialog.read_with(vcx, |d, _| d.editing.as_ref().and_then(window_problem))
}

/// Replaces what a text field holds by typing.
fn retype(vcx: &mut VisualTestContext, field: &Entity<TextInput>, text: &str) {
    vcx.update(|window, cx| field.read(cx).focus(window));
    vcx.simulate_keystrokes("cmd-a");
    if text.is_empty() {
        vcx.simulate_keystrokes("backspace");
    } else {
        vcx.simulate_input(text);
    }
}

fn fields(
    dialog: &Entity<ScheduleDialog>,
    vcx: &VisualTestContext,
) -> (Entity<TextInput>, Entity<TextInput>, Entity<TextInput>) {
    dialog.read_with(vcx, |d, _| {
        (d.start.clone(), d.end.clone(), d.minimum.clone())
    })
}

#[gpui::test]
fn a_new_window_starts_as_weekday_mornings_and_saves(cx: &mut TestAppContext) {
    let (env, dialog, vcx, _dismissed) = rig(cx);
    assert!(stored(&env).is_empty());
    click(vcx, "schedule-add");

    let (start, end, minimum) = fields(&dialog, vcx);
    assert_eq!(start.read_with(vcx, |f, _| f.text().to_string()), "09:00");
    assert_eq!(end.read_with(vcx, |f, _| f.text().to_string()), "09:30");
    assert_eq!(minimum.read_with(vcx, |f, _| f.text().to_string()), "1");
    assert_eq!(problem(&dialog, vcx), None);

    click(vcx, "schedule-save");
    let windows = stored(&env);
    assert_eq!(windows.len(), 1);
    assert!(windows[0].enabled);
    assert_eq!(windows[0].days, vec![1, 2, 3, 4, 5]);
    assert_eq!(
        (windows[0].start.as_str(), windows[0].end.as_str()),
        ("09:00", "09:30")
    );
    assert_eq!(windows[0].minimum, 1);
    assert!(windows[0].accounts.is_empty(), "every account");
    assert!(dialog.read_with(vcx, |d, _| d.editing.is_none()));
}

#[gpui::test]
fn each_way_a_window_can_be_wrong_is_named_and_blocks_saving(cx: &mut TestAppContext) {
    let (env, dialog, vcx, _dismissed) = rig(cx);
    click(vcx, "schedule-add");
    let (start, end, minimum) = fields(&dialog, vcx);

    // No days.
    for day in [1, 2, 3, 4, 5] {
        click(vcx, &format!("day-{day}"));
    }
    assert_eq!(problem(&dialog, vcx), Some("Pick at least one day."));
    click(vcx, "schedule-save");
    assert!(
        stored(&env).is_empty(),
        "Save does nothing while there is a problem"
    );
    assert!(dialog.read_with(vcx, |d, _| d.editing.is_some()));
    click(vcx, "day-6");
    assert_eq!(problem(&dialog, vcx), None);

    // A time that is not a time.
    retype(vcx, &start, "");
    assert_eq!(problem(&dialog, vcx), Some("Both times need to be set."));
    retype(vcx, &start, "25:00");
    assert_eq!(problem(&dialog, vcx), Some("Both times need to be set."));
    retype(vcx, &start, "10:00");
    assert_eq!(
        problem(&dialog, vcx),
        Some("The end time has to be after the start time.")
    );
    retype(vcx, &end, "10:00");
    assert_eq!(
        problem(&dialog, vcx),
        Some("The end time has to be after the start time."),
        "an empty window is no window"
    );
    retype(vcx, &end, "11:00");
    assert_eq!(problem(&dialog, vcx), None);

    // A minimum that is not a count.
    retype(vcx, &minimum, "0");
    assert_eq!(
        problem(&dialog, vcx),
        Some("The minimum has to be at least one review.")
    );
    retype(vcx, &minimum, "");
    assert_eq!(
        problem(&dialog, vcx),
        Some("The minimum has to be at least one review.")
    );
    retype(vcx, &minimum, "-3");
    assert_eq!(
        problem(&dialog, vcx),
        Some("The minimum has to be at least one review.")
    );
    retype(vcx, &minimum, "2.9");
    assert_eq!(problem(&dialog, vcx), None);
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.editing.as_ref().map(|w| w.minimum)),
        Some(2)
    );

    click(vcx, "schedule-save");
    let windows = stored(&env);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].days, vec![6]);
    assert_eq!(
        (windows[0].start.as_str(), windows[0].end.as_str()),
        ("10:00", "11:00")
    );
    assert_eq!(windows[0].minimum, 2);
}

#[gpui::test]
fn times_are_stored_the_way_a_time_field_writes_them(cx: &mut TestAppContext) {
    let (env, dialog, vcx, _dismissed) = rig(cx);
    click(vcx, "schedule-add");
    let (start, end, _) = fields(&dialog, vcx);
    retype(vcx, &start, "8:05");
    retype(vcx, &end, " 17:45 ");
    click(vcx, "schedule-save");
    let windows = stored(&env);
    assert_eq!(windows.len(), 1, "{:?}", problem(&dialog, vcx));
    assert_eq!(
        (windows[0].start.as_str(), windows[0].end.as_str()),
        ("08:05", "17:45")
    );
}

#[gpui::test]
fn editing_a_window_replaces_it_in_place(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let make = |id: &str, start: &str, end: &str| ReviewWindow {
        id: id.into(),
        enabled: true,
        days: vec![1, 2],
        start: start.into(),
        end: end.into(),
        minimum: 1,
        accounts: Vec::new(),
    };
    env.vault
        .save_settings(|s| {
            s.review_windows = vec![make("a", "07:00", "07:30"), make("b", "12:00", "12:30")]
        })
        .expect("saved");
    let (dialog, vcx) = open_dialog(cx, ScheduleDialog::new);

    click(vcx, "edit-window-1");
    let (start, end, minimum) = fields(&dialog, vcx);
    assert_eq!(start.read_with(vcx, |f, _| f.text().to_string()), "12:00");
    retype(vcx, &end, "14:00");
    retype(vcx, &start, "13:00");
    retype(vcx, &minimum, "4");
    click(vcx, "window-enabled");
    click(vcx, "schedule-save");

    let windows = stored(&env);
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].id, "a");
    assert_eq!(windows[0].start, "07:00", "the other window is untouched");
    assert_eq!(windows[1].id, "b");
    assert_eq!(windows[1].start, "13:00");
    assert_eq!(windows[1].minimum, 4);
    assert!(!windows[1].enabled, "the box was ticked off");
}

#[gpui::test]
fn cancel_leaves_the_windows_as_they_were(cx: &mut TestAppContext) {
    let (env, dialog, vcx, _dismissed) = rig(cx);
    click(vcx, "schedule-add");
    click(vcx, "day-6");
    click(vcx, "schedule-cancel");
    assert!(stored(&env).is_empty());
    assert!(dialog.read_with(vcx, |d, _| d.editing.is_none()));
    // A new window is a fresh one, not the abandoned one.
    click(vcx, "schedule-add");
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.editing.as_ref().map(|w| w.days.clone())),
        Some(vec![1, 2, 3, 4, 5])
    );
}

#[gpui::test]
fn removing_a_window_deletes_it(cx: &mut TestAppContext) {
    let (env, dialog, vcx, _dismissed) = rig(cx);
    for _ in 0..2 {
        click(vcx, "schedule-add");
        click(vcx, "schedule-save");
    }
    let windows = stored(&env);
    assert_eq!(windows.len(), 2);
    assert_ne!(windows[0].id, windows[1].id);

    click(vcx, "remove-window-0");
    let left = stored(&env);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, windows[1].id);
    click(vcx, "remove-window-0");
    assert!(stored(&env).is_empty());
    assert!(dialog.read_with(vcx, |d, _| d.editing.is_none()));
    assert!(take_said().is_empty());
}

#[gpui::test]
fn accounts_are_picked_one_by_one_or_all_at_once(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let work = sign_in(&env, "Work");
    let home = sign_in(&env, "Home");
    let (dialog, vcx) = open_dialog(cx, ScheduleDialog::new);
    let picked = |vcx: &VisualTestContext| {
        dialog.read_with(vcx, |d, _| d.editing.as_ref().map(|w| w.accounts.clone()))
    };

    click(vcx, "schedule-add");
    assert_eq!(picked(vcx), Some(vec![]));
    click(vcx, "account-Work");
    assert_eq!(picked(vcx), Some(vec![work.id.clone()]));
    click(vcx, "account-Home");
    assert_eq!(picked(vcx), Some(vec![work.id.clone(), home.id.clone()]));
    click(vcx, "account-Work");
    assert_eq!(picked(vcx), Some(vec![home.id.clone()]));
    // "All accounts" clears the selection, because empty is what it means.
    click(vcx, "all-accounts");
    assert_eq!(picked(vcx), Some(vec![]));
    click(vcx, "account-Home");
    click(vcx, "schedule-save");
    assert_eq!(stored(&env)[0].accounts, vec![home.id.clone()]);
}

#[gpui::test]
fn escape_closes_the_dialog_from_the_list_and_from_a_field(cx: &mut TestAppContext) {
    let (_env, dialog, vcx, dismissed) = rig(cx);
    vcx.simulate_keystrokes("escape");
    assert_eq!(*dismissed.borrow(), 1);
    click(vcx, "schedule-add");
    let (start, _, _) = fields(&dialog, vcx);
    vcx.update(|window, cx| start.read(cx).focus(window));
    vcx.simulate_keystrokes("escape");
    assert_eq!(*dismissed.borrow(), 2);
}

#[gpui::test]
fn a_window_naming_only_signed_out_accounts_is_flagged(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let work = sign_in(&env, "Work");
    let window = ReviewWindow {
        id: "w".into(),
        enabled: false,
        days: vec![1],
        start: "09:00".into(),
        end: "09:30".into(),
        minimum: 1,
        accounts: vec!["gone".into()],
    };
    assert!(covers_nothing(&window, &[work.id.as_str()]));
    assert!(!covers_nothing(
        &ReviewWindow {
            accounts: vec![work.id.clone()],
            ..window
        },
        &[work.id.as_str()]
    ));
}

#[test]
fn times_are_normalised_only_when_they_are_times() {
    assert_eq!(normalise_time("9:30"), "09:30");
    assert_eq!(normalise_time(" 09:30 "), "09:30");
    assert_eq!(normalise_time("23:59"), "23:59");
    assert_eq!(normalise_time("24:00"), "24:00");
    assert_eq!(normalise_time("soon"), "soon");
}
