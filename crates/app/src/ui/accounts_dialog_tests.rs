//! Interaction tests for the accounts dialog: adding, editing, removing, and every way a
//! connection can fail.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use reviewdeck_core::http::{MockFailure, MockRequest, MockResponse};

use super::*;
use crate::ui::thread_view::test_support::{Env, boot, click, open_dialog, sign_in, take_said};

const USER: &str = r#"{"login":"octocat","name":"The Octocat","avatar_url":""}"#;

fn github_ok(request: &MockRequest) -> MockResponse {
    if request.url.ends_with("/user") {
        MockResponse::new(200, USER)
    } else {
        // The deck refresh that follows a connection asks for its reviews.
        MockResponse::new(200, r#"{"items":[]}"#)
    }
}

fn dialog(
    cx: &mut TestAppContext,
) -> (
    Entity<AccountsDialog>,
    &mut VisualTestContext,
    Rc<RefCell<usize>>,
) {
    let (dialog, vcx) = open_dialog(cx, AccountsDialog::new);
    let dismissed: Rc<RefCell<usize>> = Rc::default();
    let count = dismissed.clone();
    let subscription = vcx.update(|_, cx| {
        cx.subscribe(&dialog, move |_, _: &DismissEvent, _| {
            *count.borrow_mut() += 1
        })
    });
    std::mem::forget(subscription);
    (dialog, vcx, dismissed)
}

/// Types into the token field, which is not a `TextInput`.
fn type_token(dialog: &Entity<AccountsDialog>, vcx: &mut VisualTestContext, text: &str) {
    let token = dialog.read_with(vcx, |dialog, _| dialog.token.clone());
    vcx.update(|window, cx| token.focus_handle(cx).focus(window));
    vcx.simulate_input(text);
}

/// Types into one of the dialog's text fields.
fn type_into(vcx: &mut VisualTestContext, field: Entity<TextInput>, text: &str) {
    vcx.update(|window, cx| field.read(cx).focus(window));
    vcx.simulate_input(text);
}

fn token_of(dialog: &Entity<AccountsDialog>, vcx: &VisualTestContext) -> String {
    dialog.read_with(vcx, |dialog, cx| dialog.token.read(cx).value().to_string())
}

fn field_text(vcx: &VisualTestContext, field: &Entity<TextInput>) -> String {
    field.read_with(vcx, |input, _| input.text().to_string())
}

fn authorization(env: &Env, url_end: &str) -> Option<String> {
    let requests = env.requests.lock().expect("lock");
    requests
        .iter()
        .find(|request| request.url.ends_with(url_end))
        .and_then(|request| request.header("authorization").map(str::to_owned))
}

#[gpui::test]
fn connecting_a_github_account_signs_in_and_closes_the_form(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    assert!(dialog.read_with(vcx, |d, _| d.adding && d.kind == ProviderKind::Github));
    assert_eq!(
        field_text(vcx, &dialog.read_with(vcx, |d, _| d.host.clone())),
        "github.com"
    );

    // Nothing typed: Connect is off and Enter does nothing.
    assert!(!dialog.read_with(vcx, |d, cx| d.can_save(cx)));
    click(vcx, "accounts-save");
    assert_eq!(env.count("/user"), 0);

    type_token(&dialog, vcx, "  ghp_secret123  ");
    assert_eq!(token_of(&dialog, vcx), "  ghp_secret123  ");
    assert!(dialog.read_with(vcx, |d, cx| d.can_save(cx)));
    click(vcx, "accounts-save");

    assert_eq!(env.count("https://api.github.com/user"), 1);
    let sent = authorization(&env, "/user").unwrap_or_default();
    assert!(sent.contains("ghp_secret123"), "{sent}");
    assert!(!sent.contains("  "), "the token goes out trimmed: {sent:?}");
    let accounts = env.vault.list_accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "GitHub (octocat)");
    assert_eq!(accounts[0].display_name, "The Octocat");
    assert_eq!(
        env.vault.get_token(&accounts[0].id).ok().as_deref(),
        Some("ghp_secret123")
    );
    assert_eq!(
        take_said(),
        vec![(ToastKind::Ok, "Signed in as The Octocat.".to_string())]
    );
    // Back on the list, with nothing left of the form.
    assert!(dialog.read_with(vcx, |d, _| !d.form_open() && !d.busy));
    assert_eq!(token_of(&dialog, vcx), "");
    assert_eq!(dialog.read_with(vcx, |d, _| d.accounts.len()), 1);
}

