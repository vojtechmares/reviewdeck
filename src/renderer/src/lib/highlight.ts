/**
 * The highlighter itself, and the only place Shiki is imported.
 *
 * It runs on Shiki's JavaScript regular-expression engine rather than the
 * WebAssembly one. The renderer's script policy does not permit compiling
 * WebAssembly, and the workload here - hunks from expanded files - is small enough
 * that the two engines are indistinguishable in practice. Choosing this leaves the
 * content security policy exactly as the README advertises it, and switching later
 * is a single argument below.
 *
 * Grammars are imported when a file that needs one is first expanded, and kept for
 * the session. Shiki's registry is a map of import thunks, so the bundler splits
 * every grammar into a chunk of its own: startup loads none of them, and reviewing
 * a Terraform module one week and a Ruby service the next costs one small chunk
 * each rather than a bundle carrying every language against the chance.
 */

import {
  createHighlighterCore,
  type HighlighterCore,
  type ThemedTokenExplanation,
} from 'shiki/core'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'
import { bundledLanguages } from 'shiki/langs'
import githubDark from 'shiki/themes/github-dark-default.mjs'
import githubLight from 'shiki/themes/github-light-default.mjs'
import type { DiffHunk, DiffLine } from '@shared/diff'
import {
  backgroundFor,
  backgroundRules,
  hunkTokens,
  languageFor,
  paintsBackgrounds,
  sideText,
} from '@shared/highlight'

/** One run of characters sharing a colour. */
export interface Token {
  content: string
  /**
   * `--shiki-light` and `--shiki-dark`, and the `-bg` pair for the few tokens a
   * theme paints a background behind, so the theme toggle stays pure CSS.
   */
  style?: Record<string, string>
}

/**
 * The pair, chosen by measurement rather than taste: of fourteen matched light and
 * dark themes, this is the only one whose every token colour clears 4.5:1 against
 * the diff's own background - and 3:1 for the ones a theme mutes on purpose. It is
 * louder than the graphite palette around it was aiming for. That is the price of
 * `test/color.test.ts` asserting something rather than describing whatever the
 * theme happened to do.
 */
const THEMES = { light: 'github-light-default', dark: 'github-dark-default' } as const

/**
 * The background-setting rules of each theme, against the custom property that
 * carries them to the right side of the theme toggle. Flattened once: a patch asks
 * this question of every token it produces.
 */
const BACKGROUND_RULES = [
  { property: '--shiki-light-bg', rules: backgroundRules(githubLight.tokenColors) },
  { property: '--shiki-dark-bg', rules: backgroundRules(githubDark.tokenColors) },
]

let pending: Promise<HighlighterCore> | null = null

function highlighter(): Promise<HighlighterCore> {
  // The two themes are the only thing loaded up front, because both are needed the
  // moment anything is highlighted at all.
  pending ??= createHighlighterCore({
    themes: [githubLight, githubDark],
    langs: [],
    engine: createJavaScriptRegexEngine({ forgiving: true }),
  })
  return pending
}

/**
 * One promise per language, kept for the session: the same grammar is asked for by
 * every file of that language, in this pull request and the next, and several of
 * them can come into view at once.
 */
const grammars = new Map<string, Promise<boolean>>()

function loadGrammar(shiki: HighlighterCore, language: string): Promise<boolean> {
  let loading = grammars.get(language)
  if (!loading) {
    const grammar = bundledLanguages[language as keyof typeof bundledLanguages]
    loading = grammar
      ? shiki
          .loadLanguage(grammar)
          .then(() => true)
          .catch(() => false)
      : Promise.resolve(false)
    grammars.set(language, loading)
  }
  return loading
}

/**
 * Tokens for every line of every hunk, keyed by the line objects the diff parser
 * produced.
 *
 * Returns an empty map rather than throwing: a grammar that turns out not to exist,
 * or a file that trips the engine up, should cost the reader the colour and nothing
 * else.
 */
