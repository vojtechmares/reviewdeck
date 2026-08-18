/**
 * GitLab.com and self-hosted GitLab.
 *
 * `GET /merge_requests?scope=reviews_for_me` does the cross-project aggregation
 * for us, which is the one thing GitLab makes easier than everyone else.
 */

import { request, toOrigin } from '../http.ts'
import { limitConcurrency, makeItemId, summariseChecks, type Provider, type Session } from './types.ts'
import { gitlabThreads, type GitlabDiscussion } from './threads.ts'
import { gitlabDiscussionPayload, submitSequentially } from './submit.ts'
import { countChanges } from '@shared/diff.ts'
import type {
  Account,
  AccountDraft,
  CheckRun,
  CheckStatus,
  CheckSummary,
  DiffFile,
  DraftComment,
  MyReviewState,
  ReviewItem,
} from '@shared/types.ts'

interface GlUser {
  id: number
  username: string
  name: string
  avatar_url: string | null
  web_url: string
}

interface GlMergeRequest {
  id: number
  iid: number
  project_id: number
  title: string
  description: string | null
  state: string
  draft: boolean
  work_in_progress?: boolean
  created_at: string
  updated_at: string
  author: GlUser
  reviewers?: GlUser[]
  source_branch: string
  target_branch: string
  labels: string[]
  web_url: string
  sha: string
  references?: { full?: string }
  changes_count?: string
}

interface GlDiff {
  old_path: string
  new_path: string
  new_file: boolean
  renamed_file: boolean
  deleted_file: boolean
  diff: string
}

interface GlVersion {
  head_commit_sha: string
  base_commit_sha: string
  start_commit_sha: string
}

interface GlPipeline {
  id: number
  status: string
  web_url: string
  ref: string
}

interface GlJob {
  id: number
  name: string
  status: string
  web_url: string
  stage: string
  allow_failure: boolean
}

interface GlApproval {
  approved_by: { user: GlUser }[]
}

function headers(session: Session): Record<string, string> {
  return { 'PRIVATE-TOKEN': session.token, 'User-Agent': 'Reviewdeck' }
}

function api(session: Session, path: string): string {
  return `${session.account.baseUrl}${path}`
}

function project(item: ReviewItem): string {
  return encodeURIComponent(item.repoKey)
}

function pipelineStatus(status: string): CheckStatus {
  switch (status) {
    case 'success':
      return 'passed'
    case 'failed':
      return 'failed'
    case 'running':
    case 'pending':
    case 'created':
    case 'waiting_for_resource':
    case 'preparing':
    case 'scheduled':
      return 'running'
    case 'canceled':
    case 'canceling':
      return 'failed'
    case 'skipped':
    case 'manual':
      return 'unknown'
    default:
      return 'unknown'
  }
}

/** `gitlab-org/gitlab!123` -> `gitlab-org/gitlab` */
function pathFromReference(mr: GlMergeRequest): string {
  const full = mr.references?.full
  if (full) {
    const at = full.indexOf('!')
    if (at > 0) return full.slice(0, at)
  }
  return String(mr.project_id)
}

async function loadChecks(session: Session, projectId: string, iid: number, signal?: AbortSignal): Promise<CheckSummary> {
  const pipelines = await request<GlPipeline[]>(
    api(session, `/projects/${encodeURIComponent(projectId)}/merge_requests/${iid}/pipelines`),
    { headers: headers(session), signal },
  ).catch(() => [] as GlPipeline[])

  const latest = pipelines[0]
  if (!latest) return { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] }

  // Jobs give the per-check breakdown; if they are hidden, fall back to the pipeline itself.
  const jobs = await request<GlJob[]>(
    api(session, `/projects/${encodeURIComponent(projectId)}/pipelines/${latest.id}/jobs?per_page=100`),
    { headers: headers(session), signal },
  ).catch(() => [] as GlJob[])

  if (!jobs.length) {
    const status = pipelineStatus(latest.status)
    const run: CheckRun = { id: `pipeline-${latest.id}`, name: `Pipeline #${latest.id}`, status, url: latest.web_url }
    return { ...summariseChecks([run]), runs: [run] }
  }

  const runs: CheckRun[] = jobs.map((job) => ({
    id: `job-${job.id}`,
    name: job.name,
    // An allow_failure job that failed should not paint the whole MR red.
    status: job.status === 'failed' && job.allow_failure ? 'unknown' : pipelineStatus(job.status),
    url: job.web_url,
    description: job.stage,
  }))
  return { ...summariseChecks(runs), runs }
}

