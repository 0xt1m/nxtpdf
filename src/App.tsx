import { Fragment, useEffect } from 'react';
import { ExternalLink } from 'lucide-react';
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
import * as ipc from '@/lib/ipc';

const SHORTCUTS: { keys: string[]; action: string }[] = [
  { keys: ['Ctrl', 'S'], action: 'Save' },
  { keys: ['Ctrl', 'Shift', 'S'], action: 'Save As' },
  { keys: ['Ctrl', 'O'], action: 'Open' },
  { keys: ['Ctrl', 'P'], action: 'Print' },
  { keys: ['Ctrl', 'Shift', 'P'], action: 'Print selected pages' },
  { keys: ['Del'], action: 'Delete selected pages or fields' },
  { keys: ['Ctrl', 'C'], action: 'Copy fields' },
  { keys: ['Ctrl', 'V'], action: 'Paste fields' },
  { keys: ['↑', '↓', '←', '→'], action: 'Nudge fields (Shift for 10×)' },
  { keys: ['Ctrl', 'click'], action: 'Add to selection' },
  { keys: ['Shift', 'click'], action: 'Select a range' },
  { keys: ['Ctrl', 'A'], action: 'Select all pages' },
  { keys: ['Ctrl', 'scroll'], action: 'Zoom' },
  { keys: ['Ctrl', '0'], action: 'Reset zoom' },
  { keys: ['Esc'], action: 'Clear selection' },
];

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
  const openPrintDialog = useStore((s) => s.openPrintDialog);
  const closePrintDialog = useStore((s) => s.closePrintDialog);

  useKeyboardShortcuts();
  const updater = useAppUpdater();

  // Pick up a document that survived a webview reload during development.
  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Explorer can hand the backend a document directly — at launch, or through
  // the single-instance forwarder while the app is already running.
  useEffect(() => {
    const unlisten = listen('document-changed', () => void refresh());
    return () => {
      void unlisten.then((off) => off());
    };
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
      <Toolbar onPrint={() => openPrintDialog('all')} />
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

      {printDialogOpen && <PrintDialog onClose={closePrintDialog} />}
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
        <h3>Windows</h3>
        <button
          className="button--outline"
          onClick={() => void ipc.openDefaultAppsSettings().catch(() => {})}
        >
          <ExternalLink size={14} />
          Set as default PDF app
        </button>
        <p className="hint">
          Windows only lets you choose this yourself, so this opens Settings — find PDF
          under “Choose defaults by file type”.
        </p>
      </section>

      <section className="shortcuts">
        <h3>Shortcuts</h3>
        <dl>
          {SHORTCUTS.map(({ keys, action }) => (
            <Fragment key={action}>
              <dt>
                {keys.map((key) => (
                  <kbd key={key}>{key}</kbd>
                ))}
              </dt>
              <dd>{action}</dd>
            </Fragment>
          ))}
        </dl>
      </section>
    </>
  );
}
