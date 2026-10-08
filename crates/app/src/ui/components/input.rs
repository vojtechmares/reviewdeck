//! TextInput: a reusable single-line / multi-line (soft-wrapping, auto-growing) text field for
//! gpui 0.2.2, which ships no input widget.
//!
//! Features: placeholder, IME / marked text (EntityInputHandler), mouse selection (click, drag -
//! also outside the field, shift-click, double-click word, triple-click line), keyboard movement
//! (arrows, alt = word, cmd = line / document, home/end, ctrl-a/ctrl-e), shift-selection, select
//! all, clipboard (cmd-c/x/v), backspace/delete with alt/cmd variants, Enter (newline in
//! multi-line, Submit in single-line), cmd-Enter (Submit), Escape (Cancel), undo/redo with a
//! bounded history, disabled state, focus ring, wheel scrolling and auto-scroll to the caret.
//!
//! Offsets: the model works in UTF-8 BYTE offsets into `content`, always on char boundaries.
//! The platform input handler (IME) speaks UTF-16 code units; conversions happen only in the
//! `EntityInputHandler` impl.
//!
//! Setup: call [`bind_keys`] once at startup. Put the Edit-menu items on THIS module's actions
//! (`text_input::Copy` etc.) so the menu is enabled whenever an input is focused.
//!
//! Port of src/renderer/src/components/ui/input.tsx (`Input`, `Textarea`, `Label`). The
//! single-line field is `multi_line` off; the textarea is `multi_line(min, max)`. The look
//! comes from [`field_style`], which is re-read every frame, so the theme is always current.
//!
//! The field draws its own type (`text-[13px] text-foreground`, a 36px `h-9` box for the
//! single-line field, `leading-relaxed` rows for the textarea), so it looks the same wherever
//! it is placed. Views adjust it through the builders (or their `set_*` twins) rather than by
//! styling the element: [`TextInput::height`] (`h-8`), [`TextInput::padding_left`] (`pl-7.5`,
//! the inset that makes room for an icon), [`TextInput::set_rows`] and
//! [`TextInput::masked`].
//!
//! Masked mode is for secrets (access tokens): the text is drawn as bullets, copy and cut are
//! disabled so the secret cannot reach the clipboard, word-wise movement and double-click
//! treat the whole value as one word (a word boundary would leak its shape), the IME is
//! switched off (composition is committed immediately, never shown) and the text is not
//! offered to the platform's `text_for_range` queries. Masked applies to the single-line
//! field only.

use std::ops::Range;

use crate::ui::theme::ActiveTheme;

use super::button::with_alpha;

use gpui::{
    App, AvailableSpace, Bounds, BoxShadow, ClipboardItem, ContentMask, Context, CursorStyle,
    DispatchPhase, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId,
    InteractiveElement, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels, Point, Rems, Render,
    ScrollWheelEvent, SharedString, Style, Styled, Task, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, hsla, point, prelude::FluentBuilder,
    px, relative, rems, size,
};

use crate::ui::theme::rpx;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Enter,
        SecondaryEnter,
        Escape,
        ShowCharacterPalette,
    ]
);

/// Key context set on every input. Other bindings can exclude inputs with `!TextInput`.
pub const KEY_CONTEXT: &str = "TextInput";

/// Maximum number of undo steps kept per input.
pub const MAX_UNDO: usize = 200;

