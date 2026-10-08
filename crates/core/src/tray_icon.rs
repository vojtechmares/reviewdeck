//! The menu bar glyph, rasterised to PNG bytes: a port of src/main/tray-icon.ts.
//!
//! The Electron app learnt the hard way that the obvious route - an SVG - can come
//! back as an empty image, and an empty status item still takes its slot in the
//! menu bar and still opens its menu on click, so the failure reads as *the count
//! is broken* rather than as the icon never having drawn. So the glyph is drawn
//! here instead, straight to PNG: a PNG is zlib-deflated scanlines plus three
//! chunks, which takes a CRC and a deflate and nothing else.
//!
//! The result is a macOS *template* image, which means only the alpha channel is
//! ever read - the system paints the coverage black or white to match the menu bar
//! and to invert when the menu is open. That is why the pixel format is grey plus
//! alpha with the grey pinned at zero: the colour is dead weight, the coverage is
//! the whole picture.
//!
//! Nothing here touches AppKit, so the geometry is assertable in a test.

/// The glyph is designed on a 16pt square, the size macOS wants in the menu bar.
pub const TRAY_ICON_POINTS: u32 = 16;

/// Three stacked bars, shortest at the bottom, echoing the app icon: a deck of
/// reviews. Coordinates are in points on the 16pt square.
const BAR_HEIGHT: f64 = 2.0;
const BAR_GAP: f64 = 3.0;
const BAR_LEFT: f64 = 2.0;
const BAR_WIDTHS: [f64; 3] = [12.0, 9.0, 6.0];
/// Centres the stack: three bars and two gaps is 12pt tall on a 16pt square.
const STACK_TOP: f64 =
    (TRAY_ICON_POINTS as f64 - (BAR_WIDTHS.len() as f64 * BAR_HEIGHT + 2.0 * BAR_GAP)) / 2.0;

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// Signed distance to a rounded rectangle, used for antialiased edges.
fn rounded_rect_distance(x: f64, y: f64, half_w: f64, half_h: f64, radius: f64) -> f64 {
    let dx = x.abs() - (half_w - radius);
    let dy = y.abs() - (half_h - radius);
    let outside = dx.max(0.0).hypot(dy.max(0.0));
    outside + dx.max(dy).min(0.0) - radius
}

/// Ink coverage at a pixel centre, 0 to 1, for a glyph drawn at `size` pixels.
fn coverage_at(x: f64, y: f64, size: u32) -> f64 {
    let scale = f64::from(size) / f64::from(TRAY_ICON_POINTS);
    let mut coverage: f64 = 0.0;
    for (index, width) in BAR_WIDTHS.iter().enumerate() {
        let centre_x = (BAR_LEFT + width / 2.0) * scale;
        let centre_y =
            (STACK_TOP + index as f64 * (BAR_HEIGHT + BAR_GAP) + BAR_HEIGHT / 2.0) * scale;
        let distance = rounded_rect_distance(
            x - centre_x,
            y - centre_y,
            (width / 2.0) * scale,
            (BAR_HEIGHT / 2.0) * scale,
            (BAR_HEIGHT / 2.0) * scale,
        );
        // One pixel of feather, so the pill caps stay smooth at every scale factor.
        coverage = coverage.max(clamp01(0.5 - distance));
    }
    coverage
}

/// The CRC-32 a PNG chunk carries (IEEE polynomial, reflected).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = !0;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// Appends one chunk: length, type, data, and the CRC of type plus data.
fn push_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    // A chunk's length field is 32 bits; the largest icon asked for is a few KB.
    let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
    png.extend_from_slice(&length.to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}

