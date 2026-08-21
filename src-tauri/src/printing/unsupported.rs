//! Placeholder printing backend for non-Windows targets.
//!
//! Replacing this with a CUPS implementation means mapping [`PrintSettings`]
//! onto IPP attributes: `sides=one-sided|two-sided-long-edge|two-sided-short-edge`,
//! `print-color-mode=color|monochrome`, and `media-source` for the tray. The
//! job itself can be handed to CUPS as a PDF rather than rasterized, which is
//! both simpler and higher quality than the GDI path.

use crate::error::{AppError, AppResult};
use crate::printing::types::{PrintJobResult, PrintSettings, PrinterCapabilities, PrinterInfo};

const MESSAGE: &str = "Printing is currently implemented for Windows only. \
A CUPS backend is needed for macOS and Linux.";

pub fn default_printer_name() -> Option<String> {
    None
}

pub fn list_printers() -> AppResult<Vec<PrinterInfo>> {
    Ok(Vec::new())
}

pub fn capabilities(_printer_name: &str) -> AppResult<PrinterCapabilities> {
    Err(AppError::Print(MESSAGE.to_string()))
}

pub fn print_document(_pdf_bytes: &[u8], _settings: &PrintSettings) -> AppResult<PrintJobResult> {
    Err(AppError::Print(MESSAGE.to_string()))
}
