/**
 * Line comments written but not yet sent.
 *
 * The main process owns them, not the renderer: they outlive the window, they have
 * to be there when the app is reopened, and they are what a review submission is
 * built from.
 *
 * They are held in memory and written on a debounce, because the store they land in
 * rewrites its whole file synchronously on every change - fine once a review is
 * submitted, ruinous once per keystroke.
 *
 * Nothing here imports electron, and the clock, the identifiers and the write are
 * all injected, so the keying, the recorded references and what happens when a
 * submission fails can be tested directly.
 */

import type { DiffRefs, DraftComment } from '@shared/types.ts'

export interface DraftStoreOptions {
  load: () => DraftComment[]
  save: (drafts: DraftComment[]) => void
  /** How long after the last change to write. */
  debounceMs?: number
  now?: () => string
  id?: () => string
}

export interface NewDraft {
  itemId: string
  body: string
  path: string
  newLine?: number
  oldLine?: number
  refs: DiffRefs
}

const DEBOUNCE_MS = 800

export class DraftStore {
  private drafts: DraftComment[]
  private readonly options: Required<Omit<DraftStoreOptions, 'load'>>
  private timer: ReturnType<typeof setTimeout> | null = null

  constructor(options: DraftStoreOptions) {
    this.drafts = options.load()
    this.options = {
      save: options.save,
      debounceMs: options.debounceMs ?? DEBOUNCE_MS,
      now: options.now ?? ((): string => new Date().toISOString()),
      id: options.id ?? ((): string => `draft-${Math.random().toString(36).slice(2, 12)}`),
    }
  }

  /** Oldest first, so a review reads in the order it was written. */
  list(itemId: string): DraftComment[] {
    return this.drafts
      .filter((draft) => draft.itemId === itemId)
      .sort((a, b) => a.createdAt.localeCompare(b.createdAt))
  }

  count(itemId: string): number {
    return this.drafts.reduce((total, draft) => total + (draft.itemId === itemId ? 1 : 0), 0)
  }

  add(draft: NewDraft): DraftComment {
    const created: DraftComment = {
      ...draft,
      id: this.options.id(),
      createdAt: this.options.now(),
    }
    this.drafts = [...this.drafts, created]
    this.schedule()
    return created
  }

  update(id: string, body: string): DraftComment | undefined {
    const found = this.drafts.find((draft) => draft.id === id)
    if (!found) return undefined

    const updated = { ...found, body }
    this.drafts = this.drafts.map((draft) => (draft.id === id ? updated : draft))
    this.schedule()
    return updated
  }

  remove(id: string): DraftComment | undefined {
    const found = this.drafts.find((draft) => draft.id === id)
    if (!found) return undefined

    this.drafts = this.drafts.filter((draft) => draft.id !== id)
    this.schedule()
    return found
  }

  /**
   * Drops an item's drafts. Only ever called once a submission has come back
   * clean - a submission that fails must leave the set exactly as it was, which is
   * why nothing is taken away in advance and put back afterwards.
   */
  clear(itemId: string): void {
    if (!this.count(itemId)) return
    this.drafts = this.drafts.filter((draft) => draft.itemId !== itemId)
    this.schedule()
  }

  /** Writes now, whatever the debounce was waiting for. */
  flush(): void {
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
    this.options.save(this.drafts)
  }

  private schedule(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      this.timer = null
      this.options.save(this.drafts)
    }, this.options.debounceMs)
    // Never hold the process open for a pending write.
    this.timer.unref?.()
  }
}
