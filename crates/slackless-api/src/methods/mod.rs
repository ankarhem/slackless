//! Typed endpoint methods. Method-name strings come from [`crate::types::endpoints::method`];
//! request params are built inline per method.
//!
//! Response structs model only what slackless uses; every field is defaulted, unknown keys are
//! kept in `extra` (or ignored), and lists use lenient element parsing.

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
    Empty, File, Message, RichTextBlock, Team, TeamId, Ts, User, UserId, UserPresence, Usergroup,
};
use crate::{Result, SlackApi};

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
        let _: serde_json::Value = self.call(method::AUTH_REVOKE, Vec::new()).await?.data;
        Ok(())
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
        let _: serde_json::Value = self.call(method::CONVERSATIONS_ADD_TAB, params).await?.data;
        Ok(())
    }

    /// `conversations.removeTab`: remove a tab from `channel` while keeping shared files (`F-CORE-061`).
    pub async fn conversations_remove_tab(&self, channel: &ChannelId, tab_id: &str) -> Result<()> {
        let params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("id", tab_id.to_owned()),
            ("keep_file_share", "true".to_owned()),
            ("reason", "remove-channel-tab".to_owned()),
        ];
        let _: serde_json::Value = self
            .call(method::CONVERSATIONS_REMOVE_TAB, params)
            .await?
            .data;
        Ok(())
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
        let _: serde_json::Value = self
            .call(method::CONVERSATIONS_MOVE_TAB, params)
            .await?
            .data;
        Ok(())
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
        self.call::<Empty>(method::APPS_HOME_DISPATCH_OPEN_EVENT, params)
            .await?;
        Ok(())
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

    /// `conversations.mark`: moves the read cursor to `ts` (an earlier ts marks unread).
    /// `conversations.leave`: the signed-in user leaves `channel`.
    pub async fn conversations_leave(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_LEAVE, params)
            .await?;
        Ok(())
    }

    pub async fn conversations_mark(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![("channel", channel.to_string()), ("ts", ts.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_MARK, params)
            .await?;
        Ok(())
    }

    /// `users.markAllRead`: every conversation and thread read (Slack's Shift+Esc).
    pub async fn users_mark_all_read(&self) -> Result<()> {
        self.call::<Empty>(method::USERS_MARK_ALL_READ, Vec::new())
            .await?;
        Ok(())
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
        self.call::<Empty>(method, params).await?;
        Ok(())
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
        self.call::<Empty>(method::SUBSCRIPTIONS_THREAD_MARK, params)
            .await?;
        Ok(())
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

    /// `conversations.setTopic`: replace `channel`'s topic; the answer carries the updated
    /// channel object.
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
        self.call::<Empty>(method::CONVERSATIONS_ARCHIVE, params)
            .await?;
        Ok(())
    }

    /// `conversations.unarchive`: restore an archived channel.
    pub async fn conversations_unarchive(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_UNARCHIVE, params)
            .await?;
        Ok(())
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

    /// `chat.postMessage` in the web client's wire format: `blocks` JSON, a fresh UUID
    /// `client_msg_id` for server-side dedup, `unfurl=[]`. Set `thread_ts` to reply in a thread;
    /// `reply_broadcast` also posts the reply to the channel.
    pub async fn chat_post_message(
        &self,
        channel: &ChannelId,
        content: &MessageContent,
        thread_ts: Option<&Ts>,
        reply_broadcast: bool,
    ) -> Result<PostedMessage> {
        let client_msg_id = uuid::Uuid::new_v4().to_string();
        let mut params = vec![
            ("channel", channel.to_string()),
            ("type", "message".to_owned()),
            ("client_msg_id", client_msg_id.clone()),
            ("unfurl", "[]".to_owned()),
            ("_x_reason", REASON_MESSAGE_SEND.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        content.push(&mut params)?;
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts.to_string()));
            if reply_broadcast {
                params.push(("reply_broadcast", "true".to_owned()));
            }
        }
        let mut posted = self
            .call::<PostedMessage>(method::CHAT_POST_MESSAGE, params)
            .await?
            .data;
        posted.client_msg_id = Some(client_msg_id);
        Ok(posted)
    }

    /// `chat.command` in the web client's wire format: the arguments as one `rich_text` block
    /// (omitted when there are none) and a fresh client token. Slack answers on the
    /// websocket: a posted message, an ephemeral (Giphy's preview) or a modal.
    pub async fn chat_command(
        &self,
        channel: &ChannelId,
        command: &str,
        args: &str,
        thread_ts: Option<&Ts>,
        team: &TeamId,
    ) -> Result<()> {
        let mut params = vec![
            ("command", command.to_owned()),
            ("disp", command.to_owned()),
        ];
        if !args.is_empty() {
            let blocks: [Block; 1] = [RichTextBlock::plain_text(args.to_owned()).into()];
            params.push(("blocks", serde_json::to_string(&blocks)?));
        }
        params.push(("channel", channel.to_string()));
        params.push(("client_token", client_token()));
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts.to_string()));
        }
        params.extend([
            ("team_id", team.to_string()),
            ("_x_reason", REASON_COMMAND.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ]);
        self.call::<Empty>(method::CHAT_COMMAND, params).await?;
        Ok(())
    }

    /// `blocks.actions`: a Block Kit interaction dispatched to the app (`bot`, `app_id`) that
    /// posted the message. `action` is the element as the message declared it plus its
    /// `block_id`. Slack answers on the websocket: the message changed or deleted, a message
    /// posted.
    pub async fn blocks_actions(
        &self,
        bot: &BotId,
        app_id: &str,
        team: &TeamId,
        action: &Value,
        container: &ActionContainer,
    ) -> Result<()> {
        let params = vec![
            ("service_id", bot.to_string()),
            ("app_id", app_id.to_owned()),
            ("service_team_id", team.to_string()),
            ("actions", serde_json::to_string(&[action])?),
            ("container", container.json().to_string()),
            ("client_token", client_token()),
            ("state", r#"{"values":{}}"#.to_owned()),
            ("_x_reason", REASON_BLOCK_ACTION.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::BLOCKS_ACTIONS, params).await?;
        Ok(())
    }

    /// `blocks.suggestions`: fetch dynamic option suggestions for `external_select` /
    /// `multi_external_select` elements (`F-COLLAB-786`).
    #[allow(clippy::too_many_arguments)]
    pub async fn blocks_suggestions(
        &self,
        bot: &BotId,
        app_id: &str,
        team: &TeamId,
        action_id: &str,
        block_id: &str,
        value: &str,
        container: &ActionContainer,
    ) -> Result<Vec<(String, String)>> {
        let params = vec![
            ("service_id", bot.to_string()),
            ("app_id", app_id.to_owned()),
            ("service_team_id", team.to_string()),
            ("action_id", action_id.to_owned()),
            ("block_id", block_id.to_owned()),
            ("value", value.to_owned()),
            ("container", container.json().to_string()),
            ("client_token", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::BLOCKS_SUGGESTIONS, params).await?.data;
        let groups = raw["option_groups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| g["options"].as_array())
            .flatten();
        let options = raw["options"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(groups)
            .filter_map(|opt| {
                let text = opt["text"]["text"]
                    .as_str()
                    .or_else(|| opt["text"].as_str())?
                    .to_owned();
                let val = opt["value"].as_str()?.to_owned();
                Some((text, val))
            })
            .collect();
        Ok(options)
    }

    /// `views.submit`: submit a Block Kit modal view's `state.values` back to the app (`F-COLLAB-725`).
    pub async fn views_submit(
        &self,
        view_id: &str,
        hash: Option<&str>,
        state_values: &Value,
    ) -> Result<()> {
        let mut params = vec![
            ("view_id", view_id.to_owned()),
            (
                "state",
                serde_json::json!({ "values": state_values }).to_string(),
            ),
            ("client_token", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(hash) = hash {
            params.push(("hash", hash.to_owned()));
        }
        self.call::<Empty>(method::VIEWS_SUBMIT, params).await?;
        Ok(())
    }

    /// `views.close`: notify Slack that the user dismissed a Block Kit modal (`notify_on_close`).
    pub async fn views_close(&self, view_id: &str, hash: Option<&str>) -> Result<()> {
        let mut params = vec![
            ("view_id", view_id.to_owned()),
            ("client_token", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(hash) = hash {
            params.push(("hash", hash.to_owned()));
        }
        self.call::<Empty>(method::VIEWS_CLOSE, params).await?;
        Ok(())
    }

    /// `bots.info`: a bot's profile (name, icons, app) for messages that carry only its id.
    pub async fn bots_info(&self, bot: &BotId) -> Result<BotProfile> {
        #[derive(Deserialize)]
        struct BotInfo {
            bot: BotProfile,
        }
        let params = vec![("bot", bot.to_string())];
        Ok(self
            .call::<BotInfo>(method::BOTS_INFO, params)
            .await?
            .data
            .bot)
    }

    /// `chat.update`: replaces the text and blocks of message `ts`.
    pub async fn chat_update(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        content: &MessageContent,
    ) -> Result<PostedMessage> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        content.push(&mut params)?;
        Ok(self.call(method::CHAT_UPDATE, params).await?.data)
    }

    /// `chat.delete`.
    pub async fn chat_delete(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::CHAT_DELETE, params).await?;
        Ok(())
    }

    /// `chat.scheduleMessage`: queue a message for delivery at unix timestamp `post_at`.
    pub async fn chat_schedule_message(
        &self,
        channel: &ChannelId,
        post_at: u64,
        content: &MessageContent,
        thread_ts: Option<&Ts>,
    ) -> Result<ScheduledMessageCreated> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("post_at", post_at.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts.to_string()));
        }
        content.push(&mut params)?;
        Ok(self.call(method::CHAT_SCHEDULE_MESSAGE, params).await?.data)
    }

    /// `chat.scheduledMessages.list`: pending scheduled messages (all channels when `None`).
    pub async fn chat_scheduled_messages_list(
        &self,
        channel: Option<&ChannelId>,
    ) -> Result<Vec<ScheduledMessage>> {
        let mut params = Vec::new();
        if let Some(channel) = channel {
            params.push(("channel", channel.to_string()));
        }
        Ok(self
            .call::<ScheduledMessagesList>(method::CHAT_SCHEDULED_MESSAGES_LIST, params)
            .await?
            .data
            .scheduled_messages)
    }

    /// `chat.deleteScheduledMessage`: cancel a pending scheduled message.
    pub async fn chat_delete_scheduled_message(
        &self,
        channel: &ChannelId,
        scheduled_message_id: &str,
    ) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("scheduled_message_id", scheduled_message_id.to_owned()),
        ];
        self.call::<Empty>(method::CHAT_DELETE_SCHEDULED_MESSAGE, params)
            .await?;
        Ok(())
    }

    /// `reactions.add`. `name` may include surrounding colons and a `::skin-tone-N` suffix.
    pub async fn reactions_add(&self, channel: &ChannelId, ts: &Ts, name: &str) -> Result<()> {
        self.reaction(method::REACTIONS_ADD, channel, ts, name)
            .await
    }

    /// `reactions.remove`.
    pub async fn reactions_remove(&self, channel: &ChannelId, ts: &Ts, name: &str) -> Result<()> {
        self.reaction(method::REACTIONS_REMOVE, channel, ts, name)
            .await
    }

    async fn reaction(&self, method: &str, channel: &ChannelId, ts: &Ts, name: &str) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
            ("name", name.trim_matches(':').to_owned()),
            ("_x_reason", REASON_REACTION.to_owned()),
        ];
        self.call::<Empty>(method, params).await?;
        Ok(())
    }

    /// `users.info`.
    pub async fn users_prefs_set_notifications(
        &self,
        channel: &ChannelId,
        name: &str,
        value: &str,
    ) -> Result<()> {
        let params = vec![
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("channel_id", channel.to_string()),
            // A per-channel override, as the web client sends it (`prefs-store`).
            ("global", "false".to_owned()),
            ("sync", "false".to_owned()),
        ];
        self.call::<Empty>(method::USERS_PREFS_SET_NOTIFICATIONS, params)
            .await?;
        Ok(())
    }

    pub async fn users_info(&self, user: &UserId) -> Result<User> {
        let params = vec![("user", user.to_string())];
        Ok(self
            .call::<UserWire>(method::USERS_INFO, params)
            .await?
            .data
            .user)
    }

    /// `users.profile.get`: one user's full profile including custom `fields` and `pronouns`.
    pub async fn users_profile_get(&self, user: &UserId) -> Result<slackless_types::Profile> {
        #[derive(Deserialize)]
        struct Wire {
            profile: slackless_types::Profile,
        }
        let params = vec![
            ("user", user.to_string()),
            ("include_labels", "true".to_owned()),
        ];
        Ok(self
            .call::<Wire>(method::USERS_PROFILE_GET, params)
            .await?
            .data
            .profile)
    }

    /// `team.profile.get`: workspace custom profile field definitions (`id` -> `label`).
    pub async fn team_profile_get(&self) -> Result<HashMap<String, String>> {
        #[derive(Deserialize)]
        struct FieldDef {
            id: String,
            #[serde(default)]
            label: String,
        }
        #[derive(Deserialize)]
        struct ProfileDef {
            #[serde(default, deserialize_with = "lenient_vec")]
            fields: Vec<FieldDef>,
        }
        #[derive(Deserialize)]
        struct Wire {
            profile: ProfileDef,
        }
        let wire = self
            .call::<Wire>(method::TEAM_PROFILE_GET, Vec::new())
            .await?
            .data;
        Ok(wire
            .profile
            .fields
            .into_iter()
            .filter(|f| !f.label.is_empty())
            .map(|f| (f.id, f.label))
            .collect())
    }

    /// `dnd.info`: one user's do-not-disturb schedule and snooze state. Read-only.
    pub async fn dnd_info(&self, user: &UserId) -> Result<DndStatus> {
        let params = vec![("user", user.to_string())];
        Ok(self.call(method::DND_INFO, params).await?.data)
    }

    /// `dnd.setSnooze`: pause the signed-in user's notifications for `num_minutes`.
    pub async fn dnd_set_snooze(&self, num_minutes: u32) -> Result<DndStatus> {
        let params = vec![("num_minutes", num_minutes.to_string())];
        Ok(self.call(method::DND_SET_SNOOZE, params).await?.data)
    }

    /// `dnd.endSnooze`: resume the signed-in user's notifications immediately.
    pub async fn dnd_end_snooze(&self) -> Result<DndStatus> {
        Ok(self.call(method::DND_END_SNOOZE, Vec::new()).await?.data)
    }

    /// `users.getPresence`: one user's current presence. Read-only.
    pub async fn users_get_presence(&self, user: &UserId) -> Result<UserPresence> {
        let params = vec![("user", user.to_string())];
        Ok(self.call(method::USERS_GET_PRESENCE, params).await?.data)
    }

    /// One page of `users.list` (`limit` max 1000).
    pub async fn users_list(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> Result<Page<User>> {
        let mut params = Vec::new();
        if let Some(limit) = limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self.call::<MembersWire>(method::USERS_LIST, params).await?;
        Ok(page(resp, |w| w.members))
    }

    /// Every page of `users.list`.
    pub async fn users_list_all(&self) -> Result<Vec<User>> {
        paginate(|cursor| self.users_list(Some(1000), cursor))
            .try_collect()
            .await
    }

    /// `usergroups.list`: the workspace's active `@handle` user groups.
    pub async fn usergroups_list(&self) -> Result<Vec<Usergroup>> {
        let params = vec![
            ("include_count", "true".to_owned()),
            ("include_disabled", "false".to_owned()),
        ];
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "lenient_vec")]
            usergroups: Vec<Usergroup>,
        }
        Ok(self
            .call::<Wire>(method::USERGROUPS_LIST, params)
            .await?
            .data
            .usergroups)
    }

    /// Every page of `users.channelSections.list`: the user's sidebar sections, in the order
    /// Slack returns them (the store applies display order). Unlike most methods, pagination
    /// uses the top-level `cursor`, not `response_metadata.next_cursor`.
    pub async fn users_channel_sections_list(&self) -> Result<Vec<ChannelSection>> {
        let mut sections = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params = Vec::new();
            if let Some(cursor) = &cursor {
                params.push(("cursor", cursor.clone()));
            }
            let data = self
                .call::<ChannelSectionsWire>(method::USERS_CHANNEL_SECTIONS_LIST, params)
                .await?
                .data;
            for section in data.channel_sections.into_iter().map(ChannelSection::from) {
                merge_section(&mut sections, section);
            }
            let previous = cursor;
            // Stop on an empty cursor, or when Slack hands back the cursor it was just given
            // (the same loop guard `paginate` applies to `response_metadata` cursors).
            cursor = data
                .cursor
                .filter(|c| !c.is_empty() && Some(c) != previous.as_ref());
            if cursor.is_none() {
                break;
            }
        }
        Ok(sections)
    }

    /// `users.channelSections.create`: a sidebar section. `emoji` is a bare emoji name; the
    /// key is sent even when empty because Slack rejects a call without it
    /// (docs/research/api-channels.md).
    pub async fn users_channel_sections_create(
        &self,
        name: &str,
        emoji: &str,
    ) -> Result<SectionCreated> {
        let params = vec![("name", name.to_owned()), ("emoji", emoji.to_owned())];
        Ok(self
            .call(method::USERS_CHANNEL_SECTIONS_CREATE, params)
            .await?
            .data)
    }

    /// `users.channelSections.set`: renames a section. Every omitted key is CLEARED, so
    /// `name` and `emoji` must both be sent, with `emoji` read back from
    /// `users.channelSections.list` (`users.channelSections.update` / `.rename` answer
    /// `unknown_method`; docs/research/api-channels.md).
    pub async fn users_channel_sections_set(
        &self,
        channel_section_id: &str,
        name: &str,
        emoji: &str,
    ) -> Result<()> {
        let params = vec![
            ("channel_section_id", channel_section_id.to_owned()),
            ("name", name.to_owned()),
            ("emoji", emoji.to_owned()),
        ];
        self.call::<Empty>(method::USERS_CHANNEL_SECTIONS_SET, params)
            .await?;
        Ok(())
    }

    /// `users.channelSections.delete`: deletes a section; its conversations fall back to
    /// their default buckets.
    pub async fn users_channel_sections_delete(&self, channel_section_id: &str) -> Result<()> {
        let params = vec![("channel_section_id", channel_section_id.to_owned())];
        self.call::<Empty>(method::USERS_CHANNEL_SECTIONS_DELETE, params)
            .await?;
        Ok(())
    }

    /// `users.channelSections.channels.bulkUpdate`: moves channels between sections in one
    /// form-encoded call whose `insert` / `remove` values are JSON strings shaped
    /// `[{"channel_section_id":"L…","channel_ids":["C…"]}]` (docs/research/api-channels.md).
    /// A channel already in a section only moves when `remove` (its source) rides along with
    /// `insert` (its target); each side is omitted when empty, as the observed client does.
    pub async fn users_channel_sections_channels_bulk_update(
        &self,
        insert: Option<(&str, ChannelId)>,
        remove: Option<(&str, ChannelId)>,
    ) -> Result<()> {
        let side = |(section, channel): (&str, ChannelId)| {
            serde_json::to_string(&serde_json::json!([{
                "channel_section_id": section,
                "channel_ids": [channel.as_str()],
            }]))
            .unwrap_or_default()
        };
        let mut params = Vec::new();
        if let Some(insert) = insert {
            params.push(("insert", side(insert)));
        }
        if let Some(remove) = remove {
            params.push(("remove", side(remove)));
        }
        self.call::<Empty>(method::USERS_CHANNEL_SECTIONS_CHANNELS_BULK_UPDATE, params)
            .await?;
        Ok(())
    }

    /// `search.messages` with Slack's query modifier syntax (`in:`, `from:`, `before:`, ...).
    pub async fn search_messages(&self, query: &str, opts: &SearchOpts) -> Result<SearchMessages> {
        let mut params = vec![
            ("query", query.to_owned()),
            (
                "sort",
                match opts.sort {
                    SearchSort::Score => "score",
                    SearchSort::Timestamp => "timestamp",
                }
                .to_owned(),
            ),
            (
                "sort_dir",
                if opts.ascending { "asc" } else { "desc" }.to_owned(),
            ),
        ];
        if let Some(count) = opts.count {
            params.push(("count", count.to_string()));
        }
        if let Some(page) = opts.page {
            params.push(("page", page.to_string()));
        }
        if let Some(cursor) = &opts.cursor {
            params.push(("cursor", cursor.clone()));
        }
        if opts.highlight {
            params.push(("highlight", "true".to_owned()));
        }
        let resp = self
            .call::<SearchWire>(method::SEARCH_MESSAGES, params)
            .await?;
        Ok(resp.data.messages)
    }

    /// `emoji.list`: custom emoji name -> image URL or `alias:<name>`.
    pub async fn emoji_list(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .call::<EmojiWire>(method::EMOJI_LIST, Vec::new())
            .await?
            .data
            .emoji)
    }

    /// `pins.list`: a conversation's pinned messages and files, oldest pin first.
    pub async fn pins_list(&self, channel: &ChannelId) -> Result<PinsList> {
        let params = vec![("channel", channel.to_string())];
        Ok(self.call(method::PINS_LIST, params).await?.data)
    }

    /// `pins.add`: pin the message `ts` in `channel`.
    pub async fn pins_add(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
        ];
        self.call::<Empty>(method::PINS_ADD, params).await?;
        Ok(())
    }

    /// `pins.remove`: unpin the message `ts` in `channel`.
    pub async fn pins_remove(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
        ];
        self.call::<Empty>(method::PINS_REMOVE, params).await?;
        Ok(())
    }

    /// `bookmarks.list`: the conversation's bookmarks, folders included. Fetched lazily by
    /// the tab row (once per conversation, when a bookmarks surface is opened).
    pub async fn bookmarks_list(&self, channel: &ChannelId) -> Result<BookmarksList> {
        let params = vec![
            ("channel", channel.to_string()),
            ("include_folders", "true".to_owned()),
        ];
        Ok(self.call(method::BOOKMARKS_LIST, params).await?.data)
    }

    /// `bookmarks.add` of a `link` bookmark to the conversation's bar (or inside `parent_id`).
    pub async fn bookmarks_add_link(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
    ) -> Result<()> {
        self.bookmarks_add_link_in_folder(channel, title, link, None)
            .await
    }

    /// `bookmarks.add` of a `link` bookmark optionally placed inside a folder (`parent_id`).
    pub async fn bookmarks_add_link_in_folder(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
        parent_id: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.to_string()),
            ("title", title.to_owned()),
            ("type", "link".to_owned()),
            ("link", link.to_owned()),
        ];
        if let Some(parent) = parent_id.filter(|s| !s.is_empty()) {
            params.push(("parent_id", parent.to_owned()));
        }
        self.call::<Empty>(method::BOOKMARKS_ADD, params).await?;
        Ok(())
    }

    /// `bookmarks.remove`: delete a bookmark from `channel` (`F-CORE-062`).
    pub async fn bookmarks_remove(&self, channel: &ChannelId, bookmark_id: &str) -> Result<()> {
        let params = vec![
            ("channel_id", channel.to_string()),
            ("bookmark_id", bookmark_id.to_owned()),
        ];
        self.call::<Empty>(method::BOOKMARKS_REMOVE, params).await?;
        Ok(())
    }

    /// `drafts.list`: fetch the signed-in user's cloud-synced drafts (`F-CORE-089`).
    pub async fn drafts_list(&self) -> Result<Vec<CloudDraft>> {
        let params = vec![
            ("limit", "100".to_owned()),
            ("is_active", "true".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::DRAFTS_LIST, params).await?.data;
        Ok(parse_cloud_drafts(&raw))
    }

    /// `drafts.create`: create a cloud-synced draft for `channel` and return its draft id.
    pub async fn drafts_create(&self, channel: &ChannelId, text: &str) -> Result<Option<String>> {
        let blocks = serde_json::json!([{
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": [{ "type": "text", "text": text }]
            }]
        }]);
        let destinations = serde_json::json!([{ "channel_id": channel.as_str() }]);
        let params = vec![
            ("blocks", blocks.to_string()),
            ("destinations", destinations.to_string()),
            ("client_msg_id", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::DRAFTS_CREATE, params).await?.data;
        Ok(raw
            .get("draft")
            .and_then(|d| d.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// `drafts.update`: update an existing cloud draft's text.
    pub async fn drafts_update(
        &self,
        draft_id: &str,
        channel: &ChannelId,
        text: &str,
    ) -> Result<()> {
        let blocks = serde_json::json!([{
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": [{ "type": "text", "text": text }]
            }]
        }]);
        let destinations = serde_json::json!([{ "channel_id": channel.as_str() }]);
        let params = vec![
            ("draft_id", draft_id.to_owned()),
            ("blocks", blocks.to_string()),
            ("destinations", destinations.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::DRAFTS_UPDATE, params).await?;
        Ok(())
    }

    /// `drafts.delete`: remove a cloud draft once sent or cleared.
    pub async fn drafts_delete(&self, draft_id: &str) -> Result<()> {
        let params = vec![
            ("draft_id", draft_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::DRAFTS_DELETE, params).await?;
        Ok(())
    }

    /// One page of `files.list`: the files matching `filter`, newest first. A `channel`
    /// scopes the call to a conversation; without it Slack answers every file the
    /// signed-in user can see workspace-wide. `files.list` paginates by `page`, so a
    /// single large first page covers both the Files & links tab and the Files view.
    pub async fn files_list(
        &self,
        filter: &FilesFilter,
        count: u32,
        page_no: u32,
    ) -> Result<Page<File>> {
        let mut params = Vec::new();
        if let Some(channel) = &filter.channel {
            params.push(("channel", channel.to_string()));
        }
        if let Some(user) = &filter.user {
            params.push(("user", user.to_string()));
        }
        if let Some(types) = &filter.types {
            params.push(("types", types.clone()));
        }
        params.push(("count", count.to_string()));
        params.push(("page", page_no.to_string()));
        let resp = self.call::<FilesWire>(method::FILES_LIST, params).await?;
        Ok(page(resp, |w| w.files))
    }

    /// `files.getUploadURLExternal`: reserve an upload slot for `filename` of `length` bytes.
    pub async fn files_get_upload_url_external(
        &self,
        filename: &str,
        length: usize,
    ) -> Result<UploadSlot> {
        let params = vec![
            ("filename", filename.to_owned()),
            ("length", length.to_string()),
        ];
        Ok(self
            .call::<UploadSlot>(method::FILES_GET_UPLOAD_U_R_L_EXTERNAL, params)
            .await?
            .data)
    }

    /// `files.completeUploadExternal`: finalize uploaded file IDs and share them to `channel`
    /// (optionally inside `thread_ts` and accompanied by `content`).
    pub async fn files_complete_upload_external(
        &self,
        files: &[(String, String)],
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
        content: Option<&MessageContent>,
    ) -> Result<Vec<File>> {
        let entries: Vec<Value> = files
            .iter()
            .map(|(id, title)| serde_json::json!({ "id": id, "title": title }))
            .collect();
        let mut params = vec![
            ("files", serde_json::to_string(&entries)?),
            ("channel_id", channel.to_string()),
        ];
        if let Some(ts) = thread_ts {
            params.push(("thread_ts", ts.to_string()));
        }
        if let Some(content) = content {
            if !content.blocks.is_empty() {
                params.push(("blocks", serde_json::to_string(&content.blocks)?));
            } else if !content.text.trim().is_empty() {
                params.push(("initial_comment", content.text.clone()));
            }
        }
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "lenient_vec")]
            files: Vec<File>,
        }
        Ok(self
            .call::<Wire>(method::FILES_COMPLETE_UPLOAD_EXTERNAL, params)
            .await?
            .data
            .files)
    }

    /// Full external upload v2 flow: reserve a slot per file, POST its raw bytes, and finalize
    /// all files in one `files.completeUploadExternal` call into `channel`.
    pub async fn upload_files(
        &self,
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
        content: Option<&MessageContent>,
        files: &[UploadFile],
    ) -> Result<Vec<File>> {
        let mut completed = Vec::with_capacity(files.len());
        for file in files {
            if file.bytes.is_empty() {
                return Err(crate::ApiError::Slack("empty_file".to_owned()));
            }
            let slot = self
                .files_get_upload_url_external(&file.filename, file.bytes.len())
                .await?;
            self.post_upload_bytes(&slot.upload_url, &file.bytes)
                .await?;
            completed.push((slot.file_id, file.filename.clone()));
        }
        self.files_complete_upload_external(&completed, channel, thread_ts, content)
            .await
    }

    /// `conversations.canvases.create`: creates the conversation's empty channel canvas and
    /// returns its file id. Fails with `channel_canvas_already_exists` when one exists
    /// already (read the id from `conversations.info` -> `properties.canvas` then).
    pub async fn conversations_canvases_create(
        &self,
        channel: &ChannelId,
    ) -> Result<CanvasCreated> {
        let params = vec![("channel_id", channel.to_string())];
        Ok(self
            .call(method::CONVERSATIONS_CANVASES_CREATE, params)
            .await?
            .data)
    }

    /// `activity.feed`, page one (the view's first `limit` items, newest first; the next
    /// page's cursor rides in [`ActivityFeed::next_cursor`]). Row bodies arrive separately
    /// through [`SlackApi::messages_list`] when the feed itself carried none.
    pub async fn activity_feed(&self, limit: u32) -> Result<ActivityFeed> {
        let params = vec![
            ("types", ACTIVITY_TYPES.to_owned()),
            ("mode", "chrono_v1".to_owned()),
            ("limit", limit.to_string()),
            ("is_activity_inbox", "true".to_owned()),
        ];
        let resp = self
            .call::<ActivityFeedWire>(method::ACTIVITY_FEED, params)
            .await?;
        let next_cursor = resp.next_cursor().map(str::to_owned);
        Ok(ActivityFeed {
            items: resp.data.items.into_iter().filter_map(row).collect(),
            next_cursor,
        })
    }

    /// `messages.list`: the bodies of the given messages, as the web client fills the
    /// Activity feed's previews. At most [`MESSAGES_LIST_CHANNELS`] channels per call (a
    /// workspace that accepted 9 rejected 39), so wider asks are batched here.
    pub async fn messages_list(
        &self,
        ids: &[(ChannelId, Vec<Ts>)],
    ) -> Result<HashMap<ChannelId, Vec<Message>>> {
        let mut bodies = HashMap::new();
        for batch in ids.chunks(MESSAGES_LIST_CHANNELS) {
            let message_ids: Vec<Value> = batch
                .iter()
                .map(|(channel, stamps)| {
                    Value::Object(Map::from_iter([
                        (
                            "channel".to_owned(),
                            Value::String(channel.as_str().to_owned()),
                        ),
                        (
                            "timestamps".to_owned(),
                            Value::Array(
                                stamps
                                    .iter()
                                    .map(|ts| Value::String(ts.as_str().to_owned()))
                                    .collect(),
                            ),
                        ),
                    ]))
                })
                .collect();
            let params = vec![
                ("message_ids", serde_json::to_string(&message_ids)?),
                ("org_wide_aware", "true".to_owned()),
                ("cached_latest_updates", "{}".to_owned()),
            ];
            let resp = self
                .call::<MessagesListWire>(method::MESSAGES_LIST, params)
                .await?;
            for (channel, entry) in resp.data.messages_data {
                bodies.insert(ChannelId::from(channel.as_str()), entry.messages);
            }
        }
        Ok(bodies)
    }

    /// `activity.markRead`: marks feed entries read (each item's handles, see
    /// [`ActivityItem::entry`]).
    pub async fn activity_mark_read(&self, entries: &[ActivityEntry]) -> Result<()> {
        self.activity_mark(method::ACTIVITY_MARK_READ, entries)
            .await
    }

    /// `activity.markUnread`: marks feed entries unread.
    pub async fn activity_mark_unread(&self, entries: &[ActivityEntry]) -> Result<()> {
        self.activity_mark(method::ACTIVITY_MARK_UNREAD, entries)
            .await
    }

    async fn activity_mark(&self, method: &str, entries: &[ActivityEntry]) -> Result<()> {
        let params = vec![
            ("entries", serde_json::to_string(entries)?),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method, params).await?;
        Ok(())
    }
}

