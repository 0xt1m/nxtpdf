//! Unified error type.
//!
//! Every Tauri command returns [`AppResult`]. The error serializes to a plain
//! string so the frontend gets a readable message instead of an opaque object.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("No document is open.")]
    NoDocument,

    #[error("Page {0} does not exist in this document.")]
    PageOutOfRange(usize),

    #[error("Form field \"{0}\" was not found.")]
    FieldNotFound(String),

    #[error("This document has no form fields.")]
    NoAcroForm,

    #[error("{0}")]
    InvalidInput(String),

    #[error("PDF error: {0}")]
    Pdf(#[from] lopdf::Error),

    #[error("Render error: {0}")]
    Render(String),

    #[error("PDFium is unavailable: {0}")]
    PdfiumUnavailable(String),

    #[error("Printing error: {0}")]
    Print(String),

    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("File error: {0}")]
    Io(#[from] std::io::Error),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Converts a `pdfium_render::PdfiumError` without depending on its exact shape.
impl From<pdfium_render::prelude::PdfiumError> for AppError {
    fn from(err: pdfium_render::prelude::PdfiumError) -> Self {
        AppError::Render(format!("{err:?}"))
    }
}
