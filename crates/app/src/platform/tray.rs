//! The menu bar item (NSStatusItem); port of the tray half of src/main/index.ts.
//!
//! The app decides what the item says - [`TrayMenu::new`] and [`tray_title`] carry
//! the wording of index.ts - and [`Tray`] only draws it. Clicking the icon opens the
//! menu, as an Electron tray with a context menu does on macOS; the menu's actions
//! arrive as [`PlatformEvent`]s.

use std::sync::Once;

use cocoa::base::{id, nil};
use cocoa::foundation::NSSize;
use objc::declare::ClassDecl;
use objc::rc::StrongPtr;
use objc::runtime::{BOOL, Class, NO, Object, Sel, YES};
use objc::{class, msg_send, sel, sel_impl};

use super::{
    AutoreleasePool, EventSender, PlatformEvent, declare_sender, is_main_thread, new_with_sender,
    ns_string, send_event, string_from_ns,
};

/// The tooltip index.ts gives the tray.
const TOOLTIP: &str = "Reviewdeck";

/// NSVariableStatusItemLength: the item is as wide as its icon and title.
const VARIABLE_LENGTH: f64 = -1.0;

/// NSCellImagePosition values for the status item's button.
const IMAGE_ONLY: u64 = 1;
const IMAGE_LEFT: u64 = 2;

/// The class whose instances are the menu items' target.
const TARGET_CLASS: &str = "ReviewdeckTrayTarget";

/// What the menu bar menu says above its actions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayMenu {
    /// The disabled first line: how much is waiting, and whether it is being quiet.
    pub header: String,
    /// The disabled second line, present only while some account fails to sync.
    pub failing: Option<String>,
}

impl TrayMenu {
    /// The lines index.ts writes for `waiting` visible reviews, `failing` accounts
    /// that did not sync, and the quiet stretch `quiet_until` (the formatted end of
    /// it, from `core::review_window::quiet_until`).
    ///
    /// `waiting` is the number of reviews the window lists, not every review the
    /// deck holds: a menu bar that says three while the app shows one is the menu
    /// bar being wrong.
    ///
    /// The count keeps climbing through a quiet stretch - only the interrupt channel
    /// goes quiet, never the ambient one - so this line is the only place that can
    /// say the silence was asked for. With notifications off there is nothing to
    /// promise (saying they resume at noon would be a lie when nothing fires then),
    /// so the caller passes `None` then.
    pub fn new(waiting: usize, failing: usize, quiet_until: Option<&str>) -> TrayMenu {
        let mut header = if waiting > 0 {
            format!(
                "{waiting} review{} waiting",
                if waiting == 1 { "" } else { "s" }
            )
        } else {
            "Nothing waiting on you".to_owned()
        };
        if let Some(quiet) = quiet_until.filter(|quiet| !quiet.is_empty()) {
            header.push_str(&format!(" · quiet until {quiet}"));
        }
        TrayMenu {
            header,
            failing: (failing > 0).then(|| format!("{failing} account(s) failing to sync")),
        }
    }
}

/// The text beside the menu bar icon.
///
/// Empty rather than a space when the count is off, or when nothing is waiting:
/// the icon alone is the resting state, and it sits where it would if nothing were
/// ever drawn beside it. The menu still counts. The leading space is the gap
/// between the icon and the number.
pub fn tray_title(show_menu_bar_count: bool, waiting: usize) -> String {
    if show_menu_bar_count && waiting > 0 {
        format!(" {waiting}")
    } else {
        String::new()
    }
}

/// The menu bar item. Lives on the main thread (it holds AppKit objects, so it is
/// neither `Send` nor `Sync`); dropping it removes the item from the menu bar.
pub struct Tray {
    item: StrongPtr,
    /// The target of the menu items. NSMenuItem does not retain its target, so the
    /// tray does; it also owns the event sender.
    target: StrongPtr,
    /// Whether the icon could be built. Without it an empty title would leave the
    /// item zero-width and unclickable, so the name stands in.
    has_image: bool,
    title: Option<String>,
    menu: Option<TrayMenu>,
}

