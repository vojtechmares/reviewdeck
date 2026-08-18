import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import githubDark from 'shiki/themes/github-dark-default.mjs'
import githubLight from 'shiki/themes/github-light-default.mjs'
import {
  BLACK_BACKDROP,
  WHITE_BACKDROP,
  composite,
  contrastRatio,
  parseColor,
  relativeLuminance,
  type Rgba,
} from '../src/shared/color.ts'
import {
  backgroundFor,
  backgroundRules,
  paintsBackgrounds,
  type ThemeTokenRule,
} from '../src/shared/highlight.ts'

test('parseColor reads the hex a theme emits, in every width', () => {
  assert.deepEqual(parseColor('#fff'), { r: 1, g: 1, b: 1, a: 1 })
  assert.deepEqual(parseColor('#000000'), { r: 0, g: 0, b: 0, a: 1 })
  assert.equal(parseColor('#80808080').a, 128 / 255)
  assert.deepEqual(parseColor('#f00f'), parseColor('#ff0000'))
})

test('parseColor reads the oklch the app writes', () => {
  const white = parseColor('oklch(1 0 0)')
  assert.equal(Math.round(white.r * 255), 255)
  assert.equal(Math.round(white.g * 255), 255)
  assert.equal(Math.round(white.b * 255), 255)

  assert.equal(parseColor('oklch(0.5 0.1 200 / 0.42)').a, 0.42)
  assert.equal(parseColor('oklch(50% 0.1 200 / 42%)').a, 0.42)
})

test('parseColor throws rather than defaulting, so nothing is asserted by accident', () => {
  assert.throws(() => parseColor('rebeccapurple'))
  assert.throws(() => parseColor('#12345'))
})

test('composite lays films over a backdrop, and insists on an opaque result', () => {
  assert.deepEqual(composite([BLACK_BACKDROP, WHITE_BACKDROP]), WHITE_BACKDROP)

  const half = composite([BLACK_BACKDROP, { r: 1, g: 1, b: 1, a: 0.5 }])
  assert.equal(half.a, 1)
  assert.ok(Math.abs(half.r - 0.5) < 1e-9)

  assert.throws(() => composite([{ r: 1, g: 1, b: 1, a: 0.5 }]))
})

test('contrastRatio is WCAG, and does not care which colour is given first', () => {
  assert.equal(contrastRatio(WHITE_BACKDROP, BLACK_BACKDROP), 21)
  assert.equal(contrastRatio(BLACK_BACKDROP, WHITE_BACKDROP), 21)
  assert.equal(contrastRatio(WHITE_BACKDROP, WHITE_BACKDROP), 1)
  assert.equal(relativeLuminance(WHITE_BACKDROP), 1)
})

/*
 * The legibility assertion.
 *
 * The window is glass over a desktop the app cannot see, so what sits behind a
 * syntax token depends on somebody's wallpaper. The wallpaper is bounded rather
 * than sampled: every film the app paints is composited source-over, so the result
 * is monotonic in the backdrop and every possible desktop lands between what pure
 * white gives and what pure black gives. Assert both extremes and the whole space
 * is covered, by construction.
 *
 * Values come out of `index.css` rather than being restated here, so the assertion
 * is about the app as shipped and cannot drift away from it.
 */

const CSS = readFileSync(new URL('../src/renderer/src/index.css', import.meta.url), 'utf8')

/** The body of a top-level rule in `index.css`, by selector. */
function cssBlock(selector: string): string {
  const found = CSS.match(new RegExp(`^${selector} \\{\\n([\\s\\S]*?)^\\}`, 'm'))
  assert.ok(found, `index.css has no top-level ${selector} block`)
  return found[1]
}

const BLOCKS = { light: cssBlock(':root'), dark: cssBlock('\\.dark') }

type Theme = 'light' | 'dark'
type Row = 'context' | 'add' | 'del'
type Backdrop = 'white' | 'black'

function property(theme: Theme, name: string): string {
  const found = BLOCKS[theme].match(new RegExp(`^  --${name}: ([^;]+);`, 'm'))
  assert.ok(found, `index.css defines no --${name} for the ${theme} theme`)
  return found[1]
}

/** The alpha of a custom property, which is the knob everything here turns. */
function alphaOf(value: string): number {
  return parseColor(value).a
}

