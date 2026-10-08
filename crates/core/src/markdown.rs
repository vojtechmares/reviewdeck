//! The markdown pipeline: a port of src/shared/markdown.ts and the parts of
//! src/renderer/src/components/Markdown.tsx that decide what a body says (alerts,
//! fence languages), with a small HTML subset reader in place of
//! `rehype-raw` + `rehype-sanitize`.
//!
//! Pull request descriptions and comments are written by other people, so this is
//! a trust boundary. The Electron app rendered React elements from a sanitized HTML
//! tree; here no HTML ever exists at all. [`parse`] reads the source with
//! pulldown-cmark (the GitHub Flavored Markdown that `remark-gfm` enables: tables,
//! strikethrough, task lists, footnotes, and bare-URL autolinks, which are done
//! here), reads any raw HTML in it through a conservative tokenizer that only knows
//! the tags worth keeping, and produces a typed [`Document`] the UI walks.
//! Sanitising is by construction: there is no node for a script, an event handler,
//! a form field or an id, so none can come out.
//!
//! # What the pipeline does, in order
//!
//! 1. **Parse.** Markdown events and raw HTML tags build one tree. HTML may open an
//!    element in one place and close it after some markdown, the way `rehype-raw`
//!    lets `<details>` wrap a markdown table: an HTML element stays open until its
//!    end tag, or until the markdown element it was opened inside ends.
//! 2. **Sanitise while reading** (the GitHub-derived schema of `rehype-sanitize`):
//!    - known tags become nodes; every other tag is dropped and its text kept;
//!    - `script`, `style`, `textarea`, `iframe`, `object`, `noscript`, `template`,
//!      `noembed` and `noframes` are dropped with everything inside them;
//!    - comments, doctypes and processing instructions are dropped;
//!    - attributes outside the handful each node needs are dropped;
//!    - a link keeps its target only if it is absolute `http`, `https` or `mailto`
//!      (`javascript:`, `data:`, `vbscript:` and every other scheme are dropped; a
//!      relative target is dropped too, because nothing in the app can follow it -
//!      the TypeScript kept it on an anchor that went nowhere);
//!    - an image keeps its source only if it is `http`, `https` or relative;
//!    - table parts outside a table are dropped (their content kept);
//!    - any `<input>` is a disabled checkbox ([`Inline::Checkbox`]) - a free-text
//!      input would be a credential prompt inside someone else's prose;
//!    - character references are decoded, everywhere;
//!    - nesting is bounded at [`MAX_DEPTH`]: deeper elements are flattened into
//!      their ancestor, so hostile input cannot overflow the stack. All work is
//!      linear in the input.
//! 3. **Autolink** (`remarkAutolink` and `remark-gfm`'s literal autolinks) over
//!    markdown text only - never inside code, a link, or raw HTML text: bare
//!    `https://` / `www.` URLs and email addresses become links, then `@mentions`
//!    and `#123` references become links when the [`AutolinkContext`] gives them a
//!    target, then `:shortcode:` emoji are rendered (with or without a context).
//! 4. **Images** resolve through [`crate::images`]: [`ImageSource::Authenticated`]
//!    for a source on a signed-in account's host, [`ImageSource::Remote`] for any
//!    other http(s) source, [`ImageSource::Blocked`] for everything else.
//! 5. **Shape** the typed tree: GitHub alerts are recognised and their marker line
//!    removed, white space is collapsed the way a browser collapses it, footnotes
//!    are numbered in order of first reference.
//!
//! # The document tree, for the renderer
//!
//! A [`Document`] is a list of [`Block`]s plus the footnotes it references.
//!
//! Blocks stack vertically:
//! - [`Block::Paragraph`]: inline content with paragraph spacing.
//! - [`Block::Plain`]: inline content *without* paragraph spacing - the text of a
//!   tight list item, a table cell, or loose inline content (an `<img>` or a `<kbd>`
//!   written as a line of raw HTML). Render it like a paragraph with no margins.
//! - [`Block::Heading`]: level 1-6.
//! - [`Block::BlockQuote`]: an ordinary quote.
//! - [`Block::Alert`]: a GitHub alert (`> [!NOTE]`), marker already removed;
//!   [`AlertKind::label`] is the title to show above it ("Note", "Tip", ...).
//! - [`Block::CodeBlock`]: preformatted text, `lang` set only when it names
//!   something a highlighter could use. The text keeps the fence's trailing
//!   newline, as the TypeScript does; drop one trailing `\n` before highlighting.
//! - [`Block::List`]: ordered (numbered from `start`) or not; each [`ListItem`]
//!   carries `task: Some(checked)` for a task list item (draw a disabled checkbox).
//! - [`Block::Table`]: `head` rows then body `rows`; each cell knows whether it is a
//!   header cell and its alignment.
//! - [`Block::Rule`]: a horizontal rule.
//! - [`Block::Details`]: a collapsible section; `summary` is the clickable line
//!   (`None`: show "Details", as a browser does), `open` its initial state.
//! - [`Block::Div`]: a plain container (`<div>`, `<section>`, `<dl>`...), with the
//!   horizontal alignment raw HTML gave it, e.g. a centred logo
//!   (`<p align="center">` arrives as a `Div` around its `Paragraph`).
//!
//! Inline content flows and wraps:
//! - [`Inline::Text`]: white space already collapsed (runs of spaces and newlines
//!   are one space, none at the start or end of a block), so render it as is.
//! - [`Inline::Emphasis`], [`Inline::Strong`], [`Inline::Strikethrough`],
//!   [`Inline::Underline`] (`<ins>`), [`Inline::Kbd`] (a keyboard key),
//!   [`Inline::Sub`], [`Inline::Sup`]: styling around their children.
//! - [`Inline::Code`]: a code span, monospace, text verbatim.
//! - [`Inline::Link`]: `href` is `Some` only for an absolute http(s) or mailto URL;
//!   a link without one is styled as a link but does nothing. Only http(s) may be
//!   handed to the system browser.
//! - [`Inline::Image`]: see [`Image`]; when the source is blocked or fails to load,
//!   show the alt text.
//! - [`Inline::LineBreak`]: a hard line break.
//! - [`Inline::Checkbox`]: a disabled checkbox from raw HTML.
//! - [`Inline::FootnoteRef`]: a superscript number pointing at
//!   [`Document::footnotes`].

use std::collections::HashMap;
use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};
use regex::Regex;
use url::Url;

use crate::autolink::{AutolinkSpan, autolink_spans, issue_url, mention_url, render_emoji};
use crate::images::{ImageContext, ImageSource, plain_image_source, rewrite_image_source};
use crate::model::ProviderKind;

/// How deep elements may nest before deeper ones are flattened into their ancestor.
/// Far beyond anything a real description uses, and shallow enough that walking the
/// tree recursively can never exhaust a stack.
pub const MAX_DEPTH: usize = 48;

// ---------------------------------------------------------------------------
// The public tree
// ---------------------------------------------------------------------------

/// A parsed markdown body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// The footnotes the body references, in the order they are numbered (the order
    /// of their first reference). A definition nothing references is left out, and a
    /// reference to a footnote that is not defined stays the text it was.
    pub footnotes: Vec<Footnote>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Footnote {
    /// The label as written in the definition (`[^label]: ...`).
    pub label: String,
    /// 1-based; what every reference to it shows.
    pub number: usize,
    pub blocks: Vec<Block>,
}

/// Block-level content. See the module documentation for how each one renders.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    /// Inline content that is not a paragraph: no paragraph spacing.
    Plain(Vec<Inline>),
    Heading {
        /// 1 to 6.
        level: u8,
        inlines: Vec<Inline>,
    },
    BlockQuote(Vec<Block>),
    /// A GitHub alert, with its marker line already removed.
    Alert {
        kind: AlertKind,
        blocks: Vec<Block>,
    },
    CodeBlock {
        /// The fence's language when it is one a highlighter could use: the
        /// `[\w.+#-]+` start of the info string's first word (`ts`, `c++`, `objective-c`).
        lang: Option<String>,
        /// Verbatim, including the trailing newline a fence leaves.
        text: String,
    },
    List(List),
    Table(Table),
    Rule,
    Details {
        /// Whether the section starts expanded.
        open: bool,
        /// The always-visible line; `None` when the source gave none.
        summary: Option<Vec<Inline>>,
        blocks: Vec<Block>,
    },
    /// A plain container, optionally aligned.
    Div {
        align: Option<Alignment>,
        blocks: Vec<Block>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    /// The first number of an ordered list (1 unless the source says otherwise).
    pub start: u64,
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// `Some(checked)` for a task list item: draw a disabled checkbox before it.
    pub task: Option<bool>,
    /// A tight list item's text is a [`Block::Plain`]; a loose one's a paragraph.
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The column alignments a markdown table declares (empty for an HTML table).
    /// Every cell also carries its own resolved alignment.
    pub alignments: Vec<Option<Alignment>>,
    pub head: Vec<TableRow>,
    pub rows: Vec<TableRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    /// A `th`: bold and, in a head row, the column title.
    pub header: bool,
    pub align: Option<Alignment>,
    /// Usually one [`Block::Plain`]; raw HTML may put any blocks in a cell.
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alignment {
    Left,
    Center,
    Right,
}

/// Inline content. See the module documentation for how each one renders.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    /// `<ins>`: underlined.
    Underline(Vec<Inline>),
    Code(String),
    Link {
        /// An absolute http(s) or mailto URL, or `None` when the target was dropped.
        href: Option<String>,
        title: Option<String>,
        children: Vec<Inline>,
    },
    Image(Image),
    LineBreak,
    Kbd(Vec<Inline>),
    Sub(Vec<Inline>),
    Sup(Vec<Inline>),
    /// A disabled checkbox from raw HTML.
    Checkbox {
        checked: bool,
    },
    /// A reference to [`Document::footnotes`]; show `number` as a superscript.
    FootnoteRef {
        label: String,
        number: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub source: ImageSource,
    /// Shown when the image is blocked or fails to load.
    pub alt: String,
    pub title: Option<String>,
    /// The size raw HTML asked for, when it did.
    pub width: Option<Length>,
    pub height: Option<Length>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    /// CSS pixels (`width="120"` or `width="120px"`).
    Pixels(f32),
    /// A percentage of the available width (`width="50%"`).
    Percent(f32),
}

/// The five kinds of GitHub alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The title shown above the alert's body.
    pub fn label(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::Note => "note",
            AlertKind::Tip => "tip",
            AlertKind::Important => "important",
            AlertKind::Warning => "warning",
            AlertKind::Caution => "caution",
        }
    }
}

/// Which host a body was written on, so `@someone` and `#123` resolve there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutolinkContext {
    pub provider: ProviderKind,
    /// Host web root, which is where profiles live.
    pub web_url: String,
    /// Repository web root, or `None` when it could not be recovered from the item
    /// URL ([`crate::autolink::repository_root`]).
    pub repo_root: Option<String>,
}

/// Everything a body needs to resolve against the pull request it belongs to. Both
/// halves are optional: emoji render and bare URLs link without either.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkdownContext {
    pub autolink: Option<AutolinkContext>,
    pub images: Option<ImageContext>,
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

/// A GitHub Alert is an ordinary block quote whose first line is nothing but a
/// marker: `> [!NOTE]`. The marker has to stand alone on that line, which is what
/// keeps a block quote that merely opens with square brackets from being mistaken
/// for one.
static ALERT_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\[!(note|tip|important|warning|caution)\][ \t]*(\r?\n|$)")
        .expect("valid pattern")
});

