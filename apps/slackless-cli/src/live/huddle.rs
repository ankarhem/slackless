//! `huddle` CLI commands (device discovery, link, invite, live Chime join, offline transcripts).

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::RoomJoinResponse;
use slackless_core::domain::huddles::{CaptionDeduper, resolve_segment_at_ms};
use slackless_core::export::format_utc_timestamp;
use slackless_core::huddle::{
    AudioDevices, CameraDeviceInfo, HuddleAudioConfig, HuddleControl, HuddleCredentials,
    HuddleDataEvent, HuddlePhase, HuddleReactionKind, HuddleSessionSnapshot, HuddleUpdate,
    ScreenSource, list_audio_devices, list_cameras, list_screen_sources, spawn_huddle_session,
};
use slackless_core::types::net::huddle_fallback_url;
use slackless_core::types::{ChannelId, TeamId, Ts, UserId};

use crate::archive::Archive;
use crate::cli::ExportFormat;
use crate::output;
use crate::{Ctx, fail};

use super::session::{api_fail, api_for, display_channel, resolve_channel, resolve_channel_with};

/// Options for `huddle join`.
pub struct HuddleJoinArgs {
    pub channel: String,
    pub muted: bool,
    pub duration: Option<u64>,
    pub input: Option<String>,
    pub output_dev: Option<String>,
    pub react: Option<String>,
    pub camera: bool,
    pub blur: bool,
    pub screen: Option<String>,
    pub transcribe: slackless_core::huddle::TranscriptionProvider,
}

/// Options for `huddle transcripts`.
pub struct HuddleTranscriptsArgs {
    pub channel: Option<String>,
    pub search: Option<String>,
    pub lang: Option<String>,
    pub call_id: Option<String>,
    pub format: Option<ExportFormat>,
    pub limit: usize,
}

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
    api_url
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| huddle_fallback_url(workspace.as_str(), channel.as_str()))
}

