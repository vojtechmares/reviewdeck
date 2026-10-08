//! Markdown rendered natively: the element tree for a `reviewdeck_core::markdown`
//! [`Document`]. Port of src/renderer/src/components/Markdown.tsx, with the prose
//! rules of src/renderer/src/index.css (`.md` and `.md-compact`) turned into gpui
//! styles.
//!
//! [`MarkdownView`] is the entity a surface holds. It parses the source on a
//! background task, highlights its fenced code there too (through [`code`]), and
//! renders the cached tree. Render itself never parses or highlights.
//!
//! Known differences from the browser page, all because gpui has no per-run size,
//! no baseline shift and no CSS borders on text runs:
//! - inline code, `sub`, `sup` and `kbd` keep the body size (they are monospace or
//!   plain; `kbd` loses its border and keeps the surface colour);
//! - links are coloured but not underlined on hover, and only http(s) links open;
//!   `mailto:` links look like links and do nothing;
//! - footnote references are the plain number;
//! - text selection is not available: gpui cannot select static text.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, AsyncApp, ClickEvent, Context, CursorStyle, FontStyle, FontWeight, Hsla,
    Image, InteractiveText, Render, SharedString, StrikethroughStyle, StyledText, Task, TextRun,
    UnderlineStyle, WeakEntity, Window, div, font, img, px, relative, svg,
};
use reviewdeck_core::images::ImageSource;
use reviewdeck_core::markdown::{
    AlertKind, Alignment, Block, Document, Inline, Length, List, MarkdownContext, Table, TableCell,
    TableRow, parse,
};

use crate::ui::code::{self, CodeKind};
use crate::ui::icons::IconName;
use crate::ui::theme::{ActiveTheme, Colors, MONO_FONT, UI_FONT, rpx};

/// Loads an authenticated image: `(account id, url, app)` -> the decoded image, or
/// `None` while it is loading or when it failed. The app notifies whoever shows it
/// when the image arrives, and the view then asks again.
pub type ImageLoader = Rc<dyn Fn(&str, &str, &mut App) -> Option<Arc<Image>>>;

/// `font-weight: 650` in index.css, used for headings and `strong`.
const PROSE_WEIGHT: f32 = 650.;

/// The lucide icon each GitHub alert kind carries, as Markdown.tsx's ALERTS table
/// assigns them.
fn alert_icon(kind: AlertKind) -> IconName {
    match kind {
        AlertKind::Note => IconName::Info,
        AlertKind::Tip => IconName::Lightbulb,
        AlertKind::Important => IconName::MessageSquareWarning,
        AlertKind::Warning => IconName::TriangleAlert,
        AlertKind::Caution => IconName::OctagonAlert,
    }
}

/// The metrics of `.md` (body) or `.md-compact` (a comment on a diff line), every
/// length in CSS pixels. The em values of the stylesheet are resolved here, against
/// the font size of the element they apply to.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    compact: bool,
    size: f32,
    line: f32,
    /// `p`, `ul`, `ol`, `blockquote`, `pre`, `table`, `details`: `0.6em`.
    gap: f32,
    /// `padding-left` of a list.
    indent: f32,
    quote_pad: f32,
    pre_size: f32,
    pre_pad: (f32, f32),
    table_size: f32,
    cell_pad: (f32, f32),
    details_pad: (f32, f32),
    /// `hr` margin, top and bottom.
    rule: f32,
    /// Heading multipliers for h1 to h6.
    heading_scale: [f32; 6],
    /// Heading margins, in ems of the heading's own size.
    heading_top: f32,
    heading_bottom: f32,
    max_image_height: Option<f32>,
}

