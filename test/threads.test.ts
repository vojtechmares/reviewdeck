import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  bitbucketThreads,
  forgejoThreadId,
  forgejoThreads,
  githubFlatThreads,
  githubThreads,
  gitlabThreads,
  parseForgejoThreadId,
} from '../src/main/providers/threads.ts'

test('githubFlatThreads gives every comment its own thread with no affordances', () => {
  const threads = githubFlatThreads(
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

test('githubFlatThreads falls back to the original line and reads the left side', () => {
  const [thread] = githubFlatThreads(
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

test('forgejoThreads keeps an ordinary comment a thread of one with no affordances', () => {
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
  // Forgejo has no notion of replying to a pull request comment.
  assert.equal(threads[0].canReply, false)
  assert.equal(threads[0].canResolve, false)
  assert.equal(threads[0].path, undefined)
})

test('forgejoThreadId survives a round trip, including a path with a colon in it', () => {
  for (const [path, side, line] of [
    ['src/app.ts', 'new', 42],
    ['src/a:b.ts', 'old', 7],
    ['deep/nested/path/with:colons/x.go', 'new', 1],
  ] as [string, 'old' | 'new', number][]) {
    assert.deepEqual(parseForgejoThreadId(forgejoThreadId(path, side, line)), { path, side, line })
  }

  assert.equal(parseForgejoThreadId('not-a-thread-id'), null)
  assert.equal(parseForgejoThreadId('fj:sideways:1:a.ts'), null)
  assert.equal(parseForgejoThreadId('fj:new:notanumber:a.ts'), null)
  assert.equal(parseForgejoThreadId('fj:new:1:'), null)
})

test('forgejoThreads groups review comments by the line they were left on', () => {
  const threads = forgejoThreads(
    [],
    [
      {
        id: 20,
        user: { login: 'mnovotna' },
        body: 'Can this go into config?',
        created_at: '2026-08-02T09:00:00Z',
        path: 'internal/capture.go',
        position: 55,
        original_position: 0,
      },
      {
        id: 21,
        user: { login: 'hkramer' },
        body: 'Follow-up.',
        created_at: '2026-08-02T10:00:00Z',
        path: 'internal/capture.go',
        position: 55,
        original_position: 0,
      },
      {
        id: 22,
        user: { login: 'mnovotna' },
        body: 'Different line entirely.',
        created_at: '2026-08-02T11:00:00Z',
        path: 'internal/capture.go',
        position: 80,
        original_position: 0,
      },
    ],
  )

  assert.equal(threads.length, 2, 'two lines, two conversations')

  const [first, second] = threads
  assert.deepEqual(
    first.comments.map((comment) => comment.body),
    ['Can this go into config?', 'Follow-up.'],
  )
  assert.equal(first.line, 55)
  assert.equal(first.side, 'new')
  assert.equal(first.path, 'internal/capture.go')
  assert.equal(second.line, 80)

  // Two comments left in different reviews still share one conversation, so the
  // identifier cannot come from a review id.
  assert.equal(first.id, forgejoThreadId('internal/capture.go', 'new', 55))
})

test('forgejoThreads reads the old side from the original position', () => {
  const [thread] = forgejoThreads(
    [],
    [
      {
        id: 30,
        body: 'Why was this dropped?',
        created_at: '2026-08-02T09:00:00Z',
        path: 'a.ts',
        position: 0,
        original_position: 12,
      },
    ],
  )

  assert.equal(thread.side, 'old')
  assert.equal(thread.line, 12)
  assert.equal(thread.comments[0].author.name, 'unknown')
})

test('forgejoThreads offers a reply on an inline thread and never a resolve', () => {
  const [thread] = forgejoThreads(
    [],
    [{ id: 40, body: 'A remark.', created_at: '2026-08-02T09:00:00Z', path: 'a.ts', position: 3 }],
  )

  // A reply is another comment at the same anchor, which the API does support.
  assert.equal(thread.canReply, true)
  // Resolution has no REST endpoint at all, so the control must never appear.
  assert.equal(thread.canResolve, false)
})

test('forgejoThreads counts a conversation resolved only once every comment in it is', () => {
  const at = (id: number, resolver?: { login: string }): {
    id: number
    body: string
    created_at: string
    path: string
    position: number
    resolver?: { login: string }
  } => ({ id, body: `c${id}`, created_at: '2026-08-02T09:00:00Z', path: 'a.ts', position: 5, resolver })

  const [partly] = forgejoThreads([], [at(1, { login: 'vmares' }), at(2)])
  assert.equal(partly.resolved, false)

  const [fully] = forgejoThreads([], [at(3, { login: 'vmares' }), at(4, { login: 'vmares' })])
  assert.equal(fully.resolved, true)

  const [none] = forgejoThreads([], [at(5)])
  assert.equal(none.resolved, false)
})

test('forgejoThreads drops a review comment it cannot anchor', () => {
  const threads = forgejoThreads(
    [],
    [
      { id: 50, body: 'no path', created_at: '2026-08-02T09:00:00Z', position: 3 },
      { id: 51, body: 'no line', created_at: '2026-08-02T09:00:00Z', path: 'a.ts', position: 0 },
    ],
  )

  assert.deepEqual(threads, [])
})

test('forgejoThreads orders ordinary comments and conversations together by age', () => {
  const threads = forgejoThreads(
    [{ id: 60, body: 'second', created_at: '2026-08-02T10:00:00Z' }],
    [{ id: 61, body: 'first', created_at: '2026-08-02T09:00:00Z', path: 'a.ts', position: 1 }],
  )

  assert.deepEqual(
    threads.map((thread) => thread.comments[0].body),
    ['first', 'second'],
  )
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

const NOW = '2026-08-05T08:00:00Z'

function gqlComment(id: string, body: string, at = NOW): {
  id: string
  body: string
  createdAt: string
  author: { login: string; avatarUrl: string }
} {
  return { id, body, createdAt: at, author: { login: 'mnovotna', avatarUrl: 'https://gh.test/m.png' } }
}

test('githubThreads keeps a review thread whole and reads both capabilities off it', () => {
  const [thread] = githubThreads(
    [
      {
        id: 'RT_1',
        isResolved: false,
        isOutdated: false,
        path: 'internal/capture.go',
        line: 55,
        diffSide: 'RIGHT',
        viewerCanReply: true,
        viewerCanResolve: true,
        viewerCanUnresolve: false,
        comments: {
          nodes: [gqlComment('C_1', 'Can this go into config?'), gqlComment('C_2', 'Follow-up.')],
        },
      },
    ],
    [],
  )

  assert.equal(thread.id, 'RT_1')
  assert.deepEqual(
    thread.comments.map((comment) => comment.id),
    ['C_1', 'C_2'],
  )
  assert.equal(thread.canReply, true)
  assert.equal(thread.canResolve, true)
  assert.equal(thread.resolved, false)
  assert.equal(thread.outdated, false)
  assert.equal(thread.path, 'internal/capture.go')
  assert.equal(thread.line, 55)
  assert.equal(thread.side, 'new')
  assert.equal(thread.comments[0].author.avatarUrl, 'https://gh.test/m.png')
})

test('githubThreads reads resolvability from whichever way the thread would go', () => {
  const base = {
    id: 'RT_2',
    path: 'a.ts',
    line: 3,
    comments: { nodes: [gqlComment('C_3', 'Dealt with?')] },
  }

  // An unresolved thread is resolvable when the viewer may resolve it...
  const [open] = githubThreads(
    [{ ...base, isResolved: false, viewerCanResolve: true, viewerCanUnresolve: false }],
    [],
  )
  assert.equal(open.canResolve, true)

  // ...and a resolved one when the viewer may reopen it, which is the other flag.
  const [closed] = githubThreads(
    [{ ...base, isResolved: true, viewerCanResolve: false, viewerCanUnresolve: true }],
    [],
  )
  assert.equal(closed.resolved, true)
  assert.equal(closed.canResolve, true)

  const [locked] = githubThreads(
    [{ ...base, isResolved: true, viewerCanResolve: true, viewerCanUnresolve: false }],
    [],
  )
  assert.equal(locked.canResolve, false, 'a resolved thread is not reopenable just because it was resolvable')
})

test('githubThreads never anchors an outdated thread to a line', () => {
  // The defect this fixes: GitHub reports the line the thread was originally left
  // on, and in the diff as it stands that number is a different line entirely.
  const [thread] = githubThreads(
    [
      {
        id: 'RT_3',
        isOutdated: true,
        path: 'internal/capture.go',
        line: null,
        diffSide: 'RIGHT',
        comments: { nodes: [gqlComment('C_4', 'This moved.')] },
      },
    ],
    [],
  )

  assert.equal(thread.outdated, true)
  assert.equal(thread.line, undefined)
  // The file is kept, so the thread can still be shown against the file it belongs to.
  assert.equal(thread.path, 'internal/capture.go')
})

test('githubThreads keeps an issue comment a thread of one with no affordances', () => {
  const threads = githubThreads([], [gqlComment('IC_1', 'Looks good overall.')])

  assert.equal(threads.length, 1)
  assert.equal(threads[0].comments.length, 1)
  assert.equal(threads[0].canReply, false, 'an issue comment is not a review thread')
  assert.equal(threads[0].canResolve, false)
  assert.equal(threads[0].path, undefined)
})

test('githubThreads orders review threads and issue comments together by age', () => {
  const threads = githubThreads(
    [
      {
        id: 'RT_4',
        path: 'a.ts',
        line: 1,
        comments: { nodes: [gqlComment('C_5', 'second', '2026-08-05T09:00:00Z')] },
      },
    ],
    [gqlComment('IC_2', 'first', '2026-08-05T08:00:00Z')],
  )

  assert.deepEqual(
    threads.map((thread) => thread.comments[0].body),
    ['first', 'second'],
  )
})

test('githubThreads drops a thread whose comments have all gone', () => {
  const threads = githubThreads(
    [
      { id: 'RT_5', path: 'a.ts', line: 1, comments: { nodes: [] } },
      { id: 'RT_6', path: 'a.ts', line: 2, comments: null },
      { id: 'RT_7', path: 'a.ts', line: 3, comments: { nodes: [null, gqlComment('C_6', 'here')] } },
    ],
    [],
  )

  assert.deepEqual(
    threads.map((thread) => thread.id),
    ['RT_7'],
  )
  assert.equal(threads[0].comments.length, 1)
})