/// The glyph as a PNG `size` pixels square: 16 for a 1x menu bar, 32 for 2x.
///
/// Grey-plus-alpha (PNG colour type 4) with the grey at zero, because a template
/// image is a coverage mask and nothing else.
pub fn tray_icon_png(size: u32) -> Vec<u8> {
    let side = size as usize;
    let stride = side * 2;
    // Each scanline is prefixed with its filter type; 0 means "none".
    let mut raw = vec![0u8; (stride + 1) * side];
    for y in 0..side {
        let row = y * (stride + 1);
        raw[row] = 0;
        for x in 0..side {
            let coverage = coverage_at(x as f64 + 0.5, y as f64 + 0.5, size);
            // Coverage is clamped to [0, 1], so this lands in 0..=255.
            let alpha = (coverage * 255.0).round() as u8;
            raw[row + 1 + x * 2] = 0; // grey: unread, the system supplies the colour
            raw[row + 2 + x * 2] = alpha;
        }
    }

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(4); // colour type: grey + alpha
    ihdr.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace

    let mut png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(
        &mut png,
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9),
    );
    push_chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    /// Walks the chunk list, returning the payload of the first chunk of `kind`.
    fn chunk_data<'a>(png: &'a [u8], kind: &str) -> &'a [u8] {
        let mut offset = PNG_SIGNATURE.len();
        while offset < png.len() {
            let length = read_u32(png, offset) as usize;
            let name = &png[offset + 4..offset + 8];
            if name == kind.as_bytes() {
                // Every chunk's CRC checks out on the way past.
                let crc = read_u32(png, offset + 8 + length);
                assert_eq!(
                    crc,
                    crc32(&png[offset + 4..offset + 8 + length]),
                    "{kind} crc"
                );
                return &png[offset + 8..offset + 8 + length];
            }
            offset += 12 + length;
        }
        panic!("no {kind} chunk");
    }

    /// Alpha per pixel, row-major, undoing the per-scanline filter byte.
    fn alpha(png: &[u8], size: usize) -> Vec<u8> {
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(chunk_data(png, "IDAT"))
            .expect("IDAT inflates");
        let stride = size * 2;
        let mut values = Vec::new();
        for y in 0..size {
            assert_eq!(raw[y * (stride + 1)], 0, "expected an unfiltered scanline");
            for x in 0..size {
                values.push(raw[y * (stride + 1) + 2 + x * 2]);
            }
        }
        values
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }

    // The bug this file exists for: an icon in a format the status item cannot
    // decode leaves the menu bar blank whenever the count is not drawn over it. PNG
    // is one format everything reads.
    #[test]
    fn is_a_png_not_some_format_the_menu_bar_cannot_decode() {
        let png = tray_icon_png(TRAY_ICON_POINTS);
        assert_eq!(png[..8], PNG_SIGNATURE);

        let ihdr = chunk_data(&png, "IHDR");
        assert_eq!(read_u32(ihdr, 0), TRAY_ICON_POINTS);
        assert_eq!(read_u32(ihdr, 4), TRAY_ICON_POINTS);
        assert_eq!(ihdr[8], 8, "bit depth");
        assert_eq!(ihdr[9], 4, "colour type: grey + alpha");
        assert!(chunk_data(&png, "IEND").is_empty());
    }

    #[test]
    fn draws_an_opaque_glyph_at_every_scale_factor() {
        for size in [TRAY_ICON_POINTS, TRAY_ICON_POINTS * 2] {
            let size = size as usize;
            let values = alpha(&tray_icon_png(size as u32), size);
            assert_eq!(values.len(), size * size);
            assert!(
                values.contains(&255),
                "{size}px icon has no fully covered pixel"
            );
        }
    }

    #[test]
    fn draws_three_bars_shortest_at_the_bottom_inside_the_square() {
        let size = TRAY_ICON_POINTS as usize;
        let values = alpha(&tray_icon_png(TRAY_ICON_POINTS), size);
        let width = |y: usize| {
            values[y * size..(y + 1) * size]
                .iter()
                .filter(|value| **value > 127)
                .count()
        };

        let rows: Vec<usize> = (0..size).map(width).collect();
        let mut bands: Vec<Vec<usize>> = Vec::new();
        for &row in &rows {
            if row == 0 {
                bands.push(Vec::new());
            } else {
                match bands.last_mut() {
                    Some(band) => band.push(row),
                    None => bands.push(vec![row]),
                }
            }
        }
        let bars: Vec<usize> = bands
            .iter()
            .filter(|band| !band.is_empty())
            .filter_map(|band| band.iter().max().copied())
            .collect();

        assert_eq!(bars.len(), 3, "expected three bars, got widths {bars:?}");
        assert!(
            bars[0] > bars[1] && bars[1] > bars[2],
            "bars do not taper: {bars:?}"
        );
        // A margin all round, so the glyph is not flush against its neighbours.
        assert_eq!(rows[0], 0);
        assert_eq!(rows[size - 1], 0);
        assert!(bars[0] < size, "the widest bar touches both edges");
    }
}
