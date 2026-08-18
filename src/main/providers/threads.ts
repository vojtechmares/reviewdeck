/**
 * Where each host's idea of a conversation becomes the one thread shape the rest
 * of the app knows about.
 *
 * Every provider difference is absorbed here rather than leaking upwards, the same
 * way check roll-up already works: the renderer reads `canReply` and `canResolve`
 * and never asks which host it is talking to.
 *
 * Nothing in this module does any I/O and every import is a type, so it stays
 * reachable from the test suite.
 */

import type { CommentThread, PullComment } from '@shared/types.ts'

export interface GithubComment {
  id: number
  user?: { login?: string; avatar_url?: string }
  body: string
  created_at: string
  path?: string
  line?: number | null
  original_line?: number | null
  side?: string
}

export interface GitlabNote {
  id: number
  body: string
  author?: { username?: string; avatar_url?: string | null }
  created_at: string
  system?: boolean
  resolvable?: boolean
  resolved?: boolean
  position?: {
    new_path?: string
    old_path?: string
    new_line?: number | null
    old_line?: number | null
    head_sha?: string
  } | null
}

export interface GitlabDiscussion {
  id: string
  /** True for a standalone comment, which GitLab will not accept replies into. */
  individual_note?: boolean
  notes?: GitlabNote[]
}

export interface ForgejoComment {
  id: number
  user?: { login?: string; avatar_url?: string }
  body: string
  created_at: string
}

export interface ForgejoReviewComment {
  id: number
  user?: { login?: string; avatar_url?: string }
  body: string
  created_at: string
  path?: string
  /** Line on the new side, or 0 when the comment sits on the old one. */
  position?: number
  /** Line on the old side, or 0. Exactly one of the two is set. */
  original_position?: number
  /** Who resolved the conversation this comment belongs to, if anyone has. */
  resolver?: { login?: string } | null
}

export interface BitbucketComment {
  id: number
  user?: { display_name?: string; links?: { avatar?: { href?: string } } }
  content?: { raw?: string }
  created_on: string
  deleted?: boolean
  inline?: { path: string; to?: number | null; from?: number | null }
}

/** A host comment plus wherever in the diff it was left, before it becomes a thread. */
interface Anchored {
  comment: PullComment
  path?: string
  line?: number
  side?: 'old' | 'new'
}

/**
 * Hosts that give comments no thread structure of their own: each one becomes a
 * thread by itself with both capabilities off, so the app never offers a reply or
 * a resolve it cannot carry out.
 */
function loneThreads(anchored: Anchored[]): CommentThread[] {
  return anchored.map((entry) => ({
    id: entry.comment.id,
    comments: [entry.comment],
    resolved: false,
    outdated: false,
    path: entry.path,
    line: entry.line,
    side: entry.side,
    canReply: false,
    canResolve: false,
  }))
}

function githubAnchored(comment: GithubComment): Anchored {
  const line = comment.line ?? comment.original_line ?? undefined
  return {
    comment: {
      id: String(comment.id),
      author: { name: comment.user?.login ?? 'unknown', avatarUrl: comment.user?.avatar_url ?? '' },
      body: comment.body,
      createdAt: comment.created_at,
    },
    path: comment.path,
    line: line ?? undefined,
    side: comment.side === 'LEFT' ? 'old' : comment.path ? 'new' : undefined,
  }
}

/**
 * The REST shape, kept as the fallback for an instance whose GraphQL endpoint we
 * cannot reach: every comment stands alone, because review threads simply are not
 * in the REST API. Issue comments and review comments interleave by age.
 */
export function githubFlatThreads(
  issueComments: GithubComment[],
  reviewComments: GithubComment[],
): CommentThread[] {
  const anchored = [...issueComments, ...reviewComments]
    .map(githubAnchored)
    .sort((a, b) => a.comment.createdAt.localeCompare(b.comment.createdAt))
  return loneThreads(anchored)
}

export interface GithubGqlComment {
  id: string
  body: string
  createdAt: string
  author?: { login?: string | null; avatarUrl?: string | null } | null
}

