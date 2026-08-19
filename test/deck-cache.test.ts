import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DECK_CACHE_VERSION, readDeckCache } from '../src/shared/deck-cache.ts'
import type { ReviewItem } from '../src/shared/types.ts'

function review(number: number): ReviewItem {
  return {
    id: `acc:acme/design-tokens:${number}`,
    accountId: 'acc',
    provider: 'github',
    repoKey: 'acme/design-tokens',
    repo: 'acme/design-tokens',
    number,
    title: `Pull request ${number}`,
    url: `https://example.test/pull/${number}`,
    author: { name: 'lpeters', avatarUrl: '' },
    createdAt: '2026-08-01T10:00:00Z',
    updatedAt: '2026-08-01T10:00:00Z',
    draft: false,
    sourceBranch: 'topic',
    targetBranch: 'main',
    labels: [],
    myReviewState: 'pending',
    checks: { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] },
  }
}

const written = { version: DECK_CACHE_VERSION, items: { acc: [review(1), review(2)] } }

test('readDeckCache reads back what was written, keyed by account', () => {
  const cache = readDeckCache(structuredClone(written))
  assert.equal(cache.version, DECK_CACHE_VERSION)
  assert.deepEqual(Object.keys(cache.items), ['acc'])
  assert.deepEqual(cache.items['acc'], [review(1), review(2)])
})

test('readDeckCache starts empty rather than throwing on a vault with no deck', () => {
  assert.deepEqual(readDeckCache(undefined).items, {})
  assert.deepEqual(readDeckCache(null).items, {})
})

test('readDeckCache discards a cache written by another build', () => {
  assert.deepEqual(readDeckCache({ ...written, version: DECK_CACHE_VERSION + 1 }).items, {})
  assert.deepEqual(readDeckCache({ ...written, version: undefined }).items, {})
  assert.deepEqual(readDeckCache({ ...written, version: '1' }).items, {})
})

test('readDeckCache discards a cache whose shape is not what it writes', () => {
  assert.deepEqual(readDeckCache('reviews').items, {})
  assert.deepEqual(readDeckCache([review(1)]).items, {})
  assert.deepEqual(readDeckCache({ version: DECK_CACHE_VERSION, items: [] }).items, {})
  assert.deepEqual(readDeckCache({ version: DECK_CACHE_VERSION }).items, {})
})

test('readDeckCache drops an account whose entry is not a list of reviews', () => {
  const cache = readDeckCache({
    version: DECK_CACHE_VERSION,
    items: { acc: [review(1)], broken: { number: 7 } },
  })
  assert.deepEqual(Object.keys(cache.items), ['acc'])
})

test('readDeckCache drops the items that would not draw and keeps the rest', () => {
  const cache = readDeckCache({
    version: DECK_CACHE_VERSION,
    items: {
      acc: [
        review(1),
        null,
        'not an item',
        { ...review(2), author: undefined },
        { ...review(3), checks: { status: 'passed' } },
        { ...review(4), labels: 'ux' },
        { ...review(5), number: '5' },
        review(6),
      ],
    },
  })
  assert.deepEqual(
    cache.items['acc']?.map((item) => item.number),
    [1, 6],
  )
})
