//! Colour arithmetic, so the legibility of the diff is asserted rather than
//! eyeballed - a port of src/shared/color.ts.
//!
//! The window is frosted glass over the desktop, which means the colour behind a
//! syntax token depends on a wallpaper the app can neither see nor control. That
//! looks unmeasurable, and is not: a wallpaper is a variable that can be *bounded*
//! rather than sampled. Everything the app paints over it is a translucent film, so
//! for any given stack of films the composited result is monotonic in the backdrop -
//! every possible desktop lands between what pure white behind the window gives and
//! what pure black gives. Assert the contrast bar at both extremes and every
//! wallpaper in existence is covered, by construction.
//!
//! That turns a judgement call into a test, and makes the base film behind the code
//! columns a *derived* value - whatever alpha makes the assertion hold - rather than
//! a number somebody picked and had to defend.
//!
//! Nothing here knows about the highlighter or the UI: this is arithmetic over
//! colour notations, and the test that matters runs it against the theme's own JSON
//! and the app's own palette. The oklch conversion is hand-rolled for the same
//! reason it was in the TypeScript - it is thirty lines of matrix multiplication,
//! and a colour library would be a dependency carried for them.

use crate::error::{Result, msg};

/// A colour in gamma-encoded sRGB. Every channel and the alpha in 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    /// The colour as `0xRRGGBBAA`, the packing `gpui::rgba` takes. Channels are
    /// clamped and rounded to the nearest byte.
    pub fn to_u32(self) -> u32 {
        let byte = |channel: f64| -> u32 {
            // `as` saturates and maps NaN to 0, so nothing here can wrap.
            (clamp(channel) * 255.0).round() as u32
        };
        (byte(self.r) << 24) | (byte(self.g) << 16) | (byte(self.b) << 8) | byte(self.a)
    }

    /// The colour with a different alpha.
    pub fn with_alpha(self, a: f64) -> Rgba {
        Rgba { a, ..self }
    }
}

/// Pure white and pure black behind the window: the two ends of every wallpaper.
pub const WHITE_BACKDROP: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
pub const BLACK_BACKDROP: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

fn clamp(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// A colour in one of the two notations this app actually writes: the `oklch()` its
/// own tokens are defined in, and the hex a syntax theme carries - three, four, six
/// or eight digits, the last of those carrying alpha.
///
/// Anything else is an error rather than resolving to a default. A test that
/// quietly accepted a notation it could not read would assert nothing at all.
pub fn parse_color(value: &str) -> Result<Rgba> {
    let text = value.trim();
    if text.starts_with('#') {
        return parse_hex(text);
    }
    if text.to_lowercase().starts_with("oklch(") {
        return parse_oklch(text);
    }
    Err(msg(format!("Unsupported colour notation: {value}")))
}

fn parse_hex(text: &str) -> Result<Rgba> {
    let digits = &text[1..];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(msg(format!("Not a hex colour: {text}")));
    }

    // The three- and four-digit forms are the six- and eight-digit ones with every
    // digit doubled, so widening first leaves one parser below.
    let wide: String = if digits.len() == 3 || digits.len() == 4 {
        digits.chars().flat_map(|digit| [digit, digit]).collect()
    } else {
        digits.to_string()
    };
    if wide.len() != 6 && wide.len() != 8 {
        return Err(msg(format!("Not a hex colour: {text}")));
    }

    // Every byte is an ASCII hex digit, checked above, so the slices are on char
    // boundaries and the parse cannot fail.
    let byte = |index: usize| -> f64 {
        let pair = wide.get(index * 2..index * 2 + 2).unwrap_or("00");
        f64::from(u8::from_str_radix(pair, 16).unwrap_or(0)) / 255.0
    };
    Ok(Rgba {
        r: byte(0),
        g: byte(1),
        b: byte(2),
        a: if wide.len() == 8 { byte(3) } else { 1.0 },
    })
}

