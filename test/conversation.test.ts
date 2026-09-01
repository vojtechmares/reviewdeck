import { test } from 'node:test'
import assert from 'node:assert/strict'
import { sameConversation } from '../src/shared/threads.ts'
import type { CommentThread } from '../src/shared/types.ts'

function thread(patch: Partial<CommentThread> = {}): CommentThread {
  return {
    id: 't1',
    resolved: false,
    outdated: false,
    canReply: true,
    canResolve: true,
    comments: [
      {
        id: 'c1',
        author: { name: 'mnovotna', avatarUrl: 'https://gl.test/m.png' },
        body: 'Can this go into config?',
        createdAt: '2026-08-04T08:00:00Z',
      },
    ],
    ...patch,
  }
}

test('sameConversation holds a re-read that says nothing new', () => {
  assert.equal(sameConversation([thread()], [thread()]), true)
  assert.equal(sameConversation([], []), true)
})

test('sameConversation spots a reply arriving in a thread', () => {
  const answered = thread({
    comments: [
      ...thread().comments,
      {
        id: 'c2',
        author: { name: 'hkramer', avatarUrl: '' },
        body: 'Good call, in a follow-up.',
        createdAt: '2026-08-04T09:00:00Z',
      },
    ],
  })

  assert.equal(sameConversation([thread()], [answered]), false)
})

test('sameConversation spots a whole thread arriving or leaving', () => {
  assert.equal(sameConversation([thread()], [thread(), thread({ id: 't2' })]), false)
  assert.equal(sameConversation([thread(), thread({ id: 't2' })], [thread()]), false)
})

test('sameConversation spots a thread being resolved without a word said', () => {
  assert.equal(sameConversation([thread()], [thread({ resolved: true })]), false)
})

test('sameConversation spots a comment edited in place', () => {
  const edited = thread({
    comments: [{ ...thread().comments[0], body: 'Can this go into config, please?' }],
  })

  assert.equal(sameConversation([thread()], [edited]), false)
})

test('sameConversation spots a thread that moved in the diff', () => {
  assert.equal(sameConversation([thread({ line: 12 })], [thread({ line: 40 })]), false)
  assert.equal(sameConversation([thread({ path: 'a.ts' })], [thread({ path: 'b.ts' })]), false)
})

/**
 * Two threads whose fields run together must not compare equal just because the
 * concatenation matches - the separators are what stop that.
 */
test('sameConversation does not confuse two threads whose text runs together', () => {
  const split = [thread({ id: 'ab' }), thread({ id: 'c' })]
  const joined = [thread({ id: 'a' }), thread({ id: 'bc' })]

  assert.equal(sameConversation(split, joined), false)
})
