//! Huddle pop-out stage, video tiles, drawing, captions, drawers, and popovers.
//!
//! Styled to match official Slack's Huddle window across Light, Dark, and tinted workspace themes:
//! theme-matched top title bar & bottom control bar (left: diagnostics + invite; center: mic,
//! camera, share, hand, reactions, background/theme, thread, more; right: red Leave button),
//! luminous rounded-square participant cards on the stage, and a crisp theme-matched side drawer.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{Sizable as _, h_flex, h_resizable, resizable_panel};
use gpui_kit::gpui::{
    Background, ClickEvent, Focusable as _, FontWeight, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, StatefulInteractiveElement as _, canvas,
};
use slackless_core::{CollabCommand, HuddleCommand, HuddleControlCommand};

use crate::actions::{self, ActionHandler};
use crate::messages::{message_row_linked, same_group};

use super::*;

mod controls;
mod draw;
mod drawer;
mod header;
mod popovers;
mod stage;

use draw::*;
#[derive(Clone, Copy)]
struct HuddlePalette {
    /// Top titlebar and bottom control bar chrome.
    chrome_bg: Hsla,
    chrome_fg: Hsla,
    chrome_muted: Hsla,
    chrome_border: Hsla,
    /// Neutral control button fill & hover on the chrome bar.
    ctrl_bg: Hsla,
    ctrl_hover: Hsla,
    /// Emphasized active control button (e.g., unmuted mic / open thread toggle, like Slack).
    ctrl_active_bg: Hsla,
    ctrl_active_fg: Hsla,
    /// Stage backdrop gradient and participant card surface.
    stage_bg: Background,
    stage_solid: Hsla,
    tile_bg: Hsla,
    tile_border: Hsla,
    tile_pill_bg: Hsla,
    tile_pill_fg: Hsla,
    /// Floating scrim overlays on top of video / stage.
    scrim_bg: Hsla,
    scrim_fg: Hsla,
    scrim_muted: Hsla,
    /// Right-hand companion drawer (Thread / Notes / Diagnostics).
    drawer_bg: Hsla,
    drawer_fg: Hsla,
    drawer_muted: Hsla,
    drawer_border: Hsla,
    drawer_surface: Hsla,
    drawer_hover: Hsla,
    /// Popovers anchored above the bottom bar.
    popover_bg: Hsla,
    popover_fg: Hsla,
    popover_muted: Hsla,
    popover_border: Hsla,
    popover_hover: Hsla,
    /// Semantic accents.
    primary: Hsla,
    primary_hover: Hsla,
    primary_fg: Hsla,
    danger: Hsla,
    danger_hover: Hsla,
    danger_fg: Hsla,
    success: Hsla,
    warning: Hsla,
    warning_fg: Hsla,
    link: Hsla,
}

impl HuddlePalette {
    fn for_theme(bg_id: Option<&str>, cx: &App) -> Self {
        let theme = cx.theme();
        let chrome_dark = crate::theme::sidebar_is_dark(cx);
        let stage = crate::theme::huddle_stage_tokens(bg_id, theme);

        let (
            chrome_bg,
            chrome_fg,
            chrome_muted,
            chrome_border,
            ctrl_bg,
            ctrl_hover,
            ctrl_active_bg,
            ctrl_active_fg,
        ) = if chrome_dark {
            let fg = theme.sidebar_foreground;
            (
                theme.title_bar,
                fg,
                fg.opacity(0.68),
                theme.sidebar_border,
                fg.opacity(0.12),
                fg.opacity(0.22),
                fg.opacity(0.92),
                theme.title_bar,
            )
        } else {
            // Light default Slack huddle chrome: cool pale sky tint matching `theme.title_bar`
            // blended gently toward soft mist, crisp dark icons, dark charcoal active pills!
            let bar = theme.title_bar.blend(theme.background.opacity(0.55));
            (
                bar,
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.foreground.opacity(0.08),
                theme.foreground.opacity(0.15),
                stage.light_ctrl_active_bg,
                stage.light_ctrl_active_fg,
            )
        };

        Self {
            chrome_bg,
            chrome_fg,
            chrome_muted,
            chrome_border,
            ctrl_bg,
            ctrl_hover,
            ctrl_active_bg,
            ctrl_active_fg,
            stage_bg: stage.stage_bg,
            stage_solid: stage.stage_solid,
            tile_bg: stage.tile_bg,
            tile_border: stage.tile_border,
            tile_pill_bg: stage.tile_pill_bg,
            tile_pill_fg: stage.tile_pill_fg,
            scrim_bg: stage.scrim_bg,
            scrim_fg: stage.scrim_fg,
            scrim_muted: stage.scrim_muted,
            drawer_bg: theme.background,
            drawer_fg: theme.foreground,
            drawer_muted: theme.muted_foreground,
            drawer_border: theme.border,
            drawer_surface: theme.muted,
            drawer_hover: theme.accent,
            popover_bg: theme.popover,
            popover_fg: theme.popover_foreground,
            popover_muted: theme.muted_foreground,
            popover_border: theme.border,
            popover_hover: theme.accent,
            primary: theme.primary,
            primary_hover: theme.primary_hover,
            primary_fg: theme.primary_foreground,
            danger: theme.danger,
            danger_hover: theme.danger_hover,
            danger_fg: theme.danger_foreground,
            success: theme.success,
            warning: theme.warning,
            warning_fg: theme.warning_foreground,
            link: theme.link,
        }
    }
}

