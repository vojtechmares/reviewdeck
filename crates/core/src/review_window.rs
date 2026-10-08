//! Review windows: the recurring stretches of the day the app is allowed to
//! interrupt in. A port of src/shared/review-window.ts.
//!
//! Outside every window it stays quiet and lets reviews pile up. When a window
//! opens it raises one roll-up of everything waiting, and for the rest of that span
//! it pings live as arrivals land, exactly as the app behaves with no schedule at
//! all - which is what an empty list means, so this costs nothing to anyone who
//! never opens the schedule.
//!
//! A window may also name the accounts it covers, and that one field turns "is the
//! app quiet" into a question about each review rather than about the app. A review
//! no enabled window covers pings live exactly as it did before any of this existed,
//! which is the safest way for this to fail: there is no way to configure yourself
//! into permanent silence, and windows only ever silence what they claim.
//!
//! Every decision here reads the current local wall clock rather than a fire time
//! worked out in advance. That is what makes the end time carry its weight: a lid
//! that opens at 09:12 is still inside a 09:00-09:30 span and fires then, one that
//! opens at 14:00 has missed the morning and stays silent, and daylight saving and
//! travel need no special case because there is nothing precomputed to go stale.
//!
//! Moments are milliseconds since the Unix epoch (what `Date#getTime` returns); the
//! local calendar is read through [`crate::time::local_parts`] and built back with
//! [`crate::time::local_to_ms`], which carry fields over the way `Date` does.

use std::collections::{BTreeMap, HashMap};

use crate::model::{Account, ReviewItem, ReviewWindow};
use crate::time::{local_parts, local_to_ms};

/// All this module needs of a review: the account it came in on.
pub trait Scoped {
    fn account_id(&self) -> &str;
}

impl Scoped for ReviewItem {
    fn account_id(&self) -> &str {
        &self.account_id
    }
}

impl<T: Scoped + ?Sized> Scoped for &T {
    fn account_id(&self) -> &str {
        (**self).account_id()
    }
}

/// Where each window's roll-up last fired: window id -> the local day
/// ([`local_day`]) it fired on. The vault's `windowsFired`, whichever map holds it.
pub trait FiredRecord {
    fn fired_on(&self, window_id: &str) -> Option<&str>;
}

impl FiredRecord for HashMap<String, String> {
    fn fired_on(&self, window_id: &str) -> Option<&str> {
        self.get(window_id).map(String::as_str)
    }
}

impl FiredRecord for BTreeMap<String, String> {
    fn fired_on(&self, window_id: &str) -> Option<&str> {
        self.get(window_id).map(String::as_str)
    }
}

/// An account as a schedule row names it: its id and the label the user gave it
/// (`{ id: string; label: string }` in the TypeScript).
pub trait Labelled {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
}

impl Labelled for Account {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.label
    }
}

impl Labelled for (&str, &str) {
    fn id(&self) -> &str {
        self.0
    }
    fn label(&self) -> &str {
        self.1
    }
}

impl Labelled for (String, String) {
    fn id(&self) -> &str {
        &self.0
    }
    fn label(&self) -> &str {
        &self.1
    }
}

/// Twice a minute, because a three-minute sync would let a 09:00 span open at 09:02.
pub const WINDOW_TICK_MS: u64 = 30_000;

/// What JavaScript's `String#trim` strips: Unicode white space, and the byte order
/// mark that JavaScript counts as white space and Unicode does not.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

/// Minutes since local midnight, or `None` when the text is not a wall clock.
///
/// One or two hour digits, a colon, exactly two minute digits (`/^(\d{1,2}):(\d{2})$/`
/// on the trimmed text).
pub fn minutes_of_day(time: &str) -> Option<u32> {
    let (hours, minutes) = js_trim(time).split_once(':')?;
    let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if !(1..=2).contains(&hours.len()) || minutes.len() != 2 {
        return None;
    }
    if !all_digits(hours) || !all_digits(minutes) {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(hours * 60 + minutes)
}

/// The local calendar day, which is the key the once-a-day guarantee turns on:
/// `YYYY-MM-DD` with the month and day zero padded.
pub fn local_day(now: i64) -> String {
    let local = local_parts(now);
    format!("{}-{:02}-{:02}", local.year, local.month, local.day)
}

/// A window's span in minutes since midnight, `[start, end)`.
#[derive(Clone, Copy)]
struct Span {
    start: u32,
    end: u32,
}

/// A window's span, or `None` when it does not describe one: unreadable times, or
/// an end that fails to come after its start. A window may not cross midnight -
/// anyone wanting 22:00 to 02:00 makes two - and refusing to read one that tries is
/// what lets everything downstream be a plain comparison.
fn span_of(window: &ReviewWindow) -> Option<Span> {
    let start = minutes_of_day(&window.start)?;
    let end = minutes_of_day(&window.end)?;
    (end > start).then_some(Span { start, end })
}

/// Whether this window claims an account.
///
/// Naming none claims them all, which is also what a window left scoped to accounts
/// that have since been signed out reduces to: it names some, matches none of the
/// ones still here, and so covers nothing and can never fire. That is shown rather
/// than tidied away, because silently widening it back to everything would start
/// interrupting about accounts nobody asked it to watch.
pub fn window_covers(window: &ReviewWindow, account_id: &str) -> bool {
    window.accounts.is_empty() || window.accounts.iter().any(|id| id == account_id)
}

/// The reviews a window is responsible for, which is what it counts and summarises.
pub fn reviews_in_scope<'a, T: Scoped>(window: &ReviewWindow, reviews: &'a [T]) -> Vec<&'a T> {
    reviews
        .iter()
        .filter(|review| window_covers(window, review.account_id()))
        .collect()
}

