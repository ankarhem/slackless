# Slack Desktop Client — Client Shell, Profiles & Preferences Feature Inventory

Parity checklist for slackless against the official Slack desktop app (macOS/Windows/Linux, as of late 2026).
Format: `- [ ] F-SHELL-NNN <Feature> - <checkable behavior> - OBSERVED: <URL>`

## Profiles

- [ ] F-SHELL-001 Open own profile pane - Click your profile picture in the sidebar and select "Profile" to open your profile pane in the app - OBSERVED: https://slack.com/help/articles/204092246-Edit-your-profile
- [ ] F-SHELL-002 Edit profile - Click "Edit" on your profile, modify fields, click "Save Changes" to persist - OBSERVED: https://slack.com/help/articles/204092246-Edit-your-profile
- [ ] F-SHELL-003 Standard profile fields - People can add: full name, display name, title, pronouns, name recording, name pronunciation, phone number, time zone - OBSERVED: https://slack.com/help/articles/204092246-Edit-your-profile
- [ ] F-SHELL-004 Profile photo - Upload a profile photo that displays beside your messages and in the profile pane - OBSERVED: https://slack.com/help/articles/115005506003-Upload-a-profile-photo
- [ ] F-SHELL-005 Locked profile fields - Fields with a lock icon are read-only for members and editable only by owners/admins - OBSERVED: https://slack.com/help/articles/204092246-Edit-your-profile
- [ ] F-SHELL-006 Profile "View as" preview - From your profile, click "View as" to preview how your profile appears to coworkers and external Slack Connect people - OBSERVED: https://slack.com/help/articles/204092246-Edit-your-profile
- [ ] F-SHELL-007 Custom profile fields - Admin-created custom fields (location, start date, social profiles, etc.) appear on every member's profile and are editable on desktop but not on mobile - OBSERVED: https://slack.com/help/articles/212281478-Customize-member-profiles
- [ ] F-SHELL-008 Searchable custom fields - A custom field created with "Show up in Search" can be matched by Slack search queries - OBSERVED: https://slack.com/help/articles/212281478-Customize-member-profiles
- [ ] F-SHELL-009 Profile sections - Admins can organize profile fields into sections (incl. Slack Atlas profiles) that render grouped in the profile pane - OBSERVED: https://slack.com/help/articles/212281478-Customize-member-profiles
- [ ] F-SHELL-010 Pronoun field visibility default - The pronoun field is hidden by default; admins can display it on profiles - OBSERVED: https://slack.com/help/articles/1500011115781-Manage-pronoun-and-name-pronunciation-display-for-member-profiles
- [ ] F-SHELL-011 Name recording and pronunciation - A recorded name audio can be attached to the profile and played by others viewing it - OBSERVED: https://slack.com/blog/news/profile-updates-hybrid-work-teams
- [ ] F-SHELL-012 Local time on profile - Your time zone renders as local time on your profile so coworkers see your current local time - OBSERVED: https://slack.com/help/articles/219889247-Manage-your-time-zone-preferences

## Status

- [ ] F-SHELL-013 Set status - Click profile picture, click the "Update your status" field, type a status and save - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-014 Status emoji - Select an emoji with your status; with no emoji chosen a default 💬 speech balloon displays - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-015 Suggested statuses - The status menu offers suggested status options (e.g. vacation, in a meeting) to pick in one click - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-016 Status expiration - "Remove status after..." clears the status automatically at the chosen date/time - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-017 Status pauses notifications - Check "Pause notifications" while setting a status to pause DND until the status clears - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-018 Scheduled status - Pick "Choose a start and end time" from Duration to schedule a status that turns on automatically - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-019 Manage scheduled statuses - View, edit, or cancel scheduled statuses under "Scheduled"; up to five can be scheduled at once - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-020 Automatic statuses - Preferences > Automatic statuses can set "In a huddle", "In focus mode", and after-working-hours statuses based on activity - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-021 Status visibility - Status emoji shows next to your name everywhere it appears; hovering it on desktop shows the full status - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-022 Edit or clear status - Click the current status in the profile menu to edit, or "Clear status" to remove it - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-023 Workspace status suggestions - Workspace owners/admins can customize the status suggestions members see - OBSERVED: https://slack.com/help/articles/115002531586-Customize-status-suggestions-for-your-workspace
- [ ] F-SHELL-024 Calendar status sync - Google Calendar / Outlook Calendar apps can auto-set statuses like "In a meeting", "Out of Office", "Working remotely" from calendar events - OBSERVED: https://slack.com/help/articles/4412365549075-Automations--Sync-your-status-with-your-calendar

## Presence & Availability

- [ ] F-SHELL-025 Availability dot - A filled dot next to your display name shows active; a hollow dot shows away - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-026 Automatic presence - You are active while Slack is open; set to away after 10 minutes of desktop inactivity (or on mobile when navigating away/closing) - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability
- [ ] F-SHELL-027 Set yourself away/active - From the profile picture menu, "Set yourself as away" / "Set yourself as active" overrides presence manually - OBSERVED: https://slack.com/help/articles/201864558-Set-your-Slack-status-and-availability

## Do Not Disturb & Schedules

