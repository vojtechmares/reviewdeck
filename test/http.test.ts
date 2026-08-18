import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseNextLink, toOrigin } from '../src/main/http.ts'

test('parseNextLink picks the next relation out of a Link header', () => {
  const header =
    '<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=9>; rel="last"'
  assert.equal(parseNextLink(header), 'https://api.github.com/x?page=2')
})

test('parseNextLink returns null when there is no next page', () => {
  assert.equal(parseNextLink('<https://api.github.com/x?page=1>; rel="prev"'), null)
  assert.equal(parseNextLink(null), null)
})

test('toOrigin normalises whatever the user typed', () => {
  assert.equal(toOrigin('gitlab.com'), 'https://gitlab.com')
  assert.equal(toOrigin('https://gitlab.com/'), 'https://gitlab.com')
  assert.equal(toOrigin('  git.acme.dev  '), 'https://git.acme.dev')
  // A path prefix matters for instances hosted under a sub-path.
  assert.equal(toOrigin('https://acme.dev/git/'), 'https://acme.dev/git')
  assert.equal(toOrigin('http://localhost:3000'), 'http://localhost:3000')
})

test('toOrigin rejects an empty host', () => {
  assert.throws(() => toOrigin('   '), /host is required/)
})
