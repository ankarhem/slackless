//! Image downloads for front-ends (profile photos, emoji, workspace icons, file thumbnails), run
//! on the core's tokio runtime so a front-end on another executor (GPUI's) shares the core's
//! HTTP stack.

use std::future::Future;

use tokio::runtime::Handle;

use crate::api::{AssetClient, SlackApi};

/// Where Slack serves workspace files and their thumbnails; these need the session cookie.
const SLACK_FILES: &str = "https://files.slack.com/";

/// Cheap to clone; hand one to whatever loads remote images.
#[derive(Clone)]
pub struct Fetcher {
    runtime: Handle,
    assets: AssetClient,
    /// The signed-in client, for Slack-hosted files (`None` when it could not be built).
    files: Option<SlackApi>,
}

impl Fetcher {
    pub(crate) fn new(runtime: Handle, files: Option<SlackApi>) -> Self {
        Self {
            runtime,
            assets: AssetClient::new(),
            files,
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
        let assets = self.assets.clone();
        let files = self.files.clone().filter(|_| url.starts_with(SLACK_FILES));
        let task = self.runtime.spawn(async move {
            match files {
                Some(api) => api
                    .download_file(&url)
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|e| e.to_string()),
                None => assets.get(&url).await.map_err(|e| e.to_string()),
            }
        });
        async move { task.await.map_err(|e| e.to_string())? }
    }
}