- [ ] F-SHELL-028 Pause notifications - Profile picture > Notifications offers preset time frames (e.g. 20 min, 1 hour, tomorrow) or "Custom" to pause notifications - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-029 Resume notifications - Hover Notifications > "Resume notifications" ends a pause immediately - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-030 Snooze indicator - Others see a snooze icon next to your name while notifications are paused - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-031 Urgent DM override - While you are paused, others can override DND once per day to notify you about an urgent DM - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-032 /dnd slash commands - "/dnd for 15 mins", "/dnd until 5pm", "/dnd until tonight", "/dnd off" pause/resume notifications - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-033 Notification schedule - Preferences > Working hours: "Allow notifications" Every day/Weekdays/Custom with start and end time; outside the window notifications pause - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-034 User schedule beats org default - A member's own schedule overrides any workspace/org-level default DND hours - OBSERVED: https://slack.com/help/articles/214908388-Pause-your-Slack-notifications
- [ ] F-SHELL-035 Workspace default DND hours - Owners/admins can set default DND hours applied to all members until overridden - OBSERVED: https://slack.com/help/articles/214888418-Set-default-Do-Not-Disturb-hours

## Focus Mode

- [ ] F-SHELL-036 Focus mode toggle - Click "focus mode" in the sidebar to quiet Slack: theme turns grayscale, sidebar bolding and red badges become subtle dots, hover shows counts - OBSERVED: https://slack.com/help/articles/51416500176147-Use-focus-mode-in-Slack
- [ ] F-SHELL-037 Focus duration - Hover focus mode to pick 30 minutes, 1 hour, 2 hours, or "Don't turn off" - OBSERVED: https://slack.com/help/articles/51416500176147-Use-focus-mode-in-Slack
- [ ] F-SHELL-038 VIP breakthrough in focus mode - Hover focus mode and check "Continue to get notifications from VIPs" to let VIP sounds through - OBSERVED: https://slack.com/help/articles/51416500176147-Use-focus-mode-in-Slack
- [ ] F-SHELL-039 Automatic focus status - Preferences > Automatic statuses controls setting your status to "In focus mode" while enabled (existing statuses are not overwritten) - OBSERVED: https://slack.com/help/articles/51416500176147-Use-focus-mode-in-Slack
- [ ] F-SHELL-040 Calendar-driven focus mode - Preferences can auto-turn on focus mode when a connected Google calendar has a focus block - OBSERVED: https://slack.com/help/articles/51416500176147-Use-focus-mode-in-Slack

## Preferences: Notifications

- [ ] F-SHELL-041 Notifications tab - Preferences > Notifications exposes all notification settings - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-042 Desktop notifications on/off - Check/uncheck "Desktop notifications" under "How to notify you" - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-043 Mobile notifications on/off with overrides - Check/uncheck "Mobile notifications" and set a different mobile trigger level than desktop - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-044 Notify level - "What to notify you about": Everything, or Mentions and direct messages - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-045 "Also notify you about" extras - Checkbox rows: replies to threads you follow, VIP messages when paused/in focus mode, and huddle starting in your channels/DMs - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-046 Activity feed contents - "What to show in Activity" checkboxes: DMs and group DMs, channels with "All new posts", to-do reminders, suggested important unreads - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-047 Keyword notifications - "Channel keywords" field: words/phrases trigger notifications and are highlighted yellow in messages (exact match, not case-sensitive) - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-048 Notification preview - Check "Include a preview of the message in each notification" to show message text on banners - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-049 Mute all sounds - "Mute all sounds from Slack" silences every notification sound - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-050 Per-type sounds - Dropdowns select sounds for: new messages, VIP messages, huddles, sending a DM, receiving a DM while in that conversation - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-051 App icon badge toggle - "Show a badge • on Slack's icon to indicate new activity" turns the dock/taskbar badge on or off - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-052 Per-workspace badges - Multi-workspace members pick which workspaces contribute badge notifications - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-053 Windows taskbar flash - "Flash window when a notification is received": default flashes once when idle 10+ seconds; "Always" flashes even when active - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-054 Mobile timing from desktop - "When I'm not active on desktop" chooses immediately, on inactivity, or delayed mobile push (desktop setting mirrors to mobile) - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-055 Per-conversation notification settings - Preferences lists conversations with custom settings (custom/muted) and lets you reset them - OBSERVED: https://slack.com/help/articles/360056534254-Manage-notifications-for-specific-channels-and-direct-messages

## Preferences: VIP

- [ ] F-SHELL-056 Add VIPs - Preferences > VIP: search and add people, apps, or workflows (up to 300, including Slack Connect external orgs) - OBSERVED: https://slack.com/help/articles/34963579361683-Add-contacts-as-VIPs
- [ ] F-SHELL-057 Remove VIPs - Click "Remove" next to a VIP to drop them from the list - OBSERVED: https://slack.com/help/articles/34963579361683-Add-contacts-as-VIPs
- [ ] F-SHELL-058 VIP unreads section - DMs and mentions from VIPs collect in a "VIP unreads" section at the top of the sidebar - OBSERVED: https://slack.com/help/articles/34963579361683-Add-contacts-as-VIPs
- [ ] F-SHELL-059 VIP notification sound - Notifications > Sound & appearance > "Notification sound (VIP messages only)" gives VIP alerts a distinct sound - OBSERVED: https://slack.com/help/articles/34963579361683-Add-contacts-as-VIPs
- [ ] F-SHELL-060 VIP DND breakthrough - VIP tab checkbox "Always allow notifications from VIPs" delivers VIP alerts during DND and focus mode - OBSERVED: https://slack.com/help/articles/34963579361683-Add-contacts-as-VIPs
- [ ] F-SHELL-061 Activity VIP filter - Activity > Filters > "VIP" filters the feed to VIP-generated notifications only - OBSERVED: https://slack.com/help/articles/34963579361683-Add-contacts-as-VIPs

## Preferences: Navigation

