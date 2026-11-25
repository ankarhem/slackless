//! Transport: base URL, auth (form `token` + `Cookie: d=`), rate-limit retries, response
//! decoding.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderValue, RETRY_AFTER};
use reqwest::{Request, Response, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::types::ApiResponse;
use crate::types::endpoints;
use crate::{ApiError, Result};

/// Maximum entries retained in the in-memory diagnostic net-log ring buffer (`F-SHELL-158`).
const MAX_NET_LOG_ENTRIES: usize = 500;

static NET_LOG_ENABLED: AtomicBool = AtomicBool::new(false);
static NET_LOG: OnceLock<Mutex<VecDeque<NetLogEntry>>> = OnceLock::new();
static PROXY_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// One redacted HTTP request/response entry captured when net-log diagnostics are enabled (`F-SHELL-158`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NetLogEntry {
    pub timestamp_ms: u64,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

/// Enable or disable in-memory HTTP net-log capture (`F-SHELL-158`).
pub fn set_net_log_enabled(enabled: bool) {
    NET_LOG_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether HTTP net-log capture is currently enabled.
pub fn net_log_enabled() -> bool {
    NET_LOG_ENABLED.load(Ordering::Relaxed)
}

/// Snapshot captured net-log entries (oldest first).
pub fn net_log_snapshot() -> Vec<NetLogEntry> {
    NET_LOG
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default()
}

/// Clear all captured net-log entries.
pub fn clear_net_log() {
    if let Ok(mut g) = NET_LOG.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        g.clear();
    }
}

/// Export captured net-log entries as pretty-printed JSON (credentials never logged).
pub fn net_log_export_json() -> String {
    let entries = net_log_snapshot();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_owned())
}

/// Configure an explicit HTTP/HTTPS/SOCKS proxy URL for new [`SlackApi`] clients (`F-SHELL-192`).
/// Passing `None` or an empty string clears the override (falling back to standard env vars).
pub fn set_proxy_url(url: Option<String>) {
    let cleaned = url.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if let Ok(mut g) = PROXY_URL.get_or_init(|| Mutex::new(None)).lock() {
        *g = cleaned;
    }
}

/// Currently configured proxy URL override, or `SLACKLESS_PROXY` env var if set.
pub fn proxy_url() -> Option<String> {
    if let Ok(g) = PROXY_URL.get_or_init(|| Mutex::new(None)).lock() {
        if g.is_some() {
            return g.clone();
        }
    }
    std::env::var("SLACKLESS_PROXY")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn redact_url(url: &Url) -> String {
    let mut clean = url.clone();
    clean.set_query(None);
    clean.set_fragment(None);
    clean.to_string()
}

fn record_net_log(
    method: &str,
    url: &Url,
    status: Option<u16>,
    elapsed: Duration,
    error: Option<String>,
) {
    if !NET_LOG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let entry = NetLogEntry {
        timestamp_ms,
        method: method.to_owned(),
        url: redact_url(url),
        status,
        elapsed_ms: elapsed.as_millis() as u64,
        error,
    };
    if let Ok(mut g) = NET_LOG.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        if g.len() >= MAX_NET_LOG_ENTRIES {
            g.pop_front();
        }
        g.push_back(entry);
    }
}

/// How many times a request is retried after HTTP 429 before giving up with
/// [`ApiError::RateLimited`].
pub const MAX_RATE_LIMIT_RETRIES: u32 = 3;
/// Wait used when a 429 carries no usable `Retry-After` (slackdump uses the same 5s default).
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(5);
/// Upper bound on a single `Retry-After` wait.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
/// Browser-like UA; the web client is a browser and Slack sees what it sends.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

