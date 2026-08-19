import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import {
  DEFAULT_SETTINGS,
  type Account,
  type DeckState,
  type Settings,
} from '@shared/types'

interface AppValue {
  accounts: Account[]
  deck: DeckState
  settings: Settings
  ready: boolean
  refresh: () => Promise<void>
  reloadAccounts: () => Promise<void>
  updateSettings: (patch: Partial<Settings>) => Promise<void>
  accountFor: (id: string) => Account | undefined
}

const EMPTY_DECK: DeckState = { items: [], statuses: [], syncing: false, synced: false }

const AppContext = createContext<AppValue | null>(null)

export function AppProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const [accounts, setAccounts] = useState<Account[]>([])
  const [deck, setDeck] = useState<DeckState>(EMPTY_DECK)
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS)
  const [ready, setReady] = useState(false)

  useEffect(() => {
    let cancelled = false
    void (async () => {
      const [loadedAccounts, loadedDeck, loadedSettings] = await Promise.all([
        window.reviewdeck.accounts.list(),
        window.reviewdeck.deck.get(),
        window.reviewdeck.settings.get(),
      ])
      if (cancelled) return
      setAccounts(loadedAccounts)
      setDeck(loadedDeck)
      setSettings(loadedSettings)
      setReady(true)
    })()

    // The main process owns the deck; the renderer just mirrors it.
    const unsubscribe = window.reviewdeck.deck.onChange(setDeck)
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [])

  // Theme is applied here so every surface reacts to the setting immediately.
  useEffect(() => {
    const root = document.documentElement
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = (): void => {
      const dark = settings.theme === 'dark' || (settings.theme === 'system' && media.matches)
      root.classList.toggle('dark', dark)
    }
    apply()
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [settings.theme])

  const refresh = useCallback(async () => {
    setDeck((current) => ({ ...current, syncing: true }))
    const next = await window.reviewdeck.deck.refresh()
    setDeck(next)
  }, [])

  const reloadAccounts = useCallback(async () => {
    setAccounts(await window.reviewdeck.accounts.list())
  }, [])

  const updateSettings = useCallback(async (patch: Partial<Settings>) => {
    setSettings(await window.reviewdeck.settings.set(patch))
  }, [])

  const accountFor = useCallback(
    (id: string) => accounts.find((account) => account.id === id),
    [accounts],
  )

  const value = useMemo<AppValue>(
    () => ({ accounts, deck, settings, ready, refresh, reloadAccounts, updateSettings, accountFor }),
    [accounts, deck, settings, ready, refresh, reloadAccounts, updateSettings, accountFor],
  )

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>
}

export function useApp(): AppValue {
  const value = useContext(AppContext)
  if (!value) throw new Error('useApp must be used inside AppProvider')
  return value
}

/** Pull the message out of the Error IPC wraps around a main-process failure. */
export function errorMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error)
  // Electron prefixes it with "Error invoking remote method 'x': Error: ".
  return raw.replace(/^Error invoking remote method '[^']+':\s*/, '').replace(/^Error:\s*/, '')
}
