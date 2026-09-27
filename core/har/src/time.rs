//! The one date format HAR uses, written and read without a date library.
//!
//! ```text
//! Timestamp { seconds, nanos }  ──▶  "2026-09-20T10:11:12.345Z"
//!                               ◀──  "2026-09-20T12:11:12.345+02:00"
//! ```
//!
//! HAR 1.2 spells `startedDateTime` as ISO 8601 — `YYYY-MM-DDThh:mm:ss.sTZD`
//! — and that is the only date in the format. `proxy-storage` keeps a moment
//! as seconds and nanoseconds since the Unix epoch precisely so that it needs
//! no date library to produce, so the conversion between the two lives here
//! and is thirty lines of civil-calendar arithmetic rather than a dependency.
//!
//! What is written is always UTC, with milliseconds: one spelling, so two
//! exports of the same exchange are the same bytes. What is *read* is more
//! generous, because the file may have been written by anything: a `Z` or a
//! `+hh:mm` offset, any number of fractional digits or none, and a lowercase
//! `t` or `z`.
//!
//! # Range
//!
//! Every year from 1 to 9999, which is the whole of what a four-digit ISO
//! 8601 year can say. A moment outside it cannot be written as this format,
//! and a captured exchange never is — the clock would have to be set to
//! something implausible — so [`format`] clamps rather than failing: losing
//! an exchange from an export over a nonsensical timestamp would be the
//! worse answer.

use proxy_storage::Timestamp;

/// Seconds in one day.
const SECONDS_PER_DAY: i64 = 86_400;

/// The last second this format can write: 9999-12-31T23:59:59Z.
const MAX_SECONDS: i64 = 253_402_300_799;

/// `time` as HAR spells it: UTC, to the millisecond, ending in `Z`.
pub fn format(time: Timestamp) -> String {
    let seconds = (time.seconds.min(MAX_SECONDS as u64)) as i64;
    let millis = time.nanos / 1_000_000;

    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let within = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        within / 3_600,
        (within % 3_600) / 60,
        within % 60,
    )
}

/// Reads a moment written as ISO 8601, or says it is not one.
///
/// A moment before the Unix epoch becomes the epoch, for the same reason
/// [`Timestamp::at`] does it: a [`Timestamp`] counts from the epoch and
/// cannot say "before it", and a HAR entry dated 1969 is a file to read
/// rather than a file to refuse.
pub fn parse(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    let bytes = text.as_bytes();

    // YYYY-MM-DDThh:mm:ss is the shortest form there is.
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }

    let year = number(&text[0..4])?;
    let month = number(&text[5..7])?;
    let day = number(&text[8..10])?;
    let hour = number(&text[11..13])?;
    let minute = number(&text[14..16])?;
    let second = number(&text[17..19])?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // A leap second is written `:60`, and is a second that a count from the
    // epoch does not have. It is read as the last second of the minute
    // rather than refused: an entry is not worth losing over it.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let (nanos, rest) = fraction(&text[19..])?;
    let offset = zone(rest)?;

    let days = days_from_civil(year, month as u32, day as u32);
    let seconds = days * SECONDS_PER_DAY + hour * 3_600 + minute * 60 + second.min(59) - offset;

    Some(Timestamp {
        seconds: seconds.max(0) as u64,
        nanos,
    })
}

/// Reads the optional fractional seconds, and gives back what follows them.
fn fraction(text: &str) -> Option<(u32, &str)> {
    let Some(rest) = text.strip_prefix('.').or_else(|| text.strip_prefix(',')) else {
        return Some((0, text));
    };

    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }

    // Nine digits is a nanosecond; anything past that is finer than this
    // model can hold and is dropped rather than rounded.
    let mut nanos: u32 = 0;
    for (place, digit) in rest.as_bytes()[..digits.min(9)].iter().enumerate() {
        nanos += u32::from(digit - b'0') * 10u32.pow(8 - place as u32);
    }

    Some((nanos, &rest[digits..]))
}

/// Reads the zone designator, as the number of seconds to subtract.
///
/// A designator is required: an ISO 8601 moment with no zone is a local time,
/// and reading one as UTC would silently move the entry by however many hours
/// the writer's machine was from it.
fn zone(text: &str) -> Option<i64> {
    if matches!(text, "Z" | "z") {
        return Some(0);
    }

    let bytes = text.as_bytes();
    if bytes.len() != 6 && bytes.len() != 5 {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };

    let hours = number(&text[1..3])?;
    let minutes = if bytes.len() == 6 {
        if bytes[3] != b':' {
            return None;
        }
        number(&text[4..6])?
    } else {
        number(&text[3..5])?
    };

    if hours > 23 || minutes > 59 {
        return None;
    }

    Some(sign * (hours * 3_600 + minutes * 60))
}

/// Reads a run of ASCII digits as a number.
fn number(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    text.parse().ok()
}

