//! Opt-in local multilingual (English + Swedish code-switching) huddle transcription engines:
//!
//! - [`TranscriptionProvider::SlackBuiltIn`]: default (0 MB disk, 0 extra RAM); archives Slack's
//!   AWS Transcribe `SdkTranscriptFrame` captions when captions are enabled.
//! - [`TranscriptionProvider::ParakeetV3`]: in-process ONNX Runtime (`transcribe-rs`, runtime
//!   library loaded on demand by [`crate::onnx_runtime`]) running NVIDIA Parakeet TDT 0.6B v3 INT8
//!   (~640 MB opt-in download; 25 European languages including English & Swedish; loaded only
//!   during active huddles -> 0 MB RAM idle).
//! - [`TranscriptionProvider::WhisperTurbo`]: on-demand local `whisper-server` (or `whisper-cli`
//!   fallback) running OpenAI Whisper Large v3 Turbo `q5_0` (~574 MB opt-in download; EN + SV
//!   code-switching with `-l auto`; server stays hot during active huddles and exits on hangup).

use std::future::Future;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use slackless_types::{TranscriptCue, UserId};
use tokio::sync::mpsc as tokio_mpsc;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity};

use crate::audio::RateConverter;
use crate::{HuddleDataEvent, HuddleUpdate, onnx_runtime};

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
/// Largest model file accepted; the biggest (Parakeet's encoder, Whisper Turbo) are ~600 MB.
const MAX_MODEL_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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

    #[cfg(test)]
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
            Self::ParakeetV3 => "Parakeet TDT 0.6B v3",
            Self::WhisperTurbo => "Whisper Large v3 Turbo",
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

pub(crate) fn data_root_dir() -> PathBuf {
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
            // Runs in-process; its only binary is the ONNX Runtime library loaded on demand.
            let binary_path = onnx_runtime::locate().ok();
            ProviderSetupStatus {
                provider,
                model_ready: ready,
                binary_ready: binary_path.is_some(),
                disk_bytes: dir_disk_bytes(&dir),
                model_path: dir,
                download_size_hint: "~640 MB",
                binary_path,
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

/// Return a shared Tokio handle for blocking HTTP helpers without spawning nested runtimes per
/// call. Reuses the ambient multi-thread Tokio runtime when present; otherwise lazily initializes
/// one shared lightweight background runtime for the lifetime of the process.
fn shared_http_runtime_handle() -> tokio::runtime::Handle {
    static SHARED_HTTP_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let rt = SHARED_HTTP_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("slackless-huddle-http")
            .enable_all()
            .build()
            .expect("failed to initialize shared huddle HTTP runtime")
    });
    rt.handle().clone()
}

/// Execute an async HTTP future from a synchronous context without creating a fresh Tokio runtime.
pub(crate) fn block_on_http<F>(fut: F) -> F::Output
where
    F: Future + Send,
    F::Output: Send,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current()
        && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(|| handle.block_on(fut));
    }
    // Outside Tokio (e.g. dedicated OS worker thread) or on a `current_thread` runtime where
    // `block_in_place` is forbidden: drive on the single shared background HTTP runtime.
    let handle = shared_http_runtime_handle();
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| s.spawn(|| handle.block_on(fut)).join())
            .expect("http helper thread panicked")
    } else {
        handle.block_on(fut)
    }
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
    block_on_http(download_provider_model_async(provider, on_progress))
}

async fn download_provider_model_async(
    provider: TranscriptionProvider,
    on_progress: impl Fn(ModelDownloadProgress) + Send + Sync + 'static,
) -> std::result::Result<PathBuf, String> {
    crate::signaling::install_default_crypto_provider();
    let client = reqwest::Client::builder()
        .user_agent("slackless/1.0")
        .build()
        .map_err(|e| format!("http client error: {e}"))?;
    let assets = slackless_api::AssetClient::with_http_client(client);

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
        let progress = |downloaded_bytes, total_bytes| {
            on_progress(ModelDownloadProgress {
                file_name: name.clone(),
                file_index: idx + 1,
                file_count,
                downloaded_bytes,
                total_bytes,
            });
        };
        let final_path = dest_dir.join(&name);
        if final_path.is_file() && final_path.metadata().is_ok_and(|m| m.len() > 0) {
            let len = final_path.metadata().map_or(0, |m| m.len());
            progress(len, Some(len));
            continue;
        }
        let downloaded = assets
            .download_to_path(&url, &final_path, MAX_MODEL_FILE_BYTES, &progress)
            .await
            .map_err(|e| format!("download failed for {name}: {e}"))?;
        progress(downloaded, Some(downloaded));
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
    crate::clips::encode_wav_pcm16(samples, TARGET_RATE as u32, 1)
}

fn parse_whisper_timestamp_str(raw: &str) -> Option<f64> {
    let trimmed = raw.trim().replace(',', ".");
    let parts: Vec<&str> = trimmed.split(':').collect();
    let (h, m, s) = match parts.as_slice() {
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [h, m, s] => (
            h.parse::<f64>().ok()?,
            m.parse::<f64>().ok()?,
            s.parse::<f64>().ok()?,
        ),
        _ => return None,
    };
    if h.is_finite() && m.is_finite() && s.is_finite() && h >= 0.0 && m >= 0.0 && s >= 0.0 {
        Some(h * 3600.0 + m * 60.0 + s)
    } else {
        None
    }
}

