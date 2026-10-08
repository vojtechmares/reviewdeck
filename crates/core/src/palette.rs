//! The app palette: the light and dark colour tokens of src/renderer/src/index.css,
//! as data.
//!
//! Milky glass: translucent enough to feel like frosted glass over the desktop,
//! opaque enough that text stays crisp. Surfaces are layered white/graphite films
//! rather than tinted colour, so the palette stays neutral, and colour is reserved
//! for status.
//!
//! Every value is the exact string the stylesheet wrote, so the contrast tests in
//! `color` measure the palette as it ships and nothing restates it. [`Palette`]
//! resolves them once into [`Rgba`] for the UI to map onto its own colour type.

use crate::color::{Rgba, parse_color};

/// Which block of index.css a token comes from: `:root` or `.dark`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeName {
    Light,
    Dark,
}

/// `--radius`, the one token of the light block that is not a colour. It is the
/// same in both themes (the dark block does not override it).
pub const RADIUS: &str = "0.875rem";

/// The `:root` block, in the order the stylesheet lists it.
pub const LIGHT: &[(&str, &str)] = &[
    ("background", "oklch(0.97 0.003 265 / 0.86)"),
    ("foreground", "oklch(0.23 0.008 265)"),
    ("surface", "oklch(1 0 0 / 0.72)"),
    ("surface-strong", "oklch(1 0 0 / 0.9)"),
    ("surface-muted", "oklch(0.99 0.002 265 / 0.42)"),
    ("card", "oklch(1 0 0 / 0.7)"),
    ("card-foreground", "oklch(0.23 0.008 265)"),
    ("muted", "oklch(0.93 0.004 265 / 0.55)"),
    ("muted-foreground", "oklch(0.5 0.012 265)"),
    ("accent", "oklch(0.9 0.005 265 / 0.7)"),
    ("accent-foreground", "oklch(0.24 0.01 265)"),
    ("primary", "oklch(0.32 0.012 265)"),
    ("primary-foreground", "oklch(0.98 0.002 265)"),
    ("border", "oklch(0.35 0.01 265 / 0.12)"),
    ("border-strong", "oklch(0.35 0.01 265 / 0.2)"),
    ("ring", "oklch(0.55 0.02 265 / 0.45)"),
    ("highlight", "oklch(1 0 0 / 0.6)"),
    // Floating layers ride over the app's own UI, so they need a far thicker film.
    ("overlay", "oklch(0.985 0.004 265 / 0.95)"),
    ("overlay-border", "oklch(0.35 0.01 265 / 0.16)"),
    ("overlay-shadow", "oklch(0.2 0.02 265 / 0.3)"),
    ("scrim", "oklch(0.32 0.012 265 / 0.38)"),
    ("ok", "oklch(0.62 0.14 155)"),
    ("ok-soft", "oklch(0.62 0.14 155 / 0.14)"),
    ("bad", "oklch(0.58 0.19 25)"),
    ("bad-soft", "oklch(0.58 0.19 25 / 0.14)"),
    ("busy", "oklch(0.72 0.14 75)"),
    ("busy-soft", "oklch(0.72 0.14 75 / 0.16)"),
    ("info", "oklch(0.58 0.13 250)"),
    ("info-soft", "oklch(0.58 0.13 250 / 0.14)"),
    // Behind the diff's code columns, and nowhere else. Every other surface stays
    // as thin as it was, so the window still reads as frosted glass - but a syntax
    // token's contrast cannot depend on a wallpaper the app can neither see nor
    // control, and this is the film that bounds the desktop's contribution.
    //
    // The colour is the theme's own editor background (`#ffffff` here, `#0d1117` in
    // dark). It started as the graphite the rest of the palette is mixed from, and
    // moved: the theme's contrast is guaranteed against its own background, and at
    // this bar there is no margin to spend on a tint of ours.
    //
    // The alpha is derived rather than chosen - 0.92 is the lowest hundredth at
    // which the legibility test in `color` still passes over both a pure-white and
    // a pure-black desktop, and dark is the side that sets it. This value and the
    // two row tints below are what that test holds; none of them is free to be
    // nudged by eye.
    ("diff-code-base", "oklch(1 0 0 / 0.92)"),
    // Fainter than they were, and for a measured reason: `github-light-default`
    // clears 4.5:1 by only 5.05:1 at its worst meaning-carrying token, so anything
    // that darkens the row behind that token spends margin the theme has not got.
    // These sit at the middle of the window between the contrast bar above and the
    // 1.10:1 row separation below - a window about two percent wide.
    ("diff-add", "oklch(0.85 0.09 150 / 0.25)"),
    ("diff-add-strong", "oklch(0.8 0.13 150 / 0.55)"),
    ("diff-del", "oklch(0.85 0.09 25 / 0.21)"),
    ("diff-del-strong", "oklch(0.8 0.13 25 / 0.5)"),
    ("diff-gutter", "oklch(0.5 0.01 265 / 0.55)"),
];

