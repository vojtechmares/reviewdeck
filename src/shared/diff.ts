/**
 * Unified-diff parsing shared by every provider adapter and the diff viewer.
 *
 * Providers hand us diffs in two shapes: one blob covering every file
 * (Forgejo `.diff`, Bitbucket `/diff`) or a per-file patch (GitHub `files[].patch`,
 * GitLab `changes[].diff`). Both funnel through the same hunk parser.
 */

import type { DiffFile, LineRange, RangeEdge } from './types.ts'

export type DiffLineKind = 'context' | 'add' | 'del' | 'meta'

export interface DiffLine {
  kind: DiffLineKind
  content: string
  /** Set only on the sides the line exists on: both for context, one for a change. */
  oldLine?: number
  newLine?: number
  /**
   * Where the line sits in each file as the diff counts it, set on every line but
   * an annotation. Unlike the two above these never go missing: an added line
   * carries the old-file number it was inserted ahead of, a removed line the
   * new-file number that follows it. See `RangeEdge`.
   */
  oldPos?: number
  newPos?: number
}

export interface DiffHunk {
  header: string
  oldStart: number
  oldCount: number
  newStart: number
  newCount: number
  lines: DiffLine[]
}

const HUNK_RE = /^@@+ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@+(.*)$/

/** Parse a single file's patch body into hunks. */
export function parsePatch(patch: string): DiffHunk[] {
  if (!patch) return []
  const hunks: DiffHunk[] = []
  let current: DiffHunk | null = null
  let oldLine = 0
  let newLine = 0

  for (const raw of patch.split('\n')) {
    const match = HUNK_RE.exec(raw)
    if (match) {
      current = {
        header: raw,
        oldStart: Number(match[1]),
        oldCount: match[2] === undefined ? 1 : Number(match[2]),
        newStart: Number(match[3]),
        newCount: match[4] === undefined ? 1 : Number(match[4]),
        lines: [],
      }
      oldLine = current.oldStart
      newLine = current.newStart
      hunks.push(current)
      continue
    }
    if (!current) continue

    // "\ No newline at end of file" annotates the previous line rather than being one.
    if (raw.startsWith('\\')) {
      current.lines.push({ kind: 'meta', content: raw.slice(1).trim() })
      continue
    }

    const marker = raw[0]
    const content = raw.slice(1)
    if (marker === '+') {
      current.lines.push({ kind: 'add', content, newLine, oldPos: oldLine, newPos: newLine })
      newLine++
    } else if (marker === '-') {
      current.lines.push({ kind: 'del', content, oldLine, oldPos: oldLine, newPos: newLine })
      oldLine++
    } else if (marker === ' ' || raw === '') {
      // A fully empty line inside a hunk is a context line whose content is empty.
      current.lines.push({ kind: 'context', content, oldLine, newLine, oldPos: oldLine, newPos: newLine })
      oldLine++
      newLine++
    }
    // Anything else (stray git noise) is skipped rather than corrupting line numbers.
  }
  return hunks
}