/// `huddle link <channel>`: fetch the shareable huddle join URL (`rooms.getLink`).
pub async fn run_huddle_link(ctx: &Ctx, flag: Option<&str>, channel_arg: &str) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, _label) = resolve_channel(&session.workspace, channel_arg);
    match session.api.rooms_get_link(&channel).await {
        Ok(url_opt) => {
            let url = fallback_huddle_link(&session.workspace, &channel, url_opt);
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
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&session.workspace, channel_arg);
    let user = UserId::from(user_arg.to_owned());
    match session.api.rooms_notify_member(&channel, &user).await {
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

fn non_blank_ts(ts: Option<Ts>) -> Option<Ts> {
    ts.filter(|t| !t.as_str().trim().is_empty())
}

pub(crate) fn build_huddle_credentials(
    workspace: &TeamId,
    channel: &ChannelId,
    joined: RoomJoinResponse,
) -> HuddleCredentials {
    let thread_root_ts = non_blank_ts(joined.huddle.thread_root_ts).or_else(|| {
        non_blank_ts(
            joined
                .canvas
                .as_ref()
                .and_then(|c| c.root_thread_ts.clone()),
        )
    });
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
        thread_root_ts,
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
    let json = serde_json::json!({
        "event": "state",
        "phase": snap.phase,
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
        "huddle {display}\t{:?}\ttx={} rx={} pongs={}{err_suffix}\n",
        snap.phase, snap.packets_sent, snap.packets_received, snap.pongs_sent
    );
    (json, human)
}

/// One `huddle join` data-channel event (reaction, caption, hand raise, ...).
pub(crate) fn format_huddle_data_event(
    display: &str,
    ev: &HuddleDataEvent,
) -> (serde_json::Value, String) {
    (
        serde_json::json!({ "event": "data", "detail": ev }),
        format!("huddle {display}\tdata\t{ev:?}\n"),
    )
}

/// `huddle join <channel> [--muted] [--duration N] [--input NAME] [--output NAME] [--react EMOJI] [--camera] [--blur] [--screen SOURCE_ID]`:
/// join a live native Chime audio/video huddle from the terminal and leave cleanly on Ctrl+C or timeout.
pub async fn run_huddle_join(
    ctx: &Ctx,
    flag: Option<&str>,
    args: HuddleJoinArgs,
) -> Result<ExitCode> {
    let provider = args.transcribe;
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    // Keep one RW Store handle open across the whole huddle session (creating parent dir + DB if needed).
    let mut archive = match Archive::open_or_create_rw(&session.workspace) {
        Ok(archive) => Some(archive),
        Err(err) => {
            eprintln!(
                "slackless: could not open the local archive of {} ({err:#}); huddle transcripts are not stored",
                session.workspace
            );
            None
        }
    };
    let (channel, label) =
        resolve_channel_with(archive.as_ref(), &session.workspace, &args.channel);
    let joined = match session.api.rooms_join(&channel, None, None).await {
        Ok(resp) => resp,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };

    let creds = build_huddle_credentials(&session.workspace, &channel, joined);
    let call_id = creds.call_id.clone();
    let thread_root_ts = creds.thread_root_ts.clone();
    let audio = HuddleAudioConfig {
        muted: args.muted,
        input_device: args.input,
        output_device: args.output_dev,
        blur_background: args.blur,
        transcription_provider: provider,
        ..Default::default()
    };

    let (upd_tx, mut upd_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = spawn_huddle_session(creds, audio, upd_tx);
    let display = display_channel(&channel, label.as_deref());

    let deadline = args
        .duration
        .map(|secs| tokio::time::Instant::now() + std::time::Duration::from_secs(secs));
    let mut exit_code = ExitCode::SUCCESS;
    let mut sent_connected_actions = false;
    let mut video_frames: u64 = 0;
    let mut deduper = CaptionDeduper::default();
    let mut transcript_store_failed = false;

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
                            if let Some(emoji) = &args.react {
                                handle.send(HuddleControl::SendReaction {
                                    emoji: emoji.clone(),
                                    kind: HuddleReactionKind::Standard,
                                    emoji_url: None,
                                });
                            }
                            if args.camera {
                                handle.send(HuddleControl::SetCamera {
                                    enabled: true,
                                    device_index: None,
                                    blur: args.blur,
                                });
                            }
                            if let Some(source_id) = &args.screen {
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
                        if let HuddleDataEvent::Caption {
                            ref user,
                            ref text,
                            is_partial,
                            at_ms,
                            ref lang,
                        } = ev
                            && let Some(trimmed) = deduper.accept(user, text, is_partial, at_ms)
                            && let Some(archive) = archive.as_mut()
                        {
                            let seg_at_ms = resolve_segment_at_ms(at_ms);
                            let seg = slackless_core::store::NewHuddleTranscriptSegment {
                                channel: &channel,
                                call_id: &call_id,
                                thread_root_ts: thread_root_ts.as_ref(),
                                speaker_id: Some(user),
                                lang: lang.as_deref(),
                                source: provider.source_tag(),
                                text: &trimmed,
                                at_ms: seg_at_ms,
                            };
                            if let Err(err) = archive
                                .store_mut()
                                .insert_huddle_transcript_segment(&session.workspace, &seg)
                                && !std::mem::replace(&mut transcript_store_failed, true)
                            {
                                eprintln!(
                                    "slackless: could not store huddle transcript lines ({err}); later failures are not reported"
                                );
                            }
                        }
                        let (json, human) = format_huddle_data_event(&display, &ev);
                        output::emit(ctx.json, &json, &human);
                    }
                    Some(HuddleUpdate::VideoFrame { tile, frame }) => {
                        video_frames += 1;
                        if video_frames == 1 || video_frames.is_multiple_of(30) {
                            output::emit(
                                ctx.json,
                                &serde_json::json!({
                                    "event": "video_frame",
                                    "tile": tile,
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

fn empty_transcripts_json(args: &HuddleTranscriptsArgs) -> serde_json::Value {
    if let Some(cid) = args.call_id.as_deref() {
        serde_json::json!({ "ok": true, "call_id": cid, "count": 0, "segments": [] })
    } else if args.search.is_some() || args.lang.is_some() {
        serde_json::json!({ "ok": true, "count": 0, "hits": [] })
    } else {
        serde_json::json!({ "ok": true, "count": 0, "sessions": [] })
    }
}

/// `huddle transcripts [channel] [--search QUERY] [--lang sv|en] [--call-id ID] [--format md|json] [--limit N]`:
/// offline query, full-text FTS5 search, and export over locally archived text-only huddle transcripts.
pub fn run_huddle_transcripts(ctx: &Ctx, args: HuddleTranscriptsArgs) -> Result<ExitCode> {
    let Some(workspace) = ctx.workspace().cloned() else {
        return Ok(output::empty_local(
            ctx,
            empty_transcripts_json(&args),
            "huddle transcripts",
        ));
    };
    let archive = match Archive::open(&workspace) {
        Ok(Some(archive)) => archive,
        Ok(None) => {
            if ctx.json {
                output::emit_json(&empty_transcripts_json(&args));
            } else {
                output::archive_hint(&workspace);
            }
            return Ok(ExitCode::SUCCESS);
        }
        Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
    };
    let channel_names = match archive.channel_names(&workspace) {
        Ok(m) => m,
        Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
    };
    let user_names = match archive.user_display_map(&workspace) {
        Ok(m) => m,
        Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
    };
    let resolved_channel = match args.channel.as_deref() {
        Some(arg) => match archive.resolve_channel(&workspace, arg) {
            Ok(Some(c)) => Some(c.id),
            Ok(None) => Some(crate::commands::raw_channel_id(arg)),
            Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
        },
        None => None,
    };
    let resolve_user = |u: Option<&UserId>| -> String {
        match u {
            Some(uid) => user_names
                .get(uid)
                .cloned()
                .unwrap_or_else(|| uid.to_string()),
            None => "unknown".to_owned(),
        }
    };
    let resolve_channel_label = |ch: &ChannelId| -> String {
        channel_names
            .get(ch.as_str())
            .cloned()
            .unwrap_or_else(|| ch.to_string())
    };
    let store = archive.store();

    // Mode 1: Full transcript for one specific huddle call_id.
    if let Some(cid) = args.call_id.as_deref() {
        let segments = match store.huddle_transcript_for_call(&workspace, cid) {
            Ok(s) => s,
            Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
        };
        let as_json = ctx.json || matches!(args.format, Some(ExportFormat::Json));
        if as_json {
            let json_rows: Vec<_> = segments
                .iter()
                .map(|seg| {
                    serde_json::json!({
                        "id": seg.id,
                        "call_id": seg.call_id,
                        "channel": seg.channel.as_str(),
                        "channel_label": resolve_channel_label(&seg.channel),
                        "speaker_id": seg.speaker_id.as_ref().map(|u| u.as_str()),
                        "speaker": resolve_user(seg.speaker_id.as_ref()),
                        "lang": seg.lang,
                        "source": seg.source,
                        "at_ms": seg.at_ms,
                        "text": seg.text,
                    })
                })
                .collect();
            output::emit_json(&serde_json::json!({
                "ok": true,
                "call_id": cid,
                "count": segments.len(),
                "segments": json_rows,
            }));
            return Ok(ExitCode::SUCCESS);
        }
        let mut human = String::new();
        if matches!(args.format, Some(ExportFormat::Md)) {
            let ch_label = segments
                .first()
                .map(|s| resolve_channel_label(&s.channel))
                .unwrap_or_else(|| cid.to_owned());
            human.push_str(&format!("# Huddle Transcript — {ch_label} (`{cid}`)\n\n"));
            for seg in &segments {
                let who = resolve_user(seg.speaker_id.as_ref());
                let stamp = format_utc_timestamp(seg.at_ms.div_euclid(1_000));
                let lang_tag = seg
                    .lang
                    .as_deref()
                    .map(|l| format!(" `[{l}]`"))
                    .unwrap_or_default();
                human.push_str(&format!("- **{who}** ({stamp}){lang_tag}: {}\n", seg.text));
            }
        } else {
            for seg in &segments {
                let who = resolve_user(seg.speaker_id.as_ref());
                let stamp = format_utc_timestamp(seg.at_ms.div_euclid(1_000));
                let lang_tag = seg.lang.as_deref().unwrap_or("-");
                human.push_str(&format!("{stamp}\t{who}\t[{lang_tag}]\t{}\n", seg.text));
            }
        }
        print!("{human}");
        return Ok(ExitCode::SUCCESS);
    }

    // Mode 2: Full-text search across huddle transcripts (or filtered by --lang).
    if args.search.is_some() || args.lang.is_some() {
        let query = args.search.as_deref().unwrap_or("");
        let hits = match store.search_huddle_transcripts(
            &workspace,
            query,
            resolved_channel.as_ref(),
            args.lang.as_deref(),
            args.limit,
        ) {
            Ok(h) => h,
            Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
        };
        let json_rows: Vec<_> = hits
            .iter()
            .map(|seg| {
                serde_json::json!({
                    "id": seg.id,
                    "call_id": seg.call_id,
                    "channel": seg.channel.as_str(),
                    "channel_label": resolve_channel_label(&seg.channel),
                    "speaker_id": seg.speaker_id.as_ref().map(|u| u.as_str()),
                    "speaker": resolve_user(seg.speaker_id.as_ref()),
                    "lang": seg.lang,
                    "at_ms": seg.at_ms,
                    "text": seg.text,
                })
            })
            .collect();
        let mut human = String::new();
        for seg in &hits {
            let who = resolve_user(seg.speaker_id.as_ref());
            let ch = resolve_channel_label(&seg.channel);
            let stamp = format_utc_timestamp(seg.at_ms.div_euclid(1_000));
            let lang_tag = seg.lang.as_deref().unwrap_or("-");
            human.push_str(&format!(
                "{stamp}\t{ch}\t{}\t{who}\t[{lang_tag}]\t{}\n",
                seg.call_id, seg.text
            ));
        }
        output::emit(
            ctx.json,
            &serde_json::json!({"ok": true, "count": hits.len(), "hits": json_rows}),
            &human,
        );
        return Ok(ExitCode::SUCCESS);
    }

    // Mode 3: List recent huddle transcript sessions.
    let sessions = match store.list_huddle_transcript_sessions(
        &workspace,
        resolved_channel.as_ref(),
        args.limit,
    ) {
        Ok(s) => s,
        Err(err) => return Ok(fail(ctx, "store_error", &err.to_string())),
    };
    let json_rows: Vec<_> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "call_id": s.call_id,
                "channel": s.channel.as_str(),
                "channel_label": resolve_channel_label(&s.channel),
                "thread_root_ts": s.thread_root_ts.as_ref().map(|t| t.as_str()),
                "started_at_ms": s.started_at_ms,
                "ended_at_ms": s.ended_at_ms,
                "segment_count": s.segment_count,
                "languages": s.languages,
                "preview": s.preview,
            })
        })
        .collect();
    let mut human = String::new();
    for s in &sessions {
        let ch = resolve_channel_label(&s.channel);
        let stamp = format_utc_timestamp(s.started_at_ms.div_euclid(1_000));
        let langs = if s.languages.is_empty() {
            "-".to_owned()
        } else {
            s.languages.join(",")
        };
        human.push_str(&format!(
            "{stamp}\t{ch}\t{}\t{} lines\t[{langs}]\t{}\n",
            s.call_id, s.segment_count, s.preview
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({"ok": true, "count": sessions.len(), "sessions": json_rows}),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}
