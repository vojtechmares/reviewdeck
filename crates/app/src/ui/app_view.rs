//! Port of src/renderer/src/App.tsx: the header, the filters, the deck list, the empty
//! states, the selected pull request and the dialogs.
//!
//! What React derived on every render is derived here when something changes instead
//! ([`AppView::reload`] on a state notification, [`AppView::recompute`] when a filter or
//! the selection moves), and `render` only reads the result. The deck list is a gpui
//! `list`, so only the visible cards are built however long the deck is.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, AppContext, ClickEvent, Context,
    DismissEvent, Entity, FocusHandle, Focusable, Global, InteractiveElement, IntoElement,
    KeyBinding, ListAlignment, ListState, MouseButton, MouseDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, StyleRefinement, Styled, Subscription, Task, Timer,
    Window, actions, div, list, prelude::FluentBuilder, px, relative,
};
use reviewdeck_core::model::{
    Account, AccountStatus, CheckStatus, DeckCounts, DeckEmptyState, ReviewItem, Settings,
    deck_empty_state, is_visible_review,
};
use reviewdeck_core::time::{now_ms, relative_time};

use crate::platform::window_drag;
use crate::state::{AppEvent, AppState, GlobalState};
use crate::ui::accounts_dialog::AccountsDialog;
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant, with_alpha};
use crate::ui::components::input::{TextInput, TextInputEvent};
use crate::ui::components::scroll::thumb;
use crate::ui::components::select::{Select, SelectEvent, SelectOption};
use crate::ui::components::toast::{ToastKind, ToastStack};
use crate::ui::components::{FocusNext, FocusPrev};
use crate::ui::icons::{Icon, IconName};
use crate::ui::pull_view::PullView;
use crate::ui::review_card::ReviewCard;
use crate::ui::schedule_dialog::ScheduleDialog;
use crate::ui::settings_dialog::{DialogEvent, DialogKind, SettingsDialog};
use crate::ui::theme::{ActiveTheme, UI_FONT, rpx};

actions!(app_view, [SelectNext, SelectPrev, FocusSearch]);

/// Key bindings the deck needs. Call once at startup, before `cx.set_menus`.
///
/// j/k and the arrows move the selection except while a text field has focus (the TSX's
/// `typing` check: `!TextInput` is matched against the whole context stack, so any
/// field, wherever it is, keeps its keys). Cmd-F opens the filters and focuses the
/// search field. Cmd-R is the menu's Refresh action, bound in main.rs.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some("AppView && !TextInput && !Select")),
        KeyBinding::new("down", SelectNext, Some("AppView && !TextInput && !Select")),
        KeyBinding::new("k", SelectPrev, Some("AppView && !TextInput && !Select")),
        KeyBinding::new("up", SelectPrev, Some("AppView && !TextInput && !Select")),
        KeyBinding::new("cmd-f", FocusSearch, Some("AppView")),
    ]);
}

/// The toast stack the root view owns, reachable from any view.
pub struct ToastHost(pub Entity<ToastStack>);

impl Global for ToastHost {}

/// Shows a toast. Does nothing before the root view exists.
pub fn toast(cx: &mut App, kind: ToastKind, message: impl Into<SharedString>) {
    let Some(host) = cx.try_global::<ToastHost>() else {
        return;
    };
    let stack = host.0.clone();
    let message = message.into();
    stack.update(cx, |stack, cx| stack.push(kind, message, cx));
}

/// `Filters` in the TSX.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Filters {
    query: String,
    /// An account id, or `all`.
    account: Option<String>,
    checks: Option<CheckStatus>,
}

impl Filters {
    fn active(&self) -> bool {
        !self.query.is_empty() || self.account.is_some() || self.checks.is_some()
    }
}

/// `applyFilters`.
fn apply_filters(
    items: &[Arc<ReviewItem>],
    filters: &Filters,
    settings: &Settings,
) -> Vec<Arc<ReviewItem>> {
    let needle = filters.query.trim().to_lowercase();
    items
        .iter()
        .filter(|item| {
            if !is_visible_review(item, settings) {
                return false;
            }
            if filters
                .account
                .as_ref()
                .is_some_and(|account| *account != item.account_id)
            {
                return false;
            }
            if filters
                .checks
                .is_some_and(|status| item.checks.status != status)
            {
                return false;
            }
            if needle.is_empty() {
                return true;
            }
            item.title.to_lowercase().contains(&needle)
                || item.repo.to_lowercase().contains(&needle)
                || item.author.name.to_lowercase().contains(&needle)
                || item.number.to_string().contains(&needle)
        })
        .cloned()
        .collect()
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Gives `element` a name the interaction tests can find its bounds by. The wrapper
/// exists only in test builds; in the app the element is returned as it is.
#[cfg(test)]
fn tag(name: impl Into<String>, element: impl IntoElement) -> AnyElement {
    let name = name.into();
    div()
        .debug_selector(move || name.clone())
        .child(element)
        .into_any_element()
}

#[cfg(not(test))]
fn tag(_name: impl Into<String>, element: impl IntoElement) -> AnyElement {
    element.into_any_element()
}

/// The one dialog that is open. One modal at a time, as in the TSX: the schedule
/// replaces settings rather than stacking on it, so there is only ever one focus trap
/// and one Escape to answer.
enum OpenDialog {
    Accounts(Entity<AccountsDialog>),
    Settings(Entity<SettingsDialog>),
    Schedule(Entity<ScheduleDialog>),
}

impl OpenDialog {
    fn view(&self) -> AnyElement {
        match self {
            OpenDialog::Accounts(view) => view.clone().into_any_element(),
            OpenDialog::Settings(view) => view.clone().into_any_element(),
            OpenDialog::Schedule(view) => view.clone().into_any_element(),
        }
    }
}

/// The header's "Syncing... / Updated 3m ago / Not synced yet".
///
/// Its own entity so the clock can tick every half minute without re-rendering the
/// rest of the window (a state notification re-runs `render` from the root down).
/// While syncing, the ellipsis fills in a dot at a time so the label reads as live; it
/// only exists while a sync is in flight, so it stops by going away.
struct SyncLabel {
    syncing: bool,
    last_synced_at: Option<String>,
    _tick: Task<()>,
}

impl SyncLabel {
    fn new(cx: &mut Context<Self>) -> SyncLabel {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(30)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        SyncLabel {
            syncing: false,
            last_synced_at: None,
            _tick: tick,
        }
    }

    fn set(&mut self, syncing: bool, last_synced_at: Option<String>, cx: &mut Context<Self>) {
        if self.syncing != syncing || self.last_synced_at != last_synced_at {
            self.syncing = syncing;
            self.last_synced_at = last_synced_at;
            cx.notify();
        }
    }
}

impl Render for SyncLabel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().colors.muted_foreground;
        let label = div()
            .min_w_0()
            .truncate()
            .text_size(rpx(11.5))
            .line_height(rpx(17.25))
            .text_color(muted);
        #[cfg(test)]
        let label = label.debug_selector({
            let text = if self.syncing {
                "Syncing".to_string()
            } else if let Some(at) = &self.last_synced_at {
                format!("Updated {}", relative_time(at, now_ms()))
            } else {
                "Not synced yet".to_string()
            };
            move || format!("sync-label:{text}")
        });
        if self.syncing {
            label
                .flex()
                .child("Syncing")
                .children((0..3usize).map(|ix| {
                    div().child(".").with_animation(
                        ("sync-dot", ix),
                        Animation::new(Duration::from_millis(1200)).repeat(),
                        move |dot, delta| {
                            let phase = (delta + 1.0 - ix as f32 * 0.2) % 1.0;
                            dot.opacity(0.2 + 0.8 * (1.0 - (2.0 * phase - 1.0).abs()))
                        },
                    )
                }))
        } else if let Some(at) = &self.last_synced_at {
            label.child(format!("Updated {}", relative_time(at, now_ms())))
        } else {
            label.child("Not synced yet")
        }
    }
}

