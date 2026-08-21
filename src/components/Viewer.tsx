import { useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { defaultFieldSize, useStore } from '@/state/store';
import { pageImageUrl, VIEWER_DPI } from '@/lib/pageImage';
import {
  pdfRectToScreen,
  roundRect,
  screenRectToPdf,
  type ScreenRect,
} from '@/lib/geometry';
import {
  isPositioned,
  type FormField,
  type PageInfo,
  type PositionedField,
} from '@/lib/types';

/** Movement below this is a click, not a drag. */
const DRAG_THRESHOLD_PX = 3;

/** Below this a drawn rectangle counts as a click, and a default size is used. */
const MIN_DRAW_PX = 6;

/** Smallest field a drag may produce, in CSS pixels. */
const MIN_FIELD_PX = 8;

/** The eight resize grips, positioned as fractions of the box. */
const HANDLES = [
  { id: 'nw', x: 0, y: 0 },
  { id: 'n', x: 0.5, y: 0 },
  { id: 'ne', x: 1, y: 0 },
  { id: 'e', x: 1, y: 0.5 },
  { id: 'se', x: 1, y: 1 },
  { id: 's', x: 0.5, y: 1 },
  { id: 'sw', x: 0, y: 1 },
  { id: 'w', x: 0, y: 0.5 },
] as const;

type HandleId = (typeof HANDLES)[number]['id'];

/**
 * Whether a field has nothing in it.
 *
 * Checkboxes and radios read as empty in their `/Off` state, which is what a
 * PDF stores rather than an empty string.
 */
function isEmpty(field: FormField): boolean {
  return field.value === null || field.value === '' || field.value === 'Off';
}

export function Viewer() {
  const doc = useStore((s) => s.doc);
  const fields = useStore((s) => s.fields);
  const currentPage = useStore((s) => s.currentPage);
  const selectedFields = useStore((s) => s.selectedFields);
  const zoom = useStore((s) => s.zoom);
  const renderingAvailable = useStore((s) => s.renderingAvailable);
  const setCurrentPage = useStore((s) => s.setCurrentPage);
  const selectField = useStore((s) => s.selectField);
  const clearFieldSelection = useStore((s) => s.clearFieldSelection);
  const setFieldValue = useStore((s) => s.setFieldValue);
  const moveField = useStore((s) => s.moveField);
  const setPanel = useStore((s) => s.setPanel);
  const nudgeZoom = useStore((s) => s.nudgeZoom);
  const pendingField = useStore((s) => s.pendingField);
  const placeArmedField = useStore((s) => s.placeArmedField);

  const scrollRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [drawBox, setDrawBox] = useState<ScreenRect | null>(null);
  const drawStart = useRef<{ x: number; y: number } | null>(null);

  // Ctrl+wheel zooms instead of scrolling. This has to be a non-passive
  // listener: React attaches wheel handlers passively, and a passive listener
  // cannot call preventDefault, so the page would zoom *and* scroll.
  useEffect(() => {
    const node = scrollRef.current;
    if (!node) return;

    function onWheel(event: WheelEvent) {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      nudgeZoom(-event.deltaY / 500);
    }

    node.addEventListener('wheel', onWheel, { passive: false });
    return () => node.removeEventListener('wheel', onWheel);
  }, [nudgeZoom]);

  // Changing page should not strand an open editor.
  useEffect(() => {
    setEditing(null);
  }, [currentPage]);

  // Arming a tool cancels any in-progress edit; they are different modes.
  useEffect(() => {
    if (pendingField) setEditing(null);
  }, [pendingField]);

  if (!doc) {
    return (
      <main className="viewer viewer--empty">
        <div className="empty-state">
          <div className="empty-state__mark" aria-hidden="true">
            NXT<span>PDF</span>
          </div>
          <h2>No document open</h2>
          <p>View, reorganize, fill in forms, and print with full control.</p>
          <p className="hint">
            Press <kbd>Ctrl</kbd>+<kbd>O</kbd> to open a PDF, or use Open in the toolbar.
          </p>
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

  // The explicit predicate is required: TypeScript will not narrow through a
  // compound condition on its own.
  const pageFields = fields.filter(
    (field): field is PositionedField =>
      field.pageIndex === currentPage && isPositioned(field)
  );

  /** Pointer position relative to the page surface, in CSS pixels. */
  function surfacePoint(event: React.PointerEvent) {
    const surface = surfaceRef.current;
    if (!surface) return null;
    const bounds = surface.getBoundingClientRect();
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  }

  function beginDraw(event: React.PointerEvent) {
    const point = surfacePoint(event);
    if (!point) return;

    event.currentTarget.setPointerCapture(event.pointerId);
    drawStart.current = point;
    setDrawBox({ left: point.x, top: point.y, width: 0, height: 0 });
  }

  function continueDraw(event: React.PointerEvent) {
    const start = drawStart.current;
    if (!start) return;
    const point = surfacePoint(event);
    if (!point) return;

    setDrawBox({
      left: Math.min(start.x, point.x),
      top: Math.min(start.y, point.y),
      width: Math.abs(point.x - start.x),
      height: Math.abs(point.y - start.y),
    });
  }

  function finishDraw() {
    const start = drawStart.current;
    const box = drawBox;
    drawStart.current = null;
    setDrawBox(null);

    if (!start || !box || !page || !pendingField) return;

    // A click without a meaningful drag still places a field, at the default
    // size for its kind — insisting on a drag would just be pedantic.
    const drawn =
      box.width >= MIN_DRAW_PX && box.height >= MIN_DRAW_PX
        ? box
        : {
            left: start.x,
            top: start.y,
            width: defaultFieldSize(pendingField).width * scale,
            height: defaultFieldSize(pendingField).height * scale,
          };

    void placeArmedField(currentPage, roundRect(screenRectToPdf(drawn, page, scale)));
  }

  function cancelDraw() {
    drawStart.current = null;
    setDrawBox(null);
  }

  return (
    <main className="viewer">
      <div className="viewer__scroll" ref={scrollRef}>
        <div
          ref={surfaceRef}
          className={`page-surface${pendingField ? ' page-surface--drawing' : ''}`}
          style={{ width: displayWidth, height: displayHeight }}
          onPointerDown={(event) => {
            if (pendingField) {
              beginDraw(event);
              return;
            }
            // Pressing bare page, rather than a field, drops the selection.
            if (event.target === event.currentTarget) {
              clearFieldSelection();
              setEditing(null);
            }
          }}
          onPointerMove={continueDraw}
          onPointerUp={finishDraw}
          onPointerCancel={cancelDraw}
        >
          {renderingAvailable ? (
            <img
              className="page-surface__image"
              src={pageImageUrl(doc.id, currentPage, VIEWER_DPI, doc.revision)}
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

          {pageFields.map((field) => (
            <FieldOverlay
              key={field.name}
              field={field}
              page={page}
              scale={scale}
              selected={selectedFields.includes(field.name)}
              editing={editing === field.name}
              inert={pendingField !== null}
              onSelect={(modifiers) => {
                selectField(field.name, modifiers);
                setPanel('fields');
              }}
              onEdit={() => {
                if (!field.readOnly) setEditing(field.name);
              }}
              onToggle={() => {
                const on = !isEmpty(field);
                void setFieldValue(field.name, on ? 'Off' : 'On');
              }}
              onGeometryChange={(rect) => void moveField(field.name, rect)}
              onCommit={(value) => {
                setEditing(null);
                if (value !== (field.value ?? '')) {
                  void setFieldValue(field.name, value);
                }
              }}
              onCancel={() => setEditing(null)}
            />
          ))}
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
          <ChevronLeft size={15} />
          Prev
        </button>
        <span>
          Page {currentPage + 1} of {doc.pageCount}
        </span>
        <button
          onClick={() => setCurrentPage(currentPage + 1)}
          disabled={currentPage >= doc.pageCount - 1}
        >
          Next
          <ChevronRight size={15} />
        </button>
      </nav>
    </main>
  );
}

interface FieldOverlayProps {
  field: PositionedField;
  page: PageInfo;
  scale: number;
  selected: boolean;
  editing: boolean;
  /** True while an Add tool is armed, so drawing passes straight through. */
  inert: boolean;
  onSelect: (modifiers: { toggle: boolean; range: boolean }) => void;
  onEdit: () => void;
  onToggle: () => void;
  onGeometryChange: (rect: [number, number, number, number]) => void;
  onCommit: (value: string) => void;
  onCancel: () => void;
}

/**
 * One form field drawn over the page.
 *
 * Click selects it and reveals resize grips; dragging moves or resizes it;
 * double-click edits its contents. A drag is previewed locally and written
 * back only on release, so moving a field costs one command rather than one
 * per pointer event.
 */
function FieldOverlay({
  field,
  page,
  scale,
  selected,
  editing,
  inert,
  onSelect,
  onEdit,
  onToggle,
  onGeometryChange,
  onCommit,
  onCancel,
}: FieldOverlayProps) {
  const committed = pdfRectToScreen(field.rect, page, scale);

  const [preview, setPreview] = useState<ScreenRect | null>(null);
  const [draft, setDraft] = useState(field.value ?? '');
  const inputRef = useRef<HTMLInputElement>(null);
  const areaRef = useRef<HTMLTextAreaElement>(null);
  const selectRef = useRef<HTMLSelectElement>(null);

  // Tracks the in-flight gesture without re-rendering on every pointer move.
  const gesture = useRef<{
    handle: HandleId | null;
    startX: number;
    startY: number;
    origin: ScreenRect;
    latest: ScreenRect | null;
    moved: boolean;
  } | null>(null);

  useEffect(() => {
    if (editing) setDraft(field.value ?? '');
  }, [editing, field.value]);

  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    areaRef.current?.focus();
    selectRef.current?.focus();
  }, [editing]);

  const box = preview ?? committed;
  const style: React.CSSProperties = {
    left: box.left,
    top: box.top,
    width: box.width,
    height: box.height,
  };

  // --- Editing ---------------------------------------------------------

  if (editing) {
    const fontSize = Math.max(8, Math.min(box.height * 0.68, 20));
    const commit = () => onCommit(draft);

    // Stop Delete, arrows and Ctrl+A reaching the global shortcut handler,
    // which would otherwise delete or move the field being typed into.
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

  // --- Move and resize -------------------------------------------------

  /** Applies a pointer delta to the box the gesture started from. */
  function resized(
    origin: ScreenRect,
    handle: HandleId | null,
    dx: number,
    dy: number
  ): ScreenRect {
    if (handle === null) {
      return { ...origin, left: origin.left + dx, top: origin.top + dy };
    }

    let { left, top, width, height } = origin;

    // Dragging a west or north edge moves the origin as well as the size, and
    // must stop before the box turns inside out.
    if (handle.includes('w')) {
      const clamped = Math.min(dx, width - MIN_FIELD_PX);
      left += clamped;
      width -= clamped;
    }
    if (handle.includes('e')) {
      width = Math.max(MIN_FIELD_PX, width + dx);
    }
    if (handle.includes('n')) {
      const clamped = Math.min(dy, height - MIN_FIELD_PX);
      top += clamped;
      height -= clamped;
    }
    if (handle.includes('s')) {
      height = Math.max(MIN_FIELD_PX, height + dy);
    }

    return { left, top, width, height };
  }

  function beginGesture(event: React.PointerEvent, handle: HandleId | null) {
    if (field.readOnly && handle !== null) return;

    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);

    gesture.current = {
      handle,
      startX: event.clientX,
      startY: event.clientY,
      origin: committed,
      latest: null,
      moved: false,
    };

    // Select on press so the grips appear immediately, before any drag.
    if (handle === null) {
      onSelect({
        toggle: event.ctrlKey || event.metaKey,
        range: event.shiftKey,
      });
    }
  }

  function continueGesture(event: React.PointerEvent) {
    const active = gesture.current;
    if (!active) return;

    const dx = event.clientX - active.startX;
    const dy = event.clientY - active.startY;

    if (!active.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
    active.moved = true;

    const next = resized(active.origin, active.handle, dx, dy);
    active.latest = next;
    setPreview(next);
  }

  function endGesture(event: React.PointerEvent) {
    const active = gesture.current;
    gesture.current = null;
    if (!active) return;

    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setPreview(null);

    if (!active.moved) {
      // A plain click on a checkbox toggles it; other kinds just select.
      if (active.handle === null && field.kind === 'checkbox' && !field.readOnly) {
        onToggle();
      }
      return;
    }

    if (active.latest) {
      onGeometryChange(roundRect(screenRectToPdf(active.latest, page, scale)));
    }
  }

  const classes = [
    'field-box',
    selected && 'field-box--selected',
    field.readOnly && 'field-box--readonly',
    preview && 'field-box--dragging',
    inert && 'field-box--inert',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div
      className={classes}
      style={style}
      title={`${field.name} (${field.kind})${field.readOnly ? ' — read-only' : ''}`}
      onPointerDown={(event) => beginGesture(event, null)}
      onPointerMove={continueGesture}
      onPointerUp={endGesture}
      onPointerCancel={endGesture}
      onDoubleClick={(event) => {
        event.stopPropagation();
        if (field.kind !== 'checkbox') onEdit();
      }}
    >
      {/*
        The name is a placeholder, not a label: once the field has a value the
        page already shows it, and printing the name over it just obscures the
        document.
      */}
      {isEmpty(field) && <span className="field-box__label">{field.name}</span>}

      {selected &&
        !field.readOnly &&
        HANDLES.map((handle) => (
          <span
            key={handle.id}
            className={`field-handle field-handle--${handle.id}`}
            style={{ left: `${handle.x * 100}%`, top: `${handle.y * 100}%` }}
            onPointerDown={(event) => beginGesture(event, handle.id)}
            onPointerMove={continueGesture}
            onPointerUp={endGesture}
            onPointerCancel={endGesture}
          />
        ))}
    </div>
  );
}
