//! Port of src/renderer/src/components/Thread.tsx.
//!
//! MINIMAL STUB written by the pull-view agent so that `pull_view` compiles; the
//! threads agent's real file replaces it at merge.

use gpui::{Context, EventEmitter, IntoElement, ParentElement, Render, Window, div};
use reviewdeck_core::model::CommentThread;

pub struct ThreadCard {
    thread: CommentThread,
}

pub enum ThreadEvent {
    Changed,
}

impl EventEmitter<ThreadEvent> for ThreadCard {}

impl ThreadCard {
    pub fn new(
        _item_id: String,
        thread: CommentThread,
        _dense: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        ThreadCard { thread }
    }

    pub fn set_thread(&mut self, thread: CommentThread, cx: &mut Context<Self>) {
        self.thread = thread;
        cx.notify();
    }
}

impl Render for ThreadCard {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(format!("thread {}", self.thread.id))
    }
}
