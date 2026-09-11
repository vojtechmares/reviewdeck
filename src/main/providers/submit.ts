/**
 * Turning a set of drafts into whatever each host wants, and being honest when a
 * host that has no batch call stops part-way.
 *
 * Two of the four take a review and its comments in one request. The other two
 * take them one at a time, which means a submission can half succeed - and the
 * reviewer has to be told which half, because the difference decides whether
 * retrying repeats themselves or finishes the job.
 *
 * A comment covering several lines is the same draft with a `range`, and every
 * host takes the range differently - by start line, by extra line count, or by
 * line code - so each payload builder here reads the same draft its own way.
 *
 * Nothing here does any I/O and, a hash for GitLab's line codes aside, every
 * import is a type, so both the payload shapes and the sequential semantics stay
 * reachable from the test suite.
 */

import { createHash } from 'node:crypto'
import type { DraftComment, LineCommentDraft, RangeEdge, ReviewVerdict } from '@shared/types.ts'

/** The fields that place a comment, shared by a draft and a comment sent directly. */
export type Placed = Pick<LineCommentDraft, 'path' | 'body' | 'newLine' | 'oldLine' | 'range'>

/** `src/a.ts:12`, `src/a.ts:10-12`, or just the path for a comment whose line is gone. */
function where(comment: DraftComment): string {
  const line = comment.newLine ?? comment.oldLine
  if (line === undefined) return comment.path
  const start = comment.range?.startLine
  return start === undefined ? `${comment.path}:${line}` : `${comment.path}:${start}-${line}`
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

/**
 * How GitHub addresses a comment: by side and line, plus the start of the range
 * when there is one. The same shape serves a review's comment list and a comment
 * posted on its own. A range stays on one side here, so the start side is the
 * side.
 */
export function githubCommentPosition(comment: Placed): Record<string, unknown> {
  const side = comment.newLine ? 'RIGHT' : 'LEFT'
  return {
    path: comment.path,
    body: comment.body,
    side,
    line: comment.newLine ?? comment.oldLine,
    ...(comment.range ? { start_side: side, start_line: comment.range.startLine } : {}),
  }
}

/** GitHub takes its comments on review creation, addressed by side and line. */
export function githubReviewComments(comments: DraftComment[]): Record<string, unknown>[] {
  return comments.map(githubCommentPosition)
}

/**
 * Forgejo addresses a line by position, zero meaning not on that side, and a range
 * the other way round from everyone else: anchored at its first line, with a count
 * of how many more follow it on the same side. The comment is still shown on the
 * last line, so a draft converts rather than changing where it is.
 *
 * A Forgejo or Gitea too old to know the count ignores it, and the comment lands
 * on the first line of the range as a single-line comment - the nearest thing it
 * can do.
 */
export function forgejoInlineComment(comment: Placed): Record<string, unknown> {
  const start = comment.range?.startLine
  const newLine = comment.newLine === undefined ? undefined : (start ?? comment.newLine)
  const oldLine = comment.oldLine === undefined ? undefined : (start ?? comment.oldLine)
  const last = comment.newLine ?? comment.oldLine
  return {
    path: comment.path,
    body: comment.body,
    new_position: newLine ?? 0,
    old_position: oldLine ?? 0,
    ...(start !== undefined && last !== undefined ? { extra_lines_count: last - start } : {}),
  }
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
    comments: comments.map(forgejoInlineComment),
  }
}

/**
 * GitLab's name for a line: the file hashed, then where the line sits in the old
 * file and in the new one as the diff counts it - which is exactly what a
 * `RangeEdge` carries.
 */
export function gitlabLineCode(path: string, edge: RangeEdge): string {
  return `${createHash('sha1').update(path).digest('hex')}_${edge.oldPos}_${edge.newPos}`
}

/** One end of a GitLab line range, typed the way its own diff view sends it. */
function gitlabRangeEdge(path: string, edge: RangeEdge): Record<string, unknown> {
  return {
    line_code: gitlabLineCode(path, edge),
    // An unchanged line has no type, as in the browser: it is neither side's.
    type: edge.kind === 'add' ? 'new' : edge.kind === 'del' ? 'old' : undefined,
    old_line: edge.kind === 'add' ? undefined : edge.oldPos,
    new_line: edge.kind === 'del' ? undefined : edge.newPos,
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
      ...(comment.range
        ? {
            line_range: {
              start: gitlabRangeEdge(comment.path, comment.range.start),
              end: gitlabRangeEdge(comment.path, comment.range.end),
            },
          }
        : {}),
    },
  }
}

/**
 * Bitbucket addresses the new file with `to` and the old one with `from`, and a
 * range by where it starts on the same side.
 */
export function bitbucketCommentPayload(comment: Placed): Record<string, unknown> {
  const start = comment.range?.startLine
  return {
    content: { raw: comment.body },
    inline: comment.newLine
      ? { path: comment.path, to: comment.newLine, ...(start !== undefined ? { start_to: start } : {}) }
      : { path: comment.path, from: comment.oldLine, ...(start !== undefined ? { start_from: start } : {}) },
  }
}
