//! Every URL and Slack method name slackless talks to, in one place.
//!
//! Slack's web-client API is private and changes without notice; when an endpoint moves or a
//! method is renamed, the fix is a one-line edit here. Method names are the exact strings that go
//! after `/api/` in `POST https://<workspace>.slack.com/api/<method>`.

/// Generic API host; works for most methods with a session token + `d` cookie.
pub const SLACK_API_BASE: &str = "https://slack.com/api/";

/// Web origin sent on websocket handshakes and some HTTP calls.
pub const WEB_ORIGIN: &str = "https://app.slack.com";

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

/// Slack method names, one constant per researched endpoint (`docs/research/api-*.md`).
pub mod method {
    pub const ACTIVITY_FEED: &str = "activity.feed";
    pub const ACTIVITY_MARK_READ: &str = "activity.markRead";
    pub const ACTIVITY_MARK_UNREAD: &str = "activity.markUnread";
    pub const ACTIVITY_VIEWS: &str = "activity.views";
    pub const ADMIN_ROLES_ENTITY_LIST_ASSIGNMENTS: &str = "admin.roles.entity.listAssignments";
    pub const API_FEATURES: &str = "api.features";
    pub const APPS_ACTIONS_LIST: &str = "apps.actions.list";
    pub const APPS_ACTIONS_RUN: &str = "apps.actions.run";
    pub const APPS_HOME_DISPATCH_OPEN_EVENT: &str = "apps.home.dispatchOpenEvent";
    pub const APPS_HOME_OPEN: &str = "apps.home.open";
    pub const APPS_PERMISSIONS_INTERNAL_ADD: &str = "apps.permissions.internal.add";
    pub const APPS_PERMISSIONS_INTERNAL_DENY_ADD: &str = "apps.permissions.internal.denyAdd";
    pub const AUTH_CAPTCHA: &str = "auth.captcha";
    pub const AUTH_FIND_TEAM: &str = "auth.findTeam";
    pub const AUTH_FIND_USER: &str = "auth.findUser";
    pub const AUTH_LOGIN_MAGIC: &str = "auth.loginMagic";
    pub const AUTH_LOGIN_MAGIC_BULK: &str = "auth.loginMagicBulk";
    pub const AUTH_REVOKE: &str = "auth.revoke";
    pub const AUTH_SIGNIN: &str = "auth.signin";
    pub const AUTH_SIGNOUT: &str = "auth.signout";
    pub const AUTH_TEAMS_LIST: &str = "auth.teams.list";
    pub const AUTH_TEST: &str = "auth.test";
    pub const BLOCKS_ACTIONS: &str = "blocks.actions";
    pub const BLOCKS_SUGGESTIONS: &str = "blocks.suggestions";
    pub const BOOKMARKS_ADD: &str = "bookmarks.add";
    pub const BOOKMARKS_EDIT: &str = "bookmarks.edit";
    pub const BOOKMARKS_LIST: &str = "bookmarks.list";
    pub const BOOKMARKS_REMOVE: &str = "bookmarks.remove";
    pub const BOTS_INFO: &str = "bots.info";
    pub const CALLS_ADD: &str = "calls.add";
    pub const CALLS_END: &str = "calls.end";
    pub const CALLS_INFO: &str = "calls.info";
    pub const CALLS_PARTICIPANTS_ADD: &str = "calls.participants.add";
    pub const CALLS_PARTICIPANTS_REMOVE: &str = "calls.participants.remove";
    pub const CALLS_UPDATE: &str = "calls.update";
    pub const CANVASES_ACCESS_DELETE: &str = "canvases.access.delete";
    pub const CANVASES_ACCESS_SET: &str = "canvases.access.set";
    pub const CANVASES_CREATE: &str = "canvases.create";
    pub const CANVASES_DELETE: &str = "canvases.delete";
    pub const CANVASES_EDIT: &str = "canvases.edit";
    pub const CANVASES_GET_CONTENT: &str = "canvases.getContent";
    pub const CANVASES_LIST_HEADERS: &str = "canvases.listHeaders";
    pub const CANVASES_SECTIONS_LOOKUP: &str = "canvases.sections.lookup";
    pub const CHAT_ATTACHMENT_ACTION: &str = "chat.attachmentAction";
    pub const CHAT_ATTACHMENT_SUGGESTION: &str = "chat.attachmentSuggestion";
    pub const CHAT_COMMAND: &str = "chat.command";
    pub const CHAT_DELETE: &str = "chat.delete";
    pub const CHAT_DELETE_SCHEDULED_MESSAGE: &str = "chat.deleteScheduledMessage";
    pub const CHAT_ME_MESSAGE: &str = "chat.meMessage";
    pub const CHAT_POST_MESSAGE: &str = "chat.postMessage";
    pub const CHAT_SCHEDULED_MESSAGES_LIST: &str = "chat.scheduledMessages.list";
    pub const CHAT_SCHEDULE_MESSAGE: &str = "chat.scheduleMessage";
    pub const CHAT_UNFURL: &str = "chat.unfurl";
    pub const CHAT_UPDATE: &str = "chat.update";
    pub const CLIENT_APP_COMMANDS: &str = "client.appCommands";
    pub const CLIENT_BOOT: &str = "client.boot";
    pub const CLIENT_COUNTS: &str = "client.counts";
    pub const CLIENT_DMS: &str = "client.dms";
    pub const CLIENT_SHOULD_RELOAD: &str = "client.shouldReload";
    pub const CLIENT_USER_BOOT: &str = "client.userBoot";
    pub const COMMANDS_LIST: &str = "commands.list";
    pub const CONVERSATIONS_ACCEPT_SHARED_INVITE: &str = "conversations.acceptSharedInvite";
    pub const CONVERSATIONS_ADD_TAB: &str = "conversations.addTab";
    pub const CONVERSATIONS_ARCHIVE: &str = "conversations.archive";
    pub const CONVERSATIONS_BULK_JOIN: &str = "conversations.bulkJoin";
    pub const CONVERSATIONS_BULK_REACJI_TRIGGERS: &str = "conversations.bulkReacjiTriggers";
    pub const CONVERSATIONS_CANVASES_CREATE: &str = "conversations.canvases.create";
    pub const CONVERSATIONS_CLOSE: &str = "conversations.close";
    pub const CONVERSATIONS_CONVERT_TO_PRIVATE: &str = "conversations.convertToPrivate";
    pub const CONVERSATIONS_CREATE: &str = "conversations.create";
    pub const CONVERSATIONS_GENERIC_INFO: &str = "conversations.genericInfo";
    pub const CONVERSATIONS_HISTORY: &str = "conversations.history";
    pub const CONVERSATIONS_HISTORY_CHANGES: &str = "conversations.historyChanges";
    pub const CONVERSATIONS_INFO: &str = "conversations.info";
    pub const CONVERSATIONS_INVITE: &str = "conversations.invite";
    pub const CONVERSATIONS_INVITE_SHARED: &str = "conversations.inviteShared";
    pub const CONVERSATIONS_JOIN: &str = "conversations.join";
    pub const CONVERSATIONS_JOIN_REQUESTS_LIST: &str = "conversations.joinRequests.list";
    pub const CONVERSATIONS_KICK: &str = "conversations.kick";
    pub const CONVERSATIONS_LEAVE: &str = "conversations.leave";
    pub const CONVERSATIONS_LIST: &str = "conversations.list";
    pub const CONVERSATIONS_LIST_CONNECT_INVITES: &str = "conversations.listConnectInvites";
    pub const CONVERSATIONS_LIST_PREFS: &str = "conversations.listPrefs";
    pub const CONVERSATIONS_MARK: &str = "conversations.mark";
    pub const CONVERSATIONS_MEMBERS: &str = "conversations.members";
    pub const CONVERSATIONS_MOVE_TAB: &str = "conversations.moveTab";
    pub const CONVERSATIONS_OPEN: &str = "conversations.open";
    pub const CONVERSATIONS_REMOVE_TAB: &str = "conversations.removeTab";
    pub const CONVERSATIONS_RENAME: &str = "conversations.rename";
    pub const CONVERSATIONS_REPLIES: &str = "conversations.replies";
    pub const CONVERSATIONS_SET_TEAMS: &str = "conversations.setTeams";
    pub const CONVERSATIONS_SET_PURPOSE: &str = "conversations.setPurpose";
    pub const CONVERSATIONS_SET_TOPIC: &str = "conversations.setTopic";
    pub const CONVERSATIONS_TEAM_CONNECTIONS: &str = "conversations.teamConnections";
    pub const CONVERSATIONS_UNARCHIVE: &str = "conversations.unarchive";
    pub const CONVERSATIONS_UPDATE_TAB: &str = "conversations.updateTab";
    pub const CONVERSATIONS_VALIDATE_NAME: &str = "conversations.validateName";
    pub const CONVERSATIONS_VIEW: &str = "conversations.view";
    pub const DIALOG_GET: &str = "dialog.get";
    pub const DIALOG_NOTIFY_CANCEL: &str = "dialog.notifyCancel";
    pub const DIALOG_SELECT_SUGGESTION: &str = "dialog.selectSuggestion";
    pub const DIALOG_SUBMIT: &str = "dialog.submit";
    pub const DND_END_DND: &str = "dnd.endDnd";
    pub const DND_END_SNOOZE: &str = "dnd.endSnooze";
    pub const DND_INFO: &str = "dnd.info";
    pub const DND_SET_SNOOZE: &str = "dnd.setSnooze";
    pub const DND_TEAM_INFO: &str = "dnd.teamInfo";
    pub const DRAFTS_CREATE: &str = "drafts.create";
    pub const DRAFTS_DELETE: &str = "drafts.delete";
    pub const DRAFTS_LIST: &str = "drafts.list";
    pub const DRAFTS_LIST_ACTIVE: &str = "drafts.listActive";
    pub const DRAFTS_UPDATE: &str = "drafts.update";
    pub const EMOJI_ADMIN_LIST: &str = "emoji.adminList";
    pub const EMOJI_LIST: &str = "emoji.list";
    pub const FILES_CLOSE: &str = "files.close";
    pub const FILES_COLLECTIONS_CREATE: &str = "files.collections.create";
    pub const FILES_COLLECTIONS_DELETE: &str = "files.collections.delete";
    pub const FILES_COLLECTIONS_LIST: &str = "files.collections.list";
    pub const FILES_COLLECTIONS_UPDATE: &str = "files.collections.update";
    pub const FILES_COMPLETE_UPLOAD: &str = "files.completeUpload";
    pub const FILES_COMPLETE_UPLOAD_EXTERNAL: &str = "files.completeUploadExternal";
    pub const FILES_CREATE_CANVAS: &str = "files.createCanvas";
    pub const FILES_DELETE: &str = "files.delete";
    pub const FILES_EDIT: &str = "files.edit";
    pub const FILES_FAVORITES_ADD: &str = "files.favorites.add";
    pub const FILES_FAVORITES_LIST: &str = "files.favorites.list";
    pub const FILES_FAVORITES_REMOVE: &str = "files.favorites.remove";
    pub const FILES_GET_OVERVIEW: &str = "files.getOverview";
    pub const FILES_GET_UPLOAD_U_R_L: &str = "files.getUploadURL";
    pub const FILES_GET_UPLOAD_U_R_L_EXTERNAL: &str = "files.getUploadURLExternal";
    pub const FILES_HAS_SHARES: &str = "files.hasShares";
    pub const FILES_INFO: &str = "files.info";
    pub const FILES_LIST: &str = "files.list";
    pub const FILES_OPEN: &str = "files.open";
    pub const FILES_PING: &str = "files.ping";
    pub const FILES_RECENTLY_DELETED: &str = "files.recentlyDeleted";
    pub const FILES_REQUEST_PERMISSION: &str = "files.requestPermission";
    pub const FILES_REVOKE_PUBLIC_U_R_L: &str = "files.revokePublicURL";
    pub const FILES_SHARE: &str = "files.share";
    pub const FILES_SHARED_PUBLIC_U_R_L: &str = "files.sharedPublicURL";
    pub const FILES_UNDELETE: &str = "files.undelete";
    pub const FILES_UPLOAD: &str = "files.upload";
    pub const FUNCTIONS_WORKFLOWS_GET: &str = "functions.workflows.get";
    pub const FUNCTIONS_WORKFLOWS_LIST: &str = "functions.workflows.list";
    pub const GIF_FEATURED: &str = "gif.featured";
    pub const GIF_SEARCH: &str = "gif.search";
    pub const HUDDLES_INFO: &str = "huddles.info";
    pub const IM_LIST: &str = "im.list";
    pub const LISTS_ADD_TO_LIST: &str = "lists.addToList";
    pub const LISTS_CELLS_UPDATE: &str = "lists.cells.update";
    pub const LISTS_CREATE: &str = "lists.create";
    pub const LISTS_GET_THREADS_FOR_RECORD: &str = "lists.getThreadsForRecord";
    pub const LISTS_OPEN: &str = "lists.open";
    pub const LISTS_RECORDS_ARCHIVE: &str = "lists.records.archive";
    pub const LISTS_RECORDS_CREATE: &str = "lists.records.create";
    pub const LISTS_RECORDS_DELETE: &str = "lists.records.delete";
    pub const LISTS_RECORDS_INFO: &str = "lists.records.info";
    pub const LISTS_RECORDS_LIST: &str = "lists.records.list";
    pub const LISTS_RECORDS_OPEN: &str = "lists.records.open";
    pub const LISTS_TEMPLATES: &str = "lists.templates";
    pub const MEDIA_PROGRESS_UPDATE: &str = "media.progress.update";
    pub const MEDIA_REACTIONS_ADD: &str = "media.reactions.add";
    pub const MEDIA_REACTIONS_GET: &str = "media.reactions.get";
    pub const MESSAGES_LIST: &str = "messages.list";
    pub const PINS_ADD: &str = "pins.add";
    pub const PINS_LIST: &str = "pins.list";
    pub const PINS_REMOVE: &str = "pins.remove";
    pub const PRESENCE_SET: &str = "presence.set";
    pub const QUIP_GET_CANVAS_BULK_SECTION_IDS_DATA: &str = "quip.getCanvasBulkSectionIdsData";
    pub const QUIP_HISTORY_GET_VERSIONS: &str = "quip.history.getVersions";
    pub const QUIP_HISTORY_RESTORE_VERSION: &str = "quip.history.restoreVersion";
    pub const QUIP_LOOKUP_FILE_ID: &str = "quip.lookupFileId";
    pub const QUIP_LOOKUP_THREAD_IDS: &str = "quip.lookupThreadIds";
    pub const QUIP_THREAD_ARCHIVE: &str = "quip.thread.archive";
    pub const QUIP_THREAD_ARCHIVE_ALL: &str = "quip.thread.archiveAll";
    pub const REACTIONS_ADD: &str = "reactions.add";
    pub const REACTIONS_GET: &str = "reactions.get";
    pub const REACTIONS_LIST: &str = "reactions.list";
    pub const REACTIONS_REMOVE: &str = "reactions.remove";
    pub const REMINDERS_ADD: &str = "reminders.add";
    pub const REMINDERS_COMPLETE: &str = "reminders.complete";
    pub const REMINDERS_DELETE: &str = "reminders.delete";
    pub const REMINDERS_INFO: &str = "reminders.info";
    pub const REMINDERS_LIST: &str = "reminders.list";
    pub const ROOMS_CAN_JOIN_HUDDLE: &str = "rooms.canJoinHuddle";
    pub const ROOMS_GET_LINK: &str = "rooms.getLink";
    pub const ROOMS_JOIN: &str = "rooms.join";
    pub const ROOMS_LEAVE: &str = "rooms.leave";
    pub const ROOMS_NOTIFY_MEMBER: &str = "rooms.notifyMember";
    pub const RTM_CONNECT: &str = "rtm.connect";
    pub const RTM_START: &str = "rtm.start";
    pub const SAVED_ADD: &str = "saved.add";
    pub const SAVED_CLEAR_COMPLETED: &str = "saved.clearCompleted";
    pub const SAVED_DELETE: &str = "saved.delete";
    pub const SAVED_LIST: &str = "saved.list";
    pub const SAVED_UPDATE: &str = "saved.update";
    pub const SCREENHERO_ROOMS: &str = "screenhero.rooms";
    pub const SCREENHERO_ROOMS_CREATE: &str = "screenhero.rooms.create";
    pub const SCREENHERO_ROOMS_INFO: &str = "screenhero.rooms.info";
    pub const SEARCH_ALL: &str = "search.all";
    pub const SEARCH_AUTOCOMPLETE: &str = "search.autocomplete";
    pub const SEARCH_AUTOCOMPLETE_FILES: &str = "search.autocomplete.files";
    pub const SEARCH_AUTOCOMPLETE_MODEL: &str = "search.autocomplete.model";
    pub const SEARCH_AUTOCOMPLETE_TOP_EMOJIS: &str = "search.autocomplete.topEmojis";
    pub const SEARCH_DELETE: &str = "search.delete";
    pub const SEARCH_FEEDBACK: &str = "search.feedback";
    pub const SEARCH_FILES: &str = "search.files";
    pub const SEARCH_INLINE: &str = "search.inline";
    pub const SEARCH_MESSAGES: &str = "search.messages";
    pub const SEARCH_MODULES_CHANNELS: &str = "search.modules.channels";
    pub const SEARCH_MODULES_DMS: &str = "search.modules.dms";
    pub const SEARCH_MODULES_FILES: &str = "search.modules.files";
    pub const SEARCH_MODULES_MESSAGES: &str = "search.modules.messages";
    pub const SEARCH_MODULES_PEOPLE: &str = "search.modules.people";
    pub const SEARCH_PRECACHE: &str = "search.precache";
    pub const SEARCH_SAVE: &str = "search.save";
    pub const SIGNIN_CONFIRM_CODE: &str = "signin.confirmCode";
    pub const SIGNIN_FIND_WORKSPACES: &str = "signin.findWorkspaces";
    pub const SIGNUP_CHECK_EMAIL: &str = "signup.checkEmail";
    pub const SIGNUP_CONFIRM_EMAIL: &str = "signup.confirmEmail";
    pub const SLACK_LISTS_ACCESS_DELETE: &str = "slackLists.access.delete";
    pub const SLACK_LISTS_ACCESS_SET: &str = "slackLists.access.set";
    pub const SLACK_LISTS_CREATE: &str = "slackLists.create";
    pub const SLACK_LISTS_DOWNLOAD_GET: &str = "slackLists.download.get";
    pub const SLACK_LISTS_DOWNLOAD_START: &str = "slackLists.download.start";
    pub const SLACK_LISTS_ITEMS_CREATE: &str = "slackLists.items.create";
    pub const SLACK_LISTS_ITEMS_DELETE: &str = "slackLists.items.delete";
    pub const SLACK_LISTS_ITEMS_DELETE_MULTIPLE: &str = "slackLists.items.deleteMultiple";
    pub const SLACK_LISTS_ITEMS_INFO: &str = "slackLists.items.info";
    pub const SLACK_LISTS_ITEMS_LIST: &str = "slackLists.items.list";
    pub const SLACK_LISTS_ITEMS_UPDATE: &str = "slackLists.items.update";
    pub const SLACK_LISTS_UPDATE: &str = "slackLists.update";
    pub const STARS_ADD: &str = "stars.add";
    pub const STARS_LIST: &str = "stars.list";
    pub const STARS_REMOVE: &str = "stars.remove";
    pub const SUBSCRIPTIONS_THREAD_ADD: &str = "subscriptions.thread.add";
    pub const SUBSCRIPTIONS_THREAD_CLEAR_ALL: &str = "subscriptions.thread.clearAll";
    pub const SUBSCRIPTIONS_THREAD_GET: &str = "subscriptions.thread.get";
    pub const SUBSCRIPTIONS_THREAD_GET_VIEW: &str = "subscriptions.thread.getView";
    pub const SUBSCRIPTIONS_THREAD_MARK: &str = "subscriptions.thread.mark";
    pub const SUBSCRIPTIONS_THREAD_REMOVE: &str = "subscriptions.thread.remove";
    pub const TEAM_INFO: &str = "team.info";
    pub const TEAM_PROFILE_GET: &str = "team.profile.get";
    pub const USERGROUPS_LIST: &str = "usergroups.list";
    pub const USERGROUPS_USERS_LIST: &str = "usergroups.users.list";
    pub const USERS_CHANNEL_SECTIONS_CHANNELS_BULK_UPDATE: &str =
        "users.channelSections.channels.bulkUpdate";
    pub const USERS_CHANNEL_SECTIONS_CHANNELS_REMOVE: &str =
        "users.channelSections.channels.remove";
    pub const USERS_CHANNEL_SECTIONS_CREATE: &str = "users.channelSections.create";
    pub const USERS_CHANNEL_SECTIONS_CREATE_LINK: &str = "users.channelSections.createLink";
    pub const USERS_CHANNEL_SECTIONS_DELETE: &str = "users.channelSections.delete";
    pub const USERS_CHANNEL_SECTIONS_LIST: &str = "users.channelSections.list";
    pub const USERS_CHANNEL_SECTIONS_SET: &str = "users.channelSections.set";
    pub const USERS_CONVERSATIONS: &str = "users.conversations";
    pub const USERS_GET_PRESENCE: &str = "users.getPresence";
    pub const USERS_INFO: &str = "users.info";
    pub const USERS_LIST: &str = "users.list";
    pub const USERS_MARK_ALL_READ: &str = "users.markAllRead";
    pub const USERS_PREFS_GET: &str = "users.prefs.get";
    pub const USERS_PREFS_SET: &str = "users.prefs.set";
    pub const USERS_PREFS_SET_NOTIFICATIONS: &str = "users.prefs.setNotifications";
    pub const USERS_PRIORITY_LIST: &str = "users.priority.list";
    pub const USERS_PROFILE_GET: &str = "users.profile.get";
    pub const USERS_PROFILE_SET: &str = "users.profile.set";
    pub const USERS_SET_PHOTO: &str = "users.setPhoto";
    pub const USERS_SET_PRESENCE: &str = "users.setPresence";
    pub const VIEWS_CLOSE: &str = "views.close";
    pub const VIEWS_GET: &str = "views.get";
    pub const VIEWS_SUBMIT: &str = "views.submit";
    pub const WORKFLOWS_TRIGGERS_INFO: &str = "workflows.triggers.info";
    pub const WORKFLOWS_TRIGGERS_LIST: &str = "workflows.triggers.list";
    pub const WORKFLOWS_TRIGGERS_LIST_RECENTLY_RUN_FOR_USER: &str =
        "workflows.triggers.listRecentlyRunForUser";
    pub const WORKFLOWS_TRIGGERS_PREVIEW: &str = "workflows.triggers.preview";
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
    }
}