fn metrics(compact: bool) -> Metrics {
    if compact {
        Metrics {
            compact,
            size: 12.,
            line: 18.,
            gap: 0.45 * 12.,
            indent: 1.15 * 12.,
            quote_pad: 0.7 * 12.,
            pre_size: 0.9 * 12.,
            pre_pad: (0.5 * 12., 0.6 * 12.),
            table_size: 0.95 * 12.,
            cell_pad: (0.25 * 12., 0.5 * 12.),
            details_pad: (0.4 * 12., 0.6 * 12.),
            rule: 0.8 * 12.,
            heading_scale: [1.08, 1.04, 1., 1., 1., 1.],
            heading_top: 0.7,
            heading_bottom: 0.3,
            max_image_height: Some(14. * 16.),
        }
    } else {
        Metrics {
            compact,
            size: 13.,
            line: 1.6 * 13.,
            gap: 0.6 * 13.,
            indent: 1.4 * 13.,
            quote_pad: 0.85 * 13.,
            pre_size: 0.9 * 13.,
            pre_pad: (0.7 * 13., 0.85 * 13.),
            table_size: 0.94 * 13.,
            cell_pad: (0.35 * 13., 0.7 * 13.),
            details_pad: (0.5 * 13., 0.75 * 13.),
            rule: 1.2 * 13.,
            heading_scale: [1.25, 1.15, 1.05, 1., 1., 1.],
            heading_top: 1.2,
            heading_bottom: 0.5,
            max_image_height: None,
        }
    }
}

/// The space above and below a block. Neighbouring blocks collapse their margins
/// the way CSS does (the larger one wins), which [`stack`] reproduces.
#[derive(Debug, Clone, Copy)]
struct Gap {
    top: f32,
    bottom: f32,
}

impl Gap {
    const NONE: Gap = Gap {
        top: 0.,
        bottom: 0.,
    };

    fn both(px: f32) -> Gap {
        Gap {
            top: px,
            bottom: px,
        }
    }
}

/// What the elements below need from their surroundings and are not in the tree:
/// the metrics, the theme, the state of `details` and the image loader.
struct Env<'a> {
    m: Metrics,
    colors: Colors,
    dark: bool,
    open: &'a HashMap<usize, bool>,
    images: Option<&'a ImageLoader>,
}

/// The text style inherited from the enclosing elements. Runs carry their own font
/// and colour, so they need it spelled out, where a plain element would inherit it.
#[derive(Debug, Clone, Copy)]
struct Base {
    color: Hsla,
    weight: f32,
    align: Option<Alignment>,
}

/// The style of one run of text.
#[derive(Debug, Clone, Copy)]
struct RunStyle {
    color: Hsla,
    weight: f32,
    italic: bool,
    mono: bool,
    underline: bool,
    strike: bool,
    background: Option<Hsla>,
}

impl RunStyle {
    fn of(base: Base) -> RunStyle {
        RunStyle {
            color: base.color,
            weight: base.weight,
            italic: false,
            mono: false,
            underline: false,
            strike: false,
            background: None,
        }
    }
}

/// Inline text that becomes one [`StyledText`]: the characters, one run per piece of
/// style, and the byte ranges that are links.
#[derive(Default)]
struct Flow {
    text: String,
    runs: Vec<TextRun>,
    links: Vec<(Range<usize>, String)>,
}

impl Flow {
    fn push(&mut self, piece: &str, style: RunStyle) {
        if piece.is_empty() {
            return;
        }
        self.text.push_str(piece);
        self.runs.push(run(piece.len(), style));
    }
}

/// A piece of a paragraph. Text is set together; an image or a checkbox breaks it,
/// and the pieces then sit side by side in a wrapping row.
enum Piece<'a> {
    Text(Flow),
    Image(&'a reviewdeck_core::markdown::Image),
    Check(bool),
}

fn run(len: usize, style: RunStyle) -> TextRun {
    let mut face = font(if style.mono { MONO_FONT } else { UI_FONT });
    face.weight = FontWeight(style.weight);
    if style.italic {
        face.style = FontStyle::Italic;
    }
    TextRun {
        len,
        font: face,
        color: style.color,
        background_color: style.background,
        underline: style.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(style.color),
            wavy: false,
        }),
        strikethrough: style.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(style.color),
        }),
    }
}

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// Only these reach `cx.open_url`. Everything else in a body is inert.
fn is_web_link(href: &str) -> bool {
    href.starts_with("https://") || href.starts_with("http://")
}

/// Stable identity of a node of the parsed tree, for state kept per node. The
/// document is not moved while it is shown, so an address names a node for as long
/// as the view holds that document.
fn node_key<T>(node: &T) -> usize {
    node as *const T as usize
}

