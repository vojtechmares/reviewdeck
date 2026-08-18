import { test } from 'node:test'
import assert from 'node:assert/strict'
import type { Element, Nodes, Root } from 'hast'
import remarkParse from 'remark-parse'
import remarkRehype from 'remark-rehype'
import { unified } from 'unified'
import {
  alertKindOf,
  REHYPE_PLUGINS,
  REMARK_PLUGINS,
  SANITIZE_SCHEMA,
} from '../src/shared/markdown.ts'

/**
 * The same processor `react-markdown` builds internally: parse, our remark
 * plugins, the bridge to hast with raw HTML kept, then our rehype plugins. The
 * unified toolchain needs no DOM, so this runs in plain node and asserts on the
 * tree rather than on rendered markup that is still being iterated.
 */
async function render(source: string): Promise<Root> {
  const processor = unified()
    .use(remarkParse)
    .use(REMARK_PLUGINS)
    .use(remarkRehype, { allowDangerousHtml: true })
    .use(REHYPE_PLUGINS)
  return processor.run(processor.parse(source)) as Promise<Root>
}

function children(node: Nodes): Nodes[] {
  return 'children' in node ? node.children : []
}

function elements(tree: Nodes, tagName: string): Element[] {
  const found: Element[] = []
  const walk = (node: Nodes): void => {
    if (node.type === 'element' && node.tagName === tagName) found.push(node)
    for (const child of children(node)) walk(child)
  }
  walk(tree)
  return found
}

function text(tree: Nodes): string {
  if (tree.type === 'text') return tree.value
  return children(tree).map(text).join('')
}

/** Everything a sanitizer failure could leave behind, flattened into one haystack. */
function flatten(tree: Nodes): string {
  const parts: string[] = []
  const walk = (node: Nodes): void => {
    if (node.type === 'element') parts.push(node.tagName, JSON.stringify(node.properties))
    if (node.type === 'text') parts.push(node.value)
    for (const child of children(node)) walk(child)
  }
  walk(tree)
  return parts.join(' ')
}

test('the pipeline renders the block structure a description relies on', async () => {
  const tree = await render(
    ['# Title', '', '> quoted', '', '---', '', '- one', '- two', '', '1. first'].join('\n'),
  )

  assert.equal(elements(tree, 'h1').length, 1)
  assert.equal(text(elements(tree, 'blockquote')[0]).trim(), 'quoted')
  assert.equal(elements(tree, 'hr').length, 1)
  assert.equal(elements(tree, 'ul')[0].children.filter((c) => c.type === 'element').length, 2)
  assert.equal(elements(tree, 'ol').length, 1)
})

test('the pipeline renders GFM tables with their column alignment', async () => {
  const tree = await render(
    ['| Left | Centre | Right |', '| :--- | :----: | ----: |', '| a | b | c |'].join('\n'),
  )

  const headers = elements(tree, 'th')
  assert.deepEqual(
    headers.map((cell) => cell.properties.align),
    ['left', 'center', 'right'],
  )
  assert.equal(elements(tree, 'td').length, 3)
})

test('the pipeline renders task lists carrying their checked state', async () => {
  const tree = await render(['- [x] done', '- [ ] outstanding'].join('\n'))

  const boxes = elements(tree, 'input')
  assert.deepEqual(
    boxes.map((box) => box.properties.type),
    ['checkbox', 'checkbox'],
  )
  assert.equal(boxes[0].properties.checked, true)
  assert.equal(boxes[1].properties.checked, undefined)
  // Disabled, so a description cannot be edited by clicking it.
  assert.equal(boxes[0].properties.disabled, true)
})

test('the pipeline renders strikethrough, inline code and bare-URL autolinks', async () => {
  const tree = await render('~~dropped~~ and `--flag` and https://example.test/page')

  assert.equal(text(elements(tree, 'del')[0]), 'dropped')
  assert.equal(text(elements(tree, 'code')[0]), '--flag')

  const link = elements(tree, 'a')[0]
  assert.equal(link.properties.href, 'https://example.test/page')
})

test('the pipeline keeps a fenced block as preformatted text tagged with its language', async () => {
  const tree = await render(['```ts', 'const a = 1', '```'].join('\n'))

  const code = elements(tree, 'pre')[0].children[0] as Element
  assert.equal(code.tagName, 'code')
  assert.deepEqual(code.properties.className, ['language-ts'])
  assert.equal(text(code), 'const a = 1\n')
})

