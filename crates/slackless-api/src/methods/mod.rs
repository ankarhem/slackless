//! Typed endpoint methods. Method-name strings come from [`crate::types::endpoints::method`];
//! request params are built inline per method.
//!
//! Response structs model only what slackless uses; every field is defaulted, unknown keys are
//! kept in `extra` (or ignored), and lists use lenient element parsing.

mod activity_files;
mod canvases_lists;
mod huddles;
mod messaging;
mod wire;
pub use wire::*;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use futures::TryStreamExt;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::de::lenient_vec;
use crate::pagination::{Page, paginate};
use crate::types::endpoints::method;
use crate::types::{
    ApiResponse, Block, BotId, BotProfile, ChannelId, ChannelSection, Conversation, DndStatus,
    File, Message, RichTextBlock, Team, TeamId, Ts, User, UserId, UserPresence, Usergroup,
};
use crate::{ApiError, Result, SlackApi};

// ---------------------------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------------------------

impl SlackApi {
    /// `auth.test`: validates the session and says who/where we are.
    pub async fn auth_test(&self) -> Result<AuthTest> {
        Ok(self.call(method::AUTH_TEST, Vec::new()).await?.data)
    }

    /// `auth.revoke`: revokes the current token server-side on sign-out (`F-CORE-009`).
    pub async fn auth_revoke(&self) -> Result<()> {
        self.call_ok(method::AUTH_REVOKE, Vec::new()).await
    }

