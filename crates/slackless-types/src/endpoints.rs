//! Every URL and Slack method name slackless talks to, in one place.
//!
//! Slack's web-client API is private and changes without notice; when an endpoint moves or a
//! method is renamed, the fix is a one-line edit here. Method names are the exact strings that go
//! after `/api/` in `POST https://<workspace>.slack.com/api/<method>`.

/// Generic API host; works for most methods with a session token + `d` cookie.
pub const SLACK_API_BASE: &str = "https://slack.com/api/";

/// Web origin sent on websocket handshakes and some HTTP calls.
pub const WEB_ORIGIN: &str = "https://app.slack.com";

/// User-Agent of the desktop browser the web client runs in. Every HTTP request and the websocket
/// handshake carry it, so Slack sees one browser.
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

/// Web client page; its HTML carries the `data-version-ts` attributes used at boot.
pub const APP_CLIENT_URL: &str = "https://app.slack.com/client";

/// Edge cache API host (users/channels lookups), path continues with `<team_or_org_id>/<method>`.
pub const EDGE_API_BASE: &str = "https://edgeapi.slack.com/cache/";

/// Message-server websocket used by the web client.
pub const WSS_PRIMARY: &str = "wss://wss-primary.slack.com/";

/// Per-workspace API base, e.g. `https://acme.slack.com/api/` (Grid: `acme.enterprise`).
pub fn workspace_api_base(domain: &str) -> String {
    format!("https://{domain}.slack.com/api/")
}

/// Joins an API base (ending in `/`) and a method name.
pub fn method_url(base: &str, method: &str) -> String {
    format!("{base}{method}")
}

