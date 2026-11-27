use std::rc::Rc;
use std::sync::Arc;

use slackless_core::domain::mrkdwn::PlainDirectory;

use super::{body::memoized_mrkdwn, plain_text};

/// Block Kit text objects re-render every frame: their mrkdwn is modeled once per text (and
/// `verbatim`), and modeled again once the text, the workspace data or the emoji preference
/// changes (names and custom emoji resolve through both).
#[test]
fn block_kit_mrkdwn_is_modeled_once_per_text() {
    let fixtures = Arc::new(crate::fixtures::load());
    let text = "*hey* <@U0BOB> :rocket:";
    let first = memoized_mrkdwn(text, false, &fixtures, false);
    let again = memoized_mrkdwn(text, false, &fixtures, false);
    assert!(Rc::ptr_eq(&first, &again), "the same text reuses its model");
    assert!(
        !Rc::ptr_eq(
            &first,
            &memoized_mrkdwn("*hey* <@U0CARA>", false, &fixtures, false)
        ),
        "changed text is modeled again"
    );
    assert!(
        !Rc::ptr_eq(&first, &memoized_mrkdwn(text, true, &fixtures, false)),
        "a verbatim text object is modeled again"
    );
    assert!(
        !Rc::ptr_eq(&first, &memoized_mrkdwn(text, false, &fixtures, true)),
        "the emoji preference re-models"
    );
    let other_workspace = Arc::new(crate::fixtures::load());
    assert!(
        !Rc::ptr_eq(
            &first,
            &memoized_mrkdwn(text, false, &other_workspace, false)
        ),
        "new workspace data re-models"
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

/// Known `:shortcode:`s become glyphs and every other colon stays put, whatever byte length the
/// text around a colon has: a multi-byte first character (`Är du klar: ja?` is a DM preview on
/// every frame and a notification body), a bullet, CJK, a shortcode followed by a multi-byte
/// character, a link label, and a URL port.
#[test]
fn plain_text_expands_shortcodes_without_slicing_multibyte_text() {
    let fixtures = crate::fixtures::load();
    for (raw, shown) in [
        (":chart_with_upwards_trend: Dashboard", "📈 Dashboard"),
        (":bar_chart: Panel", "📊 Panel"),
        (":mute: Silence", "🔇 Silence"),
        (
            "https://example.com:8080/path",
            "https://example.com:8080/path",
        ),
        ("Är du klar: ja?", "Är du klar: ja?"),
        ("• Owner: Alice", "• Owner: Alice"),
        ("日程: 12:00", "日程: 12:00"),
        (":tada:Är:", "🎉Är:"),
        ("<https://example.com|:bar_chart: Panel>", "📊 Panel"),
    ] {
        assert_eq!(plain_text(raw, &fixtures), shown, "{raw:?}");
    }
}
