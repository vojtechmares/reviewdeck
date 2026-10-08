//! macOS glue gpui does not provide: the menu bar item, notifications, launch at
//! login, the app-wide appearance and the single-writer lock.
//!
//! Everything here is called from gpui's main thread. AppKit reports back - a menu
//! item clicked, a notification clicked - as a [`PlatformEvent`] on the channel
//! handed to [`tray::Tray::new`] and [`notify::Notifier::new`]; the app drains it
//! in a gpui foreground task. Nothing in an Objective-C callback calls into gpui:
//! AppKit invokes those while gpui may already be borrowed further up the stack.

// objc 0.2's `msg_send!`/`sel!` expand to `cfg(feature = "cargo-clippy")` checks.
#![allow(unexpected_cfgs)]

pub mod appearance;
pub mod instance;
pub mod login_item;
pub mod notify;
pub mod tray;
pub mod vibrancy;
pub mod window_drag;

use std::ffi::{CStr, c_void};
use std::os::raw::c_char;

use cocoa::base::{id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSString};
use futures::channel::mpsc::UnboundedSender;
use objc::declare::ClassDecl;
use objc::rc::StrongPtr;
use objc::runtime::{BOOL, Class, NO, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

/// What AppKit tells the app, delivered on the channel the app drains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    /// "Open Reviewdeck" in the menu bar menu.
    OpenWindow,
    /// "Refresh now" in the menu bar menu.
    Refresh,
    /// "Quit" in the menu bar menu.
    Quit,
    /// A notification was clicked. `target` is the review item to focus, for the
    /// notifications that announce a single review; the roll-ups only bring the
    /// window forward.
    NotificationClicked { target: Option<String> },
}

/// The sending half the Objective-C objects keep.
pub type EventSender = UnboundedSender<PlatformEvent>;

/// The bundle identifier of the running app, or `None` when it is not running from
/// a bundle that has one (`cargo run`). Notifications and launch at login are both
/// keyed by it, and both refuse - UNUserNotificationCenter by throwing - without it.
pub fn bundle_identifier() -> Option<String> {
    let _pool = AutoreleasePool::new();
    // SAFETY: NSBundle.mainBundle always exists; bundleIdentifier is nil or an
    // autoreleased NSString that outlives this function's pool.
    unsafe {
        let bundle: id = msg_send![class!(NSBundle), mainBundle];
        if bundle == nil {
            return None;
        }
        let identifier: id = msg_send![bundle, bundleIdentifier];
        string_from_ns(identifier).filter(|identifier| !identifier.is_empty())
    }
}

/// Drains the autoreleased objects created while it lives. gpui's run loop has a
/// pool of its own, but the platform calls also run before it turns (at launch)
/// and from tests, where an autoreleased object would otherwise leak.
struct AutoreleasePool(id);

impl AutoreleasePool {
    fn new() -> AutoreleasePool {
        // SAFETY: creating a pool is valid on any thread; it is drained on drop, on
        // the same thread (the guard is neither Send nor Sync: it holds a pointer).
        AutoreleasePool(unsafe { NSAutoreleasePool::new(nil) })
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        // SAFETY: the pool was created by `new` on this thread and is drained once.
        unsafe {
            let _: () = msg_send![self.0, drain];
        }
    }
}

/// An autoreleased NSString with the contents of `text`. Interior NULs survive:
/// the string is built from bytes and a length, not a C string.
///
/// # Safety
/// Needs an autorelease pool on the current thread to collect it.
unsafe fn ns_string(text: &str) -> id {
    // SAFETY: initWithBytes:length:encoding: copies the bytes; autorelease hands
    // the +1 reference from alloc/init to the pool.
    unsafe {
        let string: id = NSString::alloc(nil).init_str(text);
        msg_send![string, autorelease]
    }
}