/// Register the input key bindings. Call once at startup (before `cx.set_menus`, so the Edit
/// menu shows the shortcuts).
pub fn bind_keys(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("ctrl-h", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-d", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-w", DeleteWordLeft, c),
        KeyBinding::new("alt-delete", DeleteWordRight, c),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, c),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, c),
        KeyBinding::new("ctrl-k", DeleteToLineEnd, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("ctrl-b", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("ctrl-f", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("ctrl-p", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("ctrl-n", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("cmd-left", LineStart, c),
        KeyBinding::new("home", LineStart, c),
        KeyBinding::new("ctrl-a", LineStart, c),
        KeyBinding::new("cmd-right", LineEnd, c),
        KeyBinding::new("end", LineEnd, c),
        KeyBinding::new("ctrl-e", LineEnd, c),
        KeyBinding::new("cmd-shift-left", SelectLineStart, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("cmd-up", DocStart, c),
        KeyBinding::new("cmd-down", DocEnd, c),
        KeyBinding::new("cmd-shift-up", SelectDocStart, c),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("cmd-shift-z", Redo, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", Enter, c),
        KeyBinding::new("cmd-enter", SecondaryEnter, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]);
}

/// Events emitted by a [`TextInput`]; subscribe with `cx.subscribe(&input, ..)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The text changed (typing, paste, cut, undo, IME, ...). Not emitted by `set_text`.
    Changed,
    /// Enter in single-line mode, cmd-Enter in both modes.
    Submit,
    /// Escape. The input does not propagate Escape further; react to this event instead.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    SingleLine,
    /// Soft-wrapping text area: grows from `min_rows` to `max_rows`, then scrolls.
    MultiLine {
        min_rows: usize,
        max_rows: usize,
    },
}

/// Visual style. Lengths in rems so the field follows the window rem size (zoom).
/// Font family / size / line height / text colour are INHERITED from the parent element.
#[derive(Debug, Clone)]
pub struct TextInputStyle {
    pub background: Hsla,
    pub border: Hsla,
    pub border_focused: Hsla,
    /// Focus ring drawn as a 3px spread shadow (set alpha 0 to disable).
    pub ring: Hsla,
    pub placeholder: Hsla,
    pub selection: Hsla,
    pub selection_unfocused: Hsla,
    pub cursor: Hsla,
    pub padding_x: Rems,
    pub padding_y: Rems,
    pub radius: Rems,
    pub disabled_opacity: f32,
}

impl Default for TextInputStyle {
    fn default() -> Self {
        Self {
            background: hsla(0., 0., 1., 1.),
            border: hsla(0., 0., 0., 0.12),
            border_focused: hsla(217. / 360., 0.91, 0.6, 1.),
            ring: hsla(217. / 360., 0.91, 0.6, 0.25),
            placeholder: hsla(0., 0., 0., 0.4),
            selection: hsla(217. / 360., 0.91, 0.6, 0.3),
            selection_unfocused: hsla(0., 0., 0.5, 0.2),
            cursor: hsla(217. / 360., 0.91, 0.5, 1.),
            padding_x: rems(0.5),
            padding_y: rems(0.375),
            radius: rems(0.375),
            disabled_opacity: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

#[derive(Debug, Clone)]
struct Snapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

/// One visual (soft-wrapped) row. Offsets are absolute byte offsets into the displayed text.
#[derive(Debug, Clone, Copy)]
struct Row {
    line: usize,
    start: usize,
    /// Exclusive; for the last row of a logical line this is the offset of its '\n' (or the
    /// end of the text).
    end: usize,
    /// x of the row's first glyph within the unwrapped line layout.
    start_x: Pixels,
    last_in_line: bool,
}

/// Geometry of the last painted frame, used for hit testing, IME and vertical movement.
#[derive(Clone)]
struct InputLayout {
    lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    line_first_row: Vec<usize>,
    rows: Vec<Row>,
    line_height: Pixels,
    /// Text area bounds (inside the padding) in window coordinates.
    bounds: Bounds<Pixels>,
    max_scroll: Point<Pixels>,
    is_placeholder: bool,
}

impl InputLayout {
    fn x_for(&self, row: &Row, offset: usize) -> Pixels {
        let line = &self.lines[row.line];
        line.unwrapped_layout
            .x_for_index(offset.saturating_sub(self.line_starts[row.line]))
            - row.start_x
    }

    fn row_for_offset(&self, offset: usize) -> usize {
        self.rows
            .iter()
            .position(|r| {
                offset >= r.start && (offset < r.end || (offset == r.end && r.last_in_line))
            })
            .unwrap_or(self.rows.len().saturating_sub(1))
    }

    fn row_width(&self, row: &Row) -> Pixels {
        self.x_for(row, row.end)
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    mode: InputMode,
    disabled: bool,
    pub style: TextInputStyle,
    /// `h-8` and friends: an explicit height in CSS pixels for the single-line field.
    height: Option<f32>,
    /// `pl-7.5` and friends: horizontal padding overrides in CSS pixels.
    padding_left: Option<f32>,
    padding_right: Option<f32>,
    masked: bool,
    /// The caret is drawn while this is true; a task flips it while the field is focused.
    blink_on: bool,
    blink_task: Option<Task<()>>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// Remembered x for consecutive up/down moves.
    goal_x: Option<Pixels>,
    is_selecting: bool,
    /// Scrolled distance of the content (>= 0).
    scroll: Point<Pixels>,
    /// Scroll the caret into view on the next prepaint.
    autoscroll: bool,
    layout: Option<InputLayout>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    last_edit: Option<EditKind>,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------------

impl TextInput {
    /// A single-line input. Chain `.multi_line(..)`, `.placeholder(..)`, `.with_text(..)`.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            content: String::new(),
            placeholder: SharedString::default(),
            mode: InputMode::SingleLine,
            disabled: false,
            style: TextInputStyle::default(),
            height: None,
            padding_left: None,
            padding_right: None,
            masked: false,
            blink_on: true,
            blink_task: None,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            goal_x: None,
            is_selecting: false,
            scroll: Point::default(),
            autoscroll: false,
            layout: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: None,
        }
    }

    pub fn multi_line(mut self, min_rows: usize, max_rows: usize) -> Self {
        let min_rows = min_rows.max(1);
        self.mode = InputMode::MultiLine {
            min_rows,
            max_rows: max_rows.max(min_rows),
        };
        self
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// An explicit height in CSS pixels for the single-line field (`h-8` is 32; the default
    /// is `h-9`, 36). The textarea grows with its rows instead and ignores this.
    pub fn height(mut self, css_px: f32) -> Self {
        self.height = Some(css_px);
        self
    }

    /// Replaces the left padding (`px-3`, 12) in CSS pixels, e.g. 30 for `pl-7.5` when an icon
    /// sits inside the field's left edge.
    pub fn padding_left(mut self, css_px: f32) -> Self {
        self.padding_left = Some(css_px);
        self
    }

    /// Replaces the right padding (`px-3`, 12) in CSS pixels.
    pub fn padding_right(mut self, css_px: f32) -> Self {
        self.padding_right = Some(css_px);
        self
    }

    /// Secret mode, see the module docs. Ignored by the multi-line field.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn is_masked(&self) -> bool {
        self.masked && !self.is_multi()
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        self.marked_range = None;
        cx.notify();
    }

    /// Changes the textarea's row range after construction (`rows` on the TSX element). A
    /// single-line field becomes a textarea.
    pub fn set_rows(&mut self, min_rows: usize, max_rows: usize, cx: &mut Context<Self>) {
        let min_rows = min_rows.max(1);
        self.mode = InputMode::MultiLine {
            min_rows,
            max_rows: max_rows.max(min_rows),
        };
        cx.notify();
    }

    pub fn set_height(&mut self, css_px: Option<f32>, cx: &mut Context<Self>) {
        self.height = css_px;
        cx.notify();
    }

    pub fn set_padding_left(&mut self, css_px: Option<f32>, cx: &mut Context<Self>) {
        self.padding_left = css_px;
        cx.notify();
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content = self.sanitize(text.into());
        let end = self.content.len();
        self.selected_range = end..end;
        self
    }

    pub fn with_style(mut self, style: TextInputStyle) -> Self {
        self.style = style;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    /// Replace the whole text (programmatic: clears undo history, caret to the end, no
    /// `Changed` event).
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = self.sanitize(text.into());
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.goal_x = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
        self.autoscroll = true;
        cx.notify();
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        self.is_selecting = false;
        cx.notify();
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    /// Selected byte range (start <= end).
    pub fn selection(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    pub fn set_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let start = self.clamp_offset(range.start.min(range.end));
        let end = self.clamp_offset(range.start.max(range.end));
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.autoscroll = true;
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        self.last_edit = None;
        cx.notify();
    }

    /// Byte offset of the character boundary nearest to a window-coordinate point (as used for
    /// mouse hit testing). 0 before the first paint.
    pub fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        let Some(layout) = self.layout.as_ref() else {
            return 0;
        };
        if layout.is_placeholder || layout.rows.is_empty() {
            return 0;
        }
        let local = position - layout.bounds.origin + self.scroll;
        let row_ix = if local.y <= px(0.) {
            0
        } else {
            ((local.y / layout.line_height) as usize).min(layout.rows.len() - 1)
        };
        let row = layout.rows[row_ix];
        let line = &layout.lines[row.line];
        let x = (local.x + row.start_x).max(px(0.));
        let ix = layout.line_starts[row.line] + closest_index_for_x(&line.unwrapped_layout, x);
        // A soft-wrapped row's `end` is the first offset of the NEXT row; clamp to the last
        // boundary before it so clicking past the end of a wrapped row stays on that row.
        let max = if row.last_in_line {
            row.end
        } else {
            prev_boundary(&self.content, row.end).max(row.start)
        };
        // The layout speaks in displayed-text offsets.
        self.clamp_offset(self.display_to_content(ix.clamp(row.start, max)))
    }

    /// Window-coordinate top-left of the caret position for `offset` (None before first paint).
    pub fn position_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let layout = self.layout.as_ref()?;
        if layout.rows.is_empty() {
            return None;
        }
        let offset = if layout.is_placeholder {
            0
        } else {
            self.content_to_display(offset)
        };
        let row_ix = layout.row_for_offset(offset);
        let row = &layout.rows[row_ix];
        let x = layout.x_for(row, offset.clamp(row.start, row.end));
        Some(layout.bounds.origin + point(x, layout.line_height * row_ix as f32) - self.scroll)
    }

    /// Line height of the last frame (None before first paint).
    pub fn line_height(&self) -> Option<Pixels> {
        self.layout.as_ref().map(|l| l.line_height)
    }

    /// Number of visual rows in the last frame.
    pub fn visual_row_count(&self) -> usize {
        self.layout.as_ref().map_or(0, |l| l.rows.len())
    }

    /// Bounds of the text area (inside padding/border) in the last frame, window coordinates.
    pub fn text_bounds(&self) -> Option<Bounds<Pixels>> {
        self.layout.as_ref().map(|l| l.bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes() {
        let fam = "a👨\u{200D}👩\u{200D}👧b";
        assert_eq!(next_boundary(fam, 1), fam.len() - 1);
        assert_eq!(prev_boundary(fam, fam.len() - 1), 1);
        let flags = "🇨🇿🇩🇪";
        assert_eq!(next_boundary(flags, 0), 8);
        assert_eq!(prev_boundary(flags, 16), 8);
        assert_eq!(prev_boundary(flags, 8), 0);
        let comb = "e\u{0301}x"; // e + combining acute
        assert_eq!(next_boundary(comb, 0), 3);
        assert_eq!(prev_boundary(comb, 3), 0);
        let skin = "👍🏽!";
        assert_eq!(next_boundary(skin, 0), 8);
        assert_eq!(prev_boundary("ůx", 2), 0);
        assert_eq!(next_boundary("", 0), 0);
        assert_eq!(prev_boundary("", 0), 0);
    }

    #[test]
    fn words() {
        let t = "foo_bar  baz.qux\nnext";
        assert_eq!(next_word_boundary(t, 0), 7);
        assert_eq!(next_word_boundary(t, 7), 12);
        assert_eq!(next_word_boundary(t, 12), 13); // "." punctuation run
        assert_eq!(prev_word_boundary(t, 12), 9);
        assert_eq!(prev_word_boundary(t, 9), 0);
        assert_eq!(prev_word_boundary(t, 17), 13); // over the newline back into "qux"
        assert_eq!(word_range_at(t, 2), 0..7);
        assert_eq!(word_range_at(t, 7), 7..9); // whitespace run
        assert_eq!(word_range_at(t, 16), 13..16); // at line end: the word before
        assert_eq!(word_range_at("žluťoučký kůň", 3), 0..13);
        assert_eq!(line_range_at(t, 18), 17..21);
        assert_eq!(line_range_at(t, 3), 0..16);
    }
}

// ---------------------------------------------------------------------------------------------
// Internals: offsets, words, editing
// ---------------------------------------------------------------------------------------------

/// Like `LineLayout::closest_index_for_x`, but also rounds correctly over the LAST glyph:
/// gpui's version returns `len` for any x past the last glyph's start, so a click on the left
/// half of the last character would put the caret after it.
fn closest_index_for_x(layout: &gpui::LineLayout, x: Pixels) -> usize {
    let mut prev_index = 0;
    let mut prev_x = px(0.);
    for run in &layout.runs {
        for glyph in &run.glyphs {
            if glyph.position.x >= x {
                return if glyph.position.x - x < x - prev_x {
                    glyph.index
                } else {
                    prev_index
                };
            }
            prev_index = glyph.index;
            prev_x = glyph.position.x;
        }
    }
    if layout.width - x < x - prev_x {
        layout.len
    } else {
        prev_index
    }
}

/// Characters that extend the preceding grapheme cluster (approximation without the
/// unicode-segmentation crate: combining marks, joiners, variation selectors, emoji modifiers,
/// tags, keycap).
fn is_extend(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F
        | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x0900..=0x0903 | 0x093A..=0x094F
        | 0x0951..=0x0957 | 0x0962..=0x0963 | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C..=0x200D | 0x20D0..=0x20FF
        | 0x302A..=0x302F | 0x3099..=0x309A | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F
        | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while !text.is_char_boundary(o) {
        o -= 1;
    }
    o
}

fn is_regional_indicator(c: char) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&(c as u32))
}

/// Previous grapheme boundary (approximate, see `is_extend`).
fn prev_boundary(text: &str, offset: usize) -> usize {
    // The layout used for hit testing can be one frame stale (several key events may arrive
    // before the next paint), so never trust `offset` to be in range / on a char boundary.
    let offset = floor_char_boundary(text, offset);
    let mut chars = text[..offset].char_indices().rev().peekable();
    let Some((mut ix, mut c)) = chars.next() else {
        return 0;
    };
    loop {
        // walk back over extenders and the base they attach to
        if is_extend(c) {
            match chars.next() {
                Some((i, ch)) => {
                    ix = i;
                    c = ch;
                    continue;
                }
                None => return 0,
            }
        }
        // a char preceded by ZWJ belongs to the previous cluster
        if let Some(&(i, prev)) = chars.peek()
            && prev == '\u{200D}'
        {
            chars.next();
            ix = i;
            if let Some((i2, ch2)) = chars.next() {
                ix = i2;
                c = ch2;
                continue;
            }
            return ix;
        }
        // regional indicator pairs (flags)
        if is_regional_indicator(c) {
            let count = text[..ix]
                .chars()
                .rev()
                .take_while(|&ch| is_regional_indicator(ch))
                .count();
            if count % 2 == 1 {
                ix -= 4; // regional indicators are 4 bytes in UTF-8
            }
        }
        return ix;
    }
}

/// Next grapheme boundary (approximate, see `is_extend`).
fn next_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let mut iter = text[offset..].char_indices().peekable();
    let Some((_, first)) = iter.next() else {
        return text.len();
    };
    let mut end = offset + first.len_utf8();
    if is_regional_indicator(first) {
        let preceding = text[..offset]
            .chars()
            .rev()
            .take_while(|&ch| is_regional_indicator(ch))
            .count();
        if preceding % 2 == 0
            && let Some(&(i, c)) = iter.peek()
            && is_regional_indicator(c)
        {
            iter.next();
            end = offset + i + c.len_utf8();
        }
    }
    while let Some(&(i, c)) = iter.peek() {
        if is_extend(c) {
            iter.next();
            end = offset + i + c.len_utf8();
            if c == '\u{200D}' {
                // ZWJ joins the next char too
                if let Some((j, n)) = iter.next() {
                    end = offset + j + n.len_utf8();
                }
            }
        } else {
            break;
        }
    }
    end
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Space,
    Word,
    Punct,
    Newline,
}

fn class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' || is_extend(c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// alt-left: skip whitespace, then a run of one class.
fn prev_word_boundary(text: &str, offset: usize) -> usize {
    let mut ix = offset;
    let mut iter = text[..offset].char_indices().rev().peekable();
    while let Some(&(i, c)) = iter.peek() {
        if class(c) == CharClass::Space || class(c) == CharClass::Newline {
            ix = i;
            iter.next();
        } else {
            break;
        }
    }
    let Some(&(_, first)) = iter.peek() else {
        return ix;
    };
    let k = class(first);
    for (i, c) in iter {
        if class(c) != k {
            break;
        }
        ix = i;
    }
    ix
}

/// alt-right: skip whitespace, then a run of one class.
fn next_word_boundary(text: &str, offset: usize) -> usize {
    let mut ix = offset;
    let mut iter = text[offset..].char_indices().peekable();
    while let Some(&(i, c)) = iter.peek() {
        if class(c) == CharClass::Space || class(c) == CharClass::Newline {
            ix = offset + i + c.len_utf8();
            iter.next();
        } else {
            break;
        }
    }
    let Some(&(_, first)) = iter.peek() else {
        return ix;
    };
    let k = class(first);
    for (i, c) in iter {
        if class(c) != k {
            break;
        }
        ix = offset + i + c.len_utf8();
    }
    ix
}

/// Range of the word (or whitespace / punctuation run) at `offset` (double-click).
fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    // Prefer the char at the offset; at end of text / line use the char before it.
    let (at, c) = match text[offset..].chars().next() {
        Some(c) if c != '\n' => (offset, c),
        _ => match text[..offset].chars().next_back() {
            Some(c) if c != '\n' => (offset - c.len_utf8(), c),
            _ => return offset..offset,
        },
    };
    let k = class(c);
    let mut start = at;
    for (i, ch) in text[..at].char_indices().rev() {
        if class(ch) != k {
            break;
        }
        start = i;
    }
    let mut end = at;
    for (i, ch) in text[at..].char_indices() {
        if class(ch) != k {
            break;
        }
        end = at + i + ch.len_utf8();
    }
    start..end
}

fn line_range_at(text: &str, offset: usize) -> Range<usize> {
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    start..end
}

/// What a masked field draws for each character.
const MASK: char = '\u{2022}';

impl TextInput {
    /// The text as drawn: the content, or one bullet per character when masked.
    fn display_text(&self) -> String {
        if self.is_masked() {
            std::iter::repeat_n(MASK, self.content.chars().count()).collect()
        } else {
            self.content.clone()
        }
    }

    /// Content byte offset to the offset in [`display_text`](Self::display_text). The layout
    /// is built from the displayed text, so every offset that meets it goes through here.
    fn content_to_display(&self, offset: usize) -> usize {
        if self.is_masked() {
            let offset = self.clamp_offset(offset);
            self.content[..offset].chars().count() * MASK.len_utf8()
        } else {
            offset
        }
    }

    /// The inverse of [`to_display`](Self::to_display), rounding down to a character.
    fn display_to_content(&self, offset: usize) -> usize {
        if self.is_masked() {
            let chars = offset / MASK.len_utf8();
            self.content
                .char_indices()
                .nth(chars)
                .map_or(self.content.len(), |(ix, _)| ix)
        } else {
            offset
        }
    }

    fn is_multi(&self) -> bool {
        matches!(self.mode, InputMode::MultiLine { .. })
    }

    fn sanitize(&self, text: String) -> String {
        let text = if text.contains('\r') {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text
        };
        if self.is_multi() {
            text
        } else {
            text.replace('\n', " ")
        }
    }

    fn clamp_offset(&self, offset: usize) -> usize {
        let mut o = offset.min(self.content.len());
        while !self.content.is_char_boundary(o) {
            o -= 1;
        }
        o
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn anchor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.end
        } else {
            self.selected_range.start
        }
    }

    /// Collapse the selection to `offset` (a user movement: ends undo coalescing).
    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp_offset(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.goal_x = None;
        self.last_edit = None;
        self.autoscroll = true;
        self.blink_on = true;
        cx.notify();
    }

    /// Move the selection head to `offset`, keeping the anchor.
    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp_offset(offset);
        let anchor = self.anchor_offset();
        if offset < anchor {
            self.selected_range = offset..anchor;
            self.selection_reversed = true;
        } else {
            self.selected_range = anchor..offset;
            self.selection_reversed = false;
        }
        self.goal_x = None;
        self.last_edit = None;
        self.autoscroll = true;
        cx.notify();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.content = s.content;
        self.selected_range = s.selected_range;
        self.selection_reversed = s.selection_reversed;
        self.marked_range = None;
        self.goal_x = None;
        self.autoscroll = true;
    }

    fn push_undo(&mut self, kind: EditKind) {
        let coalesce = kind != EditKind::Other && self.last_edit == Some(kind);
        if !coalesce {
            self.undo_stack.push(self.snapshot());
            if self.undo_stack.len() > MAX_UNDO {
                self.undo_stack.remove(0);
            }
        }
        self.redo_stack.clear();
        self.last_edit = Some(kind);
    }

    /// The one place that edits `content`.
    fn replace_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        kind: EditKind,
        cx: &mut Context<Self>,
    ) {
        let start = self.clamp_offset(range.start.min(range.end));
        let end = self.clamp_offset(range.start.max(range.end));
        let text = self.sanitize(text.to_string());
        if start == end && text.is_empty() {
            return;
        }
        self.push_undo(kind);
        self.content.replace_range(start..end, &text);
        let caret = start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        self.goal_x = None;
        self.autoscroll = true;
        self.blink_on = true;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn delete_selection_or(&mut self, target: usize, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let c = self.cursor_offset();
            let target = self.clamp_offset(target);
            self.replace_range(c.min(target)..c.max(target), "", EditKind::Delete, cx);
        } else {
            self.replace_range(self.selected_range.clone(), "", EditKind::Delete, cx);
        }
    }

    /// Start / end of the visual row containing `offset` (logical line without a layout).
    fn row_bounds(&self, offset: usize) -> (usize, usize) {
        if let Some(layout) = self.layout.as_ref().filter(|l| !l.is_placeholder) {
            let row = layout.rows[layout.row_for_offset(self.content_to_display(offset))];
            let end = if row.last_in_line {
                row.end
            } else {
                prev_boundary(&self.content, row.end).max(row.start)
            };
            (
                self.display_to_content(row.start).min(self.content.len()),
                self.display_to_content(end).min(self.content.len()),
            )
        } else {
            let r = line_range_at(&self.content, offset);
            (r.start, r.end)
        }
    }

    /// Offset reached by moving one visual row up (-1) or down (+1) from the caret.
    fn vertical_target(&mut self, dir: i32) -> usize {
        let head = self.cursor_offset();
        if !self.is_multi() {
            return if dir < 0 { 0 } else { self.content.len() };
        }
        let Some(layout) = self.layout.as_ref().filter(|l| !l.is_placeholder) else {
            return if dir < 0 { 0 } else { self.content.len() };
        };
        let row_ix = layout.row_for_offset(head);
        let x = self
            .goal_x
            .unwrap_or_else(|| layout.x_for(&layout.rows[row_ix], head));
        let target = row_ix as i32 + dir;
        if target < 0 {
            return 0;
        }
        if target as usize >= layout.rows.len() {
            return self.content.len();
        }
        let row = layout.rows[target as usize];
        let p = layout.bounds.origin + point(x, layout.line_height * (target as f32 + 0.5))
            - self.scroll;
        let offset = self.offset_for_position(p);
        self.goal_x = Some(x);
        offset.clamp(row.start, row.end)
    }

    fn vertical(&mut self, dir: i32, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selected_range.is_empty() && !self.is_multi() {
            let to = if dir < 0 { 0 } else { self.content.len() };
            self.move_to(to, cx);
            return;
        }
        let goal = self.goal_x;
        let target = self.vertical_target(dir);
        let keep_goal = self.goal_x.or(goal);
        if select {
            self.select_to(target, cx)
        } else {
            self.move_to(target, cx)
        }
        self.goal_x = keep_goal; // move_to/select_to reset it
    }

    // ---- action handlers ----

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let t = prev_boundary(&self.content, self.cursor_offset());
        self.delete_selection_or(t, cx);
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let t = next_boundary(&self.content, self.cursor_offset());
        self.delete_selection_or(t, cx);
    }
    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.word_before();
        self.delete_selection_or(t, cx);
    }
    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.word_after();
        self.delete_selection_or(t, cx);
    }
    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let c = self.cursor_offset();
        let (start, _) = self.row_bounds(c);
        // at the start of a row: delete the preceding newline like NSTextView
        let t = if start == c {
            prev_boundary(&self.content, c)
        } else {
            start
        };
        self.delete_selection_or(t, cx);
    }
    fn delete_to_line_end(&mut self, _: &DeleteToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor_offset();
        let end = line_range_at(&self.content, c).end;
        let t = if end == c {
            next_boundary(&self.content, c)
        } else {
            end
        };
        self.delete_selection_or(t, cx);
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(prev_boundary(&self.content, self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(next_boundary(&self.content, self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, false, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(prev_boundary(&self.content, self.cursor_offset()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_boundary(&self.content, self.cursor_offset()), cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, true, cx);
    }
    /// Word boundary to the left of the caret; the start of the value when masked.
    fn word_before(&self) -> usize {
        if self.is_masked() {
            0
        } else {
            prev_word_boundary(&self.content, self.cursor_offset())
        }
    }
    /// Word boundary to the right of the caret; the end of the value when masked.
    fn word_after(&self) -> usize {
        if self.is_masked() {
            self.content.len()
        } else {
            next_word_boundary(&self.content, self.cursor_offset())
        }
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_before(), cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.word_after(), cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.word_before(), cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.word_after(), cx);
    }
    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        let (s, _) = self.row_bounds(self.cursor_offset());
        self.move_to(s, cx);
    }
    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let (_, e) = self.row_bounds(self.cursor_offset());
        self.move_to(e, cx);
    }
    fn select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let (s, _) = self.row_bounds(self.cursor_offset());
        self.select_to(s, cx);
    }
    fn select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let (_, e) = self.row_bounds(self.cursor_offset());
        self.select_to(e, cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }
    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }
    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }
    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.is_masked() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.is_masked() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_range(self.selected_range.clone(), "", EditKind::Other, cx);
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let range = self
                .marked_range
                .clone()
                .unwrap_or(self.selected_range.clone());
            self.replace_range(range, &text, EditKind::Other, cx);
            self.last_edit = None; // a paste is its own undo step
        }
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.restore(s);
            self.last_edit = None;
            cx.emit(TextInputEvent::Changed);
            cx.notify();
        }
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.restore(s);
            self.last_edit = None;
            cx.emit(TextInputEvent::Changed);
            cx.notify();
        }
    }
    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_multi() {
            let range = self
                .marked_range
                .clone()
                .unwrap_or(self.selected_range.clone());
            self.replace_range(range, "\n", EditKind::Other, cx);
            self.last_edit = None;
        } else {
            cx.emit(TextInputEvent::Submit);
        }
    }
    fn secondary_enter(&mut self, _: &SecondaryEnter, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submit);
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Cancel);
        // Let the action reach the ancestors: a dialog closes on it, as the TSX dialog's
        // capture-phase Escape listener does even with a field focused.
        cx.propagate();
    }
    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    // ---- mouse ----

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus_handle);
        let ix = self.offset_for_position(event.position);
        match event.click_count {
            0 | 1 => {
                if event.modifiers.shift {
                    self.select_to(ix, cx)
                } else {
                    self.move_to(ix, cx)
                }
            }
            2 => {
                let r = if self.is_masked() {
                    0..self.content.len()
                } else {
                    word_range_at(&self.content, ix)
                };
                self.move_to(r.start, cx);
                self.select_to(r.end, cx);
            }
            _ => {
                let r = line_range_at(&self.content, ix);
                self.move_to(r.start, cx);
                // include the newline like NSTextView's triple-click
                let end = if r.end < self.content.len() {
                    r.end + 1
                } else {
                    r.end
                };
                self.select_to(end, cx);
            }
        }
        self.is_selecting = event.click_count <= 1;
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let delta = event.delta.pixel_delta(layout.line_height);
        let old = self.scroll;
        if self.is_multi() {
            self.scroll.y = (self.scroll.y - delta.y).clamp(px(0.), layout.max_scroll.y);
        } else {
            // horizontal gestures only: a vertical wheel over a long single-line field must keep
            // scrolling the page (like NSTextField)
            self.scroll.x = (self.scroll.x - delta.x).clamp(px(0.), layout.max_scroll.x);
        }
        if self.scroll != old {
            // only swallow the wheel while we actually scroll, so the page scrolls otherwise
            cx.stop_propagation();
            cx.notify();
        }
    }

    // ---- UTF-16 conversion for the platform input handler ----

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if self.is_masked() {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.disabled && !ignore_disabled_input {
            return None;
        }
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let kind = if new_text.chars().any(char::is_whitespace) {
            EditKind::Other
        } else {
            EditKind::Insert
        };
        if range.is_empty() && new_text.is_empty() {
            self.marked_range = None;
            return;
        }
        self.replace_range(range, new_text, kind, cx);
        if kind == EditKind::Other {
            // whitespace ends a typing run: next word gets its own undo step
            self.last_edit = None;
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        if self.is_masked() {
            // No composition in a secret field: commit what the IME offers straight away.
            self.replace_range(range, new_text, EditKind::Insert, cx);
            return;
        }
        let range = self.clamp_offset(range.start)..self.clamp_offset(range.end);
        let new_text = self.sanitize(new_text.to_string());
        self.push_undo(EditKind::Insert);
        self.content.replace_range(range.clone(), &new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        // new_selected_range is relative to the inserted (marked) text, in UTF-16 of new_text
        self.selected_range = new_selected_range_utf16
            .map(|sel| {
                let to_utf8 = |u16_off: usize| {
                    let mut u8o = 0;
                    let mut u16o = 0;
                    for ch in new_text.chars() {
                        if u16o >= u16_off {
                            break;
                        }
                        u16o += ch.len_utf16();
                        u8o += ch.len_utf8();
                    }
                    u8o
                };
                range.start + to_utf8(sel.start)..range.start + to_utf8(sel.end)
            })
            .unwrap_or_else(|| {
                let c = range.start + new_text.len();
                c..c
            });
        self.selection_reversed = false;
        self.autoscroll = true;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let lh = self.line_height()?;
        let start = self.position_for_offset(range.start)?;
        let end = self.position_for_offset(range.end)?;
        let end_x = if (end.y - start.y).abs() < px(0.5) {
            end.x
        } else {
            start.x + px(1.)
        };
        Some(Bounds::from_corners(
            start,
            point(end_x.max(start.x), start.y + lh),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.layout.as_ref()?;
        Some(self.offset_to_utf16(self.offset_for_position(point)))
    }
}

// ---------------------------------------------------------------------------------------------
// Themed field: ui/input.tsx `FIELD` and the textarea rules
// ---------------------------------------------------------------------------------------------

/// The text field's look in the current theme: `FIELD` in ui/input.tsx.
///
/// The TSX field is `h-9 rounded-lg border-border bg-surface-strong px-3 text-[13px]`, with
/// the placeholder at `muted-foreground/70`, a `border-strong` edge on focus and a 2px ring.
/// The focus ring (`outline: 2px solid var(--ring)`, offset 2px) is a 2px spread shadow here:
/// gpui has no outline offset, so the ring hugs the border instead of floating 2px off it.
pub fn field_style(cx: &App) -> TextInputStyle {
    let colors = cx.theme().colors;
    TextInputStyle {
        background: colors.surface_strong,
        border: colors.border,
        border_focused: colors.border_strong,
        ring: colors.ring,
        placeholder: with_alpha(colors.muted_foreground, 0.7),
        selection: colors.ring,
        selection_unfocused: with_alpha(colors.muted_foreground, 0.2),
        cursor: colors.foreground,
        // `px-3` and `py-2`.
        padding_x: rems(0.75),
        padding_y: rems(0.5),
        radius: rems(0.875),
        disabled_opacity: 0.5,
    }
}

/// `Label` from ui/input.tsx: `mb-1.5 text-[12px] font-medium text-muted-foreground`.
pub fn label(text: impl Into<gpui::SharedString>, cx: &App) -> gpui::Div {
    div()
        .mb(crate::ui::theme::rpx(6.))
        .text_size(crate::ui::theme::rpx(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
}

// Render + the text element
// ---------------------------------------------------------------------------------------------

/// The caret blinks like the platform's: on for half a second, off for half a second, and
/// solid again right after any edit or movement.
const BLINK: std::time::Duration = std::time::Duration::from_millis(530);

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Re-read the theme every frame, so a light/dark switch reaches the field.
        self.style = field_style(cx);
        let focused = self.focus_handle.is_focused(window);

        // The blink runs only while the field has focus, so an idle window repaints nothing.
        if focused && !self.disabled {
            if self.blink_task.is_none() {
                self.blink_on = true;
                self.blink_task = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(BLINK).await;
                        let alive = this.update(cx, |input, cx| {
                            input.blink_on = !input.blink_on;
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                }));
            }
        } else if self.blink_task.take().is_some() {
            self.blink_on = true;
        }

        let style = &self.style;
        let multi = self.is_multi();
        let field = div()
            .debug_selector(|| "text-input".to_string())
            .key_context(KEY_CONTEXT)
            .w_full()
            .flex()
            .flex_col()
            .justify_center()
            // `text-[13px] text-foreground`; the textarea is `leading-relaxed` (1.625).
            .text_size(rpx(13.))
            .line_height(rpx(if multi { 21.125 } else { 20. }))
            .text_color(cx.theme().colors.foreground)
            .pl(self.padding_left.map_or(style.padding_x, rpx))
            .pr(self.padding_right.map_or(style.padding_x, rpx))
            // The single-line field is a fixed `h-9` box with its text centred; the
            // textarea is as tall as its rows plus `py-2`.
            .when(!multi, |d| d.h(rpx(self.height.unwrap_or(36.))))
            .when(multi, |d| d.py(style.padding_y))
            .rounded(style.radius)
            .border_1()
            .border_color(if focused {
                style.border_focused
            } else {
                style.border
            })
            .bg(style.background)
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: style.ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                }])
            });
        field
            .when(self.disabled, |d| {
                d.opacity(style.disabled_opacity).cursor(CursorStyle::Arrow)
            })
            .when(!self.disabled, |d| {
                d.track_focus(&self.focus_handle)
                    .cursor(CursorStyle::IBeam)
                    .on_action(cx.listener(Self::backspace))
                    .on_action(cx.listener(Self::delete))
                    .on_action(cx.listener(Self::delete_word_left))
                    .on_action(cx.listener(Self::delete_word_right))
                    .on_action(cx.listener(Self::delete_to_line_start))
                    .on_action(cx.listener(Self::delete_to_line_end))
                    .on_action(cx.listener(Self::left))
                    .on_action(cx.listener(Self::right))
                    .on_action(cx.listener(Self::up))
                    .on_action(cx.listener(Self::down))
                    .on_action(cx.listener(Self::select_left))
                    .on_action(cx.listener(Self::select_right))
                    .on_action(cx.listener(Self::select_up))
                    .on_action(cx.listener(Self::select_down))
                    .on_action(cx.listener(Self::word_left))
                    .on_action(cx.listener(Self::word_right))
                    .on_action(cx.listener(Self::select_word_left))
                    .on_action(cx.listener(Self::select_word_right))
                    .on_action(cx.listener(Self::line_start))
                    .on_action(cx.listener(Self::line_end))
                    .on_action(cx.listener(Self::select_line_start))
                    .on_action(cx.listener(Self::select_line_end))
                    .on_action(cx.listener(Self::doc_start))
                    .on_action(cx.listener(Self::doc_end))
                    .on_action(cx.listener(Self::select_doc_start))
                    .on_action(cx.listener(Self::select_doc_end))
                    .on_action(cx.listener(Self::on_select_all))
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::cut))
                    .on_action(cx.listener(Self::paste))
                    .on_action(cx.listener(Self::undo))
                    .on_action(cx.listener(Self::redo))
                    .on_action(cx.listener(Self::enter))
                    .on_action(cx.listener(Self::secondary_enter))
                    .on_action(cx.listener(Self::escape))
                    .on_action(cx.listener(Self::show_character_palette))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            })
            .child(TextElement { input: cx.entity() })
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    layout: InputLayout,
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    scroll: Point<Pixels>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// Text + runs to shape: the content (with the IME marked range underlined) or the placeholder.
fn display_runs(input: &TextInput, window: &Window) -> (SharedString, Vec<TextRun>, bool) {
    let style = window.text_style();
    let is_placeholder = input.content.is_empty();
    let (text, color): (SharedString, Hsla) = if is_placeholder {
        (input.placeholder.clone(), input.style.placeholder)
    } else {
        (input.display_text().into(), style.color)
    };
    let base = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let marked = input
        .marked_range
        .as_ref()
        .filter(|_| !is_placeholder)
        .map(|m| input.content_to_display(m.start)..input.content_to_display(m.end));
    let runs = match marked.as_ref() {
        Some(m) => [
            TextRun {
                len: m.start,
                ..base.clone()
            },
            TextRun {
                len: m.end - m.start,
                underline: Some(UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.),
                    wavy: false,
                }),
                ..base.clone()
            },
            TextRun {
                len: text.len() - m.end,
                ..base
            },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        None => vec![base],
    };
    (text, runs, is_placeholder)
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        match input.mode {
            InputMode::SingleLine => {
                style.size.height = line_height.into();
                (window.request_layout(style, [], cx), ())
            }
            InputMode::MultiLine { min_rows, max_rows } => {
                // Height depends on the wrap width, which is only known during layout:
                // measure by shaping the text at the width taffy offers.
                let (text, runs, _) = display_runs(input, window);
                let font_size = window.text_style().font_size.to_pixels(window.rem_size());
                let layout_id =
                    window.request_measured_layout(style, move |known, available, window, _cx| {
                        let width = known.width.unwrap_or(match available.width {
                            AvailableSpace::Definite(w) => w,
                            _ => px(10_000.),
                        });
                        let rows = window
                            .text_system()
                            .shape_text(
                                text.clone(),
                                font_size,
                                &runs,
                                Some(width.max(px(1.))),
                                None,
                            )
                            .map(|lines| {
                                lines
                                    .iter()
                                    .map(|l| l.wrap_boundaries.len() + 1)
                                    .sum::<usize>()
                            })
                            .unwrap_or(1);
                        let rows = rows.clamp(min_rows, max_rows);
                        size(width, line_height * rows as f32)
                    });
                (layout_id, ())
            }
        }
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let multi = input.is_multi();
        let line_height = window.line_height();
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let (text, runs, is_placeholder) = display_runs(input, window);
        let wrap_width = multi.then(|| bounds.size.width.max(px(1.)));
        let lines = window
            .text_system()
            .shape_text(text, font_size, &runs, wrap_width, None)
            .map(|l| l.into_vec())
            .unwrap_or_default();

        // ---- rows ----
        let mut rows = Vec::new();
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut line_first_row = Vec::with_capacity(lines.len());
        let mut start = 0;
        for (li, line) in lines.iter().enumerate() {
            line_starts.push(start);
            line_first_row.push(rows.len());
            let mut row_start = start;
            let mut row_start_x = px(0.);
            for b in line.wrap_boundaries.iter() {
                let glyph = &line.unwrapped_layout.runs[b.run_ix].glyphs[b.glyph_ix];
                rows.push(Row {
                    line: li,
                    start: row_start,
                    end: start + glyph.index,
                    start_x: row_start_x,
                    last_in_line: false,
                });
                row_start = start + glyph.index;
                row_start_x = glyph.position.x;
            }
            rows.push(Row {
                line: li,
                start: row_start,
                end: start + line.len(),
                start_x: row_start_x,
                last_in_line: true,
            });
            start += line.len() + 1;
        }

        let mut layout = InputLayout {
            lines,
            line_starts,
            line_first_row,
            rows,
            line_height,
            bounds,
            max_scroll: Point::default(),
            is_placeholder,
        };

        // ---- scrolling ----
        let cursor_w = px(2.);
        let content_h = line_height * layout.rows.len() as f32;
        let content_w = layout
            .rows
            .iter()
            .map(|r| layout.row_width(r))
            .fold(px(0.), Pixels::max)
            + cursor_w;
        layout.max_scroll = if multi {
            point(px(0.), (content_h - bounds.size.height).max(px(0.)))
        } else {
            point((content_w - bounds.size.width).max(px(0.)), px(0.))
        };
        let head = if is_placeholder {
            0
        } else {
            input.content_to_display(input.cursor_offset())
        };
        let head_row = layout.row_for_offset(head);
        let head_x = layout
            .rows
            .get(head_row)
            .map_or(px(0.), |r| layout.x_for(r, head.clamp(r.start, r.end)));
        let head_y = line_height * head_row as f32;
        let mut scroll = input.scroll;
        if input.autoscroll {
            if multi {
                if head_y < scroll.y {
                    scroll.y = head_y;
                } else if head_y + line_height > scroll.y + bounds.size.height {
                    scroll.y = head_y + line_height - bounds.size.height;
                }
            } else if head_x < scroll.x {
                scroll.x = head_x;
            } else if head_x + cursor_w > scroll.x + bounds.size.width {
                scroll.x = head_x + cursor_w - bounds.size.width;
            }
        }
        scroll.x = scroll.x.clamp(px(0.), layout.max_scroll.x);
        scroll.y = scroll.y.clamp(px(0.), layout.max_scroll.y);

        // ---- selection & cursor quads (window coordinates) ----
        let origin = bounds.origin - scroll;
        let focused = input.focus_handle.is_focused(window);
        let mut selection = Vec::new();
        let sel = input.content_to_display(input.selected_range.start)
            ..input.content_to_display(input.selected_range.end);
        if !sel.is_empty() && !is_placeholder {
            let newline_w = font_size * 0.4;
            let color = if focused {
                input.style.selection
            } else {
                input.style.selection_unfocused
            };
            for (ix, row) in layout.rows.iter().enumerate() {
                if sel.end < row.start || sel.start > row.end {
                    continue;
                }
                let s = sel.start.max(row.start);
                let e = sel.end.min(row.end);
                let covers_newline = row.last_in_line && sel.start <= row.end && sel.end > row.end;
                if s >= e && !covers_newline {
                    continue;
                }
                let x0 = layout.x_for(row, s.min(row.end));
                let x1 = layout.x_for(row, e.max(s).min(row.end))
                    + if covers_newline { newline_w } else { px(0.) };
                let y = line_height * ix as f32;
                selection.push(fill(
                    Bounds::from_corners(
                        origin + point(x0, y),
                        origin + point(x1, y + line_height),
                    ),
                    color,
                ));
            }
        }
        let cursor = (focused && sel.is_empty() && !input.disabled && input.blink_on).then(|| {
            fill(
                Bounds::new(origin + point(head_x, head_y), size(px(1.), line_height)),
                input.style.cursor,
            )
        });

        // Persist geometry and the clamped scroll for event handling.
        let stored = layout.clone();
        self.input.update(cx, |input, _| {
            input.layout = Some(stored);
            input.scroll = scroll;
            input.autoscroll = false;
        });

        PrepaintState {
            layout,
            selection,
            cursor,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, disabled) = {
            let input = self.input.read(cx);
            (input.focus_handle.clone(), input.disabled)
        };
        if !disabled {
            // Registers this element as the window's text input target while focused (IME,
            // dead keys, emoji picker, plain typing).
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }

        // Drag-selection continues outside the element: listen on the window, not the div.
        let input = self.input.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
                input.update(cx, |input, cx| {
                    if input.is_selecting {
                        let ix = input.offset_for_position(event.position);
                        input.select_to(ix, cx);
                    }
                });
            }
        });
        let input = self.input.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
            if phase == DispatchPhase::Bubble {
                input.update(cx, |input, _| input.is_selecting = false);
            }
        });

        let layout = &prepaint.layout;
        let scroll = prepaint.scroll;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }
            let lh = layout.line_height;
            for (li, line) in layout.lines.iter().enumerate() {
                let first = layout.line_first_row[li];
                let rows = line.wrap_boundaries.len() + 1;
                let top = lh * first as f32 - scroll.y;
                // skip lines entirely outside the visible area
                if top > bounds.size.height || top + lh * (rows as f32) < px(0.) {
                    continue;
                }
                let origin = bounds.origin + point(-scroll.x, top);
                line.paint(origin, lh, TextAlign::Left, None, window, cx)
                    .ok();
            }
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod interaction;
