/** Types shared between the main process, the preload bridge and the renderer. */

export type ProviderKind = 'github' | 'gitlab' | 'forgejo' | 'bitbucket'

export const PROVIDER_LABELS: Record<ProviderKind, string> = {
  github: 'GitHub',
  gitlab: 'GitLab',
  forgejo: 'Forgejo / Gitea',
  bitbucket: 'Bitbucket',
}

/** A signed-in account. The token itself never crosses the IPC bridge. */
export interface Account {
  id: string
  kind: ProviderKind
  /** User-facing name, e.g. "Work GitHub". */
  label: string
  /** API root without a trailing slash, e.g. https://api.github.com */
  baseUrl: string
  /** Web root used to build browser links, e.g. https://github.com */
  webUrl: string
  username: string
  displayName: string
  avatarUrl: string
  addedAt: string
  /**
   * Overrides the agent command for handoffs from this account, so a client's Git
   * account can route through that client's Claude configuration. Blank means the
   * setting applies.
   */
  agentCommand?: string
}

export interface AccountDraft {
  kind: ProviderKind
  label: string
  /** Host as typed by the user, e.g. "gitlab.example.com" or a full URL. */
  host: string
  token: string
  /** Bitbucket app passwords are tied to a username. */
  username?: string
  /** Per-account agent command; blank clears the override. */
  agentCommand?: string
}

export type CheckStatus = 'passed' | 'failed' | 'running' | 'unknown'

export interface CheckRun {
  id: string
  name: string
  status: CheckStatus
  url?: string
  description?: string
}

export interface CheckSummary {
  status: CheckStatus
  passed: number
  failed: number
  running: number
  total: number
  runs: CheckRun[]
}

/** How the signed-in user has reviewed a PR so far. */
export type MyReviewState = 'pending' | 'approved' | 'changes_requested' | 'commented'

export interface User {
  name: string
  avatarUrl: string
}

export interface ReviewItem {
  /** Stable across refreshes: `${accountId}:${repoKey}:${number}`. */
  id: string
  accountId: string
  provider: ProviderKind
  /** Provider-native handle for follow-up API calls (owner/repo, project id, ws/slug). */
  repoKey: string
  /** Human readable repository name. */
  repo: string
  number: number
  title: string
  url: string
  author: User
  createdAt: string
  updatedAt: string
  draft: boolean
  sourceBranch: string
  targetBranch: string
  labels: string[]
  myReviewState: MyReviewState
  checks: CheckSummary
  /** Set when the provider reports it cheaply; otherwise filled in on detail load. */
  additions?: number
  deletions?: number
  changedFiles?: number
}

export interface DiffFile {
  path: string
  oldPath: string
  status: 'added' | 'removed' | 'modified' | 'renamed'
  additions: number
  deletions: number
  /** Raw unified diff body for this file; null when the provider omits it (binary, too large). */
  patch: string | null
  binary: boolean
}

/** Provider-specific handles a line comment needs (GitLab wants diff refs, GitHub a commit sha). */
export interface DiffRefs {
  baseSha?: string
  startSha?: string
  headSha?: string
}

export interface PullComment {
  id: string
  author: User
  body: string
  createdAt: string
}

/**
 * One conversation: an opening comment and the replies under it, or a lone note on
 * a host that gives comments no thread structure at all.
 *
 * The two capability flags are per thread, not per provider - GitHub can reply to a
 * review thread and cannot reply to an ordinary issue comment - and they are the
 * only thing the renderer goes by. It never branches on which host it is talking to;
 * every difference is absorbed in the adapter.
 */
export interface CommentThread {
  id: string
  /** Oldest first. The first is the comment a reply would land under. */
  comments: PullComment[]
  resolved: boolean
  /** The thread is anchored to a version of the diff that has since moved on. */
  outdated: boolean
  /** Present for inline threads. */
  path?: string
  line?: number
  side?: 'old' | 'new'
  canReply: boolean
  canResolve: boolean
}

export interface PullDetail {
  item: ReviewItem
  description: string
  files: DiffFile[]
  threads: CommentThread[]
  refs: DiffRefs
}

export type ReviewVerdict = 'approve' | 'request_changes' | 'comment'

export interface ReviewSubmission {
  itemId: string
  verdict: ReviewVerdict
  body: string
}

