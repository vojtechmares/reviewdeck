/**
 * The menu bar glyph, rasterised to PNG bytes.
 *
 * The obvious way to write this is an SVG data URL, and it silently does not
 * work: `nativeImage` decodes PNG and JPEG only, so an `image/svg+xml` data URL
 * comes back empty. An empty tray icon still takes its slot in the menu bar and
 * still opens its menu on click, so the failure reads as *the count is broken* -
 * a number appears while reviews are waiting, and the moment the deck empties
 * there is nothing there at all - rather than as the icon never having drawn.
 *
 * So the glyph is drawn here instead, straight to PNG, the way
 * `scripts/make-icon.mjs` writes the app icon: a PNG is zlib-deflated scanlines
 * plus three chunks, and this app carries no runtime dependencies.
 *
 * The result is a macOS *template* image, which means only the alpha channel is
 * ever read - the system paints the coverage black or white to match the menu
 * bar and to invert when the menu is open. That is why the pixel format is grey
 * plus alpha with the grey pinned at zero: the colour is dead weight, the
 * coverage is the whole picture.
 *
 * Nothing here imports electron, so the geometry is assertable in a test.
 */

import { deflateSync } from 'node:zlib'

/** The glyph is designed on a 16pt square, the size macOS wants in the menu bar. */
export const TRAY_ICON_POINTS = 16

/**
 * Three stacked bars, shortest at the bottom, echoing the app icon: a deck of
 * reviews. Coordinates are in points on the 16pt square.
 */
const BAR_HEIGHT = 2
const BAR_GAP = 3
const BAR_LEFT = 2
const BAR_WIDTHS = [12, 9, 6]
/** Centres the stack: three bars and two gaps is 12pt tall on a 16pt square. */
const STACK_TOP = (TRAY_ICON_POINTS - (BAR_WIDTHS.length * BAR_HEIGHT + 2 * BAR_GAP)) / 2

const clamp01 = (value: number): number => Math.min(1, Math.max(0, value))

/** Signed distance to a rounded rectangle, used for antialiased edges. */
function roundedRectDistance(
  x: number,
  y: number,
  halfW: number,
  halfH: number,
  radius: number,
): number {
  const dx = Math.abs(x) - (halfW - radius)
  const dy = Math.abs(y) - (halfH - radius)
  const outside = Math.hypot(Math.max(dx, 0), Math.max(dy, 0))
  return outside + Math.min(Math.max(dx, dy), 0) - radius
}

/** Ink coverage at a pixel centre, 0 to 1, for a glyph drawn at `size` pixels. */
function coverageAt(x: number, y: number, size: number): number {
  const scale = size / TRAY_ICON_POINTS
  let coverage = 0
  for (const [index, width] of BAR_WIDTHS.entries()) {
    const centreX = (BAR_LEFT + width / 2) * scale
    const centreY = (STACK_TOP + index * (BAR_HEIGHT + BAR_GAP) + BAR_HEIGHT / 2) * scale
    const distance = roundedRectDistance(
      x - centreX,
      y - centreY,
      (width / 2) * scale,
      (BAR_HEIGHT / 2) * scale,
      (BAR_HEIGHT / 2) * scale,
    )
    // One pixel of feather, so the pill caps stay smooth at every scale factor.
    coverage = Math.max(coverage, clamp01(0.5 - distance))
  }
  return coverage
}

function crc32(buffer: Buffer): number {
  let crc = ~0
  for (const byte of buffer) {
    crc ^= byte
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1))
  }
  return ~crc >>> 0
}

function chunk(type: string, data: Buffer): Buffer {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([length, body, crc])
}

/**
 * The glyph as a PNG `size` pixels square: 16 for a 1x menu bar, 32 for 2x.
 *
 * Grey-plus-alpha (PNG colour type 4) with the grey at zero, because a template
 * image is a coverage mask and nothing else.
 */
export function trayIconPng(size: number): Buffer {
  const stride = size * 2
  // Each scanline is prefixed with its filter type; 0 means "none".
  const raw = Buffer.alloc((stride + 1) * size)
  for (let y = 0; y < size; y++) {
    const row = y * (stride + 1)
    raw[row] = 0
    for (let x = 0; x < size; x++) {
      const alpha = Math.round(coverageAt(x + 0.5, y + 0.5, size) * 255)
      raw[row + 1 + x * 2] = 0 // grey: unread, the system supplies the colour
      raw[row + 2 + x * 2] = alpha
    }
  }

  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 4 // colour type: grey + alpha
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}
