import { useStore, MIN_ZOOM, MAX_ZOOM } from '@/state/store';
import {
  appendDocument,
  extractSelection,
  openDocument,
  saveDocument,
  saveDocumentAs,
} from '@/lib/fileActions';

interface ToolbarProps {
  onPrint: () => void;
}

export function Toolbar({ onPrint }: ToolbarProps) {
  const doc = useStore((s) => s.doc);
  const busy = useStore((s) => s.busy);
  const zoom = useStore((s) => s.zoom);
  const selectedPages = useStore((s) => s.selectedPages);

  const newDocument = useStore((s) => s.newDocument);
  const nudgeZoom = useStore((s) => s.nudgeZoom);
  const setZoom = useStore((s) => s.setZoom);

  const hasDoc = doc !== null;

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
        <button onClick={() => void openDocument()} disabled={busy} title="Ctrl+O">
          Open…
        </button>
        <button
          onClick={() => void saveDocument()}
          disabled={!hasDoc || busy}
          title="Ctrl+S"
        >
          Save
        </button>
        <button
          onClick={() => void saveDocumentAs()}
          disabled={!hasDoc || busy}
          title="Ctrl+Shift+S"
        >
          Save As…
        </button>
      </div>

      <div className="toolbar__divider" />

      <div className="toolbar__group">
        <button onClick={() => void appendDocument()} disabled={!hasDoc || busy}>
          Append PDF…
        </button>
        <button
          onClick={() => void extractSelection()}
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
          onClick={() => nudgeZoom(-0.2)}
          disabled={!hasDoc || zoom <= MIN_ZOOM}
          title="Ctrl+− or Ctrl+scroll"
        >
          −
        </button>
        <span className="toolbar__zoom-label">{Math.round(zoom * 100)}%</span>
        <button
          onClick={() => nudgeZoom(0.2)}
          disabled={!hasDoc || zoom >= MAX_ZOOM}
          title="Ctrl++ or Ctrl+scroll"
        >
          +
        </button>
        <button
          onClick={() => setZoom(1)}
          disabled={!hasDoc || zoom === 1}
          title="Ctrl+0"
        >
          Reset
        </button>
      </div>

      <div className="toolbar__spacer" />

      <div className="toolbar__group">
        <button
          className="button--primary"
          onClick={onPrint}
          disabled={!hasDoc || busy}
          title="Ctrl+P"
        >
          Print…
        </button>
      </div>
    </header>
  );
}
