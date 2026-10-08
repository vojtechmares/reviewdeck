//! Port of src/renderer/src/components/PullView.tsx: one open pull request - its header,
//! the Files / Checks / Conversation tabs, the drafts banner and the review composer.
//!
//! The React component kept its state in hooks and talked to the main process through
//! `window.reviewdeck.*`. Here the view is an entity that reads the deck, the drafts and
//! the settings from [`AppState`] (observing it) and calls the same operations on it.
//!
//! Differences from the React version that follow from that, not from a choice:
//! - The line-comment, reply and resolve callbacks are not threaded through the diff.
//!   `DiffView` adds drafts through `AppState::add_draft` itself and each `ThreadCard`
//!   replies and resolves through `AppState`; this view only hears that a conversation
//!   changed ([`ThreadEvent::Changed`], [`DiffEvent::ThreadsChanged`]) and reads the
//!   threads again.
//! - The error state also has a "Try again" button, which the TSX lacks; it reruns the
//!   load, so a failed load no longer needs the pull request to be reselected.
//! - The composer is one input that grows from one to three rows with its content; the
//!   TSX showed three rows whenever a verdict or drafts were present. `TextInput` fixes
//!   its row range at construction.
//! - The drafts are read from `AppState` rather than remembered, so there is no
//!   `setDrafts(await ...)` after each operation.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, AppContext, Context, Corner, Entity, FocusHandle, FontWeight, HighlightStyle,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription, Task, Window,
    anchored, div, point, prelude::FluentBuilder, px,
};
use reviewdeck_core::agent_prompt::agent_command;
use reviewdeck_core::autolink::repository_root_of;
use reviewdeck_core::drafts::head_moved;
use reviewdeck_core::images::{ImageAccount, ImageContext};
use reviewdeck_core::markdown::{AutolinkContext, MarkdownContext};
use reviewdeck_core::model::{
    CommentThread, DiffFile, DiffRefs, DiffViewMode, DraftComment, PullDetail, ReviewItem,
    ReviewSubmission, ReviewVerdict, Settings,
};
use reviewdeck_core::threads::same_conversation;
use reviewdeck_core::time::{now_ms, relative_time};

use crate::state::{AppState, GlobalState};
use crate::ui::app_view::toast as show_toast;
use crate::ui::approval_badge::approval_badge;
use crate::ui::check_pill::check_pill;
use crate::ui::checks_panel::checks_panel;
use crate::ui::components::avatar::Avatar;
use crate::ui::components::badge::{Badge, BadgeTone};
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant, with_alpha};
use crate::ui::components::dialog::Dialog;
use crate::ui::components::glass::GlassExt;
use crate::ui::components::input::TextInput;
use crate::ui::components::input::TextInputEvent;
use crate::ui::components::scroll::ScrollArea;
use crate::ui::components::spinner::Spinner;
use crate::ui::components::toast::ToastKind;
use crate::ui::diff_view::{DiffEvent, DiffView};
use crate::ui::icons::{Icon, IconName};
use crate::ui::markdown_view::{ImageLoader, MarkdownView};
use crate::ui::theme::{ActiveTheme, mono_font, radius, rpx};
use crate::ui::thread_view::{ThreadCard, ThreadEvent};

/// Shows a toast. Under test it is also recorded, because the toast stack the app view
/// owns does not exist there and has no way to be read back.
fn toast(cx: &mut App, kind: ToastKind, message: impl Into<SharedString>) {
    let message = message.into();
    #[cfg(test)]
    tests::TOASTS.with(|toasts| toasts.borrow_mut().push((kind, message.to_string())));
    show_toast(cx, kind, message);
}

/// Gives a control a name the interaction tests can find its bounds by. A no-op wrapper
/// outside tests: gpui's `debug_selector` does nothing in a release build.
fn hit(name: &'static str, control: impl IntoElement) -> gpui::Div {
    div().debug_selector(|| name.to_string()).child(control)
}

/// `type Tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Diff,
    Checks,
    Conversation,
}

/// A conversation thread's card, kept between renders so a reply being typed survives
/// the conversation being re-read.
struct CardEntry {
    id: String,
    card: Entity<ThreadCard>,
    _subscription: Subscription,
}

/// The pull request on screen.
pub struct PullView {
    item_id: String,
    state: Entity<AppState>,
    /// The deck's copy of the item, refreshed whenever the deck changes (its checks and
    /// approvals move while the pull request is open). Kept when the item leaves the deck.
    item: Option<ReviewItem>,
    settings: Settings,

    /// Whether `PullDetail` has arrived. The pieces of it live in the fields below.
    loaded: bool,
    description: String,
    files: Arc<Vec<DiffFile>>,
    refs: DiffRefs,
    threads: Vec<CommentThread>,
    /// Threads the diff can show, and everything else; see [`split_threads`].
    inline: Arc<Vec<CommentThread>>,
    conversation: Vec<CommentThread>,

    drafts: Vec<DraftComment>,
    loading: bool,
    error: Option<SharedString>,
    tab: Tab,
    verdict: Option<ReviewVerdict>,
    body: Entity<TextInput>,
    /// The placeholder the composer currently shows, so it is only replaced on a change.
    placeholder: SharedString,
    /// The composer's height in rows, kept so it is only changed on a change.
    rows: usize,
    submitting: bool,
    /// The freshly loaded diff, held while the reviewer is told the author pushed.
    pushed: Option<PullDetail>,
    /// A review for this pull request went out somewhere else while drafts were pending.
    diverged: bool,
    confirm_discard: bool,

    /// The sync the conversation on screen was read at. A fresh load already answers for
    /// the sync that was current when the pull request was opened, so only a later one
    /// is worth another request.
    conversation_at: String,

    diff_view: Option<Entity<DiffView>>,
    _diff_subscription: Option<Subscription>,
    description_view: Option<Entity<MarkdownView>>,
    cards: Vec<CardEntry>,
    scroll: ScrollHandle,
    push_focus: FocusHandle,
    discard_focus: FocusHandle,

    load_task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    reload_task: Option<Task<()>>,
    check_task: Option<Task<()>>,
    send_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Threads the diff can actually show, and everything else.
///
/// A thread needs a file, a line, and a file the diff still carries. An outdated thread
/// has no line by design, and a thread on a file this diff does not touch has nowhere to
/// sit - so both belong in the conversation rather than silently disappearing between
/// the two views.
fn split_threads(
    threads: &[CommentThread],
    files: &[DiffFile],
) -> (Vec<CommentThread>, Vec<CommentThread>) {
    let paths: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    threads.iter().cloned().partition(|thread| {
        thread
            .path
            .as_deref()
            .is_some_and(|path| !path.is_empty() && paths.contains(path))
            && thread.line.is_some()
    })
}

fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// The badge over the composer: "3 pending comments". The count comes before "pending",
/// as in the TSX, which is not where [`plural`] would put it.
fn pending_label(count: usize) -> String {
    format!(
        "{count} pending comment{}",
        if count == 1 { "" } else { "s" }
    )
}

/// The composer's height: one row at rest, three once a review is taking shape - a
/// verdict picked, words typed or drafts waiting (`rows={verdict || body ||
/// drafts.length ? 3 : 1}`).
fn composer_rows(verdict: Option<ReviewVerdict>, has_drafts: bool, typed: bool) -> usize {
    if verdict.is_some() || has_drafts || typed {
        3
    } else {
        1
    }
}

/// The composer's placeholder for the verdict and drafts it holds.
fn placeholder_for(verdict: Option<ReviewVerdict>, has_drafts: bool) -> &'static str {
    match verdict {
        Some(ReviewVerdict::Approve) => "Optional note with your approval…",
        Some(ReviewVerdict::RequestChanges) => "What needs to change?",
        _ if has_drafts => "Optional summary for your review…",
        _ => "Leave a comment on this pull request…",
    }
}

/// The send button's label for the verdict and drafts it will send.
fn submit_label(verdict: Option<ReviewVerdict>, has_drafts: bool) -> &'static str {
    match verdict {
        Some(ReviewVerdict::Approve) => "Approve",
        Some(ReviewVerdict::RequestChanges) => "Request changes",
        _ if has_drafts => "Submit review",
        _ => "Comment",
    }
}