export interface GithubReviewThread {
  id: string
  isResolved?: boolean
  isOutdated?: boolean
  path?: string | null
  /** The line in the current diff; null once the thread has gone outdated. */
  line?: number | null
  diffSide?: string | null
  viewerCanReply?: boolean
  viewerCanResolve?: boolean
  viewerCanUnresolve?: boolean
  comments?: { nodes?: (GithubGqlComment | null)[] | null } | null
}

function gqlComment(comment: GithubGqlComment): PullComment {
  return {
    id: comment.id,
    author: {
      name: comment.author?.login ?? 'unknown',
      avatarUrl: comment.author?.avatarUrl ?? '',
    },
    body: comment.body,
    createdAt: comment.createdAt,
  }
}

/**
 * The GraphQL shape, which is the only place GitHub keeps review threads and the
 * only way their resolution can be read or changed.
 *
 * An outdated thread carries its file but no line. GitHub still reports the line it
 * was originally left on, and using that was the defect: in the diff as it stands
 * now that number is a different line, so the app pointed some comments at code
 * their author never saw. Better to say where the thread belongs and not pretend to
 * know where in it.
 *
 * Whether a thread can be resolved depends on which way it would go, so the flag is
 * read from whichever of the two the viewer would actually be doing.
 */
export function githubThreads(
  reviewThreads: GithubReviewThread[],
  issueComments: GithubGqlComment[],
): CommentThread[] {
  const threads: CommentThread[] = []

  for (const thread of reviewThreads) {
    const comments = (thread.comments?.nodes ?? [])
      .filter((comment): comment is GithubGqlComment => Boolean(comment))
      .map(gqlComment)
    if (!comments.length) continue

    const outdated = thread.isOutdated === true
    const resolved = thread.isResolved === true

    threads.push({
      id: thread.id,
      comments,
      resolved,
      outdated,
      path: thread.path ?? undefined,
      line: outdated ? undefined : (thread.line ?? undefined),
      side: thread.diffSide === 'LEFT' ? 'old' : thread.path ? 'new' : undefined,
      canReply: thread.viewerCanReply === true,
      canResolve: resolved ? thread.viewerCanUnresolve === true : thread.viewerCanResolve === true,
    })
  }

  // An ordinary issue comment is not a review thread and nothing can be replied
  // into it, so it stays a thread of one with neither affordance.
  for (const comment of issueComments) {
    threads.push({
      id: comment.id,
      comments: [gqlComment(comment)],
      resolved: false,
      outdated: false,
      canReply: false,
      canResolve: false,
    })
  }

  return threads.sort((a, b) =>
    (a.comments[0]?.createdAt ?? '').localeCompare(b.comments[0]?.createdAt ?? ''),
  )
}

/**
 * Forgejo has the weakest thread identity of the four: a review comment belongs to
 * a review, and a review is a batch of comments across the whole diff - so there is
 * no thread object to point at. What it does have is the rule its own UI works by:
 * a conversation is every comment left on one side of one line of one file.
 *
 * So the identifier is synthesised from that anchor, which makes it stable across
 * refreshes without depending on any id the host might renumber, and readable back
 * when a reply has to be addressed. All of it stays in the adapter; the renderer
 * reads capability flags like it does everywhere else.
 */
export function forgejoThreadId(path: string, side: 'old' | 'new', line: number): string {
  // Path last, and everything before it fixed-shape, so a path containing a colon
  // still parses back out.
  return `fj:${side}:${line}:${path}`
}

export function parseForgejoThreadId(
  id: string,
): { path: string; side: 'old' | 'new'; line: number } | null {
  const match = /^fj:(old|new):(\d+):(.+)$/.exec(id)
  if (!match) return null
  return { side: match[1] as 'old' | 'new', line: Number(match[2]), path: match[3] }
}

function forgejoComment(comment: ForgejoComment | ForgejoReviewComment): PullComment {
  return {
    id: String(comment.id),
    author: {
      name: comment.user?.login ?? 'unknown',
      avatarUrl: comment.user?.avatar_url ?? '',
    },
    body: comment.body,
    createdAt: comment.created_at,
  }
}

