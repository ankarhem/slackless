//! Controls that reach the other participants as Slack `PeerMessage`s on the data channel.

use super::Control;
use crate::peer_message::{
    SLACK_DATA_CHANNEL_TOPIC, build_draw_message, build_jukebox_message,
    build_mute_request_message, build_reacji_message, build_sticky_emoji_awareness,
};
use crate::proto::SdkSignalFrame;
use crate::session::io::{RtcController, SignalSink};
use crate::session::media::MediaDevices;
use crate::signaling::{build_data_message_frame, now_ms};
use crate::{HuddleDataEvent, HuddleDrawMessage, HuddleReactionKind, HuddleUpdate};

impl<R: RtcController, S: SignalSink, M: MediaDevices> Control<'_, '_, R, S, M> {
    pub(super) async fn send_reaction(
        &mut self,
        emoji: String,
        kind: HuddleReactionKind,
        emoji_url: Option<String>,
    ) {
        let attendee_id = &self.state.creds.attendee_id;
        let bytes = build_reacji_message(attendee_id, &emoji, kind, emoji_url.clone());
        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 4000);
        if let Err(err) = self.signal.send_frame(&frame).await {
            tracing::warn!(%err, %emoji, "failed to send reaction");
        }
        self.publish(HuddleUpdate::DataEvent(HuddleDataEvent::Reaction {
            user: self.state.self_user.clone(),
            emoji,
            kind,
            emoji_url,
            at_ms: now_ms(),
        }));
    }

    /// `PeerMessage` announcing our sticky emoji (`None` clears it).
    fn sticky_emoji_frame(&self, emoji: Option<&str>) -> SdkSignalFrame {
        let bytes = build_sticky_emoji_awareness(
            &self.state.creds.attendee_id,
            &self.state.creds.call_id,
            &self.state.self_user,
            emoji,
            now_ms() as i64,
        );
        build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 30_000)
    }

    pub(super) async fn set_hand_raised(&mut self, raised: bool) {
        self.snap.hand_raised = raised;
        let sticky = raised.then(|| "raised_hand".to_owned());
        self.snap.sticky_emoji = sticky.clone();
        let frame = self.sticky_emoji_frame(sticky.as_deref());
        if let Err(err) = self.signal.send_frame(&frame).await {
            tracing::warn!(%err, raised, "failed to send hand raise");
        }
        self.publish(HuddleUpdate::DataEvent(HuddleDataEvent::HandRaise {
            user: self.state.self_user.clone(),
            raised,
        }));
        self.publish_state();
    }

    pub(super) async fn set_sticky_emoji(&mut self, emoji: Option<String>) {
        self.snap.sticky_emoji = emoji.clone();
        self.snap.hand_raised = emoji.as_deref() == Some("raised_hand");
        let frame = self.sticky_emoji_frame(emoji.as_deref());
        if let Err(err) = self.signal.send_frame(&frame).await {
            tracing::warn!(%err, ?emoji, "failed to send sticky emoji");
        }
        self.publish(HuddleUpdate::DataEvent(HuddleDataEvent::StickyEmoji {
            user: self.state.self_user.clone(),
            emoji,
        }));
        self.publish_state();
    }

    pub(super) async fn send_draw(&mut self, draw_msg: HuddleDrawMessage) {
        let bytes = build_draw_message(&self.state.creds.attendee_id, &draw_msg);
        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
        if let Err(err) = self.signal.send_frame(&frame).await {
            tracing::warn!(%err, "failed to send draw message");
        }
        self.publish(HuddleUpdate::DataEvent(HuddleDataEvent::Draw {
            user: self.state.self_user.clone(),
            message: draw_msg,
            at_ms: now_ms(),
        }));
    }

    pub(super) async fn set_jukebox(&mut self, song_key: Option<String>) {
        let output_device = self.config.output_device.clone();
        self.state
            .set_jukebox_player(output_device, song_key.as_deref());
        self.snap.jukebox_song = song_key.clone();
        let bytes = build_jukebox_message(&self.state.creds.attendee_id, song_key.as_deref());
        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 15_000);
        if let Err(err) = self.signal.send_frame(&frame).await {
            tracing::warn!(%err, ?song_key, "failed to send jukebox message");
        }
        self.publish_state();
    }

    pub(super) async fn mute_peer(&mut self, peer_id: String) {
        let bytes = build_mute_request_message(&self.state.creds.attendee_id, &peer_id);
        let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
        if let Err(err) = self.signal.send_frame(&frame).await {
            tracing::warn!(%err, %peer_id, "failed to send mute request");
        }
    }
}