impl PullView {
    /// Opens the pull request `item_id` of the deck and starts loading its detail. The app
    /// view creates a new one whenever the selection changes, which is what the React
    /// effect keyed on `item.id` did by resetting every piece of state.
    pub fn new(item_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        let (item, settings, drafts, diverged, synced) = {
            let app = state.read(cx);
            (
                app.find(&item_id),
                app.settings(),
                app.drafts(&item_id),
                app.drafts_diverged(&item_id),
                app.deck().last_synced_at.unwrap_or_default(),
            )
        };

        let placeholder: SharedString = placeholder_for(None, !drafts.is_empty()).into();
        let rows = composer_rows(None, !drafts.is_empty(), false);
        let body = {
            let placeholder = placeholder.clone();
            cx.new(|cx| {
                TextInput::new(cx)
                    .multi_line(rows, rows)
                    .placeholder(placeholder)
            })
        };

        let subscriptions = vec![
            // The deck moves under an open pull request: checks finish, approvals land,
            // a sync completes.
            cx.observe_in(&state, window, |this, _, window, cx| {
                this.on_state_changed(window, cx);
            }),
            cx.subscribe_in(
                &body,
                window,
                |this, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed => cx.notify(),
                    // The composer's ⌘↵.
                    TextInputEvent::Submit => {
                        if this.can_submit(cx) {
                            this.submit(window, cx);
                        }
                    }
                    TextInputEvent::Cancel => {}
                },
            ),
        ];

