//! Opt-in local multilingual (English + Swedish code-switching) huddle transcription engines:
//!
//! - [`TranscriptionProvider::SlackBuiltIn`]: default (0 MB disk, 0 extra RAM); archives Slack's
//!   AWS Transcribe `SdkTranscriptFrame` captions when captions are enabled.
//! - [`TranscriptionProvider::ParakeetV3`]: in-process Rust ONNX runtime (`transcribe-rs`) running
//!   NVIDIA Parakeet TDT 0.6B v3 INT8 (~640 MB opt-in download; 25 European languages including
//!   English & Swedish; zero external binaries; loaded only during active huddles -> 0 MB RAM idle).
//! - [`TranscriptionProvider::WhisperTurbo`]: on-demand local `whisper-server` (or `whisper-cli`
//!   fallback) running OpenAI Whisper Large v3 Turbo `q5_0` (~574 MB opt-in download; EN + SV
//!   code-switching with `-l auto`; server stays hot during active huddles and exits on hangup).

use std::io::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use slackless_types::UserId;
use tokio::sync::mpsc as tokio_mpsc;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};

use crate::{HuddleDataEvent, HuddleUpdate};

/// Multilingual Whisper prompt priming both English and Swedish (plus common tech/code-switching).
const BILINGUAL_WHISPER_PROMPT: &str =
    "Bilingual English and Svenska (Swedish) workplace conversation.";

const PARAKEET_V3_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main";
const PARAKEET_V3_FILES: &[&str] = &[
    "config.json",
    "vocab.txt",
    "nemo128.onnx",
    "decoder_joint-model.int8.onnx",
    "encoder-model.int8.onnx",
];

const WHISPER_TURBO_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";
const WHISPER_TURBO_FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";

/// Selectable transcription provider for huddles (opt-in local models; default is Slack built-in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TranscriptionProvider {
    /// Slack's built-in AWS Transcribe captions (0 MB on disk).
    #[default]
    SlackBuiltIn,
    /// Local in-process NVIDIA Parakeet TDT 0.6B v3 INT8 (~640 MB; EN + SV + 23 EU languages).
    ParakeetV3,
    /// Local Whisper Large v3 Turbo q5_0 (~574 MB; EN + SV code-switching via `whisper-server`).
    WhisperTurbo,
}

impl TranscriptionProvider {
    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::ParakeetV3,
            2 => Self::WhisperTurbo,
            _ => Self::SlackBuiltIn,
        }
    }

    pub const fn to_index(self) -> usize {
        match self {
            Self::SlackBuiltIn => 0,
            Self::ParakeetV3 => 1,
            Self::WhisperTurbo => 2,
        }
    }

    pub const fn is_local(self) -> bool {
        !matches!(self, Self::SlackBuiltIn)
    }

    pub const fn source_tag(self) -> &'static str {
        match self {
            Self::SlackBuiltIn => "chime",
            Self::ParakeetV3 => "parakeet-v3",
            Self::WhisperTurbo => "whisper-turbo",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::SlackBuiltIn => "Slack built-in (AWS Transcribe)",
            Self::ParakeetV3 => "Parakeet TDT 0.6B v3 (Local EN + SV)",
            Self::WhisperTurbo => "Whisper Large v3 Turbo (Local EN + SV)",
        }
    }
}

/// Setup & disk status for one [`TranscriptionProvider`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSetupStatus {
    pub provider: TranscriptionProvider,
    pub model_ready: bool,
    pub binary_ready: bool,
    pub model_path: PathBuf,
    pub disk_bytes: u64,
    pub download_size_hint: &'static str,
    pub binary_path: Option<PathBuf>,
}

impl ProviderSetupStatus {
    pub fn is_ready(&self) -> bool {
        self.model_ready && self.binary_ready
    }
}

/// Incremental download progress emitted by [`download_provider_model`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDownloadProgress {
    pub file_name: String,
    pub file_index: usize,
    pub file_count: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

fn data_root_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("slackless");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("slackless");
    }
    std::env::temp_dir().join("slackless")
}

/// Directory storing the opt-in Parakeet TDT 0.6B v3 INT8 ONNX files (~640 MB when downloaded).
pub fn default_parakeet_model_dir() -> PathBuf {
    data_root_dir()
        .join("models")
        .join("parakeet-tdt-0.6b-v3-int8")
}

/// Directory storing optional GGML multilingual Whisper models (~574 MB when downloaded).
pub fn default_whisper_model_dir() -> PathBuf {
    data_root_dir().join("whisper")
}

