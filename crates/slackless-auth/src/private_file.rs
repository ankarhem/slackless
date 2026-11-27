//! Owner-only atomic file replacement for plaintext secrets written to disk when Keychain
//! mirroring is off: the new bytes go to a uniquely named sibling temp file (mode `0600` on
//! Unix) that is then renamed over the target, so a crash never leaves a half-written file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process counter for unique sibling temp filenames during atomic saves.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Acquires an exclusive advisory OS lock on a sibling `<target>.lock` file so concurrent
/// GUI/CLI read-modify-write sequences cannot clobber each other's updates. Drop the returned
/// file handle to release the lock.
pub(crate) fn lock_private_file(target: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = lock_file_path(target);
    let file = {
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        opts.open(&lock_path)?
    };
    file.lock()?;
    Ok(file)
}

fn lock_file_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| std::ffi::OsString::from("secrets"));
    name.push(".lock");
    target.with_file_name(name)
}

/// Replaces the file at `target` with `bytes`: writes a uniquely named sibling temp file
/// (owner-only `0600` on Unix), syncs it, then renames it over `target`. On failure the temp
/// file is removed and `target` is left as it was.
pub(crate) fn replace_private_file(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = unique_tmp_path(target);
    write_private_file(&tmp, bytes)?;
    if let Err(err) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// A unique sibling of `target` (e.g. `sessions.<pid>.<seq>.tmp`) so concurrent saves never
/// race on a shared `.tmp` path.
fn unique_tmp_path(target: &Path) -> PathBuf {
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("secrets");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!("{stem}.{}.{seq}.tmp", std::process::id()))
}

/// Writes `bytes` to a new file at `path` with owner-only (`0600`) permissions on Unix so
/// plaintext tokens are never world-readable; removes the partial file if write or sync fails.
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
