//! Syntax-highlighted code text, shared by the diff and the markdown views.
//! Port of src/renderer/src/components/Markdown.tsx.
//!
//! Highlighting is the slow part, so it happens off the main thread and its result
//! is kept in one cache keyed by what was highlighted (the language or fence tag,
//! the text and the theme). Rendering only ever reads that cache: a line is turned
//! into a [`StyledText`] from its tokens, and nothing is re-highlighted per frame.
//!
//! What is applied is what the TypeScript applied: each token's colour, and its
//! background where the grammar paints one. Font style from the highlighter is not
//! used, because the page never showed it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, AppContext, AvailableSpace, Bounds, Element, ElementId, GlobalElementId, HighlightStyle,
    InspectorElementId, IntoElement, LayoutId, Pixels, ShapedLine, SharedString, Size, StyledText,
    Task, TextRun, Window, px,
};
use reviewdeck_core::highlight::{self, Token};

use crate::ui::theme::packed;

/// One highlighted block: a token list per line, in source order.
pub type Lines = Vec<Vec<Token>>;

/// What a block of code is, which decides the grammar it is read with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CodeKind {
    /// A language id the diff picked from the file name (`language_for`).
    #[allow(dead_code)]
    Language(String),
    /// The tag of a markdown fence, read the way `highlight_code` reads it.
    Fence(String),
}

/// What the cache knows a block by. The text is kept as a hash and a length: a
/// 64-bit hash collision would colour a block wrongly, never crash, and holding the
/// whole text as a key would double the memory of every block shown.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    kind: CodeKind,
    len: usize,
    hash: u64,
    dark: bool,
}

/// Enough entries for every block of a few long pull requests. Past it the cache
/// starts again, so memory stays bounded and the cost is one more highlight.
const CACHE_LIMIT: usize = 512;