/**
 * A line comment written but not yet sent.
 *
 * Drafts are owned by the main process and carry the diff references they were
 * written against, so they can be submitted against the code that was actually
 * read rather than against whatever the branch looks like by then.
 */
export interface DraftComment {
  id: string
  itemId: string
  body: string
  path: string
  /** Line in the file after the change, when the comment is on an added or context line. */
  newLine?: number
  /** Line in the file before the change, when the comment is on a removed line. */
  oldLine?: number
  createdAt: string
  refs: DiffRefs
}

export interface LineCommentDraft {
  itemId: string
  body: string
  path: string
  /** Line number in the file *after* the change, when commenting on an added/context line. */
  newLine?: number
  /** Line number in the file *before* the change, when commenting on a removed line. */
  oldLine?: number
}

/**
 * A recurring stretch of the day the app is allowed to interrupt in.
 *
 * The list of them is the whole feature: empty means the app notifies whenever a
 * poll finds something, exactly as it always has, and adding one is what buys the
 * quiet. The rules that read this live in `review-window.ts`.
 */
export interface ReviewWindow {
  id: string
  enabled: boolean
  /** The days it covers, numbered as `Date#getDay` does - 0 is Sunday. */
  days: number[]
  /** Local wall clock, `HH:MM`. The span is [start, end) and may not cross midnight. */
  start: string
  end: string
  /** Stays quiet until at least this many reviews are waiting in its own scope. */
  minimum: number
  /**
   * The accounts it covers. Empty means every account, now and in future - which
   * is why an account connected next March is picked up by an existing
   * all-accounts window rather than quietly going dark.
   */
  accounts: string[]
}

export interface Settings {
  /** Seconds between background refreshes. */
  pollInterval: number
  /** Seconds between check-status polls while any check is running. */
  checkPollInterval: number
  notificationsEnabled: boolean
  playSound: boolean
  diffView: 'split' | 'unified'
  theme: 'system' | 'light' | 'dark'
  /** Hide PRs the user has already approved. */
  hideApproved: boolean
  /** Hide PRs still being written, by the host's flag or by the title convention. */
  hideDrafts: boolean
  /** Draw the waiting count beside the menu bar icon. Off leaves the icon alone. */
  showMenuBarCount: boolean
  launchAtLogin: boolean
  /** When the app may interrupt. Empty means whenever it finds something. */
  reviewWindows: ReviewWindow[]
  /**
   * The command a copied agent handoff is built around. Any command or shell alias
   * will do - it is the user's own shell that runs it, not the app.
   */
  agentCommand: string
}

/** Plain Claude Code, unless settings or an account say otherwise. */
export const DEFAULT_AGENT_COMMAND = 'claude'

export const DEFAULT_SETTINGS: Settings = {
  pollInterval: 180,
  checkPollInterval: 45,
  notificationsEnabled: true,
  playSound: true,
  diffView: 'split',
  theme: 'system',
  hideApproved: false,
  hideDrafts: true,
  showMenuBarCount: true,
  launchAtLogin: false,
  reviewWindows: [],
  agentCommand: DEFAULT_AGENT_COMMAND,
}

/**
 * Stored settings laid over the defaults, so a vault written by an older build
 * still loads: a setting added since is filled in from the defaults, and one
 * removed since is carried along harmlessly rather than throwing.
 */
export function mergeSettings(stored: Partial<Settings> | null | undefined): Settings {
  const merged = { ...DEFAULT_SETTINGS, ...(stored ?? {}) }
  return {
    ...merged,
    // A window written before one could name accounts covers all of them, which is
    // exactly what an empty list means - so the migration is the default itself.
    reviewWindows: (merged.reviewWindows ?? []).map((window) => ({
      ...window,
      accounts: window.accounts ?? [],
    })),
  }
}

/**
 * Whether a review is one the user still wants in front of them.
 *
 * Shared rather than kept with the renderer's other filters, because the menu bar
 * counts the same deck the window lists, and the two disagreeing is something the
 * user sees. Only standing preferences belong here - the query box, the account
 * picker and the check picker stay in the renderer, because those are a view
 * somebody is driving by hand rather than a rule the whole app should honour.
 */
export function isVisibleReview(item: ReviewItem, settings: Settings): boolean {
  if (settings.hideApproved && item.myReviewState === 'approved') return false
  if (settings.hideDrafts && isDraftReview(item)) return false
  return true
}

