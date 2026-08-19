import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  announcingAllowedFor,
  coversNothing,
  describeDays,
  describeScope,
  describeWindow,
  isWithinWindow,
  localDay,
  minutesOfDay,
  nextWindowStart,
  quietUntil,
  reviewsInScope,
  windowCovers,
  windowProblem,
  windowShouldFire,
  windowsToFire,
} from '../src/shared/review-window.ts'
import type { ReviewWindow } from '../src/shared/types.ts'

const WEEKDAYS = [1, 2, 3, 4, 5]

function reviewWindow(patch: Partial<ReviewWindow> = {}): ReviewWindow {
  return {
    id: 'morning',
    enabled: true,
    days: WEEKDAYS,
    start: '09:00',
    end: '09:30',
    minimum: 1,
    accounts: [],
    ...patch,
  }
}

/** Windows count and cover reviews; all any of that needs is the account. */
const on = (accountId: string): { accountId: string } => ({ accountId })

const WORK = 'work-github'
const PERSONAL = 'personal-forgejo'
const BOTH = [WORK, PERSONAL]

/** Local wall clock, which is the only clock any of this reads. */
const at = (day: number, hour: number, minute: number): Date =>
  new Date(2026, 7, day, hour, minute)

// 2026-08-19 is a Wednesday; 2026-08-22 a Saturday.
const WEDNESDAY = 19
const SATURDAY = 22

test('minutesOfDay reads a wall clock and refuses anything else', () => {
  assert.equal(minutesOfDay('09:00'), 540)
  assert.equal(minutesOfDay('00:00'), 0)
  assert.equal(minutesOfDay('23:59'), 1439)
  assert.equal(minutesOfDay('9:05'), 545)
  for (const bad of ['', 'noon', '24:00', '09:60', '09', '09:00:00']) {
    assert.equal(minutesOfDay(bad), null, bad)
  }
})

test('the span runs from the start up to but not including the end', () => {
  const window = reviewWindow()
  assert.equal(isWithinWindow(window, at(WEDNESDAY, 8, 59)), false)
  assert.equal(isWithinWindow(window, at(WEDNESDAY, 9, 0)), true)
  assert.equal(isWithinWindow(window, at(WEDNESDAY, 9, 29)), true)
  assert.equal(isWithinWindow(window, at(WEDNESDAY, 9, 30)), false)
})

test('a day the window does not cover is outside it whatever the clock says', () => {
  assert.equal(isWithinWindow(reviewWindow(), at(SATURDAY, 9, 15)), false)
  assert.equal(isWithinWindow(reviewWindow({ days: [6] }), at(SATURDAY, 9, 15)), true)
})

test('a window that does not end after it starts is inside nothing, ever', () => {
  const crossing = reviewWindow({ start: '22:00', end: '02:00' })
  for (const hour of [21, 22, 23, 0, 1, 2, 3]) {
    assert.equal(isWithinWindow(crossing, at(WEDNESDAY, hour, 30)), false, String(hour))
  }
})

test('a window inside its span with the count met fires', () => {
  assert.equal(windowShouldFire(reviewWindow(), at(WEDNESDAY, 9, 0), undefined, 1), true)
})

test('a span that elapsed while the machine slept never fires', () => {
  // Lid opens at 14:00; the 09:00 session is long gone and a banner about it now
  // would just be noise.
  assert.equal(windowShouldFire(reviewWindow(), at(WEDNESDAY, 14, 0), undefined, 5), false)
})

test('a span still open when the machine wakes fires on that tick', () => {
  // Asleep at 09:00, lid opens at 09:12 - still inside, so it fires then.
  assert.equal(windowShouldFire(reviewWindow(), at(WEDNESDAY, 9, 12), undefined, 3), true)
})

test('a window below its minimum stays quiet, then fires when the count arrives', () => {
  const lunch = reviewWindow({ id: 'lunch', start: '12:00', end: '13:00', minimum: 2 })
  assert.equal(windowShouldFire(lunch, at(WEDNESDAY, 12, 0), undefined, 1), false)
  // A second review lands at 12:10 and the same span now qualifies.
  assert.equal(windowShouldFire(lunch, at(WEDNESDAY, 12, 10), undefined, 2), true)
})