fn text_of_code(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

/// Adds one inline element's text to the pieces, with the style it carries.
fn flow_inlines<'a>(
    out: &mut Vec<Piece<'a>>,
    inlines: &'a [Inline],
    style: RunStyle,
    colors: &Colors,
) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => push_text(out, text, style),
            Inline::Emphasis(children) => flow_inlines(
                out,
                children,
                RunStyle {
                    italic: true,
                    ..style
                },
                colors,
            ),
            Inline::Strong(children) => flow_inlines(
                out,
                children,
                RunStyle {
                    weight: PROSE_WEIGHT,
                    ..style
                },
                colors,
            ),
            Inline::Strikethrough(children) => flow_inlines(
                out,
                children,
                RunStyle {
                    strike: true,
                    color: with_alpha(style.color, style.color.a * 0.65),
                    ..style
                },
                colors,
            ),
            Inline::Underline(children) => flow_inlines(
                out,
                children,
                RunStyle {
                    underline: true,
                    ..style
                },
                colors,
            ),
            Inline::Code(text) => push_text(
                out,
                text,
                RunStyle {
                    mono: true,
                    background: Some(colors.muted),
                    ..style
                },
            ),
            Inline::Kbd(children) => flow_inlines(
                out,
                children,
                RunStyle {
                    mono: true,
                    background: Some(colors.surface_muted),
                    ..style
                },
                colors,
            ),
            // Sub and sup keep their text at body size: gpui cannot shrink a run or
            // move its baseline. The words stay in place.
            Inline::Sub(children) | Inline::Sup(children) => {
                flow_inlines(out, children, style, colors)
            }
            Inline::Link { href, children, .. } => {
                let piece = ensure_text(out);
                let start = text_len(out, piece);
                flow_inlines(
                    out,
                    children,
                    RunStyle {
                        color: colors.info,
                        ..style
                    },
                    colors,
                );
                if let (Some(href), Some(Piece::Text(flow))) = (href, out.get_mut(piece))
                    && flow.text.len() > start
                {
                    flow.links.push((start..flow.text.len(), href.clone()));
                }
            }
            Inline::Image(image) => out.push(Piece::Image(image)),
            Inline::LineBreak => push_text(out, "\n", style),
            Inline::Checkbox { checked } => out.push(Piece::Check(*checked)),
            Inline::FootnoteRef { number, .. } => push_text(
                out,
                &number.to_string(),
                RunStyle {
                    color: colors.info,
                    ..style
                },
            ),
        }
    }
}

fn push_text<'a>(out: &mut Vec<Piece<'a>>, text: &str, style: RunStyle) {
    if let Some(Piece::Text(flow)) = out.last_mut() {
        flow.push(text, style);
        return;
    }
    let mut flow = Flow::default();
    flow.push(text, style);
    out.push(Piece::Text(flow));
}

/// Makes sure the last piece is text and returns its index, for a link to note
/// where its text starts.
fn ensure_text(out: &mut Vec<Piece<'_>>) -> usize {
    if !matches!(out.last(), Some(Piece::Text(_))) {
        out.push(Piece::Text(Flow::default()));
    }
    out.len() - 1
}

fn text_len(out: &[Piece<'_>], piece: usize) -> usize {
    match out.get(piece) {
        Some(Piece::Text(flow)) => flow.text.len(),
        _ => 0,
    }
}

fn pieces_of<'a>(inlines: &'a [Inline], style: RunStyle, colors: &Colors) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    flow_inlines(&mut out, inlines, style, colors);
    out
}

/// A paragraph's inline content. Plain text is one [`StyledText`] (with a click
/// handler when it holds links); images and checkboxes make it a wrapping row.
fn inline_element(
    pieces: Vec<Piece<'_>>,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let mixed = pieces.iter().any(|piece| !matches!(piece, Piece::Text(_)));
    if !mixed {
        return match pieces.into_iter().next() {
            Some(Piece::Text(flow)) => text_element(flow),
            _ => div().into_any_element(),
        };
    }

    let mut row = div().flex().flex_row().flex_wrap().items_center();
    row = match base.align {
        Some(Alignment::Center) => row.justify_center(),
        Some(Alignment::Right) => row.justify_end(),
        _ => row,
    };
    for piece in pieces {
        row = match piece {
            Piece::Text(flow) if flow.text.is_empty() => row,
            Piece::Text(flow) => row.child(text_element(flow)),
            Piece::Image(image) => row.child(image_element(image, env, cx)),
            Piece::Check(checked) => row.child(checkbox(checked, &env.colors)),
        };
    }
    row.into_any_element()
}

