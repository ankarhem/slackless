//! Credential-free downloads of public Slack assets: profile photos, bot icons, custom emoji
//! and workspace icons all live on public CDNs (`avatars.slack-edge.com`, `emoji.slack-edge.com`,
//! gravatar), so no token or cookie is sent with them.

use std::time::Instant;

use url::Url;

use crate::client::{
    DEFAULT_REQUEST_TIMEOUT, build_http_client, net_log_enabled, proxy_url, record_net_log,
};
use crate::{ApiError, Result};

/// Largest body [`AssetClient::get`] buffers. Avatars, emoji, icons and message images are far
/// smaller; the cap keeps a hostile or broken image host from streaming an endless body into the
/// memory of every client that renders it.
const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;

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
        read_capped(response, MAX_ASSET_BYTES).await
    }
}

/// Buffers `response`'s body, failing with [`ApiError::BodyTooLarge`] as soon as it announces
/// (`Content-Length`) or streams more than `limit` bytes.
async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let announced = response
        .content_length()
        .map(|len| usize::try_from(len).unwrap_or(usize::MAX));
    if announced.is_some_and(|len| len > limit) {
        return Err(ApiError::BodyTooLarge { limit });
    }
    let mut body = Vec::with_capacity(announced.unwrap_or(0));
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ApiError::BodyTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
        let server = MockServer::start().await;
        Mock::given(path("/huge.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; MAX_ASSET_BYTES + 1]))
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
        let streamed = client.get(&serve_chunked_once(MAX_ASSET_BYTES + 1)).await;
        assert!(too_large(&streamed), "{:?}", streamed.map(|b| b.len()));

        let small = client.get(&format!("{}/small.png", server.uri())).await;
        assert_eq!(small.expect("small body"), b"PNG!");
    }
}
