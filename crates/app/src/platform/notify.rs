//! Notifications through UNUserNotificationCenter; port of the notification half of
//! src/main/deck.ts (Electron's `Notification`).
//!
//! What to announce, and when to stay quiet - first sync, review windows, a focused
//! window - is decided by the app, exactly as deck.ts decides it; this module only
//! delivers. A click comes back as [`PlatformEvent::NotificationClicked`] carrying
//! the review to focus, if the notification was about one.

use std::sync::Once;

use block::{Block, ConcreteBlock};
use cocoa::base::{id, nil};
use objc::declare::ClassDecl;
use objc::rc::StrongPtr;
use objc::runtime::{BOOL, Class, NO, Object, Protocol, Sel};
use objc::{class, msg_send, sel, sel_impl};

use super::{
    AutoreleasePool, EventSender, PlatformEvent, bundle_identifier, declare_sender,
    error_description, is_main_thread, new_with_sender, ns_string, send_event, string_from_ns,
};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

/// UNAuthorizationOptions: what the app asks to be allowed (alert + sound; the dock
/// badge is not used, as in the Electron app).
const AUTHORIZE_SOUND: u64 = 1 << 1;
const AUTHORIZE_ALERT: u64 = 1 << 2;

/// UNNotificationPresentationOptions for a notification arriving while the app is
/// frontmost: shown as usual. Electron shows them then too, and the cases where
/// that would be noise (the roll-up while the window is focused) are filtered out
/// before anything gets here.
const PRESENT_SOUND: u64 = 1 << 1;
const PRESENT_LIST: u64 = 1 << 3;
const PRESENT_BANNER: u64 = 1 << 4;

/// The userInfo key carrying the review a click should focus.
const TARGET_KEY: &str = "target";

/// UNNotificationDismissActionIdentifier. Only delivered for categories that ask for
/// it (none here); a dismissal is not a click either way.
const DISMISS_ACTION: &str = "com.apple.UNNotificationDismissActionIdentifier";

/// The class of the center's delegate.
const DELEGATE_CLASS: &str = "ReviewdeckNotificationDelegate";

/// One notification, in the terms of Electron's `new Notification({...})`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    /// The request identifier. A notification with the id of one still on screen
    /// replaces it.
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    /// No sound (`!settings.playSound`).
    pub silent: bool,
    /// The review item a click focuses (`deck.emit('focus-item', item.id)`); `None`
    /// only brings the window forward.
    pub target: Option<String>,
}

/// Whether notifications can be used at all: only from an app bundle with an
/// identifier. An unbundled `cargo run` has none, and UNUserNotificationCenter
/// throws an Objective-C exception (aborting the process) when asked for its
/// center there, so it must never be touched. Electron's
/// `Notification.isSupported()`, in effect.
pub fn available() -> bool {
    bundle_identifier().is_some()
}

/// The notification center with Reviewdeck's delegate installed. Main thread only;
/// keep it for the life of the app (the center holds its delegate weakly, and
/// dropping this uninstalls it).
pub struct Notifier {
    center: id,
    delegate: StrongPtr,
}

impl Notifier {
    /// Installs the delegate that reports clicks on `events`, or `None` when
    /// [`available`] is false (or the delegate class cannot be made).
    ///
    /// Call it inside gpui's `Application::run` callback: that runs within
    /// `applicationDidFinishLaunching:`, which is early enough for the click that
    /// launched the app to reach the delegate.
    pub fn new(events: EventSender) -> Option<Notifier> {
        if !available() {
            return None;
        }
        debug_assert!(
            is_main_thread(),
            "notifications are AppKit: main thread only"
        );
        let _pool = AutoreleasePool::new();
        let class = delegate_class()?;
        // SAFETY: main thread; the app has a bundle identifier, so the center exists;
        // the class was declared through `declare_sender`. The center is a process
        // singleton, never deallocated, so keeping its pointer unretained is sound.
        unsafe {
            let delegate = new_with_sender(class, events)?;
            let center: id = msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
            if center == nil {
                return None;
            }
            let _: () = msg_send![center, setDelegate: *delegate];
            Some(Notifier { center, delegate })
        }
    }