type Cache = HashMap<Key, Option<Arc<Lines>>>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(kind: &CodeKind, text: &str, dark: bool) -> Key {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    Key {
        kind: kind.clone(),
        len: text.len(),
        hash: hasher.finish(),
        dark,
    }
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    // A poisoned cache holds only finished entries, so it is still safe to use.
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What is already known about a block, without highlighting anything.
///
/// `None` means it was never highlighted. `Some(None)` means it was, and the
/// grammar could not read it (an unknown fence tag), so it renders plain.
pub fn cached(kind: &CodeKind, text: &str, dark: bool) -> Option<Option<Arc<Lines>>> {
    lock().get(&key(kind, text, dark)).cloned()
}

/// Highlights a block now, or returns what an earlier call already worked out.
///
/// Blocking: call it from a background task, never from render.
pub fn highlight(kind: &CodeKind, text: &str, dark: bool) -> Option<Arc<Lines>> {
    let wanted = key(kind, text, dark);
    if let Some(hit) = lock().get(&wanted).cloned() {
        return hit;
    }

    let lines = match kind {
        CodeKind::Language(language) => Some(highlight::highlight_lines(language, text, dark)),
        CodeKind::Fence(tag) => highlight::highlight_code(text, tag, dark),
    }
    .map(Arc::new);

    let mut cache = lock();
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(wanted, lines.clone());
    lines
}

/// [`highlight`] as a task: ready at once on a cache hit, otherwise run on the
/// background executor and handed back to whoever awaits it.
#[allow(dead_code)] // the diff view keeps its own per-file cache
pub fn highlight_task<C: AppContext>(
    cx: &C,
    kind: CodeKind,
    text: String,
    dark: bool,
) -> Task<Option<Arc<Lines>>> {
    if let Some(hit) = cached(&kind, &text, dark) {
        return Task::ready(hit);
    }
    cx.background_spawn(async move { highlight(&kind, &text, dark) })
}

/// Tab stops every this many columns, which is what the browser does with
/// `white-space: pre` and the default `tab-size` of 8. gpui would draw a tab as a
/// wide stop of its own, so tabs are expanded here.
const TAB_SIZE: usize = 8;

/// The line with tabs expanded to spaces, and for each byte offset of the original
/// line the matching byte offset of the expanded one (`None` when there are no tabs,
/// so the offsets do not move).
fn expand_tabs(line: &str) -> (Cow<'_, str>, Option<Vec<usize>>) {
    if !line.contains('\t') {
        return (Cow::Borrowed(line), None);
    }

    let mut out = String::with_capacity(line.len() + 8);
    let mut map = vec![0; line.len() + 1];
    let mut column = 0;
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
    (Cow::Owned(out), Some(map))
}

/// One line of code as styled text.
///
/// `tokens` are the line's tokens from [`highlight`] (`None` renders plain, which is
/// how a block shows before its colour arrives, and for lines with no grammar). The
/// caller sets the font (`mono_font()`) and size on the element holding the line, which
/// the text inherits; colours and backgrounds come from the tokens.
pub fn code_line(line: &str, tokens: Option<&[Token]>) -> StyledText {
    let (text, map) = expand_tabs(line);
    let styled = StyledText::new(SharedString::from(text.to_string()));

    let Some(tokens) = tokens else {
        return styled;
    };
    let highlights = token_highlights(tokens, &text, line.len(), map.as_deref());
    if highlights.is_empty() {
        styled
    } else {
        styled.with_highlights(highlights)
    }
}

/// The tokens as sorted, non-overlapping highlights on the expanded text. A token
/// that does not fit (out of order, past the end, or off a character boundary) is
/// dropped rather than trusted, so a bad token costs colour, never a panic.
fn token_highlights(
    tokens: &[Token],
    text: &str,
    original_len: usize,
    map: Option<&[usize]>,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let at = |offset: usize| -> usize {
        let offset = offset.min(original_len);
        match map {
            Some(map) => map.get(offset).copied().unwrap_or(text.len()),
            None => offset.min(text.len()),
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

// ---------------------------------------------------------------------------------------
// Browser-style wrapping of a monospace line
// ---------------------------------------------------------------------------------------

/// Where a line of code may break, the way the browser breaks `white-space: pre-wrap`
/// text: after a run of spaces, and after a `-` that is followed by something
/// other than a digit or another `-`. A `/` is no opportunity, as in Chromium. gpui's wrapper only offers the spot before
/// a punctuation mark and never inside `checkout-api`, which strands a lone closing
/// quote at the start of a line.
///
/// Returns the byte offset of the start of each piece after the first.
fn break_opportunities(text: &str) -> Vec<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    for (i, &(_, c)) in chars.iter().enumerate() {
        let Some(&(next_at, next)) = chars.get(i + 1) else {
            break;
        };
        let after = match c {
            ' ' => next != ' ',
            '-' => {
                i > 0
                    && !matches!(chars[i - 1].1, ' ' | '-')
                    && !next.is_ascii_digit()
                    && !matches!(next, ' ' | '-')
            }
            _ => false,
        };
        if after {
            out.push(next_at);
        }
    }
    out
}

/// Columns a character takes in a monospace face.
fn columns_of(c: char) -> usize {
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// The byte offsets at which each visual line after the first starts, for `text` in
/// lines of at most `cols` columns. A piece that cannot fit a line by itself is cut at
/// the edge, which is `overflow-wrap: break-word`.
pub fn wrap_points(text: &str, cols: usize) -> Vec<usize> {
    let cols = cols.max(1);
    let mut pieces = break_opportunities(text);
    pieces.push(text.len());
    let mut starts = Vec::new();
    let mut used = 0; // columns on the current line
    let mut piece_start = 0;
    for end in pieces {
        let piece = &text[piece_start..end];
        // Trailing spaces hang past the edge instead of forcing a break.
        let body = piece.trim_end_matches(' ');
        let width: usize = body.chars().map(columns_of).sum();
        let full: usize = piece.chars().map(columns_of).sum();
        if used > 0 && used + width > cols {
            starts.push(piece_start);
            used = 0;
        }
        if width > cols {
            for (offset, c) in piece.char_indices() {
                let w = columns_of(c);
                if used + w > cols && used > 0 && c != ' ' {
                    starts.push(piece_start + offset);
                    used = 0;
                }
                used += w;
            }
        } else {
            used += full;
        }
        piece_start = end;
    }
    starts
}

/// The slice of `runs` that covers `range` of the text they describe.
fn slice_runs(runs: &[TextRun], range: std::ops::Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut at = 0;
    for run in runs {
        let (start, end) = (at, at + run.len);
        at = end;
        let (from, to) = (start.max(range.start), end.min(range.end));
        if from < to {
            let mut piece = run.clone();
            piece.len = to - from;
            out.push(piece);
        }
    }
    out
}

struct WrapLayout {
    lines: Vec<ShapedLine>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    size: Size<Pixels>,
    bounds: Option<Bounds<Pixels>>,
}

/// One line of code wrapped like the browser wraps it. A drop-in for [`StyledText`] in
/// a monospace cell: it takes the font, size and line height from the element it sits
/// in and wraps to the width the layout gives it.
pub struct WrappedCode {
    text: SharedString,
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    layout: Rc<RefCell<Option<WrapLayout>>>,
}

impl WrappedCode {
    pub fn new(
        text: impl Into<SharedString>,
        highlights: impl IntoIterator<Item = (std::ops::Range<usize>, HighlightStyle)>,
    ) -> Self {
        Self {
            text: text.into(),
            highlights: highlights.into_iter().collect(),
            layout: Rc::default(),
        }
    }
}

impl IntoElement for WrappedCode {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for WrappedCode {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style
            .line_height
            .to_pixels(font_size.into(), window.rem_size());

        let mut runs = Vec::new();
        let mut at = 0;
        for (range, highlight) in &self.highlights {
            if at < range.start {
                runs.push(style.to_run(range.start - at));
            }
            runs.push(style.clone().highlight(*highlight).to_run(range.len()));
            at = range.end;
        }
        if at < self.text.len() {
            runs.push(style.to_run(self.text.len() - at));
        }

        let text = self.text.clone();
        let state = self.layout.clone();
        let id = window.request_measured_layout(
            Default::default(),
            move |known, available, window, _cx| {
                let wrap_width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                if let Some(layout) = state.borrow().as_ref()
                    && layout.wrap_width == wrap_width
                {
                    return layout.size;
                }

                let system = window.text_system().clone();
                let font_id = system.resolve_font(&style.font());
                let advance = system
                    .advance(font_id, font_size, 'm')
                    .map(|size| size.width)
                    .unwrap_or(font_size * 0.6);
                let starts = match wrap_width {
                    Some(width) if advance > px(0.) => {
                        let cols = ((width / advance) + 0.001).floor().max(1.) as usize;
                        wrap_points(&text, cols)
                    }
                    _ => Vec::new(),
                };

                let mut lines = Vec::with_capacity(starts.len() + 1);
                let mut from = 0;
                for end in starts.into_iter().chain(std::iter::once(text.len())) {
                    let slice = SharedString::from(text[from..end].to_string());
                    lines.push(system.shape_line(
                        slice,
                        font_size,
                        &slice_runs(&runs, from..end),
                        None,
                    ));
                    from = end;
                }
                let width = lines
                    .iter()
                    .map(|line| line.width)
                    .fold(px(0.), Pixels::max)
                    .ceil();
                let size = Size {
                    width,
                    height: line_height * lines.len() as f32,
                };
                *state.borrow_mut() = Some(WrapLayout {
                    lines,
                    line_height,
                    wrap_width,
                    size,
                    bounds: None,
                });
                size
            },
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
        if let Some(layout) = self.layout.borrow_mut().as_mut() {
            layout.bounds = Some(bounds);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = self.layout.borrow();
        let Some(layout) = state.as_ref() else { return };
        let Some(bounds) = layout.bounds else { return };
        let mut origin = bounds.origin;
        for line in &layout.lines {
            line.paint_background(origin, layout.line_height, window, cx)
                .ok();
            line.paint(origin, layout.line_height, window, cx).ok();
            origin.y += layout.line_height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_run_to_the_next_eight_column_stop() {
        let (text, map) = expand_tabs("a\tb\t\tc");
        assert_eq!(text, "a       b               c");
        let map = map.expect("a line with tabs has a map");
        // The `b` sat at byte 2 and lands at column 8.
        assert_eq!(map[2], 8);
        assert_eq!(text.as_bytes()[map[2]], b'b');
    }

    #[test]
    fn a_line_without_tabs_is_borrowed_unchanged() {
        let (text, map) = expand_tabs("let x = 1;");
        assert!(matches!(text, Cow::Borrowed("let x = 1;")));
        assert!(map.is_none());
    }

    #[test]
    fn a_token_on_a_bad_boundary_is_dropped_not_trusted() {
        // "é" is two bytes; a range that starts inside it must not be used.
        let token = Token {
            range: 1..2,
            color: 0xff00_00ff,
            font_style: Default::default(),
            background: None,
        };
        let highlights = token_highlights(&[token], "é", 2, None);
        assert!(highlights.is_empty());
    }

    #[test]
    fn overlapping_tokens_keep_the_first_one() {
        let make = |range: std::ops::Range<usize>| Token {
            range,
            color: 0x0000_00ff,
            font_style: Default::default(),
            background: None,
        };
        let tokens = [make(0..4), make(2..6)];
        let highlights = token_highlights(&tokens, "abcdef", 6, None);
        assert_eq!(highlights.len(), 2);
        assert_eq!(highlights[1].0, 4..6);
    }

    #[test]
    fn a_line_breaks_after_a_hyphen_like_the_browser_and_never_before_a_quote() {
        let text = "  \"github.com/acme/checkout-api/internal/gateway\"";
        // 45 columns: the whole thing is 49, so it wraps after `checkout-`.
        let starts = wrap_points(text, 45);
        assert_eq!(starts.len(), 1);
        assert_eq!(&text[starts[0]..], "api/internal/gateway\"");
    }

    #[test]
    fn a_word_longer_than_the_line_is_cut_at_the_edge() {
        let starts = wrap_points("abcdefghij", 4);
        assert_eq!(starts, vec![4, 8]);
    }

    #[test]
    fn spaces_break_and_trailing_spaces_hang() {
        let text = "aaa bbb ccc";
        assert_eq!(wrap_points(text, 7), vec![8]);
        assert_eq!(wrap_points(text, 3), vec![4, 8]);
    }

    #[test]
    fn a_hyphen_before_a_digit_or_after_a_space_is_no_opportunity() {
        assert_eq!(wrap_points("x -1 y-2 z", 4), vec![5, 9]);
    }
}
