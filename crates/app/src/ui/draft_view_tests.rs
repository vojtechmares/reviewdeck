//! Interaction tests for the draft card: edit, save, cancel, delete and their failures.

use gpui::{Entity, TestAppContext, VisualTestContext};
use reviewdeck_core::drafts::NewDraft;
use reviewdeck_core::http::MockResponse;
use reviewdeck_core::model::{DiffRefs, EdgeKind, LineRange, RangeEdge};

use super::*;
use crate::ui::thread_view::test_support::{Env, boot, click, deck_with_review, take_said};

/// A draft on the review in the deck, and a card showing it.
fn card<'a>(
    cx: &'a mut TestAppContext,
    env: &Env,
    body: &str,
) -> (Entity<DraftCard>, &'a mut VisualTestContext, String) {
    let item = deck_with_review(cx, env);
    let drafts = env.state.update(cx, |state, cx| {
        state
            .add_draft(
                NewDraft {
                    item_id: item.id.clone(),
                    body: body.to_string(),
                    path: "src/lib.rs".to_string(),
                    new_line: Some(4),
                    old_line: None,
                    range: None,
                    refs: DiffRefs::default(),
                },
                cx,
            )
            .expect("the draft is added")
    });
    let draft = drafts[0].clone();
    let id = draft.id.clone();
    let (card, vcx) = cx.add_window_view(|window, cx| DraftCard::new(draft, window, cx));
    (card, vcx, id)
}

fn stored(env: &Env, cx: &VisualTestContext, item_id: &str) -> Vec<String> {
    env.state
        .read_with(cx, |state, _| state.drafts(item_id))
        .into_iter()
        .map(|draft| draft.body)
        .collect()
}

fn editor_text(card: &Entity<DraftCard>, cx: &VisualTestContext) -> Option<String> {
    card.read_with(cx, |card, cx| {
        card.editor
            .as_ref()
            .map(|editor| editor.input.read(cx).text().to_string())
    })
}

#[gpui::test]
fn editing_starts_from_the_stored_body_and_saves_it_trimmed(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (card, vcx, _id) = card(cx, &env, "Original remark.");
    let item_id = card.read_with(vcx, |card, _| card.draft.item_id.clone());

    click(vcx, "draft-edit");
    assert_eq!(editor_text(&card, vcx).as_deref(), Some("Original remark."));

    // The caret is in the field, at the end: typing appends.
    vcx.simulate_input("  And more.  ");
    assert_eq!(
        editor_text(&card, vcx).as_deref(),
        Some("Original remark.  And more.  ")
    );
    click(vcx, "draft-save");

    assert_eq!(editor_text(&card, vcx), None, "saving closes the editor");
    assert_eq!(
        stored(&env, vcx, &item_id),
        vec!["Original remark.  And more."]
    );
    assert_eq!(
        card.read_with(vcx, |c, _| c.draft.body.clone()),
        "Original remark.  And more.",
        "the card shows what was saved without waiting for its owner"
    );
    assert!(take_said().is_empty());
}

#[gpui::test]
fn cmd_enter_saves_and_escape_abandons(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (card, vcx, _id) = card(cx, &env, "Keep me");
    let item_id = card.read_with(vcx, |card, _| card.draft.item_id.clone());

    click(vcx, "draft-edit");
    vcx.simulate_input(" changed");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_eq!(editor_text(&card, vcx), None);
    assert_eq!(stored(&env, vcx, &item_id), vec!["Keep me"]);

    // The abandoned edit left nothing behind.
    click(vcx, "draft-edit");
    assert_eq!(editor_text(&card, vcx).as_deref(), Some("Keep me"));
    vcx.simulate_input("!");
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    assert_eq!(editor_text(&card, vcx), None);
    assert_eq!(stored(&env, vcx, &item_id), vec!["Keep me!"]);
}

#[gpui::test]
fn the_cancel_button_abandons_the_edit(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (card, vcx, _id) = card(cx, &env, "Keep me");
    let item_id = card.read_with(vcx, |card, _| card.draft.item_id.clone());
    click(vcx, "draft-edit");
    vcx.simulate_input(" changed");
    click(vcx, "draft-cancel");
    assert_eq!(editor_text(&card, vcx), None);
    assert_eq!(stored(&env, vcx, &item_id), vec!["Keep me"]);
}

#[gpui::test]
fn an_empty_body_is_not_saved(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (card, vcx, _id) = card(cx, &env, "Short");
    let item_id = card.read_with(vcx, |card, _| card.draft.item_id.clone());
    click(vcx, "draft-edit");
    card.update(vcx, |card, cx| {
        if let Some(editor) = &card.editor {
            editor
                .input
                .update(cx, |input, cx| input.set_text("   ", cx));
        }
    });
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    click(vcx, "draft-save");
    assert_eq!(
        editor_text(&card, vcx).as_deref(),
        Some("   "),
        "the editor stays open"
    );
    assert_eq!(stored(&env, vcx, &item_id), vec!["Short"]);
    assert!(take_said().is_empty());
}

#[gpui::test]
fn a_draft_that_is_gone_keeps_the_editor_and_says_so(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (card, vcx, id) = card(cx, &env, "Doomed");
    click(vcx, "draft-edit");
    vcx.simulate_input("!");
    env.state.update(vcx, |state, cx| {
        state.remove_draft(&id, cx);
    });
    click(vcx, "draft-save");
    assert_eq!(editor_text(&card, vcx).as_deref(), Some("Doomed!"));
    assert_eq!(
        take_said(),
        vec![(ToastKind::Bad, "That draft is gone.".to_string())]
    );
}

#[gpui::test]
fn deleting_removes_the_draft(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(500, ""));
    let (card, vcx, _id) = card(cx, &env, "Not needed");
    let item_id = card.read_with(vcx, |card, _| card.draft.item_id.clone());
    click(vcx, "draft-delete");
    assert!(stored(&env, vcx, &item_id).is_empty());
}

#[test]
fn a_range_reads_first_line_to_anchor_line() {
    let edge = RangeEdge {
        kind: EdgeKind::Add,
        old_pos: 0,
        new_pos: 0,
    };
    let mut draft = DraftComment {
        id: "d".into(),
        item_id: "i".into(),
        body: "b".into(),
        path: "p".into(),
        new_line: Some(55),
        old_line: None,
        range: Some(LineRange {
            start_line: 53,
            start: edge,
            end: edge,
        }),
        created_at: String::new(),
        refs: DiffRefs::default(),
    };
    assert_eq!(range_label(&draft).as_deref(), Some("Lines 53-55"));
    // A comment on a removed line is anchored by its old line.
    draft.new_line = None;
    draft.old_line = Some(60);
    assert_eq!(range_label(&draft).as_deref(), Some("Lines 53-60"));
    draft.range = None;
    assert_eq!(range_label(&draft), None);
}
