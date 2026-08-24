import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
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
  type TextRun,
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

/**
 * The document view: every page stacked vertically and scrolled continuously,
 * the way a reader expects rather than one page at a time.
 */
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

  /**
   * A callback ref, not `useRef`.
   *
   * The scroll container only exists once a document is open, so a `useRef`
   * read inside an effect that runs on mount finds `null` and never retries —
   * which is exactly why Ctrl+scroll silently did nothing. Storing the node in
   * state re-runs the effect the moment it appears.
   */
  const [scrollNode, setScrollNode] = useState<HTMLDivElement | null>(null);
  const pageNodes = useRef(new Map<number, HTMLDivElement>());
  // Editing lives in the store, not here: Enter opens a field from the global
  // shortcut handler, and Tab walks to the next one from inside the store.
  const editing = useStore((s) => s.editingField);
  const setEditing = useStore((s) => s.editField);
  const fieldDraft = useStore((s) => s.fieldDraft);
  const setFieldDraft = useStore((s) => s.setFieldDraft);
  const editAdjacentField = useStore((s) => s.editAdjacentField);
  const textMode = useStore((s) => s.textMode);
  const textRuns = useStore((s) => s.textRuns);
  const loadTextRuns = useStore((s) => s.loadTextRuns);
  const editingTextRun = useStore((s) => s.editingTextRun);
  const editTextRun = useStore((s) => s.editTextRun);
  const setTextRun = useStore((s) => s.setTextRun);

  /** Set while we scroll programmatically, so it is not read back as intent. */
  const scrollingTo = useRef<number | null>(null);

  /** True while space is held: the pointer pans instead of selecting. */
  const [panReady, setPanReady] = useState(false);
  const panning = useRef<{ x: number; y: number; left: number; top: number } | null>(
    null
  );

  /**
   * Where the cursor was when a zoom started, so that point can be put back
   * under the cursor once the new scale has been laid out.
   */
  const zoomAnchor = useRef<{
    /** Cursor position in content coordinates, before the zoom. */
    contentX: number;
    contentY: number;
    /** Cursor position within the viewport, which must not move. */
    viewportX: number;
    viewportY: number;
    from: number;
  } | null>(null);

  // Ctrl+wheel zooms instead of scrolling. The listener has to be non-passive:
  // React attaches wheel handlers passively, and a passive listener cannot
  // preventDefault, so the page would zoom *and* scroll.
  useEffect(() => {
    if (!scrollNode) return;

    // Captured once, so the handler needs no non-null assertions.
    const node = scrollNode;

    function onWheel(event: WheelEvent) {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();

      const bounds = node.getBoundingClientRect();
      const viewportX = event.clientX - bounds.left;
      const viewportY = event.clientY - bounds.top;

      zoomAnchor.current = {
        contentX: node.scrollLeft + viewportX,
        contentY: node.scrollTop + viewportY,
        viewportX,
        viewportY,
        // Read live rather than closing over `zoom`, so the effect does not
        // have to re-attach the listener on every zoom step.
        from: useStore.getState().zoom,
      };

      nudgeZoom(-event.deltaY / 500);
    }

    node.addEventListener('wheel', onWheel, { passive: false });
    return () => node.removeEventListener('wheel', onWheel);
  }, [scrollNode, nudgeZoom]);

  // Keep the point under the cursor pinned across a zoom.
  //
  // Content scales about its own origin, so a point at `contentX` moves to
  // `contentX * ratio`; scrolling by the difference puts it back under the
  // cursor. This is a layout effect so the correction lands in the same frame
  // as the resize — in a plain effect the view visibly jumps first.
  useLayoutEffect(() => {
    const anchor = zoomAnchor.current;
    if (!anchor || !scrollNode) return;
    zoomAnchor.current = null;

    const ratio = zoom / anchor.from;
    if (ratio === 1) return;

    scrollNode.scrollLeft = anchor.contentX * ratio - anchor.viewportX;
    scrollNode.scrollTop = anchor.contentY * ratio - anchor.viewportY;
  }, [zoom, scrollNode]);

  // Track the page being read, straight from geometry.
  //
  // IntersectionObserver is the wrong tool here, for three reasons:
  //   * its callback receives only the pages whose intersection *changed*, so
  //     "the most visible one" would be picked from a partial set;
  //   * `intersectionRatio` is relative to each element's own size, so a short
  //     page fully on screen scores 1.0 while a tall page filling the viewport
  //     scores 0.3 — the small one wins;
  //   * it fires only when a threshold is crossed, so the highlight freezes
  //     between them.
  //
  // Measuring against an anchor line has none of those problems and is a
  // handful of rectangle reads per frame.
  const pageCount = doc?.pageCount ?? 0;
  const documentId = doc?.id ?? null;

  useEffect(() => {
    const node = scrollNode;
    if (!node || pageCount === 0) return;

    let frame = 0;

    const update = () => {
      frame = 0;

      const view = node.getBoundingClientRect();
      // A third down the viewport, not the top edge: that is where the page
      // you are actually reading sits once you have scrolled into a document.
      const anchor = view.top + view.height * 0.33;

      let best: number | null = null;
      let bestDistance = Number.POSITIVE_INFINITY;

      for (const [index, element] of pageNodes.current) {
        const box = element.getBoundingClientRect();

        // Distance from the anchor line to this page, zero while it spans it.
        const distance =
          box.top > anchor
            ? box.top - anchor
            : box.bottom < anchor
              ? anchor - box.bottom
              : 0;

        if (distance < bestDistance) {
          bestDistance = distance;
          best = index;
        }
      }

      if (best === null) return;

      // While a programmatic scroll is travelling, the pages it flies past
      // must not steal the highlight.
      if (scrollingTo.current !== null) {
        if (scrollingTo.current !== best) return;
        scrollingTo.current = null;
      }

      setCurrentPage(best);
    };

    // Coalesce to one measurement per frame; scroll fires far more often.
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };

    node.addEventListener('scroll', onScroll, { passive: true });
    update();

    return () => {
      node.removeEventListener('scroll', onScroll);
      if (frame) cancelAnimationFrame(frame);
    };
    // Keyed on page count and identity rather than the whole snapshot: `doc`
    // is replaced on every edit, which would tear this down and rebuild it
    // constantly. Zoom is included because it changes every page's height.
  }, [scrollNode, pageCount, documentId, zoom, setCurrentPage]);

  /** Brings a page into view; used by the pager and the thumbnail list. */
  const scrollToPage = useCallback((index: number) => {
    const node = pageNodes.current.get(index);
    if (!node) return;
    scrollingTo.current = index;
    node.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, []);

  // Selecting a page elsewhere — a thumbnail, a field in the panel — should
  // bring it into view here, but only when it is actually off screen.
  useEffect(() => {
    if (scrollingTo.current !== null) return;

    const node = pageNodes.current.get(currentPage);
    if (!node || !scrollNode) return;

    const pageBox = node.getBoundingClientRect();
    const viewBox = scrollNode.getBoundingClientRect();
    if (pageBox.bottom < viewBox.top || pageBox.top > viewBox.bottom) {
      scrollToPage(currentPage);
    }
  }, [currentPage, scrollNode, scrollToPage]);

  // Hold space to pan, the way every canvas app does it.
  //
  // The keydown must preventDefault or the browser's own space-scrolls-down
  // behaviour fires as well. Typing is excluded, or space would stop being a
  // space in every input on the page.
  useEffect(() => {
    function isTyping(target: EventTarget | null): boolean {
      if (!(target instanceof HTMLElement)) return false;
      if (target.isContentEditable) return true;
      const tag = target.tagName;
      return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.code !== 'Space' || event.repeat || isTyping(event.target)) return;
      event.preventDefault();
      setPanReady(true);
    }

    function onKeyUp(event: KeyboardEvent) {
      if (event.code !== 'Space') return;
      setPanReady(false);
      panning.current = null;
    }

    // Losing focus mid-drag would otherwise leave the cursor stuck in pan mode.
    function onBlur() {
      setPanReady(false);
      panning.current = null;
    }

    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
      window.removeEventListener('blur', onBlur);
    };
  }, []);

  // Arming a tool cancels any in-progress edit; they are different modes.
  useEffect(() => {
    if (pendingField) setEditing(null);
  }, [pendingField, setEditing]);

  // Switching document must not leave an editor open on a same-named field.
  useEffect(() => {
    setEditing(null);
  }, [doc?.id, setEditing]);

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

  return (
    <main className="viewer">
      <div
        className={`viewer__scroll${panReady ? ' viewer__scroll--pan' : ''}${
          panning.current ? ' viewer__scroll--panning' : ''
        }`}
        ref={setScrollNode}
        onPointerDown={(event) => {
          if (!panReady || !scrollNode) return;
          // Capture so the drag survives the pointer leaving the element, and
          // stop it reaching the page beneath, which would start a selection.
          event.preventDefault();
          event.stopPropagation();
          event.currentTarget.setPointerCapture(event.pointerId);
          panning.current = {
            x: event.clientX,
            y: event.clientY,
            left: scrollNode.scrollLeft,
            top: scrollNode.scrollTop,
          };
        }}
        onPointerMove={(event) => {
          const start = panning.current;
          if (!start || !scrollNode) return;
          // Drag the content with the cursor, so the scroll goes the other way.
          scrollNode.scrollLeft = start.left - (event.clientX - start.x);
          scrollNode.scrollTop = start.top - (event.clientY - start.y);
        }}
        onPointerUp={(event) => {
          panning.current = null;
          if (event.currentTarget.hasPointerCapture?.(event.pointerId)) {
            event.currentTarget.releasePointerCapture(event.pointerId);
          }
        }}
        onPointerCancel={() => {
          panning.current = null;
        }}
      >
        <div className="viewer__pages">
          {doc.pages.map((page) => (
            <PageCanvas
              key={page.index}
              page={page}
              documentId={doc.id}
              revision={doc.revision}
              zoom={zoom}
              renderingAvailable={renderingAvailable}
              fields={fields.filter(
                (field): field is PositionedField =>
                  field.pageIndex === page.index && isPositioned(field)
              )}
              selectedFields={selectedFields}
              textMode={textMode}
              textRuns={textRuns[page.index]}
              onNeedTextRuns={() => void loadTextRuns(page.index)}
              editingTextRun={editingTextRun}
              onEditTextRun={editTextRun}
              onCommitTextRun={(runId, value) =>
                void setTextRun(page.index, runId, value)
              }
              editing={editing}
              draft={fieldDraft}
              onDraft={setFieldDraft}
              onEditAdjacent={editAdjacentField}
              pendingField={pendingField !== null}
              register={(node) => {
                if (node) pageNodes.current.set(page.index, node);
                else pageNodes.current.delete(page.index);
              }}
              onClearSelection={() => {
                clearFieldSelection();
                setEditing(null);
              }}
              onSelectField={(name, modifiers) => {
                selectField(name, modifiers);
                setPanel('fields');
              }}
              onEditField={(name) => setEditing(name)}
              onToggleField={(field) =>
                void setFieldValue(field.name, isEmpty(field) ? 'On' : 'Off')
              }
              onMoveField={(name, rect) => void moveField(name, rect)}
              onCommitField={(field, value) => {
                setEditing(null);
                if (value !== (field.value ?? '')) void setFieldValue(field.name, value);
              }}
              onCancelEdit={() => setEditing(null)}
              onDraw={(rect) => void placeArmedField(page.index, rect)}
            />
          ))}
        </div>
      </div>

      <nav className="viewer__pager">
        <button
          onClick={() => scrollToPage(currentPage - 1)}
          disabled={currentPage === 0}
        >
          <ChevronLeft size={15} />
          Prev
        </button>
        <span>
          Page {currentPage + 1} of {doc.pageCount}
        </span>
        <button
          onClick={() => scrollToPage(currentPage + 1)}
          disabled={currentPage >= doc.pageCount - 1}
        >
          Next
          <ChevronRight size={15} />
        </button>
      </nav>
    </main>
  );
}

