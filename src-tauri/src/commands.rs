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
use crate::printing;
use crate::printing::types::{PrintJobResult, PrintSettings, PrinterCapabilities, PrinterInfo};
use crate::state::{AppState, DocumentSession};

/// Builds the snapshot the frontend renders from.
fn snapshot(session: &DocumentSession) -> DocumentInfo {
    document::describe(
        &session.doc,
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

#[tauri::command]
pub fn open_document(state: State<'_, AppState>, path: String) -> AppResult<DocumentInfo> {
    let path = PathBuf::from(path);
    let doc = document::open(&path)?;

    let mut guard = state.session.lock();
    let session = guard.insert(DocumentSession::new(doc, Some(path)));
    Ok(snapshot(session))
}

#[tauri::command]
pub fn new_document(state: State<'_, AppState>) -> AppResult<DocumentInfo> {
    let doc = document::blank()?;

    let mut guard = state.session.lock();
    let session = guard.insert(DocumentSession::new(doc, None));
    Ok(snapshot(session))
}

#[tauri::command]
pub fn close_document(state: State<'_, AppState>) {
    *state.session.lock() = None;
}

#[tauri::command]
pub fn document_info(state: State<'_, AppState>) -> AppResult<Option<DocumentInfo>> {
    let mut guard = state.session.lock();
    Ok(guard.as_mut().map(|session| snapshot(session)))
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
