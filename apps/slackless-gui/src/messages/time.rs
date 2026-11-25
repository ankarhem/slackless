//! Local-time & civil-calendar formatting (zero chrono/time dependency; cached `localtime_r`).

use std::cell::RefCell;

use slackless_core::types::Ts;
use slackless_core::types::time as core_time;

use super::MONTHS;

// ---------------------------------------------------------------------------
// Local-time formatting: no chrono/time dependency, so the platform `localtime`
// provides the UTC offset and the civil conversion is done by hand.
// ---------------------------------------------------------------------------

/// Local wall-clock fields of a Unix timestamp, used for clock and date-divider labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CivilTime {
    pub(super) year: i32,
    pub(super) month: u32,
    pub(super) day: u32,
    /// Local days since the Unix epoch; date breaks compare this.
    pub(super) day_index: i64,
    pub(super) hour: u32,
    pub(super) minute: u32,
}

impl CivilTime {
    pub(super) fn from_unix_with_offset(secs: i64, offset_secs: i64) -> Self {
        let shifted = secs + offset_secs;
        let day_index = shifted.div_euclid(86_400);
        let second_of_day = shifted.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(day_index);
        CivilTime {
            year,
            month,
            day,
            day_index,
            hour: u32::try_from(second_of_day / 3_600).unwrap_or(0),
            minute: u32::try_from((second_of_day % 3_600) / 60).unwrap_or(0),
        }
    }

    pub(super) fn from_unix(secs: i64) -> Self {
        Self::from_unix_with_offset(secs, local_utc_offset(secs))
    }

    pub(super) fn now() -> Self {
        Self::from_unix(now_secs())
    }
}

pub(super) fn day_index(ts: &Ts) -> Option<i64> {
    ts.secs()
        .map(|secs| (secs + local_utc_offset(secs)).div_euclid(86_400))
}

pub(super) fn format_clock_civil(time: &CivilTime, clock_24h: bool) -> String {
    if clock_24h {
        format!("{:02}:{:02}", time.hour, time.minute)
    } else {
        let hour12 = match time.hour % 12 {
            0 => 12,
            hour => hour,
        };
        let meridiem = if time.hour < 12 { "AM" } else { "PM" };
        format!("{}:{:02} {}", hour12, time.minute, meridiem)
    }
}

pub(super) fn format_clock(ts: &Ts) -> String {
    format_clock_with(ts, false)
}

pub(super) fn format_clock_with(ts: &Ts, clock_24h: bool) -> String {
    match ts.secs() {
        Some(secs) => format_clock_civil(&CivilTime::from_unix(secs), clock_24h),
        None => ts.as_str().to_owned(),
    }
}

/// Current Unix seconds.
pub(crate) fn now_secs() -> i64 {
    core_time::now_epoch_secs()
}

/// Default `(YYYY-MM-DD, HH:MM)` local strings ~1 hour from now for custom schedule/reminder dialogs.
pub(crate) fn default_custom_datetime() -> (String, String) {
    let local = now_secs() + local_offset_now() + 3_600;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{:02}:{:02}", secs / 3_600, (secs % 3_600) / 60),
    )
}

/// Parse a local `YYYY-MM-DD` date and `HH:MM` (24-hour) time into a future unix timestamp.
pub(crate) fn parse_local_schedule(date_str: &str, time_str: &str) -> Option<u64> {
    let date = slackless_core::store::Date::parse(date_str)?;
    let (h_str, m_str) = time_str.split_once(':')?;
    let hour: i64 = h_str.trim().parse().ok()?;
    let min: i64 = m_str.trim().parse().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&min) {
        return None;
    }
    let naive_utc = date.unix_start() + hour * 3_600 + min * 60;
    let approx_utc = naive_utc - local_offset_now();
    let offset = local_utc_offset(approx_utc);
    let utc = naive_utc - offset;
    let now = now_secs();
    (utc > now + 30).then_some(utc as u64)
}

