//! First-launch sign-in. The WebKit sign-in window needs its own event loop and GPUI owns this
//! process's, so it runs in a helper: this same binary started with [`FLAG`].

use std::process::Command;

use slackless_auth::{AuthError, Session, SessionStore, SignInRequest, embedded_sign_in};

/// Starts the binary as the sign-in helper.
pub const FLAG: &str = "--sign-in";

/// Helper-process entry: opens the sign-in window and stores the sessions it captured. Returns
/// the exit code: 0 signed in, 1 failed, 2 cancelled.
pub fn run_helper() -> i32 {
    let stored = embedded_sign_in(SignInRequest::default()).and_then(|sessions| {
        let store = SessionStore::new()?;
        for session in sessions {
            store.upsert(session)?;
        }
        Ok(())
    });
    match stored {
        Ok(()) => 0,
        Err(AuthError::SignInCancelled) => 2,
        Err(e) => {
            eprintln!("slackless: sign-in failed: {e}");
            1
        }
    }
}

/// Every signed-in session to run live with: all stored sessions that have secrets, signing in
/// through the helper first when none are stored. An empty list (sign-in cancelled or failed, or
/// `SLACKLESS_FIXTURES` / `SLACKLESS_GUI_DEMO` set) runs the bundled sample workspace instead.
pub fn sessions() -> Vec<Session> {
    if std::env::var_os("SLACKLESS_FIXTURES").is_some()
        || std::env::var_os("SLACKLESS_GUI_DEMO").is_some()
    {
        return Vec::new();
    }
    let existing = stored_all();
    if !existing.is_empty() {
        return existing;
    }
    let helper = std::env::current_exe().and_then(|exe| Command::new(exe).arg(FLAG).status());
    match helper {
        Ok(status) if status.success() => stored_all(),
        // Cancelled, or failed and the helper said why.
        Ok(_) => Vec::new(),
        Err(e) => {
            eprintln!("slackless: could not open the sign-in window ({e})");
            Vec::new()
        }
    }
}

fn stored_all() -> Vec<Session> {
    match SessionStore::new().and_then(|store| store.load()) {
        Ok(sessions) => sessions.into_iter().filter(Session::has_secrets).collect(),
        Err(e) => {
            eprintln!("slackless: could not read saved sessions ({e})");
            Vec::new()
        }
    }
}
