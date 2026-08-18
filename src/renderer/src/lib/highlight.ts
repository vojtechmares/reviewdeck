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
 * Grammars are imported statically for now, so this build carries a common set and
 * everything else reads as plain text.
 */

import { createHighlighterCore, type HighlighterCore } from 'shiki/core'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'
import vitesseDark from 'shiki/themes/vitesse-dark.mjs'
import vitesseLight from 'shiki/themes/vitesse-light.mjs'
import c from 'shiki/langs/c.mjs'
import cpp from 'shiki/langs/cpp.mjs'
import csharp from 'shiki/langs/csharp.mjs'
import css from 'shiki/langs/css.mjs'
import docker from 'shiki/langs/docker.mjs'
import go from 'shiki/langs/go.mjs'
import hcl from 'shiki/langs/hcl.mjs'
import html from 'shiki/langs/html.mjs'
import java from 'shiki/langs/java.mjs'
import javascript from 'shiki/langs/javascript.mjs'
import json from 'shiki/langs/json.mjs'
import jsonc from 'shiki/langs/jsonc.mjs'
import jsx from 'shiki/langs/jsx.mjs'
import kotlin from 'shiki/langs/kotlin.mjs'
import make from 'shiki/langs/make.mjs'
import markdown from 'shiki/langs/markdown.mjs'
import php from 'shiki/langs/php.mjs'
import python from 'shiki/langs/python.mjs'
import ruby from 'shiki/langs/ruby.mjs'
import rust from 'shiki/langs/rust.mjs'
import scss from 'shiki/langs/scss.mjs'
import shellscript from 'shiki/langs/shellscript.mjs'
import sql from 'shiki/langs/sql.mjs'
import swift from 'shiki/langs/swift.mjs'
import toml from 'shiki/langs/toml.mjs'
import tsx from 'shiki/langs/tsx.mjs'
import typescript from 'shiki/langs/typescript.mjs'
import xml from 'shiki/langs/xml.mjs'
import yaml from 'shiki/langs/yaml.mjs'
import type { DiffHunk, DiffLine } from '@shared/diff'
import { hunkTokens, sideText } from '@shared/highlight'

/** One run of characters sharing a colour. */
export interface Token {
  content: string
  /** `--shiki-light` and `--shiki-dark`, so the theme toggle is pure CSS. */
  style?: Record<string, string>
}

const THEMES = { light: 'vitesse-light', dark: 'vitesse-dark' } as const

const GRAMMARS = [
  c,
  cpp,
  csharp,
  css,
  docker,
  go,
  hcl,
  html,
  java,
  javascript,
  json,
  jsonc,
  jsx,
  kotlin,
  make,
  markdown,
  php,
  python,
  ruby,
  rust,
  scss,
  shellscript,
  sql,
  swift,
  toml,
  tsx,
  typescript,
  xml,
  yaml,
]

let pending: Promise<HighlighterCore> | null = null

function highlighter(): Promise<HighlighterCore> {
  pending ??= createHighlighterCore({
    themes: [vitesseLight, vitesseDark],
    langs: GRAMMARS,
    engine: createJavaScriptRegexEngine({ forgiving: true }),
  })
  return pending
}

/**
 * Tokens for every line of every hunk, keyed by the line objects the diff parser
 * produced.
 *
 * Returns an empty map rather than throwing: a grammar this build turns out not to
 * carry, or a file that trips the engine up, should cost the reader the colour and
 * nothing else.
 */
export async function highlightHunks(
  hunks: DiffHunk[],
  language: string,
): Promise<Map<DiffLine, Token[]>> {
  const tokens = new Map<DiffLine, Token[]>()

  try {
    const shiki = await highlighter()

    for (const hunk of hunks) {
      const oldSide = tokenize(shiki, hunk, 'old', language)
      const newSide = tokenize(shiki, hunk, 'new', language)
      for (const [line, line_tokens] of hunkTokens(hunk, oldSide, newSide)) {
        tokens.set(line, line_tokens)
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

  const { tokens } = shiki.codeToTokens(text, {
    lang: language,
    themes: THEMES,
    defaultColor: false,
  })

  return tokens.map((line) =>
    line.map((token) => ({ content: token.content, style: token.htmlStyle })),
  )
}
