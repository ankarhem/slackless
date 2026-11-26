//! Native PDF page inspection and rasterization (`CGPDFDocument` + `ImageIO` on macOS).

use std::path::Path;

use crate::Result;

/// One rasterized PDF page encoded as PNG bytes together with its pixel dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPdfPage {
    pub png_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::path::Path;

    use crate::macos::ffi::{
        slackless_free_buf, slackless_pdf_page_count, slackless_pdf_render_page_png,
    };
    use crate::{HuddleError, Result};

    use super::RenderedPdfPage;

    fn path_cstring(path: &Path) -> Result<CString> {
        let s = path.to_str().ok_or_else(|| {
            HuddleError::Video(format!("PDF path is not valid UTF-8: {}", path.display()))
        })?;
        CString::new(s).map_err(|_| {
            HuddleError::Video(format!(
                "PDF path contains interior NUL byte: {}",
                path.display()
            ))
        })
    }

    pub fn pdf_page_count(path: &Path) -> Result<u32> {
        if !path.is_file() {
            return Err(HuddleError::Video(format!(
                "PDF file not found: {}",
                path.display()
            )));
        }
        let c_path = path_cstring(path)?;
        // SAFETY: `c_path` is a valid NUL-terminated UTF-8 string.
        let count = unsafe { slackless_pdf_page_count(c_path.as_ptr()) };
        if count == 0 {
            return Err(HuddleError::Video(format!(
                "failed to open PDF or document has no pages: {}",
                path.display()
            )));
        }
        Ok(count)
    }

    pub fn render_pdf_page_png(
        path: &Path,
        page_index: u32,
        max_width: u32,
        max_height: u32,
    ) -> Result<RenderedPdfPage> {
        if !path.is_file() {
            return Err(HuddleError::Video(format!(
                "PDF file not found: {}",
                path.display()
            )));
        }
        let c_path = path_cstring(path)?;
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut width: u32 = 0;
        let mut height: u32 = 0;
        // SAFETY: `c_path` is a valid NUL-terminated UTF-8 string; out-pointers are stack locals.
        let ok = unsafe {
            slackless_pdf_render_page_png(
                c_path.as_ptr(),
                page_index,
                max_width,
                max_height,
                &mut buf,
                &mut len,
                &mut width,
                &mut height,
            )
        };
        if !ok || buf.is_null() || len == 0 {
            if !buf.is_null() {
                // SAFETY: `buf` was allocated with `malloc` in `slackless_pdf_render_page_png`.
                unsafe { slackless_free_buf(buf) };
            }
            return Err(HuddleError::Video(format!(
                "failed to render PDF page {} of {}",
                page_index + 1,
                path.display()
            )));
        }
        // SAFETY: `buf` is valid for `len` initialized bytes until freed immediately below.
        let png_bytes = unsafe { std::slice::from_raw_parts(buf, len) }.to_vec();
        // SAFETY: `buf` was allocated with `malloc` in `slackless_pdf_render_page_png`.
        unsafe { slackless_free_buf(buf) };
        Ok(RenderedPdfPage {
            png_bytes,
            width,
            height,
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    use crate::{HuddleError, Result};

    use super::RenderedPdfPage;

    pub fn pdf_page_count(_path: &Path) -> Result<u32> {
        Err(HuddleError::Unsupported(
            "native in-app PDF rendering is only supported on macOS".to_owned(),
        ))
    }

    pub fn render_pdf_page_png(
        _path: &Path,
        _page_index: u32,
        _max_width: u32,
        _max_height: u32,
    ) -> Result<RenderedPdfPage> {
        Err(HuddleError::Unsupported(
            "native in-app PDF rendering is only supported on macOS".to_owned(),
        ))
    }
}

/// Return the number of pages in a local PDF document.
pub fn pdf_page_count(path: &Path) -> Result<u32> {
    imp::pdf_page_count(path)
}

/// Render zero-based `page_index` of the local PDF at `path` into PNG bytes fitted within
/// `(max_width, max_height)` pixels while preserving aspect ratio.
pub fn render_pdf_page_png(
    path: &Path,
    page_index: u32,
    max_width: u32,
    max_height: u32,
) -> Result<RenderedPdfPage> {
    imp::render_pdf_page_png(path, page_index, max_width, max_height)
}

/// Build a minimal valid multi-page PDF document in memory (useful for tests).
#[cfg(test)]
pub(crate) fn build_minimal_test_pdf(page_count: usize) -> Vec<u8> {
    let page_count = page_count.max(1);
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");

    let mut offsets = Vec::new();
    // Object 1: Catalog
    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    // Object 2: Pages
    offsets.push(out.len());
    let kids: String = (0..page_count)
        .map(|i| format!("{} 0 R", 3 + i))
        .collect::<Vec<_>>()
        .join(" ");
    out.extend_from_slice(
        format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {page_count} >>\nendobj\n")
            .as_bytes(),
    );

    // Objects 3..=2+page_count: individual Page objects (alternate portrait / landscape boxes).
    for i in 0..page_count {
        offsets.push(out.len());
        let (w, h) = if i % 2 == 0 { (300, 400) } else { (400, 200) };
        out.extend_from_slice(
            format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] >>\nendobj\n",
                3 + i
            )
            .as_bytes(),
        );
    }

    let xref_offset = out.len();
    let total_objs = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {total_objs}\n0000000000 65535 f \n").as_bytes());
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {total_objs} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_corrupt_pdf_return_clean_error() {
        let missing =
            std::env::temp_dir().join(format!("slackless-pdf-missing-{}.pdf", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        assert!(pdf_page_count(&missing).is_err());
        assert!(render_pdf_page_png(&missing, 0, 800, 800).is_err());

        let corrupt =
            std::env::temp_dir().join(format!("slackless-pdf-corrupt-{}.pdf", std::process::id()));
        std::fs::write(&corrupt, b"not a valid pdf document").unwrap();
        assert!(pdf_page_count(&corrupt).is_err());
        assert!(render_pdf_page_png(&corrupt, 0, 800, 800).is_err());
        let _ = std::fs::remove_file(&corrupt);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn renders_multipage_synthetic_pdf_and_rejects_out_of_range_page() {
        let path =
            std::env::temp_dir().join(format!("slackless-pdf-test-{}.pdf", std::process::id()));
        std::fs::write(&path, build_minimal_test_pdf(2)).expect("write test PDF");

        assert_eq!(pdf_page_count(&path).expect("page count"), 2);

        let page0 = render_pdf_page_png(&path, 0, 600, 800).expect("render page 0");
        assert!(page0.png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!((page0.width, page0.height), (600, 800));

        let page1 = render_pdf_page_png(&path, 1, 600, 800).expect("render page 1");
        assert!(page1.png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!((page1.width, page1.height), (600, 300));

        assert!(
            render_pdf_page_png(&path, 2, 600, 800).is_err(),
            "out-of-range page must fail cleanly"
        );

        let _ = std::fs::remove_file(&path);
    }
}