/// Whether the clock is inside this window's span right now: on a day it covers,
/// at or after the start, and before the end.
///
/// A window that does not end after it starts is inside nothing, ever. Windows may
/// not cross midnight - anyone wanting 22:00 to 02:00 makes two - and the check
/// being a plain comparison is exactly why the rest of this file needs no cases.
pub fn is_within_window(window: &ReviewWindow, now: i64) -> bool {
    let Some(span) = span_of(window) else {
        return false;
    };
    let local = local_parts(now);
    if !window.days.contains(&local.weekday) {
        return false;
    }
    let at = local.hour * 60 + local.minute;
    at >= span.start && at < span.end
}

/// Whether this window should raise its roll-up on this tick.
///
/// The minimum is tested here rather than once as the span opens, so a lunch that
/// begins with one review waiting stays quiet and fires the moment a second lands.
pub fn window_should_fire(
    window: &ReviewWindow,
    now: i64,
    fired_on: Option<&str>,
    waiting: usize,
) -> bool {
    if !window.enabled {
        return false;
    }
    if !is_within_window(window, now) {
        return false;
    }
    if fired_on == Some(local_day(now).as_str()) {
        return false;
    }
    i64::try_from(waiting).unwrap_or(i64::MAX) >= window.minimum
}

/// Every window due on this tick. Two of them means one notification, not two.
///
/// Each is asked about its own scope rather than about the whole deck, so a window
/// watching one account with a minimum of two waits for two reviews on that account
/// and is not tripped by a busy afternoon somewhere else.
pub fn windows_to_fire<'a, T: Scoped>(
    windows: &'a [ReviewWindow],
    now: i64,
    fired_on: &impl FiredRecord,
    waiting: &[T],
) -> Vec<&'a ReviewWindow> {
    windows
        .iter()
        .filter(|window| {
            window_should_fire(
                window,
                now,
                fired_on.fired_on(&window.id),
                reviews_in_scope(window, waiting).len(),
            )
        })
        .collect()
}

/// Whether the app may ping about an arrival on this account right now.
///
/// An account no enabled window covers is always yes, which is what keeps both an
/// empty schedule and an unscheduled account identical to how the app has always
/// behaved. Where a window does cover it, liveness begins when that window's roll-up
/// does rather than when the clock enters the span, so arrivals ahead of it push the
/// count the roll-up will state instead of pinging one at a time - and it is the
/// union across covering windows, because any one of them opening is enough.
pub fn announcing_allowed_for(
    account_id: &str,
    windows: &[ReviewWindow],
    now: i64,
    fired_on: &impl FiredRecord,
) -> bool {
    let mut covering = windows
        .iter()
        .filter(|window| window.enabled && window_covers(window, account_id))
        .peekable();
    if covering.peek().is_none() {
        return true;
    }
    let today = local_day(now);
    covering.any(|window| {
        is_within_window(window, now) && fired_on.fired_on(&window.id) == Some(today.as_str())
    })
}

/// The next moment an enabled window opens, or `None` when none ever does.
///
/// Built by walking forward a day at a time from today rather than by arithmetic on
/// a timestamp, so the answer is a local wall clock on a real calendar day: a
/// Friday evening lands on Monday morning when the schedule is weekdays, and a
/// clock that goes forward overnight moves the boundary with it.
pub fn next_window_start(windows: &[ReviewWindow], now: i64) -> Option<i64> {
    let mut soonest: Option<i64> = None;
    let today = local_parts(now);

    // Eight days, not seven: a window that only covers today has already opened, so
    // its next opening is the same weekday a week out.
    for offset in 0..=7 {
        for window in windows {
            if !window.enabled {
                continue;
            }
            let Some(span) = span_of(window) else {
                continue;
            };
            // `new Date(year, month, date + offset, 0, span.start)`: the day and the
            // minutes carry over the way `Date` carries them.
            let Some(opens_at) = local_to_ms(
                today.year,
                today.month as i32,
                today.day as i32 + offset,
                0,
                span.start as i32,
            ) else {
                continue;
            };
            if !window.days.contains(&local_parts(opens_at).weekday) {
                continue;
            }
            if opens_at <= now {
                continue;
            }
            if soonest.is_none_or(|soonest| opens_at < soonest) {
                soonest = Some(opens_at);
            }
        }
        if soonest.is_some() {
            return soonest;
        }
    }
    soonest
}

