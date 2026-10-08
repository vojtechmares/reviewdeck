//! Port of src/renderer/src/components/DiffView.tsx, and the diff parts of
//! src/renderer/src/lib/highlight.ts and index.css.
//!
//! The whole diff is one virtualised [`list`]: every file header, hunk header, code
//! row, inline thread, draft and composer is a row of it, so only the rows near the
//! viewport are ever built and a diff of tens of thousands of lines scrolls as
//! smoothly as a short one. The TypeScript did the same job with an
//! `IntersectionObserver` and measured placeholder heights per file (`useNearViewport`,
//! `measuredHeight`); the list's own lazy measurement replaces both.
//!
//! The view must sit in a parent that gives it a definite height (it scrolls
//! itself), the way the diff tab's scroll area did.
//!
//! What is derived is derived once, off the render path: hunks are parsed when the
//! view is built, a file's printable line texts (tabs expanded) the first time it is
//! open, and its syntax colours on the background executor, cached across views by
//! the hash of the patch. Render only builds elements for the visible rows from that.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, HighlightStyle, Hsla, KeyDownEvent, ListAlignment, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Subscription, Window, div, list, prelude::*,
    px, relative,
};
use reviewdeck_core::diff::{
    CommentTarget, DiffHunk, DiffLine, DiffLineKind, DiffSide, LineAnchor, LineRef, SplitRow,
    covers_line, line_on, parse_patch, range_target, side_target, to_split_rows,
};
use reviewdeck_core::drafts::NewDraft;
use reviewdeck_core::highlight::{self, Token, language_for};
use reviewdeck_core::model::{
    CommentThread, DiffFile, DiffRefs, DiffViewMode, DraftComment, FileStatus, LineRange, Side,
};

use crate::state::{AppState, GlobalState};
use crate::ui::app_view::toast;
use crate::ui::code::WrappedCode;
use crate::ui::components::button::{Button, ButtonSize, ButtonVariant};
use crate::ui::components::glass::GlassExt;
use crate::ui::components::input::{TextInput, TextInputEvent};
use crate::ui::components::scroll::thumb;
use crate::ui::components::toast::ToastKind;
use crate::ui::draft_view::DraftCard;
use crate::ui::icons::{Icon, IconName};
use crate::ui::theme::{ActiveTheme, UI_FONT, mono_font, packed, radius, rpx};
use crate::ui::thread_view::{ThreadCard, ThreadEvent};

/// Files past this many lines start collapsed so opening a big PR stays instant.
const AUTO_COLLAPSE_LINES: usize = 600;

/// `.mono`: 12px with `line-height: 1.55`.
const CODE_SIZE: f32 = 12.;
const CODE_LINE: f32 = 12. * 1.55;
/// The gutters and hunk headers are `!text-[11px]` inside the same `.mono` rule, whose
/// unitless line height follows the font size.
const SMALL_SIZE: f32 = 11.;
const SMALL_LINE: f32 = 11. * 1.55;

/// `::-webkit-scrollbar { width: 10px }`: the track the pane reserves on its right.
const SCROLLBAR_WIDTH: f32 = 10.;

/// `w-11`: one line-number gutter.
const GUTTER_WIDTH: f32 = 44.;
/// `p-4` around the files and `gap-3` between them.
const OUTER_PAD: f32 = 16.;
const FILE_GAP: f32 = 12.;

/// The marker (`+`, `-`) takes the first column of a code cell: the TypeScript set it
/// in an 8px box with a 4px margin, here it is painted over two blank characters so a
/// wrapped line starts under it, as the browser's did.
const MARKER_PREFIX: &str = "  ";

/// `font-variant-ligatures: none` of the `.mono` rule: no ligatures and no contextual
/// alternates, which would otherwise stretch a `-` before a digit.
fn no_ligatures<E: Styled>(mut element: E) -> E {
    element
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(gpui::FontFeatures(Arc::new(vec![
        ("liga".to_string(), 0),
        ("clig".to_string(), 0),
        ("calt".to_string(), 0),
    ])));
    element
}

/// How far past the viewport the list builds rows, so scrolling never shows blanks.
const OVERDRAW: f32 = 600.;

/// What the view tells its owner.
pub enum DiffEvent {
    /// A reply or a resolve toggle in an inline thread: the owner reloads the threads
    /// and hands them back through [`DiffView::set_threads`].
    ThreadsChanged,
}

// ---------------------------------------------------------------------------------------
// Derived per-file data
// ---------------------------------------------------------------------------------------

/// The colour runs of one line, on the text [`expand`] produced for it.
type LineStyles = Option<Arc<Vec<(Range<usize>, HighlightStyle)>>>;
/// One slot per line of every hunk.
type Highlights = Vec<Vec<LineStyles>>;

/// Where a file's syntax colour stands. The diff renders plain first and gains colour
/// when it arrives, so loading a grammar never stands between the reader and the code.
enum Colour {
    Idle,
    /// `previous` is the other theme's colours, kept on screen until the new ones
    /// arrive: the TypeScript switched themes with a CSS variable pair, so the code
    /// never lost its colour for a moment.
    Pending {
        dark: bool,
        generation: u64,
        previous: Option<Arc<Highlights>>,
    },
    Ready {
        dark: bool,
        styles: Arc<Highlights>,
    },
}

/// A comment being written: where it lands, the text field, and what went wrong.
struct Composer {
    target: CommentTarget,
    input: Entity<TextInput>,
    /// The message of a failed save, shown under the field. The composer stays open so
    /// nothing typed is lost.
    error: Option<SharedString>,
    _subscription: Subscription,
}

struct FileState {
    hunks: Arc<Vec<DiffHunk>>,
    line_count: usize,
    open: bool,
    /// Hash of the patch, which keys the highlight cache.
    patch_hash: u64,
    language: Option<&'static str>,
    /// Printable text of every line (tabs expanded, marker prefix), built the first
    /// time the file is open.
    texts: Option<Vec<Vec<SharedString>>>,
    /// The side-by-side rows of every hunk, built the first time split is shown.
    split: Option<Vec<Vec<SplitRow>>>,
    colour: Colour,
    threads: Vec<CommentThread>,
    /// Changes whenever a thread's content does, so a stale cached height is not kept.
    thread_revs: Vec<u64>,
    drafts: Vec<DraftComment>,
    composer: Option<Composer>,
}

/// A range being drawn: from the line the pointer went down on to where it is now.
#[derive(Clone, Copy)]
struct Selecting {
    file: usize,
    hunk: usize,
    side: DiffSide,
    from: usize,
    to: usize,
}

/// Which tint a line's gutter takes on one side, if anything there covers it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Coverage {
    Active,
    Pending,
    Thread,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Notice {
    Binary,
    NoDiff,
    Collapsed,
}