impl SlackApi {
    /// Featured/trending GIFs for Slack's emoji/GIF picker (`gif.featured`).
    pub async fn gif_featured(&self, limit: u32) -> Result<Vec<GifItem>> {
        let resp: ApiResponse<GifListData> = self
            .call(method::GIF_FEATURED, vec![("limit", limit.to_string())])
            .await?;
        Ok(parse_gif_list(resp.data))
    }

    /// Search GIFs for Slack's emoji/GIF picker (`gif.search`).
    pub async fn gif_search(&self, query: &str, limit: u32) -> Result<Vec<GifItem>> {
        let resp: ApiResponse<GifListData> = self
            .call(
                method::GIF_SEARCH,
                vec![("query", query.to_owned()), ("limit", limit.to_string())],
            )
            .await?;
        Ok(parse_gif_list(resp.data))
    }

    /// `saved.list`: items saved for Later under `filter` (`"saved"`, `"completed"`, `"archived"`).
    pub async fn saved_list(&self, filter: &str, limit: u32) -> Result<Vec<SavedItem>> {
        let resp: ApiResponse<SavedListWire> = self
            .call(
                method::SAVED_LIST,
                vec![
                    ("filter", filter.to_owned()),
                    ("limit", limit.to_string()),
                    ("include_tombstones", "true".to_owned()),
                    ("_x_reason", "saved-api/savedList".to_owned()),
                    ("_x_mode", MODE_ONLINE.to_owned()),
                ],
            )
            .await?;
        Ok(parse_saved_list(resp.data, filter))
    }

