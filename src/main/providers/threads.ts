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
 * GitHub's review threads and their resolution live only in GraphQL, so over REST
 * every comment stands alone. Issue comments and review comments interleave by age.
 */
export function githubThreads(
  issueComments: GithubComment[],
  reviewComments: GithubComment[],
): CommentThread[] {
  const anchored = [...issueComments, ...reviewComments]
    .map(githubAnchored)
    .sort((a, b) => a.comment.createdAt.localeCompare(b.comment.createdAt))
  return loneThreads(anchored)
}

export function forgejoThreads(comments: ForgejoComment[]): CommentThread[] {
  return loneThreads(
    comments.map((comment) => ({
      comment: {
        id: String(comment.id),
        author: {
          name: comment.user?.login ?? 'unknown',
          avatarUrl: comment.user?.avatar_url ?? '',
        },
        body: comment.body,
        createdAt: comment.created_at,
      },
    })),
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
