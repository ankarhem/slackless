//! Local ONNX sentence embeddings (`all-MiniLM-L6-v2` / `bge-small-en-v1.5`), pure-Rust
//! WordPiece tokenizer, deterministic hash-projection fallback/test embedder, and opt-in
//! Hugging Face model download/status management.
//!
//! Model files stream to disk through [`AssetClient::download_to_path`] (atomic `.part` ->
//! rename). Zero external binaries, zero telemetry, and zero network calls during
//! unit/integration tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use slackless_store::l2_normalize;

use crate::api::AssetClient;
use crate::paths;
use crate::runtime::block_on;

/// Largest model file accepted; the INT8 weights are ~23-33 MB.
const MAX_MODEL_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Supported local ONNX sentence embedding models (quantized INT8 for compact disk & fast CPU).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmbeddingModel {
    /// `Xenova/all-MiniLM-L6-v2` quantized INT8 (~23 MB, 384-dim, 256 token context).
    #[default]
    AllMiniLmL6V2,
    /// `Xenova/bge-small-en-v1.5` quantized INT8 (~33 MB, 384-dim, 512 token context).
    BgeSmallEnV15,
}

impl EmbeddingModel {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::AllMiniLmL6V2 => "all-MiniLM-L6-v2",
            Self::BgeSmallEnV15 => "bge-small-en-v1.5",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::AllMiniLmL6V2 => "all-MiniLM-L6-v2 (INT8, ~23 MB)",
            Self::BgeSmallEnV15 => "bge-small-en-v1.5 (INT8, ~33 MB)",
        }
    }

    pub const fn dims(self) -> usize {
        384
    }

    pub const fn max_seq_len(self) -> usize {
        match self {
            Self::AllMiniLmL6V2 => 256,
            Self::BgeSmallEnV15 => 512,
        }
    }

    pub const fn download_size_hint(self) -> &'static str {
        match self {
            Self::AllMiniLmL6V2 => "~23 MB",
            Self::BgeSmallEnV15 => "~33 MB",
        }
    }

    /// Optional instruction prefix prepended to search queries (BGE v1.5 retrieval convention).
    pub const fn query_prefix(self) -> Option<&'static str> {
        match self {
            Self::AllMiniLmL6V2 => None,
            Self::BgeSmallEnV15 => {
                Some("Represent this sentence for searching relevant passages: ")
            }
        }
    }

    /// Whether pooling uses the `[CLS]` token (BGE) vs attention-masked mean pooling (MiniLM).
    pub const fn uses_cls_pooling(self) -> bool {
        matches!(self, Self::BgeSmallEnV15)
    }

    /// Parse a CLI/config model identifier (`minilm`, `all-minilm-l6-v2`, `bge-small`, etc.).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minilm" | "all-minilm" | "all-minilm-l6-v2" | "default" => Some(Self::AllMiniLmL6V2),
            "bge" | "bge-small" | "bge-small-en" | "bge-small-en-v1.5" => Some(Self::BgeSmallEnV15),
            _ => None,
        }
    }

    fn download_files(self) -> Vec<(String, &'static str)> {
        let base = match self {
            Self::AllMiniLmL6V2 => "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main",
            Self::BgeSmallEnV15 => "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main",
        };
        vec![
            (format!("{base}/vocab.txt"), "vocab.txt"),
            (
                format!("{base}/onnx/model_quantized.onnx"),
                "model_quantized.onnx",
            ),
        ]
    }
}

/// Disk & readiness status for one local [`EmbeddingModel`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingModelStatus {
    pub model: EmbeddingModel,
    pub model_ready: bool,
    pub model_path: PathBuf,
    pub disk_bytes: u64,
    pub dims: usize,
    pub download_size_hint: String,
}

/// Incremental progress update emitted by [`download_embedding_model`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingDownloadProgress {
    pub file_name: String,
    pub file_index: usize,
    pub file_count: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// Default directory for one embedding model (`<app_data_dir>/models/embeddings/<slug>`),
/// honoring `SLACKLESS_EMBEDDING_MODEL_DIR` when set.
pub fn default_embedding_model_dir(model: EmbeddingModel) -> PathBuf {
    if let Ok(override_dir) = std::env::var("SLACKLESS_EMBEDDING_MODEL_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    paths::semantic_models_dir()
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("slackless")
                .join("models")
                .join("embeddings")
        })
        .join(model.slug())
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

fn resolve_onnx_file(dir: &Path) -> Option<PathBuf> {
    for name in ["model_quantized.onnx", "model.onnx"] {
        let p = dir.join(name);
        if p.is_file() && p.metadata().is_ok_and(|m| m.len() > 0) {
            return Some(p);
        }
    }
    None
}

fn embedding_files_present(dir: &Path) -> bool {
    let vocab = dir.join("vocab.txt");
    vocab.is_file()
        && vocab.metadata().is_ok_and(|m| m.len() > 0)
        && resolve_onnx_file(dir).is_some()
}