/// Active Slack method names referenced across the workspace.
pub mod method {
    pub const ACTIVITY_FEED: &str = "activity.feed";
    pub const ACTIVITY_MARK_READ: &str = "activity.markRead";
    pub const ACTIVITY_MARK_UNREAD: &str = "activity.markUnread";
    pub const ADMIN_CONVERSATIONS_DELETE: &str = "admin.conversations.delete";
    pub const ADMIN_ROLES_ENTITY_LIST_ASSIGNMENTS: &str = "admin.roles.entity.listAssignments";
    pub const APPS_HOME_DISPATCH_OPEN_EVENT: &str = "apps.home.dispatchOpenEvent";
    pub const APPS_HOME_OPEN: &str = "apps.home.open";
    pub const AUTH_REVOKE: &str = "auth.revoke";
    pub const AUTH_TEST: &str = "auth.test";
    pub const BLOCKS_ACTIONS: &str = "blocks.actions";
    pub const BLOCKS_SUGGESTIONS: &str = "blocks.suggestions";
    pub const BOOKMARKS_ADD: &str = "bookmarks.add";
    pub const BOOKMARKS_LIST: &str = "bookmarks.list";
    pub const BOOKMARKS_REMOVE: &str = "bookmarks.remove";
    pub const BOTS_INFO: &str = "bots.info";
    pub const CALLS_INFO: &str = "calls.info";
    pub const CANVASES_ACCESS_DELETE: &str = "canvases.access.delete";
    pub const CANVASES_ACCESS_SET: &str = "canvases.access.set";
    pub const CANVASES_CREATE: &str = "canvases.create";
    pub const CANVASES_DELETE: &str = "canvases.delete";
    pub const CANVASES_EDIT: &str = "canvases.edit";
    pub const CANVASES_GET_CONTENT: &str = "canvases.getContent";
    pub const CHAT_COMMAND: &str = "chat.command";
    pub const CHAT_DELETE: &str = "chat.delete";
    pub const CHAT_DELETE_SCHEDULED_MESSAGE: &str = "chat.deleteScheduledMessage";
    pub const CHAT_POST_MESSAGE: &str = "chat.postMessage";
    pub const CHAT_SCHEDULED_MESSAGES_LIST: &str = "chat.scheduledMessages.list";
    pub const CHAT_SCHEDULE_MESSAGE: &str = "chat.scheduleMessage";
    pub const CHAT_UPDATE: &str = "chat.update";
    pub const CLIENT_APP_COMMANDS: &str = "client.appCommands";
    pub const CLIENT_COUNTS: &str = "client.counts";
    pub const CLIENT_DMS: &str = "client.dms";
    pub const CLIENT_USER_BOOT: &str = "client.userBoot";
    pub const CONVERSATIONS_ADD_TAB: &str = "conversations.addTab";
    pub const CONVERSATIONS_ARCHIVE: &str = "conversations.archive";
    pub const CONVERSATIONS_CANVASES_CREATE: &str = "conversations.canvases.create";
    pub const CONVERSATIONS_CONVERT_TO_PRIVATE: &str = "conversations.convertToPrivate";
    pub const CONVERSATIONS_CREATE: &str = "conversations.create";
    pub const CONVERSATIONS_HISTORY: &str = "conversations.history";
    pub const CONVERSATIONS_INFO: &str = "conversations.info";
    pub const CONVERSATIONS_INVITE: &str = "conversations.invite";
    pub const CONVERSATIONS_JOIN: &str = "conversations.join";
    pub const CONVERSATIONS_LEAVE: &str = "conversations.leave";
    pub const CONVERSATIONS_LIST: &str = "conversations.list";
    pub const CONVERSATIONS_MARK: &str = "conversations.mark";
    pub const CONVERSATIONS_MEMBERS: &str = "conversations.members";
    pub const CONVERSATIONS_MOVE_TAB: &str = "conversations.moveTab";
    pub const CONVERSATIONS_OPEN: &str = "conversations.open";
    pub const CONVERSATIONS_REMOVE_TAB: &str = "conversations.removeTab";
    pub const CONVERSATIONS_RENAME: &str = "conversations.rename";
    pub const CONVERSATIONS_REPLIES: &str = "conversations.replies";
    pub const CONVERSATIONS_SET_PURPOSE: &str = "conversations.setPurpose";
    pub const CONVERSATIONS_SET_TOPIC: &str = "conversations.setTopic";
    pub const CONVERSATIONS_TEAM_CONNECTIONS: &str = "conversations.teamConnections";
    pub const CONVERSATIONS_UNARCHIVE: &str = "conversations.unarchive";
    pub const CONVERSATIONS_VIEW: &str = "conversations.view";
    pub const DND_END_DND: &str = "dnd.endDnd";
    pub const DND_END_SNOOZE: &str = "dnd.endSnooze";
    pub const DND_INFO: &str = "dnd.info";
    pub const DND_SET_SNOOZE: &str = "dnd.setSnooze";
    pub const DND_TEAM_INFO: &str = "dnd.teamInfo";
    pub const DRAFTS_CREATE: &str = "drafts.create";
    pub const DRAFTS_DELETE: &str = "drafts.delete";
    pub const DRAFTS_LIST: &str = "drafts.list";
    pub const DRAFTS_UPDATE: &str = "drafts.update";
    pub const EMOJI_LIST: &str = "emoji.list";
    pub const FILES_COLLECTIONS_CREATE: &str = "files.collections.create";
    pub const FILES_COLLECTIONS_DELETE: &str = "files.collections.delete";
    pub const FILES_COLLECTIONS_LIST: &str = "files.collections.list";
    pub const FILES_COMPLETE_UPLOAD_EXTERNAL: &str = "files.completeUploadExternal";
    pub const FILES_CREATE_CANVAS: &str = "files.createCanvas";
    pub const FILES_DELETE: &str = "files.delete";
    pub const FILES_FAVORITES_ADD: &str = "files.favorites.add";
    pub const FILES_FAVORITES_LIST: &str = "files.favorites.list";
    pub const FILES_FAVORITES_REMOVE: &str = "files.favorites.remove";
    pub const FILES_GET_UPLOAD_URL_EXTERNAL: &str = "files.getUploadURLExternal";
    /// Compatibility alias for [`FILES_GET_UPLOAD_URL_EXTERNAL`].
    pub const FILES_GET_UPLOAD_U_R_L_EXTERNAL: &str = FILES_GET_UPLOAD_URL_EXTERNAL;
    pub const FILES_INFO: &str = "files.info";
    pub const FILES_LIST: &str = "files.list";
    pub const FILES_RECENTLY_DELETED: &str = "files.recentlyDeleted";
    pub const FILES_REQUEST_PERMISSION: &str = "files.requestPermission";
    pub const FILES_UNDELETE: &str = "files.undelete";
    pub const GIF_FEATURED: &str = "gif.featured";
    pub const GIF_SEARCH: &str = "gif.search";
    pub const HUDDLES_UPDATE: &str = "huddles.update";
    pub const LISTS_ADD_TO_LIST: &str = "lists.addToList";
    pub const LISTS_CELLS_UPDATE: &str = "lists.cells.update";
    pub const LISTS_CREATE: &str = "lists.create";
    pub const LISTS_GET_THREADS_FOR_RECORD: &str = "lists.getThreadsForRecord";
    pub const LISTS_RECORDS_ARCHIVE: &str = "lists.records.archive";
    pub const LISTS_RECORDS_CREATE: &str = "lists.records.create";
    pub const LISTS_RECORDS_DELETE: &str = "lists.records.delete";
    pub const LISTS_RECORDS_LIST: &str = "lists.records.list";
    pub const MESSAGES_LIST: &str = "messages.list";
    pub const PINS_ADD: &str = "pins.add";
    pub const PINS_LIST: &str = "pins.list";
    pub const PINS_REMOVE: &str = "pins.remove";
    pub const PRESENCE_SET: &str = "presence.set";
    pub const QUIP_THREAD_ARCHIVE: &str = "quip.thread.archive";
    pub const QUIP_THREAD_ARCHIVE_ALL: &str = "quip.thread.archiveAll";
    pub const QUIP_THREAD_UNARCHIVE: &str = "quip.thread.unarchive";
    pub const REACTIONS_ADD: &str = "reactions.add";
    pub const REACTIONS_REMOVE: &str = "reactions.remove";
    pub const ROOMS_GET_LINK: &str = "rooms.getLink";
    pub const ROOMS_INVITE_RESPONSE: &str = "rooms.inviteResponse";
    pub const ROOMS_JOIN: &str = "rooms.join";
    pub const ROOMS_NOTIFY_MEMBER: &str = "rooms.notifyMember";
    pub const ROOMS_START_TRANSCRIPTION: &str = "rooms.startTranscription";
    pub const ROOMS_STOP_TRANSCRIPTION: &str = "rooms.stopTranscription";
    pub const SAVED_ADD: &str = "saved.add";
    pub const SAVED_CLEAR_COMPLETED: &str = "saved.clearCompleted";
    pub const SAVED_DELETE: &str = "saved.delete";
    pub const SAVED_LIST: &str = "saved.list";
    pub const SAVED_UPDATE: &str = "saved.update";
    pub const SCREENHERO_ROOMS_RENAME: &str = "screenhero.rooms.rename";
    pub const SEARCH_MESSAGES: &str = "search.messages";
    pub const SEARCH_MODULES_FILES: &str = "search.modules.files";
    pub const SEARCH_MODULES_MESSAGES: &str = "search.modules.messages";
    pub const SLACK_LISTS_ACCESS_DELETE: &str = "slackLists.access.delete";
    pub const SLACK_LISTS_ACCESS_SET: &str = "slackLists.access.set";
    pub const SLACK_LISTS_ITEMS_CREATE: &str = "slackLists.items.create";
    pub const SLACK_LISTS_ITEMS_DELETE: &str = "slackLists.items.delete";
    pub const SLACK_LISTS_ITEMS_LIST: &str = "slackLists.items.list";
    pub const SLACK_LISTS_ITEMS_UPDATE: &str = "slackLists.items.update";
    pub const SUBSCRIPTIONS_THREAD_ADD: &str = "subscriptions.thread.add";
    pub const SUBSCRIPTIONS_THREAD_GET_VIEW: &str = "subscriptions.thread.getView";
    pub const SUBSCRIPTIONS_THREAD_MARK: &str = "subscriptions.thread.mark";
    pub const SUBSCRIPTIONS_THREAD_REMOVE: &str = "subscriptions.thread.remove";
    pub const TEAM_INFO: &str = "team.info";
    pub const TEAM_PROFILE_GET: &str = "team.profile.get";
    pub const USERGROUPS_CREATE: &str = "usergroups.create";
    pub const USERGROUPS_LIST: &str = "usergroups.list";
    pub const USERGROUPS_UPDATE: &str = "usergroups.update";
    pub const USERS_CHANNEL_SECTIONS_CHANNELS_BULK_UPDATE: &str =
        "users.channelSections.channels.bulkUpdate";
    pub const USERS_CHANNEL_SECTIONS_CREATE: &str = "users.channelSections.create";
    pub const USERS_CHANNEL_SECTIONS_DELETE: &str = "users.channelSections.delete";
    pub const USERS_CHANNEL_SECTIONS_LIST: &str = "users.channelSections.list";
    pub const USERS_CHANNEL_SECTIONS_SET: &str = "users.channelSections.set";
    pub const USERS_CONVERSATIONS: &str = "users.conversations";
    pub const USERS_GET_PRESENCE: &str = "users.getPresence";
    pub const USERS_INFO: &str = "users.info";
    pub const USERS_LIST: &str = "users.list";
    pub const USERS_MARK_ALL_READ: &str = "users.markAllRead";
    pub const USERS_PREFS_SET: &str = "users.prefs.set";
    pub const USERS_PREFS_SET_NOTIFICATIONS: &str = "users.prefs.setNotifications";
    pub const USERS_PROFILE_GET: &str = "users.profile.get";
    pub const USERS_PROFILE_SET: &str = "users.profile.set";
    pub const USERS_SET_PHOTO: &str = "users.setPhoto";
    pub const VIEWS_CLOSE: &str = "views.close";
    pub const VIEWS_GET: &str = "views.get";
    pub const VIEWS_SUBMIT: &str = "views.submit";
    pub const WORKFLOWS_TRIGGERS_INFO: &str = "workflows.triggers.info";
    pub const WORKFLOWS_TRIGGERS_TRIP: &str = "workflows.triggers.trip";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_method_urls() {
        assert_eq!(
            method_url(&workspace_api_base("acme"), method::CHAT_POST_MESSAGE),
            "https://acme.slack.com/api/chat.postMessage"
        );
        assert_eq!(
            method_url(SLACK_API_BASE, method::CLIENT_USER_BOOT),
            "https://slack.com/api/client.userBoot"
        );
        assert_eq!(
            method::FILES_GET_UPLOAD_URL_EXTERNAL,
            method::FILES_GET_UPLOAD_U_R_L_EXTERNAL
        );
    }
}
