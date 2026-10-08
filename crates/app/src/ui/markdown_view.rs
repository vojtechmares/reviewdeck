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
//! no baseline shift, no CSS borders on text runs and no table layout:
//! - tables are laid out by row with the columns sharing the width by an estimate of
//!   their content (see [`table_element`]), not by the browser's table algorithm;
//! - the footnote section has a rule and no hidden heading or back-reference arrows;
//! - inline code, `sub`, `sup` and `kbd` keep the body size (they are monospace or
//!   plain; `kbd` loses its border and keeps the surface colour);
//! - links are coloured but not underlined on hover, and only http(s) links open;
//!   `mailto:` links look like links and do nothing;
//! - footnote references are the plain number;
//! - text selection is not available: gpui cannot select static text.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, AsyncApp, ClickEvent, Context, CursorStyle, FontStyle, FontWeight, Hsla,
    Image, InteractiveText, ObjectFit, Render, SharedString, StrikethroughStyle, StyledText, Task,
    TextRun, UnderlineStyle, WeakEntity, Window, div, font, img, px, relative, svg,
};
use reviewdeck_core::images::ImageSource;
use reviewdeck_core::markdown::{
    AlertKind, Alignment, Block, Document, Inline, Length, List, MarkdownContext, Table, TableCell,
    TableRow, parse,
};

use crate::ui::code::{self, CodeKind};
use crate::ui::icons::IconName;
use crate::ui::theme::{ActiveTheme, Colors, UI_FONT, mono_font, rpx};

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
    /// Counts the texts built so far in this frame, which names each one for gpui
    /// (see [`text_element`]). The tree is walked in the same order every frame, so
    /// the n-th text is the same text from one frame to the next.
    texts: Cell<usize>,
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
    let mut face = font(if style.mono { mono_font() } else { UI_FONT });
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
            Some(Piece::Text(flow)) => text_element(flow, env),
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
            Piece::Text(flow) => row.child(text_element(flow, env)),
            Piece::Image(image) => row.child(image_element(image, env, cx)),
            Piece::Check(checked) => row.child(checkbox(checked, &env.colors)),
        };
    }
    row.into_any_element()
}

fn text_element(flow: Flow, env: &Env<'_>) -> AnyElement {
    let Flow { text, runs, links } = flow;
    if text.is_empty() {
        return div().into_any_element();
    }
    // gpui keeps the hover and click state of a link under the element's id, and a
    // press and its release arrive in different frames, so the id has to be the same
    // in both. The run list is rebuilt every frame (its address is not stable);
    // the position of the text in the walk is.
    let id_key = env.texts.get();
    env.texts.set(id_key + 1);
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
    // `.md img { max-width: 100%; height: auto }` and, for a comment on a diff line,
    // `.md-compact img { width: auto; max-height: 14rem }`: the stylesheet beats the
    // `width` and `height` attributes of the tag, so the height attribute never
    // counts, and the width attribute only outside the compact scale. Whatever the
    // box ends up as, the picture is drawn inside it at its own proportions, so a
    // clamped dimension never stretches it.
    el = match (image.width, env.m.compact) {
        (Some(Length::Pixels(v)), false) => el.w(rpx(v)),
        // A percentage of the column; the height, a percentage of nothing, follows
        // from the proportions of the picture.
        (Some(Length::Percent(v)), false) => el.w(relative(v / 100.)).h(relative(1.)),
        _ => el,
    };
    if let Some(max) = env.m.max_image_height {
        el = el.max_h(rpx(max));
    }
    el.max_w(relative(1.))
        .object_fit(ObjectFit::Contain)
        .rounded(rpx(6.))
        .with_fallback(move || alt_element(&alt, base))
        .into_any_element()
}

/// Stacks blocks vertically, collapsing the margins between neighbours the way CSS
/// does: the larger of the two wins.
///
/// The margins at the two ends are not applied but handed back as a [`Gap`], because
/// what happens to them depends on the box around the stack. A box with no padding
/// or border above and below (a block quote, a list item, a plain `div`) lets them
/// collapse through into its own margins: [`through`]. A box with either (`details`,
/// a table cell) keeps them inside: [`enclosed`]. The root and an alert drop them,
/// as `.md > *:first-child` and the alert's `[&>:first-child]:mt-0` do.
fn stack(children: Vec<(Gap, AnyElement)>) -> (Gap, AnyElement) {
    let edges = Gap {
        top: children.first().map_or(0., |(gap, _)| gap.top),
        bottom: children.last().map_or(0., |(gap, _)| gap.bottom),
    };
    let mut column = div().flex().flex_col().min_w_0();
    let mut above = 0.;
    for (index, (gap, element)) in children.into_iter().enumerate() {
        let top = if index == 0 { 0. } else { gap.top.max(above) };
        above = gap.bottom;
        column = column.child(div().pt(rpx(top)).min_w_0().child(element));
    }
    (edges, column.into_any_element())
}