fn text_element(flow: Flow) -> AnyElement {
    let Flow { text, runs, links } = flow;
    if text.is_empty() {
        return div().into_any_element();
    }
    // The address of the run list names this text for the whole life of the tree,
    // which is what gpui needs to keep the hover and click state of a link.
    let id_key = runs.as_ptr() as usize;
    let styled = StyledText::new(SharedString::from(text)).with_runs(runs);
    if links.is_empty() {
        return styled.into_any_element();
    }

    let (ranges, hrefs): (Vec<Range<usize>>, Vec<String>) = links.into_iter().unzip();
    let hover_ranges = ranges.clone();
    let hrefs = Rc::new(hrefs);
    InteractiveText::new(("md-link", id_key), styled)
        .on_click(ranges, move |index, _window, cx| {
            if let Some(href) = hrefs.get(index)
                && is_web_link(href)
            {
                cx.open_url(href);
            }
        })
        .on_hover(move |index, _event, window, _cx| {
            let on_link = index.is_some_and(|at| hover_ranges.iter().any(|r| r.contains(&at)));
            let style = if on_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            };
            window.set_window_cursor_style(style);
        })
        .into_any_element()
}

/// A GitHub-style checkbox, disabled, as the browser draws `input[type=checkbox]`
/// in the app's accent colour.
fn checkbox(checked: bool, colors: &Colors) -> AnyElement {
    let frame = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .mt(rpx(2.))
        .size(rpx(13.))
        .rounded(rpx(3.))
        .border_1();
    if checked {
        frame
            .border_color(colors.primary)
            .bg(colors.primary)
            .text_color(colors.primary_foreground)
            .text_size(rpx(10.))
            .font_weight(FontWeight(700.))
            .child("✓")
            .into_any_element()
    } else {
        frame
            .border_color(colors.border_strong)
            .bg(colors.card)
            .into_any_element()
    }
}

fn alt_element(alt: &str, base: Base) -> AnyElement {
    div()
        .text_color(base.color)
        .child(SharedString::from(alt.to_string()))
        .into_any_element()
}

/// An image: remote through gpui's own loader, authenticated through the app, and
/// the alt text while there is nothing to show (blocked, loading or failed).
fn image_element(
    image: &reviewdeck_core::markdown::Image,
    env: &Env<'_>,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let alt = image.alt.clone();
    let base = Base {
        color: env.colors.muted_foreground,
        weight: 400.,
        align: None,
    };
    let mut el = match &image.source {
        ImageSource::Remote(url) => img(url.clone()),
        ImageSource::Authenticated { account_id, url } => {
            match env.images.and_then(|load| load(account_id, url, cx)) {
                Some(picture) => img(picture),
                None => return alt_element(&alt, base),
            }
        }
        ImageSource::Blocked => return alt_element(&alt, base),
    };
    el = match image.width {
        Some(Length::Pixels(v)) => el.w(rpx(v)),
        Some(Length::Percent(v)) => el.w(relative(v / 100.)),
        None => el,
    };
    el = match image.height {
        Some(Length::Pixels(v)) => el.h(rpx(v)),
        Some(Length::Percent(v)) => el.h(relative(v / 100.)),
        None => el,
    };
    if let Some(max) = env.m.max_image_height {
        el = el.max_h(rpx(max));
    }
    el.max_w(relative(1.))
        .rounded(rpx(6.))
        .with_fallback(move || alt_element(&alt, base))
        .into_any_element()
}

/// Stacks blocks vertically, collapsing the margins between neighbours the way CSS
/// does: the larger of the two wins. The first top and the last bottom margin are
/// dropped, as `.md > *:first-child` and `:last-child` do.
fn stack(children: Vec<(Gap, AnyElement)>) -> AnyElement {
    let mut column = div().flex().flex_col().min_w_0();
    let mut above = 0.;
    for (index, (gap, element)) in children.into_iter().enumerate() {
        let top = if index == 0 { 0. } else { gap.top.max(above) };
        above = gap.bottom;
        column = column.child(div().pt(rpx(top)).min_w_0().child(element));
    }
    column.into_any_element()
}

fn blocks_element(
    blocks: &[Block],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let children = blocks
        .iter()
        .map(|block| block_child(block, env, base, cx, false))
        .collect();
    stack(children)
}

