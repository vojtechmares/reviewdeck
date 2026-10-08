//! The window's milky glass: the same native vibrancy the Electron window had.
//!
//! Electron opened the window with `vibrancy: 'under-window'` and
//! `visualEffectState: 'active'`, which puts an `NSVisualEffectView` with the
//! under-window-background material behind the page, blending with what is behind the
//! window and staying vibrant when the window is not key. The page then painted its
//! heavy translucent film (`--background`) over that material.
//!
//! gpui's own `WindowBackgroundAppearance::Blurred` is not that. It also adds an
//! `NSVisualEffectView`, but deliberately strips it to a bare blur - no material
//! colour, no desktop tinting, no saturation - so the same film over it lets the
//! desktop show through far more than it did in Electron, and reads as see-through
//! rather than frosted. So the window is opened `Transparent` and this view, left
//! exactly as AppKit makes it, goes in underneath gpui's content instead.
//!
//! The material follows the window's effective appearance, which follows
//! `NSApp.appearance` (see `appearance.rs`), so it turns light or dark with the theme.

use cocoa::base::{id, nil};
use cocoa::foundation::NSRect;
use objc::{class, msg_send, sel, sel_impl};

/// `NSVisualEffectMaterialUnderWindowBackground`: Electron's `'under-window'`.
const MATERIAL_UNDER_WINDOW_BACKGROUND: i64 = 21;
/// `NSVisualEffectBlendingModeBehindWindow`.
const BLENDING_BEHIND_WINDOW: i64 = 0;
/// `NSVisualEffectStateActive`: Electron's `visualEffectState: 'active'`.
const STATE_ACTIVE: i64 = 1;
/// `NSViewWidthSizable | NSViewHeightSizable`.
const RESIZE_WITH_SUPERVIEW: u64 = 2 | 16;
/// `NSWindowBelow`.
const BELOW: i64 = -1;

/// Puts the vibrancy view under a gpui window's content. Call it when the window
/// opens; it is idempotent. Open the window with
/// `WindowBackgroundAppearance::Transparent`, or gpui's own stripped blur sits on top.
pub fn install_under(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // `Window` has an inherent `window_handle()` that shadows the trait method.
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let ns_view = appkit.ns_view.as_ptr() as id;
    // SAFETY: gpui hands out the live view of a window it owns, on the main thread
    // where windows are opened; `-window` returns the NSWindow that view is in.
    unsafe {
        let ns_window: id = msg_send![ns_view, window];
        install(ns_window);
    }
}

/// Puts the vibrancy view under the content of the window `ns_window`. Safe to call
/// more than once for a window: it adds the view only the first time.
///
/// # Safety
///
/// `ns_window` must be a live `NSWindow`, and this must run on the main thread.
unsafe fn install(ns_window: id) {
    if ns_window == nil {
        return;
    }
    // SAFETY: the caller guarantees a live NSWindow on the main thread. Every message
    // below is a documented AppKit call on objects owned by that window; the new view
    // is retained by its superview once added, so the alloc's own reference is
    // released with `autorelease`.
    unsafe {
        let content: id = msg_send![ns_window, contentView];
        if content == nil {
            return;
        }
        let effect_class = class!(NSVisualEffectView);
        let subviews: id = msg_send![content, subviews];
        let count: u64 = msg_send![subviews, count];
        for index in 0..count {
            let view: id = msg_send![subviews, objectAtIndex: index];
            // Exactly NSVisualEffectView, not gpui's BlurredView subclass.
            let class: *const objc::runtime::Class = msg_send![view, class];
            if std::ptr::eq(class, effect_class) {
                return;
            }
        }

        let frame: NSRect = msg_send![content, bounds];
        let view: id = msg_send![effect_class, alloc];
        let view: id = msg_send![view, initWithFrame: frame];
        let _: () = msg_send![view, setMaterial: MATERIAL_UNDER_WINDOW_BACKGROUND];
        let _: () = msg_send![view, setBlendingMode: BLENDING_BEHIND_WINDOW];
        let _: () = msg_send![view, setState: STATE_ACTIVE];
        let _: () = msg_send![view, setAutoresizingMask: RESIZE_WITH_SUPERVIEW];
        // A backdrop never takes the keyboard (an NSView refuses first responder by
        // default) or a click: it sits below gpui's view, which covers the window.
        let _: () = msg_send![content, addSubview: view positioned: BELOW relativeTo: nil];
        let _: id = msg_send![view, autorelease];
    }
}
