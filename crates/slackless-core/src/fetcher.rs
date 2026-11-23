//! Image downloads for front-ends (profile photos, emoji, workspace icons, file thumbnails), run
//! on the core's tokio runtime so a front-end on another executor (GPUI's) shares the core's
//! HTTP stack. Includes a bounded in-memory LRU byte cache so repeated avatar/emoji loads stay
//! instant without unbounded RAM growth.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;

use crate::api::{AssetClient, SlackApi};

/// Where Slack serves workspace files and their thumbnails; these need the session cookie.
const SLACK_FILES: &str = "https://files.slack.com/";
/// Maximum total bytes kept in the in-memory asset cache (24 MiB).
const MAX_CACHE_BYTES: usize = 24 * 1024 * 1024;
/// Maximum number of cached URLs in the in-memory asset cache.
const MAX_CACHE_ENTRIES: usize = 512;
/// Skip caching single assets larger than 4 MiB so large file downloads never evict avatars.
const MAX_SINGLE_ENTRY_BYTES: usize = 4 * 1024 * 1024;

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
}

impl Fetcher {
    pub(crate) fn new(runtime: Handle, files: Option<SlackApi>) -> Self {
        Self {
            runtime,
            assets: AssetClient::new(),
            files,
            cache: Arc::new(Mutex::new(ByteLru::new(MAX_CACHE_BYTES, MAX_CACHE_ENTRIES))),
        }
    }

    /// The bytes at `url`. Slack-hosted files (thumbnails under `https://files.slack.com/`)
    /// download through the signed-in client, which sends the session cookie to Slack file
    /// hosts only; everything else is a public asset. The download starts now on the core
    /// runtime; the returned future can be awaited from any executor.
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
        let files = self.files.clone().filter(|_| url.starts_with(SLACK_FILES));
        let cache = Arc::clone(&self.cache);
        let task = self.runtime.spawn(async move {
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
                        guard.insert(url, bytes.clone());
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
}