/// Parse `whisper-cli -oj` or `whisper-server` (`verbose_json`) JSON output into
/// `(detected_language, cleaned_text, timed_cues)`, shifting all cue timestamps by `offset_secs`.
pub fn parse_whisper_json_cues(
    raw: &str,
    offset_secs: f64,
    fallback_dur_secs: f64,
) -> Option<(Option<String>, String, Vec<TranscriptCue>)> {
    let val: serde_json::Value = serde_json::from_str(raw).ok()?;
    let offset = if offset_secs.is_finite() && offset_secs >= 0.0 {
        offset_secs
    } else {
        0.0
    };
    let fallback_dur = if fallback_dur_secs.is_finite() && fallback_dur_secs > 0.0 {
        fallback_dur_secs
    } else {
        0.0
    };
    let lang = normalize_lang_tag(
        val.pointer("/result/language")
            .or_else(|| val.get("language"))
            .and_then(|v| v.as_str()),
    );
    let mut parts = Vec::new();
    let mut cues = Vec::new();
    if let Some(items) = val.get("transcription").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                let t = text.trim();
                if !t.is_empty() && !is_stt_artifact(t) {
                    parts.push(t.to_owned());
                    let start = item
                        .pointer("/offsets/from")
                        .and_then(|v| v.as_f64())
                        .map(|ms| ms / 1000.0)
                        .or_else(|| {
                            item.pointer("/timestamps/from")
                                .and_then(|v| v.as_str())
                                .and_then(parse_whisper_timestamp_str)
                        });
                    let end = item
                        .pointer("/offsets/to")
                        .and_then(|v| v.as_f64())
                        .map(|ms| ms / 1000.0)
                        .or_else(|| {
                            item.pointer("/timestamps/to")
                                .and_then(|v| v.as_str())
                                .and_then(parse_whisper_timestamp_str)
                        });
                    if let Some(s) = start.filter(|v| v.is_finite() && *v >= 0.0) {
                        let start_secs = offset + s;
                        let end_secs = end
                            .filter(|v| v.is_finite() && *v >= 0.0)
                            .map_or(start_secs, |e| (offset + e).max(start_secs));
                        cues.push(TranscriptCue {
                            start_secs,
                            end_secs,
                            text: t.to_owned(),
                        });
                    }
                }
            }
        }
    } else if let Some(segments) = val.get("segments").and_then(|v| v.as_array()) {
        for seg in segments {
            if let Some(text) = seg.get("text").and_then(|v| v.as_str()) {
                let t = text.trim();
                if !t.is_empty() && !is_stt_artifact(t) {
                    parts.push(t.to_owned());
                    let start = seg.get("start").and_then(|v| v.as_f64());
                    let end = seg.get("end").and_then(|v| v.as_f64());
                    if let Some(s) = start.filter(|v| v.is_finite() && *v >= 0.0) {
                        let start_secs = offset + s;
                        let end_secs = end
                            .filter(|v| v.is_finite() && *v >= 0.0)
                            .map_or(start_secs, |e| (offset + e).max(start_secs));
                        cues.push(TranscriptCue {
                            start_secs,
                            end_secs,
                            text: t.to_owned(),
                        });
                    }
                }
            }
        }
    } else if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
        let t = text.trim();
        if !t.is_empty() && !is_stt_artifact(t) {
            parts.push(t.to_owned());
        }
    }
    let combined = parts.join(" ").trim().to_owned();
    if combined.is_empty() {
        return None;
    }
    if cues.is_empty() && fallback_dur > 0.0 {
        cues.push(TranscriptCue {
            start_secs: offset,
            end_secs: offset + fallback_dur,
            text: combined.clone(),
        });
    }
    let resolved_lang = lang.or_else(|| detect_spoken_lang(&combined).map(str::to_owned));
    Some((resolved_lang, combined, cues))
}

/// Parse `whisper-cli -oj` or `whisper-server` JSON output into `(detected_language, cleaned_text)`.
#[cfg(test)]
pub fn parse_whisper_json(raw: &str) -> Option<(Option<String>, String)> {
    parse_whisper_json_cues(raw, 0.0, 0.0).map(|(lang, text, _)| (lang, text))
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
        child: Arc<Mutex<Option<Child>>>,
        url: String,
        http: reqwest::Client,
    },
    WhisperCli {
        bin: PathBuf,
        model: PathBuf,
    },
}