/**
 * Ordinary comments stay threads of one - Forgejo has no notion of replying to a
 * pull request comment - while review comments group by the line they were left on.
 *
 * A reply is another comment at the same anchor, which is how the conversation is
 * joined, so an inline thread can be replied to. Resolution is a different matter:
 * the REST API has no endpoint for it at all, so the state is read where the host
 * reports it and the control is never offered.
 */
export function forgejoThreads(
  comments: ForgejoComment[],
  reviewComments: ForgejoReviewComment[] = [],
): CommentThread[] {
  const threads: CommentThread[] = loneThreads(
    comments.map((comment) => ({ comment: forgejoComment(comment) })),
  )

  const conversations = new Map<string, ForgejoReviewComment[]>()
  for (const comment of reviewComments) {
    if (!comment.path) continue
    const side = comment.position ? 'new' : 'old'
    const line = comment.position || comment.original_position
    if (!line) continue

    const id = forgejoThreadId(comment.path, side, line)
    const existing = conversations.get(id)
    if (existing) existing.push(comment)
    else conversations.set(id, [comment])
  }

  for (const [id, group] of conversations) {
    const anchor = parseForgejoThreadId(id)
    const ordered = [...group].sort((a, b) => a.created_at.localeCompare(b.created_at))

    threads.push({
      id,
      comments: ordered.map(forgejoComment),
      // Forgejo resolves a whole conversation, so it counts only when all of it is.
      resolved: ordered.every((comment) => Boolean(comment.resolver)),
      outdated: false,
      path: anchor?.path,
      line: anchor?.line,
      side: anchor?.side,
      canReply: true,
      canResolve: false,
    })
  }

  return threads.sort((a, b) =>
    (a.comments[0]?.createdAt ?? '').localeCompare(b.comments[0]?.createdAt ?? ''),
  )
}

export function bitbucketThreads(comments: BitbucketComment[]): CommentThread[] {
  return loneThreads(
    comments
      .filter((comment) => !comment.deleted)
      .map((comment) => ({
        comment: {
          id: String(comment.id),
          author: {
            name: comment.user?.display_name ?? 'unknown',
            avatarUrl: comment.user?.links?.avatar?.href ?? '',
          },
          body: comment.content?.raw ?? '',
          createdAt: comment.created_on,
        },
        path: comment.inline?.path,
        line: comment.inline?.to ?? comment.inline?.from ?? undefined,
        side: comment.inline ? (comment.inline.to ? 'new' : 'old') : undefined,
      })),
  )
}

/**
 * GitLab is the host with real threads: a discussion carries its own notes, and
 * both replying and resolving are ordinary REST calls.
 *
 * `individual_note` marks a standalone comment, which GitLab refuses replies into,
 * and only some notes are resolvable at all - so both capabilities are read off the
 * discussion rather than assumed from the host.
 *
 * @param headSha the merge request's current head, to spot a thread left against a
 *   version of the diff that has since moved on.
 */
export function gitlabThreads(
  discussions: GitlabDiscussion[],
  headSha?: string,
): CommentThread[] {
  const threads: CommentThread[] = []

  for (const discussion of discussions) {
    // System notes are the "changed the description" chatter, not conversation.
    const notes = (discussion.notes ?? []).filter((note) => !note.system)
    if (!notes.length) continue

    const resolvable = notes.filter((note) => note.resolvable)
    const position = notes.find((note) => note.position)?.position ?? undefined
    const line = position?.new_line ?? position?.old_line ?? undefined

    threads.push({
      id: discussion.id,
      comments: notes.map((note) => ({
        id: String(note.id),
        author: {
          name: note.author?.username ?? 'unknown',
          avatarUrl: note.author?.avatar_url ?? '',
        },
        body: note.body,
        createdAt: note.created_at,
      })),
      resolved: resolvable.length > 0 && resolvable.every((note) => note.resolved === true),
      outdated: Boolean(headSha && position?.head_sha && position.head_sha !== headSha),
      path: position ? (position.new_path ?? position.old_path) : undefined,
      line: line ?? undefined,
      side: position?.new_line ? 'new' : position?.old_line ? 'old' : undefined,
      canReply: discussion.individual_note !== true,
      canResolve: resolvable.length > 0,
    })
  }

  return threads
}
