//! Public-asset downloads (profile photos, emoji, workspace icons) for front-ends, run on the
//! core's tokio runtime so a front-end on another executor (GPUI's) shares the core's HTTP stack.

use std::future::Future;

use tokio::runtime::Handle;

use crate::api::AssetClient;

/// Cheap to clone; hand one to whatever loads remote images.
#[derive(Clone)]
pub struct Fetcher {
    runtime: Handle,
    assets: AssetClient,
}

impl Fetcher {
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            assets: AssetClient::new(),
        }
    }

    /// The bytes at `url`. The download starts now on the core runtime; the returned future
    /// can be awaited from any executor.
    pub fn get(
        &self,
        url: String,
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send + 'static {
        let assets = self.assets.clone();
        let task = self
            .runtime
            .spawn(async move { assets.get(&url).await.map_err(|e| e.to_string()) });
        async move { task.await.map_err(|e| e.to_string())? }
    }
}
