//! Multilingual (English + Swedish code-switching) huddle transcription helpers:
//!
//! 1. Lightweight EN/SV language detection (`detect_spoken_lang`) for spoken captions that lack
//!    explicit BCP-47 language tags.
//! 2. Pure-Rust 48 kHz -> 16 kHz energy-based VAD utterance segmenter (`VadSegmenter`) +
//!    in-memory WAV encoder (`encode_wav_16khz_mono`).
//! 3. Opt-in local `whisper-cli` (whisper.cpp) worker that transcribes per-utterance chunks with
//!    `-l auto` (auto-detecting English vs Swedish even when speakers switch languages mid-call),
//!    deletes each transient WAV immediately after inference (zero audio kept on disk), and emits
//!    standard [`HuddleDataEvent::Caption`] events.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, SyncSender};
use std::time::{SystemTime, UNIX_EPOCH};

use slackless_types::UserId;
use tokio::sync::mpsc as tokio_mpsc;

use crate::{HuddleDataEvent, HuddleUpdate};

/// Multilingual Whisper prompt priming both English and Swedish (plus common tech/code-switching).
const BILINGUAL_WHISPER_PROMPT: &str =
    "Bilingual English and Svenska (Swedish) workplace conversation.";

/// Whisper setup availability reported to Preferences -> Audio & video and CLI diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhisperSetupStatus {
    pub binary: Option<PathBuf>,
    pub model: Option<PathBuf>,
    pub default_model_dir: PathBuf,
}

impl WhisperSetupStatus {
    pub fn is_ready(&self) -> bool {
        self.binary.is_some() && self.model.is_some()
    }
}

/// Default directory where Slackless looks for optional GGML multilingual Whisper models
/// (`~/.local/share/slackless/whisper/` on Unix/macOS). Zero bytes are written unless the user
/// downloads a model themselves.
pub fn default_whisper_model_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("slackless").join("whisper");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("slackless")
            .join("whisper");
    }
    std::env::temp_dir().join("slackless-whisper")
}

/// Probe host paths for an installed `whisper-cli` binary and a multilingual GGML model file.
pub fn detect_whisper_setup(
    custom_bin: Option<&str>,
    custom_model: Option<&str>,
) -> WhisperSetupStatus {
    let default_model_dir = default_whisper_model_dir();
    let binary = find_whisper_binary(custom_bin);
    let model = find_whisper_model(custom_model, &default_model_dir);
    WhisperSetupStatus {
        binary,
        model,
        default_model_dir,
    }
}