    /// `conversations.addTab`: add a tab (`files`, `bookmarks`, `folder`, `canvas`, etc.) to `channel` (`F-CORE-061`).
    pub async fn conversations_add_tab(
        &self,
        channel: &ChannelId,
        tab_type: &str,
        label: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("type", tab_type.to_owned()),
            ("reason", "add-channel-tab".to_owned()),
        ];
        if let Some(label) = label.filter(|s| !s.is_empty()) {
            params.push(("label", label.to_owned()));
        }
        self.call_ok(method::CONVERSATIONS_ADD_TAB, params).await
    }

    /// `conversations.removeTab`: remove a tab from `channel` while keeping shared files (`F-CORE-061`).
    pub async fn conversations_remove_tab(&self, channel: &ChannelId, tab_id: &str) -> Result<()> {
        let params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("id", tab_id.to_owned()),
            ("keep_file_share", "true".to_owned()),
            ("reason", "remove-channel-tab".to_owned()),
        ];
        self.call_ok(method::CONVERSATIONS_REMOVE_TAB, params).await
    }

    /// `conversations.moveTab`: reorder a tab inside `channel` (`F-CORE-061`).
    pub async fn conversations_move_tab(
        &self,
        channel: &ChannelId,
        tab_id: &str,
        after: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("id", tab_id.to_owned()),
        ];
        if let Some(after) = after.filter(|s| !s.is_empty()) {
            params.push(("after", after.to_owned()));
        }
        self.call_ok(method::CONVERSATIONS_MOVE_TAB, params).await
    }

    /// `client.userBoot` (cold boot): self, team, channels, IMs, priorities, prefs.
    pub async fn client_user_boot(&self) -> Result<UserBoot> {
        let params = vec![
            ("version_all_channels", "false".to_owned()),
            // The official boot asks for every MPDM relevant to the sidebar; without it the
            // boot channel list is capped and newer group DMs never arrive
            // (docs/research/api-channels.md, "The boot channel list is not a directory").
            ("return_all_relevant_mpdms", "true".to_owned()),
            ("omit_channels", "false".to_owned()),
            ("include_min_version_bump_check", "1".to_owned()),
            ("_x_reason", "initial-data".to_owned()),
            ("_x_sonic", "true".to_owned()),
        ];
        Ok(self.call(method::CLIENT_USER_BOOT, params).await?.data)
    }

    /// `client.counts`: unread and mention counts for every open conversation and threads.
    pub async fn client_counts(&self) -> Result<ClientCounts> {
        let params = vec![
            ("thread_counts_by_channel", "true".to_owned()),
            // As the official client asks: without it Slack leaves some joined conversations
            // (e.g. Slack Connect channels) out, and the sidebar cannot list them.
            ("include_all_unreads", "true".to_owned()),
            ("org_wide_aware", "true".to_owned()),
            ("include_file_channels", "true".to_owned()),
            (
                "_x_reason",
                "client-counts-api/fetchClientCounts".to_owned(),
            ),
        ];
        Ok(self.call(method::CLIENT_COUNTS, params).await?.data)
    }

    /// `conversations.view`: channel metadata plus the newest `count` messages in one call.
    pub async fn conversations_view(
        &self,
        channel: &ChannelId,
        count: Option<u32>,
    ) -> Result<ConversationView> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("canonical_avatars", "true".to_owned()),
        ];
        if let Some(count) = count {
            params.push(("count", count.to_string()));
        }
        Ok(self.call(method::CONVERSATIONS_VIEW, params).await?.data)
    }

    /// One page of `conversations.history`, newest first. Pass the previous page's
    /// `next_cursor` to continue; see [`paginate`] to walk them all.
    pub async fn conversations_history(
        &self,
        channel: &ChannelId,
        opts: &HistoryOpts,
        cursor: Option<String>,
    ) -> Result<Page<Message>> {
        let mut params = vec![("channel", channel.to_string())];
        opts.push(&mut params);
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self
            .call::<MessagesWire>(method::CONVERSATIONS_HISTORY, params)
            .await?;
        Ok(page(resp, |w| w.messages))
    }

    /// One page of `conversations.replies`: the thread root followed by replies.
    pub async fn conversations_replies(
        &self,
        channel: &ChannelId,
        thread_ts: &Ts,
        opts: &HistoryOpts,
        cursor: Option<String>,
    ) -> Result<Page<Message>> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("ts", thread_ts.to_string()),
        ];
        opts.push(&mut params);
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self
            .call::<MessagesWire>(method::CONVERSATIONS_REPLIES, params)
            .await?;
        Ok(page(resp, |w| w.messages))
    }

    /// `conversations.info`.
    pub async fn conversations_info(&self, channel: &ChannelId) -> Result<Conversation> {
        let params = vec![
            ("channel", channel.to_string()),
            ("include_num_members", "true".to_owned()),
        ];
        let resp = self
            .call::<ChannelWire>(method::CONVERSATIONS_INFO, params)
            .await?;
        Ok(resp.data.channel)
    }

    /// `conversations.info` with `return_app_home=true`: fetches an app DM's Home tab flags
    /// and published Block Kit `home_view` (`F-COLLAB-480`, `docs/research/api-apps-blocks.md`).
    pub async fn conversations_app_home(&self, channel: &ChannelId) -> Result<AppHomeResponse> {
        let params = vec![
            ("channel", channel.to_string()),
            ("return_app_home", "true".to_owned()),
            ("_x_reason", "app-home-open-api".to_owned()),
        ];
        let resp = self
            .call::<AppHomeWire>(method::CONVERSATIONS_INFO, params)
            .await?;
        Ok(resp.data.into_response())
    }

    /// `apps.home.dispatchOpenEvent`: notifies an app that the user opened its Home tab so it
    /// can publish or refresh `home_view` via `views.publish` (`F-COLLAB-480`).
    pub async fn apps_home_dispatch_open_event(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![
            ("id", channel.to_string()),
            ("type", "home".to_owned()),
            ("_x_reason", "app-home-logging".to_owned()),
        ];
        self.call_ok(method::APPS_HOME_DISPATCH_OPEN_EVENT, params)
            .await
    }

    /// `apps.home.open`: resolve an app's DM conversation id from `(app_id, team_id)` (`F-COLLAB-481`, `F-COLLAB-523`).
    pub async fn apps_home_open(
        &self,
        app_id: &str,
        app_installed_team_id: &str,
    ) -> Result<Option<ChannelId>> {
        let params = vec![
            ("app_id", app_id.to_owned()),
            ("app_installed_team_id", app_installed_team_id.to_owned()),
            ("_x_reason", "app-home-open-api".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let data: Value = self.call(method::APPS_HOME_OPEN, params).await?.data;
        let conv = data
            .get("app_home")
            .and_then(|v| v.get("conversation_id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ChannelId::from);
        Ok(conv)
    }

    /// `workflows.triggers.trip`: trigger a Slack Workflow Builder link trigger (`https://slack.com/shortcuts/Ft...`,
    /// `F-COLLAB-319`, `F-COLLAB-376`). Any resulting modal arrives over the websocket as `view_opened`.
    pub async fn workflows_triggers_trip(&self, url: &str, context: &Value) -> Result<()> {
        let parsed =
            url::Url::parse(url).map_err(|_| crate::ApiError::UntrustedUrl(url.to_owned()))?;
        if !crate::client::is_trusted_slack_https_url(&parsed) {
            return Err(crate::ApiError::UntrustedUrl(url.to_owned()));
        }
        let params = vec![
            ("client_token", client_token()),
            ("url", url.to_owned()),
            ("context", context.to_string()),
            (
                "_x_reason",
                "trip-trigger-from-bk-workflow-button".to_owned(),
            ),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::WORKFLOWS_TRIGGERS_TRIP, params).await
    }

    /// One page of `conversations.list`.
    pub async fn conversations_list(
        &self,
        opts: &ListOpts,
        cursor: Option<String>,
    ) -> Result<Page<Conversation>> {
        let params = listing_params(opts, cursor);
        let resp = self
            .call::<ChannelsWire>(method::CONVERSATIONS_LIST, params)
            .await?;
        Ok(page(resp, |w| w.channels))
    }

    /// One page of `users.conversations`: the conversations the signed-in user is a member
    /// of. Reports no `is_open`, so it is no source for DM visibility.
    pub async fn users_conversations(
        &self,
        opts: &ListOpts,
        cursor: Option<String>,
    ) -> Result<Page<Conversation>> {
        let params = listing_params(opts, cursor);
        let resp = self
            .call::<ChannelsWire>(method::USERS_CONVERSATIONS, params)
            .await?;
        Ok(page(resp, |w| w.channels))
    }

    /// Every page of `users.conversations`.
    pub async fn users_conversations_all(&self, opts: &ListOpts) -> Result<Vec<Conversation>> {
        paginate(|cursor| self.users_conversations(opts, cursor))
            .try_collect()
            .await
    }

    /// Every page of `conversations.list`.
    pub async fn conversations_list_all(&self, opts: &ListOpts) -> Result<Vec<Conversation>> {
        paginate(|cursor| self.conversations_list(opts, cursor))
            .try_collect()
            .await
    }

    /// One page of `conversations.members`: the conversation's member ids.
    pub async fn conversations_members(
        &self,
        channel: &ChannelId,
        cursor: Option<String>,
    ) -> Result<Page<UserId>> {
        let mut params = vec![("channel", channel.to_string())];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self
            .call::<MemberIdsWire>(method::CONVERSATIONS_MEMBERS, params)
            .await?;
        Ok(page(resp, |w| w.members))
    }

    /// Every page of `conversations.members`.
    pub async fn conversations_members_all(&self, channel: &ChannelId) -> Result<Vec<UserId>> {
        paginate(|cursor| self.conversations_members(channel, cursor))
            .try_collect()
            .await
    }

    /// A channel's managers: the users holding Slack's channel-manager role on it, from
    /// `admin.roles.entity.listAssignments` (what Slack's channel details read). Read-only.
    pub async fn channel_managers(&self, channel: &ChannelId) -> Result<Vec<UserId>> {
        let params = vec![("entity_id", channel.to_string())];
        let resp = self
            .call::<RoleAssignmentsWire>(method::ADMIN_ROLES_ENTITY_LIST_ASSIGNMENTS, params)
            .await?;
        Ok(resp
            .data
            .role_assignments
            .into_iter()
            .filter(|assignment| assignment.role_id == CHANNEL_MANAGER_ROLE)
            .flat_map(|assignment| assignment.users)
            .collect())
    }

    /// One page of `client.dms`: the DMs-tab list (IMs and MPIMs) with per-DM `is_open`,
    /// closed DMs included (docs/research/api-channels.md, `client.dms`).
    pub async fn client_dms(&self, cursor: Option<String>) -> Result<Page<Conversation>> {
        let mut params = vec![
            ("count", "250".to_owned()),
            ("include_closed", "true".to_owned()),
            ("include_channel", "true".to_owned()),
            ("_x_reason", "dms-tab-populate".to_owned()),
        ];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self.call::<DmsWire>(method::CLIENT_DMS, params).await?;
        let next_cursor = resp.next_cursor().map(str::to_owned);
        let items = resp
            .data
            .ims
            .into_iter()
            .chain(resp.data.mpims)
            .map(DmWire::into_conversation)
            .collect();
        Ok(Page { items, next_cursor })
    }

    /// Every page of `client.dms`.
    pub async fn client_dms_all(&self) -> Result<Vec<Conversation>> {
        paginate(|cursor| self.client_dms(cursor))
            .try_collect()
            .await
    }

    /// `conversations.leave`: the signed-in user leaves `channel`.
    pub async fn conversations_leave(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call_ok(method::CONVERSATIONS_LEAVE, params).await
    }

    /// `conversations.mark`: moves the read cursor to `ts` (an earlier ts marks unread).
    pub async fn conversations_mark(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![("channel", channel.to_string()), ("ts", ts.to_string())];
        self.call_ok(method::CONVERSATIONS_MARK, params).await
    }

    /// `users.markAllRead`: every conversation and thread read (Slack's Shift+Esc).
    pub async fn users_mark_all_read(&self) -> Result<()> {
        self.call_ok(method::USERS_MARK_ALL_READ, Vec::new()).await
    }

    /// `subscriptions.thread.add` / `subscriptions.thread.remove`: follow or unfollow a
    /// thread, so its replies notify you and it lists under Threads. `last_read` is the
    /// thread's read cursor, which Slack's client sends with both.
    pub async fn subscriptions_thread_set(
        &self,
        channel: &ChannelId,
        thread_ts: &Ts,
        last_read: &Ts,
        subscribed: bool,
    ) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("thread_ts", thread_ts.to_string()),
            ("last_read", last_read.to_string()),
        ];
        let method = if subscribed {
            method::SUBSCRIPTIONS_THREAD_ADD
        } else {
            method::SUBSCRIPTIONS_THREAD_REMOVE
        };
        self.call_ok(method, params).await
    }

    /// `subscriptions.thread.mark`: update the read cursor of a single thread (`read=1` marks
    /// replies up to and including `ts` as read; moving `ts` earlier marks newer replies unread).
    pub async fn subscriptions_thread_mark(
        &self,
        channel: &ChannelId,
        thread_ts: &Ts,
        ts: &Ts,
    ) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("thread_ts", thread_ts.to_string()),
            ("ts", ts.to_string()),
            ("read", "1".to_owned()),
        ];
        self.call_ok(method::SUBSCRIPTIONS_THREAD_MARK, params)
            .await
    }

    /// `conversations.open`: opens (or finds) the DM with `users`, a comma-separated id list.
    /// `return_im` makes a single-user call answer one `channel` object.
    pub async fn conversations_open(&self, users: &str) -> Result<Conversation> {
        let params = vec![
            ("users", users.to_owned()),
            ("return_im", "true".to_owned()),
        ];
        let resp = self
            .call::<ChannelWire>(method::CONVERSATIONS_OPEN, params)
            .await?;
        Ok(resp.data.channel)
    }

    /// `conversations.join`: the signed-in user joins public `channel`; the answer carries
    /// the joined channel.
    pub async fn conversations_join(&self, channel: &ChannelId) -> Result<Conversation> {
        let params = vec![("channel", channel.to_string())];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_JOIN, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.setTopic`: replace `channel`'s topic; the answer carries the updated
    /// channel object.
    pub async fn conversations_set_topic(
        &self,
        channel: &ChannelId,
        topic: &str,
    ) -> Result<Conversation> {
        let params = vec![
            ("channel", channel.to_string()),
            ("topic", topic.to_owned()),
        ];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_SET_TOPIC, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.setPurpose`: replace `channel`'s description; the answer carries the
    /// updated channel object.
    pub async fn conversations_set_purpose(
        &self,
        channel: &ChannelId,
        purpose: &str,
    ) -> Result<Conversation> {
        let params = vec![
            ("channel", channel.to_string()),
            ("purpose", purpose.to_owned()),
        ];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_SET_PURPOSE, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.create`: create a new public or private channel.
    pub async fn conversations_create(&self, name: &str, is_private: bool) -> Result<Conversation> {
        let params = vec![
            ("name", name.to_owned()),
            (
                "is_private",
                if is_private { "true" } else { "false" }.to_owned(),
            ),
        ];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_CREATE, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.rename`: rename an existing channel.
    pub async fn conversations_rename(
        &self,
        channel: &ChannelId,
        name: &str,
    ) -> Result<Conversation> {
        let params = vec![("channel", channel.to_string()), ("name", name.to_owned())];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_RENAME, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.archive`: archive an active channel.
    pub async fn conversations_archive(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call_ok(method::CONVERSATIONS_ARCHIVE, params).await
    }

    /// `conversations.unarchive`: restore an archived channel.
    pub async fn conversations_unarchive(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call_ok(method::CONVERSATIONS_UNARCHIVE, params).await
    }

    /// `admin.conversations.delete`: permanently delete a channel (`F-CORE-053`; requires
    /// workspace admin privileges).
    pub async fn admin_conversations_delete(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel_id", channel.to_string())];
        self.call_ok(method::ADMIN_CONVERSATIONS_DELETE, params)
            .await
    }

    /// `conversations.convertToPrivate`: convert a public channel or group DM into a private
    /// channel (`F-CORE-049`, `F-CORE-072`).
    pub async fn conversations_convert_to_private(
        &self,
        channel: &ChannelId,
    ) -> Result<Conversation> {
        let params = vec![("channel", channel.to_string())];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_CONVERT_TO_PRIVATE, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.invite`: invite one or more users into `channel`.
    pub async fn conversations_invite(
        &self,
        channel: &ChannelId,
        users: &[UserId],
    ) -> Result<Conversation> {
        let joined = users
            .iter()
            .map(UserId::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let params = vec![("channel", channel.to_string()), ("users", joined)];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_INVITE, params)
            .await?
            .data
            .channel)
    }

    /// `team.info`: one workspace's record, by team id. Reaches the other organizations of a
    /// Slack Connect channel, which `client.userBoot` never carries.
    pub async fn team_info(&self, team: &TeamId) -> Result<Team> {
        let params = vec![("team", team.to_string())];
        Ok(self
            .call::<TeamWire>(method::TEAM_INFO, params)
            .await?
            .data
            .team)
    }

    /// `subscriptions.thread.getView`: the followed threads of the Threads view, newest
    /// activity first (the web client's `fetch_threads_state` / `priority_mode` parameters;
    /// `limit` is the page size, `has_more` says older threads exist).
    pub async fn subscriptions_thread_get_view(&self, limit: u32) -> Result<ThreadsView> {
        let params = vec![
            ("limit", limit.to_string()),
            ("fetch_threads_state", "true".to_owned()),
            ("priority_mode", "all".to_owned()),
        ];
        Ok(self
            .call(method::SUBSCRIPTIONS_THREAD_GET_VIEW, params)
            .await?
            .data)
    }
}
