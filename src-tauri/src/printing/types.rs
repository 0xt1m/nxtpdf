//! Platform-neutral printing vocabulary shared by the frontend and backend.
//!
//! These types deliberately avoid Win32 spellings so a CUPS backend can
//! implement the same surface later without changing the UI.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterInfo {
    pub name: String,
    pub driver: String,
    pub port: String,
    pub is_default: bool,
    /// Human-readable status ("Ready", "Offline", "Paused", ...).
    pub status: String,
    pub location: String,
    pub comment: String,
    /// Jobs currently queued on the device.
    pub jobs_queued: u32,
}

/// An input tray. `id` is the driver's own bin identifier and must be passed
/// back verbatim to select it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperSource {
    pub id: i16,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperSize {
    pub id: i16,
    pub name: String,
    pub width_mm: f32,
    pub height_mm: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub x_dpi: i32,
    pub y_dpi: i32,
}

/// What a specific printer can actually do, queried from its driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterCapabilities {
    pub printer_name: String,
    pub supports_duplex: bool,
    pub supports_color: bool,
    pub supports_collate: bool,
    /// Maximum copies the driver will accept in one job.
    pub max_copies: i32,
    pub paper_sources: Vec<PaperSource>,
    pub paper_sizes: Vec<PaperSize>,
    pub resolutions: Vec<Resolution>,
    /// The driver's current defaults, suitable for seeding the print dialog.
    pub defaults: PrinterDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDefaults {
    pub duplex: DuplexMode,
    pub color: ColorMode,
    pub paper_source_id: Option<i16>,
    pub paper_size_id: Option<i16>,
    pub orientation: Orientation,
    pub copies: i32,
    pub collate: bool,
}

impl Default for PrinterDefaults {
    fn default() -> Self {
        Self {
            duplex: DuplexMode::Simplex,
            color: ColorMode::Color,
            paper_source_id: None,
            paper_size_id: None,
            orientation: Orientation::Auto,
            copies: 1,
            collate: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Job settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplexMode {
    /// One-sided.
    Simplex,
    /// Two-sided, flipping on the long edge (the usual "book" binding).
    LongEdge,
    /// Two-sided, flipping on the short edge ("tablet" / calendar binding).
    ShortEdge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorMode {
    Color,
    Monochrome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Orientation {
    /// Match each page's own aspect ratio.
    Auto,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageScaling {
    /// Print at true size; oversized pages are clipped.
    ActualSize,
    /// Scale each page up or down to fill the printable area.
    FitToPage,
    /// Scale down only when the page exceeds the printable area.
    ShrinkOversized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintSettings {
    pub printer_name: String,
    /// 0-based page indices, in the order they should print.
    /// `None` prints every page in document order.
    #[serde(default)]
    pub pages: Option<Vec<usize>>,
    #[serde(default = "one")]
    pub copies: i32,
    #[serde(default = "yes")]
    pub collate: bool,
    pub duplex: DuplexMode,
    pub color: ColorMode,
    #[serde(default)]
    pub paper_source_id: Option<i16>,
    #[serde(default)]
    pub paper_size_id: Option<i16>,
    pub orientation: Orientation,
    pub scaling: PageScaling,
    /// Rasterization DPI. `None` follows the device, capped at 300.
    #[serde(default)]
    pub render_dpi: Option<f32>,
    #[serde(default)]
    pub reverse_order: bool,
    /// Name shown in the Windows print queue.
    #[serde(default = "default_job_name")]
    pub job_name: String,
}

fn one() -> i32 {
    1
}

fn yes() -> bool {
    true
}

fn default_job_name() -> String {
    "NXTPDF Document".to_string()
}

impl PrintSettings {
    /// Resolves the page list against a document of `page_count` pages.
    pub fn resolve_pages(&self, page_count: usize) -> Result<Vec<usize>, String> {
        let mut pages = match &self.pages {
            Some(list) if list.is_empty() => return Err("No pages selected to print.".to_string()),
            Some(list) => {
                if let Some(&bad) = list.iter().find(|&&index| index >= page_count) {
                    return Err(format!(
                        "Page {} does not exist (document has {page_count}).",
                        bad + 1
                    ));
                }
                list.clone()
            }
            None => (0..page_count).collect(),
        };

        if self.reverse_order {
            pages.reverse();
        }
        Ok(pages)
    }

    /// Clamps the requested raster DPI against the device's own resolution.
    ///
    /// Rendering above the device DPI wastes memory for no visible gain; the
    /// 300 DPI ceiling keeps a full-page raster near 35 MB rather than 140 MB.
    pub fn effective_dpi(&self, device_dpi: f32) -> f32 {
        const DEFAULT_CAP: f32 = 300.0;
        const HARD_CAP: f32 = 600.0;

        let requested = self.render_dpi.unwrap_or(device_dpi.min(DEFAULT_CAP));
        requested.clamp(72.0, device_dpi.clamp(72.0, HARD_CAP))
    }
}

/// Outcome of a submitted job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintJobResult {
    pub printer_name: String,
    pub pages_printed: usize,
    pub copies: i32,
    /// DPI the pages were actually rasterized at.
    pub render_dpi: f32,
    /// Settings the driver silently refused, so the UI can say so.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> PrintSettings {
        PrintSettings {
            printer_name: "Test".into(),
            pages: None,
            copies: 1,
            collate: true,
            duplex: DuplexMode::Simplex,
            color: ColorMode::Color,
            paper_source_id: None,
            paper_size_id: None,
            orientation: Orientation::Auto,
            scaling: PageScaling::FitToPage,
            render_dpi: None,
            reverse_order: false,
            job_name: "job".into(),
        }
    }

    #[test]
    fn none_means_every_page() {
        assert_eq!(settings().resolve_pages(3).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn reverse_order_flips_the_list() {
        let mut s = settings();
        s.reverse_order = true;
        assert_eq!(s.resolve_pages(3).unwrap(), vec![2, 1, 0]);
    }

    #[test]
    fn explicit_selection_is_preserved_in_order() {
        let mut s = settings();
        s.pages = Some(vec![2, 0]);
        assert_eq!(s.resolve_pages(3).unwrap(), vec![2, 0]);
    }

    #[test]
    fn out_of_range_pages_are_rejected() {
        let mut s = settings();
        s.pages = Some(vec![5]);
        assert!(s.resolve_pages(3).is_err());
    }

    #[test]
    fn empty_selection_is_rejected() {
        let mut s = settings();
        s.pages = Some(vec![]);
        assert!(s.resolve_pages(3).is_err());
    }

    #[test]
    fn dpi_defaults_to_device_capped_at_300() {
        assert_eq!(settings().effective_dpi(600.0), 300.0);
        assert_eq!(settings().effective_dpi(203.0), 203.0);
    }

    #[test]
    fn dpi_never_exceeds_the_device() {
        let mut s = settings();
        s.render_dpi = Some(1200.0);
        assert_eq!(s.effective_dpi(300.0), 300.0);
    }

    #[test]
    fn dpi_has_a_hard_ceiling() {
        let mut s = settings();
        s.render_dpi = Some(2400.0);
        assert_eq!(s.effective_dpi(4800.0), 600.0);
    }
}