/// When the app is next able to interrupt, phrased for the menu bar - or `None`
/// when it is not in a quiet stretch and there is nothing to reassure anybody about.
///
/// A feature whose job is silence has to prove the silence is deliberate: reviews
/// arriving unannounced read as a broken notification until something says the
/// quiet was asked for and when it lifts.
///
/// Inside an open window this says nothing. The interrupt channel is open there
/// whether or not the roll-up has landed in the last few seconds, and "quiet until"
/// is about the stretches between windows rather than the wait for the next tick.
pub fn quiet_until<S: AsRef<str>, T: Scoped>(
    windows: &[ReviewWindow],
    account_ids: &[S],
    waiting: &[T],
    now: i64,
) -> Option<String> {
    let scheduled: Vec<&ReviewWindow> = windows.iter().filter(|window| window.enabled).collect();

    // An account is quiet when something covers it and nothing covering it is open.
    let hushed: Vec<&str> = account_ids
        .iter()
        .map(AsRef::as_ref)
        .filter(|account_id| {
            let covering: Vec<&&ReviewWindow> = scheduled
                .iter()
                .filter(|window| window_covers(window, account_id))
                .collect();
            !covering.is_empty() && !covering.iter().any(|window| is_within_window(window, now))
        })
        .collect();
    if hushed.is_empty() {
        return None;
    }

    // Something is waiting and none of it is being held: whatever is there pinged as
    // it landed, so there is no silence to account for.
    if !waiting.is_empty()
        && !waiting
            .iter()
            .any(|review| hushed.contains(&review.account_id()))
    {
        return None;
    }

    // The soonest moment any hushed account comes back, and no more than that. A menu
    // line enumerating accounts and their separate resume times helps nobody.
    let resuming: Vec<ReviewWindow> = scheduled
        .iter()
        .filter(|window| {
            hushed
                .iter()
                .any(|account_id| window_covers(window, account_id))
        })
        .map(|window| (*window).clone())
        .collect();
    let resumes = next_window_start(&resuming, now)?;

    let at = local_parts(resumes);
    let clock = format!("{:02}:{:02}", at.hour, at.minute);
    // The day only earns a mention when it is not this one, so the common case -
    // quiet this morning, back at noon - stays a time and nothing else.
    Some(if local_day(resumes) == local_day(now) {
        clock
    } else {
        format!("{} {clock}", day_name(at.weekday))
    })
}

/// Why this window cannot be saved, or `None` when it is fine.
pub fn window_problem(window: &ReviewWindow) -> Option<&'static str> {
    if window.days.is_empty() {
        return Some("Pick at least one day.");
    }
    let (Some(start), Some(end)) = (minutes_of_day(&window.start), minutes_of_day(&window.end))
    else {
        return Some("Both times need to be set.");
    };
    if end <= start {
        return Some("The end time has to be after the start time.");
    }
    // The TypeScript also refuses a fractional minimum; an integer field cannot hold one.
    if window.minimum < 1 {
        return Some("The minimum has to be at least one review.");
    }
    None
}

/// Monday first, because that is how a working week is read.
const WEEK: [u32; 7] = [1, 2, 3, 4, 5, 6, 0];

/// The short name of a day numbered as `Date#getDay` does, or `?` for anything else.
pub fn day_name(day: u32) -> &'static str {
    match day {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        6 => "Sat",
        _ => "?",
    }
}

