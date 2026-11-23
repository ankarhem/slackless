//! Persistent sidebar mini-dock for an active native Slack Huddle (`F-COLLAB-001..059`).
//!
//! Pinned at the bottom of the sidebar column so the user can switch channels, threads, DMs, or
//! Unreads without ever losing audio controls, active-speaker visibility, or one-click pop-out.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, RenderImage, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::huddle::{
    AudioDevices, HuddleAudioLevels, HuddleDataEvent, HuddleDrawMessage, HuddleDrawSegment,
    HuddlePhase, HuddleReactionKind, HuddleSessionSnapshot, VideoFrame, VideoTileKey,
    list_audio_devices,
};
use slackless_core::types::{ChannelId, UserId};

use crate::avatars::user_chip;
use crate::fixtures::Fixtures;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Convert a BGRA8888 [`VideoFrame`] into a GPU-ready GPUI [`RenderImage`].
pub fn frame_to_render_image(frame: &VideoFrame) -> Option<Arc<RenderImage>> {
    let rgba = image::RgbaImage::from_raw(frame.width, frame.height, (*frame.bgra).clone())?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

/// Format an elapsed duration in seconds as `MM:SS` or `H:MM:SS`.
pub fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Active fading drawing stroke on a shared screen (`PeerMessage.Draw`, `F-COLLAB-025`).
#[derive(Clone, Debug)]
pub struct DrawStrokeEntry {
    pub user: UserId,
    pub item_id: String,
    pub hue: i32,
    pub segments: Vec<HuddleDrawSegment>,
    pub updated_at_ms: u64,
}

/// Live colored pointer cursor on a shared screen (`PeerMessage.Draw` `POSITION`).
#[derive(Clone, Debug)]
pub struct DrawCursorEntry {
    pub item_id: String,
    pub hue: i32,
    pub x: f64,
    pub y: f64,
    pub updated_at_ms: u64,
}

/// Ephemeral huddle reaction / effect entry (`PeerMessage.Reacji`).
#[derive(Clone, Debug)]
pub struct ReactionEntry {
    pub user: UserId,
    pub emoji: String,
    pub at_ms: u64,
}

/// Snapshot consumed by `PopoutKind::Huddle` so the Pop-out Huddle Window stays in lock-step with
/// the sidebar mini-dock.
#[derive(Clone)]
pub struct HuddleViewState {
    pub session: HuddleSessionSnapshot,
    pub participants: Vec<UserId>,
    pub levels: HuddleAudioLevels,
    pub elapsed_secs: u64,
    pub video_frames: HashMap<VideoTileKey, Arc<RenderImage>>,
    pub raised_hands: HashSet<UserId>,
    pub sticky_emojis: HashMap<UserId, String>,
    pub reactions: Vec<ReactionEntry>,
    pub active_effect: Option<(UserId, String, u64)>,
    pub draw_strokes: Vec<DrawStrokeEntry>,
    pub draw_cursors: HashMap<UserId, DrawCursorEntry>,
    pub captions: Vec<(UserId, String, bool)>,
}

pub enum HuddleDockEvent {
    ToggleMute,
    ToggleCamera,
    ToggleScreenShare,
    Leave,
    PopOut,
    JumpToChannel(ChannelId),
    OpenThread,
    OpenCanvas(String),
    CopyLink(ChannelId),
    OpenBrowser(String),
    RetryJoin(ChannelId),
    OpenAudioSettings,
    SwitchInputDevice(Option<String>),
    SwitchOutputDevice(Option<String>),
}

pub struct HuddleDock {
    fixtures: Arc<Fixtures>,
    session: HuddleSessionSnapshot,
    participants: Vec<UserId>,
    levels: HuddleAudioLevels,
    elapsed_secs: u64,
    video_frames: HashMap<VideoTileKey, Arc<RenderImage>>,
    raised_hands: HashSet<UserId>,
    sticky_emojis: HashMap<UserId, String>,
    reactions: Vec<ReactionEntry>,
    active_effect: Option<(UserId, String, u64)>,
    draw_strokes: Vec<DrawStrokeEntry>,
    draw_cursors: HashMap<UserId, DrawCursorEntry>,
    user_hues: HashMap<UserId, i32>,
    captions: Vec<(UserId, String, bool)>,
    menu_open: bool,
    devices: AudioDevices,
}

impl EventEmitter<HuddleDockEvent> for HuddleDock {}

impl HuddleDock {
    pub fn new(
        fixtures: Arc<Fixtures>,
        session: HuddleSessionSnapshot,
        participants: Vec<UserId>,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            // 1-second timer to advance the elapsed duration clock & expire floating reactions.
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let keep = this
                        .update(cx, |dock: &mut HuddleDock, cx| {
                            let mut dirty = false;
                            if dock.session.started_at_secs > 0
                                && matches!(
                                    dock.session.phase,
                                    HuddlePhase::Connected | HuddlePhase::Reconnecting
                                )
                            {
                                dock.elapsed_secs =
                                    now_secs().saturating_sub(dock.session.started_at_secs);
                                dirty = true;
                            }
                            let now = now_ms();
                            let react_cutoff = now.saturating_sub(4_500);
                            let before = dock.reactions.len();
                            dock.reactions.retain(|r| r.at_ms >= react_cutoff);
                            if dock.reactions.len() != before {
                                dirty = true;
                            }
                            if dock
                                .active_effect
                                .as_ref()
                                .is_some_and(|(_, _, at)| *at < now.saturating_sub(3_500))
                            {
                                dock.active_effect = None;
                                dirty = true;
                            }
                            let stroke_cutoff = now.saturating_sub(6_000);
                            let strokes_before = dock.draw_strokes.len();
                            dock.draw_strokes
                                .retain(|s| s.updated_at_ms >= stroke_cutoff);
                            if dock.draw_strokes.len() != strokes_before {
                                dirty = true;
                            }
                            let cursor_cutoff = now.saturating_sub(3_000);
                            let cursors_before = dock.draw_cursors.len();
                            dock.draw_cursors
                                .retain(|_, c| c.updated_at_ms >= cursor_cutoff);
                            if dock.draw_cursors.len() != cursors_before {
                                dirty = true;
                            }
                            if dirty {
                                cx.notify();
                            }
                        })
                        .is_ok();
                    if !keep {
                        break;
                    }
                }
            })
            .detach();

            let started = session.started_at_secs;
            Self {
                fixtures,
                session,
                participants,
                levels: HuddleAudioLevels::default(),
                elapsed_secs: if started > 0 {
                    now_secs().saturating_sub(started)
                } else {
                    0
                },
                video_frames: HashMap::new(),
                raised_hands: HashSet::new(),
                sticky_emojis: HashMap::new(),
                reactions: Vec::new(),
                active_effect: None,
                draw_strokes: Vec::new(),
                draw_cursors: HashMap::new(),
                user_hues: HashMap::new(),
                captions: Vec::new(),
                menu_open: false,
                devices: AudioDevices::default(),
            }
        })
    }

    pub fn update_session(&mut self, session: HuddleSessionSnapshot, cx: &mut Context<Self>) {
        if session.started_at_secs > 0 {
            self.elapsed_secs = now_secs().saturating_sub(session.started_at_secs);
        }
        if session.hand_raised {
            self.raised_hands.insert(self.fixtures.self_user.clone());
        } else {
            self.raised_hands.remove(&self.fixtures.self_user);
        }
        self.session = session;
        cx.notify();
    }

    pub fn set_participants(&mut self, participants: Vec<UserId>, cx: &mut Context<Self>) {
        if self.participants != participants {
            self.participants = participants;
            cx.notify();
        }
    }

    pub fn update_levels(&mut self, levels: HuddleAudioLevels, cx: &mut Context<Self>) {
        self.levels = levels;
        cx.notify();
    }

    pub fn update_video_frame(
        &mut self,
        tile: VideoTileKey,
        frame: &VideoFrame,
        cx: &mut Context<Self>,
    ) {
        if let Some(img) = frame_to_render_image(frame) {
            if let Some(old) = self.video_frames.insert(tile, img) {
                cx.drop_image(old, None);
            }
            cx.notify();
        }
    }

    pub fn close_video_tile(&mut self, tile: &VideoTileKey, cx: &mut Context<Self>) {
        if let Some(old) = self.video_frames.remove(tile) {
            cx.drop_image(old, None);
            cx.notify();
        }
    }

    pub fn clear_video_frames(&mut self, cx: &mut Context<Self>) {
        for (_, old) in self.video_frames.drain() {
            cx.drop_image(old, None);
        }
    }

    pub fn apply_room_meta(
        &mut self,
        topic: Option<String>,
        background_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut dirty = false;
        if let Some(t) = topic {
            let next = (!t.trim().is_empty()).then_some(t);
            if self.session.topic != next {
                self.session.topic = next;
                dirty = true;
            }
        }
        if let Some(bg) = background_id {
            let next = (!bg.trim().is_empty()).then_some(bg);
            if self.session.background_id != next {
                self.session.background_id = next;
                dirty = true;
            }
        }
        if dirty {
            cx.notify();
        }
    }

    pub fn set_background_local(&mut self, bg: String, cx: &mut Context<Self>) {
        self.session.background_id = (!bg.trim().is_empty()).then_some(bg);
        cx.notify();
    }

    pub fn set_jukebox_local(&mut self, song: Option<String>, cx: &mut Context<Self>) {
        self.session.jukebox_song = song;
        cx.notify();
    }

    pub fn apply_data_event(&mut self, event: &HuddleDataEvent, cx: &mut Context<Self>) {
        match event {
            HuddleDataEvent::Reaction {
                user,
                emoji,
                kind,
                at_ms,
                ..
            } => {
                let ts = if *at_ms == 0 { now_ms() } else { *at_ms };
                if *kind == HuddleReactionKind::Effect {
                    self.active_effect = Some((user.clone(), emoji.clone(), ts));
                }
                self.reactions.push(ReactionEntry {
                    user: user.clone(),
                    emoji: emoji.clone(),
                    at_ms: ts,
                });
                if self.reactions.len() > 24 {
                    let excess = self.reactions.len() - 24;
                    self.reactions.drain(0..excess);
                }
            }
            HuddleDataEvent::HandRaise { user, raised } => {
                if *raised {
                    self.raised_hands.insert(user.clone());
                } else {
                    self.raised_hands.remove(user);
                }
                if *user == self.fixtures.self_user {
                    self.session.hand_raised = *raised;
                }
            }
            HuddleDataEvent::StickyEmoji { user, emoji } => {
                match emoji {
                    Some(em) if !em.is_empty() => {
                        if em == "raised_hand" || em == "✋" {
                            self.raised_hands.insert(user.clone());
                        }
                        self.sticky_emojis.insert(user.clone(), em.clone());
                    }
                    _ => {
                        self.sticky_emojis.remove(user);
                        self.raised_hands.remove(user);
                    }
                }
                if *user == self.fixtures.self_user {
                    self.session.sticky_emoji = emoji.clone();
                    self.session.hand_raised =
                        matches!(emoji.as_deref(), Some("raised_hand") | Some("✋"));
                }
            }
            HuddleDataEvent::Topic { topic, .. } => {
                self.session.topic = (!topic.is_empty()).then(|| topic.clone());
            }
            HuddleDataEvent::Caption {
                user,
                text,
                is_partial,
                ..
            } => {
                if !text.trim().is_empty() {
                    if let Some(last) = self.captions.last_mut() {
                        if last.0 == *user && last.2 {
                            last.1 = text.clone();
                            last.2 = *is_partial;
                            cx.notify();
                            return;
                        }
                    }
                    self.captions
                        .push((user.clone(), text.clone(), *is_partial));
                    if self.captions.len() > 4 {
                        let excess = self.captions.len() - 4;
                        self.captions.drain(0..excess);
                    }
                }
            }
            HuddleDataEvent::Draw { user, message, .. } => {
                let ts = now_ms();
                match message {
                    HuddleDrawMessage::ChangeHue { hue, .. } => {
                        self.user_hues.insert(user.clone(), *hue);
                    }
                    HuddleDrawMessage::New { item_id, segment } => {
                        let hue = *self.user_hues.get(user).unwrap_or(&210);
                        // Append to an active stroke from this user if fresh (<250ms), else start new.
                        let append_to_last = self.draw_strokes.last().is_some_and(|s| {
                            s.user == *user
                                && s.item_id == *item_id
                                && s.hue == hue
                                && ts.saturating_sub(s.updated_at_ms) < 250
                                && s.segments.len() < 120
                        });
                        if append_to_last {
                            if let Some(last) = self.draw_strokes.last_mut() {
                                last.segments.push(*segment);
                                last.updated_at_ms = ts;
                            }
                        } else {
                            self.draw_strokes.push(DrawStrokeEntry {
                                user: user.clone(),
                                item_id: item_id.clone(),
                                hue,
                                segments: vec![*segment],
                                updated_at_ms: ts,
                            });
                            if self.draw_strokes.len() > 64 {
                                let excess = self.draw_strokes.len() - 64;
                                self.draw_strokes.drain(0..excess);
                            }
                        }
                    }
                    HuddleDrawMessage::Edit { item_id, segment } => {
                        if let Some(stroke) = self
                            .draw_strokes
                            .iter_mut()
                            .rev()
                            .find(|s| s.user == *user && s.item_id == *item_id)
                        {
                            stroke.segments.push(*segment);
                            stroke.updated_at_ms = ts;
                        }
                    }
                    HuddleDrawMessage::End { .. } => {}
                    HuddleDrawMessage::Position { item_id, x, y } => {
                        let hue = *self.user_hues.get(user).unwrap_or(&210);
                        self.draw_cursors.insert(
                            user.clone(),
                            DrawCursorEntry {
                                item_id: item_id.clone(),
                                hue,
                                x: *x,
                                y: *y,
                                updated_at_ms: ts,
                            },
                        );
                    }
                }
            }
            HuddleDataEvent::Jukebox { song_key, .. } => {
                self.session.jukebox_song = song_key.clone();
            }
            HuddleDataEvent::MutedByPeer { .. } => {
                self.session.muted = true;
            }
        }
        cx.notify();
    }

    pub fn set_muted_local(&mut self, muted: bool, cx: &mut Context<Self>) {
        if self.session.muted != muted {
            self.session.muted = muted;
            cx.notify();
        }
    }

    pub fn set_camera_local(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.session.video != on {
            self.session.video = on;
            if !on {
                self.close_video_tile(&VideoTileKey::LocalCamera, cx);
            }
            cx.notify();
        }
    }

    pub fn set_blur_local(&mut self, blur: bool, cx: &mut Context<Self>) {
        if self.session.blur_background != blur {
            self.session.blur_background = blur;
            cx.notify();
        }
    }

    pub fn set_sharing_local(&mut self, source_id: Option<String>, cx: &mut Context<Self>) {
        self.session.sharing = source_id.is_some();
        self.session.share_source_id = source_id;
        if !self.session.sharing {
            self.close_video_tile(&VideoTileKey::LocalScreen, cx);
        }
        cx.notify();
    }

    pub fn set_captions_local(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.session.captions_enabled != enabled {
            self.session.captions_enabled = enabled;
            cx.notify();
        }
    }

    pub fn session(&self) -> &HuddleSessionSnapshot {
        &self.session
    }

    pub fn view_state(&self) -> HuddleViewState {
        HuddleViewState {
            session: self.session.clone(),
            participants: self.participants.clone(),
            levels: self.levels.clone(),
            elapsed_secs: self.elapsed_secs,
            video_frames: self.video_frames.clone(),
            raised_hands: self.raised_hands.clone(),
            sticky_emojis: self.sticky_emojis.clone(),
            reactions: self.reactions.clone(),
            active_effect: self.active_effect.clone(),
            draw_strokes: self.draw_strokes.clone(),
            draw_cursors: self.draw_cursors.clone(),
            captions: self.captions.clone(),
        }
    }

    fn channel_label(&self) -> String {
        self.fixtures
            .conversation(&self.session.channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.session.channel.to_string())
    }

    fn browser_url(&self) -> String {
        if let Some(link) = &self.session.huddle_link {
            return link.clone();
        }
        let team = self
            .fixtures
            .workspaces
            .first()
            .map(|t| t.id.to_string())
            .unwrap_or_default();
        format!(
            "https://app.slack.com/huddle/{team}/{}",
            self.session.channel
        )
    }

    fn all_people(&self) -> Vec<UserId> {
        let mut people = self.participants.clone();
        if !people.contains(&self.fixtures.self_user) {
            people.push(self.fixtures.self_user.clone());
        }
        people
    }

    fn is_speaking(&self, user: &UserId) -> bool {
        if *user == self.fixtures.self_user {
            !self.session.muted && self.levels.local_rms > 0.03
        } else {
            self.levels
                .active_speakers
                .iter()
                .any(|(uid, lvl)| uid == user && *lvl > 0.05)
        }
    }

    fn is_muted_user(&self, user: &UserId) -> bool {
        if *user == self.fixtures.self_user {
            self.session.muted
        } else {
            self.levels.muted_peers.contains(user)
        }
    }

    fn render_menu(
        &self,
        channel: &ChannelId,
        browser_url: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let theme = cx.theme();
        let (card_bg, border, muted_fg) = (theme.background, theme.border, theme.muted_foreground);
        let copy_ch = channel.clone();
        let canvas_id = self.session.canvas_file_id.clone();
        let active_in = self.session.input_device.clone();
        let active_out = self.session.output_device.clone();

        v_flex()
            .w_full()
            .p_2()
            .gap_1()
            .rounded_lg()
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .text_xs()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child("Microphone"),
            )
            .children(self.devices.inputs.iter().enumerate().map(|(ix, dev)| {
                let selected = active_in.as_deref() == Some(dev.name.as_str())
                    || (active_in.is_none() && dev.is_default);
                let dev_name = dev.name.clone();
                Button::new(("dock-in-dev", ix))
                    .ghost()
                    .with_size(Size::XSmall)
                    .label(if selected {
                        format!("✓ {}", dev.name)
                    } else {
                        dev.name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::SwitchInputDevice(Some(dev_name.clone())));
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .mt_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child("Speaker"),
            )
            .children(self.devices.outputs.iter().enumerate().map(|(ix, dev)| {
                let selected = active_out.as_deref() == Some(dev.name.as_str())
                    || (active_out.is_none() && dev.is_default);
                let dev_name = dev.name.clone();
                Button::new(("dock-out-dev", ix))
                    .ghost()
                    .with_size(Size::XSmall)
                    .label(if selected {
                        format!("✓ {}", dev.name)
                    } else {
                        dev.name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::SwitchOutputDevice(Some(dev_name.clone())));
                        cx.notify();
                    }))
            }))
            .child(div().h(px(1.)).my_1().bg(border))
            .child(
                Button::new("dock-copy-link")
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::Link)
                    .label("Copy huddle link")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::CopyLink(copy_ch.clone()));
                        cx.notify();
                    })),
            )
            .when_some(canvas_id, |m, fid| {
                m.child(
                    Button::new("dock-open-canvas")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::FileText)
                        .label("Open huddle notes")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu_open = false;
                            cx.emit(HuddleDockEvent::OpenCanvas(fid.clone()));
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("dock-audio-settings")
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::Settings)
                    .label("Audio & video preferences…")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenAudioSettings);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("dock-open-browser")
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::ExternalLink)
                    .label("Open huddle in browser")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenBrowser(browser_url.clone()));
                        cx.notify();
                    })),
            )
    }
    fn render_action_bar(&self, cx: &mut Context<Self>) -> gpui_kit::gpui::Div {
        let mic_button = {
            let btn = Button::new("huddle-dock-mic")
                .with_size(Size::Small)
                .icon(if self.session.muted {
                    IconName::MicOff
                } else {
                    IconName::Mic
                })
                .tooltip(if self.session.muted {
                    "Unmute microphone (⌘⇧Space)"
                } else {
                    "Mute microphone (⌘⇧Space)"
                })
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleMute)));
            if self.session.muted {
                btn.ghost()
            } else {
                btn.primary()
            }
        };
        let video_button = {
            let btn = Button::new("huddle-dock-video")
                .with_size(Size::Small)
                .icon(if self.session.video {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .tooltip(if self.session.video {
                    "Turn camera off"
                } else {
                    "Turn camera on"
                })
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(HuddleDockEvent::ToggleCamera);
                }));
            if self.session.video {
                btn.primary()
            } else {
                btn.ghost()
            }
        };
        let share_button = {
            let btn = Button::new("huddle-dock-share")
                .with_size(Size::Small)
                .icon(IconName::ScreenShare)
                .tooltip(if self.session.sharing {
                    "Stop screen sharing"
                } else {
                    "Share screen"
                })
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(HuddleDockEvent::ToggleScreenShare);
                }));
            if self.session.sharing {
                btn.primary()
            } else {
                btn.ghost()
            }
        };
        h_flex()
            .items_center()
            .gap_1()
            .child(mic_button)
            .child(video_button)
            .child(share_button)
            .child(
                Button::new("huddle-dock-thread")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(IconName::MessageSquare)
                    .tooltip("Huddle thread")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(HuddleDockEvent::OpenThread);
                    })),
            )
            .child(
                Button::new("huddle-dock-more")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(IconName::Ellipsis)
                    .tooltip("Audio devices & huddle options")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = !this.menu_open;
                        if this.menu_open {
                            cx.spawn(async move |dock, cx| {
                                let devices = cx
                                    .background_executor()
                                    .spawn(async move { list_audio_devices() })
                                    .await;
                                let _ = dock.update(cx, |dock: &mut HuddleDock, cx| {
                                    dock.devices = devices;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("huddle-dock-leave")
                    .danger()
                    .with_size(Size::Small)
                    .icon(IconName::PhoneOff)
                    .tooltip("Leave huddle")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(HuddleDockEvent::Leave);
                    })),
            )
    }
}

