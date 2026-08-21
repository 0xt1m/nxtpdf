import { useRef, useState } from 'react';
import { useStore } from '@/state/store';
import { pageImageUrl, VIEWER_DPI } from '@/lib/pageImage';
import {
  pdfRectToScreen,
  roundRect,
  screenRectToPdf,
  type ScreenRect,
} from '@/lib/geometry';
import type { FieldKind, FormField, PageInfo } from '@/lib/types';

/** A field we know has a rectangle, so it can actually be drawn. */
type PlacedField = FormField & { rect: NonNullable<FormField['rect']> };

/** Ignore accidental micro-drags when drawing a new field. */
const MIN_DRAW_PX = 6;

interface ViewerProps {
  /** When set, dragging on the page draws a new field of this kind. */
  drawKind: FieldKind | null;
  onDrawComplete: (rect: [number, number, number, number], pageIndex: number) => void;
  focusedField: string | null;
  onFocusField: (name: string | null) => void;
}

export function Viewer({
  drawKind,
  onDrawComplete,
  focusedField,
  onFocusField,
}: ViewerProps) {
  const doc = useStore((s) => s.doc);
  const fields = useStore((s) => s.fields);
  const currentPage = useStore((s) => s.currentPage);
  const zoom = useStore((s) => s.zoom);
  const renderingAvailable = useStore((s) => s.renderingAvailable);
  const setCurrentPage = useStore((s) => s.setCurrentPage);

  const surfaceRef = useRef<HTMLDivElement>(null);
  const [drawStart, setDrawStart] = useState<{ x: number; y: number } | null>(null);
  const [drawBox, setDrawBox] = useState<ScreenRect | null>(null);

  if (!doc) {
    return (
      <main className="viewer viewer--empty">
        <div className="empty-state">
          <h2>No document open</h2>
          <p>Open a PDF or create a new one to get started.</p>
        </div>
      </main>
    );
  }

  const page: PageInfo | undefined = doc.pages[currentPage];
  if (!page) {
    return <main className="viewer viewer--empty" />;
  }

  // CSS pixels per PDF point. The image is rasterized at VIEWER_DPI and then
  // scaled down by CSS, which keeps it sharp on HiDPI displays.
  const scale = zoom;
  const displayWidth = page.widthPt * scale;
  const displayHeight = page.heightPt * scale;

  const pageFields = fields.filter(
    (field): field is PlacedField =>
      field.pageIndex === currentPage && field.rect !== null
  );

  function pointerPosition(event: React.PointerEvent): { x: number; y: number } | null {
    const surface = surfaceRef.current;
    if (!surface) return null;
    const bounds = surface.getBoundingClientRect();
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  }

  function handlePointerDown(event: React.PointerEvent) {
    if (!drawKind) return;
    const position = pointerPosition(event);
    if (!position) return;

    event.currentTarget.setPointerCapture(event.pointerId);
    setDrawStart(position);
    setDrawBox({ left: position.x, top: position.y, width: 0, height: 0 });
  }

  function handlePointerMove(event: React.PointerEvent) {
    if (!drawStart) return;
    const position = pointerPosition(event);
    if (!position) return;

    setDrawBox({
      left: Math.min(drawStart.x, position.x),
      top: Math.min(drawStart.y, position.y),
      width: Math.abs(position.x - drawStart.x),
      height: Math.abs(position.y - drawStart.y),
    });
  }

  function handlePointerUp() {
    const box = drawBox;
    setDrawStart(null);
    setDrawBox(null);

    if (!box || !page) return;
    if (box.width < MIN_DRAW_PX || box.height < MIN_DRAW_PX) return;

    onDrawComplete(roundRect(screenRectToPdf(box, page, scale)), currentPage);
  }

  return (
    <main className="viewer">
      <div className="viewer__scroll">
        <div
          ref={surfaceRef}
          className={`page-surface${drawKind ? ' page-surface--drawing' : ''}`}
          style={{ width: displayWidth, height: displayHeight }}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerUp}
        >
          {renderingAvailable ? (
            <img
              className="page-surface__image"
              src={pageImageUrl(currentPage, VIEWER_DPI, doc.revision)}
              alt={`Page ${currentPage + 1}`}
              draggable={false}
            />
          ) : (
            <div className="page-surface__placeholder">
              <p>Page rendering is unavailable.</p>
              <p className="hint">
                PDFium did not load. Run <code>pnpm setup:pdfium</code> and restart.
              </p>
            </div>
          )}

          {/* Existing form fields, positioned in PDF space. */}
          {pageFields.map((field) => {
            const box = pdfRectToScreen(field.rect, page, scale);
            const isFocused = focusedField === field.name;
            return (
              <button
                key={field.name}
                type="button"
                className={`field-box${isFocused ? ' field-box--focused' : ''}`}
                style={{
                  left: box.left,
                  top: box.top,
                  width: box.width,
                  height: box.height,
                }}
                title={`${field.name} (${field.kind})`}
                onClick={(event) => {
                  event.stopPropagation();
                  onFocusField(field.name);
                }}
              >
                <span className="field-box__label">{field.name}</span>
              </button>
            );
          })}

          {/* Live rubber band while drawing a new field. */}
          {drawBox && (
            <div
              className="draw-box"
              style={{
                left: drawBox.left,
                top: drawBox.top,
                width: drawBox.width,
                height: drawBox.height,
              }}
            />
          )}
        </div>
      </div>

      <nav className="viewer__pager">
        <button
          onClick={() => setCurrentPage(currentPage - 1)}
          disabled={currentPage === 0}
        >
          ‹ Prev
        </button>
        <span>
          Page {currentPage + 1} of {doc.pageCount}
        </span>
        <button
          onClick={() => setCurrentPage(currentPage + 1)}
          disabled={currentPage >= doc.pageCount - 1}
        >
          Next ›
        </button>
      </nav>
    </main>
  );
}
