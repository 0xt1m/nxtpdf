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
import type { DocumentInfo, FieldKind, FormField, NewField } from '@/lib/types';
import { isPlaced, isPositioned } from '@/lib/types';
import type { PlacedField, PositionedField } from '@/lib/types';
import { roundRect, screenDeltaToPdf, translateRect, type PdfRect } from '@/lib/geometry';

export type SidePanel = 'fields' | 'pages';

/** A field held on the in-app clipboard, ready to be pasted. */
export interface ClipboardField {
  kind: AddableKind;
  rect: PdfRect;
  multiline: boolean;
  required: boolean;
  maxLength: number | null;
  fontSize: number | null;
  options: string[];
  value: string | null;
}

/** Field kinds the add buttons can create. */
export type AddableKind = Extract<FieldKind, 'text' | 'checkbox' | 'choice'>;

/** Default size in PDF points for each kind of new field. */
const DEFAULT_SIZE: Record<AddableKind, { width: number; height: number }> = {
  text: { width: 200, height: 22 },
  checkbox: { width: 16, height: 16 },
  choice: { width: 160, height: 22 },
};

/** Default size in points, for a click that places without dragging. */
export function defaultFieldSize(kind: AddableKind): { width: number; height: number } {
  return DEFAULT_SIZE[kind];
}

/** Stem of the auto-generated name for each kind. */
const DEFAULT_NAME: Record<AddableKind, string> = {
  text: 'new_input',
  checkbox: 'new_checkbox',
  choice: 'new_dropdown',
};

/**
 * First unused name of the form `new_input`, `new_input2`, `new_input3`...
 *
 * The first one carries no suffix, which reads better than `new_input1` when
 * a form only ever gets one.
 */
/** Kinds the clipboard can round-trip; others have no create path. */
const COPYABLE: AddableKind[] = ['text', 'checkbox', 'choice'];

export function nextFieldName(kind: AddableKind, taken: string[]): string {
  const base = DEFAULT_NAME[kind];
  if (!taken.includes(base)) return base;

  let suffix = 2;
  while (taken.includes(`${base}${suffix}`)) suffix += 1;
  return `${base}${suffix}`;
}

/**
 * Which selection the Delete key acts on.
 *
 * Pages and fields are selected independently, so pressing Delete needs to
 * know which one the user last touched — otherwise deleting a field after
 * having selected pages would silently remove the wrong thing.
 */
export type Focus = 'pages' | 'fields' | null;

/**
 * How the print dialog opens.
 *
 * Ctrl+P prints the whole document; Ctrl+Shift+P starts from whatever pages
 * are selected, so "print just these" is one gesture rather than four clicks.
 */
export type PrintPreset = 'all' | 'selected';

interface AppState {
  // --- Documents ---
  /** Every open tab, in tab order. */
  docs: DocumentInfo[];
  /** The active tab's snapshot, or null when nothing is open. */
  doc: DocumentInfo | null;
  fields: FormField[];
  /**
   * The field type armed by the Add buttons, if any.
   *
   * While armed, dragging on the page draws the new field rather than
   * selecting one, and the toolbar button reads as pressed.
   */
  pendingField: AddableKind | null;

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
  /** Lives here rather than in App so Ctrl+P can open it. */
  printDialogOpen: boolean;
  /** Which page choice the dialog should start on. */
  printPreset: PrintPreset;
  /** Cleared if the backend reports PDFium failed to load at startup. */
  renderingAvailable: boolean;
  /**
   * Copied fields.
   *
   * This is an in-app clipboard rather than the system one: a PDF form field
   * has no sensible text representation, and round-tripping it through the OS
   * clipboard would lose everything but its name.
   */
  clipboard: ClipboardField[];

  // --- Actions ---
  setError: (message: string | null) => void;
  setNotice: (message: string | null) => void;
  setRenderingAvailable: (available: boolean) => void;
  setPanel: (panel: SidePanel) => void;
  openPrintDialog: (preset?: PrintPreset) => void;
  closePrintDialog: () => void;
  setZoom: (zoom: number) => void;
  nudgeZoom: (delta: number) => void;
  setCurrentPage: (index: number) => void;

  activateTab: (id: number) => Promise<void>;
  closeTab: (id: number) => Promise<void>;

