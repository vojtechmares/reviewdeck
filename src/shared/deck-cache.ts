/**
 * The deck as it goes to disk, so a relaunch has reviews on screen before the
 * first fan-out returns.
 *
 * Only what the sidebar and the menu bar draw. A ReviewItem is already the
 * list-level view of a pull request - the description, diff and threads that make
 * up its detail are fetched when it is opened and never written down, so opening a
 * cached review still costs exactly the round-trip it always did.
 *
 * The cache is a hint, never truth: a completed sync replaces it wholesale, and
 * anything unrecognised here is dropped rather than allowed to reach the UI
 * half-formed. Starting cold is a slower launch; rendering a card built from a
 * shape nobody wrote is a crash.
 */

import type { CheckSummary, ReviewItem, User } from './types.ts'

/**
 * Bumped whenever a ReviewItem gains a field the cards rely on, so a deck written
 * by an older build is discarded rather than drawn with holes in it.
 */
export const DECK_CACHE_VERSION = 1

export interface DeckCache {
  version: number
  /** accountId -> the items that account returned on the last completed sync. */
  items: Record<string, ReviewItem[]>
}

export function emptyDeckCache(): DeckCache {
  return { version: DECK_CACHE_VERSION, items: {} }
}

/**
 * A cache read back off the vault, or an empty one when there is nothing usable.
 *
 * An envelope that is not what this build writes - a version from another build, a
 * shape that is not an object - takes the whole cache with it. A single item that
 * does not survive its own check only takes itself, because the rest of the deck is
 * still worth showing and the sync on its way replaces all of it anyway.
 */
export function readDeckCache(stored: unknown): DeckCache {
  if (!isRecord(stored)) return emptyDeckCache()
  if (stored['version'] !== DECK_CACHE_VERSION) return emptyDeckCache()
  const storedItems = stored['items']
  if (!isRecord(storedItems)) return emptyDeckCache()

  const items: Record<string, ReviewItem[]> = {}
  for (const [accountId, cached] of Object.entries(storedItems)) {
    if (!Array.isArray(cached)) continue
    items[accountId] = cached.filter(isReviewItem)
  }
  return { version: DECK_CACHE_VERSION, items }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isUser(value: unknown): value is User {
  return isRecord(value) && typeof value['name'] === 'string' && typeof value['avatarUrl'] === 'string'
}

/** The counts a check pill draws; the runs behind it may legitimately be empty. */
function isCheckSummary(value: unknown): value is CheckSummary {
  return (
    isRecord(value) &&
    typeof value['status'] === 'string' &&
    typeof value['passed'] === 'number' &&
    typeof value['failed'] === 'number' &&
    typeof value['running'] === 'number' &&
    typeof value['total'] === 'number' &&
    Array.isArray(value['runs'])
  )
}

function isReviewItem(value: unknown): value is ReviewItem {
  if (!isRecord(value)) return false
  return (
    typeof value['id'] === 'string' &&
    typeof value['accountId'] === 'string' &&
    typeof value['provider'] === 'string' &&
    typeof value['repoKey'] === 'string' &&
    typeof value['repo'] === 'string' &&
    typeof value['number'] === 'number' &&
    typeof value['title'] === 'string' &&
    typeof value['url'] === 'string' &&
    typeof value['createdAt'] === 'string' &&
    typeof value['updatedAt'] === 'string' &&
    typeof value['draft'] === 'boolean' &&
    typeof value['sourceBranch'] === 'string' &&
    typeof value['targetBranch'] === 'string' &&
    typeof value['myReviewState'] === 'string' &&
    Array.isArray(value['labels']) &&
    isUser(value['author']) &&
    isCheckSummary(value['checks'])
  )
}
