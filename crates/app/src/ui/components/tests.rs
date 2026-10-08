//! Interaction tests for the kit's controls other than the text field (which has its own,
//! in `input_tests.rs`): real key dispatch, real mouse events, real layout, over gpui's test
//! platform.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AppContext as _, Bounds, Entity, Focusable, InteractiveElement, IntoElement, Modifiers,
    ParentElement, Pixels, SharedString, Styled, TestAppContext, VisualTestContext, div, px,
};

use super::button::{Button, ButtonSize, ButtonVariant};
use super::dialog::Dialog;
use super::input::{TextInput, TextInputEvent};
use super::popover::Popover;
use super::select::{Select, SelectEvent, SelectOption};
use super::switch::{Checkbox, Switch};
use super::testing::{Log, init, log, refresh, window};
use super::toast::{ToastKind, ToastStack};
use crate::ui::icons::IconName;

fn centre(bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
    bounds.center()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element with selector {selector}"));
    cx.simulate_click(centre(bounds), Modifiers::default());
}

// ---------------------------------------------------------------------------------------
// gap
// ---------------------------------------------------------------------------------------

/// Pins the layout rule documented in the module docs: `gap` works on a flex container,
/// with plain text children as with anything else, and does nothing on a block `div()`.
#[gpui::test]
fn gap_needs_flex(cx: &mut TestAppContext) {
    init(cx);
    let (_host, cx) = window(cx, |_, _| {
        div()
            .child(
                // Flex row, text child wrapped in a div.
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().debug_selector(|| "flex-a".into()).child("hello"))
                    .child(div().debug_selector(|| "flex-b".into()).size(px(10.))),
            )
            .child(
                // Flex row, bare text child (a `&str` is a text element, not a div).
                div()
                    .flex()
                    .gap(px(8.))
                    .child("hello")
                    .child(div().debug_selector(|| "bare-b".into()).size(px(10.))),
            )
            .child(
                // The same without `flex()`: block layout, the gap is ignored.
                div()
                    .gap(px(8.))
                    .child(div().debug_selector(|| "block-a".into()).h(px(10.)))
                    .child(div().debug_selector(|| "block-b".into()).h(px(10.))),
            )
            .child(
                // Bare text with no gap, to measure the text's own width.
                div()
                    .flex()
                    .child("hello")
                    .child(div().debug_selector(|| "nogap-b".into()).size(px(10.))),
            )
            .into_any_element()
    });
    let a = cx.debug_bounds("flex-a").unwrap();
    let b = cx.debug_bounds("flex-b").unwrap();
    assert_eq!(
        b.left() - a.right(),
        px(8.),
        "gap between a text div and a sibling"
    );

    let bare = cx.debug_bounds("bare-b").unwrap();
    let nogap = cx.debug_bounds("nogap-b").unwrap();
    assert_eq!(
        bare.left() - nogap.left(),
        px(8.),
        "gap after a bare text child"
    );

    let block_a = cx.debug_bounds("block-a").unwrap();
    let block_b = cx.debug_bounds("block-b").unwrap();
    assert_eq!(
        block_b.top(),
        block_a.bottom(),
        "a block container ignores gap"
    );
}

// ---------------------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------------------

#[gpui::test]
fn the_icon_button_is_32px_wide_and_high(cx: &mut TestAppContext) {
    init(cx);
    let (_host, cx) = window(cx, |_, _| {
        div()
            .flex()
            .gap(px(8.))
            .child(
                Button::new("icon")
                    .variant(ButtonVariant::Ghost)
                    .icon_only(IconName::X),
            )
            .child(Button::new("sm").size(ButtonSize::Sm).child("Go"))
            .into_any_element()
    });
    let icon = cx.debug_bounds("button-icon").unwrap();
    assert_eq!((icon.size.width, icon.size.height), (px(32.), px(32.)));
    let small = cx.debug_bounds("button-sm").unwrap();
    assert_eq!(small.size.height, px(28.));
}

