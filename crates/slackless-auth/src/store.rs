//! Multi-workspace session persistence: a JSON file under the OS config dir, with the secret
//! fields mirrored into the macOS Keychain when it is available.
//!
//! On macOS the on-disk JSON holds only the non-secret metadata (`workspace_url`, `team_id`,
//! `user_id`); the `xoxc` token and `d` cookie live in the login Keychain, keyed by `team_id`.
//! On every other platform (and whenever the Keychain is disabled) the full record, secrets
//! included, is written to disk.
//!
//! Keychain reads and writes go through Apple's `/usr/bin/security` tool rather than the
//! Security framework in-process. Items it creates trust it (access list and partition list),
//! and it is Apple-signed and never changes, so rebuilt slackless binaries never trigger a
//! Keychain password prompt. The secret never appears on a command line.

#[cfg(target_os = "macos")]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::Arc;

use slackless_types::TeamId;

use crate::error::{AuthError, Result};
use crate::session::Session;

/// Keychain generic-password service these secrets are stored under.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "slackless";

/// The tool every Keychain access goes through (see the module docs).
#[cfg(target_os = "macos")]
const SECURITY: &str = "/usr/bin/security";

/// `security find-generic-password`'s exit status for a missing item.
#[cfg(target_os = "macos")]
const ITEM_NOT_FOUND: i32 = 44;

/// Seam over the OS Keychain so [`SessionStore`] can be tested without touching the login Keychain.
pub(crate) trait KeychainBackend: std::fmt::Debug + Send + Sync {
    fn set(&self, team_id: &TeamId, xoxc: &str, d: &str) -> Result<()>;
    fn get(&self, team_id: &TeamId) -> Result<Option<(String, String)>>;
    fn delete(&self, team_id: &TeamId) -> Result<()>;
}

/// Default Keychain backend backed by Apple's `/usr/bin/security` tool on macOS (no-op elsewhere).
#[derive(Debug)]
struct OsKeychain;

impl KeychainBackend for OsKeychain {
    fn set(&self, team_id: &TeamId, xoxc: &str, d: &str) -> Result<()> {
        keychain_set(team_id, xoxc, d)
    }

    fn get(&self, team_id: &TeamId) -> Result<Option<(String, String)>> {
        keychain_get(team_id)
    }

    fn delete(&self, team_id: &TeamId) -> Result<()> {
        keychain_delete(team_id)
    }
}

/// Durable store for one-or-more [`Session`]s.
#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
    use_keychain: bool,
    keychain: Arc<dyn KeychainBackend>,
}

impl SessionStore {
    /// The store at the default location: `<config dir>/slackless/sessions.json`
    /// (`~/Library/Application Support/slackless/...` on macOS). Secrets are mirrored to the
    /// Keychain on macOS.
    pub fn new() -> Result<Self> {
        let dir = dirs::config_dir().ok_or(AuthError::NoConfigDir)?;
        Ok(Self {
            path: dir.join("slackless").join("sessions.json"),
            use_keychain: cfg!(target_os = "macos"),
            keychain: Arc::new(OsKeychain),
        })
    }

