/**
 * Persistence: accounts and settings land in a plain JSON file under userData,
 * while tokens go through Electron's safeStorage so they end up encrypted with a
 * key held in the macOS Keychain rather than sitting on disk in the clear.
 */

import { app, safeStorage } from 'electron'
import type { DraftSet, DraftState } from './drafts.ts'
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import { emptyDeckCache, readDeckCache, type DeckCache } from '@shared/deck-cache.ts'
import {
  DEFAULT_SETTINGS,
  mergeSettings,
  type Account,
  type DraftComment,
  type ReviewItem,
  type Settings,
} from '@shared/types.ts'

interface Vault {
  version: 1
  accounts: Account[]
  /** accountId -> base64 of the safeStorage-encrypted token. */
  tokens: Record<string, string>
  settings: Settings
  /** Item ids we have already notified about, so a restart does not re-announce them. */
  seen: string[]
  /** Line comments written but not yet submitted, across every pull request. */
  drafts: DraftComment[]
  /** What is known about each item's draft set beyond the text: baseline, divergence. */
  draftSets: Record<string, DraftSet>
  /** The last synced deck, so a launch has reviews to show before the fan-out. */
  deck: DeckCache
}

const EMPTY: Vault = {
  version: 1,
  accounts: [],
  tokens: {},
  settings: { ...DEFAULT_SETTINGS },
  seen: [],
  drafts: [],
  draftSets: {},
  deck: emptyDeckCache(),
}

let cache: Vault | null = null

function file(): string {
  const dir = app.getPath('userData')
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true })
  return join(dir, 'reviewdeck.json')
}

function load(): Vault {
  if (cache) return cache
  const path = file()
  if (!existsSync(path)) {
    cache = { ...EMPTY }
    return cache
  }
  try {
    const parsed = JSON.parse(readFileSync(path, 'utf8')) as Partial<Vault>
    cache = {
      version: 1,
      accounts: parsed.accounts ?? [],
      tokens: parsed.tokens ?? {},
      settings: mergeSettings(parsed.settings),
      seen: parsed.seen ?? [],
      drafts: parsed.drafts ?? [],
      draftSets: parsed.draftSets ?? {},
      deck: readDeckCache(parsed.deck),
    }
  } catch (error) {
    // A corrupt file should not brick the app; keep the bad copy for forensics.
    console.error('[store] could not read vault, starting fresh:', error)
    try {
      renameSync(path, `${path}.corrupt-${Date.now()}`)
    } catch {
      /* best effort */
    }
    cache = { ...EMPTY }
  }
  return cache
}

function persist(): void {
  const path = file()
  const tmp = `${path}.tmp`
  writeFileSync(tmp, JSON.stringify(load(), null, 2), { mode: 0o600 })
  renameSync(tmp, path)
}

export function listAccounts(): Account[] {
  return load().accounts.map((account) => ({ ...account }))
}

export function getAccount(id: string): Account | undefined {
  return load().accounts.find((account) => account.id === id)
}

export function addAccount(account: Omit<Account, 'id' | 'addedAt'>, token: string): Account {
  const vault = load()
  const created: Account = { ...account, id: randomUUID(), addedAt: new Date().toISOString() }
  vault.accounts.push(created)
  vault.tokens[created.id] = encrypt(token)
  persist()
  return created
}

export function updateAccount(id: string, patch: Partial<Account>): void {
  const vault = load()
  const index = vault.accounts.findIndex((account) => account.id === id)
  if (index < 0) return
  vault.accounts[index] = { ...vault.accounts[index], ...patch, id }
  persist()
}

export function removeAccount(id: string): void {
  const vault = load()
  vault.accounts = vault.accounts.filter((account) => account.id !== id)
  delete vault.tokens[id]
  // A signed-out host does not get to leave its reviews on disk. Replaced rather
  // than mutated, because a fresh vault shares its empty cache with the default.
  vault.deck = {
    ...vault.deck,
    items: Object.fromEntries(Object.entries(vault.deck.items).filter(([key]) => key !== id)),
  }
  persist()
}

export function getToken(id: string): string {
  const stored = load().tokens[id]
  if (!stored) throw new Error('No token stored for this account. Remove it and sign in again.')
  return decrypt(stored)
}

export function setToken(id: string, token: string): void {
  const vault = load()
  if (!vault.accounts.some((account) => account.id === id)) {
    throw new Error('No such account.')
  }
  vault.tokens[id] = encrypt(token)
  persist()
}

export function loadDraftState(): DraftState {
  const vault = load()
  return { comments: vault.drafts, sets: vault.draftSets }
}

export function persistDraftState(state: DraftState): void {
  const vault = load()
  vault.drafts = state.comments
  vault.draftSets = state.sets
  persist()
}

/**
 * The cached deck, minus anything belonging to an account that is no longer here -
 * a vault edited by hand, or an account dropped by a build that did not prune.
 */
export function loadDeckCache(): Record<string, ReviewItem[]> {
  const vault = load()
  const connected = new Set(vault.accounts.map((account) => account.id))
  const items: Record<string, ReviewItem[]> = {}
  for (const [accountId, cached] of Object.entries(vault.deck.items)) {
    if (connected.has(accountId)) items[accountId] = cached
  }
  return items
}

/** Replaces the cached deck wholesale, so a sync's removals are removals here too. */
export function persistDeckCache(items: Record<string, ReviewItem[]>): void {
  const vault = load()
  vault.deck = { ...emptyDeckCache(), items }
  persist()
}

export function getSettings(): Settings {
  return { ...load().settings }
}

export function saveSettings(patch: Partial<Settings>): Settings {
  const vault = load()
  vault.settings = { ...vault.settings, ...patch }
  persist()
  return { ...vault.settings }
}

/**
 * Returns the ids that are new since the last call and records them, so the
 * poller only ever raises a notification once per review request.
 */
export function markSeen(ids: string[]): string[] {
  const vault = load()
  const known = new Set(vault.seen)
  const fresh = ids.filter((id) => !known.has(id))
  if (fresh.length) {
    // Keep the list bounded; closed PRs never come back with the same id.
    vault.seen = [...vault.seen, ...fresh].slice(-2000)
    persist()
  }
  return fresh
}

function encrypt(value: string): string {
  if (!safeStorage.isEncryptionAvailable()) {
    // Without a Keychain-backed key we would be writing plaintext; refuse instead.
    throw new Error('Encrypted storage is unavailable, so the token cannot be saved safely.')
  }
  return safeStorage.encryptString(value).toString('base64')
}

function decrypt(value: string): string {
  return safeStorage.decryptString(Buffer.from(value, 'base64'))
}
