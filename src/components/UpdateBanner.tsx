import type { AppUpdater } from '@/hooks/useAppUpdater';

interface UpdateBannerProps {
  updater: AppUpdater;
}

/**
 * The update notice.
 *
 * Deliberately a slim strip under the toolbar rather than a modal: an update
 * is never urgent enough to block what the user opened the app to do. Ignoring
 * it is a valid choice — the update installs on close either way.
 */
export function UpdateBanner({ updater }: UpdateBannerProps) {
  const { stage, version, progress, error, dismissed } = updater;

  // Nothing to say while idle or mid-check.
  if (dismissed || stage === 'idle' || stage === 'checking') return null;

  const busy = stage === 'downloading' || stage === 'installing';

  return (
    <div className={`update-banner update-banner--${stage}`} role="status">
      <span className="update-banner__icon" aria-hidden="true">
        {stage === 'error' ? '!' : '↑'}
      </span>

      <div className="update-banner__text">
        {stage === 'downloading' && (
          <>
            <strong>Version {version} is available.</strong>
            <span className="update-banner__detail">
              Downloading
              {progress === null ? '…' : ` — ${progress}%`}. It installs when you close
              NXTPDF, or update now.
            </span>
          </>
        )}

        {stage === 'ready' && (
          <>
            <strong>Version {version} is ready to install.</strong>
            <span className="update-banner__detail">
              It installs automatically when you close NXTPDF.
            </span>
          </>
        )}

        {stage === 'installing' && (
          <>
            <strong>Installing version {version}…</strong>
            <span className="update-banner__detail">
              NXTPDF will reopen automatically.
            </span>
          </>
        )}

        {stage === 'error' && (
          <>
            <strong>Update failed.</strong>
            <span className="update-banner__detail">{error}</span>
          </>
        )}
      </div>

      {stage === 'downloading' && progress !== null && (
        <div className="update-banner__progress" aria-hidden="true">
          <div
            className="update-banner__progress-fill"
            style={{ width: `${progress}%` }}
          />
        </div>
      )}

      <div className="update-banner__actions">
        {(stage === 'downloading' || stage === 'ready') && (
          <button
            className="button--primary"
            onClick={() => void updater.updateNow()}
            title={
              stage === 'ready'
                ? 'Install now and reopen NXTPDF'
                : 'Install as soon as the download finishes, then reopen NXTPDF'
            }
          >
            Update now
          </button>
        )}

        {!busy && (
          <button
            className="update-banner__dismiss"
            onClick={updater.dismiss}
            aria-label="Dismiss"
            title="Hide until next launch"
          >
            ×
          </button>
        )}
      </div>
    </div>
  );
}
