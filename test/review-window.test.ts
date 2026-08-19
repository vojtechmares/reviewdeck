import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  announcingAllowed,
  describeDays,
  describeWindow,
  isWithinWindow,
  localDay,
  minutesOfDay,
  nextWindowStart,
  quietUntil,
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
    ...patch,
  }
}

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
  const both = windowsToFire(windows, at(WEDNESDAY, 9, 20), {}, 3)
  assert.deepEqual(
    both.map((window) => window.id),
    ['a', 'b'],
  )

  // Minutes apart is a different matter: the second span is a genuinely new event,
  // so once the first has fired only the second is still due.
  const later = windowsToFire(windows, at(WEDNESDAY, 9, 20), { a: localDay(at(WEDNESDAY, 9, 20)) }, 3)
  assert.deepEqual(
    later.map((window) => window.id),
    ['b'],
  )
})

test('with no schedule the app announces whenever it finds something', () => {
  assert.equal(announcingAllowed([], at(WEDNESDAY, 3, 0), {}), true)
  // A schedule of nothing but disabled windows is no schedule at all.
  assert.equal(announcingAllowed([reviewWindow({ enabled: false })], at(WEDNESDAY, 3, 0), {}), true)
})

test('liveness begins when the roll-up does, not when the clock enters the span', () => {
  const windows = [reviewWindow()]
  const today = localDay(at(WEDNESDAY, 9, 5))

  // Inside the span but the roll-up has not gone out: still quiet, so arrivals push
  // the count rather than pinging one at a time.
  assert.equal(announcingAllowed(windows, at(WEDNESDAY, 9, 5), {}), false)
  // Once it has fired, the rest of the span is live.
  assert.equal(announcingAllowed(windows, at(WEDNESDAY, 9, 5), { morning: today }), true)
  // And the moment the span closes it is quiet again, fired or not.
  assert.equal(announcingAllowed(windows, at(WEDNESDAY, 9, 30), { morning: today }), false)
  // Yesterday's firing does not make today live.
  assert.equal(announcingAllowed(windows, at(WEDNESDAY, 9, 5), { morning: '2026-08-18' }), false)
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
  assert.equal(quietUntil([], at(WEDNESDAY, 3, 0)), null)
  assert.equal(quietUntil([reviewWindow({ enabled: false })], at(WEDNESDAY, 3, 0)), null)
})

test('the menu bar says nothing extra while a window is open', () => {
  assert.equal(quietUntil([reviewWindow()], at(WEDNESDAY, 9, 0)), null)
  assert.equal(quietUntil([reviewWindow()], at(WEDNESDAY, 9, 29)), null)
})

test('a quiet stretch says when it lifts, naming the day only when it is not today', () => {
  const windows = [
    reviewWindow({ id: 'morning', start: '09:00', end: '09:30' }),
    reviewWindow({ id: 'lunch', start: '12:00', end: '13:00' }),
  ]
  assert.equal(quietUntil(windows, at(WEDNESDAY, 8, 47)), '09:00')
  assert.equal(quietUntil(windows, at(WEDNESDAY, 9, 30)), '12:00')
  assert.equal(quietUntil(windows, at(WEDNESDAY, 18, 0)), 'Thu 09:00')
  // Friday evening on a weekdays-only schedule reads as Monday morning.
  assert.equal(quietUntil(windows, at(SATURDAY - 1, 18, 0)), 'Mon 09:00')
  assert.equal(quietUntil(windows, at(SATURDAY, 11, 0)), 'Mon 09:00')
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

test('a row says its own schedule in plain language', () => {
  assert.equal(describeWindow(reviewWindow()), 'Mon-Fri · 09:00-09:30 · 1+ waiting')
  assert.equal(
    describeWindow(reviewWindow({ days: [6, 0], start: '11:00', end: '12:00', minimum: 3 })),
    'Sat, Sun · 11:00-12:00 · 3+ waiting',
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