/// The civil date `days` days after 1970-01-01.
///
/// Howard Hinnant's `civil_from_days`, which is the algorithm the C++
/// standard's own calendar support is specified with: exact for every day in
/// the proleptic Gregorian calendar, and no table.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = month_position + if month_position < 10 { 3 } else { -9 };

    (year + i64::from(month <= 2), month, day)
}

/// How many days after 1970-01-01 a civil date is.
///
/// The exact inverse of [`civil_from_days`], from the same place.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64, nanos: u32) -> Timestamp {
        Timestamp { seconds, nanos }
    }

    #[test]
    fn the_epoch_is_written_as_the_epoch() {
        assert_eq!(format(at(0, 0)), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_moment_is_written_to_the_millisecond_in_utc() {
        assert_eq!(
            format(at(1_700_000_000, 123_456_789)),
            "2023-11-14T22:13:20.123Z"
        );
    }

    #[test]
    fn a_leap_day_is_written_as_one() {
        // 2024-02-29T00:00:00Z.
        assert_eq!(format(at(1_709_164_800, 0)), "2024-02-29T00:00:00.000Z");
    }

    #[test]
    fn a_moment_beyond_the_format_is_clamped_rather_than_lost() {
        let written = format(at(u64::MAX, 0));

        assert_eq!(written, "9999-12-31T23:59:59.000Z");
        assert!(parse(&written).is_some());
    }

    #[test]
    fn what_is_written_reads_back_as_what_was_written() {
        for seconds in [
            0,
            1,
            1_000_000,
            1_700_000_000,
            4_102_444_800,
            MAX_SECONDS as u64,
        ] {
            let time = at(seconds, 500_000_000);
            let back = parse(&format(time)).expect("reads back");

            assert_eq!(back.seconds, seconds);
            assert_eq!(back.nanos, 500_000_000);
        }
    }

    #[test]
    fn every_day_of_four_centuries_round_trips_through_the_calendar() {
        // The two calendar functions are inverses or they are wrong; this is
        // the whole Gregorian cycle, day by day.
        for days in -25_567..120_000 {
            let (year, month, day) = civil_from_days(days);

            assert_eq!(days_from_civil(year, month as u32, day as u32), days);
        }
    }

    #[test]
    fn an_offset_is_applied_rather_than_ignored() {
        let utc = parse("2026-09-20T10:11:12.345Z").expect("a moment");
        let ahead = parse("2026-09-20T12:11:12.345+02:00").expect("a moment");
        let behind = parse("2026-09-20T05:11:12.345-05:00").expect("a moment");

        assert_eq!(ahead, utc);
        assert_eq!(behind, utc);
    }

    #[test]
    fn an_offset_written_without_a_colon_is_read_too() {
        assert_eq!(
            parse("2026-09-20T12:11:12+0200"),
            parse("2026-09-20T10:11:12Z")
        );
    }

    #[test]
    fn fractional_seconds_are_optional_and_of_any_length() {
        assert_eq!(parse("2026-09-20T10:11:12Z").expect("a moment").nanos, 0);
        assert_eq!(
            parse("2026-09-20T10:11:12.5Z").expect("a moment").nanos,
            500_000_000
        );
        assert_eq!(
            parse("2026-09-20T10:11:12.123456789123Z")
                .expect("a moment")
                .nanos,
            123_456_789
        );
    }

    #[test]
    fn a_lowercase_separator_is_read() {
        assert_eq!(parse("2026-09-20t10:11:12z"), parse("2026-09-20T10:11:12Z"));
    }

    #[test]
    fn a_moment_before_the_epoch_becomes_the_epoch() {
        assert_eq!(parse("1969-07-20T20:17:00Z"), Some(at(0, 0)));
    }

    #[test]
    fn a_moment_with_no_zone_is_refused() {
        // A local time read as UTC would move the entry by however many hours
        // the machine that wrote it was from UTC.
        assert_eq!(parse("2026-09-20T10:11:12"), None);
        assert_eq!(parse("2026-09-20T10:11:12.345"), None);
    }

    #[test]
    fn text_that_is_not_a_moment_is_refused() {
        for text in [
            "",
            "yesterday",
            "2026-09-20",
            "2026-13-20T10:11:12Z",
            "2026-09-32T10:11:12Z",
            "2026-09-20T25:11:12Z",
            "2026-09-20T10:61:12Z",
            "2026-09-20T10:11:12.Z",
            "2026-09-20T10:11:12+25:00",
            "2026/09/20T10:11:12Z",
        ] {
            assert_eq!(parse(text), None, "{text} was read as a moment");
        }
    }

    #[test]
    fn a_leap_second_is_read_as_the_last_second_of_its_minute() {
        assert_eq!(parse("2016-12-31T23:59:60Z"), parse("2016-12-31T23:59:59Z"));
    }
}