fn find_whisper_binary(custom_bin: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = custom_bin.map(str::trim).filter(|s| !s.is_empty()) {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Ok(env_bin) = std::env::var("SLACKLESS_WHISPER_BIN") {
        let pb = PathBuf::from(env_bin.trim());
        if pb.is_file() {
            return Some(pb);
        }
    }
    let candidates = [
        "/opt/homebrew/bin/whisper-cli",
        "/usr/local/bin/whisper-cli",
        "/opt/homebrew/bin/whisper-cpp",
        "/usr/local/bin/whisper-cpp",
    ];
    for c in candidates {
        let pb = PathBuf::from(c);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in ["whisper-cli", "whisper-cpp"] {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

fn find_whisper_model(custom_model: Option<&str>, default_dir: &Path) -> Option<PathBuf> {
    if let Some(p) = custom_model.map(str::trim).filter(|s| !s.is_empty()) {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Ok(env_model) = std::env::var("SLACKLESS_WHISPER_MODEL") {
        let pb = PathBuf::from(env_model.trim());
        if pb.is_file() {
            return Some(pb);
        }
    }
    // Prefer multilingual models (large-v3-turbo / medium / small / base / tiny); skip `.en` models.
    const PREFERRED: &[&str] = &[
        "ggml-large-v3-turbo-q5_0.bin",
        "ggml-large-v3-turbo.bin",
        "ggml-medium-q5_0.bin",
        "ggml-medium.bin",
        "ggml-small-q5_1.bin",
        "ggml-small.bin",
        "ggml-base-q5_1.bin",
        "ggml-base.bin",
        "ggml-tiny.bin",
    ];
    for name in PREFERRED {
        let cand = default_dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    if let Ok(entries) = std::fs::read_dir(default_dir) {
        let mut bins: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension().and_then(|e| e.to_str()) == Some("bin")
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.contains(".en."))
            })
            .collect();
        bins.sort();
        if let Some(first) = bins.into_iter().next() {
            return Some(first);
        }
    }
    None
}

/// Fast zero-dependency spoken language classifier distinguishing Swedish (`"sv"`) vs English
/// (`"en"`) for captions that do not carry an explicit BCP-47 language tag. Handles Swedish
/// diacritics (`å`, `ä`, `ö`) and high-frequency stop words cleanly.
pub fn detect_spoken_lang(text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    const SV_WORDS: &[&str] = &[
        "och", "att", "det", "som", "för", "med", "inte", "jag", "har", "kan", "ska", "vi", "är",
        "på", "av", "ett", "den", "till", "om", "men", "från", "eller", "också", "bara", "hur",
        "var", "när", "hej", "tack", "bra", "precis", "kanske", "tycker", "behöver", "möte",
        "kolla", "tänker", "skulle", "blir", "göra", "tror", "alltså", "liksom",
    ];
    const EN_WORDS: &[&str] = &[
        "the", "and", "that", "this", "with", "have", "from", "they", "what", "when", "where",
        "which", "their", "would", "could", "should", "about", "there", "because", "think",
        "going", "really", "actually", "please", "thanks", "hello", "looks", "sounds", "need",
        "just", "like", "know", "make", "good", "right", "here", "yeah", "sure",
    ];

    let mut sv_score: i32 = 0;
    let mut en_score: i32 = 0;

    if trimmed
        .chars()
        .any(|c| matches!(c, 'å' | 'ä' | 'ö' | 'Å' | 'Ä' | 'Ö'))
    {
        sv_score += 3;
    }

    for raw in trimmed.split(|c: char| !c.is_alphabetic()) {
        if raw.is_empty() {
            continue;
        }
        let w = raw.to_lowercase();
        if SV_WORDS.contains(&w.as_str()) {
            sv_score += 2;
        }
        if EN_WORDS.contains(&w.as_str()) {
            en_score += 2;
        }
    }

    if sv_score == 0 && en_score == 0 {
        None
    } else if sv_score > en_score {
        Some("sv")
    } else if en_score > sv_score {
        Some("en")
    } else {
        // Equal score with Swedish diacritics -> Swedish; otherwise English.
        if trimmed
            .chars()
            .any(|c| matches!(c, 'å' | 'ä' | 'ö' | 'Å' | 'Ä' | 'Ö'))
        {
            Some("sv")
        } else {
            Some("en")
        }
    }
}

/// Normalize a BCP-47 language string (`"sv-SE"`, `"en-US"`, `"SV"`) to a lowercase 2-letter
/// prefix (`"sv"`, `"en"`) when present.
pub fn normalize_lang_tag(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("auto") {
        return None;
    }
    let primary = s.split(['-', '_']).next().unwrap_or(s);
    let norm = primary.to_ascii_lowercase();
    (!norm.is_empty()).then_some(norm)
}

/// Pure-Rust energy-based VAD utterance segmenter converting 48 kHz mono `f32` PCM into
/// 16 kHz mono `i16` utterance buffers suitable for per-utterance multilingual Whisper inference.
pub struct VadSegmenter {
    buf_16k: Vec<i16>,
    in_speech: bool,
    speech_samples: usize,
    silence_samples: usize,
}

const TARGET_RATE: usize = 16_000;
const MIN_SPEECH_SAMPLES: usize = TARGET_RATE * 400 / 1000; // 400 ms
const TRAILING_SILENCE_SAMPLES: usize = TARGET_RATE * 650 / 1000; // 650 ms
const MAX_UTTERANCE_SAMPLES: usize = TARGET_RATE * 15; // 15 s cap
const SPEECH_RMS_THRESHOLD: f32 = 0.015;

impl Default for VadSegmenter {
    fn default() -> Self {
        Self::new()
    }
}

impl VadSegmenter {
    pub fn new() -> Self {
        Self {
            buf_16k: Vec::with_capacity(TARGET_RATE * 4),
            in_speech: false,
            speech_samples: 0,
            silence_samples: 0,
        }
    }

    /// Feed a 48 kHz mono `f32` frame; returns `Some(pcm_16k)` when a complete utterance ends.
    pub fn push_48k(&mut self, pcm_48k: &[f32]) -> Option<Vec<i16>> {
        if pcm_48k.is_empty() {
            return None;
        }
        let rms = (pcm_48k.iter().map(|&s| s * s).sum::<f32>() / pcm_48k.len() as f32).sqrt();
        let voiced = rms >= SPEECH_RMS_THRESHOLD;

        if !self.in_speech && !voiced {
            return None;
        }

        // 3:1 box-filter decimation (48 kHz -> 16 kHz) to reduce aliasing before i16 quantization.
        let mut added = 0usize;
        for chunk in pcm_48k.as_chunks::<3>().0 {
            let avg = (chunk[0] + chunk[1] + chunk[2]) * (1.0 / 3.0);
            let sample = (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            self.buf_16k.push(sample);
            added += 1;
        }

        if voiced {
            self.in_speech = true;
            self.speech_samples += added;
            self.silence_samples = 0;
        } else {
            self.silence_samples += added;
        }

        let should_flush = (self.in_speech && self.silence_samples >= TRAILING_SILENCE_SAMPLES)
            || self.buf_16k.len() >= MAX_UTTERANCE_SAMPLES;

        if should_flush {
            return self.flush();
        }
        None
    }

    /// Force-flush any buffered speech (e.g. when leaving a huddle).
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        let ready = self.speech_samples >= MIN_SPEECH_SAMPLES;
        self.in_speech = false;
        self.speech_samples = 0;
        self.silence_samples = 0;
        if ready {
            Some(std::mem::take(&mut self.buf_16k))
        } else {
            self.buf_16k.clear();
            None
        }
    }
}

/// Encode 16 kHz mono 16-bit PCM into a standard RIFF WAVE byte buffer.
pub fn encode_wav_16khz_mono(samples: &[i16]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(TARGET_RATE as u32).to_le_bytes());
    out.extend_from_slice(&((TARGET_RATE as u32) * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Parse `whisper-cli -oj` JSON output into `(detected_language, cleaned_text)`.
/// Filters common Whisper silence hallucinations (`[BLANK_AUDIO]`, `(music)`, etc.).
pub fn parse_whisper_json(raw: &str) -> Option<(Option<String>, String)> {
    let val: serde_json::Value = serde_json::from_str(raw).ok()?;
    let lang = normalize_lang_tag(val.pointer("/result/language").and_then(|v| v.as_str()));
    let mut parts = Vec::new();
    if let Some(items) = val.get("transcription").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                let t = text.trim();
                if !t.is_empty() && !is_whisper_artifact(t) {
                    parts.push(t);
                }
            }
        }
    }
    let combined = parts.join(" ").trim().to_owned();
    if combined.is_empty() {
        return None;
    }
    let resolved_lang = lang.or_else(|| detect_spoken_lang(&combined).map(str::to_owned));
    Some((resolved_lang, combined))
}

fn is_whisper_artifact(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    lower.is_empty()
        || (lower.starts_with('[') && lower.ends_with(']'))
        || (lower.starts_with('(') && lower.ends_with(')'))
        || lower == "you"
        || lower == "thank you."
        || lower == "tack."
}

/// Tagged 48 kHz mono audio chunk sent to [`LocalTranscriber`].
pub struct TranscribeChunk {
    pub is_local: bool,
    pub speaker: UserId,
    pub pcm_48k: Vec<f32>,
}

/// Background opt-in local multilingual (`-l auto`) Whisper transcriber for a live huddle.
/// Drops cleanly when the huddle leaves; writes zero persistent audio files to disk.
pub struct LocalTranscriber {
    tx: SyncSender<TranscribeChunk>,
}

impl LocalTranscriber {
    /// Start a local multilingual Whisper worker if `whisper-cli` and a multilingual model are
    /// present on the host. Returns `None` gracefully when not installed.
    pub fn try_start(
        updates: tokio_mpsc::UnboundedSender<HuddleUpdate>,
        custom_bin: Option<&str>,
        custom_model: Option<&str>,
    ) -> Option<Self> {
        let setup = detect_whisper_setup(custom_bin, custom_model);
        let (bin, model) = (setup.binary?, setup.model?);
        let (tx, rx) = mpsc::sync_channel::<TranscribeChunk>(64);
        let _ = std::thread::Builder::new()
            .name("slackless-whisper-stt".to_owned())
            .spawn(move || {
                let mut local_vad = VadSegmenter::new();
                let mut remote_vad = VadSegmenter::new();
                let mut last_local_speaker = UserId::from("U0SELF");
                let mut last_remote_speaker = UserId::from("U0REMOTE");
                let mut seq: u64 = 0;

                while let Ok(chunk) = rx.recv() {
                    let utterance = if chunk.is_local {
                        last_local_speaker = chunk.speaker;
                        local_vad
                            .push_48k(&chunk.pcm_48k)
                            .map(|pcm| (last_local_speaker.clone(), pcm))
                    } else {
                        last_remote_speaker = chunk.speaker;
                        remote_vad
                            .push_48k(&chunk.pcm_48k)
                            .map(|pcm| (last_remote_speaker.clone(), pcm))
                    };
                    if let Some((speaker, pcm_16k)) = utterance {
                        seq = seq.wrapping_add(1);
                        run_whisper_utterance(&bin, &model, seq, speaker, &pcm_16k, &updates);
                    }
                }
                if let Some(pcm_16k) = local_vad.flush() {
                    seq = seq.wrapping_add(1);
                    run_whisper_utterance(
                        &bin,
                        &model,
                        seq,
                        last_local_speaker,
                        &pcm_16k,
                        &updates,
                    );
                }
                if let Some(pcm_16k) = remote_vad.flush() {
                    seq = seq.wrapping_add(1);
                    run_whisper_utterance(
                        &bin,
                        &model,
                        seq,
                        last_remote_speaker,
                        &pcm_16k,
                        &updates,
                    );
                }
            });
        Some(Self { tx })
    }

    /// Non-blocking enqueue of a 48 kHz mono frame (drops silently under backpressure so real-time
    /// huddle audio never stalls).
    pub fn push(&self, is_local: bool, speaker: UserId, pcm_48k: &[f32]) {
        let _ = self.tx.try_send(TranscribeChunk {
            is_local,
            speaker,
            pcm_48k: pcm_48k.to_vec(),
        });
    }
}

fn run_whisper_utterance(
    bin: &Path,
    model: &Path,
    seq: u64,
    speaker: UserId,
    pcm_16k: &[i16],
    updates: &tokio_mpsc::UnboundedSender<HuddleUpdate>,
) {
    let pid = std::process::id();
    let stem = std::env::temp_dir().join(format!("slackless-stt-{pid}-{seq}"));
    let wav_path = stem.with_extension("wav");
    let json_path = stem.with_extension("json");

    struct Cleanup(PathBuf, PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(&self.1);
        }
    }
    let _cleanup = Cleanup(wav_path.clone(), json_path.clone());

    let wav_bytes = encode_wav_16khz_mono(pcm_16k);
    if std::fs::write(&wav_path, wav_bytes).is_err() {
        return;
    }

    let ok = Command::new(bin)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&wav_path)
        .arg("-l")
        .arg("auto")
        .arg("--prompt")
        .arg(BILINGUAL_WHISPER_PROMPT)
        .arg("-oj")
        .arg("-of")
        .arg(&stem)
        .arg("--no-prints")
        .status()
        .is_ok_and(|s| s.success());

    if !ok {
        return;
    }

    let Ok(raw_json) = std::fs::read_to_string(&json_path) else {
        return;
    };
    let Some((lang, text)) = parse_whisper_json(&raw_json) else {
        return;
    };
    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let _ = updates.send(HuddleUpdate::DataEvent(HuddleDataEvent::Caption {
        user: speaker,
        text,
        is_partial: false,
        at_ms,
        lang,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_spoken_lang_handles_swedish_english_and_code_switching() {
        assert_eq!(
            detect_spoken_lang("Hej allihopa, vi behöver kolla på mötet imorgon"),
            Some("sv")
        );
        assert_eq!(
            detect_spoken_lang("Let's deploy the release and check what happens next"),
            Some("en")
        );
        assert_eq!(
            detect_spoken_lang("Vi kan göra en deploy av checkout-order idag"),
            Some("sv")
        );
        assert_eq!(detect_spoken_lang("   "), None);
    }

    #[test]
    fn normalize_lang_tag_strips_region_and_auto() {
        assert_eq!(normalize_lang_tag(Some("sv-SE")), Some("sv".into()));
        assert_eq!(normalize_lang_tag(Some("en_US")), Some("en".into()));
        assert_eq!(normalize_lang_tag(Some("auto")), None);
        assert_eq!(normalize_lang_tag(None), None);
    }

    #[test]
    fn vad_segmenter_frames_utterance_and_encodes_valid_wav() {
        let mut vad = VadSegmenter::new();
        let voiced = vec![0.12f32; 960]; // 20ms at 48kHz
        let silent = vec![0.0f32; 960];

        // Leading silence ignored.
        assert!(vad.push_48k(&silent).is_none());

        // 500ms of speech (25 x 20ms).
        for _ in 0..25 {
            assert!(vad.push_48k(&voiced).is_none());
        }

        // Trailing silence triggers utterance emission after ~650ms.
        let mut emitted = None;
        for _ in 0..40 {
            if let Some(pcm) = vad.push_48k(&silent) {
                emitted = Some(pcm);
                break;
            }
        }
        let pcm = emitted.expect("utterance should flush after trailing silence");
        assert!(pcm.len() >= MIN_SPEECH_SAMPLES);

        let wav = encode_wav_16khz_mono(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + pcm.len() * 2);
    }

    #[test]
    fn parse_whisper_json_extracts_language_and_skips_blank_artifacts() {
        let sv_json = r#"{
            "result": { "language": "sv" },
            "transcription": [
                { "text": " [BLANK_AUDIO] " },
                { "text": " Vi tar det i morgon bitti." }
            ]
        }"#;
        assert_eq!(
            parse_whisper_json(sv_json),
            Some((Some("sv".into()), "Vi tar det i morgon bitti.".into()))
        );

        let en_json = r#"{
            "result": { "language": "en" },
            "transcription": [
                { "text": " Sounds good, let's ship it." }
            ]
        }"#;
        assert_eq!(
            parse_whisper_json(en_json),
            Some((Some("en".into()), "Sounds good, let's ship it.".into()))
        );

        let blank_only = r#"{
            "result": { "language": "en" },
            "transcription": [{ "text": "[BLANK_AUDIO]" }]
        }"#;
        assert_eq!(parse_whisper_json(blank_only), None);
    }
}