/// One row of the list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Header(usize),
    Notice(usize, Notice),
    Hunk(usize, usize),
    Meta(usize, usize, usize),
    /// A unified-layout code row: file, hunk, line.
    Line(usize, usize, usize),
    /// A side-by-side code row: file, hunk, row of that hunk's split rows.
    Row(usize, usize, usize),
    Thread(usize, usize, u64),
    Draft(usize, usize, u64),
    Composer(usize, u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Entry {
    item: Item,
    /// The last row of its file's section, which closes the rounded box.
    last: bool,
    /// The first code row under an open header, which carries the `border-t`.
    divider: bool,
    first_file: bool,
    last_file: bool,
}

// ---------------------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------------------

pub struct DiffView {
    item_id: String,
    refs: DiffRefs,
    source: Arc<Vec<DiffFile>>,
    files: Vec<FileState>,
    mode: DiffViewMode,
    drafts: Vec<DraftComment>,
    entries: Vec<Entry>,
    list: ListState,
    selecting: Option<Selecting>,
    focus: FocusHandle,
    state: Entity<AppState>,
    thread_cards: HashMap<String, (Entity<ThreadCard>, Subscription)>,
    draft_cards: HashMap<String, Entity<DraftCard>>,
    next_generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DiffEvent> for DiffView {}

impl Focusable for DiffView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

impl DiffView {
    pub fn new(
        item_id: String,
        files: Arc<Vec<DiffFile>>,
        inline_threads: Arc<Vec<CommentThread>>,
        refs: DiffRefs,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.global::<GlobalState>().0.clone();
        let (mode, drafts) = {
            let state = state.read(cx);
            (state.settings().diff_view, state.drafts(&item_id))
        };

        let states = files
            .iter()
            .map(|file| {
                let patch = file.patch.as_deref().unwrap_or("");
                let hunks = parse_patch(patch);
                let line_count = hunks.iter().map(|hunk| hunk.lines.len()).sum::<usize>();
                FileState {
                    hunks: Arc::new(hunks),
                    line_count,
                    // `useState(lineCount > 0 && lineCount <= AUTO_COLLAPSE_LINES)`
                    open: line_count > 0 && line_count <= AUTO_COLLAPSE_LINES,
                    patch_hash: hash_of(&patch),
                    language: language_for(&file.path),
                    texts: None,
                    split: None,
                    colour: Colour::Idle,
                    threads: Vec::new(),
                    thread_revs: Vec::new(),
                    drafts: Vec::new(),
                    composer: None,
                }
            })
            .collect();

        let subscriptions = vec![cx.observe(&state, |this, _, cx| this.sync_state(cx))];
        let mut view = DiffView {
            item_id,
            refs,
            source: files,
            files: states,
            mode,
            drafts: Vec::new(),
            entries: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            selecting: None,
            focus: cx.focus_handle(),
            state,
            thread_cards: HashMap::new(),
            draft_cards: HashMap::new(),
            next_generation: 0,
            _subscriptions: subscriptions,
        };
        view.assign_threads(&inline_threads);
        view.assign_drafts(drafts);
        view.rebuild(cx);
        view
    }

    /// The inline threads changed (a reply landed, one was resolved, the poll found
    /// news). Open cards keep their reply drafts and expanded state.
    pub fn set_threads(&mut self, inline_threads: Arc<Vec<CommentThread>>, cx: &mut Context<Self>) {
        self.assign_threads(&inline_threads);
        // Cards whose thread is gone are dropped; the rest are told their new content.
        let live: HashMap<&str, &CommentThread> = inline_threads
            .iter()
            .map(|thread| (thread.id.as_str(), thread))
            .collect();
        self.thread_cards
            .retain(|id, _| live.contains_key(id.as_str()));
        for (id, (card, _)) in &self.thread_cards {
            if let Some(thread) = live.get(id.as_str()) {
                card.update(cx, |card, cx| card.set_thread((*thread).clone(), cx));
            }
        }
        self.rebuild(cx);
        cx.notify();
    }

    /// Threads by file, the ones with a path and a line only (`byPath`).
    fn assign_threads(&mut self, threads: &[CommentThread]) {
        for file in &mut self.files {
            file.threads.clear();
            file.thread_revs.clear();
        }
        for (index, source) in self.source.iter().enumerate() {
            let file = &mut self.files[index];
            for thread in threads {
                if thread.path.as_deref() == Some(source.path.as_str()) && thread.line.is_some() {
                    file.thread_revs.push(hash_of(&format!("{thread:?}")));
                    file.threads.push(thread.clone());
                }
            }
        }
    }

    fn assign_drafts(&mut self, drafts: Vec<DraftComment>) {
        for (index, source) in self.source.iter().enumerate() {
            self.files[index].drafts = drafts
                .iter()
                .filter(|draft| draft.path == source.path)
                .cloned()
                .collect();
        }
        self.drafts = drafts;
    }

    /// AppState changed: only a different mode or different drafts matter here.
    fn sync_state(&mut self, cx: &mut Context<Self>) {
        let (mode, drafts) = {
            let state = self.state.read(cx);
            (state.settings().diff_view, state.drafts(&self.item_id))
        };
        if mode == self.mode && drafts == self.drafts {
            return;
        }
        self.mode = mode;
        if drafts != self.drafts {
            let live: HashMap<&str, &DraftComment> = drafts
                .iter()
                .map(|draft| (draft.id.as_str(), draft))
                .collect();
            self.draft_cards
                .retain(|id, _| live.contains_key(id.as_str()));
            for (id, card) in &self.draft_cards {
                if let Some(draft) = live.get(id.as_str()) {
                    card.update(cx, |card, cx| card.set_draft((*draft).clone(), cx));
                }
            }
            self.assign_drafts(drafts);
        }
        self.rebuild(cx);
        cx.notify();
    }

    // ----- the rows ------------------------------------------------------------------

    /// Recomputes the rows and tells the list only about the stretch that changed, so
    /// the scroll position and the measured heights of everything else stay.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let _ = cx;
        let split = self.mode == DiffViewMode::Split;
        for (index, file) in self.files.iter_mut().enumerate() {
            if file.open {
                if file.texts.is_none() {
                    file.texts = Some(prepare_texts(&file.hunks));
                }
                if split && file.split.is_none() {
                    file.split = Some(file.hunks.iter().map(to_split_rows).collect());
                }
            }
            let _ = index;
        }

        let mut items: Vec<(Item, bool)> = Vec::new();
        for (f, file) in self.files.iter().enumerate() {
            let source = &self.source[f];
            let start = items.len();
            items.push((Item::Header(f), false));
            if file.open {
                if source.binary || source.patch.as_deref().is_none_or(str::is_empty) {
                    let notice = if source.binary {
                        Notice::Binary
                    } else {
                        Notice::NoDiff
                    };
                    items.push((Item::Notice(f, notice), false));
                } else {
                    for (h, hunk) in file.hunks.iter().enumerate() {
                        items.push((Item::Hunk(f, h), false));
                        if split {
                            let rows = file.split.as_ref().map(|rows| rows[h].as_slice());
                            for (r, row) in rows.unwrap_or(&[]).iter().enumerate() {
                                items.push((Item::Row(f, h, r), false));
                                let (left, right) = (
                                    row.left.map(|l| &hunk.lines[l]),
                                    row.right.map(|l| &hunk.lines[l]),
                                );
                                // Threads: on the new line when there is one, else the old.
                                let threads =
                                    file.threads.iter().enumerate().filter(|(_, t)| {
                                        match (right, left) {
                                            (Some(right), _) => t.line == right.new_line,
                                            (None, Some(left)) => t.line == left.old_line,
                                            _ => false,
                                        }
                                    });
                                for (t, _) in threads {
                                    items.push((Item::Thread(f, t, file.thread_revs[t]), false));
                                }
                                let anchor = row.right.or(row.left).map(|l| &hunk.lines[l]);
                                push_drafts(&mut items, f, &file.drafts, anchor);
                                if let Some(composer) = &file.composer {
                                    let target = &composer.target;
                                    let active = right.is_some_and(|r| {
                                        target.new_line.is_some() && target.new_line == r.new_line
                                    }) || left.is_some_and(|l| {
                                        target.old_line.is_some() && target.old_line == l.old_line
                                    });
                                    if active {
                                        items.push((
                                            Item::Composer(
                                                f,
                                                hash_of(
                                                    &composer.error.as_ref().map(|e| e.to_string()),
                                                ),
                                            ),
                                            false,
                                        ));
                                    }
                                }
                            }
                        } else {
                            for (l, line) in hunk.lines.iter().enumerate() {
                                if line.kind == DiffLineKind::Meta {
                                    items.push((Item::Meta(f, h, l), false));
                                    continue;
                                }
                                items.push((Item::Line(f, h, l), false));
                                let at = line.new_line.or(line.old_line);
                                for (t, thread) in file.threads.iter().enumerate() {
                                    if thread.line == at {
                                        items
                                            .push((Item::Thread(f, t, file.thread_revs[t]), false));
                                    }
                                }
                                push_drafts(&mut items, f, &file.drafts, Some(line));
                                if let Some(composer) = &file.composer
                                    && same_line(&composer.target, line)
                                {
                                    items.push((
                                        Item::Composer(
                                            f,
                                            hash_of(
                                                &composer.error.as_ref().map(|e| e.to_string()),
                                            ),
                                        ),
                                        false,
                                    ));
                                }
                            }
                        }
                    }
                }
            } else if file.line_count > AUTO_COLLAPSE_LINES {
                items.push((Item::Notice(f, Notice::Collapsed), false));
            }
            let _ = start;
        }

        let file_count = self.files.len();
        let mut entries = Vec::with_capacity(items.len());
        for (index, (item, _)) in items.iter().enumerate() {
            let file_of = |item: &Item| match *item {
                Item::Header(f)
                | Item::Notice(f, _)
                | Item::Hunk(f, _)
                | Item::Meta(f, _, _)
                | Item::Line(f, _, _)
                | Item::Row(f, _, _)
                | Item::Thread(f, _, _)
                | Item::Draft(f, _, _)
                | Item::Composer(f, _) => f,
            };
            let f = file_of(item);
            let last = items
                .get(index + 1)
                .is_none_or(|(next, _)| file_of(next) != f);
            let divider = self.files[f].open
                && index > 0
                && matches!(items[index - 1].0, Item::Header(_))
                && !matches!(item, Item::Header(_));
            entries.push(Entry {
                item: *item,
                last,
                divider,
                first_file: f == 0,
                last_file: f + 1 == file_count,
            });
        }

        // Splice only what differs: the common head and tail keep their measurements.
        let old = &self.entries;
        let head = old.iter().zip(&entries).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(entries[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_len = old.len();
        let new_len = entries.len();
        self.entries = entries;
        if head != old_len || head != new_len {
            self.list
                .splice(head..old_len - tail, new_len - head - tail);
        }
    }

    // ----- highlighting ----------------------------------------------------------------

    /// Starts colouring a file the first time one of its rows is drawn (and again when
    /// the theme flipped since). The result is applied when it is ready.
    fn request_colour(&mut self, f: usize, dark: bool, cx: &mut Context<Self>) {
        let file = &mut self.files[f];
        match file.colour {
            Colour::Pending { dark: d, .. } | Colour::Ready { dark: d, .. } if d == dark => return,
            _ => {}
        }
        let Some(language) = file.language else {
            return;
        };
        if file.hunks.is_empty() {
            return;
        }
        self.next_generation += 1;
        let generation = self.next_generation;
        let previous = match std::mem::replace(&mut file.colour, Colour::Idle) {
            Colour::Ready { styles, .. } => Some(styles),
            Colour::Pending { previous, .. } => previous,
            Colour::Idle => None,
        };
        file.colour = Colour::Pending {
            dark,
            generation,
            previous,
        };

        let hunks = file.hunks.clone();
        let key = hash_of(&(file.patch_hash, language, dark));
        cx.spawn(async move |this, cx| {
            let styles = cx
                .background_executor()
                .spawn(async move { colour_hunks(key, &hunks, language, dark) })
                .await;
            this.update(cx, |this, cx| {
                let file = &mut this.files[f];
                if let Colour::Pending { generation: g, .. } = file.colour
                    && g == generation
                {
                    file.colour = Colour::Ready { dark, styles };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ----- comment composer and range selection -------------------------------------------

    /// The list row showing new-side line `line` of file `f`, in either layout.
    #[cfg(debug_assertions)]
    fn row_for_new_line(&self, f: usize, line: u32) -> Option<usize> {
        let hunks = &self.files.get(f)?.hunks;
        self.entries.iter().position(|entry| match entry.item {
            Item::Line(file, h, l) if file == f => hunks[h].lines[l].new_line == Some(line),
            Item::Row(file, h, r) if file == f => self.files[f]
                .split
                .as_ref()
                .and_then(|split| split[h][r].right)
                .is_some_and(|l| hunks[h].lines[l].new_line == Some(line)),
            _ => false,
        })
    }

    /// The diff steps of `REVIEWDECK_SCENE`: the comment button of a line, a scroll
    /// position, and editing the first draft. Debug builds only.
    #[cfg(debug_assertions)]
    fn apply_scene(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::scene::{self, Kind, Step};

        // The layout step changes the rows these steps address.
        // And a layout the settings changed has to have reached this view first.
        if scene::deck_pending(cx)
            || scene::pending(cx, Kind::Layout).is_some()
            || self.state.read(cx).settings().diff_view != self.mode
        {
            return;
        }
        if let Some(Step::Comment(side, line)) = scene::pending(cx, Kind::Comment) {
            scene::mark_done(cx, Kind::Comment);
            let found = self.files.first().and_then(|file| {
                file.hunks.iter().enumerate().find_map(|(h, hunk)| {
                    hunk.lines
                        .iter()
                        .position(|l| line_on(l, side) == Some(line))
                        .map(|l| (h, l))
                })
            });
            match found {
                Some((h, l)) if self.files[0].open => {
                    self.pick(0, h, side, l, window, cx);
                }
                _ => eprintln!("REVIEWDECK_SCENE: no commentable line {line} in the first file"),
            }
        }
        if let Some(Step::Scroll(line)) = scene::pending(cx, Kind::Scroll) {
            scene::mark_done(cx, Kind::Scroll);
            match self.row_for_new_line(0, line) {
                Some(ix) => self.list.scroll_to(gpui::ListOffset {
                    item_ix: ix,
                    offset_in_item: px(0.),
                }),
                None => eprintln!("REVIEWDECK_SCENE: no row for new line {line} in the first file"),
            }
        }
        if scene::pending(cx, Kind::DraftEdit).is_some() && !self.draft_cards.is_empty() {
            scene::mark_done(cx, Kind::DraftEdit);
            let first = self.files.first().and_then(|file| file.drafts.first());
            if let Some(card) = first.and_then(|draft| self.draft_cards.get(&draft.id)) {
                card.update(cx, |card, cx| card.start_edit(window, cx));
            }
        }
    }

    /// `setTarget`: opens the composer on a target, moves it, or closes it. The text
    /// typed so far is kept while the composer stays on the same line.
    fn set_target(
        &mut self,
        f: usize,
        target: Option<CommentTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = target else {
            self.files[f].composer = None;
            self.rebuild(cx);
            cx.notify();
            return;
        };
        let placeholder = placeholder_for(&target);
        if let Some(composer) = &mut self.files[f].composer
            && composer.target.new_line == target.new_line
            && composer.target.old_line == target.old_line
        {
            composer.target = target;
            composer
                .input
                .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
            // Pressing the gutter button took the focus (so Escape could cancel the
            // drag); the field gets it back, as the browser never took it away.
            composer.input.read(cx).focus(window);
            self.rebuild(cx);
            cx.notify();
            return;
        }

        let input = cx.new(|cx| TextInput::new(cx).multi_line(3, 3).placeholder(placeholder));
        let subscription =
            cx.subscribe(
                &input,
                move |this, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Changed => cx.notify(),
                    TextInputEvent::Submit => this.send(f, cx),
                    TextInputEvent::Cancel => this.close_composer(f, cx),
                },
            );
        input.read(cx).focus(window);
        self.files[f].composer = Some(Composer {
            target,
            input,
            error: None,
            _subscription: subscription,
        });
        self.rebuild(cx);
        cx.notify();
    }

    fn close_composer(&mut self, f: usize, cx: &mut Context<Self>) {
        if self.files[f].composer.take().is_some() {
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// The composer's `send`: the text becomes a draft on the pull request, written
    /// against the refs this diff was loaded with.
    fn send(&mut self, f: usize, cx: &mut Context<Self>) {
        let Some(composer) = &self.files[f].composer else {
            return;
        };
        let body = composer.input.read(cx).text().trim().to_string();
        if body.is_empty() {
            return;
        }
        let target = &composer.target;
        let draft = NewDraft {
            item_id: self.item_id.clone(),
            body,
            path: target.path.clone(),
            new_line: target.new_line,
            old_line: target.old_line,
            range: target.range,
            refs: self.refs.clone(),
        };
        let result = self
            .state
            .update(cx, |state, cx| state.add_draft(draft, cx));
        match result {
            Ok(_) => {
                self.sync_state(cx);
                self.close_composer(f, cx);
            }
            // PullView.tsx toasted the failure and left the composer open with what
            // was typed; the message also stays under the composer until it is closed.
            Err(error) => {
                toast(cx, ToastKind::Bad, error.to_string());
                if let Some(composer) = &mut self.files[f].composer {
                    composer.error = Some(error.to_string().into());
                }
                self.rebuild(cx);
                cx.notify();
            }
        }
    }

    /// `selection.begin`: the pointer went down on a line's comment button. Shift
    /// stretches the open composer to take this line in as well, so a range can be
    /// made without a steady hand.
    #[allow(clippy::too_many_arguments)]
    fn begin(
        &mut self,
        f: usize,
        h: usize,
        side: DiffSide,
        l: usize,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Dragging along a gutter must not also drag-select the code beside it; and
        // the focus is what lets Escape cancel the drag.
        window.focus(&self.focus);
        let file = &self.files[f];
        let mut from = l;
        if shift
            && let Some(composer) = &file.composer
            && let Some(far) = stretch(&file.hunks[h], side, &composer.target, l)
        {
            from = far;
        }
        self.selecting = Some(Selecting {
            file: f,
            hunk: h,
            side,
            from,
            to: l,
        });
        cx.notify();
    }

    /// `selection.extend`: the pointer entered a gutter while a range was being drawn.
    fn extend(&mut self, f: usize, h: usize, side: DiffSide, l: usize, cx: &mut Context<Self>) {
        let Some(current) = &mut self.selecting else {
            return;
        };
        if current.file != f || current.hunk != h || current.side != side || current.to == l {
            return;
        }
        let has_number = self.files[f].hunks[h]
            .lines
            .get(l)
            .is_some_and(|line| line_on(line, side).is_some());
        if !has_number {
            return;
        }
        current.to = l;
        cx.notify();
    }

    /// `selection.pick`: the button was activated from the keyboard, which has no drag
    /// to wait for.
    fn pick(
        &mut self,
        f: usize,
        h: usize,
        side: DiffSide,
        l: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.files[f]
            .hunks
            .get(h)
            .and_then(|hunk| hunk.lines.get(l))
            .and_then(|line| side_target(&self.source[f].path, line, side));
        self.set_target(f, target, window, cx);
    }

    /// The pointer went up anywhere: the range being drawn becomes the composer's
    /// target, or nothing when it cannot make one.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selecting) = self.selecting.take() else {
            return;
        };
        let target = range_target(
            &self.source[selecting.file].path,
            &self.files[selecting.file].hunks,
            selecting.side,
            LineRef {
                hunk: selecting.hunk,
                line: selecting.from,
            },
            LineRef {
                hunk: selecting.hunk,
                line: selecting.to,
            },
        );
        self.set_target(selecting.file, target, window, cx);
    }

    fn selection_covers(&self, f: usize, side: DiffSide, line: &DiffLine) -> bool {
        let Some(selecting) = &self.selecting else {
            return false;
        };
        if selecting.file != f || selecting.side != side {
            return false;
        }
        let hunk = &self.files[f].hunks[selecting.hunk];
        let (Some(number), Some(a), Some(b)) = (
            line_on(line, side),
            hunk.lines
                .get(selecting.from)
                .and_then(|l| line_on(l, side)),
            hunk.lines.get(selecting.to).and_then(|l| line_on(l, side)),
        ) else {
            return false;
        };
        number >= a.min(b) && number <= a.max(b)
    }

    /// What covers a line on one side, strongest first: the range being drawn or the
    /// open composer, then a pending draft, then a thread already on the host. Only
    /// things reaching over several lines count for the last two - a single-line
    /// thread is marked by its card, and tinting its gutter as well would say nothing.
    fn coverage(&self, f: usize, side: DiffSide, line: &DiffLine) -> Option<Coverage> {
        if self.selection_covers(f, side, line) {
            return Some(Coverage::Active);
        }
        let file = &self.files[f];
        if let Some(composer) = &file.composer {
            let target = &composer.target;
            let anchor = anchor_of(target.new_line, target.old_line, target.range);
            if anchor.is_some_and(|a| a.side == side && covers_line(a, line)) {
                return Some(Coverage::Active);
            }
        }
        for draft in &file.drafts {
            if draft.range.is_none() {
                continue;
            }
            let anchor = anchor_of(draft.new_line, draft.old_line, draft.range);
            if anchor.is_some_and(|a| a.side == side && covers_line(a, line)) {
                return Some(Coverage::Pending);
            }
        }
        for thread in &file.threads {
            let (Some(start), Some(end)) = (thread.start_line, thread.line) else {
                continue;
            };
            if thread.side != Some(side) {
                continue;
            }
            let anchor = LineAnchor {
                side,
                line: end,
                start_line: Some(start),
            };
            if covers_line(anchor, line) {
                return Some(Coverage::Thread);
            }
        }
        None
    }

    // ----- rendering --------------------------------------------------------------------

    fn render_entry(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.entries.get(ix).copied() else {
            return div().into_any_element();
        };
        let colors = cx.theme().colors;
        let dark = cx.theme().dark;

        let (body, is_header) = match entry.item {
            Item::Header(f) => (self.header(f, ix, cx), true),
            Item::Notice(f, notice) => (self.notice(f, notice, cx), false),
            Item::Hunk(f, h) => (self.hunk_header(f, h, cx), false),
            Item::Meta(f, h, l) => (self.meta(f, h, l, cx), false),
            Item::Line(f, h, l) => {
                self.request_colour(f, dark, cx);
                (self.unified_row(f, h, l, ix, cx), false)
            }
            Item::Row(f, h, r) => {
                self.request_colour(f, dark, cx);
                (self.split_row(f, h, r, ix, cx), false)
            }
            Item::Thread(f, t, _) => (self.thread_row(f, t, window, cx), false),
            Item::Draft(f, d, _) => (self.draft_row(f, d, window, cx), false),
            Item::Composer(f, _) => (self.composer_row(f, cx), false),
        };

        // The section's rounded glass box, cut into rows: each carries the side
        // borders, the first the top edge and corners, the last the bottom ones.
        let radius = rpx(radius::LG);
        let boxed = div()
            .w_full()
            .bg(colors.surface)
            .border_l_1()
            .border_r_1()
            .border_color(colors.border)
            .when(is_header, |d| d.border_t_1().rounded_t(radius))
            .when(entry.divider, |d| d.border_t_1())
            .when(entry.last, |d| {
                d.border_b_1().rounded_b(radius).overflow_hidden()
            })
            .child(body);
        div()
            .w_full()
            .px(rpx(OUTER_PAD))
            .when(is_header && entry.first_file, |d| d.pt(rpx(OUTER_PAD)))
            .when(entry.last, |d| {
                d.pb(rpx(if entry.last_file { OUTER_PAD } else { FILE_GAP }))
            })
            .child(boxed)
            .into_any_element()
    }

    fn header(&mut self, f: usize, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let file = &self.source[f];
        let open = self.files[f].open;
        let icon = match file.status {
            FileStatus::Added => Some(IconName::FilePlus2),
            FileStatus::Removed => Some(IconName::FileMinus2),
            FileStatus::Renamed => Some(IconName::FileSymlink),
            _ => None,
        };
        let renamed = file.status == FileStatus::Renamed && file.old_path != file.path;

        // `truncate`: one line, cut with an ellipsis when the path outgrows the header.
        // The old name of a rename is part of the same run of text, in the muted colour.
        let prefix = if renamed {
            format!("{} \u{2192} ", file.old_path)
        } else {
            String::new()
        };
        let mut shown = StyledText::new(format!("{prefix}{}", file.path));
        if !prefix.is_empty() {
            shown = shown.with_highlights([(
                0..prefix.len(),
                HighlightStyle {
                    color: Some(colors.muted_foreground),
                    ..Default::default()
                },
            )]);
        }
        let path = div()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(mono_font())
            .text_size(rpx(CODE_SIZE))
            .line_height(rpx(CODE_LINE))
            .font_weight(FontWeight::MEDIUM)
            .child(shown);

        div()
            .flex()
            .items_center()
            .gap(rpx(8.))
            .px(rpx(10.))
            .py(rpx(8.))
            .child(
                div()
                    .id(("diff-file", ix))
                    .debug_selector(|| format!("diff-file-{f}"))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(rpx(6.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let file = &mut this.files[f];
                        file.open = !file.open;
                        this.rebuild(cx);
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(14.)
                        .color(colors.muted_foreground),
                    )
                    .when_some(icon, |d, icon| {
                        d.child(Icon::new(icon).size(14.).color(colors.muted_foreground))
                    })
                    .child(path),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(rpx(SMALL_SIZE * 0.6))
                    .font_family(mono_font())
                    .text_size(rpx(SMALL_SIZE))
                    .line_height(rpx(SMALL_LINE))
                    .child(
                        div()
                            .text_color(colors.ok)
                            .child(format!("+{}", file.additions)),
                    )
                    .child(
                        div()
                            .text_color(colors.bad)
                            .child(format!("\u{2212}{}", file.deletions)),
                    ),
            )
            .into_any_element()
    }

    fn notice(&self, f: usize, notice: Notice, cx: &App) -> AnyElement {
        let colors = cx.theme().colors;
        match notice {
            Notice::Binary | Notice::NoDiff => div()
                .px(rpx(12.))
                .py(rpx(16.))
                .text_center()
                .text_size(rpx(12.))
                .text_color(colors.muted_foreground)
                .child(if notice == Notice::Binary {
                    "Binary file - no diff to show."
                } else {
                    "No diff available for this file."
                })
                .into_any_element(),
            Notice::Collapsed => div()
                .px(rpx(12.))
                .py(rpx(8.))
                .text_size(rpx(11.5))
                .text_color(colors.muted_foreground)
                .child(format!(
                    "{} changed lines - collapsed for speed.",
                    group_thousands(self.files[f].line_count)
                ))
                .into_any_element(),
        }
    }

    fn hunk_header(&self, f: usize, h: usize, cx: &App) -> AnyElement {
        let colors = cx.theme().colors;
        no_ligatures(div())
            .w_full()
            .bg(colors.muted)
            .px(rpx(12.))
            .py(rpx(4.))
            .font_family(mono_font())
            .text_size(rpx(SMALL_SIZE))
            .line_height(rpx(SMALL_LINE))
            .text_color(colors.muted_foreground)
            .child(SharedString::from(self.files[f].hunks[h].header.clone()))
            .into_any_element()
    }

    fn meta(&self, f: usize, h: usize, l: usize, cx: &App) -> AnyElement {
        let colors = cx.theme().colors;
        let content = self.files[f].hunks[h]
            .lines
            .get(l)
            .map(|line| line.content.clone())
            .unwrap_or_default();
        div()
            .w_full()
            .px(rpx(12.))
            .py(rpx(2.))
            .font_family(mono_font())
            .text_size(rpx(SMALL_SIZE))
            .line_height(rpx(SMALL_LINE))
            .italic()
            .text_color(colors.muted_foreground)
            .child(content)
            .into_any_element()
    }

    /// One line number cell, with the comment button that shows while the row is
    /// hovered. `pick` is `(file, hunk, side, line)` when the line can take a comment
    /// on this side.
    #[allow(clippy::too_many_arguments)]
    fn gutter(
        &self,
        ix: usize,
        column: usize,
        value: Option<u32>,
        background: Option<Hsla>,
        pick: Option<(usize, usize, DiffSide, usize)>,
        group: &SharedString,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = cx.theme().colors;
        let mut cell = div()
            .relative()
            .flex_none()
            .w(rpx(GUTTER_WIDTH))
            .px(rpx(6.))
            .border_r_1()
            .border_color(colors.border.opacity(0.6))
            .text_right()
            .font_family(mono_font())
            .text_size(rpx(SMALL_SIZE))
            .line_height(rpx(SMALL_LINE))
            .text_color(colors.diff_gutter)
            .when_some(background, |d, bg| d.bg(bg))
            .children(value.map(|value| SharedString::from(value.to_string())));

        let Some((f, h, side, l)) = pick else {
            return cell;
        };
        let side_name = if side == Side::Old { "old" } else { "new" };
        if let Some(value) = value {
            cell = cell.debug_selector(|| format!("diff-gutter-{f}-{side_name}-{value}"));
        }
        // The pointer arriving over this cell is how a range grows. Only while one is
        // being drawn is anyone listening.
        if self.selecting.is_some() {
            cell = cell.on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                this.extend(f, h, side, l, cx);
            }));
        }
        if value.is_none() {
            return cell;
        }
        cell.child(
            div()
                .id(("diff-comment", ix * 4 + column))
                .debug_selector(|| format!("diff-comment-{f}-{side_name}-{}", value.unwrap_or(0)))
                .absolute()
                .left(rpx(-2.))
                .top(relative(0.5))
                .mt(rpx(-8.))
                .size(rpx(16.))
                .items_center()
                .justify_center()
                .rounded(rpx(4.))
                .bg(colors.info)
                .cursor_pointer()
                .shadow_sm()
                .flex()
                // Visibility, not `display: none`: gpui works a hover style out once in
                // prepaint and again in paint, and a display flip between the two paints
                // children that were never prepainted (a panic). Absolutely positioned,
                // so taking space while invisible changes nothing.
                .invisible()
                .group_hover(group.clone(), |style| style.visible())
                .hover(|style| style.opacity(0.9))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.begin(f, h, side, l, event.modifiers.shift, window, cx);
                        cx.stop_propagation();
                    }),
                )
                // A click with no pointer behind it came from the keyboard.
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.is_keyboard() {
                        this.pick(f, h, side, l, window, cx);
                    }
                }))
                .child(Icon::new(IconName::Plus).size(12.).color(gpui::white())),
        )
    }

    /// The code column: the base film behind the text, the row tint over it, and the
    /// text itself. The column paints the tint itself, over its own base film, so
    /// contrast against a syntax token is bounded by the film rather than by whatever
    /// the desktop shows through.
    fn code_cell(&self, f: usize, h: usize, l: usize, cx: &App) -> Div {
        let colors = cx.theme().colors;
        let file = &self.files[f];
        let Some(line) = file.hunks.get(h).and_then(|hunk| hunk.lines.get(l)) else {
            return div().flex_1().min_w_0().bg(colors.diff_code_base);
        };
        let tint = match line.kind {
            DiffLineKind::Add => Some(colors.diff_add),
            DiffLineKind::Del => Some(colors.diff_del),
            _ => None,
        };
        let marker = match line.kind {
            DiffLineKind::Add => "+",
            DiffLineKind::Del => "\u{2212}",
            _ => " ",
        };

        let text = file
            .texts
            .as_ref()
            .and_then(|texts| texts.get(h))
            .and_then(|lines| lines.get(l))
            .cloned()
            .unwrap_or_default();
        let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let (Colour::Ready { styles, .. }
        | Colour::Pending {
            previous: Some(styles),
            ..
        }) = &file.colour
        else {
            return self.code_body(colors, tint, marker, WrappedCode::new(text, highlights));
        };
        if let Some(runs) = styles
            .get(h)
            .and_then(|lines| lines.get(l))
            .and_then(Option::as_ref)
        {
            highlights = runs.iter().cloned().collect();
        }
        self.code_body(colors, tint, marker, WrappedCode::new(text, highlights))
    }

    fn code_body(
        &self,
        colors: crate::ui::theme::Colors,
        tint: Option<Hsla>,
        marker: &'static str,
        styled: WrappedCode,
    ) -> Div {
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .bg(colors.diff_code_base)
            .when_some(tint, |d, tint| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .bg(tint),
                )
            })
            .child(
                no_ligatures(div())
                    .relative()
                    .px(rpx(8.))
                    .font_family(mono_font())
                    .text_size(rpx(CODE_SIZE))
                    .line_height(rpx(CODE_LINE))
                    .text_color(colors.foreground)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(rpx(8.))
                            .text_color(colors.diff_gutter)
                            .child(marker),
                    )
                    .child(styled),
            )
    }

    fn unified_row(
        &self,
        f: usize,
        h: usize,
        l: usize,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let Some(line) = self.files[f]
            .hunks
            .get(h)
            .and_then(|hunk| hunk.lines.get(l))
        else {
            return div().into_any_element();
        };
        let tint = match line.kind {
            DiffLineKind::Add => Some(colors.diff_add),
            DiffLineKind::Del => Some(colors.diff_del),
            _ => None,
        };
        let group = SharedString::from(format!("diff-row-{ix}"));
        let path = &self.source[f].path;

        let mut row = div()
            .group(group.clone())
            .flex()
            .w_full()
            .when_some(tint, |d, t| d.bg(t));
        for (column, side) in [(0, Side::Old), (1, Side::New)] {
            let pick = side_target(path, line, side).map(|_| (f, h, side, l));
            let background = self.coverage(f, side, line).map(|c| coverage_colour(c, cx));
            row = row.child(self.gutter(
                ix,
                column,
                line_on(line, side),
                background,
                pick,
                &group,
                cx,
            ));
        }
        row.child(self.code_cell(f, h, l, cx)).into_any_element()
    }

    fn split_row(
        &self,
        f: usize,
        h: usize,
        r: usize,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let file = &self.files[f];
        let Some(row) = file
            .split
            .as_ref()
            .and_then(|rows| rows.get(h))
            .and_then(|rows| rows.get(r))
            .copied()
        else {
            return div().into_any_element();
        };
        let hunk = &file.hunks[h];
        let group = SharedString::from(format!("diff-row-{ix}"));
        let path = &self.source[f].path;
        // A context line occupies both sides; the same index is used for both.
        let paired = row.left == row.right;

        let mut out = div().group(group.clone()).flex().w_full();
        for (column, side, index) in [(0, Side::Old, row.left), (2, Side::New, row.right)] {
            let line = index.map(|i| &hunk.lines[i]);
            let tint = match side {
                Side::Old => colors.diff_del,
                Side::New => colors.diff_add,
            };
            let background = line
                .and_then(|line| self.coverage(f, side, line))
                .map(|c| coverage_colour(c, cx))
                .or((line.is_some() && !paired).then_some(tint));
            let pick = match (line, index) {
                (Some(line), Some(i)) => side_target(path, line, side).map(|_| (f, h, side, i)),
                _ => None,
            };
            let value = line.and_then(|line| line_on(line, side));
            out = out.child(self.gutter(ix, column, value, background, pick, &group, cx));
            out = match index {
                Some(i) => out.child(self.code_cell(f, h, i, cx)),
                // The empty half is still part of the code column.
                None => out.child(div().flex_1().min_w_0().bg(colors.diff_code_base)),
            };
        }
        out.into_any_element()
    }

    fn thread_row(
        &mut self,
        f: usize,
        t: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(thread) = self.files[f].threads.get(t).cloned() else {
            return div().into_any_element();
        };
        let id = thread.id.clone();
        if !self.thread_cards.contains_key(&id) {
            let item_id = self.item_id.clone();
            let card = cx.new(|cx| ThreadCard::new(item_id, thread, true, window, cx));
            let subscription = cx.subscribe(&card, |_, _, event: &ThreadEvent, cx| match event {
                ThreadEvent::Changed => cx.emit(DiffEvent::ThreadsChanged),
            });
            self.thread_cards.insert(id.clone(), (card, subscription));
        }
        let card = self.thread_cards[&id].0.clone();
        self.prose(AnyView::from(card), cx)
    }

    fn draft_row(
        &mut self,
        f: usize,
        d: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(draft) = self.files[f].drafts.get(d).cloned() else {
            return div().into_any_element();
        };
        let id = draft.id.clone();
        let card = self
            .draft_cards
            .entry(id)
            .or_insert_with(|| cx.new(|cx| DraftCard::new(draft, window, cx)))
            .clone();
        self.prose(AnyView::from(card), cx)
    }

    /// Cards inside the diff are set in the UI font, not the table's monospace.
    fn prose(&self, card: AnyView, cx: &App) -> AnyElement {
        div()
            .w_full()
            .font_family(UI_FONT)
            .text_size(rpx(crate::ui::theme::BASE_TEXT))
            .text_color(cx.theme().colors.foreground)
            .child(card)
            .into_any_element()
    }

    fn composer_row(&self, f: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let Some(composer) = &self.files[f].composer else {
            return div().into_any_element();
        };
        let target = &composer.target;
        let empty = composer.input.read(cx).text().trim().is_empty();
        let line = target.new_line.or(target.old_line).unwrap_or(0);
        let heading = match target.range {
            Some(range) => format!(
                "Commenting on lines {}\u{2013}{} of",
                range.start_line, line
            ),
            None => format!("Commenting on line {line} of"),
        };

        div()
            .debug_selector(|| format!("diff-composer-{f}"))
            .m(rpx(6.))
            .p(rpx(8.))
            .rounded(rpx(radius::MD))
            .glass_quiet(cx)
            .font_family(UI_FONT)
            .text_size(rpx(13.))
            .line_height(rpx(21.))
            .text_color(colors.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(6.))
                    .mb(rpx(6.))
                    .text_size(rpx(SMALL_SIZE))
                    .line_height(rpx(16.))
                    .text_color(colors.muted_foreground)
                    .child(
                        Icon::new(IconName::MessageSquarePlus)
                            .size(14.)
                            .color(colors.muted_foreground),
                    )
                    .child(heading)
                    .child(
                        div()
                            .font_family(mono_font())
                            .text_size(rpx(SMALL_SIZE))
                            // `.mono`'s 1.55 line height makes the label row 17px tall.
                            .line_height(rpx(SMALL_LINE))
                            .child(target.path.clone()),
                    ),
            )
            .child(composer.input.clone())
            .when_some(composer.error.clone(), |d, error| {
                d.child(
                    div()
                        .mt(rpx(6.))
                        .text_size(rpx(12.))
                        .line_height(rpx(16.))
                        .text_color(colors.bad)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(rpx(6.))
                    // `mt-1.5` plus the 6px the browser leaves under an inline-block
                    // textarea (its baseline gap).
                    .mt(rpx(12.))
                    .child(
                        div()
                            .mr_auto()
                            .text_size(rpx(10.5))
                            .line_height(rpx(16.))
                            .text_color(colors.muted_foreground)
                            .child("\u{2318}\u{21b5} to send"),
                    )
                    .child(
                        Button::new(("diff-composer-cancel", f))
                            .size(ButtonSize::Sm)
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.close_composer(f, cx);
                            }))
                            .child("Cancel"),
                    )
                    .child(
                        Button::new(("diff-composer-send", f))
                            .size(ButtonSize::Sm)
                            .variant(ButtonVariant::Default)
                            .disabled(empty)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.send(f, cx);
                            }))
                            .child("Comment"),
                    ),
            )
            .into_any_element()
    }
}

