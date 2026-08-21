/**
 * Builds URLs for the `nxtpdf://` page-image scheme registered in `lib.rs`.
 *
 * Page rasters bypass the JSON IPC channel entirely and are fetched by the
 * webview as ordinary images. That avoids base64-encoding several hundred
 * kilobytes per page and lets the webview cache the result.
 */

/**
 * Tauri rewrites custom schemes differently per platform: Windows and Android
 * route them through `http://<scheme>.localhost` because their webviews will
 * not accept a registered custom scheme, while macOS and Linux use the scheme
 * directly. The CSP in `tauri.conf.json` allows both spellings.
 */
const isHttpStyleScheme =
  typeof navigator !== 'undefined' && /Windows|Android/i.test(navigator.userAgent);

const SCHEME = 'nxtpdf';

/** Screen rendering resolution. 144 DPI is 2x CSS pixels — crisp on HiDPI. */
export const VIEWER_DPI = 144;

/** Thumbnails are small; 32 DPI is plenty and keeps sidebar scrolling smooth. */
export const THUMBNAIL_DPI = 32;

/**
 * URL for one rendered page.
 *
 * Neither `documentId` nor `revision` is read by the backend as data — both
 * exist to keep the webview's cache honest. The response is marked immutable,
 * so any two requests sharing a URL share an image. Without the document id,
 * page 1 of a newly opened file would collide with page 1 of the previous one
 * (both start at revision 1) and the stale page would be served from cache.
 */
export function pageImageUrl(
  documentId: number,
  pageIndex: number,
  dpi: number,
  revision: number
): string {
  const path = `page/${documentId}/${pageIndex}/${Math.round(dpi)}/${revision}`;
  return isHttpStyleScheme
    ? `http://${SCHEME}.localhost/${path}`
    : `${SCHEME}://localhost/${path}`;
}
