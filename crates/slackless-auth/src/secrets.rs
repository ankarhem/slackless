//! Named secret storage backed by the macOS login Keychain ([`crate::keychain`]) with a private
//! (`0600`) JSON file fallback for non-macOS platforms and deterministic tests.
//!
//! Used for BYO-key credentials such as the AI summary provider API key so secrets never live
//! in plaintext preference files or appear on process command lines.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{AuthError, Result};
use crate::keychain::{KeychainBackend, SecurityKeychain, validate_account};
use crate::private_file::replace_private_file;

/// Account identifier used for the local/BYO-key AI summary provider API key.
pub const AI_SUMMARY_API_KEY_ACCOUNT: &str = "ai-summary-api-key";

/// Persistent named secret store (macOS Keychain when enabled, else `0600` JSON file).
#[derive(Debug, Clone)]
pub struct SecretStore {
    path: PathBuf,
    use_keychain: bool,
    keychain: Arc<dyn KeychainBackend>,
}

impl SecretStore {
    /// Default secret store (`<config dir>/slackless/secrets.json` fallback, macOS Keychain
    /// primary on macOS).
    pub fn new() -> Result<Self> {
        let dir = dirs::config_dir().ok_or(AuthError::NoConfigDir)?;
        Ok(Self {
            path: dir.join("slackless").join("secrets.json"),
            use_keychain: cfg!(target_os = "macos"),
            keychain: Arc::new(SecurityKeychain),
        })
    }

    /// File-only store at `path` with Keychain disabled (for non-macOS and tests).
    #[cfg(test)]
    fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            use_keychain: false,
            keychain: Arc::new(SecurityKeychain),
        }
    }

    #[cfg(test)]
    fn with_keychain(path: impl Into<PathBuf>, keychain: Arc<dyn KeychainBackend>) -> Self {
        Self {
            path: path.into(),
            use_keychain: true,
            keychain,
        }
    }

    /// Store or update a secret under `account`. Empty/whitespace-only values delete the secret.
    pub fn set(&self, account: &str, value: &str) -> Result<()> {
        let account = validate_account(account)?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return self.delete(account);
        }
        if self.use_keychain {
            self.keychain.set(
                account,
                &encode_envelope(trimmed).expect("one string always serializes"),
            )
        } else {
            let _lock = crate::private_file::lock_private_file(&self.path)?;
            let mut map = self.read_file_map()?;
            map.insert(account.to_owned(), trimmed.to_owned());
            self.write_file_map(&map)
        }
    }

    /// Read the secret stored under `account`, if any.
    pub fn get(&self, account: &str) -> Result<Option<String>> {
        let account = validate_account(account)?;
        if self.use_keychain
            && let Some(blob) = self.keychain.get(account)?
        {
            return Ok(Some(decode_envelope(&blob)?));
        }
        // Also check the file: a key stored before Keychain was enabled, or the file-only mode.
        Ok(self.read_file_map()?.remove(account))
    }

    /// Returns `true` when a non-empty secret is stored under `account`.
    pub fn has(&self, account: &str) -> Result<bool> {
        Ok(self.get(account)?.is_some_and(|v| !v.trim().is_empty()))
    }

    /// Delete the secret stored under `account` (no-op if absent).
    pub fn delete(&self, account: &str) -> Result<()> {
        let account = validate_account(account)?;
        if self.use_keychain {
            self.keychain.delete(account)?;
        }
        if self.path.exists() {
            let _lock = crate::private_file::lock_private_file(&self.path)?;
            let mut map = self.read_file_map()?;
            if map.remove(account).is_some() {
                self.write_file_map(&map)?;
            }
        }
        Ok(())
    }

    fn read_file_map(&self) -> Result<BTreeMap<String, String>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn write_file_map(&self, map: &BTreeMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(map)?;
        replace_private_file(&self.path, &json)?;
        Ok(())
    }
}

/// A named secret as its Keychain blob stores it: `{"value":...}` (the shape earlier builds
/// wrote, kept so existing items keep reading).
#[derive(serde::Serialize, serde::Deserialize)]
struct SecretEnvelope {
    value: String,
}

fn encode_envelope(value: &str) -> Result<String> {
    Ok(serde_json::to_string(&SecretEnvelope {
        value: value.to_owned(),
    })?)
}

fn decode_envelope(blob: &str) -> Result<String> {
    Ok(serde_json::from_str::<SecretEnvelope>(blob)?.value)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::keychain::MockKeychain;

    #[test]
    fn file_store_round_trip_and_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SecretStore::with_path(dir.path().join("secrets.json"));
        assert!(!store.has(AI_SUMMARY_API_KEY_ACCOUNT).expect("has"));

        store
            .set(AI_SUMMARY_API_KEY_ACCOUNT, "  sk-test-12345  ")
            .expect("set");
        assert!(store.has(AI_SUMMARY_API_KEY_ACCOUNT).expect("has"));
        assert_eq!(
            store.get(AI_SUMMARY_API_KEY_ACCOUNT).expect("get"),
            Some("sk-test-12345".to_owned())
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join("secrets.json"))
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Empty value deletes the entry.
        store.set(AI_SUMMARY_API_KEY_ACCOUNT, "   ").expect("clear");
        assert_eq!(store.get(AI_SUMMARY_API_KEY_ACCOUNT).expect("get"), None);
    }

    #[test]
    fn keychain_mode_keeps_secrets_off_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.json");
        let kc = Arc::new(MockKeychain::default());
        let store = SecretStore::with_keychain(&path, kc.clone());

        store
            .set(AI_SUMMARY_API_KEY_ACCOUNT, "sk-live-secret")
            .expect("set");
        assert!(!path.exists(), "secret should not be written to disk");
        assert_eq!(
            store.get(AI_SUMMARY_API_KEY_ACCOUNT).expect("get"),
            Some("sk-live-secret".to_owned())
        );
        assert_eq!(
            kc.ops(),
            vec![
                "set:ai-summary-api-key".to_owned(),
                "get:ai-summary-api-key".to_owned()
            ]
        );

        store.delete(AI_SUMMARY_API_KEY_ACCOUNT).expect("delete");
        assert_eq!(store.get(AI_SUMMARY_API_KEY_ACCOUNT).expect("get"), None);
    }
}
