/**
 * Opening PDFs by dropping them on the window.
 *
 * The events come from the OS through Tauri rather than from the DOM, so this
 * listens on the webview instead of using HTML drag events — a DOM `drop` never
 * carries a real filesystem path, which is what the backend needs.
 */

import { useEffect, useState } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { useStore } from '@/state/store';

function isPdf(path: string): boolean {
  return path.toLowerCase().endsWith('.pdf');
}

/** Trims a path down to its file name, for error messages. */
function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/**
 * Returns whether a drag is currently hovering the window, so the UI can show
 * a drop target.
 */
export function useFileDrop(): boolean {
  const [over, setOver] = useState(false);

  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent(async (event) => {
      const payload = event.payload;

      if (payload.type === 'enter' || payload.type === 'over') {
        setOver(true);
        return;
      }

      if (payload.type === 'leave') {
        setOver(false);
        return;
      }

      setOver(false);

      const pdfs = payload.paths.filter(isPdf);
      const rejected = payload.paths.filter((path) => !isPdf(path));

      const { openDocument, setError } = useStore.getState();

      // Opened one at a time: each call goes through the backend and returns a
      // new snapshot, and firing them together would race over the active tab.
      for (const path of pdfs) {
        await openDocument(path);
      }

      if (rejected.length > 0) {
        setError(
          pdfs.length > 0
            ? `Opened ${pdfs.length} PDF(s). Skipped ${rejected.length} other file(s).`
            : `NXTPDF opens PDFs. ${rejected.map(fileName).join(', ')} could not be opened.`
        );
      }
    });

    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  return over;
}
