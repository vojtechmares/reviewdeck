/**
 * Whether a re-read of the conversation says anything the app is not already
 * showing.
 *
 * The sync behind an open pull request comes round every poll, and nearly every one
 * of them hands back the conversation exactly as it stands. Passing those on is not
 * free: the diff rebuilds around its inline threads and the page the reviewer was
 * reading moves under them, which is precisely what a background refresh must never
 * do. So a re-read only reaches the view when something in it changed.
 *
 * Everything the app draws from a thread goes into the comparison - what was said as
 * much as who said it, since a comment can be edited after the fact - and nothing
 * else does.
 */

import type { CommentThread } from './types.ts'

/**
 * Every drawn field of every thread, as one comparable string.
 *
 * The fields go in as arrays rather than joined by hand: a comment body can contain
 * any separator that might be picked, and JSON quotes it rather than letting two
 * fields run together into a false match.
 */
function signature(threads: CommentThread[]): string {
  return JSON.stringify(
    threads.map((thread) => [
      thread.id,
      thread.resolved,
      thread.outdated,
      thread.path ?? '',
      thread.line ?? '',
      thread.side ?? '',
      thread.canReply,
      thread.canResolve,
      thread.comments.map((comment) => [
        comment.id,
        comment.author.name,
        comment.author.avatarUrl,
        comment.createdAt,
        comment.body,
      ]),
    ]),
  )
}

export function sameConversation(a: CommentThread[], b: CommentThread[]): boolean {
  return a.length === b.length && signature(a) === signature(b)
}
