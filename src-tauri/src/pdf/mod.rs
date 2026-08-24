//! PDF domain logic.
//!
//! * [`document`] — opening, saving, and page-tree surgery (lopdf).
//! * [`forms`]    — AcroForm discovery, filling, and field creation (lopdf).
//! * [`render`]   — rasterizing pages to images (PDFium).
//! * [`text`]     — reading and editing the text drawn on a page (lopdf).

pub mod document;
pub mod forms;
pub mod render;
pub mod text;

/// A PDF user-space unit is 1/72 inch.
pub const POINTS_PER_INCH: f32 = 72.0;
