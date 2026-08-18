import { test } from 'node:test'
import assert from 'node:assert/strict'
import { countChanges, parsePatch, parseUnifiedDiff, toSplitRows } from '../src/shared/diff.ts'

const PATCH = `@@ -1,6 +1,7 @@
 const a = 1
-const b = 2
+const b = 3
+const c = 4
 const d = 5
 const e = 6
 const f = 7`

test('parsePatch numbers lines on both sides', () => {
  const [hunk] = parsePatch(PATCH)
  assert.equal(hunk.oldStart, 1)
  assert.equal(hunk.oldCount, 6)
  assert.equal(hunk.newStart, 1)
  assert.equal(hunk.newCount, 7)

  const kinds = hunk.lines.map((line) => line.kind)
  assert.deepEqual(kinds, ['context', 'del', 'add', 'add', 'context', 'context', 'context'])

  const removed = hunk.lines[1]
  assert.equal(removed.oldLine, 2)
  assert.equal(removed.newLine, undefined)

  const added = hunk.lines[2]
  assert.equal(added.newLine, 2)
  assert.equal(added.oldLine, undefined)

  // The context line after two additions must resume at old 3 / new 4.
  const after = hunk.lines[4]
  assert.equal(after.oldLine, 3)
  assert.equal(after.newLine, 4)
})

test('parsePatch handles a hunk header without counts', () => {
  const [hunk] = parsePatch('@@ -7 +7 @@\n-old\n+new')
  assert.equal(hunk.oldStart, 7)
  assert.equal(hunk.oldCount, 1)
  assert.equal(hunk.newCount, 1)
})

test('parsePatch keeps blank context lines aligned', () => {
  const [hunk] = parsePatch('@@ -1,3 +1,3 @@\n a\n\n-b\n+c')
  const kinds = hunk.lines.map((line) => line.kind)
  assert.deepEqual(kinds, ['context', 'context', 'del', 'add'])
  assert.equal(hunk.lines[2].oldLine, 3)
})

test('parsePatch treats the no-newline marker as metadata', () => {
  const [hunk] = parsePatch('@@ -1,1 +1,1 @@\n-a\n\\ No newline at end of file\n+b')
  assert.equal(hunk.lines[1].kind, 'meta')
  // The marker must not consume a line number.
  assert.equal(hunk.lines[2].newLine, 1)
})

test('toSplitRows zips replacement runs and shares context lines', () => {
  const [hunk] = parsePatch(PATCH)
  const rows = toSplitRows(hunk)

  assert.equal(rows[0].left, rows[0].right)

  // One deletion against two additions: pair the first, leave the second alone.
  assert.equal(rows[1].left?.content, 'const b = 2')
  assert.equal(rows[1].right?.content, 'const b = 3')
  assert.equal(rows[2].left, undefined)
  assert.equal(rows[2].right?.content, 'const c = 4')
})

const MULTI = `diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,2 +1,3 @@
 import x
+import y
 export default x
diff --git a/README.md b/README.md
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Title
+Body
diff --git a/old.txt b/renamed.txt
similarity index 100%
rename from old.txt
rename to renamed.txt
diff --git a/logo.png b/logo.png
index 4444444..5555555 100644
Binary files a/logo.png and b/logo.png differ
`

test('parseUnifiedDiff splits a multi-file blob', () => {
  const files = parseUnifiedDiff(MULTI)
  assert.equal(files.length, 4)

  const [app, readme, renamed, binary] = files

  assert.equal(app.path, 'src/app.ts')
  assert.equal(app.status, 'modified')
  assert.equal(app.additions, 1)
  assert.equal(app.deletions, 0)
  assert.match(app.patch ?? '', /^@@ -1,2 \+1,3 @@/m)
  // A file's patch must not bleed into the next file's.
  assert.doesNotMatch(app.patch ?? '', /# Title/)

  assert.equal(readme.path, 'README.md')
  assert.equal(readme.status, 'added')
  assert.equal(readme.additions, 2)

  assert.equal(renamed.status, 'renamed')
  assert.equal(renamed.oldPath, 'old.txt')
  assert.equal(renamed.path, 'renamed.txt')

  assert.equal(binary.binary, true)
  assert.equal(binary.path, 'logo.png')
})

test('parseUnifiedDiff copes with paths containing spaces', () => {
  const files = parseUnifiedDiff(
    'diff --git a/my dir/a b.ts b/my dir/a b.ts\n--- a/my dir/a b.ts\n+++ b/my dir/a b.ts\n@@ -1 +1 @@\n-x\n+y\n',
  )
  assert.equal(files.length, 1)
  assert.equal(files[0].path, 'my dir/a b.ts')
})

test('parseUnifiedDiff returns nothing for empty input', () => {
  assert.deepEqual(parseUnifiedDiff(''), [])
  assert.deepEqual(parsePatch(''), [])
})

test('countChanges ignores the ---/+++ file headers', () => {
  const counted = countChanges('--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+c')
  assert.deepEqual(counted, { additions: 2, deletions: 1 })
})
