import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Toolbar } from '@/components/Toolbar';
import { PageSidebar } from '@/components/PageSidebar';
import { Viewer } from '@/components/Viewer';
import { FieldsPanel } from '@/components/FieldsPanel';
import { DesignPanel, EMPTY_DRAFT, type DraftField } from '@/components/DesignPanel';
import { PrintDialog } from '@/components/PrintDialog';
import { StatusBar } from '@/components/StatusBar';
import { UpdateBanner } from '@/components/UpdateBanner';
import { useKeyboardShortcuts } from '@/hooks/useKeyboardShortcuts';
import { useAppUpdater } from '@/hooks/useAppUpdater';
import { useStore, type SidePanel } from '@/state/store';
import type { FieldKind, NewField } from '@/lib/types';

const PANELS: { id: SidePanel; label: string }[] = [
  { id: 'pages', label: 'Pages' },
  { id: 'fields', label: 'Fields' },
  { id: 'design', label: 'Design' },
];

export default function App() {
  const doc = useStore((s) => s.doc);
  const panel = useStore((s) => s.panel);
  const setPanel = useStore((s) => s.setPanel);
  const refresh = useStore((s) => s.refresh);
  const createField = useStore((s) => s.createField);
  const setRenderingAvailable = useStore((s) => s.setRenderingAvailable);
  const setError = useStore((s) => s.setError);

  const [printOpen, setPrintOpen] = useState(false);
  const [draft, setDraft] = useState<DraftField>(EMPTY_DRAFT);
  const [drawKind, setDrawKind] = useState<FieldKind | null>(null);

  useKeyboardShortcuts();
  const updater = useAppUpdater();

  // Pick up a document that survived a webview reload during development.
  useEffect(() => {
    void refresh();
  }, [refresh]);

  // The backend emits this when PDFium could not be loaded at startup. Page
  // and form editing still work; only rendering and printing are lost.
  useEffect(() => {
    const unlisten = listen<string>('pdfium-unavailable', (event) => {
      setRenderingAvailable(false);
      setError(event.payload);
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, [setRenderingAvailable, setError]);

  // Leaving the Design tab should never leave the viewer stuck in draw mode.
  useEffect(() => {
    if (panel !== 'design') setDrawKind(null);
  }, [panel]);

  const handleDrawComplete = useCallback(
    async (rect: [number, number, number, number], pageIndex: number) => {
      const name = draft.name.trim();
      if (!name) return;

      const field: NewField = {
        pageIndex,
        name,
        kind: draft.kind,
        rect,
        fontSize: draft.kind === 'text' ? draft.fontSize : null,
        multiline: draft.kind === 'text' ? draft.multiline : false,
        required: draft.required,
        maxLength: null,
        options:
          draft.kind === 'choice'
            ? draft.optionsText
                .split('\n')
                .map((option) => option.trim())
                .filter(Boolean)
            : [],
      };

      await createField(field);
      setDrawKind(null);
      // Clear only the name so several similar fields can be placed in a row.
      setDraft((current) => ({ ...current, name: '' }));
    },
    [draft, createField]
  );

  return (
    <div className="app">
      <Toolbar onPrint={() => setPrintOpen(true)} />
      <UpdateBanner updater={updater} />

      <div className="app__body">
        {doc && <PageSidebar />}

        <Viewer
          drawKind={drawKind}
          onDrawComplete={(rect, pageIndex) => void handleDrawComplete(rect, pageIndex)}
        />

        {doc && (
          <aside className="panel">
            <nav className="panel__tabs">
              {PANELS.map((entry) => (
                <button
                  key={entry.id}
                  className={`panel__tab${panel === entry.id ? ' panel__tab--active' : ''}`}
                  onClick={() => setPanel(entry.id)}
                >
                  {entry.label}
                </button>
              ))}
            </nav>

            <div className="panel__content">
              {panel === 'pages' && <DocumentSummary />}
              {panel === 'fields' && <FieldsPanel />}
              {panel === 'design' && (
                <DesignPanel
                  draft={draft}
                  onDraftChange={setDraft}
                  drawing={drawKind !== null}
                  onToggleDrawing={() =>
                    setDrawKind((current) => (current ? null : draft.kind))
                  }
                />
              )}
            </div>
          </aside>
        )}
      </div>

      <StatusBar />

      {printOpen && <PrintDialog onClose={() => setPrintOpen(false)} />}
    </div>
  );
}

function DocumentSummary() {
  const doc = useStore((s) => s.doc);
  const fields = useStore((s) => s.fields);
  if (!doc) return null;

  return (
    <>
      <dl className="summary">
        <dt>File</dt>
        <dd title={doc.path ?? 'Not saved yet'}>{doc.name}</dd>

        <dt>Location</dt>
        <dd className="summary__path">{doc.path ?? '— not saved —'}</dd>

        <dt>Pages</dt>
        <dd>{doc.pageCount}</dd>

        <dt>PDF version</dt>
        <dd>{doc.pdfVersion}</dd>

        <dt>Form fields</dt>
        <dd>{doc.hasAcroForm ? fields.length : 'none'}</dd>

        <dt>Unsaved changes</dt>
        <dd>{doc.dirty ? 'yes' : 'no'}</dd>
      </dl>

      <section className="shortcuts">
        <h3>Shortcuts</h3>
        <dl>
          <dt>Ctrl + S</dt>
          <dd>Save</dd>
          <dt>Ctrl + Shift + S</dt>
          <dd>Save As</dd>
          <dt>Ctrl + O</dt>
          <dd>Open</dd>
          <dt>Del</dt>
          <dd>Delete selected pages or fields</dd>
          <dt>Ctrl + click</dt>
          <dd>Add to selection</dd>
          <dt>Shift + click</dt>
          <dd>Select a range</dd>
          <dt>Ctrl + A</dt>
          <dd>Select all pages</dd>
          <dt>Ctrl + scroll</dt>
          <dd>Zoom</dd>
          <dt>Ctrl + 0</dt>
          <dd>Reset zoom</dd>
          <dt>Esc</dt>
          <dd>Clear selection</dd>
        </dl>
      </section>
    </>
  );
}