function stripPrefix(path: string): string {
  if (path === '/dev/null') return ''
  // git uses a/ and b/ prefixes, but honours -p0 style diffs too.
  return path.replace(/^[ab]\//, '')
}

/** Split a multi-file unified diff blob into per-file entries. */
export function parseUnifiedDiff(text: string): DiffFile[] {
  const files: DiffFile[] = []
  if (!text) return files

  const lines = text.split('\n')
  let i = 0

  while (i < lines.length) {
    if (!lines[i].startsWith('diff --git ') && !lines[i].startsWith('diff ')) {
      i++
      continue
    }

    const headerLine = lines[i]
    i++

    let oldPath = ''
    let newPath = ''
    let binary = false
    let added = false
    let removed = false
    let renamed = false
    const body: string[] = []

    // Consume the extended header block that precedes the first hunk.
    while (i < lines.length && !lines[i].startsWith('@@') && !lines[i].startsWith('diff ')) {
      const line = lines[i]
      if (line.startsWith('--- ')) oldPath = stripPrefix(line.slice(4).trim())
      else if (line.startsWith('+++ ')) newPath = stripPrefix(line.slice(4).trim())
      else if (line.startsWith('new file mode')) added = true
      else if (line.startsWith('deleted file mode')) removed = true
      else if (line.startsWith('rename from ')) {
        renamed = true
        oldPath = line.slice('rename from '.length).trim()
      } else if (line.startsWith('rename to ')) {
        renamed = true
        newPath = line.slice('rename to '.length).trim()
      } else if (line.startsWith('Binary files') || line.startsWith('GIT binary patch')) {
        binary = true
      }
      i++
    }

    // Collect the hunks until the next file header.
    while (i < lines.length && !lines[i].startsWith('diff ')) {
      body.push(lines[i])
      i++
    }

    // Fall back to the `diff --git a/x b/y` line when there were no ---/+++ markers.
    if (!oldPath && !newPath) {
      const match = /^diff --git ["']?a\/(.+?)["']? ["']?b\/(.+?)["']?$/.exec(headerLine)
      if (match) {
        oldPath = match[1]
        newPath = match[2]
      }
    }

    const path = newPath || oldPath
    if (!path) continue

    const patch = body.join('\n').trim() ? body.join('\n') : null
    const { additions, deletions } = countChanges(patch ?? '')

    files.push({
      path,
      oldPath: oldPath || path,
      status: added || !oldPath ? 'added' : removed || !newPath ? 'removed' : renamed ? 'renamed' : 'modified',
      additions,
      deletions,
      patch,
      binary,
    })
  }

  return files
}

export function countChanges(patch: string): { additions: number; deletions: number } {
  let additions = 0
  let deletions = 0
  for (const line of patch.split('\n')) {
    if (line.startsWith('+') && !line.startsWith('+++')) additions++
    else if (line.startsWith('-') && !line.startsWith('---')) deletions++
  }
  return { additions, deletions }
}

export interface SplitRow {
  left?: DiffLine
  right?: DiffLine
}

/**
 * Lay a hunk out for side-by-side viewing: context lines occupy both columns,
 * and consecutive runs of removals/additions are zipped so a rewritten line
 * lines up with its replacement.
 */
export function toSplitRows(hunk: DiffHunk): SplitRow[] {
  const rows: SplitRow[] = []
  let dels: DiffLine[] = []
  let adds: DiffLine[] = []

  const flush = (): void => {
    const max = Math.max(dels.length, adds.length)
    for (let i = 0; i < max; i++) rows.push({ left: dels[i], right: adds[i] })
    dels = []
    adds = []
  }

  for (const line of hunk.lines) {
    if (line.kind === 'del') dels.push(line)
    else if (line.kind === 'add') adds.push(line)
    else if (line.kind === 'meta') continue
    else {
      flush()
      rows.push({ left: line, right: line })
    }
  }
  flush()
  return rows
}

/**
 * Where a line comment lands: the file, the line on whichever side it is on, and
 * the lines above it that it also covers, if any.
 */
export interface CommentTarget {
  path: string
  newLine?: number
  oldLine?: number
  range?: LineRange
}

export type DiffSide = 'old' | 'new'

/** The number a line has on one side, or nothing when it is not on that side. */
export function lineOn(line: DiffLine, side: DiffSide): number | undefined {
  return side === 'new' ? line.newLine : line.oldLine
}

function edge(line: DiffLine): RangeEdge | undefined {
  if (line.kind === 'meta' || line.oldPos === undefined || line.newPos === undefined) return undefined
  return { kind: line.kind, oldPos: line.oldPos, newPos: line.newPos }
}

/**
 * The comment covering every line from one to another on one side of a hunk, in
 * whichever order the two were picked - or nothing when the pair cannot make one.
 *
 * Both lines have to be on the side asked for, and both in the same hunk: the
 * lines between two hunks are not in the diff, and a host asked to cover them
 * refuses the whole comment. The same line twice is an ordinary single-line
 * comment, which is what a drag that never left its row should produce.
 */
export function rangeTarget(
  path: string,
  hunk: DiffHunk,
  side: DiffSide,
  first: DiffLine,
  second: DiffLine,
): CommentTarget | undefined {
  if (!hunk.lines.includes(first) || !hunk.lines.includes(second)) return undefined
  const a = lineOn(first, side)
  const b = lineOn(second, side)
  if (a === undefined || b === undefined) return undefined

  const [from, to] = a <= b ? [first, second] : [second, first]
  const last = sideTarget(path, to, side)
  if (!last) return undefined
  if (from === to) return last

  const start = edge(from)
  const end = edge(to)
  if (!start || !end) return undefined
  return { ...last, range: { startLine: lineOn(from, side)!, start, end } }
}

/** The comment this line takes from one gutter, or nothing when it has no line there. */
export function sideTarget(path: string, line: DiffLine, side: DiffSide): CommentTarget | undefined {
  return commentTargets(path, line).find((target) =>
    side === 'old' ? target.oldLine !== undefined : target.newLine !== undefined,
  )
}

/**
 * Whether a comment standing on `last` and reaching back to `startLine` covers a
 * line - by number on that side, which is all a range is once it has left the
 * diff it was drawn on.
 */
export function coversLine(
  anchor: { side: DiffSide; line: number; startLine?: number },
  line: DiffLine,
): boolean {
  const number = lineOn(line, anchor.side)
  if (number === undefined) return false
  return number <= anchor.line && number >= (anchor.startLine ?? anchor.line)
}

/**
 * The comments a diff line can take, one per side it exists on.
 *
 * Every host addresses a line by which side's number is given: an added line
 * carries only a new line number, a removed line only an old one, and a context
 * line has both - so it can be commented on from either side.
 */
export function commentTargets(path: string, line: DiffLine): CommentTarget[] {
  if (line.kind === 'add') return [{ path, newLine: line.newLine }]
  if (line.kind === 'del') return [{ path, oldLine: line.oldLine }]
  if (line.kind === 'context') return [{ path, oldLine: line.oldLine }, { path, newLine: line.newLine }]
  return []
}