fn dir_disk_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map_or(0, |m| m.len());
    }
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

fn parakeet_files_present(dir: &Path) -> bool {
    const REQUIRED: &[&str] = &[
        "encoder-model.int8.onnx",
        "decoder_joint-model.int8.onnx",
        "nemo128.onnx",
        "vocab.txt",
    ];
    REQUIRED.iter().all(|f| dir.join(f).is_file())
}

/// Inspect whether `provider` is ready to run locally (model files + optional external binary).
pub fn detect_provider_setup(provider: TranscriptionProvider) -> ProviderSetupStatus {
    match provider {
        TranscriptionProvider::SlackBuiltIn => ProviderSetupStatus {
            provider,
            model_ready: true,
            binary_ready: true,
            model_path: PathBuf::new(),
            disk_bytes: 0,
            download_size_hint: "0 MB",
            binary_path: None,
        },
        TranscriptionProvider::ParakeetV3 => {
            let dir = std::env::var("SLACKLESS_PARAKEET_DIR")
                .ok()
                .map(|s| PathBuf::from(s.trim()))
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(default_parakeet_model_dir);
            let ready = parakeet_files_present(&dir);
            ProviderSetupStatus {
                provider,
                model_ready: ready,
                binary_ready: true, // In-process Rust ONNX runtime — zero external binaries!
                disk_bytes: dir_disk_bytes(&dir),
                model_path: dir,
                download_size_hint: "~640 MB",
                binary_path: None,
            }
        }
        TranscriptionProvider::WhisperTurbo => {
            let dir = default_whisper_model_dir();
            let model = find_whisper_model(None, &dir);
            let server = find_whisper_binary(&["whisper-server"]);
            let cli = find_whisper_binary(&["whisper-cli", "whisper-cpp"]);
            let binary_path = server.or(cli);
            let disk_bytes = model
                .as_deref()
                .map_or_else(|| dir_disk_bytes(&dir), dir_disk_bytes);
            ProviderSetupStatus {
                provider,
                model_ready: model.is_some(),
                binary_ready: binary_path.is_some(),
                model_path: model.unwrap_or_else(|| dir.join(WHISPER_TURBO_FILENAME)),
                disk_bytes,
                download_size_hint: "~574 MB",
                binary_path,
            }
        }
    }
}

fn find_whisper_binary(names: &[&str]) -> Option<PathBuf> {
    if let Ok(env_bin) = std::env::var("SLACKLESS_WHISPER_BIN") {
        let pb = PathBuf::from(env_bin.trim());
        if pb.is_file() {
            return Some(pb);
        }
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        for name in names {
            let pb = PathBuf::from(dir).join(name);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in names {
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

/// Download the opt-in local model weights for `provider` atomically (`.part` -> rename) with
/// incremental progress updates. Blocking helper — call on a background executor/thread.
pub fn download_provider_model(
    provider: TranscriptionProvider,
    on_progress: impl Fn(ModelDownloadProgress) + Send + Sync + 'static,
) -> std::result::Result<PathBuf, String> {
    if !provider.is_local() {
        return Err("Slack built-in captions do not require a model download.".to_owned());
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create download runtime: {e}"))?;
    rt.block_on(download_provider_model_async(provider, on_progress))
}

async fn download_provider_model_async(
    provider: TranscriptionProvider,
    on_progress: impl Fn(ModelDownloadProgress) + Send + Sync + 'static,
) -> std::result::Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .user_agent("slackless/1.0")
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let (dest_dir, files): (PathBuf, Vec<(String, String)>) = match provider {
        TranscriptionProvider::SlackBuiltIn => return Ok(PathBuf::new()),
        TranscriptionProvider::ParakeetV3 => {
            let dir = default_parakeet_model_dir();
            let list = PARAKEET_V3_FILES
                .iter()
                .map(|name| (format!("{PARAKEET_V3_BASE_URL}/{name}"), (*name).to_owned()))
                .collect();
            (dir, list)
        }
        TranscriptionProvider::WhisperTurbo => {
            let dir = default_whisper_model_dir();
            (
                dir,
                vec![(
                    WHISPER_TURBO_URL.to_owned(),
                    WHISPER_TURBO_FILENAME.to_owned(),
                )],
            )
        }
    };

    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("failed to create {}: {e}", dest_dir.display()))?;

    let file_count = files.len();
    for (idx, (url, name)) in files.into_iter().enumerate() {
        let final_path = dest_dir.join(&name);
        if final_path.is_file() && final_path.metadata().is_ok_and(|m| m.len() > 0) {
            let len = final_path.metadata().map_or(0, |m| m.len());
            on_progress(ModelDownloadProgress {
                file_name: name,
                file_index: idx + 1,
                file_count,
                downloaded_bytes: len,
                total_bytes: Some(len),
            });
            continue;
        }
        let part_path = dest_dir.join(format!("{name}.part"));
        let mut resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("download failed for {name}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {} downloading {name}", resp.status()));
        }
        let total_bytes = resp.content_length();
        let mut file = std::fs::File::create(&part_path)
            .map_err(|e| format!("cannot create {}: {e}", part_path.display()))?;
        let mut downloaded: u64 = 0;
        let mut last_emit: u64 = 0;
        on_progress(ModelDownloadProgress {
            file_name: name.clone(),
            file_index: idx + 1,
            file_count,
            downloaded_bytes: 0,
            total_bytes,
        });
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("stream error downloading {name}: {e}"))?
        {
            file.write_all(&chunk)
                .map_err(|e| format!("write error for {name}: {e}"))?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            if downloaded.saturating_sub(last_emit) >= 512 * 1024 {
                last_emit = downloaded;
                on_progress(ModelDownloadProgress {
                    file_name: name.clone(),
                    file_index: idx + 1,
                    file_count,
                    downloaded_bytes: downloaded,
                    total_bytes,
                });
            }
        }
        file.flush()
            .map_err(|e| format!("flush error for {name}: {e}"))?;
        drop(file);
        std::fs::rename(&part_path, &final_path)
            .map_err(|e| format!("rename error for {name}: {e}"))?;
        on_progress(ModelDownloadProgress {
            file_name: name,
            file_index: idx + 1,
            file_count,
            downloaded_bytes: downloaded,
            total_bytes: total_bytes.or(Some(downloaded)),
        });
    }

    Ok(dest_dir)
}