#[gpui::test]
fn the_name_and_the_claude_command_are_stored_with_the_account(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    let (label, command) = dialog.read_with(vcx, |d, _| (d.label.clone(), d.agent_command.clone()));
    type_into(vcx, label, " Work ");
    type_into(vcx, command, " claude-work ");
    type_token(&dialog, vcx, "ghp_x");
    click(vcx, "accounts-save");
    let accounts = env.vault.list_accounts();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "Work");
    assert_eq!(accounts[0].agent_command.as_deref(), Some("claude-work"));
}

#[gpui::test]
fn enter_in_the_token_field_connects(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    type_token(&dialog, vcx, "ghp_enter");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(env.vault.list_accounts().len(), 1);
    assert!(!dialog.read_with(vcx, |d, _| d.form_open()));
}

#[gpui::test]
fn enter_with_an_empty_token_does_nothing(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    type_token(&dialog, vcx, "   ");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(env.count("/user"), 0);
    assert!(dialog.read_with(vcx, |d, _| d.form_open()));
}

#[gpui::test]
fn a_rejected_token_says_why_and_keeps_the_form(cx: &mut TestAppContext) {
    let env = boot(cx, |_| {
        MockResponse::new(401, r#"{"message":"Bad credentials"}"#)
    });
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    type_token(&dialog, vcx, "ghp_wrong");
    click(vcx, "accounts-save");

    assert!(env.vault.list_accounts().is_empty());
    let said = take_said();
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].0, ToastKind::Bad);
    assert!(!said[0].1.is_empty());
    assert!(
        !said[0].1.contains("ghp_wrong"),
        "the token never reaches a toast: {}",
        said[0].1
    );
    // Still adding, still holding what was typed, and ready for another go.
    assert!(dialog.read_with(vcx, |d, _| d.adding && !d.busy));
    assert_eq!(token_of(&dialog, vcx), "ghp_wrong");
    click(vcx, "accounts-save");
    assert_eq!(env.count("/user"), 2);
}

#[gpui::test]
fn an_unreachable_host_says_so(cx: &mut TestAppContext) {
    let env = boot(cx, |_| {
        let mut response = MockResponse::new(0, "");
        response.failure = Some(MockFailure::Unreachable("offline".into()));
        response
    });
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    type_token(&dialog, vcx, "ghp_x");
    click(vcx, "accounts-save");
    let said = take_said();
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].0, ToastKind::Bad);
    assert!(said[0].1.contains("Could not reach"), "{}", said[0].1);
    assert!(env.vault.list_accounts().is_empty());
    assert!(dialog.read_with(vcx, |d, _| !d.busy));
}

#[gpui::test]
fn a_second_connect_while_verifying_is_ignored(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    type_token(&dialog, vcx, "ghp_x");
    dialog.update(vcx, |d, cx| {
        d.save(cx);
        assert!(d.busy);
        d.save(cx);
    });
    vcx.run_until_parked();
    assert_eq!(env.count("https://api.github.com/user"), 1);
    assert_eq!(env.vault.list_accounts().len(), 1);
}

#[gpui::test]
fn cancel_forgets_what_was_typed(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    let label = dialog.read_with(vcx, |d, _| d.label.clone());
    type_into(vcx, label.clone(), "Half done");
    type_token(&dialog, vcx, "ghp_half");
    click(vcx, "accounts-cancel");
    assert!(dialog.read_with(vcx, |d, _| !d.form_open()));
    assert_eq!(token_of(&dialog, vcx), "");
    assert_eq!(field_text(vcx, &label), "");
    assert_eq!(env.count("/user"), 0);
}

