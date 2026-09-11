import { test } from 'node:test'
import assert from 'node:assert/strict'
import { commentTargets, parsePatch } from '../src/shared/diff.ts'

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
