/**
 * Review windows: the recurring stretches of the day the app is allowed to
 * interrupt in.
 *
 * Outside every window it stays quiet and lets reviews pile up. When a window
 * opens it raises one roll-up of everything waiting, and for the rest of that span
 * it pings live as arrivals land, exactly as the app behaves with no schedule at
 * all - which is what an empty list means, so this costs nothing to anyone who
 * never opens the schedule.
 *
 * A window may also name the accounts it covers, and that one field turns "is the
 * app quiet" into a question about each review rather than about the app. A review
 * no enabled window covers pings live exactly as it did before any of this existed,
 * which is the safest way for this to fail: there is no way to configure yourself
 * into permanent silence, and windows only ever silence what they claim.
 *
 * Every decision here reads the current local wall clock rather than a fire time
 * worked out in advance. That is what makes the end time carry its weight: a lid
 * that opens at 09:12 is still inside a 09:00-09:30 span and fires then, one that
 * opens at 14:00 has missed the morning and stays silent, and daylight saving and
 * travel need no special case because there is nothing precomputed to go stale.
 */

import type { ReviewWindow } from './types.ts'

/** All this module needs of a review: the account it came in on. */
interface Scoped {
  accountId: string
}

/** Twice a minute, because a three-minute sync would let a 09:00 span open at 09:02. */
export const WINDOW_TICK_MS = 30_000

function pad(value: number): string {
  return String(value).padStart(2, '0')
}

/** Minutes since local midnight, or null when the text is not a wall clock. */
export function minutesOfDay(time: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time.trim())
  if (!match) return null
  const hours = Number(match[1])
  const minutes = Number(match[2])
  if (hours > 23 || minutes > 59) return null
  return hours * 60 + minutes
}

