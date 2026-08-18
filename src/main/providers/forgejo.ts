/**
 * Forgejo and Gitea share an API surface, so one adapter covers both.
 *
 * `/repos/issues/search?review_requested=true` aggregates across every repo the
 * token can see, which keeps the sync to a single round trip plus per-PR detail.
 */

import { request, toOrigin } from '../http.ts'
import { limitConcurrency, makeItemId, summariseChecks, type Provider, type Session } from './types.ts'
import { parseUnifiedDiff } from '@shared/diff.ts'
import type {
  Account,
  AccountDraft,
  CheckRun,
  CheckStatus,
  CheckSummary,
  MyReviewState,
  PullComment,
  ReviewItem,
} from '@shared/types.ts'

interface FjUser {
  id: number
  login: string
  full_name: string
  avatar_url: string
}

interface FjRepository {
  id: number
  full_name: string
  owner: FjUser
  name: string
}

interface FjIssue {
  id: number
  number: number
  title: string
  body: string
  html_url: string
  state: string
  created_at: string
  updated_at: string
  user: FjUser
  labels: { name: string }[]
  repository?: { id: number; name: string; owner: string; full_name?: string }
  pull_request?: { merged: boolean; draft?: boolean }
}

interface FjPull {
  number: number
  title: string
  body: string
  html_url: string
  draft: boolean
  created_at: string
  updated_at: string
  user: FjUser
  head: { ref: string; sha: string; repo?: FjRepository }
  base: { ref: string; sha: string; repo?: FjRepository }
  additions?: number
  deletions?: number
  changed_files?: number
  labels: { name: string }[]
}

interface FjReview {
  id: number
  state: string
  user: FjUser
  body: string
  submitted_at: string
}

interface FjComment {
  id: number
  user: FjUser
  body: string
  created_at: string
}

interface FjStatus {
  id: number
  status: string
  context: string
  target_url: string
  description: string
}

interface FjCombinedStatus {
  state: string
  statuses: FjStatus[]
}

function headers(session: Session): Record<string, string> {
  return {
    Authorization: `token ${session.token}`,
    Accept: 'application/json',
    'User-Agent': 'Reviewdeck',
  }
}

function api(session: Session, path: string): string {
  return `${session.account.baseUrl}${path}`
}

function statusState(status: string): CheckStatus {
  switch (status) {
    case 'success':
      return 'passed'
    case 'failure':
    case 'error':
      return 'failed'
    case 'pending':
      return 'running'
    case 'warning':
      return 'unknown'
    default:
      return 'unknown'
  }
}

/** The search result nests repo differently across Gitea/Forgejo versions. */
function repoName(issue: FjIssue): string {
  const repo = issue.repository
  if (!repo) {
    // Fall back to slicing the html_url: https://host/owner/repo/pulls/12
    try {
      const parts = new URL(issue.html_url).pathname.split('/').filter(Boolean)
      if (parts.length >= 2) return `${parts[0]}/${parts[1]}`
    } catch {
      /* ignore */
    }
    return 'unknown/unknown'
  }
  return repo.full_name ?? `${repo.owner}/${repo.name}`
}

async function loadChecks(session: Session, repo: string, sha: string, signal?: AbortSignal): Promise<CheckSummary> {
  try {
    const combined = await request<FjCombinedStatus>(
      api(session, `/repos/${repo}/commits/${sha}/status`),
      { headers: headers(session), signal },
    )
    const runs: CheckRun[] = (combined.statuses ?? []).map((status) => ({
      id: `status-${status.id}`,
      name: status.context || 'check',
      status: statusState(status.status),
      url: status.target_url || undefined,
      description: status.description || undefined,
    }))
    if (!runs.length) {
      // No individual statuses but a combined verdict still tells us something.
      const rolled = statusState(combined.state)
      if (rolled === 'unknown') return emptyChecks()
      const run: CheckRun = { id: 'combined', name: 'Checks', status: rolled }
      return { ...summariseChecks([run]), runs: [run] }
    }
    return { ...summariseChecks(runs), runs }
  } catch {
    return emptyChecks()
  }
}

async function myReviewState(
  session: Session,
  repo: string,
  number: number,
  signal?: AbortSignal,
): Promise<MyReviewState> {
  try {
    const reviews = await request<FjReview[]>(api(session, `/repos/${repo}/pulls/${number}/reviews`), {
      headers: headers(session),
      signal,
    })
    const mine = reviews.filter((review) => review.user?.login === session.account.username)
    for (let i = mine.length - 1; i >= 0; i--) {
      if (mine[i].state === 'APPROVED') return 'approved'
      if (mine[i].state === 'REQUEST_CHANGES') return 'changes_requested'
    }
    if (mine.some((review) => review.state === 'COMMENT')) return 'commented'
  } catch {
    /* not fatal */
  }
  return 'pending'
}

