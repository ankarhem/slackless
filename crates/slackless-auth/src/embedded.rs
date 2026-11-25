//! First-party embedded sign-in: slackless acquires its **own** session.
//!
//! The product's primary acquisition path (`docs/research/api-auth.md`, flow (a) /
//! RECOMMENDATION #1) is an embedded WebKit window pointed at `https://app.slack.com/signin`.
//! Slack renders whatever it demands (email code, CAPTCHA, SSO, 2FA); on success its web client
//! writes `localConfig_v2.teams[<id>].token` into **this webview's** `localStorage`, and sets the
//! `d` cookie in **this webview's** cookie store. We read our own `localConfig_v2` via injected JS
//! and harvest the `d` cookie from the webview cookie store (it is HttpOnly, so it comes from the
//! `WKWebsiteDataStore` API, not `document.cookie`). No credentials from any other app are read,
//! and nothing is decrypted — the webview hands back the cookie in clear.
//!
//! The capture-and-build step ([`capture_sessions`]) is pure and tested here. The window itself
//! ([`embedded_sign_in`], feature `webview`) is a `tao` window hosting a `wry` WKWebView: an
//! initialization script polls the page's `localConfig_v2` and posts it over wry's IPC channel
//! once it holds an `xoxc-` token; the host then reads `d` from the webview cookie store and
//! hands both to [`capture_sessions`].

use serde_json::Value;
use slackless_types::{TeamId, UserId};

#[cfg(feature = "webview")]
use crate::error::AuthError;
use crate::error::Result;
use crate::session::Session;

/// What to open the embedded sign-in on.
#[derive(Debug, Clone, Default)]
pub struct SignInRequest {
    /// Optional workspace subdomain to deep-link to (`acme` -> `https://acme.slack.com/`); when
    /// `None`, the generic `https://app.slack.com/signin` workspace picker is shown.
    pub workspace_domain: Option<String>,
}

impl SignInRequest {
    /// The page the sign-in window opens on.
    pub fn url(&self) -> String {
        match self.workspace_domain.as_deref().filter(|d| !d.is_empty()) {
            Some(domain) => domain_url(domain),
            None => "https://app.slack.com/signin".to_owned(),
        }
    }
}

/// The raw material extracted from the embedded webview after a successful sign-in: the webview's
/// own `localConfig_v2` localStorage value and the `d` cookie read from its cookie store.
#[derive(Debug, Clone)]
pub struct WebviewCapture {
    /// The JSON string stored under `localStorage["localConfig_v2"]`.
    pub local_config_v2: String,
    /// The `d` cookie value (`xoxd-…`) from the webview cookie store, already in clear.
    pub d_cookie: String,
}

/// Builds the signed-in [`Session`]s from one [`WebviewCapture`].
///
/// `localConfig_v2` holds every workspace the webview is signed into
/// (`{"teams":{"<id>":{"token":"xoxc-…","url":…,"user_id":…}}}`); each becomes one session paired
/// with the single account-wide `d` cookie. Teams with no token, or whose workspace URL cannot be
/// resolved from the entry, are skipped. Unknown fields are ignored so a shape change elsewhere in
/// the blob never breaks capture (patchability).
pub fn capture_sessions(capture: &WebviewCapture) -> Result<Vec<Session>> {
    let root: Value = serde_json::from_str(&capture.local_config_v2)?;
    let teams = match root.get("teams").and_then(Value::as_object) {
        Some(teams) => teams,
        None => return Ok(Vec::new()),
    };

    let mut sessions = Vec::new();
    for (id, team) in teams {
        let Some(token) = team
            .get("token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let Some(workspace_url) = team_workspace_url(team) else {
            continue;
        };
        let user_id = team
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        sessions.push(Session::new(
            workspace_url,
            TeamId::from(id.as_str()),
            token,
            capture.d_cookie.clone(),
            UserId::from(user_id),
        ));
    }
    Ok(sessions)
}

/// Resolves a team entry's workspace URL from its `url` field, falling back to `domain`.
fn team_workspace_url(team: &Value) -> Option<String> {
    if let Some(url) = team
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
    {
        return Some(url.to_owned());
    }
    team.get("domain")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty())
        .map(domain_url)
}

/// `acme` -> `https://acme.slack.com/`; Grid `acme.enterprise` -> `https://acme.enterprise.slack.com/`;
/// a full `*.slack.com` host (`acme.enterprise.slack.com`) is kept.
fn domain_url(domain: &str) -> String {
    if domain.ends_with(".slack.com") {
        format!("https://{domain}/")
    } else {
        format!("https://{domain}.slack.com/")
    }
}

/// Whether a cookie `domain` belongs to Slack (`slack.com` or any subdomain like `.slack.com`).
#[cfg(any(test, feature = "webview"))]
fn is_slack_cookie_domain(domain: &str) -> bool {
    let d = domain.to_ascii_lowercase();
    d == "slack.com" || d.ends_with(".slack.com")
}