/// `3:42 PM` for `secs` on this machine's clock, or in the zone `utc_offset` seconds from UTC.
pub(crate) fn clock(secs: i64, utc_offset: Option<i64>) -> String {
    let shifted = secs + utc_offset.unwrap_or_else(|| local_utc_offset(secs));
    let second_of_day = shifted.rem_euclid(86_400);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let hour12 = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    let meridiem = if hour < 12 { "AM" } else { "PM" };
    format!("{hour12}:{minute:02} {meridiem}")
}

/// This machine's offset from UTC right now, in seconds.
pub(crate) fn local_offset_now() -> i64 {
    local_utc_offset(now_secs())
}

/// Pure relative compact stamp for list rows against an explicit `now`.
pub(super) fn list_stamp_at(time: &CivilTime, now: &CivilTime) -> String {
    if time.day_index == now.day_index {
        format_clock_civil(time, false)
    } else if (1..=7).contains(&(now.day_index - time.day_index)) {
        // A week back still reads as the weekday ("Friday" on a Friday), as Slack shows it.
        let label = date_label(time, now);
        match label.split_once(',') {
            Some((weekday, _)) => weekday.to_owned(),
            None => label,
        }
    } else {
        date_label(time, now)
    }
}

/// Compact stamp for list rows (DM list, activity, search): the clock time for today's
/// messages, just the weekday within the past week ("Monday"), the divider's day label
/// otherwise.
pub(crate) fn list_stamp(ts: &Ts) -> String {
    let now = CivilTime::now();
    match ts.secs().map(CivilTime::from_unix) {
        Some(time) => list_stamp_at(&time, &now),
        None => ts.as_str().to_owned(),
    }
}

/// Pure file creation day string against an explicit `now`.
pub(super) fn file_day_at(time: &CivilTime, now: &CivilTime) -> String {
    let label = date_label(time, now);
    if time.day_index == now.day_index || time.day_index + 1 == now.day_index {
        return label.to_lowercase();
    }
    // Within the current year the label leads with the weekday: "Monday, October 1st".
    match label.split_once(", ") {
        Some((_, day)) if time.year == now.year => format!("on {day}"),
        _ => format!("on {label}"),
    }
}

/// When a file was created, as the Files view words it after "Created": "today",
/// "yesterday", "on October 1st", or "on January 19th, 2025" outside the current year.
pub(crate) fn file_day(secs: i64) -> String {
    let now = CivilTime::now();
    let time = CivilTime::from_unix(secs);
    file_day_at(&time, &now)
}

/// Pure message-card stamp against an explicit `now` honoring `clock_24h`.
pub(super) fn card_stamp_at(time: &CivilTime, now: &CivilTime, clock_24h: bool) -> String {
    match now.day_index - time.day_index {
        0 => format_clock_civil(time, clock_24h),
        2..=6 => format!(
            "{} at {}",
            weekday_name(time.day_index),
            format_clock_civil(time, clock_24h)
        ),
        _ => day_and_time_at(time, now, clock_24h),
    }
}

/// The stamp on a message card outside the day-divided transcript (threads, search, pins):
/// like Slack, the time for today's messages, "Yesterday at …", the weekday within the past
/// week ("Tuesday at 9:01 AM"), the full date before that.
pub(super) fn card_stamp(ts: &Ts, clock_24h: bool) -> String {
    let Some(secs) = ts.secs() else {
        return ts.as_str().to_owned();
    };
    let time = CivilTime::from_unix(secs);
    let now = CivilTime::now();
    card_stamp_at(&time, &now, clock_24h)
}

/// The weekday of a day counted from the Unix epoch, which began on a Thursday.
pub(super) fn weekday_name(day_index: i64) -> &'static str {
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    WEEKDAYS[(day_index + 4).rem_euclid(7) as usize]
}

/// The day header of a day-grouped list (the Activity page): the divider's day label
/// (`Today`, `Yesterday`, `Tuesday, November 10th`), or the raw ts when it does not parse.
pub(crate) fn day_label(ts: &Ts) -> String {
    ts.secs()
        .map(|secs| date_label(&CivilTime::from_unix(secs), &CivilTime::now()))
        .unwrap_or_else(|| ts.as_str().to_owned())
}