/// One block, with the margins it has. `in_item` is set for a block inside a list
/// item, where nested lists have smaller margins (`.md li > ul`).
fn block_child(
    block: &Block,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
    in_item: bool,
) -> (Gap, AnyElement) {
    let m = env.m;
    let colors = &env.colors;
    match block {
        Block::Paragraph(inlines) => {
            let pieces = pieces_of(inlines, RunStyle::of(base), colors);
            (Gap::both(m.gap), inline_element(pieces, env, base, cx))
        }
        Block::Plain(inlines) => {
            let pieces = pieces_of(inlines, RunStyle::of(base), colors);
            (Gap::NONE, inline_element(pieces, env, base, cx))
        }
        Block::Heading { level, inlines } => {
            let index = usize::from(level.saturating_sub(1)).min(5);
            let size = m.size * m.heading_scale[index];
            let heading_base = Base {
                weight: PROSE_WEIGHT,
                ..base
            };
            let pieces = pieces_of(inlines, RunStyle::of(heading_base), colors);
            let mut el = div()
                .text_size(rpx(size))
                .line_height(rpx(size * 1.3))
                .font_weight(FontWeight(PROSE_WEIGHT))
                .min_w_0()
                .child(inline_element(pieces, env, heading_base, cx));
            // The underline under the two largest headings, as the stylesheet draws it.
            if *level <= 2 && !m.compact {
                el = el
                    .pb(rpx(size * 0.25))
                    .border_b_1()
                    .border_color(colors.border);
            }
            let gap = Gap {
                top: size * m.heading_top,
                bottom: size * m.heading_bottom,
            };
            (gap, el.into_any_element())
        }
        Block::BlockQuote(blocks) => {
            let inner = Base {
                color: colors.muted_foreground,
                ..base
            };
            let el = div()
                .border_l(px(3.))
                .border_color(colors.border_strong)
                .pl(rpx(m.quote_pad))
                .text_color(colors.muted_foreground)
                .child(blocks_element(blocks, env, inner, cx));
            (Gap::both(m.gap), el.into_any_element())
        }
        Block::Alert { kind, blocks } => (
            Gap::both(0.7 * m.size),
            alert_element(*kind, blocks, env, base, cx),
        ),
        Block::CodeBlock { lang, text } => {
            (Gap::both(m.gap), code_block(lang.as_deref(), text, env))
        }
        Block::List(list) => {
            let gap = if in_item { 0.2 * m.size } else { m.gap };
            (Gap::both(gap), list_element(list, env, base, cx))
        }
        Block::Table(table) => (Gap::both(m.gap), table_element(table, env, base, cx)),
        Block::Rule => {
            let el = div().w_full().h(px(1.)).bg(colors.border);
            (Gap::both(m.rule), el.into_any_element())
        }
        Block::Details {
            open,
            summary,
            blocks,
        } => (
            Gap::both(m.gap),
            details_element(block, *open, summary.as_deref(), blocks, env, base, cx),
        ),
        Block::Div { align, blocks } => {
            let inner = Base {
                align: *align,
                ..base
            };
            let mut el = div().min_w_0();
            el = match align {
                Some(Alignment::Center) => el.text_center(),
                Some(Alignment::Right) => el.text_right(),
                _ => el,
            };
            (
                Gap::NONE,
                el.child(blocks_element(blocks, env, inner, cx))
                    .into_any_element(),
            )
        }
    }
}

fn alert_element(
    kind: AlertKind,
    blocks: &[Block],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let colors = &env.colors;
    let (tone, soft) = match kind {
        AlertKind::Note | AlertKind::Important => (colors.info, colors.info_soft),
        AlertKind::Tip => (colors.ok, colors.ok_soft),
        AlertKind::Warning => (colors.busy, colors.busy_soft),
        AlertKind::Caution => (colors.bad, colors.bad_soft),
    };
    let label = SharedString::from(kind.label());
    let title = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(rpx(6.))
        .mb(rpx(4.))
        .text_size(rpx(12.))
        .font_weight(FontWeight(600.))
        .text_color(tone)
        .child(
            svg()
                .path(alert_icon(kind).path())
                .size(rpx(14.))
                .flex_none()
                .text_color(tone),
        )
        .child(label);
    let body = Base {
        color: colors.foreground,
        ..base
    };
    div()
        .rounded(rpx(10.))
        .border_1()
        .border_color(with_alpha(tone, 0.3))
        .bg(soft)
        .px(rpx(12.))
        .py(rpx(10.))
        .min_w_0()
        .child(title)
        .child(blocks_element(blocks, env, body, cx))
        .into_any_element()
}

