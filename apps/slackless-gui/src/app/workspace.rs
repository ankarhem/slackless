//! Per-workspace view construction (`build_workspace`) and event wiring (`subscribe_workspace`).

use super::*;

/// Build the views of workspace `index` from its fixtures, opened on `channel`.
pub(super) fn build_workspace(
    fixtures: &Arc<Fixtures>,
    index: usize,
    archive: Option<SharedStore>,
    channel: &ChannelId,
    window: &mut Window,
    cx: &mut gpui::App,
) -> WorkspaceViews {
    let sidebar = Sidebar::new(fixtures.clone(), channel.clone(), window, cx);
    let messages = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
    let thread = ThreadPanel::new(fixtures.clone(), window, cx);
    let results = SearchView::new(fixtures.clone(), archive, cx);
    if let Some(team) = fixtures
        .workspaces
        .get(index)
        .or_else(|| fixtures.workspaces.first())
    {
        let ws_id = team.id.clone();
        results.update(cx, |view, cx| view.set_workspace(ws_id, cx));
    }
    let browser = ChannelBrowser::new(fixtures.clone(), window, cx);
    let huddle_map: HashMap<ChannelId, Vec<slackless_core::types::UserId>> =
        huddle_state::fixture_huddle(fixtures)
            .map(|h| (h.channel, h.participants))
            .into_iter()
            .collect();
    sidebar.update(cx, |sidebar, cx| {
        sidebar.set_huddles(huddle_map, cx);
        sidebar.set_workspace(index, cx);
    });
    let profile = ProfilePanel::new(fixtures.clone(), cx);
    let switcher = QuickSwitcher::new(fixtures.clone(), window, cx);
    WorkspaceViews {
        sidebar,
        messages,
        thread,
        results,
        browser,
        profile,
        switcher,
    }
}

