import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  DEFAULT_SETTINGS,
  isVisibleReview,
  mergeSettings,
  visibleReviews,
  type MyReviewState,
  type ReviewItem,
  type Settings,
} from '../src/shared/types.ts'

function review(id: string, myReviewState: MyReviewState): ReviewItem {
  return {
    id,
    accountId: 'acc',
    provider: 'github',
    repoKey: 'acme/design-tokens',
    repo: 'acme/design-tokens',
    number: Number(id),
    title: `Pull request ${id}`,
    url: `https://example.test/pull/${id}`,
    author: { name: 'lpeters', avatarUrl: '' },
    createdAt: '2026-08-01T10:00:00Z',
    updatedAt: '2026-08-01T10:00:00Z',
    draft: false,
    sourceBranch: 'topic',
    targetBranch: 'main',
    labels: [],
    myReviewState,
    checks: { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] },
  }
}

const settings = (hideApproved: boolean): Settings => ({ ...DEFAULT_SETTINGS, hideApproved })

test('mergeSettings fills in everything the stored vault does not carry', () => {
  assert.deepEqual(mergeSettings({}), DEFAULT_SETTINGS)
  assert.deepEqual(mergeSettings(undefined), DEFAULT_SETTINGS)
  assert.deepEqual(mergeSettings(null), DEFAULT_SETTINGS)

  const merged = mergeSettings({ diffView: 'unified' })
  assert.equal(merged.diffView, 'unified')
  assert.equal(merged.pollInterval, DEFAULT_SETTINGS.pollInterval)
})

test('mergeSettings keeps a stored value even when it matches no default', () => {
  const merged = mergeSettings({ notificationsEnabled: false, playSound: false })
  assert.equal(merged.notificationsEnabled, false)
  assert.equal(merged.playSound, false)
})

test('mergeSettings loads a vault written before a setting was removed', () => {
  // `showWhitespace` was defined and defaulted but read nowhere, so it was removed.
  // A vault written by a build that still had it must keep loading.
  const older = { diffView: 'unified', showWhitespace: true, theme: 'dark' } as Partial<Settings>

  const merged = mergeSettings(older)

  assert.equal(merged.diffView, 'unified')
  assert.equal(merged.theme, 'dark')
  assert.equal(merged.hideApproved, DEFAULT_SETTINGS.hideApproved)
})

test('the settings type no longer carries the whitespace-display key', () => {
  assert.equal(Object.hasOwn(DEFAULT_SETTINGS, 'showWhitespace'), false)
})

test('the menu bar shows its count unless somebody turns it off', () => {
  assert.equal(DEFAULT_SETTINGS.showMenuBarCount, true)
  assert.equal(mergeSettings({ showMenuBarCount: false }).showMenuBarCount, false)
})

test('a vault written before the quiet menu bar existed keeps its count', () => {
  const older = { theme: 'dark', hideApproved: true } as Partial<Settings>

  const merged = mergeSettings(older)

  assert.equal(merged.showMenuBarCount, true)
  assert.equal(merged.hideApproved, true)
})

test('the approved filter is off by default, so nothing is hidden', () => {
  const item = review('1', 'approved')
  assert.equal(isVisibleReview(item, DEFAULT_SETTINGS), true)
})

test('a review the user approved is hidden only while the setting is on', () => {
  const item = review('1', 'approved')

  assert.equal(isVisibleReview(item, settings(true)), false)
  assert.equal(isVisibleReview(item, settings(false)), true)
})

test('every other review state survives the approved filter', () => {
  for (const state of ['pending', 'changes_requested', 'commented'] as MyReviewState[]) {
    assert.equal(isVisibleReview(review('1', state), settings(true)), true, state)
  }
})

test('the count the menu bar carries is the count the window lists', () => {
  const items = [
    review('1', 'pending'),
    review('2', 'approved'),
    review('3', 'commented'),
    review('4', 'approved'),
  ]

  assert.equal(visibleReviews(items, settings(false)).length, 4)

  const shown = visibleReviews(items, settings(true))
  assert.deepEqual(
    shown.map((item) => item.id),
    ['1', '3'],
  )
})

test('a deck of nothing but approved reviews counts zero, so the menu bar shows no number', () => {
  const items = [review('1', 'approved'), review('2', 'approved')]

  assert.equal(visibleReviews(items, settings(true)).length, 0)
  assert.equal(visibleReviews([], settings(false)).length, 0)
})
