//! Credential-free downloads of public Slack assets: profile photos, bot icons, custom emoji
//! and workspace icons all live on public CDNs (`avatars.slack-edge.com`, `emoji.slack-edge.com`,
//! gravatar), so no token or cookie is sent with them.

use crate::{ApiError, Result};

/// Cheap to clone: the inner `reqwest::Client` is reference-counted.
#[derive(Clone, Debug, Default)]
pub struct AssetClient {
    http: reqwest::Client,
}

impl AssetClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// The body of `url`, which must answer 2xx.
    pub async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.http.get(url).send().await?;
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