/// Form fields of one API call, `token` excluded (added by the client).
pub(crate) type Params = Vec<(&'static str, String)>;

/// Async client for Slack's private web-client REST API, bound to one workspace session.
///
/// Cheap to clone: the inner `reqwest::Client` is reference-counted.
#[derive(Clone)]
pub struct SlackApi {
    http: reqwest::Client,
    /// API base ending in `/`, e.g. `https://acme.slack.com/api/`.
    base: Url,
    /// Edge cache API base ending in `/`, e.g. `https://edgeapi.slack.com/cache/`.
    edge_base: Url,
    token: String,
    /// Prebuilt `d=<url-encoded xoxd>` header value.
    cookie: HeaderValue,
}

impl fmt::Debug for SlackApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackApi")
            .field("base", &self.base.as_str())
            .field("token", &"<redacted>")
            .field("cookie", &"<redacted>")
            .finish()
    }
}

impl SlackApi {
    /// A client for the API base `base_url` (e.g. `https://acme.slack.com/api/`), the session
    /// token (`xoxc-...`) and the `d` cookie value (`xoxd-...`, raw or already URL-encoded).
    /// Honors [`set_proxy_url`] / `SLACKLESS_PROXY` (plus standard `HTTPS_PROXY` / `HTTP_PROXY`
    /// environment variables, `F-SHELL-192`).
    pub fn new(base_url: &str, token: impl Into<String>, d_cookie: &str) -> Result<Self> {
        let mut builder = reqwest::Client::builder().user_agent(BROWSER_USER_AGENT);
        if let Some(proxy) = proxy_url() {
            builder = builder.proxy(reqwest::Proxy::all(&proxy)?);
        }
        let http = builder.build()?;
        Self::with_http_client(http, base_url, token, d_cookie)
    }

    /// As [`SlackApi::new`] for workspace domain `acme` (Grid: `acme.enterprise`).
    pub fn for_workspace(domain: &str, token: impl Into<String>, d_cookie: &str) -> Result<Self> {
        Self::new(&endpoints::workspace_api_base(domain), token, d_cookie)
    }

    /// As [`SlackApi::new`] with a caller-configured `reqwest::Client`.
    pub fn with_http_client(
        http: reqwest::Client,
        base_url: &str,
        token: impl Into<String>,
        d_cookie: &str,
    ) -> Result<Self> {
        let mut base = Url::parse(base_url)?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let mut cookie = HeaderValue::from_str(&format!("d={}", encode_cookie(d_cookie)))
            .expect("percent-encoded cookie value is visible ascii");
        cookie.set_sensitive(true);
        Ok(Self {
            http,
            base,
            edge_base: Url::parse(endpoints::EDGE_API_BASE).expect("EDGE_API_BASE is a valid URL"),
            token: token.into(),
            cookie,
        })
    }

    /// Overrides the Edge cache API base URL (defaults to [`endpoints::EDGE_API_BASE`]).
    pub fn with_edge_base(mut self, edge_base: &str) -> Result<Self> {
        let mut url = Url::parse(edge_base)?;
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        self.edge_base = url;
        Ok(self)
    }

    /// The API base URL this client posts to.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// `POST <base><method>` with `token` + `params` form-encoded and the `d` cookie.
    pub(crate) fn build_request(
        &self,
        method: &str,
        params: &[(&'static str, String)],
    ) -> Result<Request> {
        let url = self.base.join(method)?;
        let form: Vec<(&str, &str)> = std::iter::once(("token", self.token.as_str()))
            .chain(params.iter().map(|(k, v)| (*k, v.as_str())))
            .collect();
        Ok(self
            .http
            .post(url)
            .header(COOKIE, self.cookie.clone())
            .form(&form)
            .build()?)
    }

    /// Calls `method` and decodes the payload, mapping `ok: false` to [`ApiError`].
    pub(crate) async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Params,
    ) -> Result<ApiResponse<T>> {
        let resp = self
            .send_with_retry(|| self.build_request(method, &params))
            .await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => Ok(ApiResponse::<T>::from_value(value)?),
            Err(_) if !status.is_success() => Err(status_error(status, &body)),
            Err(e) => Err(ApiError::Decode(e)),
        }
    }

