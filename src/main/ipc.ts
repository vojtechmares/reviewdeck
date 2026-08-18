/** Every channel the renderer can call. Each handler is the only way in. */

import { app, ipcMain, nativeTheme } from 'electron'
import { deck, openExternal } from './deck.ts'
import { DEMO_ACCOUNTS, demoDetail, demoEnabled } from './demo.ts'
import { providerFor } from './providers/index.ts'
import {
  addAccount,
  getSettings,
  listAccounts,
  removeAccount,
  saveSettings,
  updateAccount,
} from './store.ts'
import type {
  Account,
  AccountDraft,
  LineCommentDraft,
  PullDetail,
  ReviewSubmission,
  Settings,
} from '@shared/types.ts'

/** Turn a thrown error into something the UI can render without leaking stack traces. */
function fail(error: unknown): never {
  const message = error instanceof Error ? error.message : String(error)
  throw new Error(message)
}

export function registerIpc(): void {
  ipcMain.handle('accounts:list', () => (demoEnabled() ? DEMO_ACCOUNTS : listAccounts()))

  ipcMain.handle('accounts:add', async (_event, draft: AccountDraft): Promise<Account> => {
    try {
      if (!draft.token?.trim()) throw new Error('A token is required.')
      const provider = providerFor(draft.kind)
      const resolved = await provider.connect({ ...draft, token: draft.token.trim() })
      const account = addAccount(resolved, draft.token.trim())
      void deck.refresh()
      return account
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle('accounts:remove', (_event, id: string) => {
    removeAccount(id)
    void deck.refresh()
    return listAccounts()
  })

  ipcMain.handle('accounts:rename', (_event, id: string, label: string) => {
    updateAccount(id, { label })
    deck.publish()
    return listAccounts()
  })

  ipcMain.handle('deck:get', () => deck.state())

  ipcMain.handle('deck:refresh', async () => {
    try {
      return await deck.refresh()
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle('pull:detail', async (_event, itemId: string): Promise<PullDetail> => {
    try {
      const item = deck.find(itemId)
      if (!item) throw new Error('That pull request is no longer in the deck.')
      if (demoEnabled()) return demoDetail(item)
      const provider = providerFor(item.provider)
      const detail = await provider.loadDetail(deck.session(item.accountId), item)
      // Keep the deck's copy in sync with the numbers the detail call resolved.
      deck.patch(itemId, {
        additions: detail.item.additions,
        deletions: detail.item.deletions,
        changedFiles: detail.item.changedFiles,
      })
      return detail
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle('pull:review', async (_event, submission: ReviewSubmission) => {
    try {
      const item = deck.find(submission.itemId)
      if (!item) throw new Error('That pull request is no longer in the deck.')
      const provider = providerFor(item.provider)
      await provider.submitReview(deck.session(item.accountId), item, submission.verdict, submission.body)
      deck.patch(item.id, {
        myReviewState:
          submission.verdict === 'approve'
            ? 'approved'
            : submission.verdict === 'request_changes'
              ? 'changes_requested'
              : 'commented',
      })
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle('pull:comment', async (_event, itemId: string, body: string) => {
    try {
      const item = deck.find(itemId)
      if (!item) throw new Error('That pull request is no longer in the deck.')
      if (!body.trim()) throw new Error('The comment is empty.')
      await providerFor(item.provider).addComment(deck.session(item.accountId), item, body)
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle(
    'pull:lineComment',
    async (_event, draft: LineCommentDraft, refs: PullDetail['refs']) => {
      try {
        const item = deck.find(draft.itemId)
        if (!item) throw new Error('That pull request is no longer in the deck.')
        if (!draft.body.trim()) throw new Error('The comment is empty.')
        await providerFor(item.provider).addLineComment(deck.session(item.accountId), item, draft, refs)
      } catch (error) {
        fail(error)
      }
    },
  )

  ipcMain.handle('settings:get', (): Settings => getSettings())

  ipcMain.handle('settings:set', (_event, patch: Partial<Settings>): Settings => {
    const next = saveSettings(patch)
    if (patch.pollInterval !== undefined || patch.checkPollInterval !== undefined) deck.reschedule()
    // The window's vibrancy and title bar follow the native theme, not the CSS class.
    if (patch.theme !== undefined) nativeTheme.themeSource = patch.theme
    if (patch.launchAtLogin !== undefined) {
      app.setLoginItemSettings({ openAtLogin: patch.launchAtLogin, openAsHidden: true })
    }
    return next
  })

  ipcMain.handle('app:openExternal', (_event, url: string) => openExternal(url))

  ipcMain.handle('app:info', () => ({
    version: app.getVersion(),
    platform: process.platform,
    electron: process.versions.electron,
  }))
}
