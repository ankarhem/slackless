use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::HuddleUpdate;
use crate::av1::Av1Frames;
use crate::backend::{VideoDecoderHandle, platform_backend};
use crate::metrics::HuddlePerfCounters;
use crate::rtc::InboundVideoPacket;
use crate::signaling::RemoteVideoSource;
use crate::video::{H264VideoDecoder, VideoFrame, VideoTileKey};

pub(super) struct VideoPushResult {
    pub(super) frame: Option<VideoFrame>,
    pub(super) decode_err: Option<String>,
    pub(super) needs_keyframe: bool,
    pub(super) seq_gaps_delta: u64,
}

pub(super) enum RemoteVideoDecoder {
    Av1 {
        frames: Av1Frames,
        dec: Box<dyn VideoDecoderHandle>,
    },
    H264(H264VideoDecoder),
}

impl RemoteVideoDecoder {
    pub(super) fn for_mime(mime: &str) -> Option<Self> {
        if mime.eq_ignore_ascii_case("video/AV1") {
            platform_backend()
                .create_av1_decoder()
                .ok()
                .map(|dec| Self::Av1 {
                    frames: Av1Frames::default(),
                    dec,
                })
        } else if mime.eq_ignore_ascii_case("video/H264") {
            H264VideoDecoder::new().ok().map(Self::H264)
        } else {
            None
        }
    }

