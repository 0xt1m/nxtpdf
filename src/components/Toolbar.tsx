import {
  FilePlus2,
  FolderOpen,
  Save,
  SaveAll,
  FileInput,
  FileOutput,
  Minus,
  Plus,
  Printer,
} from 'lucide-react';
import { useStore, MIN_ZOOM, MAX_ZOOM } from '@/state/store';
import {
  appendDocument,
  extractSelection,
  openDocument,
  saveDocument,
  saveDocumentAs,
} from '@/lib/fileActions';

/** Icons are sized once here so every button lines up. */
const ICON = 15;

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
        <button onClick={newDocument} disabled={busy} title="New document">
          <FilePlus2 size={ICON} />
          New
        </button>
        <button onClick={() => void openDocument()} disabled={busy} title="Open (Ctrl+O)">
          <FolderOpen size={ICON} />
          Open
        </button>
        <button
          onClick={() => void saveDocument()}
          disabled={!hasDoc || busy}
          title="Save (Ctrl+S)"
        >
          <Save size={ICON} />
          Save
        </button>
        <button
          className="button--icon"
          onClick={() => void saveDocumentAs()}
          disabled={!hasDoc || busy}
          title="Save As (Ctrl+Shift+S)"
          aria-label="Save As"
        >
          <SaveAll size={ICON} />
        </button>
      </div>

      <div className="toolbar__divider" />

      <div className="toolbar__group">
        <button
          onClick={() => void appendDocument()}
          disabled={!hasDoc || busy}
          title="Append the pages of another PDF"
        >
          <FileInput size={ICON} />
          Append
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
          <FileOutput size={ICON} />
          Extract
        </button>
      </div>

      <div className="toolbar__divider" />

      <div className="toolbar__group toolbar__zoom">
        <button
          onClick={() => nudgeZoom(-0.2)}
          disabled={!hasDoc || zoom <= MIN_ZOOM}
          title="Zoom out (Ctrl+− or Ctrl+scroll)"
          aria-label="Zoom out"
        >
          <Minus size={14} />
        </button>
        <button
          className="toolbar__zoom-label"
          onClick={() => setZoom(1)}
          disabled={!hasDoc}
          title="Reset zoom (Ctrl+0)"
        >
          {Math.round(zoom * 100)}%
        </button>
        <button
          onClick={() => nudgeZoom(0.2)}
          disabled={!hasDoc || zoom >= MAX_ZOOM}
          title="Zoom in (Ctrl++ or Ctrl+scroll)"
          aria-label="Zoom in"
        >
          <Plus size={14} />
        </button>
      </div>

      <div className="toolbar__spacer" />

      <button
        className="button--primary"
        onClick={onPrint}
        disabled={!hasDoc || busy}
        title="Print (Ctrl+P)"
      >
        <Printer size={ICON} />
        Print
      </button>
    </header>
  );
}
