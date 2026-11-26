//! Image downloads for front-ends (profile photos, emoji, workspace icons, file thumbnails), run
//! on the core's tokio runtime so a front-end on another executor (GPUI's) shares the core's
//! HTTP stack. Includes a bounded in-memory LRU byte cache so repeated avatar/emoji loads stay
//! instant without unbounded RAM growth.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::runtime::Handle;

use crate::api::{AssetClient, MAX_FILE_BYTES, SlackApi};
use crate::types::time::now_epoch_secs;

/// Where Slack serves workspace files and their thumbnails; these need the session cookie and are
/// kept in memory only (never written to the public asset disk cache).
const SLACK_FILES: &str = "https://files.slack.com/";
/// Maximum total bytes kept in the in-memory asset cache (12 MiB).
const MAX_CACHE_BYTES: usize = 12 * 1024 * 1024;
/// Maximum number of cached URLs in the in-memory asset cache.
const MAX_CACHE_ENTRIES: usize = 384;
/// Skip caching single assets larger than 4 MiB so large file downloads never evict avatars.
const MAX_SINGLE_ENTRY_BYTES: usize = 4 * 1024 * 1024;
/// Maximum total size of the persistent disk cache for public avatars and custom emoji (128 MiB).
const MAX_DISK_CACHE_BYTES: u64 = 128 * 1024 * 1024;
/// Minimum interval (seconds) between periodic disk-cache prune sweeps.
const PRUNE_INTERVAL_SECS: i64 = 30 * 60;
/// Trigger a disk-cache prune check every N disk writes even inside the time window.
const PRUNE_EVERY_WRITES: u64 = 128;

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
    crate::paths::app_cache_dir().map(|dir| dir.join("media"))
}

fn read_disk_cache(dir: &Path, url: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(cache_key(url))).ok()
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_PRUNED_SECS: AtomicI64 = AtomicI64::new(0);
static WRITES_SINCE_PRUNE: AtomicU64 = AtomicU64::new(0);

fn should_prune_now(now: i64) -> bool {
    let prev = LAST_PRUNED_SECS.load(Ordering::Relaxed);
    if now.saturating_sub(prev) >= PRUNE_INTERVAL_SECS {
        return LAST_PRUNED_SECS
            .compare_exchange(prev, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok();
    }
    let writes = WRITES_SINCE_PRUNE.fetch_add(1, Ordering::Relaxed) + 1;
    if writes >= PRUNE_EVERY_WRITES {
        WRITES_SINCE_PRUNE.store(0, Ordering::Relaxed);
        LAST_PRUNED_SECS.store(now, Ordering::Relaxed);
        return true;
    }
    false
}

fn write_disk_cache(dir: &Path, url: &str, bytes: &[u8]) {
    if let Err(err) = std::fs::create_dir_all(dir) {
        tracing::warn!(dir = %dir.display(), %err, "failed to create media cache directory");
        return;
    }
    let key = cache_key(url);
    let final_path = dir.join(&key);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!("{key}.tmp.{}.{seq}", std::process::id()));
    if let Err(err) = std::fs::write(&tmp_path, bytes) {
        tracing::warn!(path = %tmp_path.display(), %err, "failed to write temp media cache file");
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    if let Err(err) = std::fs::rename(&tmp_path, &final_path) {
        tracing::warn!(
            from = %tmp_path.display(),
            to = %final_path.display(),
            %err,
            "failed to rename media cache temp file; removing temp file"
        );
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    if should_prune_now(now_epoch_secs()) {
        prune_disk_cache_if_needed(dir, MAX_DISK_CACHE_BYTES);
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
            if let Some(oldest) = self.order.pop_front()
                && let Some(evicted) = self.entries.remove(&oldest)
            {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.len());
            }
        }
        self.total_bytes += data.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, data);
    }
}

type InFlightMap = HashMap<String, tokio::sync::broadcast::Sender<Result<Vec<u8>, String>>>;
type FetchFuture =
    std::pin::Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'static>>;