impl Tray {
    /// Adds the item to the menu bar: the template icon from the two PNG renderings
    /// (`core::tray_icon::tray_icon_png(TRAY_ICON_POINTS)` and `(TRAY_ICON_POINTS * 2)`),
    /// the "Reviewdeck" tooltip, no title and no menu yet.
    ///
    /// Both scale factors are attached rather than one image resized: the glyph is
    /// drawn to fit whole pixels at each size, and letting macOS scale a 16px bitmap
    /// onto a Retina menu bar would throw that away. The image is a template, so it
    /// adapts to the menu bar's theme.
    ///
    /// Main thread only. Fails only if the Objective-C target class cannot be made.
    pub fn new(events: EventSender, icon_1x: &[u8], icon_2x: &[u8]) -> Result<Tray, String> {
        debug_assert!(is_main_thread(), "the tray is AppKit: main thread only");
        let _pool = AutoreleasePool::new();
        let class = target_class().ok_or("Reviewdeck could not set up its menu bar item.")?;
        // SAFETY: main thread (asserted above, required of the caller); the class
        // was declared through `declare_sender`.
        let target = unsafe { new_with_sender(class, events) }
            .ok_or("Reviewdeck could not set up its menu bar item.")?;
        // SAFETY: AppKit calls on the main thread. statusItemWithLength: returns an
        // object the status bar does not keep alive for us, so it is retained here
        // and removed + released in Drop.
        unsafe {
            let bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
            let item: id = msg_send![bar, statusItemWithLength: VARIABLE_LENGTH];
            if item == nil {
                return Err("Reviewdeck could not set up its menu bar item.".to_owned());
            }
            let item = StrongPtr::retain(item);
            let button: id = msg_send![*item, button];
            let image = template_image(icon_1x, icon_2x);
            if button != nil {
                if let Some(image) = image {
                    let _: () = msg_send![button, setImage: image];
                }
                let _: () = msg_send![button, setToolTip: ns_string(TOOLTIP)];
            }
            let mut tray = Tray {
                item,
                target,
                has_image: image.is_some(),
                title: None,
                menu: None,
            };
            tray.set_title("");
            Ok(tray)
        }
    }

    /// The text beside the icon; see [`tray_title`]. Main thread only.
    pub fn set_title(&mut self, title: &str) {
        debug_assert!(is_main_thread(), "the tray is AppKit: main thread only");
        if self.title.as_deref() == Some(title) {
            return;
        }
        self.title = Some(title.to_owned());
        let shown = if self.has_image {
            title.to_owned()
        } else {
            format!("{TOOLTIP}{title}")
        };
        let _pool = AutoreleasePool::new();
        // SAFETY: AppKit calls on the main thread; `self.item` is a live status item.
        unsafe {
            let button: id = msg_send![*self.item, button];
            if button == nil {
                return;
            }
            let position = if shown.is_empty() {
                IMAGE_ONLY
            } else {
                IMAGE_LEFT
            };
            let _: () = msg_send![button, setImagePosition: position];
            let _: () = msg_send![button, setTitle: ns_string(&shown)];
        }
    }

    /// Rebuilds the menu exactly as index.ts does: the disabled header, the optional
    /// disabled failing line, a separator, "Open Reviewdeck", "Refresh now", a
    /// separator and "Quit". Unchanged content leaves the open menu alone. Main
    /// thread only.
    pub fn set_menu(&mut self, menu: TrayMenu) {
        debug_assert!(is_main_thread(), "the tray is AppKit: main thread only");
        if self.menu.as_ref() == Some(&menu) {
            return;
        }
        let _pool = AutoreleasePool::new();
        // SAFETY: AppKit calls on the main thread; the status item retains the menu
        // it is given, and the target outlives the menu items (see `Drop`).
        unsafe {
            let ns_menu = build_menu(&menu, *self.target);
            let _: () = msg_send![*self.item, setMenu: ns_menu];
        }
        self.menu = Some(menu);
    }

