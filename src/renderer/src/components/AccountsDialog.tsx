import { useState } from 'react'
import { AlertTriangle, ExternalLink, Loader2, Plus, Trash2 } from 'lucide-react'
import { PROVIDER_LABELS, type AccountDraft, type ProviderKind } from '@shared/types'
import { errorMessage, useApp } from '@/hooks/useApp'
import { Avatar } from './ui/avatar'
import { Button } from './ui/button'
import { Dialog } from './ui/dialog'
import { Input, Label } from './ui/input'
import { useToast } from './ui/toast'
import { ProviderIcon } from './ProviderIcon'

/**
 * Reviewdeck signs in with personal access tokens rather than OAuth: every host
 * a freelancer runs into (self-hosted GitLab, a company Forgejo) can mint one,
 * whereas OAuth would need an app registered on each instance up front.
 */
const GUIDES: Record<ProviderKind, { host: string; scopes: string; url: string; hostHint: string }> = {
  github: {
    host: 'github.com',
    scopes: 'repo, read:org',
    url: 'https://github.com/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck',
    hostHint: 'Use your Enterprise Server hostname for GHES, e.g. github.acme.com',
  },
  gitlab: {
    host: 'gitlab.com',
    scopes: 'api',
    url: 'https://gitlab.com/-/user_settings/personal_access_tokens',
    hostHint: 'Self-hosted works too, e.g. gitlab.acme.com',
  },
  forgejo: {
    host: 'codeberg.org',
    scopes: 'read:user, read:repository, write:issue, write:repository',
    url: 'https://codeberg.org/user/settings/applications',
    hostHint: 'Any Forgejo or Gitea instance',
  },
  bitbucket: {
    host: 'bitbucket.org',
    scopes: 'Account: Read · Pull requests: Write',
    url: 'https://bitbucket.org/account/settings/app-passwords/new',
    hostHint: 'Bitbucket Cloud only',
  },
}

