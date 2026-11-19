# Slack Desktop — Core Messaging Feature Inventory

Parity checklist for "slackless" against the official Slack desktop app (late 2026). One line per user-facing feature in the core-messaging group; sub-bullets are variants or notable details. OBSERVED URLs point at the official doc (Help Center, features page, release notes, product blogs) where the behavior is documented.

## Workspaces & sign-in

- [ ] F-CORE-001 Sign in to multiple workspaces at once - Clicking the workspace icon lists every signed-in workspace; opening one loads it in place - OBSERVED: https://slack.com/help/articles/1500002200741-Switch-between-workspaces
- [ ] F-CORE-002 Workspace switcher show/hide - View-options icon (or Cmd+Shift+S) expands or collapses the workspace switcher rail; collapsed mode shows a single workspace icon at top of the nav bar - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-003 Reorder signed-in workspaces - Workspace icons can be dragged into a custom order in the switcher - OBSERVED: https://slack.com/help/articles/1500002200741-Switch-between-workspaces
- [ ] F-CORE-004 Cycle workspaces with keyboard - Ctrl+Tab / Ctrl+Shift+Tab switches to next/previous workspace - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-005 Jump to workspace by number - Cmd+[1-9] opens the workspace whose icon shows that assigned number - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-006 Unread badge on workspace icons - Numbered badge appears on other workspaces' icons when they contain unread activity - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-007 All Workspaces org view (Enterprise Grid) - On Enterprise plans, selecting All Workspaces in the sidebar filters conversations across the org's workspaces - OBSERVED: https://slack.com/help/articles/1500002200741-Switch-between-workspaces
- [ ] F-CORE-008 Enterprise org name and workspace admin entry point - Organization name sits in the top-left corner of the desktop app and opens org-level administration for Grid workspaces - OBSERVED: https://slack.com/help/articles/115005225987-Manage-a-workspace-in-an-Enterprise-organization
- [ ] F-CORE-009 Sign out of a workspace - Signing out of one workspace removes it from the client without touching other signed-in workspaces - OBSERVED: https://slack.com/help/articles/214613347-Sign-out-of-Slack
- [ ] F-CORE-010 Multiple app windows - Conversations and threads can be popped into separate windows; Cmd+W closes a window, Cmd+Shift+W reopens the last closed one - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-011 Preferences window - Cmd+, opens the preferences dialog with sidebar/notification/language sections - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts

## Sidebar & navigation

- [ ] F-CORE-012 Navigation bar tabs - Sidebar top holds tabs (Home, DMs, Activity, To-dos, Files, More) that switch the main view - OBSERVED: https://slack.com/help/articles/360059928654-How-to-use-Slack-your-quick-start-guide
- [ ] F-CORE-013 Configure which tabs appear in the nav bar - Preferences > Navigation lets you check/uncheck tabs; unchecked items move under the More menu - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-014 Reorder nav bar tabs - Tabs can be dragged (or moved with arrows) into a custom order - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-015 Sidebar conversation sections - Home tab lists channels/DMs/apps grouped into default sections (e.g. Starred, Channels, Direct messages) - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
  - Default sections can be dragged to reorder
- [ ] F-CORE-016 Collapse and expand sections - Clicking a section header collapses/expands it; Option+Click expands or collapses all sections at once - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-017 Custom sidebar sections - Create named, collapsible sections and drag channels/DMs/apps into them (paid plans) - OBSERVED: https://slack.com/help/articles/360043207674-Organize-your-sidebar-with-custom-sections
  - Rename, delete, or move conversations between sections