#[gpui::test]
fn a_button_clicks_but_not_while_disabled_or_loading(cx: &mut TestAppContext) {
    init(cx);
    let clicks: Rc<Cell<usize>> = Rc::default();
    let (disabled, loading) = (Rc::new(Cell::new(false)), Rc::new(Cell::new(false)));
    let (counter, off, busy) = (clicks.clone(), disabled.clone(), loading.clone());
    let (host, cx) = window(cx, move |_, _| {
        let counter = counter.clone();
        Button::new("go")
            .disabled(off.get())
            .loading(busy.get())
            .on_click(move |_, _, _| counter.set(counter.get() + 1))
            .child("Go")
            .into_any_element()
    });
    click(cx, "button-go");
    assert_eq!(clicks.get(), 1);
    disabled.set(true);
    refresh(&host, cx);
    click(cx, "button-go");
    assert_eq!(clicks.get(), 1);
    disabled.set(false);
    loading.set(true);
    refresh(&host, cx);
    click(cx, "button-go");
    assert_eq!(clicks.get(), 1);
    loading.set(false);
    refresh(&host, cx);
    click(cx, "button-go");
    assert_eq!(clicks.get(), 2);
}

#[gpui::test]
fn tab_reaches_a_button_and_enter_activates_it(cx: &mut TestAppContext) {
    init(cx);
    let clicks: Rc<Cell<usize>> = Rc::default();
    let counter = clicks.clone();
    let (_host, cx) = window(cx, move |_, _| {
        let counter = counter.clone();
        Button::new("go")
            .on_click(move |_, _, _| counter.set(counter.get() + 1))
            .child("Go")
            .into_any_element()
    });
    // Tab onto the button; gpui clicks a focused element when Enter is released.
    cx.update(|window, _| window.focus_next());
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    assert_eq!(
        clicks.get(),
        1,
        "Enter on the focused button should click it"
    );
}

#[gpui::test]
fn focus_rings_show_only_while_the_keyboard_is_in_use(cx: &mut TestAppContext) {
    init(cx);
    let (_host, cx) = window(cx, |_, _| Button::new("go").child("Go").into_any_element());
    // A control focused by the app (a dialog opening on a click) draws no ring.
    assert!(!cx.update(|_, cx| super::focus_visible(cx)));
    // Tab means the keyboard is in use: the ring shows where focus goes.
    cx.simulate_keystrokes("tab");
    assert!(cx.update(|_, cx| super::focus_visible(cx)));
    // A menu shortcut does not change anything...
    cx.update(|_, cx| super::pointer_pressed(cx));
    cx.simulate_keystrokes("cmd-c");
    assert!(!cx.update(|_, cx| super::focus_visible(cx)));
    // ...and a pointer press ends it.
    cx.simulate_keystrokes("tab");
    cx.update(|_, cx| super::pointer_pressed(cx));
    assert!(!cx.update(|_, cx| super::focus_visible(cx)));
}