/// Inspect whether `model` weights + `vocab.txt` are present on disk.
pub fn detect_embedding_model_setup(model: EmbeddingModel) -> EmbeddingModelStatus {
    let dir = default_embedding_model_dir(model);
    let ready = embedding_files_present(&dir);
    EmbeddingModelStatus {
        model,
        model_ready: ready,
        disk_bytes: dir_disk_bytes(&dir),
        model_path: dir,
        dims: model.dims(),
        download_size_hint: model.download_size_hint().to_owned(),
    }
}

/// Download the quantized ONNX model and `vocab.txt` for `model` atomically (`.part` -> rename).
/// Blocking helper — call on a background executor/thread.
pub fn download_embedding_model(
    model: EmbeddingModel,
    on_progress: impl Fn(EmbeddingDownloadProgress) + Send + Sync + 'static,
) -> std::result::Result<PathBuf, String> {
    block_on(download_embedding_model_async(model, on_progress))
        .map_err(|e| format!("failed to start async HTTP runtime: {e}"))?
}

/// Async version of [`download_embedding_model`] for CLI / Tokio callers.
pub async fn download_embedding_model_async(
    model: EmbeddingModel,
    on_progress: impl Fn(EmbeddingDownloadProgress) + Send + Sync + 'static,
) -> std::result::Result<PathBuf, String> {
    let dest_dir = default_embedding_model_dir(model);
    let files = model.download_files();
    download_files_to_dir(&dest_dir, files, on_progress).await?;
    Ok(dest_dir)
}

async fn download_files_to_dir(
    dest_dir: &Path,
    files: Vec<(String, &'static str)>,
    on_progress: impl Fn(EmbeddingDownloadProgress) + Send + Sync + 'static,
) -> std::result::Result<(), String> {
    crate::api::install_default_crypto_provider();
    let client = reqwest::Client::builder()
        .user_agent("slackless/1.0")
        .build()
        .map_err(|e| format!("http client error: {e}"))?;
    let assets = AssetClient::with_http_client(client);

    std::fs::create_dir_all(dest_dir)
        .map_err(|e| format!("failed to create {}: {e}", dest_dir.display()))?;

    let file_count = files.len();
    for (idx, (url, name)) in files.into_iter().enumerate() {
        let progress = |downloaded_bytes, total_bytes| {
            on_progress(EmbeddingDownloadProgress {
                file_name: name.to_owned(),
                file_index: idx + 1,
                file_count,
                downloaded_bytes,
                total_bytes,
            });
        };
        let final_path = dest_dir.join(name);
        if final_path.is_file() && final_path.metadata().is_ok_and(|m| m.len() > 0) {
            let len = final_path.metadata().map_or(0, |m| m.len());
            progress(len, Some(len));
            continue;
        }
        let downloaded = assets
            .download_to_path(&url, &final_path, MAX_MODEL_FILE_BYTES, &progress)
            .await
            .map_err(|e| format!("download failed for {name}: {e}"))?;
        if downloaded == 0 {
            // An empty file counts as missing, so the next attempt downloads it again.
            return Err(format!("downloaded 0 bytes for {name}"));
        }
        progress(downloaded, Some(downloaded));
    }
    Ok(())
}

/// Remove locally downloaded model weights for `model` and return reclaimed bytes.
pub fn delete_embedding_model(model: EmbeddingModel) -> std::result::Result<u64, String> {
    let status = detect_embedding_model_setup(model);
    let reclaimed = status.disk_bytes;
    if status.model_path.exists() {
        std::fs::remove_dir_all(&status.model_path)
            .map_err(|e| format!("failed to remove {}: {e}", status.model_path.display()))?;
    }
    Ok(reclaimed)
}

/// Encoded token sequence ready for BERT/MiniLM/BGE ONNX Runtime inference.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenizedInput {
    input_ids: Vec<i64>,
    attention_mask: Vec<i64>,
    token_type_ids: Vec<i64>,
}

/// Pure-Rust BERT / WordPiece tokenizer compatible with Hugging Face `vocab.txt` files
/// (`all-MiniLM-L6-v2`, `bge-small-en-v1.5`, BERT-uncased).
#[derive(Debug, Clone)]
struct WordPieceTokenizer {
    vocab: HashMap<String, i64>,
    unk_id: i64,
    cls_id: i64,
    sep_id: i64,
    pad_id: i64,
    max_input_chars_per_word: usize,
    do_lower_case: bool,
}

