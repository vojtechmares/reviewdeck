import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Check,
  ClipboardCopy,
  Columns2,
  ExternalLink,
  FileDiff,
  GitBranch,
  Loader2,
  MessageSquare,
  Rows3,
  Send,
  X,
} from 'lucide-react'
import type { LineCommentDraft, PullDetail, ReviewItem, ReviewVerdict } from '@shared/types'
import { agentCommand } from '@shared/agent-prompt'
import { repositoryRoot } from '@shared/autolink'
import { cn, relativeTime } from '@/lib/utils'
import { errorMessage, useApp } from '@/hooks/useApp'
import { Avatar } from './ui/avatar'
import { Badge } from './ui/badge'
import { Button } from './ui/button'
import { Textarea } from './ui/input'
import { Tooltip } from './ui/tooltip'
import { useToast } from './ui/toast'
import { CheckPill } from './CheckPill'
import { ChecksPanel } from './ChecksPanel'
import { DiffView } from './DiffView'
import { Markdown, MarkdownLinks } from './Markdown'
import { ProviderIcon } from './ProviderIcon'
import { ThreadCard } from './Thread'

type Tab = 'diff' | 'checks' | 'conversation'

export function PullView({ item }: { item: ReviewItem }): React.JSX.Element {
  const { settings, updateSettings, refresh, accountFor } = useApp()
  const toast = useToast()

  const [detail, setDetail] = useState<PullDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [tab, setTab] = useState<Tab>('diff')
  const [verdict, setVerdict] = useState<ReviewVerdict | null>(null)
  const [body, setBody] = useState('')
  const [submitting, setSubmitting] = useState(false)

  // Reload whenever the selection changes; a stale diff would be worse than a spinner.
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setError(null)
    setDetail(null)
    setTab('diff')
    setVerdict(null)
    setBody('')

    void window.reviewdeck.pull
      .detail(item.id)
      .then((loaded) => {
        if (!cancelled) setDetail(loaded)
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(errorMessage(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [item.id])

  const conversation = useMemo(
    () => (detail?.threads ?? []).filter((thread) => !thread.path),
    [detail],
  )
  const inlineCount = (detail?.threads.length ?? 0) - conversation.length

  const openExternal = useCallback((url: string) => {
    void window.reviewdeck.app.openExternal(url)
  }, [])

  const addLineComment = useCallback(
    async (draft: Omit<LineCommentDraft, 'itemId'>) => {
      if (!detail) return
      try {
        await window.reviewdeck.pull.lineComment({ ...draft, itemId: item.id }, detail.refs)
        toast.ok('Comment posted.')
        const reloaded = await window.reviewdeck.pull.detail(item.id)
        setDetail(reloaded)
      } catch (cause) {
        toast.bad(errorMessage(cause))
        throw cause
      }
    },
    [detail, item.id, toast],
  )

  const replyToThread = useCallback(
    async (threadId: string, body: string) => {
      try {
        await window.reviewdeck.pull.replyToThread(item.id, threadId, body)
        toast.ok('Reply posted.')
        setDetail(await window.reviewdeck.pull.detail(item.id))
      } catch (cause) {
        toast.bad(errorMessage(cause))
        throw cause
      }
    },
    [item.id, toast],
  )

  const resolveThread = useCallback(
    async (threadId: string, resolved: boolean) => {
      try {
        await window.reviewdeck.pull.resolveThread(item.id, threadId, resolved)
        toast.ok(resolved ? 'Thread resolved.' : 'Thread reopened.')
        setDetail(await window.reviewdeck.pull.detail(item.id))
      } catch (cause) {
        toast.bad(errorMessage(cause))
        throw cause
      }
    },
    [item.id, toast],
  )

  // Nothing is spawned: the command goes on the clipboard for the user to run in the
  // terminal they already have open in that repository.
  const copyAgentPrompt = useCallback(() => {
    const command = agentCommand(item, detail?.threads ?? [], accountFor(item.accountId), settings)
    void window.reviewdeck.app
      .copyText(command)
      .then(() => toast.ok('Claude prompt copied - paste it in your terminal.'))
      .catch((cause: unknown) => toast.bad(errorMessage(cause)))
  }, [accountFor, detail, item, settings, toast])

  const submit = useCallback(async () => {
    if (submitting) return
    setSubmitting(true)
    try {
      if (verdict) {
        await window.reviewdeck.pull.review({ itemId: item.id, verdict, body: body.trim() })
        toast.ok(
          verdict === 'approve'
            ? 'Approved.'
            : verdict === 'request_changes'
              ? 'Changes requested.'
              : 'Review comment posted.',
        )
      } else {
        await window.reviewdeck.pull.comment(item.id, body.trim())
        toast.ok('Comment posted.')
      }
      setBody('')
      setVerdict(null)
      const reloaded = await window.reviewdeck.pull.detail(item.id)
      setDetail(reloaded)
      void refresh()
    } catch (cause) {
      toast.bad(errorMessage(cause))
    } finally {
      setSubmitting(false)
    }
  }, [body, item.id, refresh, submitting, toast, verdict])

  const canSubmit = (verdict === 'approve' || body.trim().length > 0) && !submitting

  // Where `@someone` and `#123` in this pull request's prose should point.
  const autolink = useMemo(
    () => ({
      provider: item.provider,
      webUrl: accountFor(item.accountId)?.webUrl ?? '',
      repoRoot: repositoryRoot(item),
    }),
    [accountFor, item],
  )

  return (
    <MarkdownLinks value={autolink}>
      <div className="flex min-h-0 flex-1 flex-col">
        <header className="glass-quiet shrink-0 border-b border-border px-5 pt-3 pb-0">
          <div className="flex items-start gap-3">
            <Avatar src={item.author.avatarUrl} name={item.author.name} className="mt-0.5 size-7" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
                <ProviderIcon kind={item.provider} className="size-3 opacity-70" />
                <span className="truncate font-medium">{item.repo}</span>
                <span className="opacity-60">#{item.number}</span>
                <span className="opacity-50">·</span>
                <span>
                  {item.author.name} opened {relativeTime(item.createdAt)}
                </span>
              </div>
              <h1 className="mt-0.5 text-[15px] leading-snug font-semibold">{item.title}</h1>

              <div className="mt-2 flex flex-wrap items-center gap-1.5">
                <CheckPill checks={item.checks} />
                <Badge>
                  <GitBranch className="size-3" />
                  <span className="mono !text-[10.5px]">
                    {item.sourceBranch} → {item.targetBranch}
                  </span>
                </Badge>
                {item.draft && <Badge tone="info">Draft</Badge>}
                {item.labels.slice(0, 4).map((label) => (
                  <Badge key={label}>{label}</Badge>
                ))}
              </div>
            </div>

            <div className="flex shrink-0 items-center gap-1">
              <Tooltip label="Copy a prompt for your terminal" side="bottom">
                <Button variant="ghost" size="sm" onClick={copyAgentPrompt}>
                  <ClipboardCopy className="size-3.5" />
                  Copy Claude prompt
                </Button>
              </Tooltip>
              <Button variant="ghost" size="sm" onClick={() => openExternal(item.url)}>
                <ExternalLink className="size-3.5" />
                Open
              </Button>
            </div>
          </div>

          <nav className="mt-3 flex items-center gap-1" role="tablist">
            <TabButton active={tab === 'diff'} onClick={() => setTab('diff')}>
              <FileDiff className="size-3.5" />
              Files
              {detail && <Count>{detail.files.length}</Count>}
            </TabButton>
            <TabButton active={tab === 'checks'} onClick={() => setTab('checks')}>
              <Check className="size-3.5" />
              Checks
              {item.checks.total > 0 && <Count>{item.checks.total}</Count>}
            </TabButton>
            <TabButton active={tab === 'conversation'} onClick={() => setTab('conversation')}>
              <MessageSquare className="size-3.5" />
              Conversation
              {conversation.length > 0 && <Count>{conversation.length}</Count>}
            </TabButton>

            {tab === 'diff' && (
              <div className="mb-1 ml-auto flex items-center gap-0.5">
                <Tooltip label="Side by side" side="bottom">
                  <Button
                    variant={settings.diffView === 'split' ? 'subtle' : 'ghost'}
                    size="icon"
                    aria-label="Side by side"
                    onClick={() => void updateSettings({ diffView: 'split' })}
                  >
                    <Columns2 className="size-4" />
                  </Button>
                </Tooltip>
                <Tooltip label="Unified" side="bottom">
                  <Button
                    variant={settings.diffView === 'unified' ? 'subtle' : 'ghost'}
                    size="icon"
                    aria-label="Unified"
                    onClick={() => void updateSettings({ diffView: 'unified' })}
                  >
                    <Rows3 className="size-4" />
                  </Button>
                </Tooltip>
              </div>
            )}
          </nav>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {loading && (
            <div className="flex h-full items-center justify-center gap-2 text-muted-foreground">
              <Loader2 className="spin size-4" />
              <span className="text-[13px]">Loading the diff…</span>
            </div>
          )}

          {error && !loading && (
            <div className="flex h-full flex-col items-center justify-center gap-3 px-8 text-center">
              <p className="max-w-md text-[13px] text-bad">{error}</p>
              <Button size="sm" onClick={() => openExternal(item.url)}>
                Open in browser instead
              </Button>
            </div>
          )}

          {detail && !loading && !error && (
            <>
              {tab === 'diff' && (
                <DiffView
                  files={detail.files}
                  threads={detail.threads}
                  mode={settings.diffView}
                  onComment={addLineComment}
                  onReply={replyToThread}
                  onResolve={resolveThread}
                />
              )}
              {tab === 'checks' && <ChecksPanel checks={item.checks} onOpen={openExternal} />}
              {tab === 'conversation' && (
                <div className="flex flex-col gap-3 p-4">
                  {detail.description.trim() && (
                    <article className="glass rounded-lg p-3.5">
                      <p className="mb-1.5 text-[11.5px] font-semibold text-muted-foreground">
                        {item.author.name} wrote
                      </p>
                      <Markdown>{detail.description}</Markdown>
                    </article>
                  )}

                  {conversation.map((thread) => (
                    <ThreadCard
                      key={thread.id}
                      thread={thread}
                      onReply={replyToThread}
                      onResolve={resolveThread}
                    />
                  ))}

                  {!conversation.length && !detail.description.trim() && (
                    <p className="py-10 text-center text-[13px] text-muted-foreground">
                      No conversation yet.
                    </p>
                  )}

                  {inlineCount > 0 && (
                    <p className="text-center text-[11.5px] text-muted-foreground">
                      {inlineCount} inline thread{inlineCount === 1 ? '' : 's'} shown on the Files tab.
                    </p>
                  )}
                </div>
              )}
            </>
          )}
        </div>

        <footer className="glass-quiet shrink-0 border-t border-border p-3">
          <Textarea
            rows={verdict || body ? 3 : 1}
            value={body}
            placeholder={
              verdict === 'approve'
                ? 'Optional note with your approval…'
                : verdict === 'request_changes'
                  ? 'What needs to change?'
                  : 'Leave a comment on this pull request…'
            }
            onChange={(event) => setBody(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && canSubmit) void submit()
            }}
          />
          <div className="mt-2 flex items-center gap-1.5">
            <VerdictButton
              active={verdict === 'approve'}
              variant="success"
              onClick={() => setVerdict(verdict === 'approve' ? null : 'approve')}
            >
              <Check className="size-3.5" />
              Approve
            </VerdictButton>
            <VerdictButton
              active={verdict === 'request_changes'}
              variant="danger"
              onClick={() => setVerdict(verdict === 'request_changes' ? null : 'request_changes')}
            >
              <X className="size-3.5" />
              Request changes
            </VerdictButton>

            <span className="ml-auto text-[10.5px] text-muted-foreground">⌘↵</span>
            <Button
              variant="default"
              size="sm"
              disabled={!canSubmit}
              onClick={() => void submit()}
            >
              {submitting ? <Loader2 className="spin size-3.5" /> : <Send className="size-3.5" />}
              {verdict === 'approve'
                ? 'Approve'
                : verdict === 'request_changes'
                  ? 'Request changes'
                  : 'Comment'}
            </Button>
          </div>
        </footer>
      </div>
    </MarkdownLinks>
  )
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={cn(
        'flex items-center gap-1.5 rounded-t-md border-b-2 px-2.5 pt-1.5 pb-2 text-[12.5px] font-medium transition-colors',
        active
          ? 'border-foreground text-foreground'
          : 'border-transparent text-muted-foreground hover:text-foreground',
      )}
    >
      {children}
    </button>
  )
}

function Count({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="rounded bg-muted px-1 py-px text-[10.5px] tabular-nums text-muted-foreground">
      {children}
    </span>
  )
}

function VerdictButton({
  active,
  variant,
  onClick,
  children,
}: {
  active: boolean
  variant: 'success' | 'danger'
  onClick: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <Button
      size="sm"
      variant={active ? variant : 'ghost'}
      onClick={onClick}
      aria-pressed={active}
      className={cn(active && 'ring-1 ring-current/25')}
    >
      {children}
    </Button>
  )
}