  armField: (kind: AddableKind) => void;
  disarmField: () => void;
  /** Places the armed field at a rectangle drawn on the page. */
  placeArmedField: (
    pageIndex: number,
    rect: [number, number, number, number]
  ) => Promise<void>;

  selectPage: (index: number, modifiers: SelectionModifiers) => void;
  selectAllPages: () => void;
  clearPageSelection: () => void;
  selectField: (name: string, modifiers: SelectionModifiers) => void;
  clearFieldSelection: () => void;

  refresh: () => Promise<void>;
  newDocument: () => Promise<void>;
  openDocument: (path: string) => Promise<void>;
  save: () => Promise<void>;
  saveAs: (path: string) => Promise<void>;

  rotatePage: (index: number, degrees: number) => Promise<void>;
  deleteSelectedPages: () => Promise<void>;
  movePage: (from: number, to: number) => Promise<void>;
  appendPdf: (path: string) => Promise<void>;
  extractSelection: (path: string) => Promise<void>;

  setFieldValue: (name: string, value: string) => Promise<void>;
  /** Adds a field to the current page at a free default position. */
  addField: (kind: AddableKind) => Promise<void>;
  setFieldFontSize: (name: string, size: number) => Promise<void>;
  copySelectedFields: () => void;
  pasteFields: () => Promise<void>;
  moveField: (name: string, rect: [number, number, number, number]) => Promise<void>;
  /** Moves every selected field by a screen-space delta, in points. */
  nudgeSelectedFields: (du: number, dv: number) => Promise<void>;
  renameField: (name: string, newName: string) => Promise<void>;
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
  /** Grows with each paste so repeated Ctrl+V cascades instead of stacking. */
  let pastesSinceCopy = 0;

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

  /**
   * Applies a new snapshot of the active document.
   *
   * The tab list is re-read alongside it so titles and their unsaved-changes
   * markers cannot drift from the document they describe.
   */
  async function adopt(doc: DocumentInfo) {
    const [fields, docs] = await Promise.all([
      doc.hasAcroForm ? ipc.listFormFields() : Promise.resolve([]),
      ipc.listDocuments(),
    ]);

    const pageCount = doc.pageCount;
    const names = new Set(fields.map((field) => field.name));

    set((state) => ({
      doc,
      docs,
      fields,
      // Keep the viewport and both selections valid after pages or fields go.
      currentPage: Math.min(state.currentPage, Math.max(0, pageCount - 1)),
      selectedPages: state.selectedPages.filter((index) => index < pageCount),
      selectedFields: state.selectedFields.filter((name) => names.has(name)),
    }));
  }

  /** Remembers where each tab was, so switching back lands in the same place. */
  const views = new Map<number, { currentPage: number; zoom: number }>();

  function rememberView() {
    const { doc, currentPage, zoom } = get();
    if (doc) views.set(doc.id, { currentPage, zoom });
  }

