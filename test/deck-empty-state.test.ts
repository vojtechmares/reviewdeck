import { test } from 'node:test'
import assert from 'node:assert/strict'
import { deckEmptyState } from '../src/shared/types.ts'

const cold = { accountCount: 1, synced: false, visibleCount: 0, filtersActive: false }

test('a deck with no accounts says so straight away, synced or not', () => {
  assert.equal(deckEmptyState({ ...cold, accountCount: 0 }), 'no-accounts')
  assert.equal(deckEmptyState({ ...cold, accountCount: 0, synced: true }), 'no-accounts')
})

test('an unsynced deck is checking, never inbox zero', () => {
  assert.equal(deckEmptyState(cold), 'syncing')
  assert.equal(deckEmptyState({ ...cold, filtersActive: true }), 'syncing')
})

test('inbox zero waits for a sync that genuinely returned nothing', () => {
  assert.equal(deckEmptyState({ ...cold, synced: true }), 'inbox-zero')
})

test('filters take precedence over inbox zero once synced', () => {
  assert.equal(deckEmptyState({ ...cold, synced: true, filtersActive: true }), 'no-matches')
})

test('reviews to show beat every empty state except a missing account', () => {
  assert.equal(deckEmptyState({ ...cold, visibleCount: 3 }), null)
  assert.equal(deckEmptyState({ ...cold, synced: true, visibleCount: 3 }), null)
  assert.equal(
    deckEmptyState({ ...cold, synced: true, visibleCount: 3, filtersActive: true }),
    null,
  )
})
