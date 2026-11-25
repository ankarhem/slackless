//! Native Amazon Chime huddle signaling and room lifecycle methods.

use super::*;

impl SlackApi {
    /// `rooms.join`: create or join a huddle in `channel_id` and obtain Amazon Chime (`free_willy`)
    /// signaling/media credentials (`docs/research/api-huddles.md`). Rejects legacy Screenhero
    /// rooms with [`ApiError::UnsupportedHuddleBackend`].
    pub async fn rooms_join(
        &self,
        channel_id: &ChannelId,
        thread_ts: Option<&Ts>,
        regions: Option<&str>,
    ) -> Result<RoomJoinResponse> {
        let mut params = vec![
            ("channel_id", channel_id.to_string()),
            ("multidevice", "true".to_owned()),
        ];
        if let Some(ts) = thread_ts {
            params.push(("thread_ts", ts.to_string()));
        }
        if let Some(region) = regions.filter(|r| !r.is_empty()) {
            params.push(("regions", region.to_owned()));
        }
        let wire: RoomsJoinWire = self.call(method::ROOMS_JOIN, params).await?.data;
        let call = wire.call.ok_or_else(|| {
            ApiError::UnsupportedHuddleBackend("rooms.join response missing call block".to_owned())
        })?;
        if let Some(ref backend) = call.huddle.media_backend_type
            && !backend.is_empty()
            && backend != "free_willy"
        {
            return Err(ApiError::UnsupportedHuddleBackend(backend.clone()));
        }
        let chime = call.free_willy.ok_or_else(|| {
            ApiError::UnsupportedHuddleBackend(
                "legacy non-Chime huddle backend (missing free_willy credentials)".to_owned(),
            )
        })?;
        if chime.meeting.media_placement.signaling_url.is_empty()
            || chime.attendee.join_token.is_empty()
        {
            return Err(ApiError::UnsupportedHuddleBackend(
                "incomplete Chime credentials (missing SignalingUrl or JoinToken)".to_owned(),
            ));
        }
        let call_id = if !call.call_id.is_empty() {
            call.call_id
        } else {
            call.huddle.id.clone()
        };
        Ok(RoomJoinResponse {
            call_id,
            chime,
            huddle: call.huddle,
            canvas: call.canvas,
        })
    }

    /// `rooms.notifyMember`: invite `user_id` into the active huddle in `channel_id`.
    pub async fn rooms_notify_member(
        &self,
        channel_id: &ChannelId,
        user_id: &UserId,
    ) -> Result<()> {
        let params = vec![
            ("channel_id", channel_id.to_string()),
            ("user_id", user_id.to_string()),
        ];
        self.call_ok(method::ROOMS_NOTIFY_MEMBER, params).await
    }

    /// `rooms.getLink`: fetch a shareable huddle link (falling back to the canonical web URL).
    pub async fn rooms_get_link(&self, channel_id: &ChannelId) -> Result<Option<String>> {
        let params = vec![("channel_id", channel_id.to_string())];
        let wire: RoomsGetLinkWire = self.call(method::ROOMS_GET_LINK, params).await?.data;
        Ok(wire
            .link
            .or(wire.huddle_link)
            .or(wire.url)
            .filter(|s| !s.is_empty()))
    }

    /// `screenhero.rooms.rename`: set or clear the live huddle's topic (`F-COLLAB-016`).
    pub async fn rooms_rename(&self, room_id: &str, room_name: &str) -> Result<()> {
        let params = vec![
            ("room", room_id.to_owned()),
            ("room_name", room_name.to_owned()),
        ];
        self.call_ok(method::SCREENHERO_ROOMS_RENAME, params).await
    }

    /// `rooms.startTranscription`: enable server-side AWS Transcribe live captions (`F-COLLAB-037`).
    pub async fn rooms_start_transcription(&self, call_id: &str) -> Result<()> {
        let params = vec![
            ("call_id", call_id.to_owned()),
            ("reason", "start-event-log".to_owned()),
        ];
        self.call_ok(method::ROOMS_START_TRANSCRIPTION, params)
            .await
    }

    /// `rooms.stopTranscription`: disable server-side AWS Transcribe live captions.
    pub async fn rooms_stop_transcription(&self, call_id: &str) -> Result<()> {
        let params = vec![
            ("call_id", call_id.to_owned()),
            ("reason", "stop-event-log".to_owned()),
        ];
        self.call_ok(method::ROOMS_STOP_TRANSCRIPTION, params).await
    }

    /// `rooms.inviteResponse`: decline or respond (`"decline"`, `"joining_soon"`) to a DM huddle invite.
    pub async fn rooms_invite_response(
        &self,
        channel_id: &ChannelId,
        room_id: &str,
        response: &str,
    ) -> Result<()> {
        let params = vec![
            ("channel_id", channel_id.to_string()),
            ("room_id", room_id.to_owned()),
            ("response", response.to_owned()),
            ("reason", "respond-to-huddle-invite".to_owned()),
        ];
        self.call_ok(method::ROOMS_INVITE_RESPONSE, params).await
    }

    /// `huddles.update`: update the huddle's theme/background id (`F-COLLAB-035`).
    pub async fn huddles_update_background(
        &self,
        huddle_id: &str,
        background_id: &str,
    ) -> Result<()> {
        let params = vec![
            ("huddle_id", huddle_id.to_owned()),
            ("background_id", background_id.to_owned()),
            ("reason", "canvas-background-update".to_owned()),
        ];
        self.call_ok(method::HUDDLES_UPDATE, params).await
    }
}
