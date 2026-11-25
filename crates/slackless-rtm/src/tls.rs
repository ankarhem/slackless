//! Shared rustls WebSocket TLS connector.

use std::sync::Arc;

use tokio_tungstenite::Connector;

/// Builds a rustls WebSocket [`Connector`] backed by `webpki-roots` and the `ring` crypto provider.
pub fn tls_connector() -> Result<Connector, rustls::Error> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    // Explicit provider: no reliance on a process-wide default crypto provider.
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}
