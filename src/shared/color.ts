/**
 * Colour arithmetic, so the legibility of the diff is asserted rather than
 * eyeballed.
 *
 * The window is frosted glass over the desktop, which means the colour behind a
 * syntax token depends on a wallpaper the app can neither see nor control. That
 * looks unmeasurable, and is not: a wallpaper is a variable that can be *bounded*
 * rather than sampled. Everything the app paints over it is a translucent film, so
 * for any given stack of films the composited result is monotonic in the backdrop -
 * every possible desktop lands between what pure white behind the window gives and
 * what pure black gives. Assert the contrast bar at both extremes and every
 * wallpaper in existence is covered, by construction.
 *
 * That turns a judgement call into a test, and makes the base film behind the code
 * columns a *derived* value - whatever alpha makes the assertion hold - rather than
 * a number somebody picked and had to defend.
 *
 * Nothing here imports the highlighter, the renderer or electron: this is
 * arithmetic over colour notations, and the test that matters runs it against the
 * theme's own JSON with no browser anywhere near it. The oklch conversion is
 * hand-rolled for the same reason the rest of the app has an empty `dependencies`
 * object - it is thirty lines of matrix multiplication, and a colour library would
 * be a runtime dependency carried for them.
 */

/** A colour in gamma-encoded sRGB. Every channel and the alpha in 0..1. */
export interface Rgba {
  r: number
  g: number
  b: number
  a: number
}

/** Pure white and pure black behind the window: the two ends of every wallpaper. */
export const WHITE_BACKDROP: Rgba = { r: 1, g: 1, b: 1, a: 1 }
export const BLACK_BACKDROP: Rgba = { r: 0, g: 0, b: 0, a: 1 }

function clamp(value: number): number {
  return value < 0 ? 0 : value > 1 ? 1 : value
}

/**
 * A colour in one of the two notations this app actually writes: the `oklch()` its
 * own tokens are defined in, and the hex a Shiki theme emits - three, four, six or
 * eight digits, the last of those carrying alpha.
 *
 * Anything else throws rather than resolving to a default. A test that quietly
 * accepted a notation it could not read would assert nothing at all.
 */
export function parseColor(value: string): Rgba {
  const text = value.trim()
  if (text.startsWith('#')) return parseHex(text)
  if (text.toLowerCase().startsWith('oklch(')) return parseOklch(text)
  throw new Error(`Unsupported colour notation: ${value}`)
}

function parseHex(text: string): Rgba {
  const digits = text.slice(1)
  if (!/^[0-9a-fA-F]+$/.test(digits)) throw new Error(`Not a hex colour: ${text}`)

  // The three- and four-digit forms are the six- and eight-digit ones with every
  // digit doubled, so widening first leaves one parser below.
  const wide =
    digits.length === 3 || digits.length === 4
      ? digits
          .split('')
          .map((digit) => digit + digit)
          .join('')
      : digits
  if (wide.length !== 6 && wide.length !== 8) throw new Error(`Not a hex colour: ${text}`)

  const byte = (index: number): number => parseInt(wide.slice(index * 2, index * 2 + 2), 16) / 255
  return { r: byte(0), g: byte(1), b: byte(2), a: wide.length === 8 ? byte(3) : 1 }
}

function parseOklch(text: string): Rgba {
  const inside = text.slice(text.indexOf('(') + 1, text.lastIndexOf(')'))
  const [coords, alpha] = inside.split('/')
  const parts = coords.trim().split(/\s+/)
  if (parts.length !== 3) throw new Error(`Not an oklch colour: ${text}`)

  const [lightness, chroma, hue] = parts.map(number)
  return { ...oklchToSrgb(lightness, chroma, hue), a: alpha === undefined ? 1 : number(alpha) }
}

/** A component, with the percentage form meaning the same as its 0..1 fraction. */
function number(token: string): number {
  const text = token.trim()
  const value = Number.parseFloat(text)
  if (Number.isNaN(value)) throw new Error(`Not a number: ${token}`)
  return text.endsWith('%') ? value / 100 : value
}

/**
 * Oklch to gamma-encoded sRGB: polar to cartesian, the inverse of the Oklab
 * transform to linear sRGB, then the sRGB transfer function.
 *
 * Coefficients are Björn Ottosson's, as CSS Color 4 specifies them. Channels are
 * clamped rather than gamut-mapped, which is what a browser does with an
 * out-of-gamut `oklch()` on an sRGB display and therefore what the assertion needs
 * to agree with. The app's own colours are all comfortably inside sRGB anyway.
 */
function oklchToSrgb(lightness: number, chroma: number, hue: number): Omit<Rgba, 'a'> {
  const radians = (hue * Math.PI) / 180
  const a = chroma * Math.cos(radians)
  const b = chroma * Math.sin(radians)

  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3

  return {
    r: encode(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    g: encode(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    b: encode(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  }
}

/** The sRGB transfer function: linear light to the value a channel is stored as. */
function encode(linear: number): number {
  const value = linear <= 0.0031308 ? 12.92 * linear : 1.055 * Math.abs(linear) ** (1 / 2.4) - 0.055
  return clamp(value)
}

/**
 * A stack of layers flattened to the one opaque colour a screen shows.
 *
 * Layers are given **bottom first**: the backdrop, then each film painted over it.
 * This is `source-over` on gamma-encoded channels, which is what a browser does
 * with plain alpha rather than any wide-gamut interpolation.
 *
 * The result must come out opaque - contrast against a colour that is still partly
 * transparent is not a number, it is a question about what is underneath - so a
 * stack that does not start with an opaque layer throws.
 */
export function composite(layers: Rgba[]): Rgba {
  if (layers.length === 0) throw new Error('Nothing to composite')

  let out = layers[0]
  for (const over of layers.slice(1)) {
    const alpha = over.a + out.a * (1 - over.a)
    if (alpha === 0) {
      out = { r: 0, g: 0, b: 0, a: 0 }
      continue
    }
    const mix = (top: number, under: number): number =>
      (top * over.a + under * out.a * (1 - over.a)) / alpha
    out = {
      r: mix(over.r, out.r),
      g: mix(over.g, out.g),
      b: mix(over.b, out.b),
      a: alpha,
    }
  }

  if (out.a < 1 - 1e-9) throw new Error('Composited stack is not opaque; the bottom layer must be')
  return { ...out, a: 1 }
}

/** WCAG relative luminance. The alpha is ignored: this wants an opaque colour. */
export function relativeLuminance(color: Rgba): number {
  const linear = (channel: number): number =>
    channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  return 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/** WCAG contrast ratio between two opaque colours, 1 to 21, order-independent. */
export function contrastRatio(one: Rgba, other: Rgba): number {
  const a = relativeLuminance(one)
  const b = relativeLuminance(other)
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}
