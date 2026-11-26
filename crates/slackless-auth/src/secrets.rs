//! Named secret storage backed by the macOS login Keychain (`/usr/bin/security`) with a
//! private (`0600`) JSON file fallback for non-macOS platforms and deterministic tests.
//!
//! Used for BYO-key credentials such as the AI summary provider API key so secrets never live
//! in plaintext preference files or appear on process command lines.

use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::io::Write as _;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{AuthError, Result};

/// Account identifier used for the local/BYO-key AI summary provider API key.
pub const AI_SUMMARY_API_KEY_ACCOUNT: &str = "ai-summary-api-key";

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "slackless";
#[cfg(target_os = "macos")]
const SECURITY: &str = "/usr/bin/security";
#[cfg(any(test, target_os = "macos"))]
const ITEM_NOT_FOUND: i32 = 44;

/// Seam over the OS Keychain for deterministic unit tests.
pub(crate) trait SecretKeychainBackend: std::fmt::Debug + Send + Sync {
    fn set(&self, account: &str, value: &str) -> Result<()>;
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn delete(&self, account: &str) -> Result<()>;
}

#[derive(Debug)]
struct OsSecretKeychain;

impl SecretKeychainBackend for OsSecretKeychain {
    fn set(&self, account: &str, value: &str) -> Result<()> {
        keychain_secret_set(account, value)
    }

    fn get(&self, account: &str) -> Result<Option<String>> {
        keychain_secret_get(account)
    }

    fn delete(&self, account: &str) -> Result<()> {
        keychain_secret_delete(account)
    }
}

/// Persistent named secret store (macOS Keychain when enabled, else `0600` JSON file).
#[derive(Debug, Clone)]
pub struct SecretStore {
    path: PathBuf,
    use_keychain: bool,
    keychain: Arc<dyn SecretKeychainBackend>,
}

impl SecretStore {
    /// Default secret store (`<config dir>/slackless/secrets.json` fallback, macOS Keychain
    /// primary on macOS).
    pub fn new() -> Result<Self> {
        let dir = dirs::config_dir().ok_or(AuthError::NoConfigDir)?;
        Ok(Self {
            path: dir.join("slackless").join("secrets.json"),
            use_keychain: cfg!(target_os = "macos"),
            keychain: Arc::new(OsSecretKeychain),
        })
    }

    /// File-only store at `path` with Keychain disabled (for non-macOS and tests).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            use_keychain: false,
            keychain: Arc::new(OsSecretKeychain),
        }
    }

    #[cfg(test)]
    fn with_keychain(path: impl Into<PathBuf>, keychain: Arc<dyn SecretKeychainBackend>) -> Self {
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
            self.keychain.set(account, trimmed)
        } else {
            let mut map = self.read_file_map()?;
            map.insert(account.to_owned(), trimmed.to_owned());
            self.write_file_map(&map)
        }
    }

    /// Read the secret stored under `account`, if any.
    pub fn get(&self, account: &str) -> Result<Option<String>> {
        let account = validate_account(account)?;
        if self.use_keychain {
            if let Some(val) = self.keychain.get(account)? {
                return Ok(Some(val));
            }
            // Also check file fallback in case a key was stored before Keychain was enabled.
            let map = self.read_file_map()?;
            Ok(map.get(account).cloned())
        } else {
            let map = self.read_file_map()?;
            Ok(map.get(account).cloned())
        }
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
        let tmp = unique_tmp_path(&self.path);
        write_private_file(&tmp, &json)?;
        if let Err(err) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
        Ok(())
    }
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_tmp_path(target: &Path) -> PathBuf {
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("secrets");
    let pid = std::process::id();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!("{stem}.{pid}.{seq}.tmp"))
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        if let Err(err) = f.write_all(bytes).and_then(|()| f.sync_all()) {
            let _ = std::fs::remove_file(path);
            return Err(err);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// Validate a Keychain account identifier so it stays one bare token on a `security` command.
fn validate_account(account: &str) -> Result<&str> {
    if !account.is_empty()
        && account.len() <= 64
        && account
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        Ok(account)
    } else {
        Err(AuthError::Keychain(format!(
            "invalid secret account name {account:?}"
        )))
    }
}

#[cfg(target_os = "macos")]
#[derive(serde::Serialize, serde::Deserialize)]
struct SecretEnvelope {
    value: String,
}

#[cfg(target_os = "macos")]
fn keychain_secret_set(account: &str, value: &str) -> Result<()> {
    let account = validate_account(account)?;
    let blob = serde_json::to_vec(&SecretEnvelope {
        value: value.to_owned(),
    })?;
    let hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
    let command = format!("add-generic-password -U -s {KEYCHAIN_SERVICE} -a {account} -X {hex}\n");
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(command.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    let errors = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() || !errors.trim().is_empty() {
        return Err(AuthError::Keychain(errors.trim().to_owned()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn keychain_secret_get(account: &str) -> Result<Option<String>> {
    let account = validate_account(account)?;
    let output = Command::new(SECURITY)
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a"])
        .arg(account)
        .arg("-w")
        .output()?;
    match output.status.code() {
        Some(0) => {
            let env: SecretEnvelope = serde_json::from_slice(output.stdout.trim_ascii_end())?;
            Ok(Some(env.value))
        }
        Some(ITEM_NOT_FOUND) => Ok(None),
        _ => Err(AuthError::Keychain(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )),
    }
}

#[cfg(target_os = "macos")]
fn keychain_secret_delete(account: &str) -> Result<()> {
    let account = validate_account(account)?;
    let output = Command::new(SECURITY)
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a"])
        .arg(account)
        .output()?;
    match output.status.code() {
        Some(0) | Some(ITEM_NOT_FOUND) => Ok(()),
        other => {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if detail.is_empty() {
                Err(AuthError::Keychain(format!(
                    "delete-generic-password failed (exit {other:?})"
                )))
            } else {
                Err(AuthError::Keychain(detail))
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_secret_set(_account: &str, _value: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn keychain_secret_get(_account: &str) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn keychain_secret_delete(_account: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    struct MockKeychain(Mutex<BTreeMap<String, String>>);

    impl SecretKeychainBackend for MockKeychain {
        fn set(&self, account: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .expect("mock keychain lock")
                .insert(account.to_owned(), value.to_owned());
            Ok(())
        }

        fn get(&self, account: &str) -> Result<Option<String>> {
            Ok(self
                .0
                .lock()
                .expect("mock keychain lock")
                .get(account)
                .cloned())
        }

        fn delete(&self, account: &str) -> Result<()> {
            self.0.lock().expect("mock keychain lock").remove(account);
            Ok(())
        }
    }

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
        let store = SecretStore::with_keychain(&path, kc);

        store
            .set(AI_SUMMARY_API_KEY_ACCOUNT, "sk-live-secret")
            .expect("set");
        assert!(!path.exists(), "secret should not be written to disk");
        assert_eq!(
            store.get(AI_SUMMARY_API_KEY_ACCOUNT).expect("get"),
            Some("sk-live-secret".to_owned())
        );

        store.delete(AI_SUMMARY_API_KEY_ACCOUNT).expect("delete");
        assert_eq!(store.get(AI_SUMMARY_API_KEY_ACCOUNT).expect("get"), None);
    }

    #[test]
    fn rejects_unsafe_account_names() {
        assert!(validate_account("").is_err());
        assert!(validate_account("bad account").is_err());
        assert!(validate_account("bad;rm -rf").is_err());
        assert!(validate_account("ai-summary-api-key").is_ok());
    }
}