fn kill_whisper_server(slot: &Mutex<Option<Child>>) {
    if let Ok(mut guard) = slot.lock()
        && let Some(mut child) = guard.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for ActiveSttBackend {
    fn drop(&mut self) {
        if let ActiveSttBackend::WhisperServer { child, .. } = self {
            kill_whisper_server(child);
        }
    }
}

impl ActiveSttBackend {
    fn transcribe(&mut self, seq: u64, pcm_16k: &[i16]) -> Option<(Option<String>, String)> {
        self.transcribe_timed(seq, pcm_16k, 0.0)
            .map(|(lang, text, _)| (lang, text))
    }

    fn transcribe_timed(
        &mut self,
        seq: u64,
        pcm_16k: &[i16],
        offset_secs: f64,
    ) -> Option<(Option<String>, String, Vec<TranscriptCue>)> {
        let chunk_dur = pcm_16k.len() as f64 / TARGET_RATE as f64;
        match self {
            ActiveSttBackend::Parakeet(model) => {
                let samples_f32: Vec<f32> =
                    pcm_16k.iter().map(|&s| f32::from(s) / 32_768.0).collect();
                let params = ParakeetParams {
                    language: None,
                    timestamp_granularity: Some(TimestampGranularity::Segment),
                };
                let res = model.transcribe_with(&samples_f32, &params).ok()?;
                let cues = parakeet_segments_to_cues(&res, offset_secs, chunk_dur);
                let text = if !cues.is_empty() {
                    TranscriptCue::plain_text(&cues)
                } else {
                    res.text.trim().to_owned()
                };
                if text.is_empty() || is_stt_artifact(&text) {
                    return None;
                }
                let lang = detect_spoken_lang(&text).map(str::to_owned);
                Some((lang, text, cues))
            }
            ActiveSttBackend::WhisperServer { url, http, .. } => {
                let wav_bytes = encode_wav_16khz_mono(pcm_16k);
                let url = url.clone();
                let http = http.clone();
                block_on_http(async move {
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
                    parse_whisper_json_cues(&body, offset_secs, chunk_dur)
                })
            }
            ActiveSttBackend::WhisperCli { bin, model } => {
                run_whisper_cli_utterance_timed(bin, model, seq, pcm_16k, offset_secs, chunk_dur)
            }
        }
    }
}

fn parakeet_segments_to_cues(
    res: &transcribe_rs::TranscriptionResult,
    offset_secs: f64,
    chunk_dur_secs: f64,
) -> Vec<TranscriptCue> {
    let offset = if offset_secs.is_finite() && offset_secs >= 0.0 {
        offset_secs
    } else {
        0.0
    };
    let mut cues = Vec::new();
    if let Some(segments) = res.segments.as_ref() {
        for seg in segments {
            let text = seg.text.trim();
            if text.is_empty() || is_stt_artifact(text) {
                continue;
            }
            let start = if seg.start.is_finite() && seg.start >= 0.0 {
                offset + f64::from(seg.start)
            } else {
                offset
            };
            let end = if seg.end.is_finite() && seg.end >= 0.0 {
                (offset + f64::from(seg.end)).max(start)
            } else {
                start
            };
            cues.push(TranscriptCue {
                start_secs: start,
                end_secs: end,
                text: text.to_owned(),
            });
        }
    }
    if cues.is_empty() {
        let fallback = res.text.trim();
        if !fallback.is_empty() && !is_stt_artifact(fallback) {
            let dur = if chunk_dur_secs.is_finite() && chunk_dur_secs > 0.0 {
                chunk_dur_secs
            } else {
                0.0
            };
            cues.push(TranscriptCue {
                start_secs: offset,
                end_secs: offset + dur,
                text: fallback.to_owned(),
            });
        }
    }
    cues
}

/// Background opt-in local multilingual transcriber for a live huddle.
/// Loads weights only while the huddle is active; drops completely on hangup (**0 MB RAM idle**)
/// and joins its worker thread / terminates any spawned `whisper-server` synchronously on drop.
pub struct LocalTranscriber {
    tx: Option<SyncSender<TranscribeChunk>>,
    server_child: Arc<Mutex<Option<Child>>>,
    worker: Option<std::thread::JoinHandle<()>>,
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
        let server_child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let worker_server_child = Arc::clone(&server_child);
        let (tx, rx) = mpsc::sync_channel::<TranscribeChunk>(64);
        let worker = std::thread::Builder::new()
            .name(format!("slackless-stt-{}", provider.source_tag()))
            .spawn(move || {
                let Some(mut backend) = init_stt_backend(provider, &setup, worker_server_child)
                else {
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
            })
            .ok()?;
        Some(Self {
            tx: Some(tx),
            server_child,
            worker: Some(worker),
        })
    }

    /// Non-blocking enqueue of a 48 kHz mono frame (drops silently under backpressure so real-time
    /// huddle audio never stalls).
    pub fn push(&self, is_local: bool, speaker: UserId, pcm_48k: &[f32]) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(TranscribeChunk {
                is_local,
                speaker,
                pcm_48k: pcm_48k.to_vec(),
            });
        }
    }
}

