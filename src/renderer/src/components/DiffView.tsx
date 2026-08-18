import { Fragment, useMemo, useState } from 'react'
import { ChevronRight, FilePlus2, FileMinus2, FileSymlink, MessageSquarePlus, Plus } from 'lucide-react'
import { parsePatch, toSplitRows, type DiffHunk, type DiffLine } from '@shared/diff'
import type { DiffFile, LineCommentDraft, PullComment } from '@shared/types'
import { cn } from '@/lib/utils'
import { Avatar } from './ui/avatar'
import { Button } from './ui/button'
import { Textarea } from './ui/input'

/** Files past this many lines start collapsed so opening a big PR stays instant. */
const AUTO_COLLAPSE_LINES = 600

export interface CommentTarget {
  path: string
  newLine?: number
  oldLine?: number
}

interface DiffViewProps {
  files: DiffFile[]
  comments: PullComment[]
  mode: 'split' | 'unified'
  onComment: (draft: Omit<LineCommentDraft, 'itemId'>) => Promise<void>
}

export function DiffView({ files, comments, mode, onComment }: DiffViewProps): React.JSX.Element {
  const byPath = useMemo(() => {
    const map = new Map<string, PullComment[]>()
    for (const comment of comments) {
      if (!comment.path || comment.line === undefined) continue
      const list = map.get(comment.path) ?? []
      list.push(comment)
      map.set(comment.path, list)
    }
    return map
  }, [comments])

  if (!files.length) {
    return (
      <p className="px-5 py-10 text-center text-[13px] text-muted-foreground">
        No file changes to show.
      </p>
    )
  }

  return (
    <div className="flex flex-col gap-3 p-4">
      {files.map((file) => (
        <FileBlock
          key={`${file.oldPath}->${file.path}`}
          file={file}
          comments={byPath.get(file.path) ?? []}
          mode={mode}
          onComment={onComment}
        />
      ))}
    </div>
  )
}

function FileBlock({
  file,
  comments,
  mode,
  onComment,
}: {
  file: DiffFile
  comments: PullComment[]
  mode: 'split' | 'unified'
  onComment: DiffViewProps['onComment']
}): React.JSX.Element {
  const hunks = useMemo(() => parsePatch(file.patch ?? ''), [file.patch])
  const lineCount = useMemo(
    () => hunks.reduce((total, hunk) => total + hunk.lines.length, 0),
    [hunks],
  )
  const [open, setOpen] = useState(lineCount > 0 && lineCount <= AUTO_COLLAPSE_LINES)

  const Icon =
    file.status === 'added'
      ? FilePlus2
      : file.status === 'removed'
        ? FileMinus2
        : file.status === 'renamed'
          ? FileSymlink
          : null

  return (
    <section className="glass overflow-hidden rounded-lg">
      <header className="flex items-center gap-2 px-2.5 py-2">
        <button
          onClick={() => setOpen((value) => !value)}
          className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
          aria-expanded={open}
        >
          <ChevronRight
            className={cn(
              'size-3.5 shrink-0 text-muted-foreground transition-transform',
              open && 'rotate-90',
            )}
          />
          {Icon && <Icon className="size-3.5 shrink-0 text-muted-foreground" />}
          <span className="mono truncate !text-[12px] font-medium">
            {file.status === 'renamed' && file.oldPath !== file.path && (
              <span className="text-muted-foreground">{file.oldPath} → </span>
            )}
            {file.path}
          </span>
        </button>

        <span className="mono shrink-0 !text-[11px] tabular-nums">
          <span className="text-ok">+{file.additions}</span>{' '}
          <span className="text-bad">−{file.deletions}</span>
        </span>
      </header>

      {open && (
        <div className="border-t border-border">
          {file.binary || !file.patch ? (
            <p className="px-3 py-4 text-center text-[12px] text-muted-foreground">
              {file.binary ? 'Binary file - no diff to show.' : 'No diff available for this file.'}
            </p>
          ) : mode === 'split' ? (
            <SplitHunks hunks={hunks} path={file.path} comments={comments} onComment={onComment} />
          ) : (
            <UnifiedHunks hunks={hunks} path={file.path} comments={comments} onComment={onComment} />
          )}
        </div>
      )}

      {!open && lineCount > AUTO_COLLAPSE_LINES && (
        <p className="px-3 py-2 text-[11.5px] text-muted-foreground">
          {lineCount.toLocaleString()} changed lines - collapsed for speed.
        </p>
      )}
    </section>
  )
}

