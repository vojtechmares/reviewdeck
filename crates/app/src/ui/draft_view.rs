//! Port of src/renderer/src/components/Draft.tsx.
//!
//! Placeholder: the minimal contract from PLAN.md so the diff view compiles. The
//! threads agent's real file replaces this one at merge.

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};
use reviewdeck_core::model::DraftComment;

use crate::ui::theme::{ActiveTheme, rpx};

pub struct DraftCard {
    draft: DraftComment,
}

impl DraftCard {
    pub fn new(draft: DraftComment, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        DraftCard { draft }
    }

    pub fn set_draft(&mut self, draft: DraftComment, cx: &mut Context<Self>) {
        self.draft = draft;
        cx.notify();
    }
}

impl Render for DraftCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .m(rpx(6.))
            .p(rpx(8.))
            .rounded(rpx(10.))
            .bg(cx.theme().colors.surface_muted)
            .text_size(rpx(12.))
            .child(self.draft.body.clone())
    }
}
