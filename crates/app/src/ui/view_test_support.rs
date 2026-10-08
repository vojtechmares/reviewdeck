//! What the interaction tests of the cards and dialogs share: a state over a mock host and
//! a temporary vault, the globals the views read, and a record of what was said in toasts.
//!
//! Included from `thread_view.rs` as `test_support`, so it stays inside the files this
//! area owns.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::LocalBoxFuture;
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use reviewdeck_core::demo::DEMO_ITEMS;
use reviewdeck_core::error::Result;
use reviewdeck_core::http::{Http, MockRequest, MockResponse};
use reviewdeck_core::model::{
    Account, CheckSummary, DraftComment, NewAccount, ProviderKind, ReviewItem, ReviewVerdict,
    ThemeMode, make_item_id,
};
use reviewdeck_core::providers::Session;
use reviewdeck_core::store::{MemoryTokens, TokenStore, Vault};

use crate::state::{AppDeps, AppState, GlobalState, Remote};
use crate::ui::app_view::ToastHost;
use crate::ui::components::toast::{ToastKind, ToastStack};
use crate::ui::theme::Theme;

thread_local! {
    /// Every toast raised through `say` on this thread, oldest first. Each test runs on
    /// its own thread, so tests do not see each other's.
    pub(crate) static SAID: RefCell<Vec<(ToastKind, String)>> = const { RefCell::new(Vec::new()) };
}

/// The toasts raised so far, and forgets them.
pub(crate) fn take_said() -> Vec<(ToastKind, String)> {
    SAID.with(|said| std::mem::take(&mut *said.borrow_mut()))
}

/// A host that lists a fixed set of reviews.
#[derive(Default)]
pub(crate) struct FixedRemote {
    pub(crate) items: Mutex<Vec<ReviewItem>>,
}

impl Remote for FixedRemote {
    fn list_review_requests(
        &self,
        _session: Session,
    ) -> LocalBoxFuture<'static, Result<Vec<ReviewItem>>> {
        let items = self
            .items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Box::pin(async move { Ok(items) })
    }

    fn refresh_checks(
        &self,
        _session: Session,
        item: ReviewItem,
    ) -> LocalBoxFuture<'static, Result<CheckSummary>> {
        Box::pin(async move { Ok(item.checks) })
    }

    fn submit_review(
        &self,
        _session: Session,
        _item: ReviewItem,
        _verdict: ReviewVerdict,
        _body: String,
        _drafts: Vec<DraftComment>,
    ) -> LocalBoxFuture<'static, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

pub(crate) struct Env {
    pub(crate) state: Entity<AppState>,
    pub(crate) vault: Arc<Vault>,
    pub(crate) remote: Arc<FixedRemote>,
    /// Every request the mock host saw.
    pub(crate) requests: Arc<Mutex<Vec<MockRequest>>>,
}

impl Env {
    /// Method, URL and body of the requests so far, oldest first.
    pub(crate) fn seen(&self) -> Vec<(String, Option<String>)> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|request| (request.url.clone(), request.body.clone()))
            .collect()
    }

    /// How many requests went to a URL that contains `part`.
    pub(crate) fn count(&self, part: &str) -> usize {
        self.seen()
            .iter()
            .filter(|(url, _)| url.contains(part))
            .count()
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A state over a temporary vault and a mock host, installed as the globals the views
/// read. `host` answers every request the state makes.
pub(crate) fn boot(
    cx: &mut TestAppContext,
    host: impl Fn(&MockRequest) -> MockResponse + Send + Sync + 'static,
) -> Env {
    let dir = std::env::temp_dir().join(format!(
        "reviewdeck-views-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let tokens: Arc<dyn TokenStore> = Arc::new(MemoryTokens::new());
    let vault = Arc::new(Vault::open_at(dir.join("reviewdeck.json"), tokens));
    let remote = Arc::new(FixedRemote::default());
    let requests: Arc<Mutex<Vec<MockRequest>>> = Arc::default();
    let seen = requests.clone();
    let http = Http::mock(move |request| {
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        host(request)
    });
    let state = cx.new(|cx| {
        AppState::new(
            AppDeps {
                http,
                vault: vault.clone(),
                remote: Some(remote.clone()),
                demo: false,
                clock: None,
                notify: None,
                tray: None,
            },
            cx,
        )
    });
    take_said();
    let toasts = cx.new(|_| ToastStack::new());
    cx.update(|cx| {
        cx.set_global(GlobalState(state.clone()));
        cx.set_global(ToastHost(toasts.clone()));
        Theme::apply(ThemeMode::Light, cx);
        crate::ui::components::bind_keys(cx);
    });
    Env {
        state,
        vault,
        remote,
        requests,
    }
}

/// Signs a GitHub account into the vault, as the dialog would have.
pub(crate) fn sign_in(env: &Env, label: &str) -> Account {
    let added = env.vault.add_account(
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

/// A review in the deck: signed in, listed by the remote and synced.
pub(crate) fn deck_with_review(cx: &mut TestAppContext, env: &Env) -> ReviewItem {
    let account = sign_in(env, "Work GitHub");
    let mut item = DEMO_ITEMS[0].clone();
    item.account_id = account.id.clone();
    item.number = 7;
    item.id = make_item_id(&account.id, &item.repo_key, 7);
    env.remote
        .items
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(item.clone());
    let task = env.state.update(cx, |state, cx| state.refresh(cx));
    cx.executor().block_test(task).expect("the sync succeeds");
    item
}

/// Clicks the middle of the element that was tagged with `probe`. gpui never forgets a
/// selector once drawn, so absence cannot be asserted through `debug_bounds`: assert on
/// the card's state instead.
pub(crate) fn click(cx: &mut VisualTestContext, tag: &str) {
    cx.run_until_parked();
    let tag: &'static str = Box::leak(tag.to_string().into_boxed_str());
    let bounds = cx
        .debug_bounds(tag)
        .unwrap_or_else(|| panic!("nothing is drawn under {tag:?}"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// The root the app gives a dialog: a window-sized, positioned box with the dialog as its
/// last child, so the dialog's scrim and panel fill the window.
struct Host<V: gpui::Render>(Entity<V>);

impl<V: gpui::Render> gpui::Render for Host<V> {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled, div};
        div().size_full().relative().child(self.0.clone())
    }
}

/// Opens a dialog (or any view) the way `AppView` hosts one, in a window tall enough that
/// nothing needs scrolling to be clicked.
pub(crate) fn open_dialog<V: gpui::Render + 'static>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut gpui::Window, &mut gpui::Context<V>) -> V,
) -> (Entity<V>, &mut VisualTestContext) {
    let mut built = None;
    let (_host, vcx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| build(window, cx));
        built = Some(view.clone());
        Host(view)
    });
    vcx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(2600.)));
    vcx.run_until_parked();
    (built.expect("the view was built"), vcx)
}