impl WordPieceTokenizer {
    /// Load a `vocab.txt` file (one token per line; 0-based line number = token ID).
    pub fn from_file(path: &Path) -> std::result::Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read vocab {}: {e}", path.display()))?;
        Self::from_vocab_str(&raw)
    }

    /// Build a tokenizer directly from `vocab.txt` text content.
    pub fn from_vocab_str(contents: &str) -> std::result::Result<Self, String> {
        let mut vocab = HashMap::new();
        for (idx, line) in contents.lines().enumerate() {
            let token = line.trim_end_matches('\r');
            if !token.is_empty() {
                vocab.insert(token.to_owned(), idx as i64);
            }
        }
        if vocab.is_empty() {
            return Err("WordPiece vocabulary is empty".to_owned());
        }
        let unk_id = vocab.get("[UNK]").copied().unwrap_or(100);
        let cls_id = vocab.get("[CLS]").copied().unwrap_or(101);
        let sep_id = vocab.get("[SEP]").copied().unwrap_or(102);
        let pad_id = vocab.get("[PAD]").copied().unwrap_or(0);
        Ok(Self {
            vocab,
            unk_id,
            cls_id,
            sep_id,
            pad_id,
            max_input_chars_per_word: 100,
            do_lower_case: true,
        })
    }

    #[cfg(test)]
    fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    pub fn pad_id(&self) -> i64 {
        self.pad_id
    }

    /// Encode `text` into `[CLS] <wordpiece tokens> [SEP]`, truncated to `max_len` (min 2).
    pub fn encode(&self, text: &str, max_len: usize) -> TokenizedInput {
        let cap = max_len.max(2);
        let max_body = cap - 2;
        let mut ids = Vec::with_capacity(cap.min(64));
        ids.push(self.cls_id);

        for word in basic_tokenize(text, self.do_lower_case) {
            if ids.len() > max_body {
                break;
            }
            for sub_id in self.wordpiece_tokenize(&word) {
                if ids.len() > max_body {
                    break;
                }
                ids.push(sub_id);
            }
        }
        ids.push(self.sep_id);

        let len = ids.len();
        TokenizedInput {
            input_ids: ids,
            attention_mask: vec![1; len],
            token_type_ids: vec![0; len],
        }
    }

    fn wordpiece_tokenize(&self, word: &str) -> Vec<i64> {
        let chars: Vec<char> = word.chars().collect();
        if chars.is_empty() {
            return Vec::new();
        }
        if chars.len() > self.max_input_chars_per_word {
            return vec![self.unk_id];
        }
        let mut out = Vec::new();
        let mut start = 0usize;
        let mut buf = String::with_capacity(word.len() + 2);
        while start < chars.len() {
            let mut end = chars.len();
            let mut matched: Option<i64> = None;
            while start < end {
                buf.clear();
                if start > 0 {
                    buf.push_str("##");
                }
                for &ch in &chars[start..end] {
                    buf.push(ch);
                }
                if let Some(&id) = self.vocab.get(buf.as_str()) {
                    matched = Some(id);
                    break;
                }
                end -= 1;
            }
            match matched {
                Some(id) => {
                    out.push(id);
                    start = end;
                }
                None => return vec![self.unk_id],
            }
        }
        out
    }
}

/// BERT BasicTokenizer: cleans control chars, isolates CJK ideographs, lowercases & folds
/// common Latin accents when `do_lower_case`, and splits on whitespace & punctuation.
fn basic_tokenize(text: &str, do_lower_case: bool) -> Vec<String> {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\0' || ch == '\u{FFFD}' || is_control(ch) {
            continue;
        }
        if ch.is_whitespace() {
            cleaned.push(' ');
        } else if is_cjk(ch) {
            cleaned.push(' ');
            cleaned.push(ch);
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }

    let mut tokens = Vec::new();
    for raw in cleaned.split_whitespace() {
        let normalized = if do_lower_case {
            fold_bert_uncased(raw)
        } else {
            raw.to_owned()
        };
        let mut current = String::new();
        for ch in normalized.chars() {
            if is_bert_punctuation(ch) {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
    }
    tokens
}

fn is_control(ch: char) -> bool {
    if ch == '\t' || ch == '\n' || ch == '\r' {
        return false;
    }
    ch.is_control()
}

fn is_bert_punctuation(ch: char) -> bool {
    let cp = ch as u32;
    (33..=47).contains(&cp)
        || (58..=64).contains(&cp)
        || (91..=96).contains(&cp)
        || (123..=126).contains(&cp)
        || (!ch.is_alphanumeric() && !ch.is_whitespace())
}

fn is_cjk(ch: char) -> bool {
    let cp = ch as u32;
    (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0x2F800..=0x2FA1F).contains(&cp)
}

/// BERT uncased lowercasing + Latin diacritic stripping (zero extra crates; handles Swedish
/// `å`/`ä`/`ö`, Western European accents, and Unicode combining marks cleanly).
fn fold_bert_uncased(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars().flat_map(char::to_lowercase) {
        // Strip Unicode Combining Diacritical Marks (U+0300..=U+036F).
        let cp = ch as u32;
        if (0x0300..=0x036F).contains(&cp) {
            continue;
        }
        let folded = match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
            'ď' | 'đ' => 'd',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
            'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
            'ŕ' | 'ŗ' | 'ř' => 'r',
            'ś' | 'ŝ' | 'ş' | 'š' => 's',
            'ţ' | 'ť' | 'ŧ' => 't',
            'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
            'ý' | 'ÿ' | 'ŷ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            other => other,
        };
        out.push(folded);
    }
    out
}

