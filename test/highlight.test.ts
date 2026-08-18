import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parsePatch, toSplitRows } from '../src/shared/diff.ts'
import {
  hunkTokens,
  languageFor,
  sideLines,
  sideText,
  HIGHLIGHT_LANGUAGES,
} from '../src/shared/highlight.ts'

/** One token line per source line, standing in for whatever the highlighter emits. */
function tokenLines(text: string): string[][] {
  return text.split('\n').map((line) => [line])
}

test('sideText reconstructs each side from the lines that belong to it', () => {
  const [hunk] = parsePatch(
    ['@@ -1,4 +1,4 @@', ' const a = 1', '-const b = 2', '+const b = 3', ' const c = 4'].join('\n'),
  )

  assert.equal(sideText(hunk, 'old'), 'const a = 1\nconst b = 2\nconst c = 4')
  assert.equal(sideText(hunk, 'new'), 'const a = 1\nconst b = 3\nconst c = 4')
})

test('sideText leaves meta lines out, which would otherwise shift every line after them', () => {
  const [hunk] = parsePatch(
    ['@@ -1,2 +1,2 @@', '-a', '\\ No newline at end of file', '+b', ' c'].join('\n'),
  )

  assert.equal(sideText(hunk, 'old'), 'a\nc')
  assert.equal(sideText(hunk, 'new'), 'b\nc')
  assert.equal(sideLines(hunk, 'old').length, 2)
})

test('sideText keeps a blank context line, so the lines after it stay aligned', () => {
  const [hunk] = parsePatch(['@@ -1,4 +1,4 @@', ' a', '', ' b', '-c', '+d'].join('\n'))

  assert.equal(sideText(hunk, 'old'), 'a\n\nb\nc')
  assert.equal(sideText(hunk, 'new'), 'a\n\nb\nd')
})

test('hunkTokens zips an interleaved replacement run onto the right lines', () => {
  const [hunk] = parsePatch(
    [
      '@@ -1,5 +1,5 @@',
      ' keep one',
      '-old two',
      '-old three',
      '+new two',
      '+new three',
      ' keep four',
    ].join('\n'),
  )

  const tokens = hunkTokens(
    hunk,
    tokenLines(sideText(hunk, 'old')),
    tokenLines(sideText(hunk, 'new')),
  )

  // Every line gets the tokens of its own text, not of the line beside it.
  for (const line of hunk.lines) {
    assert.deepEqual(tokens.get(line), [line.content], `mismatched on ${line.content}`)
  }
})

test('hunkTokens handles a hunk that is nothing but additions', () => {
  const [hunk] = parsePatch(['@@ -0,0 +1,3 @@', '+one', '+two', '+three'].join('\n'))

  const tokens = hunkTokens(hunk, tokenLines(sideText(hunk, 'old')), tokenLines(sideText(hunk, 'new')))

  assert.equal(sideText(hunk, 'old'), '')
  assert.deepEqual(
    hunk.lines.map((line) => tokens.get(line)),
    [['one'], ['two'], ['three']],
  )
})

test('hunkTokens handles a hunk that is nothing but deletions', () => {
  const [hunk] = parsePatch(['@@ -1,3 +0,0 @@', '-one', '-two', '-three'].join('\n'))

  const tokens = hunkTokens(hunk, tokenLines(sideText(hunk, 'old')), tokenLines(sideText(hunk, 'new')))

  assert.equal(sideText(hunk, 'new'), '')
  assert.deepEqual(
    hunk.lines.map((line) => tokens.get(line)),
    [['one'], ['two'], ['three']],
  )
})

test('hunkTokens leaves a line the highlighter said nothing about unmapped', () => {
  const [hunk] = parsePatch(['@@ -1,2 +1,2 @@', ' a', '-b', '+c'].join('\n'))

  // Two lines on the old side, one token line: the second stays plain rather than
  // borrowing tokens that belong to another line.
  const tokens = hunkTokens(hunk, [['a']], tokenLines(sideText(hunk, 'new')))

  assert.deepEqual(tokens.get(hunk.lines[0]), ['a'])
  assert.equal(tokens.get(hunk.lines[1]), undefined)
})

test('hunkTokens lands on the right cell of every split row', () => {
  const [hunk] = parsePatch(
    ['@@ -1,4 +1,4 @@', ' shared', '-removed', '+added', ' tail'].join('\n'),
  )

  const tokens = hunkTokens(hunk, tokenLines(sideText(hunk, 'old')), tokenLines(sideText(hunk, 'new')))
  const rows = toSplitRows(hunk)

  assert.deepEqual(
    rows.map((row) => [
      row.left && tokens.get(row.left),
      row.right && tokens.get(row.right),
    ]),
    [
      [['shared'], ['shared']],
      [['removed'], ['added']],
      [['tail'], ['tail']],
    ],
  )
})

