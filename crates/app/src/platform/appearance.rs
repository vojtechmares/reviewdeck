//! The app-wide light/dark appearance (NSApp.appearance); port of nativeTheme in
//! src/main/index.ts and src/main/ipc.ts.
//!
//! The window's vibrancy and title bar follow the native appearance, not the
//! palette the views draw with, so the theme setting has to reach AppKit too:
//! at launch (`nativeTheme.themeSource = getSettings().theme`) and whenever the
//! setting changes.

use cocoa::base::{id, nil};
use objc::{class, msg_send, sel, sel_impl};
use reviewdeck_core::model::ThemeMode;

use super::{AutoreleasePool, is_main_thread, ns_string};

/// The AppKit appearance name `mode` asks for; `None` follows the system. The
/// values of the NSAppearanceName* constants are their own names.
fn appearance_name(mode: ThemeMode) -> Option<&'static str> {
    match mode {
        ThemeMode::System => None,
        ThemeMode::Light => Some("NSAppearanceNameAqua"),
        ThemeMode::Dark => Some("NSAppearanceNameDarkAqua"),
    }
}

/// Sets `NSApp.appearance`: Aqua, DarkAqua, or nil to follow the system - Electron's
/// `nativeTheme.themeSource`. Main thread only.
///
/// gpui only notices when AppKit redisplays the window, so re-render afterwards
/// (`cx.refresh_windows()`) for `window.appearance()` to read the new value.
pub fn set_app_appearance(mode: ThemeMode) {
    debug_assert!(is_main_thread(), "NSApp is AppKit: main thread only");
    let _pool = AutoreleasePool::new();
    // SAFETY: main-thread AppKit calls; sharedApplication exists (or is created) and
    // appearanceNamed: returns an autoreleased appearance or nil, which the app retains.
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let appearance: id = match appearance_name(mode) {
            None => nil,
            Some(name) => msg_send![class!(NSAppearance), appearanceNamed: ns_string(name)],
        };
        let _: () = msg_send![app, setAppearance: appearance];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_names_its_appearance() {
        assert_eq!(appearance_name(ThemeMode::System), None);
        assert_eq!(
            appearance_name(ThemeMode::Light),
            Some("NSAppearanceNameAqua")
        );
        assert_eq!(
            appearance_name(ThemeMode::Dark),
            Some("NSAppearanceNameDarkAqua")
        );
    }

    #[test]
    fn the_names_are_ones_appkit_knows() {
        let _pool = AutoreleasePool::new();
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let Some(name) = appearance_name(mode) else {
                panic!("{mode:?} names an appearance");
            };
            // SAFETY: NSAppearance lookups are plain class-method calls.
            let appearance: id =
                unsafe { msg_send![class!(NSAppearance), appearanceNamed: ns_string(name)] };
            assert!(appearance != nil, "{name} resolves");
        }
    }
}