/// Text embedding interface implemented by [`OnnxEmbedder`] and, in test builds, the
/// deterministic `HashProjectionEmbedder`.
pub trait Embedder: Send + Sync {
    /// Dimensionality of output vectors (e.g. `384`).
    fn dims(&self) -> usize;

    /// Stable identifier stored in `semantic_index.sqlite` metadata so switching models
    /// invalidates incompatible vectors cleanly.
    fn model_id(&self) -> &str;

    /// Embed a single archived document (message, snippet, canvas, or transcript).
    fn embed_document(&self, text: &str) -> std::result::Result<Vec<f32>, String>;

    /// Embed a search query (applies model-specific retrieval query prefix when needed).
    fn embed_query(&self, query: &str) -> std::result::Result<Vec<f32>, String> {
        self.embed_document(query)
    }

    /// Embed a batch of documents (override for batched ONNX inference).
    fn embed_batch(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, String> {
        texts.iter().map(|t| self.embed_document(t)).collect()
    }
}

/// In-process ONNX Runtime embedder (`ort`) for `all-MiniLM-L6-v2` / `bge-small-en-v1.5`.
pub struct OnnxEmbedder {
    model: EmbeddingModel,
    model_id: String,
    tokenizer: WordPieceTokenizer,
    session: Mutex<Session>,
    has_token_type_ids: bool,
}

impl OnnxEmbedder {
    /// Load `model` from its default app-data directory (`detect_embedding_model_setup`).
    pub fn load_default(model: EmbeddingModel) -> std::result::Result<Self, String> {
        let status = detect_embedding_model_setup(model);
        if !status.model_ready {
            return Err(format!(
                "Local embedding model {} is not downloaded (expected in {}).",
                model.slug(),
                status.model_path.display()
            ));
        }
        Self::load_from_dir(model, &status.model_path)
    }

    /// Load `model` from an explicit directory containing `vocab.txt` + `model_quantized.onnx` (or `model.onnx`).
    pub fn load_from_dir(model: EmbeddingModel, dir: &Path) -> std::result::Result<Self, String> {
        let vocab_path = dir.join("vocab.txt");
        let onnx_path = resolve_onnx_file(dir).ok_or_else(|| {
            format!(
                "missing model_quantized.onnx or model.onnx in {}",
                dir.display()
            )
        })?;
        // ONNX Runtime is a shared library mapped on first use; `ort` must not run before it.
        crate::huddle::onnx_runtime::ensure_loaded()?;
        let tokenizer = WordPieceTokenizer::from_file(&vocab_path)?;
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().clamp(1, 4))
            .unwrap_or(2);
        let session = Session::builder()
            .map_err(|e| format!("ort session builder error: {e}"))?
            .with_intra_threads(threads)
            .map_err(|e| format!("ort thread config error: {e}"))?
            .commit_from_file(&onnx_path)
            .map_err(|e| format!("failed to load ONNX model {}: {e}", onnx_path.display()))?;
        let has_token_type_ids = session
            .inputs()
            .iter()
            .any(|inp| inp.name() == "token_type_ids");
        Ok(Self {
            model,
            model_id: format!("onnx:{}", model.slug()),
            tokenizer,
            session: Mutex::new(session),
            has_token_type_ids,
        })
    }