/// Pure long stamp against an explicit `now` honoring `clock_24h`.
pub(super) fn day_and_time_at(time: &CivilTime, now: &CivilTime, clock_24h: bool) -> String {
    format!(
        "{} at {}",
        date_label(time, now),
        format_clock_civil(time, clock_24h)
    )
}

/// Slack's long stamp honoring `clock_24h` (`Yesterday at 09:25`, `Yesterday at 9:25 AM`).
pub(crate) fn day_and_time_with(ts: &Ts, clock_24h: bool) -> String {
    match ts.secs() {
        Some(secs) => day_and_time_at(&CivilTime::from_unix(secs), &CivilTime::now(), clock_24h),
        None => ts.as_str().to_owned(),
    }
}

/// Slack's long stamp (attachment footers, thread replies from other days):
/// `Yesterday at 9:25 AM`, `Today at 3:01 PM`, `Tuesday, November 10th at 8:00 AM`.
pub(crate) fn day_and_time(ts: &Ts) -> String {
    day_and_time_with(ts, false)
}

/// `2026-11-19` (local date) for Unix `secs`; Block Kit date pickers use this format.
pub(crate) fn iso_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    format!("{:04}-{:02}-{:02}", time.year, time.month, time.day)
}

/// Slack's divider label: `Today` / `Yesterday` / `Tuesday, November 10th`, plus `, 2025`
/// outside the current year.
pub(super) fn date_label(time: &CivilTime, now: &CivilTime) -> String {
    const MONTH_NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    if time.day_index == now.day_index {
        return "Today".to_owned();
    }
    if time.day_index + 1 == now.day_index {
        return "Yesterday".to_owned();
    }
    let weekday = weekday_name(time.day_index);
    let month = MONTH_NAMES[(time.month - 1) as usize];
    let day = time.day;
    let suffix = ordinal_suffix(day);
    // Slack names the weekday only within the current year: "October 25th, 2022".
    if time.year == now.year {
        format!("{weekday}, {month} {day}{suffix}")
    } else {
        format!("{month} {day}{suffix}, {}", time.year)
    }
}

/// `st`, `nd`, `rd` or `th` after `day`.
pub(super) fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// Pure shared-message short date against an explicit `now`.
pub(super) fn short_date_at(time: &CivilTime, now: &CivilTime) -> String {
    let day = format!(
        "{} {}{}",
        MONTHS[(time.month - 1) as usize],
        time.day,
        ordinal_suffix(time.day)
    );
    if time.year == now.year {
        day
    } else {
        format!("{day}, {}", time.year)
    }
}

/// A shared message's day as Slack's card for it shows it: `Nov 18th`, plus `, 2025` outside
/// the current year.
pub(crate) fn short_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    short_date_at(&time, &CivilTime::now())
}

pub(super) fn date_label_absolute(time: &CivilTime) -> String {
    format!("{} {}", MONTHS[(time.month - 1) as usize], time.day)
}

/// Manual timezone offset override in seconds (`i64::MIN` = follow OS timezone, `F-SHELL-100`).
#[cfg(not(test))]
static TIMEZONE_OVERRIDE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);

#[cfg(test)]
thread_local! {
    static TIMEZONE_OVERRIDE: std::cell::Cell<i64> = const { std::cell::Cell::new(i64::MIN) };
}

fn load_timezone_override() -> i64 {
    #[cfg(not(test))]
    {
        TIMEZONE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TIMEZONE_OVERRIDE.with(std::cell::Cell::get)
    }
}