    /// Posts JSON to Slack's Edge cache API (`https://edgeapi.slack.com/cache/<team>/<path>`),
    /// attaching `token` in the JSON body and the `d` session cookie (docs/research/api-search.md).
    pub(crate) async fn call_edge_json<T: DeserializeOwned>(
        &self,
        team_id: &str,
        path: &str,
        mut payload: serde_json::Value,
    ) -> Result<ApiResponse<T>> {
        let url = Url::parse(&format!(
            "{}{}/{}?_x_app_name=client",
            self.edge_base, team_id, path
        ))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "token".to_owned(),
                serde_json::Value::String(self.token.clone()),
            );
        }
        let body_bytes = serde_json::to_vec(&payload)?;
        let resp = self
            .send_with_retry(|| {
                Ok(self
                    .http
                    .post(url.clone())
                    .header(COOKIE, self.cookie.clone())
                    .header(CONTENT_TYPE, "text/plain;charset=UTF-8")
                    .body(body_bytes.clone())
                    .build()?)
            })
            .await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => Ok(ApiResponse::<T>::from_value(value)?),
            Err(_) if !status.is_success() => Err(status_error(status, &body)),
            Err(e) => Err(ApiError::Decode(e)),
        }
    }

    /// Downloads a Slack-hosted file (`url_private`, `url_private_download`, thumbnails).
    ///
    /// File hosts authenticate the browser way, with the `d` cookie only. The cookie is sent only
    /// to `https` Slack hosts (or this client's own API host); anything else is refused.
    pub async fn download_file(&self, url_private: &str) -> Result<Bytes> {
        let url = Url::parse(url_private)?;
        if !self.is_trusted_file_url(&url) {
            return Err(ApiError::UntrustedUrl(url_private.to_owned()));
        }
        let resp = self
            .send_with_retry(|| {
                Ok(self
                    .http
                    .get(url.clone())
                    .header(COOKIE, self.cookie.clone())
                    .build()?)
            })
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await?;
            return Err(status_error(status, &body));
        }
        // A 200 text/html is Slack's sign-in page: the cookie was not accepted.
        let is_html = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| {
                ct.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("text/html")
            });
        if is_html {
            return Err(ApiError::FileAuthRejected);
        }
        Ok(resp.bytes().await?)
    }

    /// Uploads raw bytes to an external upload slot returned by `files.getUploadURLExternal`.
    /// No Slack session cookie or token is sent; retries on HTTP 429.
    pub(crate) async fn post_upload_bytes(&self, upload_url: &str, bytes: &[u8]) -> Result<()> {
        let url = Url::parse(upload_url)?;
        let resp = self
            .send_with_retry(|| {
                Ok(self
                    .http
                    .post(url.clone())
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .body(bytes.to_vec())
                    .build()?)
            })
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await?;
            return Err(status_error(status, &body));
        }
        Ok(())
    }

    /// Posts a `multipart/form-data` API method carrying `token` and a binary file field
    /// (used by `users.setPhoto`, `F-SHELL-004`).
    pub(crate) async fn post_multipart_file(
        &self,
        api_method: &str,
        field_name: &str,
        filename: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<serde_json::Value> {
        let url = self.base.join(api_method)?;
        let boundary = "----SlacklessMultipartBoundary7MA4YWxkTrZu0gW";
        let mut body = Vec::with_capacity(bytes.len() + 512);
        let safe_name = filename.replace(['"', '\r', '\n'], "_");
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\n{}\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; name=\"{field_name}\"; filename=\"{safe_name}\"\r\n\
                 Content-Type: {content_type}\r\n\r\n",
                self.token
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let ct_header = format!("multipart/form-data; boundary={boundary}");
        let resp = self
            .send_with_retry(|| {
                Ok(self
                    .http
                    .post(url.clone())
                    .header(COOKIE, self.cookie.clone())
                    .header(CONTENT_TYPE, ct_header.clone())
                    .body(body.clone())
                    .build()?)
            })
            .await?;
        let status = resp.status();
        let resp_bytes = resp.bytes().await?;
        match serde_json::from_slice::<serde_json::Value>(&resp_bytes) {
            Ok(value) => Ok(ApiResponse::<serde_json::Value>::from_value(value)?.data),
            Err(_) if !status.is_success() => Err(status_error(status, &resp_bytes)),
            Err(e) => Err(ApiError::Decode(e)),
        }
    }

    fn is_trusted_file_url(&self, url: &Url) -> bool {
        let same_origin = url.origin() == self.base.origin();
        let slack_host = url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|h| h == "slack.com" || h.ends_with(".slack.com"));
        same_origin || slack_host
    }

    /// Sends the request built by `build`, retrying on HTTP 429 after `Retry-After`, at most
    /// [`MAX_RATE_LIMIT_RETRIES`] times.
    async fn send_with_retry(&self, build: impl Fn() -> Result<Request>) -> Result<Response> {
        let mut attempt = 0;
        loop {
            let req = build()?;
            let capture = NET_LOG_ENABLED.load(Ordering::Relaxed);
            let meta =
                capture.then(|| (req.method().to_string(), req.url().clone(), Instant::now()));
            let resp = match self.http.execute(req).await {
                Ok(resp) => {
                    if let Some((method, url, started)) = meta {
                        record_net_log(
                            &method,
                            &url,
                            Some(resp.status().as_u16()),
                            started.elapsed(),
                            None,
                        );
                    }
                    resp
                }
                Err(err) => {
                    if let Some((method, url, started)) = meta {
                        record_net_log(
                            &method,
                            &url,
                            None,
                            started.elapsed(),
                            Some(err.to_string()),
                        );
                    }
                    return Err(err.into());
                }
            };
            if resp.status() != StatusCode::TOO_MANY_REQUESTS {
                return Ok(resp);
            }
            let retry_after = retry_after(&resp);
            if attempt >= MAX_RATE_LIMIT_RETRIES {
                return Err(ApiError::RateLimited { retry_after });
            }
            attempt += 1;
            tokio::time::sleep(retry_after).await;
        }
    }
}

