//! Interaction tests for the settings dialog: every control saves, with its side effect.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use reviewdeck_core::http::MockResponse;
use reviewdeck_core::model::ReviewWindow;

use super::*;
use crate::ui::thread_view::test_support::{Env, boot, click, open_dialog, take_said};

struct Rig {
    env: Env,
    dialog: Entity<SettingsDialog>,
    dismissed: Rc<RefCell<usize>>,
    opened: Rc<RefCell<Vec<DialogKind>>>,
}

fn rig(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (dialog, vcx) = open_dialog(cx, SettingsDialog::new);
    let dismissed: Rc<RefCell<usize>> = Rc::default();
    let opened: Rc<RefCell<Vec<DialogKind>>> = Rc::default();
    let count = dismissed.clone();
    let log = opened.clone();
    let subscriptions = vcx.update(|_, cx| {
        (
            cx.subscribe(&dialog, move |_, _: &DismissEvent, _| {
                *count.borrow_mut() += 1
            }),
            cx.subscribe(&dialog, move |_, event: &DialogEvent, _| {
                let DialogEvent::Open(kind) = event;
                log.borrow_mut().push(*kind)
            }),
        )
    });
    std::mem::forget(subscriptions);
    (
        Rig {
            env,
            dialog,
            dismissed,
            opened,
        },
        vcx,
    )
}

fn settings(rig: &Rig) -> Settings {
    rig.env.vault.settings()
}

/// What choosing `value` in a select does: the select announces it.
fn choose(vcx: &mut VisualTestContext, select: &Entity<Select>, value: &'static str) {
    select.update(vcx, |_, cx| cx.emit(SelectEvent::Changed(value.into())));
    vcx.run_until_parked();
}

#[gpui::test]
fn each_switch_saves_when_clicked_and_back_when_clicked_again(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let defaults = settings(&rig);

    type Read = fn(&Settings) -> bool;
    let switches: [(&str, Read); 6] = [
        ("notifications", |s| s.notifications_enabled),
        ("hide-approved", |s| s.hide_approved),
        ("hide-fully-approved", |s| s.hide_fully_approved),
        ("hide-drafts", |s| s.hide_drafts),
        ("menu-bar-count", |s| s.show_menu_bar_count),
        // Sound is only reachable while notifications are on, which is the default.
        ("play-sound", |s| s.play_sound),
    ];
    for (id, read) in switches {
        let before = read(&settings(&rig));
        click(vcx, id);
        assert_eq!(read(&settings(&rig)), !before, "{id} turns over");
        click(vcx, id);
        assert_eq!(read(&settings(&rig)), before, "{id} turns back");
    }
    assert_eq!(settings(&rig), defaults, "nothing else moved");
}

#[gpui::test]
fn the_sound_and_the_schedule_need_notifications(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    click(vcx, "notifications");
    assert!(!settings(&rig).notifications_enabled);
    let sound = settings(&rig).play_sound;
    click(vcx, "play-sound");
    assert_eq!(settings(&rig).play_sound, sound, "the sound switch is dead");
    click(vcx, "open-schedule");
    assert!(rig.opened.borrow().is_empty(), "so is the schedule button");

    click(vcx, "notifications");
    click(vcx, "open-schedule");
    assert_eq!(*rig.opened.borrow(), vec![DialogKind::Schedule]);
}

#[gpui::test]
fn the_selects_save_what_they_choose(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let (poll, check, diff) = rig.dialog.read_with(vcx, |d, _| {
        (
            d.poll_interval.clone(),
            d.check_poll_interval.clone(),
            d.diff_view.clone(),
        )
    });
    choose(vcx, &poll, "900");
    choose(vcx, &check, "20");
    choose(vcx, &diff, "unified");
    let saved = settings(&rig);
    assert_eq!(saved.poll_interval, 900);
    assert_eq!(saved.check_poll_interval, 20);
    assert_eq!(saved.diff_view, DiffViewMode::Unified);

    choose(vcx, &diff, "split");
    assert_eq!(settings(&rig).diff_view, DiffViewMode::Split);
}

/// Choosing a theme also switches AppKit's appearance, which only the main thread may do,
/// so the theme select is tested down to the value it saves.
#[test]
fn the_theme_and_layout_values_map_to_their_settings() {
    assert_eq!(theme_from_value("system"), ThemeMode::System);
    assert_eq!(theme_from_value("light"), ThemeMode::Light);
    assert_eq!(theme_from_value("dark"), ThemeMode::Dark);
    assert_eq!(diff_view_from_value("split"), DiffViewMode::Split);
    assert_eq!(diff_view_from_value("unified"), DiffViewMode::Unified);
    for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
        assert_eq!(theme_from_value(theme_value(mode)), mode);
    }
    for mode in [DiffViewMode::Split, DiffViewMode::Unified] {
        assert_eq!(diff_view_from_value(diff_view_value(mode)), mode);
    }
}