/// Override the display clock timezone (`None` restores automatic system timezone).
pub fn set_timezone_override(offset_secs: Option<i64>) {
    let raw = offset_secs.unwrap_or(i64::MIN);
    #[cfg(not(test))]
    TIMEZONE_OVERRIDE.store(raw, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    TIMEZONE_OVERRIDE.with(|slot| slot.set(raw));
}

/// Offset from UTC in seconds at `unix_secs`, honoring manual override or platform `localtime`.
/// Cached per UTC hour in thread-local storage to avoid locking libc's global timezone mutex on
/// every visible message row per frame.
#[cfg(unix)]
pub(crate) fn local_utc_offset(unix_secs: i64) -> i64 {
    let manual = load_timezone_override();
    if manual != i64::MIN {
        return manual;
    }
    let hour_bucket = unix_secs.div_euclid(3600);
    thread_local! {
        static OFFSET_CACHE: RefCell<Vec<(i64, i64)>> = const { RefCell::new(Vec::new()) };
    }
    if let Some(cached) = OFFSET_CACHE.with(|cache| {
        cache
            .borrow()
            .iter()
            .find(|(h, _)| *h == hour_bucket)
            .map(|(_, off)| *off)
    }) {
        return cached;
    }
    let time_val: libc::time_t = unix_secs as libc::time_t;
    // SAFETY: `libc::tm` is a C POD struct zero-initialized before passing to `localtime_r`,
    // and `time_val` outlives the call.
    let offset = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&raw const time_val, &raw mut tm).is_null() {
            tm.tm_gmtoff
        } else {
            0
        }
    };
    OFFSET_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 64 {
            cache.remove(0);
        }
        cache.push((hour_bucket, offset));
    });
    offset
}

#[cfg(not(unix))]
pub(crate) fn local_utc_offset(_unix_secs: i64) -> i64 {
    let manual = load_timezone_override();
    if manual != i64::MIN {
        return manual;
    }
    0
}

/// (year, month, day) from days since the Unix epoch (proleptic Gregorian).
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    core_time::civil_from_days(days)
}

#[cfg(test)]
mod huddle_card_tests {
    use super::super::render::format_huddle_duration;

    #[test]
    fn formats_huddle_durations_cleanly() {
        assert_eq!(format_huddle_duration(16), "16s");
        assert_eq!(format_huddle_duration(120), "2m");
        assert_eq!(format_huddle_duration(724), "12m 4s");
        assert_eq!(format_huddle_duration(3600), "1h");
        assert_eq!(format_huddle_duration(3900), "1h 5m");
    }
}

#[cfg(test)]
mod time_tests {
    use super::*;

    fn civil(day_index: i64, hour: u32, minute: u32) -> CivilTime {
        let (year, month, day) = civil_from_days(day_index);
        CivilTime {
            year,
            month,
            day,
            day_index,
            hour,
            minute,
        }
    }