const CELL_BG: Record<string, string> = {
  add: 'bg-[var(--diff-add)]',
  del: 'bg-[var(--diff-del)]',
  context: '',
  meta: '',
}

function UnifiedHunks({
  hunks,
  path,
  comments,
  onComment,
}: {
  hunks: DiffHunk[]
  path: string
  comments: PullComment[]
  onComment: DiffViewProps['onComment']
}): React.JSX.Element {
  const [target, setTarget] = useState<CommentTarget | null>(null)

  return (
    <table className="mono w-full border-collapse">
      <tbody>
        {hunks.map((hunk, hunkIndex) => (
          <Fragment key={hunkIndex}>
            <tr>
              <td
                colSpan={3}
                className="bg-muted px-3 py-1 !text-[11px] text-muted-foreground select-none"
              >
                {hunk.header}
              </td>
            </tr>
            {hunk.lines.map((line, lineIndex) => {
              if (line.kind === 'meta') {
                return (
                  <tr key={lineIndex}>
                    <td colSpan={3} className="px-3 py-0.5 !text-[11px] text-muted-foreground italic">
                      {line.content}
                    </td>
                  </tr>
                )
              }
              const attached = comments.filter(
                (comment) => comment.line === (line.newLine ?? line.oldLine),
              )
              const isTarget = target !== null && sameLine(target, line)

              return (
                <Fragment key={lineIndex}>
                  <tr className={cn('group', CELL_BG[line.kind])}>
                    <Gutter value={line.oldLine} onAdd={() => setTarget(targetFor(path, line))} />
                    <Gutter value={line.newLine} />
                    <Code line={line} />
                  </tr>
                  {attached.map((comment) => (
                    <CommentRow key={comment.id} comment={comment} span={3} />
                  ))}
                  {isTarget && (
                    <tr>
                      <td colSpan={3} className="p-0">
                        <Composer
                          target={target}
                          onCancel={() => setTarget(null)}
                          onSubmit={async (body) => {
                            await onComment({ body, ...target })
                            setTarget(null)
                          }}
                        />
                      </td>
                    </tr>
                  )}
                </Fragment>
              )
            })}
          </Fragment>
        ))}
      </tbody>
    </table>
  )
}

function SplitHunks({
  hunks,
  path,
  comments,
  onComment,
}: {
  hunks: DiffHunk[]
  path: string
  comments: PullComment[]
  onComment: DiffViewProps['onComment']
}): React.JSX.Element {
  const [target, setTarget] = useState<CommentTarget | null>(null)

  return (
    <table className="mono w-full table-fixed border-collapse">
      <colgroup>
        <col className="w-11" />
        <col className="w-[calc(50%-2.75rem)]" />
        <col className="w-11" />
        <col className="w-[calc(50%-2.75rem)]" />
      </colgroup>
      <tbody>
        {hunks.map((hunk, hunkIndex) => (
          <Fragment key={hunkIndex}>
            <tr>
              <td
                colSpan={4}
                className="bg-muted px-3 py-1 !text-[11px] text-muted-foreground select-none"
              >
                {hunk.header}
              </td>
            </tr>
            {toSplitRows(hunk).map((row, rowIndex) => {
              const left = row.left
              const right = row.right
              // A context line occupies both sides; the same object is reused.
              const paired = left === right
              const attached = comments.filter(
                (comment) =>
                  (right && comment.line === right.newLine) ||
                  (!right && left && comment.line === left.oldLine),
              )
              const activeSide =
                target && ((right && target.newLine === right.newLine) || (left && target.oldLine === left.oldLine))

              return (
                <Fragment key={rowIndex}>
                  <tr className="group">
                    <Gutter
                      value={left?.oldLine}
                      className={left && !paired ? 'bg-[var(--diff-del)]' : ''}
                      onAdd={left ? () => setTarget(targetFor(path, left, 'old')) : undefined}
                    />
                    <Code line={left} className={left && !paired ? 'bg-[var(--diff-del)]' : ''} />
                    <Gutter
                      value={right?.newLine}
                      className={right && !paired ? 'bg-[var(--diff-add)]' : ''}
                      onAdd={right ? () => setTarget(targetFor(path, right, 'new')) : undefined}
                    />
                    <Code line={right} className={right && !paired ? 'bg-[var(--diff-add)]' : ''} />
                  </tr>
                  {attached.map((comment) => (
                    <CommentRow key={comment.id} comment={comment} span={4} />
                  ))}
                  {activeSide && target && (
                    <tr>
                      <td colSpan={4} className="p-0">
                        <Composer
                          target={target}
                          onCancel={() => setTarget(null)}
                          onSubmit={async (body) => {
                            await onComment({ body, ...target })
                            setTarget(null)
                          }}
                        />
                      </td>
                    </tr>
                  )}
                </Fragment>
              )
            })}
          </Fragment>
        ))}
      </tbody>
    </table>
  )
}

