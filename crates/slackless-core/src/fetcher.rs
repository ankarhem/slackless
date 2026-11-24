//! Image downloads for front-ends (profile photos, emoji, workspace icons, file thumbnails), run
//! on the core's tokio runtime so a front-end on another executor (GPUI's) shares the core's
//! HTTP stack. Includes a bounded in-memory LRU byte cache so repeated avatar/emoji loads stay
//! instant without unbounded RAM growth.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;

use crate::api::{AssetClient, SlackApi};

/// Where Slack serves workspace files and their thumbnails; these need the session cookie and are
/// kept in memory only (never written to the public asset disk cache).
const SLACK_FILES: &str = "https://files.slack.com/";
/// Maximum total bytes kept in the in-memory asset cache (24 MiB).
const MAX_CACHE_BYTES: usize = 24 * 1024 * 1024;
/// Maximum number of cached URLs in the in-memory asset cache.
const MAX_CACHE_ENTRIES: usize = 512;
/// Skip caching single assets larger than 4 MiB so large file downloads never evict avatars.
const MAX_SINGLE_ENTRY_BYTES: usize = 4 * 1024 * 1024;
/// Maximum total size of the persistent disk cache for public avatars and custom emoji (128 MiB).
const MAX_DISK_CACHE_BYTES: u64 = 128 * 1024 * 1024;

/// Stable 128-bit FNV-1a hex filename for caching a public asset URL on disk without extra deps.
fn cache_key(url: &str) -> String {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x8422_2325_cbf2_9ce4;
    for &b in url.as_bytes() {
        h1 ^= u64::from(b);
        h1 = h1.wrapping_mul(0x0100_0000_01b3);
        h2 = h2.wrapping_mul(0x0100_0000_01b3);
        h2 ^= u64::from(b);
    }
    format!("{h1:016x}{h2:016x}.bin")
}

/// Platform cache directory for persistent public media assets (avatars, workspace icons, emoji).
pub fn disk_cache_dir() -> Option<PathBuf> {
    let mut path = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        path.push("Library/Caches/slackless/media");
    } else {
        path.push(".cache/slackless/media");
    }
    Some(path)
}

fn read_disk_cache(dir: &Path, url: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(cache_key(url))).ok()
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn write_disk_cache(dir: &Path, url: &str, bytes: &[u8]) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let key = cache_key(url);
    let final_path = dir.join(&key);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!("{key}.tmp.{}.{seq}", std::process::id()));
    if std::fs::write(&tmp_path, bytes).is_ok() {
        let _ = std::fs::rename(&tmp_path, &final_path);
    }
}

fn prune_disk_cache_if_needed(dir: &Path, cap: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let len = meta.len();
        total = total.saturating_add(len);
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        files.push((entry.path(), len, modified));
    }
    if total <= cap {
        return;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, len, _) in files {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

#[derive(Default)]
struct ByteLru {
    entries: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
    total_bytes: usize,
    max_bytes: usize,
    max_entries: usize,
}

impl ByteLru {
    fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            total_bytes: 0,
            max_bytes,
            max_entries,
        }
    }

    fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        let data = self.entries.get(key)?.clone();
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_owned());
        Some(data)
    }

    fn insert(&mut self, key: String, data: Vec<u8>) {
        if data.len() > self.max_bytes {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.len());
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        }
        while (self.total_bytes + data.len() > self.max_bytes
            || self.entries.len() >= self.max_entries)
            && !self.order.is_empty()
        {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(evicted) = self.entries.remove(&oldest) {
                    self.total_bytes = self.total_bytes.saturating_sub(evicted.len());
                }
            }
        }
        self.total_bytes += data.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, data);
    }
}

/// Cheap to clone; hand one to whatever loads remote images.
#[derive(Clone)]
pub struct Fetcher {
    runtime: Handle,
    assets: AssetClient,
    /// The signed-in client, for Slack-hosted files (`None` when it could not be built).
    files: Option<SlackApi>,
    /// Bounded LRU cache of fetched asset bytes shared across clones.
    cache: Arc<Mutex<ByteLru>>,
    /// Persistent on-disk cache directory for public avatars and custom emoji.
    disk_dir: Option<PathBuf>,
}

