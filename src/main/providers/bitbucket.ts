/**
 * Bitbucket Cloud, authenticated with an app password (username + password pair).
 *
 * Bitbucket has no "PRs awaiting my review" endpoint - `/pullrequests/{user}` only
 * returns PRs the user *authored*. So we walk the workspaces the token can see,
 * list each repo's open PRs, and keep the ones naming us as a reviewer. The repo
 * fan-out is bounded and runs with limited concurrency to stay polite.
 */

import { request, toOrigin } from '../http.ts'
import { limitConcurrency, makeItemId, summariseChecks, type Provider, type Session } from './types.ts'
import { bitbucketThreads, type BitbucketComment } from './threads.ts'
import { bitbucketCommentPayload, submitSequentially } from './submit.ts'
import { parseUnifiedDiff } from '@shared/diff.ts'
import type {
  Account,
  AccountDraft,
  CheckRun,
  CheckStatus,
  CheckSummary,
  DraftComment,
  MyReviewState,
  ReviewItem,
} from '@shared/types.ts'

const API_ROOT = 'https://api.bitbucket.org/2.0'
const WEB_ROOT = 'https://bitbucket.org'
/** Guard rail: a token with access to hundreds of repos should not stall a sync. */
const MAX_REPOS = 120

interface BbPage<T> {
  values: T[]
  next?: string
}

interface BbUser {
  uuid: string
  display_name: string
  nickname?: string
  account_id?: string
  links?: { avatar?: { href: string } }
}

interface BbWorkspace {
  slug: string
  name: string
}

interface BbRepo {
  full_name: string
  slug: string
  workspace: { slug: string }
  updated_on: string
}

interface BbParticipant {
  user: BbUser
  role: string
  approved: boolean
  state: string | null
}

interface BbPullRequest {
  id: number
  title: string
  description: string
  state: string
  created_on: string
  updated_on: string
  author: BbUser
  reviewers?: BbUser[]
  participants?: BbParticipant[]
  source: { branch: { name: string }; commit?: { hash: string } }
  destination: { branch: { name: string }; commit?: { hash: string } }
  links: { html: { href: string } }
  draft?: boolean
}

interface BbStatus {
  key: string
  name: string
  state: string
  url: string
  description: string
}

function headers(session: Session): Record<string, string> {
  // App passwords use HTTP Basic; the account stores the username alongside them.
  const basic = Buffer.from(`${session.account.username}:${session.token}`).toString('base64')
  return { Authorization: `Basic ${basic}`, Accept: 'application/json', 'User-Agent': 'Reviewdeck' }
}

function avatar(user: BbUser | undefined): string {
  return user?.links?.avatar?.href ?? ''
}

function statusState(state: string): CheckStatus {
  switch (state) {
    case 'SUCCESSFUL':
      return 'passed'
    case 'FAILED':
      return 'failed'
    case 'INPROGRESS':
      return 'running'
    case 'STOPPED':
      return 'failed'
    default:
      return 'unknown'
  }
}

/** Walk Bitbucket's `next`-URL pagination, bounded by `max` items. */
async function collect<T>(url: string, session: Session, max: number, signal?: AbortSignal): Promise<T[]> {
  const out: T[] = []
  let next: string | undefined = url
  while (next && out.length < max) {
    const page: BbPage<T> = await request<BbPage<T>>(next, { headers: headers(session), signal })
    out.push(...(page.values ?? []))
    next = page.next
  }
  return out.slice(0, max)
}

async function loadChecks(session: Session, repo: string, id: number, signal?: AbortSignal): Promise<CheckSummary> {
  try {
    const statuses = await collect<BbStatus>(
      `${API_ROOT}/repositories/${repo}/pullrequests/${id}/statuses?pagelen=50`,
      session,
      50,
      signal,
    )
    const runs: CheckRun[] = statuses.map((status) => ({
      id: `status-${status.key}`,
      name: status.name || status.key,
      status: statusState(status.state),
      url: status.url || undefined,
      description: status.description || undefined,
    }))
    return { ...summariseChecks(runs), runs }
  } catch {
    return emptyChecks()
  }
}