    /// The live NSMenu read back from AppKit, one string per line: the title,
    /// `(title)` for a disabled line, `---` for a separator. For smoke checks of the
    /// real menu bar item, not for the app's own logic. Main thread only.
    pub fn menu_lines(&self) -> Vec<String> {
        debug_assert!(is_main_thread(), "the tray is AppKit: main thread only");
        let _pool = AutoreleasePool::new();
        let mut lines = Vec::new();
        // SAFETY: AppKit getters on the main thread; the menu and its items are owned
        // by the status item for the duration of the loop.
        unsafe {
            let ns_menu: id = msg_send![*self.item, menu];
            if ns_menu == nil {
                return lines;
            }
            let count: i64 = msg_send![ns_menu, numberOfItems];
            for index in 0..count {
                let item: id = msg_send![ns_menu, itemAtIndex: index];
                let separator: BOOL = msg_send![item, isSeparatorItem];
                let enabled: BOOL = msg_send![item, isEnabled];
                let title: id = msg_send![item, title];
                let title = string_from_ns(title).unwrap_or_default();
                lines.push(if separator != NO {
                    "---".to_owned()
                } else if enabled == NO {
                    format!("({title})")
                } else {
                    title
                });
            }
        }
        lines
    }

    /// Performs the enabled menu line titled `title` as if it were clicked, so its
    /// [`PlatformEvent`] goes out. Returns whether there was such a line. For smoke
    /// checks, like [`Tray::menu_lines`]. Main thread only.
    pub fn click_menu_item(&self, title: &str) -> bool {
        let Some(index) = self.menu_lines().iter().position(|line| line == title) else {
            return false;
        };
        let _pool = AutoreleasePool::new();
        // SAFETY: main thread; `index` is in range of the menu read just above.
        unsafe {
            let ns_menu: id = msg_send![*self.item, menu];
            let item: id = msg_send![ns_menu, itemAtIndex: index as i64];
            let enabled: BOOL = msg_send![item, isEnabled];
            let separator: BOOL = msg_send![item, isSeparatorItem];
            if enabled == NO || separator != NO {
                return false;
            }
            let _: () = msg_send![ns_menu, performActionForItemAtIndex: index as i64];
        }
        true
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _pool = AutoreleasePool::new();
        // SAFETY: AppKit calls on the main thread (Tray is !Send). The menu goes
        // first, so no item is left pointing at the target the StrongPtr releases.
        unsafe {
            let _: () = msg_send![*self.item, setMenu: nil];
            let bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
            let _: () = msg_send![bar, removeStatusItem: *self.item];
        }
    }
}

/// The menu bar target class, declared once per process.
fn target_class() -> Option<&'static Class> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let Some(mut decl) = ClassDecl::new(TARGET_CLASS, class!(NSObject)) else {
            return;
        };
        declare_sender(&mut decl);
        // SAFETY: each signature matches an action method (`-(void)name:(id)sender`).
        unsafe {
            decl.add_method(
                sel!(openWindow:),
                open_window as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(sel!(refresh:), refresh as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(quit:), quit as extern "C" fn(&Object, Sel, id));
        }
        decl.register();
    });
    Class::get(TARGET_CLASS)
}

extern "C" fn open_window(this: &Object, _: Sel, _sender: id) {
    // SAFETY: only instances of the target class receive this action.
    unsafe { send_event(this, PlatformEvent::OpenWindow) }
}

extern "C" fn refresh(this: &Object, _: Sel, _sender: id) {
    // SAFETY: only instances of the target class receive this action.
    unsafe { send_event(this, PlatformEvent::Refresh) }
}

extern "C" fn quit(this: &Object, _: Sel, _sender: id) {
    // SAFETY: only instances of the target class receive this action.
    unsafe { send_event(this, PlatformEvent::Quit) }
}