export const forgejo: Provider = {
  kind: 'forgejo',

  async connect(draft: AccountDraft) {
    const origin = toOrigin(draft.host)
    const baseUrl = `${origin}/api/v1`
    const session = { account: { baseUrl } as Account, token: draft.token }
    const user = await request<FjUser>(`${baseUrl}/user`, { headers: headers(session) })
    return {
      kind: 'forgejo' as const,
      label: draft.label || `${new URL(origin).host} (${user.login})`,
      baseUrl,
      webUrl: origin,
      username: user.login,
      displayName: user.full_name || user.login,
      avatarUrl: user.avatar_url,
    }
  },

  async listReviewRequests(session, signal) {
    const issues = await request<FjIssue[]>(
      api(session, '/repos/issues/search?type=pulls&state=open&review_requested=true&limit=50&sort=recentupdate'),
      { headers: headers(session), signal },
    )

    return limitConcurrency(issues, 5, async (issue) => {
      const repo = repoName(issue)
      const pull = await request<FjPull>(api(session, `/repos/${repo}/pulls/${issue.number}`), {
        headers: headers(session),
        signal,
      })
      const [checks, reviewState] = await Promise.all([
        loadChecks(session, repo, pull.head.sha, signal),
        myReviewState(session, repo, issue.number, signal),
      ])

      const item: ReviewItem = {
        id: makeItemId(session.account.id, repo, issue.number),
        accountId: session.account.id,
        provider: 'forgejo',
        repoKey: repo,
        repo,
        number: issue.number,
        title: pull.title,
        url: pull.html_url,
        author: { name: pull.user.login, avatarUrl: pull.user.avatar_url },
        createdAt: pull.created_at,
        updatedAt: pull.updated_at,
        draft: pull.draft ?? false,
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
    const [pull, diff, comments] = await Promise.all([
      request<FjPull>(api(session, `/repos/${item.repoKey}/pulls/${item.number}`), {
        headers: headers(session),
        signal,
      }),
      request<string>(api(session, `/repos/${item.repoKey}/pulls/${item.number}.diff`), {
        headers: headers(session),
        raw: true,
        signal,
      }).catch(() => ''),
      request<FjComment[]>(api(session, `/repos/${item.repoKey}/issues/${item.number}/comments`), {
        headers: headers(session),
        signal,
      }).catch(() => [] as FjComment[]),
    ])

    const files = parseUnifiedDiff(diff)
    const totals = files.reduce(
      (acc, file) => ({ a: acc.a + file.additions, d: acc.d + file.deletions }),
      { a: 0, d: 0 },
    )

    const mapped: PullComment[] = comments.map((comment) => ({
      id: String(comment.id),
      author: { name: comment.user?.login ?? 'unknown', avatarUrl: comment.user?.avatar_url ?? '' },
      body: comment.body,
      createdAt: comment.created_at,
    }))

    return {
      item: {
        ...item,
        additions: pull.additions ?? totals.a,
        deletions: pull.deletions ?? totals.d,
        changedFiles: pull.changed_files ?? files.length,
      },
      description: pull.body ?? '',
      files,
      comments: mapped,
      refs: { headSha: pull.head.sha, baseSha: pull.base.sha },
    }
  },

  async refreshChecks(session, item, signal) {
    const pull = await request<FjPull>(api(session, `/repos/${item.repoKey}/pulls/${item.number}`), {
      headers: headers(session),
      signal,
    })
    return loadChecks(session, item.repoKey, pull.head.sha, signal)
  },

  async submitReview(session, item, verdict, body) {
    const event =
      verdict === 'approve' ? 'APPROVED' : verdict === 'request_changes' ? 'REQUEST_CHANGES' : 'COMMENT'
    await request(api(session, `/repos/${item.repoKey}/pulls/${item.number}/reviews`), {
      method: 'POST',
      headers: headers(session),
      // A COMMENT review with an empty body is rejected, so always send something.
      body: { event, body: body || (verdict === 'approve' ? 'Approved.' : 'Reviewed.') },
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
    // Forgejo attaches inline comments to a review rather than to the PR directly.
    await request(api(session, `/repos/${item.repoKey}/pulls/${item.number}/reviews`), {
      method: 'POST',
      headers: headers(session),
      body: {
        event: 'COMMENT',
        body: '',
        commit_id: refs.headSha,
        comments: [
          {
            path: draft.path,
            body: draft.body,
            new_position: draft.newLine ?? 0,
            old_position: draft.oldLine ?? 0,
          },
        ],
      },
    })
  },
}

function emptyChecks(): CheckSummary {
  return { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] }
}
