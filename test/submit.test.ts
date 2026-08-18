import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  bitbucketCommentPayload,
  forgejoReviewPayload,
  githubReviewComments,
  gitlabDiscussionPayload,
  PartialSubmitError,
  submitSequentially,
} from '../src/main/providers/submit.ts'
import type { DraftComment } from '../src/shared/types.ts'

const REFS = { baseSha: 'base1', startSha: 'start1', headSha: 'head1' }

function draft(overrides: Partial<DraftComment> = {}): DraftComment {
  return {
    id: 'd1',
    itemId: 'acct:repo:1',
    body: 'a remark',
    path: 'src/a.ts',
    newLine: 12,
    createdAt: '2026-08-05T10:00:00Z',
    refs: REFS,
    ...overrides,
  }
}

test('githubReviewComments addresses each line by side', () => {
  assert.deepEqual(
    githubReviewComments([
      draft({ id: 'd1', newLine: 12 }),
      draft({ id: 'd2', newLine: undefined, oldLine: 30, path: 'src/b.ts', body: 'gone why?' }),
    ]),
    [
      { path: 'src/a.ts', body: 'a remark', side: 'RIGHT', line: 12 },
      { path: 'src/b.ts', body: 'gone why?', side: 'LEFT', line: 30 },
    ],
  )
})

test('forgejoReviewPayload carries the comments with the verdict in one request', () => {
  const payload = forgejoReviewPayload('request_changes', 'Please fix.', [
    draft({ id: 'd1', newLine: 12 }),
    draft({ id: 'd2', newLine: undefined, oldLine: 30, path: 'src/b.ts', body: 'gone why?' }),
  ])

  assert.equal(payload.event, 'REQUEST_CHANGES')
  assert.equal(payload.body, 'Please fix.')
  // Against the commit the drafts were written on, not whatever the branch is now.
  assert.equal(payload.commit_id, 'head1')
  assert.deepEqual(payload.comments, [
    { path: 'src/a.ts', body: 'a remark', new_position: 12, old_position: 0 },
    { path: 'src/b.ts', body: 'gone why?', new_position: 0, old_position: 30 },
  ])
})

test('forgejoReviewPayload says something when there is nothing else to say', () => {
  // A review with neither a body nor comments is rejected outright.
  assert.equal(forgejoReviewPayload('approve', '', []).body, 'Approved.')
  assert.equal(forgejoReviewPayload('comment', '', []).body, 'Reviewed.')

  // With comments there is content, so nothing has to be invented.
  assert.equal(forgejoReviewPayload('comment', '', [draft()]).body, '')
})

test('gitlabDiscussionPayload places a comment with the shas the draft recorded', () => {
  const payload = gitlabDiscussionPayload(draft()) as {
    body: string
    position: Record<string, unknown>
  }

  assert.equal(payload.body, 'a remark')
  assert.deepEqual(payload.position, {
    position_type: 'text',
    base_sha: 'base1',
    start_sha: 'start1',
    head_sha: 'head1',
    new_path: 'src/a.ts',
    old_path: 'src/a.ts',
    new_line: 12,
    old_line: undefined,
  })
})

test('gitlabDiscussionPayload refuses a draft it cannot place', () => {
  // Without the refs the comment has nowhere to go, and guessing would put someone
  // else's words on a line they never read.
  assert.throws(() => gitlabDiscussionPayload(draft({ refs: { headSha: 'head1' } })), /diff refs/)
  assert.throws(() => gitlabDiscussionPayload(draft({ refs: {} })), /diff refs/)
})

test('bitbucketCommentPayload addresses the new file with to and the old with from', () => {
  assert.deepEqual(bitbucketCommentPayload(draft()), {
    content: { raw: 'a remark' },
    inline: { path: 'src/a.ts', to: 12 },
  })
  assert.deepEqual(bitbucketCommentPayload(draft({ newLine: undefined, oldLine: 30 })), {
    content: { raw: 'a remark' },
    inline: { path: 'src/a.ts', from: 30 },
  })
})

