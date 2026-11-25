//! `huddle` CLI commands (device discovery, link, invite, live Chime join).

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::RoomJoinResponse;
use slackless_core::huddle::{
    AudioDevices, CameraDeviceInfo, HuddleAudioConfig, HuddleControl, HuddleCredentials,
    HuddlePhase, HuddleReactionKind, HuddleSessionSnapshot, HuddleUpdate, ScreenSource,
    list_audio_devices, list_cameras, list_screen_sources, spawn_huddle_session,
};
use slackless_core::types::{ChannelId, TeamId, UserId};

use crate::Ctx;
use crate::output;

use super::session::{api_fail, api_for, display_channel, resolve_channel};

pub(crate) fn format_huddle_devices(devices: &AudioDevices) -> (serde_json::Value, String) {
    let mut human = String::new();
    human.push_str("INPUTS\n");
    for d in &devices.inputs {
        let mark = if d.is_default { "*" } else { " " };
        human.push_str(&format!("{mark}\t{}\n", d.name));
    }
    human.push_str("OUTPUTS\n");
    for d in &devices.outputs {
        let mark = if d.is_default { "*" } else { " " };
        human.push_str(&format!("{mark}\t{}\n", d.name));
    }
    let json = serde_json::json!({
        "inputs": devices.inputs.iter().map(|d| serde_json::json!({"name": d.name, "default": d.is_default})).collect::<Vec<_>>(),
        "outputs": devices.outputs.iter().map(|d| serde_json::json!({"name": d.name, "default": d.is_default})).collect::<Vec<_>>(),
    });
    (json, human)
}