test('hunkTokens gives a split row with only one side nothing for the other', () => {
  const [hunk] = parsePatch(['@@ -1,1 +1,3 @@', ' a', '+b', '+c'].join('\n'))

  const tokens = hunkTokens(hunk, tokenLines(sideText(hunk, 'old')), tokenLines(sideText(hunk, 'new')))
  const rows = toSplitRows(hunk)

  assert.deepEqual(
    rows.map((row) => [
      row.left ? tokens.get(row.left) : null,
      row.right ? tokens.get(row.right) : null,
    ]),
    [
      [['a'], ['a']],
      [null, ['b']],
      [null, ['c']],
    ],
  )
})

test('languageFor resolves the languages a review actually turns up', () => {
  assert.equal(languageFor('src/main/index.ts'), 'typescript')
  assert.equal(languageFor('src/App.tsx'), 'tsx')
  assert.equal(languageFor('internal/capture.go'), 'go')
  assert.equal(languageFor('infra/main.tf'), 'hcl')
  assert.equal(languageFor('infra/prod.tfvars'), 'hcl')
  assert.equal(languageFor('app/models/user.rb'), 'ruby')
  assert.equal(languageFor('src/Controller.php'), 'php')
  assert.equal(languageFor('src/lib.rs'), 'rust')
  assert.equal(languageFor('scripts/release.sh'), 'shellscript')
  assert.equal(languageFor('db/schema.sql'), 'sql')
})

test('languageFor resolves the configuration formats every repository has', () => {
  assert.equal(languageFor('deploy/values.YAML'), 'yaml')
  assert.equal(languageFor('.github/workflows/ci.yml'), 'yaml')
  assert.equal(languageFor('Cargo.toml'), 'toml')
  assert.equal(languageFor('tsconfig.json'), 'json')
  assert.equal(languageFor('.vscode/settings.jsonc'), 'jsonc')
  assert.equal(languageFor('setup.cfg'), 'ini')
  assert.equal(languageFor('nginx.conf'), 'ini')
  assert.equal(languageFor('gradle.properties'), 'properties')
  assert.equal(languageFor('infra/network.bicep'), 'bicep')
  assert.equal(languageFor('api/service.proto'), 'proto')
})

test('languageFor takes the last extension, so a compound name still resolves', () => {
  assert.equal(languageFor('src/shared/types.d.ts'), 'typescript')
  assert.equal(languageFor('test/diff.test.ts'), 'typescript')
  assert.equal(languageFor('docker-compose.override.yml'), 'yaml')
})

test('languageFor resolves well-known names that carry no extension', () => {
  assert.equal(languageFor('Dockerfile'), 'docker')
  assert.equal(languageFor('build/Dockerfile.dev'), 'docker')
  assert.equal(languageFor('Makefile'), 'make')
  assert.equal(languageFor('GNUmakefile'), 'make')
  assert.equal(languageFor('Rakefile'), 'ruby')
  assert.equal(languageFor('Gemfile'), 'ruby')
  assert.equal(languageFor('Jenkinsfile'), 'groovy')
  assert.equal(languageFor('CMakeLists.txt'), 'cmake')
  assert.equal(languageFor('.zshrc'), 'shellscript')
})

test('languageFor gives up on anything else rather than throwing', () => {
  assert.equal(languageFor('LICENSE'), null)
  assert.equal(languageFor('.gitignore'), null)
  assert.equal(languageFor('assets/logo.sketch'), null)
  assert.equal(languageFor(''), null)
  assert.equal(languageFor('some/dir/'), null)
})

test('every language the detector can name is one the registry can actually load', async () => {
  // Only the registry of import thunks is read here, not a single grammar: this is
  // the guard that catches a language id that does not exist, which would otherwise
  // show up as one file quietly rendering plain and nothing saying why.
  const { bundledLanguages } = await import('shiki/langs')

  const unknown = HIGHLIGHT_LANGUAGES.filter((language) => !(language in bundledLanguages))
  assert.deepEqual(unknown, [], `not in the registry: ${unknown.join(', ')}`)

  for (const path of ['a.ts', 'a.tf', 'Dockerfile', 'Makefile', 'a.kt', 'a.jsonc']) {
    const language = languageFor(path)
    assert.ok(language && HIGHLIGHT_LANGUAGES.includes(language), `${path} -> ${language}`)
  }
  assert.deepEqual(HIGHLIGHT_LANGUAGES, [...new Set(HIGHLIGHT_LANGUAGES)].sort())
})
