import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  DEFAULT_SETTINGS,
  approvalsAfterApproving,
  isVisibleReview,
  mergeSettings,
  visibleReviews,
  type ApprovalSummary,
  type ReviewItem,
  type Settings,
} from '../src/shared/types.ts'
import { countApprovers, restrictionCovers, summariseApprovals } from '../src/main/providers/types.ts'
import { DECK_CACHE_VERSION, readDeckCache } from '../src/shared/deck-cache.ts'

function review(id: string, approvals: ApprovalSummary, patch: Partial<ReviewItem> = {}): ReviewItem {
  return {
    id,
    accountId: 'acc',
    provider: 'gitlab',
    repoKey: '2841',
    repo: 'acme/platform/edge-router',
    number: Number(id),
    title: `Merge request ${id}`,
    url: `https://example.test/merge_requests/${id}`,
    author: { name: 'dnovak', avatarUrl: '' },
    createdAt: '2026-08-01T10:00:00Z',
    updatedAt: '2026-08-01T10:00:00Z',
    draft: false,
    sourceBranch: 'topic',
    targetBranch: 'main',
    labels: [],
    myReviewState: 'pending',
    approvals,
    checks: { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] },
    ...patch,
  }
}

const noneRequired = review('1', { given: 1, outcome: 'none_required' })
const short = review('2', { given: 1, required: 2, outcome: 'pending' })
const satisfied = review('3', { given: 2, required: 2, outcome: 'satisfied' })

test('a fully approved review is hidden by default', () => {
  assert.equal(DEFAULT_SETTINGS.hideFullyApproved, true)
  assert.equal(isVisibleReview(satisfied, DEFAULT_SETTINGS), false)
})

test('a review still short of approvals, or on a host asking for none, stays', () => {
  assert.equal(isVisibleReview(short, DEFAULT_SETTINGS), true)
  assert.equal(isVisibleReview(noneRequired, DEFAULT_SETTINGS), true)
})

test('turning the preference off shows the fully approved ones again', () => {
  const showing: Settings = { ...DEFAULT_SETTINGS, hideFullyApproved: false }
  assert.deepEqual(
    visibleReviews([noneRequired, short, satisfied], showing).map((item) => item.id),
    ['1', '2', '3'],
  )
})

test('the preference is on for a vault written before it existed', () => {
  const older = { theme: 'dark', hideApproved: true } as Partial<Settings>
  const merged = mergeSettings(older)
  assert.equal(merged.hideFullyApproved, true)
  assert.equal(merged.hideApproved, true)
})

test('approving moves the count at once and settles the outcome only with a figure', () => {
  assert.deepEqual(approvalsAfterApproving({ given: 1, required: 2, outcome: 'pending' }, false), {
    given: 2,
    required: 2,
    outcome: 'satisfied',
  })
  assert.deepEqual(approvalsAfterApproving({ given: 1, required: 3, outcome: 'pending' }, false), {
    given: 2,
    required: 3,
    outcome: 'pending',
  })
  // A host that gave a verdict but no figure keeps its verdict until asked again.
  assert.deepEqual(approvalsAfterApproving({ given: 0, outcome: 'pending' }, false), {
    given: 1,
    outcome: 'pending',
  })
  // Nothing required stays nothing required, however many approve.
  assert.deepEqual(approvalsAfterApproving({ given: 0, outcome: 'none_required' }, false), {
    given: 1,
    outcome: 'none_required',
  })
  // Approving again is not a second approval.
  const already: ApprovalSummary = { given: 1, required: 2, outcome: 'pending' }
  assert.deepEqual(approvalsAfterApproving(already, true), already)
})

test('summariseApprovals reads a missing or zero requirement as none', () => {
  assert.deepEqual(summariseApprovals(2, undefined), { given: 2, required: undefined, outcome: 'none_required' })
  assert.deepEqual(summariseApprovals(2, 0), { given: 2, required: 0, outcome: 'none_required' })
  assert.deepEqual(summariseApprovals(1, 2), { given: 1, required: 2, outcome: 'pending' })
  assert.deepEqual(summariseApprovals(3, 2), { given: 3, required: 2, outcome: 'satisfied' })
})

test('countApprovers takes each reviewer at their latest verdict', () => {
  assert.equal(
    countApprovers([
      { login: 'ana', state: 'APPROVED' },
      { login: 'ben', state: 'CHANGES_REQUESTED' },
      { login: 'ben', state: 'COMMENTED' },
      { login: 'ben', state: 'APPROVED' },
      { login: 'cyd', state: 'APPROVED' },
      { login: 'cyd', state: 'DISMISSED' },
      { login: 'dee', state: 'COMMENTED' },
      { login: undefined, state: 'APPROVED' },
    ]),
    2,
  )
})

test('a Bitbucket restriction pattern is a glob with one wildcard', () => {
  assert.equal(restrictionCovers('main', 'main'), true)
  assert.equal(restrictionCovers('release/*', 'release/2.4'), true)
  assert.equal(restrictionCovers('release/*', 'main'), false)
  assert.equal(restrictionCovers('*', 'anything/at/all'), true)
  assert.equal(restrictionCovers('v1.0', 'v1x0'), false)
})

test('a deck cached without approvals is discarded rather than drawn with holes', () => {
  const { approvals: _dropped, ...older } = satisfied
  const cache = readDeckCache({ version: DECK_CACHE_VERSION, items: { acc: [older, short] } })
  assert.deepEqual(cache.items.acc.map((item) => item.id), ['2'])
})