test('the sanitizer keeps the HTML real pull request bodies rely on', async () => {
  const tree = await render(
    [
      '<details open><summary>Compatibility</summary>',
      '',
      '| Package | Change |',
      '| --- | --- |',
      '| a | 1.0<br>2.0 |',
      '',
      '</details>',
      '',
      '<img src="https://example.test/shot.png" width="120" alt="a screenshot">',
      '',
      'Press <kbd>Cmd</kbd>.',
    ].join('\n'),
  )

  const details = elements(tree, 'details')[0]
  assert.equal(details.properties.open, true)
  assert.equal(text(elements(tree, 'summary')[0]), 'Compatibility')

  // A line break inside a table cell keeps a bot report's layout.
  assert.equal(elements(tree, 'br').length, 1)

  const image = elements(tree, 'img')[0]
  assert.equal(image.properties.src, 'https://example.test/shot.png')
  assert.equal(image.properties.width, 120)
  assert.equal(image.properties.alt, 'a screenshot')

  assert.equal(text(elements(tree, 'kbd')[0]), 'Cmd')
})

test('the sanitizer removes scripts, event handlers and dangerous URL schemes', async () => {
  const tree = await render(
    [
      '<script>alert(1)</script>',
      '',
      '<img src="x" onerror="alert(2)">',
      '',
      '<a href="javascript:alert(3)" onclick="alert(4)">click</a>',
      '',
      '<a href="data:text/html;base64,PHNjcmlwdD4=">data</a>',
      '',
      '<a href="vbscript:alert(5)">vbscript</a>',
      '',
      '<iframe src="https://evil.test"></iframe>',
      '',
      '<object data="https://evil.test"></object>',
      '',
      '<form action="https://evil.test"><input type="text" name="stolen"></form>',
    ].join('\n'),
  )

  const rendered = flatten(tree)
  for (const tagName of ['script', 'iframe', 'object', 'form']) {
    assert.equal(elements(tree, tagName).length, 0, `${tagName} survived sanitization`)
  }
  assert.equal(rendered.includes('alert('), false, 'an event handler survived sanitization')
  for (const scheme of ['javascript:', 'data:', 'vbscript:']) {
    assert.equal(rendered.includes(scheme), false, `${scheme} survived sanitization`)
  }

  // The anchors themselves stay - only their unusable targets are dropped.
  const hrefs = elements(tree, 'a').map((anchor) => anchor.properties.href)
  assert.deepEqual(hrefs, [undefined, undefined, undefined])

  // A free-text input would be a credential prompt inside someone else's prose,
  // so whatever survives is forced back into a disabled checkbox.
  for (const box of elements(tree, 'input')) {
    assert.equal(box.properties.type, 'checkbox')
    assert.equal(box.properties.disabled, true)
  }
})

test('the sanitizer namespaces ids so a description cannot shadow the app own', async () => {
  const tree = await render('<div id="deck-search">borrowed</div>')

  assert.equal(elements(tree, 'div')[0].properties.id, 'user-content-deck-search')
  assert.equal(SANITIZE_SCHEMA.clobberPrefix, 'user-content-')
})

test('the sanitize schema pins the elements the app depends on', () => {
  for (const tagName of ['details', 'summary', 'kbd', 'img', 'br', 'table']) {
    assert.ok(SANITIZE_SCHEMA.tagNames?.includes(tagName), `${tagName} is not permitted`)
  }
  assert.deepEqual(
    SANITIZE_SCHEMA.tagNames,
    [...new Set(SANITIZE_SCHEMA.tagNames)],
    'the tag name list has duplicates',
  )
})

/** The alert kind of each block quote the source produces, in order. */
async function alertKinds(source: string): Promise<(string | null)[]> {
  const tree = await render(source)
  return elements(tree, 'blockquote').map(alertKindOf)
}

test('alertKindOf recognises every alert marker', async () => {
  assert.deepEqual(
    await alertKinds(
      [
        '> [!NOTE]',
        '> Useful information.',
        '',
        '> [!TIP]',
        '> A shortcut.',
        '',
        '> [!IMPORTANT]',
        '> Do not miss this.',
        '',
        '> [!WARNING]',
        '> Urgent.',
        '',
        '> [!CAUTION]',
        '> Risky.',
      ].join('\n'),
    ),
    ['note', 'tip', 'important', 'warning', 'caution'],
  )
})

test('alertKindOf recognises a marker standing alone in its own paragraph', async () => {
  // A blank quoted line puts the marker and the body in separate paragraphs.
  assert.deepEqual(await alertKinds(['> [!WARNING]', '>', '> Urgent.'].join('\n')), ['warning'])
})

test('alertKindOf leaves an ordinary block quote alone', async () => {
  assert.deepEqual(await alertKinds('> Just something someone said.'), [null])
  assert.deepEqual(await alertKinds('> [not a marker] and then some.'), [null])
})

test('alertKindOf refuses a malformed or unknown marker', async () => {
  assert.deepEqual(
    await alertKinds(
      [
        '> [!BOGUS]',
        '> not a kind that exists',
        '',
        '> [!NOTE',
        '> unclosed',
        '',
        '> !NOTE]',
        '> no opening bracket',
        '',
        '> [!NOTE] trailing text on the marker line',
        '> so the marker does not stand alone',
      ].join('\n'),
    ),
    [null, null, null, null],
  )
})
