//! Port of src/renderer/src/components/ui/avatar.tsx.
//!
//! Falls back to initials while the picture loads and when it fails, which matters because
//! self-hosted avatars often 404. The initials are also what shows when there is no URL.

use gpui::{
    AnyElement, App, FontWeight, IntoElement, ObjectFit, ParentElement, RenderOnce, SharedString,
    Styled, StyledImage, Window, div, img,
};

use crate::ui::theme::{ActiveTheme, rpx};

/// `lib/utils.ts` `initials`: up to two letters, from the first and last words of `name`.
pub fn initials(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    match parts.as_slice() {
        [] => "?".to_string(),
        [only] => only.chars().take(2).collect::<String>().to_uppercase(),
        [first, .., last] => {
            let mut out = String::new();
            out.extend(first.chars().take(1));
            out.extend(last.chars().take(1));
            out.to_uppercase()
        }
    }
}

/// A round avatar, `size-6` (24px) by default.
#[derive(IntoElement)]
pub struct Avatar {
    name: SharedString,
    src: Option<SharedString>,
    size: f32,
}

impl Avatar {
    pub fn new(name: impl Into<SharedString>) -> Avatar {
        Avatar {
            name: name.into(),
            src: None,
            size: 24.,
        }
    }

    /// The picture's URL. Without one, or when it fails, the initials show.
    pub fn src(mut self, url: impl Into<SharedString>) -> Avatar {
        self.src = Some(url.into());
        self
    }

    /// Side length in CSS pixels.
    pub fn size(mut self, css_px: f32) -> Avatar {
        self.size = css_px;
        self
    }
}

/// The initials as the disc's content: `text-[9.5px] font-semibold text-muted-foreground`.
fn initials_text(name: &str) -> AnyElement {
    div().child(initials(name)).into_any_element()
}

/// The disc is `inline-flex size-6 shrink-0 items-center justify-center overflow-hidden
/// rounded-full border border-border bg-muted`, and the picture inside it is `size-full
/// object-cover`. The picture carries its own `rounded-full` because gpui's `overflow_hidden`
/// clips to a rectangle (inside the border), not to the rounded shape, so a square image
/// would poke out of the circle's corners. While the picture loads the disc is empty, as the
/// `<img>` is in the TSX, and a failed load shows the initials. The TSX `title={name}` hover
/// hint is not reproduced: the avatar has no element id to hang a tooltip on, and two
/// avatars of the same person would share one.
impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let name = self.name.to_string();
        let disc = div()
            .size(rpx(self.size))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded_full()
            .border_1()
            .border_color(colors.border)
            .bg(colors.muted)
            .text_color(colors.muted_foreground)
            .text_size(rpx(9.5))
            .font_weight(FontWeight::SEMIBOLD);
        match self.src {
            Some(url) => {
                let fallback_name = name.clone();
                disc.child(
                    img(url)
                        .size_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| div().into_any_element())
                        .with_fallback(move || initials_text(&fallback_name)),
                )
            }
            None => disc.child(initials(&name)),
        }
    }
}