export async function highlightHunks(
  hunks: DiffHunk[],
  language: string,
): Promise<Map<DiffLine, Token[]>> {
  const tokens = new Map<DiffLine, Token[]>()

  try {
    const shiki = await highlighter()
    if (!(await loadGrammar(shiki, language))) return tokens

    for (const hunk of hunks) {
      const oldSide = tokenize(shiki, hunk, 'old', language)
      const newSide = tokenize(shiki, hunk, 'new', language)
      for (const [line, lineTokens] of hunkTokens(hunk, oldSide, newSide)) {
        tokens.set(line, lineTokens)
      }
    }
  } catch {
    return new Map()
  }

  return tokens
}

function tokenize(
  shiki: HighlighterCore,
  hunk: DiffHunk,
  side: 'old' | 'new',
  language: string,
): Token[][] {
  const text = sideText(hunk, side)
  if (!text) return []
  return toTokens(shiki, text, language)
}

/**
 * Tokenizing, and the one place the extra question is asked.
 *
 * Asking every token which scopes it matched roughly doubles the cost, so it is
 * asked only of the languages whose grammars can emit a scope a theme paints a
 * background behind. Everything else takes the cheap path and carries colour alone.
 */
function toTokens(shiki: HighlighterCore, text: string, language: string): Token[][] {
  const explained = paintsBackgrounds(language)

  const { tokens } = shiki.codeToTokens(text, {
    lang: language,
    themes: THEMES,
    defaultColor: false,
    ...(explained ? { includeExplanation: 'scopeName' as const } : {}),
  })

  return tokens.map((line) =>
    line.map((token) => ({
      content: token.content,
      style: explained
        ? { ...token.htmlStyle, ...tokenBackground(token.explanation) }
        : token.htmlStyle,
    })),
  )
}

/**
 * The custom properties that paint a token's own background, for the few tokens
 * that have one.
 *
 * A token can span several explanation ranges, and is only merged into one when its
 * style is the same throughout - so the background is painted only where every
 * range agrees on it, and a disagreement leaves the row showing through rather than
 * guessing which half was right.
 */
function tokenBackground(explanation?: ThemedTokenExplanation[]): Record<string, string> {
  if (!explanation?.length) return {}

  const style: Record<string, string> = {}
  for (const { property, rules } of BACKGROUND_RULES) {
    const resolved = explanation.map((range) =>
      backgroundFor(
        rules,
        range.scopes.map((scope) => scope.scopeName),
      ),
    )
    const [first] = resolved
    if (first && resolved.every((background) => background === first)) style[property] = first
  }
  return style
}

/**
 * The language a fence tag means, or null when nothing here knows.
 *
 * The registry answers most of it, aliases included, so ```ts and ```yml resolve
 * without a table of our own. What it does not know is asked of the extension map,
 * which rescues the tags that are really file extensions - ```patch, ```conf - and
 * is the same map the diff resolves paths through. What is left is the short list
 * below: tags people write that are neither, and would otherwise read as prose.
 */
const FENCE_ALIASES: Record<string, string> = {
  golang: 'go',
  'shell-session': 'shellscript',
}

function fenceLanguage(tag: string): string | null {
  const lower = tag.toLowerCase()
  if (lower in bundledLanguages) return lower
  return FENCE_ALIASES[lower] ?? languageFor(`fence.${lower}`)
}

/**
 * Tokens for a fenced code block, or null when the fence carries no language this
 * build can resolve - in which case it stays the preformatted text it already was.
 *
 * The same highlighter, themes and grammar cache as the diff: a snippet in a
 * description and the code it is about are coloured by one thing, and a grammar
 * fetched for either serves both.
 */
export async function highlightCode(code: string, tag: string): Promise<Token[][] | null> {
  const language = fenceLanguage(tag)
  if (!language) return null

  try {
    const shiki = await highlighter()
    if (!(await loadGrammar(shiki, language))) return null

    return toTokens(shiki, code, language)
  } catch {
    return null
  }
}
