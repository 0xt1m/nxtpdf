//! Application state: the single open document, plus the PDFium binding.
//!
//! # Design
//!
//! `lopdf::Document` is the **single source of truth**. Every edit (page moves,
//! rotation, form values, new fields) mutates that object model.
//!
//! PDFium is used only as a *renderer*. To draw the current — possibly edited —
//! state, we serialize the lopdf model to bytes and hand those to PDFium. The
//! serialized bytes are cached and invalidated by `revision`, so repeated
//! renders of an unchanged document do not re-serialize.
//!
//! This split keeps the two libraries from fighting over ownership, and means a
//! PDFium API change can never corrupt a saved file.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;
use pdfium_render::prelude::Pdfium;

use crate::error::{AppError, AppResult};

/// One open document and everything we track about it.
pub struct DocumentSession {
    /// The authoritative object model.
    pub doc: lopdf::Document,
    /// Where it came from / where Save writes. `None` for a new document.
    pub path: Option<PathBuf>,
    /// Set on any mutation, cleared on save.
    pub dirty: bool,
    /// Bumped on any mutation. Doubles as a cache-busting token for page images.
    pub revision: u64,
    /// Serialized bytes matching `revision`, lazily produced for rendering.
    serialized: Option<(u64, Vec<u8>)>,
}

impl DocumentSession {
    pub fn new(doc: lopdf::Document, path: Option<PathBuf>) -> Self {
        Self {
            doc,
            path,
            dirty: false,
            revision: 1,
            serialized: None,
        }
    }

    /// Records a mutation. Call this after every edit.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.serialized = None;
    }

    /// Returns the document serialized to PDF bytes, reusing the cache when the
    /// document has not changed since the last call.
    pub fn bytes(&mut self) -> AppResult<&[u8]> {
        let needs_refresh = !matches!(&self.serialized, Some((rev, _)) if *rev == self.revision);

        if needs_refresh {
            let mut buffer = Vec::new();
            self.doc.save_to(&mut buffer)?;
            self.serialized = Some((self.revision, buffer));
        }

        Ok(&self
            .serialized
            .as_ref()
            .expect("serialized cache populated above")
            .1)
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.pdf".to_string())
    }
}

/// Global application state registered with Tauri.
#[derive(Default)]
pub struct AppState {
    pub session: Mutex<Option<DocumentSession>>,
}

impl AppState {
    /// Runs `f` against the open session, or fails with [`AppError::NoDocument`].
    pub fn with_document<T>(
        &self,
        f: impl FnOnce(&mut DocumentSession) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut guard = self.session.lock();
        let session = guard.as_mut().ok_or(AppError::NoDocument)?;
        f(session)
    }
}

// ---------------------------------------------------------------------------
// PDFium binding
// ---------------------------------------------------------------------------

/// PDFium is a single native library loaded once for the process lifetime.
///
/// It is deliberately a `OnceLock` rather than Tauri-managed state: the
/// `pdfium-render` types borrow from the `Pdfium` instance, and a `'static`
/// binding avoids threading lifetimes through every render call. The
/// `thread_safe` feature serializes access internally.
static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

/// Search order for the native library, most specific first.
fn candidate_paths(resource_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // 1. Next to the bundled executable (production installs).
    if let Some(dir) = resource_dir {
        candidates.push(dir.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
        }
    }

    // 2. The dev-time location populated by `pnpm setup:pdfium`.
    candidates.push(PathBuf::from("./lib"));
    candidates.push(PathBuf::from("./src-tauri/lib"));
    candidates.push(PathBuf::from("../src-tauri/lib"));

    candidates
}

/// Binds PDFium, searching bundled and development locations before falling
/// back to whatever the system linker can find.
pub fn init_pdfium(resource_dir: Option<&Path>) -> Result<(), String> {
    if PDFIUM.get().is_some() {
        return Ok(());
    }

    let mut attempts = Vec::new();

    for dir in candidate_paths(resource_dir) {
        let lib = Pdfium::pdfium_platform_library_name_at_path(&dir);
        match Pdfium::bind_to_library(&lib) {
            Ok(bindings) => {
                let _ = PDFIUM.set(Pdfium::new(bindings));
                log::info!("PDFium loaded from {}", dir.display());
                return Ok(());
            }
            Err(err) => attempts.push(format!("  {} -> {err:?}", dir.display())),
        }
    }

    match Pdfium::bind_to_system_library() {
        Ok(bindings) => {
            let _ = PDFIUM.set(Pdfium::new(bindings));
            log::info!("PDFium loaded from the system library path");
            Ok(())
        }
        Err(err) => {
            attempts.push(format!("  <system library path> -> {err:?}"));
            Err(format!(
                "Could not load PDFium. Run `pnpm setup:pdfium`.\nTried:\n{}",
                attempts.join("\n")
            ))
        }
    }
}

/// Returns the process-wide PDFium instance.
pub fn pdfium() -> AppResult<&'static Pdfium> {
    PDFIUM.get().ok_or_else(|| {
        AppError::PdfiumUnavailable("PDFium was not initialized at startup.".to_string())
    })
}