/// Posts the page's `localConfig_v2` to the host once it holds a token. Re-posts every second
/// so a `d` cookie that lands after the token is still picked up on a later tick.
///
/// Sign-in ends on Slack's "Launching <workspace>" page (`/ssb/redirect`), which hands off to
/// the desktop app and never boots the web client that writes `localConfig_v2`; the script
/// leaves it for the page's web-client link (Slack's own "use Slack in your browser").
#[cfg(feature = "webview")]
const CAPTURE_SCRIPT: &str = r#"(() => {
  let leaving = false;
  setInterval(() => {
    try {
      if (!leaving && location.pathname.startsWith("/ssb/redirect")) {
        leaving = true;
        const web = document.querySelector('a[href*="/client"]');
        location.replace(web ? web.href : "https://app.slack.com/client");
        return;
      }
      const config = window.localStorage.getItem("localConfig_v2");
      if (config && config.includes("xoxc-")) window.ipc.postMessage(config);
    } catch (_) {}
  }, 1000);
})();"#;

/// Safari's user agent at the installed Safari's version. WKWebView's default omits
/// `Version/… Safari/…`, and Slack's sign-in page rejects that, and outdated versions, as an
/// unsupported browser.
#[cfg(feature = "webview")]
fn user_agent() -> String {
    let version = std::fs::read_to_string("/Applications/Safari.app/Contents/Info.plist")
        .ok()
        .and_then(|plist| plist_string(&plist, "CFBundleShortVersionString"))
        .unwrap_or_else(|| "26.0".to_owned());
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
         (KHTML, like Gecko) Version/{version} Safari/605.1.15"
    )
}

/// The `<string>` value immediately following `key` in an XML property list.
#[cfg(any(test, feature = "webview"))]
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let (_, after) = plist.split_once(&format!("<key>{key}</key>"))?;
    let value = after.trim_start().strip_prefix("<string>")?;
    let (value, _) = value.split_once("</string>")?;
    Some(value.trim().to_owned())
}

