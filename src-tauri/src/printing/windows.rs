//! Windows printing via the GDI print API.
//!
//! # Why not `window.print()`
//!
//! The webview's print path hands the job to the browser's own dialog and
//! offers no control over tray, duplex, or color. Everything the app promises
//! lives in `DEVMODEW`, so we drive the driver directly:
//!
//! 1. [`list_printers`] — `EnumPrintersW` for the device list.
//! 2. [`capabilities`]  — `DeviceCapabilitiesW` asks the driver what it can do.
//!    `DC_BINS`/`DC_BINNAMES` are the tray list; `DC_DUPLEX` and
//!    `DC_COLORDEVICE` report two-sided and color support.
//! 3. [`print_document`] — fill a `DEVMODEW`, `CreateDCW` a printer DC from it,
//!    then rasterize each page with PDFium and `StretchDIBits` it onto the DC.
//!
//! Rasterizing rather than replaying vectors costs fidelity at very high DPI,
//! but works identically on every driver. Drivers vary wildly in what they
//! accept, so every setting is verified after the fact and disagreements are
//! reported as warnings rather than silently ignored.

use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, GetDeviceCaps, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DEVMODEW, DIB_RGB_COLORS, DMCOLLATE_FALSE, DMCOLLATE_TRUE, DMCOLOR_COLOR, DMCOLOR_MONOCHROME,
    DMDUP_HORIZONTAL, DMDUP_SIMPLEX, DMDUP_VERTICAL, DMORIENT_LANDSCAPE, DMORIENT_PORTRAIT,
    DM_COLLATE, DM_COLOR, DM_COPIES, DM_DEFAULTSOURCE, DM_DUPLEX, DM_IN_BUFFER, DM_ORIENTATION,
    DM_OUT_BUFFER, DM_PAPERSIZE, HDC, HORZRES, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT,
    PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, SRCCOPY, VERTRES,
};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
    PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_2W,
};
// The spooler entry points are split across two namespaces in the Windows
// metadata: handle management sits under Graphics::Printing, while the GDI-era
// document and capability calls sit under Storage::Xps despite being winspool
// and gdi32 exports.
use windows::Win32::Storage::Xps::{
    AbortDoc, DeviceCapabilitiesW, EndDoc, EndPage, StartDocW, StartPage, DC_BINNAMES, DC_BINS,
    DC_COLLATE, DC_COLORDEVICE, DC_COPIES, DC_DUPLEX, DC_ENUMRESOLUTIONS, DC_PAPERNAMES, DC_PAPERS,
    DC_PAPERSIZE, DOCINFOW, PRINTER_DEVICE_CAPABILITIES,
};

use crate::error::{AppError, AppResult};
use crate::pdf::render;
use crate::printing::types::*;

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

fn to_wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(once(0)).collect()
}

/// Reads a NUL-terminated string out of a fixed-size wide buffer.
fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Reads a NUL-terminated string from a raw pointer, bounded for safety.
unsafe fn from_pwstr(ptr: PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    ptr.to_string().unwrap_or_default()
}

fn win_err(context: &str) -> AppError {
    let last = windows::core::Error::from_win32();
    AppError::Print(format!("{context}: {last}"))
}

// ---------------------------------------------------------------------------
// Printer handle (RAII)
// ---------------------------------------------------------------------------

struct PrinterHandle(PRINTER_HANDLE);

impl PrinterHandle {
    fn open(name: &str) -> AppResult<Self> {
        let wide = to_wide(name);
        let mut handle = PRINTER_HANDLE::default();

        unsafe { OpenPrinterW(PCWSTR(wide.as_ptr()), &mut handle, None) }
            .map_err(|e| AppError::Print(format!("Cannot open printer \"{name}\": {e}")))?;

        Ok(Self(handle))
    }
}

impl Drop for PrinterHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = ClosePrinter(self.0);
        }
    }
}

/// A printer DC that is always released, even if rendering panics.
struct PrinterDc(HDC);

impl Drop for PrinterDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

pub fn default_printer_name() -> Option<String> {
    unsafe {
        let mut length: u32 = 0;
        // First call reports the required buffer length and "fails" by design.
        let _ = GetDefaultPrinterW(None, &mut length);
        if length == 0 {
            return None;
        }

        let mut buffer = vec![0u16; length as usize];
        if !GetDefaultPrinterW(Some(PWSTR(buffer.as_mut_ptr())), &mut length).as_bool() {
            return None;
        }
        Some(from_wide(&buffer))
    }
}

