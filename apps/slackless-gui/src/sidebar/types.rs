//! Public and internal types for the sidebar: events, nav destinations, preferences, and badges.

use gpui_kit::assets::IconName;
use slackless_core::NotifyLevel;
use slackless_core::types::{ChannelId, Ts};

use crate::files_page::FilesSection;

pub enum SidebarEvent {
    SelectConversation(ChannelId),
    /// The "Add channels" row: open the channel browser.
    BrowseChannels,
    /// Create a new public or private channel (`conversations.create`).
    CreateChannel,
    /// A workspace tile in the rail was clicked (index into `Fixtures::workspaces`).
    SwitchWorkspace(usize),
    /// Reorder a workspace tile in the rail (`F-CORE-003`).
    ReorderWorkspace {
        from: usize,
        to: usize,
    },
    /// Open `channel` in a separate native GPUI window (`F-CORE-026`).
    OpenInNewWindow(ChannelId),
    /// Per-section sort or filter changed; persist to SQLite `local_state`.
    SectionPrefsChanged,
    /// The "Drafts & sent" row.
    OpenDrafts,
    /// The "Saved" row.
    OpenLater,
    /// The "Threads" row: open the Threads view.
    OpenThreads,
    /// The Home list's "Unreads" row: open the All Unreads view (Slack's `p-unreads_view`).
    OpenUnreads,
    /// The "Huddles" row: open the Huddles page.
    OpenHuddles,
    /// The "Directories" row: open the directory.
    OpenDirectories,
    /// The workspace header's pen: open the "New message" view.
    OpenCompose,
    /// The gear menu's "Create a channel section" dialog confirmed `name`.
    CreateSection {
        name: String,
    },
    /// The row menu's "+ Create new section": the dialog confirmed `name`, and `channel`
    /// should move into the fresh section.
    CreateSectionWithChannel {
        name: String,
        channel: ChannelId,
    },
    /// Rename a custom section (`users.channelSections.set`).
    RenameSection {
        section: String,
        name: String,
    },
    /// Delete a custom section (`users.channelSections.delete`), after its confirm dialog.
    DeleteSection {
        section: String,
    },
    /// Move a conversation between sections (`users.channelSections.channels.bulkUpdate`);
    /// `from` / `to` name custom sections, `None` being the default bucket on either side.
    MoveChannel {
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
    },
    /// A section menu's "Mark all as read": move `channel`'s cursor to `ts` (the root's
    /// existing `Command::MarkRead` path).
    MarkRead {
        channel: ChannelId,
        ts: Ts,
    },
    /// A row menu or "Mute all conversations" choice: the existing
    /// `Command::SetNotifyLevel` path.
    SetNotifyLevel {
        channel: ChannelId,
        level: NotifyLevel,
    },
    /// A row menu's "Channel details": open the existing details dialog.
    OpenChannelDetails(ChannelId),
    /// A row menu's "Leave channel": the existing `Command::LeaveConversation` path.
    LeaveChannel(ChannelId),
    /// The rail's moon: turn focus mode on or off.
    ToggleFocus,
    /// The focus card's Duration choice (an index into `status::FOCUS_DURATIONS`).
    FocusDuration(usize),
    /// The rail's own avatar: open the user menu.
    OpenUserMenu,
    /// The rail's Activity tile: open the Activity page, which the root renders beside the
    /// rail instead of this sidebar's list column.
    OpenActivity,
    /// A rail destination other than Activity was shown while the Activity page was open;
    /// the root stops rendering the page.
    LeftActivity,
    /// The rail's Files tile: open the Files view, whose page the root renders in the main
    /// area beside this sidebar's Files list column.
    OpenFiles,
    /// A Files list-column row: show that page of the Files view.
    ShowFilesSection(FilesSection),
    /// The rail's Agents & tools tile: open the dedicated native Tools page.
    OpenTools,
    /// The rail's Admin tile: open workspace administration in the browser.
    OpenAdmin,
    /// Open the "Clean up inactive channels" bulk-leave assistant (`F-CORE-023`, `F-SHELL-070`).
    OpenCleanupChannels,
}

