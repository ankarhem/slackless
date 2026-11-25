//! Canvas document data model re-exported from `slackless_core::domain::canvas_doc`, plus the
//! GPUI lookup adapter and bundled sample canvas fixture.

use crate::canvas::CanvasUserLookupFn;

pub use slackless_core::domain::canvas_doc::{
    CanvasBlock, CanvasDoc, CanvasEmbedKind, CanvasMediaKind as MediaKind, CanvasUserProfile,
    serialize_markdown_table,
};

/// Resolve profile card display fields against a host-supplied [`CanvasUserLookupFn`]
/// (`F-COLLAB-147`).
pub fn resolve_profile_card(
    user_id: &str,
    fallback_name: Option<&str>,
    fallback_title: Option<&str>,
    fallback_handle: Option<&str>,
    lookup: Option<&CanvasUserLookupFn>,
) -> CanvasUserProfile {
    slackless_core::domain::canvas_doc::resolve_profile_card(
        user_id,
        fallback_name,
        fallback_title,
        fallback_handle,
        lookup.map(|f| move |id: &str| f(id)),
    )
}

/// The sample canvas every conversation shows in fixture mode.
pub(super) fn fixture() -> CanvasDoc {
    let blocks = vec![
        CanvasBlock::Heading {
            level: 1,
            text: "Q4 launch plan :rocket:".into(),
        },
        CanvasBlock::Paragraph(
            "This canvas tracks everything we need to ship the desktop client before the \
             end of the quarter. Owners update their rows; the channel gets a weekly digest."
                .into(),
        ),
        CanvasBlock::Heading {
            level: 2,
            text: "Goals".into(),
        },
        CanvasBlock::Bullets(vec![
            "Native desktop client with feature parity for messaging".into(),
            "Offline search over the local archive".into(),
            "Huddles via native audio and video".into(),
        ]),
        CanvasBlock::Heading {
            level: 2,
            text: "Rollout steps".into(),
        },
        CanvasBlock::NumberedList(vec![
            "Verify release artifacts and notarization".into(),
            "Publish beta build to internal testers".into(),
            "Announce general availability in #announcements".into(),
        ]),
        CanvasBlock::Quote(
            "Keep every milestone owner accountable.\nFlag blockers in the weekly sync.".into(),
        ),
        CanvasBlock::Heading {
            level: 2,
            text: "Launch checklist".into(),
        },
        CanvasBlock::Checklist(vec![
            (true, "Sign the macOS build".into()),
            (true, "Fixture-driven screenshots for the docs".into()),
            (false, "Beta invite list reviewed by support".into()),
            (false, "Release notes drafted".into()),
        ]),
        CanvasBlock::Divider,
        CanvasBlock::Heading {
            level: 2,
            text: "Milestones".into(),
        },
        CanvasBlock::Table {
            header: vec![
                "Milestone".into(),
                "Owner".into(),
                "Due".into(),
                "Status".into(),
            ],
            rows: vec![
                vec![
                    "Private beta".into(),
                    "Maya Chen".into(),
                    "Oct 14".into(),
                    ":white_check_mark: Done".into(),
                ],
                vec![
                    "Public beta".into(),
                    "Leo Park".into(),
                    "Nov 4".into(),
                    "In progress".into(),
                ],
                vec![
                    "General availability".into(),
                    "Sam Rivera".into(),
                    "Dec 2".into(),
                    "Not started".into(),
                ],
            ],
        },
        CanvasBlock::Heading {
            level: 3,
            text: "Open questions".into(),
        },
        CanvasBlock::Paragraph(
            "Do we gate huddles behind a setting while rolling out screen sharing?".into(),
        ),
        CanvasBlock::ProfileCard {
            user_id: "U01MAYA01".into(),
            display_name: Some("Maya Chen".into()),
            title: Some("Staff Engineer · Launch Lead".into()),
            handle: Some("maya".into()),
        },
        CanvasBlock::MediaEmbed {
            kind: MediaKind::Video,
            title: "desktop-beta-walkthrough.mp4".into(),
            url:
                "https://files.slack.com/files-pri/T01SAMPLE-F01VID01/desktop-beta-walkthrough.mp4"
                    .into(),
            file_id: Some("F01VID01".into()),
            duration: Some("2:14".into()),
            mime: Some("video/mp4".into()),
        },
        CanvasBlock::DocEmbed {
            kind: CanvasEmbedKind::List,
            file_id: "F01LIST01".into(),
            title: "Launch Readiness Tracker".into(),
            url: "https://slackless.slack.com/lists/T01SAMPLE/F01LIST01".into(),
        },
    ];
    let section_ids = vec![None; blocks.len()];
    CanvasDoc {
        title: "Q4 launch plan".into(),
        meta: "Canvas · last edited by Maya Chen".into(),
        blocks,
        section_ids,
    }
}
