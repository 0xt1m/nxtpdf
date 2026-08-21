import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Toolbar } from '@/components/Toolbar';
import { PageSidebar } from '@/components/PageSidebar';
import { Viewer } from '@/components/Viewer';
import { FieldsPanel } from '@/components/FieldsPanel';
import { FieldToolbar } from '@/components/FieldToolbar';
import { PrintDialog } from '@/components/PrintDialog';
import { StatusBar } from '@/components/StatusBar';
import { UpdateBanner } from '@/components/UpdateBanner';
import { useKeyboardShortcuts } from '@/hooks/useKeyboardShortcuts';
import { useAppUpdater } from '@/hooks/useAppUpdater';
import { useStore, type SidePanel } from '@/state/store';

const PANELS: { id: SidePanel; label: string }[] = [
  { id: 'pages', label: 'Pages' },
  { id: 'fields', label: 'Fields' },
];

export default function App() {
  const doc = useStore((s) => s.doc);
  const panel = useStore((s) => s.panel);
  const setPanel = useStore((s) => s.setPanel);
  const refresh = useStore((s) => s.refresh);
  const setRenderingAvailable = useStore((s) => s.setRenderingAvailable);
  const setError = useStore((s) => s.setError);
  const printDialogOpen = useStore((s) => s.printDialogOpen);
  const setPrintDialogOpen = useStore((s) => s.setPrintDialogOpen);

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

  return (
    <div className="app">
      <Toolbar onPrint={() => setPrintDialogOpen(true)} />
      <UpdateBanner updater={updater} />
      <FieldToolbar />

      <div className="app__body">
        {doc && <PageSidebar />}

        <Viewer />

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
            </div>
          </aside>
        )}
      </div>

      <StatusBar />

      {printDialogOpen && <PrintDialog onClose={() => setPrintDialogOpen(false)} />}
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
          <dt>Ctrl + P</dt>
          <dd>Print</dd>
          <dt>Del</dt>
          <dd>Delete selected pages or fields</dd>
          <dt>Ctrl + C / V</dt>
          <dd>Copy and paste fields</dd>
          <dt>Arrows</dt>
          <dd>Nudge selected fields (Shift for 10×)</dd>
          <dt>Double-click</dt>
          <dd>Edit a field on the page</dd>
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
