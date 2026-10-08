//! Port of src/renderer/src/components/DiffView.tsx.
//!
//! MINIMAL STUB written by the pull-view agent so that `pull_view` compiles; the
//! diff agent's real file replaces it at merge.

use std::sync::Arc;

use gpui::{Context, EventEmitter, IntoElement, ParentElement, Render, Window, div};
use reviewdeck_core::model::{CommentThread, DiffFile, DiffRefs};

pub struct DiffView {
    files: Arc<Vec<DiffFile>>,
    threads: Arc<Vec<CommentThread>>,
}

pub enum DiffEvent {
    ThreadsChanged,
}

impl EventEmitter<DiffEvent> for DiffView {}

impl DiffView {
    pub fn new(
        _item_id: String,
        files: Arc<Vec<DiffFile>>,
        inline_threads: Arc<Vec<CommentThread>>,
        _refs: DiffRefs,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        DiffView {
            files,
            threads: inline_threads,
        }
    }

    pub fn set_threads(&mut self, inline_threads: Arc<Vec<CommentThread>>, cx: &mut Context<Self>) {
        self.threads = inline_threads;
        cx.notify();
    }
}

impl Render for DiffView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(format!(
            "{} files, {} threads",
            self.files.len(),
            self.threads.len()
        ))
    }
}
