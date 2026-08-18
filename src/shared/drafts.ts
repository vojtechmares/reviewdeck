/**
 * Whether a pull request has moved on from the diff a review was written against.
 *
 * On an active pull request the author pushing mid-review is ordinary rather than
 * exceptional, and each draft already records the references it was written
 * against, so this is a comparison rather than a guess.
 *
 * What is deliberately absent is any attempt to re-anchor. Matching a draft's
 * content onto the new head puts a reviewer's words on the wrong line the moment
 * the match is ambiguous, and refusing to submit punishes the reviewer for the
 * author's timing. Submitting against the recorded references instead lands each
 * remark on the code that was actually read, and lets the host mark it outdated
 * itself.
 */

import type { DiffRefs, DraftComment } from './types.ts'

/** The heads these drafts were written against, in the order first seen. */
export function draftedHeads(drafts: DraftComment[]): string[] {
  const heads: string[] = []
  for (const draft of drafts) {
    const head = draft.refs.headSha
    if (head && !heads.includes(head)) heads.push(head)
  }
  return heads
}

/**
 * True when at least one draft was written against a head the pull request no
 * longer has.
 *
 * A draft that recorded no head, or a pull request whose current head is unknown,
 * says nothing either way: without both sides there is nothing to compare, and
 * warning on a guess would train the reviewer to dismiss the warning.
 */
export function headMoved(drafts: DraftComment[], current: DiffRefs | undefined): boolean {
  const head = current?.headSha
  if (!head) return false
  return drafts.some((draft) => Boolean(draft.refs.headSha) && draft.refs.headSha !== head)
}
