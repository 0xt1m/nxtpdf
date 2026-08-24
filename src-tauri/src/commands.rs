//! The Tauri command surface — the entire contract with the frontend.
//!
//! Every mutating command returns a fresh [`DocumentInfo`] so the UI never has
//! to guess what changed; it just replaces its snapshot. The `revision` field
//! in that snapshot also busts the page-image cache (see `lib.rs`).
//!
//! Tauri runs synchronous commands off the main thread, so the blocking PDF
//! and printing work here never stalls the UI.

use std::path::PathBuf;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::pdf::document::{self, DocumentInfo};
use crate::pdf::forms::{self, FormField, NewField};
use crate::pdf::text::{self, EditOutcome, TextRun};
use crate::printing;
use crate::printing::types::{PrintJobResult, PrintSettings, PrinterCapabilities, PrinterInfo};
use crate::state::{AppState, DocumentId, DocumentSession};

/// Builds the snapshot the frontend renders from.
fn snapshot(session: &DocumentSession) -> DocumentInfo {
    document::describe(
        &session.doc,
        session.id,
        session.display_name(),
        session.path.clone(),
        session.dirty,
        session.revision,
    )
}

/// Runs a mutation, marks the document dirty, and returns the new snapshot.
fn mutate<F>(state: &AppState, change: F) -> AppResult<DocumentInfo>
where
    F: FnOnce(&mut lopdf::Document) -> AppResult<()>,
{
    state.with_document(|session| {
        change(&mut session.doc)?;
        session.touch();
        Ok(snapshot(session))
    })
}

// ---------------------------------------------------------------------------
// Document lifecycle
// ---------------------------------------------------------------------------

/// Opens a file as a new tab, or focuses the tab already showing it.
///
/// Re-opening a path that is already open would give two independent models of
/// one file, whose saves would overwrite each other.
#[tauri::command]
pub fn open_document(state: State<'_, AppState>, path: String) -> AppResult<DocumentInfo> {
    let path = PathBuf::from(path);

    {
        let mut workspace = state.workspace.lock();
        if let Some(existing) = workspace.find_by_path(&path) {
            workspace.activate(existing);
            let session = workspace.by_id_mut(existing).ok_or(AppError::NoDocument)?;
            return Ok(snapshot(session));
        }
    }

    let doc = document::open(&path)?;

    let mut workspace = state.workspace.lock();
    let id = workspace.open(doc, Some(path));
    let session = workspace.by_id_mut(id).ok_or(AppError::NoDocument)?;
    Ok(snapshot(session))
}

#[tauri::command]
pub fn new_document(state: State<'_, AppState>) -> AppResult<DocumentInfo> {
    let doc = document::blank()?;

    let mut workspace = state.workspace.lock();
    let id = workspace.open(doc, None);
    let session = workspace.by_id_mut(id).ok_or(AppError::NoDocument)?;
    Ok(snapshot(session))
}

/// Closes one tab. Returns whichever tab became active, or `None` if that was
/// the last one.
#[tauri::command]
pub fn close_document(
    state: State<'_, AppState>,
    id: DocumentId,
) -> AppResult<Option<DocumentInfo>> {
    let mut workspace = state.workspace.lock();
    let active = workspace.close(id);

    Ok(active.and_then(|id| workspace.by_id_mut(id).map(|session| snapshot(session))))
}

#[tauri::command]
pub fn activate_document(state: State<'_, AppState>, id: DocumentId) -> AppResult<DocumentInfo> {
    let mut workspace = state.workspace.lock();
    if !workspace.activate(id) {
        return Err(AppError::NoDocument);
    }

    let session = workspace.by_id_mut(id).ok_or(AppError::NoDocument)?;
    Ok(snapshot(session))
}

/// Every open tab, in tab order — what the tab bar renders from.
#[tauri::command]
pub fn list_documents(state: State<'_, AppState>) -> AppResult<Vec<DocumentInfo>> {
    let mut workspace = state.workspace.lock();
    Ok(workspace
        .iter_mut()
        .map(|session| snapshot(session))
        .collect())
}

#[tauri::command]
pub fn document_info(state: State<'_, AppState>) -> AppResult<Option<DocumentInfo>> {
    let mut workspace = state.workspace.lock();
    Ok(workspace.active_mut().map(|session| snapshot(session)))
}

#[tauri::command]
pub fn save_document(state: State<'_, AppState>) -> AppResult<DocumentInfo> {
    state.with_document(|session| {
        let path = session.path.clone().ok_or_else(|| {
            AppError::InvalidInput("This document has no location yet — use Save As.".into())
        })?;

        document::save_to_path(&mut session.doc, &path)?;
        session.dirty = false;
        Ok(snapshot(session))
    })
}

#[tauri::command]
pub fn save_document_as(state: State<'_, AppState>, path: String) -> AppResult<DocumentInfo> {
    let path = PathBuf::from(path);

    state.with_document(|session| {
        document::save_to_path(&mut session.doc, &path)?;
        session.path = Some(path.clone());
        session.dirty = false;
        Ok(snapshot(session))
    })
}

// ---------------------------------------------------------------------------
// Page operations
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn rotate_page(
    state: State<'_, AppState>,
    index: usize,
    degrees: i64,
) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| document::rotate_page(doc, index, degrees))
}

#[tauri::command]
pub fn delete_pages(state: State<'_, AppState>, indices: Vec<usize>) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| document::delete_pages(doc, &indices))
}

