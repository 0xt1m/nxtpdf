/**
 * Global keyboard shortcuts.
 *
 * Registered once on the window rather than per component, so a shortcut works
 * regardless of where focus happens to be — except inside a text field, where
 * Delete and Ctrl+A must keep their normal editing meaning.
 */

import { useEffect } from 'react';
import { ask } from '@tauri-apps/plugin-dialog';
import { useStore } from '@/state/store';
import { openDocument, saveDocument, saveDocumentAs } from '@/lib/fileActions';

/**
 * Closes the active tab, confirming first if it has unsaved changes.
 *
 * Ctrl+W is a reflex, so it must not be the one path that discards work
 * silently.
 */
async function closeActiveTab(): Promise<void> {
  const { doc, closeTab } = useStore.getState();
  if (!doc) return;

  if (doc.dirty) {
    const discard = await ask(`Close “${doc.name}” without saving your changes?`, {
      title: 'Unsaved changes',
      kind: 'warning',
      okLabel: 'Discard',
      cancelLabel: 'Keep editing',
    });
    if (!discard) return;
  }

  await closeTab(doc.id);
}

/** True when the event came from somewhere the user is typing. */
function isTextEntry(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;

  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
}

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    function handle(event: KeyboardEvent) {
      const state = useStore.getState();
      const typing = isTextEntry(event.target);
      const accel = event.ctrlKey || event.metaKey;

      // Escape closes the print dialog even when focus is inside one of its
      // controls, which the typing guard below would otherwise swallow.
      if (event.key === 'Escape' && state.printDialogOpen) {
        event.preventDefault();
        state.closePrintDialog();
        return;
      }

      // --- Accelerated shortcuts ---
      if (accel) {
        switch (event.key.toLowerCase()) {
          case 's':
            event.preventDefault();
            void (event.shiftKey ? saveDocumentAs() : saveDocument());
            return;

          case 'o':
            event.preventDefault();
            void openDocument();
            return;

          case 'w': {
            const active = state.doc;
            if (!active) return;
            event.preventDefault();
            // Unsaved work is confirmed in the tab bar; here it would be a
            // silent discard, so route through the same guarded path.
            void closeActiveTab();
            return;
          }

          case 'p':
            // Always preventDefault: the webview has its own Ctrl+P that would
            // open the browser print dialog, which has no tray or duplex
            // control and is exactly what this app exists to replace.
            event.preventDefault();
            if (!state.doc) return;
            // Shift starts the dialog on the current page selection.
            state.openPrintDialog(event.shiftKey ? 'selected' : 'all');
            return;

          case 'c':
            // Only when fields are selected — otherwise leave Ctrl+C alone so
            // copying text out of an input still works.
            if (typing || state.selectedFields.length === 0) return;
            event.preventDefault();
            state.copySelectedFields();
            return;

          case 'v':
            if (typing || state.clipboard.length === 0) return;
            event.preventDefault();
            void state.pasteFields();
            return;

          case 'a':
            // Inside a text box Ctrl+A must still select the text.
            if (typing || !state.doc) return;
            event.preventDefault();
            state.selectAllPages();
            return;

          // Both the main row and the numpad, and '=' because Ctrl++ needs
          // Shift on most layouts.
          case '=':
          case '+':
            event.preventDefault();
            state.nudgeZoom(0.2);
            return;

          case '-':
            event.preventDefault();
            state.nudgeZoom(-0.2);
            return;

          case '0':
            event.preventDefault();
            state.setZoom(1);
            return;
        }
        return;
      }

      // --- Unmodified keys ---
      if (typing) return;

      // Arrow keys nudge the selected fields. One point is the smallest step
      // that survives a round trip through the PDF; Shift makes it ten so a
      // field can be moved across a page without a hundred keypresses.
      const NUDGE: Record<string, [number, number]> = {
        ArrowLeft: [-1, 0],
        ArrowRight: [1, 0],
        ArrowUp: [0, -1],
        ArrowDown: [0, 1],
      };

      const nudge = NUDGE[event.key];
      if (nudge) {
        if (state.selectedFields.length === 0) return;
        event.preventDefault();
        const step = event.shiftKey ? 10 : 1;
        void state.nudgeSelectedFields(nudge[0] * step, nudge[1] * step);
        return;
      }

      if (event.key === 'Escape' && state.pendingField) {
        event.preventDefault();
        state.disarmField();
        return;
      }

      switch (event.key) {
        case 'Delete':
        case 'Backspace':
          if (!state.doc) return;
          event.preventDefault();
          void state.deleteFocusedSelection();
          return;

        case 'Escape':
          state.clearPageSelection();
          state.clearFieldSelection();
          return;

        case 'PageDown':
          if (!state.doc) return;
          event.preventDefault();
          state.setCurrentPage(state.currentPage + 1);
          return;

        case 'PageUp':
          if (!state.doc) return;
          event.preventDefault();
          state.setCurrentPage(state.currentPage - 1);
          return;
      }
    }

    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, []);
}