/// A box's own margins combined with the ones that collapsed through it from its
/// first and last child.
fn through(own: Gap, inner: Gap) -> Gap {
    Gap {
        top: own.top.max(inner.top),
        bottom: own.bottom.max(inner.bottom),
    }
}

/// A stack's end margins kept inside the box that holds it.
fn enclosed(edges: Gap, element: AnyElement) -> AnyElement {
    div()
        .pt(rpx(edges.top))
        .pb(rpx(edges.bottom))
        .min_w_0()
        .child(element)
        .into_any_element()
}

/// Blocks stacked, with the margins at the two ends handed back (see [`stack`]).
fn blocks_stack(
    blocks: &[Block],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> (Gap, AnyElement) {
    let children = blocks
        .iter()
        .map(|block| block_child(block, env, base, cx, false))
        .collect();
    stack(children)
}

/// Blocks stacked with the end margins dropped.
fn blocks_element(
    blocks: &[Block],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    blocks_stack(blocks, env, base, cx).1
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
            let (edges, body) = blocks_stack(blocks, env, inner, cx);
            let el = div()
                .border_l(px(3.))
                .border_color(colors.border_strong)
                .pl(rpx(m.quote_pad))
                .text_color(colors.muted_foreground)
                .child(body);
            (through(Gap::both(m.gap), edges), el.into_any_element())
        }
        Block::Alert { kind, blocks } => (
            Gap::both(0.7 * m.size),
            alert_element(*kind, blocks, env, base, cx),
        ),
        Block::CodeBlock { lang, text } => {
            (Gap::both(m.gap), code_block(lang.as_deref(), text, env))
        }
        Block::List(list) => {
            let own = if in_item { 0.2 * m.size } else { m.gap };
            let (edges, el) = list_element(list, env, base, cx);
            (through(Gap::both(own), edges), el)
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
            let (edges, body) = blocks_stack(blocks, env, inner, cx);
            (through(Gap::NONE, edges), el.child(body).into_any_element())
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
        .line_height(rpx(12. * env.m.line / env.m.size))
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
    // An empty fence is an empty `pre`: padding and border, no line.
    for (index, line) in text.split('\n').filter(|_| !text.is_empty()).enumerate() {
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
        .font_family(mono_font())
        .text_size(rpx(m.pre_size))
        .line_height(rpx(m.pre_size * m.line / m.size))
        .text_color(env.colors.foreground)
        .child(column);
    el.into_any_element()
}

/// What stands in a list item's marker column.
enum Marker {
    /// `list-style: disc`, drawn as the filled circle a browser draws: the bullet
    /// character of the system font is a speck next to it.
    Disc,
    Number(String),
}

/// The marker column of a list item: the number or bullet, right-aligned against
/// the text in the list's padding. A number wider than the padding (`10.`) grows
/// to the left, out of the box, as a browser's outside marker does.
fn marker_box(marker: Marker, env: &Env<'_>) -> AnyElement {
    let m = &env.m;
    let column = div()
        .flex_none()
        .w(rpx(m.indent))
        .pr(rpx(0.6 * m.size))
        .flex()
        .flex_row()
        .justify_end()
        .whitespace_nowrap();
    match marker {
        Marker::Disc => column
            .h(rpx(m.line))
            .items_center()
            .child(
                div()
                    .flex_none()
                    .size(rpx(0.38 * m.size))
                    .rounded_full()
                    .bg(env.colors.foreground),
            )
            .into_any_element(),
        Marker::Number(text) => column.child(SharedString::from(text)).into_any_element(),
    }
}

fn list_element(
    list: &List,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> (Gap, AnyElement) {
    let m = env.m;
    // A list with any task item drops its bullets and numbers (`contains-task-list`),
    // for the ordinary items among the tasks too.
    let tasks = list.items.iter().any(|item| item.task.is_some());
    let mut children = Vec::with_capacity(list.items.len());
    for (index, item) in list.items.iter().enumerate() {
        let mut column = Vec::with_capacity(item.blocks.len());
        for block in &item.blocks {
            column.push(block_child(block, env, base, cx, true));
        }
        let (inner, content) = stack(column);
        let row = if let Some(checked) = item.task {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(rpx(0.5 * m.size))
                .child(checkbox(checked, &env.colors))
                .child(div().flex_1().min_w_0().child(content))
        } else if tasks {
            div().min_w_0().child(content)
        } else {
            let marker = if list.ordered {
                Marker::Number(format!("{}.", list.start.saturating_add(index as u64)))
            } else {
                Marker::Disc
            };
            div()
                .flex()
                .flex_row()
                .items_start()
                .child(marker_box(marker, env))
                .child(div().flex_1().min_w_0().child(content))
        };
        // `li { margin: 0.2em 0 }`, with the margins of a loose item's paragraphs
        // collapsing through it.
        children.push((
            through(Gap::both(0.2 * m.size), inner),
            row.into_any_element(),
        ));
    }
    let (edges, list_el) = stack(children);
    let padding = if tasks { 0.15 * m.size } else { 0. };
    (
        edges,
        div()
            .pl(rpx(padding))
            .min_w_0()
            .child(list_el)
            .into_any_element(),
    )
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

    // `details[open] > summary { margin-bottom: 0.4em }` collapses with the top
    // margin of the first block under it. The box has padding and a border, so the
    // margins at its ends stay inside.
    let mut parts = vec![(
        Gap {
            top: 0.,
            bottom: if open { 0.4 * env.m.size } else { 0. },
        },
        head.into_any_element(),
    )];
    if open {
        for block in blocks {
            parts.push(block_child(block, env, base, cx, false));
        }
    }
    let (edges, inner) = stack(parts);

    let (pad_y, pad_x) = env.m.details_pad;
    div()
        .px(rpx(pad_x))
        .py(rpx(pad_y))
        .rounded(rpx(10.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface_muted)
        .min_w_0()
        .child(enclosed(edges, inner))
        .into_any_element()
}

fn text_flow(text: &str, style: RunStyle) -> Flow {
    let mut flow = Flow::default();
    flow.push(text, style);
    flow
}

/// What a table cell says as plain text, lines apart, for measuring it.
fn cell_text(blocks: &[Block], out: &mut String) {
    fn inlines(items: &[Inline], out: &mut String) {
        for inline in items {
            match inline {
                Inline::Text(text) | Inline::Code(text) => out.push_str(text),
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Underline(children)
                | Inline::Kbd(children)
                | Inline::Sub(children)
                | Inline::Sup(children)
                | Inline::Link { children, .. } => inlines(children, out),
                Inline::LineBreak => out.push('\n'),
                Inline::Image(image) => out.push_str(&image.alt),
                Inline::Checkbox { .. } => out.push_str("[ ]"),
                Inline::FootnoteRef { number, .. } => out.push_str(&number.to_string()),
            }
        }
    }
    for block in blocks {
        match block {
            Block::Paragraph(items)
            | Block::Plain(items)
            | Block::Heading { inlines: items, .. } => {
                inlines(items, out);
                out.push('\n');
            }
            Block::CodeBlock { text, .. } => {
                out.push_str(text);
                out.push('\n');
            }
            Block::BlockQuote(blocks)
            | Block::Alert { blocks, .. }
            | Block::Details { blocks, .. }
            | Block::Div { blocks, .. } => cell_text(blocks, out),
            Block::List(list) => {
                for item in &list.items {
                    out.push_str("   ");
                    cell_text(&item.blocks, out);
                }
            }
            Block::Table(_) | Block::Rule => {}
        }
    }
}

/// How wide a column wants to be, in characters: the longest line in it (its
/// max-content), and the longest word (its min-content, which is as narrow as it can
/// get without breaking a word).
fn column_chars(rows: &[&TableRow], column: usize) -> (usize, usize) {
    let (mut line, mut word) = (1, 1);
    for cell in rows.iter().filter_map(|row| row.cells.get(column)) {
        let mut text = String::new();
        cell_text(&cell.blocks, &mut text);
        for part in text.lines() {
            line = line.max(part.chars().count());
        }
        for part in text.split_whitespace() {
            word = word.max(part.chars().count());
        }
    }
    (line, word)
}

/// A table, the way `.md table` draws it.
///
/// A browser sizes the columns to their content: as wide as the longest line while
/// the table fits, narrower (wrapping) when it does not. gpui has no table layout, and
/// laying the table out by column leaves the cells of one row different heights as
/// soon as one of them wraps, which breaks the grid. So it is laid out by row, which
/// keeps every row's cells the same height, and the columns share the width in
/// proportion to the longest line in each (a stand-in for max-content, counted in
/// characters). A short table is as wide as its text, and a wide one fills the width
/// and scrolls only when a word cannot wrap any further.
fn table_element(
    table: &Table,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let rows: Vec<&TableRow> = table.head.iter().chain(table.rows.iter()).collect();
    let columns = rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0)
        .max(table.alignments.len());
    if columns == 0 {
        return div().into_any_element();
    }

    let (_, pad_x) = env.m.cell_pad;
    // The advance of a character, in ems: generous, because a column of code or bold
    // text is wider than the average of the UI font, and a column that comes out too
    // wide costs a little white space where one too narrow breaks words.
    const EM_PER_CHAR: f32 = 0.6;
    let wanted: Vec<(f32, f32)> = (0..columns)
        .map(|column| {
            let (line, word) = column_chars(&rows, column);
            let em = EM_PER_CHAR * env.m.table_size;
            (line as f32 * em + 2. * pad_x, word as f32 * em + 2. * pad_x)
        })
        .collect();
    let total: f32 = wanted.iter().map(|(line, _)| line).sum::<f32>() + 1.;

    let mut frame = div()
        .flex()
        .flex_col()
        .w_full()
        .border_t_1()
        .border_l_1()
        .border_color(env.colors.border);
    let head_rows = table.head.len();
    for (row_index, row) in rows.iter().enumerate() {
        let last_row = row_index + 1 == rows.len();
        let mut line = div().flex().flex_row().w_full();
        for (column, &(wants, least)) in wanted.iter().enumerate() {
            let cell = row.cells.get(column);
            let align = cell
                .and_then(|cell| cell.align)
                .or_else(|| table.alignments.get(column).copied().flatten());
            let head = row_index < head_rows || cell.is_some_and(|cell| cell.header);
            let look = CellLook {
                head,
                align,
                last_row,
                last_column: column + 1 == columns,
                least,
            };
            let element = cell_element(cell, look, env, base, cx);
            // Every row gives a column the same share of the width, in proportion
            // to what the column wants, and never less than its longest word.
            let mut slot = div().flex().flex_basis(px(0.)).child(element);
            slot.style().flex_grow = Some(wants);
            line = line.child(slot);
        }
        frame = frame.child(line);
    }

    div()
        .id(("md-table", node_key(table)))
        .overflow_x_scroll()
        .w(rpx(total))
        .max_w(relative(1.))
        .text_size(rpx(env.m.table_size))
        .line_height(rpx(env.m.table_size * env.m.line / env.m.size))
        .child(frame)
        .into_any_element()
}

/// How one table cell is drawn: what the cell is, and where it sits in the grid.
#[derive(Clone, Copy)]
struct CellLook {
    head: bool,
    align: Option<Alignment>,
    last_row: bool,
    last_column: bool,
    /// The narrowest it may get, in CSS pixels.
    least: f32,
}

fn cell_element(
    cell: Option<&TableCell>,
    look: CellLook,
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> AnyElement {
    let CellLook {
        head,
        align,
        last_row,
        last_column,
        least,
    } = look;
    let (pad_y, pad_x) = env.m.cell_pad;
    let inner = Base {
        weight: if head { 600. } else { base.weight },
        align,
        ..base
    };
    let mut el = div()
        .size_full()
        .min_w(rpx(least))
        .px(rpx(pad_x))
        .py(rpx(pad_y))
        .border_color(env.colors.border);
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
        Some(cell) => {
            // A cell has padding, so what sticks out of its content stays inside it.
            let (edges, body) = blocks_stack(&cell.blocks, env, inner, cx);
            el.child(enclosed(edges, body))
        }
        None => el,
    }
    .into_any_element()
}

/// The footnotes under the body: a rule, then the numbered notes.
///
/// Deviation: remark-rehype's footnote section carries a visually hidden "Footnotes"
/// heading and a back-reference arrow on every note. Neither does anything here
/// (the anchors do not resolve, see the sanitiser's id prefix in markdown.ts), so
/// the rule stands in for the heading and the arrows are left out.
fn footnotes_children(
    footnotes: &[reviewdeck_core::markdown::Footnote],
    env: &Env<'_>,
    base: Base,
    cx: &mut Context<MarkdownView>,
) -> Vec<(Gap, AnyElement)> {
    let m = env.m;
    let rule = div().w_full().h(px(1.)).bg(env.colors.border);
    let mut children = vec![(Gap::both(m.rule), rule.into_any_element())];
    for footnote in footnotes {
        let (inner, body) = blocks_stack(&footnote.blocks, env, base, cx);
        let row = div()
            .flex()
            .flex_row()
            .items_start()
            .child(marker_box(
                Marker::Number(format!("{}.", footnote.number)),
                env,
            ))
            .child(div().flex_1().min_w_0().child(body));
        children.push((
            through(Gap::both(0.2 * m.size), inner),
            row.into_any_element(),
        ));
    }
    children
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
            texts: Cell::new(0),
        };
        let base = Base {
            color: colors.foreground,
            weight: 400.,
            align: None,
        };

        // Blocks and footnotes are one stack, so the margins between them collapse
        // like any others and the ends of the whole are flush with the box.
        let mut children: Vec<(Gap, AnyElement)> = document
            .blocks
            .iter()
            .map(|block| block_child(block, &env, base, cx, false))
            .collect();
        if !document.footnotes.is_empty() {
            children.extend(footnotes_children(&document.footnotes, &env, base, cx));
        }
        let (_, body) = stack(children);
        let root = div()
            .w_full()
            .min_w_0()
            .font_family(UI_FONT)
            .text_size(rpx(m.size))
            .line_height(rpx(m.line))
            .text_color(colors.foreground)
            .child(body);
        root.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;
    use gpui::{
        Bounds, Entity, ImageFormat, Modifiers, Pixels, TestAppContext, VisualTestContext, point,
    };

    /// A window holding one markdown view in a column of a known width, so the
    /// height of that column is the height of the rendered document.
    struct Host {
        view: Entity<MarkdownView>,
        width: f32,
    }

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .debug_selector(|| "md".to_string())
                .child(self.view.clone())
        }
    }

    fn host<'a>(
        cx: &'a mut TestAppContext,
        source: &str,
        context: MarkdownContext,
        compact: bool,
        images: Option<ImageLoader>,
        width: f32,
    ) -> (Entity<MarkdownView>, &'a mut VisualTestContext) {
        cx.update(|cx| cx.set_global(Theme::new(false)));
        let source = source.to_string();
        let (host, cx) = cx.add_window_view(|_, cx| {
            let view = cx.new(|cx| MarkdownView::new(source, context, images, compact, cx));
            Host { view, width }
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let view = host.read_with(cx, |host, _| host.view.clone());
        (view, cx)
    }

    fn plain<'a>(
        cx: &'a mut TestAppContext,
        source: &str,
        width: f32,
    ) -> &'a mut VisualTestContext {
        host(cx, source, MarkdownContext::default(), false, None, width).1
    }

    fn height(cx: &mut VisualTestContext) -> f32 {
        let bounds: Bounds<Pixels> = cx.debug_bounds("md").expect("the host is drawn");
        f32::from(bounds.size.height)
    }

    fn near(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.6,
            "height {actual}, expected {expected}"
        );
    }

    #[gpui::test]
    fn paragraphs_are_a_line_each_with_the_paragraph_margin_between(cx: &mut TestAppContext) {
        let cx = plain(cx, "one\n\ntwo", 400.);
        // Two lines of 1.6 * 13px, and 0.6em between; none above or below.
        near(height(cx), 2. * 20.8 + 7.8);
    }

    #[gpui::test]
    fn a_loose_list_keeps_the_paragraph_margin_between_items(cx: &mut TestAppContext) {
        let cx = plain(cx, "- a\n\n- b", 400.);
        // The margins of the paragraphs collapse through the items (0.2em) and win.
        near(height(cx), 2. * 20.8 + 7.8);
    }

    #[gpui::test]
    fn a_tight_list_has_only_the_item_margin_between_items(cx: &mut TestAppContext) {
        let cx = plain(cx, "- a\n- b", 400.);
        near(height(cx), 2. * 20.8 + 2.6);
    }

    #[gpui::test]
    fn a_centred_paragraph_keeps_its_margins(cx: &mut TestAppContext) {
        let cx = plain(cx, "one\n\n<p align=\"center\">two</p>\n\nthree", 400.);
        near(height(cx), 3. * 20.8 + 2. * 7.8);
    }

    #[gpui::test]
    fn the_summary_toggles_its_details(cx: &mut TestAppContext) {
        let source = "<details><summary>Why</summary>\n\nBecause.\n\n</details>";
        let cx = plain(cx, source, 400.);
        let closed = height(cx);
        // Padding of 0.5em above and below the summary line, and the 1px borders.
        near(closed, 20.8 + 2. * 6.5 + 2.);

        cx.simulate_click(point(px(30.), px(14.)), Modifiers::none());
        cx.run_until_parked();
        let open = height(cx);
        // 0.6em between the summary (0.4em) and the paragraph (0.6em) collapses to
        // 0.6em, and the paragraph's own bottom margin stays inside the box.
        near(open, closed + 7.8 + 20.8 + 7.8);

        cx.simulate_click(point(px(30.), px(14.)), Modifiers::none());
        cx.run_until_parked();
        near(height(cx), closed);
    }

    fn images_context() -> MarkdownContext {
        MarkdownContext {
            autolink: None,
            images: Some(reviewdeck_core::images::ImageContext {
                repo_root: Some("https://git.example/acme/api".into()),
                accounts: vec![reviewdeck_core::images::ImageAccount {
                    id: "acct".into(),
                    base_url: "https://git.example/api/v4".into(),
                    web_url: "https://git.example".into(),
                }],
            }),
        }
    }

    fn svg_loader(width: u32, height: u32) -> ImageLoader {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="red"/></svg>"#
        );
        let picture = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
        Rc::new(move |_, _, _| Some(picture.clone()))
    }

    #[gpui::test]
    fn an_image_wider_than_the_column_keeps_its_aspect_ratio(cx: &mut TestAppContext) {
        let (_, cx) = host(
            cx,
            "![shot](https://git.example/uploads/a.png)",
            images_context(),
            false,
            Some(svg_loader(800, 400)),
            300.,
        );
        near(height(cx), 150.);
    }

    #[gpui::test]
    fn an_image_narrower_than_the_column_keeps_its_own_size(cx: &mut TestAppContext) {
        let (_, cx) = host(
            cx,
            "![shot](https://git.example/uploads/a.png)",
            images_context(),
            false,
            Some(svg_loader(100, 40)),
            300.,
        );
        near(height(cx), 40.);
    }

    fn picture_height(
        cx: &mut TestAppContext,
        source: &str,
        compact: bool,
        picture: (u32, u32),
        column: f32,
    ) -> f32 {
        let (_, cx) = host(
            cx,
            source,
            images_context(),
            compact,
            Some(svg_loader(picture.0, picture.1)),
            column,
        );
        height(cx)
    }

    #[gpui::test]
    fn an_image_that_is_blocked_or_still_loading_shows_its_alt_text(cx: &mut TestAppContext) {
        // No repository to resolve a relative source against: blocked.
        let (_, vcx) = host(cx, "![chart](pic.png)", images_context(), false, None, 300.);
        near(height(vcx), 20.8);

        // Waiting for the credentialed fetch: the loader has nothing yet.
        let waiting: ImageLoader = Rc::new(|_, _, _| None);
        let (_, vcx) = host(
            cx,
            "![chart](https://git.example/uploads/a.png)",
            images_context(),
            false,
            Some(waiting),
            300.,
        );
        near(height(vcx), 20.8);
    }

    #[gpui::test]
    fn a_web_link_opens_in_the_browser_and_nothing_else_does(cx: &mut TestAppContext) {
        let source = "[docs](https://example.com/docs) plain text";
        let (_, vcx) = host(cx, source, MarkdownContext::default(), false, None, 400.);
        vcx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
        assert_eq!(
            vcx.opened_url().as_deref(),
            Some("https://example.com/docs")
        );
    }

    #[gpui::test]
    fn clicking_the_text_around_a_link_or_a_mailto_link_opens_nothing(cx: &mut TestAppContext) {
        let source = "[docs](https://example.com/docs) plain text that is long enough to click";
        let (_, vcx) = host(cx, source, MarkdownContext::default(), false, None, 400.);
        vcx.simulate_click(point(px(300.), px(10.)), Modifiers::none());
        assert_eq!(vcx.opened_url(), None);

        let (_, vcx) = host(
            cx,
            "[write](mailto:me@example.com) text",
            MarkdownContext::default(),
            false,
            None,
            400.,
        );
        vcx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
        assert_eq!(vcx.opened_url(), None);
    }

    #[gpui::test]
    fn a_blank_line_in_a_code_block_is_a_line_tall(cx: &mut TestAppContext) {
        let vcx = plain(cx, "```\na\n\nb\n```", 400.);
        // Three lines of 1.6 * 0.9em, the padding of 0.7em, and the 1px border.
        near(height(vcx), 3. * 18.72 + 2. * 9.1 + 2.);
    }

    #[gpui::test]
    fn a_comment_on_a_line_uses_the_compact_scale(cx: &mut TestAppContext) {
        let (_, vcx) = host(
            cx,
            "one\n\ntwo",
            MarkdownContext::default(),
            true,
            None,
            400.,
        );
        // 12px text at a line height of 1.5, and 0.45em between paragraphs.
        near(height(vcx), 2. * 18. + 5.4);
    }

    #[gpui::test]
    fn the_rows_of_a_table_are_as_tall_as_their_cells(cx: &mut TestAppContext) {
        let source = "| a | b |\n|---|---|\n| c | d |";
        let vcx = plain(cx, source, 400.);
        // 0.94em text at 1.6, 0.35em of padding above and below, 1px borders.
        let row = 0.94 * 13. * 1.6 + 2. * 0.35 * 13.;
        near(height(vcx), 1. + (row + 1.) + row);
    }

    #[gpui::test]
    fn fenced_code_is_highlighted_for_the_theme_showing(cx: &mut TestAppContext) {
        let source = "```rust\nfn unique_marker_for_light() {}\n```";
        let (_, vcx) = host(cx, source, MarkdownContext::default(), false, None, 400.);
        let kind = CodeKind::Fence("rust".into());
        let text = "fn unique_marker_for_light() {}";
        assert!(matches!(code::cached(&kind, text, false), Some(Some(_))));
        assert!(code::cached(&kind, text, true).is_none());

        // The theme changes: the same block is coloured again for the other one.
        vcx.update(|window, cx| {
            cx.set_global(Theme::new(true));
            window.refresh();
        });
        vcx.run_until_parked();
        assert!(matches!(code::cached(&kind, text, true), Some(Some(_))));
    }

    #[gpui::test]
    fn a_fence_no_grammar_knows_stays_plain(cx: &mut TestAppContext) {
        let source = "```nosuchlanguage\nplain\n```";
        let (_, _vcx) = host(cx, source, MarkdownContext::default(), false, None, 400.);
        let kind = CodeKind::Fence("nosuchlanguage".into());
        assert!(matches!(code::cached(&kind, "plain", false), Some(None)));
    }

    const SHOT: &str = "![shot](https://git.example/uploads/a.png)";

    #[gpui::test]
    fn a_width_attribute_sets_the_width_and_the_height_follows(cx: &mut TestAppContext) {
        let source = r#"<img src="https://git.example/uploads/a.png" width="200">"#;
        near(picture_height(cx, source, false, (800, 400), 600.), 100.);
        let source = r#"<img src="https://git.example/uploads/a.png" width="50%">"#;
        near(picture_height(cx, source, false, (800, 400), 600.), 150.);
    }

    #[gpui::test]
    fn a_height_attribute_never_counts(cx: &mut TestAppContext) {
        // `height: auto` in the stylesheet beats the tag's attribute.
        let source = r#"<img src="https://git.example/uploads/a.png" height="10">"#;
        near(picture_height(cx, source, false, (100, 40), 600.), 40.);
    }

    #[gpui::test]
    fn a_comment_on_a_line_caps_the_image_at_fourteen_rem_and_ignores_its_width(
        cx: &mut TestAppContext,
    ) {
        near(picture_height(cx, SHOT, true, (800, 400), 1000.), 224.);
        let source = r#"<img src="https://git.example/uploads/a.png" width="20">"#;
        near(picture_height(cx, source, true, (100, 40), 1000.), 40.);
    }
}
