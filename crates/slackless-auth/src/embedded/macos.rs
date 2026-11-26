//! The macOS sign-in window: a `WKWebView` filling an `NSWindow`, driven by `NSApplication`'s
//! event loop until a session is captured or the window is closed.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::{DynBlock, RcBlock};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSEvent, NSEventModifierFlags, NSEventType, NSTitlebarSeparatorStyle, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSHTTPCookie, NSNotification, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest,
    ns_string,
};
use objc2_web_kit::{
    WKHTTPCookieStore, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKScriptMessage, WKScriptMessageHandler, WKUIDelegate, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    WKWindowFeatures,
};
use slackless_types::net::redact_url;

use super::{
    SignInRequest, WebviewCapture, capture_script, capture_sessions, is_slack_cookie_domain,
    is_slack_https_url, user_agent,
};
use crate::error::{AuthError, Result};
use crate::session::Session;

/// The script message handler pages post their `localConfig_v2` to.
const IPC_HANDLER: &str = "ipc";

/// Defines the `window.ipc.postMessage` the capture script calls, on top of [`IPC_HANDLER`].
const IPC_SHIM: &str = "Object.defineProperty(window, 'ipc', { value: Object.freeze({ \
    postMessage: function(s) { window.webkit.messageHandlers.ipc.postMessage(s) } }) });";

/// What the window's callbacks share with the completion handler of a cookie read.
#[derive(Default)]
struct State {
    /// Set once sign-in is over: the captured sessions, or why it ended without any.
    outcome: RefCell<Option<Result<Vec<Session>>>>,
    reported_foreign_page: Cell<bool>,
    reported_missing_cookie: Cell<bool>,
}

impl State {
    fn is_finished(&self) -> bool {
        self.outcome.borrow().is_some()
    }

    /// Ends sign-in with `outcome` and makes [`run_window`]'s event loop return. Only the
    /// first outcome counts: closing the window after a capture does not cancel it.
    fn finish(&self, outcome: Result<Vec<Session>>, mtm: MainThreadMarker) {
        if self.is_finished() {
            return;
        }
        *self.outcome.borrow_mut() = Some(outcome);

        let app = NSApplication::sharedApplication(mtm);
        app.stop(None);
        // `stop:` takes effect when the event being handled returns, and WebKit's callbacks
        // arrive outside of any event: post one so the loop notices.
        let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
        if let Some(wake) = wake {
            app.postEvent_atStart(&wake, true);
        }
    }

    /// Pairs a Slack page's `localConfig_v2` with the `d` cookie from the cookie store, and
    /// finishes once they make at least one session.
    fn capture(
        &self,
        local_config_v2: &str,
        cookies: &NSArray<NSHTTPCookie>,
        mtm: MainThreadMarker,
    ) {
        if self.is_finished() {
            return;
        }
        // `d` is HttpOnly on `.slack.com`: only the cookie store has it. The store holds every
        // site's cookies, so only a Slack domain's `d` will do.
        let d_cookie = cookies
            .iter()
            .find(|cookie| {
                cookie.name().to_string() == "d"
                    && is_slack_cookie_domain(&cookie.domain().to_string())
            })
            .map(|cookie| cookie.value().to_string());
        let Some(d_cookie) = d_cookie else {
            if !self.reported_missing_cookie.replace(true) {
                tracing::warn!(
                    "Slack's web client has a token, but its `d` cookie is not in the cookie store yet"
                );
            }
            return;
        };
        match capture_sessions(&WebviewCapture {
            local_config_v2: local_config_v2.to_owned(),
            d_cookie,
        }) {
            // Not signed in to a workspace yet: the page posts again every second.
            Ok(sessions) if sessions.is_empty() => {}
            captured => self.finish(captured, mtm),
        }
    }
}

struct Ivars {
    state: Rc<State>,
    cookies: Retained<WKHTTPCookieStore>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `SignIn` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct SignIn;

    // SAFETY: `NSObjectProtocol` asks for nothing beyond what `NSObject` already implements.
    unsafe impl NSObjectProtocol for SignIn {}

