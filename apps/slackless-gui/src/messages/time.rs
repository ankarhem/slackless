//! Local-time & civil-calendar formatting (zero chrono/time dependency; cached `localtime_r`).

use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

use slackless_core::types::Ts;

use super::MONTHS;

// ---------------------------------------------------------------------------
// Local-time formatting: no chrono/time dependency, so the platform `localtime`
// provides the UTC offset and the civil conversion is done by hand.
// ---------------------------------------------------------------------------

/// Local wall-clock fields of a Unix timestamp, used for clock and date-divider labels.
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
    pub(super) fn from_unix(secs: i64) -> Self {
        let shifted = secs + local_utc_offset(secs);
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

    pub(super) fn now() -> Self {
        Self::from_unix(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs() as i64)
                .unwrap_or(0),
        )
    }
}

pub(super) fn day_index(ts: &Ts) -> Option<i64> {
    ts.secs()
        .map(|secs| (secs + local_utc_offset(secs)).div_euclid(86_400))
}

pub(super) fn format_clock(ts: &Ts) -> String {
    format_clock_with(ts, false)
}

pub(super) fn format_clock_with(ts: &Ts, clock_24h: bool) -> String {
    match ts.secs() {
        Some(secs) => {
            let time = CivilTime::from_unix(secs);
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
        None => ts.as_str().to_owned(),
    }
}

/// Current Unix seconds.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
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

/// Compact stamp for list rows (DM list, activity, search): the clock time for today's
/// messages, just the weekday within the past week ("Monday"), the divider's day label
/// otherwise.
pub(crate) fn list_stamp(ts: &Ts) -> String {
    let now = CivilTime::now();
    match ts.secs().map(CivilTime::from_unix) {
        Some(time) if time.day_index == now.day_index => format_clock(ts),
        // A week back still reads as the weekday ("Friday" on a Friday), as Slack shows it.
        Some(time) if now.day_index - time.day_index <= 7 => {
            let label = date_label(&time, &now);
            match label.split_once(',') {
                Some((weekday, _)) => weekday.to_owned(),
                None => label,
            }
        }
        Some(time) => date_label(&time, &now),
        None => ts.as_str().to_owned(),
    }
}

/// When a file was created, as the Files view words it after "Created": "today",
/// "yesterday", "on October 1st", or "on January 19th, 2025" outside the current year.
pub(crate) fn file_day(secs: i64) -> String {
    let now = CivilTime::now();
    let time = CivilTime::from_unix(secs);
    let label = date_label(&time, &now);
    if time.day_index == now.day_index || time.day_index + 1 == now.day_index {
        return label.to_lowercase();
    }
    // Within the current year the label leads with the weekday: "Monday, October 1st".
    match label.split_once(", ") {
        Some((_, day)) if time.year == now.year => format!("on {day}"),
        _ => format!("on {label}"),
    }
}

/// The stamp on a message card outside the day-divided transcript (threads, search, pins):
/// like Slack, the time for today's messages, "Yesterday at …", the weekday within the past
/// week ("Tuesday at 9:01 AM"), the full date before that.
pub(super) fn card_stamp(ts: &Ts, clock_24h: bool) -> String {
    let Some(secs) = ts.secs() else {
        return ts.as_str().to_owned();
    };
    let day = CivilTime::from_unix(secs).day_index;
    match CivilTime::now().day_index - day {
        0 => format_clock_with(ts, clock_24h),
        2..=6 => format!(
            "{} at {}",
            weekday_name(day),
            format_clock_with(ts, clock_24h)
        ),
        _ => day_and_time(ts),
    }
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

/// Slack's long stamp (attachment footers, thread replies from other days):
/// `Yesterday at 9:25 AM`, `Today at 3:01 PM`, `Tuesday, November 10th at 8:00 AM`.
pub(crate) fn day_and_time(ts: &Ts) -> String {
    match ts.secs() {
        Some(secs) => format!(
            "{} at {}",
            date_label(&CivilTime::from_unix(secs), &CivilTime::now()),
            format_clock(ts)
        ),
        None => ts.as_str().to_owned(),
    }
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

/// A shared message's day as Slack's card for it shows it: `Nov 18th`, plus `, 2025` outside
/// the current year.
pub(crate) fn short_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    let day = format!(
        "{} {}{}",
        MONTHS[(time.month - 1) as usize],
        time.day,
        ordinal_suffix(time.day)
    );
    if time.year == CivilTime::now().year {
        day
    } else {
        format!("{day}, {}", time.year)
    }
}

pub(super) fn date_label_absolute(time: &CivilTime) -> String {
    format!("{} {}", MONTHS[(time.month - 1) as usize], time.day)
}

/// Manual timezone offset override in seconds (`i64::MIN` = follow OS timezone, `F-SHELL-100`).
static TIMEZONE_OVERRIDE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);

/// Override the display clock timezone (`None` restores automatic system timezone).
pub fn set_timezone_override(offset_secs: Option<i64>) {
    TIMEZONE_OVERRIDE.store(
        offset_secs.unwrap_or(i64::MIN),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Offset from UTC in seconds at `unix_secs`, honoring manual override or platform `localtime`.
/// Cached per UTC hour in thread-local storage to avoid locking libc's global timezone mutex on
/// every visible message row per frame.
#[cfg(unix)]
pub(super) fn local_utc_offset(unix_secs: i64) -> i64 {
    let manual = TIMEZONE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
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
    /// `struct tm` as laid out by glibc and Darwin: nine `int` fields then
    /// `tm_gmtoff`/`tm_zone`.
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const i8,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    // SAFETY: `tm` is a valid, fully initialized `struct tm` owned by this call, and `unix_secs`
    // outlives it.
    let ok = unsafe { !localtime_r(&unix_secs, &mut tm).is_null() };
    let offset = if ok {
        tm.tm_gmtoff
    } else {
        // The rest of `struct tm` exists for the C ABI; reading it keeps every field live.
        let _ = (
            tm.tm_sec, tm.tm_min, tm.tm_hour, tm.tm_mday, tm.tm_mon, tm.tm_year,
        );
        let _ = (tm.tm_wday, tm.tm_yday, tm.tm_isdst, tm.tm_zone);
        0
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
pub(super) fn local_utc_offset(_unix_secs: i64) -> i64 {
    let manual = TIMEZONE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if manual != i64::MIN {
        return manual;
    }
    0
}

/// (year, month, day) from days since the Unix epoch (proleptic Gregorian).
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shift = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_shift + 2) / 5 + 1) as u32;
    let month = if month_shift < 10 {
        month_shift + 3
    } else {
        month_shift - 9
    } as u32;
    ((year + i64::from(month <= 2)) as i32, month, day)
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