fn parse_ts_secs(ts: &str) -> Option<f64> {
    ts.trim().parse::<f64>().ok()
}

fn send_huddle_draw(msg: slackless_core::huddle::HuddleDrawMessage) -> Command {
    HuddleControlCommand::SendDraw(msg).into()
}

impl PopoutWindow {
    fn huddle_state_entity(&self) -> Option<Entity<crate::huddle_state::HuddleState>> {
        match &self.kind {
            PopoutKind::Huddle { source, .. } => source.upgrade(),
            _ => None,
        }
    }

    fn close_huddle_popovers(&mut self) {
        self.huddle_popover = None;
    }

    fn toggle_huddle_popover(&mut self, popover: HuddlePopover) -> bool {
        if self.huddle_popover == Some(popover) {
            self.huddle_popover = None;
            false
        } else {
            self.huddle_popover = Some(popover);
            true
        }
    }

    fn close_share_modal(&mut self, cx: &mut Context<Self>) {
        if self.huddle_popover == Some(HuddlePopover::Share) {
            self.huddle_popover = None;
        }
        for (_, img) in self.huddle_screen_thumbs.drain() {
            cx.drop_image(img, None);
        }
    }

    fn open_audio_video_preferences(&mut self, cx: &mut Context<Self>) {
        self.close_huddle_popovers();
        if let Some(state) = self.huddle_state_entity() {
            state.update(cx, |s, cx| s.open_audio_settings(cx));
        }
        cx.notify();
    }

