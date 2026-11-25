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
    /// An app/bot DM's Home tab (`conversations.info` `return_app_home=true`, `F-COLLAB-480`).
    Home,
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
    /// Open the channel canvas (its file id) in the native reader.
    OpenCanvas(String),
    /// Open a Slack List tab (its file id) in the native read-only Lists viewer.
    OpenList(String),
    /// Create the channel canvas, then open it like an existing one.
    AddCanvas,
}

/// One entry of the rendered tab row.
pub struct TabSpec {
    pub id: SharedString,
    /// Slack's `properties.tabs[].id` when derived from conversation properties (used by
    /// `conversations.removeTab` / `conversations.moveTab`, `F-CORE-061`).
    pub tab_id: Option<String>,
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
        tab_id: None,
        label: SharedString::from(label.to_owned()),
        icon,
        action,
    }
}

fn spec_with_id(
    id: &str,
    tab_id: Option<String>,
    label: &str,
    icon: IconName,
    action: TabAction,
) -> TabSpec {
    TabSpec {
        id: SharedString::from(id.to_owned()),
        tab_id,
        label: SharedString::from(label.to_owned()),
        icon,
        action,
    }
}

/// Slack's default tab set for a conversation that stores no tab configuration:
/// matches official Slack's clean `Messages | Add canvas | +` header (users can still add
/// Files & links or other tabs via `+`).
fn default_tabs() -> Vec<ConversationTabSpec> {
    Vec::new()
}

/// Slack's conversation header tab cap (Messages + up to 9 additional tabs = 10 total;
/// `F-COLLAB-201`).
pub const MAX_CONVERSATION_TABS: usize = 10;

/// Whether the derived conversation tab row currently exposes a Canvas or Add canvas tab
/// (`F-COLLAB-123`, `F-COLLAB-124`).
pub fn has_canvas_tab(specs: &[TabSpec]) -> bool {
    specs.iter().any(|s| {
        matches!(
            s.action,
            TabAction::OpenCanvas(_)
                | TabAction::AddCanvas
                | TabAction::Switch(ConversationTab::Canvas)
        )
    })
}

/// Whether another tab can be added to the conversation header under Slack's tab limit
/// (`F-COLLAB-201`).
pub fn can_add_tab(specs: &[TabSpec]) -> bool {
    specs.len() < MAX_CONVERSATION_TABS
}

/// Human-readable tab limit guard message when at or above [`MAX_CONVERSATION_TABS`]
/// (`F-COLLAB-201`).
pub fn tab_limit_message(specs: &[TabSpec]) -> Option<String> {
    if can_add_tab(specs) {
        None
    } else {
        Some(format!(
            "Tab limit reached ({}/{}). Remove a tab before adding another.",
            specs.len(),
            MAX_CONVERSATION_TABS
        ))
    }
}

/// Prepend the `Home` tab to `specs` for app/bot DMs (`F-COLLAB-480`).
pub fn with_app_home(mut specs: Vec<TabSpec>) -> Vec<TabSpec> {
    specs.insert(
        0,
        spec(
            "tab-home",
            "Home",
            IconName::House,
            TabAction::Switch(ConversationTab::Home),
        ),
    );
    specs
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
        (Some(tab), None) if !tab.is_disabled => Some(("Add canvas", TabAction::AddCanvas)),
        (None, Some(file)) => Some(("Canvas", TabAction::OpenCanvas(file))),
        (None, None) => Some(("Add canvas", TabAction::AddCanvas)),
        // A disabled canvas tab hides the canvas without offering to add another.
        (Some(_), _) => None,
    };
    if let Some((label, action)) = canvas {
        let cid = canvas_tab.and_then(|t| t.id.clone());
        row.push(spec_with_id(
            "tab-canvas",
            cid,
            label,
            IconName::LayoutTemplate,
            action,
        ));
    }

    for tab in tabs {
        match tab.kind.as_ref() {
            Some(ConversationTabKind::Files) => row.push(spec_with_id(
                "tab-files",
                tab.id.clone(),
                "Files & links",
                IconName::Files,
                TabAction::Switch(ConversationTab::Files),
            )),
            Some(ConversationTabKind::Bookmarks) => row.push(spec_with_id(
                "tab-pins",
                tab.id.clone(),
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
                row.push(spec_with_id(
                    "tab-folder",
                    tab.id.clone(),
                    label,
                    IconName::Folder,
                    TabAction::Switch(ConversationTab::Folder {
                        label: SharedString::from(label.to_owned()),
                        bookmark_id: folder.to_owned(),
                    }),
                ));
            }
            Some(ConversationTabKind::List) => {
                let list_id = tab
                    .data
                    .as_ref()
                    .and_then(|data| data.file_id.as_deref().or(data.list_id.as_deref()))
                    .filter(|id| !id.is_empty());
                let Some(list_id) = list_id else {
                    continue;
                };
                let label = tab
                    .label
                    .as_deref()
                    .filter(|label| !label.is_empty())
                    .unwrap_or("List");
                row.push(spec_with_id(
                    "tab-list",
                    tab.id.clone(),
                    label,
                    IconName::ListChecks,
                    TabAction::OpenList(list_id.to_owned()),
                ));
            }
            // The canvas slot is handled above; unknown types are left out.
            Some(ConversationTabKind::ChannelCanvas)
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
            ["Messages -> Switch(Messages)", "Add canvas -> AddCanvas",]
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
    fn derives_live_list_tabs_when_file_id_is_present() {
        let list_tab = ConversationTabSpec {
            kind: Some(ConversationTabKind::List),
            label: Some("Launch tasks".into()),
            data: Some(TabData {
                file_id: Some("F09LIST".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            row(&conversation(vec![list_tab], None), false),
            [
                "Messages -> Switch(Messages)",
                "Add canvas -> AddCanvas",
                "Launch tasks -> OpenList(\"F09LIST\")",
            ]
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

    #[test]
    fn exposes_canvas_tab_by_default_and_enforces_tab_limit() {
        // New conversation with no stored tabs gets Add canvas by default (`F-COLLAB-123`).
        let fresh = derive(&Conversation::default(), false);
        assert!(has_canvas_tab(&fresh));
        assert!(can_add_tab(&fresh));
        assert_eq!(tab_limit_message(&fresh), None);

        // Enabled ChannelCanvas tab without a file id yet still offers Add canvas (`F-COLLAB-123`).
        let uninit = derive(
            &conversation(vec![tab_spec(ConversationTabKind::ChannelCanvas)], None),
            false,
        );
        assert!(has_canvas_tab(&uninit));

        // Disabled canvas tab hides canvas (`F-COLLAB-124`).
        let mut disabled = tab_spec(ConversationTabKind::ChannelCanvas);
        disabled.is_disabled = true;
        let hidden = derive(&conversation(vec![disabled], Some("F1")), false);
        assert!(!has_canvas_tab(&hidden));

        // At MAX_CONVERSATION_TABS, adding more tabs is blocked with a clear message (`F-COLLAB-201`).
        let mut many_tabs = Vec::new();
        for i in 0..MAX_CONVERSATION_TABS {
            many_tabs.push(ConversationTabSpec {
                kind: Some(ConversationTabKind::Folder),
                label: Some(format!("Folder {i}")),
                data: Some(TabData {
                    folder_bookmark_id: Some(format!("Bk{i}")),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        let full = derive(&conversation(many_tabs, Some("F1")), false);
        assert!(full.len() >= MAX_CONVERSATION_TABS);
        assert!(!can_add_tab(&full));
        assert!(
            tab_limit_message(&full)
                .expect("limit message")
                .contains("Tab limit reached")
        );
    }
}