- [ ] F-SHELL-062 Navigation tab - Preferences > Navigation configures the sidebar navigation bar - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-063 Nav-bar tab visibility - Check/uncheck tabs under "Configure these tabs in the navigation bar"; unchecked tabs move under "More" - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-064 Nav-bar tab reorder - Drag and drop (or arrow buttons) reorder items in the navigation bar - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-065 Workspace switcher show/hide - The View options icon shows or hides the workspace switcher rail - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-066 Sidebar display mode - Right-click sidebar to switch between "icons only" and "icons and text" - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-067 Sidebar resize - Click and drag the sidebar edge to change its width - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-068 Sidebar filter - Manage my sidebar icon > Filter and sort > "Active only" (default) hides conversations with no activity in 30 days - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-069 Muted conversations visibility - "Hide muted conversations" / "Show muted conversations" toggle from the manage-sidebar menu - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-070 Clean up channel list - Quick tips > "Clean up your channel list" bulk-leaves (or mutes) channels not viewed in a chosen timeframe - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences

## Preferences: Home

- [ ] F-SHELL-071 Home tab preferences - Preferences > Home customizes what appears at the top of the sidebar in the Home tab - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-072 Home conversation list - Preferences > Home chooses which conversations are shown in the sidebar - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-073 Home sort order - Preferences > Home sets how sidebar conversations are sorted - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-074 Section organization - Sidebar conversations live in default sections that drag-reorder; paid plans add custom sections with emoji - OBSERVED: https://slack.com/help/articles/360043207674-Organize-your-sidebar-with-custom-sections

## Preferences: Appearance & Themes

- [ ] F-SHELL-075 Theme gallery - Preferences > Themes lists preset Slack themes, including vision-assistive themes for colorblind users - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-076 Per-workspace themes - A different theme can be selected per workspace to tell them apart - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-077 Custom theme colors - Appearance > Custom theme: pick a color per UI element via color picker and Save changes - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-078 Theme sharing - "Share" copies theme colors; pasting them into a message renders an "Apply Slack theme" button for recipients - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-079 Legacy theme import - "Import theme" > "Paste your legacy theme colors" > Apply restores pre-redesign colors - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-080 Window gradient - Custom theme preference toggles windows between solid color and gradient - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-081 Darker sidebars - Custom theme preference increases sidebar contrast on light-sidebar themes - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-082 Dark mode - Appearance > Color Mode: Light, Dark, or System (follows OS dark mode on Mac/Windows) - OBSERVED: https://slack.com/help/articles/360019434914-Use-dark-mode-in-Slack
- [ ] F-SHELL-083 Font - Change the font used across the app from preferences - OBSERVED: https://slack.com/help/articles/33702041349395-Change-the-font-in-Slack

## Preferences: Messages & Media

- [ ] F-SHELL-084 Message density - Messages & media: "Clean" (photos beside messages) vs "Compact" (denser, no photos) - OBSERVED: https://slack.com/help/articles/213893898-Change-how-messages-are-displayed
- [ ] F-SHELL-085 Typing indicator - Additional option to display who is currently typing a message - OBSERVED: https://slack.com/help/articles/213893898-Change-how-messages-are-displayed
- [ ] F-SHELL-086 24-hour clock - Additional option to show times with 24-hour clock - OBSERVED: https://slack.com/help/articles/213893898-Change-how-messages-are-displayed
- [ ] F-SHELL-087 Hex color swatches - Additional option to display color swatches next to hexadecimal values in messages - OBSERVED: https://slack.com/help/articles/213893898-Change-how-messages-are-displayed
- [ ] F-SHELL-088 Name display - Names preference: "Full & display names" or "Just display names" (affects only your view) - OBSERVED: https://slack.com/help/articles/115004692303-Choose-how-member-names-appear-for-you-in-Slack
- [ ] F-SHELL-089 Emoji text mode - Emoji preference: display emoji as Unicode or plain text - OBSERVED: https://slack.com/help/articles/4406393601683-Manage-your-emoji-preferences
- [ ] F-SHELL-090 Large emoji - Emoji preference renders emoji-only messages larger (Jumbomoji) - OBSERVED: https://slack.com/help/articles/4406393601683-Manage-your-emoji-preferences
- [ ] F-SHELL-091 Emoticon conversion - Emoji preference converts typed emoticons (e.g. :-)) to emoji - OBSERVED: https://slack.com/help/articles/4406393601683-Manage-your-emoji-preferences
- [ ] F-SHELL-092 Emoji style and skin tone - Emoji render Apple-style on Mac and Google-style on Windows/Linux; default skin tone selectable from the emoji picker hand icon - OBSERVED: https://slack.com/help/articles/4406393601683-Manage-your-emoji-preferences
- [ ] F-SHELL-093 One-click reactions - "Show one-click reactions on messages" with three customizable emoji appears on message hover - OBSERVED: https://slack.com/help/articles/4406393601683-Manage-your-emoji-preferences
- [ ] F-SHELL-094 Inline media & links - "Show images and files from linked websites" toggles inline image/file unfurls - OBSERVED: https://slack.com/help/articles/204399343-Share-links-and-set-preview-preferences
- [ ] F-SHELL-095 Link text previews - "Show text previews of linked websites" toggles text unfurls - OBSERVED: https://slack.com/help/articles/204399343-Share-links-and-set-preview-preferences
- [ ] F-SHELL-096 Email-to-Slack address - Messages & media > "Bring emails into Slack" provides an address that forwards emails into DMs (Slackbot) - OBSERVED: https://slack.com/help/articles/206819278-Send-emails-to-Slack

