//! Placeholder until the dialogs agent's real file is merged (PLAN.md placeholder rule).

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window,
    div,
};

pub struct AccountsDialog {
    focus: FocusHandle,
}

impl AccountsDialog {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        AccountsDialog {
            focus: cx.focus_handle(),
        }
    }
}

impl EventEmitter<DismissEvent> for AccountsDialog {}

impl Focusable for AccountsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AccountsDialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
