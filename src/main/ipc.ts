/** Every channel the renderer can call. Each handler is the only way in. */

import { app, clipboard, ipcMain, nativeTheme } from 'electron'
import { deck, openExternal } from './deck.ts'
import { DEMO_ACCOUNTS, demoDetail, demoEnabled } from './demo.ts'
import { providerFor } from './providers/index.ts'
import {
  addAccount,
  getAccount,
  getSettings,
  getToken,
  listAccounts,
  removeAccount,
  saveSettings,
  setToken,
  updateAccount,
} from './store.ts'
import { PROVIDER_LABELS } from '@shared/types.ts'
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
      const account = addAccount(
        { ...resolved, agentCommand: draft.agentCommand?.trim() || undefined },
        draft.token.trim(),
      )
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

  ipcMain.handle('accounts:update', async (_event, id: string, draft: AccountDraft): Promise<Account> => {
    try {
      const existing = getAccount(id)
      if (!existing) throw new Error('That account is gone.')
      const token = draft.token?.trim() || getToken(id)
      if (!token) throw new Error('A token is required.')
      const resolved = await providerFor(existing.kind).connect({
        ...draft,
        kind: existing.kind,
        token,
      })
      updateAccount(id, {
        ...resolved,
        label: draft.label.trim() || existing.label,
        // Blank clears the override, so the setting applies again.
        agentCommand: draft.agentCommand?.trim() || undefined,
      })
      if (draft.token?.trim()) setToken(id, draft.token.trim())
      void deck.refresh()
      const updated = getAccount(id)
      if (!updated) throw new Error('That account is gone.')
      return updated
    } catch (error) {
      fail(error)
    }
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
    'pull:replyToThread',
    async (_event, itemId: string, threadId: string, body: string) => {
      try {
        const item = deck.find(itemId)
        if (!item) throw new Error('That pull request is no longer in the deck.')
        if (!body.trim()) throw new Error('The reply is empty.')
        const provider = providerFor(item.provider)
        // The renderer only offers this where the thread says it can, so reaching
        // here without support is a bug rather than something the user did.
        if (!provider.replyToThread) {
          throw new Error(`${PROVIDER_LABELS[item.provider]} cannot reply to a thread from here.`)
        }
        await provider.replyToThread(deck.session(item.accountId), item, threadId, body)
      } catch (error) {
        fail(error)
      }
    },
  )

  ipcMain.handle(
    'pull:resolveThread',
    async (_event, itemId: string, threadId: string, resolved: boolean) => {
      try {
        const item = deck.find(itemId)
        if (!item) throw new Error('That pull request is no longer in the deck.')
        const provider = providerFor(item.provider)
        if (!provider.setThreadResolved) {
          throw new Error(`${PROVIDER_LABELS[item.provider]} cannot resolve a thread from here.`)
        }
        await provider.setThreadResolved(deck.session(item.accountId), item, threadId, resolved)
      } catch (error) {
        fail(error)
      }
    },
  )

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

  // The renderer has no clipboard of its own worth relying on, and this keeps the
  // handoff to one place: text in, clipboard out, nothing executed.
  ipcMain.handle('app:copyText', (_event, text: string) => clipboard.writeText(text))

  ipcMain.handle('app:info', () => ({
    version: app.getVersion(),
    platform: process.platform,
    electron: process.versions.electron,
  }))
}
