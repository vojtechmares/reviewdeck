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
    KeyBinding, ListAlignment, ListState, ParentElement, Render, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, Subscription, Task, Timer, Window,
    WindowControlArea, actions, div, list, prelude::FluentBuilder, px, relative,
};
use reviewdeck_core::model::{
    Account, AccountStatus, CheckStatus, DeckCounts, DeckEmptyState, ReviewItem, Settings,
    deck_empty_state, is_visible_review,
};
use reviewdeck_core::time::{now_ms, relative_time};

use crate::state::{AppEvent, AppState, GlobalState};
use crate::ui::accounts_dialog::AccountsDialog;
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant, with_alpha};
use crate::ui::components::input::{TextInput, TextInputEvent};
use crate::ui::components::scroll::thumb;
use crate::ui::components::select::{Select, SelectEvent, SelectOption};
use crate::ui::components::toast::{ToastKind, ToastStack};
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
        KeyBinding::new("j", SelectNext, Some("AppView && !TextInput")),
        KeyBinding::new("down", SelectNext, Some("AppView && !TextInput")),
        KeyBinding::new("k", SelectPrev, Some("AppView && !TextInput")),
        KeyBinding::new("up", SelectPrev, Some("AppView && !TextInput")),
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
            .text_color(muted);
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
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> AppView {
        let state = cx.global::<GlobalState>().0.clone();
        let focus = cx.focus_handle();

        let toasts = cx.new(|_| ToastStack::new());
        cx.set_global(ToastHost(toasts.clone()));

        let search =
            cx.new(|cx| TextInput::new(cx).placeholder("Filter by title, repo or author…"));
        let account_select =
            cx.new(|cx| Select::new(vec![SelectOption::new("all", "All accounts")], "all", cx));
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
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.items.is_empty() || self.dialog.is_some() {
            return;
        }
        let index = self.selected_index().unwrap_or(0) as isize;
        let next = (index + delta).clamp(0, self.items.len() as isize - 1) as usize;
        let id = self.items[next].id.clone();
        self.select(Some(id), cx);
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
    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.show_filters = true;
        cx.notify();
        let search = self.search.clone();
        // Wait for the field to mount before focusing it.
        window.on_next_frame(move |window, cx| search.read(cx).focus(window));
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
                cx.listener(move |this, _: &ClickEvent, _, cx| this.select(Some(id.clone()), cx)),
            ))
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let failing: Vec<&AccountStatus> = self.statuses.iter().filter(|s| !s.ok).collect();
        let filters_active = self.filters.active();
        let syncing = self.syncing;

        let failing_button = (!failing.is_empty()).then(|| {
            let text = failing
                .iter()
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
                .collect::<Vec<_>>()
                .join("\n\n");
            Button::new("sync-problems")
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::Icon)
                .tooltip(text)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_dialog(DialogKind::Accounts, window, cx)
                }))
                .child(icon_child_in(IconName::TriangleAlert, colors.bad))
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
            // The strip under the traffic lights the whole window can be dragged by.
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_baseline()
                    .gap(rpx(8.))
                    .child(
                        div()
                            .text_size(rpx(13.5))
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
                    .children(failing_button)
                    .child(
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
                    )
                    .child(
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
                    )
                    .child(
                        Button::new("accounts")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .child(icon_child(IconName::UserRoundPlus, colors))
                            .tooltip("Accounts")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(DialogKind::Accounts, window, cx)
                            })),
                    )
                    .child(
                        Button::new("settings")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Icon)
                            .child(icon_child(IconName::Settings2, colors))
                            .tooltip("Settings  ⌘,")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(DialogKind::Settings, window, cx)
                            })),
                    ),
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
                    // The magnifier sits at the right: the text field's padding is fixed,
                    // so it cannot make room for one on the left as `pl-7.5` does.
                    .child(
                        div()
                            .absolute()
                            .top(rpx(0.))
                            .bottom(rpx(0.))
                            .right(rpx(12.))
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
                        d.child(
                            Button::new("clear-filters")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Icon)
                                .on_click(cx.listener(|this, _, _, cx| this.clear_filters(cx)))
                                .child(icon_child(IconName::X, colors)),
                        )
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
                Some(
                    Button::new("empty-add-account")
                        .variant(ButtonVariant::Default)
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_dialog(DialogKind::Accounts, window, cx)
                        }))
                        .child("Add an account")
                        .into_any_element(),
                ),
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
                    Some(
                        Button::new("empty-show-drafts")
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reveal_drafts = true;
                                this.recompute(cx);
                            }))
                            .child(if one { "Show it" } else { "Show them" })
                            .into_any_element(),
                    ),
                    cx,
                )
            }
            DeckEmptyState::NoMatches => empty(
                Icon::new(IconName::Inbox),
                "Nothing matches",
                "No review requests match the current filters.",
                Some(
                    Button::new("empty-clear-filters")
                        .size(ButtonSize::Sm)
                        .on_click(cx.listener(|this, _, _, cx| this.clear_filters(cx)))
                        .child("Clear filters")
                        .into_any_element(),
                ),
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
            .line_height(rpx(16.))
            .text_color(muted)
            .child(format!(
                "{} of {} waiting",
                self.items.len(),
                self.all_items.len()
            ))
            .when(self.hidden_drafts > 0, |d| {
                let foreground = colors.foreground;
                d.child(
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
                        .child(format!(
                            "{} {}",
                            plural(self.hidden_drafts, "draft"),
                            if self.reveal_drafts {
                                "shown"
                            } else {
                                "hidden"
                            }
                        )),
                )
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
        .child(
            div()
                .text_size(rpx(13.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
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
            .border_r_1()
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
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(Self::focus_search))
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
