//! Palette tokens for light and dark (from src/renderer/src/index.css), sizes and
//! fonts.

use gpui::{Rems, rems};

/// A length in CSS pixels at the default zoom, as rems (`v / 16`).
///
/// Every length in the UI goes through this, so View > Zoom In / Zoom Out / Actual
/// Size work by changing the window's rem size, the way the browser's zoom scales a
/// page laid out in pixels.
pub fn rpx(v: f32) -> Rems {
    rems(v / 16.)
}
