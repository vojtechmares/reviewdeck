// The views that call these land with the integration; until then they read as
// unused. Removed at integration, like ui/mod.rs.
#![allow(dead_code)]

//! Port of src/main/deck.ts, src/main/ipc.ts, src/main/images.ts and the app-level
//! state the renderer kept in src/renderer/src/hooks/useApp.tsx.
//!
//! [`AppState`] is the one gpui entity the views read and the platform glue drives.
//! It owns what the Electron main process owned: the HTTP client, the vault, the
//! draft store, the aggregated deck, the timers, the menu bar item and the
//! notifications. Every IPC channel of ipc.ts is a method here. Anything that talks
//! to a host returns a gpui [`Task`]; anything local returns its value directly.
//!
//! Views never see a token. Authenticated images are fetched here and handed back
//! as decoded [`Image`]s.
//!
//! Keep the tasks you get back: dropping a [`Task`] cancels it, so a caller that
//! does not need the result says so with `.detach()`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{LocalBoxFuture, join_all};
use gpui::{
    App, AsyncApp, ClipboardItem, Context, Entity, EventEmitter, Global, Image, ImageFormat, Task,
    WeakEntity,
};
use reviewdeck_core::demo::{DEMO_ACCOUNTS, DEMO_ITEMS, demo_detail};
use reviewdeck_core::drafts::{DraftSet, DraftState, DraftStore, NewDraft, SAVE_DEBOUNCE_MS};
use reviewdeck_core::error::{Error, Result, msg};
use reviewdeck_core::http::Http;
use reviewdeck_core::images::{self, ImageAccount};
use reviewdeck_core::model::{
    Account, AccountDraft, AccountStatus, ApprovalSummary, CheckStatus, CheckSummary,
    CommentThread, DeckState, DraftComment, MyReviewState, NewAccount, PullDetail, ReviewItem,
    ReviewSubmission, ReviewVerdict, ReviewWindow, Settings, approvals_after_approving,
    ids_to_record, reviews_to_announce, visible_reviews,
};
use reviewdeck_core::providers::{self, Session};
use reviewdeck_core::review_window::{
    WINDOW_TICK_MS, announcing_allowed_for, local_day, quiet_until, window_covers, windows_to_fire,
};
use reviewdeck_core::store::Vault;
use reviewdeck_core::time::{format_iso, now_ms};

use crate::platform::appearance;
use crate::platform::login_item;
use crate::platform::notify::{Notification, Notifier};
use crate::platform::tray::{Tray, TrayMenu, tray_title};
use crate::ui::theme::Theme;

/// What the sync engine and the submit path need from a host.
///
/// Production answers through the provider adapters; tests substitute scripted
/// answers, which is how the engine is exercised without the network.
pub trait Remote: Send + Sync {
    fn list_review_requests(
        &self,
        session: Session,
    ) -> LocalBoxFuture<'static, Result<Vec<ReviewItem>>>;
    fn refresh_checks(
        &self,
        session: Session,
        item: ReviewItem,
    ) -> LocalBoxFuture<'static, Result<CheckSummary>>;
    fn submit_review(
        &self,
        session: Session,
        item: ReviewItem,
        verdict: ReviewVerdict,
        body: String,
        drafts: Vec<DraftComment>,
    ) -> LocalBoxFuture<'static, Result<()>>;
}

/// The real hosts, through the provider adapters.
pub struct ProviderRemote {
    http: Http,
}

impl ProviderRemote {
    pub fn new(http: Http) -> Self {
        ProviderRemote { http }
    }
}

impl Remote for ProviderRemote {
    fn list_review_requests(
        &self,
        session: Session,
    ) -> LocalBoxFuture<'static, Result<Vec<ReviewItem>>> {
        let http = self.http.clone();
        async move { providers::list_review_requests(&http, &session).await }.boxed_local()
    }

    fn refresh_checks(
        &self,
        session: Session,
        item: ReviewItem,
    ) -> LocalBoxFuture<'static, Result<CheckSummary>> {
        let http = self.http.clone();
        async move { providers::refresh_checks(&http, &session, &item).await }.boxed_local()
    }

    fn submit_review(
        &self,
        session: Session,
        item: ReviewItem,
        verdict: ReviewVerdict,
        body: String,
        drafts: Vec<DraftComment>,
    ) -> LocalBoxFuture<'static, Result<()>> {
        let http = self.http.clone();
        async move {
            providers::submit_review(&http, &session, &item, verdict, &body, &drafts).await
        }
        .boxed_local()
    }
}

/// Where the menu bar, the notifications and the platform glue go. The platform
/// implementation is [`Notifier`]; tests record what would have been shown.
pub trait Notify {
    fn show(&self, notification: &Notification);
}

impl Notify for Notifier {
    fn show(&self, notification: &Notification) {
        Notifier::show(self, notification);
    }
}

/// What the app layer announces to the views beyond "something changed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// Bring this review item into view (a notification was clicked).
    FocusItem(String),
    /// Open the settings dialog (Reviewdeck > Settings…).
    OpenSettings,
}

/// The single app-state entity, reachable from anywhere as a gpui global.
pub struct GlobalState(pub Entity<AppState>);

impl Global for GlobalState {}

/// The app's version and platform, for the about surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub version: &'static str,
    /// `process.platform` in the TypeScript: `darwin` on macOS.
    pub platform: &'static str,
}

/// Everything [`AppState`] is built from. The app passes the vault it opened at
/// launch (there must be one writer); tests build one with a mock [`Http`], a
/// temporary vault and a fake [`Remote`].
pub struct AppDeps {
    pub http: Http,
    pub vault: Arc<Vault>,
    /// The hosts. `None` means the provider adapters over `http`.
    pub remote: Option<Arc<dyn Remote>>,
    /// The demo fixtures (`REVIEWDECK_DEMO=1`).
    pub demo: bool,
    /// Unix milliseconds. `None` means the system clock.
    pub clock: Option<Box<dyn Fn() -> i64>>,
    /// Where notifications go. `None` means notifications are unsupported
    /// (`cargo run` has no bundle identifier), which the TypeScript read as
    /// `Notification.isSupported() === false`.
    pub notify: Option<Box<dyn Notify>>,
    /// The menu bar item. `None` when it could not be created.
    pub tray: Option<Tray>,
}

/// An authenticated image: a fetch in flight, the decoded image, or a failure that
/// is not retried (every frame would otherwise start the same request).
enum ImageSlot {
    Pending,
    Ready(Arc<Image>),
    Failed,
}