test('a window fires once a day, and the record is what a relaunch reads', () => {
  const window = reviewWindow()
  const today = localDay(at(WEDNESDAY, 9, 5))

  assert.equal(windowShouldFire(window, at(WEDNESDAY, 9, 5), today, 4), false)
  // Still the same span after a quit and relaunch, and still already fired.
  assert.equal(windowShouldFire(window, at(WEDNESDAY, 9, 20), today, 4), false)
  // Tomorrow is a fresh day.
  assert.equal(windowShouldFire(window, at(WEDNESDAY + 1, 9, 5), today, 4), true)
})

test('a window turned off never fires', () => {
  const off = reviewWindow({ enabled: false })
  assert.equal(windowShouldFire(off, at(WEDNESDAY, 9, 5), undefined, 9), false)
})

test('two windows open on one tick come back together, for one notification', () => {
  const windows = [
    reviewWindow({ id: 'a', start: '09:00', end: '09:30' }),
    reviewWindow({ id: 'b', start: '09:15', end: '10:00' }),
  ]
  const waiting = [on(WORK), on(WORK), on(PERSONAL)]
  const both = windowsToFire(windows, at(WEDNESDAY, 9, 20), {}, waiting)
  assert.deepEqual(
    both.map((window) => window.id),
    ['a', 'b'],
  )

  // Minutes apart is a different matter: the second span is a genuinely new event,
  // so once the first has fired only the second is still due.
  const later = windowsToFire(
    windows,
    at(WEDNESDAY, 9, 20),
    { a: localDay(at(WEDNESDAY, 9, 20)) },
    waiting,
  )
  assert.deepEqual(
    later.map((window) => window.id),
    ['b'],
  )
})

test('with no schedule the app announces whenever it finds something', () => {
  assert.equal(announcingAllowedFor(WORK, [], at(WEDNESDAY, 3, 0), {}), true)
  // A schedule of nothing but disabled windows is no schedule at all.
  const off = [reviewWindow({ enabled: false })]
  assert.equal(announcingAllowedFor(WORK, off, at(WEDNESDAY, 3, 0), {}), true)
})

test('liveness begins when the roll-up does, not when the clock enters the span', () => {
  const windows = [reviewWindow()]
  const today = localDay(at(WEDNESDAY, 9, 5))

  // Inside the span but the roll-up has not gone out: still quiet, so arrivals push
  // the count rather than pinging one at a time.
  assert.equal(announcingAllowedFor(WORK, windows, at(WEDNESDAY, 9, 5), {}), false)
  // Once it has fired, the rest of the span is live.
  assert.equal(announcingAllowedFor(WORK, windows, at(WEDNESDAY, 9, 5), { morning: today }), true)
  // And the moment the span closes it is quiet again, fired or not.
  assert.equal(announcingAllowedFor(WORK, windows, at(WEDNESDAY, 9, 30), { morning: today }), false)
  // Yesterday's firing does not make today live.
  assert.equal(
    announcingAllowedFor(WORK, windows, at(WEDNESDAY, 9, 5), { morning: '2026-08-18' }),
    false,
  )
})

test('the next boundary is the next time a window opens, later the same day', () => {
  const windows = [
    reviewWindow({ id: 'morning', start: '09:00', end: '09:30' }),
    reviewWindow({ id: 'lunch', start: '12:00', end: '13:00' }),
  ]
  assert.deepEqual(nextWindowStart(windows, at(WEDNESDAY, 7, 0)), at(WEDNESDAY, 9, 0))
  // Inside the morning span the morning has already opened; lunch is next.
  assert.deepEqual(nextWindowStart(windows, at(WEDNESDAY, 9, 15)), at(WEDNESDAY, 12, 0))
  assert.deepEqual(nextWindowStart(windows, at(WEDNESDAY, 9, 30)), at(WEDNESDAY, 12, 0))
})