const BASE_ALPHA = alphaOf(property('light', 'diff-code-base'))
const BACKDROPS: Record<Backdrop, Rgba> = { white: WHITE_BACKDROP, black: BLACK_BACKDROP }
const THEMES = { light: githubLight, dark: githubDark }

/**
 * What sits behind a line of code, bottom first: the desktop, the app's own film,
 * the file panel's glass, the code column's base film, and the row tint over it.
 *
 * Vibrancy blurs and saturates what is behind the window, and both are the identity
 * on a uniform achromatic backdrop - which is exactly what the two extremes are - so
 * neither shifts these numbers.
 */
function rowStack(theme: Theme, row: Row, backdrop: Backdrop, baseAlpha: number): Rgba[] {
  const base = parseColor(property(theme, 'diff-code-base'))
  const stack = [
    BACKDROPS[backdrop],
    parseColor(property(theme, 'background')),
    parseColor(property(theme, 'surface')),
    { ...base, a: baseAlpha },
  ]
  if (row !== 'context') stack.push(parseColor(property(theme, `diff-${row}`)))
  return stack
}

/**
 * Every colour the theme can put on a token, against the scope that asks for it.
 *
 * Rules that set a background are left out: those tokens are painted with the
 * theme's own background behind them, which occludes everything under it, and they
 * are asserted separately against that.
 */
function tokenColours(theme: Theme): { scope: string; color: string }[] {
  const raw = THEMES[theme] as { colors?: Record<string, string>; tokenColors?: ThemeTokenRule[] }
  const colours = [{ scope: 'editor.foreground', color: raw.colors?.['editor.foreground'] ?? '' }]

  for (const rule of raw.tokenColors ?? []) {
    const foreground = rule.settings?.foreground
    if (!foreground || rule.settings?.background) continue
    const scopes = typeof rule.scope === 'string' ? [rule.scope] : (rule.scope ?? [])
    for (const scope of scopes) colours.push({ scope, color: foreground })
  }
  return colours
}

/**
 * The bar a scope has to clear, off the theme's own scope names and nothing else.
 *
 * Comments and punctuation are muted deliberately - holding them to 4.5:1 would mean
 * overriding the very colours that made this theme worth choosing - so they take
 * WCAG's large-text bar instead. Everything that carries meaning takes 4.5:1.
 */
function barFor(scope: string): number {
  return /(^|\.)(comment|punctuation)(\.|$)/.test(scope) ? 3 : 4.5
}

/** Every way the diff fails to be legible at this base alpha, named one by one. */
function legibilityFailures(baseAlpha: number): string[] {
  const failures: string[] = []

  for (const theme of ['light', 'dark'] as const) {
    for (const row of ['context', 'add', 'del'] as const) {
      for (const backdrop of ['white', 'black'] as const) {
        const background = composite(rowStack(theme, row, backdrop, baseAlpha))
        for (const { scope, color } of tokenColours(theme)) {
          const bar = barFor(scope)
          const ratio = contrastRatio(composite([background, parseColor(color)]), background)
          if (ratio < bar) {
            failures.push(
              `${theme} ${scope} ${color} on a ${row} row over a ${backdrop} desktop: ` +
                `${ratio.toFixed(2)}:1, needs ${bar}:1`,
            )
          }
        }
      }
    }
  }
  return failures
}

/** The floor a changed row has to stay above, so the change is still the loudest thing. */
const SEPARATION = 1.1

/** Every way a changed row stops being obviously changed at this base alpha. */
function separationFailures(baseAlpha: number): string[] {
  const failures: string[] = []

  for (const theme of ['light', 'dark'] as const) {
    for (const backdrop of ['white', 'black'] as const) {
      const context = composite(rowStack(theme, 'context', backdrop, baseAlpha))
      for (const row of ['add', 'del'] as const) {
        const changed = composite(rowStack(theme, row, backdrop, baseAlpha))
        const ratio = contrastRatio(changed, context)
        if (ratio < SEPARATION) {
          failures.push(
            `${theme} ${row} row over a ${backdrop} desktop sits ${ratio.toFixed(3)}:1 ` +
              `from context, needs ${SEPARATION}:1`,
          )
        }
      }
    }
  }
  return failures
}

