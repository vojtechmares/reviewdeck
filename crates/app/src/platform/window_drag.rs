//! Moving the window by its header, which gpui 0.2.2 cannot do on macOS.
//!
//! The TSX marked the header `-webkit-app-region: drag`. gpui has a
//! `WindowControlArea::Drag`, but its macOS window never reads it back (the hit-test
//! callback is dropped), and `Window::start_window_move` is a no-op there. What a
//! native title bar does is ask AppKit to run the drag itself, with the mouse-down
//! event that started it: `-[NSWindow performWindowDragWithEvent:]`. That also gets
//! the things a hand-rolled drag would have to imitate - the snapping, the Spaces
//! hand-off, the drag outline when the window is moved across displays.
//!
//! The double-click is gpui's own `Window::titlebar_double_click`, which reads
//! `AppleActionOnDoubleClick` (zoom, minimise, fill or nothing) like a native title
//! bar does.

use cocoa::base::{id, nil};
use objc::{class, msg_send, sel, sel_impl};

/// Hands the mouse-down that is being dispatched to AppKit as a window drag.
///
/// Call it from a mouse-down handler only: the event is `NSApp.currentEvent`, which
/// gpui's `mouseDown:` is still inside of while it runs the handlers, and the window
/// to move is the one that event was delivered to. Returns whether a drag was
/// started - `false` when there is no current event (a test, a synthesised click) or
/// it is not a mouse-down.
pub fn begin_window_drag() -> bool {
    // SAFETY: plain Objective-C messaging on the main thread, where gpui runs its
    // handlers. `currentEvent` and `window` are autoreleased and only used within
    // this call; `performWindowDragWithEvent:` is sent to a live NSWindow with an
    // event that belongs to it.
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let event: id = msg_send![app, currentEvent];
        if event == nil {
            return false;
        }
        // NSEventTypeLeftMouseDown = 1.
        let kind: u64 = msg_send![event, type];
        if kind != 1 {
            return false;
        }
        let window: id = msg_send![event, window];
        if window == nil {
            return false;
        }
        let _: () = msg_send![window, performWindowDragWithEvent: event];
        true
    }
}

#[cfg(test)]
mod tests {
    use super::begin_window_drag;

    #[test]
    fn there_is_nothing_to_drag_without_a_mouse_down() {
        // A test binary has no event loop and so no current event: the call must say
        // so rather than message a nil window.
        assert!(!begin_window_drag());
    }
}
