/** Every channel the renderer can call. Each handler is the only way in. */

import { app, clipboard, ipcMain, nativeTheme } from 'electron'
import { deck, openExternal } from './deck.ts'
import { drafts } from './draft-store.ts'
import { type NewDraft } from './drafts.ts'
import { DEMO_ACCOUNTS, demoDetail, demoEnabled } from './demo.ts'
import { providerFor } from './providers/index.ts'
import { PartialSubmitError } from './providers/submit.ts'
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
  DraftComment,
  PullDetail,
  ReviewSubmission,
  Settings,
} from '@shared/types.ts'

/** Turn a thrown error into something the UI can render without leaking stack traces. */
function fail(error: unknown): never {
  const message = error instanceof Error ? error.message : String(error)
  throw new Error(message)
}

/** Write anything still pending before the process goes away. */
export function flushDrafts(): void {
  drafts.flush()
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
      const pending = drafts.list(item.id)
      if (!pending.length && !submission.body.trim() && submission.verdict !== 'approve') {
        throw new Error('There is nothing to submit.')
      }

      try {
        await provider.submitReview(
          deck.session(item.accountId),
          item,
          submission.verdict,
          submission.body,
          pending,
        )
      } catch (error) {
        // A host with no batch call can get part of the way. Drop what landed and
        // keep the rest, so retrying finishes the review instead of saying the
        // first half of it twice.
        if (error instanceof PartialSubmitError) {
          for (const posted of error.posted) drafts.remove(posted.id)
          // Part of the review did go out, so what is left starts again from here
          // rather than reading as a review somebody else submitted.
          drafts.recordSubmission(item.id, item.myReviewState)
        }
        throw error
      }

      const state =
        submission.verdict === 'approve'
          ? 'approved'
          : submission.verdict === 'request_changes'
            ? 'changes_requested'
            : 'commented'

      // Before clearing, so a sync already in flight cannot report this app's own
      // review as one submitted elsewhere.
      drafts.recordSubmission(item.id, state)
      // Only now: a submission that threw outright leaves every draft where it was.
      drafts.clear(item.id)
      deck.patch(item.id, { myReviewState: state })
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

  ipcMain.handle('drafts:list', (_event, itemId: string): DraftComment[] => drafts.list(itemId))

  ipcMain.handle('drafts:add', (_event, draft: NewDraft): DraftComment[] => {
    try {
      if (!deck.find(draft.itemId)) throw new Error('That pull request is no longer in the deck.')
      if (!draft.body.trim()) throw new Error('The comment is empty.')

      const item = deck.find(draft.itemId)
      drafts.add({ ...draft, body: draft.body.trim() }, item?.myReviewState)
      return drafts.list(draft.itemId)
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle('drafts:update', (_event, id: string, body: string): DraftComment[] => {
    try {
      if (!body.trim()) throw new Error('The comment is empty.')
      const updated = drafts.update(id, body.trim())
      if (!updated) throw new Error('That draft is gone.')
      return drafts.list(updated.itemId)
    } catch (error) {
      fail(error)
    }
  })

  ipcMain.handle('drafts:diverged', (_event, itemId: string): boolean => drafts.diverged(itemId))

  /** Keep the drafts: the mark goes and not one character of them is touched. */
  ipcMain.handle('drafts:acknowledge', (_event, itemId: string): boolean => {
    const item = deck.find(itemId)
    drafts.acknowledge(itemId, item?.myReviewState ?? 'pending')
    return drafts.diverged(itemId)
  })

  /** Discard them: only ever reached through an explicit confirmation in the app. */
  ipcMain.handle('drafts:discard', (_event, itemId: string): DraftComment[] => {
    drafts.clear(itemId)
    return drafts.list(itemId)
  })

  ipcMain.handle('drafts:remove', (_event, id: string): DraftComment[] => {
    const removed = drafts.remove(id)
    return removed ? drafts.list(removed.itemId) : []
  })

  ipcMain.handle('settings:get', (): Settings => getSettings())

  ipcMain.handle('settings:set', (_event, patch: Partial<Settings>): Settings => {
    const next = saveSettings(patch)
    if (patch.pollInterval !== undefined || patch.checkPollInterval !== undefined) deck.reschedule()
    // The menu bar counts what the deck shows, so a setting that changes what it
    // shows has to redraw it now rather than at the next background refresh.
    if (patch.hideApproved !== undefined) deck.publish()
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
