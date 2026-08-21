import { useState } from 'react';
import { useStore } from '@/state/store';
import { pageImageUrl, THUMBNAIL_DPI } from '@/lib/pageImage';
import type { PageInfo } from '@/lib/types';

/**
 * Page thumbnails with multi-select and drag-to-reorder.
 *
 * Reordering uses the native HTML drag-and-drop API rather than a dependency:
 * a vertical list of ~100 items is well within what it handles cleanly.
 */
export function PageSidebar() {
  const doc = useStore((s) => s.doc);
  const currentPage = useStore((s) => s.currentPage);
  const selectedPages = useStore((s) => s.selectedPages);
  const busy = useStore((s) => s.busy);
  const renderingAvailable = useStore((s) => s.renderingAvailable);

  const setCurrentPage = useStore((s) => s.setCurrentPage);
  const togglePageSelection = useStore((s) => s.togglePageSelection);
  const selectAllPages = useStore((s) => s.selectAllPages);
  const clearSelection = useStore((s) => s.clearSelection);
  const rotatePage = useStore((s) => s.rotatePage);
  const deleteSelectedPages = useStore((s) => s.deleteSelectedPages);
  const movePage = useStore((s) => s.movePage);

  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  if (!doc) return null;

  function handleDrop(target: number) {
    if (dragIndex !== null && dragIndex !== target) {
      void movePage(dragIndex, target);
    }
    setDragIndex(null);
    setDropIndex(null);
  }

  return (
    <aside className="sidebar">
      <div className="sidebar__header">
        <span>
          {doc.pageCount} page{doc.pageCount === 1 ? '' : 's'}
        </span>
        <div className="sidebar__header-actions">
          <button onClick={selectAllPages} disabled={busy}>
            All
          </button>
          <button onClick={clearSelection} disabled={busy || selectedPages.length === 0}>
            None
          </button>
        </div>
      </div>

      {selectedPages.length > 0 && (
        <div className="sidebar__selection-bar">
          <span>{selectedPages.length} selected</span>
          <button
            className="button--danger"
            onClick={deleteSelectedPages}
            disabled={busy || selectedPages.length >= doc.pageCount}
            title={
              selectedPages.length >= doc.pageCount
                ? 'A document must keep at least one page'
                : 'Delete selected pages'
            }
          >
            Delete
          </button>
        </div>
      )}

      <ol className="thumbnails">
        {doc.pages.map((page) => (
          <Thumbnail
            key={page.index}
            page={page}
            revision={doc.revision}
            isCurrent={page.index === currentPage}
            isSelected={selectedPages.includes(page.index)}
            isDropTarget={dropIndex === page.index}
            renderingAvailable={renderingAvailable}
            busy={busy}
            onSelect={(additive) => {
              setCurrentPage(page.index);
              togglePageSelection(page.index, additive);
            }}
            onRotate={(degrees) => void rotatePage(page.index, degrees)}
            onDragStart={() => setDragIndex(page.index)}
            onDragOver={() => setDropIndex(page.index)}
            onDrop={() => handleDrop(page.index)}
            onDragEnd={() => {
              setDragIndex(null);
              setDropIndex(null);
            }}
          />
        ))}
      </ol>
    </aside>
  );
}

interface ThumbnailProps {
  page: PageInfo;
  revision: number;
  isCurrent: boolean;
  isSelected: boolean;
  isDropTarget: boolean;
  renderingAvailable: boolean;
  busy: boolean;
  onSelect: (additive: boolean) => void;
  onRotate: (degrees: number) => void;
  onDragStart: () => void;
  onDragOver: () => void;
  onDrop: () => void;
  onDragEnd: () => void;
}

function Thumbnail({
  page,
  revision,
  isCurrent,
  isSelected,
  isDropTarget,
  renderingAvailable,
  busy,
  onSelect,
  onRotate,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
}: ThumbnailProps) {
  const classes = [
    'thumbnail',
    isCurrent && 'thumbnail--current',
    isSelected && 'thumbnail--selected',
    isDropTarget && 'thumbnail--drop-target',
  ]
    .filter(Boolean)
    .join(' ');

  // Preserve aspect ratio so the box does not jump when the image loads.
  const aspectRatio = page.widthPt > 0 ? page.widthPt / page.heightPt : 0.77;

  return (
    <li
      className={classes}
      draggable={!busy}
      onDragStart={onDragStart}
      onDragOver={(event) => {
        event.preventDefault();
        onDragOver();
      }}
      onDrop={(event) => {
        event.preventDefault();
        onDrop();
      }}
      onDragEnd={onDragEnd}
      onClick={(event) => onSelect(event.ctrlKey || event.metaKey || event.shiftKey)}
    >
      <div className="thumbnail__frame" style={{ aspectRatio }}>
        {renderingAvailable ? (
          <img
            src={pageImageUrl(page.index, THUMBNAIL_DPI, revision)}
            alt={`Page ${page.index + 1}`}
            loading="lazy"
            draggable={false}
          />
        ) : (
          <div className="thumbnail__placeholder">no preview</div>
        )}
      </div>

      <div className="thumbnail__meta">
        <span className="thumbnail__number">{page.index + 1}</span>
        {page.hasFormFields && (
          <span className="badge" title="This page has form fields">
            F
          </span>
        )}
        {page.rotation !== 0 && <span className="badge">{page.rotation}°</span>}
      </div>

      <div className="thumbnail__actions">
        <button
          title="Rotate left"
          disabled={busy}
          onClick={(event) => {
            event.stopPropagation();
            onRotate(-90);
          }}
        >
          ⟲
        </button>
        <button
          title="Rotate right"
          disabled={busy}
          onClick={(event) => {
            event.stopPropagation();
            onRotate(90);
          }}
        >
          ⟳
        </button>
      </div>
    </li>
  );
}
