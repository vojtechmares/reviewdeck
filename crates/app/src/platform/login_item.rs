//! Launch at login through SMAppService; port of `app.setLoginItemSettings` in
//! src/main/ipc.ts.
//!
//! `SMAppService.mainAppService` registers the app bundle itself as a login item
//! (macOS 13 and later), which is what Electron does on those systems as well.
//! Electron's `openAsHidden` has no equivalent there, in Electron or here.

use cocoa::base::{id, nil};
use objc::runtime::{BOOL, Class, NO};
use objc::{msg_send, sel, sel_impl};

use super::{AutoreleasePool, bundle_identifier, error_description};

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// Shown when the system predates SMAppService.
const UNSUPPORTED: &str = "Launch at login needs macOS 13 or later.";

/// Shown when running outside an app bundle (`cargo run`), which has nothing to
/// register.
const UNBUNDLED: &str = "Launch at login only works when Reviewdeck runs from its app bundle.";

/// Where the main app stands as a login item (SMAppServiceStatus, plus the cases in
/// which there is no service to ask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginItemStatus {
    /// Not registered.
    NotRegistered,
    /// Registered and allowed: the app opens at login.
    Enabled,
    /// Registered, but the user has to allow it in System Settings > General >
    /// Login Items before it takes effect.
    RequiresApproval,
    /// The system could not find the service (an app outside /Applications can end
    /// up here).
    NotFound,
    /// No SMAppService (before macOS 13), or not running from a bundle.
    Unsupported,
}

impl LoginItemStatus {
    fn from_raw(raw: i64) -> LoginItemStatus {
        match raw {
            0 => LoginItemStatus::NotRegistered,
            1 => LoginItemStatus::Enabled,
            2 => LoginItemStatus::RequiresApproval,
            _ => LoginItemStatus::NotFound,
        }
    }
}

/// `SMAppService.mainAppService`, or the message saying why there is none.
///
/// # Safety
/// Inside an autorelease pool (the service is autoreleased).
unsafe fn main_app_service() -> Result<id, &'static str> {
    let class = Class::get("SMAppService").ok_or(UNSUPPORTED)?;
    if bundle_identifier().is_none() {
        return Err(UNBUNDLED);
    }
    // SAFETY: +mainAppService is a plain class getter on macOS 13+, where the class
    // exists.
    let service: id = unsafe { msg_send![class, mainAppService] };
    if service == nil {
        return Err(UNSUPPORTED);
    }
    Ok(service)
}

/// Where the app stands as a login item.
pub fn status() -> LoginItemStatus {
    let _pool = AutoreleasePool::new();
    // SAFETY: inside a pool; -status is a plain NSInteger getter.
    unsafe {
        match main_app_service() {
            Ok(service) => {
                let raw: i64 = msg_send![service, status];
                LoginItemStatus::from_raw(raw)
            }
            Err(_) => LoginItemStatus::Unsupported,
        }
    }
}

/// Whether the app opens at login now.
pub fn is_enabled() -> bool {
    status() == LoginItemStatus::Enabled
}

/// Registers (`true`) or unregisters (`false`) the app as a login item. Asking for
/// what is already the case succeeds without touching the system. The error is a
/// sentence for the user: the system's own description, or why there is nothing to
/// register. A registration that needs the user's approval in System Settings
/// reports success; the status then reads [`LoginItemStatus::RequiresApproval`].
pub fn set_launch_at_login(enabled: bool) -> Result<(), String> {
    let _pool = AutoreleasePool::new();
    // SAFETY: inside a pool; register/unregisterAndReturnError: take an NSError**
    // out-parameter that is nil or an autoreleased NSError afterwards.
    unsafe {
        let service = main_app_service().map_err(str::to_owned)?;
        let raw: i64 = msg_send![service, status];
        let current = LoginItemStatus::from_raw(raw);
        let mut error: id = nil;
        let ok: BOOL = if enabled {
            if current == LoginItemStatus::Enabled {
                return Ok(());
            }
            msg_send![service, registerAndReturnError: &mut error]
        } else {
            if matches!(
                current,
                LoginItemStatus::NotRegistered | LoginItemStatus::NotFound
            ) {
                return Ok(());
            }
            msg_send![service, unregisterAndReturnError: &mut error]
        };
        if ok != NO {
            return Ok(());
        }
        let raw: i64 = msg_send![service, status];
        if enabled && LoginItemStatus::from_raw(raw) == LoginItemStatus::RequiresApproval {
            return Ok(());
        }
        Err(error_description(error).unwrap_or_else(|| {
            if enabled {
                "Reviewdeck could not be added to your login items.".to_owned()
            } else {
                "Reviewdeck could not be removed from your login items.".to_owned()
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_statuses_map_to_their_meaning() {
        assert_eq!(LoginItemStatus::from_raw(0), LoginItemStatus::NotRegistered);
        assert_eq!(LoginItemStatus::from_raw(1), LoginItemStatus::Enabled);
        assert_eq!(
            LoginItemStatus::from_raw(2),
            LoginItemStatus::RequiresApproval
        );
        assert_eq!(LoginItemStatus::from_raw(3), LoginItemStatus::NotFound);
        assert_eq!(LoginItemStatus::from_raw(42), LoginItemStatus::NotFound);
    }

    #[test]
    fn an_unbundled_run_has_nothing_to_register() {
        // The test binary is unbundled, like `cargo run`: no system call is made.
        assert_eq!(status(), LoginItemStatus::Unsupported);
        assert!(!is_enabled());
        let expected = if Class::get("SMAppService").is_some() {
            UNBUNDLED
        } else {
            UNSUPPORTED
        };
        assert_eq!(set_launch_at_login(true), Err(expected.to_owned()));
        assert_eq!(set_launch_at_login(false), Err(expected.to_owned()));
    }
}
