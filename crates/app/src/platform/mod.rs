//! macOS glue gpui does not provide: the menu bar item, notifications, launch at
//! login, the app-wide appearance and the single-writer lock.

pub mod appearance;
pub mod instance;
pub mod login_item;
pub mod notify;
pub mod tray;
