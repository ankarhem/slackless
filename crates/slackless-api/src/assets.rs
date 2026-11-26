//! Credential-free downloads. Public Slack assets (profile photos, bot icons, custom emoji and
//! workspace icons on public CDNs: `avatars.slack-edge.com`, `emoji.slack-edge.com`, gravatar)
//! are buffered by [`AssetClient::get`]; large public files (model weights, release archives)
//! stream to disk through [`AssetClient::download_to_path`]. No token or cookie is sent.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use bytes::Bytes;
use tokio::io::AsyncWriteExt as _;
use url::Url;

use crate::client::{
    DEFAULT_REQUEST_TIMEOUT, build_http_client, net_log_enabled, proxy_url, record_net_log,
};
use crate::{ApiError, DownloadError, Result};

/// Largest body [`AssetClient::get`] buffers. Avatars, emoji, icons and message images are far
/// smaller; the cap keeps a hostile or broken image host from streaming an endless body into the
/// memory of every client that renders it.
const MAX_ASSET_BYTES: u64 = 32 * 1024 * 1024;
/// Bytes [`stream_to_path`] writes between two progress reports.
const PROGRESS_STEP: u64 = 256 * 1024;

/// Cheap to clone: the inner `reqwest::Client` is reference-counted.
#[derive(Clone, Debug)]
pub struct AssetClient {
    http: reqwest::Client,
}

impl Default for AssetClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetClient {
    /// Creates an asset client honoring [`crate::set_proxy_url`] / `SLACKLESS_PROXY`, browser
    /// User-Agent, and default connect/request timeouts.
    pub fn new() -> Self {
        if let Ok(client) = Self::with_proxy(proxy_url().as_deref()) {
            return client;
        }
        let http = build_http_client(None, DEFAULT_REQUEST_TIMEOUT).unwrap_or_default();
        Self { http }
    }

    /// Creates an asset client with an explicit proxy override (or `None` to use env defaults).
    pub fn with_proxy(proxy: Option<&str>) -> Result<Self> {
        let http = build_http_client(proxy, DEFAULT_REQUEST_TIMEOUT)?;
        Ok(Self { http })
    }

    /// Creates an asset client backed by a caller-configured `reqwest::Client`.
    pub fn with_http_client(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// The body of `url`, which must answer 2xx.
    ///
    /// # Errors
    ///
    /// [`ApiError::BodyTooLarge`] when the body announces or streams more than 32 MiB,
    /// [`ApiError::Status`] for a non-2xx answer, and the URL or transport error otherwise.
    pub async fn get(&self, url: &str) -> Result<Vec<u8>> {
        read_capped(self.send_get(url).await?, MAX_ASSET_BYTES).await
    }

    /// Streams the body of `url`, which must answer 2xx, into `dest` and returns its length.
    /// The body goes to a `.part` file that is renamed over `dest` once complete, so `dest` never
    /// holds a partial body. `on_progress` gets (bytes written, announced length) as the body
    /// starts and every 256 KiB.
    ///
    /// # Errors
    ///
    /// [`DownloadError::Api`] with [`ApiError::BodyTooLarge`] when the body announces or streams
    /// more than `max_bytes`, [`ApiError::Status`] for a non-2xx answer, or the URL or transport
    /// error; [`DownloadError::Write`] when the file cannot be written. A failed download leaves
    /// `dest` as it was and no partial file behind.
    pub async fn download_to_path(
        &self,
        url: &str,
        dest: &Path,
        max_bytes: u64,
        on_progress: impl FnMut(u64, Option<u64>),
    ) -> Result<u64, DownloadError> {
        let response = self.send_get(url).await?;
        stream_to_path(response, dest, max_bytes, on_progress).await
    }

    /// GETs `url`, recording it in the net log; a non-2xx answer is [`ApiError::Status`].
    async fn send_get(&self, url: &str) -> Result<reqwest::Response> {
        let parsed = Url::parse(url)?;
        let started = net_log_enabled().then(Instant::now);
        let response = match self.http.get(parsed.clone()).send().await {
            Ok(resp) => {
                if let Some(started) = started {
                    record_net_log(
                        "GET",
                        &parsed,
                        Some(resp.status().as_u16()),
                        started.elapsed(),
                        None,
                    );
                }
                resp
            }
            Err(err) => {
                if let Some(started) = started {
                    record_net_log(
                        "GET",
                        &parsed,
                        None,
                        started.elapsed(),
                        Some(err.to_string()),
                    );
                }
                return Err(err.into());
            }
        };
        let status = response.status();
        if !status.is_success() {
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: String::new(),
            });
        }
        Ok(response)
    }
}