#[gpui::test]
fn choosing_a_provider_sets_its_host(cx: &mut TestAppContext) {
    let _env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    let host = dialog.read_with(vcx, |d, _| d.host.clone());

    click(vcx, "provider-gitlab");
    assert_eq!(dialog.read_with(vcx, |d, _| d.kind), ProviderKind::Gitlab);
    assert_eq!(field_text(vcx, &host), "gitlab.com");
    assert!(!host.read_with(vcx, |h, _| h.is_disabled()));

    click(vcx, "provider-forgejo");
    assert_eq!(field_text(vcx, &host), "codeberg.org");

    // Bitbucket is Cloud only: the host is fixed, and a username is asked for.
    click(vcx, "provider-bitbucket");
    assert_eq!(field_text(vcx, &host), "bitbucket.org");
    assert!(host.read_with(vcx, |h, _| h.is_disabled()));

    click(vcx, "provider-github");
    assert_eq!(field_text(vcx, &host), "github.com");
    assert!(!host.read_with(vcx, |h, _| h.is_disabled()));
}

#[gpui::test]
fn a_bitbucket_account_is_verified_with_its_username(cx: &mut TestAppContext) {
    let env = boot(cx, |request| {
        if request.url.contains("/user") {
            MockResponse::new(
                200,
                r#"{"username":"jdoe","display_name":"J Doe","links":{"avatar":{"href":""}}}"#,
            )
        } else {
            MockResponse::new(200, r#"{"values":[]}"#)
        }
    });
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    click(vcx, "provider-bitbucket");
    let username = dialog.read_with(vcx, |d, _| d.username.clone());
    type_into(vcx, username, "jdoe");
    type_token(&dialog, vcx, "app-password");
    click(vcx, "accounts-save");
    let sent = authorization(&env, "/user").unwrap_or_default();
    assert!(sent.starts_with("Basic "), "{sent}");
    assert_eq!(env.vault.list_accounts().len(), 1, "{:?}", take_said());
}

#[gpui::test]
fn the_token_link_opens_the_page_that_mints_one(cx: &mut TestAppContext) {
    let _env = boot(cx, github_ok);
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, "accounts-add");
    click(vcx, "create-token");
    let opened = vcx.opened_url().unwrap_or_default();
    assert!(
        opened.starts_with("https://github.com/settings/"),
        "{opened}"
    );

    click(vcx, "provider-gitlab");
    click(vcx, "create-token");
    let opened = vcx.opened_url().unwrap_or_default();
    assert!(opened.starts_with("https://gitlab.com/"), "{opened}");
    drop(dialog);
}

#[gpui::test]
fn editing_with_a_blank_token_keeps_the_stored_one(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let account = sign_in(&env, "Work GitHub");
    let (dialog, vcx, _dismissed) = dialog(cx);

    click(vcx, &format!("edit-{}", account.label));
    let (host, label, token) = dialog.read_with(vcx, |d, _| {
        (d.host.clone(), d.label.clone(), d.token.clone())
    });
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.editing_id.clone()),
        Some(account.id.clone())
    );
    assert_eq!(field_text(vcx, &host), "github.com");
    assert_eq!(field_text(vcx, &label), "Work GitHub");
    assert_eq!(token.read_with(vcx, |t, _| t.value().to_string()), "");
    // A blank token is allowed when editing.
    assert!(dialog.read_with(vcx, |d, cx| d.can_save(cx)));

    // The provider cannot change under an account.
    click(vcx, "provider-gitlab");
    assert_eq!(dialog.read_with(vcx, |d, _| d.kind), ProviderKind::Github);

    type_into(vcx, label.clone(), " Renamed");
    click(vcx, "accounts-save");

    assert_eq!(
        authorization(&env, "/user").map(|a| a.contains("ghp_test")),
        Some(true)
    );
    assert_eq!(
        env.vault.get_token(&account.id).ok().as_deref(),
        Some("ghp_test")
    );
    let stored = env.vault.list_accounts();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].label, "Work GitHub Renamed");
    assert_eq!(
        take_said(),
        vec![(ToastKind::Ok, "Updated Work GitHub Renamed.".to_string())]
    );
    assert!(dialog.read_with(vcx, |d, _| !d.form_open()));
}