/// The `.dark` block, in the order the stylesheet lists it.
pub const DARK: &[(&str, &str)] = &[
    ("background", "oklch(0.21 0.006 265 / 0.88)"),
    ("foreground", "oklch(0.95 0.003 265)"),
    ("surface", "oklch(0.99 0.002 265 / 0.06)"),
    ("surface-strong", "oklch(0.99 0.002 265 / 0.11)"),
    ("surface-muted", "oklch(0.99 0.002 265 / 0.04)"),
    ("card", "oklch(0.99 0.002 265 / 0.08)"),
    ("card-foreground", "oklch(0.95 0.003 265)"),
    ("muted", "oklch(0.99 0.002 265 / 0.08)"),
    ("muted-foreground", "oklch(0.72 0.008 265)"),
    ("accent", "oklch(0.99 0.002 265 / 0.12)"),
    ("accent-foreground", "oklch(0.96 0.003 265)"),
    ("primary", "oklch(0.93 0.004 265)"),
    ("primary-foreground", "oklch(0.22 0.008 265)"),
    ("border", "oklch(1 0 0 / 0.1)"),
    ("border-strong", "oklch(1 0 0 / 0.16)"),
    ("ring", "oklch(0.8 0.01 265 / 0.4)"),
    ("highlight", "oklch(1 0 0 / 0.12)"),
    ("overlay", "oklch(0.245 0.007 265 / 0.95)"),
    ("overlay-border", "oklch(1 0 0 / 0.14)"),
    ("overlay-shadow", "oklch(0.05 0.01 265 / 0.65)"),
    ("scrim", "oklch(0.12 0.008 265 / 0.55)"),
    ("ok", "oklch(0.76 0.15 155)"),
    ("ok-soft", "oklch(0.76 0.15 155 / 0.16)"),
    ("bad", "oklch(0.7 0.17 25)"),
    ("bad-soft", "oklch(0.7 0.17 25 / 0.16)"),
    ("busy", "oklch(0.82 0.14 78)"),
    ("busy-soft", "oklch(0.82 0.14 78 / 0.16)"),
    ("info", "oklch(0.72 0.12 250)"),
    ("info-soft", "oklch(0.72 0.12 250 / 0.16)"),
    // `#0d1117`, `github-dark-default`'s editor background. See the light block.
    ("diff-code-base", "oklch(0.176 0.014 258.36 / 0.92)"),
    ("diff-add", "oklch(0.6 0.1 150 / 0.22)"),
    ("diff-add-strong", "oklch(0.65 0.14 150 / 0.34)"),
    ("diff-del", "oklch(0.55 0.11 25 / 0.22)"),
    ("diff-del-strong", "oklch(0.6 0.15 25 / 0.34)"),
    ("diff-gutter", "oklch(0.8 0.01 265 / 0.4)"),
];

/// Every token of one theme, as `(name, value)` with the name lacking its `--`.
pub fn tokens(theme: ThemeName) -> &'static [(&'static str, &'static str)] {
    match theme {
        ThemeName::Light => LIGHT,
        ThemeName::Dark => DARK,
    }
}

/// The exact string index.css gives a token, or `None` for a name it does not define.
pub fn token(theme: ThemeName, name: &str) -> Option<&'static str> {
    tokens(theme)
        .iter()
        .find(|(token, _)| *token == name)
        .map(|(_, value)| *value)
}

/// A token resolved to a colour, or `None` for a name the palette does not define.
pub fn resolve(theme: ThemeName, name: &str) -> Option<Rgba> {
    parse_color(token(theme, name)?).ok()
}

/// Fully transparent: what a token that somehow failed to resolve paints as, so a
/// typo shows up as something missing rather than as a wrong colour. The tests
/// below make sure no shipped token takes this path.
const MISSING: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