/// The alert kind the leading text of a block quote's first paragraph names, or
/// `None` for an ordinary block quote - including a marker that is misspelt or names
/// a kind that does not exist, which renders as the block quote it looks like rather
/// than as a broken callout.
pub fn alert_kind_of(text: &str) -> Option<AlertKind> {
    let captures = ALERT_MARKER.captures(text)?;
    match captures.get(1)?.as_str().to_ascii_lowercase().as_str() {
        "note" => Some(AlertKind::Note),
        "tip" => Some(AlertKind::Tip),
        "important" => Some(AlertKind::Important),
        "warning" => Some(AlertKind::Warning),
        "caution" => Some(AlertKind::Caution),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Parses a markdown body written by someone else into a [`Document`].
///
/// Never fails and never panics: anything it does not understand is text.
pub fn parse(source: &str, context: &MarkdownContext) -> Document {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;

    let mut builder = Builder::new();
    for event in Parser::new_ext(source, options) {
        builder.event(event);
    }
    let (mut root, definitions) = builder.finish();

    let autolink = context.autolink.as_ref();
    autolink_nodes(&mut root, autolink);
    let mut definitions: Vec<Definition> = definitions
        .into_iter()
        .map(|mut definition| {
            autolink_nodes(&mut definition.nodes, autolink);
            definition
        })
        .collect();

    // Footnotes are numbered in order of first reference: the body first, then the
    // footnotes themselves as they are listed (one may reference another).
    let mut numbering = Numbering::new(&definitions);
    numbering.visit(&mut root);
    let mut index = 0;
    while index < numbering.order.len() {
        let definition = numbering.order[index];
        let mut nodes = std::mem::take(&mut definitions[definition].nodes);
        numbering.visit(&mut nodes);
        definitions[definition].nodes = nodes;
        index += 1;
    }

    let converter = Converter {
        images: context.images.as_ref(),
    };
    let blocks = converter.blocks(root);
    let footnotes = numbering
        .order
        .iter()
        .enumerate()
        .map(|(position, &definition)| {
            let definition = &mut definitions[definition];
            Footnote {
                label: definition.label.clone(),
                number: position + 1,
                blocks: converter.blocks(std::mem::take(&mut definition.nodes)),
            }
        })
        .collect();

    Document { blocks, footnotes }
}

// ---------------------------------------------------------------------------
// The working tree
// ---------------------------------------------------------------------------

/// The tree the parse builds before it is shaped into [`Block`]s and [`Inline`]s:
/// close to HTML, so markdown and raw HTML can nest in each other freely.
#[derive(Debug)]
enum Node {
    Text(TextNode),
    El(El),
}

#[derive(Debug)]
struct TextNode {
    value: String,
    /// Markdown text, as opposed to code or raw HTML: the only text the autolink
    /// transform touches.
    prose: bool,
}

#[derive(Debug)]
struct El {
    kind: Kind,
    children: Vec<Node>,
}

#[derive(Debug)]
enum Kind {
    Root,
    Paragraph {
        align: Option<Alignment>,
    },
    Heading {
        level: u8,
        align: Option<Alignment>,
    },
    BlockQuote,
    CodeBlock {
        lang: Option<String>,
    },
    Pre,
    List {
        ordered: bool,
        start: u64,
    },
    Item {
        task: Option<bool>,
    },
    Table {
        alignments: Vec<Option<Alignment>>,
    },
    TableSection {
        head: bool,
    },
    TableRow,
    TableCell {
        header: bool,
        align: Option<Alignment>,
    },
    Rule,
    Details {
        open: bool,
    },
    Summary,
    Div {
        align: Option<Alignment>,
    },
    Emphasis,
    Strong,
    Strike,
    Underline,
    Code {
        lang: Option<String>,
    },
    Link {
        href: Option<String>,
        title: Option<String>,
    },
    Image {
        src: Option<String>,
        alt: String,
        title: Option<String>,
        width: Option<Length>,
        height: Option<Length>,
    },
    LineBreak,
    Kbd,
    Sub,
    Sup,
    Quote,
    Checkbox {
        checked: bool,
    },
    FootnoteRef {
        label: String,
        number: usize,
    },
    FootnoteDefinition {
        label: String,
    },
    /// Swallows everything put inside it (`<script>` and friends).
    Dropped,
}

impl Kind {
    fn is_block(&self) -> bool {
        matches!(
            self,
            Kind::Root
                | Kind::Paragraph { .. }
                | Kind::Heading { .. }
                | Kind::BlockQuote
                | Kind::CodeBlock { .. }
                | Kind::Pre
                | Kind::List { .. }
                | Kind::Item { .. }
                | Kind::Table { .. }
                | Kind::TableSection { .. }
                | Kind::TableRow
                | Kind::TableCell { .. }
                | Kind::Rule
                | Kind::Details { .. }
                | Kind::Summary
                | Kind::Div { .. }
                | Kind::FootnoteDefinition { .. }
        )
    }
}

fn el(kind: Kind) -> El {
    El {
        kind,
        children: Vec::new(),
    }
}

/// A footnote definition, as collected while parsing.
struct Definition {
    key: String,
    label: String,
    nodes: Vec<Node>,
}

/// Footnote labels match the way link labels do: case-insensitively, with runs of
/// white space as one.
fn footnote_key(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// Building: markdown events and raw HTML into one tree
// ---------------------------------------------------------------------------

struct Entry {
    el: El,
    /// The tag name when raw HTML opened this element; `None` for markdown.
    html: Option<&'static str>,
}

struct Builder {
    /// Never empty: the root sits at the bottom and is never popped.
    stack: Vec<Entry>,
    /// Markdown elements that were not pushed because the tree was already
    /// [`MAX_DEPTH`] deep; their end events are skipped in turn.
    overflow: usize,
    /// How many [`Kind::Dropped`] elements are open; anything added meanwhile is
    /// discarded.
    dropped: usize,
    /// Whether the next text may join the previous text node. Adjacent text events
    /// are one run of text, as they are one text node in the markdown syntax tree;
    /// anything in between (a tag, a comment) separates them.
    can_merge: bool,
    /// The raw HTML of the HTML block being read, tokenized when it ends.
    html_block: Option<String>,
    /// The column of the next markdown table cell in the current row.
    column: usize,
    definitions: Vec<Definition>,
    entities: Entities,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            stack: vec![Entry {
                el: el(Kind::Root),
                html: None,
            }],
            overflow: 0,
            dropped: 0,
            can_merge: false,
            html_block: None,
            column: 0,
            definitions: Vec::new(),
            entities: Entities::default(),
        }
    }

    fn finish(mut self) -> (Vec<Node>, Vec<Definition>) {
        if let Some(html) = self.html_block.take() {
            self.feed_html(&html);
        }
        while self.stack.len() > 1 {
            self.pop();
        }
        let root = self
            .stack
            .pop()
            .map(|entry| entry.el.children)
            .unwrap_or_default();
        (root, self.definitions)
    }

    fn top(&mut self) -> &mut El {
        let last = self.stack.len() - 1;
        &mut self.stack[last].el
    }

    fn append(&mut self, node: Node) {
        self.can_merge = false;
        if self.dropped > 0 {
            return;
        }
        self.top().children.push(node);
    }

    fn text(&mut self, value: &str, prose: bool) {
        if self.dropped > 0 || value.is_empty() {
            return;
        }
        let can_merge = self.can_merge;
        self.can_merge = true;
        let children = &mut self.top().children;
        if can_merge
            && let Some(Node::Text(last)) = children.last_mut()
            && last.prose == prose
        {
            last.value.push_str(value);
            return;
        }
        children.push(Node::Text(TextNode {
            value: value.to_string(),
            prose,
        }));
    }

    /// Opens an element; `false` when the tree is already as deep as it may go.
    fn push(&mut self, kind: Kind, html: Option<&'static str>) -> bool {
        self.can_merge = false;
        if self.stack.len() >= MAX_DEPTH {
            return false;
        }
        if matches!(kind, Kind::Dropped) {
            self.dropped += 1;
        }
        self.stack.push(Entry { el: el(kind), html });
        true
    }

    /// Closes the innermost element and hands it to its parent.
    fn pop(&mut self) {
        self.can_merge = false;
        if self.stack.len() <= 1 {
            return;
        }
        let Some(Entry { mut el, .. }) = self.stack.pop() else {
            return;
        };
        if matches!(el.kind, Kind::Dropped) {
            self.dropped = self.dropped.saturating_sub(1);
            return;
        }
        if self.dropped > 0 {
            return;
        }
        match &mut el.kind {
            Kind::FootnoteDefinition { label } => {
                // The first definition of a label wins.
                let key = footnote_key(label);
                if !self.definitions.iter().any(|d| d.key == key) {
                    self.definitions.push(Definition {
                        key,
                        label: std::mem::take(label),
                        nodes: el.children,
                    });
                }
                return;
            }
            Kind::Image { alt, .. } => {
                // A markdown image's description is its alt text, flattened.
                *alt = text_of(&el.children);
                el.children.clear();
            }
            _ => {}
        }
        self.top().children.push(Node::El(el));
    }

    fn md_start(&mut self, kind: Kind) {
        if closes_paragraph(&kind) {
            self.close_html_paragraph();
        }
        if !self.push(kind, None) {
            self.overflow += 1;
        }
    }

    fn md_end(&mut self) {
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        // HTML elements still open inside a markdown element end with it.
        while self.stack.len() > 1 {
            let markdown = self.stack.last().is_some_and(|entry| entry.html.is_none());
            self.pop();
            if markdown {
                break;
            }
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(Tag::HtmlBlock) => {
                self.html_block = Some(String::new());
            }
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(html) = self.html_block.take() {
                    self.feed_html(&html);
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => match self.html_block.as_mut() {
                Some(block) => block.push_str(&html),
                None => self.feed_html(&html),
            },
            Event::Start(tag) => self.start(tag),
            Event::End(_) => self.md_end(),
            Event::Text(text) => self.text(&text, true),
            Event::Code(code) => {
                let mut node = el(Kind::Code { lang: None });
                node.children.push(Node::Text(TextNode {
                    value: code.replace('\n', " "),
                    prose: false,
                }));
                self.append(Node::El(node));
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => self.text(&text, false),
            Event::FootnoteReference(label) => self.append(Node::El(el(Kind::FootnoteRef {
                label: label.to_string(),
                number: 0,
            }))),
            // A line ending inside a paragraph is part of its text, as it is in the
            // markdown syntax tree; white space is collapsed later.
            Event::SoftBreak => self.text("\n", true),
            Event::HardBreak => self.append(Node::El(el(Kind::LineBreak))),
            Event::Rule => {
                self.close_html_paragraph();
                self.append(Node::El(el(Kind::Rule)));
            }
            Event::TaskListMarker(checked) => {
                if let Some(Entry {
                    el:
                        El {
                            kind: Kind::Item { task },
                            ..
                        },
                    ..
                }) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|entry| matches!(entry.el.kind, Kind::Item { .. }))
                {
                    *task = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let kind = match tag {
            Tag::Paragraph => Kind::Paragraph { align: None },
            Tag::Heading { level, .. } => Kind::Heading {
                level: level as u8,
                align: None,
            },
            Tag::BlockQuote(_) => Kind::BlockQuote,
            Tag::CodeBlock(kind) => Kind::CodeBlock {
                lang: match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().and_then(fence_language)
                    }
                    CodeBlockKind::Indented => None,
                },
            },
            // Handled in `event`; never reaches here.
            Tag::HtmlBlock => return,
            Tag::List(start) => Kind::List {
                ordered: start.is_some(),
                start: start.unwrap_or(1),
            },
            Tag::Item => Kind::Item { task: None },
            Tag::FootnoteDefinition(label) => Kind::FootnoteDefinition {
                label: label.to_string(),
            },
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {
                Kind::Div { align: None }
            }
            Tag::Table(alignments) => Kind::Table {
                alignments: alignments
                    .into_iter()
                    .map(|alignment| match alignment {
                        pulldown_cmark::Alignment::None => None,
                        pulldown_cmark::Alignment::Left => Some(Alignment::Left),
                        pulldown_cmark::Alignment::Center => Some(Alignment::Center),
                        pulldown_cmark::Alignment::Right => Some(Alignment::Right),
                    })
                    .collect(),
            },
            Tag::TableHead => {
                self.column = 0;
                Kind::TableSection { head: true }
            }
            Tag::TableRow => {
                self.column = 0;
                Kind::TableRow
            }
            Tag::TableCell => self.markdown_cell(),
            Tag::Emphasis => Kind::Emphasis,
            Tag::Strong => Kind::Strong,
            Tag::Strikethrough => Kind::Strike,
            Tag::Superscript => Kind::Sup,
            Tag::Subscript => Kind::Sub,
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                let href = if link_type == LinkType::Email {
                    sanitize_href(&format!("mailto:{dest_url}"))
                } else {
                    sanitize_href(&dest_url)
                };
                Kind::Link {
                    href,
                    title: non_empty(&title),
                }
            }
            Tag::Image {
                dest_url, title, ..
            } => Kind::Image {
                src: sanitize_src(&dest_url),
                alt: String::new(),
                title: non_empty(&title),
                width: None,
                height: None,
            },
            Tag::MetadataBlock(_) => Kind::Dropped,
        };
        self.md_start(kind);
    }

    /// A markdown table cell takes its column's alignment, and is a header cell in
    /// the head row.
    fn markdown_cell(&mut self) -> Kind {
        let column = self.column;
        self.column += 1;
        let header = self
            .stack
            .last()
            .is_some_and(|parent| matches!(parent.el.kind, Kind::TableSection { head: true }));
        let align = self
            .stack
            .iter()
            .rev()
            .find_map(|entry| match &entry.el.kind {
                Kind::Table { alignments } => Some(alignments.get(column).copied().flatten()),
                _ => None,
            });
        Kind::TableCell {
            header,
            align: align.flatten(),
        }
    }

    // --- raw HTML ---

    fn feed_html(&mut self, html: &str) {
        self.can_merge = false;
        for token in tokenize(html, &mut self.entities) {
            match token {
                Token::Text(text) => self.text(&text, false),
                Token::Start { name, attrs } => self.html_start(&name, &attrs),
                Token::End { name } => self.html_end(&name),
            }
        }
        self.can_merge = false;
    }

    /// The index of the innermost open HTML element `matches` accepts, looking no
    /// further than the innermost markdown element (HTML cannot close what markdown
    /// opened) or an element named in `stop`.
    fn find_open(&self, matches: impl Fn(&str) -> bool, stop: &[&str]) -> Option<usize> {
        for index in (1..self.stack.len()).rev() {
            let name = self.stack[index].html?;
            if matches(name) {
                return Some(index);
            }
            if stop.contains(&name) {
                return None;
            }
        }
        None
    }

    fn close_to(&mut self, index: usize) {
        while self.stack.len() > index.max(1) {
            self.pop();
        }
    }

    fn close_open(&mut self, names: &[&str], stop: &[&str]) {
        if let Some(index) = self.find_open(|name| names.contains(&name), stop) {
            self.close_to(index);
        }
    }

    /// A block-level start tag closes an open `<p>`, as an HTML parser does.
    fn close_html_paragraph(&mut self) {
        self.close_open(&["p"], &["table", "td", "th", "button", "object"]);
    }

    fn in_table(&self) -> bool {
        self.stack
            .iter()
            .any(|entry| matches!(entry.el.kind, Kind::Table { .. }))
    }

    fn html_start(&mut self, name: &str, attrs: &[(String, String)]) {
        let attr = |key: &str| {
            attrs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let align = || attr("align").and_then(parse_alignment);

        // Void elements: nothing to close later.
        match name {
            "br" => return self.append(Node::El(el(Kind::LineBreak))),
            "hr" => {
                self.close_html_paragraph();
                return self.append(Node::El(el(Kind::Rule)));
            }
            "img" => {
                return self.append(Node::El(el(Kind::Image {
                    src: attr("src").and_then(sanitize_src),
                    alt: attr("alt").unwrap_or_default().to_string(),
                    title: attr("title").and_then(non_empty),
                    width: attr("width").and_then(parse_length),
                    height: attr("height").and_then(parse_length),
                })));
            }
            // Whatever an input claimed to be, it survives only as a disabled checkbox.
            "input" => {
                return self.append(Node::El(el(Kind::Checkbox {
                    checked: attr("checked").is_some(),
                })));
            }
            _ if is_void(name) => return,
            _ => {}
        }

        let (static_name, kind): (&'static str, Kind) = match name {
            "script" => ("script", Kind::Dropped),
            "style" => ("style", Kind::Dropped),
            "textarea" => ("textarea", Kind::Dropped),
            "iframe" => ("iframe", Kind::Dropped),
            "object" => ("object", Kind::Dropped),
            "noscript" => ("noscript", Kind::Dropped),
            "template" => ("template", Kind::Dropped),
            "noembed" => ("noembed", Kind::Dropped),
            "noframes" => ("noframes", Kind::Dropped),
            "p" => ("p", Kind::Paragraph { align: align() }),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name.as_bytes().get(1).map_or(1, |digit| digit - b'0');
                let static_name =
                    ["h1", "h2", "h3", "h4", "h5", "h6"][usize::from(level.clamp(1, 6) - 1)];
                (
                    static_name,
                    Kind::Heading {
                        level,
                        align: align(),
                    },
                )
            }
            "div" => ("div", Kind::Div { align: align() }),
            "section" => ("section", Kind::Div { align: align() }),
            "dl" => ("dl", Kind::Div { align: align() }),
            "dt" => ("dt", Kind::Div { align: align() }),
            "dd" => ("dd", Kind::Div { align: align() }),
            "blockquote" => ("blockquote", Kind::BlockQuote),
            "pre" => ("pre", Kind::Pre),
            "details" => (
                "details",
                Kind::Details {
                    open: attr("open").is_some(),
                },
            ),
            "summary" => ("summary", Kind::Summary),
            "ul" => (
                "ul",
                Kind::List {
                    ordered: false,
                    start: 1,
                },
            ),
            "ol" => (
                "ol",
                Kind::List {
                    ordered: true,
                    start: attr("start")
                        .and_then(|start| start.trim().parse().ok())
                        .unwrap_or(1),
                },
            ),
            "li" => ("li", Kind::Item { task: None }),
            "table" => (
                "table",
                Kind::Table {
                    alignments: Vec::new(),
                },
            ),
            // Table parts only mean something inside a table.
            "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" if !self.in_table() => return,
            "thead" => ("thead", Kind::TableSection { head: true }),
            "tbody" => ("tbody", Kind::TableSection { head: false }),
            "tfoot" => ("tfoot", Kind::TableSection { head: false }),
            "tr" => ("tr", Kind::TableRow),
            "td" => (
                "td",
                Kind::TableCell {
                    header: false,
                    align: align(),
                },
            ),
            "th" => (
                "th",
                Kind::TableCell {
                    header: true,
                    align: align(),
                },
            ),
            "b" => ("b", Kind::Strong),
            "strong" => ("strong", Kind::Strong),
            "i" => ("i", Kind::Emphasis),
            "em" => ("em", Kind::Emphasis),
            "var" => ("var", Kind::Emphasis),
            "s" => ("s", Kind::Strike),
            "del" => ("del", Kind::Strike),
            "strike" => ("strike", Kind::Strike),
            "ins" => ("ins", Kind::Underline),
            "code" => (
                "code",
                Kind::Code {
                    lang: attr("class").and_then(class_language),
                },
            ),
            "tt" => ("tt", Kind::Code { lang: None }),
            "samp" => ("samp", Kind::Code { lang: None }),
            "kbd" => ("kbd", Kind::Kbd),
            "sub" => ("sub", Kind::Sub),
            "sup" => ("sup", Kind::Sup),
            "q" => ("q", Kind::Quote),
            "a" => (
                "a",
                Kind::Link {
                    href: attr("href").and_then(sanitize_href),
                    title: attr("title").and_then(non_empty),
                },
            ),
            // Everything else - span, picture, ruby, font, form, button, svg, a custom
            // element - is dropped, and its content stays where it was.
            _ => return,
        };

        // The end tags an HTML parser would imply.
        match static_name {
            "li" => self.close_open(&["li"], &["ul", "ol"]),
            "dt" | "dd" => self.close_open(&["dt", "dd"], &["dl"]),
            "td" | "th" => self.close_open(&["td", "th"], &["tr", "table"]),
            "tr" => self.close_open(&["tr"], &["table", "thead", "tbody", "tfoot"]),
            "thead" | "tbody" | "tfoot" => {
                self.close_open(&["thead", "tbody", "tfoot"], &["table"]);
            }
            "a" => self.close_open(&["a"], &[]),
            _ => {}
        }
        if closes_paragraph(&kind) {
            self.close_html_paragraph();
        }
        if matches!(kind, Kind::Heading { .. })
            && self
                .stack
                .last()
                .is_some_and(|entry| entry.html.is_some_and(is_heading))
        {
            self.pop();
        }
        // Too deep: the tag is ignored and its content joins the ancestor.
        self.push(kind, Some(static_name));
    }

    fn html_end(&mut self, name: &str) {
        if name == "br" {
            // `</br>` is read as `<br>`.
            return self.append(Node::El(el(Kind::LineBreak)));
        }
        if let Some(index) = self.find_open(|open| open == name, &[]) {
            self.close_to(index);
        }
        self.can_merge = false;
    }
}

fn is_heading(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "basefont"
            | "bgsound"
            | "col"
            | "embed"
            | "frame"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Whether opening this element closes an open HTML `<p>`.
fn closes_paragraph(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Paragraph { .. }
            | Kind::Heading { .. }
            | Kind::BlockQuote
            | Kind::CodeBlock { .. }
            | Kind::Pre
            | Kind::List { .. }
            | Kind::Table { .. }
            | Kind::Rule
            | Kind::Details { .. }
            | Kind::Summary
            | Kind::Div { .. }
    )
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_alignment(value: &str) -> Option<Alignment> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Alignment::Left),
        "center" | "centre" | "middle" => Some(Alignment::Center),
        "right" => Some(Alignment::Right),
        _ => None,
    }
}