/// A fenced block: `pre` with its highlighted lines, drawn plain while the colour is
/// on its way (or when the grammar cannot read it).
fn code_block(lang: Option<&str>, text: &str, env: &Env<'_>) -> AnyElement {
    let text = text_of_code(text);
    let tokens = lang
        .and_then(|tag| code::cached(&CodeKind::Fence(tag.to_string()), text, env.dark))
        .flatten();
    let m = env.m;
    let mut column = div().flex().flex_col();
    for (index, line) in text.split('\n').enumerate() {
        let tokens = tokens
            .as_ref()
            .and_then(|lines| lines.get(index))
            .map(|line| line.as_slice());
        column = column.child(
            div()
                .whitespace_nowrap()
                .child(code::code_line(line, tokens)),
        );
    }
    let (pad_y, pad_x) = m.pre_pad;
    let el = div()
        .id(("md-pre", text.as_ptr() as usize))
        .overflow_x_scroll()
        .py(rpx(pad_y))
        .px(rpx(pad_x))
        .rounded(rpx(10.))
        .border_1()
        .border_color(env.colors.border)
        .bg(env.colors.surface_muted)
        .font_family(MONO_FONT)
        .text_size(rpx(m.pre_size))
        .line_height(rpx(m.pre_size * 1.6))
        .text_color(env.colors.foreground)
        .child(column);
    el.into_any_element()
}

fn list_element(
    list: &List,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let m = env.m;
    // A list with any task item drops its bullets and numbers (`contains-task-list`).
    let tasks = list.items.iter().any(|item| item.task.is_some());
    let mut children = Vec::with_capacity(list.items.len());
    for (index, item) in list.items.iter().enumerate() {
        let content = {
            let mut column = Vec::with_capacity(item.blocks.len());
            for block in &item.blocks {
                column.push(block_child(block, env, base, cx, true));
            }
            stack(column)
        };
        let row = if let Some(checked) = item.task {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(rpx(0.5 * m.size))
                .child(checkbox(checked, &env.colors))
                .child(div().flex_1().min_w_0().child(content))
        } else {
            let marker = if list.ordered {
                format!("{}.", list.start.saturating_add(index as u64))
            } else {
                "•".to_string()
            };
            div()
                .flex()
                .flex_row()
                .items_start()
                .child(
                    div()
                        .flex_none()
                        .w(rpx(m.indent))
                        .pr(rpx(0.6 * m.size))
                        .text_right()
                        .child(SharedString::from(marker)),
                )
                .child(div().flex_1().min_w_0().child(content))
        };
        children.push((Gap::both(0.2 * m.size), row.into_any_element()));
    }
    let list_el = stack(children);
    let padding = if tasks { 0.15 * m.size } else { 0. };
    div()
        .pl(rpx(padding))
        .min_w_0()
        .child(list_el)
        .into_any_element()
}