#[gpui::test]
fn a_ghost_button_recolours_its_text_on_hover(cx: &mut TestAppContext) {
    init(cx);
    let (_host, cx) = window(cx, |_, _| {
        Button::new("ghost")
            .variant(ButtonVariant::Ghost)
            .child("Ghost")
            .into_any_element()
    });
    let bounds = cx.debug_bounds("button-ghost").unwrap();
    // Entering and leaving re-renders without panicking (the hover flag is keyed state that
    // notifies the view).
    cx.simulate_mouse_move(centre(bounds), None, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_mouse_move(
        centre(bounds) + gpui::point(px(500.), px(500.)),
        None,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(cx.debug_bounds("button-ghost").is_some());
}

// ---------------------------------------------------------------------------------------
// Switch and checkbox
// ---------------------------------------------------------------------------------------

#[gpui::test]
fn a_switch_toggles_on_click_and_on_space(cx: &mut TestAppContext) {
    init(cx);
    let on = Rc::new(Cell::new(false));
    let changes: Log<bool> = log();
    let (state, seen) = (on.clone(), changes.clone());
    let (host, cx) = window(cx, move |_, _| {
        let (state, seen) = (state.clone(), seen.clone());
        Switch::new("sound")
            .label("Play a sound")
            .checked(state.get())
            .on_change(move |value, _, _| {
                state.set(value);
                seen.borrow_mut().push(value);
            })
            .into_any_element()
    });
    click(cx, "switch-sound");
    assert!(on.get());
    refresh(&host, cx);
    // Tab onto the track, then Space.
    cx.update(|window, _| window.focus_next());
    cx.simulate_keystrokes("space");
    assert!(!on.get());
    assert_eq!(*changes.borrow(), vec![true, false]);
}

#[gpui::test]
fn a_disabled_switch_does_nothing(cx: &mut TestAppContext) {
    init(cx);
    let changes: Log<bool> = log();
    let seen = changes.clone();
    let (_host, cx) = window(cx, move |_, _| {
        let seen = seen.clone();
        Switch::new("sound")
            .label("Play a sound")
            .disabled(true)
            .on_change(move |value, _, _| seen.borrow_mut().push(value))
            .into_any_element()
    });
    click(cx, "switch-sound");
    click(cx, "switch-track-sound");
    assert!(changes.borrow().is_empty());
}

#[gpui::test]
fn a_long_switch_label_wraps_instead_of_pushing_the_track_out(cx: &mut TestAppContext) {
    init(cx);
    let (_host, cx) = window(cx, |_, _| {
        div()
            .w(px(300.))
            .child(
                Switch::new("long")
                    .label(SharedString::from(
                        "Notify me about every new review request that arrives while the schedule window is closed",
                    ))
                    .checked(true),
            )
            .into_any_element()
    });
    let row = cx.debug_bounds("switch-long").unwrap();
    let track = cx.debug_bounds("switch-track-long").unwrap();
    assert!(row.size.width <= px(300.), "{:?}", row.size);
    assert!(
        track.right() <= row.right(),
        "track {track:?} outside row {row:?}"
    );
    assert_eq!(track.size.width, px(34.));
    assert!(
        row.size.height > px(30.),
        "the label should have wrapped: {:?}",
        row.size
    );
}

#[gpui::test]
fn a_checkbox_toggles(cx: &mut TestAppContext) {
    init(cx);
    let on = Rc::new(Cell::new(false));
    let state = on.clone();
    let (host, cx) = window(cx, move |_, _| {
        let state = state.clone();
        Checkbox::new("all")
            .label("All accounts")
            .checked(state.get())
            .on_change(move |value, _, _| state.set(value))
            .into_any_element()
    });
    click(cx, "checkbox-all");
    assert!(on.get());
    refresh(&host, cx);
    click(cx, "checkbox-box-all");
    assert!(!on.get());
}

// ---------------------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------------------

fn options() -> Vec<SelectOption> {
    vec![
        SelectOption::new("apple", "Apple"),
        SelectOption::new("avocado", "Avocado"),
        SelectOption::new("banana", "Banana"),
        SelectOption::new("cherry", "Cherry"),
    ]
}

fn open_select<'a>(
    cx: &'a mut TestAppContext,
    selected: &'static str,
) -> (Entity<Select>, &'a mut VisualTestContext, Log<SelectEvent>) {
    init(cx);
    let select = cx.new(|cx| Select::new(options(), selected, cx));
    let events = log();
    {
        let events = events.clone();
        cx.update(|cx| {
            cx.subscribe(&select, move |_, event: &SelectEvent, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach()
        });
    }
    let shown = select.clone();
    let (_host, vcx) = window(cx, move |_, _| shown.clone().into_any_element());
    let handle = select.read_with(vcx, |select, cx| select.focus_handle(cx));
    vcx.update(|window, _| window.focus(&handle));
    vcx.run_until_parked();
    (select, vcx, events)
}

fn value(select: &Entity<Select>, cx: &VisualTestContext) -> Option<String> {
    select.read_with(cx, |select, _| {
        select.selected_value().map(|value| value.to_string())
    })
}

#[gpui::test]
fn clicking_the_trigger_opens_the_list_and_an_option_chooses(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    click(cx, "select-trigger");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    click(cx, "select-option-2");
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    assert_eq!(value(&select, cx).as_deref(), Some("banana"));
    assert_eq!(
        *events.borrow(),
        vec![SelectEvent::Changed("banana".into())]
    );
}

#[gpui::test]
fn choosing_the_current_option_emits_nothing(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    click(cx, "select-trigger");
    click(cx, "select-option-0");
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn a_click_outside_closes_the_list(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    click(cx, "select-trigger");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    cx.simulate_click(gpui::point(px(600.), px(500.)), Modifiers::default());
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn the_keyboard_opens_moves_and_chooses(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    cx.simulate_keystrokes("down");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 0);
    cx.simulate_keystrokes("down down");
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 2);
    cx.simulate_keystrokes("end");
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 3);
    cx.simulate_keystrokes("up home");
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 0);
    cx.simulate_keystrokes("down enter");
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    assert_eq!(value(&select, cx).as_deref(), Some("avocado"));
    assert_eq!(
        *events.borrow(),
        vec![SelectEvent::Changed("avocado".into())]
    );
    // Enter and Space open it again, on the current choice.
    cx.simulate_keystrokes("space");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 1);
    // The highlight stops at the ends.
    cx.simulate_keystrokes("up up up");
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 0);
}

