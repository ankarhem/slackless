//! Non-macOS fallback backend: compiles everywhere, touches no OS API.
//!
//! `Notifier` and `BadgeSetter` are true no-ops until their platform ports
//! begin. `SecretStore` keeps secrets in-process only (a `HashMap` behind a
//! mutex) so the trait contract is exercised without a system keychain;
//! persistence arrives with each OS backend. `DeepLinkHandler` shares the
//! pure parser in [`crate::deep_link`]; nothing can be registered with an OS.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::deep_link::{self, DeepLink};
use crate::{BadgeSetter, DeepLinkHandler, Notifier, PlatformError, Result, SecretStore};

/// No-op backend for platforms without a port yet.
pub struct FallbackPlatform {
    secrets: Mutex<HashMap<(String, String), Vec<u8>>>,
    pending_link: Mutex<Option<DeepLink>>,
}

impl FallbackPlatform {
    pub fn new() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
            pending_link: Mutex::new(None),
        }
    }
}

impl Default for FallbackPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifier for FallbackPlatform {
    /// No-op: surface notifications through the target front-end's own
    /// notification system once a port exists.
    fn notify(&self, _title: &str, _body: &str) {}
}

impl BadgeSetter for FallbackPlatform {
    /// No-op: no taskbar badge API is wired for this platform yet.
    fn set_badge(&self, _label: Option<&str>) {}
}

impl SecretStore for FallbackPlatform {
    fn store_secret(&self, service: &str, name: &str, secret: &[u8]) -> Result<()> {
        self.secrets
            .lock()
            .expect("secrets mutex poisoned")
            .insert((service.to_string(), name.to_string()), secret.to_vec());
        Ok(())
    }

    fn load_secret(&self, service: &str, name: &str) -> Result<Vec<u8>> {
        self.secrets
            .lock()
            .expect("secrets mutex poisoned")
            .get(&(service.to_string(), name.to_string()))
            .cloned()
            .ok_or_else(|| PlatformError::SecretNotFound {
                service: service.to_string(),
                name: name.to_string(),
            })
    }

    fn delete_secret(&self, service: &str, name: &str) -> Result<()> {
        self.secrets
            .lock()
            .expect("secrets mutex poisoned")
            .remove(&(service.to_string(), name.to_string()));
        Ok(())
    }
}

impl DeepLinkHandler for FallbackPlatform {
    fn register(&self) -> Result<Vec<String>> {
        // Nothing to register: OS scheme delivery is wired by each port.
        Ok(Vec::new())
    }

    fn handle_url(&self, url: &str) -> Result<DeepLink> {
        let link = deep_link::parse_deep_link(url)?;
        *self
            .pending_link
            .lock()
            .expect("pending link mutex poisoned") = Some(link.clone());
        Ok(link)
    }

    fn take_link(&self) -> Option<DeepLink> {
        self.pending_link
            .lock()
            .expect("pending link mutex poisoned")
            .take()
    }
}
