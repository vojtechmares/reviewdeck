/**
 * The only surface the renderer sees. Context isolation is on, node integration
 * is off, so everything privileged has to come through these named calls.
 */

import { contextBridge, ipcRenderer } from 'electron'
import type {
  Account,
  AccountDraft,
  DeckState,
  DiffRefs,
  LineCommentDraft,
  PullDetail,
  ReviewSubmission,
  Settings,
} from '@shared/types.ts'

const api = {
  accounts: {
    list: (): Promise<Account[]> => ipcRenderer.invoke('accounts:list'),
    add: (draft: AccountDraft): Promise<Account> => ipcRenderer.invoke('accounts:add', draft),
    remove: (id: string): Promise<Account[]> => ipcRenderer.invoke('accounts:remove', id),
    rename: (id: string, label: string): Promise<Account[]> =>
      ipcRenderer.invoke('accounts:rename', id, label),
    update: (id: string, draft: AccountDraft): Promise<Account> =>
      ipcRenderer.invoke('accounts:update', id, draft),
  },
  deck: {
    get: (): Promise<DeckState> => ipcRenderer.invoke('deck:get'),
    refresh: (): Promise<DeckState> => ipcRenderer.invoke('deck:refresh'),
    onChange: (handler: (state: DeckState) => void): (() => void) => {
      const listener = (_event: unknown, state: DeckState): void => handler(state)
      ipcRenderer.on('deck:changed', listener)
      return () => ipcRenderer.off('deck:changed', listener)
    },
    onFocusItem: (handler: (itemId: string) => void): (() => void) => {
      const listener = (_event: unknown, itemId: string): void => handler(itemId)
      ipcRenderer.on('deck:focus-item', listener)
      return () => ipcRenderer.off('deck:focus-item', listener)
    },
  },
  pull: {
    detail: (itemId: string): Promise<PullDetail> => ipcRenderer.invoke('pull:detail', itemId),
    review: (submission: ReviewSubmission): Promise<void> =>
      ipcRenderer.invoke('pull:review', submission),
    comment: (itemId: string, body: string): Promise<void> =>
      ipcRenderer.invoke('pull:comment', itemId, body),
    lineComment: (draft: LineCommentDraft, refs: DiffRefs): Promise<void> =>
      ipcRenderer.invoke('pull:lineComment', draft, refs),
  },
  settings: {
    get: (): Promise<Settings> => ipcRenderer.invoke('settings:get'),
    set: (patch: Partial<Settings>): Promise<Settings> => ipcRenderer.invoke('settings:set', patch),
  },
  app: {
    openExternal: (url: string): Promise<void> => ipcRenderer.invoke('app:openExternal', url),
    info: (): Promise<{ version: string; platform: string; electron: string }> =>
      ipcRenderer.invoke('app:info'),
  },
}

export type ReviewdeckApi = typeof api

contextBridge.exposeInMainWorld('reviewdeck', api)