/// One line of the menu as index.ts lists it.
enum Entry<'a> {
    /// A line that only informs.
    Disabled(&'a str),
    Separator,
    /// A clickable line: label, action and key equivalent (Cmd + key while the menu
    /// is open).
    Action(&'a str, Sel, &'a str),
}

/// The menu's lines in order. Quit carries Cmd+Q because Electron gives the `quit`
/// role its default accelerator in a tray menu too.
fn entries(menu: &TrayMenu) -> Vec<Entry<'_>> {
    let mut entries = vec![Entry::Disabled(&menu.header)];
    if let Some(failing) = &menu.failing {
        entries.push(Entry::Disabled(failing));
    }
    entries.extend([
        Entry::Separator,
        Entry::Action("Open Reviewdeck", sel!(openWindow:), ""),
        Entry::Action("Refresh now", sel!(refresh:), ""),
        Entry::Separator,
        Entry::Action("Quit", sel!(quit:), "q"),
    ]);
    entries
}

/// An autoreleased NSMenu for `menu`, its actions aimed at `target`.
///
/// # Safety
/// Main thread, inside an autorelease pool; `target` an instance of the target class.
unsafe fn build_menu(menu: &TrayMenu, target: id) -> id {
    // SAFETY: plain AppKit object construction; alloc/init results are handed to
    // the pool, and addItem: retains each item.
    unsafe {
        let ns_menu: id = msg_send![class!(NSMenu), alloc];
        let ns_menu: id = msg_send![ns_menu, initWithTitle: ns_string("")];
        let ns_menu: id = msg_send![ns_menu, autorelease];
        // Enabled states are set by hand: the informational lines stay grey and the
        // actions stay live whatever AppKit's validation would infer.
        let _: () = msg_send![ns_menu, setAutoenablesItems: NO];
        for entry in entries(menu) {
            let item: id = match entry {
                Entry::Separator => msg_send![class!(NSMenuItem), separatorItem],
                Entry::Disabled(label) => {
                    let item = menu_item(label, None, "");
                    let _: () = msg_send![item, setEnabled: NO];
                    item
                }
                Entry::Action(label, action, key) => {
                    let item = menu_item(label, Some(action), key);
                    let _: () = msg_send![item, setTarget: target];
                    let _: () = msg_send![item, setEnabled: YES];
                    item
                }
            };
            let _: () = msg_send![ns_menu, addItem: item];
        }
        ns_menu
    }
}

/// An autoreleased NSMenuItem.
///
/// # Safety
/// Main thread, inside an autorelease pool.
unsafe fn menu_item(label: &str, action: Option<Sel>, key: &str) -> id {
    // SAFETY: a null selector is the documented "no action" value of NSMenuItem.
    unsafe {
        let action = action.unwrap_or_else(|| Sel::from_ptr(std::ptr::null()));
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let item: id = msg_send![item,
            initWithTitle: ns_string(label)
            action: action
            keyEquivalent: ns_string(key)];
        msg_send![item, autorelease]
    }
}

