//! The conversation tab row under the channel header: which tabs a conversation gets and
//! what clicking each one does. Derivation in live mode is keyed on the data Slack stores
//! per conversation - `properties.tabs` in display order after the implicit Messages tab,
//! and `properties.canvas`:
//!
//! - `files` renders "Files & links" (the channel's `files.list`).
//! - `bookmarks` renders Slack's "Pins" tab (the `pins.list` panel): a conversation with a
//!   bookmarks tab shows Pins, one without does not (observed on the recorded workspaces).
//! - `channel_canvas` renders "Canvas" (opened in Slack's web client) unless `is_disabled`,
//!   which hides the canvas without offering to add another.
//! - with no canvas at all Slack offers "Add canvas" (`conversations.canvases.create`).
//! - `folder` renders a tab under its own label listing that folder's `bookmarks.list`
//!   entries.
//! - anything slackless cannot make work yet (Slack List tabs, unknown types, a folder tab
//!   without its folder id) is left out rather than shown dead.
//!
//! A conversation storing no tab configuration gets Slack's default set (files). The sample
//! workspace keeps its fixed set (Canvas and List are its read-only fixture views). How the
//! tabs and the bodies they host render lives in `tab_views`; their data is fetched each
//! time a tab opens, never per channel.

use gpui_kit::assets::IconName;
use gpui_kit::gpui::SharedString;
use slackless_core::types::{Conversation, ConversationTabKind, ConversationTabSpec};

/// The conversation tab whose body fills the main column.
#[derive(Clone, Debug, PartialEq)]
pub enum ConversationTab {
    Messages,
    /// The read-only canvas fixture view (sample workspace).
    Canvas,
    /// The read-only list fixture view (sample workspace).
    List,
    /// "Files & links": the conversation's shared files.
    Files,
    /// "Pins": the right-hand pinned-messages panel over the messages column.
    Pins,
    /// A bookmark folder tab, listing its folder's bookmarks.
    Folder {
        label: SharedString,
        bookmark_id: String,
    },
}

/// What clicking a tab row entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum TabAction {
    /// Show this tab's body in the main column.
    Switch(ConversationTab),
    /// Open the channel canvas (its file id) in Slack's web client.
    OpenCanvas(String),
    /// Create the channel canvas, then open it like an existing one.
    AddCanvas,
}

/// One entry of the rendered tab row.
pub struct TabSpec {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: IconName,
    pub action: TabAction,
}

/// The sample workspace's row after Messages; its Canvas and List are read-only fixture views.
const SAMPLE_TABS: [(&str, &str, IconName, ConversationTab); 3] = [
    (
        "tab-canvas",
        "Canvas",
        IconName::LayoutTemplate,
        ConversationTab::Canvas,
    ),
    (
        "tab-list",
        "List",
        IconName::ListChecks,
        ConversationTab::List,
    ),
    (
        "tab-files",
        "Files & links",
        IconName::Files,
        ConversationTab::Files,
    ),
];

fn spec(id: &str, label: &str, icon: IconName, action: TabAction) -> TabSpec {
    TabSpec {
        id: SharedString::from(id.to_owned()),
        label: SharedString::from(label.to_owned()),
        icon,
        action,
    }
}

/// Slack's default tab set for a conversation that stores no tab configuration.
fn default_tabs() -> Vec<ConversationTabSpec> {
    vec![ConversationTabSpec {
        kind: Some(ConversationTabKind::Files),
        ..Default::default()
    }]
}