## Preferences: Language & Region

- [ ] F-SHELL-097 Display language - Language & region tab selects the app's interface language - OBSERVED: https://slack.com/help/articles/215058658-Manage-your-language-preferences
- [ ] F-SHELL-098 Spellcheck toggle - "Enable spellcheck on your messages" turns message spell checking on/off - OBSERVED: https://slack.com/help/articles/215058658-Manage-your-language-preferences
- [ ] F-SHELL-099 Spellcheck languages - Spellchecker language(s) chosen in Language & region; multiple languages can be active at once - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-100 Time zone - Language & region > Time zone dropdown plus "Set time zone automatically" checkbox for travel - OBSERVED: https://slack.com/help/articles/219889247-Manage-your-time-zone-preferences
- [ ] F-SHELL-101 Translation language - Language & region > "Translation language" sets the default target language for AI message translation - OBSERVED: https://slack.com/help/articles/25076892548883-Guide-to-AI-features-in-Slack
- [ ] F-SHELL-102 Local-time behaviors - Changing time zone keeps reminders and notification schedules in local time; message timestamps follow the computer's clock - OBSERVED: https://slack.com/help/articles/219889247-Manage-your-time-zone-preferences

## Preferences: Accessibility

- [ ] F-SHELL-103 Zoom preference - Accessibility > Zoom selects the app zoom level (browsers only get the keyboard shortcut) - OBSERVED: https://slack.com/help/articles/236067467-Adjust-your-zoom-level-in-Slack
- [ ] F-SHELL-104 Animation toggle - Accessibility > Other display options > "Automatically play animations in Slack" controls animated GIF/emoji playback - OBSERVED: https://slack.com/help/articles/228023907-Manage-animated-images-and-emoji
- [ ] F-SHELL-105 Message verbosity - Screen reader prefs choose which message elements are read and in what order, with preview, reset, and sync-across-workspaces - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
- [ ] F-SHELL-106 Screen reader announcements - "Announce incoming messages" screen reader announcement preference - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
- [ ] F-SHELL-107 Huddle emoji read-out - Preference for how emoji are read out in huddles - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
- [ ] F-SHELL-108 Window transparency - On certain Mac devices, window transparency can be turned off from Accessibility preferences - OBSERVED: https://slack.com/help/articles/205166337-Change-your-Slack-theme
- [ ] F-SHELL-109 Simplified layout mode - Optional simplified layout reduces density and distractions for assistive-technology and single-tasking users - OBSERVED: https://www.curbcuts.co/blog/2025-5-19-slack-gives-shoutout-to-simplified-layout-mode
- [ ] F-SHELL-110 Keyboard focus model - Focus outline marks sections; F6/Shift+F6 move between sections, Tab within, arrows between list items - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard

## Preferences: Mark as Read

- [ ] F-SHELL-111 Read behavior options - Mark as read tab: "Start where you left off and mark read", "Start at the newest message and mark read", or "Start at the newest message but leave unseen messages unread" - OBSERVED: https://slack.com/help/articles/360043037853-Manage-your-Mark-as-Read-preference
- [ ] F-SHELL-112 No background mark-as-read - Messages are not marked read while the Slack window is hidden behind other applications - OBSERVED: https://slack.com/release-notes/mac

## Preferences: Audio & Video

- [ ] F-SHELL-113 Camera - Audio & video > Camera dropdown selects the default camera - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-114 Microphone - Audio & video > Microphone dropdown plus live input-level meter to test - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-115 Automatic gain control - Checkbox keeps mic volume constant for listeners - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-116 Noise suppression - Checkbox removes background sound from your mic - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-117 Speaker - Audio & video > Speaker dropdown selects output device - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-118 "In a huddle" status - Preference auto-sets your status while in a huddle - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-119 Join muted - "Always join with your audio muted" huddle preference - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-120 Automatic captions - Preference turns on captions automatically when joining - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-121 Large-channel warning - Warns before you start a huddle in a channel with more than 150 members - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-122 Background blur - "Blur your video background when your camera is on" - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-123 Join preview - Shows a camera preview before joining a huddle - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-124 Reaction animations - Toggles huddle reaction animations - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-125 Alone music - "Play music when you're the last one in a huddle" - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-126 Auto-leave on lock - Automatically leaves the huddle when your computer locks - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences
- [ ] F-SHELL-127 Huddle alert sound - Notifications > Sound & appearance > "Notification sound (huddles)" - OBSERVED: https://slack.com/help/articles/1500002037922-Adjust-your-huddles-preferences

## Preferences: Connected Accounts

- [ ] F-SHELL-128 Connection prompts - Connected accounts tab checkbox "Allow apps to connect to your Slack account" gates third-party link-click account prompts - OBSERVED: https://slack.com/help/articles/218891278-Connect-to-other-services-using-your-Slack-account
- [ ] F-SHELL-129 Disconnect a service - "Disconnect" next to a connected service removes its link to your Slack account - OBSERVED: https://slack.com/help/articles/218891278-Connect-to-other-services-using-your-Slack-account
- [ ] F-SHELL-130 Workflow-connected accounts - Connected accounts lists "Apps connected through workflows" with per-account "Manage accounts" - OBSERVED: https://slack.com/help/articles/27023161053331-Manage-third-party-accounts-connected-to-Slack-through-Workflow-Builder
- [ ] F-SHELL-131 Disconnect-all side effect - Turning off connection prompts also disconnects every connected account - OBSERVED: https://slack.com/help/articles/218891278-Connect-to-other-services-using-your-Slack-account

