import { useState } from 'react'
import { Check, CornerDownRight, Loader2, RotateCcw } from 'lucide-react'
import type { CommentThread } from '@shared/types'
import { cn, relativeTime } from '@/lib/utils'
import { Avatar } from './ui/avatar'
import { Badge } from './ui/badge'
import { Button } from './ui/button'
import { Textarea } from './ui/input'
import { Markdown } from './Markdown'

interface ThreadCardProps {
  thread: CommentThread
  /** Tighter typography and spacing, for a thread sitting inside a diff row. */
  dense?: boolean
  onReply: (threadId: string, body: string) => Promise<void>
  onResolve: (threadId: string, resolved: boolean) => Promise<void>
}

/**
 * One conversation, wherever it appears.
 *
 * What is on offer comes from the thread's own capability flags, never from which
 * host it came from - so a reply box only exists where a reply can actually be
 * sent, and the resolve control only where the host can resolve.
 */
export function ThreadCard({
  thread,
  dense,
  onReply,
  onResolve,
}: ThreadCardProps): React.JSX.Element {
  const [replying, setReplying] = useState(false)
  const [busy, setBusy] = useState(false)

  const [opening, ...replies] = thread.comments

  const toggleResolved = async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    try {
      await onResolve(thread.id, !thread.resolved)
    } catch {
      /* the caller has already surfaced it */
    } finally {
      setBusy(false)
    }
  }

  return (
    <article
      className={cn(
        dense ? 'glass-quiet m-1.5 rounded-md px-2.5 py-2 font-sans' : 'glass rounded-lg p-3',
        // Open questions are what should catch the eye first.
        thread.resolved && 'opacity-60',
      )}
    >
      {(thread.resolved || thread.outdated || (thread.path && !dense)) && (
        <div className="mb-1.5 flex flex-wrap items-center gap-1.5">
          {thread.resolved && (
            <Badge tone="ok">
              <Check className="size-3" />
              Resolved
            </Badge>
          )}
          {thread.outdated && <Badge tone="busy">Outdated</Badge>}
          {/*
            * Away from the diff, the file is the only thing placing the thread -
            * and an outdated one carries no line, deliberately, because the line it
            * was left on means something else in the diff as it stands now.
            */}
          {thread.path && !dense && (
            <span className="mono truncate !text-[11px] text-muted-foreground">
              {thread.path}
              {thread.line !== undefined && `:${thread.line}`}
            </span>
          )}
        </div>
      )}

      {opening && <Comment comment={opening} dense={dense} />}

      {replies.length > 0 && (
        <div className="mt-2 flex flex-col gap-2 border-l-2 border-border pl-2.5">
          {replies.map((reply) => (
            <Comment key={reply.id} comment={reply} dense={dense} />
          ))}
        </div>
      )}

      {(thread.canReply || thread.canResolve) && (
        <div className="mt-2 flex items-center gap-1.5">
          {thread.canReply && !replying && (
            <Button size="sm" variant="ghost" onClick={() => setReplying(true)}>
              <CornerDownRight className="size-3.5" />
              Reply
            </Button>
          )}
          {thread.canResolve && (
            <Button
              size="sm"
              variant="ghost"
              className="ml-auto"
              disabled={busy}
              onClick={() => void toggleResolved()}
            >
              {busy ? (
                <Loader2 className="spin size-3.5" />
              ) : thread.resolved ? (
                <RotateCcw className="size-3.5" />
              ) : (
                <Check className="size-3.5" />
              )}
              {thread.resolved ? 'Reopen' : 'Resolve'}
            </Button>
          )}
        </div>
      )}

      {replying && (
        <ReplyBox
          onCancel={() => setReplying(false)}
          onSubmit={async (body) => {
            await onReply(thread.id, body)
            setReplying(false)
          }}
        />
      )}
    </article>
  )
}

function Comment({
  comment,
  dense,
}: {
  comment: CommentThread['comments'][number]
  dense?: boolean
}): React.JSX.Element {
  return (
    <div className="flex gap-2.5">
      <Avatar
        src={comment.author.avatarUrl}
        name={comment.author.name}
        className={dense ? 'size-5' : undefined}
      />
      <div className="min-w-0 flex-1">
        <p className={dense ? 'text-[11.5px]' : 'text-[12px]'}>
          <span className="font-semibold">{comment.author.name}</span>{' '}
          <span className="text-muted-foreground">{relativeTime(comment.createdAt)}</span>
        </p>
        <Markdown compact={dense} className="mt-1">
          {comment.body}
        </Markdown>
      </div>
    </div>
  )
}

function ReplyBox({
  onSubmit,
  onCancel,
}: {
  onSubmit: (body: string) => Promise<void>
  onCancel: () => void
}): React.JSX.Element {
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState(false)

  const send = async (): Promise<void> => {
    if (!body.trim() || busy) return
    setBusy(true)
    try {
      await onSubmit(body.trim())
    } catch {
      /* the caller has already surfaced it; keep what was typed */
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mt-2">
      <Textarea
        autoFocus
        rows={3}
        value={body}
        placeholder="Reply to this thread…"
        onChange={(event) => setBody(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') onCancel()
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) void send()
        }}
      />
      <div className="mt-1.5 flex items-center justify-end gap-1.5">
        <span className="mr-auto text-[10.5px] text-muted-foreground">⌘↵ to send</span>
        <Button size="sm" variant="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
        <Button
          size="sm"
          variant="default"
          onClick={() => void send()}
          disabled={!body.trim() || busy}
        >
          {busy ? 'Sending…' : 'Reply'}
        </Button>
      </div>
    </div>
  )
}
