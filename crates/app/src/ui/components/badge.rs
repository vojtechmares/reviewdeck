//! Port of src/renderer/src/components/ui/badge.tsx.

use gpui::{
    AnyElement, App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, Styled, Window, div,
};

use crate::ui::theme::{ActiveTheme, radius, rpx};

use super::button::with_alpha;

/// `tone` on the TSX badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    #[default]
    Neutral,
    Ok,
    Bad,
    Busy,
    Info,
}

/// A small pill of text, `inline-flex ... rounded-md border px-1.5 py-0.5 text-[11px]`.
///
/// ```ignore
/// Badge::new().tone(BadgeTone::Ok).child("Approved")
/// ```
#[derive(IntoElement)]
pub struct Badge {
    tone: BadgeTone,
    children: Vec<AnyElement>,
}

impl Default for Badge {
    fn default() -> Self {
        Badge::new()
    }
}

impl Badge {
    pub fn new() -> Badge {
        Badge {
            tone: BadgeTone::default(),
            children: Vec::new(),
        }
    }

    pub fn tone(mut self, tone: BadgeTone) -> Badge {
        self.tone = tone;
        self
    }
}

impl ParentElement for Badge {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// (background, text, border) for a tone. `ok/25` is the colour at alpha 0.25.
fn tone_colours(tone: BadgeTone, cx: &App) -> (Hsla, Hsla, Hsla) {
    let colors = cx.theme().colors;
    match tone {
        BadgeTone::Neutral => (colors.muted, colors.muted_foreground, colors.border),
        BadgeTone::Ok => (colors.ok_soft, colors.ok, with_alpha(colors.ok, 0.25)),
        BadgeTone::Bad => (colors.bad_soft, colors.bad, with_alpha(colors.bad, 0.25)),
        BadgeTone::Busy => (colors.busy_soft, colors.busy, with_alpha(colors.busy, 0.25)),
        BadgeTone::Info => (colors.info_soft, colors.info, with_alpha(colors.info, 0.25)),
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (background, text, border) = tone_colours(self.tone, cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(rpx(4.))
            .rounded(rpx(radius::MD))
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(text)
            .px(rpx(6.))
            .py(rpx(2.))
            .text_size(rpx(11.))
            .line_height(rpx(11.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .children(self.children)
    }
}
