//! A row of toggle buttons with one selected, as the PullView tab strip and the
//! `aria-pressed` buttons in the TSX are.
//!
//! The selected item uses `active_variant` (secondary by default) and the rest are ghost
//! buttons, the way the TSX `TabButton` switches its `variant`. Each item may carry a leading
//! icon and a count, drawn as the muted `Count` pill.

use std::rc::Rc;

use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div};

use crate::ui::icons::IconName;
use crate::ui::theme::{ActiveTheme, rpx};

use super::button::{Button, ButtonSize, ButtonVariant};

/// One segment: its label, an optional leading icon and an optional count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentItem {
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub count: Option<usize>,
}

impl SegmentItem {
    pub fn new(label: impl Into<SharedString>) -> SegmentItem {
        SegmentItem {
            label: label.into(),
            icon: None,
            count: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> SegmentItem {
        self.icon = Some(icon);
        self
    }

    pub fn count(mut self, count: usize) -> SegmentItem {
        self.count = Some(count);
        self
    }
}

type SelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The toggle row. The view keeps the selected index and handles `on_select`.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: SharedString,
    items: Vec<SegmentItem>,
    selected: usize,
    active_variant: ButtonVariant,
    on_select: Option<SelectHandler>,
}

impl SegmentedControl {
    /// `id` must be unique in the window: it prefixes each segment's element id.
    pub fn new(id: impl Into<SharedString>, items: Vec<SegmentItem>) -> SegmentedControl {
        SegmentedControl {
            id: id.into(),
            items,
            selected: 0,
            active_variant: ButtonVariant::Secondary,
            on_select: None,
        }
    }

    pub fn selected(mut self, index: usize) -> SegmentedControl {
        self.selected = index;
        self
    }

    /// The look of the selected segment: `Success` or `Danger` for the review verdict tabs.
    pub fn active_variant(mut self, variant: ButtonVariant) -> SegmentedControl {
        self.active_variant = variant;
        self
    }

    /// Called with the index of the segment that was clicked.
    pub fn on_select(
        mut self,
        handler: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> SegmentedControl {
        self.on_select = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mut row = div().flex().items_center().gap(rpx(4.));
        for (ix, item) in self.items.into_iter().enumerate() {
            let active = ix == self.selected;
            let variant = if active {
                self.active_variant
            } else {
                ButtonVariant::Ghost
            };
            let mut button = Button::new(SharedString::from(format!("{}-{ix}", self.id)))
                .size(ButtonSize::Sm)
                .variant(variant);
            if let Some(icon) = item.icon {
                button = button.icon(icon);
            }
            button = button.child(item.label);
            if let Some(count) = item.count {
                button = button.child(
                    div()
                        .text_size(rpx(11.))
                        .text_color(colors.muted_foreground)
                        .child(count.to_string()),
                );
            }
            if let Some(handler) = &self.on_select {
                let handler = Rc::clone(handler);
                button = button.on_click(move |_, window, cx| handler(ix, window, cx));
            }
            row = row.child(button);
        }
        row
    }
}
