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
 * Every decision here reads the current local wall clock rather than a fire time
 * worked out in advance. That is what makes the end time carry its weight: a lid
 * that opens at 09:12 is still inside a 09:00-09:30 span and fires then, one that
 * opens at 14:00 has missed the morning and stays silent, and daylight saving and
 * travel need no special case because there is nothing precomputed to go stale.
 */

import type { ReviewWindow } from './types.ts'

/** Twice a minute, because a three-minute sync would let a 09:00 span open at 09:02. */
export const WINDOW_TICK_MS = 30_000

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
  const pad = (value: number): string => String(value).padStart(2, '0')
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
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
  const start = minutesOfDay(window.start)
  const end = minutesOfDay(window.end)
  if (start === null || end === null || end <= start) return false
  if (!window.days.includes(now.getDay())) return false
  const at = now.getHours() * 60 + now.getMinutes()
  return at >= start && at < end
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

/** Every window due on this tick. Two of them means one notification, not two. */
export function windowsToFire(
  windows: ReviewWindow[],
  now: Date,
  firedOn: Record<string, string>,
  waiting: number,
): ReviewWindow[] {
  return windows.filter((window) => windowShouldFire(window, now, firedOn[window.id], waiting))
}

/**
 * Whether the app may ping about an individual arrival right now.
 *
 * With no schedule the answer is always yes, which is what keeps an empty list
 * identical to how the app has always behaved. With one, liveness begins when the
 * roll-up does rather than when the clock enters the span, so arrivals ahead of it
 * push the count the roll-up will state instead of pinging one at a time.
 */
export function announcingAllowed(
  windows: ReviewWindow[],
  now: Date,
  firedOn: Record<string, string>,
): boolean {
  const scheduled = windows.filter((window) => window.enabled)
  if (!scheduled.length) return true
  const today = localDay(now)
  return scheduled.some((window) => isWithinWindow(window, now) && firedOn[window.id] === today)
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

/** One row of the schedule in plain language: "Mon-Fri · 09:00-09:30 · 1+ waiting". */
export function describeWindow(window: ReviewWindow): string {
  return [
    describeDays(window.days),
    `${window.start}-${window.end}`,
    `${window.minimum}+ waiting`,
  ].join(' · ')
}
