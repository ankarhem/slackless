use super::*;
use crate::blockkit::serde_json;

#[test]
fn format_bytes_scales_across_units() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1024), "1.0 KiB");
    assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MiB");
    assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GiB");
}

#[test]
fn appearance_mode_key_round_trips_and_resolves_system() {
    for mode in [
        AppearanceMode::Light,
        AppearanceMode::Dark,
        AppearanceMode::System,
    ] {
        assert_eq!(AppearanceMode::from_key(mode.key()), Some(mode));
    }
    assert_eq!(AppearanceMode::from_key("unknown"), None);
    assert!(AppearanceMode::System.resolve(WindowAppearance::Dark) == SlackTheme::Dark);
    assert!(AppearanceMode::System.resolve(WindowAppearance::VibrantLight) == SlackTheme::Light);
    assert!(AppearanceMode::Light.resolve(WindowAppearance::Dark) == SlackTheme::Light);
}

#[test]
fn settings_json_is_backwards_compatible_with_partial_payloads() {
    let from_empty: Settings = serde_json::from_str("{}").expect("empty json deserializes");
    assert!(from_empty == Settings::default());

    let partial: Settings =
        serde_json::from_str(r#"{"clock_24h":true,"enter_sends":false,"zoom":4}"#)
            .expect("partial json deserializes");
    assert!(partial.clock_24h);
    assert!(!partial.enter_sends);
    assert_eq!(partial.zoom, 4);
    assert_eq!(partial.warn_external, Settings::default().warn_external);
    assert_eq!(partial.show_dock_badge, Settings::default().show_dock_badge);

    let round_trip: Settings =
        serde_json::from_str(&serde_json::to_string(&partial).unwrap()).unwrap();
    assert!(round_trip == partial);
}

#[test]
fn spellcheck_language_timezone_and_schedule_helpers() {
    let mut s = Settings::default();
    assert_eq!(s.spellcheck_language(), Some("en_US"));
    s.language = 2;
    assert_eq!(s.spellcheck_language(), Some("sv"));
    s.language = 99;
    assert_eq!(s.spellcheck_language(), None);

    assert_eq!(s.timezone_offset_secs(), None);
    s.timezone_auto = false;
    s.timezone = 0;
    assert_eq!(s.timezone_offset_secs(), Some(3_600));
    s.timezone = 2;
    assert_eq!(s.timezone_offset_secs(), Some(-18_000));
    s.timezone = 99;
    assert_eq!(s.timezone_offset_secs(), Some(0));

    s.schedule_enabled = false;
    assert!(s.notifications_allowed_now());
    assert_eq!(s.schedule_summary(), "Off");

    s.notify_level = 0;
    assert_eq!(s.notify_level_wire(), "everything");
    s.notify_level = 1;
    assert_eq!(s.notify_level_wire(), "mentions_dms");
    s.notify_level = 2;
    assert_eq!(s.notify_level_wire(), "nothing");

    s.mobile_handoff = 3;
    assert_eq!(s.mobile_handoff_secs(), 300);
    s.schedule_days = 0;
    assert_eq!(s.schedule_days_wire(), "every_day");
    s.schedule_days = 1;
    assert_eq!(s.schedule_days_wire(), "weekdays");
    s.schedule_days = 2;
    assert_eq!(s.schedule_days_wire(), "custom");

    s.notify_level = 0;
    s.mobile_different = false;
    assert_eq!(s.mobile_notify_level_wire(), "everything");
    s.mobile_different = true;
    s.mobile_notify_level = 2;
    assert_eq!(s.mobile_notify_level_wire(), "nothing");

    s.schedule_enabled = true;
    s.schedule_days = 2;
    s.schedule_custom_days = [false; 7];
    assert!(!s.notifications_allowed_now());

    s.vip_users = "@ada, U999, Grace Hopper".to_owned();
    assert!(s.is_vip("U111", "ada", "Ada Lovelace"));
    assert!(s.is_vip("U999", "bob", "Bob"));
    assert!(s.is_vip("U222", "grace", "Grace Hopper"));
    assert!(!s.is_vip("U333", "carol", "Carol"));
    assert!(s.is_urgent_dm_breakthrough(true, "[URGENT] prod DB down"));
    assert!(s.is_urgent_dm_breakthrough(true, "\u{1F6A8} page on-call"));
    assert!(!s.is_urgent_dm_breakthrough(false, "[URGENT] channel chatter"));
    s.urgent_dm_override = false;
    assert!(!s.is_urgent_dm_breakthrough(true, "[URGENT] prod DB down"));
}