    fn run_batch(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let max_len = self.model.max_seq_len();
        let encoded: Vec<TokenizedInput> = texts
            .iter()
            .map(|t| self.tokenizer.encode(t, max_len))
            .collect();
        let batch_size = encoded.len();
        let seq_len = encoded
            .iter()
            .map(|e| e.input_ids.len())
            .max()
            .unwrap_or(2)
            .max(1);
        let pad_id = self.tokenizer.pad_id();

        let mut flat_ids = vec![pad_id; batch_size * seq_len];
        let mut flat_mask = vec![0_i64; batch_size * seq_len];
        let mut flat_types = vec![0_i64; batch_size * seq_len];

        for (b, item) in encoded.iter().enumerate() {
            let row = b * seq_len;
            let n = item.input_ids.len().min(seq_len);
            flat_ids[row..row + n].copy_from_slice(&item.input_ids[..n]);
            flat_mask[row..row + n].copy_from_slice(&item.attention_mask[..n]);
            flat_types[row..row + n].copy_from_slice(&item.token_type_ids[..n]);
        }

        let ids_tensor = Tensor::from_array(([batch_size, seq_len], flat_ids))
            .map_err(|e| format!("ort input_ids tensor error: {e}"))?;
        let mask_tensor = Tensor::from_array(([batch_size, seq_len], flat_mask.clone()))
            .map_err(|e| format!("ort attention_mask tensor error: {e}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| "ONNX embedding session lock poisoned".to_owned())?;
        let outputs = if self.has_token_type_ids {
            let types_tensor = Tensor::from_array(([batch_size, seq_len], flat_types))
                .map_err(|e| format!("ort token_type_ids tensor error: {e}"))?;
            session
                .run(ort::inputs![
                    "input_ids" => ids_tensor,
                    "attention_mask" => mask_tensor,
                    "token_type_ids" => types_tensor,
                ])
                .map_err(|e| format!("ONNX inference error: {e}"))?
        } else {
            session
                .run(ort::inputs![
                    "input_ids" => ids_tensor,
                    "attention_mask" => mask_tensor,
                ])
                .map_err(|e| format!("ONNX inference error: {e}"))?
        };

        let (_, first_output) = outputs
            .iter()
            .next()
            .ok_or_else(|| "ONNX model returned no output tensors".to_owned())?;
        let (shape, data) = first_output
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("failed to extract f32 embedding tensor: {e}"))?;

        let dims_slice: Vec<usize> = shape.iter().map(|&d| d.max(0) as usize).collect();
        pool_embedding_tensor(
            &dims_slice,
            data,
            batch_size,
            seq_len,
            &flat_mask,
            self.model.uses_cls_pooling(),
        )
    }
}

/// Pool a raw ONNX embedding output tensor (`[batch, seq_len, hidden]` or `[batch, hidden]`)
/// with strict bounds validation so malformed ONNX outputs return `Err` instead of panicking.
fn pool_embedding_tensor(
    shape: &[usize],
    data: &[f32],
    batch_size: usize,
    seq_len: usize,
    flat_mask: &[i64],
    uses_cls_pooling: bool,
) -> std::result::Result<Vec<Vec<f32>>, String> {
    match shape {
        // [batch, seq_len, hidden_dim] -> CLS or attention-masked mean pooling.
        [b, s, h] if *b == batch_size && *s > 0 && *s <= seq_len && *h > 0 => {
            let needed = b
                .checked_mul(*s)
                .and_then(|v| v.checked_mul(*h))
                .ok_or_else(|| "ONNX output tensor dimensions overflowed".to_owned())?;
            let mask_needed = batch_size
                .checked_mul(seq_len)
                .ok_or_else(|| "attention mask dimensions overflowed".to_owned())?;
            if data.len() < needed || flat_mask.len() < mask_needed {
                return Err(format!(
                    "ONNX output tensor buffer too short: got {} elements for shape {shape:?}",
                    data.len()
                ));
            }
            let mut results = Vec::with_capacity(batch_size);
            for bi in 0..batch_size {
                let mut pooled = vec![0.0_f32; *h];
                if uses_cls_pooling {
                    let start = bi * (*s) * (*h);
                    pooled.copy_from_slice(&data[start..start + *h]);
                } else {
                    let mut weight_sum = 0.0_f32;
                    for si in 0..(*s) {
                        let m = flat_mask[bi * seq_len + si] as f32;
                        if m <= 0.0 {
                            continue;
                        }
                        weight_sum += m;
                        let offset = (bi * (*s) + si) * (*h);
                        for (dst, &val) in pooled.iter_mut().zip(&data[offset..offset + *h]) {
                            *dst += val * m;
                        }
                    }
                    if weight_sum > 0.0 {
                        let inv = 1.0 / weight_sum;
                        for v in &mut pooled {
                            *v *= inv;
                        }
                    }
                }
                l2_normalize(&mut pooled);
                results.push(pooled);
            }
            Ok(results)
        }
        // [batch, hidden_dim] -> pre-pooled sentence embeddings.
        [b, h] if *b == batch_size && *h > 0 => {
            let needed = b
                .checked_mul(*h)
                .ok_or_else(|| "ONNX output tensor dimensions overflowed".to_owned())?;
            if data.len() < needed {
                return Err(format!(
                    "ONNX output tensor buffer too short: got {} elements for shape {shape:?}",
                    data.len()
                ));
            }
            let mut results = Vec::with_capacity(batch_size);
            for bi in 0..batch_size {
                let start = bi * (*h);
                let mut vec = data[start..start + *h].to_vec();
                l2_normalize(&mut vec);
                results.push(vec);
            }
            Ok(results)
        }
        other => Err(format!(
            "unexpected ONNX embedding output tensor shape: {other:?}"
        )),
    }
}

impl Embedder for OnnxEmbedder {
    fn dims(&self) -> usize {
        self.model.dims()
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed_document(&self, text: &str) -> std::result::Result<Vec<f32>, String> {
        let mut batch = self.run_batch(&[text])?;
        batch
            .pop()
            .ok_or_else(|| "empty batch output from ONNX embedder".to_owned())
    }

    fn embed_query(&self, query: &str) -> std::result::Result<Vec<f32>, String> {
        if let Some(prefix) = self.model.query_prefix() {
            let prefixed = format!("{prefix}{query}");
            self.embed_document(&prefixed)
        } else {
            self.embed_document(query)
        }
    }

    fn embed_batch(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, String> {
        self.run_batch(texts)
    }
}

/// Idle time after which the shared cached [`OnnxEmbedder`] session is unloaded (5 minutes).
pub const EMBEDDER_IDLE_TTL: Duration = Duration::from_secs(5 * 60);
/// How often the background reaper checks the shared embedder for idleness.
const EMBEDDER_REAPER_INTERVAL: Duration = Duration::from_secs(60);

struct IdleEntry<K, T> {
    key: K,
    value: Arc<T>,
    last_used: Instant,
}

/// Thread-safe slot holding one lazily loaded, idle-evictable shared value (e.g. an ONNX
/// Runtime session, ~60-90 MB resident), keyed so a different key reloads it.
struct IdleSlot<K, T> {
    inner: Mutex<Option<IdleEntry<K, T>>>,
}

impl<K, T> Default for IdleSlot<K, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, T> IdleSlot<K, T> {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<IdleEntry<K, T>>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a value is currently cached.
    pub fn is_loaded(&self) -> bool {
        self.lock().is_some()
    }

    /// Drop the cached value (callers still holding an `Arc` keep theirs alive until done).
    /// Returns `true` when something was cached.
    pub fn unload(&self) -> bool {
        self.lock().take().is_some()
    }

    /// Unload the cached value if it has not been used for `ttl` and nobody else currently holds
    /// it (an in-flight indexing run keeps its session). Returns `true` when unloaded.
    pub fn unload_if_idle(&self, ttl: Duration) -> bool {
        self.unload_if_idle_at(ttl, Instant::now())
    }

    /// [`Self::unload_if_idle`] with an explicit `now` (deterministic tests).
    pub fn unload_if_idle_at(&self, ttl: Duration, now: Instant) -> bool {
        let mut guard = self.lock();
        let idle = guard.as_ref().is_some_and(|entry| {
            Arc::strong_count(&entry.value) == 1
                && now.saturating_duration_since(entry.last_used) >= ttl
        });
        if idle {
            *guard = None;
        }
        idle
    }
}

impl<K: PartialEq, T> IdleSlot<K, T> {
    /// Return the cached value for `key` (refreshing its idle timer), or load it with `load`.
    pub fn get_or_try_load<E>(
        &self,
        key: K,
        load: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<Arc<T>, E> {
        let mut guard = self.lock();
        if let Some(entry) = guard.as_mut()
            && entry.key == key
        {
            entry.last_used = Instant::now();
            return Ok(Arc::clone(&entry.value));
        }
        let value = Arc::new(load()?);
        *guard = Some(IdleEntry {
            key,
            value: Arc::clone(&value),
            last_used: Instant::now(),
        });
        Ok(value)
    }
}

static ONNX_EMBEDDER: IdleSlot<EmbeddingModel, OnnxEmbedder> = IdleSlot::new();

/// Spawn (once) a tiny background thread that unloads the shared ONNX session after
/// [`EMBEDDER_IDLE_TTL`] of inactivity, returning its memory to the OS.
fn ensure_embedder_reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("slackless-embed-reaper".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(EMBEDDER_REAPER_INTERVAL);
                    if ONNX_EMBEDDER.unload_if_idle(EMBEDDER_IDLE_TTL) {
                        tracing::debug!("unloaded idle ONNX embedding session");
                    }
                }
            });
        if let Err(err) = spawned {
            tracing::warn!(%err, "failed to spawn embedder idle reaper");
        }
    });
}

