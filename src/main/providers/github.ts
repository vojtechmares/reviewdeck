/**
 * GitHub.com and GitHub Enterprise Server.
 *
 * GHES puts the API under /api/v3 on the same host as the web UI, so the
 * account keeps both roots and every call goes through `api()`.
 */

import { ApiError, paginate, request, toOrigin } from '../http.ts'
import { limitConcurrency, makeItemId, summariseChecks, type Provider, type Session } from './types.ts'
import {
  githubFlatThreads,
  githubThreads,
  type GithubComment,
  type GithubGqlComment,
  type GithubReviewThread,
} from './threads.ts'
import type {
  Account,
  AccountDraft,
  CheckRun,
  CheckStatus,
  CheckSummary,
  CommentThread,
  DiffFile,
  MyReviewState,
  ReviewItem,
} from '@shared/types.ts'

interface GhUser {
  login: string
  name?: string | null
  avatar_url: string
  html_url: string
}

interface GhSearchItem {
  id: number
  number: number
  title: string
  html_url: string
  state: string
  draft?: boolean
  created_at: string
  updated_at: string
  user: GhUser
  labels: { name: string }[]
  repository_url: string
  pull_request?: { url: string }
}

interface GhPull {
  number: number
  title: string
  body: string | null
  html_url: string
  draft: boolean
  created_at: string
  updated_at: string
  user: GhUser
  head: { ref: string; sha: string }
  base: { ref: string; sha: string }
  additions: number
  deletions: number
  changed_files: number
  labels: { name: string }[]
  state: string
}

interface GhFile {
  filename: string
  previous_filename?: string
  status: string
  additions: number
  deletions: number
  patch?: string
}

interface GhReview {
  id: number
  state: string
  user: GhUser
  body: string
  submitted_at: string
}

interface GhCheckRun {
  id: number
  name: string
  status: string
  conclusion: string | null
  html_url: string | null
  output?: { title?: string | null }
}

interface GhStatus {
  id: number
  context: string
  state: string
  target_url: string | null
  description: string | null
}

function headers(session: Session): Record<string, string> {
  return {
    Authorization: `Bearer ${session.token}`,
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
    'User-Agent': 'Reviewdeck',
  }
}

function api(session: Session, path: string): string {
  return `${session.account.baseUrl}${path}`
}

/** GitHub.com splits web and API onto different hosts; GHES uses /api/v3. */
function rootsFor(host: string): { baseUrl: string; webUrl: string } {
  const origin = toOrigin(host)
  const { hostname } = new URL(origin)
  if (hostname === 'github.com' || hostname === 'www.github.com' || hostname === 'api.github.com') {
    return { baseUrl: 'https://api.github.com', webUrl: 'https://github.com' }
  }
  return { baseUrl: `${origin}/api/v3`, webUrl: origin }
}

/**
 * Where the instance serves GraphQL.
 *
 * GitHub.com puts it on the API host; Enterprise Server puts it beside the REST
 * root on the instance's own host, one path segment up from `/api/v3`.
 */
export function graphqlRoot(baseUrl: string): string {
  const root = baseUrl.replace(/\/+$/, '')
  return root.endsWith('/api/v3') ? `${root.slice(0, -'/v3'.length)}/graphql` : `${root}/graphql`
}

interface GraphqlResponse<T> {
  data?: T | null
  errors?: { message?: string }[]
}

/**
 * A thin layer over the request wrapper, for GitHub only and only where REST
 * cannot answer: review threads are absent from the REST API entirely, and their
 * resolution exists nowhere but as a mutation here.
 */
