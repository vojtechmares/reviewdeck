import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DraftStore, hasDiverged, type DraftState, type NewDraft } from '../src/main/drafts.ts'
import { draftedHeads, headMoved } from '../src/shared/drafts.ts'
import type { DraftComment } from '../src/shared/types.ts'

const REFS = { baseSha: 'base1', startSha: 'start1', headSha: 'head1' }

function store(
  initial: DraftComment[] = [],
  debounceMs = 5,
): { store: DraftStore; saved: DraftState[] } {
  const saved: DraftState[] = []
  let tick = 0
  const store = new DraftStore({
    load: () => ({ comments: initial, sets: {} }),
    save: (state) => saved.push(state),
    debounceMs,
    now: () => `2026-08-05T10:00:${String(tick++).padStart(2, '0')}Z`,
    id: () => `d${tick}`,
  })
  return { store, saved }
}

function draft(itemId: string, body: string, overrides: Partial<NewDraft> = {}): NewDraft {
  return { itemId, body, path: 'src/a.ts', newLine: 12, refs: REFS, ...overrides }
}

test('drafts are keyed per item and never leak between pull requests', () => {
  const { store: drafts } = store()

  drafts.add(draft('acct:repo:1', 'on the first'))
  drafts.add(draft('acct:repo:2', 'on the second'))
  drafts.add(draft('acct:repo:1', 'also on the first'))

  assert.deepEqual(
    drafts.list('acct:repo:1').map((entry) => entry.body),
    ['on the first', 'also on the first'],
  )
  assert.deepEqual(
    drafts.list('acct:repo:2').map((entry) => entry.body),
    ['on the second'],
  )
  assert.deepEqual(drafts.list('acct:repo:3'), [])
  assert.equal(drafts.count('acct:repo:1'), 2)
})

test('a draft records the diff references it was written against', () => {
  const { store: drafts } = store()

  const created = drafts.add(draft('item', 'a remark'))

  assert.deepEqual(created.refs, REFS)
  assert.equal(created.path, 'src/a.ts')
  assert.equal(created.newLine, 12)
  assert.equal(created.oldLine, undefined)
  assert.ok(created.id)
  assert.ok(created.createdAt)

  // A later push must not rewrite what an existing draft was anchored to.
  drafts.add(draft('item', 'later', { refs: { headSha: 'head2' } }))
  assert.deepEqual(drafts.list('item')[0].refs, REFS)
})

test('a draft can be edited and deleted before it is submitted', () => {
  const { store: drafts } = store()
  const created = drafts.add(draft('item', 'first thought'))

  const updated = drafts.update(created.id, 'sharper second thought')
  assert.equal(updated?.body, 'sharper second thought')
  assert.equal(drafts.list('item')[0].body, 'sharper second thought')
  // Editing changes the body and nothing else about where it belongs.
  assert.deepEqual(drafts.list('item')[0].refs, REFS)

  assert.equal(drafts.remove(created.id)?.id, created.id)
  assert.deepEqual(drafts.list('item'), [])

  assert.equal(drafts.update('gone', 'x'), undefined)
  assert.equal(drafts.remove('gone'), undefined)
})

test('clearing takes one item drafts and leaves every other item alone', () => {
  const { store: drafts } = store()
  drafts.add(draft('item-a', 'one'))
  drafts.add(draft('item-a', 'two'))
  drafts.add(draft('item-b', 'elsewhere'))

  drafts.clear('item-a')

  assert.deepEqual(drafts.list('item-a'), [])
  assert.equal(drafts.count('item-b'), 1)
})

test('a failed submission leaves the draft set exactly as it was', () => {
  // Nothing is taken away in advance, so there is nothing to put back: the store is
  // only cleared once a submission has come back clean.
  const { store: drafts } = store()
  drafts.add(draft('item', 'one'))
  drafts.add(draft('item', 'two'))

  const before = drafts.list('item')
  const submit = async (): Promise<void> => {
    throw new Error('the host said no')
  }

  return submit()
    .then(
      () => assert.fail('the submission should have failed'),
      () => {
        assert.deepEqual(drafts.list('item'), before)
        assert.equal(drafts.count('item'), 2)
      },
    )
})