/// The root's subscriptions to one workspace's views. Handlers always act on the shown
/// workspace's fields; parked views are not rendered, so they emit nothing.
pub(super) fn subscribe_workspace(
    views: &WorkspaceViews,
    window: &mut Window,
    cx: &mut Context<App>,
) -> Vec<Subscription> {
    let subscriptions = vec![
        cx.subscribe_in(
            &views.sidebar,
            window,
            |this: &mut App, _, event: &SidebarEvent, window, cx| match event {
                SidebarEvent::SelectConversation(channel) => this.select(channel.clone(), cx),
                SidebarEvent::BrowseChannels => this.open_browser(window, cx),
                SidebarEvent::CreateChannel => this.open_create_channel(window, cx),
                SidebarEvent::SwitchWorkspace(index) => this.switch_workspace(*index, window, cx),
                SidebarEvent::ReorderWorkspace { from, to } => {
                    this.reorder_workspace(*from, *to, window, cx)
                }
                SidebarEvent::OpenInNewWindow(channel) => {
                    this.pop_out_conversation(channel.clone(), window, cx)
                }
                SidebarEvent::SectionPrefsChanged => {
                    let prefs = this.sidebar.read(cx).export_section_prefs();
                    if let Some(write) = this.bridge.remember_section_prefs(&prefs) {
                        cx.background_executor().spawn(write).detach();
                    }
                }
                SidebarEvent::OpenDrafts => this.open_drafts(cx),
                SidebarEvent::OpenLater => this.open_later(cx),
                SidebarEvent::OpenThreads => this.open_threads(cx),
                SidebarEvent::OpenUnreads => this.open_unreads(window, cx),
                SidebarEvent::OpenActivity => this.open_activity(cx),
                SidebarEvent::LeftActivity => {
                    if this.page == MainPage::Activity {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
                SidebarEvent::OpenFiles => this.open_files(cx),
                SidebarEvent::OpenTools => this.open_tools(cx),
                SidebarEvent::OpenAdmin => this.open_admin(window, cx),
                SidebarEvent::OpenCleanupChannels => this.open_inactive_channels_dialog(window, cx),
                SidebarEvent::ShowFilesSection(section) => this
                    .files_page
                    .update(cx, |page, cx| page.show_section(*section, window, cx)),
                SidebarEvent::OpenHuddles => this.open_huddles(cx),
                SidebarEvent::OpenDirectories => this.open_directory(cx),
                SidebarEvent::OpenCompose => this.open_compose("", window, cx),
                SidebarEvent::CreateSection { name } => this.create_section(name.clone(), cx),
                SidebarEvent::CreateSectionWithChannel { name, channel } => {
                    if this.fixtures.sample {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.add_section_with_channel(name.clone(), channel.clone(), cx)
                        });
                    } else {
                        this.bridge.send(ChannelCommand::CreateSectionWithChannel {
                            name: name.clone(),
                            channel: channel.clone(),
                        });
                    }
                }
                SidebarEvent::RenameSection { section, name } => {
                    if this.fixtures.sample {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.rename_section_local(section, name, cx)
                        });
                    } else {
                        this.bridge.send(ChannelCommand::RenameSection {
                            section: section.clone(),
                            name: name.clone(),
                        });
                    }
                }
                SidebarEvent::DeleteSection { section } => {
                    if this.fixtures.sample {
                        this.sidebar
                            .update(cx, |sidebar, cx| sidebar.delete_section_local(section, cx));
                    } else {
                        this.bridge.send(ChannelCommand::DeleteSection {
                            section: section.clone(),
                        });
                    }
                }
                SidebarEvent::MoveChannel { channel, from, to } => {
                    this.bridge.send(ChannelCommand::MoveChannel {
                        channel: channel.clone(),
                        from: from.clone(),
                        to: to.clone(),
                    });
                }
                SidebarEvent::MarkRead { channel, ts } => {
                    this.mark_read(channel, ts.clone(), cx);
                }
                SidebarEvent::SetNotifyLevel { channel, level } => {
                    this.bridge.send(ChannelCommand::SetNotifyLevel {
                        channel: channel.clone(),
                        level: *level,
                    });
                }
                SidebarEvent::OpenChannelDetails(channel) => {
                    this.open_channel_details(
                        channel,
                        channel_details::DetailsTab::About,
                        window,
                        cx,
                    );
                }
                SidebarEvent::LeaveChannel(channel) => {
                    this.leave(channel.clone(), window, cx);
                }
                SidebarEvent::ToggleFocus => this.toggle_focus(window, cx),
                SidebarEvent::FocusDuration(preset) => {
                    let auto_status = this.preferences.read(cx).settings().focus_auto_status;
                    this.self_state
                        .start_focus_with_status(*preset, auto_status);
                    if !this.fixtures.sample {
                        let minutes = crate::status::FOCUS_DURATIONS
                            .get(*preset)
                            .and_then(|(_, secs)| *secs)
                            .map(|secs| ((secs + 59) / 60).clamp(1, 1440) as u32)
                            .unwrap_or(480);
                        this.bridge.send(WorkspaceCommand::SetDndSnooze { minutes });
                        if auto_status {
                            this.send_status_to_slack(this.self_state.status.as_ref());
                        }
                    }
                    this.apply_preferences(window, cx);
                }
                SidebarEvent::OpenUserMenu => {
                    this.user_menu_open = !this.user_menu_open;
                    cx.notify();
                }
            },
        ),
        cx.subscribe_in(
            &views.results,
            window,
            |this: &mut App, _, event: &SearchEvent, window, cx| match event {
                SearchEvent::Close => {
                    if this.page == MainPage::Search {
                        this.page = MainPage::Channel;
                    }
                    cx.notify();
                }
                SearchEvent::Open { channel, ts } => {
                    this.select(channel.clone(), cx);
                    this.messages.update(cx, |list, cx| list.reveal(ts, cx));
                }
                SearchEvent::OpenInWorkspace {
                    workspace,
                    channel,
                    ts,
                } => {
                    this.open_message_in_workspace(
                        workspace,
                        channel.clone(),
                        ts.clone(),
                        window,
                        cx,
                    );
                }
                SearchEvent::OpenChannel(channel) => {
                    this.select(channel.clone(), cx);
                }
                SearchEvent::OpenProfile(user) => {
                    this.open_profile(user.clone(), cx);
                }
                SearchEvent::OpenTranscript {
                    call_id,
                    channel,
                    highlight_segment_id,
                    highlight_query,
                } => {
                    this.open_huddle_transcript(
                        call_id.clone(),
                        channel.clone(),
                        *highlight_segment_id,
                        highlight_query.clone(),
                        window,
                        cx,
                    );
                }
                SearchEvent::OpenTranscriptInWorkspace {
                    workspace,
                    call_id,
                    channel,
                    highlight_segment_id,
                    highlight_query,
                } => {
                    if let Some(idx) = this.workspace_index_for(workspace) {
                        if idx != this.workspace {
                            this.switch_workspace(idx, window, cx);
                        }
                        this.open_huddle_transcript(
                            call_id.clone(),
                            channel.clone(),
                            *highlight_segment_id,
                            highlight_query.clone(),
                            window,
                            cx,
                        );
                    } else {
                        notify(
                            "Workspace not loaded",
                            format!(
                                "Sign in to workspace {} to open its huddle transcript.",
                                workspace
                            ),
                            window,
                            cx,
                        );
                    }
                }
                SearchEvent::RemoteMessages { query, sort, page } => {
                    this.bridge.send(CollabCommand::SearchRemoteMessages {
                        query: query.clone(),
                        sort: *sort,
                        page: *page,
                    });
                }
                SearchEvent::RemoteFiles { query, sort, page } => {
                    this.bridge.send(CollabCommand::SearchRemoteFiles {
                        query: query.clone(),
                        sort: *sort,
                        page: *page,
                    });
                }
            },
        ),
        cx.subscribe_in(
            &views.browser,
            window,
            |this: &mut App, _, event: &ChannelBrowserEvent, window, cx| {
                let channel = match event {
                    ChannelBrowserEvent::Join(channel) => {
                        if let Some(joined) = this.fixtures.conversation(channel).cloned() {
                            this.sidebar
                                .update(cx, |sidebar, cx| sidebar.add_channel(joined, cx));
                        }
                        channel
                    }
                    ChannelBrowserEvent::Open(channel) => channel,
                };
                window.close_dialog(cx);
                this.select(channel.clone(), cx);
            },
        ),
        cx.subscribe_in(
            &views.messages,
            window,
            |this: &mut App, _, event: &MessageListEvent, window, cx| match event {
                MessageListEvent::OpenThread { channel, thread_ts } => {
                    this.open_thread(channel.clone(), thread_ts.clone(), cx);
                }
                MessageListEvent::Later(request) => this.save_for_later(request, cx),
                MessageListEvent::Changed { channel, change } => {
                    this.post_change(channel, change);
                    if let MessageChange::MarkUnread { ts } = change {
                        this.apply_local_mark_unread(channel, ts.clone(), cx);
                    }
                }
                MessageListEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                MessageListEvent::OpenLink(target) => this.open_link(target.clone(), window, cx),
                MessageListEvent::OpenImage {
                    channel,
                    images,
                    index,
                } => this.open_image_viewer(channel.clone(), images.clone(), *index, window, cx),
                MessageListEvent::Forward(link) => this.open_compose(link, window, cx),
                MessageListEvent::JoinHuddle(channel) => {
                    if this.channel != *channel {
                        this.select(channel.clone(), cx);
                    }
                    this.start_huddle(window, cx);
                }
                MessageListEvent::ViewHistory { channel, ts } => {
                    this.open_revision_history(channel.clone(), ts.clone(), window, cx)
                }
                MessageListEvent::AddToList { channel, ts } => {
                    this.open_add_to_list_dialog(channel.clone(), ts.clone(), window, cx)
                }
                MessageListEvent::NeedHistorySince { channel, since } => {
                    this.load_since(channel, *since, cx);
                }
                MessageListEvent::NeedOlder { channel, before } => {
                    this.load_older(channel, before.clone(), cx);
                }
                MessageListEvent::PickReaction {
                    channel,
                    ts,
                    position,
                } => {
                    let target = PickerTarget::Message {
                        channel: channel.clone(),
                        ts: ts.clone(),
                    };
                    this.open_picker(target, *position, window, cx);
                }
                MessageListEvent::PickEditEmoji(position) => {
                    this.open_picker(PickerTarget::MessageEdit, *position, window, cx);
                }
            },
        ),
        cx.subscribe_in(
            &views.thread,
            window,
            |this: &mut App, _, event: &ThreadPanelEvent, window, cx| match event {
                ThreadPanelEvent::PickReaction { ts, position } => {
                    let target = PickerTarget::ThreadMessage(ts.clone());
                    this.open_picker(target, *position, window, cx);
                }
                ThreadPanelEvent::PickEmoji(position) => {
                    this.open_picker(PickerTarget::ThreadComposer, *position, window, cx);
                }
                ThreadPanelEvent::PickGif(position) => {
                    this.open_picker_tab(
                        PickerTarget::ThreadComposer,
                        *position,
                        PickerTab::Gifs,
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::PickEditEmoji(position) => {
                    this.open_picker(PickerTarget::ThreadEdit, *position, window, cx);
                }
                ThreadPanelEvent::Close => {
                    this.close_thread(cx);
                    cx.notify();
                }
                ThreadPanelEvent::OpenProfile(user) => this.open_profile(user.clone(), cx),
                ThreadPanelEvent::Typing => {
                    if let Some((channel, ts)) = this.thread.read(cx).open_thread() {
                        this.bridge.send(MessagingCommand::Typing {
                            channel,
                            thread_ts: Some(ts),
                        });
                    }
                }
                ThreadPanelEvent::OpenLink(target) => this.open_link(target.clone(), window, cx),
                ThreadPanelEvent::OpenImage {
                    channel,
                    images,
                    index,
                } => this.open_image_viewer(channel.clone(), images.clone(), *index, window, cx),
                ThreadPanelEvent::Forward(link) => this.open_compose(link, window, cx),
                ThreadPanelEvent::ViewHistory { channel, ts } => {
                    this.open_revision_history(channel.clone(), ts.clone(), window, cx)
                }
                ThreadPanelEvent::AddToList { channel, ts } => {
                    this.open_add_to_list_dialog(channel.clone(), ts.clone(), window, cx)
                }
                ThreadPanelEvent::Export {
                    channel,
                    thread_ts,
                    markdown,
                } => {
                    this.export_thread_transcript(
                        channel.clone(),
                        thread_ts.clone(),
                        *markdown,
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::SendToChannel {
                    channel,
                    thread_ts,
                    content,
                } => {
                    this.bridge.send(MessagingCommand::SendMessage {
                        channel: channel.clone(),
                        content: content.clone(),
                        thread_ts: Some(thread_ts.clone()),
                        broadcast: true,
                    });
                    window.push_notification("Sent reply to channel", cx);
                }
                ThreadPanelEvent::Reply {
                    channel,
                    thread_ts,
                    content,
                    broadcast,
                } => {
                    this.send_thread_reply(
                        (channel.clone(), thread_ts.clone()),
                        content.clone(),
                        *broadcast,
                        false,
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::ReplyWithFiles {
                    channel,
                    thread_ts,
                    content,
                    files,
                } => {
                    this.upload_files(
                        channel.clone(),
                        Some(thread_ts.clone()),
                        content.clone(),
                        files.clone(),
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::SlashCommand(invocation) => {
                    let thread = this.thread.read(cx).open_thread();
                    this.run_slash(invocation, thread, window, cx)
                }
                ThreadPanelEvent::Schedule {
                    channel,
                    thread_ts,
                    text,
                    when,
                } => {
                    this.schedule_at(
                        messaging::ScheduleSpec {
                            channel: channel.clone(),
                            thread_ts: Some(thread_ts.clone()),
                            text: text.clone(),
                            post_at: when.at(now_secs()),
                            when_label: when.label().to_lowercase(),
                        },
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::ScheduleCustom {
                    channel,
                    thread_ts,
                    text,
                } => {
                    this.open_custom_schedule(
                        channel.clone(),
                        Some(thread_ts.clone()),
                        text.clone(),
                        window,
                        cx,
                    );
                }
                ThreadPanelEvent::PopOut { channel, thread_ts } => {
                    this.pop_out_thread(channel.clone(), thread_ts.clone(), window, cx);
                }
                ThreadPanelEvent::Later(request) => this.save_for_later(request, cx),
                ThreadPanelEvent::Changed { channel, change } => this.post_change(channel, change),
                ThreadPanelEvent::SetSubscribed {
                    channel,
                    thread_ts,
                    last_read,
                    subscribed,
                } => this.bridge.send(MessagingCommand::SetThreadSubscribed {
                    channel: channel.clone(),
                    thread_ts: thread_ts.clone(),
                    last_read: last_read.clone(),
                    subscribed: *subscribed,
                }),
                ThreadPanelEvent::MarkThreadRead {
                    channel,
                    thread_ts,
                    ts,
                } => {
                    this.mark_thread_unread(channel.clone(), thread_ts.clone(), ts.clone(), cx);
                }
            },
        ),
        cx.subscribe_in(
            &views.profile,
            window,
            |this: &mut App, _, event: &ProfileEvent, window, cx| match event {
                // Slack's close shuts the whole pane, the thread under the profile too.
                ProfileEvent::Close => {
                    this.set_right_pane(RightPane::None, cx);
                    this.close_thread(cx);
                    cx.notify();
                }
                ProfileEvent::Back => {
                    if matches!(this.right_pane, RightPane::Profile { over_thread: true }) {
                        this.set_right_pane(RightPane::Thread, cx);
                    } else {
                        this.set_right_pane(RightPane::None, cx);
                    }
                    cx.notify();
                }
                ProfileEvent::Message(user) => {
                    let dm = this
                        .fixtures
                        .dms
                        .iter()
                        .find(|c| c.user.as_ref() == Some(user))
                        .map(|c| c.id.clone());
                    if let Some(dm) = dm {
                        this.select(dm, cx);
                    }
                }
                ProfileEvent::EditStatus => this.open_status_picker(window, cx),
                ProfileEvent::EditProfile => this.open_profile_editor(window, cx),
            },
        ),
        cx.subscribe_in(
            &views.switcher,
            window,
            |this: &mut App, _, event: &SwitcherEvent, window, cx| match event {
                SwitcherEvent::Open(channel) => {
                    window.close_dialog(cx);
                    this.select(channel.clone(), cx);
                    this.focus.focus(window, cx);
                }
                SwitcherEvent::OpenRemoteChannel(conv) => {
                    window.close_dialog(cx);
                    if this.fixtures.conversation(&conv.id).is_none()
                        && let Some((store, ws)) = this.bridge.store().zip(this.bridge.team_id())
                    {
                        if let Ok(mut guard) = store.lock() {
                            let _ = guard.upsert_conversation(ws, conv);
                        }
                        this.reload_live(window, cx);
                    }
                    this.select(conv.id.clone(), cx);
                    this.focus.focus(window, cx);
                }
                SwitcherEvent::OpenUser(user) => {
                    window.close_dialog(cx);
                    this.message_user(user, window, cx);
                    this.focus.focus(window, cx);
                }
                SwitcherEvent::SearchRemote(query) => {
                    this.bridge.send(CollabCommand::QuickSwitchSearch {
                        query: query.clone(),
                    });
                }
            },
        ),
    ];
    subscriptions
}
