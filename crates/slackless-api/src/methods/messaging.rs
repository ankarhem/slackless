//! Messaging, Block Kit actions/modals, reactions, users, DND, sections, and search.

use super::*;

impl SlackApi {
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

    /// `dnd.endDnd`: end the signed-in user's active do-not-disturb session (`F-SHELL-034`).
    pub async fn dnd_end_dnd(&self) -> Result<()> {
        self.call::<Empty>(method::DND_END_DND, Vec::new()).await?;
        Ok(())
    }

    /// `dnd.teamInfo`: batch DND status lookup for up to 50 users per request (`F-SHELL-035`).
    pub async fn dnd_team_info(&self, users: &[UserId]) -> Result<HashMap<UserId, DndStatus>> {
        #[derive(Deserialize, Default)]
        struct DndTeamWire {
            #[serde(default)]
            users: HashMap<UserId, DndStatus>,
        }
        let mut out = HashMap::new();
        for chunk in users.chunks(50) {
            if chunk.is_empty() {
                continue;
            }
            let csv = chunk
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let wire: DndTeamWire = self
                .call(method::DND_TEAM_INFO, vec![("users", csv)])
                .await?
                .data;
            out.extend(wire.users);
        }
        Ok(out)
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

    /// `usergroups.create`: create a workspace `@handle` user group (`F-COLLAB-1161`, `1162`).
    pub async fn usergroups_create(
        &self,
        name: &str,
        handle: &str,
        description: &str,
    ) -> Result<Usergroup> {
        #[derive(Deserialize)]
        struct Wire {
            usergroup: Usergroup,
        }
        let mut params = vec![("name", name.to_owned())];
        if !handle.trim().is_empty() {
            params.push(("handle", handle.trim().trim_start_matches('@').to_owned()));
        }
        if !description.trim().is_empty() {
            params.push(("description", description.trim().to_owned()));
        }
        Ok(self
            .call::<Wire>(method::USERGROUPS_CREATE, params)
            .await?
            .data
            .usergroup)
    }

    /// `usergroups.update`: update an existing `@handle` user group's name/handle/description.
    pub async fn usergroups_update(
        &self,
        usergroup: &str,
        name: &str,
        handle: &str,
        description: &str,
    ) -> Result<Usergroup> {
        #[derive(Deserialize)]
        struct Wire {
            usergroup: Usergroup,
        }
        let mut params = vec![
            ("usergroup", usergroup.to_owned()),
            ("name", name.to_owned()),
        ];
        if !handle.trim().is_empty() {
            params.push(("handle", handle.trim().trim_start_matches('@').to_owned()));
        }
        if !description.trim().is_empty() {
            params.push(("description", description.trim().to_owned()));
        }
        Ok(self
            .call::<Wire>(method::USERGROUPS_UPDATE, params)
            .await?
            .data
            .usergroup)
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
}