    /// Asks once for permission to alert and play sounds. macOS shows its prompt the
    /// first time and answers from the user's choice afterwards, so this runs at
    /// every launch. A refusal is not an error: notifications then simply do not
    /// show, as with Electron.
    pub fn request_authorization(&self) {
        let _pool = AutoreleasePool::new();
        // The completion handler runs on a background queue: it only logs.
        let handler = ConcreteBlock::new(|_granted: BOOL, error: id| {
            // SAFETY: `error` is nil or the NSError the framework passes.
            if let Some(message) = unsafe { error_description(error) } {
                eprintln!("Reviewdeck could not ask to show notifications: {message}");
            }
        })
        .copy();
        let handler: &Block<(BOOL, id), ()> = &handler;
        // SAFETY: `self.center` is the live center; the framework copies the block
        // before returning, so it may be released when `handler` goes out of scope.
        unsafe {
            let _: () = msg_send![self.center,
                requestAuthorizationWithOptions: AUTHORIZE_ALERT | AUTHORIZE_SOUND
                completionHandler: handler];
        }
    }

    /// Delivers `notification`. Delivery failures (notifications turned off for the
    /// app, for one) are silent, as Electron's are.
    pub fn show(&self, notification: &Notification) {
        debug_assert!(
            is_main_thread(),
            "notifications are AppKit: main thread only"
        );
        let _pool = AutoreleasePool::new();
        // SAFETY: main thread, inside a pool; every object is autoreleased or owned by
        // the request, and addNotificationRequest: copies what it needs.
        unsafe {
            let content: id = msg_send![class!(UNMutableNotificationContent), new];
            if content == nil {
                return;
            }
            let content: id = msg_send![content, autorelease];
            let _: () = msg_send![content, setTitle: ns_string(&notification.title)];
            if let Some(subtitle) = &notification.subtitle {
                let _: () = msg_send![content, setSubtitle: ns_string(subtitle)];
            }
            let _: () = msg_send![content, setBody: ns_string(&notification.body)];
            if !notification.silent {
                let sound: id = msg_send![class!(UNNotificationSound), defaultSound];
                let _: () = msg_send![content, setSound: sound];
            }
            if let Some(target) = &notification.target {
                let info: id = msg_send![class!(NSDictionary),
                    dictionaryWithObject: ns_string(target)
                    forKey: ns_string(TARGET_KEY)];
                let _: () = msg_send![content, setUserInfo: info];
            }
            let request: id = msg_send![class!(UNNotificationRequest),
                requestWithIdentifier: ns_string(&notification.id)
                content: content
                trigger: nil];
            if request == nil {
                return;
            }
            let _: () = msg_send![self.center,
                addNotificationRequest: request
                withCompletionHandler: nil];
        }
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        // SAFETY: main thread (Notifier is !Send); only our own delegate is removed.
        unsafe {
            let current: id = msg_send![self.center, delegate];
            if current == *self.delegate {
                let _: () = msg_send![self.center, setDelegate: nil];
            }
        }
    }
}

/// The delegate class, declared once per process.
fn delegate_class() -> Option<&'static Class> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let Some(mut decl) = ClassDecl::new(DELEGATE_CLASS, class!(NSObject)) else {
            return;
        };
        declare_sender(&mut decl);
        if let Some(protocol) = Protocol::get("UNUserNotificationCenterDelegate") {
            decl.add_protocol(protocol);
        }
        // SAFETY: the signatures match the UNUserNotificationCenterDelegate methods
        // (three object arguments, the last a block, returning void).
        unsafe {
            decl.add_method(
                sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
                will_present as extern "C" fn(&Object, Sel, id, id, id),
            );
            decl.add_method(
                sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
                did_receive as extern "C" fn(&Object, Sel, id, id, id),
            );
        }
        decl.register();
    });
    Class::get(DELEGATE_CLASS)
}