/// The deck, the drafts and everything the main process did, as one entity.
pub struct AppState {
    http: Http,
    remote: Arc<dyn Remote>,
    vault: Arc<Vault>,
    demo: bool,
    clock: Box<dyn Fn() -> i64>,
    drafts: DraftStore,
    /// Items keyed by account, in the order the accounts were first seen. One failing
    /// host must not wipe the others, hence the split.
    items: Vec<(String, Vec<ReviewItem>)>,
    statuses: Vec<AccountStatus>,
    syncing: bool,
    /// Signature of the accounts the last completed sync covered, or `None`.
    synced_accounts: Option<String>,
    last_synced_at: Option<String>,
    /// Suppresses notifications on the very first sync after launch.
    primed: bool,
    /// Guards the window tick against itself, so a slow fan-out cannot fire a window
    /// twice.
    opening_windows: bool,
    images: HashMap<(String, String), ImageSlot>,
    notify: Option<Box<dyn Notify>>,
    tray: Option<Tray>,
    /// Set while the sync timer runs: the interval it fires at.
    sync_interval: Option<Duration>,
    sync_timer: Option<Task<()>>,
    check_timer: Option<Task<()>>,
    window_timer: Option<Task<()>>,
    draft_save: Option<Task<()>>,
}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    /// Builds the state from its dependencies. Nothing is fetched and no timer runs
    /// until [`AppState::hydrate`] and [`AppState::start`] are called, in that order.
    pub fn new(deps: AppDeps, _cx: &mut Context<Self>) -> Self {
        let (comments, sets): (Vec<DraftComment>, BTreeMap<String, DraftSet>) =
            deps.vault.load_draft_state();
        let drafts = DraftStore::new(DraftState { comments, sets });
        let remote = deps
            .remote
            .unwrap_or_else(|| Arc::new(ProviderRemote::new(deps.http.clone())));
        AppState {
            http: deps.http,
            remote,
            vault: deps.vault,
            demo: deps.demo,
            clock: deps.clock.unwrap_or_else(|| Box::new(now_ms)),
            drafts,
            items: Vec::new(),
            statuses: Vec::new(),
            syncing: false,
            synced_accounts: None,
            last_synced_at: None,
            primed: false,
            opening_windows: false,
            images: HashMap::new(),
            notify: deps.notify,
            tray: deps.tray,
            sync_interval: None,
            sync_timer: None,
            check_timer: None,
            window_timer: None,
            draft_save: None,
        }
    }

    // ----- accounts -------------------------------------------------------------

    /// `accounts:list`. In demo mode, the fixtures.
    pub fn accounts(&self) -> Vec<Account> {
        if self.demo {
            DEMO_ACCOUNTS.to_vec()
        } else {
            self.vault.list_accounts()
        }
    }

    /// One account by id, or `None` when it is not connected.
    pub fn account(&self, id: &str) -> Option<Account> {
        if self.demo {
            DEMO_ACCOUNTS
                .iter()
                .find(|account| account.id == id)
                .cloned()
        } else {
            self.vault.get_account(id)
        }
    }

    /// `accounts:add`: verifies the token against the host, stores the account and
    /// its token, then refreshes the deck.
    pub fn add_account(
        &mut self,
        draft: AccountDraft,
        cx: &mut Context<Self>,
    ) -> Task<Result<Account>> {
        let token = draft.token.trim().to_string();
        if token.is_empty() {
            return Task::ready(Err(msg("A token is required.")));
        }
        let draft = AccountDraft {
            token: token.clone(),
            ..draft
        };
        let agent_command = trimmed_or_none(draft.agent_command.as_deref());
        let remote_http = self.http.clone();
        let vault = self.vault.clone();
        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| -> Result<Account> {
                let resolved = providers::connect(&remote_http, &draft).await?;
                let account = vault.add_account(
                    NewAccount {
                        agent_command,
                        ..resolved
                    },
                    &token,
                )?;
                this.update(cx, |state, cx| state.refresh(cx).detach()).ok();
                Ok(account)
            },
        )
    }

    /// `accounts:update`: re-verifies the account (with the new token when one is
    /// given), applies the resolved details, and refreshes.
    pub fn update_account(
        &mut self,
        id: &str,
        draft: AccountDraft,
        cx: &mut Context<Self>,
    ) -> Task<Result<Account>> {
        let Some(existing) = self.vault.get_account(id) else {
            return Task::ready(Err(msg("That account is gone.")));
        };
        let new_token = draft.token.trim().to_string();
        let token = if new_token.is_empty() {
            match self.vault.get_token(id) {
                Ok(token) => token,
                Err(error) => return Task::ready(Err(error)),
            }
        } else {
            new_token.clone()
        };
        if token.is_empty() {
            return Task::ready(Err(msg("A token is required.")));
        }
        let label = draft.label.trim().to_string();
        let agent_command = trimmed_or_none(draft.agent_command.as_deref());
        let check = AccountDraft {
            kind: existing.kind,
            token,
            ..draft
        };
        let http = self.http.clone();
        let vault = self.vault.clone();
        let id = id.to_string();
        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| -> Result<Account> {
                let resolved = providers::connect(&http, &check).await?;
                vault.update_account(&id, |account| {
                    account.kind = resolved.kind;
                    account.base_url = resolved.base_url.clone();
                    account.web_url = resolved.web_url.clone();
                    account.username = resolved.username.clone();
                    account.display_name = resolved.display_name.clone();
                    account.avatar_url = resolved.avatar_url.clone();
                    account.label = if label.is_empty() {
                        existing.label.clone()
                    } else {
                        label.clone()
                    };
                    // Blank clears the override, so the setting applies again.
                    account.agent_command = agent_command.clone();
                })?;
                if !new_token.is_empty() {
                    vault.set_token(&id, &new_token)?;
                }
                this.update(cx, |state, cx| state.refresh(cx).detach()).ok();
                vault
                    .get_account(&id)
                    .ok_or_else(|| msg("That account is gone."))
            },
        )
    }

    /// `accounts:remove`. Returns the accounts that remain.
    pub fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) -> Result<Vec<Account>> {
        self.vault.remove_account(id)?;
        self.refresh(cx).detach();
        Ok(self.accounts())
    }

    /// `accounts:rename`. Returns the accounts after the change.
    pub fn rename_account(
        &mut self,
        id: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Result<Vec<Account>> {
        let label = label.to_string();
        self.vault
            .update_account(id, |account| account.label = label)?;
        self.publish(cx);
        Ok(self.accounts())
    }

    /// The token for an account, as the adapters need it, or the TypeScript message
    /// when the account is not connected.
    pub fn session(&self, account_id: &str) -> Result<Session> {
        session_for(&self.vault, account_id)
    }

    // ----- the deck -------------------------------------------------------------

    /// `deck:get`: the deck as the views show it.
    pub fn deck(&self) -> DeckState {
        DeckState {
            items: self.items(),
            statuses: self.statuses.clone(),
            syncing: self.syncing,
            synced: self.synced(),
            last_synced_at: self.last_synced_at.clone(),
        }
    }

    /// Every item, most recently updated first. Stable, so ties keep the order their
    /// accounts were first seen in, as the TypeScript's Map iteration did.
    pub fn items(&self) -> Vec<ReviewItem> {
        let mut all: Vec<ReviewItem> = self
            .items
            .iter()
            .flat_map(|(_, items)| items.iter().cloned())
            .collect();
        all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        all
    }

    /// One item by id, if the deck holds it.
    pub fn find(&self, item_id: &str) -> Option<ReviewItem> {
        self.items
            .iter()
            .flat_map(|(_, items)| items.iter())
            .find(|item| item.id == item_id)
            .cloned()
    }

    /// Whether the last completed sync covered the accounts connected right now.
    ///
    /// Connecting or dropping an account invalidates the last sync: what it found was
    /// about a different set of accounts.
    fn synced(&self) -> bool {
        let Some(signed) = &self.synced_accounts else {
            return false;
        };
        *signed == signature(self.connected_accounts().iter().map(|a| a.id.as_str()))
    }

    fn connected_accounts(&self) -> Vec<Account> {
        self.accounts()
    }

    /// Replaces one item in place, e.g. after approving it. Publishes only when the
    /// item was found, as `patch` did. Returns whether it was.
    pub fn patch(
        &mut self,
        item_id: &str,
        patch: impl FnOnce(&mut ReviewItem),
        cx: &mut Context<Self>,
    ) -> bool {
        let found = self.patch_quiet(item_id, patch);
        if found {
            self.publish(cx);
        }
        found
    }

    fn patch_quiet(&mut self, item_id: &str, patch: impl FnOnce(&mut ReviewItem)) -> bool {
        for (_, items) in self.items.iter_mut() {
            if let Some(item) = items.iter_mut().find(|item| item.id == item_id) {
                patch(item);
                return true;
            }
        }
        false
    }

    /// Drops one item from the deck, publishing if it was there.
    pub fn drop_item(&mut self, item_id: &str, cx: &mut Context<Self>) {
        for (_, items) in self.items.iter_mut() {
            if let Some(at) = items.iter().position(|item| item.id == item_id) {
                items.remove(at);
                self.publish(cx);
                return;
            }
        }
    }

    /// `deck:refresh`: fans out to every account and rebuilds the deck. A refresh
    /// already in flight is joined rather than doubled.
    pub fn refresh(&mut self, cx: &mut Context<Self>) -> Task<Result<DeckState>> {
        if self.syncing {
            return Task::ready(Ok(self.deck()));
        }

        if self.demo {
            self.set_items("demo", DEMO_ITEMS.to_vec());
            self.synced_accounts = Some(signature(DEMO_ACCOUNTS.iter().map(|a| a.id.as_str())));
            self.last_synced_at = Some(self.now_iso());
            self.publish(cx);
            return Task::ready(Ok(self.deck()));
        }

        let accounts = self.vault.list_accounts();

        // Forget accounts that were removed while we were idle.
        self.items
            .retain(|(id, _)| accounts.iter().any(|account| account.id == *id));
        self.statuses.retain(|status| {
            accounts
                .iter()
                .any(|account| account.id == status.account_id)
        });

        self.syncing = true;
        self.publish(cx);

        // Captured before the fetch: a submission made while this is in flight means
        // the states coming back predate it and must not read as someone else's review.
        let submissions_at_start = self.drafts.submissions();
        let signed = signature(accounts.iter().map(|a| a.id.as_str()));

        let fetches: Vec<_> = accounts
            .into_iter()
            .map(|account| fetch_account(self.remote.clone(), self.vault.clone(), account))
            .collect();

        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| -> Result<DeckState> {
                let results = join_all(fetches).await;
                match this.update(cx, |state, cx| {
                    state.finish_sync(results, submissions_at_start, signed, cx);
                    state.deck()
                }) {
                    Ok(deck) => Ok(deck),
                    Err(_) => Err(closing()),
                }
            },
        )
    }

    fn finish_sync(
        &mut self,
        results: Vec<(String, std::result::Result<Vec<ReviewItem>, String>)>,
        submissions_at_start: u64,
        signed: String,
        cx: &mut Context<Self>,
    ) {
        for (account_id, result) in results {
            match result {
                Ok(items) => {
                    let count = items.len() as u32;
                    self.set_items(&account_id, items);
                    self.set_status(AccountStatus {
                        account_id,
                        ok: true,
                        error: None,
                        last_synced_at: Some(self.now_iso()),
                        count,
                    });
                }
                Err(error) => {
                    // Keep whatever we had; a transient outage should not empty the deck.
                    let previous = self.statuses.iter().find(|s| s.account_id == account_id);
                    let last_synced_at = previous.and_then(|s| s.last_synced_at.clone());
                    let count = self.items_of(&account_id).len() as u32;
                    self.set_status(AccountStatus {
                        account_id,
                        ok: false,
                        error: Some(error),
                        last_synced_at,
                        count,
                    });
                }
            }
        }

        self.reconcile_drafts(submissions_at_start);
        self.syncing = false;
        // Of the accounts this sync fanned out over, not of whatever is connected now.
        self.synced_accounts = Some(signed);
        self.last_synced_at = Some(self.now_iso());
        self.persist_deck_cache();
        self.announce_new();
        self.publish(cx);
        self.schedule_check_poll(cx);
    }

    /// Notices a review the reviewer submitted somewhere else. Only items with drafts
    /// are worth looking at.
    fn reconcile_drafts(&mut self, submissions_at_start: u64) {
        for item in self.items() {
            if self.drafts.count(&item.id) == 0 {
                continue;
            }
            self.drafts
                .reconcile(&item.id, item.my_review_state, submissions_at_start);
        }
    }

    fn items_of(&self, account_id: &str) -> &[ReviewItem] {
        self.items
            .iter()
            .find(|(id, _)| id == account_id)
            .map(|(_, items)| items.as_slice())
            .unwrap_or(&[])
    }

    fn set_items(&mut self, account_id: &str, items: Vec<ReviewItem>) {
        match self.items.iter_mut().find(|(id, _)| id == account_id) {
            Some(entry) => entry.1 = items,
            None => self.items.push((account_id.to_string(), items)),
        }
    }

    fn set_status(&mut self, status: AccountStatus) {
        match self
            .statuses
            .iter_mut()
            .find(|entry| entry.account_id == status.account_id)
        {
            Some(entry) => *entry = status,
            None => self.statuses.push(status),
        }
    }

    fn persist_deck_cache(&self) {
        if let Err(error) = self.vault.persist_deck_cache(self.items.clone()) {
            eprintln!("[deck] could not save the deck cache: {error}");
        }
    }

    /// Fills the deck from the vault before the first fan-out, so the window and the
    /// menu bar open on what the last sync found. Not a sync: the deck still has
    /// nothing to say about whether the accounts are quiet.
    pub fn hydrate(&mut self, cx: &mut Context<Self>) {
        if self.demo {
            return;
        }
        for (account_id, items) in self.vault.load_deck_cache() {
            self.set_items(&account_id, items);
        }
        self.publish(cx);
    }

    /// `pull:detail`: the full pull request, patching the deck's copy of its size
    /// with the numbers the host resolved.
    pub fn load_detail(
        &mut self,
        item_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PullDetail>> {
        let Some(item) = self.find(item_id) else {
            return Task::ready(Err(not_in_deck()));
        };
        if self.demo {
            return Task::ready(Ok(demo_detail(&item)));
        }
        let session = match self.session(&item.account_id) {
            Ok(session) => session,
            Err(error) => return Task::ready(Err(error)),
        };
        let http = self.http.clone();
        let item_id = item_id.to_string();
        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| -> Result<PullDetail> {
                let detail = providers::load_detail(&http, &session, &item).await?;
                let (additions, deletions, changed_files) = (
                    detail.item.additions,
                    detail.item.deletions,
                    detail.item.changed_files,
                );
                this.update(cx, |state, cx| {
                    state.patch(
                        &item_id,
                        |item| {
                            item.additions = additions;
                            item.deletions = deletions;
                            item.changed_files = changed_files;
                        },
                        cx,
                    );
                })
                .ok();
                Ok(detail)
            },
        )
    }

    /// `pull:threads`: the conversation alone, for the poll behind an open pull request.
    pub fn load_threads(
        &mut self,
        item_id: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<CommentThread>>> {
        let Some(item) = self.find(item_id) else {
            return Task::ready(Err(not_in_deck()));
        };
        if self.demo {
            return Task::ready(Ok(demo_detail(&item).threads));
        }
        let session = match self.session(&item.account_id) {
            Ok(session) => session,
            Err(error) => return Task::ready(Err(error)),
        };
        let http = self.http.clone();
        cx.spawn(
            async move |_: WeakEntity<Self>, _: &mut AsyncApp| -> Result<Vec<CommentThread>> {
                providers::load_threads(&http, &session, &item).await
            },
        )
    }

    /// `pull:review`: submits a review with every draft of the item.
    ///
    /// On success the drafts are cleared and the item's review state patched. On a
    /// partial submission the drafts that landed are removed and the rest kept, so a
    /// retry finishes the review rather than repeating the first half of it. Any other
    /// failure leaves the drafts where they were.
    pub fn submit_review(
        &mut self,
        submission: ReviewSubmission,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(item) = self.find(&submission.item_id) else {
            return Task::ready(Err(not_in_deck()));
        };
        let pending = self.drafts.list(&item.id);
        if pending.is_empty()
            && submission.body.trim().is_empty()
            && submission.verdict != ReviewVerdict::Approve
        {
            return Task::ready(Err(msg("There is nothing to submit.")));
        }
        let session = match self.session(&item.account_id) {
            Ok(session) => session,
            Err(error) => return Task::ready(Err(error)),
        };
        let remote = self.remote.clone();
        let verdict = submission.verdict;
        let body = submission.body;
        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| -> Result<()> {
                let result = remote
                    .submit_review(session, item.clone(), verdict, body, pending)
                    .await;
                match this.update(cx, |state, cx| {
                    state.finish_submit(&item, verdict, result, cx)
                }) {
                    Ok(outcome) => outcome,
                    Err(_) => Err(closing()),
                }
            },
        )
    }

    fn finish_submit(
        &mut self,
        item: &ReviewItem,
        verdict: ReviewVerdict,
        result: Result<()>,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        match result {
            Ok(()) => {
                let state = match verdict {
                    ReviewVerdict::Approve => MyReviewState::Approved,
                    ReviewVerdict::RequestChanges => MyReviewState::ChangesRequested,
                    ReviewVerdict::Comment => MyReviewState::Commented,
                };
                // Before clearing, so a sync already in flight cannot report this app's
                // own review as one submitted elsewhere.
                self.drafts.record_submission(&item.id, state);
                // Only now: a submission that failed outright left every draft where it was.
                self.drafts.clear(&item.id);
                let approvals: ApprovalSummary = if state == MyReviewState::Approved {
                    approvals_after_approving(
                        &item.approvals,
                        item.my_review_state == MyReviewState::Approved,
                    )
                } else {
                    item.approvals.clone()
                };
                self.patch(
                    &item.id,
                    |entry| {
                        entry.my_review_state = state;
                        entry.approvals = approvals;
                    },
                    cx,
                );
                self.schedule_draft_save(cx);
                Ok(())
            }
            Err(Error::PartialSubmit { message, posted }) => {
                for id in &posted {
                    self.drafts.remove(id);
                }
                // Part of the review did go out, so what is left starts again from here
                // rather than reading as a review somebody else submitted.
                self.drafts
                    .record_submission(&item.id, item.my_review_state);
                self.schedule_draft_save(cx);
                Err(Error::PartialSubmit { message, posted })
            }
            Err(error) => Err(error),
        }
    }

    /// `pull:comment`: a general comment on the pull request.
    pub fn add_comment(
        &mut self,
        item_id: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(item) = self.find(item_id) else {
            return Task::ready(Err(not_in_deck()));
        };
        if body.trim().is_empty() {
            return Task::ready(Err(msg("The comment is empty.")));
        }
        let session = match self.session(&item.account_id) {
            Ok(session) => session,
            Err(error) => return Task::ready(Err(error)),
        };
        let http = self.http.clone();
        let body = body.to_string();
        cx.spawn(
            async move |_: WeakEntity<Self>, _: &mut AsyncApp| -> Result<()> {
                providers::add_comment(&http, &session, &item, &body).await
            },
        )
    }

    /// `pull:replyToThread`.
    pub fn reply_to_thread(
        &mut self,
        item_id: &str,
        thread_id: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(item) = self.find(item_id) else {
            return Task::ready(Err(not_in_deck()));
        };
        if body.trim().is_empty() {
            return Task::ready(Err(msg("The reply is empty.")));
        }
        // The renderer only offers this where the thread says it can, so reaching here
        // without support is a bug rather than something the user did.
        if !providers::can_reply(item.provider) {
            return Task::ready(Err(msg(format!(
                "{} cannot reply to a thread from here.",
                item.provider.label()
            ))));
        }
        let session = match self.session(&item.account_id) {
            Ok(session) => session,
            Err(error) => return Task::ready(Err(error)),
        };
        let http = self.http.clone();
        let thread_id = thread_id.to_string();
        let body = body.to_string();
        cx.spawn(
            async move |_: WeakEntity<Self>, _: &mut AsyncApp| -> Result<()> {
                providers::reply_to_thread(&http, &session, &item, &thread_id, &body).await
            },
        )
    }

    /// `pull:resolveThread`.
    pub fn set_thread_resolved(
        &mut self,
        item_id: &str,
        thread_id: &str,
        resolved: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(item) = self.find(item_id) else {
            return Task::ready(Err(not_in_deck()));
        };
        if !providers::can_resolve(item.provider) {
            return Task::ready(Err(msg(format!(
                "{} cannot resolve a thread from here.",
                item.provider.label()
            ))));
        }
        let session = match self.session(&item.account_id) {
            Ok(session) => session,
            Err(error) => return Task::ready(Err(error)),
        };
        let http = self.http.clone();
        let thread_id = thread_id.to_string();
        cx.spawn(
            async move |_: WeakEntity<Self>, _: &mut AsyncApp| -> Result<()> {
                providers::set_thread_resolved(&http, &session, &item, &thread_id, resolved).await
            },
        )
    }

    // ----- drafts ---------------------------------------------------------------

    /// `drafts:list`, oldest first.
    pub fn drafts(&self, item_id: &str) -> Vec<DraftComment> {
        self.drafts.list(item_id)
    }

    /// `drafts:add`. Returns the item's drafts after the add.
    pub fn add_draft(
        &mut self,
        draft: NewDraft,
        cx: &mut Context<Self>,
    ) -> Result<Vec<DraftComment>> {
        let item = self.find(&draft.item_id).ok_or_else(not_in_deck)?;
        if draft.body.trim().is_empty() {
            return Err(msg("The comment is empty."));
        }
        let item_id = draft.item_id.clone();
        let body = draft.body.trim().to_string();
        self.drafts
            .add(NewDraft { body, ..draft }, Some(item.my_review_state));
        self.schedule_draft_save(cx);
        Ok(self.drafts.list(&item_id))
    }

    /// `drafts:update`. Returns the item's drafts after the change.
    pub fn update_draft(
        &mut self,
        id: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) -> Result<Vec<DraftComment>> {
        if body.trim().is_empty() {
            return Err(msg("The comment is empty."));
        }
        let updated = self
            .drafts
            .update(id, body.trim())
            .ok_or_else(|| msg("That draft is gone."))?;
        self.schedule_draft_save(cx);
        Ok(self.drafts.list(&updated.item_id))
    }

    /// `drafts:remove`. Returns the item's drafts after the removal.
    pub fn remove_draft(&mut self, id: &str, cx: &mut Context<Self>) -> Vec<DraftComment> {
        let removed = self.drafts.remove(id);
        self.schedule_draft_save(cx);
        match removed {
            Some(draft) => self.drafts.list(&draft.item_id),
            None => Vec::new(),
        }
    }

    /// `drafts:diverged`: a review submitted elsewhere has left these drafts behind.
    pub fn drafts_diverged(&self, item_id: &str) -> bool {
        self.drafts.diverged(item_id)
    }

    /// `drafts:acknowledge`: keep the drafts, drop the mark. Returns whether the item
    /// is still diverged, which after an acknowledgement is false.
    pub fn acknowledge_drafts(&mut self, item_id: &str, cx: &mut Context<Self>) -> bool {
        let state = self
            .find(item_id)
            .map(|item| item.my_review_state)
            .unwrap_or(MyReviewState::Pending);
        self.drafts.acknowledge(item_id, state);
        self.schedule_draft_save(cx);
        self.drafts.diverged(item_id)
    }

    /// `drafts:discard`: only reached through an explicit confirmation in the app.
    pub fn discard_drafts(&mut self, item_id: &str, cx: &mut Context<Self>) -> Vec<DraftComment> {
        self.drafts.clear(item_id);
        self.schedule_draft_save(cx);
        self.drafts.list(item_id)
    }

    /// Restarts the debounce: the drafts are written [`SAVE_DEBOUNCE_MS`] after the
    /// last change. Replacing the stored task cancels the one still waiting.
    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        self.draft_save = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(Duration::from_millis(SAVE_DEBOUNCE_MS))
                    .await;
                this.update(cx, |state, _| state.write_drafts_if_dirty())
                    .ok();
            }),
        );
    }

    fn write_drafts_if_dirty(&mut self) {
        if !self.drafts.take_dirty() {
            return;
        }
        let state = self.drafts.snapshot();
        self.persist_drafts(&state);
    }

    /// Writes the drafts now, whatever the debounce was waiting for. Call on quit.
    pub fn flush_drafts(&mut self) {
        self.draft_save = None;
        let state = self.drafts.flush();
        self.persist_drafts(&state);
    }

    fn persist_drafts(&self, state: &DraftState) {
        if let Err(error) = self
            .vault
            .persist_draft_state(&state.comments, state.sets.iter())
        {
            eprintln!("[drafts] could not save the drafts: {error}");
        }
    }

    // ----- settings -------------------------------------------------------------

    /// `settings:get`.
    pub fn settings(&self) -> Settings {
        self.vault.settings()
    }

    /// `settings:set`. The patch is applied to the stored settings and saved; the side
    /// effects of the fields that changed follow: the sync timers when an interval
    /// changes, the menu bar when something it counts changes, the app appearance and
    /// the views when the theme changes, and the login item.
    ///
    /// The TypeScript ran each side effect whenever its field was present in the
    /// patch. Here a side effect runs only when its value actually changed, so saving
    /// an unrelated setting does not restart the sync timer.
    pub fn set_settings(
        &mut self,
        patch: impl FnOnce(&mut Settings),
        cx: &mut Context<Self>,
    ) -> Result<Settings> {
        let before = self.vault.settings();
        let next = self.vault.save_settings(patch)?;

        if next.poll_interval != before.poll_interval
            || next.check_poll_interval != before.check_poll_interval
        {
            self.reschedule(cx);
        }
        if next.hide_approved != before.hide_approved
            || next.hide_fully_approved != before.hide_fully_approved
            || next.hide_drafts != before.hide_drafts
            || next.show_menu_bar_count != before.show_menu_bar_count
        {
            self.publish(cx);
        }
        // The window's vibrancy and title bar follow the native theme, not the CSS class.
        if next.theme != before.theme {
            appearance::set_app_appearance(next.theme);
            Theme::apply(next.theme, cx);
        }
        if next.launch_at_login != before.launch_at_login
            && let Err(error) = login_item::set_launch_at_login(next.launch_at_login)
        {
            eprintln!("[settings] could not change launch at login: {error}");
        }
        Ok(next)
    }

    // ----- external and platform ------------------------------------------------

    /// `app:openExternal`. Only http(s) reaches the OS: a provider-supplied URL is
    /// untrusted input.
    pub fn open_external(&self, url: &str, cx: &App) {
        let lower = url.trim().to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            cx.open_url(url.trim());
        }
    }

    /// `app:copyText`.
    pub fn copy_text(&self, text: &str, cx: &App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    }

    /// `app:info`.
    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            version: env!("CARGO_PKG_VERSION"),
            platform: if cfg!(target_os = "macos") {
                "darwin"
            } else {
                std::env::consts::OS
            },
        }
    }

    /// An image that needs the account's credential, cached.
    ///
    /// Returns the decoded image once it has landed. On a miss it starts the fetch
    /// and returns `None`; the entity is notified when the image arrives. Failures are
    /// cached, so a broken image is not requested again every frame. A request is
    /// refused unless its host belongs to the account (see
    /// [`images::resolve_image_request`]).
    pub fn authenticated_image(
        &mut self,
        account_id: &str,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        let key = (account_id.to_string(), url.to_string());
        match self.images.get(&key) {
            Some(ImageSlot::Ready(image)) => return Some(image.clone()),
            Some(ImageSlot::Pending | ImageSlot::Failed) => return None,
            None => {}
        }

        let accounts: Vec<ImageAccount> = self
            .vault
            .list_accounts()
            .iter()
            .map(ImageAccount::from)
            .collect();
        let allowed = images::resolve_image_request(account_id, url, &accounts).is_some();
        let account = if allowed {
            self.vault.get_account(account_id)
        } else {
            None
        };
        let token = account
            .as_ref()
            .and_then(|account| self.vault.get_token(&account.id).ok());

        match (account, token) {
            (Some(account), Some(token)) => {
                self.images.insert(key.clone(), ImageSlot::Pending);
                let http = self.http.clone();
                let url = url.to_string();
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    let slot =
                        match images::fetch_authenticated(&http, &account, &token, &url).await {
                            Ok((bytes, content_type)) => {
                                match sniff_image_format(content_type.as_deref(), &bytes) {
                                    Some(format) => {
                                        ImageSlot::Ready(Arc::new(Image::from_bytes(format, bytes)))
                                    }
                                    None => ImageSlot::Failed,
                                }
                            }
                            Err(_) => ImageSlot::Failed,
                        };
                    this.update(cx, |state, cx| {
                        state.images.insert(key, slot);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            _ => {
                self.images.insert(key, ImageSlot::Failed);
            }
        }
        None
    }

    // ----- timers ---------------------------------------------------------------

    /// Starts the sync timer and the window tick, and runs a first sync. Called once
    /// the window and the menu bar exist. Starting again restarts everything.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.stop();
        self.refresh(cx).detach();
        let interval = sync_interval(self.vault.settings().poll_interval);
        self.sync_interval = Some(interval);
        self.sync_timer = Some(spawn_sync_timer(interval, cx));
        self.window_timer = Some(cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(WINDOW_TICK_MS))
                        .await;
                    if this.update(cx, |state, cx| state.tick_windows(cx)).is_err() {
                        break;
                    }
                }
            },
        ));
    }

    /// Cancels every timer. The deck and the drafts stay where they are.
    pub fn stop(&mut self) {
        self.sync_timer = None;
        self.sync_interval = None;
        self.check_timer = None;
        self.window_timer = None;
    }

    /// Quit: stops the timers and writes whatever the debounce still holds.
    pub fn shutdown(&mut self) {
        self.stop();
        self.flush_drafts();
    }

    /// Applies a settings change that affects timing without losing the deck. The
    /// sync timer restarts at the new interval only if it was running.
    pub fn reschedule(&mut self, cx: &mut Context<Self>) {
        if self.sync_timer.is_some() {
            let interval = sync_interval(self.vault.settings().poll_interval);
            self.sync_interval = Some(interval);
            self.sync_timer = Some(spawn_sync_timer(interval, cx));
        }
        self.schedule_check_poll(cx);
    }

    /// The interval the sync timer runs at, or `None` while it is stopped.
    pub fn sync_interval(&self) -> Option<Duration> {
        self.sync_interval
    }

    /// Re-reads CI status while any check is still running, every
    /// `max(15, checkPollInterval)` seconds.
    fn schedule_check_poll(&mut self, cx: &mut Context<Self>) {
        self.check_timer = None;
        if !self
            .items()
            .iter()
            .any(|item| item.checks.status == CheckStatus::Running)
        {
            return;
        }
        let interval = check_interval(self.vault.settings().check_poll_interval);
        self.check_timer = Some(cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor().timer(interval).await;
                this.update(cx, |state, cx| state.refresh_running_checks(cx).detach())
                    .ok();
            },
        ));
    }

    fn refresh_running_checks(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let running: Vec<ReviewItem> = self
            .items()
            .into_iter()
            .filter(|item| item.checks.status == CheckStatus::Running)
            .collect();
        if running.is_empty() {
            return Task::ready(());
        }
        let remote = self.remote.clone();
        let vault = self.vault.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Serial polling would take minutes on a busy deck; four at a time is plenty.
            let refreshed: Vec<(String, CheckSummary)> =
                providers::limit_concurrency(running, 4, |item: ReviewItem| {
                    let remote = remote.clone();
                    let vault = vault.clone();
                    async move {
                        let session = session_for(&vault, &item.account_id).ok()?;
                        let checks = remote.refresh_checks(session, item.clone()).await.ok()?;
                        Some((item.id, checks))
                    }
                })
                .await
                .into_iter()
                .flatten()
                .collect();
            this.update(cx, |state, cx| {
                let mut changed = false;
                for (item_id, checks) in refreshed {
                    changed |= state.patch_quiet(&item_id, |item| item.checks = checks);
                }
                if changed {
                    state.publish(cx);
                }
                state.schedule_check_poll(cx);
            })
            .ok();
        })
    }

    /// The clock tick that opens a review window. Asked twice a minute, and a
    /// fan-out only goes out when a window is genuinely due.
    fn tick_windows(&mut self, cx: &mut Context<Self>) {
        // A fan-out slower than the tick would otherwise let the next tick through
        // while this one is still deciding, and both would fire the same window.
        if self.opening_windows {
            return;
        }
        let settings = self.vault.settings();
        // The master toggle wins over everything in the schedule.
        if !settings.notifications_enabled {
            return;
        }
        if self.windows_due(&settings).is_empty() {
            return;
        }

        self.opening_windows = true;
        let refresh = self.refresh(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            refresh.await.ok();
            this.update(cx, |state, cx| {
                state.opening_windows = false;
                // Asked again against what the refresh brought back, and against the
                // clock as it is now.
                let settings = state.vault.settings();
                let confirmed = state.windows_due(&settings);
                if !confirmed.is_empty() {
                    state.fire_roll_up(&confirmed, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn windows_due(&self, settings: &Settings) -> Vec<ReviewWindow> {
        // No schedule is the common case and has to cost nothing.
        if !settings.review_windows.iter().any(|window| window.enabled) {
            return Vec::new();
        }
        let items = self.items();
        let waiting = visible_reviews(&items, settings);
        let fired = self.vault.load_windows_fired();
        windows_to_fire(&settings.review_windows, self.now_ms(), &fired, &waiting)
            .into_iter()
            .cloned()
            .collect()
    }

    /// One roll-up for however many windows opened on this tick, stating what is
    /// waiting rather than what changed.
    fn fire_roll_up(&mut self, due: &[ReviewWindow], cx: &App) {
        let settings = self.vault.settings();
        // The union of what these windows cover. Anything outside it belongs to a
        // window that has not opened yet.
        let covered: Vec<ReviewItem> = self
            .items()
            .into_iter()
            .filter(|item| {
                due.iter()
                    .any(|window| window_covers(window, &item.account_id))
            })
            .collect();
        let waiting = visible_reviews(&covered, &settings);

        let now = self.now_ms();
        let due_ids: Vec<String> = due.iter().map(|window| window.id.clone()).collect();
        if let Err(error) = self.vault.record_windows_fired(&due_ids, &local_day(now)) {
            eprintln!("[windows] could not record the roll-up: {error}");
        }
        self.mark_seen(&ids_to_record(&covered));

        // Already looking at the deck: the banner would be reading the screen back to
        // them. Recorded as fired all the same.
        if is_window_focused(cx) {
            return;
        }
        let Some(notify) = self.notify.as_ref() else {
            return;
        };
        notify.show(&roll_up_notification(&waiting, settings.play_sound));
    }

    /// Raises notifications for reviews that arrived since the last sync.
    fn announce_new(&mut self) {
        let settings = self.vault.settings();
        // On the first run after launch every item is "new"; do not blast the user.
        let first_sync = !self.primed;
        self.primed = true;

        // Asked per review rather than of the app: a window only silences the accounts
        // it covers.
        let now = self.now_ms();
        let fired = self.vault.load_windows_fired();
        let live: Vec<ReviewItem> = self
            .items()
            .into_iter()
            .filter(|item| {
                announcing_allowed_for(&item.account_id, &settings.review_windows, now, &fired)
            })
            .collect();
        if live.is_empty() {
            return;
        }

        let fresh = self.mark_seen(&ids_to_record(&live));

        if first_sync {
            return;
        }
        if !settings.notifications_enabled || fresh.is_empty() {
            return;
        }
        let Some(notify) = self.notify.as_ref() else {
            return;
        };
        let announced = reviews_to_announce(&live, &fresh, &settings);
        if let Some(notification) = announcement(&announced, settings.play_sound) {
            notify.show(&notification);
        }
    }

    /// Records the ids as seen and returns the ones that were new.
    fn mark_seen(&self, ids: &[String]) -> Vec<String> {
        match self.vault.mark_seen(ids) {
            Ok(fresh) => fresh,
            Err(error) => {
                eprintln!("[deck] could not record what was seen: {error}");
                Vec::new()
            }
        }
    }

    // ----- the menu bar and the views ------------------------------------------

    /// Tells the menu bar and the views that the deck changed.
    fn publish(&mut self, cx: &mut Context<Self>) {
        self.update_tray();
        cx.notify();
    }

    fn update_tray(&mut self) {
        if self.tray.is_none() {
            return;
        }
        let settings = self.vault.settings();
        let items = self.items();
        let account_ids: Vec<String> = self
            .connected_accounts()
            .into_iter()
            .map(|a| a.id)
            .collect();
        let state = tray_state(
            &items,
            &self.statuses,
            &settings,
            &account_ids,
            self.now_ms(),
        );
        if let Some(tray) = self.tray.as_mut() {
            tray.set_title(&tray_title(settings.show_menu_bar_count, state.waiting));
            tray.set_menu(TrayMenu::new(
                state.waiting,
                state.failing,
                state.quiet_until.as_deref(),
            ));
        }
    }

    fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    fn now_iso(&self) -> String {
        format_iso(self.now_ms())
    }
}

// ----- helpers -----------------------------------------------------------------

/// The token and account for `account_id`, as the adapters need them.
pub fn session_for(vault: &Vault, account_id: &str) -> Result<Session> {
    let account = vault
        .get_account(account_id)
        .ok_or_else(|| msg("That account is no longer signed in."))?;
    let token = vault.get_token(account_id)?;
    Ok(Session { account, token })
}

/// Fetches one account's review requests. A failure (or a missing token) becomes the
/// account's error status, not an error of the whole sync.
fn fetch_account(
    remote: Arc<dyn Remote>,
    vault: Arc<Vault>,
    account: Account,
) -> LocalBoxFuture<'static, (String, std::result::Result<Vec<ReviewItem>, String>)> {
    async move {
        let id = account.id.clone();
        let result = match vault.get_token(&id) {
            Ok(token) => {
                remote
                    .list_review_requests(Session { account, token })
                    .await
            }
            Err(error) => Err(error),
        };
        (id, result.map_err(|error| error.to_string()))
    }
    .boxed_local()
}

fn spawn_sync_timer(interval: Duration, cx: &mut Context<AppState>) -> Task<()> {
    cx.spawn(async move |this: WeakEntity<AppState>, cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(interval).await;
            if this
                .update(cx, |state, cx| state.refresh(cx).detach())
                .is_err()
            {
                break;
            }
        }
    })
}

/// The sync interval: `max(30, pollInterval)` seconds.
pub fn sync_interval(poll_interval: u32) -> Duration {
    Duration::from_secs(u64::from(poll_interval.max(30)))
}

/// The check-status poll interval: `max(15, checkPollInterval)` seconds.
pub fn check_interval(check_poll_interval: u32) -> Duration {
    Duration::from_secs(u64::from(check_poll_interval.max(15)))
}

/// What the menu bar shows, worked out from the deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayState {
    /// The reviews the window lists, not every review the deck holds.
    pub waiting: usize,
    /// Accounts whose last sync failed.
    pub failing: usize,
    /// When the app is next able to interrupt, while in a quiet stretch.
    pub quiet_until: Option<String>,
}

/// The menu bar's inputs, from the deck and the schedule at `now` (unix ms).
pub fn tray_state(
    items: &[ReviewItem],
    statuses: &[AccountStatus],
    settings: &Settings,
    account_ids: &[String],
    now: i64,
) -> TrayState {
    let visible = visible_reviews(items, settings);
    let failing = statuses.iter().filter(|status| !status.ok).count();
    // The count keeps climbing through a quiet stretch; with notifications off there is
    // nothing to promise, so no quiet line.
    let quiet_until = if settings.notifications_enabled {
        quiet_until(&settings.review_windows, account_ids, &visible, now)
    } else {
        None
    };
    TrayState {
        waiting: visible.len(),
        failing,
        quiet_until,
    }
}

/// The notification for reviews that arrived since the last sync, or `None` when
/// there is nothing to say. One review names it; several are counted.
pub fn announcement(items: &[&ReviewItem], play_sound: bool) -> Option<Notification> {
    match items {
        [] => None,
        [item] => Some(Notification {
            id: next_notification_id(),
            title: "Review requested".into(),
            subtitle: Some(format!("{} #{}", item.repo, item.number)),
            body: format!("{} - by {}", item.title, item.author.name),
            silent: !play_sound,
            target: Some(item.id.clone()),
        }),
        many => Some(Notification {
            id: next_notification_id(),
            title: format!("{} new review requests", many.len()),
            subtitle: None,
            body: list_of_reviews(many),
            silent: !play_sound,
            target: None,
        }),
    }
}

/// The roll-up for a review window: what is waiting, not what changed.
pub fn roll_up_notification(waiting: &[&ReviewItem], play_sound: bool) -> Notification {
    let count = waiting.len();
    Notification {
        id: next_notification_id(),
        title: format!(
            "{count} review{} waiting",
            if count == 1 { "" } else { "s" }
        ),
        subtitle: None,
        body: list_of_reviews(waiting),
        silent: !play_sound,
        target: None,
    }
}

/// Up to four reviews, one per line, as `repo #number`.
fn list_of_reviews(items: &[&ReviewItem]) -> String {
    items
        .iter()
        .take(4)
        .map(|item| format!("{} #{}", item.repo, item.number))
        .collect::<Vec<_>>()
        .join("\n")
}

fn next_notification_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "cz.mares.reviewdeck.{}.{}",
        now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// The format of an image, from its magic bytes first (what the file is) and its
/// `Content-Type` second (what the host says it is). `None` when neither says.
pub fn sniff_image_format(content_type: Option<&str>, bytes: &[u8]) -> Option<ImageFormat> {
    let by_magic = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else {
        None
    };
    by_magic.or_else(|| {
        let mime = content_type?.split(';').next()?.trim().to_ascii_lowercase();
        match mime.as_str() {
            "image/png" => Some(ImageFormat::Png),
            "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
            "image/gif" => Some(ImageFormat::Gif),
            "image/webp" => Some(ImageFormat::Webp),
            "image/svg+xml" => Some(ImageFormat::Svg),
            "image/bmp" => Some(ImageFormat::Bmp),
            "image/tiff" => Some(ImageFormat::Tiff),
            _ => None,
        }
    })
}

/// Whether the deck is already in front of the user, banner or no banner.
fn is_window_focused(cx: &App) -> bool {
    cx.active_window().is_some()
}

/// A stable identity for a set of accounts, so a change to it can be noticed.
fn signature<'a>(ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut ids: Vec<&str> = ids.into_iter().collect();
    ids.sort_unstable();
    ids.join(" ")
}

