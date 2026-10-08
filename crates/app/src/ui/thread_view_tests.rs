//! Interaction tests for the thread card: reply, cancel, resolve and their failures.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use reviewdeck_core::http::{MockRequest, MockResponse};
use reviewdeck_core::model::{CommentThread, PullComment, User};

use super::test_support::{Env, boot, click, deck_with_review, take_said};
use super::*;

fn comment(id: &str, who: &str, body: &str) -> PullComment {
    PullComment {
        id: id.into(),
        author: User {
            name: who.into(),
            avatar_url: String::new(),
        },
        body: body.into(),
        created_at: "2026-01-01T10:00:00Z".into(),
    }
}

fn thread(can_reply: bool, can_resolve: bool, resolved: bool) -> CommentThread {
    CommentThread {
        id: "PRRT_1".into(),
        comments: vec![
            comment("1", "alice", "Why is this a clone?"),
            comment("2", "bob", "Because of the borrow."),
        ],
        resolved,
        outdated: false,
        path: Some("src/lib.rs".into()),
        line: Some(12),
        start_line: None,
        side: None,
        can_reply,
        can_resolve,
    }
}

/// A card for a thread of a review in the deck, and the events it emitted.
fn card<'a>(
    cx: &'a mut TestAppContext,
    env: &Env,
    thread: CommentThread,
    dense: bool,
) -> (
    Entity<ThreadCard>,
    &'a mut VisualTestContext,
    Rc<RefCell<Vec<ThreadEvent>>>,
) {
    let item = deck_with_review(cx, env);
    let (card, vcx) =
        cx.add_window_view(|window, cx| ThreadCard::new(item.id, thread, dense, window, cx));
    let events: Rc<RefCell<Vec<ThreadEvent>>> = Rc::default();
    let log = events.clone();
    let subscription = vcx.update(|_, cx| {
        cx.subscribe(&card, move |_, event: &ThreadEvent, _| {
            log.borrow_mut().push(*event)
        })
    });
    std::mem::forget(subscription);
    (card, vcx, events)
}

