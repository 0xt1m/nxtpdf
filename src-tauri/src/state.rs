//! Application state: the open documents, plus the PDFium binding.
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

/// Identifies one open document for the lifetime of the process.
///
/// Tabs are addressed by id rather than by index so that closing a tab cannot
/// silently re-point a pending request — a page image still in flight for tab
/// 3 must not come back holding whatever moved into slot 3.
pub type DocumentId = u64;

/// One open document and everything we track about it.
pub struct DocumentSession {
    pub id: DocumentId,
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
    fn new(id: DocumentId, doc: lopdf::Document, path: Option<PathBuf>) -> Self {
        Self {
            id,
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

    /// Returns the document serialized to PDF bytes **for rendering and
    /// printing**, reusing the cache when nothing has changed since the last
    /// call.
    ///
    /// Field appearances are flattened into the page content first. PDFium
    /// leaves widget annotations to its form-fill module and draws nothing for
    /// them here, so without this a filled form renders and prints blank. The
    /// flattening happens on a copy — [`Self::doc`], and therefore every saved
    /// file, keeps its live editable fields.
    pub fn bytes(&mut self) -> AppResult<&[u8]> {
        let needs_refresh = !matches!(&self.serialized, Some((rev, _)) if *rev == self.revision);

        if needs_refresh {
            let mut flattened = self.doc.clone();
            crate::pdf::forms::flatten_appearances(&mut flattened);

            let mut buffer = Vec::new();
            flattened.save_to(&mut buffer)?;
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

/// Every open document, in tab order, and which one is showing.
#[derive(Default)]
pub struct Workspace {
    open: Vec<DocumentSession>,
    active: Option<DocumentId>,
    next_id: DocumentId,
}

impl Workspace {
    /// Adds a document as a new tab and makes it active. Returns its id.
    pub fn open(&mut self, doc: lopdf::Document, path: Option<PathBuf>) -> DocumentId {
        self.next_id += 1;
        let id = self.next_id;

        self.open.push(DocumentSession::new(id, doc, path));
        self.active = Some(id);
        id
    }

    /// Finds the tab already showing `path`, if any.
    ///
    /// Opening the same file twice would give two independent models of one
    /// file on disk, whose saves would silently overwrite each other.
    pub fn find_by_path(&self, path: &Path) -> Option<DocumentId> {
        self.open
            .iter()
            .find(|session| session.path.as_deref() == Some(path))
            .map(|session| session.id)
    }

    pub fn activate(&mut self, id: DocumentId) -> bool {
        if self.open.iter().any(|session| session.id == id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    /// Closes a tab and returns the id that became active, if any.
    pub fn close(&mut self, id: DocumentId) -> Option<DocumentId> {
        let Some(index) = self.open.iter().position(|session| session.id == id) else {
            return self.active;
        };

        self.open.remove(index);

        if self.active == Some(id) {
            // Focus the neighbour on the right, or the new last tab — the same
            // thing every tabbed editor does.
            self.active = self
                .open
                .get(index)
                .or_else(|| self.open.last())
                .map(|session| session.id);
        }

        self.active
    }

    pub fn active_id(&self) -> Option<DocumentId> {
        self.active
    }

    pub fn active_mut(&mut self) -> Option<&mut DocumentSession> {
        let id = self.active?;
        self.by_id_mut(id)
    }

    pub fn by_id_mut(&mut self, id: DocumentId) -> Option<&mut DocumentSession> {
        self.open.iter_mut().find(|session| session.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &DocumentSession> {
        self.open.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut DocumentSession> {
        self.open.iter_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// Global application state registered with Tauri.
#[derive(Default)]
pub struct AppState {
    pub workspace: Mutex<Workspace>,
}

impl AppState {
    /// Runs `f` against the active document, or fails with [`AppError::NoDocument`].
    pub fn with_document<T>(
        &self,
        f: impl FnOnce(&mut DocumentSession) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut workspace = self.workspace.lock();
        let session = workspace.active_mut().ok_or(AppError::NoDocument)?;
        f(session)
    }

    /// Runs `f` against a specific tab, whether or not it is the active one.
    pub fn with_document_id<T>(
        &self,
        id: DocumentId,
        f: impl FnOnce(&mut DocumentSession) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut workspace = self.workspace.lock();
        let session = workspace.by_id_mut(id).ok_or(AppError::NoDocument)?;
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
/// binding avoids threading lifetimes through every render call. See
/// `pdf::render` for why every call into it is serialized.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::document::blank;

    fn workspace_with(count: usize) -> (Workspace, Vec<DocumentId>) {
        let mut workspace = Workspace::default();
        let ids = (0..count)
            .map(|i| {
                workspace.open(
                    blank().expect("blank"),
                    Some(PathBuf::from(format!("doc{i}.pdf"))),
                )
            })
            .collect();
        (workspace, ids)
    }

    #[test]
    fn opening_activates_the_new_tab() {
        let (workspace, ids) = workspace_with(3);
        assert_eq!(workspace.active_id(), Some(ids[2]));
    }

    #[test]
    fn ids_are_unique_and_never_reused() {
        let (mut workspace, ids) = workspace_with(2);
        workspace.close(ids[1]);
        let fresh = workspace.open(blank().unwrap(), None);
        assert!(!ids.contains(&fresh));
    }

    #[test]
    fn closing_the_active_tab_focuses_its_right_neighbour() {
        let (mut workspace, ids) = workspace_with(3);
        workspace.activate(ids[1]);

        assert_eq!(workspace.close(ids[1]), Some(ids[2]));
    }

    #[test]
    fn closing_the_last_tab_focuses_the_new_last() {
        let (mut workspace, ids) = workspace_with(3);
        assert_eq!(workspace.close(ids[2]), Some(ids[1]));
    }

    #[test]
    fn closing_an_inactive_tab_leaves_focus_alone() {
        let (mut workspace, ids) = workspace_with(3);
        workspace.activate(ids[0]);
        assert_eq!(workspace.close(ids[2]), Some(ids[0]));
    }

    #[test]
    fn closing_the_only_tab_leaves_nothing_active() {
        let (mut workspace, ids) = workspace_with(1);
        assert_eq!(workspace.close(ids[0]), None);
        assert!(workspace.is_empty());
    }

    #[test]
    fn activating_an_unknown_id_is_refused() {
        let (mut workspace, ids) = workspace_with(1);
        assert!(!workspace.activate(9999));
        assert_eq!(workspace.active_id(), Some(ids[0]));
    }

    #[test]
    fn a_path_already_open_is_found() {
        let (workspace, ids) = workspace_with(2);
        assert_eq!(workspace.find_by_path(Path::new("doc0.pdf")), Some(ids[0]));
        assert_eq!(workspace.find_by_path(Path::new("nope.pdf")), None);
    }

    #[test]
    fn an_unsaved_document_matches_no_path() {
        let mut workspace = Workspace::default();
        workspace.open(blank().unwrap(), None);
        assert_eq!(workspace.find_by_path(Path::new("doc0.pdf")), None);
    }
}