/// A response body that fails with [`ApiError::BodyTooLarge`] as soon as it announces
/// (`Content-Length`) or streams more than `limit` bytes.
struct CappedBody {
    response: reqwest::Response,
    announced: Option<u64>,
    limit: u64,
    received: u64,
}

impl CappedBody {
    fn new(response: reqwest::Response, limit: u64) -> Result<Self> {
        let announced = response.content_length();
        if announced.is_some_and(|len| len > limit) {
            return Err(ApiError::BodyTooLarge { limit });
        }
        Ok(Self {
            response,
            announced,
            limit,
            received: 0,
        })
    }

    /// The next chunk of the body, `None` once it is complete.
    async fn chunk(&mut self) -> Result<Option<Bytes>> {
        let Some(chunk) = self.response.chunk().await? else {
            return Ok(None);
        };
        self.received = self
            .received
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if self.received > self.limit {
            return Err(ApiError::BodyTooLarge { limit: self.limit });
        }
        Ok(Some(chunk))
    }
}

/// Buffers `response`'s body, capped like [`CappedBody`].
async fn read_capped(response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    let mut body = CappedBody::new(response, limit)?;
    let capacity = body
        .announced
        .map_or(0, |len| usize::try_from(len).unwrap_or(0));
    let mut buf = Vec::with_capacity(capacity);
    while let Some(chunk) = body.chunk().await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Streams `response`'s body, capped like [`CappedBody`], into a `.part` file beside `dest` and
/// renames it over `dest`. The `.part` name is unique per call, so two downloads of one file
/// never write into each other; on any failure it is removed and `dest` is left as it was.
async fn stream_to_path(
    response: reqwest::Response,
    dest: &Path,
    max_bytes: u64,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, DownloadError> {
    static PART_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut body = CappedBody::new(response, max_bytes)?;
    let mut path = dest.as_os_str().to_owned();
    path.push(format!(
        ".{}-{}.part",
        std::process::id(),
        PART_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let path = PathBuf::from(path);
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(DownloadError::Write)?;
    let part = PartFile { path, kept: false };
    on_progress(0, body.announced);
    let mut reported = 0;
    while let Some(chunk) = body.chunk().await? {
        file.write_all(&chunk).await.map_err(DownloadError::Write)?;
        if body.received - reported >= PROGRESS_STEP {
            reported = body.received;
            on_progress(reported, body.announced);
        }
    }
    file.flush().await.map_err(DownloadError::Write)?;
    drop(file);
    part.rename_to(dest).await.map_err(DownloadError::Write)?;
    Ok(body.received)
}

/// A download's partial file, removed when dropped (a failed or cancelled download) unless
/// [`PartFile::rename_to`] moved it into place.
struct PartFile {
    path: PathBuf,
    kept: bool,
}

impl PartFile {
    async fn rename_to(mut self, dest: &Path) -> std::io::Result<()> {
        tokio::fs::rename(&self.path, dest).await?;
        self.kept = true;
        Ok(())
    }
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if !self.kept
            && let Err(err) = std::fs::remove_file(&self.path)
        {
            tracing::warn!(path = %self.path.display(), %err, "could not remove a partial download");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpListener;

    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// Serves one response streaming `len` body bytes with chunked transfer encoding and no
    /// `Content-Length`, so only a running byte count can stop the reader.
    fn serve_chunked_once(len: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!(
            "http://{}/endless.png",
            listener.local_addr().expect("addr")
        );
        std::thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut request = BufReader::new(stream.try_clone()?);
            let mut line = String::new();
            while request.read_line(&mut line)? > 0 && line != "\r\n" {
                line.clear();
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")?;
            let chunk = vec![b'x'; 64 * 1024];
            let mut sent = 0;
            while sent < len {
                let n = chunk.len().min(len - sent);
                write!(stream, "{n:x}\r\n")?;
                stream.write_all(&chunk[..n])?;
                stream.write_all(b"\r\n")?;
                sent += n;
            }
            stream.write_all(b"0\r\n\r\n")
        });
        url
    }

    #[tokio::test]
    async fn get_rejects_bodies_over_the_cap_and_returns_small_ones() {
        let over_cap = usize::try_from(MAX_ASSET_BYTES).expect("cap fits in memory") + 1;
        let server = MockServer::start().await;
        Mock::given(path("/huge.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; over_cap]))
            .mount(&server)
            .await;
        Mock::given(path("/small.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG!".to_vec()))
            .mount(&server)
            .await;
        // No proxy lookup: other unit tests toggle the process-wide proxy override.
        let client = AssetClient::with_proxy(None).expect("asset client");
        let too_large = |result: &Result<Vec<u8>>| {
            matches!(
                result,
                Err(ApiError::BodyTooLarge {
                    limit: MAX_ASSET_BYTES
                })
            )
        };

        // Announced too large: refused from the headers, before any body is buffered.
        let announced = client.get(&format!("{}/huge.png", server.uri())).await;
        assert!(too_large(&announced), "{:?}", announced.map(|b| b.len()));

        // No length announced: stopped once the running total passes the cap.
        let streamed = client.get(&serve_chunked_once(over_cap)).await;
        assert!(too_large(&streamed), "{:?}", streamed.map(|b| b.len()));

        let small = client.get(&format!("{}/small.png", server.uri())).await;
        assert_eq!(small.expect("small body"), b"PNG!");
    }

    #[tokio::test]
    async fn download_to_path_streams_to_disk_and_leaves_nothing_behind_on_failure() {
        let body: Vec<u8> = (0..=255u8).cycle().take(600 * 1024).collect();
        let len = u64::try_from(body.len()).expect("body length");
        let server = MockServer::start().await;
        Mock::given(path("/model.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        let dir =
            std::env::temp_dir().join(format!("slackless-api-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let dest = dir.join("model.bin");
        let client = AssetClient::with_proxy(None).expect("asset client");

        let mut reports = Vec::new();
        let written = client
            .download_to_path(&url, &dest, len, |done, total| reports.push((done, total)))
            .await
            .expect("download");
        assert_eq!(written, len);
        assert_eq!(std::fs::read(&dest).expect("downloaded file"), body);
        // Progress opens at zero with the announced size, then advances a step at a time.
        assert_eq!(reports[0], (0, Some(len)));
        assert!(
            reports.len() > 1
                && reports
                    .windows(2)
                    .all(|w| w[1].0 >= w[0].0 + PROGRESS_STEP && w[1].1 == Some(len)),
            "{reports:?}"
        );

        // An error page, a body announced over the cap, and one streamed past it (aimed at the
        // finished download) all fail without touching `dest` or leaving a partial file behind.
        let missing = client
            .download_to_path(
                &format!("{}/missing.bin", server.uri()),
                &dir.join("missing.bin"),
                len,
                |_, _| {},
            )
            .await;
        assert!(
            matches!(
                missing,
                Err(DownloadError::Api(ApiError::Status { status: 404, .. }))
            ),
            "{missing:?}"
        );
        let too_large = |result: &Result<u64, DownloadError>| {
            matches!(
                result,
                Err(DownloadError::Api(ApiError::BodyTooLarge { .. }))
            )
        };
        let announced = client
            .download_to_path(&url, &dir.join("other.bin"), len - 1, |_, _| {})
            .await;
        assert!(too_large(&announced), "{announced:?}");
        let streamed = client
            .download_to_path(&serve_chunked_once(body.len() + 1), &dest, len, |_, _| {})
            .await;
        assert!(too_large(&streamed), "{streamed:?}");
        assert_eq!(std::fs::read(&dest).expect("earlier download"), body);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(names, ["model.bin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