## Preferences: Privacy & Visibility

- [ ] F-SHELL-132 Contact sharing - Privacy & visibility > Contact sharing chooses what contact info is shareable - OBSERVED: https://slack.com/help/articles/4406144704659-Manage-your-contact-sharing-preferences
- [ ] F-SHELL-133 Slack Connect discoverability - Privacy & visibility > Slack Connect discoverability controls whether people can find you by email address - OBSERVED: https://slack.com/help/articles/5535411189267-Manage-your-Slack-Connect-discoverability-preference
- [ ] F-SHELL-134 External profile details - Admins choose whether external orgs see only basic details or standard work details (name, photo, title, status, local time, pronouns, paused status) - OBSERVED: https://slack.com/help/articles/4417558556691-Manage-profile-details-for-Slack-Connect

## Preferences: Slack AI

- [ ] F-SHELL-135 File summaries toggle - Preferences > AI > File summaries: turn off to stop summarizing files you upload and hide summaries of others' files - OBSERVED: https://slack.com/help/articles/25076892548883-Guide-to-AI-features-in-Slack
- [ ] F-SHELL-136 Agents in top bar - "Show AI Agents in top bar" checkbox plus per-agent show/hide boxes under App agents & assistants - OBSERVED: https://slack.com/help/articles/33076000248851-Work-with-AI-agents-in-Slack

## Preferences: Advanced

- [ ] F-SHELL-137 Enter key behavior - Advanced > When writing a message: press Enter to "Send message" or "Start a new line" - OBSERVED: https://slack.com/help/articles/115005523006-Set-your-Enter-key-preference
- [ ] F-SHELL-138 Search exclusions - Advanced > Search options: add channels to exclude from your search results - OBSERVED: https://slack.com/help/articles/4402305240723-Set-your-search-preferences
- [ ] F-SHELL-139 Search shortcut preference - Search preferences include a keyboard shortcut setting for opening search - OBSERVED: https://slack.com/help/articles/4402305240723-Set-your-search-preferences
- [ ] F-SHELL-140 Download location - Advanced > Download location > Change picks the default folder for saved files (direct-download builds only) - OBSERVED: https://slack.com/help/articles/4609980592915-Manage-your-default-download-location
- [ ] F-SHELL-141 Close-to-tray - Advanced > Other options > "Leave app running in notification area when the window is closed" (Windows/Linux); unchecking quits on close - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-142 Launch at login - "Launch app on login" checkbox starts Slack at system startup (with launch-and-hide honored) - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-143 Reset app data - Resetting app/cached data is available for troubleshooting and clears local state - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-144 Hardware acceleration toggle - "Disable Hardware Acceleration" preference exists for graphical issues, toggleable without foregrounding the app - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-145 Admin-managed configuration - IT can force desktop preferences (plist/mobileconfig on macOS) over user choices - OBSERVED: https://slack.com/help/articles/11906214948755-Manage-desktop-app-configurations

## Desktop Behaviors: Shell & Windows

- [ ] F-SHELL-146 New window from sidebar - Hold Cmd (Mac) or Ctrl (Windows/Linux) and click a channel/DM name in Home to open it in a separate window; unlimited windows supported - OBSERVED: https://slack.com/help/articles/4403608802963-Open-separate-windows-in-Slack
- [ ] F-SHELL-147 Thread in new window - Thread > three dots > "Open in new window" opens the thread in its own window - OBSERVED: https://slack.com/help/articles/4403608802963-Open-separate-windows-in-Slack
- [ ] F-SHELL-148 Canvas in new window - Canvas > three dots > "Open in new window" - OBSERVED: https://slack.com/help/articles/4403608802963-Open-separate-windows-in-Slack
- [ ] F-SHELL-149 Activity/To-dos windows - Hover the Activity or To-dos tab and click the window icon (or Cmd/Ctrl+click) to pop it out - OBSERVED: https://slack.com/help/articles/4403608802963-Open-separate-windows-in-Slack
- [ ] F-SHELL-150 Files browser window - Cmd/Ctrl+click "Files" in the sidebar opens the Files browser in a new window - OBSERVED: https://slack.com/help/articles/201330736-Add-files-to-Slack
- [ ] F-SHELL-151 Open-in-window via keyboard - With a channel/DM focused in the sidebar, Cmd+Enter (Mac) or Ctrl+Enter opens it in a new window - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-152 Close and reopen windows - Cmd/Ctrl+W closes a window; Cmd/Ctrl+Shift+W reopens the last closed window - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-153 Workspace switcher rail - View options icon reveals/collapses the workspace rail; collapsed state shows a single workspace icon atop the navigation bar - OBSERVED: https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences
- [ ] F-SHELL-154 Workspace switching - Ctrl+Tab / Ctrl+Shift+Tab cycles workspaces; Cmd/Ctrl+[1-9] jumps to the numbered workspace - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-155 Multi-workspace sign-in - Multiple workspaces are signed in simultaneously in one app instance - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
- [ ] F-SHELL-156 Per-workspace unread badges - Numbered badges appear on workspace icons for unread activity in other signed-in workspaces; clicking lists them - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-SHELL-157 Native menu bar - Native macOS/Windows menus: Slack (Check for Updates, Preferences, Quit), File, Edit, View, History, Window, Help - OBSERVED: https://slack.com/help/articles/226192087-Join-Slack%E2%80%99s-desktop-app-beta-program
- [ ] F-SHELL-158 Net log capture - Help menu > Troubleshooting > "Restart and Collect Net Logs" for support diagnostics - OBSERVED: https://slack.com/help/articles/205138367-Troubleshoot-connection-issues
- [ ] F-SHELL-159 Full screen - Ctrl+Cmd+F (Mac) / F11 (Windows/Linux) toggles full screen; Esc exits on Windows - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-160 Back/forward history - History toolbar with Cmd+[ / Cmd+] (Alt+arrows on Windows) and touchpad swipe navigation between recent views - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-161 Zoom shortcuts - Cmd/Ctrl + and Cmd/Ctrl - zoom in/out of the app UI - OBSERVED: https://slack.com/help/articles/236067467-Adjust-your-zoom-level-in-Slack
- [ ] F-SHELL-162 Zoom to 200% - Zoom levels extend up to 200%, including via numpad keys - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-163 App auto-update - The app self-updates; a notification confirms updates, the Help icon gains an update badge, and the card offers "Restart Slack" - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-164 Beta channel - Users can join the desktop beta to receive pre-release builds via slack.com/downloads - OBSERVED: https://slack.com/help/articles/226192087-Join-Slack%E2%80%99s-desktop-app-beta-program
- [ ] F-SHELL-165 External links to browser - External URLs open in the default web browser, never inside the Slack window - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-166 External-handoff confirmation - A confirmation pop-up appears when a link opens another program, with an "Always open files of this type" opt-out - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-167 Protocol allowlist - SSH, FTP, SFTP, and SMB links require confirmation with a per-domain "allow all future URLs" option - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-168 macOS menu-bar icon - Slackbot on macOS places a Slack icon in the system menu bar; users can hide it - OBSERVED: https://slack.com/help/articles/115004846068-Slack-updates-and-changes
- [ ] F-SHELL-169 Offline banner and recovery - On connection loss the app shows a connection-problem screen with an improved restart experience and graceful reload while throttled - OBSERVED: https://slack.com/release-notes/mac