fn parse_oklch(text: &str) -> Result<Rgba> {
    // `slice(indexOf('(') + 1, lastIndexOf(')'))`: without a closing parenthesis
    // JavaScript's -1 drops the last character, and so does this.
    let open = text.find('(').map_or(0, |at| at + 1);
    let close = text
        .rfind(')')
        .unwrap_or_else(|| text.char_indices().last().map_or(0, |(at, _)| at));
    let inside = text.get(open..close).unwrap_or("");

    let mut halves = inside.split('/');
    let coords = halves.next().unwrap_or("");
    let alpha = halves.next();
    let parts: Vec<&str> = coords.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(msg(format!("Not an oklch colour: {text}")));
    }

    let lightness = number(parts[0])?;
    let chroma = number(parts[1])?;
    let hue = number(parts[2])?;
    let (r, g, b) = oklch_to_srgb(lightness, chroma, hue);
    Ok(Rgba {
        r,
        g,
        b,
        a: match alpha {
            None => 1.0,
            Some(alpha) => number(alpha)?,
        },
    })
}

/// A component, with the percentage form meaning the same as its 0..1 fraction.
///
/// Reads the longest leading number the way JavaScript's `Number.parseFloat` does,
/// so `42%` is 42 before the percentage applies.
fn number(token: &str) -> Result<f64> {
    let text = token.trim();
    let value = parse_float_prefix(text).ok_or_else(|| msg(format!("Not a number: {token}")))?;
    Ok(if text.ends_with('%') {
        value / 100.0
    } else {
        value
    })
}

/// `Number.parseFloat`: the longest prefix that is a decimal number, or `None`
/// where JavaScript would say `NaN`.
fn parse_float_prefix(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end += 1;
    }
    if text[end..].starts_with("Infinity") {
        return text[..end + "Infinity".len()].parse().ok();
    }
    let digits_from = |start: usize| {
        let mut at = start;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        at
    };
    let integer_end = digits_from(end);
    let mut mantissa_end = integer_end;
    let mut has_digits = integer_end > end;
    if bytes.get(integer_end) == Some(&b'.') {
        let fraction_end = digits_from(integer_end + 1);
        has_digits |= fraction_end > integer_end + 1;
        mantissa_end = fraction_end;
    }
    if !has_digits {
        return None;
    }
    let mut number_end = mantissa_end;
    if matches!(bytes.get(mantissa_end), Some(b'e' | b'E')) {
        let mut exponent = mantissa_end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let exponent_end = digits_from(exponent);
        if exponent_end > exponent {
            number_end = exponent_end;
        }
    }
    // Rust's float parser rejects a bare trailing dot, which JavaScript accepts.
    text[..number_end].trim_end_matches('.').parse().ok()
}

/// Oklch to gamma-encoded sRGB: polar to cartesian, the inverse of the Oklab
/// transform to linear sRGB, then the sRGB transfer function.
///
/// Coefficients are Björn Ottosson's, as CSS Color 4 specifies them. Channels are
/// clamped rather than gamut-mapped, which is what a browser does with an
/// out-of-gamut `oklch()` on an sRGB display and therefore what the assertion needs
/// to agree with. The app's own colours are all comfortably inside sRGB anyway.
fn oklch_to_srgb(lightness: f64, chroma: f64, hue: f64) -> (f64, f64, f64) {
    let radians = (hue * std::f64::consts::PI) / 180.0;
    let a = chroma * radians.cos();
    let b = chroma * radians.sin();

    let l = (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powf(3.0);
    let m = (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powf(3.0);
    let s = (lightness - 0.089_484_177_5 * a - 1.291_485_548 * b).powf(3.0);

    (
        encode(4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s),
        encode(-1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s),
        encode(-0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s),
    )
}

/// The sRGB transfer function: linear light to the value a channel is stored as.
fn encode(linear: f64) -> f64 {
    let value = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.abs().powf(1.0 / 2.4) - 0.055
    };
    clamp(value)
}

/// A stack of layers flattened to the one opaque colour a screen shows.
///
/// Layers are given **bottom first**: the backdrop, then each film painted over it.
/// This is `source-over` on gamma-encoded channels, which is what a browser does
/// with plain alpha rather than any wide-gamut interpolation - and what the UI's
/// compositor does with the same films.
///
/// The result must come out opaque - contrast against a colour that is still partly
/// transparent is not a number, it is a question about what is underneath - so a
/// stack that does not start with an opaque layer is an error.
pub fn composite(layers: &[Rgba]) -> Result<Rgba> {
    let Some((&first, rest)) = layers.split_first() else {
        return Err(msg("Nothing to composite"));
    };

    let mut out = first;
    for over in rest {
        let alpha = over.a + out.a * (1.0 - over.a);
        if alpha == 0.0 {
            out = Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            };
            continue;
        }
        let mix = |top: f64, under: f64| (top * over.a + under * out.a * (1.0 - over.a)) / alpha;
        out = Rgba {
            r: mix(over.r, out.r),
            g: mix(over.g, out.g),
            b: mix(over.b, out.b),
            a: alpha,
        };
    }

    if out.a < 1.0 - 1e-9 {
        return Err(msg(
            "Composited stack is not opaque; the bottom layer must be",
        ));
    }
    Ok(Rgba { a: 1.0, ..out })
}

