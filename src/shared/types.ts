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
}

export interface AccountDraft {
  kind: ProviderKind
  label: string
  /** Host as typed by the user, e.g. "gitlab.example.com" or a full URL. */
  host: string
  token: string
  /** Bitbucket app passwords are tied to a username. */
  username?: string
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
  /** Present for inline comments. */
  path?: string
  line?: number
  side?: 'old' | 'new'
}

export interface PullDetail {
  item: ReviewItem
  description: string
  files: DiffFile[]
  comments: PullComment[]
  refs: DiffRefs
}

export type ReviewVerdict = 'approve' | 'request_changes' | 'comment'

export interface ReviewSubmission {
  itemId: string
  verdict: ReviewVerdict
  body: string
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
  launchAtLogin: boolean
}

export const DEFAULT_SETTINGS: Settings = {
  pollInterval: 180,
  checkPollInterval: 45,
  notificationsEnabled: true,
  playSound: true,
  diffView: 'split',
  theme: 'system',
  hideApproved: false,
  launchAtLogin: false,
}

/**
 * Stored settings laid over the defaults, so a vault written by an older build
 * still loads: a setting added since is filled in from the defaults, and one
 * removed since is carried along harmlessly rather than throwing.
 */
export function mergeSettings(stored: Partial<Settings> | null | undefined): Settings {
  return { ...DEFAULT_SETTINGS, ...(stored ?? {}) }
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
  lastSyncedAt?: string
}
