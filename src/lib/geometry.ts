/**
 * Coordinate conversion between PDF user space and screen space.
 *
 * PDF user space has its origin at the **bottom-left** with y increasing
 * upward. Screen space has its origin at the **top-left** with y increasing
 * downward. On top of that, a page carries a `/Rotate` value that the renderer
 * applies but that field rectangles are *not* stored in — a widget's `/Rect` is
 * always in unrotated user space.
 *
 * Getting this wrong puts form-field overlays in the wrong place on any rotated
 * page, so the rotation is handled explicitly here rather than ignored.
 */

import type { PageInfo } from './types';

export interface ScreenRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** PDF rectangle `[x0, y0, x1, y1]`. */
export type PdfRect = [number, number, number, number];

/**
 * The page's dimensions *before* rotation, in points.
 *
 * `PageInfo` reports post-rotation dimensions (what the user sees), but PDF
 * coordinates are expressed against the unrotated page, so undo the swap.
 */
function unrotatedSize(page: PageInfo): { width: number; height: number } {
  const quarterTurned = page.rotation === 90 || page.rotation === 270;
  return quarterTurned
    ? { width: page.heightPt, height: page.widthPt }
    : { width: page.widthPt, height: page.heightPt };
}

/** Maps one PDF-space point to display space, in points, origin top-left. */
function pointToDisplay(x: number, y: number, page: PageInfo): { u: number; v: number } {
  const { width: w, height: h } = unrotatedSize(page);

  switch (page.rotation) {
    case 90:
      return { u: h - y, v: x };
    case 180:
      return { u: w - x, v: y };
    case 270:
      return { u: y, v: w - x };
    default:
      return { u: x, v: h - y };
  }
}

/** Inverse of {@link pointToDisplay}. */
function displayToPoint(u: number, v: number, page: PageInfo): { x: number; y: number } {
  const { width: w, height: h } = unrotatedSize(page);

  switch (page.rotation) {
    case 90:
      return { x: v, y: h - u };
    case 180:
      return { x: w - u, y: v };
    case 270:
      return { x: w - v, y: u };
    default:
      return { x: u, y: h - v };
  }
}

/**
 * Converts a PDF rectangle to a CSS box.
 *
 * @param scale CSS pixels per PDF point.
 */
export function pdfRectToScreen(
  rect: PdfRect,
  page: PageInfo,
  scale: number
): ScreenRect {
  const [x0, y0, x1, y1] = rect;
  const a = pointToDisplay(x0, y0, page);
  const b = pointToDisplay(x1, y1, page);

  // Rotation can flip which corner is which, so normalize afterwards.
  const left = Math.min(a.u, b.u) * scale;
  const top = Math.min(a.v, b.v) * scale;
  const width = Math.abs(a.u - b.u) * scale;
  const height = Math.abs(a.v - b.v) * scale;

  return { left, top, width, height };
}

/** Converts a CSS box back to a PDF rectangle. */
export function screenRectToPdf(box: ScreenRect, page: PageInfo, scale: number): PdfRect {
  const a = displayToPoint(box.left / scale, box.top / scale, page);
  const b = displayToPoint(
    (box.left + box.width) / scale,
    (box.top + box.height) / scale,
    page
  );

  return [Math.min(a.x, b.x), Math.min(a.y, b.y), Math.max(a.x, b.x), Math.max(a.y, b.y)];
}

/**
 * Converts a movement in screen space to the equivalent in PDF space.
 *
 * Screen y grows downward and PDF y grows upward, and a rotated page swaps the
 * axes on top of that — so "nudge right" is only `+x` on an unrotated page.
 */
export function screenDeltaToPdf(
  du: number,
  dv: number,
  page: PageInfo
): { dx: number; dy: number } {
  switch (page.rotation) {
    case 90:
      return { dx: dv, dy: -du };
    case 180:
      return { dx: -du, dy: dv };
    case 270:
      return { dx: -dv, dy: du };
    default:
      return { dx: du, dy: -dv };
  }
}

/** Shifts a PDF rectangle by a PDF-space delta. */
export function translateRect(rect: PdfRect, dx: number, dy: number): PdfRect {
  return [rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy];
}

/** Rounds to 2dp so serialized rectangles stay readable. */
export function roundRect(rect: PdfRect): PdfRect {
  return rect.map((value) => Math.round(value * 100) / 100) as PdfRect;
}

export const POINTS_PER_INCH = 72;

export function pointsToMm(points: number): number {
  return (points / POINTS_PER_INCH) * 25.4;
}

/** Human-readable page size, e.g. `8.5 × 11 in`. */
export function describePageSize(page: PageInfo): string {
  const widthIn = page.widthPt / POINTS_PER_INCH;
  const heightIn = page.heightPt / POINTS_PER_INCH;
  const round = (value: number) => Math.round(value * 100) / 100;
  return `${round(widthIn)} × ${round(heightIn)} in`;
}