/** The local calendar day, which is the key the once-a-day guarantee turns on. */
export function localDay(now: Date): string {
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

/**
 * A window's span in minutes since midnight, or null when it does not describe
 * one: unreadable times, or an end that fails to come after its start. A window
 * may not cross midnight - anyone wanting 22:00 to 02:00 makes two - and refusing
 * to read one that tries is what lets everything downstream be a plain comparison.
 */
function spanOf(window: ReviewWindow): { start: number; end: number } | null {
  const start = minutesOfDay(window.start)
  const end = minutesOfDay(window.end)
  if (start === null || end === null || end <= start) return null
  return { start, end }
}

/**
 * Whether this window claims an account.
 *
 * Naming none claims them all, which is also what a window left scoped to accounts
 * that have since been signed out reduces to: it names some, matches none of the
 * ones still here, and so covers nothing and can never fire. That is shown rather
 * than tidied away, because silently widening it back to everything would start
 * interrupting about accounts nobody asked it to watch.
 */
export function windowCovers(window: ReviewWindow, accountId: string): boolean {
  return !window.accounts.length || window.accounts.includes(accountId)
}

/** The reviews a window is responsible for, which is what it counts and summarises. */
export function reviewsInScope<T extends Scoped>(window: ReviewWindow, reviews: T[]): T[] {
  return reviews.filter((review) => windowCovers(window, review.accountId))
}

/**
 * Whether the clock is inside this window's span right now: on a day it covers,
 * at or after the start, and before the end.
 *
 * A window that does not end after it starts is inside nothing, ever. Windows may
 * not cross midnight - anyone wanting 22:00 to 02:00 makes two - and the check
 * being a plain comparison is exactly why the rest of this file needs no cases.
 */
export function isWithinWindow(window: ReviewWindow, now: Date): boolean {
  const span = spanOf(window)
  if (!span) return false
  if (!window.days.includes(now.getDay())) return false
  const at = now.getHours() * 60 + now.getMinutes()
  return at >= span.start && at < span.end
}

/**
 * Whether this window should raise its roll-up on this tick.
 *
 * The minimum is tested here rather than once as the span opens, so a lunch that
 * begins with one review waiting stays quiet and fires the moment a second lands.
 */
export function windowShouldFire(
  window: ReviewWindow,
  now: Date,
  firedOn: string | undefined,
  waiting: number,
): boolean {
  if (!window.enabled) return false
  if (!isWithinWindow(window, now)) return false
  if (firedOn === localDay(now)) return false
  return waiting >= window.minimum
}

/**
 * Every window due on this tick. Two of them means one notification, not two.
 *
 * Each is asked about its own scope rather than about the whole deck, so a window
 * watching one account with a minimum of two waits for two reviews on that account
 * and is not tripped by a busy afternoon somewhere else.
 */
export function windowsToFire(
  windows: ReviewWindow[],
  now: Date,
  firedOn: Record<string, string>,
  waiting: Scoped[],
): ReviewWindow[] {
  return windows.filter((window) =>
    windowShouldFire(window, now, firedOn[window.id], reviewsInScope(window, waiting).length),
  )
}

/**
 * Whether the app may ping about an arrival on this account right now.
 *
 * An account no enabled window covers is always yes, which is what keeps both an
 * empty schedule and an unscheduled account identical to how the app has always
 * behaved. Where a window does cover it, liveness begins when that window's roll-up
 * does rather than when the clock enters the span, so arrivals ahead of it push the
 * count the roll-up will state instead of pinging one at a time - and it is the
 * union across covering windows, because any one of them opening is enough.
 */
export function announcingAllowedFor(
  accountId: string,
  windows: ReviewWindow[],
  now: Date,
  firedOn: Record<string, string>,
): boolean {
  const covering = windows.filter((window) => window.enabled && windowCovers(window, accountId))
  if (!covering.length) return true
  const today = localDay(now)
  return covering.some((window) => isWithinWindow(window, now) && firedOn[window.id] === today)
}

/**
 * The next moment an enabled window opens, or null when none ever does.
 *
 * Built by walking forward a day at a time from today rather than by arithmetic on
 * a timestamp, so the answer is a local wall clock on a real calendar day: a
 * Friday evening lands on Monday morning when the schedule is weekdays, and a
 * clock that goes forward overnight moves the boundary with it.
 */
export function nextWindowStart(windows: ReviewWindow[], now: Date): Date | null {
  let soonest: Date | null = null

  // Eight days, not seven: a window that only covers today has already opened, so
  // its next opening is the same weekday a week out.
  for (let offset = 0; offset <= 7; offset++) {
    for (const window of windows) {
      if (!window.enabled) continue
      const span = spanOf(window)
      if (!span) continue
      const opensAt = new Date(
        now.getFullYear(),
        now.getMonth(),
        now.getDate() + offset,
        0,
        span.start,
      )
      if (!window.days.includes(opensAt.getDay())) continue
      if (opensAt <= now) continue
      if (!soonest || opensAt < soonest) soonest = opensAt
    }
    if (soonest) return soonest
  }
  return soonest
}

/**
 * When the app is next able to interrupt, phrased for the menu bar - or null when
 * it is not in a quiet stretch and there is nothing to reassure anybody about.
 *
 * A feature whose job is silence has to prove the silence is deliberate: reviews
 * arriving unannounced read as a broken notification until something says the
 * quiet was asked for and when it lifts.
 *
 * Inside an open window this says nothing. The interrupt channel is open there
 * whether or not the roll-up has landed in the last few seconds, and "quiet until"
 * is about the stretches between windows rather than the wait for the next tick.
 */
export function quietUntil(
  windows: ReviewWindow[],
  accountIds: string[],
  waiting: Scoped[],
  now: Date,
): string | null {
  const scheduled = windows.filter((window) => window.enabled)

  // An account is quiet when something covers it and nothing covering it is open.
  const hushed = accountIds.filter((accountId) => {
    const covering = scheduled.filter((window) => windowCovers(window, accountId))
    return covering.length > 0 && !covering.some((window) => isWithinWindow(window, now))
  })
  if (!hushed.length) return null

  // Something is waiting and none of it is being held: whatever is there pinged as
  // it landed, so there is no silence to account for.
  if (waiting.length && !waiting.some((review) => hushed.includes(review.accountId))) return null

  // The soonest moment any hushed account comes back, and no more than that. A menu
  // line enumerating accounts and their separate resume times helps nobody.
  const resumes = nextWindowStart(
    scheduled.filter((window) => hushed.some((accountId) => windowCovers(window, accountId))),
    now,
  )
  if (!resumes) return null

  const clock = `${pad(resumes.getHours())}:${pad(resumes.getMinutes())}`
  // The day only earns a mention when it is not this one, so the common case -
  // quiet this morning, back at noon - stays a time and nothing else.
  return localDay(resumes) === localDay(now) ? clock : `${dayName(resumes.getDay())} ${clock}`
}

/** Why this window cannot be saved, or null when it is fine. */
export function windowProblem(window: ReviewWindow): string | null {
  if (!window.days.length) return 'Pick at least one day.'
  const start = minutesOfDay(window.start)
  const end = minutesOfDay(window.end)
  if (start === null || end === null) return 'Both times need to be set.'
  if (end <= start) return 'The end time has to be after the start time.'
  if (!Number.isInteger(window.minimum) || window.minimum < 1) {
    return 'The minimum has to be at least one review.'
  }
  return null
}

/** Monday first, because that is how a working week is read. */
const WEEK = [1, 2, 3, 4, 5, 6, 0]
const DAY_NAMES: Record<number, string> = {
  0: 'Sun',
  1: 'Mon',
  2: 'Tue',
  3: 'Wed',
  4: 'Thu',
  5: 'Fri',
  6: 'Sat',
}

export function dayName(day: number): string {
  return DAY_NAMES[day] ?? '?'
}

/** "Mon-Fri", "Sat, Sun", "Every day" - a run of three or more earns the dash. */
export function describeDays(days: number[]): string {
  const picked = WEEK.filter((day) => days.includes(day))
  if (!picked.length) return 'Never'
  if (picked.length === WEEK.length) return 'Every day'

  const runs: number[][] = []
  for (const day of picked) {
    const run = runs[runs.length - 1]
    const previous = run?.[run.length - 1]
    // Adjacent in the Monday-first week, not in the numbering Date happens to use.
    if (run && previous !== undefined && WEEK.indexOf(day) === WEEK.indexOf(previous) + 1) {
      run.push(day)
    } else {
      runs.push([day])
    }
  }

  return runs
    .map((run) =>
      run.length >= 3
        ? `${dayName(run[0])}-${dayName(run[run.length - 1])}`
        : run.map(dayName).join(', '),
    )
    .join(', ')
}

/** Whether a window names accounts that are all gone, so it covers nothing at all. */
export function coversNothing(window: ReviewWindow, accountIds: string[]): boolean {
  return window.accounts.length > 0 && !accountIds.some((id) => window.accounts.includes(id))
}

/** The scope clause of a row: which accounts, or that there are none left. */
export function describeScope(
  window: ReviewWindow,
  accounts: { id: string; label: string }[],
): string {
  if (!window.accounts.length) return 'All accounts'
  const named = accounts.filter((account) => window.accounts.includes(account.id))
  if (!named.length) return 'Covers no account'
  return named.map((account) => account.label).join(', ')
}

/** One row in plain language: "Mon-Fri · 09:00-09:30 · 1+ waiting · Work GitHub". */
export function describeWindow(
  window: ReviewWindow,
  accounts: { id: string; label: string }[],
): string {
  return [
    describeDays(window.days),
    `${window.start}-${window.end}`,
    `${window.minimum}+ waiting`,
    describeScope(window, accounts),
  ].join(' · ')
}