interface PageCanvasProps {
  page: PageInfo;
  documentId: number;
  revision: number;
  zoom: number;
  renderingAvailable: boolean;
  fields: PositionedField[];
  selectedFields: string[];
  textMode: boolean;
  textRuns: TextRun[] | undefined;
  onNeedTextRuns: () => void;
  editingTextRun: string | null;
  onEditTextRun: (key: string | null) => void;
  onCommitTextRun: (runId: number, value: string) => void;
  editing: string | null;
  draft: { name: string; value: string } | null;
  onDraft: (name: string, value: string) => void;
  onEditAdjacent: (direction: 1 | -1) => Promise<void>;
  pendingField: boolean;
  register: (node: HTMLDivElement | null) => void;
  onClearSelection: () => void;
  onSelectField: (name: string, modifiers: { toggle: boolean; range: boolean }) => void;
  onEditField: (name: string) => void;
  onToggleField: (field: PositionedField) => void;
  onMoveField: (name: string, rect: [number, number, number, number]) => void;
  onCommitField: (field: PositionedField, value: string) => void;
  onCancelEdit: () => void;
  onDraw: (rect: [number, number, number, number]) => void;
}

/** One page of the document, with its fields drawn over it. */
function PageCanvas({
  page,
  documentId,
  revision,
  zoom,
  renderingAvailable,
  fields,
  selectedFields,
  textMode,
  textRuns,
  onNeedTextRuns,
  editingTextRun,
  onEditTextRun,
  onCommitTextRun,
  editing,
  draft,
  onDraft,
  onEditAdjacent,
  pendingField,
  register,
  onClearSelection,
  onSelectField,
  onEditField,
  onToggleField,
  onMoveField,
  onCommitField,
  onCancelEdit,
  onDraw,
}: PageCanvasProps) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const [drawBox, setDrawBox] = useState<ScreenRect | null>(null);
  const drawStart = useRef<{ x: number; y: number } | null>(null);

  // Text is read per page and only while the mode is on: a long document would
  // otherwise pay to parse every page's drawing commands up front.
  const runs = textRuns;
  useEffect(() => {
    if (textMode && runs === undefined) onNeedTextRuns();
  }, [textMode, runs, onNeedTextRuns]);

  // CSS pixels per PDF point. The image is rasterized at VIEWER_DPI and scaled
  // down by CSS, which keeps it sharp on HiDPI displays.
  const scale = zoom;

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

    if (!start || !box || !pendingField) return;

    // A click without a meaningful drag still places a field, at the default
    // size for its kind — insisting on a drag would just be pedantic.
    const drawn =
      box.width >= MIN_DRAW_PX && box.height >= MIN_DRAW_PX
        ? box
        : {
            left: start.x,
            top: start.y,
            width: defaultFieldSize('text').width * scale,
            height: defaultFieldSize('text').height * scale,
          };

    onDraw(roundRect(screenRectToPdf(drawn, page, scale)));
  }

  return (
    <div className="page-slot" data-page={page.index} ref={register}>
      <div
        ref={surfaceRef}
        className={`page-surface${pendingField ? ' page-surface--drawing' : ''}`}
        style={{ width: page.widthPt * scale, height: page.heightPt * scale }}
        onPointerDown={(event) => {
          if (pendingField) {
            beginDraw(event);
            return;
          }
          if (event.target === event.currentTarget) onClearSelection();
        }}
        onPointerMove={continueDraw}
        onPointerUp={finishDraw}
        onPointerCancel={() => {
          drawStart.current = null;
          setDrawBox(null);
        }}
      >
        {renderingAvailable ? (
          <img
            className="page-surface__image"
            src={pageImageUrl(documentId, page.index, VIEWER_DPI, revision)}
            alt={`Page ${page.index + 1}`}
            loading="lazy"
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

        {textMode && (
          <TextLayer
            page={page}
            scale={scale}
            runs={runs}
            editingKey={editingTextRun}
            onEdit={onEditTextRun}
            onCommit={onCommitTextRun}
          />
        )}

        {fields.map((field) => (
          <FieldOverlay
            key={field.name}
            field={field}
            page={page}
            scale={scale}
            selected={selectedFields.includes(field.name)}
            editing={editing === field.name}
            draft={draft?.name === field.name ? draft.value : null}
            onDraft={(value) => onDraft(field.name, value)}
            onEditAdjacent={onEditAdjacent}
            inert={pendingField}
            onSelect={(modifiers) => onSelectField(field.name, modifiers)}
            onEdit={() => {
              if (!field.readOnly) onEditField(field.name);
            }}
            onToggle={() => onToggleField(field)}
            onGeometryChange={(rect) => onMoveField(field.name, rect)}
            onCommit={(value) => onCommitField(field, value)}
            onCancel={onCancelEdit}
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

      <span className="page-slot__number">{page.index + 1}</span>
    </div>
  );
}

/**
 * The page's own text, drawn as clickable boxes over the rendered image.
 *
 * Runs come from the page's drawing commands, so there is nothing to select in
 * the usual sense — each box is one stretch of text that can be replaced whole.
 */
function TextLayer({
  page,
  scale,
  runs,
  editingKey,
  onEdit,
  onCommit,
}: {
  page: PageInfo;
  scale: number;
  runs: TextRun[] | undefined;
  editingKey: string | null;
  onEdit: (key: string | null) => void;
  onCommit: (runId: number, value: string) => void;
}) {
  if (!runs) {
    return (
      <div className="text-layer__loading">Reading this page&rsquo;s text&hellip;</div>
    );
  }

  return (
    <>
      {runs.map((run) => {
        const key = `${page.index}:${run.id}`;
        return (
          <TextRunBox
            key={key}
            run={run}
            page={page}
            scale={scale}
            editing={editingKey === key}
            onEdit={() => onEdit(key)}
            onCancel={() => onEdit(null)}
            onCommit={(value) => onCommit(run.id, value)}
          />
        );
      })}
    </>
  );
}

function TextRunBox({
  run,
  page,
  scale,
  editing,
  onEdit,
  onCancel,
  onCommit,
}: {
  run: TextRun;
  page: PageInfo;
  scale: number;
  editing: boolean;
  onEdit: () => void;
  onCancel: () => void;
  onCommit: (value: string) => void;
}) {
  const box = pdfRectToScreen(run.rect, page, scale);
  const [draft, setDraft] = useState(run.text);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing) setDraft(run.text);
  }, [editing, run.text]);

  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [editing]);

  const style: React.CSSProperties = {
    left: box.left,
    top: box.top,
    width: box.width,
    height: box.height,
  };

  if (editing) {
    const commit = () => {
      if (draft === run.text) onCancel();
      else onCommit(draft);
    };

    return (
      <input
        ref={inputRef}
        className="text-run__editor"
        style={{ ...style, fontSize: Math.max(8, run.fontSize * scale) }}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          // Delete and the arrow keys belong to the text being typed, not to
          // the page behind it.
          event.stopPropagation();
          if (event.key === 'Enter') commit();
          if (event.key === 'Escape') onCancel();
        }}
      />
    );
  }

  return (
    <button
      type="button"
      className={`text-run${run.exactEdit ? '' : ' text-run--substitutes'}`}
      style={style}
      title={
        run.exactEdit
          ? `${run.text}

Click to edit (${run.fontName})`
          : `${run.text}

Click to edit. ${run.fontName} cannot be written to directly, so this will be redrawn in Helvetica.`
      }
      onClick={onEdit}
    />
  );
}

