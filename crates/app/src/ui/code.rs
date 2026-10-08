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

use gpui::{AppContext, HighlightStyle, SharedString, StyledText, Task};
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
}