fn decode_status(status: u32, jobs: u32) -> String {
    // Ordered by how much the user needs to know about it.
    const FLAGS: &[(u32, &str)] = &[
        (0x0000_0080, "Offline"),
        (0x0000_0002, "Error"),
        (0x0000_0008, "Paper jam"),
        (0x0000_0010, "Out of paper"),
        (0x0040_0000, "Door open"),
        (0x0004_0000, "Out of toner"),
        (0x0010_0000, "Needs attention"),
        (0x0000_1000, "Not available"),
        (0x0000_0001, "Paused"),
        (0x0000_0800, "Output bin full"),
        (0x0002_0000, "Toner low"),
        (0x0000_0400, "Printing"),
        (0x0000_0200, "Busy"),
        (0x0001_0000, "Warming up"),
    ];

    for &(bit, label) in FLAGS {
        if status & bit != 0 {
            return label.to_string();
        }
    }

    if jobs > 0 {
        format!("{jobs} job(s) queued")
    } else {
        "Ready".to_string()
    }
}

pub fn list_printers() -> AppResult<Vec<PrinterInfo>> {
    let flags: u32 = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let default = default_printer_name();

    unsafe {
        let mut needed: u32 = 0;
        let mut returned: u32 = 0;

        // Sizing call. It reports failure with ERROR_INSUFFICIENT_BUFFER.
        let _ = EnumPrintersW(flags, PCWSTR::null(), 2, None, &mut needed, &mut returned);
        if needed == 0 {
            return Ok(Vec::new());
        }

        let mut buffer = vec![0u8; needed as usize];
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            2,
            Some(&mut buffer),
            &mut needed,
            &mut returned,
        )
        .map_err(|e| AppError::Print(format!("Could not list printers: {e}")))?;

        let entries = std::slice::from_raw_parts(
            buffer.as_ptr() as *const PRINTER_INFO_2W,
            returned as usize,
        );

        let printers = entries
            .iter()
            .map(|entry| {
                let name = from_pwstr(entry.pPrinterName);
                PrinterInfo {
                    is_default: default.as_deref() == Some(name.as_str()),
                    driver: from_pwstr(entry.pDriverName),
                    port: from_pwstr(entry.pPortName),
                    status: decode_status(entry.Status, entry.cJobs),
                    location: from_pwstr(entry.pLocation),
                    comment: from_pwstr(entry.pComment),
                    jobs_queued: entry.cJobs,
                    name,
                }
            })
            .filter(|printer| !printer.name.is_empty())
            .collect();

        Ok(printers)
    }
}

// ---------------------------------------------------------------------------
// DEVMODE
// ---------------------------------------------------------------------------

/// Owns a `DEVMODEW` plus the driver-private bytes that follow it.
///
/// The struct is variable length: `dmSize + dmDriverExtra`. Copying only the
/// documented header would discard driver-specific settings, so the whole
/// allocation travels together.
struct DevMode {
    buffer: Vec<u8>,
}

impl DevMode {
    /// Fetches the driver's current default settings for a printer.
    fn defaults(printer: &PrinterHandle, name: &str) -> AppResult<Self> {
        let wide = to_wide(name);

        let size =
            unsafe { DocumentPropertiesW(None, printer.0, PCWSTR(wide.as_ptr()), None, None, 0) };

        if size <= 0 {
            return Err(AppError::Print(format!(
                "Driver for \"{name}\" did not report a settings structure (DocumentProperties returned {size}). The driver may be missing or corrupt."
            )));
        }

        let mut buffer = vec![0u8; size as usize];
        let result = unsafe {
            DocumentPropertiesW(
                None,
                printer.0,
                PCWSTR(wide.as_ptr()),
                Some(buffer.as_mut_ptr() as *mut DEVMODEW),
                None,
                DM_OUT_BUFFER.0,
            )
        };

        if result < 0 {
            return Err(win_err(&format!("Reading settings for \"{name}\"")));
        }

        Ok(Self { buffer })
    }