fn parse_length(value: &str) -> Option<Length> {
    let value = value.trim();
    let (number, percent) = match value.strip_suffix('%') {
        Some(number) => (number, true),
        None => (value.strip_suffix("px").unwrap_or(value), false),
    };
    let number: f32 = number.trim().parse().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    Some(if percent {
        Length::Percent(number)
    } else {
        Length::Pixels(number)
    })
}

/// `(?:^|\s)language-([\w.+#-]+)`: the language a fence or a `language-*` class
/// names, when it names one a highlighter could use.
fn fence_language(word: &str) -> Option<String> {
    let end = word
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '#' | '-')))
        .unwrap_or(word.len());
    (end > 0).then(|| word[..end].to_string())
}

fn class_language(class: &str) -> Option<String> {
    class
        .split_ascii_whitespace()
        .filter_map(|token| token.strip_prefix("language-"))
        .find_map(fence_language)
}

// ---------------------------------------------------------------------------
// URL safety (the protocol rules of the sanitize schema)
// ---------------------------------------------------------------------------

enum UrlKind {
    /// No scheme: resolved against wherever the document lives.
    Relative,
    /// A scheme, lower-cased.
    Scheme(String),
}

/// How the sanitizer reads a URL: a colon that comes after a `/`, `?` or `#` is not
/// a scheme separator, so the URL is relative.
fn url_kind(value: &str) -> UrlKind {
    let Some(colon) = value.find(':') else {
        return UrlKind::Relative;
    };
    let before = |c: char| value.find(c).is_some_and(|at| at < colon);
    if before('/') || before('?') || before('#') {
        return UrlKind::Relative;
    }
    UrlKind::Scheme(value[..colon].to_ascii_lowercase())
}

/// A link target the app can follow: absolute http(s), normalised, or mailto.
fn sanitize_href(value: &str) -> Option<String> {
    match url_kind(value) {
        UrlKind::Relative => None,
        UrlKind::Scheme(scheme) => match scheme.as_str() {
            "http" | "https" => Url::parse(value)
                .ok()
                .filter(|url| url.has_host())
                .map(String::from),
            "mailto" => Some(value.to_string()),
            _ => None,
        },
    }
}

/// An image source the sanitizer lets through: http(s) or relative.
fn sanitize_src(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    match url_kind(value) {
        UrlKind::Relative => Some(value.to_string()),
        UrlKind::Scheme(scheme) if scheme == "http" || scheme == "https" => Some(value.to_string()),
        UrlKind::Scheme(_) => None,
    }
}

// ---------------------------------------------------------------------------
// The HTML subset tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Token {
    Start {
        /// Lower-cased.
        name: String,
        /// Names lower-cased, values decoded; the first of a repeated name wins.
        attrs: Vec<(String, String)>,
    },
    End {
        name: String,
    },
    Text(String),
}

/// Elements whose content is text up to their own end tag, not markup.
fn is_raw_text(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "textarea"
            | "title"
            | "xmp"
            | "iframe"
            | "noembed"
            | "noframes"
            | "noscript"
    )
}

fn is_html_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C')
}

/// Splits raw HTML into tags and text, the way an HTML tokenizer would for the
/// constructs that matter here. One pass, no backtracking: linear in the input.
fn tokenize(input: &str, entities: &mut Entities) -> Vec<Token> {
    let bytes = input.as_bytes();
    let lower = input.to_ascii_lowercase();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    // Where the pending run of literal text starts.
    let mut run = 0;

    let flush = |text: &mut String, tokens: &mut Vec<Token>| {
        if !text.is_empty() {
            tokens.push(Token::Text(std::mem::take(text)));
        }
    };

    while i < len {
        let Some(offset) = input[i..].find('<') else {
            break;
        };
        let lt = i + offset;
        let next = bytes.get(lt + 1).copied();

        if input[lt..].starts_with("<!--") {
            entities.decode(&input[run..lt], &mut text);
            // `<!-->` and `<!--->` are complete (empty) comments.
            let body = lt + 4;
            let end = if input[body..].starts_with('>') {
                body + 1
            } else if input[body..].starts_with("->") {
                body + 2
            } else {
                input[body..].find("-->").map_or(len, |at| body + at + 3)
            };
            i = end;
            run = end;
            continue;
        } else if matches!(next, Some(b'!') | Some(b'?')) {
            // Doctype, CDATA, processing instruction: a bogus comment up to `>`.
            entities.decode(&input[run..lt], &mut text);
            let end = input[lt..].find('>').map_or(len, |at| lt + at + 1);
            i = end;
            run = end;
            continue;
        } else if next == Some(b'/') {
            match bytes.get(lt + 2) {
                Some(c) if c.is_ascii_alphabetic() => {
                    entities.decode(&input[run..lt], &mut text);
                    let name_start = lt + 2;
                    let name_end = scan_name(bytes, name_start);
                    let name = lower[name_start..name_end].to_string();
                    // An end tag cut off by the end of the input is dropped.
                    let end = match input[name_end..].find('>') {
                        Some(at) => {
                            flush(&mut text, &mut tokens);
                            tokens.push(Token::End { name });
                            name_end + at + 1
                        }
                        None => len,
                    };
                    i = end;
                    run = end;
                    continue;
                }
                Some(b'>') => {
                    // `</>` is nothing at all.
                    entities.decode(&input[run..lt], &mut text);
                    i = lt + 3;
                    run = i;
                    continue;
                }
                Some(_) => {
                    entities.decode(&input[run..lt], &mut text);
                    let end = input[lt..].find('>').map_or(len, |at| lt + at + 1);
                    i = end;
                    run = end;
                    continue;
                }
                None => {
                    i = lt + 1;
                    continue;
                }
            }
        } else if next.is_some_and(|c| c.is_ascii_alphabetic()) {
            let name_start = lt + 1;
            let name_end = scan_name(bytes, name_start);
            let name = lower[name_start..name_end].to_string();
            match scan_attributes(input, name_end, entities) {
                Some((attrs, end)) => {
                    entities.decode(&input[run..lt], &mut text);
                    flush(&mut text, &mut tokens);
                    let raw = is_raw_text(&name);
                    tokens.push(Token::Start {
                        name: name.clone(),
                        attrs,
                    });
                    i = end;
                    run = end;
                    if raw {
                        let (content_end, after) = raw_text_end(&lower, end, &name);
                        let content = &input[end..content_end];
                        if !content.is_empty() {
                            if matches!(name.as_str(), "textarea" | "title") {
                                let mut decoded = String::new();
                                entities.decode(content, &mut decoded);
                                tokens.push(Token::Text(decoded));
                            } else {
                                tokens.push(Token::Text(content.to_string()));
                            }
                        }
                        if content_end < len {
                            tokens.push(Token::End { name });
                        }
                        i = after;
                        run = after;
                    }
                }
                None => {
                    // A tag cut off by the end of the input is dropped, with
                    // everything after it.
                    entities.decode(&input[run..lt], &mut text);
                    i = len;
                    run = len;
                }
            }
            continue;
        } else {
            // A `<` that starts nothing is text.
            i = lt + 1;
            continue;
        }
    }
    if run < len {
        entities.decode(&input[run..], &mut text);
    }
    flush(&mut text, &mut tokens);
    tokens
}

/// The end of a tag name: the first space, `/` or `>`.
fn scan_name(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len()
        && !is_html_space(bytes[end])
        && bytes[end] != b'/'
        && bytes[end] != b'>'
    {
        end += 1;
    }
    end
}

/// Reads a start tag's attributes from `start` up to and including its `>`.
/// `None` when the input ends first.
fn scan_attributes(
    input: &str,
    start: usize,
    entities: &mut Entities,
) -> Option<(Vec<(String, String)>, usize)> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut i = start;
    loop {
        while i < len && is_html_space(bytes[i]) {
            i += 1;
        }
        let current = *bytes.get(i)?;
        if current == b'>' {
            return Some((attrs, i + 1));
        }
        if current == b'/' {
            i += 1;
            continue;
        }

        let name_start = i;
        i += 1;
        while i < len && !is_html_space(bytes[i]) && !matches!(bytes[i], b'/' | b'>' | b'=') {
            i += 1;
        }
        let name = input.get(name_start..i)?.to_ascii_lowercase();
        while i < len && is_html_space(bytes[i]) {
            i += 1;
        }

        let mut value = String::new();
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while i < len && is_html_space(bytes[i]) {
                i += 1;
            }
            match *bytes.get(i)? {
                quote @ (b'"' | b'\'') => {
                    let close = input[i + 1..].find(char::from(quote))? + i + 1;
                    entities.decode(&input[i + 1..close], &mut value);
                    i = close + 1;
                }
                b'>' => {}
                _ => {
                    let value_start = i;
                    while i < len && !is_html_space(bytes[i]) && bytes[i] != b'>' {
                        i += 1;
                    }
                    entities.decode(&input[value_start..i], &mut value);
                }
            }
        }
        if !attrs.iter().any(|(existing, _)| *existing == name) {
            attrs.push((name, value));
        }
    }
}

