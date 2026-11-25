use super::pause::parse_until_time;
use super::{ScheduledStatus, SelfState, UserStatus};

#[test]
fn activates_due_scheduled_statuses_in_earliest_timestamp_order() {
    let mut state = SelfState::default();
    // Push out of chronological order; earliest due timestamp must activate first.
    state.scheduled_statuses.push(ScheduledStatus {
        emoji: "palm_tree".to_owned(),
        text: "Future PTO".to_owned(),
        starts_at: i64::MAX - 10,
        clear_after: 0,
        pause: true,
    });
    state.scheduled_statuses.push(ScheduledStatus {
        emoji: "coffee".to_owned(),
        text: "Lunch".to_owned(),
        starts_at: 2_000,
        clear_after: 1,
        pause: false,
    });
    state.scheduled_statuses.push(ScheduledStatus {
        emoji: "spiral_calendar_pad".to_owned(),
        text: "Standup".to_owned(),
        starts_at: 1_000,
        clear_after: 1,
        pause: true,
    });

    let first = state
        .activate_due_scheduled_at(5_000, 0)
        .expect("earliest due status activates");
    assert_eq!(first.text, "Standup");
    assert_eq!(first.expiration, Some(6_800));
    assert!(state.paused_until == Some(6_800));

    let second = state
        .activate_due_scheduled_at(5_000, 0)
        .expect("second due status activates");
    assert_eq!(second.text, "Lunch");
    assert_eq!(state.scheduled_statuses.len(), 1);
    assert!(state.activate_due_scheduled_at(5_000, 0).is_none());
}

#[test]
fn scheduled_status_clears_at_respects_timezone_boundaries() {
    // 2024-03-07 23:00:00 UTC (Thursday):
    // - In UTC (offset 0): Today ends at 2024-03-08 00:00:00 UTC (+3600s).
    // - In UTC+02:00 (+7200): local time is already 2024-03-08 01:00 (Friday),
    //   so Today ends at 2024-03-09 00:00 local = 2024-03-08 22:00 UTC (+82800s).
    let now_utc = 1_709_852_400; // 2024-03-07 23:00:00 UTC
    let status_today = UserStatus {
        emoji: "bus".into(),
        text: "Commuting".into(),
        clear_after: 4, // Today
        pause: false,
        expiration: None,
    };
    assert_eq!(
        status_today.clears_at_from(now_utc, 0),
        Some(now_utc + 3_600)
    );
    assert_eq!(
        status_today.clears_at_from(now_utc, 7_200),
        Some(now_utc + 23 * 3_600)
    );
    // "This week" on Thursday (day 3 since Monday) -> +4 days from local midnight
    let status_week = UserStatus {
        clear_after: 5,
        ..status_today.clone()
    };
    assert_eq!(
        status_week.clears_at_from(now_utc, 0),
        Some(now_utc + 3_600 + 3 * 86_400)
    );
    // In UTC+02:00 it is already Friday (day 4 since Monday) -> +3 days from Friday local midnight
    assert_eq!(
        status_week.clears_at_from(now_utc, 7_200),
        Some(now_utc + 23 * 3_600 + 2 * 86_400)
    );
}

#[test]
fn user_status_expiry_countdown_and_self_state_expiration() {
    let mut s = UserStatus {
        emoji: "coffee".into(),
        text: "Break".into(),
        clear_after: 0,
        pause: true,
        expiration: None,
    };
    assert!(!s.is_expired_at(1_000));
    assert!(!s.is_expired());
    assert_eq!(s.format_remaining(1_000), None);

    s.expiration = Some(1_045);
    assert!(!s.is_expired_at(1_000));
    assert_eq!(s.format_remaining(1_000).as_deref(), Some("45s"));
    assert!(s.is_expired_at(1_045));
    assert!(s.is_expired_at(1_046));
    assert_eq!(s.format_remaining(1_045), None);

    s.expiration = Some(1_000 + 12 * 60 + 30);
    assert_eq!(s.format_remaining(1_000).as_deref(), Some("12m"));
    s.expiration = Some(1_000 + 2 * 3_600);
    assert_eq!(s.format_remaining(1_000).as_deref(), Some("2h"));
    s.expiration = Some(1_000 + 2 * 3_600 + 15 * 60);
    assert_eq!(s.format_remaining(1_000).as_deref(), Some("2h 15m"));
    s.expiration = Some(1_000 + 3 * 86_400);
    assert_eq!(s.format_remaining(1_000).as_deref(), Some("3d"));
    s.expiration = Some(1_000 + 3 * 86_400 + 4 * 3_600);
    assert_eq!(s.format_remaining(1_000).as_deref(), Some("3d 4h"));

    // SelfState expiration clears both status and pause
    let mut state = SelfState::default();
    s.expiration = Some(2_000);
    state.set_status(Some(s));
    assert_eq!(state.paused_until, Some(2_000));
    assert!(!state.expire_status_at(1_999));
    assert!(state.status.is_some());
    assert!(state.expire_status_at(2_000));
    assert!(state.status.is_none());
    assert!(state.paused_until.is_none());
}