test('every token colour is legible over every row, theme and desktop', () => {
  assert.deepEqual(legibilityFailures(BASE_ALPHA), [])
})

test('added and removed rows stay obviously changed over every desktop', () => {
  assert.deepEqual(separationFailures(BASE_ALPHA), [])
})

test('the base film is as thin as the assertion allows, and both themes share it', () => {
  assert.equal(
    alphaOf(property('dark', 'diff-code-base')),
    BASE_ALPHA,
    'the two themes should thicken their code columns by the same amount',
  )

  // Derived, not chosen: one hundredth thinner and the diff stops being legible, so
  // this is the lowest the code column can be and still bound the desktop out.
  const thinner = Number((BASE_ALPHA - 0.01).toFixed(2))
  assert.notDeepEqual(
    [...legibilityFailures(thinner), ...separationFailures(thinner)],
    [],
    `--diff-code-base could be ${thinner} rather than ${BASE_ALPHA}; thin it out`,
  )
})

/*
 * The scopes a theme paints a background behind.
 *
 * Shiki emits foregrounds only, so these five rules per theme would otherwise
 * render with a foreground chosen for a background nothing paints. The renderer
 * paints them, which makes them backdrop-independent by construction: the theme's
 * backgrounds are opaque, so they occlude the glass, the row tint and the desktop
 * alike, and the contrast is simply one of the theme's colours over another.
 */

/*
 * `carriage-return` is excluded by name, and only this one.
 *
 * It is 2.32:1 in dark on the theme's own colours - below even the quiet bar, with
 * no glass anywhere near it. It is a control-character marker rather than a syntax
 * token, and its foreground and background are a pair the theme picked together, so
 * the renderer shows that pair verbatim rather than second-guessing a theme it
 * chose for being measurable.
 */
const UNMEASURED_SCOPE = 'carriage-return'

test('the scopes that carry their own background are legible on it', () => {
  const failures: string[] = []

  for (const theme of ['light', 'dark'] as const) {
    const raw = THEMES[theme] as { tokenColors?: ThemeTokenRule[] }
    let measured = 0

    for (const rule of raw.tokenColors ?? []) {
      const { foreground, background } = rule.settings ?? {}
      if (!foreground || !background) continue
      const scopes = typeof rule.scope === 'string' ? [rule.scope] : (rule.scope ?? [])
      if (scopes.includes(UNMEASURED_SCOPE)) continue

      measured += 1
      const ratio = contrastRatio(parseColor(foreground), parseColor(background))
      if (ratio < 4.5) {
        failures.push(`${theme} ${scopes[0]} ${foreground} on ${background}: ${ratio.toFixed(2)}:1`)
      }
    }

    assert.equal(measured, 4, `${theme} should paint four background-carrying scopes`)
  }

  assert.deepEqual(failures, [])
})

test('a background is resolved from the scopes a token matched, not from its colour', () => {
  const rules = backgroundRules((githubLight as { tokenColors?: ThemeTokenRule[] }).tokenColors)

  assert.equal(backgroundFor(rules, ['source.diff', 'markup.deleted.diff']), '#ffebe9')
  assert.equal(backgroundFor(rules, ['source.diff', 'markup.inserted.diff']), '#dafbe1')

  // `entity.name.tag` is `#116329`, the same colour `markup.inserted` is - so a
  // lookup keyed on the foreground would paint green behind every JSX tag name.
  assert.equal(backgroundFor(rules, ['source.tsx', 'meta.tag.tsx', 'entity.name.tag.tsx']), undefined)

  // A prefix of a scope matches it; a longer name that merely starts the same does not.
  assert.equal(backgroundFor(rules, ['markup.deletedish']), undefined)
})

test('only the languages that can emit those scopes pay for asking', () => {
  assert.equal(paintsBackgrounds('diff'), true)
  assert.equal(paintsBackgrounds('markdown'), true)
  assert.equal(paintsBackgrounds('typescript'), false)
  assert.equal(paintsBackgrounds('python'), false)
})

test('the renderer highlights with the pair this file measures', () => {
  const source = readFileSync(new URL('../src/renderer/src/lib/highlight.ts', import.meta.url), 'utf8')
  assert.match(source, /light: 'github-light-default', dark: 'github-dark-default'/)
})