impl Render for DiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(debug_assertions)]
        self.apply_scene(_window, cx);
        let colors = cx.theme().colors;
        if self.files.is_empty() {
            return div()
                .px(rpx(20.))
                .py(rpx(40.))
                .text_center()
                .text_size(rpx(13.))
                .text_color(colors.muted_foreground)
                .child("No file changes to show.")
                .into_any_element();
        }

        div()
            .id("diff-view")
            .size_full()
            .key_context("DiffView")
            .track_focus(&self.focus)
            // The range being drawn is dropped on Escape, as the window-level listener did.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.selecting.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| this.finish(window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| this.finish(window, cx)),
            )
            .child({
                // The pane's own scrollbar (`::-webkit-scrollbar`): a classic 10px
                // track that takes its width from the content, with the slim thumb in it.
                let thumb = thumb(
                    self.list.scroll_px_offset_for_scrollbar().y,
                    self.list.max_offset_for_scrollbar().height,
                    self.list.viewport_bounds().size.height,
                )
                .map(|(top, height)| {
                    div()
                        .absolute()
                        .top(top)
                        .right(rpx(3.))
                        .w(rpx(4.))
                        .h(height)
                        .rounded_full()
                        .bg(colors.border_strong)
                });
                div()
                    .relative()
                    .size_full()
                    .child(
                        div().size_full().pr(rpx(SCROLLBAR_WIDTH)).child(
                            list(
                                self.list.clone(),
                                cx.processor(|this, ix, window, cx| {
                                    this.render_entry(ix, window, cx)
                                }),
                            )
                            .size_full(),
                        ),
                    )
                    .children(thumb)
            })
            .into_any_element()
    }
}