/// `Retry-After` in seconds, defaulted when missing/unparseable and capped.
fn retry_after(resp: &Response) -> Duration {
    parse_retry_after(resp.headers().get(RETRY_AFTER))
}

fn parse_retry_after(header: Option<&HeaderValue>) -> Duration {
    header
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_AFTER, Duration::from_secs)
        .min(MAX_RETRY_AFTER)
}

fn status_error(status: StatusCode, body: &[u8]) -> ApiError {
    let body = String::from_utf8_lossy(body);
    ApiError::Status {
        status: status.as_u16(),
        body: body.chars().take(512).collect(),
    }
}

/// URL-encodes the `d` cookie value exactly once. Browsers store it already encoded
/// (`xoxd-...%2F...%3D`); double-encoding breaks auth, so a value made only of URL-safe
/// characters and `%` escapes is kept as is.
fn encode_cookie(value: &str) -> String {
    let already_encoded = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b));
    if already_encoded {
        value.to_owned()
    } else {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form_pairs(req: &Request) -> Vec<(String, String)> {
        let body = req
            .body()
            .and_then(|b| b.as_bytes())
            .expect("buffered form body");
        url::form_urlencoded::parse(body).into_owned().collect()
    }

    #[test]
    fn builds_web_client_request() {
        let api = SlackApi::new(
            "https://acme.slack.com/api",
            "xoxc-1-2-3-abc",
            "xoxd-a/b+c=",
        )
        .unwrap();
        let req = api
            .build_request(
                crate::types::endpoints::method::CONVERSATIONS_HISTORY,
                &[("channel", "C123".to_owned())],
            )
            .unwrap();

        assert_eq!(req.method(), reqwest::Method::POST);
        assert_eq!(
            req.url().as_str(),
            "https://acme.slack.com/api/conversations.history"
        );
        assert_eq!(req.headers()[COOKIE], "d=xoxd-a%2Fb%2Bc%3D");
        assert_eq!(
            req.headers()[CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(
            form_pairs(&req),
            vec![
                ("token".to_owned(), "xoxc-1-2-3-abc".to_owned()),
                ("channel".to_owned(), "C123".to_owned()),
            ]
        );
    }

    #[test]
    fn already_encoded_cookie_is_not_double_encoded() {
        assert_eq!(encode_cookie("xoxd-a%2Fb%3D"), "xoxd-a%2Fb%3D");
        assert_eq!(encode_cookie("xoxd-a/b="), "xoxd-a%2Fb%3D");
    }

    #[test]
    fn debug_redacts_credentials() {
        let api = SlackApi::for_workspace("acme", "xoxc-secret", "xoxd-secret").unwrap();
        let dbg = format!("{api:?}");
        assert!(!dbg.contains("secret"), "{dbg}");
        assert_eq!(api.base_url().as_str(), "https://acme.slack.com/api/");
    }

    #[test]
    fn cookie_only_goes_to_slack_hosts() {
        let api = SlackApi::for_workspace("acme", "t", "d").unwrap();
        let ok = |u: &str| api.is_trusted_file_url(&Url::parse(u).unwrap());
        assert!(ok("https://files.slack.com/files-pri/T1-F1/a.png"));
        assert!(!ok("http://files.slack.com/files-pri/T1-F1/a.png"));
        assert!(!ok("https://files.slack.com.evil.example/x"));
        assert!(!ok("https://example.com/x"));
    }

    #[test]
    fn net_log_redacts_query_strings_and_caps_entries() {
        static TEST_GUARD: Mutex<()> = Mutex::new(());
        let _lock = TEST_GUARD.lock().unwrap();

        clear_net_log();
        set_net_log_enabled(true);
        assert!(net_log_enabled());

        let url =
            Url::parse("https://acme.slack.com/api/auth.test?token=xoxc-secret#frag").unwrap();
        record_net_log("POST", &url, Some(200), Duration::from_millis(12), None);
        let snap = net_log_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].url, "https://acme.slack.com/api/auth.test");
        assert_eq!(snap[0].status, Some(200));
        assert!(!net_log_export_json().contains("xoxc-secret"));

        // Pushing more than MAX_NET_LOG_ENTRIES (500) evicts oldest entries first.
        clear_net_log();
        for i in 0..(MAX_NET_LOG_ENTRIES + 25) {
            let entry_url = Url::parse(&format!("https://acme.slack.com/api/m{i}")).unwrap();
            record_net_log(
                "POST",
                &entry_url,
                Some(200),
                Duration::from_millis(1),
                None,
            );
        }
        let capped = net_log_snapshot();
        assert_eq!(capped.len(), MAX_NET_LOG_ENTRIES);
        assert_eq!(capped[0].url, "https://acme.slack.com/api/m25");
        assert_eq!(
            capped.last().unwrap().url,
            format!("https://acme.slack.com/api/m{}", MAX_NET_LOG_ENTRIES + 24)
        );

        set_net_log_enabled(false);
        assert!(!net_log_enabled());
        clear_net_log();
    }

    #[test]
    fn proxy_url_override_trims_and_clears() {
        static PROXY_GUARD: Mutex<()> = Mutex::new(());
        let _lock = PROXY_GUARD.lock().unwrap();

        set_proxy_url(None);
        set_proxy_url(Some("  http://127.0.0.1:8080  ".to_owned()));
        assert_eq!(proxy_url().as_deref(), Some("http://127.0.0.1:8080"));

        set_proxy_url(Some("   ".to_owned()));
        assert_ne!(proxy_url().as_deref(), Some(""));
        assert_ne!(proxy_url().as_deref(), Some("http://127.0.0.1:8080"));

        set_proxy_url(None);
    }

    #[test]
    fn retry_after_defaults_on_missing_or_invalid_and_clamps_to_max() {
        assert_eq!(parse_retry_after(None), Duration::from_secs(5));
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("not-a-number"))),
            Duration::from_secs(5)
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static(
                "Wed, 21 Oct 2015 07:28:00 GMT"
            ))),
            Duration::from_secs(5)
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("-10"))),
            Duration::from_secs(5)
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static(" 12 "))),
            Duration::from_secs(12)
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("0"))),
            Duration::from_secs(0)
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("120"))),
            MAX_RETRY_AFTER
        );
        assert_eq!(
            parse_retry_after(Some(&HeaderValue::from_static("999999"))),
            MAX_RETRY_AFTER
        );
    }
}
