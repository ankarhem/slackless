//! The shared Tokio runtime and [`block_on`], the one way to run async code from synchronous
//! code in this crate.
//!
//! GPUI is smol-based and the CLI drives a `current_thread` runtime, so blocking wrappers that
//! need `reqwest` (updater, embeddings, summarize) cannot assume anything about the calling
//! context. All of them go through [`block_on`], which picks the right strategy per context and
//! otherwise shares the multi-thread runtime that [`crate::Client::start`] runs on.

use std::future::Future;
use std::io;

use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

/// The process-wide multi-thread Tokio runtime (2 workers, IO + timers enabled) that
/// [`crate::Client::start`] drives sessions on and that [`block_on`] falls back to. Built lazily on
/// first use; every caller shares the one instance.
pub(crate) fn shared_runtime() -> io::Result<&'static Runtime> {
    static RUNTIME: std::sync::OnceLock<Runtime> = std::sync::OnceLock::new();
    static INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let _guard = INIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("slackless-tokio")
        .enable_all()
        .build()?;
    Ok(RUNTIME.get_or_init(|| rt))
}

/// Run `fut` to completion from synchronous code, whatever the calling context is:
///
/// - **No ambient runtime** (GPUI background executor thread, plain OS thread): block on the
///   shared runtime's handle.
/// - **Inside a multi-thread runtime**: `block_in_place` and block on the ambient handle, so the
///   future keeps running on the caller's runtime.
/// - **Inside a `current_thread` runtime** (the CLI): `block_in_place` panics there and the
///   ambient runtime cannot drive IO or timers from another thread, so drive `fut` on the shared
///   runtime from a scoped thread.
///
/// Panics inside `fut` are re-raised on the calling thread. The only error is a failure to build
/// the shared runtime.
pub(crate) fn block_on<F>(fut: F) -> io::Result<F::Output>
where
    F: Future + Send,
    F::Output: Send,
{
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(fut)))
        }
        Ok(_) => {
            let handle = shared_runtime()?.handle().clone();
            match std::thread::scope(|s| s.spawn(|| handle.block_on(fut)).join()) {
                Ok(output) => Ok(output),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        Err(_) => Ok(shared_runtime()?.handle().block_on(fut)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A future that needs the runtime's driver (timers) and worker threads (spawned tasks),
    /// not just polling, and reports where it ran.
    async fn probe() -> (std::thread::ThreadId, RuntimeFlavor, u32) {
        tokio::time::sleep(Duration::ZERO).await;
        let flavor = Handle::current().runtime_flavor();
        let spawned = tokio::spawn(async { 7_u32 })
            .await
            .expect("spawned task joins");
        (std::thread::current().id(), flavor, spawned)
    }

    #[test]
    fn block_on_without_ambient_runtime_uses_shared_runtime_in_place() {
        assert!(
            Handle::try_current().is_err(),
            "test must run outside tokio"
        );
        let caller = std::thread::current().id();
        let (ran_on, flavor, value) = block_on(probe()).expect("shared runtime is up");
        assert_eq!(value, 7);
        assert_eq!(
            ran_on, caller,
            "no ambient runtime: the caller blocks in place"
        );
        assert_eq!(flavor, RuntimeFlavor::MultiThread);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_on_inside_multi_thread_runtime_blocks_in_place() {
        let caller = std::thread::current().id();
        let (ran_on, flavor, value) = block_on(probe()).expect("ambient multi-thread runtime");
        assert_eq!(value, 7);
        assert_eq!(
            ran_on, caller,
            "multi-thread runtime: block_in_place on the caller thread"
        );
        assert_eq!(flavor, RuntimeFlavor::MultiThread);
    }

    #[tokio::test]
    async fn block_on_inside_current_thread_runtime_runs_on_shared_runtime_thread() {
        assert_eq!(
            Handle::current().runtime_flavor(),
            RuntimeFlavor::CurrentThread
        );
        let caller = std::thread::current().id();
        let (ran_on, flavor, value) = block_on(probe()).expect("scoped thread on shared runtime");
        assert_eq!(value, 7);
        assert_ne!(
            ran_on, caller,
            "current_thread runtime: must leave the runtime thread"
        );
        assert_eq!(
            flavor,
            RuntimeFlavor::MultiThread,
            "driven by the shared runtime"
        );
    }

    #[tokio::test]
    #[should_panic(expected = "helper boom")]
    async fn block_on_inside_current_thread_runtime_reraises_panics() {
        let _ = block_on(async { panic!("helper boom") });
    }
}