/// Opens the embedded WebKit sign-in window and returns the sessions it captured.
///
/// Runs its own event loop and returns when a session is captured or the window is closed
/// ([`AuthError::SignInCancelled`]). Must be called on the main thread of a process that runs no
/// other UI event loop (front-ends with one run this in a helper process).
#[cfg(feature = "webview")]
pub fn embedded_sign_in(request: SignInRequest) -> Result<Vec<Session>> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::window::WindowBuilder;
    use wry::{NewWindowResponse, WebViewBuilder};

    enum Signal {
        /// The page's `localConfig_v2`, posted by [`CAPTURE_SCRIPT`].
        LocalConfig(String),
        /// A `window.open` (SSO popups): followed in the sign-in window itself.
        Navigate(String),
    }

    let window_error = |e: &dyn std::fmt::Display| AuthError::SignInWindow(e.to_string());
    let mut event_loop = EventLoopBuilder::<Signal>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Sign in to Slack")
        .with_inner_size(LogicalSize::new(520.0, 760.0))
        .build(&event_loop)
        .map_err(|e| window_error(&e))?;
    let ipc = event_loop.create_proxy();
    let popups = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_url(request.url())
        .with_user_agent(user_agent())
        .with_initialization_script(CAPTURE_SCRIPT)
        // The launcher page opens `slack://` links: never hand the session to the desktop app.
        .with_navigation_handler(|url| !url.starts_with("slack:"))
        .with_ipc_handler(move |message| {
            let _ = ipc.send_event(Signal::LocalConfig(message.into_body()));
        })
        .with_new_window_req_handler(move |url, _| {
            let _ = popups.send_event(Signal::Navigate(url));
            NewWindowResponse::Deny
        })
        .build(&window)
        .map_err(|e| window_error(&e))?;

    let mut outcome = Err(AuthError::SignInCancelled);
    let mut reported_missing_cookie = false;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(Signal::Navigate(url)) => {
                let _ = webview.load_url(&url);
            }
            Event::UserEvent(Signal::LocalConfig(local_config_v2)) => {
                // `d` is HttpOnly on `.slack.com`: only the cookie store has it. wry's
                // `cookies_for_url` keeps exact-host matches only, so filter the whole store.
                let d_cookie = webview
                    .cookies()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|cookie| {
                        cookie.name() == "d"
                            && cookie.domain().is_some_and(is_slack_cookie_domain)
                    })
                    .map(|cookie| cookie.value().to_owned());
                let Some(d_cookie) = d_cookie else {
                    if !reported_missing_cookie {
                        reported_missing_cookie = true;
                        eprintln!(
                            "slackless: Slack's web client has a token, but its `d` cookie is not in the cookie store yet"
                        );
                    }
                    return;
                };
                match capture_sessions(&WebviewCapture {
                    local_config_v2,
                    d_cookie,
                }) {
                    Ok(sessions) if sessions.is_empty() => {}
                    captured => {
                        outcome = captured;
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            _ => {}
        }
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AuthError;

    fn capture(json: &str) -> WebviewCapture {
        WebviewCapture {
            local_config_v2: json.to_owned(),
            d_cookie: "xoxd-COOKIE/value=".to_owned(),
        }
    }

    #[test]
    fn captures_sessions_from_localconfig() {
        let json = r#"{
            "v": 1,
            "teams": {
                "T111": { "token": "xoxc-111", "url": "https://alpha.slack.com/", "user_id": "U1", "unknown": 7 },
                "T222": { "token": "xoxc-222", "domain": "beta" }
            }
        }"#;
        let mut got = capture_sessions(&capture(json)).unwrap();
        got.sort_by(|a, b| a.team_id.as_str().cmp(b.team_id.as_str()));

        assert_eq!(got.len(), 2);
        assert_eq!(got[0].team_id.as_str(), "T111");
        assert_eq!(got[0].workspace_url, "https://alpha.slack.com/");
        assert_eq!(got[0].xoxc_token, "xoxc-111");
        assert_eq!(got[0].user_id.as_str(), "U1");
        assert_eq!(got[0].d_cookie, "xoxd-COOKIE/value=");
        // `domain` without a dot is expanded to the slack.com host.
        assert_eq!(got[1].workspace_url, "https://beta.slack.com/");
        assert!(got.iter().all(Session::has_secrets));
    }

    #[test]
    fn skips_tokenless_and_unroutable_teams() {
        let json = r#"{
            "teams": {
                "T1": { "token": "", "url": "https://x.slack.com/" },
                "T2": { "url": "https://y.slack.com/" },
                "T3": { "token": "xoxc-3" }
            }
        }"#;
        // T1 has no token, T2 has no token, T3 has no resolvable URL -> all skipped.
        assert!(capture_sessions(&capture(json)).unwrap().is_empty());
    }

    #[test]
    fn tolerates_missing_teams_object() {
        assert!(
            capture_sessions(&capture(r#"{"other":1}"#))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_non_json() {
        assert!(matches!(
            capture_sessions(&capture("not json")),
            Err(AuthError::Json(_))
        ));
    }

    #[test]
    fn sign_in_url_defaults_to_the_picker_and_expands_domains() {
        let url = |domain: Option<&str>| {
            SignInRequest {
                workspace_domain: domain.map(str::to_owned),
            }
            .url()
        };
        assert_eq!(url(None), "https://app.slack.com/signin");
        assert_eq!(url(Some("")), "https://app.slack.com/signin");
        assert_eq!(url(Some("acme")), "https://acme.slack.com/");
        assert_eq!(
            url(Some("acme.enterprise")),
            "https://acme.enterprise.slack.com/"
        );
        assert_eq!(
            url(Some("acme.enterprise.slack.com")),
            "https://acme.enterprise.slack.com/"
        );
    }

    #[test]
    fn capture_sessions_handles_empty_url_fallback_empty_cookie_and_non_object_entries() {
        let json = r#"{
            "teams": {
                "T1": { "token": "xoxc-1", "url": "", "domain": "acme.enterprise", "user_id": "U1" },
                "T_STR": "not-an-object",
                "T_NUM": 123,
                "T_NULL": null
            }
        }"#;
        let cap = WebviewCapture {
            local_config_v2: json.to_owned(),
            d_cookie: String::new(),
        };
        let sessions = capture_sessions(&cap).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].team_id.as_str(), "T1");
        assert_eq!(
            sessions[0].workspace_url,
            "https://acme.enterprise.slack.com/"
        );
        assert_eq!(sessions[0].d_cookie, "");
        assert!(!sessions[0].has_secrets());
    }

    #[test]
    fn cookie_domain_matching_rejects_lookalike_hosts() {
        assert!(is_slack_cookie_domain("slack.com"));
        assert!(is_slack_cookie_domain(".slack.com"));
        assert!(is_slack_cookie_domain("app.slack.com"));
        assert!(is_slack_cookie_domain("APP.SLACK.COM"));
        assert!(!is_slack_cookie_domain("evilslack.com"));
        assert!(!is_slack_cookie_domain("slack.com.evil.example"));
        assert!(!is_slack_cookie_domain("example.com"));
    }

    #[test]
    fn reads_a_plist_string_value_without_skipping_non_string_values() {
        let plist = "<dict>\n\t<key>CFBundleName</key>\n\t<string>Safari</string>\n\t\
                     <key>BuildNumber</key>\n\t<integer>42</integer>\n\t\
                     <key>CFBundleShortVersionString</key>\n\t<string>27.2</string>\n</dict>";
        assert_eq!(
            plist_string(plist, "CFBundleShortVersionString").as_deref(),
            Some("27.2")
        );
        assert_eq!(
            plist_string(plist, "CFBundleName").as_deref(),
            Some("Safari")
        );
        assert_eq!(
            plist_string(plist, "BuildNumber"),
            None,
            "non-string value must not bleed into a subsequent key's <string>"
        );
        assert_eq!(plist_string(plist, "Missing"), None);
    }
}