  return {
    docs: [],
    doc: null,
    fields: [],
    pendingField: null,
    currentPage: 0,
    selectedPages: [],
    selectedFields: [],
    focus: null,
    zoom: 1,
    panel: 'pages',
    busy: false,
    error: null,
    notice: null,
    printDialogOpen: false,
    printPreset: 'all',
    renderingAvailable: true,
    clipboard: [],

    setError: (message) => set({ error: message }),
    setNotice: (message) => set({ notice: message }),
    setRenderingAvailable: (available) => set({ renderingAvailable: available }),
    setPanel: (panel) => set({ panel }),
    openPrintDialog: (preset = 'all') => {
      // Asking for the selection when nothing is selected would open a dialog
      // that cannot print anything, so fall back to the whole document.
      const usable = preset === 'selected' && get().selectedPages.length > 0;
      set({ printDialogOpen: true, printPreset: usable ? 'selected' : 'all' });
    },

    closePrintDialog: () => set({ printDialogOpen: false }),

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
        rememberView();
        // Reset the viewport before adopting: a new tab starts at page one
        // regardless of where the previous tab was scrolled to.
        set({ currentPage: 0, selectedPages: [], selectedFields: [], focus: null });
        await adopt(await ipc.newDocument());
      });
    },

    openDocument: async (path) => {
      await run(async () => {
        rememberView();
        set({
          currentPage: 0,
          selectedPages: [],
          selectedFields: [],
          focus: null,
          panel: 'pages',
        });

        const opened = await ipc.openDocument(path);

        // The backend focuses an already-open tab rather than duplicating it,
        // so restore that tab's viewport instead of starting at page one.
        const remembered = views.get(opened.id);
        if (remembered) set(remembered);

        await adopt(opened);
      });
    },

    activateTab: async (id) => {
      if (get().doc?.id === id) return;

      await run(async () => {
        rememberView();
        const active = await ipc.activateDocument(id);

        set({
          ...(views.get(id) ?? { currentPage: 0, zoom: 1 }),
          selectedPages: [],
          selectedFields: [],
          focus: null,
          pendingField: null,
        });

        await adopt(active);
      });
    },

    closeTab: async (id) => {
      await run(async () => {
        views.delete(id);
        const next = await ipc.closeDocument(id);

        if (next) {
          set({
            ...(views.get(next.id) ?? { currentPage: 0, zoom: 1 }),
            selectedPages: [],
            selectedFields: [],
            focus: null,
          });
          await adopt(next);
          return;
        }

        // That was the last tab.
        set({
          doc: null,
          docs: [],
          fields: [],
          printDialogOpen: false,
          pendingField: null,
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

    renameField: async (name, newName) => {
      if (newName.trim() === name.split('.').pop()) return;

      await run(async () => {
        const doc = await ipc.renameFormField(name, newName);
        await adopt(doc);

        // Selection is keyed by name, so re-point it at the renamed field
        // rather than silently dropping it.
        const prefix = name.includes('.')
          ? `${name.slice(0, name.lastIndexOf('.'))}.`
          : '';
        const renamed = `${prefix}${newName.trim()}`;
        fieldAnchor = renamed;
        set((state) => ({
          selectedFields: state.selectedFields.includes(name)
            ? [...state.selectedFields.filter((item) => item !== name), renamed]
            : state.selectedFields,
        }));
      });
    },

    armField: (kind) =>
      // Clicking the armed tool again puts it away, which is what a pressed
      // button implies.
      set((state) => ({
        pendingField: state.pendingField === kind ? null : kind,
      })),

    disarmField: () => set({ pendingField: null }),

    placeArmedField: async (pageIndex, rect) => {
      const { pendingField, fields } = get();
      if (!pendingField) return;

      const name = nextFieldName(
        pendingField,
        fields.map((field) => field.name)
      );

      await run(async () => {
        const updated = await ipc.createFormField({
          pageIndex,
          name,
          kind: pendingField,
          rect,
          fontSize: pendingField === 'text' ? 10 : null,
          multiline: false,
          required: false,
          maxLength: null,
          options: pendingField === 'choice' ? ['Option 1', 'Option 2'] : [],
        });
        await adopt(updated);

        fieldAnchor = name;
        set({
          selectedFields: [name],
          focus: 'fields',
          panel: 'fields',
          // One draw places one field, so the tool puts itself away.
          pendingField: null,
        });
      });
    },

    addField: async (kind) => {
      const { doc, currentPage, fields } = get();
      const page = doc?.pages[currentPage];
      if (!page) return;

      // Field rectangles live in unrotated page space, so undo the swap that
      // PageInfo applies for display.
      const quarterTurned = page.rotation === 90 || page.rotation === 270;
      const pageWidth = quarterTurned ? page.heightPt : page.widthPt;
      const pageHeight = quarterTurned ? page.widthPt : page.heightPt;

      const { width, height } = DEFAULT_SIZE[kind];

      // Cascade down the page so repeated clicks do not stack fields exactly
      // on top of each other, wrapping before running off the bottom.
      const onThisPage = fields.filter((field) => field.pageIndex === currentPage).length;
      const step = 30;
      const margin = 54;
      const usable = Math.max(1, Math.floor((pageHeight - margin * 2) / step));
      const slot = onThisPage % usable;

      const left = margin;
      const top = pageHeight - margin - slot * step;

      const rect: [number, number, number, number] = [
        left,
        Math.max(0, top - height),
        Math.min(pageWidth - margin, left + width),
        top,
      ];

      const name = nextFieldName(
        kind,
        fields.map((field) => field.name)
      );

      await run(async () => {
        const updated = await ipc.createFormField({
          pageIndex: currentPage,
          name,
          kind,
          rect,
          fontSize: kind === 'text' ? 10 : null,
          multiline: false,
          required: false,
          maxLength: null,
          options: kind === 'choice' ? ['Option 1', 'Option 2'] : [],
        });
        await adopt(updated);

        // Select it and show the panel so it can be renamed straight away.
        fieldAnchor = name;
        set({ selectedFields: [name], focus: 'fields', panel: 'fields' });
      });
    },

    nudgeSelectedFields: async (du, dv) => {
      const { doc, fields, selectedFields } = get();
      if (!doc || selectedFields.length === 0) return;

      const moves = selectedFields
        .map((name) => fields.find((field) => field.name === name))
        .filter((field): field is PlacedField => field !== undefined && isPlaced(field))
        .map((field) => {
          const page = doc.pages[field.pageIndex];
          if (!page) return null;
          const { dx, dy } = screenDeltaToPdf(du, dv, page);
          return { name: field.name, rect: translateRect(field.rect, dx, dy) };
        })
        .filter((move): move is { name: string; rect: PdfRect } => move !== null);

      if (moves.length === 0) return;

      await run(async () => {
        let latest = null;
        for (const move of moves) {
          latest = await ipc.setFormFieldRect(move.name, roundRect(move.rect));
        }
        if (latest) await adopt(latest);
      });
    },

    setFieldFontSize: async (name, size) => {
      await run(async () => {
        await adopt(await ipc.setFormFieldFontSize(name, size));
      });
    },

    copySelectedFields: () => {
      const { fields, selectedFields } = get();

      const copied = selectedFields
        .map((name) => fields.find((field) => field.name === name))
        .filter(
          (field): field is PositionedField => field !== undefined && isPositioned(field)
        )
        .filter((field) => COPYABLE.includes(field.kind as AddableKind))
        .map<ClipboardField>((field) => ({
          kind: field.kind as AddableKind,
          rect: field.rect,
          multiline: field.multiline,
          required: field.required,
          maxLength: field.maxLength,
          fontSize: field.fontSize,
          options: field.options,
          value: field.value,
        }));

      if (copied.length === 0) return;

      pastesSinceCopy = 0;
      set({
        clipboard: copied,
        notice: `Copied ${copied.length} field${copied.length === 1 ? '' : 's'}.`,
      });
    },

    pasteFields: async () => {
      const { clipboard, doc, currentPage, fields } = get();
      const page = doc?.pages[currentPage];
      if (!page || clipboard.length === 0) return;

      const quarterTurned = page.rotation === 90 || page.rotation === 270;
      const pageWidth = quarterTurned ? page.heightPt : page.widthPt;
      const pageHeight = quarterTurned ? page.widthPt : page.heightPt;

      pastesSinceCopy += 1;
      // Offset each paste so a repeated Ctrl+V builds a visible cascade rather
      // than stacking copies exactly on top of one another.
      const shift = 12 * pastesSinceCopy;

      // Names have to be reserved up front: the snapshot only refreshes after
      // each command, so two pastes in one batch would otherwise collide.
      const taken = fields.map((field) => field.name);
      const created: string[] = [];

      await run(async () => {
        let latest = null;

        for (const item of clipboard) {
          const width = item.rect[2] - item.rect[0];
          const height = item.rect[3] - item.rect[1];

          // Keep the copy on the page even when the original sat near an edge.
          const left = Math.max(0, Math.min(item.rect[0] + shift, pageWidth - width));
          const top = Math.max(height, Math.min(item.rect[3] - shift, pageHeight));

          const name = nextFieldName(item.kind, [...taken, ...created]);
          created.push(name);

          latest = await ipc.createFormField({
            pageIndex: currentPage,
            name,
            kind: item.kind,
            rect: [left, top - height, left + width, top],
            fontSize: item.fontSize,
            multiline: item.multiline,
            required: item.required,
            maxLength: item.maxLength,
            options: item.options,
          });

          if (item.value !== null && item.value !== '' && item.value !== 'Off') {
            latest = await ipc.setFormField(name, item.value);
          }
        }

        if (latest) await adopt(latest);

        fieldAnchor = created[created.length - 1] ?? null;
        set({
          selectedFields: created,
          focus: 'fields',
          panel: 'fields',
          notice: `Pasted ${created.length} field${created.length === 1 ? '' : 's'}.`,
        });
      });
    },

    moveField: async (name, rect) => {
      await run(async () => {
        await adopt(await ipc.setFormFieldRect(name, rect));
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