test('writing does not touch the store per keystroke', () => {
  const { store: drafts, saved } = store([], 50)
  const created = drafts.add(draft('item', 'a'))

  for (const body of ['ab', 'abc', 'abcd', 'abcde']) drafts.update(created.id, body)

  // Five changes so far, and the file has not been rewritten once.
  assert.equal(saved.length, 0)

  drafts.flush()
  assert.equal(saved.length, 1, 'one write for the lot')
  assert.equal(saved[0].comments[0].body, 'abcde')
})

test('the debounced write does land on its own', async () => {
  const { store: drafts, saved } = store([], 1)
  drafts.add(draft('item', 'a'))

  await new Promise((resolve) => setTimeout(resolve, 30))

  assert.equal(saved.length, 1)
  assert.equal(saved[0].comments[0].body, 'a')
})

test('drafts written before a restart are there afterwards', () => {
  const { store: first, saved } = store()
  first.add(draft('item', 'survives'))
  first.flush()

  const reopened = new DraftStore({
    load: () => saved.at(-1) ?? { comments: [], sets: {} },
    save: () => {},
  })

  assert.deepEqual(
    reopened.list('item').map((entry) => entry.body),
    ['survives'],
  )
})

function written(head: string | undefined, id = head ?? 'none'): DraftComment {
  return {
    id,
    itemId: 'item',
    body: 'a remark',
    path: 'src/a.ts',
    newLine: 12,
    createdAt: '2026-08-05T10:00:00Z',
    refs: head ? { baseSha: 'base1', startSha: 'start1', headSha: head } : {},
  }
}

test('headMoved says nothing when the pull request has not moved', () => {
  const drafts = [written('head1', 'a'), written('head1', 'b')]

  assert.equal(headMoved(drafts, { headSha: 'head1' }), false)
  assert.equal(headMoved(drafts, { baseSha: 'base9', startSha: 'start9', headSha: 'head1' }), false)
  // Nothing drafted, nothing to warn about.
  assert.equal(headMoved([], { headSha: 'head2' }), false)
})

test('headMoved spots a head the pull request no longer has', () => {
  assert.equal(headMoved([written('head1')], { headSha: 'head2' }), true)

  // One stale draft among fresh ones is still a push the reviewer should know about.
  const mixed = [written('head2', 'a'), written('head1', 'b'), written('head2', 'c')]
  assert.equal(headMoved(mixed, { headSha: 'head2' }), true)
})

test('headMoved refuses to warn on a guess', () => {
  // Without both sides there is nothing to compare, and a warning nobody can act on
  // is one the reviewer learns to dismiss.
  assert.equal(headMoved([written(undefined)], { headSha: 'head2' }), false)
  assert.equal(headMoved([written('head1')], undefined), false)
  assert.equal(headMoved([written('head1')], {}), false)
  assert.equal(headMoved([written('head1')], { baseSha: 'base1' }), false)
})

test('draftedHeads names each head once, in the order it was first written against', () => {
  assert.deepEqual(
    draftedHeads([written('head1', 'a'), written('head2', 'b'), written('head1', 'c')]),
    ['head1', 'head2'],
  )
  assert.deepEqual(draftedHeads([written(undefined)]), [])
  assert.deepEqual(draftedHeads([]), [])
})

const APPROVED = 'approved' as const
const PENDING = 'pending' as const

test('hasDiverged reports a review the reviewer submitted somewhere else', () => {
  assert.equal(
    hasDiverged({
      baseline: PENDING,
      current: APPROVED,
      submissionsAtSyncStart: 0,
      submissionsNow: 0,
    }),
    true,
  )
})

test('hasDiverged says nothing when the state has not moved', () => {
  assert.equal(
    hasDiverged({ baseline: PENDING, current: PENDING, submissionsAtSyncStart: 3, submissionsNow: 3 }),
    false,
  )
})