    pub(super) fn backend_name(&self) -> &'static str {
        match self {
            Self::Av1 { dec, .. } => dec.backend_name(),
            Self::H264(dec) => dec.backend_name(),
        }
    }

    pub(super) fn push(&mut self, pkt: &InboundVideoPacket) -> VideoPushResult {
        match self {
            Self::Av1 { frames, dec } => {
                let prev_gaps = frames.seq_gaps();
                let frame = frames
                    .push(pkt.seq, pkt.marker, &pkt.payload)
                    .and_then(|unit| dec.decode_unit(&unit));
                let seq_gaps_delta = frames.seq_gaps().saturating_sub(prev_gaps);
                let needs_keyframe = frames.take_needs_keyframe();
                VideoPushResult {
                    frame,
                    decode_err: None,
                    needs_keyframe,
                    seq_gaps_delta,
                }
            }
            Self::H264(dec) => {
                let prev_gaps = dec.seq_gaps();
                let frame = dec.push_rtp_packet(pkt.seq, &pkt.payload, pkt.marker);
                let seq_gaps_delta = dec.seq_gaps().saturating_sub(prev_gaps);
                let needs_keyframe = dec.take_needs_keyframe();
                VideoPushResult {
                    frame,
                    decode_err: None,
                    needs_keyframe,
                    seq_gaps_delta,
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct VideoRoutingTable {
    pub(super) ssrc_to_stream_id: HashMap<u32, u32>,
    pub(super) remote_sources: Vec<(String, RemoteVideoSource)>,
}

/// Spawn a lightweight dispatcher that routes inbound remote video RTP packets by `ssrc` to
/// parallel per-track decoder workers (H.264 VideoToolbox / AV1 VideoToolbox or `rav1d` /
/// OpenH264) so a heavy 1080p remote screen share never head-of-line-blocks other participants'
/// cameras and decoding scales smoothly across CPU cores.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_video_decoder_thread(
    inbound_video_rx: std::sync::mpsc::Receiver<Option<InboundVideoPacket>>,
    audio_only: Arc<AtomicBool>,
    clear_decoders: Arc<AtomicBool>,
    routing: Arc<Mutex<VideoRoutingTable>>,
    perf: Arc<HuddlePerfCounters>,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
    keyframe_req_tx: mpsc::UnboundedSender<u32>,
    codec_tx: mpsc::UnboundedSender<String>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("slackless-huddle-vdec".to_owned())
        .spawn(move || {
            struct TrackWorker {
                tx: std::sync::mpsc::SyncSender<InboundVideoPacket>,
                handle: std::thread::JoinHandle<()>,
            }
            let mut workers: HashMap<u32, TrackWorker> = HashMap::new();
            let mut unsupported_video_ssrcs: HashSet<u32> = HashSet::new();

            let stop_all = |workers: &mut HashMap<u32, TrackWorker>| {
                let drained: Vec<TrackWorker> = workers.drain().map(|(_, w)| w).collect();
                for w in drained {
                    drop(w.tx);
                    let _ = w.handle.join();
                }
            };

            while let Ok(Some(pkt)) = inbound_video_rx.recv() {
                if clear_decoders.swap(false, Ordering::Relaxed) {
                    stop_all(&mut workers);
                }
                if audio_only.load(Ordering::Relaxed) {
                    continue;
                }
                perf.record_rx_packet(pkt.payload.len() as u64);

                let ssrc = pkt.ssrc;
                if !workers.contains_key(&ssrc) && !unsupported_video_ssrcs.contains(&ssrc) {
                    if let Some(dec) = RemoteVideoDecoder::for_mime(&pkt.mime_type) {
                        let backend = dec.backend_name();
                        tracing::info!(
                            ssrc,
                            mid = %pkt.mid,
                            mime = %pkt.mime_type,
                            backend,
                            "initialized parallel per-track remote video decoder worker"
                        );
                        perf.set_rx_backend(backend);
                        let _ = codec_tx.send(pkt.mime_type.clone());
                        let _ = keyframe_req_tx.send(ssrc);

                        let (tx, rx) = std::sync::mpsc::sync_channel::<InboundVideoPacket>(256);
                        let w_routing = Arc::clone(&routing);
                        let w_perf = Arc::clone(&perf);
                        let w_updates = updates.clone();
                        let w_kf = keyframe_req_tx.clone();
                        if let Ok(handle) = std::thread::Builder::new()
                            .name(format!("slackless-vdec-{ssrc}"))
                            .spawn(move || {
                                run_single_track_decoder(
                                    dec, rx, w_routing, w_perf, w_updates, w_kf,
                                );
                            })
                        {
                            workers.insert(ssrc, TrackWorker { tx, handle });
                        }
                    } else {
                        unsupported_video_ssrcs.insert(ssrc);
                        tracing::warn!(
                            ssrc,
                            mid = %pkt.mid,
                            mime = %pkt.mime_type,
                            "no decoder available for inbound remote video codec"
                        );
                    }
                }

                if let Some(worker) = workers.get(&ssrc) {
                    // Non-blocking send: if one track's software decoder momentarily falls behind,
                    // drop only that track's overflow packet without stalling other tracks.
                    let _ = worker.tx.try_send(pkt);
                }
            }
            stop_all(&mut workers);
        })
        .ok()
}

fn run_single_track_decoder(
    mut decoder: RemoteVideoDecoder,
    rx: std::sync::mpsc::Receiver<InboundVideoPacket>,
    routing: Arc<Mutex<VideoRoutingTable>>,
    perf: Arc<HuddlePerfCounters>,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
    keyframe_req_tx: mpsc::UnboundedSender<u32>,
) {
    let mut decoded_first = false;
    let mut pending_keyframe_req = false;
    let mut last_keyframe_req = Instant::now();
    let mut last_decode_err_log: Option<Instant> = None;

    while let Ok(pkt) = rx.recv() {
        let t0 = Instant::now();
        let res = decoder.push(&pkt);
        if res.seq_gaps_delta > 0 {
            perf.record_rx_seq_gap(res.seq_gaps_delta);
        }
        if let Some(err) = &res.decode_err
            && last_decode_err_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
        {
            last_decode_err_log = Some(Instant::now());
            tracing::warn!(
                ssrc = pkt.ssrc,
                mid = %pkt.mid,
                mime = %pkt.mime_type,
                %err,
                "remote video decode error; requesting keyframe"
            );
        }
        if res.needs_keyframe || res.decode_err.is_some() {
            pending_keyframe_req = true;
        }
        if (pending_keyframe_req || (!decoded_first && pkt.marker))
            && last_keyframe_req.elapsed() >= Duration::from_millis(1500)
        {
            pending_keyframe_req = false;
            last_keyframe_req = Instant::now();
            let _ = keyframe_req_tx.send(pkt.ssrc);
        }
        if let Some(frame) = res.frame {
            let decode_us = t0.elapsed().as_micros() as u64;
            perf.record_rx_frame(frame.width, frame.height, decode_us);
            let tile = routing.lock().ok().and_then(|rt| {
                resolve_remote_video_tile(
                    pkt.ssrc,
                    &pkt.mid,
                    &rt.ssrc_to_stream_id,
                    &rt.remote_sources,
                )
            });
            if let Some(tile) = tile {
                if !decoded_first {
                    decoded_first = true;
                    tracing::info!(
                        ssrc = pkt.ssrc,
                        mid = %pkt.mid,
                        mime = %pkt.mime_type,
                        width = frame.width,
                        height = frame.height,
                        decode_us,
                        tile = ?tile,
                        "decoded first remote video frame"
                    );
                }
                let _ = updates.send(HuddleUpdate::VideoFrame { tile, frame });
            } else if last_decode_err_log.is_none_or(|t| t.elapsed() >= Duration::from_secs(3)) {
                last_decode_err_log = Some(Instant::now());
                tracing::warn!(
                    ssrc = pkt.ssrc,
                    mid = %pkt.mid,
                    "decoded remote video frame but could not map to a tile"
                );
            }
        }
    }
}

pub(super) fn resolve_remote_video_tile(
    ssrc: u32,
    mid: &str,
    ssrc_to_stream: &HashMap<u32, u32>,
    sources: &[(String, RemoteVideoSource)],
) -> Option<VideoTileKey> {
    if !mid.is_empty()
        && let Some((_, src)) = sources.iter().find(|(m, _)| m == mid)
    {
        return Some(if src.is_content {
            VideoTileKey::RemoteScreen(src.user_id.clone())
        } else {
            VideoTileKey::RemoteCamera(src.user_id.clone())
        });
    }
    if let Some(&sid) = ssrc_to_stream.get(&ssrc)
        && let Some((_, src)) = sources.iter().find(|(_, s)| s.stream_id == sid)
    {
        return Some(if src.is_content {
            VideoTileKey::RemoteScreen(src.user_id.clone())
        } else {
            VideoTileKey::RemoteCamera(src.user_id.clone())
        });
    }
    sources.first().map(|(_, src)| {
        if src.is_content {
            VideoTileKey::RemoteScreen(src.user_id.clone())
        } else {
            VideoTileKey::RemoteCamera(src.user_id.clone())
        }
    })
}

/// Compare two remote video source lists by stream identity, ignoring transient
/// `max_bitrate_kbps` updates from Chime's `INDEX` frames so bitrate fluctuations never trigger
/// disruptive WebRTC SDP renegotiations.
pub(super) fn same_remote_video_sources(
    cur: &[&RemoteVideoSource],
    next: &[RemoteVideoSource],
) -> bool {
    cur.len() == next.len()
        && cur.iter().zip(next).all(|(a, b)| {
            a.stream_id == b.stream_id
                && a.group_id == b.group_id
                && a.attendee_id == b.attendee_id
                && a.user_id == b.user_id
                && a.is_content == b.is_content
        })
}
