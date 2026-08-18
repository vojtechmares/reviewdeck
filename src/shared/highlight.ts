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
 * What to tokenise a file as.
 *
 * Owned code rather than a library's guess, because the awkward cases are the ones
 * that matter here: compound extensions, files that carry no extension at all, and
 * the configuration formats that turn up in every infrastructure repository and get
 * left out of curated bundles.
 *
 * A name that is not in here resolves to nothing and renders plain. That is a
 * deliberate boundary: guessing a language from an unknown extension would tokenise
 * a file as something it is not, which reads worse than not colouring it.
 */
const BY_EXTENSION: Record<string, string> = {
  adoc: 'asciidoc',
  ahk: 'ahk',
  applescript: 'applescript',
  as: 'actionscript-3',
  asm: 'asm',
  astro: 'astro',
  awk: 'awk',
  bash: 'shellscript',
  bat: 'bat',
  bicep: 'bicep',
  c: 'c',
  cc: 'cpp',
  cfg: 'ini',
  cjs: 'javascript',
  clj: 'clojure',
  cljc: 'clojure',
  cljs: 'clojure',
  cmake: 'cmake',
  cmd: 'bat',
  cob: 'cobol',
  coffee: 'coffee',
  conf: 'ini',
  cpp: 'cpp',
  cs: 'csharp',
  cshtml: 'razor',
  css: 'css',
  csv: 'csv',
  cts: 'typescript',
  cxx: 'cpp',
  d: 'd',
  dart: 'dart',
  diff: 'diff',
  ejs: 'erb',
  elm: 'elm',
  erl: 'erlang',
  ex: 'elixir',
  exs: 'elixir',
  fish: 'fish',
  fs: 'fsharp',
  fsx: 'fsharp',
  gemspec: 'ruby',
  gleam: 'gleam',
  glsl: 'glsl',
  go: 'go',
  gql: 'graphql',
  gradle: 'groovy',
  graphql: 'graphql',
  groovy: 'groovy',
  h: 'c',
  handlebars: 'handlebars',
  haxe: 'haxe',
  hbs: 'handlebars',
  hcl: 'hcl',
  hh: 'cpp',
  hlsl: 'hlsl',
  hpp: 'cpp',
  hrl: 'erlang',
  hs: 'haskell',
  htm: 'html',
  html: 'html',
  http: 'http',
  hxx: 'cpp',
  ini: 'ini',
  java: 'java',
  jl: 'julia',
  js: 'javascript',
  json: 'json',
  json5: 'json5',
  jsonc: 'jsonc',
  jsonl: 'json',
  jsx: 'jsx',
  kt: 'kotlin',
  kts: 'kotlin',
  latex: 'latex',
  less: 'less',
  liquid: 'liquid',
  lua: 'lua',
  m: 'objective-c',
  markdown: 'markdown',
  md: 'markdown',
  mdx: 'mdx',
  mjs: 'javascript',
  mk: 'make',
  ml: 'ocaml',
  mli: 'ocaml',
  mm: 'objective-cpp',
  move: 'move',
  mts: 'typescript',
  nim: 'nim',
  nix: 'nix',
  odin: 'odin',
  patch: 'diff',
  pas: 'pascal',
  php: 'php',
  pl: 'perl',
  plist: 'xml',
  pm: 'perl',
  powershell: 'powershell',
  pp: 'puppet',
  prisma: 'prisma',
  pro: 'prolog',
  properties: 'properties',
  proto: 'proto',
  ps1: 'powershell',
  psm1: 'powershell',
  pug: 'pug',
  purs: 'purescript',
  py: 'python',
  pyi: 'python',
  r: 'r',
  rake: 'ruby',
  rb: 'ruby',
  rs: 'rust',
  rst: 'rst',
  s: 'asm',
  sass: 'sass',
  sc: 'scala',
  scala: 'scala',
  scm: 'scheme',
  scss: 'scss',
  sh: 'shellscript',
  sol: 'solidity',
  sql: 'sql',
  styl: 'stylus',
  svelte: 'svelte',
  svg: 'xml',
  swift: 'swift',
  tcl: 'tcl',
  tex: 'latex',
  tf: 'hcl',
  tfvars: 'hcl',
  toml: 'toml',
  ts: 'typescript',
  tsv: 'csv',
  tsx: 'tsx',
  twig: 'twig',
  v: 'v',
  vb: 'vb',
  vhd: 'vhdl',
  vhdl: 'vhdl',
  vim: 'viml',
  vue: 'vue',
  wat: 'wasm',
  wgsl: 'wgsl',
  xml: 'xml',
  xsd: 'xml',
  xsl: 'xml',
  yaml: 'yaml',
  yml: 'yaml',
  zig: 'zig',
  zsh: 'shellscript',
}

/** Files that carry no extension at all but are still unambiguous. */
const BY_FILENAME: Record<string, string> = {
  '.bash_profile': 'shellscript',
  '.bashrc': 'shellscript',
  '.gitconfig': 'ini',
  '.profile': 'shellscript',
  '.zshrc': 'shellscript',
  'cmakelists.txt': 'cmake',
  brewfile: 'ruby',
  dockerfile: 'docker',
  gemfile: 'ruby',
  gnumakefile: 'make',
  jenkinsfile: 'groovy',
  justfile: 'make',
  makefile: 'make',
  podfile: 'ruby',
  rakefile: 'ruby',
  vagrantfile: 'ruby',
}

/** Every language `languageFor` can name, so the registry can be checked against it. */
export const HIGHLIGHT_LANGUAGES: string[] = [
  ...new Set([...Object.values(BY_EXTENSION), ...Object.values(BY_FILENAME)]),
].sort()

/**
 * The language to tokenise a path as, or null when this app has no mapping for it -
 * in which case the file renders as plain text rather than as an error.
 */
export function languageFor(path: string): string | null {
  const name = path.slice(path.lastIndexOf('/') + 1).toLowerCase()
  if (!name) return null

  const byName = BY_FILENAME[name]
  if (byName) return byName

  // `Dockerfile.dev` is still a Dockerfile, and `Makefile.common` still a makefile.
  const base = name.slice(0, name.indexOf('.'))
  if (base && BY_FILENAME[base]) return BY_FILENAME[base]

  // The last extension wins, so `component.test.ts` and `types.d.ts` resolve.
  const dot = name.lastIndexOf('.')
  if (dot <= 0) return null
  return BY_EXTENSION[name.slice(dot + 1)] ?? null
}
