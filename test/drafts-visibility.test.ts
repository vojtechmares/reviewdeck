import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  DEFAULT_SETTINGS,
  isDraftReview,
  isVisibleReview,
  mergeSettings,
  visibleReviews,
  type MyReviewState,
  type ReviewItem,
  type Settings,
} from '../src/shared/types.ts'

function review(id: string, patch: Partial<ReviewItem> = {}): ReviewItem {
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
    myReviewState: 'pending' as MyReviewState,
    checks: { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] },
    ...patch,
  }
}

const titled = (title: string): ReviewItem => review('1', { title })

test('the host flag alone marks a draft, whatever the title says', () => {
  assert.equal(isDraftReview(review('1', { draft: true })), true)
  assert.equal(isDraftReview(review('1', { draft: true, title: 'Ready to go' })), true)
})

test('a title that opens with the convention marks a draft on a host with no flag', () => {
  // Bitbucket Cloud has no draft concept at all, so this is the only signal there.
  for (const title of [
    'draft: cache the tokens',
    'Draft: cache the tokens',
    'DRAFT: cache the tokens',
    'wip: cache the tokens',
    'WIP: cache the tokens',
    '[draft] cache the tokens',
    '[DRAFT] cache the tokens',
    '[wip] cache the tokens',
    '[WIP] cache the tokens',
    '   WIP: leading whitespace is still a prefix',
  ]) {
    assert.equal(isDraftReview(titled(title)), true, title)
  }
})

test('a title that merely mentions the words is not a draft', () => {
  for (const title of [
    'Fix the wip: handler',
    'Rewrite the draft: parser',
    'Drafts of the release notes',
    'Add draft mode to the editor',
    'Explain [WIP] in the contributing guide',
    'draft',
    'wip',
  ]) {
    assert.equal(isDraftReview(titled(title)), false, title)
  }
})

test('drafts are hidden by default, and only while the setting is on', () => {
  assert.equal(DEFAULT_SETTINGS.hideDrafts, true)

  const drafted = review('1', { draft: true })
  assert.equal(isVisibleReview(drafted, DEFAULT_SETTINGS), false)
  assert.equal(isVisibleReview(drafted, { ...DEFAULT_SETTINGS, hideDrafts: false }), true)
})

test('a vault written before the drafts preference existed picks up the default', () => {
  const older = { theme: 'dark', hideApproved: true } as Partial<Settings>

  const merged = mergeSettings(older)

  assert.equal(merged.hideDrafts, true)
  assert.equal(merged.hideApproved, true)
  assert.equal(merged.theme, 'dark')
})

test('the two standing preferences hide independently and together', () => {
  const items = [
    review('1'),
    review('2', { myReviewState: 'approved' }),
    review('3', { draft: true }),
    review('4', { title: 'WIP: still writing it', myReviewState: 'approved' }),
  ]
  const ids = (settings: Settings): string[] =>
    visibleReviews(items, settings).map((item) => item.id)

  assert.deepEqual(ids({ ...DEFAULT_SETTINGS, hideApproved: false, hideDrafts: false }), [
    '1',
    '2',
    '3',
    '4',
  ])
  assert.deepEqual(ids({ ...DEFAULT_SETTINGS, hideApproved: false, hideDrafts: true }), ['1', '2'])
  assert.deepEqual(ids({ ...DEFAULT_SETTINGS, hideApproved: true, hideDrafts: false }), ['1', '3'])
  assert.deepEqual(ids({ ...DEFAULT_SETTINGS, hideApproved: true, hideDrafts: true }), ['1'])
})