impl Drop for LocalTranscriber {
    fn drop(&mut self) {
        // Close channel first so the worker exits its receive loop, then kill any active
        // `whisper-server` child synchronously so an in-flight HTTP inference cannot hang drop.
        drop(self.tx.take());
        kill_whisper_server(&self.server_child);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// Load the Parakeet v3 INT8 model from `dir`, loading ONNX Runtime into the process first.
fn load_parakeet(dir: &Path) -> std::result::Result<ParakeetModel, String> {
    onnx_runtime::ensure_loaded()?;
    ParakeetModel::load(dir, &Quantization::Int8).map_err(|err| err.to_string())
}

fn init_stt_backend(
    provider: TranscriptionProvider,
    setup: &ProviderSetupStatus,
    server_child: Arc<Mutex<Option<Child>>>,
) -> Option<ActiveSttBackend> {
    match provider {
        TranscriptionProvider::SlackBuiltIn => None,
        TranscriptionProvider::ParakeetV3 => match load_parakeet(&setup.model_path) {
            Ok(model) => Some(ActiveSttBackend::Parakeet(Box::new(model))),
            Err(err) => {
                tracing::error!(%err, path = %setup.model_path.display(), "failed to load Parakeet v3 INT8 model");
                None
            }
        },
        TranscriptionProvider::WhisperTurbo => {
            if let Some(server_bin) = find_whisper_binary(&["whisper-server"])
                && let Some(srv) =
                    spawn_whisper_server(&server_bin, &setup.model_path, server_child)
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

fn spawn_whisper_server(
    bin: &Path,
    model: &Path,
    server_child: Arc<Mutex<Option<Child>>>,
) -> Option<ActiveSttBackend> {
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
    if let Ok(mut slot) = server_child.lock() {
        *slot = Some(child);
    }

    crate::signaling::install_default_crypto_provider();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .ok()?;

    // Wait up to 8s for `whisper-server` to finish loading the model onto Metal/CPU.
    let health_url = format!("http://127.0.0.1:{port}/");
    let http_probe = http.clone();
    let ready = block_on_http(async move {
        for _ in 0..80 {
            if http_probe.get(&health_url).send().await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    });
    if !ready {
        kill_whisper_server(&server_child);
        return None;
    }

    Some(ActiveSttBackend::WhisperServer {
        child: server_child,
        url: format!("http://127.0.0.1:{port}/inference"),
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

fn run_whisper_cli_utterance_timed(
    bin: &Path,
    model: &Path,
    seq: u64,
    pcm_16k: &[i16],
    offset_secs: f64,
    chunk_dur_secs: f64,
) -> Option<(Option<String>, String, Vec<TranscriptCue>)> {
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
    parse_whisper_json_cues(&raw_json, offset_secs, chunk_dur_secs)
}

// ---------------------------------------------------------------------------
// Offline Media File Audio Extraction & Local Transcription
// ---------------------------------------------------------------------------

/// Cap extracted offline audio at 120 minutes @ 16 kHz mono (115.2M samples).
const MAX_OFFLINE_AUDIO_SAMPLES: usize = TARGET_RATE * 60 * 120;
/// Maximum chunk duration fed to local STT models (~28s keeps Whisper & Parakeet fast & accurate).
const MAX_CHUNK_SAMPLES: usize = TARGET_RATE * 28;
/// Start searching for a low-energy silence boundary after 20s so words aren't cut mid-syllable.
const MIN_SPLIT_SEARCH_SAMPLES: usize = TARGET_RATE * 20;
/// Analysis window for silence boundary detection (100 ms @ 16 kHz).
const SPLIT_WINDOW_SAMPLES: usize = TARGET_RATE / 10;
/// Hop size for silence boundary scan (50 ms @ 16 kHz).
const SPLIT_HOP_SAMPLES: usize = TARGET_RATE / 20;

/// Decode a RIFF WAVE byte buffer (integer PCM of any depth or 32-bit IEEE float, any channel
/// count and sample rate) into 16 kHz mono `f32` samples in `[-1.0, 1.0]`.
fn decode_wav_bytes_to_16k_mono(bytes: &[u8]) -> std::result::Result<Vec<f32>, String> {
    let reader = hound::WavReader::new(bytes).map_err(|e| format!("unreadable WAV: {e}"))?;
    let spec = reader.spec();
    let mono = match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = 2f32.powi(1 - i32::from(spec.bits_per_sample));
            downmix_wav(reader, |s: i32| s as f32 * scale)
        }
        hound::SampleFormat::Float => {
            downmix_wav(reader, |s: f32| if s.is_finite() { s } else { 0.0 })
        }
    };
    let sample_rate = spec.sample_rate;

    let mut resampled = if sample_rate == TARGET_RATE as u32 {
        mono
    } else {
        // 100 ms blocks: a steep anti-aliasing filter, so nothing above 8 kHz folds into the
        // speech band; the converter's delay is irrelevant for a file.
        let mut converter =
            RateConverter::new(sample_rate, TARGET_RATE as u32, sample_rate as usize / 10)
                .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(converter.output_capacity(mono.len()));
        converter.process(&mono, &mut out);
        out
    };
    if resampled.len() > MAX_OFFLINE_AUDIO_SAMPLES {
        resampled.truncate(MAX_OFFLINE_AUDIO_SAMPLES);
    }
    Ok(resampled)
}

/// Average each interleaved frame of `reader`'s samples into one mono sample. A truncated data
/// chunk ends the audio early rather than failing the whole file.
fn downmix_wav<S: hound::Sample>(
    mut reader: hound::WavReader<&[u8]>,
    to_f32: impl Fn(S) -> f32,
) -> Vec<f32> {
    let channels = usize::from(reader.spec().channels);
    let mut mono = Vec::with_capacity(reader.duration() as usize);
    let mut sum = 0.0f32;
    for (i, sample) in reader.samples::<S>().enumerate() {
        match sample {
            Ok(s) => sum += to_f32(s),
            Err(err) => {
                tracing::warn!(%err, "WAV data ends early; decoding the readable part");
                break;
            }
        }
        if (i + 1) % channels == 0 {
            mono.push((sum / channels as f32).clamp(-1.0, 1.0));
            sum = 0.0;
        }
    }
    mono
}

#[cfg(target_os = "macos")]
fn extract_audio_16k_mono_macos(path: &Path) -> Option<Vec<f32>> {
    use std::ffi::CString;

    let path_str = path.to_str()?;
    let c_path = CString::new(path_str).ok()?;
    let mut ptr: *mut f32 = std::ptr::null_mut();
    let mut len: usize = 0;

    // SAFETY: `c_path` is a valid NUL-terminated UTF-8 string; `ptr` and `len` are stack locals.
    let ok = unsafe {
        crate::macos::ffi::slackless_extract_audio_16k_mono(c_path.as_ptr(), &mut ptr, &mut len)
    };
    if !ok {
        return None;
    }
    if ptr.is_null() || len == 0 {
        if !ptr.is_null() {
            // SAFETY: `ptr` was allocated by `slackless_extract_audio_16k_mono`.
            unsafe { crate::macos::ffi::slackless_free_f32_buf(ptr) };
        }
        return Some(Vec::new());
    }
    // SAFETY: `ptr` is valid for `len` initialized `f32` samples until freed below.
    let samples = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    // SAFETY: `ptr` was allocated with `malloc`/`realloc` inside `slackless_extract_audio_16k_mono`.
    unsafe { crate::macos::ffi::slackless_free_f32_buf(ptr) };
    Some(samples)
}

#[cfg(not(target_os = "macos"))]
fn extract_audio_16k_mono_ffmpeg(path: &Path) -> std::result::Result<Vec<f32>, String> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("ffmpeg unavailable for decoding {}: {e}", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg failed to extract audio from {}: {}",
            path.display(),
            stderr.trim()
        ));
    }
    let mut samples = Vec::with_capacity((output.stdout.len() / 4).min(MAX_OFFLINE_AUDIO_SAMPLES));
    for chunk in output
        .stdout
        .chunks_exact(4)
        .take(MAX_OFFLINE_AUDIO_SAMPLES)
    {
        let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        samples.push(if s.is_finite() {
            s.clamp(-1.0, 1.0)
        } else {
            0.0
        });
    }
    Ok(samples)
}

/// Extract audio from any supported media file (MP4, MOV, M4A, MP3, WAV, AAC, etc.) as 16 kHz mono
/// `f32` PCM samples suitable for local transcription.
///
/// On macOS this uses native `AVAssetReader` (falling back to a pure-Rust WAV decoder if needed).
/// On non-macOS platforms this decodes PCM/float WAV files directly and falls back to `ffmpeg`.
pub fn extract_audio_16k_mono(path: &Path) -> std::result::Result<Vec<f32>, String> {
    if !path.is_file() {
        return Err(format!("media file not found: {}", path.display()));
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(samples) = extract_audio_16k_mono_macos(path) {
            return Ok(samples);
        }
        let bytes =
            std::fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            return decode_wav_bytes_to_16k_mono(&bytes);
        }
        Err(format!("no readable audio track in {}", path.display()))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let bytes =
            std::fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            return decode_wav_bytes_to_16k_mono(&bytes);
        }
        extract_audio_16k_mono_ffmpeg(path)
    }
}

/// Resolve which ready local transcription provider to use for offline media file transcription.
///
/// Prefers `preferred` when it is a local provider that is ready on disk; otherwise falls back to
/// [`TranscriptionProvider::ParakeetV3`] and then [`TranscriptionProvider::WhisperTurbo`]. Returns
/// `None` if no local model is downloaded and ready yet.
pub fn resolve_local_file_provider(
    preferred: TranscriptionProvider,
) -> Option<TranscriptionProvider> {
    if preferred.is_local() && detect_provider_setup(preferred).is_ready() {
        return Some(preferred);
    }
    [
        TranscriptionProvider::ParakeetV3,
        TranscriptionProvider::WhisperTurbo,
    ]
    .into_iter()
    .find(|&candidate| candidate != preferred && detect_provider_setup(candidate).is_ready())
}

/// Result of offline media transcription preserving both plain searchable text and timed cues.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedTranscription {
    pub provider: TranscriptionProvider,
    pub text: String,
    pub cues: Vec<TranscriptCue>,
}

/// Split 16 kHz mono audio into `(start_sample, chunk)` slices of at most 28 seconds, preferring
/// the lowest-energy silence boundary between 20s and 28s so words are not cut in half.
pub fn chunk_audio_for_transcription_with_offsets(samples: &[f32]) -> Vec<(usize, &[f32])> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < samples.len() {
        let remaining = samples.len() - start;
        if remaining <= MAX_CHUNK_SAMPLES {
            chunks.push((start, &samples[start..]));
            break;
        }

        let search_start = start + MIN_SPLIT_SEARCH_SAMPLES;
        let search_end = start + MAX_CHUNK_SAMPLES;
        let mut best_split = search_end;
        let mut best_energy = f32::INFINITY;

        let mut win_start = search_start;
        while win_start + SPLIT_WINDOW_SAMPLES <= search_end {
            let window = &samples[win_start..win_start + SPLIT_WINDOW_SAMPLES];
            let energy: f32 = window.iter().map(|&s| s * s).sum::<f32>() / window.len() as f32;
            if energy < best_energy {
                best_energy = energy;
                best_split = win_start + SPLIT_WINDOW_SAMPLES / 2;
            }
            win_start += SPLIT_HOP_SAMPLES;
        }

        chunks.push((start, &samples[start..best_split]));
        start = best_split;
    }