    fn refresh_share_thumbnails(&self, monitors_only: bool, cx: &mut Context<Self>) {
        use slackless_core::huddle::capture_screen_thumbnails;
        cx.spawn(async move |win, cx| {
            let frames = cx
                .background_executor()
                .spawn(async move { capture_screen_thumbnails(monitors_only, 360, 224) })
                .await;
            let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                if win.huddle_popover != Some(HuddlePopover::Share) {
                    return;
                }
                for (id, frame) in frames {
                    if let Some(img) = crate::huddle_state::frame_to_render_image(&frame)
                        && let Some(old) = win.huddle_screen_thumbs.insert(id, img)
                    {
                        cx.drop_image(old, None);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_selected_screen_share(&mut self, source_id: String, cx: &mut Context<Self>) {
        let share_audio = self.huddle_share_audio;
        (self.send)(
            HuddleControlCommand::SetScreenShare {
                source_id: Some(source_id.clone()),
                share_audio,
            }
            .into(),
        );
        if self.fixtures.sample
            && let Some(state) = self.huddle_state_entity()
        {
            state.update(cx, |s, cx| s.set_sharing_local(Some(source_id), cx));
        }
        self.close_share_modal(cx);
        cx.notify();
    }

    pub(super) fn render_huddle_window(
        &self,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{HuddlePhase, VideoTileKey};

        let bg_id = self
            .huddle_state
            .as_ref()
            .and_then(|s| s.session.background_id.as_deref());
        let pal = HuddlePalette::for_theme(bg_id, cx);

        let Some(state) = self.huddle_state.clone() else {
            return v_flex()
                .id("popout-huddle-ended")
                .size_full()
                .bg(pal.drawer_bg)
                .text_color(pal.drawer_fg)
                .child(
                    TitleBar::new()
                        .h(px(38.))
                        .bg(pal.chrome_bg)
                        .border_b_1()
                        .border_color(pal.chrome_border)
                        .child(
                            h_flex()
                                .flex_1()
                                .h_full()
                                .items_center()
                                .justify_center()
                                .gap_1p5()
                                .child(
                                    Icon::new(IconName::Headphones)
                                        .size(px(13.))
                                        .text_color(pal.chrome_muted),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(pal.chrome_fg)
                                        .child(self.title.clone()),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Headphones)
                                .size(px(32.))
                                .text_color(pal.drawer_muted),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(pal.drawer_muted)
                                .child("This huddle has ended."),
                        ),
                );
        };

        let snap = state.session.clone();
        let channel = snap.channel.clone();
        let mut people = state.participants.clone();
        if !people.contains(&self.fixtures.self_user) {
            people.push(self.fixtures.self_user.clone());
        }
        let (status_color, status_text) = match snap.phase {
            HuddlePhase::Joining | HuddlePhase::Connecting => {
                (pal.warning, "Connecting…".to_owned())
            }
            HuddlePhase::Connected => (
                pal.success,
                crate::huddle_state::format_elapsed(state.elapsed_secs),
            ),
            HuddlePhase::Reconnecting => (pal.warning, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (pal.chrome_muted, "Leaving…".to_owned()),
            HuddlePhase::Failed => (pal.danger, "Failed".to_owned()),
            HuddlePhase::Idle => (pal.chrome_muted, "Ended".to_owned()),
        };

        let header = self.render_huddle_titlebar(&snap, people.len(), &pal, cx);

        let has_screen_share =
            self.huddle_video
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|vs| {
                    vs.read(cx).frames().keys().any(|k| {
                        matches!(k, VideoTileKey::RemoteScreen(_) | VideoTileKey::LocalScreen)
                    })
                });

        let mut stage = v_flex()
            .id("popout-huddle-stage-container")
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(pal.stage_bg)
            .px_5()
            .py_4()
            .gap_3()
            .when(self.huddle_popover == Some(HuddlePopover::Music), |el| {
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_huddle_popovers();
                        cx.notify();
                    }),
                )
            });

        // Top-left compact floating status + topic pill (matches official Slack's top-left chip).
        stage = stage.child(
            h_flex()
                .absolute()
                .left(px(14.))
                .top(px(12.))
                .px_2p5()
                .py_1()
                .gap_2()
                .items_center()
                .rounded_lg()
                .bg(pal.tile_pill_bg)
                .border_1()
                .border_color(pal.drawer_border)
                .shadow_sm()
                .child(div().size(px(7.)).rounded_full().bg(status_color))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.tile_pill_fg)
                        .child(status_text),
                )
                .child(self.render_huddle_topic_bar(snap.topic.as_deref(), &pal, cx)),
        );

        // Top-right floating Lounge Music player pill + song-selector dropdown (matches Official
        // Slack's solo/lounge music banner that lets you switch between all 10 Slack tracks or stop).
        if let Some(active_song_key) = snap.jukebox_song.clone() {
            stage = stage.child(self.render_huddle_music_pill(&active_song_key, &pal, cx));
        }

        if snap.audio_only {
            let send_resume = self.send.clone();
            stage = stage.child(
                h_flex()
                    .mt(px(36.))
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .rounded_xl()
                    .bg(pal.popover_bg)
                    .border_1()
                    .border_color(pal.warning.opacity(0.65))
                    .shadow_sm()
                    .text_xs()
                    .text_color(pal.popover_fg)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(pal.warning),
                            )
                            .child("Audio-only mode — video paused to save bandwidth"),
                    )
                    .child(
                        div()
                            .id("popout-huddle-resume-video")
                            .cursor_pointer()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(pal.drawer_surface)
                            .hover(move |el| el.bg(pal.drawer_hover))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Resume video")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_resume)(HuddleControlCommand::SetAudioOnly(false).into());
                                if this.fixtures.sample
                                    && let Some(state) = this.huddle_state_entity()
                                {
                                    state.update(cx, |s, cx| s.set_audio_only_local(false, cx));
                                }
                            })),
                    ),
            );
        }

        if let Some(err) = snap.error.clone() {
            stage = stage.child(
                h_flex()
                    .mt(px(36.))
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .rounded_xl()
                    .bg(pal.popover_bg)
                    .border_1()
                    .border_color(pal.danger.opacity(0.65))
                    .shadow_sm()
                    .text_xs()
                    .text_color(pal.popover_fg)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(14.))
                            .text_color(pal.danger),
                    )
                    .child(err),
            );
        }

        if has_screen_share {
            if let Some(screen_stage) = self.render_huddle_stage(&state, &pal, cx) {
                stage = stage.child(screen_stage);
            }
            stage = stage.child(self.render_huddle_filmstrip(&state, &people, &pal, cx));
        } else {
            let has_bottom_overlay = state.session.captions_enabled
                || !state.captions.is_empty()
                || !state.reactions.is_empty();
            stage = stage.child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(has_bottom_overlay, |el| el.pb(px(48.)))
                    .items_center()
                    .justify_center()
                    .child(self.render_huddle_tiles(&state, &people, &pal, cx)),
            );
        }

        // Floating effect burst banner (top-center, never shifts stage layout)
        if let Some((uid, effect_name, _)) = &state.active_effect {
            let who = self.fixtures.user_name(uid).to_owned();
            let (glyph, label) = effect_label(effect_name);
            stage = stage.child(
                h_flex()
                    .absolute()
                    .top(px(12.))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(
                        h_flex()
                            .px_4()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .rounded_full()
                            .bg(pal.popover_bg)
                            .border_1()
                            .border_color(pal.success.opacity(0.65))
                            .shadow_xl()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.popover_fg)
                            .child(format!(
                                "{glyph} {glyph} {who} triggered {label}! {glyph} {glyph}"
                            )),
                    ),
            );
        }

        // Floating stacking reactions (bottom-left above dock)
        if let Some(reactions_el) = self.render_huddle_floating_reactions(&state, &pal) {
            stage = stage.child(reactions_el);
        }

        // Floating live captions subtitle overlay (bottom-center above dock)
        if let Some(captions_el) = self.render_huddle_floating_captions(&state, &pal) {
            stage = stage.child(captions_el);
        }

        let active_popover = self.render_huddle_active_popover(&snap, &channel, &people, &pal, cx);

        let body: gpui_kit::gpui::AnyElement = if let Some(drawer) = self.huddle_drawer {
            let drawer_el = self.render_huddle_drawer(
                drawer,
                &state,
                &pal,
                on_jump.clone(),
                on_open_canvas.clone(),
                cx,
            );
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    h_resizable("huddle-popout-drawer")
                        .with_handle_appearance(Rc::new(
                            |_: &gpui_kit::base::ResizeHandleContext,
                             _: &mut Window,
                             _: &mut App| {
                                Some(div().into_any_element())
                            },
                        ))
                        .child(resizable_panel().child(stage))
                        .child(
                            resizable_panel()
                                .size(px(340.))
                                .size_range(px(260.)..px(720.))
                                .fixed()
                                .child(drawer_el),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(stage)
                .into_any_element()
        };

        v_flex()
            .id("popout-huddle-root")
            .relative()
            .size_full()
            .bg(pal.stage_solid)
            .text_color(pal.drawer_fg)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && (this.huddle_topic_editing.take().is_some()
                            || this.huddle_popover.is_some()
                            || this.huddle_drawer.take().is_some())
                    {
                        this.close_share_modal(cx);
                        this.close_huddle_popovers();
                        cx.notify();
                    }
                }),
            )
            .child(header)
            .child(body)
            .child(self.render_huddle_controls(&snap, status_color, &pal, cx))
            .when_some(active_popover, |root, popover| {
                // Full-window transparent click-away backdrop so clicking anywhere outside any
                // huddle dropdown (audio, video, reactions, theme, invite, more) dismisses it.
                root.child(
                    div()
                        .id("popout-huddle-popover-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.close_huddle_popovers();
                                cx.notify();
                            }),
                        )
                        .child(
                            h_flex()
                                .absolute()
                                .bottom(px(68.))
                                .left_0()
                                .right_0()
                                .justify_center()
                                .px_4()
                                .child(
                                    div()
                                        .id("popout-huddle-popover-surface")
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .child(popover),
                                ),
                        ),
                )
            })
            .when(self.huddle_popover == Some(HuddlePopover::Share), |root| {
                root.child(self.render_huddle_share_modal(
                    snap.share_source_id.as_deref(),
                    &pal,
                    cx,
                ))
            })
    }
}
