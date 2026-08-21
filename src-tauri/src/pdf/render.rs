//! Page rasterization via PDFium.
//!
//! PDFium is used purely as a renderer. It never owns the document — it is
//! handed the serialized bytes of the current `lopdf` model on each call. See
//! [`crate::state`] for why.

use pdfium_render::prelude::{PdfDocument, PdfRenderConfig};

use crate::error::{AppError, AppResult};
use crate::pdf::POINTS_PER_INCH;
use crate::state::pdfium;

/// Guard rail: 600 DPI on a tabloid page is already ~90 MP.
const MAX_PIXELS: u64 = 120_000_000;

/// An RGBA raster of one page, plus the size it was rendered at.
pub struct PageRaster {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, row-major, top-down.
    pub rgba: Vec<u8>,
}

fn load<'a>(bytes: &'a [u8]) -> AppResult<PdfDocument<'a>> {
    pdfium()?
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(AppError::from)
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
    let document = load(bytes)?;

    let index = u16::try_from(page_index).map_err(|_| AppError::PageOutOfRange(page_index))?;

    let page = document
        .pages()
        .get(index)
        .map_err(|_| AppError::PageOutOfRange(page_index))?;

    let projected =
        (page.width().value * scale).ceil() as u64 * (page.height().value * scale).ceil() as u64;
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
}

/// Rasterizes a page and encodes it as PNG, for display in the webview.
pub fn render_page_png(
    bytes: &[u8],
    page_index: usize,
    dpi: f32,
    include_form_fields: bool,
) -> AppResult<Vec<u8>> {
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
    let document = load(bytes)?;
    let index = u16::try_from(page_index).map_err(|_| AppError::PageOutOfRange(page_index))?;
    let page = document
        .pages()
        .get(index)
        .map_err(|_| AppError::PageOutOfRange(page_index))?;

    Ok((page.width().value, page.height().value))
}

pub fn page_count(bytes: &[u8]) -> AppResult<usize> {
    Ok(load(bytes)?.pages().len() as usize)
}