// ---------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------

fn push_drafts(
    items: &mut Vec<(Item, bool)>,
    f: usize,
    drafts: &[DraftComment],
    line: Option<&DiffLine>,
) {
    let Some(line) = line else {
        return;
    };
    for (d, draft) in drafts.iter().enumerate() {
        // The drafts left on one line, on whichever side of the diff it is.
        let on_line = match draft.new_line {
            Some(number) => line.new_line == Some(number),
            None => draft.old_line.is_some() && draft.old_line == line.old_line,
        };
        if on_line {
            items.push((Item::Draft(f, d, hash_of(&(&draft.id, &draft.body))), false));
        }
    }
}

fn coverage_colour(coverage: Coverage, cx: &App) -> Hsla {
    let colors = cx.theme().colors;
    match coverage {
        Coverage::Active => colors.info.opacity(0.30),
        Coverage::Pending => colors.info.opacity(0.15),
        Coverage::Thread => colors.diff_gutter.opacity(0.15),
    }
}

fn placeholder_for(target: &CommentTarget) -> &'static str {
    if target.range.is_some() {
        "Leave a note on these lines\u{2026}"
    } else {
        "Leave a note on this line\u{2026}"
    }
}

/// Does an open composer belong to this exact line?
fn same_line(target: &CommentTarget, line: &DiffLine) -> bool {
    if let Some(number) = target.new_line {
        return line.new_line == Some(number);
    }
    if let Some(number) = target.old_line {
        return line.old_line == Some(number);
    }
    false
}

