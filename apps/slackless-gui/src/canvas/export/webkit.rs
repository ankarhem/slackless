//! macOS: the canvas PDF, drawn by WebKit from the export page in an offscreen `WKWebView`.
//!
//! WebKit only runs on the main thread, but it loads the page and draws the PDF in its own
//! processes: the main thread only starts each step and receives the result.

use std::cell::Cell;

use block2::RcBlock;
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSData, NSError, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{
    WKNavigation, WKNavigationDelegate, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};

/// The PDF's bytes, or why WebKit made none.
type Pdf = Result<Vec<u8>, String>;

/// How wide WebKit lays the page out: Letter paper's 8.5 inches at CSS's 96 px per inch.
const PAGE_WIDTH: f64 = 816.0;

const NO_PDF: &str = "WebKit made no PDF";

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `PdfPage` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<Option<oneshot::Sender<Pdf>>>]
    struct PdfPage;

    // SAFETY: `NSObjectProtocol` asks for nothing beyond what `NSObject` already implements.
    unsafe impl NSObjectProtocol for PdfPage {}

    // SAFETY: each method has the selector and signature `WKNavigationDelegate` declares.
    unsafe impl WKNavigationDelegate for PdfPage {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish_navigation(&self, web_view: &WKWebView, _navigation: Option<&WKNavigation>) {
            let Some(done) = self.ivars().take() else {
                return;
            };
            // The handler is a `Fn`, and the sender goes out once.
            let done = Cell::new(Some(done));
            let made = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
                // SAFETY: WebKit passes the PDF or the error, each null or valid for this call.
                let pdf = match unsafe { (data.as_ref(), error.as_ref()) } {
                    (Some(data), _) => Ok(data.to_vec()),
                    (None, Some(error)) => Err(error.localizedDescription().to_string()),
                    (None, None) => Err(NO_PDF.to_owned()),
                };
                deliver(done.take(), pdf);
            });
            // SAFETY: `web_view` is the live view WebKit reports on, on the main thread, where
            // WebKit also calls `made`. Without a configuration it draws the whole page.
            unsafe { web_view.createPDFWithConfiguration_completionHandler(None, &made) };
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional_navigation(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            deliver(self.ivars().take(), Err(error.localizedDescription().to_string()));
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail_navigation(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            deliver(self.ivars().take(), Err(error.localizedDescription().to_string()));
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_content_process_did_terminate(&self, _web_view: &WKWebView) {
            deliver(self.ivars().take(), Err("WebKit's page process quit".to_owned()));
        }
    }
);

impl PdfPage {
    fn new(mtm: MainThreadMarker, done: oneshot::Sender<Pdf>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(Some(done)));
        // SAFETY: `NSObject`'s `init` on a freshly allocated subclass instance.
        unsafe { msg_send![super(this), init] }
    }
}

/// Hands `pdf` to the render waiting for it, if it still waits: only the first outcome counts.
fn deliver(done: Option<oneshot::Sender<Pdf>>, pdf: Pdf) {
    if let Some(done) = done {
        // A render dropped before its PDF was made no longer wants it.
        let _ = done.send(pdf);
    }
}

/// Makes a PDF of `html` in an offscreen web view [`PAGE_WIDTH`] wide: WebKit draws the whole
/// laid-out page onto one PDF page. The web view, with the WebKit page process behind it, lives
/// only until then.
///
/// # Errors
///
/// Why WebKit made no PDF, or that this is not the main thread, the only one WebKit runs on.
pub(super) async fn render_pdf(html: &str) -> Pdf {
    let mtm = MainThreadMarker::new().ok_or("WebKit only runs on the main thread")?;
    let (done, pdf) = oneshot::channel();
    let page = PdfPage::new(mtm, done);
    // SAFETY: WebKit set up on the main thread (`mtm`), with live objects of the types each call
    // expects. The view holds `page` weakly; both live until the PDF is made, below.
    let _web_view = unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        // Nothing of the page reaches the disk.
        configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
        // One point tall: the PDF takes the page's own height, not the view's.
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(PAGE_WIDTH, 1.0));
        let web_view = WKWebView::initWithFrame_configuration(mtm.alloc(), frame, &configuration);
        web_view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*page)));
        web_view.loadHTMLString_baseURL(&NSString::from_str(html), None);
        web_view
    };
    pdf.await.unwrap_or_else(|_| Err(NO_PDF.to_owned()))
}