/// A notification arriving while the app is frontmost: present it as usual.
extern "C" fn will_present(_this: &Object, _: Sel, _center: id, _notification: id, handler: id) {
    let handler = handler as *mut Block<(u64,), ()>;
    // SAFETY: the framework passes a valid `void (^)(UNNotificationPresentationOptions)`
    // block, which must be called exactly once.
    if let Some(handler) = unsafe { handler.as_ref() } {
        // SAFETY: as above; this is the one call.
        unsafe { handler.call((PRESENT_BANNER | PRESENT_LIST | PRESENT_SOUND,)) };
    }
}

/// A click on one of our notifications: report it, then tell the framework we are
/// done with it.
extern "C" fn did_receive(this: &Object, _: Sel, _center: id, response: id, handler: id) {
    let _pool = AutoreleasePool::new();
    // SAFETY: `this` is our delegate and `response` a UNNotificationResponse; every
    // accessor returns an object owned by it or autoreleased into the pool above.
    unsafe {
        if response != nil {
            let action: id = msg_send![response, actionIdentifier];
            if string_from_ns(action).as_deref() != Some(DISMISS_ACTION) {
                send_event(
                    this,
                    PlatformEvent::NotificationClicked {
                        target: click_target(response),
                    },
                );
            }
        }
    }
    let handler = handler as *mut Block<(), ()>;
    // SAFETY: the framework passes a valid `void (^)(void)` block, to be called once.
    if let Some(handler) = unsafe { handler.as_ref() } {
        // SAFETY: as above; this is the one call.
        unsafe { handler.call(()) };
    }
}

