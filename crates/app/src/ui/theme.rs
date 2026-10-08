//! Palette tokens for light and dark (from src/renderer/src/index.css), sizes and
//! fonts.
//!
//! The colours themselves live in `reviewdeck_core::palette`, where the contrast
//! tests can hold them; this module turns them into gpui colours and decides which
//! of the two themes is showing. Every view reads colours through [`ActiveTheme`]
//! (`cx.theme().colors.border`) rather than naming a hex value, so a token changed
//! in the palette changes everywhere at once.

use gpui::{App, Global, Hsla, Pixels, Rems, Window, WindowAppearance, px, rems};
use reviewdeck_core::color::Rgba;
use reviewdeck_core::model::ThemeMode;
use reviewdeck_core::palette::Palette;

/// A length in CSS pixels at the default zoom, as rems (`v / 16`).
///
/// Every length in the UI goes through this, so View > Zoom In / Zoom Out / Actual
/// Size work by changing the window's rem size, the way the browser's zoom scales a
/// page laid out in pixels.
pub fn rpx(v: f32) -> Rems {
    rems(v / 16.)
}

/// The window's rem size at 100% zoom: 16px, the browser default the Tailwind
/// classes were written against.
pub const BASE_REM: Pixels = px(16.);

/// `--font-sans`: the system UI font, which is what `-apple-system` resolves to.
pub const UI_FONT: &str = ".SystemUIFont";
/// `--font-mono`. SF Mono is not installed for apps to use outside Apple's own, so
/// this is the next name in the stack that is always present.
pub const MONO_FONT: &str = "Menlo";

/// `body { font-size: 13.5px }`: the size text takes when nothing says otherwise.
pub const BASE_TEXT: f32 = 13.5;

#[allow(dead_code)] // the whole scale, as index.css defines it
/// Tailwind's radius scale as index.css defines it around `--radius: 0.875rem`.
pub mod radius {
    /// `rounded-sm`: `--radius - 0.5rem`.
    pub const SM: f32 = 6.;
    /// `rounded-md`: `--radius - 0.25rem`.
    pub const MD: f32 = 10.;
    /// `rounded-lg`: `--radius`.
    pub const LG: f32 = 14.;
    /// `rounded-xl`: `--radius + 0.375rem`.
    pub const XL: f32 = 20.;
    /// `rounded-full`.
    pub const FULL: f32 = 9999.;
}

/// Every palette token as a gpui colour, one field per CSS custom property.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub surface: Hsla,
    pub surface_strong: Hsla,
    pub surface_muted: Hsla,
    pub card: Hsla,
    pub card_foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub ring: Hsla,
    pub highlight: Hsla,
    pub overlay: Hsla,
    pub overlay_border: Hsla,
    pub overlay_shadow: Hsla,
    pub scrim: Hsla,
    pub ok: Hsla,
    pub ok_soft: Hsla,
    pub bad: Hsla,
    pub bad_soft: Hsla,
    pub busy: Hsla,
    pub busy_soft: Hsla,
    pub info: Hsla,
    pub info_soft: Hsla,
    pub diff_code_base: Hsla,
    pub diff_add: Hsla,
    pub diff_add_strong: Hsla,
    pub diff_del: Hsla,
    pub diff_del_strong: Hsla,
    pub diff_gutter: Hsla,
}

/// A core colour (0-1 channels) as a gpui one.
pub fn hsla(color: Rgba) -> Hsla {
    gpui::Rgba {
        r: color.r as f32,
        g: color.g as f32,
        b: color.b as f32,
        a: color.a as f32,
    }
    .into()
}

/// A colour packed as `0xRRGGBBAA`, which is how syntax tokens carry theirs.
pub fn packed(rgba: u32) -> Hsla {
    gpui::rgba(rgba).into()
}

impl Colors {
    pub fn from_palette(palette: &Palette) -> Colors {
        Colors {
            background: hsla(palette.background),
            foreground: hsla(palette.foreground),
            surface: hsla(palette.surface),
            surface_strong: hsla(palette.surface_strong),
            surface_muted: hsla(palette.surface_muted),
            card: hsla(palette.card),
            card_foreground: hsla(palette.card_foreground),
            muted: hsla(palette.muted),
            muted_foreground: hsla(palette.muted_foreground),
            accent: hsla(palette.accent),
            accent_foreground: hsla(palette.accent_foreground),
            primary: hsla(palette.primary),
            primary_foreground: hsla(palette.primary_foreground),
            border: hsla(palette.border),
            border_strong: hsla(palette.border_strong),
            ring: hsla(palette.ring),
            highlight: hsla(palette.highlight),
            overlay: hsla(palette.overlay),
            overlay_border: hsla(palette.overlay_border),
            overlay_shadow: hsla(palette.overlay_shadow),
            scrim: hsla(palette.scrim),
            ok: hsla(palette.ok),
            ok_soft: hsla(palette.ok_soft),
            bad: hsla(palette.bad),
            bad_soft: hsla(palette.bad_soft),
            busy: hsla(palette.busy),
            busy_soft: hsla(palette.busy_soft),
            info: hsla(palette.info),
            info_soft: hsla(palette.info_soft),
            diff_code_base: hsla(palette.diff_code_base),
            diff_add: hsla(palette.diff_add),
            diff_add_strong: hsla(palette.diff_add_strong),
            diff_del: hsla(palette.diff_del),
            diff_del_strong: hsla(palette.diff_del_strong),
            diff_gutter: hsla(palette.diff_gutter),
        }
    }
}

/// The theme showing right now: which of the two palettes, already resolved.
///
/// A gpui global, so any view reaches it through [`ActiveTheme`]. It is replaced -
/// never mutated in place - when the setting or the system appearance changes, and
/// the windows are refreshed so everything repaints in the new colours.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub colors: Colors,
}

impl Global for Theme {}

impl Theme {
    pub fn new(dark: bool) -> Theme {
        Theme {
            dark,
            colors: Colors::from_palette(&Palette::for_dark(dark)),
        }
    }

    /// The theme the setting asks for: light or dark outright, or whatever the
    /// system appearance is when it says "system" - the way the renderer put `.dark`
    /// on the root element.
    pub fn resolve(mode: ThemeMode, appearance: WindowAppearance) -> Theme {
        Theme::new(is_dark(mode, appearance))
    }

    /// Installs the theme for `mode` against the system appearance as gpui reports
    /// it, and repaints every window if that changed anything.
    pub fn apply(mode: ThemeMode, cx: &mut App) {
        let next = Theme::resolve(mode, cx.window_appearance());
        if cx.try_global::<Theme>() != Some(&next) {
            cx.set_global(next);
            cx.refresh_windows();
        }
    }
}

/// Whether `mode` comes out dark against `appearance`.
pub fn is_dark(mode: ThemeMode, appearance: WindowAppearance) -> bool {
    match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    }
}

/// `cx.theme()` from anywhere that can reach the app.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        // Installed before the first window opens; a missing one is a wiring bug,
        // and light is a better answer to it than a panic in the middle of a frame.
        static FALLBACK: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        self.try_global::<Theme>()
            .unwrap_or_else(|| FALLBACK.get_or_init(|| Theme::new(false)))
    }
}

/// Sets the window's rem size for a zoom factor (1.0 = Actual Size).
pub fn set_zoom(window: &mut Window, zoom: f32) {
    window.set_rem_size(BASE_REM * zoom);
    window.refresh();
}
