//! Interaction tests for [`TextInput`], driven through gpui's test platform: real key
//! dispatch through the keymap, real mouse events, the real layout.

use super::*;
use crate::ui::components::testing::{Log, init, log, window};
use gpui::{AppContext as _, Modifiers, TestAppContext, VisualTestContext};

type Opened<'a> = (
    Entity<TextInput>,
    &'a mut VisualTestContext,
    Log<TextInputEvent>,
);

/// A focused input in a window, with the events it emits collected.
fn open(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Context<TextInput>) -> TextInput,
) -> Opened<'_> {
    init(cx);
    let input = cx.new(build);
    let events = log();
    {
        let events = events.clone();
        cx.update(|cx| {
            cx.subscribe(&input, move |_, event: &TextInputEvent, _| {
                events.borrow_mut().push(*event)
            })
            .detach()
        });
    }
    let shown = input.clone();
    let (_host, vcx) = window(cx, move |_, _| shown.clone().into_any_element());
    vcx.update(|window, cx| input.read(cx).focus(window));
    vcx.run_until_parked();
    (input, vcx, events)
}

fn single(cx: &mut TestAppContext) -> Opened<'_> {
    open(cx, TextInput::new)
}

fn text(input: &Entity<TextInput>, cx: &VisualTestContext) -> String {
    input.read_with(cx, |input, _| input.text().to_string())
}

fn selection(input: &Entity<TextInput>, cx: &VisualTestContext) -> Range<usize> {
    input.read_with(cx, |input, _| input.selection())
}

fn clipboard(cx: &VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[gpui::test]
fn typing_inserts_at_the_caret(cx: &mut TestAppContext) {
    let (input, cx, events) = single(cx);
    cx.simulate_input("hello");
    assert_eq!(text(&input, cx), "hello");
    cx.simulate_keystrokes("left left");
    cx.simulate_input("X");
    assert_eq!(text(&input, cx), "helXlo");
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| *event == TextInputEvent::Changed)
    );
}

#[gpui::test]
fn selection_keys_replace_and_delete(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.simulate_input("hello world");
    cx.simulate_keystrokes("shift-left shift-left shift-left shift-left shift-left");
    assert_eq!(selection(&input, cx), 6..11);
    cx.simulate_input("there");
    assert_eq!(text(&input, cx), "hello there");
    cx.simulate_keystrokes("cmd-a backspace");
    assert_eq!(text(&input, cx), "");
    // Alt-backspace deletes a word, cmd-backspace to the start of the line.
    cx.simulate_input("one two three");
    cx.simulate_keystrokes("alt-backspace");
    assert_eq!(text(&input, cx), "one two ");
    cx.simulate_keystrokes("cmd-backspace");
    assert_eq!(text(&input, cx), "");
}

#[gpui::test]
fn word_and_line_movement(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.simulate_input("foo bar baz");
    cx.simulate_keystrokes("alt-left");
    assert_eq!(selection(&input, cx), 8..8);
    cx.simulate_keystrokes("cmd-left");
    assert_eq!(selection(&input, cx), 0..0);
    cx.simulate_keystrokes("alt-shift-right");
    assert_eq!(selection(&input, cx), 0..3);
    cx.simulate_keystrokes("cmd-right");
    assert_eq!(selection(&input, cx), 11..11);
}

#[gpui::test]
fn clipboard_copy_cut_and_paste(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.simulate_input("alpha beta");
    cx.simulate_keystrokes("cmd-a cmd-c");
    assert_eq!(clipboard(cx).as_deref(), Some("alpha beta"));
    cx.simulate_keystrokes("right cmd-v");
    assert_eq!(text(&input, cx), "alpha betaalpha beta");
    cx.simulate_keystrokes("cmd-a cmd-x");
    assert_eq!(text(&input, cx), "");
    assert_eq!(clipboard(cx).as_deref(), Some("alpha betaalpha beta"));
}

#[gpui::test]
fn a_single_line_paste_turns_newlines_into_spaces(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("a\nb\r\nc".into()));
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(text(&input, cx), "a b c");
}

#[gpui::test]
fn undo_and_redo_step_by_word(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.simulate_input("one two");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&input, cx), "one ");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&input, cx), "one");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&input, cx), "");
    cx.simulate_keystrokes("cmd-shift-z");
    assert_eq!(text(&input, cx), "one");
    // An edit after an undo discards the redo history.
    cx.simulate_input("!");
    cx.simulate_keystrokes("cmd-shift-z");
    assert_eq!(text(&input, cx), "one!");
}