/// Where the content of a raw text element ends (`</name` followed by a space, `/`
/// or `>`), and where its end tag ends. Both are the end of the input when it never
/// closes.
fn raw_text_end(lower: &str, from: usize, name: &str) -> (usize, usize) {
    let needle = format!("</{name}");
    let bytes = lower.as_bytes();
    let mut search = from;
    while let Some(offset) = lower[search..].find(&needle) {
        let at = search + offset;
        let after_name = at + needle.len();
        match bytes.get(after_name) {
            None => return (at, lower.len()),
            Some(&c) if is_html_space(c) || c == b'/' || c == b'>' => {
                let end = lower[after_name..]
                    .find('>')
                    .map_or(lower.len(), |gt| after_name + gt + 1);
                return (at, end);
            }
            Some(_) => search = after_name,
        }
    }
    (lower.len(), lower.len())
}

/// Character reference decoding for raw HTML. Markdown text arrives decoded from
/// pulldown-cmark already.
#[derive(Default)]
struct Entities {
    /// Named references looked up so far.
    cache: HashMap<String, Option<String>>,
}

impl Entities {
    /// Appends `input` to `out` with `&name;`, `&#123;` and `&#x7b;` decoded. An
    /// unknown name stays as it was written.
    fn decode(&mut self, input: &str, out: &mut String) {
        let mut rest = input;
        while let Some(at) = rest.find('&') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            match self.reference(after) {
                Some((decoded, used)) => {
                    out.push_str(&decoded);
                    rest = &after[used..];
                }
                None => {
                    out.push('&');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
    }

    /// The reference at the start of `after` (the text following a `&`): what it
    /// decodes to, and how many bytes of `after` it used.
    fn reference(&mut self, after: &str) -> Option<(String, usize)> {
        let bytes = after.as_bytes();
        if bytes.first() == Some(&b'#') {
            let (hex, digits_start) = match bytes.get(1) {
                Some(b'x' | b'X') => (true, 2),
                _ => (false, 1),
            };
            let mut end = digits_start;
            while end < bytes.len()
                && end - digits_start < 8
                && (if hex {
                    bytes[end].is_ascii_hexdigit()
                } else {
                    bytes[end].is_ascii_digit()
                })
            {
                end += 1;
            }
            if end == digits_start {
                return None;
            }
            let digits = &after[digits_start..end];
            let code = u32::from_str_radix(digits, if hex { 16 } else { 10 }).ok()?;
            let decoded = match code {
                0 => '\u{FFFD}',
                code => char::from_u32(code).unwrap_or('\u{FFFD}'),
            };
            let used = if bytes.get(end) == Some(&b';') {
                end + 1
            } else {
                end
            };
            return Some((decoded.to_string(), used));
        }

        let end = bytes
            .iter()
            .take(32)
            .position(|b| !b.is_ascii_alphanumeric())
            .unwrap_or(bytes.len().min(32));
        if end == 0 || !bytes[0].is_ascii_alphabetic() || bytes.get(end) != Some(&b';') {
            return None;
        }
        let name = &after[..end];
        let decoded = self.named(name)?;
        Some((decoded, end + 1))
    }

    /// Looks a named reference up in the HTML5 table CommonMark uses, by asking
    /// pulldown-cmark to read `&name;` - it decodes exactly the references a browser
    /// does, and leaves anything else as written.
    fn named(&mut self, name: &str) -> Option<String> {
        if let Some(cached) = self.cache.get(name) {
            return cached.clone();
        }
        let source = format!("&{name};");
        let mut decoded = String::new();
        for event in Parser::new(&source) {
            if let Event::Text(text) = event {
                decoded.push_str(&text);
            }
        }
        let result = (!decoded.is_empty() && decoded != source).then_some(decoded);
        self.cache.insert(name.to_string(), result.clone());
        result
    }
}

// ---------------------------------------------------------------------------
// Autolinking: literal URLs and emails, then mentions, references and emoji
// ---------------------------------------------------------------------------

/// Elements whose text is not prose: their contents must survive untouched.
fn is_opaque(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Code { .. }
            | Kind::CodeBlock { .. }
            | Kind::Pre
            | Kind::Link { .. }
            | Kind::Image { .. }
    )
}

/// Turns literal URLs and email addresses, `@someone`, `#123` and `:tada:` into what
/// they mean.
///
/// A transform over the tree, not over the source: only markdown text is visited,
/// so a reference inside a code span is left as the literal text it is, and a
/// mention inside an already-linked URL is not linked twice.
fn autolink_nodes(nodes: &mut Vec<Node>, context: Option<&AutolinkContext>) {
    let old = std::mem::take(nodes);
    for node in old {
        match node {
            Node::Text(text) if text.prose => nodes.extend(split_prose(&text.value, context)),
            Node::El(mut element) => {
                if !is_opaque(&element.kind) {
                    autolink_nodes(&mut element.children, context);
                }
                nodes.push(Node::El(element));
            }
            other => nodes.push(other),
        }
    }
}

fn plain(value: String) -> Node {
    Node::Text(TextNode {
        value,
        prose: false,
    })
}

fn link_node(url: &str, text: &str) -> Node {
    let mut link = el(Kind::Link {
        href: sanitize_href(url),
        title: None,
    });
    link.children.push(plain(text.to_string()));
    Node::El(link)
}

/// One text node in, that node's text and links out.
fn split_prose(value: &str, context: Option<&AutolinkContext>) -> Vec<Node> {
    let mut out = Vec::new();
    for segment in literal_autolinks(value) {
        match segment {
            Segment::Link { url, text } => out.push(link_node(&url, &text)),
            Segment::Text(text) => split_references(&text, context, &mut out),
        }
    }
    out
}

/// `remarkAutolink`'s `split`: mentions and references become links where the host
/// has a path for them, and the text around them gets its emoji.
fn split_references(value: &str, context: Option<&AutolinkContext>, out: &mut Vec<Node>) {
    let spans = if context.is_some() {
        autolink_spans(value)
    } else {
        Vec::new()
    };
    let mut cursor = 0;
    for span in &spans {
        let Some(context) = context else { break };
        let url = match span {
            AutolinkSpan::Mention { handle, .. } => {
                mention_url(context.provider, &context.web_url, handle)
            }
            AutolinkSpan::Issue { number, .. } => {
                issue_url(context.provider, context.repo_root.as_deref(), *number)
            }
        };
        // No path on this host means the text stays as it is rather than becoming a
        // link that goes nowhere.
        let Some(url) = url else { continue };
        if span.start() > cursor {
            out.push(plain(
                render_emoji(&value[cursor..span.start()]).into_owned(),
            ));
        }
        out.push(link_node(&url, span.text()));
        cursor = span.end();
    }
    if cursor < value.len() {
        out.push(plain(render_emoji(&value[cursor..]).into_owned()));
    }
}

enum Segment {
    Text(String),
    Link { url: String, text: String },
}

static PUNCTUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{P}\p{S}]$").expect("valid pattern"));

/// What may come right before a literal autolink: the start of the text, white
/// space, or punctuation (and for an email, not a `/`).
fn previous_allows(value: &str, at: usize, email: bool) -> bool {
    let Some(previous) = value[..at].chars().next_back() else {
        return true;
    };
    let mut buffer = [0; 4];
    let boundary = previous.is_whitespace()
        || previous == '\u{FEFF}'
        || PUNCTUATION.is_match(previous.encode_utf8(&mut buffer));
    boundary && !(email && previous == '/')
}

/// A domain needs a dot, and its last two labels may not contain `_` and must
/// contain a letter or a digit.
fn is_correct_domain(domain: &str) -> bool {
    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    let bad = |part: &str| {
        !part.is_empty() && (part.contains('_') || !part.chars().any(|c| c.is_ascii_alphanumeric()))
    };
    !(bad(parts[parts.len() - 1]) || bad(parts[parts.len() - 2]))
}

/// Splits trailing punctuation off a URL, keeping closing parentheses that balance
/// opening ones inside it.
fn split_url(url: &str) -> (String, String) {
    let trail_start = url
        .char_indices()
        .rev()
        .take_while(|(_, c)| {
            matches!(
                c,
                '!' | '"' | '&' | '\'' | ')' | ',' | '.' | ':' | ';' | '<' | '>' | '?' | ']' | '}'
            )
        })
        .last()
        .map_or(url.len(), |(at, _)| at);
    let mut kept = url[..trail_start].to_string();
    let mut trail = url[trail_start..].to_string();
    let opening = kept.matches('(').count();
    let mut closing = kept.matches(')').count();
    while let Some(paren) = trail.find(')') {
        if opening <= closing {
            break;
        }
        kept.push_str(&trail[..=paren]);
        trail = trail[paren + 1..].to_string();
        closing += 1;
    }
    (kept, trail)
}

fn is_domain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')
}

static URL_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)https?://|www\.").expect("valid pattern"));

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([-.A-Za-z0-9_+]+)@([-A-Za-z0-9_]+(?:\.[-A-Za-z0-9_]+)+)").expect("valid pattern")
});

/// GFM's literal autolinks, as `remark-gfm` finds them: `https://...`,
/// `www....` (linked as `http://`), then email addresses (linked as `mailto:`).
fn literal_autolinks(value: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut emitted = 0;
    let mut search = 0;
    let bytes = value.as_bytes();

    while let Some(found) = URL_START.find_at(value, search) {
        let start = found.start();
        if !previous_allows(value, start, false) {
            // Retry one character later, as the regular expression would.
            search = start + 1;
            continue;
        }
        let www = found.as_str().starts_with(['w', 'W']);
        let mut domain_end = found.end();
        while domain_end < bytes.len() && is_domain_byte(bytes[domain_end]) {
            domain_end += 1;
        }
        // After a failure, any later `www.` inside the same domain ends in the same
        // labels and fails the same way; only an `http(s)://` whose scheme is the
        // tail of this domain could still succeed.
        let retry = domain_end.saturating_sub(5).max(start + 1);
        let (prefix, protocol, domain) = if www {
            ("http://", "", &value[start..domain_end])
        } else {
            ("", found.as_str(), &value[found.end()..domain_end])
        };
        if domain.is_empty() || !is_correct_domain(domain) {
            search = retry;
            continue;
        }
        let mut path_end = domain_end;
        while path_end < bytes.len() && !matches!(bytes[path_end], b' ' | b'\t' | b'\r' | b'\n') {
            path_end += 1;
        }
        let body_start = if www { start } else { found.end() };
        let (kept, trail) = split_url(&value[body_start..path_end]);
        if kept.is_empty() {
            search = retry;
            continue;
        }
        if start > emitted {
            segments.push(Segment::Text(value[emitted..start].to_string()));
        }
        segments.push(Segment::Link {
            url: format!("{prefix}{protocol}{kept}"),
            text: format!("{protocol}{kept}"),
        });
        if !trail.is_empty() {
            segments.push(Segment::Text(trail));
        }
        emitted = path_end;
        search = path_end;
    }
    if emitted < value.len() {
        segments.push(Segment::Text(value[emitted..].to_string()));
    }

    // Emails, in what is still text.
    let mut out = Vec::new();
    for segment in segments {
        match segment {
            Segment::Text(text) => emails(&text, &mut out),
            link => out.push(link),
        }
    }
    out
}

fn emails(value: &str, out: &mut Vec<Segment>) {
    let mut emitted = 0;
    let mut search = 0;
    while let Some(captures) = EMAIL.captures_at(value, search) {
        let (Some(whole), Some(local), Some(label)) =
            (captures.get(0), captures.get(1), captures.get(2))
        else {
            break;
        };
        if label.as_str().ends_with(['-', '_'])
            || label.as_str().ends_with(|c: char| c.is_ascii_digit())
        {
            // Every start inside this local part ends in the same label; a match may
            // still start inside the label itself.
            search = label.start();
            continue;
        }
        if !previous_allows(value, whole.start(), true) {
            // Try again just past the next punctuation inside the local part, the
            // only places a match could start.
            match local.as_str()[1..].find(['-', '.', '+', '_']) {
                Some(at) => {
                    search = local.start() + 1 + at + 1;
                    continue;
                }
                None => {
                    search = whole.end();
                    continue;
                }
            }
        }
        if whole.start() > emitted {
            out.push(Segment::Text(value[emitted..whole.start()].to_string()));
        }
        out.push(Segment::Link {
            url: format!("mailto:{}", whole.as_str()),
            text: whole.as_str().to_string(),
        });
        emitted = whole.end();
        search = whole.end();
    }
    if emitted < value.len() {
        out.push(Segment::Text(value[emitted..].to_string()));
    }
}

// ---------------------------------------------------------------------------
// Footnote numbering
// ---------------------------------------------------------------------------

struct Numbering {
    keys: HashMap<String, usize>,
    /// Definition indexes, in numbering order.
    order: Vec<usize>,
    /// Definition index to its number.
    numbers: HashMap<usize, usize>,
}

impl Numbering {
    fn new(definitions: &[Definition]) -> Numbering {
        Numbering {
            keys: definitions
                .iter()
                .enumerate()
                .map(|(index, definition)| (definition.key.clone(), index))
                .collect(),
            order: Vec::new(),
            numbers: HashMap::new(),
        }
    }

    fn visit(&mut self, nodes: &mut [Node]) {
        for node in nodes {
            let Node::El(element) = node else { continue };
            if let Kind::FootnoteRef { label, number } = &mut element.kind {
                if let Some(&definition) = self.keys.get(&footnote_key(label)) {
                    *number = *self.numbers.entry(definition).or_insert_with(|| {
                        self.order.push(definition);
                        self.order.len()
                    });
                }
                continue;
            }
            self.visit(&mut element.children);
        }
    }
}

