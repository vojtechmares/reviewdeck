import type {
  Account,
  AccountDraft,
  CheckStatus,
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
  submitReview(session: Session, item: ReviewItem, verdict: ReviewVerdict, body: string): Promise<void>
  addComment(session: Session, item: ReviewItem, body: string): Promise<void>
  addLineComment(session: Session, item: ReviewItem, draft: LineCommentDraft, refs: PullDetail['refs']): Promise<void>
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
