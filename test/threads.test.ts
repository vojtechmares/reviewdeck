import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  bitbucketThreads,
  forgejoThreads,
  githubThreads,
  gitlabThreads,
} from '../src/main/providers/threads.ts'

test('githubThreads gives every comment its own thread with no affordances', () => {
  const threads = githubThreads(
    [
      {
        id: 1,
        user: { login: 'mnovotna', avatar_url: 'https://avatars.test/m.png' },
        body: 'Looks good overall.',
        created_at: '2026-08-01T10:00:00Z',
      },
    ],
    [
      {
        id: 2,
        user: { login: 'hkramer', avatar_url: '' },
        body: 'This branch reads redundant.',
        created_at: '2026-08-01T09:00:00Z',
        path: 'internal/capture.go',
        line: 55,
        side: 'RIGHT',
      },
    ],
  )

  // Issue comments and review comments interleave by age, not by which call they came from.
  assert.deepEqual(
    threads.map((thread) => thread.comments[0].body),
    ['This branch reads redundant.', 'Looks good overall.'],
  )
  for (const thread of threads) {
    assert.equal(thread.comments.length, 1)
    assert.equal(thread.canReply, false, 'REST cannot reply to a GitHub thread')
    assert.equal(thread.canResolve, false, 'resolution is a GraphQL mutation only')
    assert.equal(thread.resolved, false)
  }

  const inline = threads[0]
  assert.equal(inline.path, 'internal/capture.go')
  assert.equal(inline.line, 55)
  assert.equal(inline.side, 'new')
  assert.equal(inline.comments[0].author.name, 'hkramer')

  assert.equal(threads[1].path, undefined)
  assert.equal(threads[1].line, undefined)
})

test('githubThreads falls back to the original line and reads the left side', () => {
  const [thread] = githubThreads(
    [],
    [
      {
        id: 3,
        body: 'Was this needed?',
        created_at: '2026-08-01T09:00:00Z',
        path: 'a.ts',
        line: null,
        original_line: 12,
        side: 'LEFT',
      },
    ],
  )

  assert.equal(thread.line, 12)
  assert.equal(thread.side, 'old')
  assert.equal(thread.comments[0].author.name, 'unknown')
})

test('forgejoThreads gives every comment its own thread with no affordances', () => {
  const threads = forgejoThreads([
    {
      id: 7,
      user: { login: 'vmares', avatar_url: 'https://codeberg.test/v.png' },
      body: 'Ready from my side.',
      created_at: '2026-08-02T08:00:00Z',
    },
  ])

  assert.equal(threads.length, 1)
  assert.deepEqual(threads[0].comments, [
    {
      id: '7',
      author: { name: 'vmares', avatarUrl: 'https://codeberg.test/v.png' },
      body: 'Ready from my side.',
      createdAt: '2026-08-02T08:00:00Z',
    },
  ])
  assert.equal(threads[0].canReply, false)
  assert.equal(threads[0].canResolve, false)
  // Forgejo comments come off the issue endpoint, so none of them are anchored.
  assert.equal(threads[0].path, undefined)
})

test('bitbucketThreads keeps the inline anchor and drops deleted comments', () => {
  const threads = bitbucketThreads([
    {
      id: 11,
      user: { display_name: 'L Peters', links: { avatar: { href: 'https://bb.test/l.png' } } },
      content: { raw: 'Nit: naming.' },
      created_on: '2026-08-03T08:00:00Z',
      inline: { path: 'src/app.ts', to: 42 },
    },
    {
      id: 12,
      content: { raw: 'gone' },
      created_on: '2026-08-03T09:00:00Z',
      deleted: true,
    },
    {
      id: 13,
      content: { raw: 'On the old side.' },
      created_on: '2026-08-03T10:00:00Z',
      inline: { path: 'src/app.ts', from: 40 },
    },
  ])

  assert.deepEqual(
    threads.map((thread) => thread.id),
    ['11', '13'],
  )
  assert.equal(threads[0].line, 42)
  assert.equal(threads[0].side, 'new')
  assert.equal(threads[0].comments[0].author.avatarUrl, 'https://bb.test/l.png')
  assert.equal(threads[1].line, 40)
  assert.equal(threads[1].side, 'old')
  assert.equal(threads[1].comments[0].author.name, 'unknown')
  for (const thread of threads) {
    assert.equal(thread.canReply, false)
    assert.equal(thread.canResolve, false)
  }
})