        let mut view = PullView {
            item_id,
            state,
            item,
            settings,
            loaded: false,
            description: String::new(),
            files: Arc::new(Vec::new()),
            refs: DiffRefs::default(),
            threads: Vec::new(),
            inline: Arc::new(Vec::new()),
            conversation: Vec::new(),
            drafts,
            loading: true,
            error: None,
            tab: Tab::Diff,
            verdict: None,
            body,
            placeholder,
            rows,
            submitting: false,
            pushed: None,
            diverged,
            confirm_discard: false,
            // What this load answers for, so the refresh only follows a later sync.
            conversation_at: synced,
            diff_view: None,
            _diff_subscription: None,
            description_view: None,
            cards: Vec::new(),
            scroll: ScrollHandle::new(),
            push_focus: cx.focus_handle(),
            discard_focus: cx.focus_handle(),
            load_task: None,
            poll_task: None,
            reload_task: None,
            check_task: None,
            send_task: None,
            _subscriptions: subscriptions,
        };
        view.start_load(window, cx);
        view
    }

    // ----- loading ----------------------------------------------------------------

    /// Reloads whenever the selection changes; a stale diff would be worse than a spinner.
    /// Also what the error state's retry runs.
    fn start_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        self.loaded = false;
        let task = self
            .state
            .update(cx, |state, cx| state.load_detail(&self.item_id, cx));
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok(detail) => this.set_detail(detail, window, cx),
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// Takes a freshly loaded pull request: the diff, the description and the
    /// conversation are all replaced, and the child views follow.
    fn set_detail(&mut self, detail: PullDetail, window: &mut Window, cx: &mut Context<Self>) {
        let same_files = self.loaded && *self.files == detail.files && self.refs == detail.refs;
        self.loaded = true;
        self.description = detail.description;
        self.refs = detail.refs;
        if !same_files {
            self.files = Arc::new(detail.files);
        }
        self.threads = detail.threads;
        let (inline, conversation) = split_threads(&self.threads, &self.files);
        self.inline = Arc::new(inline);
        self.conversation = conversation;

        match (&self.diff_view, same_files) {
            (Some(view), true) => {
                let inline = self.inline.clone();
                view.update(cx, |view, cx| view.set_threads(inline, cx));
            }
            _ => {
                let view = cx.new(|cx| {
                    DiffView::new(
                        self.item_id.clone(),
                        self.files.clone(),
                        self.inline.clone(),
                        self.refs.clone(),
                        window,
                        cx,
                    )
                });
                self._diff_subscription = Some(cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &DiffEvent, window, cx| match event {
                        DiffEvent::ThreadsChanged => this.reload_threads(window, cx),
                    },
                ));
                self.diff_view = Some(view);
            }
        }
        self.sync_description(cx);
        self.sync_cards(window, cx);
        cx.notify();
    }

    /// Where `@someone`, `#123` and a relative image source in this pull request's prose
    /// should point. Every signed-in account is offered, not just this item's, because a
    /// description can embed an image from another host we are signed in to.
    fn markdown_context(&self, cx: &App) -> MarkdownContext {
        let Some(item) = &self.item else {
            return MarkdownContext::default();
        };
        let accounts = self.state.read(cx).accounts();
        let web_url = accounts
            .iter()
            .find(|account| account.id == item.account_id)
            .map(|account| account.web_url.clone())
            .unwrap_or_default();
        let repo_root = repository_root_of(item);
        MarkdownContext {
            autolink: Some(AutolinkContext {
                provider: item.provider,
                web_url,
                repo_root: repo_root.clone(),
            }),
            images: Some(ImageContext {
                repo_root,
                accounts: accounts.iter().map(ImageAccount::from).collect(),
            }),
        }
    }

    /// An image on an account's own host needs that account's credential, which only
    /// `AppState` may hold; every other image loads through gpui.
    fn image_loader(&self) -> ImageLoader {
        let state = self.state.clone();
        Rc::new(move |account_id: &str, url: &str, cx: &mut App| {
            state.update(cx, |state, cx| {
                state.authenticated_image(account_id, url, cx)
            })
        })
    }

    /// Makes the description's markdown view match the description on hand.
    fn sync_description(&mut self, cx: &mut Context<Self>) {
        if self.description.trim().is_empty() {
            self.description_view = None;
            return;
        }
        let context = self.markdown_context(cx);
        let source: SharedString = self.description.clone().into();
        match &self.description_view {
            Some(view) => view.update(cx, |view, cx| view.set_source(source, context, cx)),
            None => {
                let loader = self.image_loader();
                self.description_view =
                    Some(cx.new(|cx| MarkdownView::new(source, context, Some(loader), false, cx)));
            }
        }
    }

    /// Makes the conversation's thread cards match the conversation on hand, keeping the
    /// card of every thread that is still there.
    fn sync_cards(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut previous = std::mem::take(&mut self.cards);
        for thread in self.conversation.clone() {
            if let Some(position) = previous.iter().position(|entry| entry.id == thread.id) {
                let entry = previous.swap_remove(position);
                entry
                    .card
                    .update(cx, |card, cx| card.set_thread(thread, cx));
                self.cards.push(entry);
            } else {
                let id = thread.id.clone();
                let item_id = self.item_id.clone();
                let card = cx.new(|cx| ThreadCard::new(item_id, thread, false, window, cx));
                let subscription =
                    cx.subscribe_in(&card, window, |this, _, event: &ThreadEvent, window, cx| {
                        match event {
                            ThreadEvent::Changed => this.reload_threads(window, cx),
                        }
                    });
                self.cards.push(CardEntry {
                    id,
                    card,
                    _subscription: subscription,
                });
            }
        }
    }

    /// Replaces the conversation with a newer read of it, if it differs.
    ///
    /// A read that found nothing new leaves the view untouched, or the diff rebuilds
    /// under whoever is reading it. Only the conversation moves: the diff a reviewer is
    /// halfway through stays exactly as they found it, which is what the push dialog
    /// exists to protect.
    fn apply_threads(
        &mut self,
        threads: Vec<CommentThread>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.loaded || same_conversation(&self.threads, &threads) {
            return;
        }
        self.threads = threads;
        let (inline, conversation) = split_threads(&self.threads, &self.files);
        if *self.inline != inline {
            self.inline = Arc::new(inline);
            if let Some(view) = &self.diff_view {
                let inline = self.inline.clone();
                view.update(cx, |view, cx| view.set_threads(inline, cx));
            }
        }
        self.conversation = conversation;
        self.sync_cards(window, cx);
        cx.notify();
    }

    /// Reads the conversation again after a reply or a resolve. The React version
    /// reloaded the whole detail; only the threads can have changed.
    fn reload_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = self
            .state
            .update(cx, |state, cx| state.load_threads(&self.item_id, cx));
        self.reload_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(threads) => this.apply_threads(threads, window, cx),
                Err(error) => toast(cx, ToastKind::Bad, error.to_string()),
            })
            .ok();
        }));
    }

    /// The deck changed. Refreshes what is read from it, and re-reads the conversation on
    /// the back of a sync.
    ///
    /// A conversation moves while the diff is being read: someone answers a thread, or
    /// resolves one. This is a menu bar app left running for days, so a pull request
    /// opened yesterday would otherwise still be showing yesterday's replies - which
    /// reads as the reply never arriving rather than as a stale view. The deck's sync is
    /// when the app learns anything at all.
    fn on_state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (item, settings, synced) = {
            let app = self.state.read(cx);
            self.drafts = app.drafts(&self.item_id);
            // Re-read on every change, because that is when a review submitted in a
            // browser becomes visible: the deck's state changes, and the drafts left
            // behind here are suddenly a second half-review nobody asked for.
            self.diverged = app.drafts_diverged(&self.item_id);
            (
                app.find(&self.item_id),
                app.settings(),
                app.deck().last_synced_at.unwrap_or_default(),
            )
        };
        if item.is_some() {
            self.item = item;
        }
        self.settings = settings;
        cx.notify();

        if synced.is_empty() || synced == self.conversation_at {
            return;
        }
        self.conversation_at = synced;
        let task = self
            .state
            .update(cx, |state, cx| state.load_threads(&self.item_id, cx));
        // A newer sync replaces the read still in flight.
        self.poll_task = Some(cx.spawn_in(window, async move |this, cx| {
            // Nothing is said about a refresh that failed: what is on screen is still
            // true, and the next sync will try again.
            if let Ok(threads) = task.await {
                this.update_in(cx, |this, window, cx| {
                    this.apply_threads(threads, window, cx)
                })
                .ok();
            }
        }));
    }

    // ----- actions ----------------------------------------------------------------

    fn open_external(&self, url: &str, cx: &App) {
        self.state.read(cx).open_external(url, cx);
    }

    /// Nothing is spawned: the command goes on the clipboard for the user to run in the
    /// terminal they already have open in that repository.
    fn copy_agent_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.item.clone() else {
            return;
        };
        {
            let app = self.state.read(cx);
            let account = app.account(&item.account_id);
            let command = agent_command(&item, &self.threads, account.as_ref(), &app.settings());
            app.copy_text(&command, cx);
        }
        toast(
            cx,
            ToastKind::Ok,
            "Claude prompt copied - paste it in your terminal.",
        );
    }

    fn set_diff_view(&mut self, mode: DiffViewMode, cx: &mut Context<Self>) {
        let result = self.state.update(cx, |state, cx| {
            state.set_settings(|s| s.diff_view = mode, cx)
        });
        match result {
            Ok(next) => self.settings = next,
            Err(error) => toast(cx, ToastKind::Bad, error.to_string()),
        }
        cx.notify();
    }

    fn can_submit(&self, cx: &App) -> bool {
        (self.verdict == Some(ReviewVerdict::Approve)
            || !self.drafts.is_empty()
            || !self.body.read(cx).text().trim().is_empty())
            && !self.submitting
    }

    /// On an active pull request the author pushing mid-review is ordinary, so the
    /// reviewer is told before their remarks go out rather than after.
    ///
    /// The check needs the head as it is now, not as it was when the diff was loaded, and
    /// the reload doubles as the updated diff to offer - so it costs one request and only
    /// when there are drafts to be stale.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        if self.drafts.is_empty() {
            self.send(window, cx);
            return;
        }
        let task = self
            .state
            .update(cx, |state, cx| state.load_detail(&self.item_id, cx));
        self.check_task = Some(cx.spawn_in(window, async move |this, cx| {
            let fresh = task.await.ok();
            this.update_in(cx, |this, window, cx| {
                if let Some(fresh) = fresh
                    && head_moved(&this.drafts, Some(&fresh.refs))
                {
                    this.pushed = Some(fresh);
                    window.focus(&this.push_focus);
                    cx.notify();
                    return;
                }
                this.send(window, cx);
            })
            .ok();
        }));
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        self.submitting = true;
        cx.notify();
        let verdict = self.verdict;
        let has_drafts = !self.drafts.is_empty();
        let body = self.body.read(cx).text().trim().to_string();
        let item_id = self.item_id.clone();
        let task = self.state.update(cx, |state, cx| {
            if verdict.is_some() || has_drafts {
                // No verdict with drafts still goes through the review call, so the whole
                // set arrives together as plain comments rather than one at a time.
                state.submit_review(
                    ReviewSubmission {
                        item_id,
                        verdict: verdict.unwrap_or(ReviewVerdict::Comment),
                        body,
                    },
                    cx,
                )
            } else {
                state.add_comment(&item_id, &body, cx)
            }
        });
        let submitted_review = verdict.is_some() || has_drafts;
        self.send_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.finish_send(result, verdict, submitted_review, window, cx)
            })
            .ok();
        }));
    }

    fn finish_send(
        &mut self,
        result: reviewdeck_core::Result<()>,
        verdict: Option<ReviewVerdict>,
        submitted_review: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Whatever happened, the drafts are the store's: a partial submission removed the
        // ones that landed, and a failure left them alone.
        self.drafts = self.state.read(cx).drafts(&self.item_id);
        if let Err(error) = result {
            toast(cx, ToastKind::Bad, error.to_string());
            self.submitting = false;
            cx.notify();
            return;
        }
        toast(
            cx,
            ToastKind::Ok,
            if !submitted_review {
                "Comment posted."
            } else {
                match verdict {
                    Some(ReviewVerdict::Approve) => "Approved.",
                    Some(ReviewVerdict::RequestChanges) => "Changes requested.",
                    _ => "Review submitted.",
                }
            },
        );
        self.body.update(cx, |body, cx| body.set_text("", cx));
        self.verdict = None;
        cx.notify();

        let task = self
            .state
            .update(cx, |state, cx| state.load_detail(&self.item_id, cx));
        self.send_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(detail) => this.set_detail(detail, window, cx),
                    Err(error) => toast(cx, ToastKind::Bad, error.to_string()),
                }
                this.submitting = false;
                this.state
                    .update(cx, |state, cx| state.refresh(cx).detach());
                cx.notify();
            })
            .ok();
        }));
    }

    // ----- pieces -----------------------------------------------------------------

    fn tab_button(
        &self,
        id: &'static str,
        tab: Tab,
        icon: IconName,
        label: &'static str,
        count: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.theme().colors;
        let active = self.tab == tab;
        let foreground = if active {
            colors.foreground
        } else {
            colors.muted_foreground
        };
        div()
            .id(id)
            .debug_selector(|| id.to_string())
            .flex()
            .flex_none()
            .items_center()
            .gap(rpx(6.))
            .px(rpx(10.))
            .pt(rpx(6.))
            .pb(rpx(8.))
            .rounded_t(rpx(radius::MD))
            .border_b_2()
            .border_color(if active {
                colors.foreground
            } else {
                gpui::transparent_black()
            })
            .text_size(rpx(12.5))
            .line_height(rpx(18.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(foreground)
            .cursor_pointer()
            .when(!active, |tab| {
                tab.hover(move |style| style.text_color(colors.foreground))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
            .child(Icon::new(icon).size(14.).color(foreground))
            .child(label)
            .when_some(count, |button, count| {
                button.child(
                    div()
                        .rounded(rpx(4.))
                        .bg(colors.muted)
                        .px(rpx(4.))
                        .py(rpx(1.))
                        .text_size(rpx(10.5))
                        .line_height(rpx(15.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.muted_foreground)
                        .child(count.to_string()),
                )
            })
    }

    fn header(&self, item: &ReviewItem, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors;
        let url = item.url.clone();

        let meta = div()
            .flex()
            .items_center()
            .gap(rpx(6.))
            .text_size(rpx(11.5))
            .line_height(rpx(16.))
            .text_color(colors.muted_foreground)
            .child(
                Icon::provider(item.provider)
                    .size(12.)
                    .color(with_alpha(colors.muted_foreground, 0.7)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(item.repo.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(with_alpha(colors.muted_foreground, 0.6))
                    .child(format!("#{}", item.number)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(with_alpha(colors.muted_foreground, 0.5))
                    .child("·"),
            )
            .child(div().flex_none().child(format!(
                "{} opened {}",
                item.author.name,
                relative_time(&item.created_at, now_ms())
            )));

        let badges = div()
            .mt(rpx(8.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(6.))
            .child(check_pill(&item.checks, cx))
            .child(approval_badge(&item.approvals, "pull-approvals", cx))
            .child(
                Badge::new()
                    .child(
                        Icon::new(IconName::GitBranch)
                            .size(12.)
                            .color(colors.muted_foreground),
                    )
                    .child(
                        div()
                            .font_family(mono_font())
                            .text_size(rpx(10.5))
                            .child(format!("{} → {}", item.source_branch, item.target_branch)),
                    ),
            )
            .when(item.draft, |row| {
                row.child(Badge::new().tone(BadgeTone::Info).child("Draft"))
            })
            .children(
                item.labels
                    .iter()
                    .take(4)
                    .map(|label| Badge::new().child(label.clone())),
            );

        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(rpx(4.))
            .child(hit(
                "copy-agent-prompt",
                Button::new("copy-agent-prompt")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::ClipboardCopy)
                    .tooltip("Copy a prompt for your terminal")
                    .on_click(cx.listener(|this, _, _, cx| this.copy_agent_prompt(cx)))
                    .child("Copy Claude prompt"),
            ))
            .child(hit(
                "open-in-browser",
                Button::new("open-in-browser")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(IconName::ExternalLink)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_external(&url, cx)))
                    .child("Open"),
            ));

        let mut avatar = Avatar::new(item.author.name.clone()).size(28.);
        if !item.author.avatar_url.is_empty() {
            avatar = avatar.src(item.author.avatar_url.clone());
        }

        let split = self.settings.diff_view == DiffViewMode::Split;
        let mode_toggle = (self.tab == Tab::Diff).then(|| {
            div()
                .mb(rpx(4.))
                .ml_auto()
                .flex()
                .items_center()
                .gap(rpx(2.))
                .child(hit(
                    "diff-split",
                    Button::new("diff-split")
                        .variant(if split {
                            ButtonVariant::Subtle
                        } else {
                            ButtonVariant::Ghost
                        })
                        .icon_only(IconName::Columns2)
                        .tooltip("Side by side")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_diff_view(DiffViewMode::Split, cx)
                        })),
                ))
                .child(hit(
                    "diff-unified",
                    Button::new("diff-unified")
                        .variant(if split {
                            ButtonVariant::Ghost
                        } else {
                            ButtonVariant::Subtle
                        })
                        .icon_only(IconName::Rows3)
                        .tooltip("Unified")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_diff_view(DiffViewMode::Unified, cx)
                        })),
                ))
        });

        let tabs = div()
            .mt(rpx(12.))
            .flex()
            .items_center()
            .gap(rpx(4.))
            .child(self.tab_button(
                "tab-diff",
                Tab::Diff,
                IconName::FileDiff,
                "Files",
                self.loaded.then_some(self.files.len()),
                cx,
            ))
            .child(self.tab_button(
                "tab-checks",
                Tab::Checks,
                IconName::Check,
                "Checks",
                (item.checks.total > 0).then_some(item.checks.total as usize),
                cx,
            ))
            .child(self.tab_button(
                "tab-conversation",
                Tab::Conversation,
                IconName::MessageSquare,
                "Conversation",
                (!self.conversation.is_empty()).then_some(self.conversation.len()),
                cx,
            ))
            .children(mode_toggle);

        div()
            .flex_none()
            .bg(colors.surface_muted)
            .border_b_1()
            .border_color(colors.border)
            .px(rpx(20.))
            .pt(rpx(12.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rpx(12.))
                    .child(div().mt(rpx(2.)).flex_none().child(avatar))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(meta)
                            .child(
                                div()
                                    .mt(rpx(2.))
                                    .text_size(rpx(15.))
                                    .line_height(rpx(21.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(item.title.clone()),
                            )
                            .child(badges),
                    )
                    .child(actions),
            )
            .child(tabs)
    }

    /// The drafts that survived a review submitted somewhere else.
    fn divergence_banner(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors;
        let count = self.drafts.len();
        let lead = "A review went out elsewhere.";
        let text = format!(
            "{lead} Your review of this pull request changed without Reviewdeck sending it, \
             and {count} comment{} still drafted here. Submitting now would say {} a second time.",
            if count == 1 { " is" } else { "s are" },
            if count == 1 { "it" } else { "them" },
        );
        let styled = StyledText::new(text).with_highlights(vec![(
            0..lead.len(),
            HighlightStyle {
                color: Some(colors.busy),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        )]);
        let item_id = self.item_id.clone();
        div()
            .flex_none()
            .bg(colors.busy_soft)
            .border_b_1()
            .border_color(with_alpha(colors.busy, 0.3))
            .px(rpx(20.))
            .py(rpx(10.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(rpx(12.))
                    .gap_y(rpx(6.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(rpx(12.5))
                            .line_height(rpx(19.))
                            .child(styled),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(rpx(6.))
                            .child(hit(
                                "keep-drafts",
                                Button::new("keep-drafts")
                                    .size(ButtonSize::Sm)
                                    .variant(ButtonVariant::Secondary)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let item_id = item_id.clone();
                                        this.diverged = this.state.update(cx, |state, cx| {
                                            state.acknowledge_drafts(&item_id, cx)
                                        });
                                        cx.notify();
                                    }))
                                    .child("Keep drafts"),
                            ))
                            .child(hit(
                                "discard-all",
                                Button::new("discard-all")
                                    .size(ButtonSize::Sm)
                                    .variant(ButtonVariant::Ghost)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_discard = true;
                                        window.focus(&this.discard_focus);
                                        cx.notify();
                                    }))
                                    .child("Discard all"),
                            )),
                    ),
            )
    }

    fn centered(&self) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
    }

    /// What fills the space between the header and the composer.
    fn content(&self, item: &ReviewItem, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors;
        if self.loading {
            return self
                .centered()
                .flex_row()
                .gap(rpx(8.))
                .child(Spinner::new().size(16.).color(colors.muted_foreground))
                .child(
                    div()
                        .text_size(rpx(13.))
                        .text_color(colors.muted_foreground)
                        .child("Loading the diff…"),
                )
                .into_any_element();
        }
        if let Some(error) = &self.error {
            let url = item.url.clone();
            return self
                .centered()
                .gap(rpx(12.))
                .px(rpx(32.))
                .child(
                    div()
                        .max_w(rpx(448.))
                        .text_center()
                        .text_size(rpx(13.))
                        .line_height(rpx(20.))
                        .text_color(colors.bad)
                        .child(error.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(8.))
                        .child(hit(
                            "retry-load",
                            Button::new("retry-load")
                                .size(ButtonSize::Sm)
                                .variant(ButtonVariant::Ghost)
                                .icon(IconName::RotateCcw)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start_load(window, cx)),
                                )
                                .child("Try again"),
                        ))
                        .child(hit(
                            "open-instead",
                            Button::new("open-instead")
                                .size(ButtonSize::Sm)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.open_external(&url, cx)),
                                )
                                .child("Open in browser instead"),
                        )),
                )
                .into_any_element();
        }
        if !self.loaded {
            return div().into_any_element();
        }

        match self.tab {
            Tab::Diff => match &self.diff_view {
                Some(view) => div().size_full().child(view.clone()).into_any_element(),
                None => div().into_any_element(),
            },
            Tab::Checks => ScrollArea::new("pull-scroll", &self.scroll)
                .child(checks_panel(&item.checks, cx))
                .into_any_element(),
            Tab::Conversation => ScrollArea::new("pull-scroll", &self.scroll)
                .child(self.conversation_tab(item, cx))
                .into_any_element(),
        }
    }

    fn conversation_tab(&self, item: &ReviewItem, cx: &App) -> impl IntoElement + use<> {
        let colors = cx.theme().colors;
        let has_description = !self.description.trim().is_empty();
        let inline_count = self.inline.len();
        div()
            .flex()
            .flex_col()
            .gap(rpx(12.))
            .p(rpx(16.))
            .when_some(
                self.description_view.clone().filter(|_| has_description),
                |column, markdown| {
                    column.child(
                        div()
                            .rounded(rpx(radius::LG))
                            .glass(cx)
                            .p(rpx(14.))
                            .child(
                                div()
                                    .mb(rpx(6.))
                                    .text_size(rpx(11.5))
                                    .line_height(rpx(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.muted_foreground)
                                    .child(format!("{} wrote", item.author.name)),
                            )
                            .child(markdown),
                    )
                },
            )
            .children(self.cards.iter().map(|entry| entry.card.clone()))
            .when(self.conversation.is_empty() && !has_description, |column| {
                column.child(
                    div()
                        .py(rpx(40.))
                        .text_center()
                        .text_size(rpx(13.))
                        .text_color(colors.muted_foreground)
                        .child("No conversation yet."),
                )
            })
            .when(inline_count > 0, |column| {
                column.child(
                    div()
                        .text_center()
                        .text_size(rpx(11.5))
                        .text_color(colors.muted_foreground)
                        .child(format!(
                            "{inline_count} inline thread{} shown on the Files tab.",
                            if inline_count == 1 { "" } else { "s" }
                        )),
                )
            })
    }

    fn verdict_button(
        &self,
        id: &'static str,
        target: ReviewVerdict,
        variant: ButtonVariant,
        icon: IconName,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let active = self.verdict == Some(target);
        hit(
            id,
            Button::new(id)
                .size(ButtonSize::Sm)
                .variant(if active {
                    variant
                } else {
                    ButtonVariant::Ghost
                })
                .icon(icon)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.verdict = if this.verdict == Some(target) {
                        None
                    } else {
                        Some(target)
                    };
                    cx.notify();
                }))
                .child(label),
        )
    }

    fn composer(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors;
        let has_drafts = !self.drafts.is_empty();

        let placeholder: SharedString = placeholder_for(self.verdict, has_drafts).into();
        if placeholder != self.placeholder {
            self.placeholder = placeholder.clone();
            self.body
                .update(cx, |body, cx| body.set_placeholder(placeholder, cx));
        }
        let typed = !self.body.read(cx).text().is_empty();
        let rows = composer_rows(self.verdict, has_drafts, typed);
        if rows != self.rows {
            self.rows = rows;
            self.body
                .update(cx, |body, cx| body.set_rows(rows, rows, cx));
        }
        let can_submit = self.can_submit(cx);
        let label = submit_label(self.verdict, has_drafts);

        div()
            .flex_none()
            .bg(colors.surface_muted)
            .border_t_1()
            .border_color(colors.border)
            .p(rpx(12.))
            // The TSX took Ctrl+Enter as well as Cmd+Enter. The input binds Cmd+Enter
            // (as `TextInputEvent::Submit`); Ctrl+Enter is unbound there, so it
            // bubbles up to here as a plain key press.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                if keystroke.key == "enter"
                    && keystroke.modifiers.control
                    && !keystroke.modifiers.platform
                    && this.can_submit(cx)
                {
                    this.submit(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .w_full()
                    .text_size(rpx(13.))
                    .line_height(rpx(21.))
                    .text_color(colors.foreground)
                    .child(self.body.clone()),
            )
            .child(
                div()
                    .mt(rpx(8.))
                    .flex()
                    .items_center()
                    .gap(rpx(6.))
                    .when(has_drafts, |row| {
                        row.child(
                            Badge::new()
                                .tone(BadgeTone::Info)
                                .child(pending_label(self.drafts.len())),
                        )
                    })
                    .child(self.verdict_button(
                        "verdict-approve",
                        ReviewVerdict::Approve,
                        ButtonVariant::Success,
                        IconName::Check,
                        "Approve",
                        cx,
                    ))
                    .child(self.verdict_button(
                        "verdict-request-changes",
                        ReviewVerdict::RequestChanges,
                        ButtonVariant::Danger,
                        IconName::X,
                        "Request changes",
                        cx,
                    ))
                    .child(
                        div()
                            .ml_auto()
                            .text_size(rpx(10.5))
                            .text_color(colors.muted_foreground)
                            .child("⌘↵"),
                    )
                    .child(hit(
                        "submit-review",
                        Button::new("submit-review")
                            .variant(ButtonVariant::Default)
                            .size(ButtonSize::Sm)
                            .icon(IconName::Send)
                            .loading(self.submitting)
                            .disabled(!can_submit)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                            .child(label),
                    )),
            )
    }

    /// "The author has pushed", offered when the check before submitting finds the head
    /// moved.
    fn push_dialog(&self, item: &ReviewItem, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity();
        let close = {
            let view = view.clone();
            move |_: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    this.pushed = None;
                    cx.notify();
                })
            }
        };
        let count = self.drafts.len();
        Dialog::new("The author has pushed", self.push_focus.clone())
            .description(format!(
                "{} has pushed to {} since you started drafting.",
                item.author.name, item.source_branch
            ))
            .width(448.)
            .on_close(close)
            .child(
                div()
                    .text_size(rpx(13.))
                    .line_height(rpx(21.))
                    .child(format!(
                        "Your comments will be sent against the diff you read, so each one lands \
                         on the code you were actually looking at. {} will mark them outdated \
                         itself - nothing is dropped, and nothing is moved to a different line.",
                        item.provider.label()
                    )),
            )
            .footer(hit(
                "read-new-diff",
                Button::new("read-new-diff")
                    .variant(ButtonVariant::Secondary)
                    .on_click(cx.listener(|this, _, window, cx| {
                        // Reading first is the other reasonable thing to want, and the
                        // updated diff is already here from the check.
                        if let Some(pushed) = this.pushed.take() {
                            this.set_detail(pushed, window, cx);
                        }
                        cx.notify();
                    }))
                    .child("Read the new diff first"),
            ))
            .footer(hit(
                "submit-anyway",
                Button::new("submit-anyway")
                    .variant(ButtonVariant::Default)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pushed = None;
                        this.send(window, cx);
                    }))
                    .child(format!("Submit {}", plural(count, "comment"))),
            ))
    }

    fn discard_dialog(
        &self,
        item: &ReviewItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity();
        let close = move |_: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| {
                this.confirm_discard = false;
                cx.notify();
            })
        };
        Dialog::new(
            format!("Discard {}?", plural(self.drafts.len(), "draft comment")),
            self.discard_focus.clone(),
        )
        .width(448.)
        .on_close(close)
        .child(
            div()
                .text_size(rpx(13.))
                .line_height(rpx(21.))
                .child(format!(
                    "They are only here - nothing has been sent to {} - so this cannot be undone.",
                    item.provider.label()
                )),
        )
        .footer(hit(
            "discard-cancel",
            Button::new("discard-cancel")
                .variant(ButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_discard = false;
                    cx.notify();
                }))
                .child("Cancel"),
        ))
        .footer(hit(
            "discard-confirm",
            Button::new("discard-confirm")
                .variant(ButtonVariant::Danger)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_discard = false;
                    let item_id = this.item_id.clone();
                    this.drafts = this
                        .state
                        .update(cx, |state, cx| state.discard_drafts(&item_id, cx));
                    this.diverged = false;
                    toast(cx, ToastKind::Info, "Drafts discarded.");
                    cx.notify();
                }))
                .child("Discard them"),
        ))
    }
}

