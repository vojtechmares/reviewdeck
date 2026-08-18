/**
 * Turning a set of drafts into whatever each host wants, and being honest when a
 * host that has no batch call stops part-way.
 *
 * Two of the four take a review and its comments in one request. The other two
 * take them one at a time, which means a submission can half succeed - and the
 * reviewer has to be told which half, because the difference decides whether
 * retrying repeats themselves or finishes the job.
 *
 * Nothing here does any I/O and every import is a type, so both the payload shapes
 * and the sequential semantics stay reachable from the test suite.
 */

import type { DraftComment, ReviewVerdict } from '@shared/types.ts'

/** `src/a.ts:12`, or just the path for a comment whose line is gone. */
function where(comment: DraftComment): string {
  const line = comment.newLine ?? comment.oldLine
  return line === undefined ? comment.path : `${comment.path}:${line}`
}

function list(comments: DraftComment[], limit = 4): string {
  const named = comments.slice(0, limit).map(where).join(', ')
  const rest = comments.length - limit
  return rest > 0 ? `${named} and ${rest} more` : named
}

function reason(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause)
}

/**
 * A submission that got some of the way. Carries both halves, so the caller can
 * drop the drafts that landed and keep the ones that did not - which is what makes
 * retrying finish the review rather than say everything twice.
 */
export class PartialSubmitError extends Error {
  readonly posted: DraftComment[]
  readonly unposted: DraftComment[]

  constructor(posted: DraftComment[], unposted: DraftComment[], cause: unknown) {
    const total = posted.length + unposted.length
    const tail = unposted.length
      ? `Still drafted: ${list(unposted)}.`
      : 'Every comment landed; only the verdict did not.'

    super(
      `Posted ${posted.length} of ${total} comment${total === 1 ? '' : 's'}, then stopped: ` +
        `${reason(cause)}. Posted: ${list(posted)}. ${tail}`,
    )
    this.name = 'PartialSubmitError'
    this.posted = posted
    this.unposted = unposted
  }
}

/**
 * Posts each comment in order and then applies the verdict, for a host with no call
 * that takes both.
 *
 * The verdict goes last so the author's one notification about it arrives with the
 * comments already in place. If anything fails once a comment has landed, the
 * failure names what got through - including the case where every comment posted
 * and only the verdict did not, which still leaves the reviewer needing to know
 * their remarks are already out there.
 */
export async function submitSequentially(
  comments: DraftComment[],
  post: (comment: DraftComment) => Promise<void>,
  verdict: () => Promise<void>,
): Promise<void> {
  const posted: DraftComment[] = []

  for (const [index, comment] of comments.entries()) {
    try {
      await post(comment)
      posted.push(comment)
    } catch (error) {
      // Nothing landed, so nothing needs explaining - the host's own error is the
      // clearest thing the reviewer can be told.
      if (!posted.length) throw error
      throw new PartialSubmitError(posted, comments.slice(index), error)
    }
  }

  try {
    await verdict()
  } catch (error) {
    if (!posted.length) throw error
    throw new PartialSubmitError(posted, [], error)
  }
}

/** GitHub takes its comments on review creation, addressed by side and line. */
export function githubReviewComments(
  comments: DraftComment[],
): { path: string; body: string; side: string; line: number | undefined }[] {
  return comments.map((comment) => ({
    path: comment.path,
    body: comment.body,
    side: comment.newLine ? 'RIGHT' : 'LEFT',
    line: comment.newLine ?? comment.oldLine,
  }))
}

/**
 * Forgejo takes them the same way, addressed by position instead: zero means the
 * comment is not on that side.
 */
export function forgejoReviewPayload(
  verdict: ReviewVerdict,
  body: string,
  comments: DraftComment[],
): Record<string, unknown> {
  const event =
    verdict === 'approve' ? 'APPROVED' : verdict === 'request_changes' ? 'REQUEST_CHANGES' : 'COMMENT'

  return {
    event,
    // A review with neither a body nor comments is rejected, so always say something.
    body: body || (comments.length ? '' : verdict === 'approve' ? 'Approved.' : 'Reviewed.'),
    commit_id: comments[0]?.refs.headSha,
    comments: comments.map((comment) => ({
      path: comment.path,
      body: comment.body,
      new_position: comment.newLine ?? 0,
      old_position: comment.oldLine ?? 0,
    })),
  }
}

/**
 * GitLab addresses a line by the three shas the diff was read at, which is why a
 * draft records them: without them the comment cannot be placed at all.
 */
export function gitlabDiscussionPayload(comment: DraftComment): Record<string, unknown> {
  const { baseSha, startSha, headSha } = comment.refs
  if (!baseSha || !startSha || !headSha) {
    throw new Error('GitLab needs the merge request diff refs; reload the merge request and try again.')
  }

  return {
    body: comment.body,
    position: {
      position_type: 'text',
      base_sha: baseSha,
      start_sha: startSha,
      head_sha: headSha,
      new_path: comment.path,
      old_path: comment.path,
      // Added lines carry only new_line, removed lines only old_line, context both.
      new_line: comment.newLine,
      old_line: comment.oldLine,
    },
  }
}

/** Bitbucket addresses the new file with `to` and the old one with `from`. */
export function bitbucketCommentPayload(comment: DraftComment): Record<string, unknown> {
  return {
    content: { raw: comment.body },
    inline: comment.newLine
      ? { path: comment.path, to: comment.newLine }
      : { path: comment.path, from: comment.oldLine },
  }
}