/// The contents of an NSString, or `None` for nil or anything that is not one.
///
/// # Safety
/// `string` must be nil or a valid Objective-C object.
unsafe fn string_from_ns(string: id) -> Option<String> {
    if string == nil {
        return None;
    }
    // SAFETY: `string` is a live object; UTF8String returns a NUL-terminated buffer
    // owned by the string (or the current pool), copied out before either goes away.
    unsafe {
        let is_string: BOOL = msg_send![string, isKindOfClass: class!(NSString)];
        if is_string == NO {
            return None;
        }
        let utf8: *const c_char = msg_send![string, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

/// The localized description of an NSError, for messages the user reads.
///
/// # Safety
/// `error` must be nil or a valid NSError.
unsafe fn error_description(error: id) -> Option<String> {
    if error == nil {
        return None;
    }
    // SAFETY: -localizedDescription is defined on every NSError and returns an
    // autoreleased NSString.
    unsafe {
        let description: id = msg_send![error, localizedDescription];
        string_from_ns(description)
    }
}

/// Brings back every minimised window of the app - `window.restore()` in the
/// TypeScript's `show()`. gpui can order a window front but has no way to
/// deminiaturise one, so without this "Open Reviewdeck" and a Dock click would do
/// nothing for a window sitting in the Dock. Main thread only.
pub fn restore_miniaturized_windows() {
    debug_assert!(is_main_thread(), "NSApp is AppKit: main thread only");
    let _pool = AutoreleasePool::new();
    // SAFETY: main-thread AppKit calls on the live application; `windows` is an
    // autoreleased array of live windows, each asked only for its state.
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let windows: id = msg_send![app, windows];
        if windows == nil {
            return;
        }
        let count: usize = msg_send![windows, count];
        for index in 0..count {
            let window: id = msg_send![windows, objectAtIndex: index];
            let miniaturized: BOOL = msg_send![window, isMiniaturized];
            if miniaturized != NO {
                let _: () = msg_send![window, deminiaturize: nil];
            }
        }
    }
}

/// Whether the caller is on the main thread, which every AppKit call here needs.
fn is_main_thread() -> bool {
    // SAFETY: +[NSThread isMainThread] is thread-safe and has no preconditions.
    let main: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
    main != NO
}

/// The ivar the event-sending classes keep their boxed [`EventSender`] in.
const SENDER_IVAR: &str = "reviewdeckEvents";

/// Adds the sender ivar to a class being declared, and the `dealloc` that frees it,
/// so the sender lives exactly as long as the Objective-C object holding it - which
/// AppKit, not Rust, may be the last to let go of.
fn declare_sender(decl: &mut ClassDecl) {
    decl.add_ivar::<*mut c_void>(SENDER_IVAR);
    // SAFETY: the signature matches -dealloc (no arguments, no return value).
    unsafe {
        decl.add_method(
            sel!(dealloc),
            dealloc_with_sender as extern "C" fn(&mut Object, Sel),
        );
    }
}

/// A new instance of `class` (declared with [`declare_sender`]) holding `events`.
///
/// # Safety
/// `class` must have been declared through [`declare_sender`]. Main thread.
unsafe fn new_with_sender(class: &Class, events: EventSender) -> Option<StrongPtr> {
    // SAFETY: +new returns a +1 instance with zeroed ivars, which StrongPtr takes
    // over; the ivar exists because the class went through `declare_sender`.
    unsafe {
        let object: id = msg_send![class, new];
        if object == nil {
            return None;
        }
        let boxed = Box::into_raw(Box::new(events)) as *mut c_void;
        (*object).set_ivar(SENDER_IVAR, boxed);
        Some(StrongPtr::new(object))
    }
}

/// Sends `event` from inside one of the objects' Objective-C methods. A closed
/// channel means the app is going away; there is nobody left to tell.
///
/// # Safety
/// `this` must be an instance of a class declared through [`declare_sender`].
unsafe fn send_event(this: &Object, event: PlatformEvent) {
    // SAFETY: the ivar holds null or the pointer `new_with_sender` boxed, which
    // stays valid until `dealloc`, and `dealloc` cannot run while a method of the
    // same object is executing.
    unsafe {
        let pointer: *mut c_void = *this.get_ivar(SENDER_IVAR);
        if let Some(sender) = (pointer as *const EventSender).as_ref() {
            let _ = sender.unbounded_send(event);
        }
    }
}

extern "C" fn dealloc_with_sender(this: &mut Object, _: Sel) {
    // SAFETY: the ivar holds null or the Box `new_with_sender` leaked, freed exactly
    // once here; then NSObject's dealloc finishes the object.
    unsafe {
        let pointer: *mut c_void = *this.get_ivar(SENDER_IVAR);
        if !pointer.is_null() {
            this.set_ivar(SENDER_IVAR, std::ptr::null_mut::<c_void>());
            drop(Box::from_raw(pointer as *mut EventSender));
        }
        let _: () = msg_send![super(this, class!(NSObject)), dealloc];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;

    #[test]
    fn ns_string_round_trips_utf8() {
        let _pool = AutoreleasePool::new();
        for text in [
            "",
            "Reviewdeck",
            "3 reviews waiting · quiet until 09:00",
            "příliš žluťoučký 🦀",
        ] {
            // SAFETY: a pool is in place.
            let back = unsafe { string_from_ns(ns_string(text)) };
            assert_eq!(back.as_deref(), Some(text));
        }
    }

    #[test]
    fn string_from_ns_rejects_nil_and_non_strings() {
        let _pool = AutoreleasePool::new();
        // SAFETY: nil is allowed; an NSNumber is a valid object that is no NSString.
        unsafe {
            assert_eq!(string_from_ns(nil), None);
            let number: id = msg_send![class!(NSNumber), numberWithInt: 3];
            assert_eq!(string_from_ns(number), None);
        }
    }

    #[test]
    fn an_unbundled_test_binary_has_no_bundle_identifier() {
        // The test harness is a bare executable, like `cargo run`: exactly the case
        // in which notifications must never be touched.
        assert_eq!(bundle_identifier(), None);
    }

    #[test]
    fn the_sender_reaches_the_channel_and_dies_with_the_object() {
        static REGISTER: std::sync::Once = std::sync::Once::new();
        REGISTER.call_once(|| {
            if let Some(mut decl) = ClassDecl::new("ReviewdeckSenderProbe", class!(NSObject)) {
                declare_sender(&mut decl);
                decl.register();
            }
        });
        let Some(class) = Class::get("ReviewdeckSenderProbe") else {
            panic!("the probe class registers");
        };
        let (events, mut receiver) = mpsc::unbounded();
        // SAFETY: the class was declared through `declare_sender`.
        let object = unsafe { new_with_sender(class, events) };
        let Some(object) = object else {
            panic!("the probe instantiates");
        };
        // SAFETY: `object` is a live instance of that class.
        unsafe { send_event(&**object, PlatformEvent::Refresh) };
        assert_eq!(receiver.try_recv().ok(), Some(PlatformEvent::Refresh));
        drop(object);
        // Dealloc dropped the sender, so the channel reports closed.
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Closed)
        ));
    }
}