#[gpui::test]
fn enter_escape_and_cmd_enter_emit_events(cx: &mut TestAppContext) {
    let (input, cx, events) = single(cx);
    cx.simulate_input("x");
    events.borrow_mut().clear();
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(
        *events.borrow(),
        vec![
            TextInputEvent::Submit,
            TextInputEvent::Cancel,
            TextInputEvent::Submit
        ]
    );
    assert_eq!(text(&input, cx), "x");
}

#[gpui::test]
fn multi_line_enter_inserts_a_newline_and_cmd_enter_submits(cx: &mut TestAppContext) {
    let (input, cx, events) = open(cx, |cx| TextInput::new(cx).multi_line(2, 4));
    cx.simulate_input("a");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("b");
    assert_eq!(text(&input, cx), "a\nb");
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(events.borrow().last(), Some(&TextInputEvent::Submit));
    assert_eq!(text(&input, cx), "a\nb");
}

#[gpui::test]
fn set_rows_changes_the_height_after_construction(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).multi_line(1, 1));
    let one = cx.debug_bounds("text-input").unwrap().size.height;
    input.update(cx, |input, cx| input.set_rows(3, 5, cx));
    cx.run_until_parked();
    let three = cx.debug_bounds("text-input").unwrap().size.height;
    assert!(three > one + px(30.), "{three:?} vs {one:?}");
    for digit in 1..=6 {
        cx.simulate_input(&digit.to_string());
        cx.simulate_keystrokes("enter");
    }
    let capped = cx.debug_bounds("text-input").unwrap().size.height;
    // Five rows at most, then it scrolls.
    assert!(capped > three && capped < three + px(60.), "{capped:?}");
}

#[gpui::test]
fn the_single_line_field_is_36px_high_and_height_overrides_it(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    assert_eq!(cx.debug_bounds("text-input").unwrap().size.height, px(36.));
    input.update(cx, |input, cx| input.set_height(Some(32.), cx));
    cx.run_until_parked();
    assert_eq!(cx.debug_bounds("text-input").unwrap().size.height, px(32.));
}

#[gpui::test]
fn padding_left_moves_the_text_in(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    let before = input.read_with(cx, |input, _| input.text_bounds().unwrap().origin.x);
    input.update(cx, |input, cx| input.set_padding_left(Some(30.), cx));
    cx.run_until_parked();
    let after = input.read_with(cx, |input, _| input.text_bounds().unwrap().origin.x);
    assert_eq!(after - before, px(30. - 12.));
}

#[gpui::test]
fn the_disabled_field_ignores_typing(cx: &mut TestAppContext) {
    let (input, cx, events) = single(cx);
    input.update(cx, |input, cx| input.set_disabled(true, cx));
    cx.run_until_parked();
    cx.simulate_input("nope");
    assert_eq!(text(&input, cx), "");
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn clicking_places_the_caret_and_double_click_selects_a_word(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.simulate_input("alpha beta");
    let line_height = input.read_with(cx, |input, _| input.line_height().unwrap());
    let nudge = point(px(0.5), line_height / 2.);
    // Click between "alph" and "a".
    let at = input.read_with(cx, |input, _| input.position_for_offset(4).unwrap());
    cx.simulate_click(at + nudge, Modifiers::default());
    assert_eq!(selection(&input, cx), 4..4);
    // Shift-click extends.
    let end = input.read_with(cx, |input, _| input.position_for_offset(10).unwrap());
    cx.simulate_click(end + nudge, Modifiers::shift());
    assert_eq!(selection(&input, cx), 4..10);
    // Double click on "beta".
    let beta = input.read_with(cx, |input, _| input.position_for_offset(7).unwrap());
    cx.simulate_event(MouseDownEvent {
        position: beta + nudge,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    assert_eq!(selection(&input, cx), 6..10);
}

#[gpui::test]
fn the_placeholder_is_laid_out_while_empty(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).placeholder("Filter"));
    let is_placeholder = |cx: &VisualTestContext| {
        input.read_with(cx, |input, _| input.layout.as_ref().unwrap().is_placeholder)
    };
    assert!(is_placeholder(cx));
    cx.simulate_input("a");
    assert!(!is_placeholder(cx));
    assert_eq!(text(&input, cx), "a");
}

#[gpui::test]
fn a_marked_ime_composition_is_replaced_not_appended(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "k", None, window, cx);
            assert_eq!(input.text(), "k");
            input.replace_and_mark_text_in_range(None, "ka", None, window, cx);
            assert_eq!(input.text(), "ka");
            input.replace_text_in_range(None, "\u{304b}", window, cx);
            assert_eq!(input.text(), "\u{304b}");
            assert!(input.marked_range.is_none());
        })
    });
}