    #[test]
    fn civil_from_days_covers_epoch_pre_1970_leap_and_year_boundaries() {
        // Unix epoch & pre-1970 negative days
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(-365), (1969, 1, 1));
        // 1900 is NOT a leap year (divisible by 100, not 400)
        assert_eq!(civil_from_days(-25_567), (1900, 1, 1));
        assert_eq!(civil_from_days(-25_509), (1900, 2, 28));
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1));
        // 2000 IS a leap year (divisible by 400)
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 2024 IS a leap year
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
        // 2025 is NOT a leap year
        assert_eq!(civil_from_days(20_147), (2025, 2, 28));
        assert_eq!(civil_from_days(20_148), (2025, 3, 1));
        // 2100 is NOT a leap year
        assert_eq!(civil_from_days(47_540), (2100, 2, 28));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
        // Year boundary Dec 31 -> Jan 1
        assert_eq!(civil_from_days(20_453), (2025, 12, 31));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
    }

    #[test]
    fn format_clock_civil_and_malformed_ts_passthrough() {
        let cases = [
            (0, 0, "12:00 AM", "00:00"),
            (0, 30, "12:30 AM", "00:30"),
            (1, 5, "1:05 AM", "01:05"),
            (11, 59, "11:59 AM", "11:59"),
            (12, 0, "12:00 PM", "12:00"),
            (12, 45, "12:45 PM", "12:45"),
            (13, 5, "1:05 PM", "13:05"),
            (23, 59, "11:59 PM", "23:59"),
        ];
        for (h, m, expected_12h, expected_24h) in cases {
            let ct = civil(20_454, h, m);
            assert_eq!(format_clock_civil(&ct, false), expected_12h);
            assert_eq!(format_clock_civil(&ct, true), expected_24h);
        }

        // Malformed Ts passes through verbatim across clock/list/card/day helpers
        let bad = Ts::from("not-a-timestamp");
        assert_eq!(format_clock(&bad), "not-a-timestamp");
        assert_eq!(format_clock_with(&bad, true), "not-a-timestamp");
        assert_eq!(list_stamp(&bad), "not-a-timestamp");
        assert_eq!(card_stamp(&bad, false), "not-a-timestamp");
        assert_eq!(card_stamp(&bad, true), "not-a-timestamp");
        assert_eq!(day_and_time(&bad), "not-a-timestamp");
        assert_eq!(day_and_time_with(&bad, true), "not-a-timestamp");
        assert_eq!(day_label(&bad), "not-a-timestamp");
    }

    #[test]
    fn clock_handles_positive_negative_fractional_offsets_and_pre_epoch_timestamps() {
        // 1970-01-01 00:00:00 UTC
        assert_eq!(clock(0, Some(0)), "12:00 AM");
        // +01:00 (CET)
        assert_eq!(clock(0, Some(3_600)), "1:00 AM");
        // -08:00 (PST) wraps to prior day 4:00 PM
        assert_eq!(clock(0, Some(-28_800)), "4:00 PM");
        // +05:30 (IST)
        assert_eq!(clock(0, Some(19_800)), "5:30 AM");
        // -03:30 (NST)
        assert_eq!(clock(0, Some(-12_600)), "8:30 PM");
        // +05:45 (NPT)
        assert_eq!(clock(0, Some(20_700)), "5:45 AM");
        // Noon UTC + 05:45 = 5:45 PM
        assert_eq!(clock(12 * 3_600, Some(20_700)), "5:45 PM");
        // Negative Unix timestamps (1969-12-31 23:30:00 UTC = -1800)
        assert_eq!(clock(-1_800, Some(0)), "11:30 PM");
        assert_eq!(clock(-1_800, Some(3_600)), "12:30 AM");
        assert_eq!(clock(-86_400, Some(0)), "12:00 AM");
        assert_eq!(clock(-3_660, Some(-12_600)), "7:29 PM");
    }

    #[test]
    fn ordinal_suffix_covers_all_days_1_through_31() {
        let expected = [
            (1, "st"),
            (2, "nd"),
            (3, "rd"),
            (4, "th"),
            (5, "th"),
            (6, "th"),
            (7, "th"),
            (8, "th"),
            (9, "th"),
            (10, "th"),
            (11, "th"),
            (12, "th"),
            (13, "th"),
            (14, "th"),
            (15, "th"),
            (16, "th"),
            (17, "th"),
            (18, "th"),
            (19, "th"),
            (20, "th"),
            (21, "st"),
            (22, "nd"),
            (23, "rd"),
            (24, "th"),
            (25, "th"),
            (26, "th"),
            (27, "th"),
            (28, "th"),
            (29, "th"),
            (30, "th"),
            (31, "st"),
        ];
        for (day, suffix) in expected {
            assert_eq!(ordinal_suffix(day), suffix, "day {day}");
        }
    }

    #[test]
    fn weekday_name_handles_epoch_negative_indices_and_known_dates() {
        // Epoch 1970-01-01 was Thursday
        assert_eq!(weekday_name(0), "Thursday");
        // Negative indices wrapping backwards from Thursday
        assert_eq!(weekday_name(-1), "Wednesday");
        assert_eq!(weekday_name(-2), "Tuesday");
        assert_eq!(weekday_name(-3), "Monday");
        assert_eq!(weekday_name(-4), "Sunday");
        assert_eq!(weekday_name(-5), "Saturday");
        assert_eq!(weekday_name(-6), "Friday");
        assert_eq!(weekday_name(-7), "Thursday");
        // Known calendar dates
        assert_eq!(weekday_name(10_957), "Saturday"); // 2000-01-01
        assert_eq!(weekday_name(11_016), "Tuesday"); // 2000-02-29
        assert_eq!(weekday_name(19_782), "Thursday"); // 2024-02-29
        assert_eq!(weekday_name(20_454), "Thursday"); // 2026-01-01
    }

    #[test]
    fn relative_formatters_cover_today_yesterday_past_week_older_prior_year_and_future() {
        // Anchor: Wednesday, November 25th, 2026 (2026-01-01 is 20454; Nov 25 is +328 = 20782).
        let now = civil(20_782, 15, 30);
        assert_eq!((now.year, now.month, now.day), (2026, 11, 25));
        assert_eq!(weekday_name(now.day_index), "Wednesday");

        let today = civil(20_782, 9, 5);
        let yesterday = civil(20_781, 21, 15);
        let two_days_ago = civil(20_780, 14, 0); // Monday, Nov 23
        let six_days_ago = civil(20_776, 8, 30); // Thursday, Nov 19
        let seven_days_ago = civil(20_775, 18, 45); // Wednesday, Nov 18
        let older_same_year = civil(20_740, 10, 0); // Wednesday, Oct 14, 2026
        let prior_year = civil(20_400, 13, 20); // Saturday, Nov 8, 2025
        let future_same_year = civil(20_784, 16, 0); // Friday, Nov 27, 2026
        let future_next_year = civil(20_820, 7, 10); // Saturday, Jan 2, 2027

        // date_label
        assert_eq!(date_label(&today, &now), "Today");
        assert_eq!(date_label(&yesterday, &now), "Yesterday");
        assert_eq!(date_label(&two_days_ago, &now), "Monday, November 23rd");
        assert_eq!(date_label(&six_days_ago, &now), "Thursday, November 19th");
        assert_eq!(
            date_label(&seven_days_ago, &now),
            "Wednesday, November 18th"
        );
        assert_eq!(
            date_label(&older_same_year, &now),
            "Wednesday, October 14th"
        );
        assert_eq!(date_label(&prior_year, &now), "November 8th, 2025");
        assert_eq!(date_label(&future_same_year, &now), "Friday, November 27th");
        assert_eq!(date_label(&future_next_year, &now), "January 2nd, 2027");

        // list_stamp_at: today -> clock, 1 day -> Yesterday, 2..=7 -> weekday, >7 or future -> date_label
        assert_eq!(list_stamp_at(&today, &now), "9:05 AM");
        assert_eq!(list_stamp_at(&yesterday, &now), "Yesterday");
        assert_eq!(list_stamp_at(&two_days_ago, &now), "Monday");
        assert_eq!(list_stamp_at(&six_days_ago, &now), "Thursday");
        assert_eq!(list_stamp_at(&seven_days_ago, &now), "Wednesday");
        assert_eq!(
            list_stamp_at(&older_same_year, &now),
            "Wednesday, October 14th"
        );
        assert_eq!(list_stamp_at(&prior_year, &now), "November 8th, 2025");
        // Future timestamps must NOT collapse to bare weekday (guards negative delta bug!)
        assert_eq!(
            list_stamp_at(&future_same_year, &now),
            "Friday, November 27th"
        );
        assert_eq!(list_stamp_at(&future_next_year, &now), "January 2nd, 2027");

        // file_day_at
        assert_eq!(file_day_at(&today, &now), "today");
        assert_eq!(file_day_at(&yesterday, &now), "yesterday");
        assert_eq!(file_day_at(&two_days_ago, &now), "on November 23rd");
        assert_eq!(file_day_at(&older_same_year, &now), "on October 14th");
        assert_eq!(file_day_at(&prior_year, &now), "on November 8th, 2025");
        assert_eq!(file_day_at(&future_same_year, &now), "on November 27th");
        assert_eq!(file_day_at(&future_next_year, &now), "on January 2nd, 2027");

        // card_stamp_at: honors clock_24h across ALL branches (today, yesterday, 2..=6, >=7, prior year, future)
        assert_eq!(card_stamp_at(&today, &now, false), "9:05 AM");
        assert_eq!(card_stamp_at(&today, &now, true), "09:05");
        assert_eq!(
            card_stamp_at(&yesterday, &now, false),
            "Yesterday at 9:15 PM"
        );
        assert_eq!(card_stamp_at(&yesterday, &now, true), "Yesterday at 21:15");
        assert_eq!(
            card_stamp_at(&two_days_ago, &now, false),
            "Monday at 2:00 PM"
        );
        assert_eq!(card_stamp_at(&two_days_ago, &now, true), "Monday at 14:00");
        assert_eq!(
            card_stamp_at(&six_days_ago, &now, false),
            "Thursday at 8:30 AM"
        );
        assert_eq!(
            card_stamp_at(&six_days_ago, &now, true),
            "Thursday at 08:30"
        );
        assert_eq!(
            card_stamp_at(&seven_days_ago, &now, false),
            "Wednesday, November 18th at 6:45 PM"
        );
        assert_eq!(
            card_stamp_at(&seven_days_ago, &now, true),
            "Wednesday, November 18th at 18:45"
        );
        assert_eq!(
            card_stamp_at(&prior_year, &now, false),
            "November 8th, 2025 at 1:20 PM"
        );
        assert_eq!(
            card_stamp_at(&prior_year, &now, true),
            "November 8th, 2025 at 13:20"
        );
        assert_eq!(
            card_stamp_at(&future_same_year, &now, false),
            "Friday, November 27th at 4:00 PM"
        );
        assert_eq!(
            card_stamp_at(&future_same_year, &now, true),
            "Friday, November 27th at 16:00"
        );

        // day_and_time_at
        assert_eq!(day_and_time_at(&today, &now, false), "Today at 9:05 AM");
        assert_eq!(day_and_time_at(&today, &now, true), "Today at 09:05");

        // short_date_at & date_label_absolute
        assert_eq!(short_date_at(&today, &now), "Nov 25th");
        assert_eq!(short_date_at(&older_same_year, &now), "Oct 14th");
        assert_eq!(short_date_at(&prior_year, &now), "Nov 8th, 2025");
        assert_eq!(short_date_at(&future_next_year, &now), "Jan 2nd, 2027");
        assert_eq!(date_label_absolute(&today), "Nov 25");
        assert_eq!(date_label_absolute(&future_next_year), "Jan 2");
    }

    #[test]
    fn timezone_override_affects_local_utc_offset_iso_date_and_format_clock() {
        struct RestoreGuard(Option<i64>);
        impl Drop for RestoreGuard {
            fn drop(&mut self) {
                set_timezone_override(self.0);
            }
        }
        let prev = {
            let raw = load_timezone_override();
            (raw != i64::MIN).then_some(raw)
        };
        let _guard = RestoreGuard(prev);

        // UTC (0)
        set_timezone_override(Some(0));
        assert_eq!(local_utc_offset(0), 0);
        assert_eq!(iso_date(0), "1970-01-01");
        let ts = Ts::from("1709812800.000000"); // 2024-03-07 12:00:00 UTC
        assert_eq!(format_clock(&ts), "12:00 PM");
        assert_eq!(format_clock_with(&ts, true), "12:00");

        // Tokyo (+09:00): 2024-03-07 12:00 UTC -> 21:00 JST, and 2024-03-07 20:00 UTC -> 2024-03-08
        set_timezone_override(Some(9 * 3_600));
        assert_eq!(local_utc_offset(0), 9 * 3_600);
        assert_eq!(format_clock(&ts), "9:00 PM");
        assert_eq!(format_clock_with(&ts, true), "21:00");
        assert_eq!(iso_date(1_709_841_600), "2024-03-08");

        // PST (-08:00): 1970-01-01 00:00 UTC -> 1969-12-31
        set_timezone_override(Some(-8 * 3_600));
        assert_eq!(iso_date(0), "1969-12-31");
        assert_eq!(format_clock_with(&Ts::from("0.000000"), true), "16:00");
    }
}