/// The nav destinations stacked in the workspace rail.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum NavItem {
    Home,
    Dms,
    Activity,
    Files,
    Todos,
    Agents,
    Admin,
}

impl NavItem {
    pub(super) fn label(self) -> &'static str {
        match self {
            NavItem::Home => "Home",
            NavItem::Dms => "DMs",
            NavItem::Activity => "Activity",
            NavItem::Files => "Files",
            NavItem::Todos => "To-dos",
            NavItem::Agents => "Agents & tools",
            NavItem::Admin => "Admin",
        }
    }

    pub(super) fn icon(self) -> IconName {
        match self {
            NavItem::Home => IconName::House,
            NavItem::Dms => IconName::MessageCircle,
            NavItem::Activity => IconName::Bell,
            NavItem::Files => IconName::Files,
            NavItem::Todos => IconName::CircleCheck,
            NavItem::Agents => IconName::LayoutGrid,
            NavItem::Admin => IconName::Settings,
        }
    }
}

/// Which Home sidebar shortcut page currently owns the main column (`None` = a conversation).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HomePage {
    Unreads,
    Threads,
    Huddles,
    Drafts,
    Saved,
    Directories,
}

/// Navigation rail visibility, order, and label style driven by Preferences -> Navigation (`F-CORE-013`, `F-CORE-014`, `F-SHELL-062`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NavPrefs {
    pub show_home: bool,
    pub show_dms: bool,
    pub show_activity: bool,
    pub show_files: bool,
    pub show_later: bool,
    pub icons_only: bool,
    /// Display order of the 5 configurable tabs (`0=Home, 1=DMs, 2=Activity, 3=Files, 4=Later`).
    pub order: [usize; 5],
}

impl Default for NavPrefs {
    fn default() -> Self {
        Self {
            show_home: true,
            show_dms: true,
            show_activity: true,
            show_files: true,
            show_later: true,
            icons_only: false,
            order: [0, 1, 2, 3, 4],
        }
    }
}

impl NavPrefs {
    const CONFIGURABLE: [NavItem; 5] = [
        NavItem::Home,
        NavItem::Dms,
        NavItem::Activity,
        NavItem::Files,
        NavItem::Todos,
    ];

    pub(super) fn shows(self, item: NavItem) -> bool {
        match item {
            NavItem::Home => self.show_home,
            NavItem::Dms => self.show_dms,
            NavItem::Activity => self.show_activity,
            NavItem::Files => self.show_files,
            NavItem::Todos => self.show_later,
            NavItem::Agents | NavItem::Admin => true,
        }
    }

    pub(super) fn ordered_items(self) -> Vec<NavItem> {
        let mut items = Vec::with_capacity(7);
        for &idx in &self.order {
            if let Some(&item) = Self::CONFIGURABLE.get(idx)
                && !items.contains(&item)
            {
                items.push(item);
            }
        }
        for item in Self::CONFIGURABLE {
            if !items.contains(&item) {
                items.push(item);
            }
        }
        items.push(NavItem::Agents);
        items.push(NavItem::Admin);
        items
    }
}

/// Home sidebar display preferences driven by Preferences -> Home (`F-CORE-022`, `F-CORE-024`, `F-SHELL-066`, `F-SHELL-072`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HomePrefs {
    pub show: usize,
    pub sort: usize,
    pub unreads_section: bool,
    pub separate_private: bool,
    pub profile_photos: bool,
    pub compact: bool,
    pub show_muted: bool,
}

impl Default for HomePrefs {
    fn default() -> Self {
        Self {
            show: 0,
            sort: 0,
            unreads_section: false,
            separate_private: false,
            profile_photos: true,
            compact: false,
            show_muted: false,
        }
    }
}

/// Unread/mention badge summary for one tile in the workspace switcher rail (`F-CORE-006`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct WorkspaceBadge {
    pub mentions: u32,
    pub has_unreads: bool,
}

/// Unread state of one conversation row, derived from the fixture history: messages the
/// signed-in user did not author that are newer than the newest message in the open
/// conversation (the fixture-mode "read" watermark). `mentions` is the subset of `count`
/// that @-mentions the signed-in user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Unread {
    pub(super) count: u32,
    pub(super) mentions: u32,
}