test('hasDiverged says nothing without a baseline to compare against', () => {
  assert.equal(
    hasDiverged({
      baseline: undefined,
      current: APPROVED,
      submissionsAtSyncStart: 0,
      submissionsNow: 0,
    }),
    false,
  )
})

test('hasDiverged never reports this app own submission as somebody else', () => {
  // A sync that began before the submission is holding the state from before it.
  // Reading that as a divergence would accuse the reviewer of their own review.
  assert.equal(
    hasDiverged({
      baseline: APPROVED,
      current: PENDING,
      submissionsAtSyncStart: 0,
      submissionsNow: 1,
    }),
    false,
  )
})

test('a draft set takes its baseline from the state when it began', () => {
  const { store: drafts } = store()

  drafts.add(draft('item', 'first'), PENDING)
  // A later draft does not move the baseline the set started from.
  drafts.add(draft('item', 'second'), APPROVED)

  assert.equal(drafts.reconcile('item', PENDING, 0), false)
  assert.equal(drafts.reconcile('item', APPROVED, 0), true)
  assert.equal(drafts.diverged('item'), true)
})

test('a set created after an external change starts from what is there now', () => {
  // The reviewer approved in a browser, then came back and started drafting. The
  // change is already reflected, so there is nothing to reconcile.
  const { store: drafts } = store()
  drafts.add(draft('item', 'written after the fact'), APPROVED)

  assert.equal(drafts.reconcile('item', APPROVED, 0), false)
  assert.equal(drafts.diverged('item'), false)
})

test('an item with no drafts never reports divergence', () => {
  const { store: drafts } = store()

  assert.equal(drafts.reconcile('untouched', APPROVED, 0), false)
  assert.equal(drafts.diverged('untouched'), false)

  // Nor once its drafts are gone.
  drafts.add(draft('item', 'one'), PENDING)
  drafts.reconcile('item', APPROVED, 0)
  assert.equal(drafts.diverged('item'), true)
  drafts.clear('item')
  assert.equal(drafts.diverged('item'), false)
})

test('the app own submission rebaselines instead of diverging', () => {
  const { store: drafts } = store()
  drafts.add(draft('item', 'one'), PENDING)

  const before = drafts.submissions()
  drafts.recordSubmission('item', APPROVED)
  assert.equal(drafts.submissions(), before + 1)

  // A sync that started before it is ignored, and the state it settled on is the
  // new baseline, so the next sync agrees.
  assert.equal(drafts.reconcile('item', PENDING, before), false)
  assert.equal(drafts.reconcile('item', APPROVED, drafts.submissions()), false)
  assert.equal(drafts.diverged('item'), false)
})

test('keeping drafts clears the mark and touches not one character of them', () => {
  const { store: drafts } = store()
  drafts.add(draft('item', 'a carefully worded remark'), PENDING)
  drafts.reconcile('item', APPROVED, 0)
  assert.equal(drafts.diverged('item'), true)

  const before = drafts.list('item')
  drafts.acknowledge('item', APPROVED)

  assert.equal(drafts.diverged('item'), false)
  assert.deepEqual(drafts.list('item'), before)

  // And the same change is not reported a second time.
  assert.equal(drafts.reconcile('item', APPROVED, 0), false)
})

test('the diverged mark is written down, so it is still there after a restart', () => {
  const { store: first, saved } = store()
  first.add(draft('item', 'one'), PENDING)
  first.reconcile('item', APPROVED, 0)
  first.flush()

  const reopened = new DraftStore({
    load: () => saved.at(-1) ?? { comments: [], sets: {} },
    save: () => {},
  })

  assert.equal(reopened.diverged('item'), true)
  assert.equal(reopened.count('item'), 1)
})

test('divergence is per item and does not spread', () => {
  const { store: drafts } = store()
  drafts.add(draft('item-a', 'one'), PENDING)
  drafts.add(draft('item-b', 'two'), PENDING)

  drafts.reconcile('item-a', APPROVED, 0)

  assert.equal(drafts.diverged('item-a'), true)
  assert.equal(drafts.diverged('item-b'), false)
})