/// Lays a dialog over the whole window rather than over this pane.
///
/// The dialog's scrim is `absolute` inside whatever parent holds it, and this view sits
/// to the right of the sidebar, so left alone the scrim would stop at the sidebar's edge
/// and leave the deck clickable while a modal is open. The React dialog was `fixed
/// inset-0`; an anchored layer at the window's origin, sized to the window, is the same
/// thing here.
fn window_wide(dialog: impl IntoElement, window: &Window) -> impl IntoElement {
    let viewport = window.viewport_size();
    anchored()
        .anchor(Corner::TopLeft)
        .position(point(px(0.), px(0.)))
        .child(
            div()
                .relative()
                .w(viewport.width)
                .h(viewport.height)
                .debug_selector(|| "pull-dialog-layer".into())
                .child(dialog),
        )
}

impl Render for PullView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;

        // The drafts are the store's, and the diff adds to them without telling this view,
        // so they are read back on every frame rather than remembered. It is a filter over
        // a handful of entries.
        {
            let app = self.state.read(cx);
            let drafts = app.drafts(&self.item_id);
            let diverged = app.drafts_diverged(&self.item_id);
            if drafts != self.drafts {
                self.drafts = drafts;
            }
            self.diverged = diverged;
        }

        let Some(item) = self.item.clone() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(rpx(13.))
                .text_color(colors.muted_foreground)
                .child("That pull request is no longer in the deck.")
                .into_any_element();
        };

        let show_banner = self.diverged && !self.drafts.is_empty();
        let content = self.content(&item, cx);
        let header = self.header(&item, cx);
        let banner = show_banner.then(|| self.divergence_banner(cx));
        let composer = self.composer(cx);
        let push_dialog = self.pushed.is_some().then(|| self.push_dialog(&item, cx));
        let discard_dialog = self.confirm_discard.then(|| self.discard_dialog(&item, cx));

        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .children(banner)
            .child(div().flex_1().min_h_0().relative().child(content))
            .child(composer)
            .children(push_dialog.map(|dialog| window_wide(dialog, window)))
            .children(discard_dialog.map(|dialog| window_wide(dialog, window)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use futures::future::LocalBoxFuture;
    use gpui::{
        Bounds, Context, Entity, IntoElement, Modifiers, Pixels, Render, TestAppContext,
        VisualTestContext, Window, div, px,
    };
    use reviewdeck_core::demo::DEMO_ITEMS;
    use reviewdeck_core::drafts::NewDraft;
    use reviewdeck_core::http::{Http, Method, MockResponse};
    use reviewdeck_core::model::{
        CheckRun, CheckStatus, CheckSummary, MyReviewState, NewAccount, ProviderKind, make_item_id,
    };
    use reviewdeck_core::store::{MemoryTokens, TokenStore, Vault};
    use serde_json::{Value, json};

    use super::*;
    use crate::state::{AppDeps, Remote};
    use crate::ui::theme::Theme;

    thread_local! {
        pub(super) static TOASTS: RefCell<Vec<(ToastKind, String)>> = const { RefCell::new(Vec::new()) };
    }

    fn toasts() -> Vec<(ToastKind, String)> {
        TOASTS.with(|toasts| toasts.borrow().clone())
    }

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    const NUMBER: u64 = 7;

    /// The host the mock transport plays: GitHub, with a pull request whose head, files and
    /// conversation the test can change, and a record of every write that reached it.
    struct Server {
        head: Mutex<String>,
        comments: Mutex<Vec<Value>>,
        detail_fails: AtomicBool,
        review_status: Mutex<u16>,
        writes: Mutex<Vec<(String, Value)>>,
    }

    impl Server {
        fn new() -> Arc<Server> {
            Arc::new(Server {
                head: Mutex::new("h1".into()),
                comments: Mutex::new(vec![json!({
                    "id": 1,
                    "user": { "login": "mnovotna", "avatar_url": "" },
                    "body": "Looks good overall.",
                    "created_at": "2026-08-01T10:00:00Z",
                })]),
                detail_fails: AtomicBool::new(false),
                review_status: Mutex::new(200),
                writes: Mutex::new(Vec::new()),
            })
        }

        fn http(self: &Arc<Server>) -> Http {
            let server = self.clone();
            Http::mock(move |request| {
                let url = request.url.as_str();
                if url.ends_with("/graphql") {
                    // The REST fallback is what the conversation is read from.
                    return MockResponse::new(500, Vec::new());
                }
                if request.method == Method::Post {
                    locked(&server.writes)
                        .push((url.to_string(), request.json_body().unwrap_or(Value::Null)));
                    let status = if url.ends_with("/reviews") {
                        *locked(&server.review_status)
                    } else {
                        201
                    };
                    return MockResponse::json(status, &json!({}));
                }
                if url.ends_with(&format!("/pulls/{NUMBER}")) {
                    if server.detail_fails.load(Ordering::SeqCst) {
                        return MockResponse::new(500, Vec::new());
                    }
                    return MockResponse::json(
                        200,
                        &json!({
                            "body": "Replaces the **palette**.",
                            "head": { "sha": locked(&server.head).clone() },
                            "base": { "sha": "b1" },
                            "additions": 1, "deletions": 0, "changed_files": 1,
                        }),
                    );
                }
                if url.contains(&format!("/pulls/{NUMBER}/files")) {
                    return MockResponse::json(
                        200,
                        &json!([{
                            "filename": "src/a.rs", "status": "modified",
                            "additions": 1, "deletions": 0,
                            "patch": "@@ -1,1 +1,2 @@\n a\n+b",
                        }]),
                    );
                }
                if url.contains(&format!("/issues/{NUMBER}/comments")) {
                    return MockResponse::json(
                        200,
                        &Value::Array(locked(&server.comments).clone()),
                    );
                }
                MockResponse::json(200, &json!([]))
            })
        }

        fn writes_to(&self, suffix: &str) -> Vec<Value> {
            locked(&self.writes)
                .iter()
                .filter(|(url, _)| url.ends_with(suffix))
                .map(|(_, body)| body.clone())
                .collect()
        }

        fn no_writes(&self) -> bool {
            locked(&self.writes).is_empty()
        }
    }

    /// The deck's hosts: the item list is scripted, everything else is the real adapters
    /// over the mock transport, except a submission the test wants to fail its own way.
    struct Remotes {
        items: Mutex<Vec<ReviewItem>>,
        real: crate::state::ProviderRemote,
        submit: Mutex<Option<reviewdeck_core::Result<()>>>,
    }

    impl Remote for Remotes {
        fn list_review_requests(
            &self,
            _: reviewdeck_core::providers::Session,
        ) -> LocalBoxFuture<'static, reviewdeck_core::Result<Vec<ReviewItem>>> {
            let items = locked(&self.items).clone();
            Box::pin(async move { Ok(items) })
        }

        fn refresh_checks(
            &self,
            session: reviewdeck_core::providers::Session,
            item: ReviewItem,
        ) -> LocalBoxFuture<'static, reviewdeck_core::Result<CheckSummary>> {
            self.real.refresh_checks(session, item)
        }

        fn submit_review(
            &self,
            session: reviewdeck_core::providers::Session,
            item: ReviewItem,
            verdict: ReviewVerdict,
            body: String,
            drafts: Vec<DraftComment>,
        ) -> LocalBoxFuture<'static, reviewdeck_core::Result<()>> {
            if let Some(outcome) = locked(&self.submit).clone() {
                return Box::pin(async move { outcome });
            }
            self.real
                .submit_review(session, item, verdict, body, drafts)
        }
    }

    struct Rig {
        state: Entity<AppState>,
        server: Arc<Server>,
        remotes: Arc<Remotes>,
        item: ReviewItem,
        clock: Arc<AtomicI64>,
    }

    static NEXT_DIR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn rig(cx: &mut TestAppContext) -> Rig {
        let dir = std::env::temp_dir().join(format!(
            "reviewdeck-pull-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let tokens: Arc<dyn TokenStore> = Arc::new(MemoryTokens::new());
        let vault = Arc::new(Vault::open_at(dir.join("reviewdeck.json"), tokens));
        let account = vault
            .add_account(
                NewAccount {
                    kind: ProviderKind::Github,
                    label: "Work".into(),
                    base_url: "https://api.github.com".into(),
                    web_url: "https://github.com".into(),
                    username: "octocat".into(),
                    display_name: "Octo Cat".into(),
                    avatar_url: String::new(),
                    agent_command: None,
                },
                "ghp_test",
            )
            .expect("the fixture vault takes an account");

        let mut item = DEMO_ITEMS[0].clone();
        item.account_id = account.id.clone();
        item.provider = ProviderKind::Github;
        item.repo_key = "acme/tokens".into();
        item.repo = "acme/tokens".into();
        item.number = NUMBER;
        item.id = make_item_id(&account.id, &item.repo_key, NUMBER);
        item.my_review_state = MyReviewState::Pending;
        item.checks = CheckSummary {
            status: CheckStatus::Passed,
            total: 1,
            passed: 1,
            failed: 0,
            running: 0,
            runs: vec![CheckRun {
                id: "c1".into(),
                name: "build".into(),
                status: CheckStatus::Passed,
                description: None,
                url: None,
            }],
        };

        let server = Server::new();
        let http = server.http();
        let remotes = Arc::new(Remotes {
            items: Mutex::new(vec![item.clone()]),
            real: crate::state::ProviderRemote::new(http.clone()),
            submit: Mutex::new(None),
        });
        let clock = Arc::new(AtomicI64::new(1_780_000_000_000));
        let ticking = clock.clone();
        let state = cx.new(|cx| {
            AppState::new(
                AppDeps {
                    http,
                    vault,
                    remote: Some(remotes.clone()),
                    demo: false,
                    clock: Some(Box::new(move || ticking.load(Ordering::SeqCst))),
                    notify: None,
                    tray: None,
                },
                cx,
            )
        });
        cx.update(|cx| {
            cx.set_global(GlobalState(state.clone()));
            cx.set_global(Theme::new(false));
            crate::ui::components::bind_keys(cx);
        });
        let task = state.update(cx, |state, cx| state.refresh(cx));
        cx.executor().block_test(task).expect("the deck syncs");
        TOASTS.with(|toasts| toasts.borrow_mut().clear());
        Rig {
            state,
            server,
            remotes,
            item,
            clock,
        }
    }

    /// A sidebar and, beside it, the pull request - the shape the app view gives it.
    struct Host {
        view: Entity<PullView>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .w(px(200.))
                        .h_full()
                        .debug_selector(|| "sidebar".into()),
                )
                .child(div().flex_1().min_w_0().child(self.view.clone()))
        }
    }

    fn open<'a>(
        cx: &'a mut TestAppContext,
        rig: &Rig,
    ) -> (Entity<PullView>, &'a mut VisualTestContext) {
        let item_id = rig.item.id.clone();
        let (host, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| PullView::new(item_id, window, cx));
            Host { view }
        });
        cx.run_until_parked();
        let view = host.read_with(cx, |host, _| host.view.clone());
        (view, cx)
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
    }

    fn on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
        cx.debug_bounds(selector).is_some()
    }

    fn type_into_composer(view: &Entity<PullView>, cx: &mut VisualTestContext, text: &str) {
        let body = view.read_with(cx, |view, _| view.body.clone());
        cx.update(|window, cx| body.read(cx).focus(window));
        cx.simulate_input(text);
        cx.run_until_parked();
    }

    fn draft(rig: &Rig, cx: &mut VisualTestContext, head: &str, body: &str) -> DraftComment {
        let item_id = rig.item.id.clone();
        let body = body.to_string();
        let head = head.to_string();
        let drafts = rig
            .state
            .update(cx, |state, cx| {
                state.add_draft(
                    NewDraft {
                        item_id,
                        body,
                        path: "src/a.rs".into(),
                        new_line: Some(2),
                        old_line: None,
                        range: None,
                        refs: DiffRefs {
                            base_sha: Some("b1".into()),
                            start_sha: None,
                            head_sha: Some(head),
                        },
                    },
                    cx,
                )
            })
            .expect("the draft is added");
        cx.run_until_parked();
        drafts.last().cloned().expect("a draft exists")
    }

    // ----- pure helpers ---------------------------------------------------------

    #[test]
    fn the_pending_badge_puts_the_count_before_pending() {
        assert_eq!(pending_label(1), "1 pending comment");
        assert_eq!(pending_label(3), "3 pending comments");
    }

    #[test]
    fn the_composer_words_follow_the_verdict_then_the_drafts() {
        use ReviewVerdict::*;
        assert_eq!(
            placeholder_for(Some(Approve), true),
            "Optional note with your approval…"
        );
        assert_eq!(
            placeholder_for(Some(RequestChanges), false),
            "What needs to change?"
        );
        assert_eq!(
            placeholder_for(None, true),
            "Optional summary for your review…"
        );
        assert_eq!(
            placeholder_for(None, false),
            "Leave a comment on this pull request…"
        );
        assert_eq!(submit_label(Some(Approve), true), "Approve");
        assert_eq!(submit_label(Some(RequestChanges), false), "Request changes");
        assert_eq!(submit_label(None, true), "Submit review");
        assert_eq!(submit_label(None, false), "Comment");
    }

    // ----- loading --------------------------------------------------------------

    #[gpui::test]
    fn the_pull_request_loads_with_its_counts_and_conversation(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        view.read_with(cx, |view, _| {
            assert!(view.loaded && !view.loading && view.error.is_none());
            assert_eq!(view.files.len(), 1);
            assert_eq!(view.description, "Replaces the **palette**.");
            assert!(view.description_view.is_some());
            // The issue comment has no file or line, so it belongs to the conversation.
            assert_eq!(view.conversation.len(), 1);
            assert!(view.inline.is_empty());
            assert_eq!(view.tab, Tab::Diff);
        });
        assert!(on_screen(cx, "tab-diff") && on_screen(cx, "tab-checks"));
    }

    #[gpui::test]
    fn a_failed_load_offers_retry_and_opening_in_the_browser(cx: &mut TestAppContext) {
        let rig = rig(cx);
        rig.server.detail_fails.store(true, Ordering::SeqCst);
        let (view, cx) = open(cx, &rig);
        view.read_with(cx, |view, _| {
            assert!(!view.loaded && !view.loading);
            assert!(view.error.is_some());
        });
        assert!(on_screen(cx, "retry-load") && on_screen(cx, "open-instead"));

        click(cx, "open-instead");
        assert_eq!(cx.opened_url().as_deref(), Some(rig.item.url.as_str()));

        rig.server.detail_fails.store(false, Ordering::SeqCst);
        click(cx, "retry-load");
        view.read_with(cx, |view, _| {
            assert!(view.loaded && view.error.is_none());
        });
    }

    #[gpui::test]
    fn the_tabs_switch_and_the_mode_toggle_belongs_to_files(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        click(cx, "tab-checks");
        view.read_with(cx, |view, _| assert_eq!(view.tab, Tab::Checks));
        click(cx, "tab-conversation");
        view.read_with(cx, |view, _| assert_eq!(view.tab, Tab::Conversation));
        click(cx, "tab-diff");
        assert!(on_screen(cx, "diff-split") && on_screen(cx, "diff-unified"));
    }

    #[gpui::test]
    fn the_diff_layout_toggle_is_saved_in_the_settings(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (_, cx) = open(cx, &rig);
        click(cx, "diff-unified");
        let mode = rig
            .state
            .read_with(cx, |state, _| state.settings().diff_view);
        assert_eq!(mode, DiffViewMode::Unified);
        click(cx, "diff-split");
        let mode = rig
            .state
            .read_with(cx, |state, _| state.settings().diff_view);
        assert_eq!(mode, DiffViewMode::Split);
    }

    // ----- the conversation poll ------------------------------------------------

    #[gpui::test]
    fn a_later_sync_rereads_the_conversation_and_an_unchanged_one_leaves_it(
        cx: &mut TestAppContext,
    ) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        let first_card = view.read_with(cx, |view, _| view.cards[0].card.entity_id());

        // A sync that finds the same conversation: nothing is rebuilt.
        rig.clock.fetch_add(60_000, Ordering::SeqCst);
        let task = rig.state.update(cx, |state, cx| state.refresh(cx));
        cx.executor().block_test(task).expect("the deck syncs");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.conversation.len(), 1);
            assert_eq!(view.cards[0].card.entity_id(), first_card);
        });

        // A reply lands, and the next sync brings it in without touching the diff.
        locked(&rig.server.comments).push(json!({
            "id": 2,
            "user": { "login": "hkramer", "avatar_url": "" },
            "body": "One more thing.",
            "created_at": "2026-08-01T11:00:00Z",
        }));
        rig.clock.fetch_add(60_000, Ordering::SeqCst);
        let task = rig.state.update(cx, |state, cx| state.refresh(cx));
        cx.executor().block_test(task).expect("the deck syncs");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.conversation.len(), 2);
            assert_eq!(view.files.len(), 1);
            // The card of a thread that was already there is kept.
            assert!(
                view.cards
                    .iter()
                    .any(|entry| entry.card.entity_id() == first_card)
            );
        });
    }

    // ----- the header actions ---------------------------------------------------

    #[gpui::test]
    fn copying_the_prompt_puts_it_on_the_clipboard_and_says_so(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (_, cx) = open(cx, &rig);
        click(cx, "copy-agent-prompt");
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("the clipboard holds the command");
        assert!(copied.contains(&rig.item.url) || copied.contains("acme/tokens"));
        assert_eq!(
            toasts(),
            vec![(
                ToastKind::Ok,
                "Claude prompt copied - paste it in your terminal.".to_string()
            )]
        );
    }

    #[gpui::test]
    fn open_goes_to_the_pull_requests_page(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (_, cx) = open(cx, &rig);
        click(cx, "open-in-browser");
        assert_eq!(cx.opened_url().as_deref(), Some(rig.item.url.as_str()));
    }

    // ----- the composer ---------------------------------------------------------

    #[gpui::test]
    fn an_empty_composer_cannot_be_submitted(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        view.read_with(cx, |view, cx| assert!(!view.can_submit(cx)));
        click(cx, "submit-review");
        assert!(rig.server.no_writes());
        assert!(toasts().is_empty());
    }

    #[gpui::test]
    fn a_comment_alone_is_posted_as_a_comment(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        type_into_composer(&view, cx, "  Nice work  ");
        view.read_with(cx, |view, cx| assert!(view.can_submit(cx)));
        click(cx, "submit-review");

        assert_eq!(
            rig.server.writes_to(&format!("/issues/{NUMBER}/comments")),
            vec![json!({ "body": "Nice work" })]
        );
        assert!(rig.server.writes_to("/reviews").is_empty());
        assert_eq!(
            toasts(),
            vec![(ToastKind::Ok, "Comment posted.".to_string())]
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(view.body.read(cx).text(), "");
            assert!(!view.submitting);
        });
    }

    #[gpui::test]
    fn approving_needs_no_words_and_resets_the_verdict(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        click(cx, "verdict-approve");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.verdict, Some(ReviewVerdict::Approve));
            assert!(view.can_submit(cx));
        });
        click(cx, "submit-review");

        assert_eq!(
            rig.server.writes_to("/reviews"),
            vec![json!({ "event": "APPROVE" })]
        );
        assert_eq!(toasts(), vec![(ToastKind::Ok, "Approved.".to_string())]);
        view.read_with(cx, |view, _| assert_eq!(view.verdict, None));
    }

    #[gpui::test]
    fn the_verdict_buttons_toggle_and_exclude_each_other(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        click(cx, "verdict-approve");
        click(cx, "verdict-approve");
        view.read_with(cx, |view, _| assert_eq!(view.verdict, None));
        click(cx, "verdict-approve");
        click(cx, "verdict-request-changes");
        view.read_with(cx, |view, _| {
            assert_eq!(view.verdict, Some(ReviewVerdict::RequestChanges));
        });
    }

    #[gpui::test]
    fn requesting_changes_needs_words(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        click(cx, "verdict-request-changes");
        view.read_with(cx, |view, cx| assert!(!view.can_submit(cx)));
        click(cx, "submit-review");
        assert!(rig.server.no_writes());

        type_into_composer(&view, cx, "Rename it");
        click(cx, "submit-review");
        assert_eq!(
            rig.server.writes_to("/reviews"),
            vec![json!({ "event": "REQUEST_CHANGES", "body": "Rename it" })]
        );
        assert_eq!(
            toasts(),
            vec![(ToastKind::Ok, "Changes requested.".to_string())]
        );
    }

    #[gpui::test]
    fn drafts_go_out_together_as_a_review_with_the_summary(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.drafts.len(), 1);
            assert!(view.can_submit(cx), "a draft alone is enough to submit");
        });
        type_into_composer(&view, cx, "Summary");
        click(cx, "submit-review");

        let reviews = rig.server.writes_to("/reviews");
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0]["event"], "COMMENT");
        assert_eq!(reviews[0]["body"], "Summary");
        assert_eq!(reviews[0]["commit_id"], "h1");
        assert_eq!(reviews[0]["comments"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            toasts(),
            vec![(ToastKind::Ok, "Review submitted.".to_string())]
        );
        view.read_with(cx, |view, _| assert!(view.drafts.is_empty()));
    }

    #[gpui::test]
    fn cmd_enter_and_ctrl_enter_both_submit(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        let comments = format!("/issues/{NUMBER}/comments");
        type_into_composer(&view, cx, "First");
        cx.simulate_keystrokes("cmd-enter");
        cx.run_until_parked();
        assert_eq!(rig.server.writes_to(&comments).len(), 1);

        type_into_composer(&view, cx, "Second");
        cx.simulate_keystrokes("ctrl-enter");
        cx.run_until_parked();
        assert_eq!(rig.server.writes_to(&comments).len(), 2);

        // Nothing to send: the shortcut does nothing, as the button is disabled.
        cx.simulate_keystrokes("ctrl-enter");
        cx.run_until_parked();
        assert_eq!(rig.server.writes_to(&comments).len(), 2);
    }

    #[gpui::test]
    fn a_rejected_review_keeps_the_words_the_drafts_and_the_verdict(cx: &mut TestAppContext) {
        let rig = rig(cx);
        *locked(&rig.server.review_status) = 422;
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        click(cx, "verdict-request-changes");
        type_into_composer(&view, cx, "Rename it");
        click(cx, "submit-review");

        let shown = toasts();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].0, ToastKind::Bad);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.drafts.len(), 1);
            assert_eq!(view.body.read(cx).text(), "Rename it");
            assert_eq!(view.verdict, Some(ReviewVerdict::RequestChanges));
            assert!(!view.submitting, "the button is usable again");
        });
    }

    #[gpui::test]
    fn a_partial_submission_says_so_and_shows_what_is_left(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        let first = draft(&rig, cx, "h1", "One");
        draft(&rig, cx, "h1", "Two");
        *locked(&rig.remotes.submit) = Some(Err(reviewdeck_core::Error::PartialSubmit {
            message: "Only 1 of 2 comments was posted.".into(),
            posted: vec![first.id.clone()],
        }));
        click(cx, "submit-review");

        assert_eq!(
            toasts(),
            vec![(
                ToastKind::Bad,
                "Only 1 of 2 comments was posted.".to_string()
            )]
        );
        view.read_with(cx, |view, _| {
            assert_eq!(view.drafts.len(), 1, "the one that landed is gone");
            assert_ne!(view.drafts[0].id, first.id);
            assert!(!view.submitting);
        });
    }

    // ----- the author pushed ----------------------------------------------------

    #[gpui::test]
    fn a_moved_head_asks_before_anything_is_sent(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        *locked(&rig.server.head) = "h2".into();
        click(cx, "submit-review");

        view.read_with(cx, |view, _| assert!(view.pushed.is_some()));
        assert!(rig.server.no_writes());
        assert!(on_screen(cx, "submit-anyway") && on_screen(cx, "read-new-diff"));

        // Sent against the code that was read, not the new head.
        click(cx, "submit-anyway");
        let reviews = rig.server.writes_to("/reviews");
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0]["commit_id"], "h1");
        view.read_with(cx, |view, _| assert!(view.pushed.is_none()));
    }

    #[gpui::test]
    fn reading_the_new_diff_first_adopts_it_and_sends_nothing(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        *locked(&rig.server.head) = "h2".into();
        click(cx, "submit-review");
        click(cx, "read-new-diff");

        assert!(rig.server.no_writes());
        view.read_with(cx, |view, _| {
            assert!(view.pushed.is_none());
            assert_eq!(view.refs.head_sha.as_deref(), Some("h2"));
            assert_eq!(view.drafts.len(), 1, "the drafts are kept");
        });
    }

    #[gpui::test]
    fn escape_dismisses_the_push_dialog(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        *locked(&rig.server.head) = "h2".into();
        click(cx, "submit-review");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.pushed.is_none()));
        assert!(rig.server.no_writes());
    }

    #[gpui::test]
    fn the_scrim_covers_the_whole_window_not_just_the_pane(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (_, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        *locked(&rig.server.head) = "h2".into();
        click(cx, "submit-review");

        let layer: Bounds<Pixels> = cx
            .debug_bounds("pull-dialog-layer")
            .expect("a dialog layer");
        let sidebar = cx.debug_bounds("sidebar").expect("the sidebar");
        assert_eq!(layer.origin, gpui::point(px(0.), px(0.)));
        assert!(layer.contains(&sidebar.origin) && layer.contains(&sidebar.bottom_right()));
        let window = cx.update(|window, _| window.viewport_size());
        assert_eq!(layer.size, window);
    }

    // ----- a review sent from elsewhere -----------------------------------------

    /// The review goes out in a browser, and the next sync finds the item approved.
    fn diverge(rig: &Rig, cx: &mut VisualTestContext) {
        for item in locked(&rig.remotes.items).iter_mut() {
            item.my_review_state = MyReviewState::Approved;
        }
        rig.clock.fetch_add(60_000, Ordering::SeqCst);
        let task = rig.state.update(cx, |state, cx| state.refresh(cx));
        cx.executor().block_test(task).expect("the deck syncs");
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_banner_appears_for_drafts_left_behind_and_keep_dismisses_it(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        assert!(!on_screen(cx, "keep-drafts"));
        draft(&rig, cx, "h1", "Why this?");
        assert!(
            !on_screen(cx, "keep-drafts"),
            "nothing went out elsewhere yet"
        );
        diverge(&rig, cx);
        view.read_with(cx, |view, _| assert!(view.diverged));
        assert!(on_screen(cx, "keep-drafts") && on_screen(cx, "discard-all"));

        click(cx, "keep-drafts");
        view.read_with(cx, |view, _| {
            assert!(!view.diverged);
            assert_eq!(view.drafts.len(), 1, "keeping loses nothing");
        });
    }

    #[gpui::test]
    fn discarding_asks_first_and_cancel_loses_nothing(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        diverge(&rig, cx);

        click(cx, "discard-all");
        view.read_with(cx, |view, _| assert!(view.confirm_discard));
        click(cx, "discard-cancel");
        view.read_with(cx, |view, _| {
            assert!(!view.confirm_discard);
            assert_eq!(view.drafts.len(), 1);
        });

        click(cx, "discard-all");
        click(cx, "discard-confirm");
        view.read_with(cx, |view, _| {
            assert!(view.drafts.is_empty() && !view.diverged && !view.confirm_discard);
        });
        assert_eq!(
            toasts(),
            vec![(ToastKind::Info, "Drafts discarded.".to_string())]
        );
        assert!(
            rig.state
                .read_with(cx, |state, _| state.drafts(&rig.item.id).is_empty())
        );
    }

    #[gpui::test]
    fn escape_dismisses_the_discard_dialog(cx: &mut TestAppContext) {
        let rig = rig(cx);
        let (view, cx) = open(cx, &rig);
        draft(&rig, cx, "h1", "Why this?");
        diverge(&rig, cx);
        click(cx, "discard-all");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(!view.confirm_discard);
            assert_eq!(view.drafts.len(), 1);
        });
    }

    // ----- the checks panel and the thread split --------------------------------

    fn summary(status: CheckStatus, passed: u32, failed: u32, running: u32) -> CheckSummary {
        CheckSummary {
            status,
            total: passed + failed + running,
            passed,
            failed,
            running,
            runs: Vec::new(),
        }
    }

    #[test]
    fn the_checks_headline_counts_what_matters_for_the_status() {
        use crate::ui::checks_panel::headline;
        assert_eq!(
            headline(&summary(CheckStatus::Running, 1, 0, 1)),
            "1 check still running"
        );
        assert_eq!(
            headline(&summary(CheckStatus::Running, 0, 0, 3)),
            "3 checks still running"
        );
        assert_eq!(
            headline(&summary(CheckStatus::Failed, 1, 2, 0)),
            "2 checks failing"
        );
        assert_eq!(
            headline(&summary(CheckStatus::Failed, 0, 1, 0)),
            "1 check failing"
        );
        assert_eq!(
            headline(&summary(CheckStatus::Passed, 2, 0, 0)),
            "All checks passed"
        );
        assert_eq!(
            headline(&summary(CheckStatus::Unknown, 0, 0, 0)),
            "Check status unknown"
        );
    }

    #[test]
    fn threads_without_a_file_a_line_or_a_file_in_the_diff_belong_to_the_conversation() {
        let thread = |id: &str, path: Option<&str>, line: Option<u32>| CommentThread {
            id: id.into(),
            comments: Vec::new(),
            resolved: false,
            outdated: false,
            path: path.map(Into::into),
            line,
            start_line: None,
            side: None,
            can_reply: true,
            can_resolve: true,
        };
        let file = DiffFile {
            path: "src/a.rs".into(),
            old_path: "src/a.rs".into(),
            status: reviewdeck_core::model::FileStatus::Modified,
            additions: 1,
            deletions: 0,
            binary: false,
            patch: None,
        };
        let (inline, rest) = split_threads(
            &[
                thread("anchored", Some("src/a.rs"), Some(2)),
                thread("outdated", Some("src/a.rs"), None),
                thread("elsewhere", Some("src/b.rs"), Some(1)),
                thread("general", None, None),
            ],
            &[file],
        );
        assert_eq!(
            inline.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["anchored"]
        );
        assert_eq!(
            rest.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["outdated", "elsewhere", "general"]
        );
    }
}