## Desktop Behaviors: Notifications, Badge & Dock

- [ ] F-SHELL-170 Banner notifications - OS banner notifications show who sent the message and in which conversation - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-SHELL-171 Click-through - Clicking a banner notification navigates directly to the message that triggered it - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-172 Dismissible banners - Banner notifications expose a Close button for dismissing - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-173 OS notification integration - Delivery honors OS-level notification settings (e.g. macOS System Settings > Notifications for Slack, display-mirroring suppression) - OBSERVED: https://slack.com/help/articles/360001559367-Troubleshoot-Slack-notifications
- [ ] F-SHELL-174 macOS dock badge - Unread activity shows a red badge on the Slack dock icon; a dot means unread activity exists, a number means DMs/mentions/keywords/thread replies - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-SHELL-175 Dock bounce - Incoming notifications bounce the dock icon; bouncing is suppressed while DND is enabled - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-176 Windows taskbar badge - Red badge on the Slack taskbar icon for DMs/mentions/keywords/thread replies - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-SHELL-177 Windows tray icon badges - Notification-area icon shows a blue badge (workspace unread) or red badge (direct notification); shown on Linux too - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-SHELL-178 Badge triggers - Badges fire for DMs, keywords, mentions, app notifications, thread replies, Slack Connect invitations, and overdue reminders; thread replies not mentioning you can be excluded - OBSERVED: https://slack.com/help/articles/201355156-Configure-your-Slack-notifications
- [ ] F-SHELL-179 Sidebar signals - Unread conversations bold in the sidebar with numbered mention badges; opening highlights the mention - OBSERVED: https://slack.com/help/articles/360025446073-Guide-to-Slack-notifications
- [ ] F-SHELL-180 Quiet-hours sound suppression - Notification sounds set to "None" are never played, even for important messages - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-181 Duplicate-notification guard - With multiple windows open, each message produces exactly one notification - OBSERVED: https://slack.com/release-notes/mac

## Desktop Behaviors: Deep Links & Downloads

- [ ] F-SHELL-182 slack://channel deep link - slack://channel?team=<TEAM_ID>&id=<CHANNEL_ID> opens the channel in the desktop app - OBSERVED: https://docs.slack.dev/interactivity/deep-linking
- [ ] F-SHELL-183 slack://user deep link - slack://user?team=<TEAM_ID>&id=<USER_ID> opens the member's profile in the app - OBSERVED: https://docs.slack.dev/interactivity/deep-linking
- [ ] F-SHELL-184 slack://open deep link - slack://open (and tab-specific variants) focuses/opens the Slack app - OBSERVED: https://docs.slack.dev/interactivity/deep-linking
- [ ] F-SHELL-185 app.slack.com links - https://app.slack.com/client/<TEAM_ID>/<CHANNEL_ID> links route into the desktop app instead of the browser - OBSERVED: https://docs.slack.dev/interactivity/deep-linking
- [ ] F-SHELL-186 Browser-to-app handoff - The sign-in flow and the browser page's "Open the Slack App" button hand off into the desktop app - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-187 Profile deep links - Workspace URLs of the form <workspace>.slack.com/team/<USER_ID> open member profiles when clicked in Slack - OBSERVED: https://docs.slack.dev/interactivity/deep-linking
- [ ] F-SHELL-188 Download files - Files download via the download icon; multi-file messages offer "Download [number] files" - OBSERVED: https://slack.com/help/articles/201330736-Add-files-to-Slack
- [ ] F-SHELL-189 Per-download location - Preference can ask where to save each download instead of one folder for all - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-190 In-app download prompt - A completed download shows an in-app prompt rather than a system notification - OBSERVED: https://slack.com/release-notes/mac
- [ ] F-SHELL-191 Downloads list - Cmd/Ctrl+Shift+J opens the list of all downloaded files - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-192 Proxy support - Slack connects via WebSockets over port 443; proxies must support them (or exempt wss-primary/backup/mobile.slack.com), with proxy header injection for workspace restriction and a proxy credential prompt - OBSERVED: https://slack.com/help/articles/360001603387-Manage-Slack-connection-issues