/// An autoreleased template NSImage with one representation per decodable PNG, or
/// `None` when neither decodes.
///
/// The image's size in points is the 1x rendering's size in pixels (half the 2x
/// one's when only that decodes); every representation is told that size, which is
/// how AppKit learns which one is the Retina rendering.
///
/// # Safety
/// Main thread, inside an autorelease pool.
unsafe fn template_image(icon_1x: &[u8], icon_2x: &[u8]) -> Option<id> {
    // SAFETY: NSData copies the bytes; imageRepWithData: returns nil for anything it
    // cannot decode, and every object made here is autoreleased or owned by the image.
    unsafe {
        let mut reps = Vec::new();
        for (bytes, scale) in [(icon_1x, 1.0), (icon_2x, 2.0)] {
            if bytes.is_empty() {
                continue;
            }
            let data: id = msg_send![class!(NSData),
                dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void
                length: bytes.len()];
            if data == nil {
                continue;
            }
            let rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: data];
            if rep == nil {
                continue;
            }
            let wide: i64 = msg_send![rep, pixelsWide];
            let high: i64 = msg_send![rep, pixelsHigh];
            if wide <= 0 || high <= 0 {
                continue;
            }
            reps.push((rep, NSSize::new(wide as f64 / scale, high as f64 / scale)));
        }
        let (_, points) = *reps.first()?;
        let image: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![image, initWithSize: points];
        let image: id = msg_send![image, autorelease];
        for (rep, _) in reps {
            let _: () = msg_send![rep, setSize: points];
            let _: () = msg_send![image, addRepresentation: rep];
        }
        let _: () = msg_send![image, setTemplate: YES];
        Some(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_counts_what_is_waiting() {
        assert_eq!(TrayMenu::new(0, 0, None).header, "Nothing waiting on you");
        assert_eq!(TrayMenu::new(1, 0, None).header, "1 review waiting");
        assert_eq!(TrayMenu::new(3, 0, None).header, "3 reviews waiting");
    }

    #[test]
    fn the_header_says_when_the_quiet_was_asked_for() {
        assert_eq!(
            TrayMenu::new(2, 0, Some("09:00")).header,
            "2 reviews waiting · quiet until 09:00"
        );
        assert_eq!(
            TrayMenu::new(0, 0, Some("Mon 09:00")).header,
            "Nothing waiting on you · quiet until Mon 09:00"
        );
        // An empty string is falsy in the TS: no suffix.
        assert_eq!(TrayMenu::new(1, 0, Some("")).header, "1 review waiting");
    }

    #[test]
    fn the_failing_line_appears_only_while_an_account_fails() {
        assert_eq!(TrayMenu::new(1, 0, None).failing, None);
        assert_eq!(
            TrayMenu::new(1, 1, None).failing.as_deref(),
            Some("1 account(s) failing to sync")
        );
        assert_eq!(
            TrayMenu::new(0, 2, None).failing.as_deref(),
            Some("2 account(s) failing to sync")
        );
    }

    #[test]
    fn the_title_is_empty_unless_counting_something() {
        assert_eq!(tray_title(true, 0), "");
        assert_eq!(tray_title(false, 4), "");
        assert_eq!(tray_title(true, 4), " 4");
    }

    #[test]
    fn the_menu_lists_the_lines_of_index_ts_in_order() {
        let describe = |menu: &TrayMenu| -> Vec<String> {
            entries(menu)
                .iter()
                .map(|entry| match entry {
                    Entry::Disabled(label) => format!("({label})"),
                    Entry::Separator => "---".to_owned(),
                    Entry::Action(label, _, "") => (*label).to_owned(),
                    Entry::Action(label, _, key) => format!("{label} [cmd-{key}]"),
                })
                .collect()
        };
        assert_eq!(
            describe(&TrayMenu::new(2, 1, None)),
            [
                "(2 reviews waiting)",
                "(1 account(s) failing to sync)",
                "---",
                "Open Reviewdeck",
                "Refresh now",
                "---",
                "Quit [cmd-q]",
            ]
        );
        assert_eq!(
            describe(&TrayMenu::new(0, 0, None)),
            [
                "(Nothing waiting on you)",
                "---",
                "Open Reviewdeck",
                "Refresh now",
                "---",
                "Quit [cmd-q]",
            ]
        );
    }

    #[test]
    fn the_actions_map_to_their_events() {
        let actions: Vec<Sel> = entries(&TrayMenu::default())
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action(_, action, _) => Some(*action),
                _ => None,
            })
            .collect();
        assert_eq!(actions, [sel!(openWindow:), sel!(refresh:), sel!(quit:)]);
    }

    #[test]
    fn an_undecodable_icon_gives_no_image() {
        let _pool = AutoreleasePool::new();
        // SAFETY: a pool is in place; NSImage/NSBitmapImageRep work off the main
        // thread for decoding.
        unsafe {
            assert!(template_image(&[], &[]).is_none());
            assert!(template_image(b"not a png", b"\x89PNG").is_none());
        }
    }
}
