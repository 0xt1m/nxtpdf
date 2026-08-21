/**
 * Auto-update lifecycle.
 *
 * On startup the app asks the update endpoint whether a newer version exists.
 * If one does it is downloaded **immediately, in the background**, so it is
 * already on disk by the time it is needed. From there:
 *
 * * Ignore the banner and the update installs silently when the app closes,
 *   so the next launch is the new version.
 * * Press *Update now* and it installs at once and reopens the app. If the
 *   download is still running, the press waits for it rather than restarting
 *   the transfer.
 *
 * Installing on close is why the Windows bundle is a per-user NSIS install
 * with `installMode: "quiet"`: a per-machine install needs a UAC prompt, which
 * cannot be answered by an app that is in the middle of exiting.
 *
 * A failed check is deliberately silent. Being offline, behind a proxy, or
 * pointed at an endpoint that does not exist yet is not worth nagging about on
 * every launch.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { relaunch } from '@tauri-apps/plugin-process';

export type UpdateStage =
  'idle' | 'checking' | 'downloading' | 'ready' | 'installing' | 'error';

export interface UpdateState {
  stage: UpdateStage;
  /** Version offered by the endpoint, once known. */
  version: string | null;
  /** Release notes from the update manifest, if it carried any. */
  notes: string | null;
  /** 0–100 while downloading, or null when the server sent no length. */
  progress: number | null;
  error: string | null;
}

const INITIAL: UpdateState = {
  stage: 'idle',
  version: null,
  notes: null,
  progress: null,
  error: null,
};

export interface AppUpdater extends UpdateState {
  /** Installs as soon as the download finishes, then reopens the app. */
  updateNow: () => Promise<void>;
  /** Hides the banner for this session; the update still installs on close. */
  dismiss: () => void;
  dismissed: boolean;
}

export function useAppUpdater(): AppUpdater {
  const [state, setState] = useState<UpdateState>(INITIAL);
  const [dismissed, setDismissed] = useState(false);

  // These must survive until the window closes, and the close handler must not
  // capture a stale render's copy — hence refs rather than state.
  const updateRef = useRef<Update | null>(null);
  const readyRef = useRef(false);
  /** In-flight download, so *Update now* joins it instead of starting a second. */
  const downloadRef = useRef<Promise<boolean> | null>(null);

  /** Downloads the pending update. Resolves true once it is on disk. */
  const beginDownload = useCallback((update: Update): Promise<boolean> => {
    if (downloadRef.current) return downloadRef.current;

    const task = (async () => {
      setState((current) => ({ ...current, stage: 'downloading', progress: null }));

      let total = 0;
      let received = 0;

      try {
        await update.download((event) => {
          switch (event.event) {
            case 'Started':
              total = event.data.contentLength ?? 0;
              setState((current) => ({ ...current, progress: total > 0 ? 0 : null }));
              break;

            case 'Progress':
              received += event.data.chunkLength;
              if (total > 0) {
                const percent = Math.min(100, Math.round((received / total) * 100));
                setState((current) => ({ ...current, progress: percent }));
              }
              break;

            case 'Finished':
              setState((current) => ({ ...current, progress: 100 }));
              break;
          }
        });

        readyRef.current = true;
        setState((current) => ({ ...current, stage: 'ready', progress: 100 }));
        return true;
      } catch (error) {
        console.error('[updater] download failed:', error);
        setState((current) => ({
          ...current,
          stage: 'error',
          error: error instanceof Error ? error.message : String(error),
        }));
        // Clear so a later attempt can retry rather than reusing the failure.
        downloadRef.current = null;
        return false;
      }
    })();

    downloadRef.current = task;
    return task;
  }, []);

  // --- Check on startup, then fetch straight away ---
  useEffect(() => {
    let cancelled = false;

    (async () => {
      setState((current) => ({ ...current, stage: 'checking' }));

      try {
        const update = await check();

        if (cancelled) return;
        if (!update) {
          setState(INITIAL);
          return;
        }

        updateRef.current = update;
        setState({
          stage: 'downloading',
          version: update.version,
          notes: update.body ?? null,
          progress: null,
          error: null,
        });

        void beginDownload(update);
      } catch (error) {
        // Silent by design — see the module docs.
        console.warn('[updater] check failed:', error);
        if (!cancelled) setState(INITIAL);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [beginDownload]);

  // --- Install on close ---
  useEffect(() => {
    const appWindow = getCurrentWindow();

    const registration = appWindow.onCloseRequested(async (event) => {
      const update = updateRef.current;
      if (!readyRef.current || !update) return;

      // Hold the window open just long enough to hand off to the installer.
      // With installMode "quiet" this is invisible.
      event.preventDefault();

      try {
        await update.install();
      } catch (error) {
        console.error('[updater] install on close failed:', error);
      }

      // `install()` normally terminates the app itself. Destroying the window
      // covers the case where it returns instead, so a failed update can never
      // leave a window that refuses to close.
      await appWindow.destroy();
    });

    return () => {
      void registration.then((unlisten) => unlisten());
    };
  }, []);

  const updateNow = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;

    setState((current) => ({ ...current, stage: 'installing' }));

    // Joins the background download if it is still running.
    const downloaded = readyRef.current || (await beginDownload(update));
    if (!downloaded) return;

    try {
      await update.install();
      await relaunch();
    } catch (error) {
      console.error('[updater] install failed:', error);
      setState((current) => ({
        ...current,
        stage: 'error',
        error: error instanceof Error ? error.message : String(error),
      }));
    }
  }, [beginDownload]);

  const dismiss = useCallback(() => setDismissed(true), []);

  return { ...state, updateNow, dismiss, dismissed };
}