#[gpui::test]
fn editing_with_a_new_token_replaces_it_and_blank_command_clears_the_override(
    cx: &mut TestAppContext,
) {
    let env = boot(cx, github_ok);
    let account = sign_in(&env, "Work GitHub");
    env.vault
        .update_account(&account.id, |a| a.agent_command = Some("old-claude".into()))
        .expect("the account is there");
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, &format!("edit-{}", account.label));
    let command = dialog.read_with(vcx, |d, _| d.agent_command.clone());
    assert_eq!(field_text(vcx, &command), "old-claude");
    command.update(vcx, |input, cx| input.set_text("", cx));
    type_token(&dialog, vcx, "ghp_new");
    click(vcx, "accounts-save");
    assert_eq!(
        env.vault.get_token(&account.id).ok().as_deref(),
        Some("ghp_new")
    );
    assert_eq!(env.vault.list_accounts()[0].agent_command, None);
}

#[gpui::test]
fn a_failed_edit_keeps_the_form_and_the_old_account(cx: &mut TestAppContext) {
    let env = boot(cx, |_| {
        MockResponse::new(401, r#"{"message":"Bad credentials"}"#)
    });
    let account = sign_in(&env, "Work GitHub");
    let (dialog, vcx, _dismissed) = dialog(cx);
    click(vcx, &format!("edit-{}", account.label));
    type_token(&dialog, vcx, "ghp_bad");
    click(vcx, "accounts-save");
    assert_eq!(take_said().len(), 1);
    assert!(dialog.read_with(vcx, |d, _| d.editing_id.is_some() && !d.busy));
    assert_eq!(
        env.vault.get_token(&account.id).ok().as_deref(),
        Some("ghp_test")
    );
    assert_eq!(env.vault.list_accounts()[0].label, "Work GitHub");
}

#[gpui::test]
fn removing_an_account_signs_it_out(cx: &mut TestAppContext) {
    let env = boot(cx, github_ok);
    let first = sign_in(&env, "Work GitHub");
    let second = sign_in(&env, "Home GitHub");
    let (dialog, vcx, _dismissed) = dialog(cx);
    assert_eq!(dialog.read_with(vcx, |d, _| d.accounts.len()), 2);

    click(vcx, &format!("remove-{}", first.label));
    let left = env.vault.list_accounts();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, second.id);
    assert_eq!(
        take_said(),
        vec![(ToastKind::Info, "Removed Work GitHub.".to_string())]
    );
    assert_eq!(dialog.read_with(vcx, |d, _| d.accounts.len()), 1);

    click(vcx, &format!("remove-{}", second.label));
    assert!(env.vault.list_accounts().is_empty());
    assert!(dialog.read_with(vcx, |d, _| d.accounts.is_empty()));
}

#[gpui::test]
fn escape_closes_the_dialog_wherever_the_focus_is(cx: &mut TestAppContext) {
    let _env = boot(cx, github_ok);
    let (dialog, vcx, dismissed) = dialog(cx);
    // On the list.
    vcx.simulate_keystrokes("escape");
    assert_eq!(*dismissed.borrow(), 1);

    // In the token field.
    click(vcx, "accounts-add");
    type_token(&dialog, vcx, "ghp_x");
    vcx.simulate_keystrokes("escape");
    assert_eq!(*dismissed.borrow(), 2);

    // In a text field.
    let label = dialog.read_with(vcx, |d, _| d.label.clone());
    type_into(vcx, label, "x");
    vcx.simulate_keystrokes("escape");
    assert_eq!(*dismissed.borrow(), 3);
}

#[test]
fn the_save_button_reads_by_what_it_is_doing() {
    assert_eq!(save_label(false, false), "Connect");
    assert_eq!(save_label(false, true), "Save");
    assert_eq!(save_label(true, false), "Verifying…");
    assert_eq!(save_label(true, true), "Verifying…");
}

#[test]
fn every_provider_names_its_scopes_and_host_hint() {
    for kind in ProviderKind::ALL {
        let guide = guide(kind);
        assert!(!guide.host.is_empty() && !guide.scopes.is_empty() && !guide.host_hint.is_empty());
    }
    assert_eq!(guide(ProviderKind::Github).scopes, "repo, read:org");
    assert_eq!(guide(ProviderKind::Gitlab).scopes, "api");
    assert_eq!(
        guide(ProviderKind::Forgejo).scopes,
        "issue: Read and write · repository: Read and write · user: Read"
    );
    assert_eq!(
        guide(ProviderKind::Bitbucket).scopes,
        "Account: Read · Pull requests: Write"
    );
}
