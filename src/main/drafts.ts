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

import type { DiffRefs, DraftComment, MyReviewState } from '@shared/types.ts'

/**
 * What is known about one item's drafts beyond the text itself.
 *
 * Local drafts are invisible to the host by design - that is what lets one
 * mechanism serve four of them - and the price is that a review submitted in a
 * browser leaves a full draft set here with nothing saying so. The baseline is what
 * makes that noticeable: the reviewer's own review state when the set began, to
 * compare against what the next sync reports.
 */
export interface DraftSet {
  baseline: MyReviewState
  /** The state changed without this app submitting, and the reviewer has not said what to do. */
  diverged: boolean
}

export interface DraftState {
  comments: DraftComment[]
  sets: Record<string, DraftSet>
}

/**
 * Whether an item's drafts have been left behind by a review submitted elsewhere.
 *
 * Pure, and deliberately conservative: it reports divergence only when it can
 * account for the change. A set with no baseline has nothing to compare, and a
 * sync that was already in flight when this app submitted is holding a state from
 * before that submission - reading it would report the app's own review as
 * somebody else's.
 */
export function hasDiverged(options: {
  baseline: MyReviewState | undefined
  current: MyReviewState
  /** Submissions this app had made when the sync began, and now. */
  submissionsAtSyncStart: number
  submissionsNow: number
}): boolean {
  if (!options.baseline) return false
  if (options.submissionsNow !== options.submissionsAtSyncStart) return false
  return options.current !== options.baseline
}

export interface DraftStoreOptions {
  load: () => DraftState
  save: (state: DraftState) => void
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
  private sets: Record<string, DraftSet>
  /**
   * How many reviews this app has submitted, ever. Only differences matter: it is
   * how a sync tells whether its own data predates a submission made since.
   */
  private submitted = 0
  private readonly options: Required<Omit<DraftStoreOptions, 'load'>>
  private timer: ReturnType<typeof setTimeout> | null = null

  constructor(options: DraftStoreOptions) {
    const loaded = options.load()
    this.drafts = loaded.comments
    this.sets = loaded.sets
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

  /**
   * @param state the reviewer's own review state right now, which becomes the
   *   baseline if this is the first draft for the item. Recording it here rather
   *   than tracking every item in the deck is what makes a set created after an
   *   external change start from the current state instead of reporting divergence
   *   the moment it exists.
   */
  add(draft: NewDraft, state?: MyReviewState): DraftComment {
    const created: DraftComment = {
      ...draft,
      id: this.options.id(),
      createdAt: this.options.now(),
    }
    this.drafts = [...this.drafts, created]
    if (!this.sets[draft.itemId] && state) {
      this.sets = { ...this.sets, [draft.itemId]: { baseline: state, diverged: false } }
    }
    this.schedule()
    return created
  }

  /** How many reviews this app has submitted, for the sync race guard. */
  submissions(): number {
    return this.submitted
  }

  /**
   * The app has submitted for this item: the set starts again from where that left
   * it, and any divergence it was showing is answered.
   */
  recordSubmission(itemId: string, state: MyReviewState): void {
    this.submitted++
    if (!this.count(itemId)) return
    this.sets = { ...this.sets, [itemId]: { baseline: state, diverged: false } }
    this.schedule()
  }

  /** Compares a freshly synced state against the baseline and marks the set if it moved. */
  reconcile(itemId: string, current: MyReviewState, submissionsAtSyncStart: number): boolean {
    const set = this.sets[itemId]
    if (!set || !this.count(itemId)) return false

    const diverged = hasDiverged({
      baseline: set.baseline,
      current,
      submissionsAtSyncStart,
      submissionsNow: this.submitted,
    })
    if (!diverged || set.diverged) return set.diverged

    this.sets = { ...this.sets, [itemId]: { ...set, diverged: true } }
    this.schedule()
    return true
  }

  diverged(itemId: string): boolean {
    return this.sets[itemId]?.diverged === true && this.count(itemId) > 0
  }

  /**
   * The reviewer has said to keep the drafts. The mark goes, the text is not
   * touched, and the baseline moves to what is there now so the same change is not
   * reported twice.
   */
  acknowledge(itemId: string, state: MyReviewState): void {
    const set = this.sets[itemId]
    if (!set) return
    this.sets = { ...this.sets, [itemId]: { baseline: state, diverged: false } }
    this.schedule()
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
    const { [itemId]: _gone, ...rest } = this.sets
    this.sets = rest
    this.schedule()
  }

  /** Writes now, whatever the debounce was waiting for. */
  flush(): void {
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
    this.options.save({ comments: this.drafts, sets: this.sets })
  }

  private schedule(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      this.timer = null
      this.options.save({ comments: this.drafts, sets: this.sets })
    }, this.options.debounceMs)
    // Never hold the process open for a pending write.
    this.timer.unref?.()
  }
}
