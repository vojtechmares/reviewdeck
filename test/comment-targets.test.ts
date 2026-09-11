import { test } from 'node:test'
import assert from 'node:assert/strict'
import { commentTargets, coversLine, parsePatch, rangeTarget } from '../src/shared/diff.ts'

const PATCH = `@@ -1,3 +1,3 @@
 const a = 1
-const b = 2
+const b = 3
 const d = 5`

const [hunk] = parsePatch(PATCH)
const [context, removed, added] = hunk.lines

test('an added line can be commented on, addressed by its new line number', () => {
  assert.deepEqual(commentTargets('src/a.ts', added), [{ path: 'src/a.ts', newLine: 2 }])
})

test('a removed line can be commented on, addressed by its old line number', () => {
  assert.deepEqual(commentTargets('src/a.ts', removed), [{ path: 'src/a.ts', oldLine: 2 }])
})

test('a context line can be commented on from either side', () => {
  assert.deepEqual(commentTargets('src/a.ts', context), [
    { path: 'src/a.ts', oldLine: 1 },
    { path: 'src/a.ts', newLine: 1 },
  ])
})

test('a "no newline" annotation is not a line and takes no comment', () => {
  const [{ lines }] = parsePatch('@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+a\n')
  assert.deepEqual(commentTargets('src/a.ts', lines[1]), [])
})

// --- ranges ---

/**
 * Where each line sits in both files as the diff counts it, which is what GitLab's
 * line codes are built from. An added line takes the old-file number it was
 * inserted ahead of, a removed line the new-file number that follows it.
 */
test('every line records its position in both files, added and removed lines included', () => {
  const [{ lines }] = parsePatch(`@@ -1,3 +1,4 @@
 a
-b
+b2
+c
 d`)
  assert.deepEqual(
    lines.map((line) => [line.kind, line.oldPos, line.newPos]),
    [
      ['context', 1, 1],
      ['del', 2, 2],
      ['add', 3, 2],
      ['add', 3, 3],
      ['context', 3, 4],
    ],
  )
})

const RANGE_PATCH = `@@ -10,4 +10,5 @@
 keep
-gone
+here
+more
 tail
@@ -30,2 +31,2 @@
 far
+away`

const [first, second] = parsePatch(RANGE_PATCH)
const [keep, gone, here, more, tail] = first.lines

test('a range covers every line between two picks on one side, whichever was picked first', () => {
  const forward = rangeTarget('src/a.ts', first, 'new', keep, more)
  assert.deepEqual(forward, {
    path: 'src/a.ts',
    newLine: 12,
    range: {
      startLine: 10,
      start: { kind: 'context', oldPos: 10, newPos: 10 },
      // The old position is what a removed line above it moved the counter to.
      end: { kind: 'add', oldPos: 12, newPos: 12 },
    },
  })
  assert.deepEqual(rangeTarget('src/a.ts', first, 'new', more, keep), forward)
})

test('a range on the old side is addressed by old line numbers', () => {
  assert.deepEqual(rangeTarget('src/a.ts', first, 'old', gone, tail), {
    path: 'src/a.ts',
    oldLine: 12,
    range: {
      startLine: 11,
      start: { kind: 'del', oldPos: 11, newPos: 11 },
      end: { kind: 'context', oldPos: 12, newPos: 13 },
    },
  })
})

test('a pick that never left its line is an ordinary single-line comment', () => {
  assert.deepEqual(rangeTarget('src/a.ts', first, 'new', here, here), {
    path: 'src/a.ts',
    newLine: 11,
  })
})

test('a range cannot take in a line that is not on its side', () => {
  // The removed line has no new number, so a range on the new side cannot end there.
  assert.equal(rangeTarget('src/a.ts', first, 'new', keep, gone), undefined)
  assert.equal(rangeTarget('src/a.ts', first, 'old', here, tail), undefined)
})

test('a range cannot reach into another hunk, whose lines the diff does not show between', () => {
  const [far] = second.lines
  assert.equal(rangeTarget('src/a.ts', first, 'new', keep, far), undefined)
  assert.equal(rangeTarget('src/a.ts', second, 'new', keep, far), undefined)
})

test('coversLine reads a range by number on its side and a single line as itself', () => {
  const anchor = { side: 'new' as const, line: 12, startLine: 10 }
  assert.deepEqual(
    first.lines.map((line) => coversLine(anchor, line)),
    [true, false, true, true, false],
  )
  // A removed line is not on the new side at all, wherever the range is.
  assert.equal(coversLine(anchor, gone), false)

  const single = { side: 'old' as const, line: 11 }
  assert.deepEqual(
    first.lines.map((line) => coversLine(single, line)),
    [false, true, false, false, false],
  )
})
