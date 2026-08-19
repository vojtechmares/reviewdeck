/**
 * The sync engine: fans out across every account, keeps the aggregated deck in
 * memory, raises notifications for genuinely new review requests, and runs a
 * faster secondary poll while any CI check is still running.
 */

import { BrowserWindow, Notification, shell } from 'electron'
import { EventEmitter } from 'node:events'
import { drafts } from './draft-store.ts'
import { providerFor } from './providers/index.ts'
import { limitConcurrency } from './providers/types.ts'
import { DEMO_ACCOUNTS, DEMO_ITEMS, demoEnabled } from './demo.ts'
import {
  getSettings,
  getToken,
  listAccounts,
  loadDeckCache,
  markSeen,
  persistDeckCache,
} from './store.ts'
import type { Account, AccountStatus, DeckState, ReviewItem } from '@shared/types.ts'
import type { Session } from './providers/types.ts'

class Deck extends EventEmitter {
  /** Items keyed by account so one failing host does not wipe the others. */
  private byAccount = new Map<string, ReviewItem[]>()
  private statuses = new Map<string, AccountStatus>()
  private syncing = false
  /** Signature of the accounts the last completed sync covered, or undefined. */
  private syncedAccounts: string | undefined
  private lastSyncedAt: string | undefined
  private syncTimer: NodeJS.Timeout | null = null
  private checkTimer: NodeJS.Timeout | null = null
  /** Suppresses notifications on the very first sync after launch. */
  private primed = false

  state(): DeckState {
    return {
      items: this.items(),
      statuses: [...this.statuses.values()],
      syncing: this.syncing,
      synced: this.synced(),
      lastSyncedAt: this.lastSyncedAt,
    }
  }

  /**
   * Whether the deck has looked at the accounts that are connected right now.
   *
   * Connecting or dropping one invalidates the last sync: what it found was about a
   * different set of accounts, so it says nothing about whether this one is quiet.
   */
  private synced(): boolean {
    if (this.syncedAccounts === undefined) return false
    return this.syncedAccounts === signatureOf(connectedAccounts())
  }

  items(): ReviewItem[] {
    const all: ReviewItem[] = []
    for (const items of this.byAccount.values()) all.push(...items)
    // Most recently touched first - that is what you want to look at.
    return all.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
  }

  find(itemId: string): ReviewItem | undefined {
    return this.items().find((item) => item.id === itemId)
  }

  session(accountId: string): Session {
    const account = listAccounts().find((entry) => entry.id === accountId)
    if (!account) throw new Error('That account is no longer signed in.')
    return { account, token: getToken(accountId) }
  }

  /** Replace a single item in place, e.g. after approving it. */
  patch(itemId: string, patch: Partial<ReviewItem>): void {
    for (const [accountId, items] of this.byAccount) {
      const index = items.findIndex((item) => item.id === itemId)
      if (index >= 0) {
        const next = [...items]
        next[index] = { ...next[index], ...patch }
        this.byAccount.set(accountId, next)
        this.publish()
        return
      }
    }
  }

  drop(itemId: string): void {
    for (const [accountId, items] of this.byAccount) {
      if (items.some((item) => item.id === itemId)) {
        this.byAccount.set(
          accountId,
          items.filter((item) => item.id !== itemId),
        )
        this.publish()
        return
      }
    }
  }

  async refresh(): Promise<DeckState> {
    if (this.syncing) return this.state()

    if (demoEnabled()) {
      this.byAccount.set('demo', DEMO_ITEMS)
      this.syncedAccounts = signatureOf(DEMO_ACCOUNTS)
      this.lastSyncedAt = new Date().toISOString()
      this.publish()
      return this.state()
    }

    const accounts = listAccounts()

    // Forget accounts that were removed while we were idle.
    for (const key of [...this.byAccount.keys()]) {
      if (!accounts.some((account) => account.id === key)) {
        this.byAccount.delete(key)
        this.statuses.delete(key)
      }
    }

    this.syncing = true
    this.publish()

    // Captured before the fetch: if the app submits while this is in flight, the
    // states coming back predate it and must not be read as somebody else's review.
    const submissionsAtStart = drafts.submissions()

    await Promise.all(accounts.map((account) => this.refreshAccount(account)))

    this.reconcileDrafts(submissionsAtStart)
    this.syncing = false
    // Of the accounts we just fanned out over, not of whatever is connected now -
    // one added mid-flight was never asked, so the deck cannot claim to know it.
    this.syncedAccounts = signatureOf(accounts)
    this.lastSyncedAt = new Date().toISOString()
    persistDeckCache(Object.fromEntries(this.byAccount))
    this.announceNew()
    this.publish()
    this.scheduleCheckPoll()
    return this.state()
  }

  /**
   * Notices a review the reviewer submitted somewhere else.
   *
   * Only items with drafts are worth looking at - with nothing pending there is
   * nothing to reconcile, and the card already shows the new state.
   */
  private reconcileDrafts(submissionsAtStart: number): void {
    for (const item of this.items()) {
      if (!drafts.count(item.id)) continue
      drafts.reconcile(item.id, item.myReviewState, submissionsAtStart)
    }
  }