/// "Mon-Fri", "Sat, Sun", "Every day" - a run of three or more earns the dash.
pub fn describe_days(days: &[u32]) -> String {
    let picked: Vec<u32> = WEEK.into_iter().filter(|day| days.contains(day)).collect();
    if picked.is_empty() {
        return "Never".into();
    }
    if picked.len() == WEEK.len() {
        return "Every day".into();
    }

    let position = |day: u32| WEEK.iter().position(|&d| d == day);
    let mut runs: Vec<Vec<u32>> = Vec::new();
    for day in picked {
        // Adjacent in the Monday-first week, not in the numbering Date happens to use.
        let extends = runs.last().and_then(|run| run.last()).is_some_and(|&previous| {
            matches!((position(day), position(previous)), (Some(at), Some(before)) if at == before + 1)
        });
        match runs.last_mut() {
            Some(run) if extends => run.push(day),
            _ => runs.push(vec![day]),
        }
    }

    runs.iter()
        .map(|run| match run.as_slice() {
            [first, .., last] if run.len() >= 3 => {
                format!("{}-{}", day_name(*first), day_name(*last))
            }
            _ => run
                .iter()
                .map(|day| day_name(*day))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a window names accounts that are all gone, so it covers nothing at all.
pub fn covers_nothing<S: AsRef<str>>(window: &ReviewWindow, account_ids: &[S]) -> bool {
    !window.accounts.is_empty()
        && !account_ids
            .iter()
            .any(|id| window.accounts.iter().any(|named| named == id.as_ref()))
}

/// The scope clause of a row: which accounts, or that there are none left.
pub fn describe_scope<A: Labelled>(window: &ReviewWindow, accounts: &[A]) -> String {
    if window.accounts.is_empty() {
        return "All accounts".into();
    }
    let named: Vec<&str> = accounts
        .iter()
        .filter(|account| window.accounts.iter().any(|id| id == account.id()))
        .map(Labelled::label)
        .collect();
    if named.is_empty() {
        return "Covers no account".into();
    }
    named.join(", ")
}

/// One row in plain language: "Mon-Fri · 09:00-09:30 · 1+ waiting · Work GitHub".
pub fn describe_window<A: Labelled>(window: &ReviewWindow, accounts: &[A]) -> String {
    [
        describe_days(&window.days),
        format!("{}-{}", window.start, window.end),
        format!("{}+ waiting", window.minimum),
        describe_scope(window, accounts),
    ]
    .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEKDAYS: [u32; 5] = [1, 2, 3, 4, 5];

    fn review_window() -> ReviewWindow {
        ReviewWindow {
            id: "morning".into(),
            enabled: true,
            days: WEEKDAYS.to_vec(),
            start: "09:00".into(),
            end: "09:30".into(),
            minimum: 1,
            accounts: vec![],
        }
    }

    fn with(patch: impl FnOnce(&mut ReviewWindow)) -> ReviewWindow {
        let mut window = review_window();
        patch(&mut window);
        window
    }

    fn span(id: &str, start: &str, end: &str) -> ReviewWindow {
        with(|w| {
            w.id = id.into();
            w.start = start.into();
            w.end = end.into();
        })
    }

    /// Windows count and cover reviews; all any of that needs is the account.
    struct On(&'static str);

    impl Scoped for On {
        fn account_id(&self) -> &str {
            self.0
        }
    }

    const WORK: &str = "work-github";
    const PERSONAL: &str = "personal-forgejo";
    const BOTH: [&str; 2] = [WORK, PERSONAL];
    const NOTHING: [On; 0] = [];

    const SIGNED_IN: [(&str, &str); 2] = [(WORK, "Work GitHub"), (PERSONAL, "Personal Forgejo")];

    /// Local wall clock, which is the only clock any of this reads.
    fn at(day: i32, hour: i32, minute: i32) -> i64 {
        local_to_ms(2026, 8, day, hour, minute).expect("representable")
    }

    fn fired(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(id, day)| (id.to_string(), day.to_string()))
            .collect()
    }

    fn none_fired() -> HashMap<String, String> {
        HashMap::new()
    }

    fn ids(windows: &[&ReviewWindow]) -> Vec<String> {
        windows.iter().map(|window| window.id.clone()).collect()
    }

    // 2026-08-19 is a Wednesday; 2026-08-22 a Saturday.
    const WEDNESDAY: i32 = 19;
    const SATURDAY: i32 = 22;

    #[test]
    fn minutes_of_day_reads_a_wall_clock_and_refuses_anything_else() {
        assert_eq!(minutes_of_day("09:00"), Some(540));
        assert_eq!(minutes_of_day("00:00"), Some(0));
        assert_eq!(minutes_of_day("23:59"), Some(1439));
        assert_eq!(minutes_of_day("9:05"), Some(545));
        for bad in ["", "noon", "24:00", "09:60", "09", "09:00:00"] {
            assert_eq!(minutes_of_day(bad), None, "{bad}");
        }
    }

    #[test]
    fn minutes_of_day_trims_like_javascript_and_wants_ascii_digits() {
        assert_eq!(minutes_of_day("  09:00\n"), Some(540));
        assert_eq!(minutes_of_day("\u{FEFF}09:00"), Some(540));
        assert_eq!(minutes_of_day("+9:00"), None);
        assert_eq!(minutes_of_day("٠٩:٠٠"), None);
        assert_eq!(minutes_of_day("123:00"), None);
    }

    #[test]
    fn the_span_runs_from_the_start_up_to_but_not_including_the_end() {
        let window = review_window();
        assert!(!is_within_window(&window, at(WEDNESDAY, 8, 59)));
        assert!(is_within_window(&window, at(WEDNESDAY, 9, 0)));
        assert!(is_within_window(&window, at(WEDNESDAY, 9, 29)));
        assert!(!is_within_window(&window, at(WEDNESDAY, 9, 30)));
    }

    #[test]
    fn a_day_the_window_does_not_cover_is_outside_it_whatever_the_clock_says() {
        assert!(!is_within_window(&review_window(), at(SATURDAY, 9, 15)));
        assert!(is_within_window(
            &with(|w| w.days = vec![6]),
            at(SATURDAY, 9, 15)
        ));
    }

    #[test]
    fn a_window_that_does_not_end_after_it_starts_is_inside_nothing_ever() {
        let crossing = span("morning", "22:00", "02:00");
        for hour in [21, 22, 23, 0, 1, 2, 3] {
            assert!(
                !is_within_window(&crossing, at(WEDNESDAY, hour, 30)),
                "{hour}"
            );
        }
    }

    #[test]
    fn a_window_inside_its_span_with_the_count_met_fires() {
        assert!(window_should_fire(
            &review_window(),
            at(WEDNESDAY, 9, 0),
            None,
            1
        ));
    }

    #[test]
    fn a_span_that_elapsed_while_the_machine_slept_never_fires() {
        // Lid opens at 14:00; the 09:00 session is long gone and a banner about it now
        // would just be noise.
        assert!(!window_should_fire(
            &review_window(),
            at(WEDNESDAY, 14, 0),
            None,
            5
        ));
    }

    #[test]
    fn a_span_still_open_when_the_machine_wakes_fires_on_that_tick() {
        // Asleep at 09:00, lid opens at 09:12 - still inside, so it fires then.
        assert!(window_should_fire(
            &review_window(),
            at(WEDNESDAY, 9, 12),
            None,
            3
        ));
    }

    #[test]
    fn a_window_below_its_minimum_stays_quiet_then_fires_when_the_count_arrives() {
        let lunch = with(|w| {
            w.id = "lunch".into();
            w.start = "12:00".into();
            w.end = "13:00".into();
            w.minimum = 2;
        });
        assert!(!window_should_fire(&lunch, at(WEDNESDAY, 12, 0), None, 1));
        // A second review lands at 12:10 and the same span now qualifies.
        assert!(window_should_fire(&lunch, at(WEDNESDAY, 12, 10), None, 2));
    }

    #[test]
    fn a_window_fires_once_a_day_and_the_record_is_what_a_relaunch_reads() {
        let window = review_window();
        let today = local_day(at(WEDNESDAY, 9, 5));

        assert!(!window_should_fire(
            &window,
            at(WEDNESDAY, 9, 5),
            Some(&today),
            4
        ));
        // Still the same span after a quit and relaunch, and still already fired.
        assert!(!window_should_fire(
            &window,
            at(WEDNESDAY, 9, 20),
            Some(&today),
            4
        ));
        // Tomorrow is a fresh day.
        assert!(window_should_fire(
            &window,
            at(WEDNESDAY + 1, 9, 5),
            Some(&today),
            4
        ));
    }

    #[test]
    fn a_window_turned_off_never_fires() {
        let off = with(|w| w.enabled = false);
        assert!(!window_should_fire(&off, at(WEDNESDAY, 9, 5), None, 9));
    }

    #[test]
    fn two_windows_open_on_one_tick_come_back_together_for_one_notification() {
        let windows = [span("a", "09:00", "09:30"), span("b", "09:15", "10:00")];
        let waiting = [On(WORK), On(WORK), On(PERSONAL)];
        let both = windows_to_fire(&windows, at(WEDNESDAY, 9, 20), &none_fired(), &waiting);
        assert_eq!(ids(&both), ["a", "b"]);

        // Minutes apart is a different matter: the second span is a genuinely new event,
        // so once the first has fired only the second is still due.
        let later = windows_to_fire(
            &windows,
            at(WEDNESDAY, 9, 20),
            &fired(&[("a", &local_day(at(WEDNESDAY, 9, 20)))]),
            &waiting,
        );
        assert_eq!(ids(&later), ["b"]);
    }

    #[test]
    fn with_no_schedule_the_app_announces_whenever_it_finds_something() {
        assert!(announcing_allowed_for(
            WORK,
            &[],
            at(WEDNESDAY, 3, 0),
            &none_fired()
        ));
        // A schedule of nothing but disabled windows is no schedule at all.
        let off = [with(|w| w.enabled = false)];
        assert!(announcing_allowed_for(
            WORK,
            &off,
            at(WEDNESDAY, 3, 0),
            &none_fired()
        ));
    }

    #[test]
    fn liveness_begins_when_the_roll_up_does_not_when_the_clock_enters_the_span() {
        let windows = [review_window()];
        let today = local_day(at(WEDNESDAY, 9, 5));

        // Inside the span but the roll-up has not gone out: still quiet, so arrivals push
        // the count rather than pinging one at a time.
        assert!(!announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 9, 5),
            &none_fired()
        ));
        // Once it has fired, the rest of the span is live.
        assert!(announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 9, 5),
            &fired(&[("morning", &today)])
        ));
        // And the moment the span closes it is quiet again, fired or not.
        assert!(!announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 9, 30),
            &fired(&[("morning", &today)])
        ));
        // Yesterday's firing does not make today live.
        assert!(!announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 9, 5),
            &fired(&[("morning", "2026-08-18")])
        ));
    }

    #[test]
    fn the_next_boundary_is_the_next_time_a_window_opens_later_the_same_day() {
        let windows = [
            span("morning", "09:00", "09:30"),
            span("lunch", "12:00", "13:00"),
        ];
        assert_eq!(
            next_window_start(&windows, at(WEDNESDAY, 7, 0)),
            Some(at(WEDNESDAY, 9, 0))
        );
        // Inside the morning span the morning has already opened; lunch is next.
        assert_eq!(
            next_window_start(&windows, at(WEDNESDAY, 9, 15)),
            Some(at(WEDNESDAY, 12, 0))
        );
        assert_eq!(
            next_window_start(&windows, at(WEDNESDAY, 9, 30)),
            Some(at(WEDNESDAY, 12, 0))
        );
    }

    #[test]
    fn the_next_boundary_rolls_over_to_the_next_day_the_schedule_covers() {
        let windows = [review_window()];
        // Wednesday evening, so tomorrow morning.
        assert_eq!(
            next_window_start(&windows, at(WEDNESDAY, 18, 0)),
            Some(at(WEDNESDAY + 1, 9, 0))
        );
        // Friday evening on a weekdays-only schedule has to reach Monday, not Saturday.
        assert_eq!(
            next_window_start(&windows, at(SATURDAY - 1, 18, 0)),
            Some(at(SATURDAY + 2, 9, 0))
        );
    }

    #[test]
    fn the_next_boundary_crosses_a_month_end_like_date_does() {
        // Monday 31 August, evening: Tuesday 1 September is next.
        assert_eq!(
            next_window_start(&[review_window()], at(31, 18, 0)),
            local_to_ms(2026, 9, 1, 9, 0)
        );
    }

    #[test]
    fn a_window_that_covers_only_today_opens_again_a_week_out_not_never() {
        let wednesdays = [with(|w| w.days = vec![3])];
        assert_eq!(
            next_window_start(&wednesdays, at(WEDNESDAY, 10, 0)),
            Some(at(WEDNESDAY + 7, 9, 0))
        );
    }

    #[test]
    fn a_schedule_that_describes_no_span_at_all_has_no_next_boundary() {
        assert_eq!(next_window_start(&[], at(WEDNESDAY, 10, 0)), None);
        assert_eq!(
            next_window_start(&[with(|w| w.enabled = false)], at(WEDNESDAY, 10, 0)),
            None
        );
        assert_eq!(
            next_window_start(&[with(|w| w.days = vec![])], at(WEDNESDAY, 10, 0)),
            None
        );
        // Crosses midnight, so it describes nothing this can open.
        assert_eq!(
            next_window_start(&[span("morning", "22:00", "02:00")], at(WEDNESDAY, 10, 0)),
            None
        );
    }

    #[test]
    fn the_menu_bar_says_nothing_extra_when_there_is_no_schedule_to_be_quiet_for() {
        assert_eq!(quiet_until(&[], &BOTH, &NOTHING, at(WEDNESDAY, 3, 0)), None);
        assert_eq!(
            quiet_until(
                &[with(|w| w.enabled = false)],
                &BOTH,
                &NOTHING,
                at(WEDNESDAY, 3, 0)
            ),
            None
        );
    }

    #[test]
    fn the_menu_bar_says_nothing_extra_while_a_window_is_open() {
        assert_eq!(
            quiet_until(&[review_window()], &BOTH, &NOTHING, at(WEDNESDAY, 9, 0)),
            None
        );
        assert_eq!(
            quiet_until(&[review_window()], &BOTH, &NOTHING, at(WEDNESDAY, 9, 29)),
            None
        );
    }

    #[test]
    fn a_quiet_stretch_says_when_it_lifts_naming_the_day_only_when_it_is_not_today() {
        let windows = [
            span("morning", "09:00", "09:30"),
            span("lunch", "12:00", "13:00"),
        ];
        let quiet = |now: i64| quiet_until(&windows, &BOTH, &NOTHING, now);
        assert_eq!(quiet(at(WEDNESDAY, 8, 47)).as_deref(), Some("09:00"));
        assert_eq!(quiet(at(WEDNESDAY, 9, 30)).as_deref(), Some("12:00"));
        assert_eq!(quiet(at(WEDNESDAY, 18, 0)).as_deref(), Some("Thu 09:00"));
        // Friday evening on a weekdays-only schedule reads as Monday morning.
        assert_eq!(quiet(at(SATURDAY - 1, 18, 0)).as_deref(), Some("Mon 09:00"));
        assert_eq!(quiet(at(SATURDAY, 11, 0)).as_deref(), Some("Mon 09:00"));
    }

    #[test]
    fn a_window_naming_no_account_covers_every_one_including_one_added_later() {
        let all = review_window();
        assert!(window_covers(&all, WORK));
        assert!(window_covers(&all, PERSONAL));
        assert!(window_covers(&all, "connected-next-march"));
    }

    #[test]
    fn a_window_naming_accounts_covers_those_and_no_others() {
        let work = with(|w| w.accounts = vec![WORK.into()]);
        assert!(window_covers(&work, WORK));
        assert!(!window_covers(&work, PERSONAL));
    }

    #[test]
    fn a_window_left_scoped_to_accounts_that_are_gone_covers_nothing() {
        let orphan = with(|w| w.accounts = vec!["signed-out-months-ago".into()]);
        assert!(covers_nothing(&orphan, &BOTH));
        assert!(!window_covers(&orphan, WORK));
        // Which is what stops it firing: its scope is empty, so its count is always zero.
        assert_eq!(
            reviews_in_scope(&orphan, &[On(WORK), On(PERSONAL)]).len(),
            0
        );
        assert!(
            windows_to_fire(
                std::slice::from_ref(&orphan),
                at(WEDNESDAY, 9, 5),
                &none_fired(),
                &[On(WORK), On(PERSONAL)]
            )
            .is_empty()
        );
        // An all-accounts window is never in that state, however few accounts there are.
        assert!(!covers_nothing(&review_window(), &[] as &[&str]));
    }

    #[test]
    fn a_review_no_enabled_window_covers_pings_live_whatever_the_clock_says() {
        let work = [with(|w| w.accounts = vec![WORK.into()])];
        // Deep in the night, outside every span: the covered account is quiet...
        assert!(!announcing_allowed_for(
            WORK,
            &work,
            at(WEDNESDAY, 3, 0),
            &none_fired()
        ));
        // ...and the one nothing claims behaves as it did before any of this existed.
        assert!(announcing_allowed_for(
            PERSONAL,
            &work,
            at(WEDNESDAY, 3, 0),
            &none_fired()
        ));
    }

    #[test]
    fn liveness_is_the_union_across_the_windows_covering_an_account() {
        let today = local_day(at(WEDNESDAY, 12, 30));
        let windows = [
            with(|w| {
                w.id = "work-morning".into();
                w.accounts = vec![WORK.into()];
            }),
            span("everything-lunch", "12:00", "13:00"),
        ];
        // Lunch has fired and covers everything, so both accounts are live inside it.
        let lunch = fired(&[("everything-lunch", &today)]);
        assert!(announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 12, 30),
            &lunch
        ));
        assert!(announcing_allowed_for(
            PERSONAL,
            &windows,
            at(WEDNESDAY, 12, 30),
            &lunch
        ));
        // The morning window having fired says nothing about the afternoon.
        let morning = fired(&[("work-morning", &today)]);
        assert!(!announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 12, 30),
            &morning
        ));
        assert!(announcing_allowed_for(
            WORK,
            &windows,
            at(WEDNESDAY, 9, 5),
            &morning
        ));
        // Personal is covered only by lunch, so the morning span leaves it quiet.
        assert!(!announcing_allowed_for(
            PERSONAL,
            &windows,
            at(WEDNESDAY, 9, 5),
            &morning
        ));
    }

    #[test]
    fn a_window_counts_its_threshold_over_its_own_scope_not_the_whole_deck() {
        let work = [with(|w| {
            w.accounts = vec![WORK.into()];
            w.minimum = 2;
        })];
        let busy_elsewhere = [On(PERSONAL), On(PERSONAL), On(PERSONAL), On(WORK)];
        assert!(
            windows_to_fire(&work, at(WEDNESDAY, 9, 5), &none_fired(), &busy_elsewhere).is_empty()
        );
        // A second review on the account it actually watches is what trips it.
        let more = [On(PERSONAL), On(PERSONAL), On(PERSONAL), On(WORK), On(WORK)];
        let due = windows_to_fire(&work, at(WEDNESDAY, 9, 5), &none_fired(), &more);
        assert_eq!(ids(&due), ["morning"]);
    }

    #[test]
    fn windows_of_differing_scope_firing_on_one_tick_come_back_together() {
        let windows = [
            with(|w| {
                w.id = "work".into();
                w.accounts = vec![WORK.into()];
            }),
            with(|w| {
                w.id = "personal".into();
                w.accounts = vec![PERSONAL.into()];
            }),
            with(|w| {
                w.id = "quiet-one".into();
                w.accounts = vec!["nobody".into()];
            }),
        ];
        let due = windows_to_fire(
            &windows,
            at(WEDNESDAY, 9, 5),
            &none_fired(),
            &[On(WORK), On(PERSONAL)],
        );
        // One notification, over the union of what these two cover; the third covers
        // nothing and stays out of it.
        assert_eq!(ids(&due), ["work", "personal"]);
    }

    #[test]
    fn windows_to_fire_reads_a_btree_record_and_borrowed_reviews() {
        let record: BTreeMap<String, String> =
            [("morning".to_string(), local_day(at(WEDNESDAY, 9, 5)))].into();
        let work = On(WORK);
        // A list of references, the shape `visible_reviews` hands back.
        let borrowed = [&work];
        assert!(
            windows_to_fire(&[review_window()], at(WEDNESDAY, 9, 5), &record, &borrowed).is_empty()
        );
        assert_eq!(
            windows_to_fire(
                &[review_window()],
                at(WEDNESDAY + 1, 9, 5),
                &record,
                &borrowed
            )
            .len(),
            1
        );
    }

    #[test]
    fn the_scope_clause_names_the_accounts_or_says_there_are_none_left() {
        assert_eq!(describe_scope(&review_window(), &SIGNED_IN), "All accounts");
        assert_eq!(
            describe_scope(&with(|w| w.accounts = vec![WORK.into()]), &SIGNED_IN),
            "Work GitHub"
        );
        assert_eq!(
            describe_scope(
                &with(|w| w.accounts = vec![WORK.into(), PERSONAL.into()]),
                &SIGNED_IN
            ),
            "Work GitHub, Personal Forgejo"
        );
        assert_eq!(
            describe_scope(&with(|w| w.accounts = vec!["gone".into()]), &SIGNED_IN),
            "Covers no account"
        );
    }

    #[test]
    fn the_quiet_line_follows_the_accounts_a_schedule_actually_claims() {
        let work = [with(|w| w.accounts = vec![WORK.into()])];
        // Work is quiet and a work review is waiting for it: say when that lifts.
        assert_eq!(
            quiet_until(&work, &BOTH, &[On(WORK)], at(WEDNESDAY, 8, 0)).as_deref(),
            Some("09:00")
        );
        // Only an unclaimed account is waiting, and that pinged as it landed - there is
        // no silence to account for, so the line stays away.
        assert_eq!(
            quiet_until(&work, &BOTH, &[On(PERSONAL)], at(WEDNESDAY, 8, 0)),
            None
        );
        // Nothing waiting at all still reassures: the schedule is holding the line.
        assert_eq!(
            quiet_until(&work, &BOTH, &NOTHING, at(WEDNESDAY, 8, 0)).as_deref(),
            Some("09:00")
        );
        // Nothing claims personal, so on its own it is never a reason to be quiet.
        assert_eq!(
            quiet_until(&work, &[PERSONAL], &NOTHING, at(WEDNESDAY, 8, 0)),
            None
        );
    }

    #[test]
    fn the_quiet_line_reports_the_soonest_return_across_the_accounts_being_held() {
        let windows = [
            with(|w| {
                w.id = "work".into();
                w.accounts = vec![WORK.into()];
            }),
            with(|w| {
                w.id = "personal".into();
                w.accounts = vec![PERSONAL.into()];
                w.start = "17:00".into();
                w.end = "18:00".into();
            }),
        ];
        // Both held at 08:00; work comes back first and that is the whole answer.
        assert_eq!(
            quiet_until(&windows, &BOTH, &NOTHING, at(WEDNESDAY, 8, 0)).as_deref(),
            Some("09:00")
        );
        // Inside the work span, only personal is still held, so its own time is next.
        assert_eq!(
            quiet_until(&windows, &BOTH, &NOTHING, at(WEDNESDAY, 9, 10)).as_deref(),
            Some("17:00")
        );
    }

    #[test]
    fn local_day_names_the_local_calendar_day_zero_padded() {
        assert_eq!(
            local_day(local_to_ms(2026, 1, 5, 23, 30).expect("representable")),
            "2026-01-05"
        );
        assert_eq!(
            local_day(local_to_ms(2026, 12, 31, 0, 1).expect("representable")),
            "2026-12-31"
        );
    }

    #[test]
    fn a_window_has_to_be_saveable_before_it_can_be_saved() {
        let problem = |patch: fn(&mut ReviewWindow)| window_problem(&with(patch)).unwrap_or("");
        assert_eq!(window_problem(&review_window()), None);
        assert!(problem(|w| w.days = vec![]).contains("at least one day"));
        assert!(problem(|w| w.end = "09:00".into()).contains("after the start"));
        assert!(problem(|w| w.end = "08:00".into()).contains("after the start"));
        assert!(problem(|w| w.end = String::new()).contains("Both times"));
        assert!(problem(|w| w.minimum = 0).contains("at least one review"));
        assert!(problem(|w| w.minimum = -3).contains("at least one review"));
    }

    #[test]
    fn a_row_says_its_own_schedule_in_plain_language() {
        assert_eq!(
            describe_window(&review_window(), &SIGNED_IN),
            "Mon-Fri · 09:00-09:30 · 1+ waiting · All accounts"
        );
        assert_eq!(
            describe_window(
                &with(|w| {
                    w.days = vec![6, 0];
                    w.start = "11:00".into();
                    w.end = "12:00".into();
                    w.minimum = 3;
                }),
                &SIGNED_IN
            ),
            "Sat, Sun · 11:00-12:00 · 3+ waiting · All accounts"
        );
        assert_eq!(
            describe_window(&with(|w| w.accounts = vec![WORK.into()]), &SIGNED_IN),
            "Mon-Fri · 09:00-09:30 · 1+ waiting · Work GitHub"
        );
    }

    #[test]
    fn days_read_as_a_working_week_rather_than_as_the_numbers_underneath() {
        assert_eq!(describe_days(&[0, 1, 2, 3, 4, 5, 6]), "Every day");
        assert_eq!(describe_days(&WEEKDAYS), "Mon-Fri");
        assert_eq!(describe_days(&[1]), "Mon");
        assert_eq!(describe_days(&[1, 3, 5]), "Mon, Wed, Fri");
        assert_eq!(describe_days(&[6, 0]), "Sat, Sun");
        // Sunday closes the week here, so it never joins a run that starts on Monday.
        assert_eq!(describe_days(&[0, 1, 2]), "Mon, Tue, Sun");
        assert_eq!(describe_days(&[]), "Never");
    }

    #[test]
    fn day_name_answers_a_question_mark_outside_the_week() {
        assert_eq!(day_name(0), "Sun");
        assert_eq!(day_name(6), "Sat");
        assert_eq!(day_name(7), "?");
    }
}