test('submitSequentially posts every comment in order, then the verdict', async () => {
  const order: string[] = []
  const comments = [draft({ id: 'd1' }), draft({ id: 'd2' }), draft({ id: 'd3' })]

  await submitSequentially(
    comments,
    async (comment) => {
      order.push(comment.id)
    },
    async () => {
      order.push('verdict')
    },
  )

  // The verdict last, so the author's notification about it finds the comments there.
  assert.deepEqual(order, ['d1', 'd2', 'd3', 'verdict'])
})

test('submitSequentially reports which comments landed and which did not', async () => {
  const comments = [
    draft({ id: 'd1', path: 'src/a.ts', newLine: 1 }),
    draft({ id: 'd2', path: 'src/b.ts', newLine: 2 }),
    draft({ id: 'd3', path: 'src/c.ts', newLine: 3 }),
    draft({ id: 'd4', path: 'src/d.ts', newLine: 4 }),
  ]

  const failure = await submitSequentially(
    comments,
    async (comment) => {
      if (comment.id === 'd3') throw new Error('Bitbucket returned 429.')
    },
    async () => assert.fail('the verdict should not be reached'),
  ).then(
    () => null,
    (error: unknown) => error,
  )

  assert.ok(failure instanceof PartialSubmitError)
  assert.deepEqual(
    failure.posted.map((comment) => comment.id),
    ['d1', 'd2'],
  )
  assert.deepEqual(
    failure.unposted.map((comment) => comment.id),
    ['d3', 'd4'],
  )

  // The message has to be enough on its own to know what to do next.
  assert.match(failure.message, /Posted 2 of 4 comments/)
  assert.match(failure.message, /Bitbucket returned 429\./)
  assert.match(failure.message, /Posted: src\/a\.ts:1, src\/b\.ts:2/)
  assert.match(failure.message, /Still drafted: src\/c\.ts:3, src\/d\.ts:4/)
})

test('submitSequentially lets the host speak for itself when nothing landed', async () => {
  // No half state to explain, so the reviewer gets the host's own error.
  const failure = await submitSequentially(
    [draft({ id: 'd1' })],
    async () => {
      throw new Error('Not authorised on bitbucket.org.')
    },
    async () => assert.fail('the verdict should not be reached'),
  ).then(
    () => null,
    (error: unknown) => error,
  )

  assert.ok(failure instanceof Error)
  assert.equal(failure instanceof PartialSubmitError, false)
  assert.equal(failure.message, 'Not authorised on bitbucket.org.')
})

test('submitSequentially still explains itself when only the verdict fails', async () => {
  // Every remark is already out there; the reviewer needs to know that before they
  // decide whether to try again.
  const failure = await submitSequentially(
    [draft({ id: 'd1', path: 'src/a.ts', newLine: 1 })],
    async () => {},
    async () => {
      throw new Error('Approval was refused.')
    },
  ).then(
    () => null,
    (error: unknown) => error,
  )

  assert.ok(failure instanceof PartialSubmitError)
  assert.deepEqual(
    failure.posted.map((comment) => comment.id),
    ['d1'],
  )
  assert.deepEqual(failure.unposted, [])
  assert.match(failure.message, /Every comment landed; only the verdict did not\./)
})

test('submitSequentially with no comments is just the verdict', async () => {
  let applied = false
  await submitSequentially(
    [],
    async () => assert.fail('nothing to post'),
    async () => {
      applied = true
    },
  )
  assert.equal(applied, true)

  // And a verdict that fails on its own stays the host's error, not a partial one.
  const failure = await submitSequentially([], async () => {}, async () => {
    throw new Error('nope')
  }).then(
    () => null,
    (error: unknown) => error,
  )
  assert.equal(failure instanceof PartialSubmitError, false)
})

test('a long partial failure names a few and counts the rest', async () => {
  const comments = Array.from({ length: 9 }, (_, index) =>
    draft({ id: `d${index}`, path: `src/f${index}.ts`, newLine: index }),
  )

  const failure = (await submitSequentially(
    comments,
    async (comment) => {
      if (comment.id === 'd6') throw new Error('stopped')
    },
    async () => {},
  ).then(
    () => null,
    (error: unknown) => error,
  )) as PartialSubmitError

  assert.match(failure.message, /Posted 6 of 9 comments/)
  assert.match(failure.message, /and 2 more/)
})
