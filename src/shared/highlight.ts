/**
 * Putting syntax tokens back on diff lines.
 *
 * A diff is two interleaved versions of a file, and neither is valid source on its
 * own, so each side of a hunk is reconstructed and tokenized separately: the old
 * side from context and deleted lines, the new side from context and added ones.
 * The highlighter hands tokens back already grouped per line, which is what makes
 * putting them back a zip rather than character-offset arithmetic.
 *
 * Nothing here imports the highlighter - the zip is generic over whatever a token
 * turns out to be - which keeps it out of the main-process bundle and out of the
 * way of dependency externalisation, and lets it be tested directly. That matters
 * more here than almost anywhere else in the app, because wrong output still looks
 * like syntax highlighting.
 *
 * Accepted limitation: a hunk that starts inside a multi-line construct can
 * mis-tokenize, because the hunk is all the text there is. This is what the hosts
 * themselves do, and it goes away if whole files ever become readable.
 */

import type { DiffHunk, DiffLine } from './diff.ts'

export type DiffSide = 'old' | 'new'

/**
 * The lines of a hunk that exist on one side, in order.
 *
 * Meta lines - the no-trailing-newline marker - are not source and would shift
 * every token line after them by one.
 */
export function sideLines(hunk: DiffHunk, side: DiffSide): DiffLine[] {
  const changed = side === 'old' ? 'del' : 'add'
  return hunk.lines.filter((line) => line.kind === 'context' || line.kind === changed)
}

/** The source text one side of a hunk represents, for the highlighter to read. */
export function sideText(hunk: DiffHunk, side: DiffSide): string {
  return sideLines(hunk, side)
    .map((line) => line.content)
    .join('\n')
}

/**
 * Token lines back onto the diff lines they came from, keyed by the line objects
 * the diff parser produced - which is what both the unified and the split layouts
 * hand around, the split one sharing a single context-line object across both of
 * its columns.
 *
 * A line the highlighter produced nothing for is simply absent, so it renders
 * plain rather than rendering wrong.
 */
export function hunkTokens<T>(hunk: DiffHunk, oldSide: T[][], newSide: T[][]): Map<DiffLine, T[]> {
  const tokens = new Map<DiffLine, T[]>()

  // Old first, so a context line keeps the new side's tokens: that is the text the
  // reader is looking at, and the two can differ around a changed line.
  for (const side of ['old', 'new'] as const) {
    const lines = sideLines(hunk, side)
    const produced = side === 'old' ? oldSide : newSide
    lines.forEach((line, index) => {
      const tokenLine = produced[index]
      if (tokenLine) tokens.set(line, tokenLine)
    })
  }

  return tokens
}

/**
 * The languages this build carries grammars for. Universal coverage is its own
 * change; until then an unlisted file reads as plain text, which is the same thing
 * it read as before.
 */
const BY_EXTENSION: Record<string, string> = {
  bash: 'shellscript',
  c: 'c',
  cjs: 'javascript',
  cpp: 'cpp',
  cs: 'csharp',
  css: 'css',
  cts: 'typescript',
  go: 'go',
  h: 'c',
  hcl: 'hcl',
  hpp: 'cpp',
  htm: 'html',
  html: 'html',
  java: 'java',
  js: 'javascript',
  json: 'json',
  jsonc: 'jsonc',
  jsx: 'jsx',
  kt: 'kotlin',
  kts: 'kotlin',
  markdown: 'markdown',
  md: 'markdown',
  mjs: 'javascript',
  mts: 'typescript',
  php: 'php',
  py: 'python',
  rb: 'ruby',
  rs: 'rust',
  scss: 'scss',
  sh: 'shellscript',
  sql: 'sql',
  swift: 'swift',
  tf: 'hcl',
  tfvars: 'hcl',
  toml: 'toml',
  ts: 'typescript',
  tsx: 'tsx',
  xml: 'xml',
  yaml: 'yaml',
  yml: 'yaml',
  zsh: 'shellscript',
}

/** Files that carry no extension at all but are still unambiguous. */
const BY_FILENAME: Record<string, string> = {
  dockerfile: 'docker',
  makefile: 'make',
}

/** Every language `languageFor` can name, so the grammar set can be checked against it. */
export const HIGHLIGHT_LANGUAGES: string[] = [
  ...new Set([...Object.values(BY_EXTENSION), ...Object.values(BY_FILENAME)]),
].sort()

/**
 * The language to tokenize a path as, or null when this build cannot - in which
 * case the file renders as plain text rather than as an error.
 */
export function languageFor(path: string): string | null {
  const name = path.slice(path.lastIndexOf('/') + 1).toLowerCase()
  if (!name) return null

  const byName = BY_FILENAME[name]
  if (byName) return byName

  // `Dockerfile.dev` is still a Dockerfile.
  const base = name.slice(0, name.indexOf('.'))
  if (base && BY_FILENAME[base]) return BY_FILENAME[base]

  // The last extension wins, so `component.test.ts` and `types.d.ts` resolve.
  const dot = name.lastIndexOf('.')
  if (dot <= 0) return null
  return BY_EXTENSION[name.slice(dot + 1)] ?? null
}
