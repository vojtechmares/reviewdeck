//! Placeholder until the pull-view agent's real file is merged (PLAN.md placeholder rule).

use gpui::{Context, IntoElement, Render, Window, div, prelude::*};

pub struct PullView {
    #[allow(dead_code)]
    item_id: String,
}

impl PullView {
    pub fn new(item_id: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        PullView { item_id }
    }
}

impl Render for PullView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}