export function AccountsDialog({
  open,
  onClose,
}: {
  open: boolean
  onClose: () => void
}): React.JSX.Element {
  const { accounts, reloadAccounts, deck } = useApp()
  const toast = useToast()

  const [adding, setAdding] = useState(false)
  const [busy, setBusy] = useState(false)
  const [kind, setKind] = useState<ProviderKind>('github')
  const [host, setHost] = useState(GUIDES.github.host)
  const [label, setLabel] = useState('')
  const [username, setUsername] = useState('')
  const [token, setToken] = useState('')

  const guide = GUIDES[kind]

  const reset = (): void => {
    setAdding(false)
    setToken('')
    setLabel('')
    setUsername('')
  }

  const pickKind = (next: ProviderKind): void => {
    setKind(next)
    setHost(GUIDES[next].host)
  }

  const save = async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    try {
      const draft: AccountDraft = { kind, host, label: label.trim(), token: token.trim(), username: username.trim() }
      const account = await window.reviewdeck.accounts.add(draft)
      await reloadAccounts()
      toast.ok(`Signed in as ${account.displayName}.`)
      reset()
    } catch (cause) {
      toast.bad(errorMessage(cause))
    } finally {
      setBusy(false)
    }
  }

  const remove = async (id: string, name: string): Promise<void> => {
    await window.reviewdeck.accounts.remove(id)
    await reloadAccounts()
    toast.info(`Removed ${name}.`)
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Accounts"
      description="Connect every host you review on. Tokens are encrypted with your macOS keychain."
      className="max-w-xl"
      footer={
        adding ? (
          <>
            <Button variant="ghost" onClick={reset} disabled={busy}>
              Cancel
            </Button>
            <Button variant="default" onClick={() => void save()} disabled={busy || !token.trim()}>
              {busy && <Loader2 className="spin size-3.5" />}
              {busy ? 'Verifying…' : 'Connect'}
            </Button>
          </>
        ) : (
          <Button variant="default" onClick={() => setAdding(true)}>
            <Plus className="size-3.5" />
            Add account
          </Button>
        )
      }
    >
      {!adding ? (
        <ul className="flex flex-col gap-2">
          {accounts.map((account) => {
            const status = deck.statuses.find((entry) => entry.accountId === account.id)
            return (
              <li key={account.id} className="glass flex items-center gap-3 rounded-lg px-3 py-2.5">
                <Avatar src={account.avatarUrl} name={account.displayName} className="size-8" />
                <div className="min-w-0 flex-1">
                  <p className="flex items-center gap-1.5 text-[13px] font-medium">
                    <ProviderIcon kind={account.kind} className="size-3 opacity-70" />
                    <span className="truncate">{account.label}</span>
                  </p>
                  <p className="truncate text-[11.5px] text-muted-foreground">
                    {account.username} · {new URL(account.webUrl).host}
                  </p>
                  {status && !status.ok && (
                    <p className="mt-1 flex items-start gap-1 text-[11.5px] text-bad">
                      <AlertTriangle className="mt-px size-3 shrink-0" />
                      <span>{status.error}</span>
                    </p>
                  )}
                </div>
                <span className="shrink-0 text-[11.5px] tabular-nums text-muted-foreground">
                  {status?.count ?? 0}
                </span>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${account.label}`}
                  onClick={() => void remove(account.id, account.label)}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </li>
            )
          })}

          {!accounts.length && (
            <p className="py-8 text-center text-[13px] text-muted-foreground">
              No accounts yet. Add one to start collecting review requests.
            </p>
          )}
        </ul>
      ) : (
        <div className="flex flex-col gap-3.5">
          <div>
            <Label>Provider</Label>
            <div className="grid grid-cols-2 gap-1.5">
              {(Object.keys(PROVIDER_LABELS) as ProviderKind[]).map((option) => (
                <button
                  key={option}
                  onClick={() => pickKind(option)}
                  className={cnKind(kind === option)}
                >
                  <ProviderIcon kind={option} className="size-4" />
                  {PROVIDER_LABELS[option]}
                </button>
              ))}
            </div>
          </div>

          <div>
            <Label htmlFor="host">Host</Label>
            <Input
              id="host"
              value={host}
              onChange={(event) => setHost(event.target.value)}
              placeholder={guide.host}
              disabled={kind === 'bitbucket'}
              spellCheck={false}
            />
            <p className="mt-1 text-[11px] text-muted-foreground">{guide.hostHint}</p>
          </div>

          {kind === 'bitbucket' && (
            <div>
              <Label htmlFor="username">Bitbucket username</Label>
              <Input
                id="username"
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                placeholder="your-handle"
                spellCheck={false}
              />
              <p className="mt-1 text-[11px] text-muted-foreground">
                App passwords authenticate as username + password, so both are needed.
              </p>
            </div>
          )}

          <div>
            <Label htmlFor="token">
              {kind === 'bitbucket' ? 'App password' : 'Personal access token'}
            </Label>
            <Input
              id="token"
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              placeholder="••••••••••••••••"
              spellCheck={false}
              autoComplete="off"
              onKeyDown={(event) => {
                if (event.key === 'Enter' && token.trim()) void save()
              }}
            />
            <p className="mt-1 flex flex-wrap items-center gap-1 text-[11px] text-muted-foreground">
              Needs <span className="mono !text-[10.5px]">{guide.scopes}</span>.
              <button
                className="inline-flex items-center gap-0.5 text-info hover:underline"
                onClick={() => void window.reviewdeck.app.openExternal(guide.url)}
              >
                Create one
                <ExternalLink className="size-2.5" />
              </button>
            </p>
          </div>

          <div>
            <Label htmlFor="label">Name (optional)</Label>
            <Input
              id="label"
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              placeholder="Work GitHub"
            />
          </div>
        </div>
      )}
    </Dialog>
  )
}

function cnKind(active: boolean): string {
  return [
    'flex items-center gap-2 rounded-lg border px-3 py-2 text-[12.5px] font-medium transition-colors',
    active
      ? 'border-border-strong bg-surface-strong'
      : 'border-border bg-transparent text-muted-foreground hover:bg-accent hover:text-foreground',
  ].join(' ')
}
