//! Port of src/renderer/src/components/AccountsDialog.tsx: the list of connected
//! accounts, and the form that adds one or edits one.
//!
//! Reviewdeck signs in with personal access tokens rather than OAuth: every host a
//! freelancer runs into (self-hosted GitLab, a company Forgejo) can mint one, whereas
//! OAuth would need an app registered on each instance up front. The per-provider
//! [`guide`] below carries what a person needs to mint the right one.
//!
//! Everything goes through [`AppState`]: `add_account` and `update_account` verify the
//! token against the host and produce the user-facing error text, which is shown in a
//! toast exactly as the TSX did. The token is held only by a masked [`TextInput`] (the
//! `<Input type="password">`) and handed to `AppState`; it is never logged, never put in
//! a toast, never copied out of the field and never rendered (the field shows bullets).

use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder,
    px, relative,
};
use reviewdeck_core::model::{
    Account, AccountDraft, AccountStatus, DEFAULT_AGENT_COMMAND, ProviderKind,
};
use reviewdeck_core::token_url::token_create_url;

use crate::state::{AppState, GlobalState};
use crate::ui::components::avatar::Avatar;
use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::components::dialog::Dialog;
use crate::ui::components::glass::GlassExt;
use crate::ui::components::input::{TextInput, TextInputEvent, label};
use crate::ui::components::toast::ToastKind;
use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, UI_FONT, mono_font, radius, rpx};
use crate::ui::thread_view::{probe, say};

/// What a person needs to know to connect one provider.
struct Guide {
    /// The host the form starts with, and the placeholder.
    host: &'static str,
    /// The token scopes (or permissions) the app uses.
    scopes: &'static str,
    /// The line under the host field.
    host_hint: &'static str,
}

fn guide(kind: ProviderKind) -> &'static Guide {
    match kind {
        ProviderKind::Github => &Guide {
            host: "github.com",
            scopes: "repo, read:org",
            host_hint: "Use your Enterprise Server hostname for GHES, e.g. github.acme.com",
        },
        ProviderKind::Gitlab => &Guide {
            host: "gitlab.com",
            scopes: "api",
            host_hint: "Self-hosted works too, e.g. gitlab.acme.com",
        },
        ProviderKind::Forgejo => &Guide {
            host: "codeberg.org",
            scopes: "issue: Read and write · repository: Read and write · user: Read",
            host_hint: "Any Forgejo or Gitea instance",
        },
        ProviderKind::Bitbucket => &Guide {
            host: "bitbucket.org",
            scopes: "Account: Read · Pull requests: Write",
            host_hint: "Bitbucket Cloud only",
        },
    }
}

// ---------------------------------------------------------------------------------------------
// The dialog
// ---------------------------------------------------------------------------------------------

/// `AccountsDialog`. Render it as the last child of a `size_full` root, while open; it
/// emits [`DismissEvent`] when it wants to close (Escape, the scrim, the close button).
pub struct AccountsDialog {
    state: Entity<AppState>,
    focus: FocusHandle,
    /// Cached from `AppState` when it changes, so `render` does no cloning of the deck.
    accounts: Vec<Account>,
    statuses: Vec<AccountStatus>,
    agent_command_setting: String,
    adding: bool,
    editing_id: Option<String>,
    busy: bool,
    kind: ProviderKind,
    host: Entity<TextInput>,
    label: Entity<TextInput>,
    username: Entity<TextInput>,
    token: Entity<TextInput>,
    agent_command: Entity<TextInput>,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for AccountsDialog {}

impl Focusable for AccountsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AccountsDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        let focus = cx.focus_handle();
        focus.focus(window);

