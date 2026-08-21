import {
  open as openFileDialog,
  save as saveFileDialog,
} from '@tauri-apps/plugin-dialog';
import { useStore, MIN_ZOOM, MAX_ZOOM } from '@/state/store';

const PDF_FILTER = [{ name: 'PDF Document', extensions: ['pdf'] }];

interface ToolbarProps {
  onPrint: () => void;
}

export function Toolbar({ onPrint }: ToolbarProps) {
  const doc = useStore((s) => s.doc);
  const busy = useStore((s) => s.busy);
  const zoom = useStore((s) => s.zoom);
  const selectedPages = useStore((s) => s.selectedPages);

  const newDocument = useStore((s) => s.newDocument);
  const openDocument = useStore((s) => s.openDocument);
  const save = useStore((s) => s.save);
  const saveAs = useStore((s) => s.saveAs);
  const appendPdf = useStore((s) => s.appendPdf);
  const extractSelection = useStore((s) => s.extractSelection);
  const setZoom = useStore((s) => s.setZoom);

  const hasDoc = doc !== null;

  async function handleOpen() {
    const path = await openFileDialog({ multiple: false, filters: PDF_FILTER });
    if (typeof path === 'string') await openDocument(path);
  }

  async function handleSave() {
    // A document created in-app has no path yet, so Save becomes Save As.
    if (doc?.path) {
      await save();
      return;
    }
    await handleSaveAs();
  }

  async function handleSaveAs() {
    const path = await saveFileDialog({
      filters: PDF_FILTER,
      defaultPath: doc?.name ?? 'Untitled.pdf',
    });
    if (path) await saveAs(path);
  }

  async function handleAppend() {
    const path = await openFileDialog({ multiple: false, filters: PDF_FILTER });
    if (typeof path === 'string') await appendPdf(path);
  }

  async function handleExtract() {
    const path = await saveFileDialog({
      filters: PDF_FILTER,
      defaultPath: 'Extracted.pdf',
    });
    if (path) await extractSelection(path);
  }

  return (
    <header className="toolbar">
      <div className="toolbar__brand">
        <span className="toolbar__logo">NXT</span>
        <span className="toolbar__logo-sub">PDF</span>
      </div>

      <div className="toolbar__group">
        <button onClick={newDocument} disabled={busy}>
          New
        </button>
        <button onClick={handleOpen} disabled={busy}>
          Open…
        </button>
        <button onClick={handleSave} disabled={!hasDoc || busy}>
          Save
        </button>
        <button onClick={handleSaveAs} disabled={!hasDoc || busy}>
          Save As…
        </button>
      </div>

      <div className="toolbar__divider" />

      <div className="toolbar__group">
        <button onClick={handleAppend} disabled={!hasDoc || busy}>
          Append PDF…
        </button>
        <button
          onClick={handleExtract}
          disabled={!hasDoc || busy || selectedPages.length === 0}
          title={
            selectedPages.length === 0
              ? 'Select pages in the sidebar first'
              : `Export ${selectedPages.length} selected page(s)`
          }
        >
          Extract…
        </button>
      </div>

      <div className="toolbar__divider" />

      <div className="toolbar__group toolbar__zoom">
        <button
          onClick={() => setZoom(zoom - 0.25)}
          disabled={!hasDoc || zoom <= MIN_ZOOM}
        >
          −
        </button>
        <span className="toolbar__zoom-label">{Math.round(zoom * 100)}%</span>
        <button
          onClick={() => setZoom(zoom + 0.25)}
          disabled={!hasDoc || zoom >= MAX_ZOOM}
        >
          +
        </button>
        <button onClick={() => setZoom(1)} disabled={!hasDoc || zoom === 1}>
          Reset
        </button>
      </div>

      <div className="toolbar__spacer" />

      <div className="toolbar__group">
        <button className="button--primary" onClick={onPrint} disabled={!hasDoc || busy}>
          Print…
        </button>
      </div>
    </header>
  );
}