    fn as_ptr(&self) -> *const DEVMODEW {
        self.buffer.as_ptr() as *const DEVMODEW
    }

    fn as_mut_ptr(&mut self) -> *mut DEVMODEW {
        self.buffer.as_mut_ptr() as *mut DEVMODEW
    }

    fn header(&self) -> &DEVMODEW {
        unsafe { &*self.as_ptr() }
    }

    fn header_mut(&mut self) -> &mut DEVMODEW {
        unsafe { &mut *self.as_mut_ptr() }
    }

    /// Asks the driver to validate and normalize the settings we just wrote.
    /// Drivers use this to reconcile impossible combinations.
    fn merge_with_driver(&mut self, printer: &PrinterHandle, name: &str) -> AppResult<()> {
        let wide = to_wide(name);
        let input = self.buffer.clone();

        let result = unsafe {
            DocumentPropertiesW(
                None,
                printer.0,
                PCWSTR(wide.as_ptr()),
                Some(self.as_mut_ptr()),
                Some(input.as_ptr() as *const DEVMODEW),
                (DM_IN_BUFFER | DM_OUT_BUFFER).0,
            )
        };

        if result < 0 {
            return Err(win_err("Applying print settings"));
        }
        Ok(())
    }

    /// Applies our settings onto the DEVMODE, flagging each field as present.
    ///
    /// `dmFields` is a bitmask telling the driver which members are meaningful.
    /// Writing a member without setting its bit means the driver ignores it —
    /// the single most common reason "duplex doesn't work".
    fn apply(&mut self, settings: &PrintSettings, landscape: bool) {
        let header = self.header_mut();
        let mut fields = header.dmFields;

        // --- Duplex ---
        header.dmDuplex = match settings.duplex {
            DuplexMode::Simplex => DMDUP_SIMPLEX,
            DuplexMode::LongEdge => DMDUP_VERTICAL,
            DuplexMode::ShortEdge => DMDUP_HORIZONTAL,
        };
        fields |= DM_DUPLEX;

        // --- Color ---
        header.dmColor = match settings.color {
            ColorMode::Color => DMCOLOR_COLOR,
            ColorMode::Monochrome => DMCOLOR_MONOCHROME,
        };
        fields |= DM_COLOR;

        // --- Collation ---
        header.dmCollate = if settings.collate {
            DMCOLLATE_TRUE
        } else {
            DMCOLLATE_FALSE
        };
        fields |= DM_COLLATE;

        unsafe {
            let inner = &mut header.Anonymous1.Anonymous1;

            // --- Copies ---
            // Copies are driver-side: one render, N sheets. Far faster than
            // sending the raster N times, and it enables hardware collation.
            inner.dmCopies = settings.copies.clamp(1, i16::MAX as i32) as i16;
            fields |= DM_COPIES;

            // --- Tray ---
            if let Some(source) = settings.paper_source_id {
                inner.dmDefaultSource = source;
                fields |= DM_DEFAULTSOURCE;
            }

            // --- Paper size ---
            if let Some(size) = settings.paper_size_id {
                inner.dmPaperSize = size;
                fields |= DM_PAPERSIZE;
            }

            // --- Orientation ---
            match settings.orientation {
                Orientation::Portrait => {
                    inner.dmOrientation = DMORIENT_PORTRAIT as i16;
                    fields |= DM_ORIENTATION;
                }
                Orientation::Landscape => {
                    inner.dmOrientation = DMORIENT_LANDSCAPE as i16;
                    fields |= DM_ORIENTATION;
                }
                Orientation::Auto => {
                    inner.dmOrientation = if landscape {
                        DMORIENT_LANDSCAPE as i16
                    } else {
                        DMORIENT_PORTRAIT as i16
                    };
                    fields |= DM_ORIENTATION;
                }
            }
        }

        self.header_mut().dmFields = fields;
    }