        let text_field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextInput::new(cx).placeholder(placeholder))
        };
        let host = text_field(guide(ProviderKind::Github).host, cx);
        let label = text_field("Work GitHub", cx);
        let username = text_field("your-handle", cx);
        let agent_command = text_field(DEFAULT_AGENT_COMMAND, cx);
        let token = cx.new(|cx| TextInput::new(cx).masked(true));

        // Escape inside a field reports `Cancel` and travels on to the Dialog, which closes
        // - the TSX dialog closed on Escape wherever the focus was.
        let mut subscriptions = vec![cx.observe(&state, |this, _, cx| {
            this.reload(cx);
            cx.notify();
        })];
        // Enter in the token field submits, when there is something to submit.
        subscriptions.push(cx.subscribe(&token, |this, _, event: &TextInputEvent, cx| {
            if matches!(event, TextInputEvent::Submit) && this.can_save(cx) {
                this.save(cx);
            }
        }));

        let mut dialog = AccountsDialog {
            state,
            focus,
            accounts: Vec::new(),
            statuses: Vec::new(),
            agent_command_setting: String::new(),
            adding: false,
            editing_id: None,
            busy: false,
            kind: ProviderKind::Github,
            host,
            label,
            username,
            token,
            agent_command,
            save_task: None,
            _subscriptions: subscriptions,
        };
        dialog.reload(cx);
        dialog
    }

    /// `reloadAccounts`, and the statuses and settings the rows and placeholders read.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        self.accounts = state.accounts();
        self.statuses = state.deck().statuses;
        self.agent_command_setting = state.settings().agent_command;
    }

    fn form_open(&self) -> bool {
        self.adding || self.editing_id.is_some()
    }

    /// `canSave`: a token is typed, or an existing account is being edited (blank keeps the
    /// stored token).
    fn can_save(&self, cx: &App) -> bool {
        !self.token.read(cx).text().trim().is_empty() || self.editing_id.is_some()
    }

    /// `reset`: leaves the form and forgets everything typed in it.
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.adding = false;
        self.editing_id = None;
        self.token.update(cx, |token, cx| token.set_text("", cx));
        self.label.update(cx, |input, cx| input.set_text("", cx));
        self.username.update(cx, |input, cx| input.set_text("", cx));
        self.agent_command
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    /// Switches the form to `kind` and fills the host and its placeholder with that
    /// provider's default. The host is fixed for Bitbucket (Cloud only).
    fn set_kind(&mut self, kind: ProviderKind, host: &str, cx: &mut Context<Self>) {
        self.kind = kind;
        let placeholder = guide(kind).host;
        self.host.update(cx, |input, cx| {
            input.set_text(host, cx);
            input.set_placeholder(placeholder, cx);
            input.set_disabled(kind == ProviderKind::Bitbucket, cx);
        });
        cx.notify();
    }

    /// `pickKind`: choosing a provider is only possible while adding.
    fn pick_kind(&mut self, kind: ProviderKind, cx: &mut Context<Self>) {
        if self.editing_id.is_some() {
            return;
        }
        self.set_kind(kind, guide(kind).host, cx);
    }

    pub(super) fn start_add(&mut self, cx: &mut Context<Self>) {
        self.set_kind(ProviderKind::Github, guide(ProviderKind::Github).host, cx);
        self.set_agent_placeholder(cx);
        self.adding = true;
        cx.notify();
    }

    fn start_edit(&mut self, account: &Account, cx: &mut Context<Self>) {
        self.adding = false;
        self.editing_id = Some(account.id.clone());
        let host = host_of(&account.web_url, account.kind);
        self.set_kind(account.kind, &host, cx);
        self.label
            .update(cx, |input, cx| input.set_text(account.label.clone(), cx));
        let username = if account.kind == ProviderKind::Bitbucket {
            account.username.clone()
        } else {
            String::new()
        };
        self.username
            .update(cx, |input, cx| input.set_text(username, cx));
        self.token.update(cx, |token, cx| token.set_text("", cx));
        self.agent_command.update(cx, |input, cx| {
            input.set_text(account.agent_command.clone().unwrap_or_default(), cx)
        });
        self.set_agent_placeholder(cx);
        cx.notify();
    }

    /// The per-account command's placeholder is the command in settings, or the default.
    fn set_agent_placeholder(&mut self, cx: &mut Context<Self>) {
        let placeholder = if self.agent_command_setting.is_empty() {
            DEFAULT_AGENT_COMMAND.to_string()
        } else {
            self.agent_command_setting.clone()
        };
        self.agent_command
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        let token_placeholder = if self.editing_id.is_some() {
            "Leave blank to keep the current token"
        } else {
            "••••••••••••••••"
        };
        self.token
            .update(cx, |token, cx| token.set_placeholder(token_placeholder, cx));
    }

    /// `save`: verifies the token against the host and stores the account. The error text is
    /// the one `AppState` produces, shown as a toast; the form stays open for another go.
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.can_save(cx) {
            return;
        }
        self.busy = true;
        cx.notify();

        let editing_id = self.editing_id.clone();
        let draft = AccountDraft {
            kind: self.kind,
            host: self.host.read(cx).text().to_string(),
            label: self.label.read(cx).text().trim().to_string(),
            token: self.token.read(cx).text().trim().to_string(),
            username: Some(self.username.read(cx).text().trim().to_string()),
            agent_command: Some(self.agent_command.read(cx).text().trim().to_string()),
        };
        let editing = editing_id.is_some();
        let task = self.state.update(cx, |state, cx| match &editing_id {
            Some(id) => state.update_account(id, draft, cx),
            None => state.add_account(draft, cx),
        });
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(account) => {
                        this.reload(cx);
                        say(
                            cx,
                            ToastKind::Ok,
                            if editing {
                                format!("Updated {}.", account.label)
                            } else {
                                format!("Signed in as {}.", account.display_name)
                            },
                        );
                        this.reset(cx);
                    }
                    Err(error) => say(cx, ToastKind::Bad, error.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// `remove`. The TSX asks for no confirmation: signing back in is one token away.
    fn remove(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        let removed = self
            .state
            .update(cx, |state, cx| state.remove_account(id, cx));
        match removed {
            Ok(_) => {
                self.reload(cx);
                say(cx, ToastKind::Info, format!("Removed {name}."));
            }
            Err(error) => say(cx, ToastKind::Bad, error.to_string()),
        }
        cx.notify();
    }

    fn close_handler(cx: &mut Context<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        let weak = cx.entity().downgrade();
        move |_, cx| {
            weak.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// The `text-[11px] text-muted-foreground` line under a field.
fn hint(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .mt(rpx(4.))
        .text_size(rpx(11.))
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
}

impl AccountsDialog {
    fn render_list(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;
        let mut list = div().flex().flex_col().gap(rpx(8.));
        for account in &self.accounts {
            let status = self
                .statuses
                .iter()
                .find(|entry| entry.account_id == account.id);
            let count = status.map_or(0, |status| status.count);
            let edit_account = account.clone();
            let remove_id = account.id.clone();
            let remove_name = account.label.clone();
            let mut avatar = Avatar::new(account.display_name.clone()).size(32.);
            if !account.avatar_url.is_empty() {
                avatar = avatar.src(account.avatar_url.clone());
            }
            let details = div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(6.))
                        .text_size(rpx(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(
                            div()
                                .opacity(0.7)
                                .child(Icon::provider(account.kind).size(12.)),
                        )
                        .child(div().truncate().child(account.label.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(rpx(11.5))
                        .text_color(colors.muted_foreground)
                        .child(format!(
                            "{} · {}",
                            account.username,
                            url_host(&account.web_url)
                        )),
                )
                .when_some(status.filter(|status| !status.ok), |d, status| {
                    d.child(
                        div()
                            .mt(rpx(4.))
                            .flex()
                            .items_start()
                            .gap(rpx(4.))
                            .text_size(rpx(11.5))
                            .text_color(colors.bad)
                            .child(
                                div().mt(px(1.)).flex_none().child(
                                    Icon::new(IconName::AlertTriangle)
                                        .size(12.)
                                        .color(colors.bad),
                                ),
                            )
                            .child(div().child(status.error.clone().unwrap_or_default())),
                    )
                });
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(12.))
                    .px(rpx(12.))
                    .py(rpx(10.))
                    .rounded(rpx(radius::LG))
                    .glass(cx)
                    .child(avatar)
                    .child(details)
                    .child(
                        div()
                            .flex_none()
                            .text_size(rpx(11.5))
                            .text_color(colors.muted_foreground)
                            .child(count.to_string()),
                    )
                    .child(probe(
                        format!("edit-{}", account.label),
                        Button::new(SharedString::from(format!("edit-{}", account.id)))
                            .variant(ButtonVariant::Ghost)
                            .icon_only(IconName::Pencil)
                            .tooltip(format!("Edit {}", account.label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_edit(&edit_account, cx)
                            })),
                    ))
                    .child(probe(
                        format!("remove-{}", account.label),
                        Button::new(SharedString::from(format!("remove-{}", account.id)))
                            .variant(ButtonVariant::Ghost)
                            .icon_only(IconName::Trash2)
                            .tooltip(format!("Remove {}", account.label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove(&remove_id, &remove_name, cx)
                            })),
                    )),
            );
        }
        if self.accounts.is_empty() {
            list = list.child(
                div()
                    .py(rpx(32.))
                    .flex()
                    .justify_center()
                    .text_size(rpx(13.))
                    .text_color(colors.muted_foreground)
                    .child("No accounts yet. Add one to start collecting review requests."),
            );
        }
        list
    }

    fn render_form(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.theme().colors;
        let kind = self.kind;
        let locked = self.editing_id.is_some();
        let guide = guide(kind);

        let mut options = div().grid().grid_cols(2).gap(rpx(6.));
        for option in ProviderKind::ALL {
            let active = kind == option;
            let tag = format!("provider-{}", option.as_str());
            options = options.child(probe(
                tag,
                div()
                    .id(SharedString::from(format!("provider-{}", option.as_str())))
                    .flex()
                    .items_center()
                    .gap(rpx(8.))
                    .px(rpx(12.))
                    .py(rpx(8.))
                    .rounded(rpx(radius::LG))
                    .border_1()
                    .text_size(rpx(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .when(locked, |d| d.opacity(0.7).cursor_not_allowed())
                    .when(!locked, |d| d.cursor_pointer())
                    .map(|d| {
                        if active {
                            d.border_color(colors.border_strong)
                                .bg(colors.surface_strong)
                        } else {
                            d.border_color(colors.border)
                                .text_color(colors.muted_foreground)
                                .when(!locked, |d| {
                                    d.hover(move |s| {
                                        s.bg(colors.accent).text_color(colors.foreground)
                                    })
                                })
                        }
                    })
                    .child(Icon::provider(option).size(16.).color(if active {
                        colors.foreground
                    } else {
                        colors.muted_foreground
                    }))
                    .child(option.label())
                    .when(!locked, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.pick_kind(option, cx)))
                    }),
            ));
        }

        let token_label = if kind == ProviderKind::Bitbucket {
            "App password"
        } else {
            "Personal access token"
        };
        let host_text = self.host.read(cx).text().to_string();
        let create_url = token_create_url(kind, &host_text);

        let token_help = div()
            .mt(rpx(4.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(4.))
            .text_size(rpx(11.))
            .text_color(colors.muted_foreground)
            .child("Needs")
            .child(
                div()
                    .font_family(mono_font())
                    .text_size(rpx(10.5))
                    .child(guide.scopes),
            )
            .child(".")
            .child(probe(
                "create-token",
                div()
                    .id("create-token")
                    .flex()
                    .items_center()
                    .gap(rpx(2.))
                    .cursor_pointer()
                    .text_color(colors.info)
                    .hover(|s| s.underline())
                    .child("Create one")
                    .child(
                        Icon::new(IconName::ExternalLink)
                            .size(10.)
                            .color(colors.info),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.read(cx).open_external(&create_url, cx);
                    })),
            ));

        div()
            .flex()
            .flex_col()
            .gap(rpx(14.))
            .child(div().child(label("Provider", cx)).child(options))
            .child(
                div()
                    .child(label("Host", cx))
                    .child(self.host.clone())
                    .child(hint(guide.host_hint, cx)),
            )
            .when(kind == ProviderKind::Bitbucket, |d| {
                d.child(
                    div()
                        .child(label("Bitbucket username", cx))
                        .child(self.username.clone())
                        .child(hint(
                            "App passwords authenticate as username + password, so both are needed.",
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .child(label(token_label, cx))
                    .child(self.token.clone())
                    .child(token_help),
            )
            .child(
                div()
                    .child(label("Name (optional)", cx))
                    .child(self.label.clone()),
            )
            .child(
                div()
                    .child(label("Claude command for this account (optional)", cx))
                    .child(self.agent_command.clone())
                    .child(hint(
                        "Routes handoffs from this account through a different Claude configuration. Leave blank to use the one in settings.",
                        cx,
                    )),
            )
    }
}

impl Render for AccountsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let form_open = self.form_open();
        let busy = self.busy;
        let editing = self.editing_id.is_some();
        let can_save = self.can_save(cx);

        let footer_actions = if form_open {
            div()
                .flex()
                .items_center()
                .gap(rpx(8.))
                .child(probe(
                    "accounts-cancel",
                    Button::new("accounts-cancel")
                        .variant(ButtonVariant::Ghost)
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.reset(cx)))
                        .child("Cancel"),
                ))
                .child(probe(
                    "accounts-save",
                    Button::new("accounts-save")
                        .variant(ButtonVariant::Default)
                        .loading(busy)
                        .disabled(busy || !can_save)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                        .child(save_label(busy, editing)),
                ))
        } else {
            div().child(probe(
                "accounts-add",
                Button::new("accounts-add")
                    .variant(ButtonVariant::Default)
                    .icon(IconName::Plus)
                    .on_click(cx.listener(|this, _, _, cx| this.start_add(cx)))
                    .child("Add account"),
            ))
        };

        let body = if form_open {
            self.render_form(cx)
        } else {
            self.render_list(cx)
        };

        Dialog::new("Accounts", self.focus.clone())
            .description(
                "Connect every host you review on. Tokens are encrypted with your macOS keychain.",
            )
            .width(576.)
            .on_close(Self::close_handler(cx))
            .footer(footer_actions)
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(rpx(13.))
                    .line_height(relative(1.5))
                    .text_color(colors.foreground)
                    .child(body),
            )
    }
}