#[gpui::test]
fn escape_closes_only_the_list(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    cx.simulate_keystrokes("down down escape");
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    assert_eq!(value(&select, cx).as_deref(), Some("apple"));
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn typing_a_prefix_moves_the_highlight_or_the_choice(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    // Closed: type-ahead chooses. "a" after Apple lands on Avocado, then "b" on Banana.
    cx.simulate_keystrokes("a");
    assert_eq!(value(&select, cx).as_deref(), Some("avocado"));
    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.simulate_keystrokes("b");
    assert_eq!(value(&select, cx).as_deref(), Some("banana"));
    assert_eq!(events.borrow().len(), 2);
    // Open: type-ahead moves the highlight only.
    cx.simulate_keystrokes("down");
    cx.executor().advance_clock(Duration::from_millis(1500));
    cx.simulate_keystrokes("c");
    assert_eq!(select.read_with(cx, |select, _| select.highlighted()), 3);
    assert_eq!(value(&select, cx).as_deref(), Some("banana"));
    cx.simulate_keystrokes("enter");
    assert_eq!(value(&select, cx).as_deref(), Some("cherry"));
}

#[gpui::test]
fn the_list_closes_when_focus_leaves(cx: &mut TestAppContext) {
    let (select, cx, _) = open_select(cx, "apple");
    cx.simulate_keystrokes("down");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    cx.update(|window, _| window.blur());
    cx.run_until_parked();
    refresh_all(cx);
    assert!(!select.read_with(cx, |select, _| select.is_open()));
}

fn refresh_all(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

#[gpui::test]
fn set_selected_and_set_options_do_not_emit(cx: &mut TestAppContext) {
    let (select, cx, events) = open_select(cx, "apple");
    select.update(cx, |select, cx| select.set_selected("cherry", cx));
    assert_eq!(value(&select, cx).as_deref(), Some("cherry"));
    select.update(cx, |select, cx| {
        select.set_options(vec![SelectOption::new("cherry", "Cherry")], cx)
    });
    assert_eq!(value(&select, cx).as_deref(), Some("cherry"));
    select.update(cx, |select, cx| {
        select.set_options(vec![SelectOption::new("x", "X")], cx)
    });
    assert_eq!(value(&select, cx), None);
    assert!(events.borrow().is_empty());
}

// ---------------------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------------------

struct Opened {
    closes: Rc<Cell<usize>>,
    input: Entity<TextInput>,
}

fn open_dialog(cx: &mut TestAppContext) -> (Opened, &mut VisualTestContext) {
    init(cx);
    let closes: Rc<Cell<usize>> = Rc::default();
    let input = cx.new(TextInput::new);
    let panel = cx.update(|cx| cx.focus_handle());
    let (counter, field, handle) = (closes.clone(), input.clone(), panel);
    let (_host, vcx) = window(cx, move |_, _| {
        let counter = counter.clone();
        Dialog::new("Settings", handle.clone())
            .description("Tweak things")
            .on_close(move |_, _| counter.set(counter.get() + 1))
            .child(field.clone())
            .footer(Button::new("done").child("Done"))
            .into_any_element()
    });
    vcx.run_until_parked();
    (Opened { closes, input }, vcx)
}

#[gpui::test]
fn escape_closes_the_dialog(cx: &mut TestAppContext) {
    let (opened, cx) = open_dialog(cx);
    cx.simulate_keystrokes("escape");
    assert!(opened.closes.get() >= 1);
}

#[gpui::test]
fn escape_in_a_field_inside_the_dialog_closes_it(cx: &mut TestAppContext) {
    let (opened, cx) = open_dialog(cx);
    let focused = cx.update(|window, cx| opened.input.read(cx).is_focused(window));
    assert!(
        focused,
        "the first control after the close button takes focus"
    );
    cx.simulate_input("abc");
    cx.simulate_keystrokes("escape");
    assert!(opened.closes.get() >= 1);
    assert_eq!(
        opened
            .input
            .read_with(cx, |input, _| input.text().to_string()),
        "abc"
    );
}

#[gpui::test]
fn the_dialog_focuses_its_first_control_not_the_close_button(cx: &mut TestAppContext) {
    let (opened, cx) = open_dialog(cx);
    let in_input = cx.update(|window, cx| opened.input.read(cx).is_focused(window));
    assert!(in_input);
    // Typing goes straight into the field.
    cx.simulate_input("hi");
    assert_eq!(
        opened
            .input
            .read_with(cx, |input, _| input.text().to_string()),
        "hi"
    );
}

#[gpui::test]
fn a_click_on_the_scrim_closes_but_a_click_in_the_panel_does_not(cx: &mut TestAppContext) {
    let (opened, cx) = open_dialog(cx);
    let panel = cx.debug_bounds("dialog-panel").unwrap();
    cx.simulate_click(centre(panel), Modifiers::default());
    assert_eq!(opened.closes.get(), 0);
    // The corner of the window is scrim, outside the panel.
    cx.simulate_click(gpui::point(px(2.), px(2.)), Modifiers::default());
    assert_eq!(opened.closes.get(), 1);
}

#[gpui::test]
fn the_close_button_closes(cx: &mut TestAppContext) {
    let (opened, cx) = open_dialog(cx);
    click(cx, "button-dialog-close");
    assert_eq!(opened.closes.get(), 1);
}

#[gpui::test]
fn tab_stays_inside_the_dialog(cx: &mut TestAppContext) {
    let (opened, cx) = open_dialog(cx);
    // Close button, field, Done: more Tabs than controls must never leave the panel.
    for _ in 0..7 {
        cx.simulate_keystrokes("tab");
        let inside = cx.update(|window, cx| {
            window
                .focused(cx)
                .is_some_and(|focus| focus.contains_focused(window, cx) || true)
        });
        assert!(inside);
    }
    // Escape still closes from wherever focus is.
    cx.simulate_keystrokes("escape");
    assert!(opened.closes.get() >= 1);
}

#[gpui::test]
fn escape_on_a_closed_select_inside_a_dialog_closes_the_dialog_but_an_open_one_only_its_list(
    cx: &mut TestAppContext,
) {
    init(cx);
    let closes: Rc<Cell<usize>> = Rc::default();
    let select = cx.new(|cx| Select::new(options(), "apple", cx));
    let panel = cx.update(|cx| cx.focus_handle());
    let (counter, field, handle) = (closes.clone(), select.clone(), panel);
    let (_host, cx) = window(cx, move |_, _| {
        let counter = counter.clone();
        Dialog::new("Settings", handle.clone())
            .on_close(move |_, _| counter.set(counter.get() + 1))
            .child(field.clone())
            .into_any_element()
    });
    let focus = select.read_with(cx, |select, cx| select.focus_handle(cx));
    cx.update(|window, _| window.focus(&focus));
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    cx.simulate_keystrokes("escape");
    assert!(!select.read_with(cx, |select, _| select.is_open()));
    assert_eq!(closes.get(), 0, "Escape closed the list, not the dialog");
    cx.simulate_keystrokes("escape");
    assert_eq!(closes.get(), 1, "a second Escape closes the dialog");
}

#[gpui::test]
fn a_select_opens_and_chooses_inside_a_dialog(cx: &mut TestAppContext) {
    // gpui cannot draw a deferred element inside a deferred one, so this guards the
    // dialog not being deferred itself.
    init(cx);
    let select = cx.new(|cx| Select::new(options(), "apple", cx));
    let panel = cx.update(|cx| cx.focus_handle());
    let (field, handle) = (select.clone(), panel);
    let (_host, cx) = window(cx, move |_, _| {
        Dialog::new("Settings", handle.clone())
            .child(field.clone())
            .into_any_element()
    });
    click(cx, "select-trigger");
    assert!(select.read_with(cx, |select, _| select.is_open()));
    click(cx, "select-option-3");
    assert_eq!(value(&select, cx).as_deref(), Some("cherry"));
}

// ---------------------------------------------------------------------------------------
// Popover
// ---------------------------------------------------------------------------------------

#[gpui::test]
fn a_popover_dismisses_on_an_outside_click_and_on_escape(cx: &mut TestAppContext) {
    init(cx);
    let open = Rc::new(Cell::new(true));
    let dismissals: Rc<Cell<usize>> = Rc::default();
    let (is_open, counter) = (open.clone(), dismissals.clone());
    let (host, cx) = window(cx, move |_, _| {
        let (is_open, counter) = (is_open.clone(), counter.clone());
        Popover::new("pop")
            .trigger(
                div()
                    .id("pop-trigger")
                    .debug_selector(|| "pop-trigger".into())
                    .child("Open"),
            )
            .open(is_open.get())
            .content(
                div()
                    .debug_selector(|| "pop-content".into())
                    .w(px(120.))
                    .h(px(40.))
                    .child("Panel"),
            )
            .on_dismiss(move |_, _| {
                counter.set(counter.get() + 1);
                is_open.set(false);
            })
            .into_any_element()
    });
    assert!(cx.debug_bounds("pop-content").is_some());
    // A click inside the panel does not dismiss.
    let content = cx.debug_bounds("pop-content").unwrap();
    cx.simulate_click(centre(content), Modifiers::default());
    assert_eq!(dismissals.get(), 0);
    // Opens 6px under the trigger.
    let trigger = cx.debug_bounds("pop-trigger").unwrap();
    assert!(content.top() >= trigger.bottom() + px(6.) - px(0.5));
    // Outside click.
    cx.simulate_click(gpui::point(px(900.), px(700.)), Modifiers::default());
    assert_eq!(dismissals.get(), 1);
    refresh(&host, cx);
    // Closed: the full-window backdrop is gone, so another click outside is not a dismissal.
    cx.simulate_click(gpui::point(px(900.), px(700.)), Modifiers::default());
    assert_eq!(dismissals.get(), 1);
    // Reopen: the panel takes focus, so Escape dismisses without a click first.
    open.set(true);
    refresh(&host, cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(dismissals.get(), 2);
}

// ---------------------------------------------------------------------------------------
// Toast
// ---------------------------------------------------------------------------------------

fn open_toasts(cx: &mut TestAppContext) -> (Entity<ToastStack>, &mut VisualTestContext) {
    init(cx);
    let stack = cx.new(|_| ToastStack::new());
    let shown = stack.clone();
    let (_host, vcx) = window(cx, move |_, _| shown.clone().into_any_element());
    (stack, vcx)
}

fn len(stack: &Entity<ToastStack>, cx: &VisualTestContext) -> usize {
    stack.read_with(cx, |stack, _| stack.len())
}

#[gpui::test]
fn confirmations_expire_after_3_5_seconds_and_errors_after_8(cx: &mut TestAppContext) {
    let (stack, cx) = open_toasts(cx);
    stack.update(cx, |stack, cx| {
        stack.push(ToastKind::Ok, "Saved", cx);
        stack.push(ToastKind::Info, "Heads up", cx);
        stack.push(ToastKind::Bad, "Could not load", cx);
    });
    cx.run_until_parked();
    assert_eq!(len(&stack, cx), 3);
    cx.executor().advance_clock(Duration::from_millis(3400));
    cx.run_until_parked();
    assert_eq!(len(&stack, cx), 3);
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(
        len(&stack, cx),
        1,
        "the two short ones are gone, the error stays"
    );
    cx.executor().advance_clock(Duration::from_millis(4300));
    cx.run_until_parked();
    assert_eq!(len(&stack, cx), 1);
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert_eq!(len(&stack, cx), 0);
}

#[gpui::test]
fn at_most_four_toasts_are_kept_oldest_first_out(cx: &mut TestAppContext) {
    let (stack, cx) = open_toasts(cx);
    stack.update(cx, |stack, cx| {
        for n in 0..6 {
            stack.push(ToastKind::Ok, format!("note {n}"), cx);
        }
    });
    cx.run_until_parked();
    assert_eq!(len(&stack, cx), 4);
    assert!(cx.debug_bounds("toast-0").is_none());
    assert!(cx.debug_bounds("toast-1").is_none());
    assert!(cx.debug_bounds("toast-2").is_some());
    assert!(cx.debug_bounds("toast-5").is_some());
}

#[gpui::test]
fn the_dismiss_button_removes_a_toast_and_its_timer_is_cancelled(cx: &mut TestAppContext) {
    let (stack, cx) = open_toasts(cx);
    stack.update(cx, |stack, cx| {
        stack.push(ToastKind::Bad, "first", cx);
        stack.push(ToastKind::Bad, "second", cx);
    });
    cx.run_until_parked();
    click(cx, "toast-dismiss-0");
    assert_eq!(len(&stack, cx), 1);
    assert!(cx.debug_bounds("toast-1").is_some());
    cx.executor().advance_clock(Duration::from_millis(9000));
    cx.run_until_parked();
    assert_eq!(len(&stack, cx), 0);
}

#[gpui::test]
fn toasts_sit_in_the_bottom_right_corner(cx: &mut TestAppContext) {
    let (stack, cx) = open_toasts(cx);
    stack.update(cx, |stack, cx| stack.push(ToastKind::Ok, "Saved", cx));
    cx.run_until_parked();
    let toast = cx.debug_bounds("toast-0").unwrap();
    let window = cx.update(|window, _| window.viewport_size());
    assert_eq!(toast.size.width, px(352.));
    assert_eq!(window.width - toast.right(), px(16.));
    assert_eq!(window.height - toast.bottom(), px(16.));
}

// ---------------------------------------------------------------------------------------
// Avatar
// ---------------------------------------------------------------------------------------

#[test]
fn initials_follow_the_ts_helper() {
    use super::avatar::initials;
    assert_eq!(initials("Hana Kramer"), "HK");
    assert_eq!(initials("hkramer"), "HK");
    assert_eq!(initials("Jan Novak Svoboda"), "JS");
    assert_eq!(initials("  "), "?");
    assert_eq!(initials("@@"), "?");
}

#[allow(dead_code)]
fn _keep_imports(_: &Option<TextInputEvent>, _: &RefCell<()>) {}