  private async refreshAccount(account: Account): Promise<void> {
    try {
      const provider = providerFor(account.kind)
      const session: Session = { account, token: getToken(account.id) }
      const items = await provider.listReviewRequests(session)
      this.byAccount.set(account.id, items)
      this.statuses.set(account.id, {
        accountId: account.id,
        ok: true,
        lastSyncedAt: new Date().toISOString(),
        count: items.length,
      })
    } catch (error) {
      // Keep whatever we had; a transient outage should not empty the deck.
      this.statuses.set(account.id, {
        accountId: account.id,
        ok: false,
        error: (error as Error).message,
        lastSyncedAt: this.statuses.get(account.id)?.lastSyncedAt,
        count: this.byAccount.get(account.id)?.length ?? 0,
      })
    }
  }

  /** Re-read CI status for items whose checks were still running. */
  private async refreshRunningChecks(): Promise<void> {
    const running = this.items().filter((item) => item.checks.status === 'running')
    if (!running.length) return

    // Serial polling would take minutes on a busy deck; four at a time is plenty.
    await limitConcurrency(running, 4, async (item) => {
      try {
        const provider = providerFor(item.provider)
        const checks = await provider.refreshChecks(this.session(item.accountId), item)
        this.patch(item.id, { checks })
      } catch {
        /* leave the previous status in place */
      }
    })
    this.scheduleCheckPoll()
  }

  private scheduleCheckPoll(): void {
    if (this.checkTimer) clearTimeout(this.checkTimer)
    const anyRunning = this.items().some((item) => item.checks.status === 'running')
    if (!anyRunning) return
    const { checkPollInterval } = getSettings()
    this.checkTimer = setTimeout(() => {
      void this.refreshRunningChecks()
    }, Math.max(15, checkPollInterval) * 1000)
  }

  private announceNew(): void {
    const ids = this.items().map((item) => item.id)
    const fresh = markSeen(ids)

    // On the first run after install every item is "new"; do not blast the user.
    if (!this.primed) {
      this.primed = true
      return
    }
    const settings = getSettings()
    if (!settings.notificationsEnabled || !fresh.length) return
    if (!Notification.isSupported()) return

    const items = this.items().filter((item) => fresh.includes(item.id))
    if (items.length === 1) {
      const [item] = items
      const notification = new Notification({
        title: 'Review requested',
        subtitle: `${item.repo} #${item.number}`,
        body: `${item.title} - by ${item.author.name}`,
        silent: !settings.playSound,
      })
      notification.on('click', () => {
        this.emit('focus-item', item.id)
        focusWindow()
      })
      notification.show()
    } else if (items.length > 1) {
      const notification = new Notification({
        title: `${items.length} new review requests`,
        body: items
          .slice(0, 4)
          .map((item) => `${item.repo} #${item.number}`)
          .join('\n'),
        silent: !settings.playSound,
      })
      notification.on('click', focusWindow)
      notification.show()
    }
  }

  /**
   * Fills the deck from the vault before the first fan-out, so the window and the
   * menu bar open on the reviews the last sync found instead of on nothing.
   *
   * Deliberately does not count as a sync: these reviews are as old as the quit
   * that wrote them, so the deck still has nothing to say about whether the
   * accounts are quiet until it has actually asked them.
   */
  hydrate(): void {
    if (demoEnabled()) return
    for (const [accountId, items] of Object.entries(loadDeckCache())) {
      this.byAccount.set(accountId, items)
    }
    this.publish()
  }

  start(): void {
    this.stop()
    const { pollInterval } = getSettings()
    void this.refresh()
    this.syncTimer = setInterval(
      () => {
        void this.refresh()
      },
      Math.max(30, pollInterval) * 1000,
    )
  }

  stop(): void {
    if (this.syncTimer) clearInterval(this.syncTimer)
    if (this.checkTimer) clearTimeout(this.checkTimer)
    this.syncTimer = null
    this.checkTimer = null
  }

  /** Apply a settings change that affects timing without losing the current deck. */
  reschedule(): void {
    if (this.syncTimer) {
      clearInterval(this.syncTimer)
      const { pollInterval } = getSettings()
      this.syncTimer = setInterval(
        () => {
          void this.refresh()
        },
        Math.max(30, pollInterval) * 1000,
      )
    }
    this.scheduleCheckPoll()
  }

  publish(): void {
    const state = this.state()
    for (const window of BrowserWindow.getAllWindows()) {
      window.webContents.send('deck:changed', state)
    }
    this.emit('changed', state)
  }
}

/** The accounts the app is showing, which in demo mode are the fixtures. */
function connectedAccounts(): Account[] {
  return demoEnabled() ? DEMO_ACCOUNTS : listAccounts()
}

/** A stable identity for a set of accounts, so a change to it can be noticed. */
function signatureOf(entries: Account[]): string {
  return entries
    .map((account) => account.id)
    .sort()
    .join(' ')
}

function focusWindow(): void {
  const [window] = BrowserWindow.getAllWindows()
  if (window) {
    if (window.isMinimized()) window.restore()
    window.show()
    window.focus()
  }
}

export function openExternal(url: string): void {
  // Only ever hand http(s) to the OS; a provider-supplied url is untrusted input.
  try {
    const parsed = new URL(url)
    if (parsed.protocol === 'http:' || parsed.protocol === 'https:') void shell.openExternal(url)
  } catch {
    /* ignore malformed urls */
  }
}

export const deck = new Deck()
