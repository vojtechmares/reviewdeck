/**
 * Generates resources/icon.icns without any image dependency.
 *
 * PNGs are written by hand (a PNG is just zlib-deflated scanlines plus three
 * chunks), then `iconutil` - which ships with macOS - packs the iconset.
 * Run with `node scripts/make-icon.mjs`.
 */

import { deflateSync } from 'node:zlib'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { join } from 'node:path'

const OUT = 'resources'
const WORK = join(OUT, 'Reviewdeck.iconset')

function crc32(buffer) {
  let crc = ~0
  for (const byte of buffer) {
    crc ^= byte
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1))
  }
  return ~crc >>> 0
}

function chunk(type, data) {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([length, body, crc])
}

/** `pixels` is RGBA, row-major, length size*size*4. */
function encodePng(size, pixels) {
  const stride = size * 4
  // Each scanline is prefixed with its filter type; 0 means "none".
  const raw = Buffer.alloc((stride + 1) * size)
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0
    pixels.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride)
  }

  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // colour type: RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

const clamp01 = (value) => Math.min(1, Math.max(0, value))
const mix = (a, b, t) => a + (b - a) * t

/** Signed distance to a rounded rectangle, used for antialiased edges. */
function roundedRectDistance(x, y, halfW, halfH, radius) {
  const dx = Math.abs(x) - (halfW - radius)
  const dy = Math.abs(y) - (halfH - radius)
  const outside = Math.hypot(Math.max(dx, 0), Math.max(dy, 0))
  return outside + Math.min(Math.max(dx, dy), 0) - radius
}

function render(size) {
  const pixels = Buffer.alloc(size * size * 4)
  const s = size / 1024 // design the icon at 1024 and scale down

  // macOS app icons sit inside a padded "squircle" rather than filling the tile.
  const inset = 100 * s
  const half = size / 2 - inset
  const radius = 228 * s
  const centre = size / 2

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const px = x + 0.5 - centre
      const py = y + 0.5 - centre
      const index = (y * size + x) * 4

      const distance = roundedRectDistance(px, py, half, half, radius)
      // 1.5px of feather keeps the edge smooth at every icon size.
      const coverage = clamp01(0.5 - distance / (1.5 * Math.max(1, s * 1.2)))
      if (coverage <= 0) continue

      // Vertical graphite gradient with a soft top-left sheen: the milky glass.
      const t = clamp01((py + half) / (half * 2))
      let r = mix(64, 30, t)
      let g = mix(69, 33, t)
      let b = mix(80, 40, t)

      const sheen = clamp01(1 - Math.hypot(px + half * 0.45, py + half * 0.55) / (half * 1.25))
      const glow = Math.pow(sheen, 2.2) * 46
      r += glow
      g += glow
      b += glow

      // Three stacked bars: the review deck, shortest at the bottom.
      const barHeight = 92 * s
      const gap = 62 * s
      const barRadius = barHeight / 2
      const widths = [520 * s, 396 * s, 272 * s]
      const left = -260 * s

      for (let i = 0; i < 3; i++) {
        const barCentreY = (i - 1) * (barHeight + gap)
        const barCentreX = left + widths[i] / 2
        const inBar = roundedRectDistance(
          px - barCentreX,
          py - barCentreY,
          widths[i] / 2,
          barHeight / 2,
          barRadius,
        )
        const barCoverage = clamp01(0.5 - inBar / 1.5)
        if (barCoverage > 0) {
          // Bars fade slightly as they descend, echoing an inbox emptying out.
          const tint = 255 - i * 26
          r = mix(r, tint, barCoverage)
          g = mix(g, tint, barCoverage)
          b = mix(b, tint, barCoverage)
        }
      }

      pixels[index] = Math.round(r)
      pixels[index + 1] = Math.round(g)
      pixels[index + 2] = Math.round(b)
      pixels[index + 3] = Math.round(coverage * 255)
    }
  }
  return encodePng(size, pixels)
}

rmSync(WORK, { recursive: true, force: true })
mkdirSync(WORK, { recursive: true })

// The exact set of names `iconutil` expects.
const variants = [
  [16, 'icon_16x16.png'],
  [32, 'icon_16x16@2x.png'],
  [32, 'icon_32x32.png'],
  [64, 'icon_32x32@2x.png'],
  [128, 'icon_128x128.png'],
  [256, 'icon_128x128@2x.png'],
  [256, 'icon_256x256.png'],
  [512, 'icon_256x256@2x.png'],
  [512, 'icon_512x512.png'],
  [1024, 'icon_512x512@2x.png'],
]

for (const [size, name] of variants) {
  writeFileSync(join(WORK, name), render(size))
  process.stdout.write(`.`)
}
console.log()

writeFileSync(join(OUT, 'icon.png'), render(512))
execFileSync('iconutil', ['-c', 'icns', WORK, '-o', join(OUT, 'icon.icns')])
rmSync(WORK, { recursive: true, force: true })
console.log('wrote resources/icon.icns and resources/icon.png')
