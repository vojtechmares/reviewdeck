import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { ChevronRight, FilePlus2, FileMinus2, FileSymlink, MessageSquarePlus, Plus } from 'lucide-react'
import { parsePatch, toSplitRows, type DiffHunk, type DiffLine } from '@shared/diff'
import { languageFor } from '@shared/highlight'
import type { CommentThread, DiffFile, LineCommentDraft } from '@shared/types'
import { highlightHunks, type Token } from '@/lib/highlight'
import { cn } from '@/lib/utils'
import { Button } from './ui/button'
import { Textarea } from './ui/input'
import { ThreadCard } from './Thread'

/** Files past this many lines start collapsed so opening a big PR stays instant. */
const AUTO_COLLAPSE_LINES = 600

/** How far outside the viewport a file section starts rendering its rows. */
const WINDOW_MARGIN = '700px'

/**
 * Sections rendered without waiting for the observer, so opening a diff never
 * flashes empty boxes where the first screenful should be.
 */
const EAGER_FILES = 2

/**
 * Row heights used to size a section that has never been on screen. Only a guess -
 * a section is measured the first time it renders, and the measurement is what
 * holds the scroll position from then on.
 */
const ESTIMATED_ROW_HEIGHT = 19
const ESTIMATED_HUNK_HEADER_HEIGHT = 27

/** Shared so a file without threads keeps the same array across renders. */
const NO_THREADS: CommentThread[] = []

export interface CommentTarget {
  path: string
  newLine?: number
  oldLine?: number
}

interface DiffViewProps {
  files: DiffFile[]
  threads: CommentThread[]
  mode: 'split' | 'unified'
  onComment: (draft: Omit<LineCommentDraft, 'itemId'>) => Promise<void>
  onReply: (threadId: string, body: string) => Promise<void>
  onResolve: (threadId: string, resolved: boolean) => Promise<void>
}

export function DiffView({
  files,
  threads,
  mode,
  onComment,
  onReply,
  onResolve,
}: DiffViewProps): React.JSX.Element {
  const byPath = useMemo(() => {
    const map = new Map<string, CommentThread[]>()
    for (const thread of threads) {
      if (!thread.path || thread.line === undefined) continue
      const list = map.get(thread.path) ?? []
      list.push(thread)
      map.set(thread.path, list)
    }
    return map
  }, [threads])

  if (!files.length) {
    return (
      <p className="px-5 py-10 text-center text-[13px] text-muted-foreground">
        No file changes to show.
      </p>
    )
  }

  return (
    <div className="flex flex-col gap-3 p-4">
      {files.map((file, index) => (
        <FileBlock
          key={`${file.oldPath}->${file.path}`}
          file={file}
          index={index}
          threads={byPath.get(file.path) ?? NO_THREADS}
          mode={mode}
          onComment={onComment}
          onReply={onReply}
          onResolve={onResolve}
        />
      ))}
    </div>
  )
}

/**
 * Whether an element is close enough to the viewport to be worth rendering.
 *
 * The observer accounts for the clipping the scroll container does, so watching
 * the viewport is the same as watching the container - and saves threading a root
 * reference down through the tree.
 */