/// `huddle devices` (offline): list host input microphones and output speakers.
pub fn run_huddle_devices(ctx: &Ctx) -> Result<ExitCode> {
    let devices = list_audio_devices();
    let (json, human) = format_huddle_devices(&devices);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn format_huddle_cameras(cameras: &[CameraDeviceInfo]) -> (serde_json::Value, String) {
    let mut human = String::new();
    for c in cameras {
        human.push_str(&format!("{}\t{}\n", c.index, c.name));
    }
    let json = serde_json::json!({
        "cameras": cameras.iter().map(|c| serde_json::json!({"index": c.index, "name": c.name})).collect::<Vec<_>>(),
    });
    (json, human)
}

/// `huddle cameras` (offline): list available local cameras.
pub fn run_huddle_cameras(ctx: &Ctx) -> Result<ExitCode> {
    let cameras = list_cameras();
    let (json, human) = format_huddle_cameras(&cameras);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn format_huddle_screens(sources: &[ScreenSource]) -> (serde_json::Value, String) {
    let mut human = String::new();
    for s in sources {
        let kind = if s.is_monitor { "monitor" } else { "window" };
        human.push_str(&format!(
            "{}\t{kind}\t{}x{}\t{}\n",
            s.id, s.width, s.height, s.name
        ));
    }
    let json = serde_json::json!({
        "sources": sources.iter().map(|s| serde_json::json!({
            "id": s.id,
            "name": s.name,
            "monitor": s.is_monitor,
            "width": s.width,
            "height": s.height,
        })).collect::<Vec<_>>(),
    });
    (json, human)
}

/// `huddle screens` (offline): list monitors and windows available for screen sharing.
pub fn run_huddle_screens(ctx: &Ctx) -> Result<ExitCode> {
    let sources = list_screen_sources();
    let (json, human) = format_huddle_screens(&sources);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn fallback_huddle_link(
    workspace: &TeamId,
    channel: &ChannelId,
    api_url: Option<String>,
) -> String {
    api_url.unwrap_or_else(|| format!("https://app.slack.com/huddle/{workspace}/{channel}"))
}

/// `huddle link <channel>`: fetch the shareable huddle join URL (`rooms.getLink`).
pub async fn run_huddle_link(ctx: &Ctx, flag: Option<&str>, channel_arg: &str) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, _label) = resolve_channel(&workspace, channel_arg);
    match api.rooms_get_link(&channel).await {
        Ok(url_opt) => {
            let url = fallback_huddle_link(&workspace, &channel, url_opt);
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "url": url,
                }),
                &format!("{url}\n"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `huddle invite <channel> <user>`: ring/notify a workspace member to join (`rooms.notifyMember`).
pub async fn run_huddle_invite(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    user_arg: &str,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let user = UserId::from(user_arg.to_owned());
    match api.rooms_notify_member(&channel, &user).await {
        Ok(()) => {
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "user": user.as_str(),
                }),
                &format!(
                    "invited {} to huddle in {}\n",
                    user,
                    display_channel(&channel, label.as_deref())
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

pub(crate) fn build_huddle_credentials(
    workspace: &TeamId,
    channel: &ChannelId,
    joined: RoomJoinResponse,
) -> HuddleCredentials {
    HuddleCredentials {
        channel: channel.clone(),
        call_id: joined.call_id,
        meeting_id: joined.chime.meeting.meeting_id,
        attendee_id: joined.chime.attendee.attendee_id,
        external_user_id: joined.chime.attendee.external_user_id,
        join_token: joined.chime.attendee.join_token,
        signaling_url: joined.chime.meeting.media_placement.signaling_url,
        turn_control_url: joined.chime.meeting.media_placement.turn_control_url,
        audio_host_url: joined.chime.meeting.media_placement.audio_host_url,
        thread_root_ts: joined.huddle.thread_root_ts.or_else(|| {
            joined
                .canvas
                .as_ref()
                .and_then(|c| c.root_thread_ts.clone())
        }),
        canvas_file_id: joined.canvas.as_ref().and_then(|c| c.file_id.clone()),
        huddle_link: Some(fallback_huddle_link(
            workspace,
            channel,
            joined.huddle.huddle_link,
        )),
    }
}

pub(crate) fn format_huddle_state(
    display: &str,
    snap: &HuddleSessionSnapshot,
    include_error: bool,
) -> (serde_json::Value, String) {
    let phase = format!("{:?}", snap.phase);
    let json = serde_json::json!({
        "event": "state",
        "phase": phase,
        "channel": snap.channel.as_str(),
        "call_id": snap.call_id,
        "muted": snap.muted,
        "packets_sent": snap.packets_sent,
        "packets_received": snap.packets_received,
        "pongs_sent": snap.pongs_sent,
        "error": snap.error,
        "udp_blocked": snap.udp_blocked,
    });
    let err_suffix = if include_error {
        snap.error
            .as_deref()
            .map(|e| format!("\t{e}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let human = format!(
        "huddle {display}\t{phase}\ttx={} rx={} pongs={}{err_suffix}\n",
        snap.packets_sent, snap.packets_received, snap.pongs_sent
    );
    (json, human)
}

/// `huddle join <channel> [--muted] [--duration N] [--input NAME] [--output NAME] [--react EMOJI] [--camera] [--blur] [--screen SOURCE_ID]`:
/// join a live native Chime audio/video huddle from the terminal and leave cleanly on Ctrl+C or timeout.
#[allow(clippy::too_many_arguments)]
pub async fn run_huddle_join(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    muted: bool,
    duration: Option<u64>,
    input: Option<String>,
    output_dev: Option<String>,
    react: Option<String>,
    camera: bool,
    blur: bool,
    screen: Option<String>,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let joined = match api.rooms_join(&channel, None, None).await {
        Ok(resp) => resp,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };

    let creds = build_huddle_credentials(&workspace, &channel, joined);
    let audio = HuddleAudioConfig {
        muted,
        input_device: input,
        output_device: output_dev,
        blur_background: blur,
        ..Default::default()
    };

    let (upd_tx, mut upd_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn_huddle_session(creds, audio, upd_tx);
    let display = display_channel(&channel, label.as_deref());

    let deadline =
        duration.map(|secs| tokio::time::Instant::now() + std::time::Duration::from_secs(secs));
    let mut exit_code = ExitCode::SUCCESS;
    let mut sent_connected_actions = false;
    let mut video_frames: u64 = 0;

    loop {
        let timer_fut = async {
            if let Some(at) = deadline {
                tokio::time::sleep_until(at).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                handle.leave();
                break;
            }
            _ = timer_fut => {
                handle.leave();
                break;
            }
            upd = upd_rx.recv() => {
                match upd {
                    Some(HuddleUpdate::State(snap)) => {
                        let (json, human) = format_huddle_state(&display, &snap, true);
                        output::emit(ctx.json, &json, &human);
                        if snap.phase == HuddlePhase::Connected && !sent_connected_actions {
                            sent_connected_actions = true;
                            if let Some(emoji) = &react {
                                handle.send(HuddleControl::SendReaction {
                                    emoji: emoji.clone(),
                                    kind: HuddleReactionKind::Standard,
                                    emoji_url: None,
                                });
                            }
                            if camera {
                                handle.send(HuddleControl::SetCamera {
                                    enabled: true,
                                    device_index: None,
                                    blur,
                                });
                            }
                            if let Some(source_id) = &screen {
                                handle.send(HuddleControl::SetScreenShare {
                                    source_id: Some(source_id.clone()),
                                    share_audio: false,
                                });
                            }
                        }
                        if snap.phase == HuddlePhase::Failed {
                            exit_code = ExitCode::FAILURE;
                            break;
                        }
                        if snap.phase == HuddlePhase::Idle {
                            break;
                        }
                    }
                    Some(HuddleUpdate::DataEvent(ev)) => {
                        output::emit(
                            ctx.json,
                            &serde_json::json!({
                                "event": "data",
                                "detail": format!("{ev:?}"),
                            }),
                            &format!("huddle {display}\tdata\t{ev:?}\n"),
                        );
                    }
                    Some(HuddleUpdate::VideoFrame { tile, frame }) => {
                        video_frames += 1;
                        if video_frames == 1 || video_frames.is_multiple_of(30) {
                            output::emit(
                                ctx.json,
                                &serde_json::json!({
                                    "event": "video_frame",
                                    "tile": format!("{tile:?}"),
                                    "width": frame.width,
                                    "height": frame.height,
                                    "frames": video_frames,
                                }),
                                &format!(
                                    "huddle {display}\tvideo\t{tile:?}\t{}x{}\tframes={video_frames}\n",
                                    frame.width, frame.height
                                ),
                            );
                        }
                    }
                    Some(HuddleUpdate::AudioLevels(_) | HuddleUpdate::VideoTileClosed(_)) => {}
                    None => break,
                }
            }
        }
    }

    // Drain briefly so Chime receives the LEAVE frame before process exit, printing final stats.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(upd) = upd_rx.recv().await {
            if let HuddleUpdate::State(snap) = upd {
                let (json, human) = format_huddle_state(&display, &snap, false);
                output::emit(ctx.json, &json, &human);
                if snap.phase == HuddlePhase::Idle {
                    break;
                }
            }
        }
    })
    .await;

    Ok(exit_code)
}