impl Fetcher {
    pub(crate) fn new(runtime: Handle, files: Option<SlackApi>) -> Self {
        let disk_dir = disk_cache_dir();
        if let Some(dir) = disk_dir.clone() {
            runtime.spawn_blocking(move || prune_disk_cache_if_needed(&dir, MAX_DISK_CACHE_BYTES));
        }
        Self {
            runtime,
            assets: AssetClient::new(),
            files,
            cache: Arc::new(Mutex::new(ByteLru::new(MAX_CACHE_BYTES, MAX_CACHE_ENTRIES))),
            disk_dir,
        }
    }

    /// Directory holding the persistent disk cache for public avatars & custom emoji.
    pub fn disk_cache_dir() -> Option<PathBuf> {
        disk_cache_dir()
    }

    /// The bytes at `url`. Slack-hosted files (thumbnails under `https://files.slack.com/`)
    /// download through the signed-in client, which sends the session cookie to Slack file
    /// hosts only; everything else is a public asset (served from the persistent disk cache on
    /// cold start before hitting the network).
    pub fn get(
        &self,
        url: String,
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send + 'static {
        if let Ok(mut guard) = self.cache.lock() {
            if let Some(hit) = guard.get(&url) {
                return futures::future::Either::Left(async move { Ok(hit) });
            }
        }
        let assets = self.assets.clone();
        let is_private_file = url.starts_with(SLACK_FILES);
        let files = self.files.clone().filter(|_| is_private_file);
        let cache = Arc::clone(&self.cache);
        let disk_dir = (!is_private_file).then(|| self.disk_dir.clone()).flatten();
        let task = self.runtime.spawn(async move {
            if let Some(ref dir) = disk_dir {
                let dir_r = dir.clone();
                let url_r = url.clone();
                if let Ok(Some(bytes)) =
                    tokio::task::spawn_blocking(move || read_disk_cache(&dir_r, &url_r)).await
                {
                    if bytes.len() <= MAX_SINGLE_ENTRY_BYTES {
                        if let Ok(mut guard) = cache.lock() {
                            guard.insert(url, bytes.clone());
                        }
                    }
                    return Ok(bytes);
                }
            }
            let result = match files {
                Some(api) => api
                    .download_file(&url)
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|e| e.to_string()),
                None => assets.get(&url).await.map_err(|e| e.to_string()),
            };
            if let Ok(ref bytes) = result {
                if bytes.len() <= MAX_SINGLE_ENTRY_BYTES {
                    if let Ok(mut guard) = cache.lock() {
                        guard.insert(url.clone(), bytes.clone());
                    }
                    if let Some(dir) = disk_dir {
                        let data = bytes.clone();
                        // Persist asynchronously so disk I/O never stalls the image decode path.
                        tokio::task::spawn_blocking(move || {
                            write_disk_cache(&dir, &url, &data);
                        });
                    }
                }
            }
            result
        });
        futures::future::Either::Right(async move { task.await.map_err(|e| e.to_string())? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_lru_evicts_least_recently_used_under_byte_cap() {
        let mut lru = ByteLru::new(10, 4);
        lru.insert("a".into(), vec![1; 4]);
        lru.insert("b".into(), vec![2; 4]);
        // Touch "a" so "b" becomes least recently used.
        assert_eq!(lru.get("a"), Some(vec![1; 4]));
        lru.insert("c".into(), vec![3; 4]);
        assert_eq!(lru.get("b"), None);
        assert_eq!(lru.get("a"), Some(vec![1; 4]));
        assert_eq!(lru.get("c"), Some(vec![3; 4]));
        assert_eq!(lru.total_bytes, 8);
    }

    #[test]
    fn disk_cache_round_trips_and_prunes_under_cap() {
        let dir =
            std::env::temp_dir().join(format!("slackless-fetcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let url_a = "https://ca.slack-edge.com/T1-U1-abc-72";
        let url_b = "https://ca.slack-edge.com/T1-U2-def-72";
        write_disk_cache(&dir, url_a, b"avatar-a-bytes");
        assert_eq!(
            read_disk_cache(&dir, url_a).as_deref(),
            Some(b"avatar-a-bytes".as_slice())
        );
        write_disk_cache(&dir, url_b, b"avatar-b-bytes-longer");
        prune_disk_cache_if_needed(&dir, 16);
        let remaining: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.metadata().ok().map(|m| m.len()))
            .sum();
        assert!(remaining <= 16);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