/// The tab row of `conversation`: the sample workspace's fixed set, else the conversation's
/// `properties.tabs` (see the module docs for the rules).
pub fn derive(conversation: &Conversation, sample: bool) -> Vec<TabSpec> {
    let mut row = vec![spec(
        "tab-messages",
        "Messages",
        IconName::MessageSquare,
        TabAction::Switch(ConversationTab::Messages),
    )];
    if sample {
        row.extend(
            SAMPLE_TABS
                .into_iter()
                .map(|(id, label, icon, tab)| spec(id, label, icon, TabAction::Switch(tab))),
        );
        return row;
    }

    let properties = conversation.properties.as_ref();
    let stored = properties.map(|p| p.tabs.as_slice()).unwrap_or_default();
    let defaults = default_tabs();
    let tabs: &[ConversationTabSpec] = if stored.is_empty() { &defaults } else { stored };

    let canvas_tab = tabs
        .iter()
        .find(|tab| tab.kind.as_ref() == Some(&ConversationTabKind::ChannelCanvas));
    let canvas_file = properties
        .and_then(|p| p.canvas.as_ref())
        .and_then(|canvas| canvas.file_id.as_ref())
        .map(|file| file.to_string());
    let canvas = match (canvas_tab, canvas_file) {
        (Some(tab), Some(file)) if !tab.is_disabled => {
            Some(("Canvas", TabAction::OpenCanvas(file)))
        }
        (None, Some(file)) => Some(("Canvas", TabAction::OpenCanvas(file))),
        (None, None) => Some(("Add canvas", TabAction::AddCanvas)),
        // A disabled canvas tab (or one without its file id) shows nothing.
        (Some(_), _) => None,
    };
    if let Some((label, action)) = canvas {
        row.push(spec("tab-canvas", label, IconName::LayoutTemplate, action));
    }

    for tab in tabs {
        match tab.kind.as_ref() {
            Some(ConversationTabKind::Files) => row.push(spec(
                "tab-files",
                "Files & links",
                IconName::Files,
                TabAction::Switch(ConversationTab::Files),
            )),
            Some(ConversationTabKind::Bookmarks) => row.push(spec(
                "tab-pins",
                "Pins",
                IconName::Pin,
                TabAction::Switch(ConversationTab::Pins),
            )),
            Some(ConversationTabKind::Folder) => {
                let Some(folder) = tab
                    .data
                    .as_ref()
                    .and_then(|data| data.folder_bookmark_id.as_deref())
                    .filter(|id| !id.is_empty())
                else {
                    continue;
                };
                let label = tab
                    .label
                    .as_deref()
                    .filter(|label| !label.is_empty())
                    .unwrap_or("Bookmarks");
                row.push(spec(
                    "tab-folder",
                    label,
                    IconName::Folder,
                    TabAction::Switch(ConversationTab::Folder {
                        label: SharedString::from(label.to_owned()),
                        bookmark_id: folder.to_owned(),
                    }),
                ));
            }
            // The canvas slot is handled above; List tabs and unknown types are left out.
            Some(ConversationTabKind::ChannelCanvas)
            | Some(ConversationTabKind::List)
            | Some(ConversationTabKind::Other(_))
            | None => {}
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::types::{
        ChannelCanvas, ChannelId, ConversationProperties, FileId, TabData,
    };

    fn conversation(tabs: Vec<ConversationTabSpec>, canvas_file: Option<&str>) -> Conversation {
        Conversation {
            id: ChannelId::from("C1"),
            properties: Some(ConversationProperties {
                tabs,
                canvas: canvas_file.map(|file| ChannelCanvas {
                    file_id: Some(FileId::from(file)),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn tab_spec(kind: ConversationTabKind) -> ConversationTabSpec {
        ConversationTabSpec {
            kind: Some(kind),
            ..Default::default()
        }
    }

    /// The derived row as `label -> action` tokens.
    fn row(conversation: &Conversation, sample: bool) -> Vec<String> {
        derive(conversation, sample)
            .into_iter()
            .map(|spec| match spec.action {
                TabAction::Switch(ConversationTab::Folder { bookmark_id, .. }) => {
                    format!("{} -> folder {bookmark_id}", spec.label)
                }
                action => format!("{} -> {action:?}", spec.label),
            })
            .collect()
    }

    /// #servicedesk_public as recorded: no canvas, so "Add canvas"; bookmarks renders Pins.
    #[test]
    fn derives_servicedesk_row_with_add_canvas_and_pins() {
        let tabs = vec![
            tab_spec(ConversationTabKind::Files),
            tab_spec(ConversationTabKind::Bookmarks),
        ];
        assert_eq!(
            row(&conversation(tabs, None), false),
            [
                "Messages -> Switch(Messages)",
                "Add canvas -> AddCanvas",
                "Files & links -> Switch(Files)",
                "Pins -> Switch(Pins)",
            ]
        );
    }

    /// #out-of-office as recorded: a disabled canvas tab hides the canvas without offering
    /// to add one, and without a bookmarks tab there is no Pins tab.
    #[test]
    fn derives_out_of_office_row_without_canvas_or_pins() {
        let mut canvas_tab = tab_spec(ConversationTabKind::ChannelCanvas);
        canvas_tab.is_disabled = true;
        let tabs = vec![tab_spec(ConversationTabKind::Files), canvas_tab];
        assert_eq!(
            row(&conversation(tabs, Some("F06M7CVHPJP")), false),
            [
                "Messages -> Switch(Messages)",
                "Files & links -> Switch(Files)"
            ]
        );
    }

    /// #eng-pim-public's tabs plus an enabled canvas: the canvas opens its stored file and
    /// the folder tab carries its label and folder bookmark id, in stored order.
    #[test]
    fn derives_canvas_and_folder_tabs_in_stored_order() {
        let folder = ConversationTabSpec {
            kind: Some(ConversationTabKind::Folder),
            label: Some("Bookmarks".into()),
            data: Some(TabData {
                folder_bookmark_id: Some("Bk0BTFNME285".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let tabs = vec![
            folder,
            tab_spec(ConversationTabKind::Bookmarks),
            tab_spec(ConversationTabKind::Files),
            tab_spec(ConversationTabKind::ChannelCanvas),
        ];
        assert_eq!(
            row(&conversation(tabs, Some("F1")), false),
            [
                "Messages -> Switch(Messages)",
                "Canvas -> OpenCanvas(\"F1\")",
                "Bookmarks -> folder Bk0BTFNME285",
                "Pins -> Switch(Pins)",
                "Files & links -> Switch(Files)",
            ]
        );
    }

    #[test]
    fn derives_the_default_row_without_stored_tabs() {
        let conversation = Conversation {
            id: ChannelId::from("C2"),
            ..Default::default()
        };
        assert_eq!(
            row(&conversation, false),
            [
                "Messages -> Switch(Messages)",
                "Add canvas -> AddCanvas",
                "Files & links -> Switch(Files)",
            ]
        );
    }

    #[test]
    fn leaves_out_tabs_it_cannot_serve() {
        let tabs = vec![
            tab_spec(ConversationTabKind::List),
            tab_spec(ConversationTabKind::Other("workflow".into())),
            // A folder tab without its folder bookmark id has nothing to list.
            tab_spec(ConversationTabKind::Folder),
        ];
        assert_eq!(
            row(&conversation(tabs, None), false),
            ["Messages -> Switch(Messages)", "Add canvas -> AddCanvas"]
        );
    }

    #[test]
    fn keeps_the_sample_workspace_row() {
        assert_eq!(
            row(&Conversation::default(), true),
            [
                "Messages -> Switch(Messages)",
                "Canvas -> Switch(Canvas)",
                "List -> Switch(List)",
                "Files & links -> Switch(Files)",
            ]
        );
    }
}
