use super::expand_shortcodes_in_text;

#[test]
fn expands_standard_emoji_shortcodes_inside_link_labels() {
    assert_eq!(
        expand_shortcodes_in_text(":chart_with_upwards_trend: Dashboard"),
        "📈 Dashboard"
    );
    assert_eq!(expand_shortcodes_in_text(":bar_chart: Panel"), "📊 Panel");
    assert_eq!(expand_shortcodes_in_text(":mute: Silence"), "🔇 Silence");
    assert_eq!(
        expand_shortcodes_in_text("https://example.com:8080/path"),
        "https://example.com:8080/path"
    );
}
