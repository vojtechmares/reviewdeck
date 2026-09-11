import type {
  Account,
  AccountDraft,
  ApprovalSummary,
  CheckStatus,
  CommentThread,
  DraftComment,
  LineCommentDraft,
  PullDetail,
  ReviewItem,
  ReviewVerdict,
} from '@shared/types.ts'

export interface Session {
  account: Account
  token: string
}

/** Everything a provider must do for Reviewdeck to be useful. */
export interface Provider {
  kind: Account['kind']
  /** Verify the token and resolve the identity behind it. */
  connect(draft: AccountDraft): Promise<Omit<Account, 'id' | 'addedAt'>>
  /** Open PRs/MRs awaiting this user's review. */
  listReviewRequests(session: Session, signal?: AbortSignal): Promise<ReviewItem[]>
  /** Diff, description and existing comments for one item. */
  loadDetail(session: Session, item: ReviewItem, signal?: AbortSignal): Promise<PullDetail>
  /** Re-read just the CI status, for the running-checks poll. */
  refreshChecks(session: Session, item: ReviewItem, signal?: AbortSignal): Promise<ReviewItem['checks']>
  /**
   * Re-read just the conversation, for the poll behind an open pull request.
   *
   * The diff is the expensive half of `loadDetail` - a merge request in a monorepo
   * is megabytes of it - and none of it moves when someone answers a thread. So a
   * reply arriving while the diff is being read costs the comments and nothing else.
   */
  loadThreads(session: Session, item: ReviewItem, signal?: AbortSignal): Promise<CommentThread[]>
  /**
   * Submit a review. `comments` are the drafts written against this pull request,
   * which each adapter maps onto its host's batch call where one exists and onto
   * sequential posts where it does not.
   */
  submitReview(
    session: Session,
    item: ReviewItem,
    verdict: ReviewVerdict,
    body: string,
    comments: DraftComment[],
  ): Promise<void>
  addComment(session: Session, item: ReviewItem, body: string): Promise<void>
  addLineComment(session: Session, item: ReviewItem, draft: LineCommentDraft, refs: PullDetail['refs']): Promise<void>
  /**
   * Reply into an existing thread. Absent on a host that cannot do it - support is
   * expressed through each thread's `canReply` flag, not by throwing.
   */
  replyToThread?(session: Session, item: ReviewItem, threadId: string, body: string): Promise<void>
  /** Resolve or reopen a thread. Absent on a host that cannot do it, as above. */
  setThreadResolved?(
    session: Session,
    item: ReviewItem,
    threadId: string,
    resolved: boolean,
  ): Promise<void>
}

/** Collapse a set of individual check states into the one badge the deck shows. */
export function rollUp(states: CheckStatus[]): CheckStatus {
  if (!states.length) return 'unknown'
  if (states.includes('failed')) return 'failed'
  if (states.includes('running')) return 'running'
  if (states.every((state) => state === 'unknown')) return 'unknown'
  return 'passed'
}

export function summariseChecks(
  runs: { status: CheckStatus }[],
): { status: CheckStatus; passed: number; failed: number; running: number; total: number } {
  const passed = runs.filter((run) => run.status === 'passed').length
  const failed = runs.filter((run) => run.status === 'failed').length
  const running = runs.filter((run) => run.status === 'running').length
  return {
    status: rollUp(runs.map((run) => run.status)),
    passed,
    failed,
    running,
    total: runs.length,
  }
}

/**
 * Fold an approval count against what the host asks for.
 *
 * For hosts that only publish a plain number: a rule that names who has to
 * approve (GitLab's rules, GitHub's code owners) is settled by the host itself,
 * and those adapters build the summary from its verdict rather than from here.
 * `required` unknown or zero both read as nothing required, which is also what
 * the card shows when the token was not allowed to ask.
 */
export function summariseApprovals(given: number, required?: number): ApprovalSummary {
  if (!required) return { given, required, outcome: 'none_required' }
  return { given, required, outcome: given >= required ? 'satisfied' : 'pending' }
}

/**
 * Who stands approving once every reviewer's latest verdict is taken. A review
 * that only comments does not move a verdict; a dismissed one withdraws it.
 */
export function countApprovers(
  reviews: { login: string | undefined; state: string }[],
): number {
  const standing = new Map<string, string>()
  for (const review of reviews) {
    if (!review.login) continue
    const state = review.state.toUpperCase()
    if (state === 'COMMENTED' || state === 'COMMENT' || state === 'PENDING') continue
    standing.set(review.login, state)
  }
  let given = 0
  for (const state of standing.values()) if (state === 'APPROVED') given++
  return given
}

/**
 * Whether a branch restriction's pattern covers a branch. Bitbucket's patterns
 * are globs where `*` stands for any run of characters, and one per pattern is
 * what the app needs - `**` and character classes are read literally.
 */
export function restrictionCovers(pattern: string, branch: string): boolean {
  const escaped = pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')
  return new RegExp(`^${escaped}$`).test(branch)
}

export function makeItemId(accountId: string, repoKey: string, number: number): string {
  return `${accountId}:${repoKey}:${number}`
}

/** Run `worker` over `items` with at most `limit` in flight, preserving input order. */
export function limitConcurrency<T, R>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<R>,
): Promise<R[]> {
  const results: R[] = new Array(items.length)
  let cursor = 0

  async function run(): Promise<void> {
    while (cursor < items.length) {
      const index = cursor++
      results[index] = await worker(items[index])
    }
  }

  return Promise.all(Array.from({ length: Math.min(limit, items.length) }, run)).then(() => results)
}