/// The root of the window: App.tsx.
pub struct AppView {
    state: Entity<AppState>,
    focus: FocusHandle,
    /// Kept alive for the life of the view.
    _subscriptions: Vec<Subscription>,
    dialog_subscriptions: Vec<Subscription>,

    // What the deck state looked like at the last notification.
    accounts: Vec<Account>,
    all_items: Vec<Arc<ReviewItem>>,
    statuses: Vec<AccountStatus>,
    synced: bool,
    syncing: bool,
    settings: Settings,
    /// The last value of `settings.hide_drafts` the reveal effect saw.
    seen_hide_drafts: Option<bool>,

    // What the view itself decides.
    filters: Filters,
    show_filters: bool,
    reveal_drafts: bool,
    selected_id: Option<String>,

    // Derived by `recompute`.
    items: Vec<Arc<ReviewItem>>,
    hidden_drafts: usize,
    empty: Option<DeckEmptyState>,

    list_state: ListState,
    last_rem: gpui::Pixels,
    pull: Option<(String, Entity<PullView>)>,

    search: Entity<TextInput>,
    account_select: Entity<Select>,
    checks_select: Entity<Select>,
    sync_label: Entity<SyncLabel>,
    dialog: Option<OpenDialog>,
    toasts: Entity<ToastStack>,
    /// The click counts of the presses the header's drag strip received.
    #[cfg(test)]
    header_presses: Vec<usize>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> AppView {
        let state = cx.global::<GlobalState>().0.clone();
        let focus = cx.focus_handle();

        let toasts = cx.new(|_| ToastStack::new());
        cx.set_global(ToastHost(toasts.clone()));

        // `h-8 pl-7.5`: room on the left for the magnifier.
        let search = cx.new(|cx| {
            TextInput::new(cx)
                .placeholder("Filter by title, repo or author…")
                .height(32.)
                .padding_left(30.)
        });
        // `h-7 text-[11.5px]`, the compact select.
        let account_select = cx.new(|cx| {
            Select::new(vec![SelectOption::new("all", "All accounts")], "all", cx).compact()
        });
        let checks_select = cx.new(|cx| {
            Select::new(
                vec![
                    SelectOption::new("all", "Any check status"),
                    SelectOption::new("passed", "Checks passed"),
                    SelectOption::new("failed", "Checks failed"),
                    SelectOption::new("running", "Checks running"),
                    SelectOption::new("unknown", "No checks"),
                ],
                "all",
                cx,
            )
            .compact()
        });
        let sync_label = cx.new(SyncLabel::new);

        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.reload(cx)),
            // Clicking a notification (or the Settings menu item) routes through here.
            cx.subscribe_in(
                &state,
                window,
                |this, _, event: &AppEvent, window, cx| match event {
                    AppEvent::FocusItem(id) if id == "__settings__" => {
                        this.open_dialog(DialogKind::Settings, window, cx)
                    }
                    AppEvent::FocusItem(id) => this.select(Some(id.clone()), cx),
                    AppEvent::OpenSettings => this.open_dialog(DialogKind::Settings, window, cx),
                },
            ),
            cx.subscribe_in(
                &search,
                window,
                |this, input, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Changed => {
                        this.filters.query = input.read(cx).text().to_string();
                        this.recompute(cx);
                    }
                    TextInputEvent::Cancel => {
                        this.show_filters = false;
                        window.focus(&this.focus);
                        cx.notify();
                    }
                    TextInputEvent::Submit => {}
                },
            ),
            cx.subscribe(
                &account_select,
                |this, _, SelectEvent::Changed(value): &SelectEvent, cx| {
                    this.filters.account = (value.as_ref() != "all").then(|| value.to_string());
                    this.recompute(cx);
                },
            ),
            cx.subscribe(
                &checks_select,
                |this, _, SelectEvent::Changed(value): &SelectEvent, cx| {
                    this.filters.checks = match value.as_ref() {
                        "passed" => Some(CheckStatus::Passed),
                        "failed" => Some(CheckStatus::Failed),
                        "running" => Some(CheckStatus::Running),
                        "unknown" => Some(CheckStatus::Unknown),
                        _ => None,
                    };
                    this.recompute(cx);
                },
            ),
        ];

        // Keys only reach the deck when something inside it has focus.
        window.focus(&focus);

        let mut view = AppView {
            state,
            focus,
            _subscriptions: subscriptions,
            dialog_subscriptions: Vec::new(),
            accounts: Vec::new(),
            all_items: Vec::new(),
            statuses: Vec::new(),
            synced: false,
            syncing: false,
            settings: Settings::default(),
            seen_hide_drafts: None,
            filters: Filters::default(),
            show_filters: false,
            reveal_drafts: false,
            selected_id: None,
            items: Vec::new(),
            hidden_drafts: 0,
            empty: None,
            list_state: ListState::new(0, ListAlignment::Top, px(400.)),
            last_rem: window.rem_size(),
            pull: None,
            search,
            account_select,
            checks_select,
            sync_label,
            dialog: None,
            toasts,
            #[cfg(test)]
            header_presses: Vec::new(),
        };
        view.reload(cx);
        view
    }

    fn account_for(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|account| account.id == id)
    }

    /// Reads the app state after a notification and derives everything from it.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let (accounts, deck, settings) = {
            let state = self.state.read(cx);
            (state.accounts(), state.deck(), state.settings())
        };

        if accounts != self.accounts {
            let mut options = vec![SelectOption::new("all", "All accounts")];
            options.extend(
                accounts
                    .iter()
                    .map(|account| SelectOption::new(account.id.clone(), account.label.clone())),
            );
            self.account_select
                .update(cx, |select, cx| select.set_options(options, cx));
            self.accounts = accounts;
        }
        self.statuses = deck.statuses;
        self.synced = deck.synced;
        self.sync_label.update(cx, |label, cx| {
            label.set(deck.syncing, deck.last_synced_at.clone(), cx)
        });
        self.syncing = deck.syncing;

        // Turning the preference back on is a decision about the deck itself, so it wins
        // over a reveal somebody left switched on earlier in the session.
        if self.seen_hide_drafts != Some(settings.hide_drafts) {
            self.seen_hide_drafts = Some(settings.hide_drafts);
            if settings.hide_drafts {
                self.reveal_drafts = false;
            }
        }
        self.settings = settings;

        let unchanged = self.all_items.len() == deck.items.len()
            && self
                .all_items
                .iter()
                .zip(&deck.items)
                .all(|(old, new)| **old == *new);
        if !unchanged {
            self.all_items = deck.items.into_iter().map(Arc::new).collect();
        }
        self.recompute(cx);
    }

    /// `shown`, `withDrafts`, `hiddenDrafts`, `items`, `empty` and the selection effect.
    fn recompute(&mut self, cx: &mut Context<Self>) {
        let shown = apply_filters(&self.all_items, &self.filters, &self.settings);
        // The same view with the drafts preference lifted, so the deck can say how many
        // it is holding back and show them when asked. Deliberately view-only: the
        // setting is the standing rule the menu bar counts by, this is a look somebody
        // is taking by hand, and it belongs with the query box rather than with the rule.
        let with_drafts = if self.settings.hide_drafts {
            let lifted = Settings {
                hide_drafts: false,
                ..self.settings.clone()
            };
            apply_filters(&self.all_items, &self.filters, &lifted)
        } else {
            shown.clone()
        };
        self.hidden_drafts = with_drafts.len() - shown.len();
        let items = if self.reveal_drafts {
            with_drafts
        } else {
            shown
        };

        let changed = self.items.len() != items.len()
            || self
                .items
                .iter()
                .zip(&items)
                .any(|(old, new)| **old != **new);
        if changed {
            let old = self.items.len();
            self.items = items;
            // `splice` and not `reset`: the scroll position stays where it was.
            self.list_state.splice(0..old, self.items.len());
        }

        // Keep a valid selection as the deck changes underneath us.
        let selection_valid = self
            .selected_id
            .as_ref()
            .is_some_and(|id| self.items.iter().any(|item| item.id == *id));
        if !selection_valid {
            self.selected_id = self.items.first().map(|item| item.id.clone());
            self.reveal_selected();
        }

        self.empty = deck_empty_state(DeckCounts {
            account_count: self.accounts.len(),
            synced: self.synced,
            visible_count: self.items.len(),
            hidden_draft_count: self.hidden_drafts,
            filters_active: self.filters.active(),
        });
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected_id.as_ref()?;
        self.items.iter().position(|item| item.id == *id)
    }

    /// j/k moves the selection without touching the scroll position, so follow it.
    fn reveal_selected(&self) {
        if let Some(ix) = self.selected_index() {
            self.list_state.scroll_to_reveal_item(ix);
        }
    }

    /// `setSelectedId`. An id that is not in the list is replaced by the first item on
    /// the next `recompute`, as the TSX's effect did.
    fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.selected_id == id {
            return;
        }
        // Heights depend on the weight of the title, so the two cards are measured again.
        for ix in [self.selected_index(), {
            id.as_ref()
                .and_then(|id| self.items.iter().position(|item| item.id == *id))
        }]
        .into_iter()
        .flatten()
        {
            self.list_state.splice(ix..ix + 1, 1);
        }
        self.selected_id = id;
        self.recompute(cx);
        self.reveal_selected();
    }

    /// `move`.
    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.items.is_empty() || self.dialog.is_some() {
            return;
        }
        let index = self.selected_index().unwrap_or(0) as isize;
        let next = (index + delta).clamp(0, self.items.len() as isize - 1) as usize;
        let id = self.items[next].id.clone();
        self.select(Some(id), cx);
        self.keep_focus(window);
    }

    /// Puts focus back on the deck itself. A card that was tabbed to or clicked holds
    /// focus, and the list builds only the cards in view: once it scrolls away, the
    /// focus would point at nothing and j/k would stop reaching the view.
    fn keep_focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// A press on the header's empty strip: a double-click does what the system says a
    /// title bar double-click does (`AppleActionOnDoubleClick`: zoom, minimise, fill
    /// or nothing), anything else starts dragging the window.
    fn header_pressed(&mut self, event: &MouseDownEvent, window: &mut Window) {
        #[cfg(test)]
        {
            self.header_presses.push(event.click_count);
        }
        if event.click_count >= 2 {
            window.titlebar_double_click();
        } else {
            window_drag::begin_window_drag();
        }
    }

    fn clear_filters(&mut self, cx: &mut Context<Self>) {
        self.filters = Filters::default();
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.account_select
            .update(cx, |select, cx| select.set_selected("all", cx));
        self.checks_select
            .update(cx, |select, cx| select.set_selected("all", cx));
        self.recompute(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.refresh(cx).detach());
    }

    fn open_dialog(&mut self, kind: DialogKind, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog_subscriptions.clear();
        match kind {
            DialogKind::Accounts => {
                let dialog = cx.new(|cx| AccountsDialog::new(window, cx));
                self.dialog_subscriptions.push(cx.subscribe_in(
                    &dialog,
                    window,
                    |this, _, _: &DismissEvent, window, cx| this.close_dialog(window, cx),
                ));
                window.focus(&dialog.focus_handle(cx));
                self.dialog = Some(OpenDialog::Accounts(dialog));
            }
            DialogKind::Settings => {
                let dialog = cx.new(|cx| SettingsDialog::new(window, cx));
                self.dialog_subscriptions.push(cx.subscribe_in(
                    &dialog,
                    window,
                    |this, _, _: &DismissEvent, window, cx| this.close_dialog(window, cx),
                ));
                self.dialog_subscriptions.push(cx.subscribe_in(
                    &dialog,
                    window,
                    |this, _, event: &DialogEvent, window, cx| match event {
                        DialogEvent::Open(kind) => this.open_dialog(*kind, window, cx),
                    },
                ));
                window.focus(&dialog.focus_handle(cx));
                self.dialog = Some(OpenDialog::Settings(dialog));
            }
            DialogKind::Schedule => {
                let dialog = cx.new(|cx| ScheduleDialog::new(window, cx));
                self.dialog_subscriptions.push(cx.subscribe_in(
                    &dialog,
                    window,
                    |this, _, _: &DismissEvent, window, cx| this.close_dialog(window, cx),
                ));
                window.focus(&dialog.focus_handle(cx));
                self.dialog = Some(OpenDialog::Schedule(dialog));
            }
        }
        cx.notify();
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.dialog_subscriptions.clear();
        window.focus(&self.focus);
        cx.notify();
    }

    /// Cmd-F: show the filters and put the caret in the search field.
    /// The deck-level steps of `REVIEWDECK_SCENE`, each through the method its click
    /// handler calls. Debug builds only.
    #[cfg(debug_assertions)]
    fn apply_scene(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::scene::{self, Kind, SceneDialog, Step};

        // The "N drafts hidden" footer link.
        if scene::pending(cx, Kind::RevealDrafts).is_some() && self.synced {
            scene::mark_done(cx, Kind::RevealDrafts);
            if self.hidden_drafts > 0 && !self.reveal_drafts {
                self.reveal_drafts = true;
                self.recompute(cx);
            }
        }
        // A click on the n-th card, once the deck has it.
        if scene::pending(cx, Kind::RevealDrafts).is_none()
            && let Some(Step::Select(n)) = scene::pending(cx, Kind::Select)
            && self.synced
        {
            scene::mark_done(cx, Kind::Select);
            match self.items.get(n).map(|item| item.id.clone()) {
                Some(id) => {
                    self.select(Some(id), cx);
                    self.keep_focus(window);
                }
                None => eprintln!("REVIEWDECK_SCENE: no card {n} in the deck"),
            }
        }
        if scene::pending(cx, Kind::Filters).is_some() {
            scene::mark_done(cx, Kind::Filters);
            self.show_filters = true;
            cx.notify();
        }
        if !scene::deck_pending(cx)
            && self.synced
            && let Some(Step::Dialog(dialog)) = scene::pending(cx, Kind::Dialog)
        {
            scene::mark_done(cx, Kind::Dialog);
            match dialog {
                SceneDialog::Accounts => self.open_dialog(DialogKind::Accounts, window, cx),
                SceneDialog::AccountsAdd => {
                    self.open_dialog(DialogKind::Accounts, window, cx);
                    if let Some(OpenDialog::Accounts(dialog)) = &self.dialog {
                        dialog.update(cx, |dialog, cx| dialog.start_add(cx));
                    }
                }
                SceneDialog::Settings => self.open_dialog(DialogKind::Settings, window, cx),
                SceneDialog::Schedule => self.open_dialog(DialogKind::Schedule, window, cx),
            }
        }
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.show_filters = true;
        // The field mounts in the frame this notification draws, and focus is a handle
        // the window holds rather than something the element tree has to contain yet,
        // so there is nothing to wait for (the TSX waited a frame for React to mount it).
        self.search.read(cx).focus(window);
        cx.notify();
    }

    /// The pull request pane is a new view whenever the selected item changes
    /// (`key={selected.id}`).
    fn ensure_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let selected = self.selected_id.clone()?;
        if self.pull.as_ref().map(|(id, _)| id) != Some(&selected) {
            let id = selected.clone();
            let view = cx.new(|cx| PullView::new(id, window, cx));
            self.pull = Some((selected, view));
        }
        let (_, view) = self.pull.as_ref()?;
        Some(AnyView::from(view.clone()).cached(StyleRefinement::default().size_full()))
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(ix).cloned() else {
            return div().into_any_element();
        };
        let id = item.id.clone();
        let label = self
            .account_for(&item.account_id)
            .map(|account| SharedString::from(account.label.clone()));
        let selected = self.selected_id.as_deref() == Some(item.id.as_str());
        div()
            .pb(rpx(2.))
            .child(ReviewCard::new(
                item,
                label,
                selected,
                cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.select(Some(id.clone()), cx);
                    // A click leaves focus on the deck (see `keep_focus`); Enter on a
                    // tabbed-to card leaves it there, so Tab carries on from that card.
                    if event.mouse_position().is_some() {
                        this.keep_focus(window);
                    }
                }),
            ))
            .into_any_element()
    }

    /// The text of the failing-account banner's tooltip, one block per account, or
    /// `None` when every account synced.
    fn sync_problems(&self) -> Option<String> {
        let text = self
            .statuses
            .iter()
            .filter(|status| !status.ok)
            .map(|status| {
                let label = self
                    .account_for(&status.account_id)
                    .map(|account| account.label.as_str())
                    .unwrap_or("Account");
                format!(
                    "{label}: {}",
                    status.error.as_deref().unwrap_or("Sync failed")
                )
            })
            .collect::<Vec<_>>();
        (!text.is_empty()).then(|| text.join("\n\n"))
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let filters_active = self.filters.active();
        let syncing = self.syncing;

        let failing_button = self.sync_problems().map(|text| {
            tag(
                "sync-problems",
                Button::new("sync-problems")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Icon)
                    .tooltip(text)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_dialog(DialogKind::Accounts, window, cx)
                    }))
                    .child(icon_child_in(IconName::TriangleAlert, colors.bad)),
            )
        });

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(rpx(8.))
            .h(rpx(52.))
            // Room for the traffic lights, which sit over the transparent title bar.
            .pl(rpx(88.))
            .pr(rpx(12.))
            .border_b_1()
            .border_color(colors.border)
            // The strip under the traffic lights the whole window can be dragged by
            // (`-webkit-app-region: drag`), and double-clicked like a title bar.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, _| {
                    this.header_pressed(event, window)
                }),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_baseline()
                    .gap(rpx(8.))
                    .child(
                        div()
                            .text_size(rpx(13.5))
                            .line_height(rpx(20.25))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Reviewdeck"),
                    )
                    .child(self.sync_label.clone()),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(rpx(4.))
                    // `no-drag`: a press on a button is a click, not the start of a drag.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(failing_button)
                    .child(tag(
                        "filter",
                        Button::new("filter")
                            .variant(if self.show_filters || filters_active {
                                ButtonVariant::Subtle
                            } else {
                                ButtonVariant::Ghost
                            })
                            .size(ButtonSize::Icon)
                            .child(icon_child_in(
                                IconName::Filter,
                                if self.show_filters || filters_active {
                                    colors.foreground
                                } else {
                                    colors.muted_foreground
                                },
                            ))
                            .tooltip("Filter  ⌘F")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_filters = !this.show_filters;
                                cx.notify();
                            })),
                    ))
                    .child(tag(
                        "refresh",
                        Button::new("refresh")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .disabled(syncing)
                            .tooltip("Refresh  ⌘R")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                            .child(div().w(rpx(30.)).flex().justify_center().child({
                                let icon = Icon::new(IconName::RefreshCw)
                                    .size(16.)
                                    .color(colors.muted_foreground);
                                if syncing { icon.spin() } else { icon }
                            })),
                    ))
                    .child(tag(
                        "accounts",
                        Button::new("accounts")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .child(icon_child(IconName::UserRoundPlus, colors))
                            .tooltip("Accounts")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(DialogKind::Accounts, window, cx)
                            })),
                    ))
                    .child(tag(
                        "settings",
                        Button::new("settings")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .child(icon_child(IconName::Settings2, colors))
                            .tooltip("Settings  ⌘,")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(DialogKind::Settings, window, cx)
                            })),
                    )),
            )
    }

    fn filter_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(rpx(8.))
            .p(rpx(10.))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .relative()
                    .child(self.search.clone())
                    // The magnifier sits in the field's left padding.
                    .child(
                        div()
                            .absolute()
                            .top(rpx(0.))
                            .bottom(rpx(0.))
                            .left(rpx(10.))
                            .flex()
                            .items_center()
                            .child(
                                Icon::new(IconName::Search)
                                    .size(14.)
                                    .color(colors.muted_foreground),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(6.))
                    .child(div().flex_1().min_w_0().child(self.account_select.clone()))
                    .child(div().flex_1().min_w_0().child(self.checks_select.clone()))
                    .when(self.filters.active(), |d| {
                        d.child(tag(
                            "clear-filters",
                            Button::new("clear-filters")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Icon)
                                .on_click(cx.listener(|this, _, _, cx| this.clear_filters(cx)))
                                .child(icon_child(IconName::X, colors)),
                        ))
                    }),
            )
    }

    /// `Empty`, with the exact copy of each state.
    fn empty_state(&self, state: DeckEmptyState, cx: &mut Context<Self>) -> AnyElement {
        match state {
            DeckEmptyState::NoAccounts => empty(
                Icon::new(IconName::UserRoundPlus),
                "No accounts connected",
                "Add a GitHub, GitLab, Forgejo or Bitbucket account to start collecting the reviews people are waiting on you for.",
                Some(tag(
                    "empty-add-account",
                    Button::new("empty-add-account")
                        .variant(ButtonVariant::Default)
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_dialog(DialogKind::Accounts, window, cx)
                        }))
                        .child("Add an account"),
                )),
                cx,
            ),
            DeckEmptyState::Syncing => empty(
                Icon::new(IconName::Loader2).spin(),
                "Checking for reviews…",
                "Asking every connected account what is waiting on you.",
                None,
                cx,
            ),
            DeckEmptyState::OnlyDrafts => {
                let one = self.hidden_drafts == 1;
                empty(
                    Icon::new(IconName::GitPullRequestDraft),
                    if one {
                        "Only a draft waiting"
                    } else {
                        "Only drafts waiting"
                    },
                    format!(
                        "Nobody is waiting on a finished review, but {} {} hidden by your settings.",
                        plural(self.hidden_drafts, "draft"),
                        if one { "is" } else { "are" }
                    ),
                    Some(tag(
                        "empty-show-drafts",
                        Button::new("empty-show-drafts")
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reveal_drafts = true;
                                this.recompute(cx);
                            }))
                            .child(if one { "Show it" } else { "Show them" }),
                    )),
                    cx,
                )
            }
            DeckEmptyState::NoMatches => empty(
                Icon::new(IconName::Inbox),
                "Nothing matches",
                "No review requests match the current filters.",
                Some(tag(
                    "empty-clear-filters",
                    Button::new("empty-clear-filters")
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(|this, _, _, cx| this.clear_filters(cx)))
                        .child("Clear filters"),
                )),
                cx,
            ),
            DeckEmptyState::InboxZero => empty(
                Icon::new(IconName::Inbox),
                "Inbox zero",
                "Nobody is waiting on a review from you right now.",
                None,
                cx,
            ),
        }
    }

    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let muted = colors.muted_foreground;
        div()
            .flex()
            .flex_none()
            .items_center()
            .border_t_1()
            .border_color(colors.border)
            .px(rpx(12.))
            .py(rpx(8.))
            .text_size(rpx(11.))
            .line_height(rpx(16.5))
            .text_color(muted)
            .child(tag(
                format!(
                    "footer:{} of {} waiting",
                    self.items.len(),
                    self.all_items.len()
                ),
                format!("{} of {} waiting", self.items.len(), self.all_items.len()),
            ))
            .when(self.hidden_drafts > 0, |d| {
                let foreground = colors.foreground;
                let text = format!(
                    "{} {}",
                    plural(self.hidden_drafts, "draft"),
                    if self.reveal_drafts {
                        "shown"
                    } else {
                        "hidden"
                    }
                );
                d.child(tag(
                    format!("toggle-drafts:{text}"),
                    div()
                        .id("toggle-drafts")
                        .ml(rpx(8.))
                        .cursor_pointer()
                        .underline()
                        .hover(move |s| s.text_color(foreground))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reveal_drafts = !this.reveal_drafts;
                            this.recompute(cx);
                        }))
                        .child(text),
                ))
            })
            .child(
                div()
                    .ml_auto()
                    .text_color(with_alpha(muted, muted.a * 0.7))
                    .child("j / k to move"),
            )
    }
}