    chunks
}

/// Split 16 kHz mono audio into chunks of at most 28 seconds, preferring the lowest-energy
/// silence boundary between 20s and 28s so words are not cut in half.
#[cfg(test)]
fn chunk_audio_for_transcription(samples: &[f32]) -> Vec<&[f32]> {
    chunk_audio_for_transcription_with_offsets(samples)
        .into_iter()
        .map(|(_, slice)| slice)
        .collect()
}

fn is_effectively_silent(samples: &[f32]) -> bool {
    samples.iter().all(|&s| s.abs() < 1e-4)
}

/// Transcribe an offline audio or video file using a locally downloaded STT model (`ParakeetV3`
/// or `WhisperTurbo`), preserving per-segment cue timestamps (`start_secs`, `end_secs`, `text`)
/// alongside the combined plain text. Loads model weights only for the duration of this call
/// (0 MB idle RAM) and tears down any spawned `whisper-server` cleanly before returning.
pub fn transcribe_media_file_timed(
    path: &Path,
    preferred: TranscriptionProvider,
) -> std::result::Result<TimedTranscription, String> {
    let Some(provider) = resolve_local_file_provider(preferred) else {
        // A downloaded Parakeet model that only lacks its runtime is the more useful thing to report.
        if detect_provider_setup(TranscriptionProvider::ParakeetV3).model_ready {
            onnx_runtime::locate()?;
        }
        return Err(
            "No local transcription model is downloaded yet. Download Parakeet v3 or Whisper Turbo in Preferences → Audio & Video."
                .to_owned(),
        );
    };

    let samples = extract_audio_16k_mono(path)?;
    if samples.is_empty() || is_effectively_silent(&samples) {
        return Ok(TimedTranscription {
            provider,
            text: String::new(),
            cues: Vec::new(),
        });
    }

    let chunks = chunk_audio_for_transcription_with_offsets(&samples);
    let setup = detect_provider_setup(provider);
    let mut parts: Vec<String> = Vec::with_capacity(chunks.len());
    let mut cues: Vec<TranscriptCue> = Vec::new();

    match provider {
        TranscriptionProvider::ParakeetV3 => {
            let mut model = load_parakeet(&setup.model_path)
                .map_err(|e| format!("failed to load Parakeet v3 model: {e}"))?;
            let params = ParakeetParams {
                language: None,
                timestamp_granularity: Some(TimestampGranularity::Segment),
            };
            for (start_sample, chunk) in chunks {
                if is_effectively_silent(chunk) {
                    continue;
                }
                let offset_secs = start_sample as f64 / TARGET_RATE as f64;
                let chunk_dur_secs = chunk.len() as f64 / TARGET_RATE as f64;
                let res = model
                    .transcribe_with(chunk, &params)
                    .map_err(|e| format!("Parakeet transcription failed: {e}"))?;
                let chunk_cues = parakeet_segments_to_cues(&res, offset_secs, chunk_dur_secs);
                if !chunk_cues.is_empty() {
                    parts.push(TranscriptCue::plain_text(&chunk_cues));
                    cues.extend(chunk_cues);
                } else {
                    let text = res.text.trim();
                    if !text.is_empty() && !is_stt_artifact(text) {
                        parts.push(text.to_owned());
                    }
                }
            }
            drop(model);
        }
        TranscriptionProvider::WhisperTurbo => {
            let server_child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
            let mut backend =
                init_stt_backend(provider, &setup, server_child).ok_or_else(|| {
                    "failed to initialize local Whisper transcription backend".to_owned()
                })?;
            for (idx, (start_sample, chunk)) in chunks.into_iter().enumerate() {
                if is_effectively_silent(chunk) {
                    continue;
                }
                let offset_secs = start_sample as f64 / TARGET_RATE as f64;
                let pcm_16k: Vec<i16> = chunk
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect();
                if let Some((_, text, chunk_cues)) =
                    backend.transcribe_timed(idx as u64 + 1, &pcm_16k, offset_secs)
                    && !text.is_empty()
                {
                    parts.push(text);
                    cues.extend(chunk_cues);
                }
            }
            drop(backend);
        }
        TranscriptionProvider::SlackBuiltIn => {
            return Err(
                "Slack built-in provider does not support offline file transcription.".to_owned(),
            );
        }
    }

    let text = if !cues.is_empty() {
        TranscriptCue::plain_text(&cues)
    } else {
        parts.join(" ")
    };
    Ok(TimedTranscription {
        provider,
        text,
        cues,
    })
}