async function myReviewState(
  session: Session,
  projectId: string,
  iid: number,
  signal?: AbortSignal,
): Promise<MyReviewState> {
  try {
    const approvals = await request<GlApproval>(
      api(session, `/projects/${encodeURIComponent(projectId)}/merge_requests/${iid}/approvals`),
      { headers: headers(session), signal },
    )
    const approved = (approvals.approved_by ?? []).some(
      (entry) => entry.user?.username === session.account.username,
    )
    if (approved) return 'approved'
  } catch {
    /* approvals are a premium feature on some tiers; treat as not-approved */
  }
  return 'pending'
}

export const gitlab: Provider = {
  kind: 'gitlab',

  async connect(draft: AccountDraft) {
    const origin = toOrigin(draft.host || 'gitlab.com')
    const baseUrl = `${origin}/api/v4`
    const session = { account: { baseUrl } as Account, token: draft.token }
    const user = await request<GlUser>(`${baseUrl}/user`, { headers: headers(session) })
    return {
      kind: 'gitlab' as const,
      label: draft.label || `GitLab (${user.username})`,
      baseUrl,
      webUrl: origin,
      username: user.username,
      displayName: user.name || user.username,
      avatarUrl: user.avatar_url ?? '',
    }
  },

  async listReviewRequests(session, signal) {
    const merges = await request<GlMergeRequest[]>(
      api(session, '/merge_requests?scope=reviews_for_me&state=opened&per_page=50&order_by=updated_at'),
      { headers: headers(session), signal },
    )

    return limitConcurrency(merges, 5, async (mr) => {
      const projectPath = pathFromReference(mr)
      const projectId = String(mr.project_id)
      const [checks, reviewState] = await Promise.all([
        loadChecks(session, projectId, mr.iid, signal).catch(() => emptyChecks()),
        myReviewState(session, projectId, mr.iid, signal),
      ])

      const item: ReviewItem = {
        id: makeItemId(session.account.id, projectId, mr.iid),
        accountId: session.account.id,
        provider: 'gitlab',
        // API calls use the numeric id; the path is only for display.
        repoKey: projectId,
        repo: projectPath,
        number: mr.iid,
        title: mr.title,
        url: mr.web_url,
        author: { name: mr.author.username, avatarUrl: mr.author.avatar_url ?? '' },
        createdAt: mr.created_at,
        updatedAt: mr.updated_at,
        draft: mr.draft ?? mr.work_in_progress ?? false,
        sourceBranch: mr.source_branch,
        targetBranch: mr.target_branch,
        labels: mr.labels ?? [],
        myReviewState: reviewState,
        checks,
      }
      return item
    })
  },

  async loadDetail(session, item, signal) {
    const [mr, changes, versions, discussions] = await Promise.all([
      request<GlMergeRequest>(api(session, `/projects/${project(item)}/merge_requests/${item.number}`), {
        headers: headers(session),
        signal,
      }),
      request<{ diffs?: GlDiff[]; changes?: GlDiff[] }>(
        api(session, `/projects/${project(item)}/merge_requests/${item.number}/changes`),
        { headers: headers(session), signal },
      ),
      request<GlVersion[]>(
        api(session, `/projects/${project(item)}/merge_requests/${item.number}/versions`),
        { headers: headers(session), signal },
      ).catch(() => [] as GlVersion[]),
      request<GitlabDiscussion[]>(
        api(session, `/projects/${project(item)}/merge_requests/${item.number}/discussions?per_page=100`),
        { headers: headers(session), signal },
      ).catch(() => [] as GitlabDiscussion[]),
    ])

    const rawDiffs = changes.changes ?? changes.diffs ?? []
    const files: DiffFile[] = rawDiffs.map((diff) => {
      const { additions, deletions } = countChanges(diff.diff ?? '')
      return {
        path: diff.new_path || diff.old_path,
        oldPath: diff.old_path || diff.new_path,
        status: diff.new_file ? 'added' : diff.deleted_file ? 'removed' : diff.renamed_file ? 'renamed' : 'modified',
        additions,
        deletions,
        // GitLab omits the `@@` header on empty diffs and on binaries.
        patch: diff.diff ? diff.diff : null,
        binary: !diff.diff,
      }
    })

    const version = versions[0]
    const threads = gitlabThreads(discussions, version?.head_commit_sha ?? mr.sha)

    const totals = files.reduce(
      (acc, file) => ({ a: acc.a + file.additions, d: acc.d + file.deletions }),
      { a: 0, d: 0 },
    )

    return {
      item: { ...item, additions: totals.a, deletions: totals.d, changedFiles: files.length },
      description: mr.description ?? '',
      files,
      threads,
      refs: version
        ? { baseSha: version.base_commit_sha, startSha: version.start_commit_sha, headSha: version.head_commit_sha }
        : { headSha: mr.sha },
    }
  },

  async refreshChecks(session, item, signal) {
    return loadChecks(session, item.repoKey, item.number, signal)
  },

  async submitReview(session, item, verdict, body, comments) {
    // No call here takes a review and its comments together, and no server-side
    // draft mechanism is used, so the comments go one at a time and the verdict
    // last - reported honestly if it stops part-way.
    await submitSequentially(
      comments,
      (comment) => postDiscussion(session, item, comment),
      async () => {
        // GitLab has no single "submit review" call: approval and the note are separate.
        if (verdict === 'approve') {
          await request(
            api(session, `/projects/${project(item)}/merge_requests/${item.number}/approve`),
            { method: 'POST', headers: headers(session) },
          )
          if (body) await this.addComment(session, item, body)
          return
        }
        if (verdict === 'request_changes') {
          // Closest equivalent: drop any approval and say why.
          await request(
            api(session, `/projects/${project(item)}/merge_requests/${item.number}/unapprove`),
            { method: 'POST', headers: headers(session) },
          ).catch(() => undefined)
          await this.addComment(session, item, body || 'Changes requested.')
          return
        }
        if (body) await this.addComment(session, item, body)
      },
    )
  },

  async addComment(session, item, body) {
    await request(api(session, `/projects/${project(item)}/merge_requests/${item.number}/notes`), {
      method: 'POST',
      headers: headers(session),
      body: { body },
    })
  },

  async replyToThread(session, item, threadId, body) {
    await request(
      api(
        session,
        `/projects/${project(item)}/merge_requests/${item.number}/discussions/${encodeURIComponent(threadId)}/notes`,
      ),
      { method: 'POST', headers: headers(session), body: { body } },
    )
  },

  async setThreadResolved(session, item, threadId, resolved) {
    await request(
      api(
        session,
        `/projects/${project(item)}/merge_requests/${item.number}/discussions/${encodeURIComponent(threadId)}`,
      ),
      { method: 'PUT', headers: headers(session), body: { resolved } },
    )
  },

  async addLineComment(session, item, draft, refs) {
    await postDiscussion(session, item, {
      id: '',
      itemId: item.id,
      createdAt: '',
      body: draft.body,
      path: draft.path,
      newLine: draft.newLine,
      oldLine: draft.oldLine,
      refs,
    })
  },
}

/** One comment, placed by the three shas the draft recorded. */
async function postDiscussion(
  session: Session,
  item: ReviewItem,
  comment: DraftComment,
): Promise<void> {
  await request(api(session, `/projects/${project(item)}/merge_requests/${item.number}/discussions`), {
    method: 'POST',
    headers: headers(session),
    form: true,
    body: gitlabDiscussionPayload(comment),
  })
}

function emptyChecks(): CheckSummary {
  return { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] }
}
