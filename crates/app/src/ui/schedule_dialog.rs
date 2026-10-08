//! Placeholder until the dialogs agent's real file is merged (PLAN.md placeholder rule).

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window,
    div,
};

pub struct ScheduleDialog {
    focus: FocusHandle,
}

impl ScheduleDialog {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        ScheduleDialog {
            focus: cx.focus_handle(),
        }
    }
}

impl EventEmitter<DismissEvent> for ScheduleDialog {}

impl Focusable for ScheduleDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ScheduleDialog {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
