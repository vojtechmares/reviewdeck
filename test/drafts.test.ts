import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DraftStore, type NewDraft } from '../src/main/drafts.ts'
import { draftedHeads, headMoved } from '../src/shared/drafts.ts'
import type { DraftComment } from '../src/shared/types.ts'

const REFS = { baseSha: 'base1', startSha: 'start1', headSha: 'head1' }

function store(
  initial: DraftComment[] = [],
  debounceMs = 5,
): { store: DraftStore; saved: DraftComment[][] } {
  const saved: DraftComment[][] = []
  let tick = 0
  const store = new DraftStore({
    load: () => initial,
    save: (drafts) => saved.push(drafts),
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
  assert.equal(saved[0][0].body, 'abcde')
})

test('the debounced write does land on its own', async () => {
  const { store: drafts, saved } = store([], 1)
  drafts.add(draft('item', 'a'))

  await new Promise((resolve) => setTimeout(resolve, 30))

  assert.equal(saved.length, 1)
  assert.equal(saved[0][0].body, 'a')
})

test('drafts written before a restart are there afterwards', () => {
  const { store: first, saved } = store()
  first.add(draft('item', 'survives'))
  first.flush()

  const reopened = new DraftStore({ load: () => saved.at(-1) ?? [], save: () => {} })

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