function Gutter({
  value,
  onAdd,
  className,
}: {
  value?: number
  onAdd?: () => void
  className?: string
}): React.JSX.Element {
  return (
    <td
      className={cn(
        'relative w-11 border-r border-border/60 px-1.5 text-right align-top',
        '!text-[11px] text-[var(--diff-gutter)] tabular-nums select-none',
        className,
      )}
    >
      {value ?? ''}
      {onAdd && value !== undefined && (
        <button
          onClick={onAdd}
          aria-label={`Comment on line ${value}`}
          className={cn(
            'absolute top-1/2 -left-0.5 hidden size-4 -translate-y-1/2 items-center justify-center',
            'rounded bg-info text-white shadow-sm group-hover:flex hover:brightness-110',
          )}
        >
          <Plus className="size-3" />
        </button>
      )}
    </td>
  )
}

function Code({ line, className }: { line?: DiffLine; className?: string }): React.JSX.Element {
  if (!line) return <td className={cn('align-top', className)} />
  return (
    <td className={cn('px-2 align-top whitespace-pre-wrap', className)}>
      <span className="mr-1 inline-block w-2 shrink-0 text-[var(--diff-gutter)] select-none">
        {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ' '}
      </span>
      {line.content || ' '}
    </td>
  )
}

function CommentRow({ comment, span }: { comment: PullComment; span: number }): React.JSX.Element {
  return (
    <tr>
      <td colSpan={span} className="p-0">
        <div className="glass-quiet m-1.5 flex gap-2 rounded-md px-2.5 py-2">
          <Avatar src={comment.author.avatarUrl} name={comment.author.name} className="size-5" />
          <div className="min-w-0 flex-1 font-sans">
            <p className="text-[11.5px] font-semibold">{comment.author.name}</p>
            <p className="mt-0.5 text-[12px] leading-snug whitespace-pre-wrap">{comment.body}</p>
          </div>
        </div>
      </td>
    </tr>
  )
}

function Composer({
  target,
  onSubmit,
  onCancel,
}: {
  target: CommentTarget
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
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="glass-quiet m-1.5 rounded-md p-2 font-sans">
      <p className="mb-1.5 flex items-center gap-1.5 text-[11px] text-muted-foreground">
        <MessageSquarePlus className="size-3.5" />
        Commenting on line {target.newLine ?? target.oldLine} of{' '}
        <span className="mono !text-[11px]">{target.path}</span>
      </p>
      <Textarea
        autoFocus
        rows={3}
        value={body}
        placeholder="Leave a note on this line…"
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
        <Button size="sm" variant="default" onClick={() => void send()} disabled={!body.trim() || busy}>
          {busy ? 'Sending…' : 'Comment'}
        </Button>
      </div>
    </div>
  )
}

/**
 * GitHub/GitLab both want "which side of the diff" expressed as which line
 * number is present: added lines carry only a new line, removed only an old one.
 */
function targetFor(path: string, line: DiffLine, prefer?: 'old' | 'new'): CommentTarget {
  if (line.kind === 'add') return { path, newLine: line.newLine }
  if (line.kind === 'del') return { path, oldLine: line.oldLine }
  // Context lines exist on both sides; use whichever column was clicked.
  if (prefer === 'old') return { path, oldLine: line.oldLine }
  return { path, newLine: line.newLine }
}

/** Does an open composer belong to this exact line? */
function sameLine(target: CommentTarget, line: DiffLine): boolean {
  if (target.newLine !== undefined) return target.newLine === line.newLine
  if (target.oldLine !== undefined) return target.oldLine === line.oldLine
  return false
}