/// Cheap to clone; hand one to whatever loads remote images.
#[derive(Clone)]
pub struct Fetcher {
    runtime: Handle,
    assets: AssetClient,
    /// The signed-in client, for Slack-hosted files (`None` when it could not be built).
    files: Option<SlackApi>,
    /// Bounded LRU cache of fetched asset bytes shared across clones.
    cache: Arc<Mutex<ByteLru>>,
    /// Coalesces concurrent misses for the same URL into a single in-flight fetch.
    in_flight: Arc<Mutex<InFlightMap>>,
    /// Persistent on-disk cache directory for public avatars and custom emoji.
    disk_dir: Option<PathBuf>,
    /// URL prefix treated as private authenticated files (never read from or written to `disk_dir`).
    private_prefix: Cow<'static, str>,
}

struct InFlightGuard {
    in_flight: Arc<Mutex<InFlightMap>>,
    url: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut flights = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        flights.remove(&self.url);
    }
}

fn shared_asset_client() -> AssetClient {
    static CLIENT: std::sync::OnceLock<AssetClient> = std::sync::OnceLock::new();
    CLIENT.get_or_init(AssetClient::new).clone()
}

impl Fetcher {
    pub(crate) fn new(runtime: Handle, files: Option<SlackApi>) -> Self {
        let disk_dir = disk_cache_dir();
        if let Some(dir) = disk_dir.clone()
            && should_prune_now(now_epoch_secs())
        {
            runtime.spawn_blocking(move || prune_disk_cache_if_needed(&dir, MAX_DISK_CACHE_BYTES));
        }
        Self {
            runtime,
            assets: shared_asset_client(),
            files,
            cache: Arc::new(Mutex::new(ByteLru::new(MAX_CACHE_BYTES, MAX_CACHE_ENTRIES))),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            disk_dir,
            private_prefix: Cow::Borrowed(SLACK_FILES),
        }
    }