#[test]
fn pause_label_formats_today_tomorrow_multi_day_indefinite_and_expired() {
    // 2024-03-07 10:00:00 UTC
    let mut state = SelfState {
        paused_until: Some(NOW - 1),
        ..SelfState::default()
    };
    assert_eq!(state.pause_label_at(NOW, 0), None);

    state.paused_until = Some(i64::MAX);
    assert_eq!(
        state.pause_label_at(NOW, 0).as_deref(),
        Some("Notifications paused")
    );

    state.paused_until = Some(NOW + 23_400); // 4:30 PM today
    assert_eq!(
        state.pause_label_at(NOW, 0).as_deref(),
        Some("Notifications paused until 4:30 PM")
    );

    state.paused_until = Some(NOW + 86_400); // 10:00 AM tomorrow
    assert_eq!(
        state.pause_label_at(NOW, 0).as_deref(),
        Some("Notifications paused until tomorrow, 10:00 AM")
    );

    state.paused_until = Some(NOW + 3 * 86_400);
    assert_eq!(
        state.pause_label_at(NOW, 0).as_deref(),
        Some("Notifications paused until 10:00 AM, in 3 days")
    );
}

/// 2024-03-07 10:00:00 UTC (`offset` 0 in the calls below).
const NOW: i64 = 1_709_805_600;

fn until(text: &str) -> Option<i64> {
    parse_until_time(text, NOW, 0)
}

#[test]
fn parses_clock_times_to_the_next_occurrence() {
    // 4:30 PM today is ahead of 10:00 AM: today at 16:30.
    assert_eq!(until("4:30 pm"), Some(NOW + 23_400));
    assert_eq!(until("4:30pm"), Some(NOW + 23_400));
    assert_eq!(until("4:30 PM"), Some(NOW + 23_400));
    assert_eq!(until("  16:30 "), Some(NOW + 23_400));
    // Noon: 12 PM and 24-hour 12:00 are both noon today; 12 AM and 0:00 are midnight tomorrow.
    assert_eq!(until("12 pm"), Some(NOW + 7_200));
    assert_eq!(until("12:00 PM"), Some(NOW + 7_200));
    assert_eq!(until("12:00"), Some(NOW + 7_200));
    assert_eq!(until("12 am"), Some(NOW + 86_400 - 36_000));
    assert_eq!(until("12:00 am"), Some(NOW + 86_400 - 36_000));
    assert_eq!(until("0:00"), Some(NOW + 86_400 - 36_000));
    assert_eq!(until("00:00"), Some(NOW + 86_400 - 36_000));
    // Single-digit hours and exact current time (rolls to tomorrow).
    assert_eq!(until("9:05 am"), Some(NOW + 83_100));
    assert_eq!(until("9:05 AM"), Some(NOW + 83_100));
    assert_eq!(until("10:00"), Some(NOW + 86_400));
    assert_eq!(until("11"), Some(NOW + 3_600));
    // Bare hours count as 24-hour clock times.
    assert_eq!(until("23"), Some(NOW + 46_800));
    // Timezone offset boundary: at 10:00 UTC in UTC-08:00 it is 02:00 local, so 09:00 local is +7h.
    assert_eq!(
        parse_until_time("9:00 am", NOW, -8 * 3_600),
        Some(NOW + 7 * 3_600)
    );
}

#[test]
fn rejects_non_times() {
    assert_eq!(until(""), None);
    assert_eq!(until("soon"), None);
    assert_eq!(until("13 pm"), None);
    assert_eq!(until("13:00 pm"), None);
    assert_eq!(until("13:00 am"), None);
    assert_eq!(until("0 pm"), None);
    assert_eq!(until("0:00 am"), None);
    assert_eq!(until("4:65 pm"), None);
    assert_eq!(until("12:60"), None);
    assert_eq!(until("24:00"), None);
    assert_eq!(until("25:00"), None);
    assert_eq!(until("4:3:00 pm"), None);
    assert_eq!(until("-4 pm"), None);
    assert_eq!(until("4:"), None);
}
