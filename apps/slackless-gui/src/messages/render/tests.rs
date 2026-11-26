use slackless_core::domain::mrkdwn::PlainDirectory;

use super::{expand_shortcodes_in_text, plain_text};

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

/// The clipboard / notification / list-preview text: references as the transcript names them,
/// formatting and link syntax dropped, standard emoji as glyphs, custom emoji as `:name:`.
#[test]
fn plain_text_names_references_through_the_workspace() {
    let fixtures = crate::fixtures::load();
    let raw = "cc <@U0BOB> and <@U0CARA|cara>, ask <@U0NOPE|zoe> in <#C0GENERAL> <!channel> \
               :rocket: :partyparrot: *bold* <https://github.com/acme/api|diff> &amp; done";
    assert_eq!(
        plain_text(raw, &fixtures),
        "cc @bob and @cara, ask @zoe in #general @channel 🚀 :partyparrot: bold diff & done"
    );
    // Without a directory that knows the workspace, a mention keeps its label, else its id.
    assert_eq!(
        plain_text("<@U0BOB> <@U0CARA|cara> <#C0GENERAL>", &PlainDirectory),
        "@U0BOB @cara #C0GENERAL"
    );
}