impl Render for HuddleDock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.sidebar;
        let card_bg = theme.background;
        let border = theme.border;
        let fg = theme.foreground;
        let muted_fg = theme.muted_foreground;
        let success = theme.success;
        let warning = theme.warning;
        let danger = theme.danger;

        let (status_color, status_text) = match self.session.phase {
            HuddlePhase::Joining | HuddlePhase::Connecting => (warning, "Connecting…".to_owned()),
            HuddlePhase::Connected => (success, format_elapsed(self.elapsed_secs)),
            HuddlePhase::Reconnecting => (warning, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (muted_fg, "Leaving…".to_owned()),
            HuddlePhase::Failed => (danger, "Audio failed".to_owned()),
            HuddlePhase::Idle => (muted_fg, "Ended".to_owned()),
        };

        let channel = self.session.channel.clone();
        let channel_label = self.channel_label();
        let browser_url = self.browser_url();
        let people = self.all_people();
        let visible_people: Vec<UserId> = people.iter().take(4).cloned().collect();
        let overflow_count = people.len().saturating_sub(visible_people.len());
        let local_bar_pct = if self.session.muted {
            0.0
        } else {
            (self.levels.local_rms * 3.5).clamp(0.0, 1.0)
        };

        let avatars = h_flex()
            .items_center()
            .gap_1p5()
            .children(visible_people.into_iter().map(|uid| {
                let speaking = self.is_speaking(&uid);
                let user_muted = self.is_muted_user(&uid);
                div()
                    .relative()
                    .p(px(1.5))
                    .rounded_md()
                    .border_2()
                    .border_color(if speaking {
                        success
                    } else {
                        border.opacity(0.4)
                    })
                    .child(user_chip(&self.fixtures, &uid, 24.))
                    .when(user_muted, |el| {
                        el.child(
                            div()
                                .absolute()
                                .right(px(-3.))
                                .bottom(px(-3.))
                                .size(px(13.))
                                .rounded_full()
                                .bg(card_bg)
                                .border_1()
                                .border_color(border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::MicOff).size(px(8.)).text_color(danger)),
                        )
                    })
            }))
            .when(overflow_count > 0, |row| {
                row.child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(border.opacity(0.35))
                        .text_xs()
                        .text_color(muted_fg)
                        .child(format!("+{overflow_count}")),
                )
            });

        let action_bar = self.render_action_bar(cx);

        let mut card = v_flex()
            .id("huddle-sidebar-dock")
            .w_full()
            .p_2()
            .gap_2()
            .bg(bg)
            .border_t_1()
            .border_color(border)
            .child(
                v_flex()
                    .w_full()
                    .p_2p5()
                    .gap_2()
                    .rounded_lg()
                    .bg(card_bg)
                    .border_1()
                    .border_color(status_color.opacity(0.6))
                    .text_color(fg)
                    // Header row: status icon, clickable channel name, elapsed timer, pop-out
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(14.))
                                    .text_color(status_color),
                            )
                            .child(
                                div()
                                    .id("huddle-dock-jump")
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .child(channel_label)
                                    .on_click(cx.listener({
                                        let ch = channel.clone();
                                        move |_, _, _, cx| {
                                            cx.emit(HuddleDockEvent::JumpToChannel(ch.clone()));
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(status_color)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(status_text),
                            )
                            .child(
                                Button::new("huddle-dock-popout")
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Pop out huddle window")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(HuddleDockEvent::PopOut);
                                    })),
                            ),
                    )
                    // Participant avatars + live local RMS level strip
                    .child(avatars)
                    .when(self.session.phase == HuddlePhase::Connected, |box_el| {
                        box_el.child(
                            div()
                                .w_full()
                                .h(px(3.))
                                .rounded_full()
                                .bg(border.opacity(0.4))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui_kit::gpui::relative(local_bar_pct))
                                        .rounded_full()
                                        .bg(success),
                                ),
                        )
                    })
                    // Error / UDP blocked callout with Retry + Open in browser
                    .when_some(self.session.error.clone(), |box_el, err_msg| {
                        let retry_ch = channel.clone();
                        let fallback_url = browser_url.clone();
                        box_el.child(
                            v_flex()
                                .p_2()
                                .gap_1p5()
                                .rounded_md()
                                .bg(danger.opacity(0.12))
                                .border_1()
                                .border_color(danger.opacity(0.4))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_start()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(12.))
                                                .text_color(danger),
                                        )
                                        .child(
                                            div().flex_1().text_xs().text_color(fg).child(err_msg),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new("huddle-dock-retry")
                                                .outline()
                                                .with_size(Size::XSmall)
                                                .icon(IconName::RefreshCw)
                                                .label("Retry")
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(HuddleDockEvent::RetryJoin(
                                                        retry_ch.clone(),
                                                    ));
                                                })),
                                        )
                                        .child(
                                            Button::new("huddle-dock-browser-fallback")
                                                .primary()
                                                .with_size(Size::XSmall)
                                                .icon(IconName::ExternalLink)
                                                .label("Open in browser")
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(HuddleDockEvent::OpenBrowser(
                                                        fallback_url.clone(),
                                                    ));
                                                })),
                                        ),
                                ),
                        )
                    })
                    // Action control bar: Mute, Camera, Share, Thread, More/Devices, Leave
                    .when_some(self.session.topic.clone(), |box_el, topic| {
                        box_el.child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(muted_fg)
                                .child(format!("Topic: {topic}")),
                        )
                    })
                    .child(action_bar),
            );

        if self.menu_open {
            card = card.child(self.render_menu(&channel, browser_url, cx));
        }

        card
    }
}
