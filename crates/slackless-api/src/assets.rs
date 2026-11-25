//! Credential-free downloads of public Slack assets: profile photos, bot icons, custom emoji
//! and workspace icons all live on public CDNs (`avatars.slack-edge.com`, `emoji.slack-edge.com`,
//! gravatar), so no token or cookie is sent with them.

use std::time::Instant;

use url::Url;

use crate::client::{
    DEFAULT_REQUEST_TIMEOUT, build_http_client, net_log_enabled, proxy_url, record_net_log,
};
use crate::{ApiError, Result};

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
        Ok(response.bytes().await?.to_vec())
    }
}