macro_rules! palette {
    ($($field:ident => $name:literal),* $(,)?) => {
        /// One theme's palette, resolved: one field per index.css token.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Palette {
            $(
                #[doc = concat!("`--", $name, "`")]
                pub $field: Rgba,
            )*
        }

        impl Palette {
            /// Resolves every token of a theme.
            pub fn new(theme: ThemeName) -> Palette {
                Palette {
                    $($field: resolve(theme, $name).unwrap_or(MISSING),)*
                }
            }
        }

        /// Every token name a [`Palette`] has a field for, in field order.
        pub const PALETTE_TOKENS: &[&str] = &[$($name),*];
    };
}

palette! {
    background => "background",
    foreground => "foreground",
    surface => "surface",
    surface_strong => "surface-strong",
    surface_muted => "surface-muted",
    card => "card",
    card_foreground => "card-foreground",
    muted => "muted",
    muted_foreground => "muted-foreground",
    accent => "accent",
    accent_foreground => "accent-foreground",
    primary => "primary",
    primary_foreground => "primary-foreground",
    border => "border",
    border_strong => "border-strong",
    ring => "ring",
    highlight => "highlight",
    overlay => "overlay",
    overlay_border => "overlay-border",
    overlay_shadow => "overlay-shadow",
    scrim => "scrim",
    ok => "ok",
    ok_soft => "ok-soft",
    bad => "bad",
    bad_soft => "bad-soft",
    busy => "busy",
    busy_soft => "busy-soft",
    info => "info",
    info_soft => "info-soft",
    diff_code_base => "diff-code-base",
    diff_add => "diff-add",
    diff_add_strong => "diff-add-strong",
    diff_del => "diff-del",
    diff_del_strong => "diff-del-strong",
    diff_gutter => "diff-gutter",
}

impl Palette {
    pub fn light() -> Palette {
        Palette::new(ThemeName::Light)
    }

    pub fn dark() -> Palette {
        Palette::new(ThemeName::Dark)
    }

    pub fn for_dark(dark: bool) -> Palette {
        if dark {
            Palette::dark()
        } else {
            Palette::light()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_resolves_in_both_themes() {
        for theme in [ThemeName::Light, ThemeName::Dark] {
            for (name, value) in tokens(theme) {
                assert!(parse_color(value).is_ok(), "{theme:?} --{name}: {value}");
            }
        }
    }

    #[test]
    fn both_themes_define_the_same_tokens_and_the_palette_has_a_field_for_each() {
        let names = |theme| {
            tokens(theme)
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(ThemeName::Light), names(ThemeName::Dark));
        assert_eq!(names(ThemeName::Light), PALETTE_TOKENS);
    }

    #[test]
    fn the_palette_resolves_its_fields_from_the_tokens() {
        let light = Palette::light();
        assert_eq!(light.diff_code_base.a, 0.92);
        assert_eq!(light.foreground.a, 1.0);
        assert_eq!(Palette::for_dark(true), Palette::dark());
        assert_eq!(Palette::dark().diff_code_base.to_u32() & 0xff, 0xeb);
        assert_eq!(token(ThemeName::Dark, "nope"), None);
        assert_eq!(resolve(ThemeName::Dark, "nope"), None);
    }

    /// While the stylesheet still exists, the data here has to say exactly what it
    /// says: this is the guard against the two drifting apart during the port.
    #[test]
    fn the_tokens_match_index_css_while_it_exists() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/renderer/src/index.css"
        );
        let Ok(css) = std::fs::read_to_string(path) else {
            return;
        };

        fn block<'a>(css: &'a str, selector: &str) -> &'a str {
            let opener = format!("\n{selector} {{\n");
            let start = css.find(&opener).map(|at| at + opener.len());
            let Some(start) = start else {
                panic!("index.css has no top-level {selector} block");
            };
            let end = css[start..].find("\n}").map_or(css.len(), |at| start + at);
            &css[start..end]
        }

        fn properties(block: &str) -> Vec<(String, String)> {
            block
                .lines()
                .filter_map(|line| line.strip_prefix("  --"))
                .filter_map(|line| {
                    let (name, value) = line.split_once(": ")?;
                    Some((name.to_string(), value.strip_suffix(';')?.to_string()))
                })
                .collect()
        }

        let owned = |theme| {
            tokens(theme)
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<Vec<_>>()
        };

        let mut light = properties(block(&css, ":root"));
        let radius = light.iter().position(|(name, _)| name == "radius");
        assert_eq!(
            radius.map(|at| light.remove(at).1),
            Some(RADIUS.to_string())
        );
        assert_eq!(light, owned(ThemeName::Light));
        assert_eq!(properties(block(&css, ".dark")), owned(ThemeName::Dark));
    }
}