    #[cfg(test)]
    fn new_for_test(
        runtime: Handle,
        files: Option<SlackApi>,
        disk_dir: Option<PathBuf>,
        private_prefix: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            runtime,
            assets: AssetClient::new(),
            files,
            cache: Arc::new(Mutex::new(ByteLru::new(MAX_CACHE_BYTES, MAX_CACHE_ENTRIES))),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            disk_dir,
            private_prefix: private_prefix.into(),
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
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send + 'static + use<> {
        {
            let mut guard = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(hit) = guard.get(&url) {
                return futures::future::Either::Left(async move { Ok(hit) });
            }
        }
        let mut follower_rx = None;
        let mut leader_tx = None;
        {
            let mut flights = self
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(tx) = flights.get(&url) {
                follower_rx = Some(tx.subscribe());
            } else {
                let (tx, _) = tokio::sync::broadcast::channel(1);
                flights.insert(url.clone(), tx.clone());
                leader_tx = Some(tx);
            }
        }
        if let Some(mut rx) = follower_rx {
            return futures::future::Either::Right(Box::pin(async move {
                match rx.recv().await {
                    Ok(res) => res,
                    Err(_) => Err("in-flight asset fetch cancelled".to_owned()),
                }
            }) as FetchFuture);
        }
        let assets = self.assets.clone();
        let is_private_file = url.starts_with(self.private_prefix.as_ref());
        let files = self.files.clone().filter(|_| is_private_file);
        let cache = Arc::clone(&self.cache);
        let guard = InFlightGuard {
            in_flight: Arc::clone(&self.in_flight),
            url: url.clone(),
        };
        let disk_dir = (!is_private_file).then(|| self.disk_dir.clone()).flatten();
        let task = self.runtime.spawn(async move {
            let guard = guard;
            let outcome = async {
                if let Some(ref dir) = disk_dir {
                    let dir_r = dir.clone();
                    let url_r = url.clone();
                    if let Ok(Some(bytes)) =
                        tokio::task::spawn_blocking(move || read_disk_cache(&dir_r, &url_r)).await
                    {
                        if bytes.len() <= MAX_SINGLE_ENTRY_BYTES {
                            cache
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .insert(url.clone(), bytes.clone());
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
                if let Ok(ref bytes) = result
                    && bytes.len() <= MAX_SINGLE_ENTRY_BYTES
                {
                    cache
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(url.clone(), bytes.clone());
                    if let Some(dir) = disk_dir {
                        let data = bytes.clone();
                        let url_w = url.clone();
                        // Persist asynchronously so disk I/O never stalls the image decode path.
                        tokio::task::spawn_blocking(move || {
                            write_disk_cache(&dir, &url_w, &data);
                        });
                    }
                }
                result
            }
            .await;
            drop(guard);
            if let Some(tx) = leader_tx {
                let _ = tx.send(outcome.clone());
            }
            outcome
        });
        futures::future::Either::Right(Box::pin(
            async move { task.await.map_err(|e| e.to_string())? },
        ) as FetchFuture)
    }

    /// Streams the file at `url` into `dest` without holding it in memory and resolves to its
    /// length. URLs route as in [`Fetcher::get`], bypassing the caches; the file arrives through
    /// a `.part` file renamed into place, so `dest` only ever holds a whole file.
    pub fn download_to_path(
        &self,
        url: String,
        dest: PathBuf,
    ) -> impl Future<Output = Result<u64, String>> + Send + 'static + use<> {
        let assets = self.assets.clone();
        let files = self
            .files
            .clone()
            .filter(|_| url.starts_with(self.private_prefix.as_ref()));
        let task = self.runtime.spawn(async move {
            match files {
                Some(api) => api.download_file_to_path(&url, &dest).await,
                None => {
                    assets
                        .download_to_path(&url, &dest, MAX_FILE_BYTES, |_, _| {})
                        .await
                }
            }
            .map_err(|e| e.to_string())
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    fn unique_temp_dir(label: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "slackless-fetcher-test-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    struct TestHttpServer {
        base_url: String,
        hits: Arc<AtomicUsize>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Drop for TestHttpServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            // Wake accept loop.
            let addr = self.base_url.trim_start_matches("http://");
            let _ = std::net::TcpStream::connect(addr);
        }
    }

    fn spawn_test_server<F>(handler: F) -> TestHttpServer
    where
        F: Fn(&str, &str) -> (u16, &'static str, Vec<u8>) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handler = Arc::new(handler);
        let hits_bg = Arc::clone(&hits);
        let stop_bg = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let handler = Arc::clone(&handler);
                let hits_req = Arc::clone(&hits_bg);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let req = String::from_utf8_lossy(&buf);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    if path == "/" && req.is_empty() {
                        return;
                    }
                    hits_req.fetch_add(1, Ordering::SeqCst);
                    let (status, content_type, body) = handler(path, &req);
                    let reason = match status {
                        200 => "OK",
                        500 => "Internal Server Error",
                        _ => "Error",
                    };
                    let header = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&body);
                    let _ = stream.flush();
                });
            }
        });
        TestHttpServer {
            base_url: format!("http://{addr}"),
            hits,
            stop,
        }
    }

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
    fn byte_lru_entry_count_eviction_oversize_skip_and_overwrite_accounting() {
        // Entry-count eviction even when total bytes are well under max_bytes.
        let mut lru = ByteLru::new(1024, 2);
        lru.insert("a".into(), vec![1; 3]);
        lru.insert("b".into(), vec![2; 4]);
        lru.insert("c".into(), vec![3; 5]);
        assert_eq!(lru.entries.len(), 2);
        assert_eq!(lru.get("a"), None);
        assert_eq!(lru.get("b"), Some(vec![2; 4]));
        assert_eq!(lru.get("c"), Some(vec![3; 5]));
        assert_eq!(lru.total_bytes, 9);

        // Oversize entry (> max_bytes) is skipped without evicting existing entries.
        let mut small = ByteLru::new(10, 4);
        small.insert("keep".into(), vec![9; 4]);
        small.insert("too_big".into(), vec![7; 11]);
        assert_eq!(small.get("too_big"), None);
        assert_eq!(small.get("keep"), Some(vec![9; 4]));
        assert_eq!(small.total_bytes, 4);

        // Overwriting an existing key updates total_bytes and order cleanly.
        small.insert("keep".into(), vec![8; 7]);
        assert_eq!(small.total_bytes, 7);
        assert_eq!(small.entries.len(), 1);
        assert_eq!(small.order.len(), 1);
        small.insert("keep".into(), vec![6; 2]);
        assert_eq!(small.total_bytes, 2);
        assert_eq!(small.get("keep"), Some(vec![6; 2]));
    }

    #[test]
    fn disk_cache_round_trips_and_prunes_under_cap() {
        let dir = unique_temp_dir("prune");
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

    #[tokio::test]
    async fn private_files_never_read_or_write_disk_cache() {
        let dir = unique_temp_dir("private");
        let saw_cookie = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_cookie_srv = Arc::clone(&saw_cookie);
        let srv = spawn_test_server(move |path, req| {
            if path.starts_with("/private/") {
                if req.to_ascii_lowercase().contains("cookie: d=xoxd-secret") {
                    saw_cookie_srv.store(true, Ordering::SeqCst);
                }
                (
                    200,
                    "application/octet-stream",
                    b"private-live-bytes".to_vec(),
                )
            } else {
                (200, "image/png", b"public-avatar-bytes".to_vec())
            }
        });

        let api = SlackApi::new(&srv.base_url, "xoxc-test", "xoxd-secret").unwrap();
        let private_prefix = format!("{}/private/", srv.base_url);
        let fetcher = Fetcher::new_for_test(
            Handle::current(),
            Some(api.clone()),
            Some(dir.clone()),
            private_prefix.clone(),
        );

        // Pre-seed disk cache for a private URL: Fetcher must NEVER read it!
        let private_url_seeded = format!("{private_prefix}seeded.png");
        write_disk_cache(&dir, &private_url_seeded, b"stale-disk-data");
        let got = fetcher.get(private_url_seeded.clone()).await.unwrap();
        assert_eq!(got, b"private-live-bytes");
        assert!(
            saw_cookie.load(Ordering::SeqCst),
            "private file download must go through signed-in SlackApi"
        );
        // Disk cache must remain untouched (still stale bytes, not overwritten).
        assert_eq!(
            read_disk_cache(&dir, &private_url_seeded).as_deref(),
            Some(b"stale-disk-data".as_slice())
        );

        // Unseeded private URL must NEVER be written to disk cache.
        let private_url_fresh = format!("{private_prefix}fresh.png");
        let got2 = fetcher.get(private_url_fresh.clone()).await.unwrap();
        assert_eq!(got2, b"private-live-bytes");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(read_disk_cache(&dir, &private_url_fresh), None);

        // Public URL DOES write to disk cache and serves cold-start hits without network.
        let public_url = format!("{}/public/avatar.png", srv.base_url);
        let got_pub = fetcher.get(public_url.clone()).await.unwrap();
        assert_eq!(got_pub, b"public-avatar-bytes");
        // Wait for async spawn_blocking disk write.
        for _ in 0..20 {
            if read_disk_cache(&dir, &public_url).is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            read_disk_cache(&dir, &public_url).as_deref(),
            Some(b"public-avatar-bytes".as_slice())
        );

        let hits_before = srv.hits.load(Ordering::SeqCst);
        let cold_fetcher = Fetcher::new_for_test(
            Handle::current(),
            Some(api),
            Some(dir.clone()),
            private_prefix,
        );
        let from_disk = cold_fetcher.get(public_url).await.unwrap();
        assert_eq!(from_disk, b"public-avatar-bytes");
        assert_eq!(
            srv.hits.load(Ordering::SeqCst),
            hits_before,
            "public cold start hit disk cache without network"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn concurrent_get_coalesces_requests_and_retries_after_error() {
        let dir = unique_temp_dir("coalesce");
        let fail_flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let fail_srv = Arc::clone(&fail_flag);
        let srv = spawn_test_server(move |path, _| {
            // Brief delay so concurrent callers subscribe to the leader in-flight entry.
            std::thread::sleep(std::time::Duration::from_millis(60));
            if path == "/flaky" && fail_srv.load(Ordering::SeqCst) {
                (500, "text/plain", b"boom".to_vec())
            } else {
                (200, "image/png", format!("ok:{path}").into_bytes())
            }
        });

        let fetcher =
            Fetcher::new_for_test(Handle::current(), None, Some(dir.clone()), SLACK_FILES);

        // 1. Concurrent success coalescing: 5 callers -> 1 HTTP request.
        let ok_url = format!("{}/avatar.png", srv.base_url);
        let futs: Vec<_> = (0..5).map(|_| fetcher.get(ok_url.clone())).collect();
        let results = futures::future::join_all(futs).await;
        assert_eq!(srv.hits.load(Ordering::SeqCst), 1);
        for res in results {
            assert_eq!(res.unwrap(), b"ok:/avatar.png");
        }

        // 2. Concurrent error coalescing: 4 callers -> 1 HTTP request, all receive Err.
        let err_url = format!("{}/flaky", srv.base_url);
        let err_futs: Vec<_> = (0..4).map(|_| fetcher.get(err_url.clone())).collect();
        let err_results = futures::future::join_all(err_futs).await;
        assert_eq!(srv.hits.load(Ordering::SeqCst), 2);
        for res in err_results {
            assert!(res.is_err());
        }

        // 3. Recovery: after failure, retry is NOT poisoned and succeeds once server recovers.
        fail_flag.store(false, Ordering::SeqCst);
        let recovered = fetcher.get(err_url).await.unwrap();
        assert_eq!(recovered, b"ok:/flaky");
        assert_eq!(srv.hits.load(Ordering::SeqCst), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn oversize_asset_returns_bytes_without_caching_in_memory_or_disk() {
        let dir = unique_temp_dir("oversize");
        let big_len = MAX_SINGLE_ENTRY_BYTES + 1;
        let srv =
            spawn_test_server(move |_, _| (200, "application/octet-stream", vec![0xAB; big_len]));

        let fetcher =
            Fetcher::new_for_test(Handle::current(), None, Some(dir.clone()), SLACK_FILES);
        let url = format!("{}/huge.bin", srv.base_url);
        let bytes = fetcher.get(url.clone()).await.unwrap();
        assert_eq!(bytes.len(), big_len);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            fetcher.cache.lock().unwrap().get(&url).is_none(),
            "oversize entry skips memory LRU"
        );
        assert_eq!(
            read_disk_cache(&dir, &url),
            None,
            "oversize entry skips disk cache"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_flight_guard_cleans_up_entry_on_panic() {
        let in_flight: Arc<Mutex<InFlightMap>> = Arc::new(Mutex::new(HashMap::new()));
        let url = "https://ca.slack-edge.com/T1-panic".to_owned();
        let (tx, _rx) = tokio::sync::broadcast::channel(1);
        in_flight.lock().unwrap().insert(url.clone(), tx);

        let map_clone = Arc::clone(&in_flight);
        let url_clone = url.clone();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = InFlightGuard {
                in_flight: map_clone,
                url: url_clone,
            };
            panic!("simulated leader panic");
        }));
        assert!(res.is_err());
        assert!(
            !in_flight.lock().unwrap().contains_key(&url),
            "Drop guard must remove URL from in_flight on panic"
        );
    }
}
