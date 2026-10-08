//! Helpers for the kit's interaction tests: a host view per control, built over gpui's test
//! platform. Compiled for tests only.

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, px,
};

/// Binds the kit's keys. Call first in every test.
pub fn init(cx: &mut TestAppContext) {
    cx.update(super::bind_keys);
}

type Build = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A window root that renders whatever its closure returns, every frame, with 24px of
/// padding so the control is not flush with the window edge. State lives outside, in the
/// entities and cells the closure captures; call [`refresh`] after changing a cell.
pub struct Host {
    build: Build,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(24.))
            .font_family(crate::ui::theme::UI_FONT)
            .text_size(px(13.))
            .child((self.build)(window, cx))
    }
}

/// Opens a window showing whatever `build` returns.
pub fn window(
    cx: &mut TestAppContext,
    build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> (Entity<Host>, &mut VisualTestContext) {
    cx.add_window_view(|_, _| Host {
        build: Box::new(build),
    })
}

/// Re-renders the host after a captured cell changed.
pub fn refresh(host: &Entity<Host>, cx: &mut VisualTestContext) {
    host.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    redraw(cx);
}

/// Draws the window now, so `debug_bounds` reflects the latest state.
pub fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
}

/// A shared list the tests read results out of.
pub type Log<T> = std::rc::Rc<std::cell::RefCell<Vec<T>>>;

pub fn log<T>() -> Log<T> {
    std::rc::Rc::new(std::cell::RefCell::new(Vec::new()))
}