/// Shared, lazily loaded [`OnnxEmbedder`] for `model` (0 MB until first semantic use). Each call
/// refreshes the idle timer; the session is unloaded automatically after [`EMBEDDER_IDLE_TTL`].
pub fn cached_onnx_embedder(
    model: EmbeddingModel,
) -> std::result::Result<Arc<OnnxEmbedder>, String> {
    let embedder = ONNX_EMBEDDER.get_or_try_load(model, || OnnxEmbedder::load_default(model))?;
    ensure_embedder_reaper();
    Ok(embedder)
}

/// Explicitly unload the shared cached ONNX session (e.g. after deleting the model). Returns
/// `true` when a session was cached.
pub fn unload_cached_embedder() -> bool {
    ONNX_EMBEDDER.unload()
}

/// Whether the shared ONNX session is currently resident in memory.
pub fn embedder_loaded() -> bool {
    ONNX_EMBEDDER.is_loaded()
}

/// Deterministic pure-Rust signed feature-hashing embedder (zero network, zero model weights).
///
/// Projects lowercased word unigrams, word bigrams, and character trigrams into an L2-normalized
/// dense vector using SHA-256 hashing. Ideal for deterministic unit/integration tests and as an
/// offline fallback when verifying end-to-end vector indexing & RRF pipelines without model weights.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone)]
pub struct HashProjectionEmbedder {
    dims: usize,
    model_id: String,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for HashProjectionEmbedder {
    fn default() -> Self {
        Self::new(384)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl HashProjectionEmbedder {
    pub fn new(dims: usize) -> Self {
        Self {
            dims: dims.max(8),
            model_id: format!("hash-proj-v1:{}", dims.max(8)),
        }
    }

    #[cfg(test)]
    pub fn with_model_id(dims: usize, model_id: impl Into<String>) -> Self {
        Self {
            dims: dims.max(8),
            model_id: model_id.into(),
        }
    }

    fn project_feature(&self, out: &mut [f32], feature: &str, weight: f32) {
        let digest = Sha256::digest(feature.as_bytes());
        // Project into 2 signed buckets per feature to reduce hash collisions cleanly.
        for slot in 0..2 {
            let base = slot * 8;
            let idx_raw = u32::from_le_bytes([
                digest[base],
                digest[base + 1],
                digest[base + 2],
                digest[base + 3],
            ]) as usize;
            let sign = if digest[base + 4] & 1 == 0 {
                1.0_f32
            } else {
                -1.0_f32
            };
            out[idx_raw % self.dims] += sign * weight;
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Embedder for HashProjectionEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed_document(&self, text: &str) -> std::result::Result<Vec<f32>, String> {
        let mut vec = vec![0.0_f32; self.dims];
        let words: Vec<String> = basic_tokenize(text, true)
            .into_iter()
            .filter(|w| w.chars().any(char::is_alphanumeric))
            .collect();
        for (i, word) in words.iter().enumerate() {
            self.project_feature(&mut vec, &format!("w:{word}"), 2.0);
            // Stem-like prefix for morphological overlap (e.g. deploy / deployment / deploying).
            let chars: Vec<char> = word.chars().collect();
            if chars.len() >= 5 {
                let prefix: String = chars[..4].iter().collect();
                self.project_feature(&mut vec, &format!("p:{prefix}"), 0.8);
            }
            // Character trigrams for subword/typo resilience.
            if chars.len() >= 3 {
                for tri in chars.windows(3) {
                    let s: String = tri.iter().collect();
                    self.project_feature(&mut vec, &format!("c3:{s}"), 0.5);
                }
            }
            // Word bigrams for phrase sensitivity.
            if let Some(next) = words.get(i + 1) {
                self.project_feature(&mut vec, &format!("b:{word}_{next}"), 1.2);
            }
        }
        l2_normalize(&mut vec);
        Ok(vec)
    }
}

/// Compute a deterministic SHA-256 hex digest for incremental change detection.
pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_store::cosine_similarity;

    fn sample_vocab() -> WordPieceTokenizer {
        let vocab = [
            "[PAD]",
            "[UNK]",
            "[CLS]",
            "[SEP]",
            "hello",
            "world",
            "deploy",
            "##ing",
            "##ment",
            "kubernetes",
            "cluster",
            "mote",
            "svenska",
            ",",
            "!",
            "中",
            "文",
        ]
        .join("\n");
        WordPieceTokenizer::from_vocab_str(&vocab).unwrap()
    }

    #[test]
    fn wordpiece_tokenizer_handles_subwords_punctuation_diacritics_cjk_and_truncation() {
        let tok = sample_vocab();
        assert_eq!(tok.vocab_size(), 17);

        let enc = tok.encode("Hello, deploying möte! 中文", 32);
        // [CLS]=2, hello=4, ,=13, deploy=6, ##ing=7, mote=11, !=14, 中=15, 文=16, [SEP]=3
        assert_eq!(enc.input_ids, vec![2, 4, 13, 6, 7, 11, 14, 15, 16, 3]);
        assert_eq!(enc.attention_mask, vec![1; 10]);
        assert_eq!(enc.token_type_ids, vec![0; 10]);

        // Unknown word maps cleanly to [UNK]=1.
        let unk = tok.encode("xylophone", 16);
        assert_eq!(unk.input_ids, vec![2, 1, 3]);

        // Max length truncation preserves [CLS] and trailing [SEP].
        let trunc = tok.encode("hello world deploy kubernetes cluster", 4);
        assert_eq!(trunc.input_ids, vec![2, 4, 5, 3]);
        assert_eq!(trunc.attention_mask.len(), 4);

        // Overlong token (>100 chars) falls back to [UNK] without pathological backtracking.
        let huge = "a".repeat(150);
        let huge_enc = tok.encode(&huge, 16);
        assert_eq!(huge_enc.input_ids, vec![2, 1, 3]);
    }

    #[test]
    fn hash_projection_embedder_is_deterministic_normalized_and_semantically_ordered() {
        let emb = HashProjectionEmbedder::new(128);
        let q = emb.embed_query("kubernetes deployment rollback").unwrap();
        let d_rel = emb
            .embed_document("rolling back the kubernetes deploying cluster release")
            .unwrap();
        let d_unrel = emb
            .embed_document("lunch menu pasta salad cafeteria coffee")
            .unwrap();

        assert_eq!(q.len(), 128);
        let norm: f32 = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);

        // Deterministic across repeated calls.
        assert_eq!(
            q,
            emb.embed_query("kubernetes deployment rollback").unwrap()
        );

        let sim_rel = cosine_similarity(&q, &d_rel);
        let sim_unrel = cosine_similarity(&q, &d_unrel);
        assert!(
            sim_rel > sim_unrel + 0.2,
            "expected related score ({sim_rel}) >> unrelated ({sim_unrel})"
        );
    }

    #[test]
    fn embedding_model_parsing_and_metadata() {
        assert_eq!(
            EmbeddingModel::parse("minilm"),
            Some(EmbeddingModel::AllMiniLmL6V2)
        );
        assert_eq!(
            EmbeddingModel::parse("all-MiniLM-L6-v2"),
            Some(EmbeddingModel::AllMiniLmL6V2)
        );
        assert_eq!(
            EmbeddingModel::parse("bge-small"),
            Some(EmbeddingModel::BgeSmallEnV15)
        );
        assert_eq!(EmbeddingModel::parse("unknown-model"), None);
        assert!(EmbeddingModel::AllMiniLmL6V2.query_prefix().is_none());
        assert!(EmbeddingModel::BgeSmallEnV15.query_prefix().is_some());
    }

    #[test]
    fn idle_slot_loads_once_reuses_reloads_on_key_change_and_unloads_when_idle() {
        let slot: IdleSlot<u8, String> = IdleSlot::new();
        assert!(!slot.is_loaded());
        let mut loads = 0;
        let a = slot
            .get_or_try_load(1, || {
                loads += 1;
                Ok::<_, String>("one".to_owned())
            })
            .unwrap();
        let again = slot
            .get_or_try_load(1, || {
                loads += 1;
                Ok::<_, String>("other".to_owned())
            })
            .unwrap();
        assert_eq!(loads, 1);
        assert!(Arc::ptr_eq(&a, &again));
        drop(again);

        // Not idle yet, and still held by `a`: never unloaded.
        let ttl = Duration::from_secs(300);
        assert!(!slot.unload_if_idle_at(ttl, Instant::now()));
        let far_future = Instant::now() + Duration::from_secs(301);
        assert!(
            !slot.unload_if_idle_at(ttl, far_future),
            "in-use sessions must not be unloaded"
        );
        drop(a);
        assert!(slot.unload_if_idle_at(ttl, far_future));
        assert!(!slot.is_loaded());

        // Load errors leave the slot empty.
        let err = slot.get_or_try_load(2, || Err::<String, _>("boom".to_owned()));
        assert_eq!(err.unwrap_err(), "boom");
        assert!(!slot.is_loaded());

        // A different key reloads; explicit unload clears.
        slot.get_or_try_load(3, || Ok::<_, String>("three".into()))
            .unwrap();
        let four = slot
            .get_or_try_load(4, || Ok::<_, String>("four".into()))
            .unwrap();
        assert_eq!(four.as_str(), "four");
        assert!(slot.unload());
        assert!(!slot.unload());
        assert_eq!(four.as_str(), "four", "callers keep their Arc after unload");
    }

    #[test]
    fn cached_onnx_embedder_reports_missing_model_without_caching() {
        // The default model is never downloaded in CI; the error must not poison the cache.
        if detect_embedding_model_setup(EmbeddingModel::BgeSmallEnV15).model_ready {
            return;
        }
        assert!(cached_onnx_embedder(EmbeddingModel::BgeSmallEnV15).is_err());
        assert!(!embedder_loaded());
    }

    #[test]
    fn content_hash_is_stable_sha256_hex() {
        let h1 = content_hash("hello slackless");
        let h2 = content_hash("hello slackless");
        let h3 = content_hash("hello slackless!");
        assert_eq!(h1.len(), 64);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn pool_embedding_tensor_validates_bounds_and_masks() {
        // Mean pooling ignores masked-out padding tokens.
        let data = vec![2.0, 0.0, 0.0, 2.0, 100.0, 100.0];
        let mask = vec![1, 1, 0];
        let mean = pool_embedding_tensor(&[1, 3, 2], &data, 1, 3, &mask, false).unwrap();
        assert_eq!(mean.len(), 1);
        assert!((mean[0][0] - mean[0][1]).abs() < 1e-5);

        // CLS pooling takes token 0 only.
        let cls = pool_embedding_tensor(&[1, 3, 2], &data, 1, 3, &mask, true).unwrap();
        assert!((cls[0][0] - 1.0).abs() < 1e-5);
        assert!(cls[0][1].abs() < 1e-5);

        // Out-of-bounds sequence length or truncated buffers return Err cleanly instead of panicking.
        assert!(pool_embedding_tensor(&[1, 4, 2], &data, 1, 3, &mask, false).is_err());
        assert!(pool_embedding_tensor(&[1, 0, 2], &data, 1, 3, &mask, false).is_err());
        assert!(pool_embedding_tensor(&[1, 3, 2], &data[..4], 1, 3, &mask, false).is_err());
        assert!(pool_embedding_tensor(&[1, 4], &[1.0, 2.0], 1, 3, &mask, false).is_err());
    }
}
