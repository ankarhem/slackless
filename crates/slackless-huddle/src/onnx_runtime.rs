//! On-demand loading of the ONNX Runtime shared library.
//!
//! `ort` is built with `load-dynamic`, so the runtime (~28 MB) is not linked into the binaries and
//! is only mapped when semantic search, smart-rule scoring, or Parakeet transcription first runs.
//! Every `ort` entry point must call [`ensure_loaded`] first: left to itself `ort` panics on first
//! use when it cannot find the library.

use std::env::consts::{DLL_PREFIX, DLL_SUFFIX};
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// Environment variable naming the ONNX Runtime library to load instead of the bundled one.
pub const ORT_DYLIB_ENV: &str = "ORT_DYLIB_PATH";

/// Where the library may live, in lookup order. An explicit override is the only candidate, so a
/// wrong `ORT_DYLIB_PATH` is reported instead of being silently replaced by another runtime.
fn candidates(env_override: Option<PathBuf>, exe: Option<&Path>, data_root: &Path) -> Vec<PathBuf> {
    if let Some(path) = env_override {
        return vec![path];
    }
    let file_name = format!("{DLL_PREFIX}onnxruntime{DLL_SUFFIX}");
    let mut found = Vec::with_capacity(2);
    // `Slackless.app/Contents/MacOS/<exe>` -> `Contents/Frameworks/`. Canonicalized so the
    // `slackless` CLI symlink into the bundle resolves to the bundle.
    if let Some(exe) = exe.and_then(|exe| exe.canonicalize().ok())
        && let Some(contents) = exe.parent().and_then(Path::parent)
    {
        found.push(contents.join("Frameworks").join(&file_name));
    }
    found.push(data_root.join("onnxruntime").join(file_name));
    found
}

fn first_existing(candidates: &[PathBuf]) -> Result<PathBuf, String> {
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| {
            let searched: Vec<String> = candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            format!(
                "ONNX Runtime library not found (looked for {}); run scripts/fetch-onnxruntime.sh or set {ORT_DYLIB_ENV}",
                searched.join(", ")
            )
        })
}

/// Path of the ONNX Runtime library that [`ensure_loaded`] loads.
///
/// Lookup order: `ORT_DYLIB_PATH`; `Contents/Frameworks/` of the app bundle holding the running
/// executable; `<data dir>/onnxruntime/`, which `scripts/fetch-onnxruntime.sh` fills for unbundled
/// binaries (`cargo run`, a standalone `slackless` CLI).
///
/// # Errors
/// Returns a user-facing message naming the searched paths when none of them is a file.
pub fn locate() -> Result<PathBuf, String> {
    let env_override = std::env::var_os(ORT_DYLIB_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let exe = std::env::current_exe().ok();
    first_existing(&candidates(
        env_override,
        exe.as_deref(),
        &crate::transcribe::data_root_dir(),
    ))
}

/// Check that `path` is an ONNX Runtime new enough for `ort`, before `ort` sees it.
///
/// `ort` 2.0.0-rc.12 hangs forever instead of returning when its own load fails: building its
/// error value needs the runtime API, which re-enters the load that is still running. So every
/// reason it could reject the library is ruled out here first, with plain error messages.
fn probe(path: &Path) -> Result<(), String> {
    let unusable = |why: &dyn std::fmt::Display| {
        format!("cannot use ONNX Runtime at {}: {why}", path.display())
    };
    // SAFETY: loading a library runs its initializers. This is the file the bundle, the fetch
    // script, or the user designated as ONNX Runtime, and the one `ort` loads right after.
    let library = unsafe { libloading::Library::new(path) }.map_err(|err| unusable(&err))?;
    // SAFETY: `OrtGetApiBase` is ONNX Runtime's C entry point and has this signature.
    let base = unsafe {
        library
            .get::<unsafe extern "C" fn() -> *const ort::sys::OrtApiBase>(b"OrtGetApiBase")
            .map_err(|err| unusable(&err))?()
    };
    // SAFETY: a non-null `OrtApiBase` is a static table inside `library`, which is still loaded;
    // `GetVersionString` returns a static NUL-terminated string, copied before `library` drops.
    let version = unsafe { base.as_ref() }
        .map(|base| unsafe { CStr::from_ptr((base.GetVersionString)()) })
        .ok_or_else(|| unusable(&"it returned no API"))?
        .to_string_lossy()
        .into_owned();
    let minor = version
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u32>().ok())
        .unwrap_or(0);
    if minor < ort::MINOR_VERSION {
        return Err(unusable(&format_args!(
            "it is version {version}, but 1.{} or newer is required",
            ort::MINOR_VERSION
        )));
    }
    Ok(())
}

/// Load ONNX Runtime into the process once. Cheap after the first success; a failure is not
/// remembered, so installing the runtime fixes the next attempt without a restart.
///
/// # Errors
/// Returns a user-facing message when the library is missing ([`locate`]) or cannot be used
/// (unreadable, wrong architecture, not ONNX Runtime, or older than the API `ort` was built for).
pub fn ensure_loaded() -> Result<(), String> {
    static LOADED: Mutex<bool> = Mutex::new(false);
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    if *loaded {
        return Ok(());
    }
    let path = locate()?;
    probe(&path)?;
    ort::init_from(&path)
        .map_err(|err| format!("failed to load ONNX Runtime from {}: {err}", path.display()))?
        .commit();
    tracing::info!(path = %path.display(), "loaded ONNX Runtime");
    *loaded = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn runtime_lookup_prefers_override_then_bundle_then_data_dir_and_missing_is_an_error() {
        let root = std::env::temp_dir().join(format!("slackless-ort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let macos = root.join("Slackless.app/Contents/MacOS");
        let data = root.join("data");
        std::fs::create_dir_all(&macos).expect("create bundle dir");
        std::fs::write(macos.join("slackless"), b"").expect("create bundled CLI");
        let link = root.join("slackless");
        std::os::unix::fs::symlink(macos.join("slackless"), &link).expect("symlink CLI");

        // An explicit override is the only candidate.
        let custom = PathBuf::from("/custom/libonnxruntime.dylib");
        assert_eq!(
            candidates(Some(custom.clone()), Some(&link), &data),
            [custom]
        );

        // Bundle first (through the CLI symlink), data dir second.
        let file_name = format!("{DLL_PREFIX}onnxruntime{DLL_SUFFIX}");
        let bundled = root
            .canonicalize()
            .expect("canonical root")
            .join("Slackless.app/Contents/Frameworks")
            .join(&file_name);
        let fetched = data.join("onnxruntime").join(&file_name);
        let found = candidates(None, Some(&link), &data);
        assert_eq!(found, [bundled.clone(), fetched.clone()]);

        // No runtime anywhere is an error, never a panic.
        assert!(first_existing(&found).is_err());
        for lib in [&fetched, &bundled] {
            std::fs::create_dir_all(lib.parent().expect("parent")).expect("create lib dir");
            std::fs::write(lib, b"").expect("create lib");
            assert_eq!(first_existing(&found).as_ref(), Ok(lib));
        }
        // A file that is not ONNX Runtime is rejected here and never reaches `ort`, which would hang.
        assert!(probe(&bundled).is_err());
        let _ = std::fs::remove_dir_all(&root);

        // The real loader agrees with what this machine has: a resolvable runtime must load (this
        // is what catches a pinned runtime that `ort` rejects), a missing one must fail cleanly.
        match locate() {
            Ok(path) => {
                ensure_loaded().unwrap_or_else(|err| panic!("{} must load: {err}", path.display()));
            }
            Err(missing) => assert_eq!(ensure_loaded(), Err(missing)),
        }
    }
}