test('gitlabThreads keeps a discussion together and reads both capabilities off it', () => {
  const [thread] = gitlabThreads([
    {
      id: 'abc123',
      individual_note: false,
      notes: [
        {
          id: 1,
          body: 'Can this go into config?',
          author: { username: 'mnovotna', avatar_url: 'https://gl.test/m.png' },
          created_at: '2026-08-04T08:00:00Z',
          resolvable: true,
          resolved: false,
          position: {
            new_path: 'internal/capture.go',
            old_path: 'internal/capture.go',
            new_line: 55,
            head_sha: 'head1',
          },
        },
        {
          id: 2,
          body: 'Follow-up, so this can ship today.',
          author: { username: 'hkramer' },
          created_at: '2026-08-04T09:00:00Z',
          resolvable: true,
          resolved: false,
        },
      ],
    },
  ], 'head1')

  assert.equal(thread.id, 'abc123')
  assert.deepEqual(
    thread.comments.map((comment) => comment.id),
    ['1', '2'],
  )
  assert.equal(thread.canReply, true)
  assert.equal(thread.canResolve, true)
  assert.equal(thread.resolved, false)
  assert.equal(thread.outdated, false)
  assert.equal(thread.path, 'internal/capture.go')
  assert.equal(thread.line, 55)
  assert.equal(thread.side, 'new')
})

test('gitlabThreads counts a discussion resolved only once every resolvable note is', () => {
  const notes = (resolved: boolean[]): { id: number; body: string; created_at: string; resolvable: boolean; resolved: boolean }[] =>
    resolved.map((value, index) => ({
      id: index + 1,
      body: `note ${index}`,
      created_at: '2026-08-04T08:00:00Z',
      resolvable: true,
      resolved: value,
    }))

  const [partly] = gitlabThreads([{ id: 'd1', notes: notes([true, false]) }])
  assert.equal(partly.resolved, false)

  const [fully] = gitlabThreads([{ id: 'd2', notes: notes([true, true]) }])
  assert.equal(fully.resolved, true)
})

test('gitlabThreads refuses replies into a standalone note and resolution where nothing resolves', () => {
  const [thread] = gitlabThreads([
    {
      id: 'd3',
      individual_note: true,
      notes: [
        {
          id: 9,
          body: 'Just a comment.',
          author: { username: 'vmares' },
          created_at: '2026-08-04T08:00:00Z',
          resolvable: false,
        },
      ],
    },
  ])

  assert.equal(thread.canReply, false)
  assert.equal(thread.canResolve, false)
  assert.equal(thread.resolved, false, 'nothing resolvable must not read as resolved')
})

test('gitlabThreads drops system notes and the discussions made only of them', () => {
  const threads = gitlabThreads([
    {
      id: 'd4',
      notes: [
        { id: 1, body: 'changed the description', created_at: '2026-08-04T08:00:00Z', system: true },
      ],
    },
    {
      id: 'd5',
      notes: [
        { id: 2, body: 'assigned to @vmares', created_at: '2026-08-04T08:00:00Z', system: true },
        { id: 3, body: 'A real remark.', created_at: '2026-08-04T09:00:00Z' },
      ],
    },
  ])

  assert.deepEqual(
    threads.map((thread) => thread.id),
    ['d5'],
  )
  assert.deepEqual(
    threads[0].comments.map((comment) => comment.body),
    ['A real remark.'],
  )
})

test('gitlabThreads flags a thread left against a diff that has moved on', () => {
  const discussion = {
    id: 'd6',
    notes: [
      {
        id: 1,
        body: 'On an older version.',
        created_at: '2026-08-04T08:00:00Z',
        position: { new_path: 'a.ts', new_line: 3, head_sha: 'old-head' },
      },
    ],
  }

  assert.equal(gitlabThreads([discussion], 'new-head')[0].outdated, true)
  assert.equal(gitlabThreads([discussion], 'old-head')[0].outdated, false)
  // Without a head to compare against, saying it is outdated would be a guess.
  assert.equal(gitlabThreads([discussion])[0].outdated, false)
})
