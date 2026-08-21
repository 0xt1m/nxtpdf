/**
 * Typed wrappers over the Tauri command surface.
 *
 * Nothing else in the app calls `invoke` directly — this module is the single
 * place where a command name is spelled out, so a rename on the Rust side has
 * exactly one place to be fixed.
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  DocumentInfo,
  FormField,
  NewField,
  PrintJobResult,
  PrintSettings,
  PrinterCapabilities,
  PrinterInfo,
} from './types';

// ---------------------------------------------------------------------------
// Document lifecycle
// ---------------------------------------------------------------------------

export const openDocument = (path: string) =>
  invoke<DocumentInfo>('open_document', { path });

export const newDocument = () => invoke<DocumentInfo>('new_document');

export const closeDocument = () => invoke<void>('close_document');

export const documentInfo = () => invoke<DocumentInfo | null>('document_info');

export const saveDocument = () => invoke<DocumentInfo>('save_document');

export const saveDocumentAs = (path: string) =>
  invoke<DocumentInfo>('save_document_as', { path });

// ---------------------------------------------------------------------------
// Page operations
// ---------------------------------------------------------------------------

export const rotatePage = (index: number, degrees: number) =>
  invoke<DocumentInfo>('rotate_page', { index, degrees });

export const deletePages = (indices: number[]) =>
  invoke<DocumentInfo>('delete_pages', { indices });

export const movePage = (from: number, to: number) =>
  invoke<DocumentInfo>('move_page', { from, to });

export const reorderPages = (order: number[]) =>
  invoke<DocumentInfo>('reorder_pages', { order });

export const appendPdf = (path: string) => invoke<DocumentInfo>('append_pdf', { path });

export const extractPagesToFile = (indices: number[], path: string) =>
  invoke<void>('extract_pages_to_file', { indices, path });

// ---------------------------------------------------------------------------
// Forms
// ---------------------------------------------------------------------------

export const listFormFields = () => invoke<FormField[]>('list_form_fields');

export const setFormField = (name: string, value: string) =>
  invoke<DocumentInfo>('set_form_field', { name, value });

export const createFormField = (field: NewField) =>
  invoke<DocumentInfo>('create_form_field', { field });

export const renameFormField = (name: string, newName: string) =>
  invoke<DocumentInfo>('rename_form_field', { name, newName });

export const deleteFormField = (name: string) =>
  invoke<DocumentInfo>('delete_form_field', { name });

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

export const listPrinters = () => invoke<PrinterInfo[]>('list_printers');

export const defaultPrinter = () => invoke<string | null>('default_printer');

export const printerCapabilities = (printerName: string) =>
  invoke<PrinterCapabilities>('printer_capabilities', { printerName });

export const printDocument = (settings: PrintSettings) =>
  invoke<PrintJobResult>('print_document', { settings });

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Rust command errors arrive as plain strings (see `AppError`'s Serialize
 * impl). Anything else is unexpected, so fall back to a readable form rather
 * than showing `[object Object]`.
 */
export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
