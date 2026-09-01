import { test } from 'node:test'
import assert from 'node:assert/strict'
import { inflateSync } from 'node:zlib'
import { TRAY_ICON_POINTS, trayIconPng } from '../src/main/tray-icon.ts'

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])

/** Walks the chunk list, returning the payload of the first chunk of `type`. */
function chunkData(png: Buffer, type: string): Buffer {
  let offset = PNG_SIGNATURE.length
  while (offset < png.length) {
    const length = png.readUInt32BE(offset)
    const name = png.subarray(offset + 4, offset + 8).toString('ascii')
    if (name === type) return png.subarray(offset + 8, offset + 8 + length)
    offset += 12 + length
  }
  assert.fail(`no ${type} chunk`)
}

/** Alpha per pixel, row-major, undoing the per-scanline filter byte. */
function alpha(png: Buffer, size: number): number[] {
  const raw = inflateSync(chunkData(png, 'IDAT'))
  const stride = size * 2
  const values: number[] = []
  for (let y = 0; y < size; y++) {
    assert.equal(raw[y * (stride + 1)], 0, 'expected an unfiltered scanline')
    for (let x = 0; x < size; x++) values.push(raw[y * (stride + 1) + 2 + x * 2]!)
  }
  return values
}

// The bug this file exists for: an SVG data URL decodes to an empty image in
// `nativeImage`, which leaves the menu bar blank whenever the count is not drawn
// over it. PNG is one of the two formats it actually reads.
test('is a PNG, not some format nativeImage cannot decode', () => {
  const png = trayIconPng(TRAY_ICON_POINTS)
  assert.deepEqual(png.subarray(0, 8), PNG_SIGNATURE)

  const ihdr = chunkData(png, 'IHDR')
  assert.equal(ihdr.readUInt32BE(0), TRAY_ICON_POINTS)
  assert.equal(ihdr.readUInt32BE(4), TRAY_ICON_POINTS)
  assert.equal(ihdr[8], 8, 'bit depth')
  assert.equal(ihdr[9], 4, 'colour type: grey + alpha')
})

test('draws an opaque glyph at every scale factor', () => {
  for (const size of [TRAY_ICON_POINTS, TRAY_ICON_POINTS * 2]) {
    const values = alpha(trayIconPng(size), size)
    assert.equal(values.length, size * size)
    assert.ok(
      values.some((value) => value === 255),
      `${size}px icon has no fully covered pixel`,
    )
  }
})

test('draws three bars, shortest at the bottom, inside the square', () => {
  const size = TRAY_ICON_POINTS
  const values = alpha(trayIconPng(size), size)
  const width = (y: number): number =>
    values.slice(y * size, (y + 1) * size).filter((value) => value > 127).length

  const rows = Array.from({ length: size }, (_, y) => width(y))
  const bands: number[][] = []
  for (const row of rows) {
    if (row === 0) bands.push([])
    else (bands.at(-1) ?? bands[bands.push([]) - 1]!).push(row)
  }
  const bars = bands.filter((band) => band.length > 0).map((band) => Math.max(...band))

  assert.equal(bars.length, 3, `expected three bars, got widths ${JSON.stringify(bars)}`)
  assert.ok(bars[0]! > bars[1]! && bars[1]! > bars[2]!, `bars do not taper: ${bars}`)
  // A margin all round, so the glyph is not flush against its neighbours.
  assert.equal(rows[0], 0)
  assert.equal(rows.at(-1), 0)
  assert.ok(bars[0]! < size, 'the widest bar touches both edges')
})
