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
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use pdfium_render::prelude::Pdfium;

use crate::error::{AppError, AppResult};

/// Identifies one open document for the lifetime of the process.
///
/// Tabs are addressed by id rather than by index so that closing a tab cannot
/// silently re-point a pending request — a page image still in flight for tab
/// 3 must not come back holding whatever moved into slot 3.
pub type DocumentId = u64;

/// How many steps back the history keeps.
///
/// Each entry is the whole document serialized, so the bound is about memory
/// rather than about how far anyone plausibly wants to go back.
const HISTORY_DEPTH: usize = 40;

/// Consecutive changes of the same kind inside this window become one step.
///
/// Holding an arrow key to slide a field across the page is one movement to
/// the person doing it. Recording forty of them would make Undo useless and
/// would push every earlier edit out of a bounded history.
const COALESCE_WINDOW: Duration = Duration::from_millis(700);

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

    /// States to go back to, oldest first, and states to go forward to.
    ///
    /// Whole-document snapshots rather than a command stack. Every edit here
    /// is a different shape of change - page surgery, form values, content
    /// stream rewrites - and an inverse operation would have to be written and
    /// kept correct for each one. A snapshot is right for all of them by
    /// construction, and the cost is bounded by [`HISTORY_DEPTH`].
    undo: Vec<Vec<u8>>,
    redo: Vec<Vec<u8>>,

    /// The kind and time of the last recorded step, for coalescing.
    last_step: Option<(&'static str, Instant)>,

    /// Where in the history the file on disk sits.
    ///
    /// `undo.len()` identifies a position in a linear history: undoing
    /// decrements it, redoing increments it. Comparing the two is what lets
    /// undoing back to the last save clear the unsaved-changes marker.
    saved_at: Option<usize>,
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
            undo: Vec::new(),
            redo: Vec::new(),
            last_step: None,
            saved_at: Some(0),
        }
    }

    /// Records the current state so the next change can be undone.
    ///
    /// Call this immediately *before* mutating, while the document still holds
    /// what the user would expect to get back.
    pub fn checkpoint(&mut self) {
        self.record(None);
    }

    /// Records a step that merges with an immediately preceding one of the
    /// same kind, so a held key or a repeated gesture undoes in one go.
    pub fn checkpoint_merging(&mut self, kind: &'static str) {
        self.record(Some(kind));
    }

    fn record(&mut self, kind: Option<&'static str>) {
        if let Some(kind) = kind {
            if let Some((last, at)) = self.last_step {
                if last == kind && at.elapsed() < COALESCE_WINDOW {
                    // The snapshot already on the stack is the state this
                    // whole gesture started from, which is what to go back to.
                    self.last_step = Some((kind, Instant::now()));
                    return;
                }
            }
        }

        let mut buffer = Vec::new();
        if let Err(error) = self.doc.save_to(&mut buffer) {
            // Losing one step of history is bad; refusing the edit outright
            // would be worse.
            log::warn!("could not record an undo step: {error}");
            return;
        }

        self.undo.push(buffer);
        self.last_step = kind.map(|kind| (kind, Instant::now()));
        // A new edit abandons whatever was ahead in the history.
        self.redo.clear();

        if self.undo.len() > HISTORY_DEPTH {
            self.undo.remove(0);
            // Everything shifted down, including where the saved state sits.
            self.saved_at = self.saved_at.and_then(|at| at.checked_sub(1));
        }
    }

    /// Steps back one change. Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> AppResult<bool> {
        self.step(true)
    }

    /// Steps forward one change. Returns false when there is nothing to redo.
    pub fn redo(&mut self) -> AppResult<bool> {
        self.step(false)
    }

    fn step(&mut self, backwards: bool) -> AppResult<bool> {
        let Some(target) = (if backwards {
            self.undo.pop()
        } else {
            self.redo.pop()
        }) else {
            return Ok(false);
        };

        // Keep the state being left behind, so the move can be reversed.
        let mut current = Vec::new();
        self.doc.save_to(&mut current)?;

        let restored = lopdf::Document::load_mem(&target)?;

        if backwards {
            self.redo.push(current);
        } else {
            self.undo.push(current);
        }

        self.doc = restored;
        self.revision = self.revision.wrapping_add(1);
        self.serialized = None;
        // Whatever gesture was in progress is over; the next change starts a
        // step of its own rather than merging into the one just restored.
        self.last_step = None;
        self.refresh_dirty();

        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Records that the file on disk now matches this state.
    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.undo.len());
        self.dirty = false;
    }

    fn refresh_dirty(&mut self) {
        self.dirty = self.saved_at != Some(self.undo.len());
    }

    /// Records a mutation. Call this after every edit.
    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.serialized = None;
        self.refresh_dirty();
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

    // --- Undo history ---------------------------------------------------

    use crate::pdf::forms::{self, FieldKind, NewField};

    fn session_with_a_blank_page() -> DocumentSession {
        DocumentSession::new(1, blank().expect("blank"), None)
    }

    /// Adds a named field, recording a history step first, exactly as the
    /// command layer does.
    fn add_field(session: &mut DocumentSession, name: &str) {
        session.checkpoint();
        forms::create_field(
            &mut session.doc,
            &NewField {
                page_index: 0,
                name: name.to_string(),
                kind: FieldKind::Text,
                rect: [72.0, 600.0, 300.0, 620.0],
                font_size: Some(10.0),
                multiline: false,
                required: false,
                max_length: None,
                options: Vec::new(),
            },
        )
        .expect("create");
        session.touch();
    }

    fn field_names(session: &DocumentSession) -> Vec<String> {
        forms::list_fields(&session.doc)
            .into_iter()
            .map(|field| field.name)
            .collect()
    }

    #[test]
    fn undo_restores_the_previous_state() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");
        assert_eq!(field_names(&session), vec!["buyer"]);

        assert!(session.undo().expect("undo"));
        assert!(field_names(&session).is_empty());
    }

    #[test]
    fn undo_walks_back_one_step_at_a_time() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");
        add_field(&mut session, "seller");
        assert_eq!(field_names(&session).len(), 2);

        session.undo().expect("undo");
        assert_eq!(field_names(&session), vec!["buyer"]);

        session.undo().expect("undo");
        assert!(field_names(&session).is_empty());
    }

    #[test]
    fn undo_with_no_history_is_not_an_error() {
        let mut session = session_with_a_blank_page();
        assert!(!session.can_undo());
        assert!(!session.undo().expect("undo"));
    }

    #[test]
    fn redo_puts_the_change_back() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");

        session.undo().expect("undo");
        assert!(session.can_redo());

        assert!(session.redo().expect("redo"));
        assert_eq!(field_names(&session), vec!["buyer"]);
    }

    /// Editing after undoing abandons the branch that was ahead — the standard
    /// behaviour, and the reason redo cannot resurrect stale states.
    #[test]
    fn a_new_edit_discards_what_was_ahead() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");
        session.undo().expect("undo");
        assert!(session.can_redo());

        add_field(&mut session, "seller");
        assert!(!session.can_redo());
        assert_eq!(field_names(&session), vec!["seller"]);
    }

    #[test]
    fn every_step_moves_the_revision_so_page_images_are_refetched() {
        let mut session = session_with_a_blank_page();
        let start = session.revision;

        add_field(&mut session, "buyer");
        let edited = session.revision;
        assert_ne!(edited, start);

        session.undo().expect("undo");
        assert_ne!(session.revision, edited);
    }

    #[test]
    fn undoing_back_to_the_saved_state_clears_the_unsaved_marker() {
        let mut session = session_with_a_blank_page();
        session.mark_saved();

        add_field(&mut session, "buyer");
        assert!(session.dirty);

        session.undo().expect("undo");
        assert!(
            !session.dirty,
            "back at the saved state, so nothing is unsaved"
        );
    }

    #[test]
    fn redoing_away_from_the_saved_state_marks_it_unsaved_again() {
        let mut session = session_with_a_blank_page();
        session.mark_saved();
        add_field(&mut session, "buyer");
        session.undo().expect("undo");

        session.redo().expect("redo");
        assert!(session.dirty);
    }

    #[test]
    fn saving_after_an_undo_makes_that_the_clean_state() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");
        add_field(&mut session, "seller");
        session.undo().expect("undo");

        session.mark_saved();
        assert!(!session.dirty);

        session.undo().expect("undo");
        assert!(session.dirty, "moved away from what was written to disk");
    }

    /// Holding an arrow key is one movement to the person doing it.
    #[test]
    fn repeating_the_same_kind_of_change_is_one_step() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");

        for _ in 0..20 {
            session.checkpoint_merging("move-fields");
            session.touch();
        }

        assert_eq!(session.undo.len(), 2, "the field, then the whole movement");
    }

    #[test]
    fn a_different_kind_of_change_starts_its_own_step() {
        let mut session = session_with_a_blank_page();

        session.checkpoint_merging("move-fields");
        session.touch();
        session.checkpoint_merging("resize-fields");
        session.touch();

        assert_eq!(session.undo.len(), 2);
    }

    /// Otherwise a nudge straight after an undo would merge into the step that
    /// was just restored, and undoing again would skip past it.
    #[test]
    fn undoing_ends_the_gesture_being_merged_into() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "buyer");

        session.checkpoint_merging("move-fields");
        session.touch();
        session.undo().expect("undo");

        session.checkpoint_merging("move-fields");
        session.touch();

        assert_eq!(session.undo.len(), 2);
    }

    #[test]
    fn an_ordinary_checkpoint_never_merges() {
        let mut session = session_with_a_blank_page();

        for _ in 0..5 {
            session.checkpoint();
            session.touch();
        }

        assert_eq!(session.undo.len(), 5);
    }

    #[test]
    fn history_is_bounded() {
        let mut session = session_with_a_blank_page();
        for index in 0..(HISTORY_DEPTH + 10) {
            add_field(&mut session, &format!("field{index}"));
        }

        assert_eq!(session.undo.len(), HISTORY_DEPTH);
    }

    /// Dropping the oldest step shifts every position down, including the one
    /// the saved state is recorded at.
    #[test]
    fn the_saved_position_survives_the_history_filling_up() {
        let mut session = session_with_a_blank_page();
        add_field(&mut session, "first");
        session.mark_saved();

        for index in 0..HISTORY_DEPTH {
            add_field(&mut session, &format!("field{index}"));
        }
        assert!(session.dirty);

        // Walk back to where the file on disk sits.
        while session.can_undo() && session.dirty {
            session.undo().expect("undo");
        }
        assert!(!session.dirty);
        assert_eq!(field_names(&session), vec!["first"]);
    }

    #[test]
    fn an_unsaved_document_matches_no_path() {
        let mut workspace = Workspace::default();
        workspace.open(blank().unwrap(), None);
        assert_eq!(workspace.find_by_path(Path::new("doc0.pdf")), None);
    }
}