/// Transcribe an offline audio or video file using a locally downloaded STT model (`ParakeetV3`
/// or `WhisperTurbo`). Loads model weights only for the duration of this call (0 MB idle RAM) and
/// tears down any spawned `whisper-server` cleanly before returning.
pub fn transcribe_media_file(
    path: &Path,
    preferred: TranscriptionProvider,
) -> std::result::Result<(TranscriptionProvider, String), String> {
    let timed = transcribe_media_file_timed(path, preferred)?;
    Ok((timed.provider, timed.text))
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

    #[test]
    fn parse_whisper_json_cues_extracts_cli_and_server_timestamps_with_offset() {
        let cli_json = r#"{
            "result": { "language": "en" },
            "transcription": [
                { "offsets": { "from": 0, "to": 1800 }, "text": " First sentence." },
                { "offsets": { "from": 1800, "to": 3200 }, "text": " [MUSIC] " },
                { "timestamps": { "from": "00:00:03,200", "to": "00:00:05,700" }, "text": " Second sentence." }
            ]
        }"#;
        let (lang, text, cues) =
            parse_whisper_json_cues(cli_json, 20.0, 6.0).expect("parse whisper-cli JSON");
        assert_eq!(lang.as_deref(), Some("en"));
        assert_eq!(text, "First sentence. Second sentence.");
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start_secs - 20.0).abs() < 1e-6);
        assert!((cues[0].end_secs - 21.8).abs() < 1e-6);
        assert_eq!(cues[0].text, "First sentence.");
        assert!((cues[1].start_secs - 23.2).abs() < 1e-6);
        assert!((cues[1].end_secs - 25.7).abs() < 1e-6);
        assert_eq!(cues[1].text, "Second sentence.");

        let server_verbose = r#"{
            "language": "sv",
            "segments": [
                { "start": 0.25, "end": 2.5, "text": " Hej alla!" },
                { "start": 2.5, "end": 5.0, "text": " Nu kör vi." }
            ]
        }"#;
        let (lang, text, cues) =
            parse_whisper_json_cues(server_verbose, 10.0, 5.0).expect("parse whisper-server JSON");
        assert_eq!(lang.as_deref(), Some("sv"));
        assert_eq!(text, "Hej alla! Nu kör vi.");
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start_secs - 10.25).abs() < 1e-6);
        assert!((cues[0].end_secs - 12.5).abs() < 1e-6);
        assert!((cues[1].start_secs - 12.5).abs() < 1e-6);
        assert!((cues[1].end_secs - 15.0).abs() < 1e-6);

        let plain_only = r#"{ "language": "en", "text": "Fallback line" }"#;
        let (_, _, cues) = parse_whisper_json_cues(plain_only, 4.0, 3.0).expect("fallback cue");
        assert_eq!(cues.len(), 1);
        assert!((cues[0].start_secs - 4.0).abs() < 1e-6);
        assert!((cues[0].end_secs - 7.0).abs() < 1e-6);
        assert_eq!(cues[0].text, "Fallback line");
    }

    #[test]
    fn parakeet_segments_and_chunk_offsets_preserve_timestamps() {
        let res = transcribe_rs::TranscriptionResult {
            text: "Hello world. Ship it.".into(),
            segments: Some(vec![
                transcribe_rs::TranscriptionSegment {
                    start: 0.2,
                    end: 1.4,
                    text: "Hello world.".into(),
                },
                transcribe_rs::TranscriptionSegment {
                    start: 1.5,
                    end: 2.9,
                    text: "Ship it.".into(),
                },
            ]),
        };
        let cues = parakeet_segments_to_cues(&res, 25.0, 4.0);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start_secs - 25.2).abs() < 1e-4);
        assert!((cues[0].end_secs - 26.4).abs() < 1e-4);
        assert!((cues[1].start_secs - 26.5).abs() < 1e-4);
        assert!((cues[1].end_secs - 27.9).abs() < 1e-4);

        // 40 seconds of audio splits into 2 chunks with contiguous sample offsets.
        let samples = vec![0.1f32; TARGET_RATE * 40];
        let chunks = chunk_audio_for_transcription_with_offsets(&samples);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].0, 0);
        assert_eq!(chunks[1].0, chunks[0].1.len());
        assert_eq!(chunks[0].1.len() + chunks[1].1.len(), samples.len());
    }

    #[test]
    fn extract_audio_16k_mono_decodes_16khz_and_44100hz_wav() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();

        // 1) 1.0s 16 kHz mono 440 Hz sine wave WAV.
        let pcm_16k: Vec<i16> = (0..16_000)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 16_000.0) as i16
            })
            .collect();
        let wav_16k = crate::clips::encode_wav_pcm16(&pcm_16k, 16_000, 1);
        let path_16k = dir.join(format!("slackless-extract-16k-{pid}.wav"));
        std::fs::write(&path_16k, &wav_16k).expect("write 16k WAV");

        let extracted_16k = extract_audio_16k_mono(&path_16k).expect("extract 16k WAV");
        let _ = std::fs::remove_file(&path_16k);
        assert!(
            (extracted_16k.len() as isize - 16_000).abs() <= 256,
            "expected ~16000 samples, got {}",
            extracted_16k.len()
        );
        let rms_16k =
            (extracted_16k.iter().map(|&s| s * s).sum::<f32>() / extracted_16k.len() as f32).sqrt();
        assert!(
            rms_16k > 0.2 && rms_16k < 0.6,
            "expected ~0.35 RMS for 16k sine, got {rms_16k}"
        );

        // 2) 1.0s 44.1 kHz stereo 440 Hz sine wave WAV -> downmixed & resampled to 16 kHz mono.
        let mut pcm_44k_stereo = Vec::with_capacity(44_100 * 2);
        for i in 0..44_100 {
            let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 16_000.0) as i16;
            pcm_44k_stereo.push(s);
            pcm_44k_stereo.push(s);
        }
        let wav_44k = crate::clips::encode_wav_pcm16(&pcm_44k_stereo, 44_100, 2);
        let path_44k = dir.join(format!("slackless-extract-44k-{pid}.wav"));
        std::fs::write(&path_44k, &wav_44k).expect("write 44.1k stereo WAV");

        let extracted_44k = extract_audio_16k_mono(&path_44k).expect("extract 44.1k stereo WAV");
        let _ = std::fs::remove_file(&path_44k);
        assert!(
            (extracted_44k.len() as isize - 16_000).abs() <= 512,
            "expected ~16000 resampled samples, got {}",
            extracted_44k.len()
        );
        let rms_44k =
            (extracted_44k.iter().map(|&s| s * s).sum::<f32>() / extracted_44k.len() as f32).sqrt();
        assert!(
            rms_44k > 0.2 && rms_44k < 0.6,
            "expected ~0.35 RMS for 44.1k stereo sine, got {rms_44k}"
        );

        // 3) Pure-Rust WAV fallback decoder matches on 44.1 kHz stereo directly.
        let fallback_44k = decode_wav_bytes_to_16k_mono(&wav_44k).expect("pure-Rust WAV decode");
        assert_eq!(fallback_44k.len(), 16_000);

        // 4) ...and reads 24-bit PCM (written as WAVE_FORMAT_EXTENSIBLE) at the same scale.
        let spec_24 = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav_24 = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut wav_24, spec_24).unwrap();
        for &s in &pcm_16k {
            writer.write_sample(i32::from(s) << 8).unwrap();
        }
        writer.finalize().unwrap();
        let decoded_24 = decode_wav_bytes_to_16k_mono(wav_24.get_ref()).expect("24-bit WAV decode");
        assert_eq!(decoded_24.len(), pcm_16k.len());
        assert!((decoded_24[100] - f32::from(pcm_16k[100]) / 32_768.0).abs() < 1e-6);
        let missing = dir.join(format!("slackless-extract-missing-{pid}.wav"));
        assert!(extract_audio_16k_mono(&missing).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extract_audio_16k_mono_decodes_aac_h264_mp4_and_rejects_silent_video() {
        use crate::clips::{encode_wav_pcm16, mux_h264_aac_mp4, mux_h264_mp4, parse_adts_frames};
        use crate::video::H264VideoEncoder;

        let dir = std::env::temp_dir();
        let pid = std::process::id();

        // Build 1.0s of H.264 video frames.
        let w = 160usize;
        let h = 96usize;
        let fps = 12u32;
        let mut enc = H264VideoEncoder::new_screen().expect("openh264 encoder");
        let rgb = vec![128u8; w * h * 3];
        let frames: Vec<Vec<u8>> = (0..12)
            .map(|_| enc.encode_rgb(&rgb, w, h).expect("encode H.264"))
            .collect();

        // 1) Silent H.264-only MP4 has no audio track -> returns Err cleanly.
        let silent_mp4 = mux_h264_mp4(w as u32, h as u32, fps, &frames).expect("mux silent MP4");
        let silent_path = dir.join(format!("slackless-extract-silent-{pid}.mp4"));
        std::fs::write(&silent_path, &silent_mp4).expect("write silent MP4");
        assert!(
            extract_audio_16k_mono(&silent_path).is_err(),
            "video without audio track should fail cleanly"
        );
        let _ = std::fs::remove_file(&silent_path);

        // 2) H.264 + AAC MP4 decoded natively via macOS AVAssetReader.
        let tone_pcm: Vec<i16> = (0..48_000)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 14_000.0) as i16
            })
            .collect();
        let wav_path = dir.join(format!("slackless-extract-src-{pid}.wav"));
        let adts_path = dir.join(format!("slackless-extract-src-{pid}.aac"));
        std::fs::write(&wav_path, encode_wav_pcm16(&tone_pcm, 48_000, 1)).expect("write tone WAV");
        let status = Command::new("/usr/bin/afconvert")
            .args(["-f", "adts", "-d", "aac", "-b", "96000"])
            .arg(&wav_path)
            .arg(&adts_path)
            .status()
            .expect("invoke afconvert");
        let _ = std::fs::remove_file(&wav_path);
        assert!(status.success(), "afconvert to ADTS AAC failed");
        let adts_bytes = std::fs::read(&adts_path).expect("read ADTS AAC");
        let _ = std::fs::remove_file(&adts_path);
        let aac_track = parse_adts_frames(&adts_bytes).expect("parse ADTS AAC frames");

        let av_mp4 = mux_h264_aac_mp4(w as u32, h as u32, fps, &frames, Some(&aac_track))
            .expect("mux A/V MP4");
        let av_path = dir.join(format!("slackless-extract-av-{pid}.mp4"));
        std::fs::write(&av_path, &av_mp4).expect("write A/V MP4");

        let samples = extract_audio_16k_mono(&av_path).expect("extract audio from H.264+AAC MP4");
        let _ = std::fs::remove_file(&av_path);
        assert!(
            samples.len() >= 14_000 && samples.len() <= 18_500,
            "expected ~16000 samples (~1.0s), got {}",
            samples.len()
        );
        let rms = (samples.iter().map(|&s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        assert!(
            rms > 0.15 && rms < 0.6,
            "expected audible 440 Hz tone from AAC track, got rms={rms}"
        );
    }

    #[test]
    fn chunk_audio_for_transcription_respects_max_len_and_prefers_silence() {
        assert!(chunk_audio_for_transcription(&[]).is_empty());

        // Short audio (<= 28s) stays a single chunk.
        let short = vec![0.25f32; TARGET_RATE * 10];
        let short_chunks = chunk_audio_for_transcription(&short);
        assert_eq!(short_chunks.len(), 1);
        assert_eq!(short_chunks[0].len(), short.len());

        // 50s audio with a clear 1s silence gap at 23s..24s: split must land inside 23s..24s.
        let mut with_gap = vec![0.3f32; TARGET_RATE * 50];
        let gap_start = TARGET_RATE * 23;
        let gap_end = TARGET_RATE * 24;
        with_gap[gap_start..gap_end].fill(0.0);

        let gap_chunks = chunk_audio_for_transcription(&with_gap);
        assert_eq!(gap_chunks.len(), 2);
        assert!(
            gap_chunks[0].len() >= gap_start && gap_chunks[0].len() <= gap_end,
            "expected first split inside 23s..24s silence ({gap_start}..{gap_end}), got {}",
            gap_chunks[0].len()
        );
        assert_eq!(
            gap_chunks.iter().map(|c| c.len()).sum::<usize>(),
            with_gap.len()
        );
        for chunk in &gap_chunks {
            assert!(!chunk.is_empty());
            assert!(chunk.len() <= MAX_CHUNK_SAMPLES);
        }

        // 75s continuous speech -> lossless partitioning, every chunk <= 28s.
        let continuous: Vec<f32> = (0..TARGET_RATE * 75)
            .map(|i| (i as f32 * 0.01).sin() * 0.4)
            .collect();
        let cont_chunks = chunk_audio_for_transcription(&continuous);
        assert!(cont_chunks.len() >= 3);
        let mut rebuilt = Vec::with_capacity(continuous.len());
        for chunk in &cont_chunks {
            assert!(!chunk.is_empty());
            assert!(chunk.len() <= MAX_CHUNK_SAMPLES);
            rebuilt.extend_from_slice(chunk);
        }
        assert_eq!(rebuilt, continuous);
    }
}