- [ ] F-CORE-018 Share sidebar sections - Shared sections push a set of channels into other members' sidebars in one step; recipients can edit their copy - OBSERVED: https://slack.com/help/articles/29873996048019-Share-sidebar-sections-in-Slack
- [ ] F-CORE-019 Star conversations - Starring a channel or DM pins it to the Starred section via the star icon in the conversation header - OBSERVED: https://slack.com/help/articles/360058495594-Focus-on-what-matters-to-you
- [ ] F-CORE-020 Sidebar sort order - Conversations sort alphabetically by default with an option to sort by recent activity (and sections keep their order) - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-021 Active-only filter - Manage my sidebar > Filter hides conversations with no activity in the past 30 days by default - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-022 Hide/show muted conversations - Muted channels and DMs can be hidden from or shown in the sidebar - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-023 Bulk leave inactive channels - Clean up your channel list shows channels not viewed within a chosen timeframe for bulk leave (or mute) - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-024 Sidebar width and density - Dragging the sidebar edge resizes it; right-click toggles icons-only vs icons-and-text display - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-025 Quick switcher - Cmd+K opens a search field that jumps to any channel, DM, or person by name as you type - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
  - Entering a date or date range in the quick switcher scrolls the current conversation to that date
  - OBSERVED: https://stackoverflow.com/questions/35122312/how-do-i-jump-to-a-specific-date-in-history-or-the-oldest-message-in-a-slack-cha
- [ ] F-CORE-026 Open conversation in a new window - Focusing a sidebar item and pressing Cmd+Enter pops that channel or DM into its own window - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-CORE-027 History back/forward - Cmd+[ and Cmd+] navigate backward and forward through visited conversations - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-028 Unreads (All unreads) view - One scrollable list of every unread message, opened from Unreads at the top of the sidebar - OBSERVED: https://slack.com/help/articles/226410907-View-all-your-unread-messages
  - Sort menu: like sidebar, alphabetical, recommended, newest activity, oldest activity
  - Filter menu narrows unreads by sidebar section (custom sections on paid plans)
- [ ] F-CORE-029 Mark unreads read from the view - Mark as Read clears a conversation's group (with Undo), scrolling to the bottom offers Mark All Messages Read - OBSERVED: https://slack.com/help/articles/226410907-View-all-your-unread-messages
  - Esc marks the focused group of messages read; arrows expand/collapse groups
- [ ] F-CORE-030 Unreads view new-message refresh - A syncing-arrows icon appears in the header when new unreads arrive and clicking it reveals the fresh batch - OBSERVED: https://slack.com/help/articles/226410907-View-all-your-unread-messages
- [ ] F-CORE-031 Reply or jump from unreads - Clicking a message's timestamp jumps into the conversation; message actions (save, share, react, pin, remind, thread) work from previews - OBSERVED: https://slack.com/help/articles/226410907-View-all-your-unread-messages
- [ ] F-CORE-032 Jump to most recent unread in a conversation - Cmd+J jumps to the newest unread message in the open conversation (desktop-only) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-033 Jump between unread conversations - Option+Shift+Up/Down cycles previous/next unread channel or DM - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-034 Step through sidebar conversations - Option+Up/Down moves to previous/next channel or DM in the sidebar - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-035 Home view shortcut - Ctrl+1 (Mac) opens the Home conversation list - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-036 Browse DMs - Cmd+Shift+K opens a browsable/searchable list of all direct messages (desktop-only) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-037 Browse channels - Cmd+Shift+L opens the channel directory; Home > Directories > Channels tab lists/searches every public channel - OBSERVED: https://slack.com/help/articles/205239967-Join-a-channel
- [ ] F-CORE-038 Browse people and user groups - People view (Cmd+Shift+E) and the People directory list members and user groups - OBSERVED: https://slack.com/help/articles/360003534892-Browse-people-and-user-groups-in-Slack
- [ ] F-CORE-039 Threads view shortcut - Cmd+Shift+T opens the Threads view (desktop-only) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-040 Activity view shortcut - Cmd+Shift+M opens the Activity feed (desktop-only) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-041 More view shortcut - Ctrl+0 opens the More view holding tools not pinned to the nav bar - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-042 Nav tab switching shortcuts - Ctrl+Page up/down moves between tabs; Ctrl+[number] opens the tab with that assigned number - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-043 Keyboard focus navigation - F6/Shift+F6 move focus between sections, arrows move within lists, Home/End jump to oldest/newest message - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-CORE-044 Mark all read - Shift+Esc marks every message in every conversation read; Esc marks the current conversation read - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts

## Channels

- [ ] F-CORE-045 Create a channel - New channels get a name and a public/private choice before creation; invitees can be added at creation time - OBSERVED: https://slack.com/help/articles/201402297-Create-a-channel
- [ ] F-CORE-046 Channel directory browsing - Directories > Channels tab lists all public channels searchable by name or description, with previews before joining - OBSERVED: https://slack.com/help/articles/205239967-Join-a-channel
- [ ] F-CORE-047 Join a public channel - Join Channel button adds you to the channel immediately - OBSERVED: https://slack.com/help/articles/205239967-Join-a-channel
  - Temporary join: arrow on the button offers Just for today / For 48 hours / For 1 week, listed under Temporary channels and auto-removed after the period
- [ ] F-CORE-048 Private channels - Private channels are invite-only; they show a private indicator instead of the public one - OBSERVED: https://slack.com/help/articles/213185467-Convert-a-channel-to-private-or-public
- [ ] F-CORE-049 Convert channel public <-> private - Channel settings offer conversion in both directions (permission-gated) - OBSERVED: https://slack.com/help/articles/213185467-Convert-a-channel-to-private-or-public
- [ ] F-CORE-050 Leave a channel - Leaving removes the channel from your sidebar while the channel itself keeps existing - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-CORE-051 Archive a channel - Archiving for everyone freezes the channel read-only and drops it from the active channel list - OBSERVED: https://slack.com/help/articles/213185307-Archive-or-delete-a-channel
- [ ] F-CORE-052 Unarchive a channel - An archived channel can be restored; its name is reserved until renamed or unarchived - OBSERVED: https://slack.com/help/articles/213185307-Archive-or-delete-a-channel
- [ ] F-CORE-053 Delete a channel - Admins can permanently delete a channel and its contents - OBSERVED: https://slack.com/help/articles/213185307-Archive-or-delete-a-channel
- [ ] F-CORE-054 Rename a channel - Channel name is editable from the header/Edit menu; duplicates of archived names are rejected - OBSERVED: https://slack.com/help/articles/201654073-Rename-a-channel
- [ ] F-CORE-055 Set a topic - The topic field under the channel name is editable and displays in the header for all members - OBSERVED: https://slack.com/help/articles/201654083-Set-a-conversation-topic-or-channel-description
- [ ] F-CORE-056 Set a description - The About description explains the channel's purpose and shows in the details pane and directory listings - OBSERVED: https://slack.com/help/articles/201654083-Set-a-conversation-topic-or-channel-description
- [ ] F-CORE-057 Conversation details pane - Cmd+Shift+I toggles a right-hand pane with About (topic/description/pins/files), Members, and settings tabs - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
  - Member count and member list are browsable from the details pane; members can be added there
- [ ] F-CORE-058 Per-channel notification preferences - Each conversation offers All new messages / Mentions only / Nothing / custom options - OBSERVED: https://slack.com/help/articles/360056534254-Manage-notifications-for-specific-channels-and-direct-messages
  - Notification menu in the header offers quick presets plus More notification options
- [ ] F-CORE-059 Mute a channel - Muting silences all notifications for that conversation while keeping it in the sidebar - OBSERVED: https://slack.com/help/articles/360056534254-Manage-notifications-for-specific-channels-and-direct-messages
- [ ] F-CORE-060 Pin and unpin messages - Pinning adds the message to the channel/DM Pins tab; un-pinning removes it - OBSERVED: https://slack.com/help/articles/32562841868307-Add-and-manage-tabs-in-channels-and-direct-messages
- [ ] F-CORE-061 Conversation tabs - Every conversation starts with a Messages tab; canvas, list, workflow, message, and link tabs appear up to a limit of 15 per conversation - OBSERVED: https://slack.com/help/articles/32562841868307-Add-and-manage-tabs-in-channels-and-direct-messages
  - Tabs reorder by drag and drop; Edit tabs menu hides (eye icon) or deletes (trash icon) each tab