#[tauri::command]
pub fn move_page(state: State<'_, AppState>, from: usize, to: usize) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| document::move_page(doc, from, to))
}

#[tauri::command]
pub fn reorder_pages(state: State<'_, AppState>, order: Vec<usize>) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| document::reorder_pages(doc, &order))
}

/// Appends every page of another PDF to the open document.
#[tauri::command]
pub fn append_pdf(state: State<'_, AppState>, path: String) -> AppResult<DocumentInfo> {
    let incoming = document::open(&PathBuf::from(path))?;
    mutate(&state, move |doc| document::append_document(doc, incoming))
}

/// Writes the selected pages out as a new PDF, leaving the open document alone.
#[tauri::command]
pub fn extract_pages_to_file(
    state: State<'_, AppState>,
    indices: Vec<usize>,
    path: String,
) -> AppResult<()> {
    let destination = PathBuf::from(path);

    state.with_document(|session| {
        let mut extracted = document::extract_pages(&session.doc, &indices)?;
        document::save_to_path(&mut extracted, &destination)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Page text
// ---------------------------------------------------------------------------

/// Lists the editable text drawn on a page.
#[tauri::command]
pub fn list_text_runs(state: State<'_, AppState>, page_index: usize) -> AppResult<Vec<TextRun>> {
    state.with_document(|session| text::list_text_runs(&session.doc, page_index))
}

/// The result of editing page text, so the UI can say when the original font
/// could not be kept.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub document: DocumentInfo,
    pub outcome: EditOutcome,
}

#[tauri::command]
pub fn set_text_run(
    state: State<'_, AppState>,
    page_index: usize,
    run_id: usize,
    text: String,
) -> AppResult<TextEdit> {
    let mut workspace = state.workspace.lock();
    let session = workspace.active_mut().ok_or(AppError::NoDocument)?;

    let outcome = text::set_text_run(&mut session.doc, page_index, run_id, &text)?;
    session.touch();

    Ok(TextEdit {
        document: snapshot(session),
        outcome,
    })
}

// ---------------------------------------------------------------------------
// Forms
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_form_fields(state: State<'_, AppState>) -> AppResult<Vec<FormField>> {
    state.with_document(|session| Ok(forms::list_fields(&session.doc)))
}

#[tauri::command]
pub fn set_form_field(
    state: State<'_, AppState>,
    name: String,
    value: String,
) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| forms::set_field_value(doc, &name, &value))
}

#[tauri::command]
pub fn create_form_field(state: State<'_, AppState>, field: NewField) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| forms::create_field(doc, &field))
}

/// Moves or resizes a field on its page. `rect` is `[x0, y0, x1, y1]` in PDF
/// user space, origin bottom-left.
#[tauri::command]
pub fn set_form_field_rect(
    state: State<'_, AppState>,
    name: String,
    rect: [f32; 4],
) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| forms::set_field_rect(doc, &name, rect))
}

/// Sets a field's text size in points. `0` selects auto-sizing, where the
/// viewer shrinks the text to fit the box.
#[tauri::command]
pub fn set_form_field_font_size(
    state: State<'_, AppState>,
    name: String,
    size: f32,
) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| forms::set_field_font_size(doc, &name, size))
}

/// Renames a field. `new_name` replaces the field's own name segment; any
/// parent prefix is preserved.
#[tauri::command]
pub fn rename_form_field(
    state: State<'_, AppState>,
    name: String,
    new_name: String,
) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| forms::rename_field(doc, &name, &new_name))
}

#[tauri::command]
pub fn delete_form_field(state: State<'_, AppState>, name: String) -> AppResult<DocumentInfo> {
    mutate(&state, |doc| forms::delete_field(doc, &name))
}

// ---------------------------------------------------------------------------
// Windows integration
// ---------------------------------------------------------------------------

/// Opens the Windows "Default apps" settings page.
///
/// Since Windows 10, an application cannot make itself the default handler for
/// a file type — that would let any installer hijack every extension. The
/// association is registered by the installer, but the *choice* has to be made
/// by the user in Settings, so the most an app can do is take them there.
#[tauri::command]
pub fn open_default_apps_settings() -> AppResult<()> {
    #[cfg(windows)]
    {
        // The empty string is `start`'s title argument; without it a quoted
        // target would be treated as the window title and never launched.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:defaultapps"])
            .spawn()
            .map_err(|error| {
                AppError::InvalidInput(format!("Could not open Windows Settings: {error}"))
            })?;
        Ok(())
    }

    #[cfg(not(windows))]
    Err(AppError::InvalidInput(
        "Default-app settings are only available on Windows.".into(),
    ))
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_printers() -> AppResult<Vec<PrinterInfo>> {
    printing::list_printers()
}

#[tauri::command]
pub fn default_printer() -> Option<String> {
    printing::default_printer_name()
}

#[tauri::command]
pub fn printer_capabilities(printer_name: String) -> AppResult<PrinterCapabilities> {
    printing::capabilities(&printer_name)
}

/// Submits the open document to the spooler with the given settings.
#[tauri::command]
pub fn print_document(
    state: State<'_, AppState>,
    settings: PrintSettings,
) -> AppResult<PrintJobResult> {
    // Serialize the current in-memory state so unsaved edits and filled form
    // values reach the printer.
    let bytes = state.with_document(|session| Ok(session.bytes()?.to_vec()))?;
    printing::print_document(&bytes, &settings)
}