#[gpui::test]
fn a_new_interval_reschedules_the_running_timer(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    rig.env.state.update(vcx, |state, cx| state.start(cx));
    let poll = rig.dialog.read_with(vcx, |d, _| d.poll_interval.clone());
    choose(vcx, &poll, "60");
    assert_eq!(
        rig.env.state.read_with(vcx, |s, _| s.sync_interval()),
        Some(std::time::Duration::from_secs(60))
    );
    rig.env.state.update(vcx, |state, _| state.stop());
}

#[gpui::test]
fn the_selects_start_on_what_is_stored(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    env.vault
        .save_settings(|s| {
            s.poll_interval = 1800;
            s.check_poll_interval = 180;
            s.theme = ThemeMode::Light;
            s.diff_view = DiffViewMode::Unified;
        })
        .expect("saved");
    let (dialog, vcx) = open_dialog(cx, SettingsDialog::new);
    let (poll, check, theme, diff) = dialog.read_with(vcx, |d, _| {
        (
            d.poll_interval.clone(),
            d.check_poll_interval.clone(),
            d.theme.clone(),
            d.diff_view.clone(),
        )
    });
    let value = |select: &Entity<Select>, vcx: &VisualTestContext| {
        select.read_with(vcx, |s, _| s.selected_value().map(|v| v.to_string()))
    };
    assert_eq!(value(&poll, vcx).as_deref(), Some("1800"));
    assert_eq!(value(&check, vcx).as_deref(), Some("180"));
    assert_eq!(value(&theme, vcx).as_deref(), Some("light"));
    assert_eq!(value(&diff, vcx).as_deref(), Some("unified"));
}

#[gpui::test]
fn the_agent_command_saves_as_it_is_typed(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let field = rig.dialog.read_with(vcx, |d, _| d.agent_command.clone());
    // It starts from the stored command, which is `claude` until changed.
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_string()), "claude");
    vcx.update(|window, cx| field.read(cx).focus(window));
    vcx.simulate_input("-w");
    assert_eq!(settings(&rig).agent_command, "claude-w");
    vcx.simulate_input("ork");
    assert_eq!(settings(&rig).agent_command, "claude-work");
    vcx.simulate_keystrokes("backspace backspace");
    assert_eq!(settings(&rig).agent_command, "claude-wo");
    // Clearing it saves the empty command, which the app reads as "use the default".
    vcx.simulate_keystrokes("cmd-a backspace");
    assert_eq!(settings(&rig).agent_command, "");
}

#[gpui::test]
fn the_schedule_button_counts_the_windows(cx: &mut TestAppContext) {
    assert_eq!(schedule_label(0), "Set up…");
    assert_eq!(schedule_label(1), "1 window…");
    assert_eq!(schedule_label(2), "2 windows…");
    let (rig, vcx) = rig(cx);
    let window = ReviewWindow {
        id: "w".into(),
        enabled: true,
        days: vec![1],
        start: "09:00".into(),
        end: "09:30".into(),
        minimum: 1,
        accounts: Vec::new(),
    };
    rig.env
        .vault
        .save_settings(|s| s.review_windows = vec![window.clone(), window])
        .expect("saved");
    vcx.run_until_parked();
    click(vcx, "open-schedule");
    assert_eq!(*rig.opened.borrow(), vec![DialogKind::Schedule]);
}

#[gpui::test]
fn done_and_escape_close_the_dialog(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    click(vcx, "settings-done");
    assert_eq!(*rig.dismissed.borrow(), 1);
    vcx.simulate_keystrokes("escape");
    assert_eq!(*rig.dismissed.borrow(), 2);

    // Escape in the command field is consumed by the field, which reports it.
    let field = rig.dialog.read_with(vcx, |d, _| d.agent_command.clone());
    vcx.update(|window, cx| field.read(cx).focus(window));
    vcx.simulate_keystrokes("escape");
    assert_eq!(*rig.dismissed.borrow(), 3);
}

#[gpui::test]
fn launch_at_login_that_the_system_refuses_goes_back_off_and_says_why(cx: &mut TestAppContext) {
    // A test binary is not an app bundle, so the system has nothing to register.
    let (rig, vcx) = rig(cx);
    click(vcx, "launch-at-login");
    assert!(!settings(&rig).launch_at_login, "the switch does not lie");
    let reason = rig
        .dialog
        .read_with(vcx, |d, _| d.login_error.clone())
        .expect("the reason is shown");
    assert!(reason.contains("app bundle"), "{reason}");
    assert!(take_said().is_empty());

    // Turning it off needs nothing from the system, and clears the complaint.
    click(vcx, "launch-at-login");
    assert!(!settings(&rig).launch_at_login);
}