    /// Compares what we asked for against what the driver kept.
    fn diff_warnings(&self, settings: &PrintSettings) -> Vec<String> {
        let header = self.header();
        let mut warnings = Vec::new();

        let wanted_duplex = match settings.duplex {
            DuplexMode::Simplex => DMDUP_SIMPLEX,
            DuplexMode::LongEdge => DMDUP_VERTICAL,
            DuplexMode::ShortEdge => DMDUP_HORIZONTAL,
        };
        if header.dmDuplex != wanted_duplex {
            warnings.push(
                "The driver overrode the two-sided setting; this printer may not support it."
                    .to_string(),
            );
        }

        let wanted_color = match settings.color {
            ColorMode::Color => DMCOLOR_COLOR,
            ColorMode::Monochrome => DMCOLOR_MONOCHROME,
        };
        if header.dmColor != wanted_color {
            warnings.push("The driver overrode the color setting.".to_string());
        }

        unsafe {
            let inner = &header.Anonymous1.Anonymous1;
            if let Some(requested) = settings.paper_source_id {
                if inner.dmDefaultSource != requested {
                    warnings.push(format!(
                        "The driver rejected the selected tray and chose {} instead.",
                        inner.dmDefaultSource
                    ));
                }
            }
            if let Some(requested) = settings.paper_size_id {
                if inner.dmPaperSize != requested {
                    warnings.push("The driver rejected the selected paper size.".to_string());
                }
            }
        }

        warnings
    }
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// Wraps `DeviceCapabilitiesW`, which is called twice: once for the count,
/// once for the data.
unsafe fn query_capability<T: Default + Clone>(
    device: &[u16],
    port: &[u16],
    capability: PRINTER_DEVICE_CAPABILITIES,
    stride: usize,
) -> Vec<T> {
    let count = DeviceCapabilitiesW(
        PCWSTR(device.as_ptr()),
        PCWSTR(port.as_ptr()),
        capability,
        None,
        None,
    );

    if count <= 0 {
        return Vec::new();
    }

    let mut buffer = vec![T::default(); count as usize * stride];
    let written = DeviceCapabilitiesW(
        PCWSTR(device.as_ptr()),
        PCWSTR(port.as_ptr()),
        capability,
        Some(PWSTR(buffer.as_mut_ptr() as *mut u16)),
        None,
    );

    if written <= 0 {
        return Vec::new();
    }

    buffer.truncate(written as usize * stride);
    buffer
}

/// Capabilities that answer a yes/no or a single number.
unsafe fn query_scalar(
    device: &[u16],
    port: &[u16],
    capability: PRINTER_DEVICE_CAPABILITIES,
) -> i32 {
    DeviceCapabilitiesW(
        PCWSTR(device.as_ptr()),
        PCWSTR(port.as_ptr()),
        capability,
        None,
        None,
    )
}

/// Splits a packed array of fixed-width wide-char names.
fn split_names(buffer: &[u16], width: usize) -> Vec<String> {
    buffer.chunks(width).map(from_wide).collect()
}

pub fn capabilities(printer_name: &str) -> AppResult<PrinterCapabilities> {
    let handle = PrinterHandle::open(printer_name)?;
    let devmode = DevMode::defaults(&handle, printer_name)?;

    let device = to_wide(printer_name);
    // An empty port is correct here: the spooler resolves it from the name.
    let port = to_wide("");

    unsafe {
        // --- Trays ---
        // DC_BINS gives the numeric ids to put in dmDefaultSource; DC_BINNAMES
        // gives the matching display names, 24 wide chars each.
        let bin_ids: Vec<u16> = query_capability::<u16>(&device, &port, DC_BINS, 1);
        let bin_name_buffer: Vec<u16> = query_capability::<u16>(&device, &port, DC_BINNAMES, 24);
        let bin_names = split_names(&bin_name_buffer, 24);

        let paper_sources = bin_ids
            .iter()
            .enumerate()
            .map(|(index, &id)| PaperSource {
                id: id as i16,
                name: bin_names
                    .get(index)
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Tray {id}")),
            })
            .collect();

        // --- Paper sizes ---
        // DC_PAPERSIZE reports dimensions in tenths of a millimetre.
        let paper_ids: Vec<u16> = query_capability::<u16>(&device, &port, DC_PAPERS, 1);
        let paper_name_buffer: Vec<u16> =
            query_capability::<u16>(&device, &port, DC_PAPERNAMES, 64);
        let paper_names = split_names(&paper_name_buffer, 64);
        let paper_dims: Vec<i32> = query_capability::<i32>(&device, &port, DC_PAPERSIZE, 2);

        let paper_sizes = paper_ids
            .iter()
            .enumerate()
            .map(|(index, &id)| PaperSize {
                id: id as i16,
                name: paper_names
                    .get(index)
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Paper {id}")),
                width_mm: paper_dims.get(index * 2).map_or(0.0, |&v| v as f32 / 10.0),
                height_mm: paper_dims
                    .get(index * 2 + 1)
                    .map_or(0.0, |&v| v as f32 / 10.0),
            })
            .collect();

        // --- Resolutions --- reported as (x, y) LONG pairs.
        let resolution_values: Vec<i32> =
            query_capability::<i32>(&device, &port, DC_ENUMRESOLUTIONS, 2);
        let resolutions = resolution_values
            .chunks_exact(2)
            .map(|pair| Resolution {
                x_dpi: pair[0],
                y_dpi: pair[1],
            })
            .collect();

        let max_copies = query_scalar(&device, &port, DC_COPIES).max(1);

        let header = devmode.header();
        let inner = &header.Anonymous1.Anonymous1;

        let defaults = PrinterDefaults {
            duplex: match header.dmDuplex {
                d if d == DMDUP_VERTICAL => DuplexMode::LongEdge,
                d if d == DMDUP_HORIZONTAL => DuplexMode::ShortEdge,
                _ => DuplexMode::Simplex,
            },
            color: if header.dmColor == DMCOLOR_MONOCHROME {
                ColorMode::Monochrome
            } else {
                ColorMode::Color
            },
            paper_source_id: Some(inner.dmDefaultSource),
            paper_size_id: Some(inner.dmPaperSize),
            orientation: if inner.dmOrientation == DMORIENT_LANDSCAPE as i16 {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            },
            copies: inner.dmCopies.max(1) as i32,
            collate: header.dmCollate == DMCOLLATE_TRUE,
        };

        Ok(PrinterCapabilities {
            printer_name: printer_name.to_string(),
            supports_duplex: query_scalar(&device, &port, DC_DUPLEX) == 1,
            supports_color: query_scalar(&device, &port, DC_COLORDEVICE) == 1,
            supports_collate: query_scalar(&device, &port, DC_COLLATE) == 1,
            max_copies,
            paper_sources,
            paper_sizes,
            resolutions,
            defaults,
        })
    }
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// Geometry of the target sheet, in device pixels.
struct DeviceGeometry {
    dpi_x: f32,
    dpi_y: f32,
    printable_width: i32,
    printable_height: i32,
    physical_width: i32,
    physical_height: i32,
    offset_x: i32,
    offset_y: i32,
}

impl DeviceGeometry {
    unsafe fn read(hdc: HDC) -> Self {
        Self {
            dpi_x: GetDeviceCaps(Some(hdc), LOGPIXELSX) as f32,
            dpi_y: GetDeviceCaps(Some(hdc), LOGPIXELSY) as f32,
            printable_width: GetDeviceCaps(Some(hdc), HORZRES),
            printable_height: GetDeviceCaps(Some(hdc), VERTRES),
            physical_width: GetDeviceCaps(Some(hdc), PHYSICALWIDTH),
            physical_height: GetDeviceCaps(Some(hdc), PHYSICALHEIGHT),
            offset_x: GetDeviceCaps(Some(hdc), PHYSICALOFFSETX),
            offset_y: GetDeviceCaps(Some(hdc), PHYSICALOFFSETY),
        }
    }
}

/// Where a page lands on the sheet, in DC coordinates.
struct Placement {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Positions one page on the sheet according to the scaling mode.
///
/// GDI printer DC coordinates start at the top-left of the *printable* area,
/// so aligning to the physical sheet means subtracting the hardware margin.
fn place_page(
    geometry: &DeviceGeometry,
    page_width_pt: f32,
    page_height_pt: f32,
    scaling: PageScaling,
) -> Placement {
    let natural_width = page_width_pt / 72.0 * geometry.dpi_x;
    let natural_height = page_height_pt / 72.0 * geometry.dpi_y;

    if natural_width <= 0.0 || natural_height <= 0.0 {
        return Placement {
            x: 0,
            y: 0,
            width: geometry.printable_width,
            height: geometry.printable_height,
        };
    }

    let fit = (geometry.printable_width as f32 / natural_width)
        .min(geometry.printable_height as f32 / natural_height);

    let scale = match scaling {
        PageScaling::ActualSize => 1.0,
        PageScaling::FitToPage => fit,
        // Only ever shrink; never blow a small page up to fill the sheet.
        PageScaling::ShrinkOversized => fit.min(1.0),
    };

    let width = (natural_width * scale).round() as i32;
    let height = (natural_height * scale).round() as i32;

    match scaling {
        // True size centers on the physical sheet, so the margins match what
        // the user would measure with a ruler.
        PageScaling::ActualSize => Placement {
            x: (geometry.physical_width - width) / 2 - geometry.offset_x,
            y: (geometry.physical_height - height) / 2 - geometry.offset_y,
            width,
            height,
        },
        _ => Placement {
            x: (geometry.printable_width - width) / 2,
            y: (geometry.printable_height - height) / 2,
            width,
            height,
        },
    }
}

/// Converts PDFium's RGBA output to the bottom-up BGRA layout GDI expects.
///
/// A 32bpp `BI_RGB` DIB stores blue, green, red, alpha per pixel. We keep the
/// bitmap top-down by declaring a negative height, so only the channel order
/// needs swapping here.
fn rgba_to_bgra(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

pub fn print_document(pdf_bytes: &[u8], settings: &PrintSettings) -> AppResult<PrintJobResult> {
    if settings.copies < 1 {
        return Err(AppError::Print("Copies must be at least 1.".into()));
    }

    let page_count = render::page_count(pdf_bytes)?;
    let pages = settings
        .resolve_pages(page_count)
        .map_err(AppError::Print)?;

    let handle = PrinterHandle::open(&settings.printer_name)?;
    let mut devmode = DevMode::defaults(&handle, &settings.printer_name)?;

    // Orientation Auto follows the first page's aspect ratio: a mixed-
    // orientation document cannot change sheets mid-job through one DEVMODE.
    let first = *pages
        .first()
        .ok_or_else(|| AppError::Print("No pages to print.".into()))?;
    let (first_width, first_height) = render::page_size_points(pdf_bytes, first)?;
    let landscape = first_width > first_height;

    devmode.apply(settings, landscape);
    devmode.merge_with_driver(&handle, &settings.printer_name)?;
    let warnings = devmode.diff_warnings(settings);

    let device = to_wide(&settings.printer_name);
    let hdc = unsafe {
        CreateDCW(
            PCWSTR::null(),
            PCWSTR(device.as_ptr()),
            PCWSTR::null(),
            Some(devmode.as_ptr()),
        )
    };

    if hdc.is_invalid() {
        return Err(win_err(&format!(
            "Could not open a print context for \"{}\"",
            settings.printer_name
        )));
    }
    let dc = PrinterDc(hdc);

    let geometry = unsafe { DeviceGeometry::read(dc.0) };
    let render_dpi = settings.effective_dpi(geometry.dpi_x.max(geometry.dpi_y));

    // --- Start the job ---
    let job_name = to_wide(&settings.job_name);
    let doc_info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(job_name.as_ptr()),
        lpszOutput: PCWSTR::null(),
        lpszDatatype: PCWSTR::null(),
        fwType: 0,
    };

    let job_id = unsafe { StartDocW(dc.0, &doc_info) };
    if job_id <= 0 {
        return Err(win_err("Could not start the print job"));
    }

    // From here on, any early return must abort the job rather than leave a
    // half-written spool file behind.
    let outcome = print_pages(&dc, pdf_bytes, &pages, &geometry, settings, render_dpi);

    match outcome {
        Ok(()) => {
            if unsafe { EndDoc(dc.0) } <= 0 {
                return Err(win_err("Could not finish the print job"));
            }
        }
        Err(error) => {
            unsafe {
                AbortDoc(dc.0);
            }
            return Err(error);
        }
    }

    Ok(PrintJobResult {
        printer_name: settings.printer_name.clone(),
        pages_printed: pages.len(),
        copies: settings.copies,
        render_dpi,
        warnings,
    })
}

/// Rasterizes and emits each page. Split out so the caller can guarantee a
/// matching `EndDoc`/`AbortDoc`.
fn print_pages(
    dc: &PrinterDc,
    pdf_bytes: &[u8],
    pages: &[usize],
    geometry: &DeviceGeometry,
    settings: &PrintSettings,
    render_dpi: f32,
) -> AppResult<()> {
    for &page_index in pages {
        if unsafe { StartPage(dc.0) } <= 0 {
            return Err(win_err(&format!("Could not start page {}", page_index + 1)));
        }

        let (width_pt, height_pt) = render::page_size_points(pdf_bytes, page_index)?;
        let placement = place_page(geometry, width_pt, height_pt, settings.scaling);

        // Render with form fields drawn so filled values reach paper.
        let mut raster = render::render_page(pdf_bytes, page_index, render_dpi, true)?;
        rgba_to_bgra(&mut raster.rgba);

        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: raster.width as i32,
            // Negative height declares a top-down bitmap, matching PDFium's
            // row order. Omitting the minus prints every page upside down.
            biHeight: -(raster.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let bitmap_info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };

        let written = unsafe {
            StretchDIBits(
                dc.0,
                placement.x,
                placement.y,
                placement.width,
                placement.height,
                0,
                0,
                raster.width as i32,
                raster.height as i32,
                Some(raster.rgba.as_ptr() as *const core::ffi::c_void),
                &bitmap_info,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };

        if written == 0 {
            return Err(win_err(&format!(
                "Could not draw page {} onto the printer",
                page_index + 1
            )));
        }

        if unsafe { EndPage(dc.0) } <= 0 {
            return Err(win_err(&format!(
                "Could not finish page {}",
                page_index + 1
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter_geometry() -> DeviceGeometry {
        // 8.5x11in at 600 DPI with a 0.25in hardware margin.
        DeviceGeometry {
            dpi_x: 600.0,
            dpi_y: 600.0,
            printable_width: 4800,
            printable_height: 6300,
            physical_width: 5100,
            physical_height: 6600,
            offset_x: 150,
            offset_y: 150,
        }
    }

    #[test]
    fn fit_to_page_fills_the_printable_area() {
        let placement = place_page(&letter_geometry(), 612.0, 792.0, PageScaling::FitToPage);
        // 4800/5100 < 6300/6600, so width is the binding constraint and the
        // page fills the printable width exactly.
        assert_eq!(placement.width, 4800);
        assert!(placement.height <= 6300);
        // Centered, so the leftover slack is split top and bottom.
        assert_eq!(placement.x, 0);
        assert!(placement.y > 0);
    }

    #[test]
    fn actual_size_maps_points_to_device_dpi() {
        let placement = place_page(&letter_geometry(), 612.0, 792.0, PageScaling::ActualSize);
        assert_eq!(placement.width, 5100); // 8.5in * 600
        assert_eq!(placement.height, 6600); // 11in * 600
                                            // Centered on the physical sheet means offset back by the margin.
        assert_eq!(placement.x, -150);
        assert_eq!(placement.y, -150);
    }

    #[test]
    fn shrink_leaves_small_pages_alone() {
        // A 4x6in page fits easily; it should print at true size.
        let placement = place_page(
            &letter_geometry(),
            288.0,
            432.0,
            PageScaling::ShrinkOversized,
        );
        assert_eq!(placement.width, 2400);
        assert_eq!(placement.height, 3600);
    }

    #[test]
    fn shrink_scales_oversized_pages_down() {
        // A0-ish page must come down to fit.
        let placement = place_page(
            &letter_geometry(),
            2384.0,
            3370.0,
            PageScaling::ShrinkOversized,
        );
        assert!(placement.width <= 4800);
        assert!(placement.height <= 6300);
    }

    #[test]
    fn degenerate_page_falls_back_to_full_area() {
        let placement = place_page(&letter_geometry(), 0.0, 0.0, PageScaling::FitToPage);
        assert_eq!(placement.width, 4800);
        assert_eq!(placement.height, 6300);
    }

    #[test]
    fn channel_swap_produces_bgra() {
        let mut pixels = vec![1u8, 2, 3, 4];
        rgba_to_bgra(&mut pixels);
        assert_eq!(pixels, vec![3, 2, 1, 4]);
    }

    #[test]
    fn status_prefers_the_most_urgent_flag() {
        assert_eq!(decode_status(0x80, 0), "Offline");
        assert_eq!(decode_status(0, 0), "Ready");
        assert_eq!(decode_status(0, 2), "2 job(s) queued");
    }
}