/// Where something that stands on one line and may reach back over others sits.
fn anchor_of(
    new_line: Option<u32>,
    old_line: Option<u32>,
    range: Option<LineRange>,
) -> Option<LineAnchor> {
    let start_line = range.map(|range| range.start_line);
    if let Some(line) = new_line {
        return Some(LineAnchor {
            side: Side::New,
            line,
            start_line,
        });
    }
    old_line.map(|line| LineAnchor {
        side: Side::Old,
        line,
        start_line,
    })
}

/// The far end of an open composer, when it can be stretched to another line: the
/// line of it furthest from the one clicked, so the new range takes in both. Nothing
/// when the composer is on the other side or in another hunk.
fn stretch(
    hunk: &DiffHunk,
    side: DiffSide,
    target: &CommentTarget,
    clicked: usize,
) -> Option<usize> {
    let anchor = anchor_of(target.new_line, target.old_line, target.range)?;
    if anchor.side != side {
        return None;
    }
    let number = line_on(hunk.lines.get(clicked)?, side)?;
    let far = if number < anchor.line {
        anchor.line
    } else {
        anchor.start_line.unwrap_or(anchor.line)
    };
    hunk.lines
        .iter()
        .position(|line| line_on(line, side) == Some(far))
}

/// `1234` as `1,234`, which is what `toLocaleString` gives in the default locale.
fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Tab stops every this many columns: what the reference screenshots of the
/// Electron app show for the fixture's Go tabs. gpui would
/// draw a tab as a stop of its own, so tabs are expanded.
const TAB_SIZE: usize = 4;