fn trimmed_or_none(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn not_in_deck() -> Error {
    msg("That pull request is no longer in the deck.")
}

fn closing() -> Error {
    msg("Reviewdeck is closing.")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use gpui::{AppContext, Entity, TestAppContext};
    use reviewdeck_core::demo::{DEMO_ACCOUNTS, DEMO_ITEMS};
    use reviewdeck_core::drafts::{DraftSet, NewDraft};
    use reviewdeck_core::http::{Http, MockResponse};
    use reviewdeck_core::model::{
        CheckSummary, DiffRefs, MyReviewState, NewAccount, ProviderKind, ReviewWindow, Settings,
        ThemeMode, make_item_id,
    };
    use reviewdeck_core::store::{MemoryTokens, TokenStore};
    use reviewdeck_core::time::local_to_ms;

    use super::*;

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Scripted hosts: what each account returns, which ones fail, and how a review
    /// submission ends.
    #[derive(Default)]
    struct FakeRemote {
        reviews: Mutex<HashMap<String, Vec<ReviewItem>>>,
        failing: Mutex<HashSet<String>>,
        submit: Mutex<Option<Result<()>>>,
    }

    impl FakeRemote {
        fn set_reviews(&self, account_id: &str, items: Vec<ReviewItem>) {
            locked(&self.reviews).insert(account_id.to_string(), items);
        }

        fn set_failing(&self, account_id: &str, failing: bool) {
            let mut set = locked(&self.failing);
            if failing {
                set.insert(account_id.to_string());
            } else {
                set.remove(account_id);
            }
        }

        fn set_submit(&self, outcome: Result<()>) {
            *locked(&self.submit) = Some(outcome);
        }
    }

    impl Remote for FakeRemote {
        fn list_review_requests(
            &self,
            session: Session,
        ) -> LocalBoxFuture<'static, Result<Vec<ReviewItem>>> {
            let answer = if locked(&self.failing).contains(&session.account.id) {
                Err(msg("Could not reach GitHub."))
            } else {
                Ok(locked(&self.reviews)
                    .get(&session.account.id)
                    .cloned()
                    .unwrap_or_default())
            };
            Box::pin(async move { answer })
        }

        fn refresh_checks(
            &self,
            _session: Session,
            _item: ReviewItem,
        ) -> LocalBoxFuture<'static, Result<CheckSummary>> {
            Box::pin(async { Err(msg("not scripted")) })
        }

        fn submit_review(
            &self,
            _session: Session,
            _item: ReviewItem,
            _verdict: ReviewVerdict,
            _body: String,
            _drafts: Vec<DraftComment>,
        ) -> LocalBoxFuture<'static, Result<()>> {
            let answer = locked(&self.submit).clone().unwrap_or(Ok(()));
            Box::pin(async move { answer })
        }
    }

    /// Records what would have been shown.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Notification>>>);

    impl Recorder {
        fn shown(&self) -> Vec<Notification> {
            locked(&self.0).clone()
        }
    }

    impl Notify for Recorder {
        fn show(&self, notification: &Notification) {
            locked(&self.0).push(notification.clone());
        }
    }

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    /// A vault in a directory of its own, with tokens kept in memory.
    fn test_vault() -> Arc<Vault> {
        let dir = std::env::temp_dir().join(format!(
            "reviewdeck-state-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let tokens: Arc<dyn TokenStore> = Arc::new(MemoryTokens::new());
        Arc::new(Vault::open_at(dir.join("reviewdeck.json"), tokens))
    }

    fn memory_tokens() -> Arc<dyn TokenStore> {
        Arc::new(MemoryTokens::new())
    }

    fn add_account(vault: &Vault, label: &str) -> Account {
        let added = vault.add_account(
            NewAccount {
                kind: ProviderKind::Github,
                label: label.into(),
                base_url: "https://api.github.com".into(),
                web_url: "https://github.com".into(),
                username: "octocat".into(),
                display_name: "Octo Cat".into(),
                avatar_url: String::new(),
                agent_command: None,
            },
            "ghp_test",
        );
        match added {
            Ok(account) => account,
            Err(error) => panic!("the fixture vault refused an account: {error}"),
        }
    }

    /// A review on `account_id` numbered `number`, built from the demo fixture.
    fn review(account_id: &str, number: u64) -> ReviewItem {
        let mut item = DEMO_ITEMS[0].clone();
        item.account_id = account_id.to_string();
        item.number = number;
        item.id = make_item_id(account_id, &item.repo_key, number);
        item.title = format!("Review number {number}");
        item
    }

    fn item_id_of(account_id: &str, number: u64) -> String {
        let item = review(account_id, number);
        make_item_id(account_id, &item.repo_key, number)
    }

    fn deps(
        vault: Arc<Vault>,
        remote: Arc<FakeRemote>,
        notify: Option<Box<dyn Notify>>,
    ) -> AppDeps {
        AppDeps {
            http: Http::mock(|_| MockResponse::new(500, Vec::new())),
            vault,
            remote: Some(remote),
            demo: false,
            clock: None,
            notify,
            tray: None,
        }
    }

    /// Runs a task-returning method to completion inside the test.
    fn run<T: 'static>(
        cx: &mut TestAppContext,
        state: &Entity<AppState>,
        call: impl FnOnce(&mut AppState, &mut Context<AppState>) -> Task<Result<T>>,
    ) -> Result<T> {
        let task = state.update(cx, call);
        cx.executor().block_test(task)
    }

    fn draft(item_id: &str, body: &str) -> NewDraft {
        NewDraft {
            item_id: item_id.to_string(),
            body: body.to_string(),
            path: "src/lib.rs".to_string(),
            new_line: Some(4),
            old_line: None,
            range: None,
            refs: DiffRefs::default(),
        }
    }

    #[gpui::test]
    fn first_sync_is_silent_and_a_later_new_review_announces(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&account.id, vec![review(&account.id, 1)]);
        let shown = Recorder::default();
        let state = cx.new(|cx| {
            AppState::new(
                deps(vault.clone(), remote.clone(), Some(Box::new(shown.clone()))),
                cx,
            )
        });

        let deck = run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");
        assert_eq!(deck.items.len(), 1);
        assert!(deck.synced, "the deck has looked at the connected accounts");
        assert!(shown.shown().is_empty(), "the first sync never notifies");

        let arrival = review(&account.id, 2);
        remote.set_reviews(&account.id, vec![review(&account.id, 1), arrival.clone()]);
        run(cx, &state, |s, cx| s.refresh(cx)).expect("the second sync completes");

        let notifications = shown.shown();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title, "Review requested");
        assert_eq!(
            notifications[0].target.as_deref(),
            Some(arrival.id.as_str())
        );
        assert_eq!(
            notifications[0].subtitle.as_deref(),
            Some(format!("{} #2", arrival.repo).as_str())
        );
    }

    #[gpui::test]
    fn a_failing_account_keeps_its_previous_items(cx: &mut TestAppContext) {
        let vault = test_vault();
        let work = add_account(&vault, "Work GitHub");
        let home = add_account(&vault, "Home GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&work.id, vec![review(&work.id, 1)]);
        remote.set_reviews(&home.id, vec![review(&home.id, 7)]);
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));

        run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");

        remote.set_failing(&home.id, true);
        let deck = run(cx, &state, |s, cx| s.refresh(cx)).expect("the second sync completes");

        assert_eq!(
            deck.items.len(),
            2,
            "one host's outage does not empty the deck"
        );
        let failed = deck
            .statuses
            .iter()
            .find(|status| status.account_id == home.id)
            .expect("the failing account has a status");
        assert!(!failed.ok);
        assert_eq!(failed.error.as_deref(), Some("Could not reach GitHub."));
        assert_eq!(failed.count, 1);
        assert!(
            failed.last_synced_at.is_some(),
            "the last good sync is kept"
        );
        let working = deck
            .statuses
            .iter()
            .find(|status| status.account_id == work.id)
            .expect("the working account has a status");
        assert!(working.ok);
    }

    #[gpui::test]
    fn an_account_removed_while_idle_is_pruned_on_refresh(cx: &mut TestAppContext) {
        let vault = test_vault();
        let work = add_account(&vault, "Work GitHub");
        let home = add_account(&vault, "Home GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&work.id, vec![review(&work.id, 1)]);
        remote.set_reviews(&home.id, vec![review(&home.id, 2)]);
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));
        run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");

        if let Err(error) = vault.remove_account(&home.id) {
            panic!("the account is removed: {error}");
        }
        let deck = run(cx, &state, |s, cx| s.refresh(cx)).expect("the second sync completes");

        assert_eq!(deck.items.len(), 1);
        assert!(deck.items.iter().all(|item| item.account_id == work.id));
        assert!(
            deck.statuses
                .iter()
                .all(|status| status.account_id == work.id)
        );
    }

    #[gpui::test]
    fn a_successful_submission_clears_the_drafts(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&account.id, vec![review(&account.id, 1)]);
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));
        run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");
        let item_id = item_id_of(&account.id, 1);

        let added = state.update(cx, |s, cx| s.add_draft(draft(&item_id, "Looks right."), cx));
        assert!(added.is_ok(), "the draft is added");
        remote.set_submit(Ok(()));
        let submitted = run(cx, &state, |s, cx| {
            s.submit_review(
                ReviewSubmission {
                    item_id: item_id.clone(),
                    verdict: ReviewVerdict::Comment,
                    body: String::new(),
                },
                cx,
            )
        });
        assert!(submitted.is_ok(), "the submission succeeds");

        assert!(state.read_with(cx, |s, _| s.drafts(&item_id)).is_empty());
        let item = state.read_with(cx, |s, _| s.find(&item_id));
        assert_eq!(
            item.map(|item| item.my_review_state),
            Some(MyReviewState::Commented)
        );
    }

    #[gpui::test]
    fn a_failed_submission_keeps_every_draft(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&account.id, vec![review(&account.id, 1)]);
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));
        run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");
        let item_id = item_id_of(&account.id, 1);
        let _ = state.update(cx, |s, cx| s.add_draft(draft(&item_id, "One."), cx));

        remote.set_submit(Err(msg("Could not post the review.")));
        let outcome = run(cx, &state, |s, cx| {
            s.submit_review(
                ReviewSubmission {
                    item_id: item_id.clone(),
                    verdict: ReviewVerdict::RequestChanges,
                    body: "Please fix.".into(),
                },
                cx,
            )
        });

        assert_eq!(
            outcome.err().map(|error| error.to_string()).as_deref(),
            Some("Could not post the review.")
        );
        assert_eq!(state.read_with(cx, |s, _| s.drafts(&item_id)).len(), 1);
    }

    #[gpui::test]
    fn a_partial_submission_keeps_only_what_did_not_land(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&account.id, vec![review(&account.id, 1)]);
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));
        run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");
        let item_id = item_id_of(&account.id, 1);
        let landed = state
            .update(cx, |s, cx| s.add_draft(draft(&item_id, "Landed."), cx))
            .ok()
            .and_then(|drafts| drafts.first().map(|draft| draft.id.clone()))
            .unwrap_or_default();
        let _ = state.update(cx, |s, cx| {
            s.add_draft(draft(&item_id, "Did not land."), cx)
        });

        remote.set_submit(Err(Error::PartialSubmit {
            message: "Posted 1 of 2 comments.".into(),
            posted: vec![landed],
        }));
        let outcome = run(cx, &state, |s, cx| {
            s.submit_review(
                ReviewSubmission {
                    item_id: item_id.clone(),
                    verdict: ReviewVerdict::Comment,
                    body: String::new(),
                },
                cx,
            )
        });

        assert!(matches!(outcome, Err(Error::PartialSubmit { .. })));
        let left = state.read_with(cx, |s, _| s.drafts(&item_id));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].body, "Did not land.");
    }

    #[gpui::test]
    fn settings_changes_reschedule_the_running_sync_timer(cx: &mut TestAppContext) {
        let vault = test_vault();
        let remote = Arc::new(FakeRemote::default());
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));

        // Not running: a new interval changes nothing and starts nothing.
        let saved = state.update(cx, |s, cx| {
            s.set_settings(|settings| settings.poll_interval = 60, cx)
        });
        assert!(saved.is_ok());
        assert_eq!(state.read_with(cx, |s, _| s.sync_interval()), None);

        state.update(cx, |s, cx| s.start(cx));
        assert_eq!(
            state.read_with(cx, |s, _| s.sync_interval()),
            Some(Duration::from_secs(60))
        );

        let saved = state.update(cx, |s, cx| {
            s.set_settings(|settings| settings.poll_interval = 45, cx)
        });
        assert!(saved.is_ok());
        assert_eq!(
            state.read_with(cx, |s, _| s.sync_interval()),
            Some(Duration::from_secs(45))
        );

        // The floor is 30 seconds, whatever is saved.
        let saved = state.update(cx, |s, cx| {
            s.set_settings(|settings| settings.poll_interval = 10, cx)
        });
        assert!(saved.is_ok());
        assert_eq!(
            state.read_with(cx, |s, _| s.sync_interval()),
            Some(Duration::from_secs(30))
        );

        state.update(cx, |s, _| s.stop());
        assert_eq!(state.read_with(cx, |s, _| s.sync_interval()), None);
        assert_eq!(vault.settings().theme, ThemeMode::System);
    }

    #[gpui::test]
    fn drafts_are_written_after_the_debounce(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let remote = Arc::new(FakeRemote::default());
        remote.set_reviews(&account.id, vec![review(&account.id, 1)]);
        let state = cx.new(|cx| AppState::new(deps(vault.clone(), remote.clone(), None), cx));
        run(cx, &state, |s, cx| s.refresh(cx)).expect("the first sync completes");
        let item_id = item_id_of(&account.id, 1);

        let _ = state.update(cx, |s, cx| s.add_draft(draft(&item_id, "Saved later."), cx));
        let (before, _): (Vec<DraftComment>, BTreeMap<String, DraftSet>) =
            Vault::open_at(vault.path(), memory_tokens()).load_draft_state();
        assert!(
            before.is_empty(),
            "nothing is written before the debounce passes"
        );

        cx.executor()
            .advance_clock(Duration::from_millis(SAVE_DEBOUNCE_MS + 100));
        cx.run_until_parked();

        let (comments, _): (Vec<DraftComment>, BTreeMap<String, DraftSet>) =
            Vault::open_at(vault.path(), memory_tokens()).load_draft_state();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "Saved later.");
    }

    #[gpui::test]
    fn an_image_for_another_host_is_never_fetched_with_the_token(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let http = Http::mock(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            MockResponse::new(200, Vec::new())
        });
        let state = cx.new(|cx| {
            AppState::new(
                AppDeps {
                    http,
                    vault: vault.clone(),
                    remote: Some(Arc::new(FakeRemote::default())),
                    demo: false,
                    clock: None,
                    notify: None,
                    tray: None,
                },
                cx,
            )
        });

        let image = state.update(cx, |s, cx| {
            s.authenticated_image(&account.id, "https://attacker.example/avatar.png", cx)
        });
        cx.run_until_parked();

        assert!(image.is_none());
        assert_eq!(
            requests.load(Ordering::Relaxed),
            0,
            "no request left the app"
        );
    }

    #[gpui::test]
    fn an_image_from_the_account_host_loads_once_and_is_cached(cx: &mut TestAppContext) {
        let vault = test_vault();
        let account = add_account(&vault, "Work GitHub");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0u8; 24]);
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let http = Http::mock(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            MockResponse::new(200, png.clone()).header("content-type", "image/png")
        });
        let state = cx.new(|cx| {
            AppState::new(
                AppDeps {
                    http,
                    vault: vault.clone(),
                    remote: Some(Arc::new(FakeRemote::default())),
                    demo: false,
                    clock: None,
                    notify: None,
                    tray: None,
                },
                cx,
            )
        });
        let url = "https://github.com/octocat.png";

        let first = state.update(cx, |s, cx| s.authenticated_image(&account.id, url, cx));
        assert!(first.is_none(), "the first request is a miss");
        cx.run_until_parked();
        let second = state.update(cx, |s, cx| s.authenticated_image(&account.id, url, cx));
        let third = state.update(cx, |s, cx| s.authenticated_image(&account.id, url, cx));

        assert!(second.is_some(), "the image is cached once it landed");
        assert!(third.is_some());
        assert_eq!(
            requests.load(Ordering::Relaxed),
            1,
            "one fetch, however often it is asked for"
        );
    }

    #[test]
    fn sync_and_check_intervals_have_their_floors() {
        assert_eq!(sync_interval(0), Duration::from_secs(30));
        assert_eq!(sync_interval(29), Duration::from_secs(30));
        assert_eq!(sync_interval(45), Duration::from_secs(45));
        assert_eq!(check_interval(5), Duration::from_secs(15));
        assert_eq!(check_interval(60), Duration::from_secs(60));
    }

    #[test]
    fn a_single_arrival_is_named_and_several_are_counted() {
        let one = review("acc", 3);
        let single = announcement(&[&one], true);
        let Some(single) = single else {
            panic!("one review is announced");
        };
        assert_eq!(single.title, "Review requested");
        assert_eq!(single.subtitle, Some(format!("{} #3", one.repo)));
        assert_eq!(
            single.body,
            format!("{} - by {}", one.title, one.author.name)
        );
        assert_eq!(single.target, Some(one.id.clone()));
        assert!(!single.silent);

        let items: Vec<ReviewItem> = (1..=6).map(|n| review("acc", n)).collect();
        let refs: Vec<&ReviewItem> = items.iter().collect();
        let many = announcement(&refs, false);
        let Some(many) = many else {
            panic!("several reviews are announced");
        };
        assert_eq!(many.title, "6 new review requests");
        assert_eq!(many.target, None, "a roll-up does not focus one item");
        assert!(many.silent, "silent when sound is off");
        assert_eq!(many.body.lines().count(), 4, "at most four lines");
        assert!(announcement(&[], true).is_none());
    }

    #[test]
    fn the_roll_up_states_what_is_waiting() {
        let items: Vec<ReviewItem> = (1..=2).map(|n| review("acc", n)).collect();
        let refs: Vec<&ReviewItem> = items.iter().collect();
        let roll = roll_up_notification(&refs, true);
        assert_eq!(roll.title, "2 reviews waiting");
        assert_eq!(
            roll.body,
            format!("{} #1\n{} #2", items[0].repo, items[1].repo)
        );
        assert_eq!(roll.target, None);

        let one = review("acc", 9);
        assert_eq!(
            roll_up_notification(&[&one], true).title,
            "1 review waiting"
        );
    }

    #[test]
    fn image_formats_come_from_magic_bytes_before_the_content_type() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        assert_eq!(sniff_image_format(None, png), Some(ImageFormat::Png));
        assert_eq!(
            sniff_image_format(Some("image/jpeg"), png),
            Some(ImageFormat::Png),
            "the bytes are what the file is"
        );
        assert_eq!(
            sniff_image_format(None, &[0xff, 0xd8, 0xff, 0xe0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            sniff_image_format(None, b"GIF89a...."),
            Some(ImageFormat::Gif)
        );
        assert_eq!(
            sniff_image_format(None, b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(
            sniff_image_format(Some("image/svg+xml; charset=utf-8"), b"<svg></svg>"),
            Some(ImageFormat::Svg),
            "text without magic falls back to the content type"
        );
        assert_eq!(sniff_image_format(Some("text/html"), b"<html>"), None);
        assert_eq!(sniff_image_format(None, b""), None);
    }

    #[test]
    fn the_menu_bar_counts_what_the_window_lists_and_names_a_quiet_stretch() {
        let items: Vec<ReviewItem> = DEMO_ITEMS.clone();
        let account_ids: Vec<String> = DEMO_ACCOUNTS.iter().map(|a| a.id.clone()).collect();
        let settings = Settings::default();

        let state = tray_state(&items, &[], &settings, &account_ids, 0);
        assert_eq!(state.waiting, visible_reviews(&items, &settings).len());
        assert_eq!(state.failing, 0);
        assert_eq!(state.quiet_until, None, "no schedule, no quiet line");

        let failing = AccountStatus {
            account_id: account_ids[0].clone(),
            ok: false,
            error: Some("down".into()),
            last_synced_at: None,
            count: 0,
        };
        assert_eq!(
            tray_state(&items, &[failing], &settings, &account_ids, 0).failing,
            1
        );

        // Thursday 2026-10-08 at 03:00 local, with a 09:00-09:30 window that day.
        let Some(thursday) = local_to_ms(2026, 10, 8, 3, 0) else {
            panic!("a local time");
        };
        let scheduled = Settings {
            review_windows: vec![ReviewWindow {
                id: "morning".into(),
                enabled: true,
                days: vec![4],
                start: "09:00".into(),
                end: "09:30".into(),
                minimum: 1,
                accounts: Vec::new(),
            }],
            ..Settings::default()
        };
        let quiet = tray_state(&items, &[], &scheduled, &account_ids, thursday);
        assert_eq!(quiet.quiet_until.as_deref(), Some("09:00"));

        let off = Settings {
            notifications_enabled: false,
            ..scheduled
        };
        assert_eq!(
            tray_state(&items, &[], &off, &account_ids, thursday).quiet_until,
            None
        );
    }

    #[test]
    fn a_session_for_a_missing_account_says_so() {
        let vault = test_vault();
        let error = session_for(&vault, "nope")
            .err()
            .map(|error| error.to_string());
        assert_eq!(
            error.as_deref(),
            Some("That account is no longer signed in.")
        );
    }
}
