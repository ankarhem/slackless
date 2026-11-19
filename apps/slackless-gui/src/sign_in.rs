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

/// The session to run live with: the first stored one, signing in through the helper first
/// when none is stored. `None` (sign-in cancelled or failed, or `SLACKLESS_FIXTURES` /
/// `SLACKLESS_GUI_DEMO` set) runs the bundled sample workspace instead.
pub fn session() -> Option<Session> {
    if std::env::var_os("SLACKLESS_FIXTURES").is_some()
        || std::env::var_os("SLACKLESS_GUI_DEMO").is_some()
    {
        return None;
    }
    if let Some(session) = stored() {
        return Some(session);
    }
    let helper = std::env::current_exe().and_then(|exe| Command::new(exe).arg(FLAG).status());
    match helper {
        Ok(status) if status.success() => stored(),
        // Cancelled, or failed and the helper said why.
        Ok(_) => None,
        Err(e) => {
            eprintln!("slackless: could not open the sign-in window ({e})");
            None
        }
    }
}

fn stored() -> Option<Session> {
    match SessionStore::new().and_then(|store| store.load()) {
        Ok(sessions) => sessions.into_iter().find(Session::has_secrets),
        Err(e) => {
            eprintln!("slackless: could not read saved sessions ({e})");
            None
        }
    }
}