/// The text a line is shown as: the marker's blank prefix, then the content with tabs
/// expanded, and - when there were tabs - the map from each byte offset of the content
/// to its offset in that text (prefix included).
fn expand(line: &str) -> (String, Option<Vec<usize>>) {
    let mut out = String::with_capacity(MARKER_PREFIX.len() + line.len());
    out.push_str(MARKER_PREFIX);
    if !line.contains('\t') {
        out.push_str(line);
        return (out, None);
    }
    let mut map = vec![0; line.len() + 1];
    // Stops are counted from the line's start, marker included, as the browser did.
    let mut column = MARKER_PREFIX.len();
    for (at, ch) in line.char_indices() {
        map[at] = out.len();
        if ch == '\t' {
            let stop = TAB_SIZE - column % TAB_SIZE;
            out.extend(std::iter::repeat_n(' ', stop));
            column += stop;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    map[line.len()] = out.len();
    (out, Some(map))
}

fn prepare_texts(hunks: &[DiffHunk]) -> Vec<Vec<SharedString>> {
    hunks
        .iter()
        .map(|hunk| {
            hunk.lines
                .iter()
                .map(|line| SharedString::from(expand(&line.content).0))
                .collect()
        })
        .collect()
}

/// A line's tokens as sorted, non-overlapping highlights on its expanded text. A token
/// that does not fit (out of order, past the end, or off a character boundary) is
/// dropped rather than trusted, so a bad token costs colour, never a panic.
fn token_highlights(
    tokens: &[Token],
    text: &str,
    original_len: usize,
    map: Option<&[usize]>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let at = |offset: usize| -> usize {
        let offset = offset.min(original_len);
        match map {
            Some(map) => map.get(offset).copied().unwrap_or(text.len()),
            None => (offset + MARKER_PREFIX.len()).min(text.len()),
        }
    };
    let mut out = Vec::with_capacity(tokens.len());
    let mut previous_end = 0;
    for token in tokens {
        let start = at(token.range.start).max(previous_end);
        let end = at(token.range.end);
        if end <= start || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        previous_end = end;
        out.push((
            start..end,
            HighlightStyle {
                color: Some(packed(token.color)),
                background_color: token.background.map(packed),
                ..Default::default()
            },
        ));
    }
    out
}

const COLOUR_CACHE_LIMIT: usize = 64;

static COLOUR_CACHE: LazyLock<Mutex<HashMap<u64, Arc<Highlights>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Colours for every line of a file's hunks. Blocking: runs on the background
/// executor, and what it works out is kept so reopening the same pull request is free.
fn colour_hunks(key: u64, hunks: &[DiffHunk], language: &str, dark: bool) -> Arc<Highlights> {
    if let Some(hit) = COLOUR_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return hit.clone();
    }
    let tokens = highlight::highlight_hunks(hunks, language, dark);
    let styles: Highlights = hunks
        .iter()
        .zip(tokens)
        .map(|(hunk, lines)| {
            hunk.lines
                .iter()
                .zip(lines)
                .map(|(line, tokens)| {
                    let tokens = tokens?;
                    let (text, map) = expand(&line.content);
                    let runs = token_highlights(&tokens, &text, line.content.len(), map.as_deref());
                    (!runs.is_empty()).then(|| Arc::new(runs))
                })
                .collect()
        })
        .collect();
    let styles = Arc::new(styles);
    let mut cache = COLOUR_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= COLOUR_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, styles.clone());
    styles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: DiffLineKind, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            content: String::new(),
            old_line: old,
            new_line: new,
            old_pos: old,
            new_pos: new,
        }
    }

    #[test]
    fn expand_prefixes_the_marker_and_maps_tabs() {
        let (text, map) = expand("a\tb");
        assert_eq!(text, "  a b");
        let map = map.expect("a tab gives a map");
        assert_eq!(text.as_bytes()[map[2]], b'b');
        let (plain, none) = expand("xy");
        assert_eq!(plain, "  xy");
        assert!(none.is_none());
    }

    #[test]
    fn tokens_land_after_the_prefix() {
        let token = Token {
            range: 0..2,
            color: 0xff00_00ff,
            font_style: Default::default(),
            background: None,
        };
        let (text, map) = expand("xy");
        let runs = token_highlights(&[token], &text, 2, map.as_deref());
        assert_eq!(runs[0].0, 2..4);
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1234), "1,234");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn a_composer_belongs_to_its_own_line_only() {
        let target = CommentTarget {
            path: "a".into(),
            new_line: Some(4),
            old_line: None,
            range: None,
        };
        assert!(same_line(&target, &line(DiffLineKind::Add, None, Some(4))));
        assert!(!same_line(&target, &line(DiffLineKind::Del, Some(4), None)));
    }

    #[test]
    fn stretching_takes_in_both_ends() {
        let hunk = DiffHunk {
            header: String::new(),
            old_start: 1,
            old_count: 0,
            new_start: 1,
            new_count: 5,
            lines: (1..=5)
                .map(|n| line(DiffLineKind::Add, None, Some(n)))
                .collect(),
        };
        let target = CommentTarget {
            path: "a".into(),
            new_line: Some(3),
            old_line: None,
            range: None,
        };
        // Clicking below the composer keeps its first line as the far end.
        assert_eq!(stretch(&hunk, Side::New, &target, 4), Some(2));
        // Clicking above takes its last line.
        assert_eq!(stretch(&hunk, Side::New, &target, 0), Some(2));
        // The other side cannot be stretched to.
        assert_eq!(stretch(&hunk, Side::Old, &target, 0), None);
    }

    // ----- interaction tests -----------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{Modifiers, Point, TestAppContext, VisualTestContext};
    use reviewdeck_core::demo::DEMO_ITEMS;
    use reviewdeck_core::http::{Http, MockResponse};
    use reviewdeck_core::store::{MemoryTokens, TokenStore, Vault};

    use crate::state::{AppDeps, AppState};
    use crate::ui::theme::Theme;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn rig(cx: &mut TestAppContext, mode: DiffViewMode, dark: bool) -> Entity<AppState> {
        let dir = std::env::temp_dir().join(format!(
            "reviewdeck-diff-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let tokens: Arc<dyn TokenStore> = Arc::new(MemoryTokens::new());
        let vault = Arc::new(Vault::open_at(dir.join("reviewdeck.json"), tokens));
        vault
            .save_settings(|settings| settings.diff_view = mode)
            .expect("settings are saved");
        let state = cx.new(|cx| {
            AppState::new(
                AppDeps {
                    http: Http::mock(|_| MockResponse::new(500, Vec::new())),
                    vault,
                    remote: None,
                    demo: true,
                    clock: None,
                    notify: None,
                    tray: None,
                },
                cx,
            )
        });
        cx.update(|cx| {
            cx.set_global(GlobalState(state.clone()));
            cx.set_global(Theme::new(dark));
            crate::ui::components::bind_keys(cx);
        });
        drop(state.update(cx, |state, cx| state.refresh(cx)));
        state
    }

    fn item_id() -> String {
        DEMO_ITEMS[0].id.clone()
    }

    const PATCH: &str =
        "@@ -1,5 +1,6 @@\n fn a() {\n-    old();\n+    new();\n+    more();\n }\n \n fn b() {}";

    fn file(path: &str, patch: Option<&str>, binary: bool, status: FileStatus) -> DiffFile {
        DiffFile {
            path: path.into(),
            old_path: path.into(),
            status,
            additions: 2,
            deletions: 1,
            patch: patch.map(str::to_string),
            binary,
        }
    }

    fn lib() -> Vec<DiffFile> {
        vec![file("src/lib.rs", Some(PATCH), false, FileStatus::Modified)]
    }

    fn open_view_for(
        cx: &mut TestAppContext,
        item: String,
        files: Vec<DiffFile>,
        threads: Vec<CommentThread>,
    ) -> (Entity<DiffView>, &mut VisualTestContext) {
        let (view, vcx) = cx.add_window_view(|window, cx| {
            DiffView::new(
                item,
                Arc::new(files),
                Arc::new(threads),
                DiffRefs::default(),
                window,
                cx,
            )
        });
        vcx.run_until_parked();
        (view, vcx)
    }

    fn open_view(
        cx: &mut TestAppContext,
        files: Vec<DiffFile>,
        threads: Vec<CommentThread>,
    ) -> (Entity<DiffView>, &mut VisualTestContext) {
        open_view_for(cx, item_id(), files, threads)
    }

    fn bounds(vcx: &mut VisualTestContext, selector: String) -> gpui::Bounds<gpui::Pixels> {
        let name: &'static str = Box::leak(selector.into_boxed_str());
        vcx.debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} was not drawn"))
    }

    fn centre(vcx: &mut VisualTestContext, selector: String) -> Point<gpui::Pixels> {
        bounds(vcx, selector).center()
    }

    /// Hover a line's gutter (which shows its button) and press on the button.
    fn press(vcx: &mut VisualTestContext, side: &str, line: u32, modifiers: Modifiers) {
        let gutter = centre(vcx, format!("diff-gutter-0-{side}-{line}"));
        vcx.simulate_mouse_move(gutter, None, Modifiers::none());
        let button = centre(vcx, format!("diff-comment-0-{side}-{line}"));
        vcx.simulate_mouse_down(button, MouseButton::Left, modifiers);
    }

    fn release(vcx: &mut VisualTestContext, side: &str, line: u32) {
        let at = centre(vcx, format!("diff-gutter-0-{side}-{line}"));
        vcx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    }

    fn composer_target(
        view: &Entity<DiffView>,
        vcx: &mut VisualTestContext,
    ) -> Option<CommentTarget> {
        view.read_with(vcx, |view, _| {
            view.files[0].composer.as_ref().map(|c| c.target.clone())
        })
    }

    #[gpui::test]
    fn a_click_on_a_split_gutter_opens_a_single_line_composer(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Split, false);
        let (view, vcx) = open_view(cx, lib(), vec![]);
        press(vcx, "new", 3, Modifiers::none());
        release(vcx, "new", 3);
        let target = composer_target(&view, vcx).expect("a composer opens");
        assert_eq!(target.new_line, Some(3));
        assert_eq!(target.old_line, None);
        assert!(target.range.is_none());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("diff-composer-0").is_some(),
            "the composer row is drawn"
        );
    }

    #[gpui::test]
    fn a_click_on_a_unified_gutter_opens_a_composer_on_either_side(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Unified, false);
        let (view, vcx) = open_view(cx, lib(), vec![]);
        press(vcx, "old", 2, Modifiers::none());
        release(vcx, "old", 2);
        let target = composer_target(&view, vcx).expect("a composer opens on the removed line");
        assert_eq!((target.old_line, target.new_line), (Some(2), None));
        press(vcx, "new", 3, Modifiers::none());
        release(vcx, "new", 3);
        let target = composer_target(&view, vcx).expect("and on the added line");
        assert_eq!((target.old_line, target.new_line), (None, Some(3)));
    }

    #[gpui::test]
    fn dragging_along_the_gutter_makes_a_range_and_shift_stretches_it(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Split, false);
        let (view, vcx) = open_view(cx, lib(), vec![]);
        press(vcx, "new", 2, Modifiers::none());
        let over = centre(vcx, "diff-gutter-0-new-3".into());
        vcx.simulate_mouse_move(over, MouseButton::Left, Modifiers::none());
        release(vcx, "new", 3);
        let target = composer_target(&view, vcx).expect("a range composer opens");
        assert_eq!(target.new_line, Some(3));
        assert_eq!(target.range.map(|r| r.start_line), Some(2));

        // Shift-click a line below stretches the range to take it in.
        press(vcx, "new", 5, Modifiers::shift());
        release(vcx, "new", 5);
        let target = composer_target(&view, vcx).expect("still open");
        assert_eq!(target.new_line, Some(5));
        assert_eq!(target.range.map(|r| r.start_line), Some(2));
    }

    #[gpui::test]
    fn escape_while_dragging_drops_the_range(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Split, false);
        let (view, vcx) = open_view(cx, lib(), vec![]);
        press(vcx, "new", 2, Modifiers::none());
        assert!(view.read_with(vcx, |v, _| v.selecting.is_some()));
        vcx.simulate_keystrokes("escape");
        assert!(view.read_with(vcx, |v, _| v.selecting.is_none()));
        release(vcx, "new", 2);
        assert!(
            composer_target(&view, vcx).is_none(),
            "nothing was commented on"
        );
    }

    #[gpui::test]
    fn cmd_enter_saves_the_draft_and_escape_cancels(cx: &mut TestAppContext) {
        let state = rig(cx, DiffViewMode::Split, false);
        let (view, vcx) = open_view(cx, lib(), vec![]);
        press(vcx, "new", 3, Modifiers::none());
        release(vcx, "new", 3);
        vcx.simulate_input("looks fine");
        // Pressing the same line's button again keeps the text and the focus.
        press(vcx, "new", 3, Modifiers::none());
        release(vcx, "new", 3);
        vcx.simulate_input("!");
        assert_eq!(
            view.read_with(vcx, |v, cx| v.files[0].composer.as_ref().map(|c| c
                .input
                .read(cx)
                .text()
                .to_string())),
            Some("looks fine!".to_string())
        );
        vcx.simulate_keystrokes("escape");
        assert!(
            composer_target(&view, vcx).is_none(),
            "escape closes the composer"
        );
        assert!(state.read_with(vcx, |s, _| s.drafts(&item_id()).is_empty()));

        press(vcx, "new", 3, Modifiers::none());
        release(vcx, "new", 3);
        vcx.simulate_keystrokes("cmd-enter");
        assert!(
            composer_target(&view, vcx).is_some(),
            "an empty comment is not sent"
        );
        vcx.simulate_input("  looks fine  ");
        vcx.simulate_keystrokes("cmd-enter");
        assert!(
            composer_target(&view, vcx).is_none(),
            "sending closes the composer"
        );
        let drafts = state.read_with(vcx, |s, _| s.drafts(&item_id()));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].body, "looks fine");
        assert_eq!(
            (drafts[0].path.as_str(), drafts[0].new_line),
            ("src/lib.rs", Some(3))
        );
        vcx.run_until_parked();
        assert_eq!(view.read_with(vcx, |v, _| v.files[0].drafts.len()), 1);
        assert!(view.read_with(vcx, |v, _| {
            v.entries.iter().any(|e| matches!(e.item, Item::Draft(..)))
        }));
    }

    #[gpui::test]
    fn a_failed_save_keeps_the_composer_and_its_text(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Split, false);
        // A pull request the deck does not hold: the save is refused.
        let (view, vcx) = open_view_for(cx, "not-in-the-deck".into(), lib(), vec![]);
        press(vcx, "new", 3, Modifiers::none());
        release(vcx, "new", 3);
        vcx.simulate_input("keep me");
        vcx.simulate_keystrokes("cmd-enter");
        let (open, error, text) = view.read_with(vcx, |v, cx| {
            let c = v.files[0].composer.as_ref();
            (
                c.is_some(),
                c.and_then(|c| c.error.clone()),
                c.map(|c| c.input.read(cx).text().to_string()),
            )
        });
        assert!(open);
        assert_eq!(
            error.as_ref().map(|e| e.as_ref()),
            Some("That pull request is no longer in the deck.")
        );
        assert_eq!(text.as_deref(), Some("keep me"));
    }

    #[gpui::test]
    fn the_header_collapses_and_a_big_file_starts_collapsed(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Split, false);
        let big = format!(
            "@@ -0,0 +1,700 @@\n{}",
            (0..700)
                .map(|n| format!("+line {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let (view, vcx) = open_view(
            cx,
            vec![
                file("src/lib.rs", Some(PATCH), false, FileStatus::Modified),
                file("big.txt", Some(&big), false, FileStatus::Added),
                file("logo.png", None, true, FileStatus::Added),
                file("empty.txt", None, false, FileStatus::Modified),
            ],
            vec![],
        );
        let (open, notice) = view.read_with(vcx, |v, _| {
            (
                v.files.iter().map(|f| f.open).collect::<Vec<_>>(),
                v.entries
                    .iter()
                    .any(|e| e.item == Item::Notice(1, Notice::Collapsed)),
            )
        });
        assert_eq!(open, vec![true, false, false, false]);
        assert!(notice, "a collapsed big file says so");
        let rows_before = view.read_with(vcx, |v, _| v.entries.len());
        let header = centre(vcx, "diff-file-0".into());
        vcx.simulate_click(header, Modifiers::none());
        assert!(!view.read_with(vcx, |v, _| v.files[0].open));
        assert!(view.read_with(vcx, |v, _| v.entries.len()) < rows_before);
        for f in [2, 3] {
            let at = centre(vcx, format!("diff-file-{f}"));
            vcx.simulate_click(at, Modifiers::none());
        }
        let notices = view.read_with(vcx, |v, _| {
            v.entries
                .iter()
                .filter_map(|e| match e.item {
                    Item::Notice(f, n) if f >= 2 => Some((f, n)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(notices, vec![(2, Notice::Binary), (3, Notice::NoDiff)]);
    }

    fn thread(id: &str, line: u32, start: Option<u32>, side: Side) -> CommentThread {
        CommentThread {
            id: id.into(),
            comments: vec![],
            resolved: false,
            outdated: false,
            path: Some("src/lib.rs".into()),
            line: Some(line),
            start_line: start,
            side: Some(side),
            can_reply: false,
            can_resolve: false,
        }
    }

    #[gpui::test]
    fn threads_sit_under_their_line_in_both_layouts(cx: &mut TestAppContext) {
        for mode in [DiffViewMode::Split, DiffViewMode::Unified] {
            rig(cx, mode, false);
            let mut elsewhere = thread("gone", 3, None, Side::New);
            elsewhere.path = Some("other.rs".into());
            let mut outdated = thread("outdated", 3, None, Side::New);
            outdated.line = None;
            let (view, vcx) = open_view(
                cx,
                lib(),
                vec![thread("t", 3, Some(2), Side::New), elsewhere, outdated],
            );
            let order = view.read_with(vcx, |v, _| {
                v.entries.iter().map(|e| e.item).collect::<Vec<_>>()
            });
            let count = order
                .iter()
                .filter(|i| matches!(i, Item::Thread(..)))
                .count();
            assert_eq!(count, 1, "only the thread on a line of this file shows");
            let at = order
                .iter()
                .position(|i| matches!(i, Item::Thread(..)))
                .unwrap();
            let before = order[at - 1];
            match mode {
                DiffViewMode::Unified => {
                    let Item::Line(_, h, l) = before else {
                        panic!("a code row precedes the thread")
                    };
                    let line = view.read_with(vcx, |v, _| v.files[0].hunks[h].lines[l].clone());
                    assert_eq!(line.new_line, Some(3));
                }
                DiffViewMode::Split => assert!(matches!(before, Item::Row(..))),
            }
            let tinted = view.read_with(vcx, |v, _| {
                v.files[0].hunks[0]
                    .lines
                    .iter()
                    .filter(|l| v.coverage(0, Side::New, l) == Some(Coverage::Thread))
                    .count()
            });
            assert_eq!(tinted, 2, "lines 2 and 3 are covered");
        }
    }

    #[gpui::test]
    fn switching_the_theme_keeps_the_old_colours_until_the_new_ones_arrive(
        cx: &mut TestAppContext,
    ) {
        rig(cx, DiffViewMode::Split, false);
        let (view, vcx) = open_view(cx, lib(), vec![]);
        vcx.run_until_parked();
        assert!(
            view.read_with(vcx, |v, _| matches!(
                v.files[0].colour,
                Colour::Ready { dark: false, .. }
            )),
            "the light colours arrive"
        );
        vcx.update(|_, cx| cx.set_global(Theme::new(true)));
        view.update(vcx, |v, cx| {
            v.request_colour(0, true, cx);
            assert!(
                matches!(
                    v.files[0].colour,
                    Colour::Pending {
                        previous: Some(_),
                        ..
                    }
                ),
                "the old colours stay while the new ones are worked out"
            );
        });
        vcx.run_until_parked();
        assert!(view.read_with(vcx, |v, _| matches!(
            v.files[0].colour,
            Colour::Ready { dark: true, .. }
        )));
    }

    #[gpui::test]
    fn a_five_thousand_line_diff_builds_only_what_is_near_the_viewport(cx: &mut TestAppContext) {
        rig(cx, DiffViewMode::Unified, false);
        let n = 5000;
        let patch = format!(
            "@@ -1,{n} +1,{n} @@\n{}",
            (0..n)
                .map(|i| if i % 3 == 0 {
                    format!("-old {i}\n+new {i}")
                } else {
                    format!(" ctx {i}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
        let started = std::time::Instant::now();
        let (view, vcx) = open_view(
            cx,
            vec![file("big.rs", Some(&patch), false, FileStatus::Modified)],
            vec![],
        );
        // It starts collapsed past 600 lines; open it.
        let at = centre(vcx, "diff-file-0".into());
        vcx.simulate_click(at, Modifiers::none());
        let rows = view.read_with(vcx, |v, _| v.entries.len());
        assert!(rows > 5000);
        let drawn = (1..=n + 1)
            .filter(|number| {
                let name: &'static str =
                    Box::leak(format!("diff-gutter-0-new-{number}").into_boxed_str());
                vcx.debug_bounds(name).is_some()
            })
            .count();
        assert!(
            drawn < 150,
            "only the rows near the viewport are built, got {drawn}"
        );
        assert!(
            started.elapsed().as_secs() < 20,
            "opening and drawing stays bounded"
        );
    }
}