/// WCAG relative luminance. The alpha is ignored: this wants an opaque colour.
pub fn relative_luminance(color: Rgba) -> f64 {
    let linear = |channel: f64| {
        if channel <= 0.039_28 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// WCAG contrast ratio between two opaque colours, 1 to 21, order-independent.
pub fn contrast_ratio(one: Rgba, other: Rgba) -> f64 {
    let a = relative_luminance(one);
    let b = relative_luminance(other);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    //! test/color.test.ts. The palette values come from `palette.rs` and the theme
    //! colours from the highlighter's embedded theme data, rather than from
    //! index.css and the npm package, so the assertion is about the app as it ships.

    use std::sync::LazyLock;

    use regex::Regex;

    use super::*;
    use crate::highlight::{
        SyntaxTheme, ThemeTokenRule, background_for, background_rules, paints_backgrounds,
        syntax_theme,
    };
    use crate::palette::{self, ThemeName};

    fn parse(value: &str) -> Rgba {
        match parse_color(value) {
            Ok(color) => color,
            Err(error) => panic!("{value}: {error}"),
        }
    }

    fn flatten(layers: &[Rgba]) -> Rgba {
        match composite(layers) {
            Ok(color) => color,
            Err(error) => panic!("{error}"),
        }
    }

    fn rgba(r: f64, g: f64, b: f64, a: f64) -> Rgba {
        Rgba { r, g, b, a }
    }

    #[test]
    fn parse_color_reads_the_hex_a_theme_emits_in_every_width() {
        assert_eq!(parse("#fff"), rgba(1.0, 1.0, 1.0, 1.0));
        assert_eq!(parse("#000000"), rgba(0.0, 0.0, 0.0, 1.0));
        assert_eq!(parse("#80808080").a, 128.0 / 255.0);
        assert_eq!(parse("#f00f"), parse("#ff0000"));
    }

    #[test]
    fn parse_color_reads_the_oklch_the_app_writes() {
        let white = parse("oklch(1 0 0)");
        assert_eq!((white.r * 255.0).round(), 255.0);
        assert_eq!((white.g * 255.0).round(), 255.0);
        assert_eq!((white.b * 255.0).round(), 255.0);

        assert_eq!(parse("oklch(0.5 0.1 200 / 0.42)").a, 0.42);
        assert_eq!(parse("oklch(50% 0.1 200 / 42%)").a, 0.42);
    }

    #[test]
    fn parse_color_fails_rather_than_defaulting() {
        assert!(parse_color("rebeccapurple").is_err());
        assert!(parse_color("#12345").is_err());
        // Rust-specific: nothing that is not ASCII hex gets near a byte slice.
        assert!(parse_color("#ééé").is_err());
        assert!(parse_color("#").is_err());
        assert!(parse_color("oklch(a b c)").is_err());
        assert!(parse_color("oklch(1 0)").is_err());
    }

    #[test]
    fn parse_color_reads_numbers_the_way_parse_float_does() {
        assert_eq!(parse_float_prefix("0.5"), Some(0.5));
        assert_eq!(parse_float_prefix(".5"), Some(0.5));
        assert_eq!(parse_float_prefix("5."), Some(5.0));
        assert_eq!(parse_float_prefix("42%"), Some(42.0));
        assert_eq!(parse_float_prefix("-1e2x"), Some(-100.0));
        assert_eq!(parse_float_prefix("1e"), Some(1.0));
        assert_eq!(parse_float_prefix("abc"), None);
        assert_eq!(parse_float_prefix("."), None);
        // Upper-case notation and surrounding space are read too.
        assert_eq!(parse(" OKLCH(1 0 0) ").a, 1.0);
    }

    #[test]
    fn to_u32_packs_the_way_gpui_reads_it() {
        assert_eq!(parse("#0d1117").to_u32(), 0x0d11_17ff);
        assert_eq!(parse("#80808080").to_u32(), 0x8080_8080);
        assert_eq!(rgba(2.0, -1.0, f64::NAN, 0.5).to_u32(), 0xff00_0080);
    }

    #[test]
    fn composite_lays_films_over_a_backdrop_and_insists_on_an_opaque_result() {
        assert_eq!(flatten(&[BLACK_BACKDROP, WHITE_BACKDROP]), WHITE_BACKDROP);

        let half = flatten(&[BLACK_BACKDROP, rgba(1.0, 1.0, 1.0, 0.5)]);
        assert_eq!(half.a, 1.0);
        assert!((half.r - 0.5).abs() < 1e-9);

        assert!(composite(&[rgba(1.0, 1.0, 1.0, 0.5)]).is_err());
        assert!(composite(&[]).is_err());
    }

    #[test]
    fn contrast_ratio_is_wcag_and_does_not_care_which_colour_is_first() {
        assert_eq!(contrast_ratio(WHITE_BACKDROP, BLACK_BACKDROP), 21.0);
        assert_eq!(contrast_ratio(BLACK_BACKDROP, WHITE_BACKDROP), 21.0);
        assert_eq!(contrast_ratio(WHITE_BACKDROP, WHITE_BACKDROP), 1.0);
        assert_eq!(relative_luminance(WHITE_BACKDROP), 1.0);
    }

    /*
     * The legibility assertion.
     *
     * The window is glass over a desktop the app cannot see, so what sits behind a
     * syntax token depends on somebody's wallpaper. The wallpaper is bounded rather
     * than sampled: every film the app paints is composited source-over, so the
     * result is monotonic in the backdrop and every possible desktop lands between
     * what pure white gives and what pure black gives. Assert both extremes and the
     * whole space is covered, by construction.
     *
     * Values come out of the palette rather than being restated here, so the
     * assertion is about the app as shipped and cannot drift away from it.
     */

    #[derive(Clone, Copy)]
    enum Row {
        Context,
        Add,
        Del,
    }

    const THEMES: [ThemeName; 2] = [ThemeName::Light, ThemeName::Dark];
    const ROWS: [Row; 3] = [Row::Context, Row::Add, Row::Del];
    const BACKDROPS: [(&str, Rgba); 2] = [("white", WHITE_BACKDROP), ("black", BLACK_BACKDROP)];

    fn property(theme: ThemeName, name: &str) -> &'static str {
        match palette::token(theme, name) {
            Some(value) => value,
            None => panic!("the palette defines no --{name} for the {theme:?} theme"),
        }
    }

    /// The alpha of a custom property, which is the knob everything here turns.
    fn alpha_of(value: &str) -> f64 {
        parse(value).a
    }

    fn base_alpha() -> f64 {
        alpha_of(property(ThemeName::Light, "diff-code-base"))
    }

    fn theme_data(theme: ThemeName) -> &'static SyntaxTheme {
        syntax_theme(matches!(theme, ThemeName::Dark))
    }

    /// What sits behind a line of code, bottom first: the desktop, the app's own
    /// film, the file panel's glass, the code column's base film, and the row tint
    /// over it.
    ///
    /// Vibrancy blurs and saturates what is behind the window, and both are the
    /// identity on a uniform achromatic backdrop - which is exactly what the two
    /// extremes are - so neither shifts these numbers.
    fn row_stack(theme: ThemeName, row: Row, backdrop: Rgba, base_alpha: f64) -> Vec<Rgba> {
        let base = parse(property(theme, "diff-code-base"));
        let mut stack = vec![
            backdrop,
            parse(property(theme, "background")),
            parse(property(theme, "surface")),
            base.with_alpha(base_alpha),
        ];
        match row {
            Row::Context => {}
            Row::Add => stack.push(parse(property(theme, "diff-add"))),
            Row::Del => stack.push(parse(property(theme, "diff-del"))),
        }
        stack
    }

    fn scopes_of(rule: &ThemeTokenRule) -> Vec<String> {
        rule.scopes().map(str::to_string).collect()
    }

    /// Every colour the theme can put on a token, against the scope that asks for it.
    ///
    /// Rules that set a background are left out: those tokens are painted with the
    /// theme's own background behind them, which occludes everything under it, and
    /// they are asserted separately against that.
    fn token_colours(theme: ThemeName) -> Vec<(String, String)> {
        let raw = theme_data(theme);
        let mut colours = vec![(
            "editor.foreground".to_string(),
            raw.colors
                .get("editor.foreground")
                .cloned()
                .unwrap_or_default(),
        )];
        for rule in &raw.token_colors {
            let Some(settings) = &rule.settings else {
                continue;
            };
            let Some(foreground) = &settings.foreground else {
                continue;
            };
            if settings.background.is_some() {
                continue;
            }
            for scope in scopes_of(rule) {
                colours.push((scope, foreground.clone()));
            }
        }
        colours
    }

    /// The bar a scope has to clear, off the theme's own scope names and nothing else.
    ///
    /// Comments and punctuation are muted deliberately - holding them to 4.5:1 would
    /// mean overriding the very colours that made this theme worth choosing - so they
    /// take WCAG's large-text bar instead. Everything that carries meaning takes 4.5:1.
    fn bar_for(scope: &str) -> f64 {
        static MUTED: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(^|\.)(comment|punctuation)(\.|$)")
                .expect("the muted-scope regex is valid")
        });
        if MUTED.is_match(scope) { 3.0 } else { 4.5 }
    }

    /// Every way the diff fails to be legible at this base alpha, named one by one.
    fn legibility_failures(base_alpha: f64) -> Vec<String> {
        let mut failures = Vec::new();
        for theme in THEMES {
            for row in ROWS {
                for (backdrop_name, backdrop) in BACKDROPS {
                    let background = flatten(&row_stack(theme, row, backdrop, base_alpha));
                    for (scope, color) in token_colours(theme) {
                        let bar = bar_for(&scope);
                        let ratio =
                            contrast_ratio(flatten(&[background, parse(&color)]), background);
                        if ratio < bar {
                            failures.push(format!(
                                "{theme:?} {scope} {color} on a row over a {backdrop_name} desktop: {ratio:.2}:1, needs {bar}:1"
                            ));
                        }
                    }
                }
            }
        }
        failures
    }

    /// The floor a changed row has to stay above, so the change is still the loudest thing.
    const SEPARATION: f64 = 1.1;

    /// Every way a changed row stops being obviously changed at this base alpha.
    fn separation_failures(base_alpha: f64) -> Vec<String> {
        let mut failures = Vec::new();
        for theme in THEMES {
            for (backdrop_name, backdrop) in BACKDROPS {
                let context = flatten(&row_stack(theme, Row::Context, backdrop, base_alpha));
                for (row_name, row) in [("add", Row::Add), ("del", Row::Del)] {
                    let changed = flatten(&row_stack(theme, row, backdrop, base_alpha));
                    let ratio = contrast_ratio(changed, context);
                    if ratio < SEPARATION {
                        failures.push(format!(
                            "{theme:?} {row_name} row over a {backdrop_name} desktop sits {ratio:.3}:1 from context, needs {SEPARATION}:1"
                        ));
                    }
                }
            }
        }
        failures
    }

    #[test]
    fn every_token_colour_is_legible_over_every_row_theme_and_desktop() {
        assert_eq!(legibility_failures(base_alpha()), Vec::<String>::new());
    }

    #[test]
    fn added_and_removed_rows_stay_obviously_changed_over_every_desktop() {
        assert_eq!(separation_failures(base_alpha()), Vec::<String>::new());
    }

    #[test]
    fn the_base_film_is_as_thin_as_the_assertion_allows_and_both_themes_share_it() {
        let base = base_alpha();
        assert_eq!(
            alpha_of(property(ThemeName::Dark, "diff-code-base")),
            base,
            "the two themes should thicken their code columns by the same amount"
        );

        // Derived, not chosen: one hundredth thinner and the diff stops being legible,
        // so this is the lowest the code column can be and still bound the desktop out.
        let thinner: f64 = format!("{:.2}", base - 0.01).parse().unwrap_or(base);
        let mut failures = legibility_failures(thinner);
        failures.extend(separation_failures(thinner));
        assert!(
            !failures.is_empty(),
            "--diff-code-base could be {thinner} rather than {base}; thin it out"
        );
    }

    /*
     * The scopes a theme paints a background behind.
     *
     * The highlighter's tokens carry foregrounds from the theme resolution, so these
     * five rules per theme would otherwise render with a foreground chosen for a
     * background nothing paints. The renderer paints them, which makes them
     * backdrop-independent by construction: the theme's backgrounds are opaque, so
     * they occlude the glass, the row tint and the desktop alike, and the contrast is
     * simply one of the theme's colours over another.
     */

    /*
     * `carriage-return` is excluded by name, and only this one.
     *
     * It is 2.32:1 in dark on the theme's own colours - below even the quiet bar,
     * with no glass anywhere near it. It is a control-character marker rather than a
     * syntax token, and its foreground and background are a pair the theme picked
     * together, so the renderer shows that pair verbatim rather than second-guessing
     * a theme it chose for being measurable.
     */
    const UNMEASURED_SCOPE: &str = "carriage-return";

    #[test]
    fn the_scopes_that_carry_their_own_background_are_legible_on_it() {
        let mut failures = Vec::new();

        for theme in THEMES {
            let mut measured = 0;
            for rule in &theme_data(theme).token_colors {
                let Some(settings) = &rule.settings else {
                    continue;
                };
                let (Some(foreground), Some(background)) =
                    (&settings.foreground, &settings.background)
                else {
                    continue;
                };
                let scopes = scopes_of(rule);
                if scopes.iter().any(|scope| scope == UNMEASURED_SCOPE) {
                    continue;
                }

                measured += 1;
                let ratio = contrast_ratio(parse(foreground), parse(background));
                if ratio < 4.5 {
                    failures.push(format!(
                        "{theme:?} {} {foreground} on {background}: {ratio:.2}:1",
                        scopes.first().map_or("", String::as_str)
                    ));
                }
            }
            assert_eq!(
                measured, 4,
                "{theme:?} should paint four background-carrying scopes"
            );
        }

        assert_eq!(failures, Vec::<String>::new());
    }

    #[test]
    fn a_background_is_resolved_from_the_scopes_a_token_matched_not_from_its_colour() {
        let rules = background_rules(&syntax_theme(false).token_colors);

        assert_eq!(
            background_for(&rules, &["source.diff", "markup.deleted.diff"]),
            Some("#ffebe9")
        );
        assert_eq!(
            background_for(&rules, &["source.diff", "markup.inserted.diff"]),
            Some("#dafbe1")
        );

        // `entity.name.tag` is `#116329`, the same colour `markup.inserted` is - so a
        // lookup keyed on the foreground would paint green behind every JSX tag name.
        assert_eq!(
            background_for(
                &rules,
                &["source.tsx", "meta.tag.tsx", "entity.name.tag.tsx"]
            ),
            None
        );

        // A prefix of a scope matches it; a longer name that merely starts the same does not.
        assert_eq!(background_for(&rules, &["markup.deletedish"]), None);
    }

    #[test]
    fn only_the_languages_that_can_emit_those_scopes_pay_for_asking() {
        assert!(paints_backgrounds("diff"));
        assert!(paints_backgrounds("markdown"));
        assert!(!paints_backgrounds("typescript"));
        assert!(!paints_backgrounds("python"));
    }

    #[test]
    fn the_renderer_highlights_with_the_pair_this_file_measures() {
        assert_eq!(syntax_theme(false).name, "github-light-default");
        assert_eq!(syntax_theme(true).name, "github-dark-default");
    }
}