test('the next boundary rolls over to the next day the schedule covers', () => {
  const windows = [reviewWindow()]
  // Wednesday evening, so tomorrow morning.
  assert.deepEqual(nextWindowStart(windows, at(WEDNESDAY, 18, 0)), at(WEDNESDAY + 1, 9, 0))
  // Friday evening on a weekdays-only schedule has to reach Monday, not Saturday.
  assert.deepEqual(nextWindowStart(windows, at(SATURDAY - 1, 18, 0)), at(SATURDAY + 2, 9, 0))
})

test('a window that covers only today opens again a week out, not never', () => {
  const wednesdays = [reviewWindow({ days: [3] })]
  assert.deepEqual(nextWindowStart(wednesdays, at(WEDNESDAY, 10, 0)), at(WEDNESDAY + 7, 9, 0))
})

test('a schedule that describes no span at all has no next boundary', () => {
  assert.equal(nextWindowStart([], at(WEDNESDAY, 10, 0)), null)
  assert.equal(nextWindowStart([reviewWindow({ enabled: false })], at(WEDNESDAY, 10, 0)), null)
  assert.equal(nextWindowStart([reviewWindow({ days: [] })], at(WEDNESDAY, 10, 0)), null)
  // Crosses midnight, so it describes nothing this can open.
  assert.equal(
    nextWindowStart([reviewWindow({ start: '22:00', end: '02:00' })], at(WEDNESDAY, 10, 0)),
    null,
  )
})

test('the menu bar says nothing extra when there is no schedule to be quiet for', () => {
  assert.equal(quietUntil([], BOTH, [], at(WEDNESDAY, 3, 0)), null)
  assert.equal(quietUntil([reviewWindow({ enabled: false })], BOTH, [], at(WEDNESDAY, 3, 0)), null)
})

test('the menu bar says nothing extra while a window is open', () => {
  assert.equal(quietUntil([reviewWindow()], BOTH, [], at(WEDNESDAY, 9, 0)), null)
  assert.equal(quietUntil([reviewWindow()], BOTH, [], at(WEDNESDAY, 9, 29)), null)
})

test('a quiet stretch says when it lifts, naming the day only when it is not today', () => {
  const windows = [
    reviewWindow({ id: 'morning', start: '09:00', end: '09:30' }),
    reviewWindow({ id: 'lunch', start: '12:00', end: '13:00' }),
  ]
  assert.equal(quietUntil(windows, BOTH, [], at(WEDNESDAY, 8, 47)), '09:00')
  assert.equal(quietUntil(windows, BOTH, [], at(WEDNESDAY, 9, 30)), '12:00')
  assert.equal(quietUntil(windows, BOTH, [], at(WEDNESDAY, 18, 0)), 'Thu 09:00')
  // Friday evening on a weekdays-only schedule reads as Monday morning.
  assert.equal(quietUntil(windows, BOTH, [], at(SATURDAY - 1, 18, 0)), 'Mon 09:00')
  assert.equal(quietUntil(windows, BOTH, [], at(SATURDAY, 11, 0)), 'Mon 09:00')
})

test('a window naming no account covers every one, including one added later', () => {
  const all = reviewWindow()
  assert.equal(windowCovers(all, WORK), true)
  assert.equal(windowCovers(all, PERSONAL), true)
  assert.equal(windowCovers(all, 'connected-next-march'), true)
})

test('a window naming accounts covers those and no others', () => {
  const work = reviewWindow({ accounts: [WORK] })
  assert.equal(windowCovers(work, WORK), true)
  assert.equal(windowCovers(work, PERSONAL), false)
})

test('a window left scoped to accounts that are gone covers nothing', () => {
  const orphan = reviewWindow({ accounts: ['signed-out-months-ago'] })
  assert.equal(coversNothing(orphan, BOTH), true)
  assert.equal(windowCovers(orphan, WORK), false)
  // Which is what stops it firing: its scope is empty, so its count is always zero.
  assert.equal(reviewsInScope(orphan, [on(WORK), on(PERSONAL)]).length, 0)
  assert.deepEqual(
    windowsToFire([orphan], at(WEDNESDAY, 9, 5), {}, [on(WORK), on(PERSONAL)]),
    [],
  )
  // An all-accounts window is never in that state, however few accounts there are.
  assert.equal(coversNothing(reviewWindow(), []), false)
})