fn graphql_ok(_: &MockRequest) -> MockResponse {
    MockResponse::new(200, r#"{"data":{"ok":true}}"#)
}

fn typed(card: &Entity<ThreadCard>, cx: &VisualTestContext) -> String {
    card.read_with(cx, |card, cx| {
        card.reply
            .as_ref()
            .map(|reply| reply.input.read(cx).text().to_string())
            .unwrap_or_default()
    })
}

#[gpui::test]
fn replying_sends_the_trimmed_text_and_closes_the_box(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (card, vcx, events) = card(cx, &env, thread(true, true, false), false);
    click(vcx, "reply");
    assert!(card.read_with(vcx, |card, _| card.reply.is_some()));
    // The send button is off while there is nothing to send.
    let before = env.count("/graphql");
    click(vcx, "reply-send");
    assert_eq!(env.count("/graphql"), before, "an empty reply is not sent");

    vcx.simulate_input("   Fair enough.  ");
    assert_eq!(typed(&card, vcx), "   Fair enough.  ");
    click(vcx, "reply-send");

    let seen = env.seen();
    let mutation = seen
        .iter()
        .find(|(url, body)| {
            url.ends_with("/graphql")
                && body
                    .as_deref()
                    .is_some_and(|b| b.contains("addPullRequestReviewThreadReply"))
        })
        .expect("the reply went to GraphQL");
    let body = mutation.1.clone().unwrap_or_default();
    assert!(body.contains(r#""body":"Fair enough.""#), "{body}");
    assert!(body.contains("PRRT_1"), "{body}");
    assert!(card.read_with(vcx, |card, _| card.reply.is_none()));
    assert_eq!(
        take_said(),
        vec![(ToastKind::Ok, "Reply posted.".to_string())]
    );
    assert_eq!(*events.borrow(), vec![ThreadEvent::Changed]);
    // The Reply button is back: a second reply can be started.
    click(vcx, "reply");
    assert!(card.read_with(vcx, |card, _| card.reply.is_some()));
}

#[gpui::test]
fn cmd_enter_sends_and_escape_cancels(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (card, vcx, events) = card(cx, &env, thread(true, false, false), false);
    let before = env.count("/graphql");
    click(vcx, "reply");
    vcx.simulate_input("draft text");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(card.read_with(vcx, |card, _| card.reply.is_none()));
    assert_eq!(env.count("/graphql"), before);
    assert!(events.borrow().is_empty());

    // A new box starts empty: cancelling forgot the text.
    click(vcx, "reply");
    assert_eq!(typed(&card, vcx), "");
    vcx.simulate_input("sent with the keyboard");
    vcx.simulate_keystrokes("cmd-enter");
    vcx.run_until_parked();
    assert_eq!(env.count("/graphql"), before + 1);
    assert!(card.read_with(vcx, |card, _| card.reply.is_none()));
    assert_eq!(*events.borrow(), vec![ThreadEvent::Changed]);
}

#[gpui::test]
fn cancel_button_closes_the_box_without_sending(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (card, vcx, _events) = card(cx, &env, thread(true, false, false), false);
    let before = env.count("/graphql");
    click(vcx, "reply");
    vcx.simulate_input("never mind");
    click(vcx, "reply-cancel");
    assert!(card.read_with(vcx, |card, _| card.reply.is_none()));
    assert_eq!(env.count("/graphql"), before);
    assert!(take_said().is_empty());
}

#[gpui::test]
fn a_failed_reply_keeps_the_text_and_says_why(cx: &mut TestAppContext) {
    let env = boot(cx, |_| {
        MockResponse::new(500, r#"{"message":"Server fell over"}"#)
    });
    let (card, vcx, events) = card(cx, &env, thread(true, false, false), false);
    click(vcx, "reply");
    vcx.simulate_input("please keep me");
    let before = env.count("/graphql");
    click(vcx, "reply-send");

    assert!(card.read_with(vcx, |card, _| card.reply.is_some()));
    assert_eq!(typed(&card, vcx), "please keep me");
    let said = take_said();
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].0, ToastKind::Bad);
    assert!(!said[0].1.is_empty());
    assert!(events.borrow().is_empty(), "nothing changed on the host");
    // Idle again: it can be sent a second time.
    assert!(!card.read_with(vcx, |card, _| card.reply.as_ref().is_some_and(|r| r.busy)));
    click(vcx, "reply-send");
    assert_eq!(env.count("/graphql"), before + 2);
}

#[gpui::test]
fn resolving_toggles_the_thread(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (_card, vcx, events) = card(cx, &env, thread(false, true, false), false);
    click(vcx, "resolve");
    let body = env
        .seen()
        .last()
        .and_then(|(_, body)| body.clone())
        .unwrap_or_default();
    assert!(body.contains("resolveReviewThread"), "{body}");
    assert!(!body.contains("unresolveReviewThread"), "{body}");
    assert_eq!(
        take_said(),
        vec![(ToastKind::Ok, "Thread resolved.".to_string())]
    );
    assert_eq!(*events.borrow(), vec![ThreadEvent::Changed]);
}

#[gpui::test]
fn a_resolved_thread_offers_reopen(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (card, vcx, events) = card(cx, &env, thread(false, true, true), false);
    click(vcx, "resolve");
    let body = env
        .seen()
        .last()
        .and_then(|(_, body)| body.clone())
        .unwrap_or_default();
    assert!(
        body.contains("unresolveReviewThread"),
        "reopening is the other mutation: {body}"
    );
    assert_eq!(
        take_said(),
        vec![(ToastKind::Ok, "Thread reopened.".to_string())]
    );
    assert_eq!(*events.borrow(), vec![ThreadEvent::Changed]);
    assert!(!card.read_with(vcx, |card, _| card.resolving));
}

#[gpui::test]
fn a_failed_resolve_says_why_and_can_be_retried(cx: &mut TestAppContext) {
    let env = boot(cx, |_| MockResponse::new(502, "Bad gateway"));
    let (card, vcx, events) = card(cx, &env, thread(false, true, false), false);
    let before = env.count("/graphql");
    click(vcx, "resolve");
    let said = take_said();
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].0, ToastKind::Bad);
    assert!(events.borrow().is_empty());
    assert!(!card.read_with(vcx, |card, _| card.resolving));
    click(vcx, "resolve");
    assert_eq!(env.count("/graphql"), before + 2);
}

#[gpui::test]
fn the_controls_follow_the_threads_own_flags(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (_card, vcx, _events) = card(cx, &env, thread(false, false, false), false);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("reply").is_none());
    assert!(vcx.debug_bounds("resolve").is_none());
}

#[gpui::test]
fn one_request_at_a_time_while_resolving(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (card, vcx, _events) = card(cx, &env, thread(false, true, false), false);
    let before = env.count("/graphql");
    card.update(vcx, |card, cx| {
        card.toggle_resolved(cx);
        card.toggle_resolved(cx);
    });
    vcx.run_until_parked();
    assert_eq!(env.count("/graphql"), before + 1);
}

#[gpui::test]
fn the_outcome_counts_even_if_the_box_was_closed_meanwhile(cx: &mut TestAppContext) {
    let env = boot(cx, graphql_ok);
    let (card, vcx, events) = card(cx, &env, thread(true, false, false), false);
    click(vcx, "reply");
    vcx.simulate_input("on its way");
    card.update(vcx, |card, cx| {
        card.send_reply(cx);
        card.cancel_reply(cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        take_said(),
        vec![(ToastKind::Ok, "Reply posted.".to_string())]
    );
    assert_eq!(*events.borrow(), vec![ThreadEvent::Changed]);
}