## Keyboard Shortcuts

- [ ] F-SHELL-193 Shortcuts panel - Cmd+/ (Mac) or Ctrl+/ opens the in-app keyboard shortcuts list - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-194 Compose new message - Cmd+N (Mac) / Ctrl+N (Windows/Linux) starts a new message draft - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-195 Unsend a message - Cmd+Z (Mac) / Ctrl+Z (Windows/Linux) unsends your last message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-196 Set your status - Cmd+Shift+Y (Mac) / Ctrl+Shift+Y opens the status setter - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-197 Open preferences (desktop-only) - Cmd+, (Mac) / Ctrl+, (Windows/Linux) opens Preferences - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-198 Resize left sidebar - Left/right arrows resize the sidebar once focused via F6 - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-199 Hide right sidebar - Cmd+. (Mac) / Ctrl+. (Windows/Linux) closes the right sidebar - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-200 New canvas - Cmd+Shift+N (Mac) / Ctrl+Shift+N creates a canvas - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-201 Upload a file (desktop-only) - Cmd+O (Mac) / Ctrl+O opens the file upload dialog - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-202 View downloads - Cmd+Shift+J (Mac) / Ctrl+Shift+J opens all downloaded files - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-203 New snippet - Cmd+Shift+Enter (Mac) / Ctrl+Shift+Enter creates a snippet - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-204 Add reaction (keyboard-layout variants) - Cmd+Shift+` (or Shift+#, \, _, ¡, à, 、 depending on keyboard layout; Ctrl+Shift+ on Windows/Linux) opens the reaction picker - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-205 Start a search - Cmd+G (Mac) / Ctrl+G (Windows/Linux) focuses search - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-206 Search current conversation - Cmd+F (Mac) / Ctrl+F (Windows/Linux) searches within the open conversation - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-207 Huddle toggle - Cmd+Shift+H (Mac) / Ctrl+Shift+H starts, joins, leaves, or ends a huddle - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-208 Huddle mute - Cmd+Shift+Space (Mac) / Ctrl+Shift+Space toggles huddle mute - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-209 Open Help - F1 opens the Help menu/panel - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-210 Open People view - Cmd+Shift+E (Mac) / Ctrl+Shift+E opens the People view - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-211 Quit Slack (desktop-only) - Cmd+Q (Mac) / Ctrl+Q quits the app - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-212 Quick switcher - Cmd+K (Mac) / Ctrl+K or Ctrl+T (Windows/Linux) opens Jump-to conversation - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-213 Jump to first unread (desktop-only) - Cmd+J (Mac) / Ctrl+J (Windows/Linux) moves focus to the most recent unread message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-214 Previous unread conversation - Option+Shift+Up (Mac) / Alt+Shift+Up (Windows/Linux) jumps to the previous unread channel or DM - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-215 Next unread conversation - Option+Shift+Down (Mac) / Alt+Shift+Down goes to the next unread channel or DM - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-216 Previous sidebar conversation - Option+Up (Mac) / Alt+Up opens the previous channel or DM in the sidebar - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-217 Next sidebar conversation - Option+Down (Mac) / Alt+Down opens the next channel or DM in the sidebar - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-218 Back in history - Cmd+[ (Mac) / Alt+Left (Windows/Linux) navigates back - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-219 Forward in history - Cmd+] (Mac) / Alt+Right navigates forward - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-220 Open Home view - Ctrl+1 (Mac) / Ctrl+Shift+1 (Windows/Linux) opens the Home tab - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-221 Browse DMs - Cmd+Shift+K (Mac) / Ctrl+Shift+2 (Windows/Linux) opens the DMs list - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-222 Open Activity view (desktop-only) - Cmd+Shift+M (Mac) / Ctrl+Shift+M opens Activity - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-223 Open More view - Ctrl+0 (Mac) / Ctrl+Shift+0 (Windows/Linux) opens the More overflow menu - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-224 Open Threads view (desktop-only) - Cmd+Shift+T (Mac) / Ctrl+Shift+T opens Threads - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-225 Browse channels (desktop-only) - Cmd+Shift+L (Mac) / Ctrl+Shift+L opens the channel browser - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-226 Conversation details - Cmd+Shift+I (Mac) / Ctrl+Shift+I opens/closes the conversation details pane - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-227 Focus next section - F6 moves focus to the next UI section - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-228 Focus previous section - Shift+F6 moves focus to the previous section - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-229 Collapse/expand all sections - Option+Click (Mac) / Alt+Click on a section header expands or collapses all sidebar sections - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-230 Mark conversation read - Esc marks all messages in the current conversation as read - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-231 Mark everything read - Shift+Esc marks all messages in all conversations as read - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-232 Mark message unread - Option+Click (Mac) / Alt+Click (Windows/Linux) on a message marks it unread - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-233 All unreads view (desktop-only) - Cmd+Shift+A (Mac) / Ctrl+Shift+A opens the All unreads view - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-234 Unreads navigation - Up/Down arrows move between messages in All unreads; Left/Right expand or collapse a group; Esc marks a group read - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-235 Workspace switcher toggle - Cmd+Shift+S (Mac) / Ctrl+Shift+S expands or collapses the workspace switcher - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-236 Tab cycling - Ctrl+PageUp/Ctrl+PageDown switch to the previous/next navigation-bar tab - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-237 Jump to tab - Ctrl+[number] (Mac; Shift+Ctrl+[number] on Windows/Linux) switches to a numbered navigation-bar tab - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-238 Edit message (focus key) - E edits a sent message when it is in focus - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-239 Delete message (focus key) - Delete removes your focused message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-240 Open thread (focus key) - T or Right opens/replies to the thread on the focused message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-241 Forward (focus key) - F forwards the focused message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-242 Pin (focus key) - P pins or unpins the focused message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-243 Save (focus key) - A saves the focused message to Saved items (or removes it) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-244 Mark above unread (focus key) - U marks all messages above the focused one unread - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-245 Reminder (focus key) - M creates a reminder about the focused message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-246 React (focus key) - R adds an emoji reaction to the focused message - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-247 Edit last message - Up arrow from the empty message field starts editing your last sent message (behavior switchable in keyboard preferences) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-248 Select within field - Shift+Up selects text to the beginning of the line; Shift+Down to the end - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-249 New line - Shift+Enter inserts a line break in the composer - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-250 Bold - Cmd+B (Mac) / Ctrl+B bolds selected text - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-251 Italic - Cmd+I (Mac) / Ctrl+I italicizes selected text - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-252 Underline - Cmd+U (Mac) underlines selected text - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-253 Strikethrough - Cmd+Shift+X (Mac) / Ctrl+Shift+X strikes through selected text - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-254 Hyperlink - Cmd+Shift+U (Mac) / Ctrl+Shift+U links the selected text (paste-over a selection also links it) - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-255 Quote - Cmd+Shift+9 (Mac) / Ctrl+Shift+9 formats the selection as a blockquote - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-256 Inline code - Cmd+Shift+C (Mac) / Ctrl+Shift+C formats the selection as code - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-257 Code block - Cmd+Option+Shift+C (Mac) / Ctrl+Alt+Shift+C formats the selection as a code block - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-258 Bulleted list - Cmd+Shift+8 (Mac) / Ctrl+Shift+8 formats the selection as a bulleted list - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-259 Numbered list - Cmd+Shift+7 (Mac) / Ctrl+Shift+7 formats the selection as a numbered list - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-260 Markdown toggle - Cmd+Shift+F (Mac) / Ctrl+Shift+F applies markdown formatting to the selection - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-261 Canvas text styles - Cmd+Option+0..3 (Ctrl+Alt+0..3 on Windows/Linux) set paragraph, big/medium/small heading; Cmd/Ctrl+Shift+0 makes a checklist - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-262 Canvas style toggle - Ctrl+` toggles heading and list styles in a canvas - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-263 Canvas comments - Cmd+Option+T (Mac) / Ctrl+Alt+T opens a canvas comment thread - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-264 Canvas reader/edit - Cmd+Option+R (Mac) / Ctrl+Alt+R switches reader or edit view - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-265 Canvas context menu - Cmd+Shift+/ (Mac) / Shift+F10 (Windows/Linux) opens the canvas context menu - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-266 Canvas find - Cmd/Ctrl+F finds text in a canvas; Cmd/Ctrl+G next; Cmd/Ctrl+Shift+G previous; Cmd/Ctrl+Shift+H find and replace - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-267 Canvas anchor link - Ctrl+Option+Q (Mac) / Ctrl+Alt+Q copies an anchor link to a canvas section - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-268 Canvas undo/redo - Cmd+Z / Cmd+Shift+Z (Ctrl+Z / Ctrl+Shift+Z) undoes and redoes canvas edits - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-269 Canvas list reorder - Cmd+Option+Up/Down (Ctrl+Alt+Up/Down) moves an item up/down in a formatted list - OBSERVED: https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts
- [ ] F-SHELL-270 Focus within section - Tab and Shift+Tab move focus between controls inside a section - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-271 Jump oldest/newest - Home goes to the oldest message; End to the most recent - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-272 Day jump - Fn+Shift+Up/Down (Mac) or Shift+PageUp/PageDown (Windows/Linux) scrolls to the start of the previous/next day's messages - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-273 Message browsing keys - PageUp/PageDown jump up/down and Space scrolls through messages from the message list - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-274 Thread enter/exit - Right arrow enters the focused message's thread; Left arrow returns to the conversation - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-275 Message Actions menu - Tab+M (Mac with Full Keyboard Access) or the Menu key (Windows) opens the Message Actions menu - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-276 Activity keys - In Activity: Up/Down navigate, Enter replies, R marks read, C clears, X selects an item - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-277 Checkbox toggle - Spacebar selects or deselects a focused checkbox - OBSERVED: https://slack.com/help/articles/115003340723-Navigate-Slack-with-your-keyboard
- [ ] F-SHELL-278 Cross-workspace summary (screen reader) - Cmd+Option+S (Mac) / Ctrl+Alt+S reads out notification and unread counts across signed-in workspaces - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
- [ ] F-SHELL-279 Message replay (screen reader) - Cmd+Option+0-9 (Mac) / Alt+0-9 (Windows/Linux) replays up to the last 10 messages in order - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
- [ ] F-SHELL-280 Cmd+number tab navigation - In the new nav bar, Ctrl+1..6 (Mac) / Ctrl+Shift+1..6 (Windows/Linux) jump to Slackbot/Home/DMs/Activity/To-dos/Agents tabs - OBSERVED: https://slack.com/help/articles/360000411963-Use-Slack-with-a-screen-reader