- [ ] F-CORE-062 Folder tabs - A folder tab organizes up to 100 links and files per channel; members add/rename/delete folders and share links/files into them - OBSERVED: https://slack.com/help/articles/32562841868307-Add-and-manage-tabs-in-channels-and-direct-messages
- [ ] F-CORE-063 Tab editing permissions - Channel settings define who can add, remove, and reorder tabs (everyone vs Channel Managers) - OBSERVED: https://slack.com/help/articles/32562841868307-Add-and-manage-tabs-in-channels-and-direct-messages
- [ ] F-CORE-064 Channel posting/management permissions - Channel settings and workspace policies govern who can post and manage the channel - OBSERVED: https://slack.com/help/articles/115004988303-Adjust-channel-management-permissions
- [ ] F-CORE-065 External-organization visibility - Hovering a channel in the sidebar explains which external orgs or internal workspaces can access it - OBSERVED: https://slack.com/blog/news/simpler-streamlined-sidebar

## DMs and group DMs

- [ ] F-CORE-066 Start a direct message - The new-message (plus/compose) button in the sidebar opens a DM to any workspace member by name - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages
- [ ] F-CORE-067 DMs tab - The DMs tab lists your conversations with an inline search field and a Direct messages filter; message yourself sits at the top - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages
- [ ] F-CORE-068 Group DMs up to nine people - Starting a DM with multiple people creates a group conversation capped at nine members - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages
- [ ] F-CORE-069 Name a group DM - Hovering the member names in the header and clicking the pencil sets a custom name for the group DM - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages
- [ ] F-CORE-070 Add people to a DM - Members can be added to an existing DM to loop them into the conversation - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages
- [ ] F-CORE-071 Message yourself - A self-DM entry at the top of the DMs tab opens a private conversation with yourself (drafts, notes, files) - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages
- [ ] F-CORE-072 Convert group DM to private channel - Settings tab of a group DM offers Change to a private channel, preserving history - OBSERVED: https://slack.com/help/articles/217555437-Convert-a-group-direct-message-to-a-private-channel
- [ ] F-CORE-073 Slack Connect DMs - DMs can bridge to people at other companies when Slack Connect is enabled, with the external-org indicator - OBSERVED: https://slack.com/help/articles/212281468-Understand-direct-messages

## Messaging & composer

- [ ] F-CORE-074 Enter sends, Shift+Enter breaks the line - Messages send on Enter; Shift+Enter inserts a newline, with a preference to swap Enter to newline - OBSERVED: https://slack.com/help/articles/202288908-Format-your-messages-in-Slack
- [ ] F-CORE-075 Formatting toolbar - The composer toolbar shows live formatting (WYSIWYG before send) and is visible by default - OBSERVED: https://slack.com/help/articles/202288908-Format-your-messages-in-Slack
  - Formatting icon in the message field hides or shows the toolbar
