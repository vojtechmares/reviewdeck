import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Filter,
  Inbox,
  Loader2,
  RefreshCw,
  Search,
  Settings2,
  TriangleAlert,
  UserRoundPlus,
  X,
} from 'lucide-react'
import {
  deckEmptyState,
  isVisibleReview,
  type CheckStatus,
  type ReviewItem,
  type Settings,
} from '@shared/types'
import { cn, relativeTime } from '@/lib/utils'
import { useApp } from '@/hooks/useApp'
import { AccountsDialog } from './components/AccountsDialog'
import { SettingsDialog } from './components/SettingsDialog'
import { PullView } from './components/PullView'
import { ReviewCard } from './components/ReviewCard'
import { Button } from './components/ui/button'
import { Input } from './components/ui/input'
import { Tooltip } from './components/ui/tooltip'

type Filters = { query: string; account: string; checks: CheckStatus | 'all' }

const NO_FILTERS: Filters = { query: '', account: 'all', checks: 'all' }

export function App(): React.JSX.Element {
  const { accounts, deck, settings, ready, refresh, accountFor } = useApp()

  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [filters, setFilters] = useState<Filters>(NO_FILTERS)
  const [showFilters, setShowFilters] = useState(false)
  const [accountsOpen, setAccountsOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)

  const items = useMemo(
    () => applyFilters(deck.items, filters, settings),
    [deck.items, filters, settings],
  )

  // Keep a valid selection as the deck changes underneath us.
  useEffect(() => {
    if (!items.length) {
      setSelectedId(null)
      return
    }
    if (!selectedId || !items.some((item) => item.id === selectedId)) {
      setSelectedId(items[0].id)
    }
  }, [items, selectedId])

  // Clicking a notification (or the Settings menu item) routes through here.
  useEffect(() => {
    return window.reviewdeck.deck.onFocusItem((itemId) => {
      if (itemId === '__settings__') setSettingsOpen(true)
      else setSelectedId(itemId)
    })
  }, [])

  const move = useCallback(
    (delta: number) => {
      if (!items.length) return
      const index = items.findIndex((item) => item.id === selectedId)
      const next = Math.min(items.length - 1, Math.max(0, (index < 0 ? 0 : index) + delta))
      setSelectedId(items[next].id)
    },
    [items, selectedId],
  )

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      const target = event.target as HTMLElement
      const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)

      if ((event.metaKey || event.ctrlKey) && event.key === 'r') {
        event.preventDefault()
        void refresh()
        return
      }
      if ((event.metaKey || event.ctrlKey) && event.key === 'f') {
        event.preventDefault()
        setShowFilters(true)
        // Wait for the field to mount before focusing it.
        requestAnimationFrame(() => document.getElementById('deck-search')?.focus())
        return
      }
      if (typing) return
      if (event.key === 'j' || event.key === 'ArrowDown') {
        event.preventDefault()
        move(1)
      } else if (event.key === 'k' || event.key === 'ArrowUp') {
        event.preventDefault()
        move(-1)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [move, refresh])

  const selected = items.find((item) => item.id === selectedId) ?? null
  const failing = deck.statuses.filter((status) => !status.ok)
  const filtersActive = filters.query !== '' || filters.account !== 'all' || filters.checks !== 'all'
  const empty = deckEmptyState({
    accountCount: accounts.length,
    synced: deck.synced,
    visibleCount: items.length,
    filtersActive,
  })

  return (
    <div className="flex h-full flex-col">
      <header className="drag flex h-13 shrink-0 items-center gap-2 border-b border-border pr-3 pl-22">
        <div className="flex min-w-0 items-baseline gap-2">
          <h1 className="text-[13.5px] font-semibold tracking-tight">Reviewdeck</h1>
          <span className="truncate text-[11.5px] text-muted-foreground">
            {deck.syncing
              ? 'Syncing…'
              : deck.lastSyncedAt
                ? `Updated ${relativeTime(deck.lastSyncedAt)}`
                : 'Not synced yet'}
          </span>
        </div>

        <div className="ml-auto flex items-center gap-1">
          {failing.length > 0 && (
            <Tooltip
              label={failing
                .map((status) => `${accountFor(status.accountId)?.label ?? 'Account'}: ${status.error}`)
                .join('\n\n')}
            >
              <Button variant="ghost" size="icon" onClick={() => setAccountsOpen(true)} aria-label="Sync problems">
                <TriangleAlert className="size-4 text-bad" />
              </Button>
            </Tooltip>
          )}

          <Tooltip label="Filter  ⌘F">
            <Button
              variant={showFilters || filtersActive ? 'subtle' : 'ghost'}
              size="icon"
              aria-label="Filter"
              onClick={() => setShowFilters((value) => !value)}
            >
              <Filter className="size-4" />
            </Button>
          </Tooltip>

          <Tooltip label="Refresh  ⌘R">
            <Button
              variant="ghost"
              size="icon"
              aria-label="Refresh"
              disabled={deck.syncing}
              onClick={() => void refresh()}
            >
              <RefreshCw className={cn('size-4', deck.syncing && 'spin')} />
            </Button>
          </Tooltip>

          <Tooltip label="Accounts">
            <Button variant="ghost" size="icon" aria-label="Accounts" onClick={() => setAccountsOpen(true)}>
              <UserRoundPlus className="size-4" />
            </Button>
          </Tooltip>

          <Tooltip label="Settings  ⌘,">
            <Button variant="ghost" size="icon" aria-label="Settings" onClick={() => setSettingsOpen(true)}>
              <Settings2 className="size-4" />
            </Button>
          </Tooltip>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="glass-quiet flex w-96 shrink-0 flex-col border-r border-border">
          {showFilters && (
            <div className="rise flex flex-col gap-2 border-b border-border p-2.5">
              <div className="relative">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  id="deck-search"
                  value={filters.query}
                  placeholder="Filter by title, repo or author…"
                  className="h-8 pl-7.5"
                  onChange={(event) => setFilters((current) => ({ ...current, query: event.target.value }))}
                  onKeyDown={(event) => {
                    if (event.key === 'Escape') setShowFilters(false)
                  }}
                />
              </div>
              <div className="flex gap-1.5">
                <select
                  value={filters.account}
                  onChange={(event) => setFilters((current) => ({ ...current, account: event.target.value }))}
                  className="h-7 flex-1 rounded-md border border-border bg-surface-strong px-2 text-[11.5px]"
                >
                  <option value="all">All accounts</option>
                  {accounts.map((account) => (
                    <option key={account.id} value={account.id}>
                      {account.label}
                    </option>
                  ))}
                </select>
                <select
                  value={filters.checks}
                  onChange={(event) =>
                    setFilters((current) => ({ ...current, checks: event.target.value as Filters['checks'] }))
                  }
                  className="h-7 flex-1 rounded-md border border-border bg-surface-strong px-2 text-[11.5px]"
                >
                  <option value="all">Any check status</option>
                  <option value="passed">Checks passed</option>
                  <option value="failed">Checks failed</option>
                  <option value="running">Checks running</option>
                  <option value="unknown">No checks</option>
                </select>
                {filtersActive && (
                  <Button variant="ghost" size="icon" aria-label="Clear filters" className="h-7 w-7" onClick={() => setFilters(NO_FILTERS)}>
                    <X className="size-3.5" />
                  </Button>
                )}
              </div>
            </div>
          )}

          <div className="min-h-0 flex-1 overflow-y-auto p-1.5">
            {!ready && (
              <div className="flex h-40 items-center justify-center text-muted-foreground">
                <Loader2 className="spin size-4" />
              </div>
            )}

            {ready && empty === 'no-accounts' && (
              <Empty
                icon={<UserRoundPlus className="size-5" />}
                title="No accounts connected"
                body="Add a GitHub, GitLab, Forgejo or Bitbucket account to start collecting the reviews people are waiting on you for."
                action={
                  <Button variant="default" size="sm" onClick={() => setAccountsOpen(true)}>
                    Add an account
                  </Button>
                }
              />
            )}

            {ready && empty === 'syncing' && (
              <Empty
                icon={<Loader2 className="spin size-5" />}
                title="Checking for reviews…"
                body="Asking every connected account what is waiting on you."
              />
            )}

            {ready && (empty === 'no-matches' || empty === 'inbox-zero') && (
              <Empty
                icon={<Inbox className="size-5" />}
                title={empty === 'no-matches' ? 'Nothing matches' : 'Inbox zero'}
                body={
                  empty === 'no-matches'
                    ? 'No review requests match the current filters.'
                    : 'Nobody is waiting on a review from you right now.'
                }
                action={
                  empty === 'no-matches' ? (
                    <Button size="sm" onClick={() => setFilters(NO_FILTERS)}>
                      Clear filters
                    </Button>
                  ) : undefined
                }
              />
            )}

            <ul className="flex flex-col gap-0.5">
              {items.map((item) => (
                <li key={item.id}>
                  <ReviewCard
                    item={item}
                    account={accountFor(item.accountId)}
                    selected={item.id === selectedId}
                    onSelect={() => setSelectedId(item.id)}
                  />
                </li>
              ))}
            </ul>
          </div>

          <footer className="flex items-center gap-2 border-t border-border px-3 py-2 text-[11px] text-muted-foreground">
            <span>
              {items.length} of {deck.items.length} waiting
            </span>
            <span className="ml-auto opacity-70">j / k to move</span>
          </footer>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col">
          {selected ? (
            <PullView key={selected.id} item={selected} />
          ) : (
            <div className="flex flex-1 items-center justify-center">
              <Empty
                icon={<Inbox className="size-5" />}
                title="Nothing selected"
                body="Pick a pull request on the left to read the diff and leave a review."
              />
            </div>
          )}
        </main>
      </div>

      <AccountsDialog open={accountsOpen} onClose={() => setAccountsOpen(false)} />
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  )
}

function Empty({
  icon,
  title,
  body,
  action,
}: {
  icon: React.ReactNode
  title: string
  body: string
  action?: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex flex-col items-center gap-2 px-8 py-14 text-center">
      <span className="flex size-10 items-center justify-center rounded-full bg-muted text-muted-foreground">
        {icon}
      </span>
      <p className="text-[13.5px] font-medium">{title}</p>
      <p className="max-w-72 text-[12.5px] leading-relaxed text-muted-foreground">{body}</p>
      {action && <div className="mt-1">{action}</div>}
    </div>
  )
}

function applyFilters(items: ReviewItem[], filters: Filters, settings: Settings): ReviewItem[] {
  const needle = filters.query.trim().toLowerCase()
  return items.filter((item) => {
    if (!isVisibleReview(item, settings)) return false
    if (filters.account !== 'all' && item.accountId !== filters.account) return false
    if (filters.checks !== 'all' && item.checks.status !== filters.checks) return false
    if (!needle) return true
    return (
      item.title.toLowerCase().includes(needle) ||
      item.repo.toLowerCase().includes(needle) ||
      item.author.name.toLowerCase().includes(needle) ||
      String(item.number).includes(needle)
    )
  })
}