interface FieldOverlayProps {
  field: PositionedField;
  page: PageInfo;
  scale: number;
  selected: boolean;
  editing: boolean;
  /** In-progress text for this field, or null when it is not being typed into. */
  draft: string | null;
  onDraft: (value: string) => void;
  onEditAdjacent: (direction: 1 | -1) => Promise<void>;
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
  draft,
  onDraft,
  onEditAdjacent,
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

  // What the field reads as right now: the in-progress text if the user is
  // typing anywhere, otherwise what the document holds.
  const text = draft ?? field.value ?? '';
  const inputRef = useRef<HTMLInputElement>(null);
  const areaRef = useRef<HTMLTextAreaElement>(null);
  const selectRef = useRef<HTMLSelectElement>(null);

  const gesture = useRef<{
    handle: HandleId | null;
    startX: number;
    startY: number;
    origin: ScreenRect;
    latest: ScreenRect | null;
    moved: boolean;
  } | null>(null);

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

  if (editing) {
    const fontSize = Math.max(8, Math.min(box.height * 0.68, 20));
    const commit = () => onCommit(text);

    // Stop Delete, arrows and Ctrl+A reaching the global shortcut handler,
    // which would otherwise delete or move the field being typed into.
    const onKeyDown = (event: React.KeyboardEvent) => {
      event.stopPropagation();
      if (event.key === 'Escape') onCancel();
      if (event.key === 'Enter' && !field.multiline) commit();

      if (event.key === 'Tab') {
        // The browser would move focus to some other control on the page.
        // Walking the form is the only useful meaning of Tab here.
        event.preventDefault();
        void onEditAdjacent(event.shiftKey ? -1 : 1);
      }
    };

    if (field.kind === 'choice') {
      return (
        <select
          ref={selectRef}
          className="field-editor"
          style={{ ...style, fontSize }}
          value={text}
          onChange={(event) => {
            onDraft(event.target.value);
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
          value={text}
          maxLength={field.maxLength ?? undefined}
          onChange={(event) => onDraft(event.target.value)}
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
        value={text}
        maxLength={field.maxLength ?? undefined}
        onChange={(event) => onDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
    );
  }

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

    if (handle === null) {
      onSelect({ toggle: event.ctrlKey || event.metaKey, range: event.shiftKey });
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
      {draft !== null && draft !== (field.value ?? '') ? (
        /*
          The page image behind this box still shows the committed value, so
          text typed in the fields panel would otherwise not appear until it
          was saved and re-rendered. Painting it here keeps the two in step.
        */
        <span className="field-box__draft">{draft}</span>
      ) : (
        isEmpty(field) && <span className="field-box__label">{field.name}</span>
      )}

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
