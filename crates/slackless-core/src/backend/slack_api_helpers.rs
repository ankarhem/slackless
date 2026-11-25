//! Multi-step [`SlackApi`] adapter helpers used by [`super::slack_api`].

use crate::api::{self, FilesFilter, HistoryOpts, ListOpts, Page, SlackApi};
use crate::types::{ChannelId, Conversation, ConversationKind, File, Message, TeamId, Ts, UserId};

use super::{FILES_PAGE, FILES_WORKSPACE_PAGE, HISTORY_PAGE_LIMIT};

pub(super) async fn channel_memberships(api: &SlackApi) -> api::Result<Vec<Conversation>> {
    let opts = ListOpts {
        kinds: vec![
            ConversationKind::PublicChannel,
            ConversationKind::PrivateChannel,
        ],
        exclude_archived: true,
        limit: Some(1000),
    };
    api.users_conversations_all(&opts).await
}

pub(super) async fn history_page(
    api: &SlackApi,
    channel: ChannelId,
    latest: Option<Ts>,
    cursor: Option<String>,
) -> api::Result<Page<Message>> {
    let opts = HistoryOpts {
        latest,
        limit: Some(HISTORY_PAGE_LIMIT),
        ..Default::default()
    };
    api.conversations_history(&channel, &opts, cursor).await
}

pub(super) async fn replies_page(
    api: &SlackApi,
    channel: ChannelId,
    thread_ts: Ts,
    cursor: Option<String>,
) -> api::Result<Page<Message>> {
    let opts = HistoryOpts {
        limit: Some(HISTORY_PAGE_LIMIT),
        ..Default::default()
    };
    api.conversations_replies(&channel, &thread_ts, &opts, cursor)
        .await
}

pub(super) async fn latest_message(
    api: &SlackApi,
    channel: ChannelId,
) -> api::Result<Option<Message>> {
    let opts = HistoryOpts {
        limit: Some(1),
        ..Default::default()
    };
    Ok(api
        .conversations_history(&channel, &opts, None)
        .await?
        .items
        .into_iter()
        .next())
}

pub(super) async fn recent_history(
    api: &SlackApi,
    channel: ChannelId,
    limit: u32,
) -> api::Result<Vec<Message>> {
    let opts = HistoryOpts {
        limit: Some(limit),
        ..Default::default()
    };
    Ok(api
        .conversations_history(&channel, &opts, None)
        .await?
        .items)
}

pub(super) async fn open_dm(api: &SlackApi, users: Vec<UserId>) -> api::Result<Conversation> {
    let csv: Vec<&str> = users.iter().map(UserId::as_str).collect();
    api.conversations_open(&csv.join(",")).await
}

pub(super) async fn move_channel(
    api: &SlackApi,
    insert: Option<(String, ChannelId)>,
    remove: Option<(String, ChannelId)>,
) -> api::Result<()> {
    api.users_channel_sections_channels_bulk_update(
        insert
            .as_ref()
            .map(|(section, channel)| (section.as_str(), channel.clone())),
        remove
            .as_ref()
            .map(|(section, channel)| (section.as_str(), channel.clone())),
    )
    .await
}

pub(super) async fn files(api: &SlackApi, filter: FilesFilter) -> api::Result<Vec<File>> {
    let count = if filter.channel.is_some() {
        FILES_PAGE
    } else {
        FILES_WORKSPACE_PAGE
    };
    Ok(api.files_list(&filter, count, 1).await?.items)
}

pub(super) async fn set_pinned(
    api: &SlackApi,
    channel: ChannelId,
    ts: Ts,
    pinned: bool,
) -> api::Result<()> {
    if pinned {
        api.pins_add(&channel, &ts).await
    } else {
        api.pins_remove(&channel, &ts).await
    }
}

pub(super) async fn create_canvas(api: &SlackApi, channel: ChannelId) -> api::Result<String> {
    Ok(api.conversations_canvases_create(&channel).await?.canvas_id)
}

pub(super) async fn edge_search(
    api: &SlackApi,
    team_id: TeamId,
    query: String,
) -> api::Result<api::EdgeSearchResults> {
    let (users, channels) = tokio::join!(
        SlackApi::edge_users_search(api, &team_id, &query, 15),
        SlackApi::edge_channels_search(api, &team_id, &query, 15),
    );
    Ok(api::EdgeSearchResults {
        users: users.unwrap_or_default(),
        channels: channels.unwrap_or_default(),
    })
}

pub(super) async fn load_app_home(
    api: &SlackApi,
    channel: ChannelId,
) -> api::Result<api::AppHomeResponse> {
    let home = api.conversations_app_home(&channel).await?;
    let _ = api.apps_home_dispatch_open_event(&channel).await;
    Ok(home)
}

pub(super) async fn rooms_get_link(api: &SlackApi, channel: ChannelId) -> api::Result<String> {
    Ok(SlackApi::rooms_get_link(api, &channel)
        .await?
        .unwrap_or_else(|| format!("https://app.slack.com/huddle/{channel}")))
}