    /// `saved.add`: save a message for Later, optionally with a reminder due time (unix seconds).
    pub async fn saved_add(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        date_due: Option<u64>,
    ) -> Result<()> {
        let mut params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/addSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(due) = date_due {
            params.push(("date_due", due.to_string()));
        }
        self.call::<Empty>(method::SAVED_ADD, params).await?;
        Ok(())
    }

    /// `saved.add`: save a file for Later (`item_type=file`, `F-CORE-157`).
    pub async fn saved_add_file(&self, file_id: &str, date_due: Option<u64>) -> Result<()> {
        let mut params = vec![
            ("item_type", "file".to_owned()),
            ("item_id", file_id.to_owned()),
            ("_x_reason", "saved-api/addSavedFile".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(due) = date_due {
            params.push(("date_due", due.to_string()));
        }
        self.call::<Empty>(method::SAVED_ADD, params).await?;
        Ok(())
    }

    /// `files.info`: fetch metadata for one file or canvas (`F-COLLAB-125`).
    pub async fn files_info(&self, file_id: &str) -> Result<File> {
        #[derive(serde::Deserialize)]
        struct Resp {
            file: File,
        }
        let params = vec![("file", file_id.to_owned())];
        Ok(self
            .call::<Resp>(method::FILES_INFO, params)
            .await?
            .data
            .file)
    }

    /// `files.delete`: permanently delete a file/clip uploaded by the user (`F-CORE-130`).
    pub async fn files_delete(&self, file_id: &str) -> Result<()> {
        let params = vec![("file", file_id.to_owned())];
        self.call::<Empty>(method::FILES_DELETE, params).await?;
        Ok(())
    }

    /// Fetch a live Slack canvas's read-only HTML export (`files.info` -> `url_private` with
    /// session cookie, `F-COLLAB-125`). Returns `(title, html)`.
    pub async fn fetch_canvas_html(&self, file_id: &str) -> Result<(String, String)> {
        let file = self.files_info(file_id).await?;
        let title = file
            .title
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| file.name.clone().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "Canvas".to_owned());
        let url = file
            .url_private
            .as_deref()
            .or(file.url_private_download.as_deref())
            .ok_or_else(|| crate::ApiError::Slack("canvas_no_private_url".to_owned()))?;
        let bytes = self.download_file(url).await?;
        let html = String::from_utf8_lossy(&bytes).into_owned();
        Ok((title, html))
    }

    /// `saved.update`: transition a saved item's state (`mark`) and/or update or clear (`Some(None)`)
    /// its reminder due time (`date_due`).
    pub async fn saved_update(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        mark: Option<SavedMark>,
        date_due: Option<Option<u64>>,
    ) -> Result<()> {
        let mut params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/updateSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(mark) = mark {
            params.push(("mark", mark.as_str().to_owned()));
        }
        if let Some(due) = date_due {
            params.push(("date_due", due.unwrap_or(0).to_string()));
        }
        self.call::<Empty>(method::SAVED_UPDATE, params).await?;
        Ok(())
    }

    /// `saved.delete`: remove a message from Later.
    pub async fn saved_delete(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/deleteSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::SAVED_DELETE, params).await?;
        Ok(())
    }