/// The save button's label: connecting verifies the token against the host first.
fn save_label(busy: bool, editing: bool) -> &'static str {
    if busy {
        "Verifying…"
    } else if editing {
        "Save"
    } else {
        "Connect"
    }
}

// ---------------------------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------------------------

/// The host (with port) and the path of an http(s) URL, as `new URL(..)` reads them.
fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    if host.is_empty() {
        return None;
    }
    let tail = &rest[end..];
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    Some((host.to_ascii_lowercase(), tail[..path_end].to_string()))
}

/// `new URL(account.webUrl).host`, or the text itself when it is not a URL.
fn url_host(web_url: &str) -> String {
    split_url(web_url).map_or_else(|| web_url.to_string(), |(host, _)| host)
}

/// `hostOf`: what the host field shows when editing - the host, plus a sub-path for a
/// Forgejo or GitLab served from one. A URL that cannot be read falls back to the
/// provider's own host.
fn host_of(web_url: &str, kind: ProviderKind) -> String {
    match split_url(web_url) {
        Some((host, path)) => {
            let path = path.trim_end_matches('/');
            if path.is_empty() {
                host
            } else {
                format!("{host}{path}")
            }
        }
        None => guide(kind).host.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_keeps_a_sub_path_and_drops_the_slash() {
        assert_eq!(
            host_of("https://git.acme.com/forge/", ProviderKind::Forgejo),
            "git.acme.com/forge"
        );
        assert_eq!(
            host_of("https://github.com", ProviderKind::Github),
            "github.com"
        );
        assert_eq!(
            host_of("https://gitlab.acme.com/", ProviderKind::Gitlab),
            "gitlab.acme.com"
        );
    }

    #[test]
    fn host_of_falls_back_to_the_provider_host() {
        assert_eq!(host_of("nonsense", ProviderKind::Gitlab), "gitlab.com");
    }

    #[test]
    fn url_host_keeps_the_port_and_drops_userinfo() {
        assert_eq!(
            url_host("https://user@Git.Acme.com:8443/x?y#z"),
            "git.acme.com:8443"
        );
        assert_eq!(url_host("not a url"), "not a url");
    }
}

#[cfg(test)]
#[path = "accounts_dialog_tests.rs"]
mod dialog_tests;
