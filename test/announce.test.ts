import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  DEFAULT_SETTINGS,
  idsToRecord,
  reviewsToAnnounce,
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

const hidingApproved: Settings = { ...DEFAULT_SETTINGS, hideApproved: true }

const waiting = review('1', 'pending')
const approved = review('2', 'approved')
const deck = [waiting, approved]

test('a new review the settings hide raises nothing, and one they show is announced', () => {
  const fresh = idsToRecord(deck)
  assert.deepEqual(
    reviewsToAnnounce(deck, fresh, hidingApproved).map((item) => item.id),
    [waiting.id],
  )
})

test('a hidden review is recorded as seen all the same', () => {
  // Both halves of one sync: everything it found is seen, only what it will show
  // is announced. Recording just the announced set would leave the approved one
  // waiting to fire the day the preference is turned off.
  assert.deepEqual(idsToRecord(deck), [waiting.id, approved.id])
  assert.deepEqual(
    reviewsToAnnounce(deck, idsToRecord(deck), hidingApproved).map((item) => item.id),
    [waiting.id],
  )
})

test('announcing follows the rule the menu bar counts by, not one of its own', () => {
  const fresh = idsToRecord(deck)
  for (const settings of [DEFAULT_SETTINGS, hidingApproved]) {
    assert.deepEqual(
      reviewsToAnnounce(deck, fresh, settings),
      visibleReviews(deck, settings),
      'everything visible is new this sync, so the two sets have to match',
    )
  }
})

test('a review that is not new this sync is not announced again', () => {
  assert.deepEqual(reviewsToAnnounce(deck, [], DEFAULT_SETTINGS), [])
  assert.deepEqual(
    reviewsToAnnounce(deck, [approved.id], DEFAULT_SETTINGS).map((item) => item.id),
    [approved.id],
  )
  assert.deepEqual(reviewsToAnnounce(deck, [approved.id], hidingApproved), [])
})

test('announcing keeps the deck order, so a grouped banner lists what the deck lists', () => {
  const many = ['5', '4', '3'].map((id) => review(id, 'pending'))
  assert.deepEqual(
    reviewsToAnnounce(many, idsToRecord(many).toReversed(), DEFAULT_SETTINGS).map((item) => item.id),
    ['5', '4', '3'],
  )
})
