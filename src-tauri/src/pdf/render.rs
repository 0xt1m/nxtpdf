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
use std::os::raw::c_void;

use pdfium_render::prelude::PdfDocument;

// PDFium constants, declared here rather than imported: their path inside the
// crate's generated bindings moves between PDFium versions, and the values are
// fixed by PDFium's public API.
/// Include annotations — which is what paints form field appearances.
const FPDF_ANNOT: i32 = 1;
/// 32-bit bitmap, byte order blue, green, red, alpha.
const FPDFBITMAP_BGRA: i32 = 4;

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
fn with_pdfium<T>(bytes: &[u8], f: impl FnOnce(&PdfDocument<'_>) -> AppResult<T>) -> AppResult<T> {
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
/// # Why this uses the raw bindings
///
/// `pdfium-render`'s document type initializes PDFium's *form-fill
/// environment* as soon as a document is loaded. Once that exists, PDFium
/// stops painting widget annotations during ordinary page rendering — it
/// assumes the host application will paint them itself by calling
/// `FPDF_FFLDraw`, which in turn only works after a per-page
/// `FORM_OnAfterLoadPage` handshake that the crate never performs and does not
/// expose the handles to perform.
///
/// The result was a filled form rendering completely blank: every field's
/// value present in the file, with correct appearance streams, and none of it
/// drawn — on screen or on paper.
///
/// Loading through the bindings directly means no form-fill environment is
/// ever created, so `FPDF_ANNOT` paints widget appearances the ordinary way.
/// That is also why filled values must have real `/AP` streams; see
/// `pdf::forms`.
pub fn render_page(
    bytes: &[u8],
    page_index: usize,
    dpi: f32,
    include_form_fields: bool,
) -> AppResult<PageRaster> {
    let scale = scale_for(dpi)?;
    let index = i32::try_from(page_index).map_err(|_| AppError::PageOutOfRange(page_index))?;

    let _guard = RENDER_LOCK.lock();
    let bindings = pdfium()?.bindings();

    let document = bindings.FPDF_LoadMemDocument64(bytes, None);
    if document.is_null() {
        return Err(AppError::Render(
            "PDFium could not parse the document".into(),
        ));
    }
    // Every early return from here on must still close the document.
    let result = (|| {
        let page = bindings.FPDF_LoadPage(document, index);
        if page.is_null() {
            return Err(AppError::PageOutOfRange(page_index));
        }

        let width_pt = bindings.FPDF_GetPageWidthF(page);
        let height_pt = bindings.FPDF_GetPageHeightF(page);

        let width = ((width_pt * scale).ceil() as i32).max(1);
        let height = ((height_pt * scale).ceil() as i32).max(1);

        let projected = width as u64 * height as u64;
        if projected > MAX_PIXELS {
            bindings.FPDF_ClosePage(page);
            return Err(AppError::InvalidInput(format!(
                "Rendering page {} at {dpi} DPI would need {projected} pixels, above the {MAX_PIXELS} limit. Lower the DPI.",
                page_index + 1
            )));
        }

        // BGRA, 4 bytes per pixel — PDFium's own layout, converted below.
        let stride = width * 4;
        let mut buffer = vec![0u8; (stride * height) as usize];

        let bitmap = bindings.FPDFBitmap_CreateEx(
            width,
            height,
            FPDFBITMAP_BGRA,
            buffer.as_mut_ptr() as *mut c_void,
            stride,
        );
        if bitmap.is_null() {
            bindings.FPDF_ClosePage(page);
            return Err(AppError::Render("could not allocate a bitmap".into()));
        }

        // Paper is white; without this the page renders onto transparency.
        bindings.FPDFBitmap_FillRect(bitmap, 0, 0, width, height, 0xFFFF_FFFF);

        let flags = if include_form_fields { FPDF_ANNOT } else { 0 };
        bindings.FPDF_RenderPageBitmap(bitmap, page, 0, 0, width, height, 0, flags);

        bindings.FPDFBitmap_Destroy(bitmap);
        bindings.FPDF_ClosePage(page);

        // PDFium hands back BGRA; the rest of the app works in RGBA.
        for pixel in buffer.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        Ok(PageRaster {
            width: width as u32,
            height: height as u32,
            rgba: buffer,
        })
    })();

    bindings.FPDF_CloseDocument(document);
    result
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