/// A `details` block. Its open state lives in the view, keyed by the node, and the
/// summary toggles it.
fn details_element(
    block: &Block,
    initially_open: bool,
    summary: Option<&[Inline]>,
    blocks: &[Block],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let key = node_key(block);
    let open = env.open.get(&key).copied().unwrap_or(initially_open);
    let colors = &env.colors;

    let summary_base = Base {
        weight: 600.,
        ..base
    };
    let summary_pieces = match summary {
        Some(inlines) => pieces_of(inlines, RunStyle::of(summary_base), colors),
        None => vec![Piece::Text(text_flow(
            "Details",
            RunStyle::of(summary_base),
        ))],
    };
    let toggle = cx.listener(
        move |this: &mut MarkdownView, _: &ClickEvent, _window, cx| {
            let now = this.open.get(&key).copied().unwrap_or(initially_open);
            this.open.insert(key, !now);
            cx.notify();
        },
    );
    let marker = if open { "▾" } else { "▸" };
    let head = div()
        .id(("md-summary", key))
        .cursor_pointer()
        .flex()
        .flex_row()
        .items_start()
        .gap(rpx(6.))
        .font_weight(FontWeight(600.))
        .on_click(toggle)
        .child(div().flex_none().child(SharedString::from(marker)))
        .child(div().flex_1().min_w_0().child(inline_element(
            summary_pieces,
            env,
            summary_base,
            cx,
        )));

    let (pad_y, pad_x) = env.m.details_pad;
    let mut el = div()
        .px(rpx(pad_x))
        .py(rpx(pad_y))
        .rounded(rpx(10.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface_muted)
        .min_w_0()
        .child(head);
    if open {
        el = el.child(
            div()
                .pt(rpx(0.4 * env.m.size))
                .child(blocks_element(blocks, env, base, cx)),
        );
    }
    el.into_any_element()
}

fn text_flow(text: &str, style: RunStyle) -> Flow {
    let mut flow = Flow::default();
    flow.push(text, style);
    flow
}

fn table_element(
    table: &Table,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let rows: Vec<(&TableRow, bool)> = table
        .head
        .iter()
        .map(|row| (row, true))
        .chain(table.rows.iter().map(|row| (row, false)))
        .collect();
    let columns = rows
        .iter()
        .map(|(row, _)| row.cells.len())
        .max()
        .unwrap_or(0)
        .max(table.alignments.len());
    if columns == 0 {
        return div().into_any_element();
    }

    // Laid out by column, so every cell of a column is as wide as its widest one.
    let mut grid = div().flex().flex_row().items_start();
    for column in 0..columns {
        let mut col = div().flex().flex_col().min_w_0();
        for (row_index, (row, head)) in rows.iter().enumerate() {
            let cell = row.cells.get(column);
            let align = cell
                .and_then(|cell| cell.align)
                .or_else(|| table.alignments.get(column).copied().flatten());
            let last_row = row_index + 1 == rows.len();
            let last_column = column + 1 == columns;
            col = col.child(cell_element(
                cell,
                *head,
                align,
                (last_row, last_column),
                env,
                base,
                cx,
            ));
        }
        grid = grid.child(col);
    }

    let frame = div()
        .border_t_1()
        .border_l_1()
        .border_color(env.colors.border)
        .child(grid);
    div()
        .id(("md-table", node_key(table)))
        .overflow_x_scroll()
        .max_w(relative(1.))
        .text_size(rpx(env.m.table_size))
        .child(frame)
        .into_any_element()
}

fn cell_element(
    cell: Option<&TableCell>,
    head: bool,
    align: Option<Alignment>,
    (last_row, last_column): (bool, bool),
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let (pad_y, pad_x) = env.m.cell_pad;
    let inner = Base {
        weight: if head { 600. } else { base.weight },
        align,
        ..base
    };
    let mut el = div()
        .px(rpx(pad_x))
        .py(rpx(pad_y))
        .border_color(env.colors.border)
        .min_w_0();
    if head {
        el = el.bg(env.colors.muted).font_weight(FontWeight(600.));
    }
    if !last_column {
        el = el.border_r_1();
    }
    if !last_row {
        el = el.border_b_1();
    }
    el = match align {
        Some(Alignment::Center) => el.text_center(),
        Some(Alignment::Right) => el.text_right(),
        _ => el,
    };
    match cell {
        Some(cell) => el.child(blocks_element(&cell.blocks, env, inner, cx)),
        None => el,
    }
    .into_any_element()
}

fn footnotes_element(
    footnotes: &[reviewdeck_core::markdown::Footnote],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let m = env.m;
    let rule = div().w_full().h(px(1.)).bg(env.colors.border);
    let mut children = vec![(Gap::both(m.rule), rule.into_any_element())];
    for footnote in footnotes {
        let marker = format!("{}.", footnote.number);
        let row = div()
            .flex()
            .flex_row()
            .items_start()
            .child(
                div()
                    .flex_none()
                    .w(rpx(m.indent))
                    .pr(rpx(0.6 * m.size))
                    .text_right()
                    .child(SharedString::from(marker)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(blocks_element(&footnote.blocks, env, base, cx)),
            );
        children.push((Gap::both(0.2 * m.size), row.into_any_element()));
    }
    stack(children)
}

/// Gives every block's code its highlight, on the background, so render only looks
/// it up. Runs once per parse, and again when the theme changes.
fn highlight_document(document: &Document, dark: bool) {
    fn walk(blocks: &[Block], dark: bool) {
        for block in blocks {
            match block {
                Block::CodeBlock {
                    lang: Some(tag),
                    text,
                } => {
                    code::highlight(&CodeKind::Fence(tag.clone()), text_of_code(text), dark);
                }
                Block::BlockQuote(blocks)
                | Block::Alert { blocks, .. }
                | Block::Details { blocks, .. }
                | Block::Div { blocks, .. } => walk(blocks, dark),
                Block::List(list) => {
                    for item in &list.items {
                        walk(&item.blocks, dark);
                    }
                }
                Block::Table(table) => {
                    for row in table.head.iter().chain(table.rows.iter()) {
                        for cell in &row.cells {
                            walk(&cell.blocks, dark);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    walk(&document.blocks, dark);
    for footnote in &document.footnotes {
        walk(&footnote.blocks, dark);
    }
}

/// The markdown surface: source in, rendered body out.
///
/// Create it with `cx.new(|cx| MarkdownView::new(..))`, and call [`set_source`] when
/// the text changes. A parse is only started for a new source, and the old tree stays
/// on screen until the new one is ready.
///
/// [`set_source`]: MarkdownView::set_source
pub struct MarkdownView {
    source: SharedString,
    context: MarkdownContext,
    images: Option<ImageLoader>,
    compact: bool,
    document: Option<Arc<Document>>,
    /// Open state of each `details`, keyed by [`node_key`]. Cleared on a new parse.
    open: HashMap<usize, bool>,
    /// The theme the highlights in the cache were made for, once known.
    highlight_dark: Option<bool>,
    parse_task: Option<Task<()>>,
    highlight_task: Option<Task<()>>,
    generation: u64,
}

impl MarkdownView {
    /// `compact` is the `.md-compact` scale, for a comment on a diff line.
    pub fn new(
        source: impl Into<SharedString>,
        context: MarkdownContext,
        images: Option<ImageLoader>,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            source: source.into(),
            context,
            images,
            compact,
            document: None,
            open: HashMap::new(),
            highlight_dark: None,
            parse_task: None,
            highlight_task: None,
            generation: 0,
        };
        view.reparse(cx);
        view
    }

    /// Replaces the text (and the context it resolves against). A source that did
    /// not change costs nothing.
    pub fn set_source(
        &mut self,
        source: impl Into<SharedString>,
        context: MarkdownContext,
        cx: &mut Context<Self>,
    ) {
        let source = source.into();
        if source == self.source && context == self.context {
            return;
        }
        self.source = source;
        self.context = context;
        self.reparse(cx);
    }

    /// Replaces the authenticated image loader, and redraws with it.
    #[allow(dead_code)]
    pub fn set_images(&mut self, images: Option<ImageLoader>, cx: &mut Context<Self>) {
        self.images = images;
        cx.notify();
    }

    fn reparse(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let dark = cx.theme().dark;
        let source = self.source.to_string();
        let context = self.context.clone();
        self.open.clear();
        self.parse_task = Some(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let document = cx
                    .background_spawn(async move {
                        let document = parse(&source, &context);
                        highlight_document(&document, dark);
                        document
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.document = Some(Arc::new(document));
                        this.highlight_dark = Some(dark);
                        cx.notify();
                    }
                })
                .ok();
            }),
        );
    }

    /// Highlights the cached tree again after the theme changed. Render calls it, so
    /// it runs once per change and never on a frame that does not need it.
    fn ensure_highlight(&mut self, dark: bool, cx: &mut Context<Self>) {
        if self.highlight_dark == Some(dark) {
            return;
        }
        let Some(document) = self.document.clone() else {
            return;
        };
        self.highlight_dark = Some(dark);
        self.highlight_task = Some(cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_spawn(async move { highlight_document(&document, dark) })
                    .await;
                this.update(cx, |_, cx| cx.notify()).ok();
            },
        ));
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (dark, colors) = {
            let theme = cx.theme();
            (theme.dark, theme.colors)
        };
        self.ensure_highlight(dark, cx);

        let Some(document) = self.document.clone() else {
            return div().into_any_element();
        };
        let m = metrics(self.compact);
        let env = Env {
            m,
            colors,
            dark,
            open: &self.open,
            images: self.images.as_ref(),
        };
        let base = Base {
            color: colors.foreground,
            weight: 400.,
            align: None,
        };

        let body = blocks_element(&document.blocks, &env, base, cx);
        let mut root = div()
            .w_full()
            .min_w_0()
            .font_family(UI_FONT)
            .text_size(rpx(m.size))
            .line_height(rpx(m.line))
            .text_color(colors.foreground)
            .child(body);
        if !document.footnotes.is_empty() {
            root = root.child(footnotes_element(&document.footnotes, &env, base, cx));
        }
        root.into_any_element()
    }
}