impl Focusable for AppView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The empty-state block: a round icon, a title, a sentence and an optional action.
fn empty(
    icon: Icon,
    title: &'static str,
    body: impl Into<SharedString>,
    action: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors;
    let title_el = div()
        .text_size(rpx(13.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(title);
    #[cfg(test)]
    let title_el = title_el.debug_selector(|| format!("empty:{title}"));
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(rpx(8.))
        .px(rpx(32.))
        .py(rpx(56.))
        .text_center()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(rpx(40.))
                .rounded_full()
                .bg(colors.muted)
                .child(icon.size(20.).color(colors.muted_foreground)),
        )
        .child(title_el)
        .child(
            div()
                .max_w(rpx(288.))
                .text_size(rpx(12.5))
                .line_height(rpx(20.))
                .text_color(colors.muted_foreground)
                .child(body.into()),
        )
        .when_some(action, |d, action| d.child(div().mt(rpx(4.)).child(action)))
        .into_any_element()
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(debug_assertions)]
        self.apply_scene(window, cx);
        let colors = cx.theme().colors;

        // A zoom changes every card's height; the list keeps what it measured until told.
        if window.rem_size() != self.last_rem {
            self.last_rem = window.rem_size();
            let count = self.list_state.item_count();
            self.list_state.splice(0..count, count);
        }

        let pull = self.ensure_pull(window, cx);
        let empty_el = self.empty.map(|state| self.empty_state(state, cx));
        let has_items = !self.items.is_empty();

        let list_thumb = {
            let state = &self.list_state;
            thumb(
                state.scroll_px_offset_for_scrollbar().y,
                state.max_offset_for_scrollbar().height,
                state.viewport_bounds().size.height,
            )
            .map(|(top, height)| {
                div()
                    .absolute()
                    .top(top)
                    .right(px(3.))
                    .w(px(4.))
                    .h(height)
                    .rounded_full()
                    .bg(colors.border_strong)
            })
        };

        let deck_list = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p(rpx(6.))
            .when_some(empty_el, |d, el| d.child(el))
            .when(has_items, |d| {
                d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            list(
                                self.list_state.clone(),
                                cx.processor(|this, ix, _window, cx| this.render_row(ix, cx)),
                            )
                            .size_full(),
                        )
                        .children(list_thumb),
                )
            });

        let aside = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(rpx(384.))
            .h_full()
            .bg(colors.surface_muted)
            // `glass-quiet` carries a hairline all the way round, not just on the right.
            .border_1()
            .border_color(colors.border)
            .when(self.show_filters, |d| d.child(self.filter_panel(cx)))
            .child(deck_list)
            .child(self.footer(cx));

        let main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .map(|d| match pull {
                Some(view) => d.child(view),
                None => d.child(div().flex().flex_1().items_center().justify_center().child(
                    empty(
                        Icon::new(IconName::Inbox),
                        "Nothing selected",
                        "Pick a pull request on the left to read the diff and leave a review.",
                        None,
                        cx,
                    ),
                )),
            });

        div()
            .key_context("AppView")
            .track_focus(&self.focus)
            // A pointer press anywhere ends keyboard modality (`:focus-visible`).
            .capture_any_mouse_down(|_, _, cx| crate::ui::components::pointer_pressed(cx))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| this.step(1, window, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(Self::focus_search))
            // Tab walks the cards, the pills and the fields, as the browser did. The
            // dialogs answer these first and keep the focus inside themselves.
            .on_action(cx.listener(|_, _: &FocusNext, window, _| window.focus_next()))
            .on_action(cx.listener(|_, _: &FocusPrev, window, _| window.focus_prev()))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .font_family(UI_FONT)
            .text_size(rpx(13.5))
            .line_height(relative(1.5))
            .child(self.header(cx))
            .child(div().flex().flex_1().min_h_0().child(aside).child(main))
            .children(self.dialog.as_ref().map(OpenDialog::view))
            .child(self.toasts.clone())
    }
}