function reviewStateFor(pr: BbPullRequest, uuid: string): MyReviewState {
  const me = (pr.participants ?? []).find((participant) => participant.user?.uuid === uuid)
  if (!me) return 'pending'
  if (me.state === 'approved' || me.approved) return 'approved'
  if (me.state === 'changes_requested') return 'changes_requested'
  return 'pending'
}

function isMyReview(pr: BbPullRequest, uuid: string): boolean {
  if ((pr.reviewers ?? []).some((reviewer) => reviewer.uuid === uuid)) return true
  return (pr.participants ?? []).some(
    (participant) => participant.role === 'REVIEWER' && participant.user?.uuid === uuid,
  )
}

export const bitbucket: Provider = {
  kind: 'bitbucket',

  async connect(draft: AccountDraft) {
    if (!draft.username) throw new Error('Bitbucket needs the username the app password belongs to.')
    // Bitbucket Cloud is the only host; a custom host means Bitbucket Server, which is a different API.
    if (draft.host && !/bitbucket\.org/i.test(toOrigin(draft.host))) {
      throw new Error('Only Bitbucket Cloud (bitbucket.org) is supported right now.')
    }
    const session = {
      account: { baseUrl: API_ROOT, username: draft.username } as Account,
      token: draft.token,
    }
    const user = await request<BbUser>(`${API_ROOT}/user`, { headers: headers(session) })
    return {
      kind: 'bitbucket' as const,
      label: draft.label || `Bitbucket (${user.nickname ?? user.display_name})`,
      baseUrl: API_ROOT,
      webUrl: WEB_ROOT,
      // The uuid is what PR payloads identify people by, so keep it as the username.
      username: draft.username,
      displayName: user.display_name,
      avatarUrl: avatar(user),
    }
  },

  async listReviewRequests(session, signal) {
    const me = await request<BbUser>(`${API_ROOT}/user`, { headers: headers(session), signal })
    const workspaces = await collect<BbWorkspace>(
      `${API_ROOT}/workspaces?pagelen=50&fields=values.slug,values.name,next`,
      session,
      50,
      signal,
    )

    const repos: BbRepo[] = []
    for (const workspace of workspaces) {
      if (repos.length >= MAX_REPOS) break
      const found = await collect<BbRepo>(
        `${API_ROOT}/repositories/${workspace.slug}?role=member&sort=-updated_on&pagelen=50` +
          '&fields=values.full_name,values.slug,values.workspace.slug,values.updated_on,next',
        session,
        MAX_REPOS - repos.length,
        signal,
      ).catch(() => [] as BbRepo[])
      repos.push(...found)
    }

    const perRepo = await limitConcurrency(repos, 6, async (repo) => {
      const pulls = await collect<BbPullRequest>(
        `${API_ROOT}/repositories/${repo.full_name}/pullrequests?state=OPEN&pagelen=50`,
        session,
        50,
        signal,
      ).catch(() => [] as BbPullRequest[])
      return pulls
        .filter((pr) => isMyReview(pr, me.uuid) && pr.author?.uuid !== me.uuid)
        .map((pr) => ({ repo: repo.full_name, pr }))
    })

    const candidates = perRepo.flat()
    return limitConcurrency(candidates, 6, async ({ repo, pr }) => {
      const checks = await loadChecks(session, repo, pr.id, signal)
      const item: ReviewItem = {
        id: makeItemId(session.account.id, repo, pr.id),
        accountId: session.account.id,
        provider: 'bitbucket',
        repoKey: repo,
        repo,
        number: pr.id,
        title: pr.title,
        url: pr.links?.html?.href ?? `${WEB_ROOT}/${repo}/pull-requests/${pr.id}`,
        author: { name: pr.author?.display_name ?? 'unknown', avatarUrl: avatar(pr.author) },
        createdAt: pr.created_on,
        updatedAt: pr.updated_on,
        draft: pr.draft ?? false,
        sourceBranch: pr.source?.branch?.name ?? '',
        targetBranch: pr.destination?.branch?.name ?? '',
        labels: [],
        myReviewState: reviewStateFor(pr, me.uuid),
        checks,
      }
      return item
    })
  },

  async loadDetail(session, item, signal) {
    const [pr, diff, comments] = await Promise.all([
      request<BbPullRequest>(`${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}`, {
        headers: headers(session),
        signal,
      }),
      request<string>(`${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}/diff`, {
        headers: headers(session),
        raw: true,
        signal,
      }).catch(() => ''),
      collect<BitbucketComment>(
        `${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}/comments?pagelen=50`,
        session,
        100,
        signal,
      ).catch(() => [] as BitbucketComment[]),
    ])

    const files = parseUnifiedDiff(diff)
    const totals = files.reduce(
      (acc, file) => ({ a: acc.a + file.additions, d: acc.d + file.deletions }),
      { a: 0, d: 0 },
    )

    return {
      item: { ...item, additions: totals.a, deletions: totals.d, changedFiles: files.length },
      description: pr.description ?? '',
      files,
      threads: bitbucketThreads(comments),
      refs: { headSha: pr.source?.commit?.hash, baseSha: pr.destination?.commit?.hash },
    }
  },

  async refreshChecks(session, item, signal) {
    return loadChecks(session, item.repoKey, item.number, signal)
  },

  async submitReview(session, item, verdict, body, comments) {
    // No batch call at all here, so the comments go one at a time and the verdict
    // last - reported honestly if it stops part-way.
    const base = `${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}`

    await submitSequentially(
      comments,
      (comment) => postComment(session, item, comment),
      async () => {
        if (verdict === 'approve') {
          // Clear any standing "request changes" first, otherwise both flags linger.
          await request(`${base}/request-changes`, {
            method: 'DELETE',
            headers: headers(session),
          }).catch(() => undefined)
          await request(`${base}/approve`, { method: 'POST', headers: headers(session) })
        } else if (verdict === 'request_changes') {
          await request(`${base}/approve`, { method: 'DELETE', headers: headers(session) }).catch(
            () => undefined,
          )
          await request(`${base}/request-changes`, { method: 'POST', headers: headers(session) })
        }
        if (body) await this.addComment(session, item, body)
      },
    )
  },

  async addComment(session, item, body) {
    await request(`${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}/comments`, {
      method: 'POST',
      headers: headers(session),
      body: { content: { raw: body } },
    })
  },

  async addLineComment(session, item, draft) {
    await postComment(session, item, {
      id: '',
      itemId: item.id,
      createdAt: '',
      body: draft.body,
      path: draft.path,
      newLine: draft.newLine,
      oldLine: draft.oldLine,
      refs: {},
    })
  },

  /** A reply is an ordinary comment naming the one that opened the thread. */
  async replyToThread(session, item, threadId, body) {
    await request(`${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}/comments`, {
      method: 'POST',
      headers: headers(session),
      body: { content: { raw: body }, parent: { id: Number(threadId) } },
    })
  },

  /** Resolving posts to the thread's opening comment; reopening deletes the same. */
  async setThreadResolved(session, item, threadId, resolved) {
    const url = `${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}/comments/${encodeURIComponent(threadId)}/resolve`
    await request(url, { method: resolved ? 'POST' : 'DELETE', headers: headers(session) })
  },
}

/** One inline comment, on the new file or the old one. */
async function postComment(
  session: Session,
  item: ReviewItem,
  comment: DraftComment,
): Promise<void> {
  await request(`${API_ROOT}/repositories/${item.repoKey}/pullrequests/${item.number}/comments`, {
    method: 'POST',
    headers: headers(session),
    body: bitbucketCommentPayload(comment),
  })
}

function emptyChecks(): CheckSummary {
  return { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] }
}
