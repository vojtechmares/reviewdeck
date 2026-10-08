//! Dates and times, the way the TypeScript app handles them.
//!
//! Timestamps stay ISO-8601 strings in the model, exactly as the hosts send them and
//! as `Date#toISOString` writes them, because the app compares and sorts them as
//! strings. This module turns them into numbers when arithmetic is needed:
//! milliseconds since the Unix epoch, which is what `Date#getTime` returns.
//!
//! The local calendar (what `Date#getDay`, `getHours` and `new Date(y, m, d, h, mi)`
//! read) goes through the C library's `localtime_r` and `mktime`, which consult the
//! same time zone database as everything else on the machine.

use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_DAY: i64 = 86_400_000;

/// Milliseconds since the Unix epoch, now. `Date.now()`.
pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        // A clock set before 1970.
        Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// The current moment as `YYYY-MM-DDTHH:MM:SS.sssZ`. `new Date().toISOString()`.
pub fn now_iso() -> String {
    format_iso(now_ms())
}

/// A moment as `Date#toISOString` writes it: UTC, millisecond precision, `Z`.
///
/// Years outside 0..=9999 take the expanded six-digit form with a sign, as in
/// JavaScript, so the string still sorts and parses.
pub fn format_iso(ms: i64) -> String {
    let days = ms.div_euclid(MS_PER_DAY);
    let of_day = ms.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = of_day / 3_600_000;
    let minute = of_day / 60_000 % 60;
    let second = of_day / 1000 % 60;
    let milli = of_day % 1000;
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else if year < 0 {
        format!("-{:06}", -year)
    } else {
        format!("+{year:06}")
    };
    format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// Reads an ISO-8601 timestamp into milliseconds since the epoch, or `None` when it
/// is not one - where `new Date(iso).getTime()` would give `NaN`.
///
/// Accepts what the hosts send and what `Date` accepts of it:
/// - `YYYY-MM-DD` (midnight UTC, as `Date` reads a bare date);
/// - a date and a time joined by `T` (or `t`, or a space): `HH:MM`, optionally
///   `:SS`, optionally a fraction of any length, which is truncated to the
///   millisecond as V8 does;
/// - then `Z`, `±HH:MM` or `±HHMM`, or nothing at all, which means local time;
/// - six-digit signed years (`+002026-...`).
///
/// A day past the end of its month rolls into the next one (`02-30` is March 2nd),
/// matching V8, and `24:00` is the midnight that ends the day.
pub fn parse_iso(input: &str) -> Option<i64> {
    let mut cursor = Cursor {
        bytes: input.as_bytes(),
        at: 0,
    };

    let year = match cursor.peek()? {
        sign @ (b'+' | b'-') => {
            cursor.at += 1;
            let digits = cursor.digits(6)?;
            // "-000000" is the one spelling of year zero the format forbids.
            if sign == b'-' && digits == 0 {
                return None;
            }
            if sign == b'-' { -digits } else { digits }
        }
        _ => cursor.digits(4)?,
    };
    cursor.expect(b'-')?;
    let month = cursor.digits(2)?;
    cursor.expect(b'-')?;
    let day = cursor.digits(2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, 1) + day - 1;

    if cursor.done() {
        return Some(days * MS_PER_DAY);
    }

    match cursor.next()? {
        b'T' | b't' | b' ' => {}
        _ => return None,
    }
    let hour = cursor.digits(2)?;
    cursor.expect(b':')?;
    let minute = cursor.digits(2)?;
    let mut second = 0;
    let mut milli = 0;
    if cursor.peek() == Some(b':') {
        cursor.at += 1;
        second = cursor.digits(2)?;
        if cursor.peek() == Some(b'.') {
            cursor.at += 1;
            let start = cursor.at;
            while cursor.peek().is_some_and(|b| b.is_ascii_digit()) {
                cursor.at += 1;
            }
            let fraction = &cursor.bytes[start..cursor.at];
            if fraction.is_empty() {
                return None;
            }
            // Truncated, not rounded: ".9999" is 999 ms.
            for place in 0..3 {
                milli = milli * 10 + fraction.get(place).map_or(0, |b| i64::from(b - b'0'));
            }
        }
    }
    if hour > 24 || minute > 59 || second > 59 {
        return None;
    }
    if hour == 24 && (minute != 0 || second != 0 || milli != 0) {
        return None;
    }

    let offset_minutes = match cursor.next() {
        None => {
            // No offset on a date-time means the local wall clock.
            let ms = local_fields_to_ms(
                i32::try_from(year).ok()?,
                i32::try_from(month).ok()?,
                i32::try_from(day).ok()?,
                i32::try_from(hour).ok()?,
                i32::try_from(minute).ok()?,
                i32::try_from(second).ok()?,
            )?;
            return Some(ms + milli);
        }
        Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let hours = cursor.digits(2)?;
            if cursor.peek() == Some(b':') {
                cursor.at += 1;
            }
            let minutes = cursor.digits(2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let total = hours * 60 + minutes;
            if sign == b'-' { -total } else { total }
        }
        Some(_) => return None,
    };
    if !cursor.done() {
        return None;
    }

    let wall = days * MS_PER_DAY + ((hour * 60 + minute) * 60 + second) * 1000 + milli;
    Some(wall - offset_minutes * 60_000)
}

/// A moment on the local calendar, as `Date`'s local getters read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12. Note `Date#getMonth` counts from 0; this does not.
    pub month: u32,
    /// Day of the month, 1 to 31. `Date#getDate`.
    pub day: u32,
    /// 0 is Sunday, as `Date#getDay` numbers them.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

/// The local calendar fields of a moment.
pub fn local_parts(ms: i64) -> LocalTime {
    // `time_t` is 64-bit on macOS, so this cast is lossless there.
    let seconds = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: `tm` is plain old data, so all-zero is a valid value; it is only read
    // after `localtime_r` reports success by returning non-null.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers come from live locals for the duration of the call, and
    // `localtime_r` is the thread-safe variant that writes only into `tm`.
    let filled = unsafe { libc::localtime_r(&seconds, &mut tm) };
    if !filled.is_null() {
        return LocalTime {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            weekday: tm.tm_wday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
        };
    }
    // Only a moment outside what the C library can represent ends up here; UTC is a
    // better answer than none.
    let days = ms.div_euclid(MS_PER_DAY);
    let of_day = ms.rem_euclid(MS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    LocalTime {
        year: year as i32,
        month: month as u32,
        day: day as u32,
        weekday: (days + 4).rem_euclid(7) as u32,
        hour: (of_day / 3_600_000) as u32,
        minute: (of_day / 60_000 % 60) as u32,
    }
}

/// The moment a local wall clock reads the given time - `new Date(year, month - 1,
/// day, hour, minute).getTime()`. `month` counts from 1.
///
/// Fields out of range carry over as they do in `Date`: day 32 of August is the 1st
/// of September, minute 540 of a day is 09:00. `None` when the C library cannot
/// represent the result.
pub fn local_to_ms(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> Option<i64> {
    local_fields_to_ms(year, month, day, hour, minute, 0)
}

fn local_fields_to_ms(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
) -> Option<i64> {
    // SAFETY: `tm` is plain old data, so all-zero is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year.checked_sub(1900)?;
    tm.tm_mon = month.checked_sub(1)?;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = second;
    // Let the time zone database decide whether daylight saving is in force.
    tm.tm_isdst = -1;
    // `mktime` returns -1 both for failure and for one second before the epoch; a
    // successful call always fills in the day of the year, so that tells them apart.
    tm.tm_yday = -1;
    // SAFETY: `tm` is a live local; `mktime` reads it and normalises it in place.
    let seconds = unsafe { libc::mktime(&mut tm) };
    if seconds == -1 && tm.tm_yday == -1 {
        return None;
    }
    (seconds as i64).checked_mul(1000)
}

/// How long ago a timestamp was, in the deck's narrow style: "just now", "5m ago",
/// "yesterday", "last wk.", "3mo ago".
///
/// The `relativeTime` of renderer/src/lib/utils.ts, which formats with
/// `Intl.RelativeTimeFormat(undefined, { numeric: 'auto', style: 'narrow' })`; these
/// are that formatter's English strings. An unreadable timestamp is the empty
/// string, and one in the future is "just now".
pub fn relative_time(iso: &str, now_ms: i64) -> String {
    let Some(then) = parse_iso(iso) else {
        return String::new();
    };
    let seconds = js_round((now_ms - then) as f64 / 1000.0);
    if seconds < 45.0 {
        return "just now".into();
    }
    const UNITS: [(f64, f64, Unit); 7] = [
        (60.0, 1.0, Unit::Second),
        (3600.0, 60.0, Unit::Minute),
        (86400.0, 3600.0, Unit::Hour),
        (604800.0, 86400.0, Unit::Day),
        (2629800.0, 604800.0, Unit::Week),
        (31557600.0, 2629800.0, Unit::Month),
        (f64::INFINITY, 31557600.0, Unit::Year),
    ];
    for (limit, divisor, unit) in UNITS {
        if seconds < limit {
            return narrow_ago(js_round(seconds / divisor) as i64, unit);
        }
    }
    String::new()
}

#[derive(Clone, Copy)]
enum Unit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// `Intl.RelativeTimeFormat('en', { numeric: 'auto', style: 'narrow' })` for a
/// count in the past (always at least 1 here).
fn narrow_ago(count: i64, unit: Unit) -> String {
    match (unit, count) {
        (Unit::Day, 1) => "yesterday".into(),
        (Unit::Week, 1) => "last wk.".into(),
        (Unit::Month, 1) => "last mo.".into(),
        (Unit::Year, 1) => "last yr.".into(),
        (Unit::Second, n) => format!("{n}s ago"),
        (Unit::Minute, n) => format!("{n}m ago"),
        (Unit::Hour, n) => format!("{n}h ago"),
        (Unit::Day, n) => format!("{n}d ago"),
        (Unit::Week, n) => format!("{n}w ago"),
        (Unit::Month, n) => format!("{n}mo ago"),
        (Unit::Year, n) => format!("{n}y ago"),
    }
}

/// `Math.round`: halves go up, towards positive infinity (`f64::round` takes them
/// away from zero instead).
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A proleptic Gregorian (year, month, day) to days since 1970-01-01.
/// Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.next()? == byte).then_some(())
    }

    /// Exactly `count` ASCII digits.
    fn digits(&mut self, count: usize) -> Option<i64> {
        let mut value = 0i64;
        for _ in 0..count {
            let byte = self.next()?;
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + i64::from(byte - b'0');
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_iso_matches_to_iso_string() {
        assert_eq!(format_iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_iso(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(format_iso(1_767_261_600_123), "2026-01-01T10:00:00.123Z");
        assert_eq!(
            format_iso(-62_324_985_600_000),
            "-000005-01-01T00:00:00.000Z"
        );
        assert_eq!(
            format_iso(253_402_300_800_000),
            "+010000-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn now_iso_has_the_shape_of_to_iso_string() {
        let now = now_iso();
        assert_eq!(now.len(), 24, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(&now[10..11], "T");
        let parsed = parse_iso(&now).expect("now_iso parses back");
        assert!((now_ms() - parsed).abs() < 5_000);
    }

    #[test]
    fn parse_iso_reads_what_the_hosts_send() {
        // GitHub, GitLab, Bitbucket and Forgejo shapes.
        assert_eq!(parse_iso("2026-01-01T10:00:00Z"), Some(1_767_261_600_000));
        assert_eq!(
            parse_iso("2026-01-01T10:00:00.123Z"),
            Some(1_767_261_600_123)
        );
        assert_eq!(
            parse_iso("2026-01-01T10:00:00.123456+00:00"),
            Some(1_767_261_600_123)
        );
        assert_eq!(
            parse_iso("2026-01-01T10:00:00+02:00"),
            Some(1_767_254_400_000)
        );
        assert_eq!(
            parse_iso("2026-01-01T10:00:00+0200"),
            Some(1_767_254_400_000)
        );
        assert_eq!(
            parse_iso("2026-01-01T10:00:00.5-01:30"),
            Some(1_767_267_000_500)
        );
    }

    #[test]
    fn parse_iso_agrees_with_v8_on_the_edges() {
        // Every expectation here is what `new Date(s).getTime()` gives in Node.
        assert_eq!(parse_iso("2026-02-30T00:00:00Z"), Some(1_772_409_600_000));
        assert_eq!(parse_iso("2026-02-29T00:00:00Z"), Some(1_772_323_200_000));
        assert_eq!(parse_iso("2026-01-01T24:00:00Z"), Some(1_767_312_000_000));
        assert_eq!(parse_iso("2026-01-01T24:00:01Z"), None);
        assert_eq!(parse_iso("2026-01-01T10:00Z"), Some(1_767_261_600_000));
        assert_eq!(
            parse_iso("2026-01-01T10:00:00.123456789Z"),
            Some(1_767_261_600_123)
        );
        assert_eq!(
            parse_iso("2026-01-01T10:00:00.9999Z"),
            Some(1_767_261_600_999)
        );
        assert_eq!(parse_iso("2026-01-01T10:00:00,5Z"), None);
        assert_eq!(parse_iso("2026-01-01t10:00:00z"), Some(1_767_261_600_000));
        assert_eq!(parse_iso("2026-01-01 10:00:00Z"), Some(1_767_261_600_000));
        assert_eq!(parse_iso("2026-01-01T10:00:00+02"), None);
        assert_eq!(parse_iso("2026-01-01T10:00:00.Z"), None);
        assert_eq!(
            parse_iso("+002026-01-01T10:00:00Z"),
            Some(1_767_261_600_000)
        );
        assert_eq!(parse_iso("-000000-01-01T00:00:00Z"), None);
        assert_eq!(parse_iso("2026-1-01T10:00:00Z"), None);
        assert_eq!(parse_iso("2026-01-01T10:00:60Z"), None);
        assert_eq!(parse_iso("2026-13-01T10:00:00Z"), None);
        assert_eq!(parse_iso("2026-01-01"), Some(1_767_225_600_000));
        assert_eq!(parse_iso(""), None);
        assert_eq!(parse_iso("nonsense"), None);
        assert_eq!(parse_iso("2026-01-01T10:00:00Zjunk"), None);
    }

    #[test]
    fn a_date_time_without_an_offset_is_local() {
        assert_eq!(
            parse_iso("2026-01-01T10:00:00"),
            local_to_ms(2026, 1, 1, 10, 0)
        );
        assert_eq!(
            parse_iso("2026-01-01T10:00:00.250"),
            local_to_ms(2026, 1, 1, 10, 0).map(|ms| ms + 250)
        );
    }

    #[test]
    fn format_and_parse_round_trip() {
        for ms in [
            0,
            -1,
            1_767_261_600_123,
            1_000_000_000_000,
            -62_324_985_600_000,
        ] {
            assert_eq!(parse_iso(&format_iso(ms)), Some(ms));
        }
    }

    #[test]
    fn local_calendar_round_trips() {
        let ms = local_to_ms(2026, 8, 3, 9, 30).expect("representable");
        assert_eq!(
            local_parts(ms),
            LocalTime {
                year: 2026,
                month: 8,
                day: 3,
                // 3 August 2026 is a Monday.
                weekday: 1,
                hour: 9,
                minute: 30,
            }
        );
    }

    #[test]
    fn local_fields_carry_over_like_date() {
        // `new Date(2026, 7, 32, 0, 540)` is 1 September, 09:00.
        assert_eq!(
            local_to_ms(2026, 8, 32, 0, 540),
            local_to_ms(2026, 9, 1, 9, 0)
        );
        assert_eq!(
            local_to_ms(2026, 13, 1, 0, 0),
            local_to_ms(2027, 1, 1, 0, 0)
        );
    }

    #[test]
    fn relative_time_matches_the_renderer() {
        // Expectations from the TypeScript `relativeTime` with `Date.now()` pinned.
        let now = parse_iso("2026-08-10T12:00:00Z").expect("valid");
        for (iso, expected) in [
            ("2026-08-10T12:00:00Z", "just now"),
            ("2026-08-10T11:59:15.500Z", "45s ago"),
            ("2026-08-10T11:59:14Z", "46s ago"),
            ("2026-08-10T11:59:00Z", "1m ago"),
            ("2026-08-10T11:00:31Z", "59m ago"),
            ("2026-08-10T11:00:29Z", "60m ago"),
            ("2026-08-10T11:00:00Z", "1h ago"),
            ("2026-08-09T12:00:01Z", "24h ago"),
            ("2026-08-09T12:00:00Z", "yesterday"),
            ("2026-08-08T12:00:00Z", "2d ago"),
            ("2026-08-03T12:00:01Z", "7d ago"),
            ("2026-08-03T12:00:00Z", "last wk."),
            ("2026-07-20T12:00:00Z", "3w ago"),
            ("2026-07-10T12:00:00Z", "last mo."),
            ("2026-06-01T12:00:00Z", "2mo ago"),
            ("2025-09-01T00:00:00Z", "11mo ago"),
            ("2025-08-01T00:00:00Z", "last yr."),
            ("2020-08-01T00:00:00Z", "6y ago"),
            ("2026-08-11T00:00:00Z", "just now"),
            ("nonsense", ""),
        ] {
            assert_eq!(relative_time(iso, now), expected, "{iso}");
        }
    }

    #[test]
    fn js_round_takes_halves_up() {
        assert_eq!(js_round(44.5), 45.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(-2.6), -3.0);
        assert_eq!(js_round(2.4), 2.0);
    }
}
