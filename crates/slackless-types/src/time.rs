//! Shared calendar math (Howard Hinnant's proleptic Gregorian algorithms), epoch clock helpers,
//! and human-readable duration / byte-size formatters.

use std::time::{SystemTime, UNIX_EPOCH};

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Number of days in `month` (`1..=12`) of `year`, or `None` if `month` is out of range.
pub fn days_in_month(year: i64, month: u32) -> Option<u32> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap_year(year) { 29 } else { 28 }),
        _ => None,
    }
}

/// Howard Hinnant's `days_from_civil`: UTC `(year, month, day)` -> days since `1970-01-01`.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's `civil_from_days`: days since `1970-01-01` -> UTC `(year, month, day)`.
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(month <= 2)) as i32, month, day)
}

/// Day of week for `(year, month, day)` with `0 = Sunday ..= 6 = Saturday`.
pub fn weekday_sunday0(year: i32, month: u32, day: u32) -> u32 {
    // 1970-01-01 (day 0) was a Thursday (4).
    (days_from_civil(i64::from(year), month, day) + 4).rem_euclid(7) as u32
}

/// Current wall-clock time as whole seconds since the Unix epoch (`0` if clock precedes epoch).
pub fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Current wall-clock time as whole milliseconds since the Unix epoch (`0` if clock precedes epoch).
pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Formats unix epoch seconds as `YYYY-MM-DD HH:MM:SSZ` (UTC).
pub fn format_utc(secs: i64) -> String {
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Formats a media duration in milliseconds as `M:SS` (e.g. `0:05`, `12:34`).
pub fn format_duration(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins}:{secs:02}")
}

/// Formats a huddle duration in seconds as a compact human label (`16s`, `2m`, `12m 4s`, `1h`,
/// `1h 5m`).
pub fn format_huddle_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

/// Formats a byte count using binary IEC units (`B`, `KiB`, `MiB`, `GiB`).
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leap_years_and_days_in_month_cover_gregorian_rules() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
        assert!(!is_leap_year(2100));

        assert_eq!(days_in_month(2024, 2), Some(29));
        assert_eq!(days_in_month(2023, 2), Some(28));
        assert_eq!(days_in_month(2024, 1), Some(31));
        assert_eq!(days_in_month(2024, 4), Some(30));
        assert_eq!(days_in_month(2024, 12), Some(31));
        assert_eq!(days_in_month(2024, 0), None);
        assert_eq!(days_in_month(2024, 13), None);
    }

    #[test]
    fn civil_and_days_round_trip_across_epoch_leap_and_pre_1970() {
        let cases = [
            (0, (1970, 1, 1)),
            (-1, (1969, 12, 31)),
            (-719_162, (1, 1, 1)),
            (19_782, (2024, 2, 29)),
            (19_783, (2024, 3, 1)),
            (10_957, (2000, 1, 1)),
            (11_016, (2000, 2, 29)),
        ];
        for (days, (y, m, d)) in cases {
            assert_eq!(civil_from_days(days), (y, m, d), "days={days}");
            assert_eq!(
                days_from_civil(i64::from(y), m, d),
                days,
                "date={y}-{m}-{d}"
            );
        }

        // Dense round-trip across 400+ years around epoch.
        for days in -50_000..150_000 {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(i64::from(y), m, d), days);
        }
    }

    #[test]
    fn weekday_sunday0_matches_known_dates() {
        assert_eq!(weekday_sunday0(1970, 1, 1), 4); // Thursday
        assert_eq!(weekday_sunday0(1969, 12, 31), 3); // Wednesday
        assert_eq!(weekday_sunday0(2024, 2, 29), 4); // Thursday
        assert_eq!(weekday_sunday0(2026, 4, 12), 0); // Sunday
        assert_eq!(weekday_sunday0(2026, 4, 18), 6); // Saturday
    }

    #[test]
    fn clock_helpers_return_plausible_epoch_values() {
        let secs = now_epoch_secs();
        let ms = now_epoch_ms();
        assert!(secs > 1_700_000_000);
        assert!(ms >= (secs as u64) * 1000);
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14 22:13:20Z");
    }

    #[test]
    fn duration_and_huddle_duration_formatters() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(5_400), "0:05");
        assert_eq!(format_duration(65_000), "1:05");
        assert_eq!(format_duration(754_999), "12:34");

        assert_eq!(format_huddle_duration(0), "0s");
        assert_eq!(format_huddle_duration(16), "16s");
        assert_eq!(format_huddle_duration(120), "2m");
        assert_eq!(format_huddle_duration(724), "12m 4s");
        assert_eq!(format_huddle_duration(3600), "1h");
        assert_eq!(format_huddle_duration(3900), "1h 5m");
    }

    #[test]
    fn format_bytes_scales_across_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.50 GiB");
    }
}
