//! The `Loader2` spinner with the `.spin` class: one turn every 900ms, linear.

use gpui::{App, Hsla, IntoElement, RenderOnce, Window};

use crate::ui::icons::{Icon, IconName};

/// A spinning loader, 16px by default.
#[derive(IntoElement)]
pub struct Spinner {
    size: f32,
    color: Option<Hsla>,
}

impl Spinner {
    pub fn new() -> Spinner {
        Spinner {
            size: 16.,
            color: None,
        }
    }

    /// Side length in CSS pixels.
    pub fn size(mut self, css_px: f32) -> Spinner {
        self.size = css_px;
        self
    }

    /// Defaults to the theme's foreground.
    pub fn color(mut self, color: Hsla) -> Spinner {
        self.color = Some(color);
        self
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Spinner::new()
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon = Icon::new(IconName::Loader2).size(self.size).spin();
        match self.color {
            Some(color) => icon.color(color),
            None => icon,
        }
    }
}