// ---------------------------------------------------------------------------
// Shaping the working tree into the public one
// ---------------------------------------------------------------------------

struct Converter<'a> {
    images: Option<&'a ImageContext>,
}

impl Converter<'_> {
    /// Block content: runs of inline nodes between blocks become [`Block::Plain`].
    fn blocks(&self, nodes: Vec<Node>) -> Vec<Block> {
        let mut out = Vec::new();
        let mut run: Vec<Node> = Vec::new();
        for node in nodes {
            match node {
                Node::El(element) if element.kind.is_block() => {
                    self.flush(&mut run, &mut out);
                    self.block(element, &mut out);
                }
                other => run.push(other),
            }
        }
        self.flush(&mut run, &mut out);
        out
    }

    fn flush(&self, run: &mut Vec<Node>, out: &mut Vec<Block>) {
        if run.is_empty() {
            return;
        }
        let inlines = self.inline_content(std::mem::take(run));
        if !inlines.is_empty() {
            out.push(Block::Plain(inlines));
        }
    }

    /// Inline content of a block, white space collapsed.
    fn inline_content(&self, nodes: Vec<Node>) -> Vec<Inline> {
        let mut inlines = Vec::new();
        self.inlines(nodes, &mut inlines);
        collapse(&mut inlines);
        inlines
    }

    fn block(&self, element: El, out: &mut Vec<Block>) {
        let El { kind, children } = element;
        match kind {
            Kind::Paragraph { align } => {
                let inlines = self.inline_content(children);
                if !inlines.is_empty() {
                    out.push(aligned(align, Block::Paragraph(inlines)));
                }
            }
            Kind::Heading { level, align } => out.push(aligned(
                align,
                Block::Heading {
                    level: level.clamp(1, 6),
                    inlines: self.inline_content(children),
                },
            )),
            Kind::BlockQuote => {
                let mut children = children;
                match alert_of(&mut children) {
                    Some(kind) => out.push(Block::Alert {
                        kind,
                        blocks: self.blocks(children),
                    }),
                    None => out.push(Block::BlockQuote(self.blocks(children))),
                }
            }
            Kind::CodeBlock { lang } => out.push(Block::CodeBlock {
                lang,
                text: text_of(&children),
            }),
            Kind::Pre => out.push(pre_block(children)),
            Kind::List { ordered, start } => {
                out.push(Block::List(self.list(ordered, start, children)))
            }
            Kind::Table { alignments } => self.table(alignments, children, out),
            Kind::Rule => out.push(Block::Rule),
            Kind::Details { open } => {
                let mut summary = None;
                let mut rest = Vec::new();
                for child in children {
                    match child {
                        Node::El(El {
                            kind: Kind::Summary,
                            children,
                        }) if summary.is_none() => {
                            summary = Some(self.inline_content(children));
                        }
                        other => rest.push(other),
                    }
                }
                out.push(Block::Details {
                    open,
                    summary,
                    blocks: self.blocks(rest),
                });
            }
            Kind::Div { align } => out.push(Block::Div {
                align,
                blocks: self.blocks(children),
            }),
            // Structure out of place (a `<li>` outside a list, a `<summary>` outside
            // a details) still reads as a block of its own.
            Kind::Item { .. }
            | Kind::Summary
            | Kind::TableSection { .. }
            | Kind::TableRow
            | Kind::TableCell { .. }
            | Kind::Root
            | Kind::FootnoteDefinition { .. } => out.push(Block::Div {
                align: None,
                blocks: self.blocks(children),
            }),
            // Not blocks; never reach here.
            _ => {}
        }
    }

    fn list(&self, ordered: bool, start: u64, children: Vec<Node>) -> List {
        let mut items: Vec<ListItem> = Vec::new();
        let mut stray: Vec<Node> = Vec::new();
        for child in children {
            match child {
                Node::El(El {
                    kind: Kind::Item { task },
                    children,
                }) => {
                    self.attach_stray(&mut stray, &mut items);
                    items.push(ListItem {
                        task,
                        blocks: self.blocks(children),
                    });
                }
                other => stray.push(other),
            }
        }
        self.attach_stray(&mut stray, &mut items);
        List {
            ordered,
            start,
            items,
        }
    }

    /// Content directly inside a list but outside its items joins the item before it.
    fn attach_stray(&self, stray: &mut Vec<Node>, items: &mut Vec<ListItem>) {
        if stray.is_empty() {
            return;
        }
        let blocks = self.blocks(std::mem::take(stray));
        if blocks.is_empty() {
            return;
        }
        match items.last_mut() {
            Some(item) => item.blocks.extend(blocks),
            None => items.push(ListItem { task: None, blocks }),
        }
    }

    fn table(&self, alignments: Vec<Option<Alignment>>, children: Vec<Node>, out: &mut Vec<Block>) {
        let mut head = Vec::new();
        let mut rows = Vec::new();
        // Content in a table but outside its cells goes before the table, as a
        // browser moves it.
        let mut foster: Vec<Node> = Vec::new();
        let mut loose_cells: Vec<Node> = Vec::new();

        let flush_cells =
            |cells: &mut Vec<Node>, rows: &mut Vec<TableRow>, foster: &mut Vec<Node>| {
                if !cells.is_empty() {
                    rows.push(self.row(std::mem::take(cells), foster));
                }
            };

        for child in children {
            match child {
                Node::El(El {
                    kind: Kind::TableSection { head: is_head },
                    children,
                }) => {
                    flush_cells(&mut loose_cells, &mut rows, &mut foster);
                    let target = if is_head { &mut head } else { &mut rows };
                    let mut cells = Vec::new();
                    for part in children {
                        match part {
                            Node::El(El {
                                kind: Kind::TableRow,
                                children,
                            }) => {
                                flush_cells(&mut cells, &mut *target, &mut foster);
                                target.push(self.row(children, &mut foster));
                            }
                            cell @ Node::El(El {
                                kind: Kind::TableCell { .. },
                                ..
                            }) => cells.push(cell),
                            other => foster.push(other),
                        }
                    }
                    flush_cells(&mut cells, &mut *target, &mut foster);
                }
                Node::El(El {
                    kind: Kind::TableRow,
                    children,
                }) => {
                    flush_cells(&mut loose_cells, &mut rows, &mut foster);
                    rows.push(self.row(children, &mut foster));
                }
                cell @ Node::El(El {
                    kind: Kind::TableCell { .. },
                    ..
                }) => loose_cells.push(cell),
                other => foster.push(other),
            }
        }
        flush_cells(&mut loose_cells, &mut rows, &mut foster);

        out.extend(self.blocks(foster));
        out.push(Block::Table(Table {
            alignments,
            head,
            rows,
        }));
    }

    fn row(&self, children: Vec<Node>, foster: &mut Vec<Node>) -> TableRow {
        let mut cells = Vec::new();
        for child in children {
            match child {
                Node::El(El {
                    kind: Kind::TableCell { header, align },
                    children,
                }) => cells.push(TableCell {
                    header,
                    align,
                    blocks: self.blocks(children),
                }),
                other => foster.push(other),
            }
        }
        TableRow { cells }
    }

    fn image_source(&self, src: Option<String>) -> ImageSource {
        match (src, self.images) {
            (None, _) => ImageSource::Blocked,
            (Some(src), Some(context)) => rewrite_image_source(&src, context),
            (Some(src), None) => plain_image_source(&src),
        }
    }

    fn inlines(&self, nodes: Vec<Node>, out: &mut Vec<Inline>) {
        for node in nodes {
            match node {
                Node::Text(text) => out.push(Inline::Text(text.value)),
                Node::El(element) => self.inline(element, out),
            }
        }
    }

    fn children(&self, nodes: Vec<Node>) -> Vec<Inline> {
        let mut out = Vec::new();
        self.inlines(nodes, &mut out);
        out
    }

    fn inline(&self, element: El, out: &mut Vec<Inline>) {
        let El { kind, children } = element;
        match kind {
            Kind::Emphasis => out.push(Inline::Emphasis(self.children(children))),
            Kind::Strong => out.push(Inline::Strong(self.children(children))),
            Kind::Strike => out.push(Inline::Strikethrough(self.children(children))),
            Kind::Underline => out.push(Inline::Underline(self.children(children))),
            Kind::Kbd => out.push(Inline::Kbd(self.children(children))),
            Kind::Sub => out.push(Inline::Sub(self.children(children))),
            Kind::Sup => out.push(Inline::Sup(self.children(children))),
            Kind::Quote => {
                out.push(Inline::Text("\u{201C}".into()));
                self.inlines(children, out);
                out.push(Inline::Text("\u{201D}".into()));
            }
            Kind::Code { .. } => out.push(Inline::Code(text_of(&children).replace('\n', " "))),
            Kind::Link { href, title } => out.push(Inline::Link {
                href,
                title,
                children: self.children(children),
            }),
            Kind::Image {
                src,
                alt,
                title,
                width,
                height,
            } => out.push(Inline::Image(Image {
                source: self.image_source(src),
                alt,
                title,
                width,
                height,
            })),
            Kind::LineBreak => out.push(Inline::LineBreak),
            Kind::Checkbox { checked } => out.push(Inline::Checkbox { checked }),
            Kind::FootnoteRef { label, number } => {
                if number == 0 {
                    out.push(Inline::Text(format!("[^{label}]")));
                } else {
                    out.push(Inline::FootnoteRef { label, number });
                }
            }
            Kind::Dropped | Kind::Rule => {}
            // A block inside inline content (raw HTML can put one there) keeps its
            // text, set off by white space.
            Kind::CodeBlock { .. } | Kind::Pre => {
                out.push(Inline::Code(text_of(&children).replace('\n', " ")));
            }
            _ => {
                out.push(Inline::Text(" ".into()));
                self.inlines(children, out);
                out.push(Inline::Text(" ".into()));
            }
        }
    }
}

fn aligned(align: Option<Alignment>, block: Block) -> Block {
    match align {
        Some(align) => Block::Div {
            align: Some(align),
            blocks: vec![block],
        },
        None => block,
    }
}

/// `<pre>`: a code block, taking the language of a `<code class="language-*">` that
/// is all it holds. A newline right after `<pre>` is not content, as in HTML.
fn pre_block(children: Vec<Node>) -> Block {
    let mut elements = children.iter().filter(|node| match node {
        Node::Text(text) => !text.value.trim().is_empty(),
        Node::El(_) => true,
    });
    let lang = match (elements.next(), elements.next()) {
        (
            Some(Node::El(El {
                kind: Kind::Code { lang },
                ..
            })),
            None,
        ) => lang.clone(),
        _ => None,
    };
    let text = text_of(&children);
    let text = text.strip_prefix('\n').map(str::to_string).unwrap_or(text);
    Block::CodeBlock { lang, text }
}

/// Everything a subtree says, as plain text.
fn text_of(nodes: &[Node]) -> String {
    let mut out = String::new();
    collect_text(nodes, &mut out);
    out
}

fn collect_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(&text.value),
            Node::El(El {
                kind: Kind::LineBreak,
                ..
            }) => out.push('\n'),
            Node::El(El {
                kind: Kind::Image { alt, .. },
                ..
            }) => out.push_str(alt),
            Node::El(element) => collect_text(&element.children, out),
        }
    }
}

/// The alert a block quote is, with the marker taken out of its body.
///
/// The kind comes from the first paragraph's leading text (`alertKindOf`); the
/// marker is then removed from the leading text of the first element, which is that
/// same paragraph whenever the marker opens the quote. When the marker stood alone
/// in its own paragraph - a blank quoted line between it and the body - stripping
/// empties that paragraph, so it goes too.
fn alert_of(children: &mut Vec<Node>) -> Option<AlertKind> {
    let paragraph = children.iter().find_map(|node| match node {
        Node::El(element) if matches!(element.kind, Kind::Paragraph { .. }) => Some(element),
        _ => None,
    })?;
    let Some(Node::Text(first)) = paragraph.children.first() else {
        return None;
    };
    let kind = alert_kind_of(&first.value)?;

    let Some(index) = children.iter().position(|node| matches!(node, Node::El(_))) else {
        return Some(kind);
    };
    let Node::El(element) = &mut children[index] else {
        return Some(kind);
    };
    let only_child = element.children.len() == 1;
    if let Some(Node::Text(text)) = element.children.first_mut() {
        let rest = ALERT_MARKER.replace(&text.value, "").into_owned();
        if rest.is_empty() && only_child {
            children.remove(index);
        } else {
            text.value = rest;
        }
    }
    Some(kind)
}

// ---------------------------------------------------------------------------
// White space
// ---------------------------------------------------------------------------

/// Collapses white space the way a browser lays out normal text: a run of spaces,
/// tabs and line endings is one space, none is kept at the start of a block or of a
/// line after a hard break, and none at the end of a block. Code spans are left
/// alone. Text nodes that end up next to each other are joined.
fn collapse(inlines: &mut Vec<Inline>) {
    let mut previous_space = true;
    collapse_into(inlines, &mut previous_space);
    trim_end(inlines);
}

fn collapse_into(inlines: &mut Vec<Inline>, previous_space: &mut bool) {
    let old = std::mem::take(inlines);
    for inline in old {
        match inline {
            Inline::Text(text) => {
                let collapsed = collapse_text(&text, previous_space);
                if collapsed.is_empty() {
                    continue;
                }
                match inlines.last_mut() {
                    Some(Inline::Text(last)) => last.push_str(&collapsed),
                    _ => inlines.push(Inline::Text(collapsed)),
                }
            }
            Inline::LineBreak => {
                trim_end(inlines);
                inlines.push(Inline::LineBreak);
                *previous_space = true;
            }
            Inline::Emphasis(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Emphasis(children));
            }
            Inline::Strong(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Strong(children));
            }
            Inline::Strikethrough(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Strikethrough(children));
            }
            Inline::Underline(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Underline(children));
            }
            Inline::Kbd(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Kbd(children));
            }
            Inline::Sub(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Sub(children));
            }
            Inline::Sup(mut children) => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Sup(children));
            }
            Inline::Link {
                href,
                title,
                mut children,
            } => {
                collapse_into(&mut children, previous_space);
                inlines.push(Inline::Link {
                    href,
                    title,
                    children,
                });
            }
            other => {
                *previous_space = false;
                inlines.push(other);
            }
        }
    }
}

