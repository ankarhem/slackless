//! Transport: base URL, auth (form `token` + `Cookie: d=`), rate-limit retries, response
//! decoding.

use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderValue, RETRY_AFTER};
use reqwest::{Request, Response, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

use crate::types::ApiResponse;
use crate::types::endpoints;
use crate::{ApiError, Result};

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
    pub fn new(base_url: &str, token: impl Into<String>, d_cookie: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(BROWSER_USER_AGENT)
            .build()?;
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
            token: token.into(),
            cookie,
        })
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
            .is_some_and(|ct| ct.trim_start().starts_with("text/html"));
        if is_html {
            return Err(ApiError::FileAuthRejected);
        }
        Ok(resp.bytes().await?)
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
            let resp = self.http.execute(build()?).await?;
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
    resp.headers()
        .get(RETRY_AFTER)
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
}