/// Remove locally downloaded model weights for `provider` and return reclaimed bytes.
pub fn delete_provider_model(provider: TranscriptionProvider) -> std::result::Result<u64, String> {
    let status = detect_provider_setup(provider);
    let reclaimed = status.disk_bytes;
    match provider {
        TranscriptionProvider::SlackBuiltIn => Ok(0),
        TranscriptionProvider::ParakeetV3 => {
            if status.model_path.exists() {
                std::fs::remove_dir_all(&status.model_path).map_err(|e| {
                    format!("failed to remove {}: {e}", status.model_path.display())
                })?;
            }
            Ok(reclaimed)
        }
        TranscriptionProvider::WhisperTurbo => {
            let dir = default_whisper_model_dir();
            if status.model_path.is_file() {
                std::fs::remove_file(&status.model_path).map_err(|e| {
                    format!("failed to remove {}: {e}", status.model_path.display())
                })?;
            } else if dir.exists() {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| format!("failed to remove {}: {e}", dir.display()))?;
            }
            Ok(reclaimed)
        }
    }
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

    let has_sv_diacritics = trimmed
        .chars()
        .any(|c| matches!(c, 'å' | 'ä' | 'ö' | 'Å' | 'Ä' | 'Ö'));
    if has_sv_diacritics {
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
    } else if has_sv_diacritics {
        Some("sv")
    } else {
        Some("en")
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
/// 16 kHz mono `i16` utterance buffers suitable for per-utterance multilingual inference.
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
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(TARGET_RATE as u32).to_le_bytes());
    out.extend_from_slice(&((TARGET_RATE as u32) * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Parse `whisper-cli -oj` or `whisper-server` JSON output into `(detected_language, cleaned_text)`.
pub fn parse_whisper_json(raw: &str) -> Option<(Option<String>, String)> {
    let val: serde_json::Value = serde_json::from_str(raw).ok()?;
    let lang = normalize_lang_tag(
        val.pointer("/result/language")
            .or_else(|| val.get("language"))
            .and_then(|v| v.as_str()),
    );
    let mut parts = Vec::new();
    if let Some(items) = val.get("transcription").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                let t = text.trim();
                if !t.is_empty() && !is_stt_artifact(t) {
                    parts.push(t);
                }
            }
        }
    } else if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
        let t = text.trim();
        if !t.is_empty() && !is_stt_artifact(t) {
            parts.push(t);
        }
    }
    let combined = parts.join(" ").trim().to_owned();
    if combined.is_empty() {
        return None;
    }
    let resolved_lang = lang.or_else(|| detect_spoken_lang(&combined).map(str::to_owned));
    Some((resolved_lang, combined))
}

