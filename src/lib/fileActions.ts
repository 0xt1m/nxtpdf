/**
 * File actions shared by the toolbar and the keyboard shortcuts, so the two
 * cannot drift apart — Ctrl+S and the Save button must behave identically.
 */

import {
  open as openFileDialog,
  save as saveFileDialog,
} from '@tauri-apps/plugin-dialog';
import { useStore } from '@/state/store';

export const PDF_FILTER = [{ name: 'PDF Document', extensions: ['pdf'] }];

/**
 * Saves the document, falling back to Save As when it has no path yet
 * (a document created in-app, or one that has never been written).
 */
export async function saveDocument(): Promise<void> {
  const { doc, save } = useStore.getState();
  if (!doc) return;

  if (doc.path) {
    await save();
    return;
  }
  await saveDocumentAs();
}

export async function saveDocumentAs(): Promise<void> {
  const { doc, saveAs } = useStore.getState();
  if (!doc) return;

  const path = await saveFileDialog({
    filters: PDF_FILTER,
    defaultPath: doc.name || 'Untitled.pdf',
  });
  if (path) await saveAs(path);
}

export async function openDocument(): Promise<void> {
  const path = await openFileDialog({ multiple: false, filters: PDF_FILTER });
  if (typeof path === 'string') {
    await useStore.getState().openDocument(path);
  }
}

export async function appendDocument(): Promise<void> {
  const path = await openFileDialog({ multiple: false, filters: PDF_FILTER });
  if (typeof path === 'string') {
    await useStore.getState().appendPdf(path);
  }
}

export async function extractSelection(): Promise<void> {
  const { selectedPages, extractSelection: extract } = useStore.getState();
  if (selectedPages.length === 0) return;

  const path = await saveFileDialog({
    filters: PDF_FILTER,
    defaultPath: 'Extracted.pdf',
  });
  if (path) await extract(path);
}
