/**
 * The host conventions a pull request body is written in: `@someone`, `#123` and
 * `:tada:`.
 *
 * Everything here is a pure function over one string, because the transform that
 * uses them runs over markdown syntax-tree text nodes rather than over the raw
 * source. That distinction is the whole correctness story - a regular expression
 * over the source would happily linkify a reference sitting inside a code span,
 * which is exactly where a code sample discussing a number puts one.
 */

import type { ProviderKind, ReviewItem } from './types.ts'

/**
 * The path segment each host puts between a repository and a pull request number.
 * GitLab moved merge requests under `/-/` some versions ago, so both shapes are
 * recognised.
 */
const PULL_SEGMENTS: Record<ProviderKind, string[]> = {
  github: ['/pull/'],
  gitlab: ['/-/merge_requests/', '/merge_requests/'],
  forgejo: ['/pulls/'],
  bitbucket: ['/pull-requests/'],
}

/**
 * Where each host puts an issue, relative to the repository root, and where it
 * puts a profile, relative to the web root. Null where the host has no equivalent,
 * in which case the reference stays plain text rather than becoming a wrong link.
 *
 * Bitbucket has neither: its markdown does not treat `#123` as a reference at all,
 * and a mention there addresses an account id rather than the display name that
 * appears in the text.
 */
const LINK_PATHS: Record<ProviderKind, { issue: string | null; profile: boolean }> = {
  github: { issue: 'issues', profile: true },
  gitlab: { issue: '-/issues', profile: true },
  forgejo: { issue: 'issues', profile: true },
  bitbucket: { issue: null, profile: false },
}

/**
 * The repository's web root, recovered from the pull request's own web URL.
 *
 * Deliberately not the account's web root joined to the repository name: the
 * GitLab adapter falls back to a numeric project id for that field, so the naive
 * construction would build links that go nowhere. Returns null when the URL is not
 * the shape we expect, so a caller produces plain text rather than a wrong link.
 */
export function repositoryRoot(item: Pick<ReviewItem, 'provider' | 'url'>): string | null {
  for (const segment of PULL_SEGMENTS[item.provider] ?? []) {
    const at = item.url.lastIndexOf(segment)
    if (at > 0) return item.url.slice(0, at)
  }
  return null
}

export function mentionUrl(
  provider: ProviderKind,
  webUrl: string,
  handle: string,
): string | null {
  if (!LINK_PATHS[provider]?.profile || !webUrl) return null
  return `${webUrl.replace(/\/+$/, '')}/${handle}`
}

export function issueUrl(
  provider: ProviderKind,
  repoRoot: string | null,
  number: number,
): string | null {
  const path = LINK_PATHS[provider]?.issue
  if (!path || !repoRoot) return null
  return `${repoRoot.replace(/\/+$/, '')}/${path}/${number}`
}

export type AutolinkSpan =
  | { kind: 'mention'; start: number; end: number; text: string; handle: string }
  | { kind: 'issue'; start: number; end: number; text: string; number: number }

/**
 * A mention is `@` plus a handle, and a reference is `#` plus digits. Both have to
 * start at a boundary: without that, an email address would read as a mention and
 * a CSS colour as a reference.
 */
const AUTOLINK = /(^|[^\w/@#-])(?:@([A-Za-z0-9][A-Za-z0-9._-]*)|#(\d+))/g

export function autolinkSpans(value: string): AutolinkSpan[] {
  const spans: AutolinkSpan[] = []

  for (const match of value.matchAll(AUTOLINK)) {
    const lead = match[1] ?? ''
    const start = (match.index ?? 0) + lead.length
    const text = match[0].slice(lead.length)
    const end = start + text.length

    if (match[2] !== undefined) {
      // A trailing dot or dash is punctuation, not part of the handle.
      const handle = match[2].replace(/[._-]+$/, '')
      if (handle) spans.push({ kind: 'mention', start, end: start + 1 + handle.length, text: `@${handle}`, handle })
    } else if (match[3] !== undefined) {
      spans.push({ kind: 'issue', start, end, text, number: Number(match[3]) })
    }
  }

  return spans
}

/**
 * A curated table rather than the full set: these are the shortcodes that actually
 * turn up in pull requests, and carrying a few thousand entries to catch the rest
 * would cost more than it is worth. An unknown shortcode renders as it was typed.
 */
const EMOJI: Record<string, string> = {
  '+1': '👍',
  '-1': '👎',
  '100': '💯',
  ambulance: '🚑',
  art: '🎨',
  bell: '🔔',
  book: '📖',
  books: '📚',
  boom: '💥',
  bug: '🐛',
  bulb: '💡',
  cat: '🐱',
  checkered_flag: '🏁',
  clap: '👏',
  computer: '💻',
  confused: '😕',
  construction: '🚧',
  cry: '😢',
  dart: '🎯',
  eyes: '👀',
  fire: '🔥',
  green_heart: '💚',
  hammer: '🔨',
  heart: '❤️',
  heavy_check_mark: '✔️',
  hourglass: '⏳',
  joy: '😂',
  key: '🔑',
  lipstick: '💄',
  lock: '🔒',
  loud_sound: '🔊',
  mag: '🔍',
  memo: '📝',
  ok_hand: '👌',
  package: '📦',
  partying_face: '🥳',
  pencil: '✏️',
  pray: '🙏',
  question: '❓',
  raised_hands: '🙌',
  recycle: '♻️',
  rocket: '🚀',
  rotating_light: '🚨',
  see_no_evil: '🙈',
  shipit: '🚢',
  shrug: '🤷',
  smile: '😄',
  sob: '😭',
  sparkles: '✨',
  star: '⭐',
  tada: '🎉',
  test_tube: '🧪',
  thinking: '🤔',
  thumbsdown: '👎',
  thumbsup: '👍',
  truck: '🚚',
  warning: '⚠️',
  wave: '👋',
  white_check_mark: '✅',
  wrench: '🔧',
  x: '❌',
  zap: '⚡',
}

export function emojiFor(shortcode: string): string | null {
  return EMOJI[shortcode] ?? null
}

const SHORTCODE = /:([a-z0-9_+-]+):/g

/** Replaces the shortcodes we know; leaves the rest exactly as they were typed. */
export function renderEmoji(value: string): string {
  return value.replace(SHORTCODE, (whole, name: string) => emojiFor(name) ?? whole)
}
