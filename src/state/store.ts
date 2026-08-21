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

/**
 * Which selection the Delete key acts on.
 *
 * Pages and fields are selected independently, so pressing Delete needs to
 * know which one the user last touched — otherwise deleting a field after
 * having selected pages would silently remove the wrong thing.
 */
export type Focus = 'pages' | 'fields' | null;

interface AppState {
  // --- Document ---
  doc: DocumentInfo | null;
  fields: FormField[];

  // --- UI ---
  currentPage: number;
  selectedPages: number[];
  selectedFields: string[];
  focus: Focus;
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
  nudgeZoom: (delta: number) => void;
  setCurrentPage: (index: number) => void;

  selectPage: (index: number, modifiers: SelectionModifiers) => void;
  selectAllPages: () => void;
  clearPageSelection: () => void;
  selectField: (name: string, modifiers: SelectionModifiers) => void;
  clearFieldSelection: () => void;

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
  deleteSelectedFields: () => Promise<void>;

  /** Deletes whatever the user last selected. Bound to the Delete key. */
  deleteFocusedSelection: () => Promise<void>;
}

export interface SelectionModifiers {
  /** Ctrl/Cmd — toggle this item in and out of the selection. */
  toggle: boolean;
  /** Shift — extend the selection from the anchor to this item. */
  range: boolean;
}

export const MIN_ZOOM = 0.25;
export const MAX_ZOOM = 4;

/**
 * Shared click-selection logic for both pages and fields.
 *
 * `items` is the full ordered list, so Shift can resolve a range by position.
 * Returns the new selection and the new anchor.
 */
function applySelection<T>(
  items: T[],
  current: T[],
  clicked: T,
  anchor: T | null,
  { toggle, range }: SelectionModifiers
): { selection: T[]; anchor: T | null } {
  if (range && anchor !== null) {
    const from = items.indexOf(anchor);
    const to = items.indexOf(clicked);

    if (from !== -1 && to !== -1) {
      const [start, end] = from <= to ? [from, to] : [to, from];
      // Shift extends from the anchor, so the anchor deliberately stays put —
      // that is what lets a user widen and narrow the same range.
      return { selection: items.slice(start, end + 1), anchor };
    }
  }

  if (toggle) {
    const next = current.includes(clicked)
      ? current.filter((item) => item !== clicked)
      : [...current, clicked];
    // Keep selections in document order regardless of click order.
    next.sort((a, b) => items.indexOf(a) - items.indexOf(b));
    return { selection: next, anchor: clicked };
  }

  // A plain click replaces the selection, or clears it if this was the only
  // item already selected.
  const alreadyOnly = current.length === 1 && current[0] === clicked;
  return { selection: alreadyOnly ? [] : [clicked], anchor: clicked };
}

export const useStore = create<AppState>((set, get) => {
  // Anchors live outside the store: they are bookkeeping for Shift-select and
  // nothing renders from them.
  let pageAnchor: number | null = null;
  let fieldAnchor: string | null = null;

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
    const names = new Set(fields.map((field) => field.name));

    set((state) => ({
      doc,
      fields,
      // Keep the viewport and both selections valid after pages or fields go.
      currentPage: Math.min(state.currentPage, Math.max(0, pageCount - 1)),
      selectedPages: state.selectedPages.filter((index) => index < pageCount),
      selectedFields: state.selectedFields.filter((name) => names.has(name)),
    }));
  }

  return {
    doc: null,
    fields: [],
    currentPage: 0,
    selectedPages: [],
    selectedFields: [],
    focus: null,
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

    // Multiplicative steps keep each notch feeling the same at any zoom level;
    // a fixed +0.25 is tiny at 400% and enormous at 25%.
    nudgeZoom: (delta) =>
      set((state) => ({
        zoom: Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, state.zoom * Math.exp(delta))),
      })),

    setCurrentPage: (index) => {
      const doc = get().doc;
      if (!doc) return;
      set({ currentPage: Math.min(Math.max(0, index), doc.pageCount - 1) });
    },

    selectPage: (index, modifiers) => {
      const doc = get().doc;
      if (!doc) return;

      const all = Array.from({ length: doc.pageCount }, (_, i) => i);
      const { selection, anchor } = applySelection(
        all,
        get().selectedPages,
        index,
        pageAnchor,
        modifiers
      );
      pageAnchor = anchor;
      set({ selectedPages: selection, focus: 'pages' });
    },

    selectAllPages: () =>
      set((state) => ({
        selectedPages: state.doc
          ? Array.from({ length: state.doc.pageCount }, (_, i) => i)
          : [],
        focus: 'pages',
      })),

    clearPageSelection: () => {
      pageAnchor = null;
      set({ selectedPages: [] });
    },

    selectField: (name, modifiers) => {
      const names = get().fields.map((field) => field.name);
      const { selection, anchor } = applySelection(
        names,
        get().selectedFields,
        name,
        fieldAnchor,
        modifiers
      );
      fieldAnchor = anchor;
      set({ selectedFields: selection, focus: 'fields' });
    },

    clearFieldSelection: () => {
      fieldAnchor = null;
      set({ selectedFields: [] });
    },

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
        set({ currentPage: 0, selectedPages: [], selectedFields: [], focus: null });
      });
    },

    openDocument: async (path) => {
      await run(async () => {
        await adopt(await ipc.openDocument(path));
        set({
          currentPage: 0,
          selectedPages: [],
          selectedFields: [],
          focus: null,
          panel: 'pages',
        });
      });
    },

    closeDocument: async () => {
      await run(async () => {
        await ipc.closeDocument();
        set({
          doc: null,
          fields: [],
          currentPage: 0,
          selectedPages: [],
          selectedFields: [],
          focus: null,
        });
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
      const { selectedPages, doc } = get();
      if (selectedPages.length === 0 || !doc) return;

      if (selectedPages.length >= doc.pageCount) {
        set({ error: 'A document must keep at least one page.' });
        return;
      }

      await run(async () => {
        const count = selectedPages.length;
        await adopt(await ipc.deletePages(selectedPages));
        pageAnchor = null;
        set({
          selectedPages: [],
          notice: `Deleted ${count} page${count === 1 ? '' : 's'}.`,
        });
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

    deleteSelectedFields: async () => {
      const { selectedFields } = get();
      if (selectedFields.length === 0) return;

      await run(async () => {
        // Deleting is one command per field; the last snapshot wins.
        let latest = null;
        for (const name of selectedFields) {
          latest = await ipc.deleteFormField(name);
        }
        if (latest) await adopt(latest);

        fieldAnchor = null;
        const count = selectedFields.length;
        set({
          selectedFields: [],
          notice: `Deleted ${count} field${count === 1 ? '' : 's'}.`,
        });
      });
    },

    deleteFocusedSelection: async () => {
      const { focus, selectedFields, selectedPages } = get();

      if (focus === 'fields' && selectedFields.length > 0) {
        await get().deleteSelectedFields();
        return;
      }
      if (focus === 'pages' && selectedPages.length > 0) {
        await get().deleteSelectedPages();
        return;
      }

      // No explicit focus yet — fall back to whichever selection exists.
      if (selectedFields.length > 0) await get().deleteSelectedFields();
      else if (selectedPages.length > 0) await get().deleteSelectedPages();
    },
  };
});
