//! Reviewdeck's core: everything the app does that is not drawing.
//!
//! Pure logic, networking and persistence, with no dependency on gpui, so all of it
//! can be tested without a window. The TypeScript app this replaces is the
//! specification; each module names the file it ports.

pub mod agent_prompt;
pub mod autolink;
pub mod color;
pub mod deck_cache;
pub mod demo;
pub mod diff;
pub mod drafts;
pub mod error;
pub mod highlight;
pub mod http;
pub mod images;
pub mod keychain;
pub mod markdown;
pub mod model;
pub mod providers;
pub mod review_window;
pub mod store;
pub mod threads;
pub mod time;
pub mod token_url;
pub mod tray_icon;

pub use error::{Error, Result, msg};
