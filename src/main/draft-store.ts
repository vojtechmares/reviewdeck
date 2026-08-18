/**
 * The one draft store the app runs on.
 *
 * It lives here rather than beside the class so the class keeps no electron in its
 * import graph, and rather than inside the IPC layer so the sync engine can reach
 * it without the two importing each other.
 */

import { DraftStore } from './drafts.ts'
import { loadDraftState, persistDraftState } from './store.ts'

export const drafts = new DraftStore({ load: loadDraftState, save: persistDraftState })