fn collapse_text(text: &str, previous_space: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C') {
            if !*previous_space {
                out.push(' ');
                *previous_space = true;
            }
        } else {
            out.push(c);
            *previous_space = false;
        }
    }
    out
}

/// Removes trailing spaces at the end of inline content; `true` once it reaches
/// something that is not white space.
fn trim_end(inlines: &mut Vec<Inline>) -> bool {
    loop {
        let Some(last) = inlines.last_mut() else {
            return false;
        };
        let reached = match last {
            Inline::Text(text) => {
                let kept = text.trim_end_matches(' ').len();
                text.truncate(kept);
                kept > 0
            }
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strikethrough(children)
            | Inline::Underline(children)
            | Inline::Kbd(children)
            | Inline::Sub(children)
            | Inline::Sup(children)
            | Inline::Link { children, .. } => trim_end(children),
            _ => true,
        };
        if reached {
            return true;
        }
        // Empty text, or a wrapper with nothing left in it: look before it.
        inlines.pop();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::images::ImageAccount;

    fn render(source: &str) -> Document {
        parse(source, &MarkdownContext::default())
    }

    fn github() -> AutolinkContext {
        AutolinkContext {
            provider: ProviderKind::Github,
            web_url: "https://github.com".into(),
            repo_root: Some("https://github.com/acme/api".into()),
        }
    }

    /// The pipeline as the app runs it, with the autolink transform in place.
    fn render_linked(source: &str, autolink: Option<AutolinkContext>) -> Document {
        parse(
            source,
            &MarkdownContext {
                autolink,
                images: None,
            },
        )
    }

    fn images() -> ImageContext {
        ImageContext {
            repo_root: Some("https://gitlab.acme.dev/acme/api".into()),
            accounts: vec![ImageAccount {
                id: "acct".into(),
                base_url: "https://gitlab.acme.dev/api/v4".into(),
                web_url: "https://gitlab.acme.dev".into(),
            }],
        }
    }

    fn render_images(source: &str) -> Document {
        parse(
            source,
            &MarkdownContext {
                autolink: None,
                images: Some(images()),
            },
        )
    }

    // --- walking the tree ---

    #[derive(Default)]
    struct Walk<'a> {
        blocks: Vec<&'a Block>,
        inlines: Vec<&'a Inline>,
    }

    fn walk(document: &Document) -> Walk<'_> {
        let mut out = Walk::default();
        walk_blocks(&document.blocks, &mut out);
        for footnote in &document.footnotes {
            walk_blocks(&footnote.blocks, &mut out);
        }
        out
    }

    fn walk_blocks<'a>(blocks: &'a [Block], out: &mut Walk<'a>) {
        for block in blocks {
            out.blocks.push(block);
            match block {
                Block::Paragraph(inlines) | Block::Plain(inlines) => walk_inlines(inlines, out),
                Block::Heading { inlines, .. } => walk_inlines(inlines, out),
                Block::BlockQuote(blocks)
                | Block::Alert { blocks, .. }
                | Block::Div { blocks, .. } => walk_blocks(blocks, out),
                Block::List(list) => {
                    for item in &list.items {
                        walk_blocks(&item.blocks, out);
                    }
                }
                Block::Table(table) => {
                    for row in table.head.iter().chain(&table.rows) {
                        for cell in &row.cells {
                            walk_blocks(&cell.blocks, out);
                        }
                    }
                }
                Block::Details {
                    summary, blocks, ..
                } => {
                    if let Some(summary) = summary {
                        walk_inlines(summary, out);
                    }
                    walk_blocks(blocks, out);
                }
                Block::CodeBlock { .. } | Block::Rule => {}
            }
        }
    }

    fn walk_inlines<'a>(inlines: &'a [Inline], out: &mut Walk<'a>) {
        for inline in inlines {
            out.inlines.push(inline);
            match inline {
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Underline(children)
                | Inline::Kbd(children)
                | Inline::Sub(children)
                | Inline::Sup(children)
                | Inline::Link { children, .. } => walk_inlines(children, out),
                _ => {}
            }
        }
    }

    fn inline_text(inlines: &[Inline]) -> String {
        let mut out = String::new();
        for inline in inlines {
            match inline {
                Inline::Text(text) | Inline::Code(text) => out.push_str(text),
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Underline(children)
                | Inline::Kbd(children)
                | Inline::Sub(children)
                | Inline::Sup(children)
                | Inline::Link { children, .. } => out.push_str(&inline_text(children)),
                Inline::LineBreak => out.push('\n'),
                _ => {}
            }
        }
        out
    }

    fn block_text(blocks: &[Block]) -> String {
        let mut parts = Vec::new();
        for block in blocks {
            parts.push(match block {
                Block::Paragraph(inlines) | Block::Plain(inlines) => inline_text(inlines),
                Block::Heading { inlines, .. } => inline_text(inlines),
                Block::BlockQuote(blocks)
                | Block::Alert { blocks, .. }
                | Block::Div { blocks, .. } => block_text(blocks),
                Block::CodeBlock { text, .. } => text.clone(),
                Block::List(list) => list
                    .items
                    .iter()
                    .map(|item| block_text(&item.blocks))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Block::Table(table) => table
                    .head
                    .iter()
                    .chain(&table.rows)
                    .flat_map(|row| row.cells.iter().map(|cell| block_text(&cell.blocks)))
                    .collect::<Vec<_>>()
                    .join(" "),
                Block::Details {
                    summary, blocks, ..
                } => format!(
                    "{} {}",
                    summary.as_deref().map(inline_text).unwrap_or_default(),
                    block_text(blocks)
                ),
                Block::Rule => String::new(),
            });
        }
        parts.join("\n")
    }

    fn text(document: &Document) -> String {
        block_text(&document.blocks)
    }

    fn links(document: &Document) -> Vec<(Option<String>, String)> {
        walk(document)
            .inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Link { href, children, .. } => Some((href.clone(), inline_text(children))),
                _ => None,
            })
            .collect()
    }

    fn codes(document: &Document) -> Vec<String> {
        walk(document)
            .inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Code(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn image_list(document: &Document) -> Vec<Image> {
        walk(document)
            .inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Image(image) => Some(image.clone()),
                _ => None,
            })
            .collect()
    }

    fn blockquotes(document: &Document) -> Vec<Option<AlertKind>> {
        walk(document)
            .blocks
            .into_iter()
            .filter_map(|block| match block {
                Block::BlockQuote(_) => Some(None),
                Block::Alert { kind, .. } => Some(Some(*kind)),
                _ => None,
            })
            .collect()
    }

    fn count_blocks(document: &Document, predicate: impl Fn(&Block) -> bool) -> usize {
        walk(document)
            .blocks
            .into_iter()
            .filter(|block| predicate(block))
            .count()
    }

    fn count_inlines(document: &Document, predicate: impl Fn(&Inline) -> bool) -> usize {
        walk(document)
            .inlines
            .into_iter()
            .filter(|inline| predicate(inline))
            .count()
    }

    // --- test/markdown.test.ts ---

    #[test]
    fn the_pipeline_renders_the_block_structure_a_description_relies_on() {
        let document = render(
            &[
                "# Title", "", "> quoted", "", "---", "", "- one", "- two", "", "1. first",
            ]
            .join("\n"),
        );

        assert_eq!(
            count_blocks(&document, |b| matches!(b, Block::Heading { level: 1, .. })),
            1
        );
        let quote = document
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::BlockQuote(blocks) => Some(block_text(blocks)),
                _ => None,
            })
            .unwrap_or_default();
        assert_eq!(quote.trim(), "quoted");
        assert_eq!(count_blocks(&document, |b| matches!(b, Block::Rule)), 1);

        let lists: Vec<&List> = document
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::List(list) => Some(list),
                _ => None,
            })
            .collect();
        assert_eq!(lists.len(), 2);
        assert!(!lists[0].ordered);
        assert_eq!(lists[0].items.len(), 2);
        assert!(lists[1].ordered);
        assert_eq!(lists[1].start, 1);
    }

    #[test]
    fn the_pipeline_renders_gfm_tables_with_their_column_alignment() {
        let document = render(
            &[
                "| Left | Centre | Right |",
                "| :--- | :----: | ----: |",
                "| a | b | c |",
            ]
            .join("\n"),
        );

        let Some(Block::Table(table)) = document.blocks.first() else {
            panic!("no table: {document:?}");
        };
        assert_eq!(table.head.len(), 1);
        let headers: Vec<Option<Alignment>> =
            table.head[0].cells.iter().map(|cell| cell.align).collect();
        assert_eq!(
            headers,
            vec![
                Some(Alignment::Left),
                Some(Alignment::Center),
                Some(Alignment::Right)
            ]
        );
        assert!(table.head[0].cells.iter().all(|cell| cell.header));
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].cells.len(), 3);
        assert!(table.rows[0].cells.iter().all(|cell| !cell.header));
        assert_eq!(table.rows[0].cells[2].align, Some(Alignment::Right));
        assert_eq!(block_text(&table.rows[0].cells[1].blocks), "b");
    }

    #[test]
    fn the_pipeline_renders_task_lists_carrying_their_checked_state() {
        let document = render(&["- [x] done", "- [ ] outstanding"].join("\n"));

        let Some(Block::List(list)) = document.blocks.first() else {
            panic!("no list: {document:?}");
        };
        let tasks: Vec<Option<bool>> = list.items.iter().map(|item| item.task).collect();
        assert_eq!(tasks, vec![Some(true), Some(false)]);
        assert_eq!(block_text(&list.items[0].blocks), "done");
        // A plain list item is not a task.
        let document = render("- plain");
        let Some(Block::List(list)) = document.blocks.first() else {
            panic!("no list");
        };
        assert_eq!(list.items[0].task, None);
    }

    #[test]
    fn the_pipeline_renders_strikethrough_inline_code_and_bare_url_autolinks() {
        let document = render("~~dropped~~ and `--flag` and https://example.test/page");

        let struck: Vec<String> = walk(&document)
            .inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Strikethrough(children) => Some(inline_text(children)),
                _ => None,
            })
            .collect();
        assert_eq!(struck, vec!["dropped"]);
        assert_eq!(codes(&document), vec!["--flag"]);
        assert_eq!(
            links(&document),
            vec![(
                Some("https://example.test/page".into()),
                "https://example.test/page".into()
            )]
        );
    }

    #[test]
    fn the_pipeline_keeps_a_fenced_block_as_preformatted_text_tagged_with_its_language() {
        let document = render(&["```ts", "const a = 1", "```"].join("\n"));

        assert_eq!(
            document.blocks,
            vec![Block::CodeBlock {
                lang: Some("ts".into()),
                text: "const a = 1\n".into(),
            }]
        );
    }

    #[test]
    fn the_sanitizer_keeps_the_html_real_pull_request_bodies_rely_on() {
        let document = render(
            &[
                "<details open><summary>Compatibility</summary>",
                "",
                "| Package | Change |",
                "| --- | --- |",
                "| a | 1.0<br>2.0 |",
                "",
                "</details>",
                "",
                "<img src=\"https://example.test/shot.png\" width=\"120\" alt=\"a screenshot\">",
                "",
                "Press <kbd>Cmd</kbd>.",
            ]
            .join("\n"),
        );

        let Some(Block::Details {
            open,
            summary,
            blocks,
        }) = document.blocks.first()
        else {
            panic!("no details: {document:?}");
        };
        assert!(*open);
        assert_eq!(
            summary.as_deref().map(inline_text).as_deref(),
            Some("Compatibility")
        );
        // The markdown table between the raw tags lands inside the section.
        assert!(matches!(blocks.as_slice(), [Block::Table(_)]));

        // A line break inside a table cell keeps a bot report's layout.
        assert_eq!(
            count_inlines(&document, |inline| matches!(inline, Inline::LineBreak)),
            1
        );

        let images = image_list(&document);
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0].source,
            ImageSource::Remote("https://example.test/shot.png".into())
        );
        assert_eq!(images[0].width, Some(Length::Pixels(120.0)));
        assert_eq!(images[0].alt, "a screenshot");

        let keys: Vec<String> = walk(&document)
            .inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Kbd(children) => Some(inline_text(children)),
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec!["Cmd"]);
        assert_eq!(
            document.blocks.last(),
            Some(&Block::Paragraph(vec![
                Inline::Text("Press ".into()),
                Inline::Kbd(vec![Inline::Text("Cmd".into())]),
                Inline::Text(".".into()),
            ]))
        );
    }

    #[test]
    fn the_sanitizer_removes_scripts_event_handlers_and_dangerous_url_schemes() {
        let document = render(
            &[
                "<script>alert(1)</script>",
                "",
                "<img src=\"x\" onerror=\"alert(2)\">",
                "",
                "<a href=\"javascript:alert(3)\" onclick=\"alert(4)\">click</a>",
                "",
                "<a href=\"data:text/html;base64,PHNjcmlwdD4=\">data</a>",
                "",
                "<a href=\"vbscript:alert(5)\">vbscript</a>",
                "",
                "<iframe src=\"https://evil.test\"></iframe>",
                "",
                "<object data=\"https://evil.test\"></object>",
                "",
                "<form action=\"https://evil.test\"><input type=\"text\" name=\"stolen\"></form>",
            ]
            .join("\n"),
        );

        // Everything a sanitizer failure could leave behind, in one haystack.
        let rendered = format!("{document:?}");
        assert!(
            !rendered.contains("alert("),
            "a handler survived: {rendered}"
        );
        for scheme in ["javascript:", "data:", "vbscript:", "evil.test", "stolen"] {
            assert!(!rendered.contains(scheme), "{scheme} survived: {rendered}");
        }

        // The anchors themselves stay - only their unusable targets are dropped.
        assert_eq!(
            links(&document),
            vec![
                (None, "click".into()),
                (None, "data".into()),
                (None, "vbscript".into()),
            ]
        );

        // A free-text input would be a credential prompt inside someone else's
        // prose, so whatever survives is a disabled checkbox.
        assert_eq!(
            count_inlines(&document, |inline| matches!(
                inline,
                Inline::Checkbox { checked: false }
            )),
            1
        );

        // An image whose source has no repository to resolve against is not fetched.
        assert_eq!(
            image_list(&document)
                .into_iter()
                .map(|image| image.source)
                .collect::<Vec<_>>(),
            vec![ImageSource::Blocked]
        );
    }

    #[test]
    fn the_sanitizer_leaves_no_id_a_description_could_borrow() {
        // There are no ids in the tree at all, so one borrowed from a pull request
        // description cannot shadow one of the app's own.
        let document = render("<div id=\"deck-search\">borrowed</div>");

        assert!(!format!("{document:?}").contains("deck-search"));
        assert_eq!(
            document.blocks,
            vec![Block::Div {
                align: None,
                blocks: vec![Block::Plain(vec![Inline::Text("borrowed".into())])],
            }]
        );
    }

    #[test]
    fn the_reader_keeps_the_elements_the_app_depends_on() {
        let document = render(
            "<details><summary>s</summary><kbd>k</kbd> <img src=\"https://e.test/i.png\"><br>\
             <table><tr><td>c</td></tr></table></details>",
        );
        let Some(Block::Details {
            summary, blocks, ..
        }) = document.blocks.first()
        else {
            panic!("no details: {document:?}");
        };
        assert_eq!(summary.as_deref().map(inline_text).as_deref(), Some("s"));
        assert_eq!(count_inlines(&document, |i| matches!(i, Inline::Kbd(_))), 1);
        assert_eq!(image_list(&document).len(), 1);
        assert_eq!(
            count_inlines(&document, |i| matches!(i, Inline::LineBreak)),
            1
        );
        assert!(blocks.iter().any(|block| matches!(block, Block::Table(_))));
    }

    #[test]
    fn alert_kind_of_recognises_every_alert_marker() {
        let document = render(
            &[
                "> [!NOTE]",
                "> Useful information.",
                "",
                "> [!TIP]",
                "> A shortcut.",
                "",
                "> [!IMPORTANT]",
                "> Do not miss this.",
                "",
                "> [!WARNING]",
                "> Urgent.",
                "",
                "> [!CAUTION]",
                "> Risky.",
            ]
            .join("\n"),
        );
        assert_eq!(
            blockquotes(&document),
            vec![
                Some(AlertKind::Note),
                Some(AlertKind::Tip),
                Some(AlertKind::Important),
                Some(AlertKind::Warning),
                Some(AlertKind::Caution),
            ]
        );
        // The marker line is gone from the body.
        assert_eq!(
            document.blocks.first(),
            Some(&Block::Alert {
                kind: AlertKind::Note,
                blocks: vec![Block::Paragraph(vec![Inline::Text(
                    "Useful information.".into()
                )])],
            })
        );
    }

    #[test]
    fn alert_kind_of_recognises_a_marker_standing_alone_in_its_own_paragraph() {
        // A blank quoted line puts the marker and the body in separate paragraphs.
        let document = render(&["> [!WARNING]", ">", "> Urgent."].join("\n"));
        assert_eq!(blockquotes(&document), vec![Some(AlertKind::Warning)]);
        // Stripping empties the marker's paragraph, so it goes.
        assert_eq!(
            document.blocks,
            vec![Block::Alert {
                kind: AlertKind::Warning,
                blocks: vec![Block::Paragraph(vec![Inline::Text("Urgent.".into())])],
            }]
        );
    }

    #[test]
    fn alert_kind_of_leaves_an_ordinary_block_quote_alone() {
        assert_eq!(
            blockquotes(&render("> Just something someone said.")),
            vec![None]
        );
        assert_eq!(
            blockquotes(&render("> [not a marker] and then some.")),
            vec![None]
        );
    }

    #[test]
    fn alert_kind_of_refuses_a_malformed_or_unknown_marker() {
        let document = render(
            &[
                "> [!BOGUS]",
                "> not a kind that exists",
                "",
                "> [!NOTE",
                "> unclosed",
                "",
                "> !NOTE]",
                "> no opening bracket",
                "",
                "> [!NOTE] trailing text on the marker line",
                "> so the marker does not stand alone",
            ]
            .join("\n"),
        );
        assert_eq!(blockquotes(&document), vec![None, None, None, None]);
        // And the text is left exactly as written.
        assert!(text(&document).starts_with("[!BOGUS] not a kind that exists"));
    }

    #[test]
    fn alert_kind_of_is_case_insensitive_and_reads_only_the_first_line() {
        assert_eq!(alert_kind_of("[!note]"), Some(AlertKind::Note));
        assert_eq!(alert_kind_of("[!Tip]  \nbody"), Some(AlertKind::Tip));
        assert_eq!(alert_kind_of("[!TIP] body"), None);
        assert_eq!(AlertKind::Important.label(), "Important");
        assert_eq!(AlertKind::Caution.as_str(), "caution");
    }

    #[test]
    fn the_transform_links_mentions_and_references_in_prose() {
        let document = render_linked("cc @mnovotna about #412", Some(github()));

        assert_eq!(
            links(&document),
            vec![
                (
                    Some("https://github.com/mnovotna".into()),
                    "@mnovotna".into()
                ),
                (
                    Some("https://github.com/acme/api/issues/412".into()),
                    "#412".into()
                ),
            ]
        );
        assert_eq!(text(&document), "cc @mnovotna about #412");
    }

    #[test]
    fn the_transform_leaves_a_reference_inside_a_code_span_as_literal_text() {
        // This is the whole reason the transform walks the tree rather than the
        // source: a code sample discussing a number must survive it.
        let document = render_linked("use `#412` and `@mnovotna` verbatim", Some(github()));

        assert_eq!(links(&document), vec![]);
        assert_eq!(codes(&document), vec!["#412", "@mnovotna"]);
    }

    #[test]
    fn the_transform_leaves_a_fenced_block_alone() {
        let document = render_linked(
            &["```js", "// see #412 from @mnovotna", "```"].join("\n"),
            Some(github()),
        );

        assert_eq!(links(&document), vec![]);
        assert!(text(&document).contains("see #412 from @mnovotna"));
    }

    #[test]
    fn the_transform_does_not_link_inside_an_already_linked_url() {
        let document = render_linked("https://example.test/@someone/repo", Some(github()));

        assert_eq!(
            links(&document),
            vec![(
                Some("https://example.test/@someone/repo".into()),
                "https://example.test/@someone/repo".into()
            )]
        );
    }

    #[test]
    fn the_transform_leaves_a_reference_plain_where_the_host_has_no_path_for_it() {
        let document = render_linked(
            "see #412 and @someone",
            Some(AutolinkContext {
                provider: ProviderKind::Bitbucket,
                web_url: "https://bitbucket.org".into(),
                repo_root: Some("https://bitbucket.org/acme/web".into()),
            }),
        );

        assert_eq!(links(&document), vec![]);
        assert_eq!(text(&document), "see #412 and @someone");
    }

    #[test]
    fn the_transform_renders_emoji_in_prose_but_not_inside_code() {
        let document = render_linked(
            "Shipped :tada: but `:tada:` stays, and :nope: is unknown",
            Some(github()),
        );

        assert!(text(&document).starts_with("Shipped 🎉"));
        assert_eq!(codes(&document), vec![":tada:"]);
        assert!(text(&document).contains(":nope: is unknown"));
    }

    #[test]
    fn the_transform_renders_emoji_even_with_no_host_to_link_against() {
        let document = render_linked("Shipped :rocket: for @someone in #3", None);

        assert_eq!(text(&document), "Shipped 🚀 for @someone in #3");
        assert_eq!(links(&document), vec![]);
    }

    #[test]
    fn the_image_rewrite_sends_a_signed_in_host_through_its_account() {
        let document =
            render_images("![shot](https://gitlab.acme.dev/acme/api/uploads/a/shot.png)");

        let images = image_list(&document);
        assert_eq!(
            images[0].source,
            ImageSource::Authenticated {
                account_id: "acct".into(),
                url: "https://gitlab.acme.dev/acme/api/uploads/a/shot.png".into(),
            }
        );
        assert_eq!(images[0].alt, "shot");
    }

    #[test]
    fn the_image_rewrite_leaves_a_source_on_any_other_host_over_plain_https() {
        let document = render_images("![badge](https://img.shields.io/badge/x.svg)");

        assert_eq!(
            image_list(&document)[0].source,
            ImageSource::Remote("https://img.shields.io/badge/x.svg".into())
        );
    }

    #[test]
    fn the_image_rewrite_reaches_an_image_written_as_raw_html_too() {
        let document = render_images("<img src=\"uploads/a/shot.png\" width=\"120\" alt=\"shot\">");

        let image = &image_list(&document)[0];
        assert_eq!(
            image.source,
            ImageSource::Authenticated {
                account_id: "acct".into(),
                url: "https://gitlab.acme.dev/acme/api/uploads/a/shot.png".into(),
            }
        );
        // Sanitized attributes survive the rewrite untouched.
        assert_eq!(image.width, Some(Length::Pixels(120.0)));
        assert_eq!(image.alt, "shot");
    }

    // --- Rust-specific: the HTML reader ---

    #[test]
    fn raw_html_formatting_maps_onto_inline_nodes() {
        let document = render(
            "a <b>b</b> <i>i</i> <s>s</s> <ins>u</ins> <code>c</code> <sub>2</sub> <sup>3</sup> \
             <a href=\"https://e.test/x\" title=\"T\">l</a> <q>q</q> <span>plain</span>",
        );
        let Some(Block::Paragraph(inlines)) = document.blocks.first() else {
            panic!("no paragraph: {document:?}");
        };
        assert_eq!(
            inlines,
            &vec![
                Inline::Text("a ".into()),
                Inline::Strong(vec![Inline::Text("b".into())]),
                Inline::Text(" ".into()),
                Inline::Emphasis(vec![Inline::Text("i".into())]),
                Inline::Text(" ".into()),
                Inline::Strikethrough(vec![Inline::Text("s".into())]),
                Inline::Text(" ".into()),
                Inline::Underline(vec![Inline::Text("u".into())]),
                Inline::Text(" ".into()),
                Inline::Code("c".into()),
                Inline::Text(" ".into()),
                Inline::Sub(vec![Inline::Text("2".into())]),
                Inline::Text(" ".into()),
                Inline::Sup(vec![Inline::Text("3".into())]),
                Inline::Text(" ".into()),
                Inline::Link {
                    href: Some("https://e.test/x".into()),
                    title: Some("T".into()),
                    children: vec![Inline::Text("l".into())],
                },
                Inline::Text(" \u{201C}q\u{201D} plain".into()),
            ]
        );
    }

    #[test]
    fn raw_html_entities_are_decoded_in_text_and_attributes() {
        let document = render(
            "<p title=\"x\">&copy; &amp; &#169; &#xA9; &lt;b&gt; &bogus; &#0; &amp</p>\n\n\
             <a href=\"https://e.test/?a=1&amp;b=2\">q</a>",
        );
        assert_eq!(text(&document), "© & © © <b> &bogus; \u{FFFD} &amp\nq");
        assert_eq!(
            links(&document),
            vec![(Some("https://e.test/?a=1&b=2".into()), "q".into())]
        );
    }

    #[test]
    fn raw_html_drops_dangerous_elements_with_their_content_and_keeps_unknown_ones_text() {
        let document = render(
            "<style>body { display: none }</style>\n\n\
             <textarea>typed</textarea>\n\n\
             before <custom-thing>kept</custom-thing> <!-- a comment --> after\n\n\
             <object>\n\n# swallowed\n\n</object>\n\nlast",
        );
        let rendered = text(&document);
        assert!(!rendered.contains("display"), "{rendered}");
        assert!(!rendered.contains("typed"), "{rendered}");
        assert!(!rendered.contains("comment"), "{rendered}");
        assert!(!rendered.contains("swallowed"), "{rendered}");
        assert!(rendered.contains("before kept after"), "{rendered}");
        assert!(rendered.ends_with("last"), "{rendered}");
    }

    #[test]
    fn raw_html_script_content_with_markup_inside_stays_dropped() {
        let document = render("<script>if (a<b) { document.write('<p>x</p>') }</script>\n\nok");
        assert_eq!(text(&document), "ok");
    }

    #[test]
    fn raw_html_alignment_and_sizes_are_kept() {
        let document = render(
            "<p align=\"center\">\n  <img src=\"https://e.test/logo.png\" width=\"50%\" height=\"40px\">\n</p>\n\n\
             <h1 align=\"right\">Title</h1>",
        );
        assert_eq!(
            document.blocks,
            vec![
                Block::Div {
                    align: Some(Alignment::Center),
                    blocks: vec![Block::Paragraph(vec![Inline::Image(Image {
                        source: ImageSource::Remote("https://e.test/logo.png".into()),
                        alt: String::new(),
                        title: None,
                        width: Some(Length::Percent(50.0)),
                        height: Some(Length::Pixels(40.0)),
                    })])],
                },
                Block::Div {
                    align: Some(Alignment::Right),
                    blocks: vec![Block::Heading {
                        level: 1,
                        inlines: vec![Inline::Text("Title".into())],
                    }],
                },
            ]
        );
    }

    #[test]
    fn raw_html_lists_tables_and_pre_take_their_structure() {
        let document = render(
            "<ol start=\"3\"><li>a<li>b</ol>\n\n\
             <table><thead><tr><th align=\"left\">H</th></tr></thead><tbody><tr><td>1<td>2</tr></tbody></table>\n\n\
             <pre><code class=\"language-rust\">\nfn main() {}\n</code></pre>",
        );
        let [
            Block::List(list),
            Block::Table(table),
            Block::CodeBlock { lang, text },
        ] = document.blocks.as_slice()
        else {
            panic!("unexpected shape: {document:?}");
        };
        assert!(list.ordered);
        assert_eq!(list.start, 3);
        assert_eq!(list.items.len(), 2);
        assert_eq!(table.head.len(), 1);
        assert_eq!(table.head[0].cells[0].align, Some(Alignment::Left));
        assert!(table.head[0].cells[0].header);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].cells.len(), 2);
        assert_eq!(lang.as_deref(), Some("rust"));
        assert_eq!(text, "fn main() {}\n");
    }

    #[test]
    fn raw_html_table_parts_outside_a_table_are_dropped_but_their_text_kept() {
        let document = render("<td>cell</td> <tr>row</tr>");
        assert_eq!(count_blocks(&document, |b| matches!(b, Block::Table(_))), 0);
        assert_eq!(text(&document), "cell row");
    }

    #[test]
    fn raw_html_checked_input_is_a_checked_checkbox() {
        let document = render("<ul><li><input type=\"checkbox\" checked disabled> done</li></ul>");
        let Some(Block::List(list)) = document.blocks.first() else {
            panic!("no list: {document:?}");
        };
        assert_eq!(
            list.items[0].blocks,
            vec![Block::Plain(vec![
                Inline::Checkbox { checked: true },
                Inline::Text(" done".into()),
            ])]
        );
    }

    #[test]
    fn markdown_links_keep_only_absolute_web_and_mail_targets() {
        let document = render(
            "[a](https://e.test) [b](relative/page.md) [c](#anchor) [d](javascript:alert(1)) \
             [e](mailto:me@e.test) <https://auto.test/x> <me@e.test> [f](HTTPS://E.TEST/Up)",
        );
        assert_eq!(
            links(&document),
            vec![
                (Some("https://e.test/".into()), "a".into()),
                (None, "b".into()),
                (None, "c".into()),
                (None, "d".into()),
                (Some("mailto:me@e.test".into()), "e".into()),
                (
                    Some("https://auto.test/x".into()),
                    "https://auto.test/x".into()
                ),
                (Some("mailto:me@e.test".into()), "me@e.test".into()),
                (Some("https://e.test/Up".into()), "f".into()),
            ]
        );
    }

    #[test]
    fn literal_autolinks_follow_gfm() {
        let document = render(
            "visit www.example.com/a_b, or (https://e.test/x_(y)) and write to me@mail.test. \
             not ahttps://x.test, nor 1.2.3@host1, nor http://localhost:3000",
        );
        assert_eq!(
            links(&document),
            vec![
                (
                    Some("http://www.example.com/a_b".into()),
                    "www.example.com/a_b".into()
                ),
                (
                    Some("https://e.test/x_(y)".into()),
                    "https://e.test/x_(y)".into()
                ),
                (Some("mailto:me@mail.test".into()), "me@mail.test".into()),
            ]
        );
        assert!(text(&document).contains("(https://e.test/x_(y)) and"));
    }

    #[test]
    fn literal_autolinks_skip_a_domain_without_a_proper_last_label() {
        let document = render("https://bad_.x_ https://a.b_c/x www.ok.test");
        assert_eq!(
            links(&document),
            vec![(Some("http://www.ok.test/".into()), "www.ok.test".into())]
        );
    }

    #[test]
    fn literal_autolinks_never_reach_into_raw_html_text() {
        let document = render_linked(
            "<summary>see https://e.test and #12 :tada:</summary>",
            Some(github()),
        );
        assert_eq!(links(&document), vec![]);
        assert!(text(&document).contains(":tada:"));
    }

    #[test]
    fn markdown_between_raw_tags_is_still_autolinked() {
        let document = render_linked("<b>@octocat</b> :tada:", Some(github()));
        assert_eq!(
            links(&document),
            vec![(Some("https://github.com/octocat".into()), "@octocat".into())]
        );
        assert!(text(&document).ends_with("🎉"));
    }

    #[test]
    fn markdown_images_take_their_alt_text_and_title() {
        let document =
            render("![a *b* `c`](https://e.test/i.png \"Title\") ![x](data:image/png;base64,AA)");
        let images = image_list(&document);
        assert_eq!(images[0].alt, "a b c");
        assert_eq!(images[0].title.as_deref(), Some("Title"));
        assert_eq!(images[1].source, ImageSource::Blocked);
    }

    #[test]
    fn footnotes_are_numbered_by_first_reference_and_unused_ones_dropped() {
        let document = render(
            "b[^second] a[^first] again[^second] missing[^nope]\n\n\
             [^first]: First.\n[^second]: Second[^third].\n[^third]: Third.\n[^unused]: Never.",
        );
        let numbers: Vec<(String, usize)> = walk(&document)
            .inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::FootnoteRef { label, number } => Some((label.clone(), *number)),
                _ => None,
            })
            .collect();
        assert_eq!(
            numbers,
            vec![
                ("second".into(), 1),
                ("first".into(), 2),
                ("second".into(), 1),
                ("third".into(), 3),
            ]
        );
        assert_eq!(
            document
                .footnotes
                .iter()
                .map(|f| (f.label.as_str(), f.number, block_text(&f.blocks)))
                .collect::<Vec<_>>(),
            vec![
                ("second", 1, "Second.".to_string()),
                ("first", 2, "First.".to_string()),
                ("third", 3, "Third.".to_string()),
            ]
        );
        assert!(text(&document).contains("missing[^nope]"));
    }

    #[test]
    fn white_space_collapses_like_a_browser() {
        let document = render("one\ntwo  three\\\nfour\n\n<p>\n  spaced   <b> out </b>\n</p>");
        assert_eq!(
            document.blocks,
            vec![
                Block::Paragraph(vec![
                    Inline::Text("one two three".into()),
                    Inline::LineBreak,
                    Inline::Text("four".into()),
                ]),
                Block::Paragraph(vec![
                    Inline::Text("spaced ".into()),
                    Inline::Strong(vec![Inline::Text("out".into())]),
                ]),
            ]
        );
    }

    #[test]
    fn tight_and_loose_list_items_differ_only_in_paragraphs() {
        let tight = render("- a\n- b");
        let Some(Block::List(list)) = tight.blocks.first() else {
            panic!()
        };
        assert_eq!(
            list.items[0].blocks,
            vec![Block::Plain(vec![Inline::Text("a".into())])]
        );
        let loose = render("- a\n\n- b");
        let Some(Block::List(list)) = loose.blocks.first() else {
            panic!()
        };
        assert_eq!(
            list.items[0].blocks,
            vec![Block::Paragraph(vec![Inline::Text("a".into())])]
        );
    }

    #[test]
    fn fence_languages_keep_only_what_a_highlighter_could_use() {
        assert_eq!(fence_language("c++"), Some("c++".into()));
        assert_eq!(fence_language("rust,ignore"), Some("rust".into()));
        assert_eq!(fence_language("{.rust}"), None);
        assert_eq!(
            class_language("foo language-{x} language-py"),
            Some("py".into())
        );
        let document = render("```\nplain\n```\n\n    indented");
        assert_eq!(
            document.blocks,
            vec![
                Block::CodeBlock {
                    lang: None,
                    text: "plain\n".into()
                },
                Block::CodeBlock {
                    lang: None,
                    text: "indented".into()
                },
            ]
        );
    }

    #[test]
    fn an_unclosed_details_holds_the_rest_of_the_body() {
        let document = render("<details><summary>More</summary>\n\nhidden\n\n# also hidden");
        let [Block::Details { blocks, open, .. }] = document.blocks.as_slice() else {
            panic!("unexpected shape: {document:?}");
        };
        assert!(!open);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn an_unclosed_inline_tag_ends_with_its_paragraph() {
        let document = render("a <b>bold\n\nnext");
        assert_eq!(
            document.blocks,
            vec![
                Block::Paragraph(vec![
                    Inline::Text("a ".into()),
                    Inline::Strong(vec![Inline::Text("bold".into())]),
                ]),
                Block::Paragraph(vec![Inline::Text("next".into())]),
            ]
        );
    }

    #[test]
    fn a_tag_cut_off_by_the_end_of_the_input_is_dropped() {
        let tokens = tokenize("text <img src=\"x", &mut Entities::default());
        assert_eq!(tokens, vec![Token::Text("text ".into())]);
        let tokens = tokenize("a < b </ c <3 </x", &mut Entities::default());
        assert_eq!(tokens, vec![Token::Text("a < b ".into())]);
        let tokens = tokenize(
            "<a href='q' HREF=\"r\" data-x=y/>",
            &mut Entities::default(),
        );
        assert_eq!(
            tokens,
            vec![Token::Start {
                name: "a".into(),
                attrs: vec![("href".into(), "q".into()), ("data-x".into(), "y/".into())],
            }]
        );
    }

    #[test]
    fn non_ascii_text_survives_every_stage() {
        let document = render_linked(
            "Příliš žluťoučký kůň @kůň #1 :tada: <b>úpěl</b> ďábelské ódy 🎉 https://e.test/ř https://ž.test",
            Some(github()),
        );
        let rendered = text(&document);
        assert!(rendered.starts_with("Příliš žluťoučký kůň @kůň #1 🎉 úpěl ďábelské ódy 🎉 "));
        // A handle stops at the first letter outside ASCII, and so does a domain -
        // which leaves `https://ž.test` unlinked, as GFM does.
        assert_eq!(
            links(&document),
            vec![
                (Some("https://github.com/k".into()), "@k".into()),
                (
                    Some("https://github.com/acme/api/issues/1".into()),
                    "#1".into()
                ),
                (
                    Some("https://e.test/%C5%99".into()),
                    "https://e.test/ř".into()
                ),
            ]
        );
    }

    // --- Rust-specific: hostile input ---

    fn depth(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                Block::BlockQuote(blocks)
                | Block::Alert { blocks, .. }
                | Block::Div { blocks, .. }
                | Block::Details { blocks, .. } => 1 + depth(blocks),
                Block::List(list) => {
                    1 + list
                        .items
                        .iter()
                        .map(|item| depth(&item.blocks))
                        .max()
                        .unwrap_or(0)
                }
                _ => 1,
            })
            .max()
            .unwrap_or(0)
    }

    fn inline_depth(inlines: &[Inline]) -> usize {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Underline(children)
                | Inline::Kbd(children)
                | Inline::Sub(children)
                | Inline::Sup(children)
                | Inline::Link { children, .. } => 1 + inline_depth(children),
                _ => 1,
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn deep_nesting_is_bounded_and_keeps_the_text() {
        let quotes = format!("{} deep", ">".repeat(50_000));
        let document = render(&quotes);
        assert!(depth(&document.blocks) <= MAX_DEPTH);
        assert!(text(&document).contains("deep"));

        let divs = format!("{}inside", "<div>".repeat(100_000));
        let document = render(&divs);
        assert!(depth(&document.blocks) <= MAX_DEPTH);
        assert!(text(&document).contains("inside"));

        let bold = format!("x {}inside", "<b>".repeat(100_000));
        let document = render(&bold);
        let Some(Block::Paragraph(inlines)) = document.blocks.first() else {
            panic!("no paragraph");
        };
        assert!(inline_depth(inlines) <= MAX_DEPTH);
        assert!(text(&document).contains("inside"));

        let lists = format!("{}item", "- ".repeat(10_000));
        let document = render(&lists);
        assert!(depth(&document.blocks) <= MAX_DEPTH);
        assert!(text(&document).contains("item"));

        let mixed = "> - <details><div>\n\n".repeat(5_000);
        let document = render(&mixed);
        assert!(depth(&document.blocks) <= MAX_DEPTH + 1);
    }

    #[test]
    fn hostile_input_parses_quickly() {
        let started = Instant::now();
        let inputs = [
            "*a **b ".repeat(30_000),
            "[".repeat(100_000),
            "<".repeat(100_000),
            "&".repeat(100_000),
            "&amp;&copy;&#x41;".repeat(30_000),
            "ahttp://".repeat(30_000),
            "www.www.".repeat(30_000) + "a_",
            format!("é{}@x.co1", "a.".repeat(50_000)),
            "@a #1 :tada: https://e.test/x ".repeat(20_000),
            "<a href=x>".repeat(50_000),
            "<!--".repeat(50_000),
            "| a |\n|---|\n".to_string() + &"| <td> |\n".repeat(20_000),
            "<script>".repeat(50_000),
            "|a".repeat(50_000) + "|\n" + &"|-".repeat(50_000) + "|\n" + &"|b".repeat(50_000),
            "[^a]".repeat(5_000)
                + "\n\n"
                + &(0..5_000)
                    .map(|n| format!("x[^{n}]\n\n[^{n}]: n{n}\n\n"))
                    .collect::<String>(),
            "x".repeat(1_000_000),
        ];
        for input in &inputs {
            let one = Instant::now();
            let document = parse(
                input,
                &MarkdownContext {
                    autolink: Some(github()),
                    images: Some(images()),
                },
            );
            drop(document);
            assert!(
                one.elapsed() < Duration::from_secs(10),
                "took {:?} on {:?}...",
                one.elapsed(),
                &input[..40.min(input.len())]
            );
        }
        assert!(started.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn javascript_links_in_every_spelling_are_dropped() {
        for source in [
            "[x](javascript:alert(1))",
            "[x](JAVASCRIPT:alert(1))",
            "[x]( javascript:alert(1))",
            "<a href=\"  javascript:alert(1)\">x</a>",
            "<a href=\"java&#x09;script:alert(1)\">x</a>",
            "<a href=\"&#106;avascript:alert(1)\">x</a>",
            "<javascript:alert(1)>",
            "[x](data:text/html,hi)",
            "[x](file:///etc/passwd)",
        ] {
            let document = render(source);
            for (href, _) in links(&document) {
                assert_eq!(href, None, "{source}");
            }
        }
    }

    #[test]
    fn empty_and_blank_input_is_an_empty_document() {
        assert_eq!(render(""), Document::default());
        assert_eq!(render("   \n\n  "), Document::default());
        assert_eq!(render("<!-- only a comment -->"), Document::default());
    }
}