/// The review a clicked notification points at, from its userInfo.
///
/// # Safety
/// `response` must be a valid UNNotificationResponse; inside an autorelease pool.
unsafe fn click_target(response: id) -> Option<String> {
    // SAFETY: each step is nil-checked; objectForKey: on a dictionary of plist values.
    unsafe {
        let notification: id = msg_send![response, notification];
        if notification == nil {
            return None;
        }
        let request: id = msg_send![notification, request];
        if request == nil {
            return None;
        }
        let content: id = msg_send![request, content];
        if content == nil {
            return None;
        }
        let info: id = msg_send![content, userInfo];
        if info == nil {
            return None;
        }
        let is_dictionary: BOOL = msg_send![info, isKindOfClass: class!(NSDictionary)];
        if is_dictionary == NO {
            return None;
        }
        let target: id = msg_send![info, objectForKey: ns_string(TARGET_KEY)];
        string_from_ns(target).filter(|target| !target.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;

    #[test]
    fn unbundled_runs_never_touch_the_center() {
        // The test binary is unbundled: `available` is false and `Notifier::new`
        // returns before asking for UNUserNotificationCenter (which would abort).
        assert!(!available());
        let (events, _receiver) = mpsc::unbounded();
        assert!(Notifier::new(events).is_none());
    }

    #[test]
    fn the_delegate_reports_a_click_with_its_target_and_completes() {
        let _pool = AutoreleasePool::new();
        let Some(class) = delegate_class() else {
            panic!("the delegate class registers");
        };
        let (events, mut receiver) = mpsc::unbounded();
        // SAFETY: the class went through `declare_sender`.
        let Some(delegate) = (unsafe { new_with_sender(class, events) }) else {
            panic!("the delegate instantiates");
        };
        // A response is only ever made by the framework, so stand in for its object
        // graph with a fake answering the same selectors.
        let response = fake_response(
            Some("item-1"),
            "com.apple.UNNotificationDefaultActionIdentifier",
        );
        let completed = std::rc::Rc::new(std::cell::Cell::new(false));
        let done = completed.clone();
        let handler = ConcreteBlock::new(move || done.set(true)).copy();
        let handler: &Block<(), ()> = &handler;
        did_receive(
            // SAFETY: `delegate` is live.
            unsafe { &**delegate },
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            nil,
            *response,
            handler as *const Block<(), ()> as id,
        );
        assert!(completed.get());
        assert_eq!(
            receiver.try_recv().ok(),
            Some(PlatformEvent::NotificationClicked {
                target: Some("item-1".to_owned())
            })
        );

        let response = fake_response(None, "com.apple.UNNotificationDefaultActionIdentifier");
        did_receive(
            // SAFETY: `delegate` is live.
            unsafe { &**delegate },
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            nil,
            *response,
            nil,
        );
        assert_eq!(
            receiver.try_recv().ok(),
            Some(PlatformEvent::NotificationClicked { target: None })
        );

        let response = fake_response(Some("item-2"), DISMISS_ACTION);
        did_receive(
            // SAFETY: `delegate` is live.
            unsafe { &**delegate },
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            nil,
            *response,
            nil,
        );
        assert!(
            matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "a dismissal is not a click"
        );
    }

    #[test]
    fn a_frontmost_notification_is_presented_with_banner_and_sound() {
        let options = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let seen = options.clone();
        let handler = ConcreteBlock::new(move |presented: u64| seen.set(presented)).copy();
        let handler: &Block<(u64,), ()> = &handler;
        let _pool = AutoreleasePool::new();
        let Some(class) = delegate_class() else {
            panic!("the delegate class registers");
        };
        let (events, _receiver) = mpsc::unbounded();
        // SAFETY: the class went through `declare_sender`.
        let Some(delegate) = (unsafe { new_with_sender(class, events) }) else {
            panic!("the delegate instantiates");
        };
        will_present(
            // SAFETY: `delegate` is live.
            unsafe { &**delegate },
            sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
            nil,
            nil,
            handler as *const Block<(u64,), ()> as id,
        );
        assert_eq!(options.get(), PRESENT_BANNER | PRESENT_LIST | PRESENT_SOUND);
    }

    /// A stand-in UNNotificationResponse: `response.notification.request.content`
    /// all answer with the same object, whose `userInfo` holds `target`.
    fn fake_response(target: Option<&str>, action: &str) -> StrongPtr {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| {
            let Some(mut decl) = ClassDecl::new("ReviewdeckFakeResponse", class!(NSObject)) else {
                return;
            };
            decl.add_ivar::<id>("info");
            decl.add_ivar::<id>("action");
            extern "C" fn this(this: &Object, _: Sel) -> id {
                this as *const Object as id
            }
            extern "C" fn info(this: &Object, _: Sel) -> id {
                // SAFETY: the ivar was declared above.
                unsafe { *this.get_ivar::<id>("info") }
            }
            extern "C" fn action(this: &Object, _: Sel) -> id {
                // SAFETY: the ivar was declared above.
                unsafe { *this.get_ivar::<id>("action") }
            }
            // SAFETY: getter signatures.
            unsafe {
                decl.add_method(
                    sel!(notification),
                    this as extern "C" fn(&Object, Sel) -> id,
                );
                decl.add_method(sel!(request), this as extern "C" fn(&Object, Sel) -> id);
                decl.add_method(sel!(content), this as extern "C" fn(&Object, Sel) -> id);
                decl.add_method(sel!(userInfo), info as extern "C" fn(&Object, Sel) -> id);
                decl.add_method(
                    sel!(actionIdentifier),
                    action as extern "C" fn(&Object, Sel) -> id,
                );
            }
            decl.register();
        });
        let Some(class) = Class::get("ReviewdeckFakeResponse") else {
            panic!("the fake response class registers");
        };
        // SAFETY: test-only object graph; the dictionary and strings are kept alive by
        // the caller's autorelease pool for as long as the response is used (the
        // fake does not retain its ivars, and the test leaks them to the pool).
        unsafe {
            let object: id = msg_send![class, new];
            let info: id = match target {
                Some(target) => msg_send![class!(NSDictionary),
                    dictionaryWithObject: ns_string(target)
                    forKey: ns_string(TARGET_KEY)],
                None => msg_send![class!(NSDictionary), dictionary],
            };
            (*object).set_ivar("info", info);
            (*object).set_ivar("action", ns_string(action));
            StrongPtr::new(object)
        }
    }
}
