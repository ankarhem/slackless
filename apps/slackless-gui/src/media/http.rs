//! GPUI [`HttpClient`] backed by [`slackless_core::Fetcher`] so all remote media requests share
//! the core's cookie-authenticated reqwest client and tokio runtime.

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