    // SAFETY: the method has the selector and signature `NSApplicationDelegate` declares.
    unsafe impl NSApplicationDelegate for SignIn {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            // A binary outside an app bundle is no foreground app until its activation policy
            // says so, and AppKit only honours that (and activation) once it has launched. The
            // window was ordered front before then, so it is made key again now.
            let app = NSApplication::sharedApplication(self.mtm());
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            for window in app.windows().iter() {
                if window.isVisible() {
                    window.makeKeyAndOrderFront(None);
                }
            }
            #[expect(
                deprecated,
                reason = "`activate` only asks; the window must come in front of the launching terminal or app"
            )]
            app.activateIgnoringOtherApps(true);
        }
    }

    // SAFETY: the method has the selector and signature `NSWindowDelegate` declares.
    unsafe impl NSWindowDelegate for SignIn {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            self.ivars()
                .state
                .finish(Err(AuthError::SignInCancelled), self.mtm());
            // `run_window` closes the window once its event loop has returned.
            false
        }
    }

    // SAFETY: the method has the selector and signature `WKScriptMessageHandler` declares.
    unsafe impl WKScriptMessageHandler for SignIn {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive_script_message(
            &self,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
        ) {
            let Ivars { state, cookies } = self.ivars();
            if state.is_finished() {
                return;
            }
            // SAFETY: `message` is the live message WebKit is delivering, on the main thread.
            let (body, page) = unsafe { (message.body(), message.frameInfo().request().URL()) };
            let Ok(body) = body.downcast::<NSString>() else {
                return;
            };
            // The capture script runs on every page the window visits (navigation stays open for
            // SSO), so only a Slack page may hand over the config its session is built from.
            let page = page
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string())
                .unwrap_or_default();
            if !is_slack_https_url(&page) {
                if !state.reported_foreign_page.replace(true) {
                    tracing::warn!(
                        page = %redact_url(&page),
                        "ignoring sign-in data posted by a non-Slack page"
                    );
                }
                return;
            }

            let local_config_v2 = body.to_string();
            let state = Rc::clone(state);
            let mtm = self.mtm();
            let read = RcBlock::new(move |all: NonNull<NSArray<NSHTTPCookie>>| {
                // SAFETY: WebKit passes a valid, non-null array that outlives this call.
                let all = unsafe { all.as_ref() };
                state.capture(&local_config_v2, all, mtm);
            });
            // SAFETY: called on the main thread, where WebKit also invokes the completion
            // handler, so the block's main-thread-only captures never leave it.
            unsafe { cookies.getAllCookies(&read) };
        }
    }

    // SAFETY: the method has the selector and signature `WKNavigationDelegate` declares.
    unsafe impl WKNavigationDelegate for SignIn {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy_for_navigation_action(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            decision: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: `action` is the live navigation action WebKit is asking about.
            let (download, url) =
                unsafe { (action.shouldPerformDownload(), action.request().URL()) };
            // Nothing is downloaded here, and the launcher page opens `slack://` links: never
            // hand the session to the desktop app.
            let allowed = !download
                && url
                    .and_then(|url| url.absoluteString())
                    .is_some_and(|url| !url.to_string().starts_with("slack:"));
            decision.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }

    // SAFETY: the method has the selector and signature `WKUIDelegate` declares.
    unsafe impl WKUIDelegate for SignIn {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            webview: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // A `window.open` (SSO popups): followed in the sign-in window itself.
            // SAFETY: `action` is the live navigation action WebKit is asking about.
            if let Some(url) = unsafe { action.request() }.URL() {
                // SAFETY: `webview` is the live sign-in view, on the main thread.
                unsafe { webview.loadRequest(&NSURLRequest::requestWithURL(&url)) };
            }
            None
        }
    }
);

impl SignIn {
    fn new(
        mtm: MainThreadMarker,
        state: Rc<State>,
        cookies: Retained<WKHTTPCookieStore>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { state, cookies });
        // SAFETY: `NSObject`'s `init` on a freshly allocated subclass instance.
        unsafe { msg_send![super(this), init] }
    }
}

/// Opens the embedded WebKit sign-in window and returns the sessions it captured.
///
/// Runs `NSApplication`'s event loop and returns when a session is captured or the window is
/// closed ([`AuthError::SignInCancelled`]). Must be called on the main thread of a process that
/// runs no other UI event loop (front-ends with one run this in a helper process).
pub fn embedded_sign_in(request: SignInRequest) -> Result<Vec<Session>> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| window_error("the sign-in window can only open on the main thread"))?;
    // Outside the event loop nothing drains the main thread's autoreleased objects: do it here,
    // so the window and its web content are gone when this returns.
    autoreleasepool(|_| run_window(mtm, &request.url()))
}

fn window_error(what: &str) -> AuthError {
    AuthError::SignInWindow(what.to_owned())
}

/// Shows the sign-in window on `url` and runs the event loop until sign-in is over.
fn run_window(mtm: MainThreadMarker, url: &str) -> Result<Vec<Session>> {
    let url = NSURL::URLWithString(&NSString::from_str(url))
        .ok_or_else(|| window_error("the sign-in address is not a valid URL"))?;

    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(520.0, 760.0));
    // SAFETY: the designated initializer on a fresh allocation, on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: `window` owns a retain of its own, so AppKit must not release it again on close.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("Sign in to Slack"));
    window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
    window.center();

    let state = Rc::new(State::default());
    // SAFETY: plain WebKit setup on the main thread (`mtm`), with live objects of the types each
    // call expects. The handler and delegates registered here are kept alive by `delegate` and
    // unregistered below before it is dropped.
    let (webview, controller, delegate) = unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        // The persistent store: whoever is still signed in to Slack or their SSO provider from
        // an earlier sign-in is not asked again.
        let store = WKWebsiteDataStore::defaultDataStore(mtm);
        configuration.setWebsiteDataStore(&store);
        configuration.preferences().setTabFocusesLinks(true);

        let delegate = SignIn::new(mtm, Rc::clone(&state), store.httpCookieStore());
        let controller = configuration.userContentController();
        controller.addScriptMessageHandler_name(
            ProtocolObject::from_ref(&*delegate),
            ns_string!(IPC_HANDLER),
        );
        for script in [IPC_SHIM, &capture_script()] {
            controller.addUserScript(
                &WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    mtm.alloc(),
                    &NSString::from_str(script),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                ),
            );
        }

        let webview = WKWebView::initWithFrame_configuration(mtm.alloc(), frame, &configuration);
        webview.setCustomUserAgent(Some(&NSString::from_str(&user_agent())));
        webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.loadRequest(&NSURLRequest::requestWithURL(&url));
        (webview, controller, delegate)
    };

    window.setContentView(Some(&webview));
    window.makeFirstResponder(Some(&webview));
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    window.makeKeyAndOrderFront(None);

    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();

    // Nothing may call into `delegate` once it is gone, and the page keeps posting until then.
    app.setDelegate(None);
    window.setDelegate(None);
    // SAFETY: the live view and its content controller, on the main thread.
    unsafe {
        controller.removeScriptMessageHandlerForName(ns_string!(IPC_HANDLER));
        webview.setNavigationDelegate(None);
        webview.setUIDelegate(None);
    }
    window.close();

    state
        .outcome
        .take()
        .unwrap_or(Err(AuthError::SignInCancelled))
}
