//! Placeholder until the dialogs agent's real file is merged (PLAN.md placeholder rule).

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window,
    div,
};

/// Which dialog to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Accounts,
    Settings,
    Schedule,
}

/// Emitted by a dialog that wants another one open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogEvent {
    Open(DialogKind),
}

pub struct SettingsDialog {
    focus: FocusHandle,
}

impl SettingsDialog {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        SettingsDialog {
            focus: cx.focus_handle(),
        }
    }
}

impl EventEmitter<DismissEvent> for SettingsDialog {}
impl EventEmitter<DialogEvent> for SettingsDialog {}

impl Focusable for SettingsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
