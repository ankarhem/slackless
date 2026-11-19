//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. This is that client: requests run on the core's reqwest client and tokio runtime,
//! so the app keeps a single HTTP stack.

use std::future::Future;
use std::pin::Pin;

use gpui_kit::gpui::http_client::http::HeaderValue;
use gpui_kit::gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url};
use slackless_core::Fetcher;

pub struct CoreHttp(pub Fetcher);

impl HttpClient for CoreHttp {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Response<AsyncBody>>> + Send>> {
        let bytes = self.0.get(request.uri().to_string());
        Box::pin(async move {
            let bytes = bytes.await.map_err(anyhow::Error::msg)?;
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(bytes))?)
        })
    }
}