test('a review no enabled window covers pings live whatever the clock says', () => {
  const work = [reviewWindow({ accounts: [WORK] })]
  // Deep in the night, outside every span: the covered account is quiet...
  assert.equal(announcingAllowedFor(WORK, work, at(WEDNESDAY, 3, 0), {}), false)
  // ...and the one nothing claims behaves as it did before any of this existed.
  assert.equal(announcingAllowedFor(PERSONAL, work, at(WEDNESDAY, 3, 0), {}), true)
})

test('liveness is the union across the windows covering an account', () => {
  const today = localDay(at(WEDNESDAY, 12, 30))
  const windows = [
    reviewWindow({ id: 'work-morning', accounts: [WORK], start: '09:00', end: '09:30' }),
    reviewWindow({ id: 'everything-lunch', start: '12:00', end: '13:00' }),
  ]
  // Lunch has fired and covers everything, so both accounts are live inside it.
  const fired = { 'everything-lunch': today }
  assert.equal(announcingAllowedFor(WORK, windows, at(WEDNESDAY, 12, 30), fired), true)
  assert.equal(announcingAllowedFor(PERSONAL, windows, at(WEDNESDAY, 12, 30), fired), true)
  // The morning window having fired says nothing about the afternoon.
  const morning = { 'work-morning': today }
  assert.equal(announcingAllowedFor(WORK, windows, at(WEDNESDAY, 12, 30), morning), false)
  assert.equal(announcingAllowedFor(WORK, windows, at(WEDNESDAY, 9, 5), morning), true)
  // Personal is covered only by lunch, so the morning span leaves it quiet.
  assert.equal(announcingAllowedFor(PERSONAL, windows, at(WEDNESDAY, 9, 5), morning), false)
})

test('a window counts its threshold over its own scope, not the whole deck', () => {
  const work = reviewWindow({ accounts: [WORK], minimum: 2 })
  const busyElsewhere = [on(PERSONAL), on(PERSONAL), on(PERSONAL), on(WORK)]
  assert.deepEqual(windowsToFire([work], at(WEDNESDAY, 9, 5), {}, busyElsewhere), [])
  // A second review on the account it actually watches is what trips it.
  const due = windowsToFire([work], at(WEDNESDAY, 9, 5), {}, [...busyElsewhere, on(WORK)])
  assert.deepEqual(
    due.map((window) => window.id),
    ['morning'],
  )
})

test('windows of differing scope firing on one tick come back together', () => {
  const windows = [
    reviewWindow({ id: 'work', accounts: [WORK] }),
    reviewWindow({ id: 'personal', accounts: [PERSONAL] }),
    reviewWindow({ id: 'quiet-one', accounts: ['nobody'] }),
  ]
  const due = windowsToFire(windows, at(WEDNESDAY, 9, 5), {}, [on(WORK), on(PERSONAL)])
  // One notification, over the union of what these two cover; the third covers
  // nothing and stays out of it.
  assert.deepEqual(
    due.map((window) => window.id),
    ['work', 'personal'],
  )
})

test('the scope clause names the accounts, or says there are none left', () => {
  assert.equal(describeScope(reviewWindow(), SIGNED_IN), 'All accounts')
  assert.equal(describeScope(reviewWindow({ accounts: [WORK] }), SIGNED_IN), 'Work GitHub')
  assert.equal(
    describeScope(reviewWindow({ accounts: [WORK, PERSONAL] }), SIGNED_IN),
    'Work GitHub, Personal Forgejo',
  )
  assert.equal(describeScope(reviewWindow({ accounts: ['gone'] }), SIGNED_IN), 'Covers no account')
})

