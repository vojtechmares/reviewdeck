/**
 * The handoff to the user's coding agent.
 *
 * Nothing here spawns anything: the app builds a command as text and puts it on
 * the clipboard for the user to run in the terminal they already have open in that
 * repository. That keeps authentication, billing, binary resolution and command
 * injection out of the app entirely - and because it is the user's own interactive
 * shell that runs it, a shell alias works as a command name, which it could not if
 * the app spawned the process itself.
 *
 * The payload is instruction text. It assumes the repository is the working
 * directory and deliberately does not carry the diff.
 */

import { DEFAULT_AGENT_COMMAND } from './types.ts'
import type { Account, CommentThread, ProviderKind, ReviewItem, Settings } from './types.ts'

/**
 * How each host exposes a pull request as a ref under `origin`. Bitbucket publishes
 * no such ref, so it falls back to the source branch - as does any host whose ref
 * shape we do not know.
 */
const FETCH_REFS: Record<ProviderKind, ((number: number) => string) | null> = {
  github: (number) => `pull/${number}/head`,
  forgejo: (number) => `pull/${number}/head`,
  gitlab: (number) => `merge-requests/${number}/head`,
  bitbucket: null,
}

/** A long bot comment must not turn a handoff into a wall of pasted text. */
const COMMENT_LIMIT = 240

export function fetchCommand(item: ReviewItem): string {
  const ref = FETCH_REFS[item.provider]?.(item.number)
  return `git fetch origin ${ref ?? item.sourceBranch}`
}

/** The account's override when it has one, otherwise the setting, otherwise Claude Code. */
export function agentCommandName(
  account: Pick<Account, 'agentCommand'> | undefined,
  settings: Pick<Settings, 'agentCommand'>,
): string {
  return account?.agentCommand?.trim() || settings.agentCommand?.trim() || DEFAULT_AGENT_COMMAND
}

/**
 * POSIX single-quoting, so the whole prompt survives being pasted into a shell as
 * one argument. This produces text; it does not run it.
 */
export function singleQuote(value: string): string {
  return `'${value.replaceAll("'", `'\\''`)}'`
}

function oneLine(body: string): string {
  const flat = body.replace(/\s+/g, ' ').trim()
  return flat.length > COMMENT_LIMIT ? `${flat.slice(0, COMMENT_LIMIT - 1)}…` : flat
}

function describeThread(thread: CommentThread): string[] {
  const where = thread.path
    ? `${thread.path}${thread.line === undefined ? '' : `:${thread.line}`}`
    : 'On the pull request itself'
  return [
    `- ${where}`,
    ...thread.comments.map((comment) => `  ${comment.author.name}: ${oneLine(comment.body)}`),
  ]
}

/**
 * The instruction text itself. Findings are asked for one strict
 * path-and-line-prefixed line at a time so the output is already machine-readable
 * the day an importer is added.
 */
export function agentPrompt(item: ReviewItem, threads: CommentThread[]): string {
  const open = threads.filter((thread) => !thread.resolved)

  const lines = [
    `Review the pull request "${item.title}" (${item.repo} #${item.number}).`,
    '',
    'Fetch it into the repository you are in:',
    '',
    `  ${fetchCommand(item)}`,
    '',
    `The change is then at FETCH_HEAD and targets ${item.targetBranch}, so the diff to review is:`,
    '',
    `  git diff origin/${item.targetBranch}...FETCH_HEAD`,
    '',
  ]

  if (open.length) {
    lines.push('Open threads, so you do not repeat what has already been said:', '')
    for (const thread of open) lines.push(...describeThread(thread))
    lines.push('')
  } else {
    lines.push('No open threads on it yet.', '')
  }

  lines.push(
    'Report every finding as exactly one line in this format, and output nothing else:',
    '',
    '  path/to/file.ext:LINE: what is wrong, and what to do about it',
    '',
    'If you find nothing worth raising, output exactly:',
    '',
    '  no findings',
  )

  return lines.join('\n')
}

/** The prompt with the resolved command name in front of it, ready to paste. */
export function agentCommand(
  item: ReviewItem,
  threads: CommentThread[],
  account: Pick<Account, 'agentCommand'> | undefined,
  settings: Pick<Settings, 'agentCommand'>,
): string {
  return `${agentCommandName(account, settings)} ${singleQuote(agentPrompt(item, threads))}`
}
