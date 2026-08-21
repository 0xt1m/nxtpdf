import { useEffect, useRef, useState } from 'react';
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

/** Field kinds that open a text editor when clicked. */
const TEXT_LIKE: FieldKind[] = ['text', 'choice'];

interface ViewerProps {
  /** When set, dragging on the page draws a new field of this kind. */
  drawKind: FieldKind | null;
  onDrawComplete: (rect: [number, number, number, number], pageIndex: number) => void;
}

export function Viewer({ drawKind, onDrawComplete }: ViewerProps) {
  const doc = useStore((s) => s.doc);
  const fields = useStore((s) => s.fields);
  const currentPage = useStore((s) => s.currentPage);
  const selectedFields = useStore((s) => s.selectedFields);
  const zoom = useStore((s) => s.zoom);
  const renderingAvailable = useStore((s) => s.renderingAvailable);
  const setCurrentPage = useStore((s) => s.setCurrentPage);
  const selectField = useStore((s) => s.selectField);
  const setFieldValue = useStore((s) => s.setFieldValue);
  const nudgeZoom = useStore((s) => s.nudgeZoom);

  const scrollRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const [drawStart, setDrawStart] = useState<{ x: number; y: number } | null>(null);
  const [drawBox, setDrawBox] = useState<ScreenRect | null>(null);
  const [editing, setEditing] = useState<string | null>(null);

  // Ctrl+wheel zooms instead of scrolling. This has to be a non-passive
  // listener: React attaches wheel handlers passively, and a passive listener
  // cannot call preventDefault, so the page would zoom *and* scroll.
  useEffect(() => {
    const node = scrollRef.current;
    if (!node) return;

    function onWheel(event: WheelEvent) {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      // deltaY is negative when scrolling up (zoom in). The divisor turns one
      // notch of roughly 100 units into a comfortable step.
      nudgeZoom(-event.deltaY / 500);
    }

    node.addEventListener('wheel', onWheel, { passive: false });
    return () => node.removeEventListener('wheel', onWheel);
  }, [nudgeZoom]);

  // Changing page or entering draw mode should not strand an open editor.
  useEffect(() => {
    setEditing(null);
  }, [currentPage, drawKind]);

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

  function handleFieldClick(event: React.MouseEvent, field: PlacedField) {
    event.stopPropagation();

    const toggle = event.ctrlKey || event.metaKey;
    const range = event.shiftKey;
    selectField(field.name, { toggle, range });

    // A modified click is building a selection to act on, not opening an
    // editor, so only a plain click starts editing.
    if (toggle || range || field.readOnly) {
      setEditing(null);
      return;
    }

    if (field.kind === 'checkbox') {
      const on = field.value !== null && field.value !== '' && field.value !== 'Off';
      void setFieldValue(field.name, on ? 'Off' : 'On');
      return;
    }

    setEditing(TEXT_LIKE.includes(field.kind) ? field.name : null);
  }

  return (
    <main className="viewer">
      <div className="viewer__scroll" ref={scrollRef}>
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

            return (
              <FieldOverlay
                key={field.name}
                field={field}
                box={box}
                selected={selectedFields.includes(field.name)}
                editing={editing === field.name}
                onClick={(event) => handleFieldClick(event, field)}
                onCommit={(value) => {
                  setEditing(null);
                  if (value !== (field.value ?? '')) {
                    void setFieldValue(field.name, value);
                  }
                }}
                onCancel={() => setEditing(null)}
              />
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

interface FieldOverlayProps {
  field: PlacedField;
  box: ScreenRect;
  selected: boolean;
  editing: boolean;
  onClick: (event: React.MouseEvent) => void;
  onCommit: (value: string) => void;
  onCancel: () => void;
}

/** One form field drawn over the page, editable in place. */
function FieldOverlay({
  field,
  box,
  selected,
  editing,
  onClick,
  onCommit,
  onCancel,
}: FieldOverlayProps) {
  const [draft, setDraft] = useState(field.value ?? '');
  const inputRef = useRef<HTMLInputElement>(null);
  const areaRef = useRef<HTMLTextAreaElement>(null);
  const selectRef = useRef<HTMLSelectElement>(null);

  // Re-seed whenever editing opens or the backing value changes underneath.
  useEffect(() => {
    if (editing) setDraft(field.value ?? '');
  }, [editing, field.value]);

  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    areaRef.current?.focus();
    selectRef.current?.focus();
  }, [editing]);

  const style: React.CSSProperties = {
    left: box.left,
    top: box.top,
    width: box.width,
    height: box.height,
  };

  if (editing) {
    // Fit the text to the box so the editor lines up with the printed result.
    const fontSize = Math.max(8, Math.min(box.height * 0.68, 20));
    const commit = () => onCommit(draft);

    // Stop Delete and Ctrl+A reaching the global shortcut handler, which would
    // otherwise delete the field the user is trying to type into.
    const onKeyDown = (event: React.KeyboardEvent) => {
      event.stopPropagation();
      if (event.key === 'Escape') onCancel();
      if (event.key === 'Enter' && !field.multiline) commit();
    };

    if (field.kind === 'choice') {
      return (
        <select
          ref={selectRef}
          className="field-editor"
          style={{ ...style, fontSize }}
          value={draft}
          onChange={(event) => {
            setDraft(event.target.value);
            onCommit(event.target.value);
          }}
          onBlur={commit}
          onKeyDown={onKeyDown}
        >
          <option value="">— none —</option>
          {field.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      );
    }

    if (field.multiline) {
      return (
        <textarea
          ref={areaRef}
          className="field-editor"
          style={{ ...style, fontSize }}
          value={draft}
          maxLength={field.maxLength ?? undefined}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={commit}
          onKeyDown={onKeyDown}
        />
      );
    }

    return (
      <input
        ref={inputRef}
        className="field-editor"
        style={{ ...style, fontSize }}
        type={field.password ? 'password' : 'text'}
        value={draft}
        maxLength={field.maxLength ?? undefined}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
    );
  }

  const classes = [
    'field-box',
    selected && 'field-box--selected',
    field.readOnly && 'field-box--readonly',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <button
      type="button"
      className={classes}
      style={style}
      title={`${field.name} (${field.kind})${field.readOnly ? ' — read-only' : ''}`}
      onClick={onClick}
    >
      <span className="field-box__label">{field.name}</span>
    </button>
  );
}