fn is_stt_artifact(text: &str) -> bool {
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

/// Active local backend kept resident in RAM strictly for the lifetime of a huddle.
enum ActiveSttBackend {
    Parakeet(Box<ParakeetModel>),
    WhisperServer {
        child: Child,
        url: String,
        rt: tokio::runtime::Runtime,
        http: reqwest::Client,
    },
    WhisperCli {
        bin: PathBuf,
        model: PathBuf,
    },
}

impl Drop for ActiveSttBackend {
    fn drop(&mut self) {
        if let ActiveSttBackend::WhisperServer { child, .. } = self {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl ActiveSttBackend {
    fn transcribe(&mut self, seq: u64, pcm_16k: &[i16]) -> Option<(Option<String>, String)> {
        match self {
            ActiveSttBackend::Parakeet(model) => {
                let samples_f32: Vec<f32> =
                    pcm_16k.iter().map(|&s| f32::from(s) / 32_768.0).collect();
                let res = model
                    .transcribe_with(&samples_f32, &ParakeetParams::default())
                    .ok()?;
                let text = res.text.trim().to_owned();
                if text.is_empty() || is_stt_artifact(&text) {
                    return None;
                }
                let lang = detect_spoken_lang(&text).map(str::to_owned);
                Some((lang, text))
            }
            ActiveSttBackend::WhisperServer { url, rt, http, .. } => {
                let wav_bytes = encode_wav_16khz_mono(pcm_16k);
                let url = url.clone();
                let http = http.clone();
                rt.block_on(async move {
                    let part = reqwest::multipart::Part::bytes(wav_bytes)
                        .file_name("utterance.wav")
                        .mime_str("audio/wav")
                        .ok()?;
                    let form = reqwest::multipart::Form::new()
                        .part("file", part)
                        .text("response_format", "verbose_json")
                        .text("language", "auto")
                        .text("prompt", BILINGUAL_WHISPER_PROMPT);
                    let resp = http.post(&url).multipart(form).send().await.ok()?;
                    if !resp.status().is_success() {
                        return None;
                    }
                    let body = resp.text().await.ok()?;
                    parse_whisper_json(&body)
                })
            }
            ActiveSttBackend::WhisperCli { bin, model } => {
                run_whisper_cli_utterance(bin, model, seq, pcm_16k)
            }
        }
    }
}

/// Background opt-in local multilingual transcriber for a live huddle.
/// Loads weights only while the huddle is active; drops completely on hangup (**0 MB RAM idle**).
pub struct LocalTranscriber {
    tx: SyncSender<TranscribeChunk>,
}

impl LocalTranscriber {
    /// Start a local transcriber for `provider` when ready on the host. Returns `None` for
    /// [`TranscriptionProvider::SlackBuiltIn`] or when the selected model isn't downloaded yet.
    pub fn try_start(
        provider: TranscriptionProvider,
        updates: tokio_mpsc::UnboundedSender<HuddleUpdate>,
    ) -> Option<Self> {
        if !provider.is_local() {
            return None;
        }
        let setup = detect_provider_setup(provider);
        if !setup.is_ready() {
            tracing::warn!(
                ?provider,
                model_ready = setup.model_ready,
                binary_ready = setup.binary_ready,
                "local transcription provider selected but not ready; skipping"
            );
            return None;
        }
        let (tx, rx) = mpsc::sync_channel::<TranscribeChunk>(64);
        let _ = std::thread::Builder::new()
            .name(format!("slackless-stt-{}", provider.source_tag()))
            .spawn(move || {
                let Some(mut backend) = init_stt_backend(provider, &setup) else {
                    return;
                };
                tracing::info!(
                    ?provider,
                    "local multilingual STT backend loaded for huddle"
                );

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
                        emit_transcribed(&mut backend, seq, speaker, &pcm_16k, &updates);
                    }
                }
                if let Some(pcm_16k) = local_vad.flush() {
                    seq = seq.wrapping_add(1);
                    emit_transcribed(&mut backend, seq, last_local_speaker, &pcm_16k, &updates);
                }
                if let Some(pcm_16k) = remote_vad.flush() {
                    seq = seq.wrapping_add(1);
                    emit_transcribed(&mut backend, seq, last_remote_speaker, &pcm_16k, &updates);
                }
                tracing::info!(?provider, "local multilingual STT backend unloaded");
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

fn init_stt_backend(
    provider: TranscriptionProvider,
    setup: &ProviderSetupStatus,
) -> Option<ActiveSttBackend> {
    match provider {
        TranscriptionProvider::SlackBuiltIn => None,
        TranscriptionProvider::ParakeetV3 => {
            match ParakeetModel::load(&setup.model_path, &Quantization::Int8) {
                Ok(model) => Some(ActiveSttBackend::Parakeet(Box::new(model))),
                Err(err) => {
                    tracing::error!(%err, path = %setup.model_path.display(), "failed to load Parakeet v3 INT8 model");
                    None
                }
            }
        }
        TranscriptionProvider::WhisperTurbo => {
            if let Some(server_bin) = find_whisper_binary(&["whisper-server"])
                && let Some(srv) = spawn_whisper_server(&server_bin, &setup.model_path)
            {
                return Some(srv);
            }
            let cli_bin = setup.binary_path.clone()?;
            Some(ActiveSttBackend::WhisperCli {
                bin: cli_bin,
                model: setup.model_path.clone(),
            })
        }
    }
}

fn spawn_whisper_server(bin: &Path, model: &Path) -> Option<ActiveSttBackend> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);

    let child = Command::new(bin)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("-m")
        .arg(model)
        .arg("-l")
        .arg("auto")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .ok()?;

    // Wait up to 8s for `whisper-server` to finish loading the model onto Metal/CPU.
    let health_url = format!("http://127.0.0.1:{port}/");
    let ready = rt.block_on(async {
        for _ in 0..80 {
            if http.get(&health_url).send().await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    });
    if !ready {
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }

    Some(ActiveSttBackend::WhisperServer {
        child,
        url: format!("http://127.0.0.1:{port}/inference"),
        rt,
        http,
    })
}

fn emit_transcribed(
    backend: &mut ActiveSttBackend,
    seq: u64,
    speaker: UserId,
    pcm_16k: &[i16],
    updates: &tokio_mpsc::UnboundedSender<HuddleUpdate>,
) {
    let Some((lang, text)) = backend.transcribe(seq, pcm_16k) else {
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

fn run_whisper_cli_utterance(
    bin: &Path,
    model: &Path,
    seq: u64,
    pcm_16k: &[i16],
) -> Option<(Option<String>, String)> {
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
    std::fs::write(&wav_path, wav_bytes).ok()?;

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
        return None;
    }

    let raw_json = std::fs::read_to_string(&json_path).ok()?;
    parse_whisper_json(&raw_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_index_round_trip_and_tags() {
        for prov in [
            TranscriptionProvider::SlackBuiltIn,
            TranscriptionProvider::ParakeetV3,
            TranscriptionProvider::WhisperTurbo,
        ] {
            assert_eq!(TranscriptionProvider::from_index(prov.to_index()), prov);
        }
        assert!(!TranscriptionProvider::SlackBuiltIn.is_local());
        assert!(TranscriptionProvider::ParakeetV3.is_local());
        assert!(TranscriptionProvider::WhisperTurbo.is_local());
        assert_eq!(
            TranscriptionProvider::ParakeetV3.source_tag(),
            "parakeet-v3"
        );
    }

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
        let voiced = vec![0.12f32; 960];
        let silent = vec![0.0f32; 960];

        assert!(vad.push_48k(&silent).is_none());
        for _ in 0..25 {
            assert!(vad.push_48k(&voiced).is_none());
        }
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
    fn parse_whisper_json_handles_cli_and_server_shapes() {
        let sv_cli = r#"{
            "result": { "language": "sv" },
            "transcription": [
                { "text": " [BLANK_AUDIO] " },
                { "text": " Vi tar det i morgon bitti." }
            ]
        }"#;
        assert_eq!(
            parse_whisper_json(sv_cli),
            Some((Some("sv".into()), "Vi tar det i morgon bitti.".into()))
        );

        let en_server = r#"{ "language": "english", "text": " Sounds good, let's ship it." }"#;
        assert_eq!(
            parse_whisper_json(en_server),
            Some((Some("english".into()), "Sounds good, let's ship it.".into()))
        );

        let blank_only =
            r#"{ "result": { "language": "en" }, "transcription": [{ "text": "[BLANK_AUDIO]" }] }"#;
        assert_eq!(parse_whisper_json(blank_only), None);
    }
}