/** A title that says draft before it says anything else. */
const DRAFT_TITLE = /^\s*(?:draft:|wip:|\[draft\]|\[wip\])/i

/**
 * Whether a pull request is still being written rather than waiting on a review.
 *
 * Two signals, because no single one covers every host. GitHub has a real draft
 * flag. GitLab derives its flag from a `Draft:` title prefix and Forgejo derives
 * its own from `WIP:`, so on those the flag already carries the convention.
 * Bitbucket Cloud has no draft concept at all, which leaves its flag always false
 * and the title the only thing there is to go on. The title rule also catches
 * somebody typing the habit on a host that would not have set the flag for them.
 *
 * It has to be a prefix: a pull request about the draft feature is not a draft.
 * One false positive is accepted knowingly - a pull request genuinely called
 * "Draft: release notes for 2.0" is hidden with the rest - because the deck says
 * how many it is holding back, which is what makes that recoverable.
 */
export function isDraftReview(item: ReviewItem): boolean {
  return item.draft || DRAFT_TITLE.test(item.title)
}

/** The reviews a deck actually shows, which is the number the menu bar carries. */
export function visibleReviews(items: ReviewItem[], settings: Settings): ReviewItem[] {
  return items.filter((item) => isVisibleReview(item, settings))
}

/**
 * The reviews a completed sync should raise a notification about: the ones that
 * arrived since the last sync and that the deck is going to show.
 *
 * The third reading of the same rule, alongside the list and the menu bar count,
 * and deliberately not a fourth rule of its own. A banner for a review a standing
 * preference hides opens onto a deck that does not contain it - the app arguing
 * with itself in front of the user.
 */
export function reviewsToAnnounce(
  items: ReviewItem[],
  fresh: string[],
  settings: Settings,
): ReviewItem[] {
  const arrived = new Set(fresh)
  return visibleReviews(items, settings).filter((item) => arrived.has(item.id))
}

/**
 * The reviews a completed sync records as having been seen: every one it found,
 * including the ones no notification will mention.
 *
 * Not the same set as what gets announced, and the difference is the point. A
 * review hidden by a standing preference is one the user has decided they never
 * want, so it is seen the moment it arrives. Recording only what was announced
 * would leave months of hidden reviews waiting to fire the day the preference is
 * turned off.
 */
export function idsToRecord(items: ReviewItem[]): string[] {
  return items.map((item) => item.id)
}

/** Per-account outcome of a refresh, so the UI can show which host is unhappy. */
export interface AccountStatus {
  accountId: string
  ok: boolean
  error?: string
  lastSyncedAt?: string
  count: number
}

export interface DeckState {
  items: ReviewItem[]
  statuses: AccountStatus[]
  syncing: boolean
  /**
   * Whether a sync has completed for the accounts currently connected. Distinct
   * from `lastSyncedAt`, which survives an account being added or removed: a deck
   * that has never looked at the current set of accounts has nothing to say about
   * whether they are quiet.
   */
  synced: boolean
  lastSyncedAt?: string
}

/** What the deck pane says in place of review cards. */
export type DeckEmptyState =
  | 'no-accounts'
  | 'syncing'
  | 'no-matches'
  | 'inbox-zero'
  | 'only-drafts'

/**
 * Why the deck pane is showing nothing, or null when it has cards to show.
 *
 * Emptiness is not `items.length` - before the first sync lands the deck is empty
 * because it has not looked yet, which is a very different thing to tell the user
 * than that nobody is waiting on them.
 */
export function deckEmptyState(deck: {
  accountCount: number
  synced: boolean
  visibleCount: number
  /** Drafts the preference is holding back, already narrowed to the current view. */
  hiddenDraftCount: number
  filtersActive: boolean
}): DeckEmptyState | null {
  // Nobody with no accounts is waiting on a sync, so this one never waits.
  if (!deck.accountCount) return 'no-accounts'
  if (deck.visibleCount) return null
  if (!deck.synced) return 'syncing'
  // Ahead of both of the below, because a deck holding nothing but drafts is
  // neither at inbox zero nor short of anything matching - it is holding drafts,
  // and saying so is the only version the user can act on.
  if (deck.hiddenDraftCount) return 'only-drafts'
  return deck.filtersActive ? 'no-matches' : 'inbox-zero'
}
