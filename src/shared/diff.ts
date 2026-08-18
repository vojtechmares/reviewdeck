/**
 * Unified-diff parsing shared by every provider adapter and the diff viewer.
 *
 * Providers hand us diffs in two shapes: one blob covering every file
 * (Forgejo `.diff`, Bitbucket `/diff`) or a per-file patch (GitHub `files[].patch`,
 * GitLab `changes[].diff`). Both funnel through the same hunk parser.
 */

import type { DiffFile } from './types.ts'

export type DiffLineKind = 'context' | 'add' | 'del' | 'meta'

export interface DiffLine {
  kind: DiffLineKind
  content: string
  oldLine?: number
  newLine?: number
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
      current.lines.push({ kind: 'add', content, newLine })
      newLine++
    } else if (marker === '-') {
      current.lines.push({ kind: 'del', content, oldLine })
      oldLine++
    } else if (marker === ' ' || raw === '') {
      // A fully empty line inside a hunk is a context line whose content is empty.
      current.lines.push({ kind: 'context', content, oldLine, newLine })
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
