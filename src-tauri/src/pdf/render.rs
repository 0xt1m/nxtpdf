//! Page rasterization via PDFium.
//!
//! PDFium is used purely as a renderer. It never owns the document — it is
//! handed the serialized bytes of the current `lopdf` model on each call. See
//! [`crate::state`] for why.
//!
//! # Thread safety
//!
//! **PDFium is not thread-safe.** It keeps process-global state, so two threads
//! inside the library at once corrupts it — in practice an access violation
//! that takes the whole app down, not a recoverable error.
//!
//! `pdfium-render`'s `sync` feature does *not* make it safe: it only adds
//! `unsafe impl Send + Sync`, which asserts thread safety to the compiler
//! rather than providing it. That assertion is what lets us hold `Pdfium` in a
//! `OnceLock`, and it is sound only because every entry point below serializes
//! on [`RENDER_LOCK`].
//!
//! The lock is held across the whole load-and-render sequence, because the
//! `PdfDocument` and everything borrowed from it also touch that global state.
//!
//! Consequence: renders are serialized. A page grid issues one request per
//! visible page and they queue. That is why thumbnails render at a low DPI.
//!
//! Anything added here that calls into PDFium **must** go through
//! [`with_pdfium`] or it will reintroduce the crash.

use parking_lot::Mutex;
use pdfium_render::prelude::{PdfDocument, PdfRenderConfig};

use crate::error::{AppError, AppResult};
use crate::pdf::POINTS_PER_INCH;
use crate::state::pdfium;

/// Serializes every entry into PDFium. See the module docs.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

/// Guard rail: 600 DPI on a tabloid page is already ~90 MP.
const MAX_PIXELS: u64 = 120_000_000;

/// An RGBA raster of one page, plus the size it was rendered at.
pub struct PageRaster {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, row-major, top-down.
    pub rgba: Vec<u8>,
}

/// Runs `f` with an exclusive PDFium session over `bytes`.
///
/// This is the only place a `PdfDocument` is created, so the lock cannot be
/// bypassed by accident. It is not reentrant — never call a public function
/// from this module inside `f`.
fn with_pdfium<T>(
    bytes: &[u8],
    f: impl FnOnce(&PdfDocument<'_>) -> AppResult<T>,
) -> AppResult<T> {
    let _guard = RENDER_LOCK.lock();

    let document = pdfium()?
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(AppError::from)?;

    f(&document)
}

fn scale_for(dpi: f32) -> AppResult<f32> {
    if !(4.0..=2400.0).contains(&dpi) {
        return Err(AppError::InvalidInput(format!(
            "DPI must be between 4 and 2400 (got {dpi})."
        )));
    }
    Ok(dpi / POINTS_PER_INCH)
}

/// Rasterizes one page at the given DPI.
///
/// `include_form_fields` draws interactive widget appearances on top of the
/// page content — wanted for both the on-screen viewer and printing.
pub fn render_page(
    bytes: &[u8],
    page_index: usize,
    dpi: f32,
    include_form_fields: bool,
) -> AppResult<PageRaster> {
    let scale = scale_for(dpi)?;
    let index = u16::try_from(page_index).map_err(|_| AppError::PageOutOfRange(page_index))?;

    with_pdfium(bytes, |document| {
        let page = document
            .pages()
            .get(index)
            .map_err(|_| AppError::PageOutOfRange(page_index))?;

        let projected = (page.width().value * scale).ceil() as u64
            * (page.height().value * scale).ceil() as u64;
        if projected > MAX_PIXELS {
            return Err(AppError::InvalidInput(format!(
                "Rendering page {} at {dpi} DPI would need {projected} pixels, above the {MAX_PIXELS} limit. Lower the DPI.",
                page_index + 1
            )));
        }

        let config = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .render_form_data(include_form_fields)
            .render_annotations(true);

        let bitmap = page.render_with_config(&config).map_err(AppError::from)?;
        let image = bitmap.as_image().into_rgba8();

        Ok(PageRaster {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    })
}

/// Rasterizes a page and encodes it as PNG, for display in the webview.
pub fn render_page_png(
    bytes: &[u8],
    page_index: usize,
    dpi: f32,
    include_form_fields: bool,
) -> AppResult<Vec<u8>> {
    // Deliberately outside the PDFium lock: PNG encoding is pure CPU work on a
    // buffer we already own, so holding the lock through it would serialize
    // compression across every pending page for no reason.
    let raster = render_page(bytes, page_index, dpi, include_form_fields)?;

    let buffer = image::RgbaImage::from_raw(raster.width, raster.height, raster.rgba)
        .ok_or_else(|| AppError::Render("Raster dimensions did not match buffer".into()))?;

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;

    Ok(png)
}

/// Page dimensions in points, as PDFium sees them (rotation applied).
pub fn page_size_points(bytes: &[u8], page_index: usize) -> AppResult<(f32, f32)> {
    let index = u16::try_from(page_index).map_err(|_| AppError::PageOutOfRange(page_index))?;

    with_pdfium(bytes, |document| {
        let page = document
            .pages()
            .get(index)
            .map_err(|_| AppError::PageOutOfRange(page_index))?;

        Ok((page.width().value, page.height().value))
    })
}

pub fn page_count(bytes: &[u8]) -> AppResult<usize> {
    with_pdfium(bytes, |document| Ok(document.pages().len() as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::document;
    use std::sync::Arc;

    fn four_page_pdf() -> Vec<u8> {
        let mut doc = document::blank().expect("blank");
        for _ in 0..3 {
            document::append_document(&mut doc, document::blank().expect("blank")).expect("append");
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("serialize");
        bytes
    }

    /// Regression test for a hard crash (STATUS_ACCESS_VIOLATION, 0xc0000005).
    ///
    /// Opening a multi-page document makes the viewer request one image per
    /// visible page at once. Each arrived on its own thread and entered PDFium
    /// concurrently, corrupting its process-global state and killing the app
    /// rather than failing a request. The fix is `RENDER_LOCK`; this test fails
    /// by aborting the test binary if that lock is ever removed.
    #[test]
    fn concurrent_renders_do_not_crash() {
        if crate::state::init_pdfium(None).is_err() {
            eprintln!("skipping: PDFium library not available in this environment");
            return;
        }

        let bytes = Arc::new(four_page_pdf());

        let workers: Vec<_> = (0..12)
            .map(|i| {
                let bytes = Arc::clone(&bytes);
                std::thread::spawn(move || render_page_png(&bytes, i % 4, 72.0, true))
            })
            .collect();

        for worker in workers {
            let rendered = worker.join().expect("render thread panicked");
            assert!(rendered.is_ok(), "render failed: {:?}", rendered.err());
        }
    }

    #[test]
    fn rejects_absurd_dpi() {
        assert!(scale_for(0.0).is_err());
        assert!(scale_for(10_000.0).is_err());
        assert!(scale_for(144.0).is_ok());
    }
}
