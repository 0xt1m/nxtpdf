/**
 * Global keyboard shortcuts.
 *
 * Registered once on the window rather than per component, so a shortcut works
 * regardless of where focus happens to be — except inside a text field, where
 * Delete and Ctrl+A must keep their normal editing meaning.
 */

import { useEffect } from 'react';
import { useStore } from '@/state/store';
import { openDocument, saveDocument, saveDocumentAs } from '@/lib/fileActions';

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