// ---- masked mode ----

#[gpui::test]
fn masked_fields_draw_bullets_but_keep_the_secret(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).masked(true));
    cx.simulate_input("s3cret\u{e9}");
    assert_eq!(text(&input, cx), "s3cret\u{e9}");
    input.read_with(cx, |input, _| {
        let displayed = input.display_text();
        assert_eq!(displayed.chars().count(), 7);
        assert!(displayed.chars().all(|c| c == MASK));
        // Offsets round-trip through the bullets, including the 2-byte character.
        for ix in [0, 3, 6, 8] {
            assert_eq!(input.display_to_content(input.content_to_display(ix)), ix);
        }
        assert_eq!(input.content_to_display(8), 21);
    });
}

#[gpui::test]
fn masked_fields_do_not_copy_or_cut(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).masked(true));
    cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    cx.simulate_input("token");
    cx.simulate_keystrokes("cmd-a cmd-c");
    assert_eq!(clipboard(cx).as_deref(), Some("before"));
    cx.simulate_keystrokes("cmd-x");
    assert_eq!(text(&input, cx), "token");
    assert_eq!(clipboard(cx).as_deref(), Some("before"));
    // Pasting a token in works.
    cx.simulate_keystrokes("cmd-a cmd-v");
    assert_eq!(text(&input, cx), "before");
}

#[gpui::test]
fn masked_word_movement_treats_the_value_as_one_word(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).masked(true));
    cx.simulate_input("ab cd ef");
    cx.simulate_keystrokes("alt-left");
    assert_eq!(selection(&input, cx), 0..0);
    cx.simulate_keystrokes("alt-right");
    assert_eq!(selection(&input, cx), 8..8);
    cx.simulate_keystrokes("alt-backspace");
    assert_eq!(text(&input, cx), "");
}

#[gpui::test]
fn masked_fields_offer_no_text_to_the_platform_and_never_compose(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).masked(true));
    cx.simulate_input("abc");
    let offered = cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "d", None, window, cx);
            assert!(
                input.marked_range.is_none(),
                "no composition in a secret field"
            );
            assert_eq!(input.text(), "abcd");
            let mut actual = None;
            input.text_for_range(0..3, &mut actual, window, cx)
        })
    });
    assert_eq!(offered, None);
}

#[gpui::test]
fn masked_selection_and_clicks_land_on_the_right_character(cx: &mut TestAppContext) {
    let (input, cx, _) = open(cx, |cx| TextInput::new(cx).masked(true));
    cx.simulate_input("abcdef");
    cx.simulate_keystrokes("shift-left shift-left");
    assert_eq!(selection(&input, cx), 4..6);
    let line_height = input.read_with(cx, |input, _| input.line_height().unwrap());
    let at = input.read_with(cx, |input, _| input.position_for_offset(2).unwrap());
    cx.simulate_click(at + point(px(0.5), line_height / 2.), Modifiers::default());
    assert_eq!(selection(&input, cx), 2..2);
}

#[gpui::test]
fn escape_emits_cancel(cx: &mut TestAppContext) {
    // The input also lets Escape travel to its ancestors; the dialog test covers the receiver.
    let (_input, cx, events) = single(cx);
    cx.simulate_keystrokes("escape");
    assert_eq!(*events.borrow(), vec![TextInputEvent::Cancel]);
}

#[gpui::test]
fn the_caret_blinks_while_focused_and_is_solid_after_typing(cx: &mut TestAppContext) {
    let (input, cx, _) = single(cx);
    assert!(input.read_with(cx, |input, _| input.blink_on));
    cx.executor()
        .advance_clock(BLINK + std::time::Duration::from_millis(10));
    cx.run_until_parked();
    assert!(!input.read_with(cx, |input, _| input.blink_on));
    cx.simulate_input("a");
    assert!(input.read_with(cx, |input, _| input.blink_on));
}