function useNearViewport(ref: RefObject<Element | null>, initial: boolean): boolean {
  const [near, setNear] = useState(initial)

  useEffect(() => {
    const element = ref.current
    if (!element) return
    const observer = new IntersectionObserver(
      ([entry]) => setNear(entry.isIntersecting),
      { rootMargin: `${WINDOW_MARGIN} 0px` },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [ref])

  return near
}

/**
 * Tokens for a file's hunks, once it is on screen and once they arrive.
 *
 * Deliberately after the fact: the diff renders plain and gains colour a moment
 * later, so loading a grammar never stands between the reader and the code.
 */
function useHighlight(hunks: DiffHunk[] | null, path: string): Map<DiffLine, Token[]> | null {
  const [tokens, setTokens] = useState<Map<DiffLine, Token[]> | null>(null)
  const language = useMemo(() => languageFor(path), [path])

  useEffect(() => {
    setTokens(null)
    if (!hunks?.length || !language) return

    let cancelled = false
    void highlightHunks(hunks, language).then((result) => {
      if (!cancelled) setTokens(result)
    })
    return () => {
      cancelled = true
    }
  }, [hunks, language])

  return tokens
}

function FileBlock({
  file,
  index,
  threads,
  mode,
  onComment,
  onReply,
  onResolve,
}: {
  file: DiffFile
  index: number
  threads: CommentThread[]
  mode: 'split' | 'unified'
  onComment: DiffViewProps['onComment']
  onReply: DiffViewProps['onReply']
  onResolve: DiffViewProps['onResolve']
}): React.JSX.Element {
  const hunks = useMemo(() => parsePatch(file.patch ?? ''), [file.patch])
  const lineCount = useMemo(
    () => hunks.reduce((total, hunk) => total + hunk.lines.length, 0),
    [hunks],
  )
  const [open, setOpen] = useState(lineCount > 0 && lineCount <= AUTO_COLLAPSE_LINES)

  // Lifted out of the hunk tables: an open composer is what keeps a section
  // rendered, so a half-typed comment cannot be thrown away by a scroll.
  const [target, setTarget] = useState<CommentTarget | null>(null)

  const section = useRef<HTMLElement>(null)
  const rows = useRef<HTMLDivElement>(null)
  const [measuredHeight, setMeasuredHeight] = useState<number | null>(null)
  const near = useNearViewport(section, index < EAGER_FILES)

  // A binary file's one-line notice is not worth windowing.
  const windowed = !file.binary && hunks.length > 0
  const rendered = open && (!windowed || near || target !== null)

  const estimatedHeight =
    hunks.length * ESTIMATED_HUNK_HEADER_HEIGHT + lineCount * ESTIMATED_ROW_HEIGHT

  const tokens = useHighlight(rendered ? hunks : null, file.path)

  // The two layouts are different heights, so a measurement does not carry over.
  useLayoutEffect(() => setMeasuredHeight(null), [mode])

  useLayoutEffect(() => {
    if (!rendered) return
    // Fractional, not offsetHeight: rounding every section would drift the scroll
    // position by a pixel a time across a long diff.
    const height = rows.current?.getBoundingClientRect().height ?? 0
    if (height > 0) setMeasuredHeight((current) => (current === height ? current : height))
  }, [rendered, mode, hunks, threads, tokens])

  const Icon =
    file.status === 'added'
      ? FilePlus2
      : file.status === 'removed'
        ? FileMinus2
        : file.status === 'renamed'
          ? FileSymlink
          : null

  return (
    <section ref={section} className="glass overflow-hidden rounded-lg">
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
          {!rendered ? (
            <div aria-hidden style={{ height: measuredHeight ?? estimatedHeight }} />
          ) : (
            <div ref={rows}>
              {file.binary || !file.patch ? (
                <p className="px-3 py-4 text-center text-[12px] text-muted-foreground">
                  {file.binary
                    ? 'Binary file - no diff to show.'
                    : 'No diff available for this file.'}
                </p>
              ) : mode === 'split' ? (
                <SplitHunks
                  hunks={hunks}
                  path={file.path}
                  tokens={tokens}
                  threads={threads}
                  target={target}
                  setTarget={setTarget}
                  onComment={onComment}
                  onReply={onReply}
                  onResolve={onResolve}
                />
              ) : (
                <UnifiedHunks
                  hunks={hunks}
                  path={file.path}
                  tokens={tokens}
                  threads={threads}
                  target={target}
                  setTarget={setTarget}
                  onComment={onComment}
                  onReply={onReply}
                  onResolve={onResolve}
                />
              )}
            </div>
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

interface HunkTableProps {
  hunks: DiffHunk[]
  path: string
  tokens: Map<DiffLine, Token[]> | null
  threads: CommentThread[]
  target: CommentTarget | null
  setTarget: (target: CommentTarget | null) => void
  onComment: DiffViewProps['onComment']
  onReply: DiffViewProps['onReply']
  onResolve: DiffViewProps['onResolve']
}

function UnifiedHunks({
  hunks,
  path,
  tokens,
  threads,
  target,
  setTarget,
  onComment,
  onReply,
  onResolve,
}: HunkTableProps): React.JSX.Element {
  return (
    // Fixed layout, like the split view: without definite column widths a wide
    // table inside a comment would stretch the whole diff rather than scroll.
    <table className="mono w-full table-fixed border-collapse">
      <colgroup>
        <col className="w-11" />
        <col className="w-11" />
        <col />
      </colgroup>
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
              const attached = threads.filter(
                (thread) => thread.line === (line.newLine ?? line.oldLine),
              )
              const isTarget = target !== null && sameLine(target, line)

              return (
                <Fragment key={lineIndex}>
                  <tr className={cn('group', CELL_BG[line.kind])}>
                    <Gutter value={line.oldLine} onAdd={() => setTarget(targetFor(path, line))} />
                    <Gutter value={line.newLine} />
                    <Code line={line} tokens={tokens?.get(line)} />
                  </tr>
                  {attached.map((thread) => (
                    <ThreadRow
                      key={thread.id}
                      thread={thread}
                      span={3}
                      onReply={onReply}
                      onResolve={onResolve}
                    />
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
  tokens,
  threads,
  target,
  setTarget,
  onComment,
  onReply,
  onResolve,
}: HunkTableProps): React.JSX.Element {
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
              const attached = threads.filter(
                (thread) =>
                  (right && thread.line === right.newLine) ||
                  (!right && left && thread.line === left.oldLine),
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
                    <Code
                      line={left}
                      tokens={left ? tokens?.get(left) : undefined}
                      className={left && !paired ? 'bg-[var(--diff-del)]' : ''}
                    />
                    <Gutter
                      value={right?.newLine}
                      className={right && !paired ? 'bg-[var(--diff-add)]' : ''}
                      onAdd={right ? () => setTarget(targetFor(path, right, 'new')) : undefined}
                    />
                    <Code
                      line={right}
                      tokens={right ? tokens?.get(right) : undefined}
                      className={right && !paired ? 'bg-[var(--diff-add)]' : ''}
                    />
                  </tr>
                  {attached.map((thread) => (
                    <ThreadRow
                      key={thread.id}
                      thread={thread}
                      span={4}
                      onReply={onReply}
                      onResolve={onResolve}
                    />
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

function Code({
  line,
  tokens,
  className,
}: {
  line?: DiffLine
  tokens?: Token[]
  className?: string
}): React.JSX.Element {
  if (!line) return <td className={cn('align-top', className)} />
  return (
    // Both layouts fix their column widths, so a long unbroken line has to wrap
    // rather than run out past the edge of the diff and be clipped.
    <td className={cn('px-2 align-top break-words whitespace-pre-wrap', className)}>
      <span className="mr-1 inline-block w-2 shrink-0 text-[var(--diff-gutter)] select-none">
        {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ' '}
      </span>
      {tokens?.length
        ? tokens.map((token, index) => (
            <span key={index} className="tok" style={token.style as React.CSSProperties}>
              {token.content}
            </span>
          ))
        : line.content || ' '}
    </td>
  )
}

function ThreadRow({
  thread,
  span,
  onReply,
  onResolve,
}: {
  thread: CommentThread
  span: number
  onReply: DiffViewProps['onReply']
  onResolve: DiffViewProps['onResolve']
}): React.JSX.Element {
  return (
    <tr>
      <td colSpan={span} className="p-0">
        <ThreadCard thread={thread} dense onReply={onReply} onResolve={onResolve} />
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