/// The 16px glyph of an icon-only button, in a box as wide as the TSX's `w-8` button
/// (the kit's icon size is 32px high but only as wide as its content).
fn icon_child(name: IconName, colors: crate::ui::theme::Colors) -> gpui::Div {
    icon_child_in(name, colors.muted_foreground)
}

fn icon_child_in(name: IconName, color: gpui::Hsla) -> gpui::Div {
    div()
        .w(rpx(30.))
        .flex()
        .justify_center()
        .child(Icon::new(name).size(16.).color(color))
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use futures::future::LocalBoxFuture;
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};
    use reviewdeck_core::demo::DEMO_ITEMS;
    use reviewdeck_core::error::{Result, msg};
    use reviewdeck_core::http::{Http, MockResponse};
    use reviewdeck_core::model::{
        ApprovalOutcome, ApprovalSummary, CheckSummary, DraftComment, MyReviewState, NewAccount,
        ProviderKind, ReviewVerdict, ThemeMode, make_item_id,
    };
    use reviewdeck_core::providers::Session;
    use reviewdeck_core::store::{MemoryTokens, TokenStore, Vault};

    use super::*;
    use crate::state::{AppDeps, Remote};
    use crate::ui::theme::Theme;

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Scripted hosts: what each account returns and which ones fail.
    #[derive(Default)]
    struct Hosts {
        reviews: Mutex<HashMap<String, Vec<ReviewItem>>>,
        failing: Mutex<HashSet<String>>,
    }

    impl Remote for Hosts {
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
            _: Session,
            _: ReviewItem,
        ) -> LocalBoxFuture<'static, Result<CheckSummary>> {
            Box::pin(async { Err(msg("not scripted")) })
        }

        fn submit_review(
            &self,
            _: Session,
            _: ReviewItem,
            _: ReviewVerdict,
            _: String,
            _: Vec<DraftComment>,
        ) -> LocalBoxFuture<'static, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn test_vault() -> Arc<Vault> {
        let dir = std::env::temp_dir().join(format!(
            "reviewdeck-app-view-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let tokens: Arc<dyn TokenStore> = Arc::new(MemoryTokens::new());
        Arc::new(Vault::open_at(dir.join("reviewdeck.json"), tokens))
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

    /// A finished, unapproved review on `account_id` titled `title`; the higher the
    /// number the more recently it was updated, so the deck lists them newest first.
    fn review(account_id: &str, number: u64, title: &str) -> ReviewItem {
        let mut item = DEMO_ITEMS[0].clone();
        item.account_id = account_id.to_string();
        item.number = number;
        item.id = make_item_id(account_id, &item.repo_key, number);
        item.title = title.to_string();
        item.draft = false;
        item.checks.status = CheckStatus::Passed;
        item.my_review_state = MyReviewState::Pending;
        item.approvals = ApprovalSummary {
            given: 0,
            required: None,
            outcome: ApprovalOutcome::NoneRequired,
        };
        item.updated_at = format!("2026-03-0{number}T10:00:00Z");
        item
    }

    struct Rig {
        state: Entity<AppState>,
        hosts: Arc<Hosts>,
        accounts: Vec<Account>,
    }

    impl Rig {
        /// An app state over a temporary vault holding one account per label.
        fn new(cx: &mut TestAppContext, labels: &[&str]) -> Rig {
            let vault = test_vault();
            let accounts: Vec<Account> = labels
                .iter()
                .map(|label| add_account(&vault, label))
                .collect();
            let hosts = Arc::new(Hosts::default());
            let deps = AppDeps {
                http: Http::mock(|_| MockResponse::new(500, Vec::new())),
                vault,
                remote: Some(hosts.clone()),
                demo: false,
                clock: None,
                notify: None,
                tray: None,
            };
            let state = cx.new(|cx| AppState::new(deps, cx));
            cx.update(|cx| {
                cx.set_global(GlobalState(state.clone()));
                Theme::apply(ThemeMode::Light, cx);
                crate::ui::components::bind_keys(cx);
                bind_keys(cx);
            });
            Rig {
                state,
                hosts,
                accounts,
            }
        }

        fn script(&self, account: usize, reviews: Vec<ReviewItem>) {
            locked(&self.hosts.reviews).insert(self.accounts[account].id.clone(), reviews);
        }

        fn fail(&self, account: usize, failing: bool) {
            let id = self.accounts[account].id.clone();
            let mut set = locked(&self.hosts.failing);
            if failing {
                set.insert(id);
            } else {
                set.remove(&id);
            }
        }

        fn sync(&self, cx: &mut VisualTestContext) {
            let task = self.state.update(cx, |state, cx| state.refresh(cx));
            cx.executor().block_test(task).expect("the sync completes");
            cx.run_until_parked();
        }

        fn hide_drafts(&self, hide: bool, cx: &mut VisualTestContext) {
            self.state.update(cx, |state, cx| {
                state
                    .set_settings(|settings| settings.hide_drafts = hide, cx)
                    .expect("settings save");
            });
            cx.run_until_parked();
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<AppView>, &mut VisualTestContext) {
        let (view, vcx) = cx.add_window_view(AppView::new);
        vcx.run_until_parked();
        (view, vcx)
    }

    fn selected(view: &Entity<AppView>, cx: &mut VisualTestContext) -> Option<String> {
        view.read_with(cx, |view, _| view.selected_id.clone())
    }

    fn shown(view: &Entity<AppView>, cx: &mut VisualTestContext) -> Vec<String> {
        view.read_with(cx, |view, _| {
            view.items.iter().map(|item| item.title.clone()).collect()
        })
    }

    /// The selectors are `&'static` in gpui; leaking a test's few names is fine.
    fn leak(name: impl Into<String>) -> &'static str {
        Box::leak(name.into().into_boxed_str())
    }

    fn click(name: impl Into<String>, cx: &mut VisualTestContext) {
        let name = leak(name);
        let bounds = cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not on screen"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
    }

    fn on_screen(name: impl Into<String>, cx: &mut VisualTestContext) -> bool {
        cx.debug_bounds(leak(name)).is_some()
    }

    fn dialog_is(view: &Entity<AppView>, kind: Option<DialogKind>, cx: &mut VisualTestContext) {
        view.read_with(cx, |view, _| {
            let open = match &view.dialog {
                None => None,
                Some(OpenDialog::Accounts(_)) => Some(DialogKind::Accounts),
                Some(OpenDialog::Settings(_)) => Some(DialogKind::Settings),
                Some(OpenDialog::Schedule(_)) => Some(DialogKind::Schedule),
            };
            assert_eq!(
                open.map(|kind| format!("{kind:?}")),
                kind.map(|kind| format!("{kind:?}"))
            );
        });
    }

    fn three_reviews(rig: &Rig) {
        let account = &rig.accounts[0].id;
        rig.script(
            0,
            vec![
                review(account, 1, "Alpha change"),
                review(account, 2, "Beta change"),
                review(account, 3, "Gamma change"),
            ],
        );
    }

    /// The ids of the three reviews, in the order the deck lists them (newest first).
    fn ids(rig: &Rig) -> Vec<String> {
        (1..=3u64)
            .rev()
            .map(|number| review(&rig.accounts[0].id, number, "").id)
            .collect()
    }

    // ---- filters -------------------------------------------------------------

    #[test]
    fn the_query_matches_title_repo_author_and_number() {
        let mut item = review("a", 42, "Fix the flux capacitor");
        item.repo = "acme/Widgets".into();
        item.author.name = "Marty McFly".into();
        let items = vec![Arc::new(item)];
        let settings = Settings::default();
        let find = |query: &str| {
            let filters = Filters {
                query: query.to_string(),
                ..Filters::default()
            };
            apply_filters(&items, &filters, &settings).len()
        };
        assert_eq!(find("FLUX"), 1, "title, any case");
        assert_eq!(find("widgets"), 1, "repo");
        assert_eq!(find("mcfly"), 1, "author");
        assert_eq!(find("42"), 1, "number");
        assert_eq!(find("  flux  "), 1, "the needle is trimmed");
        assert_eq!(find("zzz"), 0);
        assert_eq!(find(""), 1);
    }

    #[test]
    fn account_and_check_filters_narrow_the_deck() {
        let mut passed = review("a", 1, "One");
        passed.checks.status = CheckStatus::Passed;
        let mut other = review("b", 2, "Two");
        other.checks.status = CheckStatus::Failed;
        let items = vec![Arc::new(passed), Arc::new(other)];
        let settings = Settings::default();
        let by = |account: Option<&str>, checks: Option<CheckStatus>| {
            let filters = Filters {
                query: String::new(),
                account: account.map(str::to_string),
                checks,
            };
            apply_filters(&items, &filters, &settings).len()
        };
        assert_eq!(by(None, None), 2);
        assert_eq!(by(Some("a"), None), 1);
        assert_eq!(by(None, Some(CheckStatus::Failed)), 1);
        assert_eq!(by(Some("a"), Some(CheckStatus::Failed)), 0);
    }

    // ---- empty states --------------------------------------------------------

    #[gpui::test]
    fn no_accounts_offers_to_add_one(cx: &mut TestAppContext) {
        let _rig = Rig::new(cx, &[]);
        let (view, cx) = open(cx);
        assert!(on_screen("empty:No accounts connected", cx));
        assert!(on_screen("empty:Nothing selected", cx));
        click("empty-add-account", cx);
        dialog_is(&view, Some(DialogKind::Accounts), cx);
    }

    #[gpui::test]
    fn an_account_that_has_not_synced_says_it_is_checking(cx: &mut TestAppContext) {
        let _rig = Rig::new(cx, &["Work"]);
        let (_view, cx) = open(cx);
        assert!(on_screen("empty:Checking for reviews…", cx));
        assert!(on_screen("sync-label:Not synced yet", cx));
        assert!(on_screen("footer:0 of 0 waiting", cx));
    }

    #[gpui::test]
    fn a_synced_empty_deck_is_inbox_zero(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        let (view, cx) = open(cx);
        rig.sync(cx);
        assert!(on_screen("empty:Inbox zero", cx));
        assert_eq!(
            view.read_with(cx, |view, _| view.empty),
            Some(DeckEmptyState::InboxZero)
        );
        assert!(on_screen("sync-label:Updated just now", cx));
    }

    #[gpui::test]
    fn a_deck_of_only_drafts_says_so_and_can_show_them(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        let mut draft = review(&rig.accounts[0].id, 1, "Wip one");
        draft.draft = true;
        rig.script(0, vec![draft]);
        let (view, cx) = open(cx);
        rig.sync(cx);
        assert!(on_screen("empty:Only a draft waiting", cx));
        assert!(on_screen("footer:0 of 1 waiting", cx));
        assert!(on_screen("toggle-drafts:1 draft hidden", cx));

        click("empty-show-drafts", cx);
        assert_eq!(shown(&view, cx), vec!["Wip one"]);
        assert!(on_screen("toggle-drafts:1 draft shown", cx));
        assert!(on_screen("footer:1 of 1 waiting", cx));
        assert_eq!(view.read_with(cx, |view, _| view.empty), None);
        assert!(selected(&view, cx).is_some(), "the first card is selected");

        click("toggle-drafts:1 draft shown", cx);
        assert!(shown(&view, cx).is_empty());
        assert!(on_screen("empty:Only a draft waiting", cx));
        assert_eq!(selected(&view, cx), None);
    }

    #[gpui::test]
    fn several_drafts_are_plural(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        let mut a = review(&rig.accounts[0].id, 1, "Wip one");
        a.draft = true;
        let mut b = review(&rig.accounts[0].id, 2, "Wip two");
        b.draft = true;
        rig.script(0, vec![a, b]);
        let (_view, cx) = open(cx);
        rig.sync(cx);
        assert!(on_screen("empty:Only drafts waiting", cx));
        assert!(on_screen("toggle-drafts:2 drafts hidden", cx));
    }

    #[gpui::test]
    fn drafts_alongside_finished_reviews_are_counted_in_the_footer(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        let mut draft = review(&rig.accounts[0].id, 1, "Wip one");
        draft.draft = true;
        rig.script(0, vec![draft, review(&rig.accounts[0].id, 2, "Done one")]);
        let (view, cx) = open(cx);
        rig.sync(cx);
        assert_eq!(shown(&view, cx), vec!["Done one"]);
        assert!(on_screen("footer:1 of 2 waiting", cx));
        assert!(on_screen("toggle-drafts:1 draft hidden", cx));
        click("toggle-drafts:1 draft hidden", cx);
        assert_eq!(shown(&view, cx), vec!["Done one", "Wip one"]);
        assert!(on_screen("footer:2 of 2 waiting", cx));
    }

    #[gpui::test]
    fn turning_the_drafts_preference_back_on_ends_a_reveal(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        let mut draft = review(&rig.accounts[0].id, 1, "Wip one");
        draft.draft = true;
        rig.script(0, vec![draft]);
        let (view, cx) = open(cx);
        rig.sync(cx);
        click("empty-show-drafts", cx);
        assert_eq!(shown(&view, cx).len(), 1);

        // Off: drafts are no longer held back, so there is nothing to reveal.
        rig.hide_drafts(false, cx);
        assert_eq!(shown(&view, cx).len(), 1);
        assert_eq!(view.read_with(cx, |view, _| view.hidden_drafts), 0);

        // On again: the earlier reveal does not survive the decision.
        rig.hide_drafts(true, cx);
        assert!(shown(&view, cx).is_empty());
        assert!(on_screen("empty:Only a draft waiting", cx));
    }

    #[gpui::test]
    fn filters_with_no_match_offer_to_clear_them(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);

        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("zzz");
        cx.run_until_parked();
        assert!(shown(&view, cx).is_empty());
        assert!(on_screen("empty:Nothing matches", cx));
        assert!(
            on_screen("clear-filters", cx),
            "the filter bar can clear too"
        );

        click("empty-clear-filters", cx);
        assert_eq!(shown(&view, cx).len(), 3);
        view.read_with(cx, |view, cx| {
            assert!(view.search.read(cx).text().is_empty());
            assert!(!view.filters.active());
        });
        assert!(!view.read_with(cx, |view, _| view.filters.active()));
    }

    // ---- the failing-account banner -----------------------------------------

    #[gpui::test]
    fn a_failing_account_shows_the_banner_and_opens_the_accounts(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work", "Home"]);
        rig.script(1, vec![review(&rig.accounts[1].id, 1, "From home")]);
        rig.fail(0, true);
        let (view, cx) = open(cx);
        assert!(!on_screen("sync-problems", cx), "nothing has failed yet");
        rig.sync(cx);
        assert!(on_screen("sync-problems", cx));
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.sync_problems().as_deref(),
                Some("Work: Could not reach GitHub.")
            );
        });
        // The other account's reviews are still there.
        assert_eq!(shown(&view, cx), vec!["From home"]);

        click("sync-problems", cx);
        dialog_is(&view, Some(DialogKind::Accounts), cx);

        rig.fail(0, false);
        cx.simulate_keystrokes("escape");
        dialog_is(&view, None, cx);
        rig.sync(cx);
        assert_eq!(view.read_with(cx, |view, _| view.sync_problems()), None);
    }

    #[gpui::test]
    fn two_failing_accounts_are_listed_apart(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work", "Home"]);
        rig.fail(0, true);
        rig.fail(1, true);
        let (view, cx) = open(cx);
        rig.sync(cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.sync_problems().as_deref(),
                Some("Work: Could not reach GitHub.\n\nHome: Could not reach GitHub.")
            );
        });
    }

    // ---- selection and keys --------------------------------------------------

    #[gpui::test]
    fn the_first_card_is_selected_and_j_k_and_arrows_move_it(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let ids = ids(&rig);
        assert_eq!(
            shown(&view, cx),
            vec!["Gamma change", "Beta change", "Alpha change"]
        );
        assert_eq!(selected(&view, cx), Some(ids[0].clone()));

        cx.simulate_keystrokes("k");
        assert_eq!(
            selected(&view, cx),
            Some(ids[0].clone()),
            "clamped at the top"
        );
        cx.simulate_keystrokes("j");
        assert_eq!(selected(&view, cx), Some(ids[1].clone()));
        cx.simulate_keystrokes("down");
        assert_eq!(selected(&view, cx), Some(ids[2].clone()));
        cx.simulate_keystrokes("j");
        assert_eq!(
            selected(&view, cx),
            Some(ids[2].clone()),
            "clamped at the bottom"
        );
        cx.simulate_keystrokes("up");
        assert_eq!(selected(&view, cx), Some(ids[1].clone()));
        cx.simulate_keystrokes("k");
        assert_eq!(selected(&view, cx), Some(ids[0].clone()));
    }

    #[gpui::test]
    fn clicking_a_card_selects_it_and_keeps_the_keys_working(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let ids = ids(&rig);
        click(format!("card:{}", ids[2]), cx);
        assert_eq!(selected(&view, cx), Some(ids[2].clone()));
        cx.simulate_keystrokes("k");
        assert_eq!(
            selected(&view, cx),
            Some(ids[1].clone()),
            "focus went back to the deck, so k still moves"
        );
    }

    #[gpui::test]
    fn the_keys_are_left_to_a_text_field(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);

        cx.simulate_keystrokes("cmd-f");
        cx.run_until_parked();
        let search = view.read_with(cx, |view, _| view.search.clone());
        assert!(
            view.read_with(cx, |view, _| view.show_filters),
            "cmd-f opens the filters"
        );
        assert!(
            cx.update(|window, cx| search.read(cx).is_focused(window)),
            "and puts the caret in the search field"
        );
        cx.simulate_input("beta");
        cx.simulate_keystrokes("j k");
        assert_eq!(
            search.read_with(cx, |input, _| input.text().to_string()),
            "betajk",
            "j and k are typed, not obeyed"
        );
        assert!(shown(&view, cx).is_empty(), "nothing matches betajk");
        cx.simulate_keystrokes("backspace backspace");
        assert_eq!(shown(&view, cx), vec!["Beta change"]);

        // Escape closes the filters (keeping the query) and hands the keys back.
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert!(!view.show_filters);
            assert!(view.filters.active(), "the query stays");
        });
        cx.simulate_keystrokes("j");
        assert_eq!(
            search.read_with(cx, |input, _| input.text().to_string()),
            "beta",
            "j no longer reaches the field"
        );
    }

    #[gpui::test]
    fn cmd_f_works_with_the_filters_already_open(cx: &mut TestAppContext) {
        let _rig = Rig::new(cx, &["Work"]);
        let (view, cx) = open(cx);
        click("filter", cx);
        assert!(view.read_with(cx, |view, _| view.show_filters));
        cx.simulate_keystrokes("cmd-f");
        cx.run_until_parked();
        let search = view.read_with(cx, |view, _| view.search.clone());
        assert!(cx.update(|window, cx| search.read(cx).is_focused(window)));
        click("filter", cx);
        assert!(!view.read_with(cx, |view, _| view.show_filters));
    }

    #[gpui::test]
    fn the_account_and_check_selects_filter_the_deck(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work", "Home"]);
        let mut failed = review(&rig.accounts[0].id, 1, "Red build");
        failed.checks.status = CheckStatus::Failed;
        rig.script(0, vec![failed, review(&rig.accounts[0].id, 2, "Green one")]);
        rig.script(1, vec![review(&rig.accounts[1].id, 3, "From home")]);
        let (view, cx) = open(cx);
        rig.sync(cx);
        assert_eq!(shown(&view, cx).len(), 3);

        click("filter", cx);
        let (accounts, checks) = view.read_with(cx, |view, _| {
            (view.account_select.clone(), view.checks_select.clone())
        });
        let home = rig.accounts[1].id.clone();
        accounts.update(cx, |_, cx| cx.emit(SelectEvent::Changed(home.into())));
        cx.run_until_parked();
        assert_eq!(shown(&view, cx), vec!["From home"]);

        accounts.update(cx, |_, cx| cx.emit(SelectEvent::Changed("all".into())));
        checks.update(cx, |_, cx| cx.emit(SelectEvent::Changed("failed".into())));
        cx.run_until_parked();
        assert_eq!(shown(&view, cx), vec!["Red build"]);
        assert!(on_screen("clear-filters", cx));

        click("clear-filters", cx);
        assert_eq!(shown(&view, cx).len(), 3);
    }

    #[gpui::test]
    fn a_selection_that_leaves_the_deck_falls_back_to_the_first(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let ids = ids(&rig);
        cx.simulate_keystrokes("j");
        assert_eq!(selected(&view, cx), Some(ids[1].clone()));

        // Beta is closed upstream.
        let account = rig.accounts[0].id.clone();
        rig.script(
            0,
            vec![
                review(&account, 1, "Alpha change"),
                review(&account, 3, "Gamma change"),
            ],
        );
        rig.sync(cx);
        assert_eq!(selected(&view, cx), Some(ids[0].clone()));

        // And one that stays is left alone when the deck changes around it.
        cx.simulate_keystrokes("j");
        let kept = selected(&view, cx);
        rig.script(
            0,
            vec![
                review(&account, 1, "Alpha change"),
                review(&account, 3, "Gamma change"),
                review(&account, 4, "Delta change"),
            ],
        );
        rig.sync(cx);
        assert_eq!(selected(&view, cx), kept);
    }

    #[gpui::test]
    fn an_emptied_deck_selects_nothing(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        rig.script(0, vec![]);
        rig.sync(cx);
        assert_eq!(selected(&view, cx), None);
        assert!(on_screen("empty:Nothing selected", cx));
        cx.simulate_keystrokes("j k");
        assert_eq!(selected(&view, cx), None);
    }

    // ---- events from the app -------------------------------------------------

    #[gpui::test]
    fn focus_item_selects_that_review(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let ids = ids(&rig);
        rig.state
            .update(cx, |_, cx| cx.emit(AppEvent::FocusItem(ids[2].clone())));
        cx.run_until_parked();
        assert_eq!(selected(&view, cx), Some(ids[2].clone()));

        // An id the deck does not hold lands on the first card, as the TSX's effect did.
        rig.state
            .update(cx, |_, cx| cx.emit(AppEvent::FocusItem("nope".into())));
        cx.run_until_parked();
        assert_eq!(selected(&view, cx), Some(ids[0].clone()));
    }

    #[gpui::test]
    fn the_settings_events_open_the_settings_dialog(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        let (view, cx) = open(cx);
        rig.state.update(cx, |_, cx| {
            cx.emit(AppEvent::FocusItem("__settings__".into()))
        });
        cx.run_until_parked();
        dialog_is(&view, Some(DialogKind::Settings), cx);
        cx.simulate_keystrokes("escape");
        dialog_is(&view, None, cx);

        rig.state
            .update(cx, |_, cx| cx.emit(AppEvent::OpenSettings));
        cx.run_until_parked();
        dialog_is(&view, Some(DialogKind::Settings), cx);
    }

    // ---- dialogs -------------------------------------------------------------

    #[gpui::test]
    fn the_header_buttons_open_one_dialog_at_a_time(cx: &mut TestAppContext) {
        let _rig = Rig::new(cx, &["Work"]);
        let (view, cx) = open(cx);
        click("accounts", cx);
        dialog_is(&view, Some(DialogKind::Accounts), cx);
        cx.simulate_keystrokes("escape");
        dialog_is(&view, None, cx);

        click("settings", cx);
        dialog_is(&view, Some(DialogKind::Settings), cx);
        // The schedule replaces the settings rather than stacking on them, and closing
        // it leaves the deck, not the settings.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_dialog(DialogKind::Schedule, window, cx)
            })
        });
        dialog_is(&view, Some(DialogKind::Schedule), cx);
        cx.simulate_keystrokes("escape");
        dialog_is(&view, None, cx);
    }

    #[gpui::test]
    fn the_deck_keys_wait_while_a_dialog_is_open(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let first = selected(&view, cx);
        click("accounts", cx);
        cx.simulate_keystrokes("j j");
        assert_eq!(selected(&view, cx), first);
    }

    #[gpui::test]
    fn the_refresh_button_runs_a_sync(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        click("refresh", cx);
        cx.run_until_parked();
        assert_eq!(shown(&view, cx).len(), 3, "the click ran a sync");
        assert!(on_screen("sync-label:Updated just now", cx));
    }

    // ---- the header as a title bar ------------------------------------------

    #[gpui::test]
    fn a_press_on_the_empty_header_drags_and_a_double_press_is_a_title_bar_click(
        cx: &mut TestAppContext,
    ) {
        let _rig = Rig::new(cx, &["Work"]);
        let (view, cx) = open(cx);
        let press = |cx: &mut VisualTestContext, x: f32, count: usize| {
            cx.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(x), px(26.)),
                modifiers: Modifiers::none(),
                click_count: count,
                first_mouse: false,
            });
        };
        press(cx, 700., 1);
        press(cx, 700., 2);
        assert_eq!(
            view.read_with(cx, |view, _| view.header_presses.clone()),
            vec![1, 2]
        );

        // On a button it is a click, not a drag.
        let bounds = cx.debug_bounds("settings").expect("the settings button");
        let x: f32 = bounds.center().x.into();
        press(cx, x, 1);
        assert_eq!(
            view.read_with(cx, |view, _| view.header_presses.len()),
            2,
            "buttons are `no-drag`"
        );

        // Below the header it is nothing.
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(700.), px(300.)),
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        assert_eq!(view.read_with(cx, |view, _| view.header_presses.len()), 2);
    }

    // ---- a card ---------------------------------------------------------------

    #[gpui::test]
    fn a_card_is_a_tab_stop_that_enter_activates(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let ids = ids(&rig);
        // Each card is a tab stop followed by its check pill's. Enter is a key-up for
        // the click gpui makes of it, which `simulate_keystrokes` does not send.
        let enter = |cx: &mut VisualTestContext| {
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("enter").expect("a keystroke"),
            });
            cx.run_until_parked();
        };
        cx.simulate_keystrokes("j j");
        assert_eq!(selected(&view, cx), Some(ids[2].clone()));

        // The header's four buttons come first, as native buttons did in the DOM; then
        // the first card, its pill, and the second card.
        cx.simulate_keystrokes("tab tab tab tab tab tab tab");
        enter(cx);
        assert_eq!(selected(&view, cx), Some(ids[1].clone()), "the second card");

        // Back past the first card's pill to the card itself.
        cx.simulate_keystrokes("shift-tab shift-tab");
        enter(cx);
        assert_eq!(selected(&view, cx), Some(ids[0].clone()), "and back again");
    }

    #[gpui::test]
    fn the_check_pill_opens_its_panel_without_selecting_the_card(cx: &mut TestAppContext) {
        let rig = Rig::new(cx, &["Work"]);
        three_reviews(&rig);
        let (view, cx) = open(cx);
        rig.sync(cx);
        let ids = ids(&rig);
        assert_eq!(selected(&view, cx), Some(ids[0].clone()));
        assert!(!on_screen(format!("pill-{}-panel", ids[2]), cx));

        click(format!("pill-{}", ids[2]), cx);
        assert!(
            on_screen(format!("pill-{}-panel", ids[2]), cx),
            "the runs behind the roll-up are listed"
        );
        assert_eq!(
            selected(&view, cx),
            Some(ids[0].clone()),
            "the click stopped at the pill"
        );
    }
}