    /// `saved.clearCompleted`: clear all completed Later items.
    pub async fn saved_clear_completed(&self) -> Result<()> {
        let params = vec![
            ("_x_reason", "manually_marked_all_completed".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::SAVED_CLEAR_COMPLETED, params)
            .await?;
        Ok(())
    }

    /// `client.appCommands`: workspace slash command catalog (`core`, `app`, `service`).
    pub async fn client_app_commands(&self) -> Result<Vec<AppCommand>> {
        let params = vec![
            ("_x_reason", "dispatch-client-extras".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let resp: ApiResponse<AppCommandsWire> =
            self.call(method::CLIENT_APP_COMMANDS, params).await?;
        Ok(parse_app_commands(resp.data))
    }

    /// Edge API `users/search`: fast typeahead lookup across workspace & org people for Cmd+K.
    pub async fn edge_users_search(
        &self,
        team_id: &TeamId,
        query: &str,
        count: u32,
    ) -> Result<Vec<User>> {
        let payload = serde_json::json!({
            "query": query,
            "count": count,
            "fuzz": 1,
            "include_profile_only_users": true,
            "enable_workspace_ranking": true,
            "search_email": true,
            "filter": "NOT deactivated",
            "default_workspace": team_id.as_str(),
        });
        let resp: ApiResponse<EdgeUsersWire> = self
            .call_edge_json(team_id.as_str(), "users/search", payload)
            .await?;
        Ok(resp.data.results)
    }

    /// Edge API `channels/search`: fast typeahead lookup across joined and unjoined channels for Cmd+K.
    pub async fn edge_channels_search(
        &self,
        team_id: &TeamId,
        query: &str,
        count: u32,
    ) -> Result<Vec<Conversation>> {
        let payload = serde_json::json!({
            "query": query,
            "count": count,
            "fuzz": 1,
            "filter": "xws",
            "include_record_channels": false,
            "check_membership": true,
            "default_workspace": team_id.as_str(),
        });
        let resp: ApiResponse<EdgeChannelsWire> = self
            .call_edge_json(team_id.as_str(), "channels/search", payload)
            .await?;
        let members: std::collections::HashSet<&ChannelId> =
            resp.data.member_channels.iter().collect();
        let mut results = resp.data.results;
        for ch in &mut results {
            if members.contains(&ch.id) {
                ch.is_member = true;
            }
        }
        Ok(results)
    }

    /// `search.modules.messages`: private web-client message search (`docs/research/api-search.md`).
    pub async fn search_modules_messages(
        &self,
        query: &str,
        sort: SearchSort,
        page: usize,
    ) -> Result<RemoteMessagePage> {
        let sort_key = match sort {
            SearchSort::Score => "score",
            SearchSort::Timestamp => "timestamp",
        };
        let params = vec![
            ("module", "messages".to_owned()),
            ("query", query.to_owned()),
            ("page", page.max(1).to_string()),
            ("count", "20".to_owned()),
            ("extracts", "1".to_owned()),
            ("highlight", "0".to_owned()),
            ("max_extract_len", "200".to_owned()),
            ("extra_message_data", "1".to_owned()),
            ("no_user_profile", "1".to_owned()),
            ("file_title_only", "false".to_owned()),
            ("query_rewrite_disabled", "false".to_owned()),
            ("include_files_shares", "1".to_owned()),
            ("search_context", "desktop_messages_tab".to_owned()),
            ("sort", sort_key.to_owned()),
            ("sort_dir", "desc".to_owned()),
            ("search_tab_filter", "messages".to_owned()),
            ("search_tab_sort", sort_key.to_owned()),
            ("_x_reason", "search-modules-messages".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
            ("_x_app_name", "client".to_owned()),
        ];
        let wire: SearchModulesMessagesWire =
            self.call("search.modules.messages", params).await?.data;
        Ok(parse_search_modules_messages(wire))
    }

    /// `search.modules.files`: private web-client file search (`docs/research/api-search.md`).
    pub async fn search_modules_files(
        &self,
        query: &str,
        sort: SearchSort,
        page: usize,
    ) -> Result<RemoteFilePage> {
        let sort_key = match sort {
            SearchSort::Score => "score",
            SearchSort::Timestamp => "timestamp",
        };
        let params = vec![
            ("module", "files".to_owned()),
            ("query", query.to_owned()),
            ("page", page.max(1).to_string()),
            ("count", "20".to_owned()),
            ("extracts", "1".to_owned()),
            ("highlight", "0".to_owned()),
            ("extra_message_data", "1".to_owned()),
            ("file_title_only", "false".to_owned()),
            ("query_rewrite_disabled", "false".to_owned()),
            ("include_files_shares", "1".to_owned()),
            ("search_context", "desktop_files_tab".to_owned()),
            ("sort", sort_key.to_owned()),
            ("sort_dir", "desc".to_owned()),
            ("search_tab_filter", "messages".to_owned()),
            ("search_tab_sort", sort_key.to_owned()),
            ("_x_reason", "search-modules-files".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
            ("_x_app_name", "client".to_owned()),
        ];
        let wire: SearchModulesFilesWire = self.call("search.modules.files", params).await?.data;
        Ok(parse_search_modules_files(wire))
    }

    /// `users.profile.set`: sparse update to the caller's profile or custom status.
    pub async fn users_profile_set(
        &self,
        update: &ProfileUpdate,
    ) -> Result<slackless_types::Profile> {
        let profile_json = serde_json::to_string(update)
            .map_err(|e| crate::ApiError::Slack(format!("invalid profile update payload: {e}")))?;
        let params = vec![
            ("profile", profile_json),
            ("_x_reason", "profile-edit".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let resp: ProfileSetWire = self.call("users.profile.set", params).await?.data;
        Ok(resp.profile)
    }

    /// `presence.set`: private web-client manual presence toggle (`"active"` or `"away"`).
    pub async fn presence_set(&self, away: bool) -> Result<()> {
        let presence = if away { "away" } else { "active" };
        let params = vec![
            ("presence", presence.to_owned()),
            ("_x_reason", "manual-presence".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>("presence.set", params).await?;
        Ok(())
    }

    /// `users.prefs.set`: write a single user preference key/value pair back to Slack.
    pub async fn users_prefs_set(&self, name: &str, value: &str) -> Result<()> {
        let params = vec![
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>("users.prefs.set", params).await?;
        Ok(())
    }

    /// `users.prefs.setNotifications` (`global=true`): write one global notification pref
    /// (`global_desktop`, `global_mobile`, `global_keywords`, `threads_everything`,
    /// `no_text_in_notifications`, `push_idle_wait`, etc.; `F-CORE-147`, `F-CORE-155`).
    pub async fn users_prefs_set_global_notification(
        &self,
        name: &str,
        value: &str,
        sync: bool,
    ) -> Result<()> {
        let params = vec![
            ("global", "true".to_owned()),
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("sync", sync.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::USERS_PREFS_SET_NOTIFICATIONS, params)
            .await?;
        Ok(())
    }

    /// Load a live Slack List (`files.info` for schema + `lists.records.list`, with public
    /// `slackLists.items.list` fallback) for read-only native table rendering.
    pub async fn fetch_list(&self, list_id: &str) -> Result<SlackListData> {
        let info_val: Value = self
            .call(
                method::FILES_INFO,
                vec![("file", list_id.to_owned()), ("count", "0".to_owned())],
            )
            .await?
            .data;
        let file_obj = info_val.get("file").unwrap_or(&info_val);
        let (mut title, mut description, mut columns) = parse_list_schema(file_obj);

        let private_res = self
            .call::<Value>(
                method::LISTS_RECORDS_LIST,
                vec![
                    ("list_id", list_id.to_owned()),
                    ("limit", "200".to_owned()),
                    ("archived", "false".to_owned()),
                ],
            )
            .await;

        let records_val = match private_res {
            Ok(resp) => resp.data,
            Err(_) => {
                self.call::<Value>(
                    method::SLACK_LISTS_ITEMS_LIST,
                    vec![
                        ("list_id", list_id.to_owned()),
                        ("limit", "200".to_owned()),
                        ("include_list", "true".to_owned()),
                    ],
                )
                .await?
                .data
            }
        };

        if columns.is_empty() {
            if let Some(list_obj) = records_val.get("list") {
                let (t, d, cols) = parse_list_schema(list_obj);
                if !t.is_empty() {
                    title = t;
                }
                if !d.is_empty() {
                    description = d;
                }
                columns = cols;
            }
        }

        let raw_records = records_val
            .get("records")
            .or_else(|| records_val.get("items"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let rows = parse_list_records(raw_records);

        Ok(SlackListData {
            file_id: list_id.to_owned(),
            title,
            description,
            columns,
            rows,
        })
    }

    /// `files.favorites.list`: starred file IDs, newest star first (`F-CORE-133`).
    pub async fn files_favorites_list(&self) -> Result<Vec<String>> {
        let wire: FileFavoritesWire = self
            .call(
                method::FILES_FAVORITES_LIST,
                vec![("type", "all".to_owned())],
            )
            .await?
            .data;
        let mut ids = wire.file_ids;
        ids.reverse();
        Ok(ids)
    }

    /// `files.favorites.add` / `.remove`: star/unstar a file or add/remove it from a collection.
    pub async fn files_favorites_set(
        &self,
        file_id: &str,
        collection_id: Option<&str>,
        starred: bool,
    ) -> Result<()> {
        let mut params = vec![
            ("file_id", file_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(cid) = collection_id.filter(|s| !s.is_empty()) {
            params.push(("collection_id", cid.to_owned()));
        }
        let endpoint = if starred {
            method::FILES_FAVORITES_ADD
        } else {
            method::FILES_FAVORITES_REMOVE
        };
        self.call::<Empty>(endpoint, params).await?;
        Ok(())
    }

    /// `files.collections.list`: custom Files sidebar sections (`F-CORE-133`).
    pub async fn files_collections_list(&self) -> Result<Vec<FileCollection>> {
        let wire: FileCollectionsListWire = self
            .call(method::FILES_COLLECTIONS_LIST, Vec::new())
            .await?
            .data;
        Ok(wire.collections.into_iter().map(Into::into).collect())
    }

    /// `files.collections.create`: create a custom Files sidebar section (`F-CORE-133`).
    pub async fn files_collections_create(
        &self,
        name: &str,
        emoji: Option<&str>,
    ) -> Result<Vec<FileCollection>> {
        let mut params = vec![
            ("name", name.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(e) = emoji.filter(|s| !s.is_empty()) {
            params.push(("emoji", e.to_owned()));
        }
        let wire: FileCollectionsListWire = self
            .call(method::FILES_COLLECTIONS_CREATE, params)
            .await?
            .data;
        Ok(wire.collections.into_iter().map(Into::into).collect())
    }

    /// `files.collections.delete`: delete a custom Files sidebar section (`F-CORE-133`).
    pub async fn files_collections_delete(&self, collection_id: &str) -> Result<()> {
        let params = vec![
            ("collection", collection_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::FILES_COLLECTIONS_DELETE, params)
            .await?;
        Ok(())
    }

    /// `files.recentlyDeleted`: canvases & lists deleted in the last 24h (`F-CORE-133`).
    pub async fn files_recently_deleted(&self) -> Result<Vec<DeletedFile>> {
        let wire: RecentlyDeletedWire = self
            .call(method::FILES_RECENTLY_DELETED, Vec::new())
            .await?
            .data;
        Ok(wire
            .files
            .into_iter()
            .filter(|f| !f.id.is_empty())
            .map(|f| DeletedFile {
                id: f.id,
                title: if f.title.is_empty() {
                    "Untitled".to_owned()
                } else {
                    f.title
                },
                date_delete: f.date_delete,
                file_type: f.file_type,
            })
            .collect())
    }

    /// `files.undelete`: restore a recently deleted canvas or list (`F-CORE-133`).
    pub async fn files_undelete(&self, file_id: &str) -> Result<()> {
        let params = vec![
            ("file_id", file_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::FILES_UNDELETE, params).await?;
        Ok(())
    }
}
