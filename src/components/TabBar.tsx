import { X } from 'lucide-react';
import { ask } from '@tauri-apps/plugin-dialog';
import { useStore } from '@/state/store';

/**
 * One tab per open document.
 *
 * Hidden entirely when a single document is open: a lone tab is chrome that
 * explains nothing. It appears the moment a second file is opened.
 */
export function TabBar() {
  const docs = useStore((s) => s.docs);
  const activeId = useStore((s) => s.doc?.id ?? null);
  const busy = useStore((s) => s.busy);
  const activateTab = useStore((s) => s.activateTab);
  const closeTab = useStore((s) => s.closeTab);

  if (docs.length < 2) return null;

  async function requestClose(id: number, name: string, dirty: boolean) {
    if (dirty) {
      const discard = await ask(`Close “${name}” without saving your changes?`, {
        title: 'Unsaved changes',
        kind: 'warning',
        okLabel: 'Discard',
        cancelLabel: 'Keep editing',
      });
      if (!discard) return;
    }
    await closeTab(id);
  }

  return (
    <div className="tab-bar" role="tablist">
      {docs.map((doc) => {
        const isActive = doc.id === activeId;

        return (
          <div
            key={doc.id}
            className={`tab${isActive ? ' tab--active' : ''}`}
            role="tab"
            aria-selected={isActive}
            title={doc.path ?? doc.name}
            onPointerDown={() => void activateTab(doc.id)}
          >
            <span className="tab__name">{doc.name}</span>
            {doc.dirty && <span className="tab__dirty" title="Unsaved changes" />}

            <button
              className="tab__close"
              disabled={busy}
              aria-label={`Close ${doc.name}`}
              title="Close"
              onPointerDown={(event) => {
                // Stop the tab underneath activating on the way to closing it.
                event.stopPropagation();
              }}
              onClick={(event) => {
                event.stopPropagation();
                void requestClose(doc.id, doc.name, doc.dirty);
              }}
            >
              <X size={13} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
