import { useEffect } from 'react';
import { useStore } from '@/state/store';

/** Auto-dismiss transient success messages after this long. */
const NOTICE_MS = 3000;

export function StatusBar() {
  const doc = useStore((s) => s.doc);
  const busy = useStore((s) => s.busy);
  const error = useStore((s) => s.error);
  const notice = useStore((s) => s.notice);
  const setError = useStore((s) => s.setError);
  const setNotice = useStore((s) => s.setNotice);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), NOTICE_MS);
    return () => window.clearTimeout(timer);
  }, [notice, setNotice]);

  return (
    <footer className="status-bar">
      <span className="status-bar__doc">
        {doc ? (
          <>
            {doc.name}
            {doc.dirty && <span className="status-bar__dirty" title="Unsaved changes" />}
          </>
        ) : (
          'No document'
        )}
      </span>

      {busy && <span className="status-bar__busy">Working…</span>}

      {notice && <span className="status-bar__notice">{notice}</span>}

      {error && (
        <span className="status-bar__error">
          {error}
          <button onClick={() => setError(null)} aria-label="Dismiss error">
            ×
          </button>
        </span>
      )}
    </footer>
  );
}