test('the quiet line follows the accounts a schedule actually claims', () => {
  const work = [reviewWindow({ accounts: [WORK] })]
  // Work is quiet and a work review is waiting for it: say when that lifts.
  assert.equal(quietUntil(work, BOTH, [on(WORK)], at(WEDNESDAY, 8, 0)), '09:00')
  // Only an unclaimed account is waiting, and that pinged as it landed - there is
  // no silence to account for, so the line stays away.
  assert.equal(quietUntil(work, BOTH, [on(PERSONAL)], at(WEDNESDAY, 8, 0)), null)
  // Nothing waiting at all still reassures: the schedule is holding the line.
  assert.equal(quietUntil(work, BOTH, [], at(WEDNESDAY, 8, 0)), '09:00')
  // Nothing claims personal, so on its own it is never a reason to be quiet.
  assert.equal(quietUntil(work, [PERSONAL], [], at(WEDNESDAY, 8, 0)), null)
})

test('the quiet line reports the soonest return across the accounts being held', () => {
  const windows = [
    reviewWindow({ id: 'work', accounts: [WORK], start: '09:00', end: '09:30' }),
    reviewWindow({ id: 'personal', accounts: [PERSONAL], start: '17:00', end: '18:00' }),
  ]
  // Both held at 08:00; work comes back first and that is the whole answer.
  assert.equal(quietUntil(windows, BOTH, [], at(WEDNESDAY, 8, 0)), '09:00')
  // Inside the work span, only personal is still held, so its own time is next.
  assert.equal(quietUntil(windows, BOTH, [], at(WEDNESDAY, 9, 10)), '17:00')
})

test('localDay names the local calendar day, zero padded', () => {
  assert.equal(localDay(new Date(2026, 0, 5, 23, 30)), '2026-01-05')
  assert.equal(localDay(new Date(2026, 11, 31, 0, 1)), '2026-12-31')
})

test('a window has to be saveable before it can be saved', () => {
  assert.equal(windowProblem(reviewWindow()), null)
  assert.match(String(windowProblem(reviewWindow({ days: [] }))), /at least one day/)
  assert.match(String(windowProblem(reviewWindow({ end: '09:00' }))), /after the start/)
  assert.match(String(windowProblem(reviewWindow({ end: '08:00' }))), /after the start/)
  assert.match(String(windowProblem(reviewWindow({ end: '' }))), /Both times/)
  assert.match(String(windowProblem(reviewWindow({ minimum: 0 }))), /at least one review/)
})

const SIGNED_IN = [
  { id: WORK, label: 'Work GitHub' },
  { id: PERSONAL, label: 'Personal Forgejo' },
]

test('a row says its own schedule in plain language', () => {
  assert.equal(
    describeWindow(reviewWindow(), SIGNED_IN),
    'Mon-Fri · 09:00-09:30 · 1+ waiting · All accounts',
  )
  assert.equal(
    describeWindow(
      reviewWindow({ days: [6, 0], start: '11:00', end: '12:00', minimum: 3 }),
      SIGNED_IN,
    ),
    'Sat, Sun · 11:00-12:00 · 3+ waiting · All accounts',
  )
  assert.equal(
    describeWindow(reviewWindow({ accounts: [WORK] }), SIGNED_IN),
    'Mon-Fri · 09:00-09:30 · 1+ waiting · Work GitHub',
  )
})

test('days read as a working week rather than as the numbers underneath', () => {
  assert.equal(describeDays([0, 1, 2, 3, 4, 5, 6]), 'Every day')
  assert.equal(describeDays(WEEKDAYS), 'Mon-Fri')
  assert.equal(describeDays([1]), 'Mon')
  assert.equal(describeDays([1, 3, 5]), 'Mon, Wed, Fri')
  assert.equal(describeDays([6, 0]), 'Sat, Sun')
  // Sunday closes the week here, so it never joins a run that starts on Monday.
  assert.equal(describeDays([0, 1, 2]), 'Mon, Tue, Sun')
  assert.equal(describeDays([]), 'Never')
})