async function graphql<T>(
  session: Session,
  query: string,
  variables: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<T> {
  const response = await request<GraphqlResponse<T>>(graphqlRoot(session.account.baseUrl), {
    method: 'POST',
    headers: headers(session),
    body: { query, variables },
    signal,
  })
  // GraphQL answers 200 with an errors array, so a failure has to be read out of
  // the body rather than the status.
  if (response.errors?.length) {
    throw new ApiError(response.errors[0]?.message ?? 'GitHub rejected the query.', 200, 'graphql')
  }
  if (!response.data) throw new ApiError('GitHub returned no data.', 200, 'graphql')
  return response.data
}

const THREADS_QUERY = `
query Threads($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          diffSide
          viewerCanReply
          viewerCanResolve
          viewerCanUnresolve
          comments(first: 50) {
            nodes { id body createdAt author { login avatarUrl } }
          }
        }
      }
      comments(first: 100) {
        nodes { id body createdAt author { login avatarUrl } }
      }
    }
  }
}`

interface ThreadsQuery {
  repository?: {
    pullRequest?: {
      reviewThreads?: { nodes?: (GithubReviewThread | null)[] | null } | null
      comments?: { nodes?: (GithubGqlComment | null)[] | null } | null
    } | null
  } | null
}

/** `acme/widgets` -> `{ owner: 'acme', name: 'widgets' }` */
function splitRepo(repoKey: string): { owner: string; name: string } {
  const at = repoKey.indexOf('/')
  return at < 0
    ? { owner: repoKey, name: repoKey }
    : { owner: repoKey.slice(0, at), name: repoKey.slice(at + 1) }
}

/** `https://api.github.com/repos/acme/widgets` -> `acme/widgets` */
function repoFromUrl(repositoryUrl: string): string {
  const parts = repositoryUrl.split('/repos/')
  return parts.length > 1 ? parts[1] : repositoryUrl
}

function checkStatus(run: GhCheckRun): CheckStatus {
  if (run.status !== 'completed') return 'running'
  switch (run.conclusion) {
    case 'success':
      return 'passed'
    case 'failure':
    case 'timed_out':
    case 'startup_failure':
      return 'failed'
    case 'cancelled':
    case 'action_required':
      return 'failed'
    case 'neutral':
    case 'skipped':
    case 'success_but_ignored':
      return 'passed'
    default:
      return 'unknown'
  }
}

function statusState(state: string): CheckStatus {
  if (state === 'success') return 'passed'
  if (state === 'failure' || state === 'error') return 'failed'
  if (state === 'pending') return 'running'
  return 'unknown'
}

function fileStatus(status: string): DiffFile['status'] {
  if (status === 'added') return 'added'
  if (status === 'removed') return 'removed'
  if (status === 'renamed') return 'renamed'
  return 'modified'
}

async function loadChecks(session: Session, repo: string, sha: string, signal?: AbortSignal): Promise<CheckSummary> {
  const runs: CheckRun[] = []

  // Check runs (Actions and most apps) and legacy commit statuses are separate APIs.
  try {
    const result = await request<{ check_runs: GhCheckRun[] }>(
      api(session, `/repos/${repo}/commits/${sha}/check-runs?per_page=100`),
      { headers: headers(session), signal },
    )
    for (const run of result.check_runs ?? []) {
      runs.push({
        id: `check-${run.id}`,
        name: run.name,
        status: checkStatus(run),
        url: run.html_url ?? undefined,
        description: run.output?.title ?? undefined,
      })
    }
  } catch (error) {
    if (!(error instanceof ApiError) || error.status !== 404) throw error
  }

  try {
    const result = await request<{ statuses: GhStatus[] }>(
      api(session, `/repos/${repo}/commits/${sha}/status`),
      { headers: headers(session), signal },
    )
    for (const status of result.statuses ?? []) {
      runs.push({
        id: `status-${status.id}`,
        name: status.context,
        status: statusState(status.state),
        url: status.target_url ?? undefined,
        description: status.description ?? undefined,
      })
    }
  } catch (error) {
    if (!(error instanceof ApiError) || error.status !== 404) throw error
  }

  return { ...summariseChecks(runs), runs }
}

async function myReviewState(
  session: Session,
  repo: string,
  number: number,
  signal?: AbortSignal,
): Promise<MyReviewState> {
  try {
    const reviews = await request<GhReview[]>(
      api(session, `/repos/${repo}/pulls/${number}/reviews?per_page=100`),
      { headers: headers(session), signal },
    )
    const mine = reviews.filter((review) => review.user?.login === session.account.username)
    // The last non-comment verdict is the one that counts.
    for (let i = mine.length - 1; i >= 0; i--) {
      const state = mine[i].state
      if (state === 'APPROVED') return 'approved'
      if (state === 'CHANGES_REQUESTED') return 'changes_requested'
    }
    if (mine.length) return 'commented'
  } catch {
    /* a missing review list should not sink the whole sync */
  }
  return 'pending'
}

export const github: Provider = {
  kind: 'github',

  async connect(draft: AccountDraft) {
    const { baseUrl, webUrl } = rootsFor(draft.host || 'github.com')
    const session = { account: { baseUrl } as Account, token: draft.token }
    const user = await request<GhUser>(`${baseUrl}/user`, { headers: headers(session) })
    return {
      kind: 'github' as const,
      label: draft.label || `GitHub (${user.login})`,
      baseUrl,
      webUrl,
      username: user.login,
      displayName: user.name || user.login,
      avatarUrl: user.avatar_url,
    }
  },

  async listReviewRequests(session, signal) {
    // The search API is the only cheap way to ask "across every org I belong to".
    const query = encodeURIComponent('is:open is:pr archived:false review-requested:@me')
    const search = await request<{ items: GhSearchItem[] }>(
      api(session, `/search/issues?q=${query}&per_page=50&sort=updated&order=desc`),
      { headers: headers(session), signal },
    )

    const items = search.items ?? []
    return limitConcurrency(items, 6, async (found) => {
      const repo = repoFromUrl(found.repository_url)
      const pull = await request<GhPull>(api(session, `/repos/${repo}/pulls/${found.number}`), {
        headers: headers(session),
        signal,
      })
      const [checks, reviewState] = await Promise.all([
        loadChecks(session, repo, pull.head.sha, signal).catch(() => emptyChecks()),
        myReviewState(session, repo, found.number, signal),
      ])

      const item: ReviewItem = {
        id: makeItemId(session.account.id, repo, found.number),
        accountId: session.account.id,
        provider: 'github',
        repoKey: repo,
        repo,
        number: found.number,
        title: pull.title,
        url: pull.html_url,
        author: { name: pull.user.login, avatarUrl: pull.user.avatar_url },
        createdAt: pull.created_at,
        updatedAt: pull.updated_at,
        draft: pull.draft,
        sourceBranch: pull.head.ref,
        targetBranch: pull.base.ref,
        labels: (pull.labels ?? []).map((label) => label.name),
        myReviewState: reviewState,
        checks,
        additions: pull.additions,
        deletions: pull.deletions,
        changedFiles: pull.changed_files,
      }
      return item
    })
  },

  async loadDetail(session, item, signal) {
    const [pull, rawFiles, threads] = await Promise.all([
      request<GhPull>(api(session, `/repos/${item.repoKey}/pulls/${item.number}`), {
        headers: headers(session),
        signal,
      }),
      paginate<GhFile>(
        api(session, `/repos/${item.repoKey}/pulls/${item.number}/files?per_page=100`),
        { headers: headers(session), signal },
        4,
      ),
      loadThreads(session, item, signal),
    ])

    const files: DiffFile[] = rawFiles.map((file) => ({
      path: file.filename,
      oldPath: file.previous_filename ?? file.filename,
      status: fileStatus(file.status),
      additions: file.additions,
      deletions: file.deletions,
      patch: file.patch ?? null,
      binary: !file.patch && file.additions === 0 && file.deletions === 0,
    }))

    return {
      item: { ...item, additions: pull.additions, deletions: pull.deletions, changedFiles: pull.changed_files },
      description: pull.body ?? '',
      files,
      threads,
      refs: { headSha: pull.head.sha, baseSha: pull.base.sha },
    }
  },

  // The thread's global node id is the whole address, so the item is not needed.
  async replyToThread(session, _item, threadId, body) {
    await graphql(
      session,
      `mutation Reply($threadId: ID!, $body: String!) {
        addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $threadId, body: $body }) {
          clientMutationId
        }
      }`,
      { threadId, body },
    )
  },

  async setThreadResolved(session, _item, threadId, resolved) {
    const mutation = resolved
      ? `mutation Resolve($threadId: ID!) {
          resolveReviewThread(input: { threadId: $threadId }) { clientMutationId }
        }`
      : `mutation Unresolve($threadId: ID!) {
          unresolveReviewThread(input: { threadId: $threadId }) { clientMutationId }
        }`
    await graphql(session, mutation, { threadId })
  },

  async refreshChecks(session, item, signal) {
    const pull = await request<GhPull>(api(session, `/repos/${item.repoKey}/pulls/${item.number}`), {
      headers: headers(session),
      signal,
    })
    return loadChecks(session, item.repoKey, pull.head.sha, signal)
  },

  async submitReview(session, item, verdict, body, comments) {
    const event =
      verdict === 'approve' ? 'APPROVE' : verdict === 'request_changes' ? 'REQUEST_CHANGES' : 'COMMENT'

    await request(api(session, `/repos/${item.repoKey}/pulls/${item.number}/reviews`), {
      method: 'POST',
      headers: headers(session),
      body: {
        event,
        body: body || undefined,
        // Against the commit the drafts were written on, not whatever the branch
        // has become, so each remark lands on the code its author actually read.
        commit_id: comments[0]?.refs.headSha,
        comments: comments.length
          ? comments.map((comment) => ({
              path: comment.path,
              body: comment.body,
              side: comment.newLine ? 'RIGHT' : 'LEFT',
              line: comment.newLine ?? comment.oldLine,
            }))
          : undefined,
      },
    })
  },

  async addComment(session, item, body) {
    await request(api(session, `/repos/${item.repoKey}/issues/${item.number}/comments`), {
      method: 'POST',
      headers: headers(session),
      body: { body },
    })
  },

  async addLineComment(session, item, draft, refs) {
    if (!refs.headSha) throw new Error('Missing the head commit for this pull request.')
    await request(api(session, `/repos/${item.repoKey}/pulls/${item.number}/comments`), {
      method: 'POST',
      headers: headers(session),
      body: {
        body: draft.body,
        commit_id: refs.headSha,
        path: draft.path,
        // GitHub addresses a line by side: RIGHT for the post-change file, LEFT for pre-change.
        side: draft.newLine ? 'RIGHT' : 'LEFT',
        line: draft.newLine ?? draft.oldLine,
      },
    })
  },
}

/**
 * Threads for one pull request, over GraphQL because REST has no notion of them.
 *
 * An instance that will not answer - an older Enterprise Server, a token GraphQL
 * turns away - falls back to the flat REST shape, which is what the app showed
 * before. Losing the thread structure is much better than losing the comments.
 */
async function loadThreads(
  session: Session,
  item: ReviewItem,
  signal?: AbortSignal,
): Promise<CommentThread[]> {
  const { owner, name } = splitRepo(item.repoKey)

  try {
    const data = await graphql<ThreadsQuery>(
      session,
      THREADS_QUERY,
      { owner, name, number: item.number },
      signal,
    )
    const pull = data.repository?.pullRequest
    return githubThreads(
      (pull?.reviewThreads?.nodes ?? []).filter(
        (thread): thread is GithubReviewThread => Boolean(thread),
      ),
      (pull?.comments?.nodes ?? []).filter(
        (comment): comment is GithubGqlComment => Boolean(comment),
      ),
    )
  } catch {
    const [issueComments, reviewComments] = await Promise.all([
      request<GithubComment[]>(
        api(session, `/repos/${item.repoKey}/issues/${item.number}/comments?per_page=100`),
        { headers: headers(session), signal },
      ).catch(() => [] as GithubComment[]),
      request<GithubComment[]>(
        api(session, `/repos/${item.repoKey}/pulls/${item.number}/comments?per_page=100`),
        { headers: headers(session), signal },
      ).catch(() => [] as GithubComment[]),
    ])
    return githubFlatThreads(issueComments, reviewComments)
  }
}

function emptyChecks(): CheckSummary {
  return { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] }
}