    /// A store backed by `path`, with no Keychain mirroring (full records go to disk). Intended
    /// for tests and headless/non-macOS setups.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            use_keychain: false,
            keychain: Arc::new(OsKeychain),
        }
    }

    /// A store backed by `path` and an injected Keychain implementation (for tests).
    #[cfg(test)]
    fn with_keychain(path: impl Into<PathBuf>, keychain: Arc<dyn KeychainBackend>) -> Self {
        Self {
            path: path.into(),
            use_keychain: true,
            keychain,
        }
    }

    /// Enables or disables mirroring secrets into the macOS Keychain. No effect off macOS.
    pub fn set_use_keychain(&mut self, enabled: bool) {
        self.use_keychain = enabled && cfg!(target_os = "macos");
    }

    /// The backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads every stored session, newest write wins. Missing file yields an empty list.
    pub fn load(&self) -> Result<Vec<Session>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut sessions: Vec<Session> = serde_json::from_slice(&bytes)?;
        if self.use_keychain {
            for s in &mut sessions {
                if !s.has_secrets() {
                    if let Some((xoxc, d)) = self.keychain.get(&s.team_id)? {
                        s.xoxc_token = xoxc;
                        s.d_cookie = d;
                    }
                }
            }
        }
        Ok(sessions)
    }

    /// Replaces the stored set with `sessions`, writing atomically. On macOS the secrets go to
    /// the Keychain and are blanked on disk.
    pub fn save(&self, sessions: &[Session]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let on_disk: Vec<Session> = if self.use_keychain {
            for s in sessions {
                self.keychain.set(&s.team_id, &s.xoxc_token, &s.d_cookie)?;
            }
            sessions
                .iter()
                .map(|s| Session {
                    xoxc_token: String::new(),
                    d_cookie: String::new(),
                    ..s.clone()
                })
                .collect()
        } else {
            sessions.to_vec()
        };

        let json = serde_json::to_vec_pretty(&on_disk)?;
        let tmp = self.path.with_extension("json.tmp");
        write_private_file(&tmp, &json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Write `bytes` to `path` with owner-only (`0600`) permissions on Unix so plaintext session tokens
/// are never world-readable when Keychain mirroring is disabled.
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

impl SessionStore {
    /// Inserts or replaces the session for its `team_id`.
    pub fn upsert(&self, session: Session) -> Result<()> {
        let mut sessions = self.load()?;
        match sessions.iter_mut().find(|s| s.team_id == session.team_id) {
            Some(existing) => *existing = session,
            None => sessions.push(session),
        }
        self.save(&sessions)
    }

    /// The stored session for `team_id`, if any.
    pub fn get(&self, team_id: &TeamId) -> Result<Option<Session>> {
        Ok(self.load()?.into_iter().find(|s| &s.team_id == team_id))
    }

    /// Removes the session for `team_id`. Returns whether one was present.
    pub fn remove(&self, team_id: &TeamId) -> Result<bool> {
        let mut sessions = self.load()?;
        let before = sessions.len();
        sessions.retain(|s| &s.team_id != team_id);
        let removed = sessions.len() != before;
        if removed {
            if self.use_keychain {
                self.keychain.delete(team_id)?;
            }
            self.save(&sessions)?;
        }
        Ok(removed)
    }
}

// --------------------------------------------------------------------------------------------
// Keychain-backed secret mirror (macOS)
// --------------------------------------------------------------------------------------------

/// Secret half of a session, as stored in one Keychain generic-password item.
#[cfg(target_os = "macos")]
#[derive(serde::Serialize, serde::Deserialize)]
struct Secret {
    xoxc: String,
    d: String,
}

#[cfg(target_os = "macos")]
fn keychain_set(team_id: &TeamId, xoxc: &str, d: &str) -> Result<()> {
    let blob = serde_json::to_vec(&Secret {
        xoxc: xoxc.to_owned(),
        d: d.to_owned(),
    })?;
    let hex: String = blob.iter().map(|byte| format!("{byte:02x}")).collect();
    // `-i` reads the command from stdin, keeping the secret off the process list; `-X` is hex.
    let command = format!(
        "add-generic-password -U -s {KEYCHAIN_SERVICE} -a {} -X {hex}\n",
        keychain_account(team_id)?
    );
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
fn keychain_get(team_id: &TeamId) -> Result<Option<(String, String)>> {
    let output = Command::new(SECURITY)
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a"])
        .arg(keychain_account(team_id)?)
        .arg("-w")
        .output()?;
    match output.status.code() {
        Some(0) => {
            let secret: Secret = serde_json::from_slice(output.stdout.trim_ascii_end())?;
            Ok(Some((secret.xoxc, secret.d)))
        }
        // Not found is not an error: the caller falls back to whatever is on disk.
        Some(ITEM_NOT_FOUND) => Ok(None),
        _ => Err(AuthError::Keychain(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )),
    }
}

#[cfg(target_os = "macos")]
fn keychain_delete(team_id: &TeamId) -> Result<()> {
    // A missing item is already deleted.
    Command::new(SECURITY)
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a"])
        .arg(keychain_account(team_id)?)
        .output()?;
    Ok(())
}

/// A team's Keychain account: its id, which must stay one bare word on a `security` command.
#[cfg(any(test, target_os = "macos"))]
fn keychain_account(team_id: &TeamId) -> Result<&str> {
    let id = team_id.as_str();
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id)
    } else {
        Err(AuthError::Keychain(format!("unexpected team id {id:?}")))
    }
}

// Non-macOS builds never set `use_keychain`, but the methods must still resolve.
#[cfg(not(target_os = "macos"))]
fn keychain_set(_team_id: &TeamId, _xoxc: &str, _d: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn keychain_get(_team_id: &TeamId) -> Result<Option<(String, String)>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn keychain_delete(_team_id: &TeamId) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SessionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::with_path(dir.path().join("sessions.json"));
        (dir, store)
    }

    #[test]
    fn empty_when_missing() {
        let (_d, store) = store();
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn upsert_get_and_multi_workspace() {
        let (_d, store) = store();
        store
            .upsert(Session::new(
                "https://a.slack.com/",
                "T1",
                "xoxc-a",
                "xoxd-a",
                "U1",
            ))
            .unwrap();
        store
            .upsert(Session::new(
                "https://b.slack.com/",
                "T2",
                "xoxc-b",
                "xoxd-b",
                "U2",
            ))
            .unwrap();
        assert_eq!(store.load().unwrap().len(), 2);

        // Upsert on the same team replaces rather than duplicates.
        store
            .upsert(Session::new(
                "https://a.slack.com/",
                "T1",
                "xoxc-a2",
                "xoxd-a2",
                "U1",
            ))
            .unwrap();
        let got = store.get(&TeamId::from("T1")).unwrap().unwrap();
        assert_eq!(got.xoxc_token, "xoxc-a2");
        assert_eq!(store.load().unwrap().len(), 2);
    }

    #[test]
    fn remove_reports_presence() {
        let (_d, store) = store();
        store
            .upsert(Session::new("https://a.slack.com/", "T1", "x", "y", "U1"))
            .unwrap();
        assert!(store.remove(&TeamId::from("T1")).unwrap());
        assert!(!store.remove(&TeamId::from("T1")).unwrap());
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn disk_only_store_keeps_secrets_on_disk() {
        let (_d, store) = store();
        store
            .upsert(Session::new(
                "https://a.slack.com/",
                "T1",
                "xoxc-a",
                "xoxd-a",
                "U1",
            ))
            .unwrap();
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(
            raw.contains("xoxc-a"),
            "disk store should retain secrets: {raw}"
        );
    }

    #[derive(Debug, Default)]
    struct MockKeychain {
        entries: std::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
        ops: std::sync::Mutex<Vec<String>>,
        fail_delete: std::sync::atomic::AtomicBool,
    }

    impl KeychainBackend for MockKeychain {
        fn set(&self, team_id: &TeamId, xoxc: &str, d: &str) -> Result<()> {
            let acct = keychain_account(team_id)?.to_owned();
            self.ops.lock().unwrap().push(format!("set:{acct}"));
            self.entries
                .lock()
                .unwrap()
                .insert(acct, (xoxc.to_owned(), d.to_owned()));
            Ok(())
        }

        fn get(&self, team_id: &TeamId) -> Result<Option<(String, String)>> {
            let acct = keychain_account(team_id)?.to_owned();
            self.ops.lock().unwrap().push(format!("get:{acct}"));
            Ok(self.entries.lock().unwrap().get(&acct).cloned())
        }

        fn delete(&self, team_id: &TeamId) -> Result<()> {
            let acct = keychain_account(team_id)?.to_owned();
            self.ops.lock().unwrap().push(format!("del:{acct}"));
            if self.fail_delete.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(AuthError::Keychain("mock delete failure".into()));
            }
            self.entries.lock().unwrap().remove(&acct);
            Ok(())
        }
    }

    #[test]
    fn keychain_save_blanks_disk_load_hydrates_tolerates_not_found_and_remove_deletes_first() {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockKeychain::default());
        let store = SessionStore::with_keychain(dir.path().join("sessions.json"), mock.clone());

        let s1 = Session::new(
            "https://a.slack.com/",
            "T1",
            "xoxc-secret1",
            "xoxd-cookie1",
            "U1",
        );
        let s2 = Session::new(
            "https://b.slack.com/",
            "T2",
            "xoxc-secret2",
            "xoxd-cookie2",
            "U2",
        );
        store.save(&[s1.clone(), s2.clone()]).unwrap();

        // Secrets must be blanked in the JSON file on disk.
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(!raw.contains("xoxc-secret1"), "token leaked to disk: {raw}");
        assert!(
            !raw.contains("xoxd-cookie1"),
            "cookie leaked to disk: {raw}"
        );
        assert!(raw.contains("\"T1\""));

        // `load()` hydrates blanked sessions from the keychain.
        let loaded = store.load().unwrap();
        assert_eq!(loaded, vec![s1.clone(), s2.clone()]);

        // Simulate ITEM_NOT_FOUND (`Ok(None)`) for T1: load succeeds without error, leaving T1 blank.
        mock.entries.lock().unwrap().remove("T1");
        let loaded_missing = store.load().unwrap();
        assert!(!loaded_missing[0].has_secrets());
        assert_eq!(loaded_missing[1], s2);

        // If keychain delete fails during remove(), disk file is untouched (delete happens before save).
        mock.fail_delete
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(matches!(
            store.remove(&TeamId::from("T2")),
            Err(AuthError::Keychain(_))
        ));
        assert!(
            std::fs::read_to_string(store.path())
                .unwrap()
                .contains("\"T2\"")
        );

        // Re-enable delete: remove() deletes keychain entry before saving remaining sessions.
        mock.fail_delete
            .store(false, std::sync::atomic::Ordering::Relaxed);
        mock.ops.lock().unwrap().clear();
        assert!(store.remove(&TeamId::from("T2")).unwrap());
        let ops = mock.ops.lock().unwrap().clone();
        let del_pos = ops
            .iter()
            .position(|o| o == "del:T2")
            .expect("del:T2 recorded");
        let set_pos = ops
            .iter()
            .position(|o| o == "set:T1")
            .expect("set:T1 recorded");
        assert!(del_pos < set_pos, "delete must precede save: {ops:?}");
        assert!(!mock.entries.lock().unwrap().contains_key("T2"));
        assert!(
            !std::fs::read_to_string(store.path())
                .unwrap()
                .contains("\"T2\"")
        );
    }

    #[test]
    fn creates_nested_dirs_sets_0600_perms_and_errors_on_corrupt_json() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deep/nested/dir/sessions.json");
        let store = SessionStore::with_path(&nested);
        store
            .save(&[Session::new("https://a.slack.com/", "T1", "x", "y", "U1")])
            .unwrap();
        assert!(nested.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "session file must be owner-only 0600");
        }

        std::fs::write(&nested, "{corrupt json").unwrap();
        assert!(matches!(store.load(), Err(AuthError::Json(_))));
    }

    #[test]
    fn keychain_account_validates_strictly_alphanumeric_ids() {
        assert_eq!(
            keychain_account(&TeamId::from("T0123ABC")).unwrap(),
            "T0123ABC"
        );
        assert_eq!(keychain_account(&TeamId::from("E999")).unwrap(), "E999");

        for bad in [
            "",
            " ",
            "T1 T2",
            "T1\nadd",
            "T1\r",
            "T1\"quote",
            "-T1",
            "T1_2",
        ] {
            assert!(
                matches!(
                    keychain_account(&TeamId::from(bad)),
                    Err(AuthError::Keychain(_))
                ),
                "expected rejection for {bad:?}"
            );
        }
    }
}