- [ ] F-CORE-076 Text styles: bold, italic, underline, strikethrough - Selecting text and clicking (or highlighting then choosing) applies any combination of the four styles - OBSERVED: https://slack.com/help/articles/202288908-Format-your-messages-in-Slack
- [ ] F-CORE-077 Inline code and code block - Toolbar applies inline `code` and fenced code blocks that preserve monospace layout - OBSERVED: https://slack.com/help/articles/202288908-Format-your-messages-in-Slack
- [ ] F-CORE-078 Block quote and lists via toolbar - Toolbar buttons create block quotes, ordered lists, and bulleted lists - OBSERVED: https://slack.com/help/articles/202288908-Format-your-messages-in-Slack
- [ ] F-CORE-079 Markdown-style autoformat - Typing `1.` + space starts an ordered list; `*` + space starts a bulleted list - OBSERVED: https://slack.com/help/articles/202288908-Format-your-messages-in-Slack
- [ ] F-CORE-080 Markup mode - A preference hides the toolbar; markup like _bold_, _italic_, ~strike~, `code`, > quote, and ``` code blocks formats on send - OBSERVED: https://slack.com/help/articles/360039953113-Format-your-messages-in-Slack-with-markup
- [ ] F-CORE-081 Code snippets - Cmd+Shift+Enter opens the snippet editor with language/syntax highlighting; snippets post as expandable code files - OBSERVED: https://slack.com/help/articles/204145658-Create-or-paste-code-snippets-in-Slack
- [ ] F-CORE-082 User mentions - Typing @ surfaces a member autocomplete; mentioned users get highlighted and notified - OBSERVED: https://slack.com/help/articles/202009646-Notify-a-channel-or-workspace
- [ ] F-CORE-083 @channel, @here, @everyone - @channel notifies all members, @here only active members, @everyone notifies everyone via #general; large-channel sends ask for confirmation - OBSERVED: https://slack.com/help/articles/202009646-Notify-a-channel-or-workspace
- [ ] F-CORE-084 User group mentions - @user-group expands to notify all its members; groups are browsable via the People directory - OBSERVED: https://slack.com/help/articles/212906697-Create-and-edit-user-groups
- [ ] F-CORE-085 Channel mentions - Typing # in the composer autocompletes channel names, linking the message to that channel - OBSERVED: https://slack.com/help/articles/202009646-Notify-a-channel-or-workspace
- [ ] F-CORE-086 Emoji in messages - The smiley icon opens the emoji menu with category browsing; emoji codes like :smile: expand after sending - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-087 Attach files from the message field - The paperclip/plus menu attaches files; previews render inline as part of the message - OBSERVED: https://slack.com/help/articles/201330736-Add-files-to-Slack
  - Upload shortcut Cmd+O attaches a file from anywhere in Slack (desktop-only)
  - Dragging files onto the Slack window shares them into the focused conversation
  - Copying an image/file and pasting into the message field uploads it
- [ ] F-CORE-088 Schedule send - The arrow next to the send button opens time options (later today, tomorrow, custom date/time) and the message posts itself at the chosen time - OBSERVED: https://slack.com/help/articles/201457107-Send-and-read-messages
- [ ] F-CORE-089 Drafts auto-save - Unsent composer text is kept per-conversation as a draft; Drafts & sent at the top of the sidebar lists them - OBSERVED: https://slack.com/help/articles/201457107-Send-and-read-messages
- [ ] F-CORE-090 Drafts & sent tabs - Drafts, Scheduled, and Sent tabs separate the three lists; hovering drafts/scheduled shows options, clicking sent jumps to the conversation - OBSERVED: https://slack.com/help/articles/201457107-Send-and-read-messages
  - Bulk draft deletion via Edit mode checkbox selection
  - Scheduled messages can be edited, rescheduled, sent immediately, cancelled, or deleted
- [ ] F-CORE-091 Edit a sent message - More actions > Edit message opens the composer on the message; Save Changes applies and shows an (edited) marker - OBSERVED: https://slack.com/help/articles/202395258-Edit-or-delete-messages
- [ ] F-CORE-092 Delete a sent message - More actions > Delete message removes it after confirmation - OBSERVED: https://slack.com/help/articles/202395258-Edit-or-delete-messages
- [ ] F-CORE-093 Unsend last message - Cmd+Z immediately unsends your most recent message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-094 Message actions menu - Right-click (or hover three dots) opens the common actions menu: edit, delete, copy link, copy text, pin, save, remind, mark unread, forward, reply in thread - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
  - One-key shortcuts on a focused message: E edit, Delete delete, T/Right thread, F forward, P pin, A save
- [ ] F-CORE-095 Copy message link - Copy link puts a permalink to that exact message on the clipboard - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-096 Mark message unread - Option/Alt+Click (or More actions > Mark unread) marks the message and everything above it unread, restoring the new-messages divider - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-097 Forward/share a message - Forward Message picks any conversation, adds an optional note, and re-posts the message content there - OBSERVED: https://slack.com/help/articles/203274767-Forward-messages-in-Slack
- [ ] F-CORE-098 Link unfurl previews - Shared URLs expand into rich inline previews (title, summary, thumbnail) in the message - OBSERVED: https://slack.com/help/articles/204399343-Share-links-and-set-preview-preferences
  - Clicking the x on a preview removes it from your view
  - OBSERVED: https://slack.com/help/articles/360001502048-Manage-link-previews
  - Preferences offer text-only previews or blocking previews per domain
- [ ] F-CORE-099 Typing indicator - Three animated dots show who is currently typing in the conversation; a preference can turn the indicator off - OBSERVED: https://x.com/SlackHQ/status/1810751810983923896
- [ ] F-CORE-100 Message grouping - Consecutive messages from the same author within a short window group under one name/avatar header - OBSERVED: https://slack.com/help/articles/201457107-Send-and-read-messages
- [ ] F-CORE-101 Date dividers - Full-width date stamps separate message groups from different days - OBSERVED: https://slack.com/help/articles/201457107-Send-and-read-messages
- [ ] F-CORE-102 New-messages divider - Opening a conversation with unreads shows a "New" red divider line at the first unread message (Cmd+J targets it) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-103 Message timestamps - Every message shows a timestamp; clicking it yields the permalink for deep linking - OBSERVED: https://slack.com/help/articles/226410907-View-all-your-unread-messages

## Threads

- [ ] F-CORE-104 Reply in thread - The Reply in thread hover icon opens the thread reply composer without cluttering the main view - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-105 Also send to channel - A checkbox below the thread reply mirrors it to the channel/DM main view - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-106 Send reply to channel after the fact - Thread three dots > Send reply to this channel re-shares an already-sent reply to the main view - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-107 Threads view - Home > Threads lists followed threads with unread replies sorted to top; clicking opens the thread in context - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
  - Hovering a thread row offers react, open in conversation, save for later, and more options
- [ ] F-CORE-108 Unfollow a thread - Unfollowing from the Threads view or thread menu stops new-reply notifications - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-109 Open thread in a window - The window icon in a thread pops it into a separate desktop window alongside other conversations - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-110 Per-thread notifications - Thread menu offers Get notified about new replies or Turn off notifications for replies - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-111 Channel-wide thread following - Channel notification options include Get notified about all replies and show them in your Threads view - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-112 Default thread follow rules - You follow threads you start, reply to, or are mentioned in; all 1:1 DM threads notify by default - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions
- [ ] F-CORE-113 Reply-count indicator - The parent message shows thread reply count and participant faces; replying happens inline in the message list or via the pane - OBSERVED: https://slack.com/help/articles/115000769927-Use-threads-to-organize-discussions

## Reactions & emoji

- [ ] F-CORE-114 Add emoji reaction - Hovering a message and clicking the Add reaction icon opens the picker to react - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-115 Frequently used reactions - The reaction hover/picker surfaces your frequently used emoji first - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-116 One-click suggested reactions - An optional preference shows suggested emoji on hover for instant reactions - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-117 Toggle own reaction - Clicking an existing reaction chip adds/removes your own; your reactions highlight in blue - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-118 See who reacted - Hovering or long-pressing a reaction chip lists the reactors - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-119 Emoji skin tones - The hand emoji in the picker's bottom-right sets your default skin tone; right-clicking multi-person emoji picks a per-emoji tone - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
  - Preferences > Appearance > Default emoji skin tone persists the choice
  - OBSERVED: https://slack.com/help/articles/4406393601683-Manage-your-emoji-preferences
- [ ] F-CORE-120 Emoji picker categories and search - Category icons browse the set; a search field finds emoji by name/keyword - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions
- [ ] F-CORE-121 Custom emoji - Workspace members add custom emoji and aliases via name, image upload, and optional alias - OBSERVED: https://slack.com/help/articles/206870177-Add-custom-emoji-and-aliases-to-your-workspace
- [ ] F-CORE-122 React to canvas text - Selecting text in a canvas on desktop and clicking Add reaction attaches the emoji to that text span - OBSERVED: https://slack.com/help/articles/202931348-Use-emoji-and-reactions

## Files & media

- [ ] F-CORE-123 File size limit - Files up to 1 GB upload successfully; larger files are rejected - OBSERVED: https://slack.com/features/document-sharing
- [ ] F-CORE-124 Inline image previews - Shared images render as previews in the message (thumbnail-cropped in preview mode) - OBSERVED: https://slack.com/help/articles/201330736-Add-files-to-Slack
- [ ] F-CORE-125 Image viewer - Clicking an image preview opens the larger viewer for close inspection - OBSERVED: https://slack.com/help/articles/201330736-Add-files-to-Slack
- [ ] F-CORE-126 Video and audio playback - Uploaded video and audio files play in Slack's built-in players - OBSERVED: https://slack.com/features/document-sharing
- [ ] F-CORE-127 Record a video clip - The camera icon records up to five minutes with optional screen share, stop, thumbnail choice, then send - OBSERVED: https://slack.com/help/articles/4406235165587-Record-audio-and-video-clips-in-Slack
  - Pre/during-recording options: camera on/off, mic mute, blur or upload background, camera/mic device selection
  - Existing video files can be uploaded as clips
- [ ] F-CORE-128 Record an audio clip - The microphone icon records audio, check-mark stops, and the clip posts with playback controls - OBSERVED: https://slack.com/help/articles/4406235165587-Record-audio-and-video-clips-in-Slack
- [ ] F-CORE-129 Clip transcripts and captions - A Generate transcript icon creates a text transcript; closed captions are available on video clips - OBSERVED: https://slack.com/help/articles/4406235165587-Record-audio-and-video-clips-in-Slack
- [ ] F-CORE-130 Delete a clip - Clips you recorded can be deleted via their three dots menu, subject to edit/delete permissions - OBSERVED: https://slack.com/help/articles/4406235165587-Record-audio-and-video-clips-in-Slack
- [ ] F-CORE-131 Reply to a clip in thread - Anyone in the channel/DM can play a clip and reply with a clip of their own in the thread - OBSERVED: https://slack.com/help/articles/4406235165587-Record-audio-and-video-clips-in-Slack
- [ ] F-CORE-132 Download files and downloads view - Files download to the device; Cmd+Shift+J opens the list of all downloaded files (desktop-only) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-CORE-133 Files browser - The Files item in the sidebar lists all your files; the plus button there creates canvases and adds files - OBSERVED: https://slack.com/help/articles/360059928654-How-to-use-Slack-your-quick-start-guide

## Search

- [ ] F-CORE-134 Global search field - The search bar at the top starts a search; suggestions appear while typing and Enter shows all results - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-135 Recent search history - The clock icon in the search bar shows/hides previous searches; recent searches are selectable - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-136 Search result filters and sort - The Filters pane refines results; sortable result lists (dates, relevance) order the hits - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-137 Result type tabs - Messages, Files, People, Channels, and Canvases tabs in the search sidebar switch result kinds - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-138 Phrase and exclusion modifiers - Quotation marks force exact-phrase matches; a leading dash excludes a word or modifier - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-139 Scope modifiers - in:#channel (or person/section), from:@person, with:@person (threads+DMs), creator:@person (canvases), each negatable with a dash - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-140 Attribute modifiers - has::emoji: reactions (hasmy: for own), is:saved, has:pin, is:thread - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-141 Date modifiers - before:, after:, on: take dates; during: takes a month or year - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-142 Prefix matching - An asterisk on a 3+ character partial word matches words that begin with it (rep* matches report/reply) - OBSERVED: https://slack.com/help/articles/202528808-Search-in-Slack
- [ ] F-CORE-143 Search shortcuts - Cmd+G starts a search; Cmd+F searches within the current conversation - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts

## Activity, mentions, unreads & notifications

- [ ] F-CORE-144 Activity feed - The Activity view lists mentions, thread replies, and reactions; a filter shows only unread items - OBSERVED: https://slack.com/help/articles/226410907-View-all-your-unread-messages
- [ ] F-CORE-145 Bold unread conversations - Conversations with unreads render bold in the sidebar until opened - OBSERVED: https://slack.com/help/articles/201457107-Send-and-read-messages
- [ ] F-CORE-146 Mention badge and highlight - A numbered badge sits on conversations where you're mentioned; opening highlights the mention - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-147 Default notification triggers - Notifications fire for DMs, thread replies you follow, direct mentions, channel-wide pings, and your keywords - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-148 Keyword alerts - A keyword list in preferences triggers notifications when the words appear in any conversation - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-149 Banner notifications - Desktop banners show sender and conversation; preferences choose the sound and whether to include a message preview - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-CORE-150 Notification schedule and do-not-disturb - A daily schedule and per-occurrence snooze suppress notifications outside chosen hours - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-CORE-151 Sound controls - Notification sounds are selectable per trigger; Mute all sounds silences banners entirely - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-CORE-152 App icon badge (Mac) - A dot marks any unread activity; a numbered red badge marks DMs, mentions, keywords, or followed-thread replies - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-153 Taskbar and tray badges (Windows) - Taskbar shows red badges for actionable activity; the notification-area icon shows blue for unreads and red for mentions/DMs - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-154 Email notifications - When inactive, bundled email digests arrive every 15 minutes or hourly; each notification email accepts a direct reply - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-CORE-155 Notification delivery timing on mobile handoff - Preferences decide whether mobile notifications fire immediately or only after desktop inactivity (or a delay) - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-CORE-156 All-messages vs mentions-only default - Preferences switch global notifications between all new messages and only mentions/DMs - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications

## Saved items, to-dos & reminders

- [ ] F-CORE-157 Save a message or file - More actions > Add to saved keeps any message or file in your private Saved list - OBSERVED: https://slack.com/help/articles/360042650274-Save-messages-and-files-for-later
- [ ] F-CORE-158 Saved section of Home - Saved items live in the Saved section, visible only to you - OBSERVED: https://slack.com/help/articles/360042650274-Save-messages-and-files-for-later
- [ ] F-CORE-159 Open saved item in context - Saved items open the original conversation/thread where they were shared - OBSERVED: https://slack.com/help/articles/360042650274-Save-messages-and-files-for-later
- [ ] F-CORE-160 Clear saved items - The clear icon removes an item; the clean-up icon bulk-removes all saved items or moves them to To-dos - OBSERVED: https://slack.com/help/articles/360042650274-Save-messages-and-files-for-later
- [ ] F-CORE-161 To-dos tab replaces Later - Since October 2026 the sidebar To-dos tab replaces Later; reminder-bearing Later items became to-dos, reminder-free ones moved to Saved - OBSERVED: https://slack.com/help/articles/55757634348435-Later-is-now-To-dos
- [ ] F-CORE-162 Create to-dos - Hover a message/file and click Add to-do, or create a custom to-do from the To-dos tab's Add button - OBSERVED: https://slack.com/help/articles/55757634348435-Later-is-now-To-dos
- [ ] F-CORE-163 To-do reminders - To-dos accept due dates/reminders that notify in Activity; due dates are editable or clearable - OBSERVED: https://slack.com/help/articles/55757634348435-Later-is-now-To-dos
- [ ] F-CORE-164 Complete and filter to-dos - Mark complete checks off an item; a status filter shows in-progress vs completed, plus due-date and reminder filters - OBSERVED: https://slack.com/help/articles/55757634348435-Later-is-now-To-dos
- [ ] F-CORE-165 Move between To-dos and Saved - More actions moves items in both directions (Move to saved / Move to to-dos); clear removes them entirely - OBSERVED: https://slack.com/help/articles/55757634348435-Later-is-now-To-dos
- [ ] F-CORE-166 To-dos bulk clean-up - The clean-up icon bulk-completes or clears overdue/in-progress/completed to-dos - OBSERVED: https://slack.com/help/articles/55757634348435-Later-is-now-To-dos
- [ ] F-CORE-167 Set a reminder on a message - The bell/reminder action offers preset times (in 20 minutes, in 1 hour, in 3 hours, tomorrow, next week, custom) delivered via Slackbot/Activity - OBSERVED: https://slack.com/help/articles/208423427-Set-a-reminder
