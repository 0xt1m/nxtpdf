/**
 * Application store.
 *
 * The Rust side owns the document; this store holds a *snapshot* of it plus
 * pure UI state (selection, zoom, which panel is open). Every mutating command
 * returns a fresh `DocumentInfo`, so actions here never patch the snapshot by
 * hand — they replace it wholesale. That keeps the two sides from drifting.
 */

import { create } from 'zustand';
import * as ipc from '@/lib/ipc';
import type { DocumentInfo, FormField, NewField } from '@/lib/types';

export type SidePanel = 'fields' | 'design' | 'pages';

interface AppState {
  // --- Document ---
  doc: DocumentInfo | null;
  fields: FormField[];

  // --- UI ---
  currentPage: number;
  selectedPages: number[];
  zoom: number;
  panel: SidePanel;
  busy: boolean;
  error: string | null;
  notice: string | null;
  /** Cleared if the backend reports PDFium failed to load at startup. */
  renderingAvailable: boolean;

  // --- Actions ---
  setError: (message: string | null) => void;
  setNotice: (message: string | null) => void;
  setRenderingAvailable: (available: boolean) => void;
  setPanel: (panel: SidePanel) => void;
  setZoom: (zoom: number) => void;
  setCurrentPage: (index: number) => void;
  togglePageSelection: (index: number, additive: boolean) => void;
  selectAllPages: () => void;
  clearSelection: () => void;

  refresh: () => Promise<void>;
  newDocument: () => Promise<void>;
  openDocument: (path: string) => Promise<void>;
  closeDocument: () => Promise<void>;
  save: () => Promise<void>;
  saveAs: (path: string) => Promise<void>;

  rotatePage: (index: number, degrees: number) => Promise<void>;
  deleteSelectedPages: () => Promise<void>;
  movePage: (from: number, to: number) => Promise<void>;
  appendPdf: (path: string) => Promise<void>;
  extractSelection: (path: string) => Promise<void>;

  setFieldValue: (name: string, value: string) => Promise<void>;
  createField: (field: NewField) => Promise<void>;
  deleteField: (name: string) => Promise<void>;
}

export const MIN_ZOOM = 0.25;
export const MAX_ZOOM = 4;

export const useStore = create<AppState>((set, get) => {
  /**
   * Runs an async action with a busy flag and uniform error capture, so no
   * individual action has to repeat try/catch/finally.
   */
  async function run<T>(action: () => Promise<T>): Promise<T | undefined> {
    set({ busy: true, error: null });
    try {
      return await action();
    } catch (error) {
      set({ error: ipc.errorMessage(error) });
      return undefined;
    } finally {
      set({ busy: false });
    }
  }

  /** Applies a new document snapshot and reloads the field list with it. */
  async function adopt(doc: DocumentInfo) {
    const fields = doc.hasAcroForm ? await ipc.listFormFields() : [];
    const pageCount = doc.pageCount;

    set((state) => ({
      doc,
      fields,
      // Keep the viewport valid after pages are deleted.
      currentPage: Math.min(state.currentPage, Math.max(0, pageCount - 1)),
      selectedPages: state.selectedPages.filter((index) => index < pageCount),
    }));
  }

  return {
    doc: null,
    fields: [],
    currentPage: 0,
    selectedPages: [],
    zoom: 1,
    panel: 'pages',
    busy: false,
    error: null,
    notice: null,
    renderingAvailable: true,

    setError: (message) => set({ error: message }),
    setNotice: (message) => set({ notice: message }),
    setRenderingAvailable: (available) => set({ renderingAvailable: available }),
    setPanel: (panel) => set({ panel }),

    setZoom: (zoom) => set({ zoom: Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom)) }),

    setCurrentPage: (index) => {
      const doc = get().doc;
      if (!doc) return;
      set({ currentPage: Math.min(Math.max(0, index), doc.pageCount - 1) });
    },

    togglePageSelection: (index, additive) =>
      set((state) => {
        if (!additive) {
          // A plain click selects just this page, or clears if already alone.
          const alreadyOnly =
            state.selectedPages.length === 1 && state.selectedPages[0] === index;
          return { selectedPages: alreadyOnly ? [] : [index] };
        }
        return state.selectedPages.includes(index)
          ? { selectedPages: state.selectedPages.filter((i) => i !== index) }
          : { selectedPages: [...state.selectedPages, index].sort((a, b) => a - b) };
      }),

    selectAllPages: () =>
      set((state) => ({
        selectedPages: state.doc
          ? Array.from({ length: state.doc.pageCount }, (_, i) => i)
          : [],
      })),

    clearSelection: () => set({ selectedPages: [] }),

    refresh: async () => {
      await run(async () => {
        const doc = await ipc.documentInfo();
        if (doc) {
          await adopt(doc);
        } else {
          set({ doc: null, fields: [] });
        }
      });
    },

    newDocument: async () => {
      await run(async () => {
        await adopt(await ipc.newDocument());
        set({ currentPage: 0, selectedPages: [] });
      });
    },

    openDocument: async (path) => {
      await run(async () => {
        await adopt(await ipc.openDocument(path));
        set({ currentPage: 0, selectedPages: [], panel: 'pages' });
      });
    },

    closeDocument: async () => {
      await run(async () => {
        await ipc.closeDocument();
        set({ doc: null, fields: [], currentPage: 0, selectedPages: [] });
      });
    },

    save: async () => {
      await run(async () => {
        await adopt(await ipc.saveDocument());
        set({ notice: 'Saved.' });
      });
    },

    saveAs: async (path) => {
      await run(async () => {
        await adopt(await ipc.saveDocumentAs(path));
        set({ notice: 'Saved.' });
      });
    },

    rotatePage: async (index, degrees) => {
      await run(async () => {
        await adopt(await ipc.rotatePage(index, degrees));
      });
    },

    deleteSelectedPages: async () => {
      const { selectedPages } = get();
      if (selectedPages.length === 0) return;

      await run(async () => {
        await adopt(await ipc.deletePages(selectedPages));
        set({ selectedPages: [] });
      });
    },

    movePage: async (from, to) => {
      if (from === to) return;
      await run(async () => {
        await adopt(await ipc.movePage(from, to));
        set({ currentPage: to });
      });
    },

    appendPdf: async (path) => {
      await run(async () => {
        await adopt(await ipc.appendPdf(path));
        set({ notice: 'Pages appended.' });
      });
    },

    extractSelection: async (path) => {
      const { selectedPages } = get();
      if (selectedPages.length === 0) return;

      await run(async () => {
        await ipc.extractPagesToFile(selectedPages, path);
        set({ notice: `Exported ${selectedPages.length} page(s).` });
      });
    },

    setFieldValue: async (name, value) => {
      await run(async () => {
        await adopt(await ipc.setFormField(name, value));
      });
    },

    createField: async (field) => {
      await run(async () => {
        await adopt(await ipc.createFormField(field));
        set({ notice: `Created field "${field.name}".` });
      });
    },

    deleteField: async (name) => {
      await run(async () => {
        await adopt(await ipc.deleteFormField(name));
      });
    },
  };
});
