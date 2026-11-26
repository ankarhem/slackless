//! The macOS login Keychain through Apple's `/usr/bin/security` tool: one generic-password item
//! per account under the `slackless` service, each holding an opaque text blob the store that
//! owns the account encodes itself ([`super::SessionStore`] a session's secrets,
//! [`super::SecretStore`] one named secret).
//!
//! The tool is used rather than the Security framework in-process: items it creates trust it
//! (access list and partition list), and it is Apple-signed and never changes, so rebuilt
//! slackless binaries never trigger a Keychain password prompt. A secret never appears on a
//! command line: it reaches the tool as hex on stdin.

use std::fmt;
#[cfg(target_os = "macos")]
use std::io::Write as _;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

use crate::error::{AuthError, Result};

/// Keychain generic-password service every slackless item is stored under.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "slackless";

/// The tool every Keychain access goes through (see the module docs).
#[cfg(target_os = "macos")]
const SECURITY: &str = "/usr/bin/security";

/// `security find-generic-password` / `delete-generic-password` exit status for a missing item.
#[cfg(any(test, target_os = "macos"))]
const ITEM_NOT_FOUND: i32 = 44;

/// Seam over the OS Keychain so the stores can be tested without touching the login Keychain.
pub(crate) trait KeychainBackend: fmt::Debug + Send + Sync {
    fn set(&self, account: &str, blob: &str) -> Result<()>;
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn delete(&self, account: &str) -> Result<()>;
}

/// The login Keychain through `/usr/bin/security` on macOS (a no-op elsewhere).
#[derive(Debug, Default)]
pub(crate) struct SecurityKeychain;

impl KeychainBackend for SecurityKeychain {
    fn set(&self, account: &str, blob: &str) -> Result<()> {
        keychain_set(account, blob)
    }

    fn get(&self, account: &str) -> Result<Option<String>> {
        keychain_get(account)
    }

    fn delete(&self, account: &str) -> Result<()> {
        keychain_delete(account)
    }
}

/// A Keychain account has to stay one bare token on a `security` command line, so it can never
/// be read as an option or split into several arguments.
pub(crate) fn validate_account(account: &str) -> Result<&str> {
    if !account.is_empty()
        && account.len() <= 64
        && !account.starts_with('-')
        && account
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        Ok(account)
    } else {
        Err(AuthError::Keychain(format!(
            "invalid keychain account name {account:?}"
        )))
    }
}

#[cfg(target_os = "macos")]
fn keychain_set(account: &str, blob: &str) -> Result<()> {
    let account = validate_account(account)?;
    let hex: String = blob.bytes().map(|byte| format!("{byte:02x}")).collect();
    // `-i` reads the command from stdin, keeping the secret off the process list; `-X` is hex.
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
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if !output.status.success() || !detail.is_empty() {
        let msg = if detail.is_empty() {
            format!(
                "add-generic-password failed (exit {:?})",
                output.status.code()
            )
        } else {
            detail.to_owned()
        };
        return Err(AuthError::Keychain(msg));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn keychain_get(account: &str) -> Result<Option<String>> {
    let account = validate_account(account)?;
    let output = Command::new(SECURITY)
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a"])
        .arg(account)
        .arg("-w")
        .output()?;
    match output.status.code() {
        Some(0) => Ok(Some(
            std::str::from_utf8(&output.stdout)
                .map_err(|_| AuthError::Keychain("keychain item is not UTF-8".to_owned()))?
                .trim_ascii_end()
                .to_owned(),
        )),
        // Not found is not an error: the caller falls back to whatever is on disk.
        Some(ITEM_NOT_FOUND) => Ok(None),
        _ => Err(AuthError::Keychain(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )),
    }
}

#[cfg(target_os = "macos")]
fn keychain_delete(account: &str) -> Result<()> {
    let account = validate_account(account)?;
    let output = Command::new(SECURITY)
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a"])
        .arg(account)
        .output()?;
    classify_delete_status(output.status.code(), &output.stderr)
}

/// Only exit 0 (deleted) and exit 44 ([`ITEM_NOT_FOUND`], already absent) succeed; locked
/// Keychain or permission errors surface as [`AuthError::Keychain`].
#[cfg(any(test, target_os = "macos"))]
fn classify_delete_status(code: Option<i32>, stderr: &[u8]) -> Result<()> {
    match code {
        Some(0) | Some(ITEM_NOT_FOUND) => Ok(()),
        other => {
            let detail = String::from_utf8_lossy(stderr).trim().to_owned();
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

// Non-macOS builds never enable Keychain mirroring, but the methods must still resolve.
#[cfg(not(target_os = "macos"))]
fn keychain_set(_account: &str, _blob: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn keychain_get(_account: &str) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn keychain_delete(_account: &str) -> Result<()> {
    Ok(())
}

/// In-memory Keychain shared by the stores' tests: records every operation as
/// `"<set|get|del>:<account>"` and can be told to fail deletes.
#[cfg(test)]
pub(crate) struct MockKeychain {
    entries: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    pub ops: std::sync::Mutex<Vec<String>>,
    pub fail_delete: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl Default for MockKeychain {
    fn default() -> Self {
        Self {
            entries: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            ops: std::sync::Mutex::new(Vec::new()),
            fail_delete: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[cfg(test)]
impl fmt::Debug for MockKeychain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockKeychain").finish_non_exhaustive()
    }
}

#[cfg(test)]
impl MockKeychain {
    pub fn ops(&self) -> Vec<String> {
        self.ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn clear_ops(&self) {
        self.ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn remove_entry(&self, account: &str) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(account);
    }

    pub fn contains(&self, account: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(account)
    }
}

#[cfg(test)]
impl KeychainBackend for MockKeychain {
    fn set(&self, account: &str, blob: &str) -> Result<()> {
        let account = validate_account(account)?.to_owned();
        self.ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("set:{account}"));
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(account, blob.to_owned());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>> {
        let account = validate_account(account)?.to_owned();
        self.ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("get:{account}"));
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&account)
            .cloned())
    }

    fn delete(&self, account: &str) -> Result<()> {
        let account = validate_account(account)?.to_owned();
        self.ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("del:{account}"));
        if self.fail_delete.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(AuthError::Keychain("mock delete failure".into()));
        }
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_stay_one_bare_token() {
        for ok in ["T0123ABC", "E999", "ai-summary-api-key", "T1_2.x"] {
            assert_eq!(validate_account(ok).expect("valid account"), ok);
        }
        for bad in [
            "",
            " ",
            "T1 T2",
            "T1\nadd",
            "T1\r",
            "T1\"quote",
            "-T1",
            "-w",
            "bad;rm -rf",
            &"a".repeat(65),
        ] {
            assert!(
                matches!(validate_account(bad), Err(AuthError::Keychain(_))),
                "expected rejection for {bad:?}"
            );
        }
    }

    #[test]
    fn delete_status_allows_zero_and_not_found_but_surfaces_errors() {
        assert!(classify_delete_status(Some(0), b"").is_ok());
        assert!(classify_delete_status(Some(ITEM_NOT_FOUND), b"item not found").is_ok());
        assert!(matches!(
            classify_delete_status(Some(36), b"User interaction is not allowed."),
            Err(AuthError::Keychain(msg)) if msg == "User interaction is not allowed."
        ));
        assert!(matches!(
            classify_delete_status(None, b""),
            Err(AuthError::Keychain(msg)) if msg.contains("None")
        ));
    }
}
