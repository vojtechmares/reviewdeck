//! The single-writer lock on the data directory; port of
//! `app.requestSingleInstanceLock()` in src/main/index.ts.
//!
//! One instance only. A second launch from Finder or the Dock never gets this far
//! (LaunchServices focuses the running app), so the lock is about the vault: two
//! processes writing the same file would each overwrite the other's accounts,
//! drafts and seen ids. A second process that cannot take it exits.
//!
//! The lock is `flock(LOCK_EX | LOCK_NB)` (std's `File::try_lock` on macOS), held
//! for as long as the returned `File` lives. The kernel releases it when the
//! process ends, however it ends, so a crash never leaves a stale lock behind.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Once;

use cocoa::base::{id, nil};
use objc::declare::ClassDecl;
use objc::rc::StrongPtr;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use super::{
    AutoreleasePool, EventSender, PlatformEvent, declare_sender, is_main_thread, new_with_sender,
    ns_string, send_event,
};

/// The lock file's name inside the data directory.
pub const LOCK_FILE_NAME: &str = ".reviewdeck.lock";

/// Takes the lock on `<data_dir>/.reviewdeck.lock`, creating the directory and the
/// file as needed. `Ok(Some(file))` holds it until `file` is dropped (keep it for
/// the life of the app); `Ok(None)` means another process holds it. `Err` means the
/// lock could not even be tried (an unwritable data directory, say) - the vault
/// will fail the same way, so the caller decides whether to carry on.
pub fn acquire_single_instance_lock(data_dir: &Path) -> io::Result<Option<File>> {
    std::fs::create_dir_all(data_dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(data_dir.join(LOCK_FILE_NAME))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// The distributed notification a second launch posts to the first.
const SECOND_LAUNCH: &str = "cz.mares.reviewdeck.second-launch";

/// The class of the observer that turns that notification into an event.
const OBSERVER_CLASS: &str = "ReviewdeckSecondLaunchObserver";

/// Tells the running instance that someone tried to start another one - the
/// TypeScript's `app.on('second-instance', show)` - so it brings its window forward.
///
/// Launching from Finder or the Dock never needs this (LaunchServices activates the
/// running app and reports a reopen), but running the binary again does, and so
/// does a login item racing a manual start. The notification carries the data
/// directory as its object, so instances on different data directories (a dev build
/// beside the real app) never wake each other. Fire and forget.
pub fn announce_second_launch(data_dir: &Path) {
    let _pool = AutoreleasePool::new();
    // SAFETY: NSDistributedNotificationCenter is thread-safe to post to; the strings
    // are autoreleased into the pool above.
    unsafe {
        let center: id = msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
        if center == nil {
            return;
        }
        let _: () = msg_send![center,
            postNotificationName: ns_string(SECOND_LAUNCH)
            object: ns_string(&data_dir.to_string_lossy())
            userInfo: nil
            deliverImmediately: true];
    }
}

/// Listens for [`announce_second_launch`] on this data directory and reports each as
/// [`PlatformEvent::OpenWindow`]. Dropping it stops listening. Main thread only.
pub struct SecondLaunchWatcher {
    observer: StrongPtr,
}

impl SecondLaunchWatcher {
    pub fn new(data_dir: &Path, events: EventSender) -> Option<SecondLaunchWatcher> {
        debug_assert!(is_main_thread(), "notification observers: main thread only");
        let class = observer_class()?;
        let _pool = AutoreleasePool::new();
        // SAFETY: main thread; the class was declared through `declare_sender`. The
        // center does not retain its observer, so the watcher keeps it alive and
        // removes it in Drop.
        unsafe {
            let observer = new_with_sender(class, events)?;
            let center: id = msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
            if center == nil {
                return None;
            }
            let _: () = msg_send![center,
                addObserver: *observer
                selector: sel!(secondLaunch:)
                name: ns_string(SECOND_LAUNCH)
                object: ns_string(&data_dir.to_string_lossy())];
            Some(SecondLaunchWatcher { observer })
        }
    }
}

impl Drop for SecondLaunchWatcher {
    fn drop(&mut self) {
        // SAFETY: main thread (the watcher is !Send); removing an observer that was
        // added is always valid.
        unsafe {
            let center: id = msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
            if center != nil {
                let _: () = msg_send![center, removeObserver: *self.observer];
            }
        }
    }
}

fn observer_class() -> Option<&'static Class> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let Some(mut decl) = ClassDecl::new(OBSERVER_CLASS, class!(NSObject)) else {
            return;
        };
        declare_sender(&mut decl);
        // SAFETY: the signature matches `-(void)secondLaunch:(NSNotification *)`.
        unsafe {
            decl.add_method(
                sel!(secondLaunch:),
                second_launch as extern "C" fn(&Object, Sel, id),
            );
        }
        decl.register();
    });
    Class::get(OBSERVER_CLASS)
}

extern "C" fn second_launch(this: &Object, _: Sel, _notification: id) {
    // SAFETY: only instances of the observer class receive this selector.
    unsafe { send_event(this, PlatformEvent::OpenWindow) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_second_launch_becomes_an_open_window_event() {
        let Some(class) = observer_class() else {
            panic!("the observer class registers");
        };
        let (events, mut receiver) = futures::channel::mpsc::unbounded();
        // SAFETY: the class went through `declare_sender`.
        let Some(observer) = (unsafe { new_with_sender(class, events) }) else {
            panic!("the observer instantiates");
        };
        second_launch(
            // SAFETY: `observer` is live.
            unsafe { &**observer },
            sel!(secondLaunch:),
            nil,
        );
        assert_eq!(receiver.try_recv().ok(), Some(PlatformEvent::OpenWindow));
    }

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir()
                .join(format!("reviewdeck-instance-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_one_holder_at_a_time() {
        let dir = TempDir::new("exclusive");
        // The directory does not exist yet: it is created.
        let first = acquire_single_instance_lock(&dir.0).ok().flatten();
        assert!(first.is_some(), "the first instance takes the lock");
        assert!(dir.0.join(LOCK_FILE_NAME).is_file());

        // flock locks belong to the open file, so a second open in this same process
        // contends exactly as a second process would.
        let second = acquire_single_instance_lock(&dir.0).ok();
        assert!(
            matches!(second, Some(None)),
            "the second instance is refused"
        );

        drop(first);
        let third = acquire_single_instance_lock(&dir.0).ok().flatten();
        assert!(third.is_some(), "the lock is free once the holder lets go");
    }

    #[test]
    fn an_unusable_data_dir_is_an_error_not_a_refusal() {
        let dir = TempDir::new("blocked");
        // A file where the directory should be.
        let _ = std::fs::write(&dir.0, b"");
        assert!(acquire_single_instance_lock(&dir.0).is_err());
        let _ = std::fs::remove_file(&dir.0);
    }
}
