import { useEffect, useMemo, useState } from 'react';
import * as ipc from '@/lib/ipc';
import { useStore } from '@/state/store';
import type {
  ColorMode,
  DuplexMode,
  Orientation,
  PageScaling,
  PrintJobResult,
  PrintSettings,
  PrinterCapabilities,
  PrinterInfo,
} from '@/lib/types';

interface PrintDialogProps {
  onClose: () => void;
}

/**
 * The print dialog.
 *
 * Every control here is driven by what the driver actually reports through
 * `DeviceCapabilitiesW` — trays come from `DC_BINNAMES`, duplex and color from
 * `DC_DUPLEX`/`DC_COLORDEVICE`. Options the device does not support are
 * disabled rather than hidden, so it is obvious *why* something is unavailable.
 */
export function PrintDialog({ onClose }: PrintDialogProps) {
  const doc = useStore((s) => s.doc);
  const selectedPages = useStore((s) => s.selectedPages);

  const [printers, setPrinters] = useState<PrinterInfo[]>([]);
  const [caps, setCaps] = useState<PrinterCapabilities | null>(null);
  const [printerName, setPrinterName] = useState('');
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<PrintJobResult | null>(null);

  // --- Job settings ---
  const [copies, setCopies] = useState(1);
  const [collate, setCollate] = useState(true);
  const [duplex, setDuplex] = useState<DuplexMode>('simplex');
  const [color, setColor] = useState<ColorMode>('color');
  const [paperSourceId, setPaperSourceId] = useState<number | null>(null);
  const [paperSizeId, setPaperSizeId] = useState<number | null>(null);
  const [orientation, setOrientation] = useState<Orientation>('auto');
  const [scaling, setScaling] = useState<PageScaling>('fitToPage');
  const [reverseOrder, setReverseOrder] = useState(false);
  const [rangeMode, setRangeMode] = useState<'all' | 'selected' | 'custom'>(
    selectedPages.length > 0 ? 'selected' : 'all'
  );
  const [customRange, setCustomRange] = useState('');

  // Load the printer list once, then select the system default.
  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        const list = await ipc.listPrinters();
        if (cancelled) return;
        setPrinters(list);

        const preferred = list.find((p) => p.isDefault) ?? list[0];
        if (preferred) setPrinterName(preferred.name);
        else setError('No printers were found on this system.');
      } catch (err) {
        if (!cancelled) setError(ipc.errorMessage(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  // Re-query capabilities whenever the selected printer changes, and seed the
  // controls from that driver's own defaults.
  useEffect(() => {
    if (!printerName) return;
    let cancelled = false;

    (async () => {
      setError(null);
      try {
        const capabilities = await ipc.printerCapabilities(printerName);
        if (cancelled) return;

        setCaps(capabilities);
        const defaults = capabilities.defaults;
        setDuplex(capabilities.supportsDuplex ? defaults.duplex : 'simplex');
        setColor(capabilities.supportsColor ? defaults.color : 'monochrome');
        setPaperSourceId(defaults.paperSourceId);
        setPaperSizeId(defaults.paperSizeId);
        setCollate(defaults.collate);
      } catch (err) {
        if (!cancelled) {
          setCaps(null);
          setError(ipc.errorMessage(err));
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [printerName]);

  const pages = useMemo(() => {
    if (!doc) return null;
    if (rangeMode === 'all') return null;
    if (rangeMode === 'selected') return selectedPages.length > 0 ? selectedPages : null;
    return parsePageRange(customRange, doc.pageCount);
  }, [rangeMode, customRange, selectedPages, doc]);

  const rangeInvalid =
    rangeMode === 'custom' &&
    customRange.trim() !== '' &&
    pages !== null &&
    pages.length === 0;

  if (!doc) return null;

  async function handlePrint() {
    setSubmitting(true);
    setError(null);
    setResult(null);

    const settings: PrintSettings = {
      printerName,
      pages,
      copies,
      collate,
      duplex,
      color,
      paperSourceId,
      paperSizeId,
      orientation,
      scaling,
      renderDpi: null,
      reverseOrder,
      jobName: doc?.name ?? 'NXTPDF Document',
    };

    try {
      setResult(await ipc.printDocument(settings));
    } catch (err) {
      setError(ipc.errorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  const selectedPrinter = printers.find((p) => p.name === printerName);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal modal--wide"
        role="dialog"
        aria-modal="true"
        aria-label="Print"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="modal__header">
          <h2>Print</h2>
          <button className="modal__close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </header>

        {loading ? (
          <div className="modal__body">
            <p>Looking for printers…</p>
          </div>
        ) : (
          <div className="modal__body print-grid">
            {/* ---------------- Destination ---------------- */}
            <section className="print-section">
              <h3>Destination</h3>

              <label className="form-control">
                <span>Printer</span>
                <select
                  value={printerName}
                  onChange={(event) => setPrinterName(event.target.value)}
                  disabled={printers.length === 0}
                >
                  {printers.map((printer) => (
                    <option key={printer.name} value={printer.name}>
                      {printer.name}
                      {printer.isDefault ? ' (default)' : ''}
                    </option>
                  ))}
                </select>
              </label>

              {selectedPrinter && (
                <p className="hint">
                  {selectedPrinter.status}
                  {selectedPrinter.location ? ` · ${selectedPrinter.location}` : ''}
                </p>
              )}

              <label className="form-control">
                <span>Copies</span>
                <input
                  type="number"
                  min={1}
                  max={caps?.maxCopies ?? 999}
                  value={copies}
                  onChange={(event) =>
                    setCopies(Math.max(1, Number(event.target.value) || 1))
                  }
                />
              </label>

              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={collate}
                  disabled={copies < 2 || caps?.supportsCollate === false}
                  onChange={(event) => setCollate(event.target.checked)}
                />
                <span>Collate</span>
              </label>
            </section>

            {/* ---------------- Pages ---------------- */}
            <section className="print-section">
              <h3>Pages</h3>

              <label className="radio">
                <input
                  type="radio"
                  checked={rangeMode === 'all'}
                  onChange={() => setRangeMode('all')}
                />
                <span>All {doc.pageCount} pages</span>
              </label>

              <label className="radio">
                <input
                  type="radio"
                  checked={rangeMode === 'selected'}
                  disabled={selectedPages.length === 0}
                  onChange={() => setRangeMode('selected')}
                />
                <span>
                  Selected ({selectedPages.length} page
                  {selectedPages.length === 1 ? '' : 's'})
                </span>
              </label>

              <label className="radio">
                <input
                  type="radio"
                  checked={rangeMode === 'custom'}
                  onChange={() => setRangeMode('custom')}
                />
                <span>Range</span>
              </label>

              <input
                type="text"
                placeholder="e.g. 1-3, 5, 8-10"
                value={customRange}
                disabled={rangeMode !== 'custom'}
                onChange={(event) => setCustomRange(event.target.value)}
              />
              {rangeInvalid && (
                <p className="form-error">
                  No valid pages in that range (document has {doc.pageCount}).
                </p>
              )}

              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={reverseOrder}
                  onChange={(event) => setReverseOrder(event.target.checked)}
                />
                <span>Reverse order</span>
              </label>
            </section>

            {/* ---------------- Paper handling ---------------- */}
            <section className="print-section">
              <h3>Paper</h3>

              <label className="form-control">
                <span>
                  Tray
                  {caps && caps.paperSources.length === 0 && ' (none reported)'}
                </span>
                <select
                  value={paperSourceId ?? ''}
                  disabled={!caps || caps.paperSources.length === 0}
                  onChange={(event) =>
                    setPaperSourceId(
                      event.target.value === '' ? null : Number(event.target.value)
                    )
                  }
                >
                  <option value="">Printer default</option>
                  {caps?.paperSources.map((source) => (
                    <option key={source.id} value={source.id}>
                      {source.name}
                    </option>
                  ))}
                </select>
              </label>

              <label className="form-control">
                <span>Paper size</span>
                <select
                  value={paperSizeId ?? ''}
                  disabled={!caps || caps.paperSizes.length === 0}
                  onChange={(event) =>
                    setPaperSizeId(
                      event.target.value === '' ? null : Number(event.target.value)
                    )
                  }
                >
                  <option value="">Printer default</option>
                  {caps?.paperSizes.map((size) => (
                    <option key={size.id} value={size.id}>
                      {size.name}
                      {size.widthMm > 0
                        ? ` — ${Math.round(size.widthMm)}×${Math.round(size.heightMm)} mm`
                        : ''}
                    </option>
                  ))}
                </select>
              </label>

              <label className="form-control">
                <span>Orientation</span>
                <select
                  value={orientation}
                  onChange={(event) => setOrientation(event.target.value as Orientation)}
                >
                  <option value="auto">Match page</option>
                  <option value="portrait">Portrait</option>
                  <option value="landscape">Landscape</option>
                </select>
              </label>
            </section>

            {/* ---------------- Output ---------------- */}
            <section className="print-section">
              <h3>Output</h3>

              <label className="form-control">
                <span>
                  Two-sided
                  {caps && !caps.supportsDuplex && ' — not supported'}
                </span>
                <select
                  value={duplex}
                  disabled={!caps?.supportsDuplex}
                  onChange={(event) => setDuplex(event.target.value as DuplexMode)}
                >
                  <option value="simplex">One-sided</option>
                  <option value="longEdge">Two-sided, flip on long edge</option>
                  <option value="shortEdge">Two-sided, flip on short edge</option>
                </select>
              </label>

              <label className="form-control">
                <span>
                  Color
                  {caps && !caps.supportsColor && ' — monochrome device'}
                </span>
                <select
                  value={color}
                  disabled={!caps?.supportsColor}
                  onChange={(event) => setColor(event.target.value as ColorMode)}
                >
                  <option value="color">Color</option>
                  <option value="monochrome">Black and white</option>
                </select>
              </label>

              <label className="form-control">
                <span>Scaling</span>
                <select
                  value={scaling}
                  onChange={(event) => setScaling(event.target.value as PageScaling)}
                >
                  <option value="fitToPage">Fit to printable area</option>
                  <option value="shrinkOversized">Shrink oversized only</option>
                  <option value="actualSize">Actual size</option>
                </select>
              </label>
            </section>
          </div>
        )}

        {error && <div className="modal__error">{error}</div>}

        {result && (
          <div className="modal__success">
            <p>
              Sent {result.pagesPrinted} page(s) × {result.copies} to {result.printerName}{' '}
              at {result.renderDpi} DPI.
            </p>
            {result.warnings.map((warning) => (
              <p key={warning} className="modal__warning">
                {warning}
              </p>
            ))}
          </div>
        )}

        <footer className="modal__footer">
          <button onClick={onClose}>{result ? 'Close' : 'Cancel'}</button>
          <button
            className="button--primary"
            onClick={handlePrint}
            disabled={submitting || loading || !printerName || rangeInvalid}
          >
            {submitting ? 'Sending…' : 'Print'}
          </button>
        </footer>
      </div>
    </div>
  );
}

/**
 * Parses a human page range ("1-3, 5, 8-10") into 0-based indices.
 *
 * Out-of-range and malformed parts are dropped rather than rejected, so a
 * trailing comma while typing does not blank the whole selection. Duplicates
 * are removed but the caller's ordering is preserved.
 */
export function parsePageRange(input: string, pageCount: number): number[] {
  const seen = new Set<number>();
  const pages: number[] = [];

  const add = (oneBased: number) => {
    const index = oneBased - 1;
    if (index < 0 || index >= pageCount || seen.has(index)) return;
    seen.add(index);
    pages.push(index);
  };

  for (const part of input.split(',')) {
    const token = part.trim();
    if (token === '') continue;

    const span = token.match(/^(\d+)\s*-\s*(\d+)$/);
    if (span) {
      const from = Number(span[1]);
      const to = Number(span[2]);
      const step = from <= to ? 1 : -1;
      for (let page = from; step > 0 ? page <= to : page >= to; page += step) {
        add(page);
      }
      continue;
    }

    const single = token.match(/^(\d+)$/);
    if (single) add(Number(single[1]));
  }

  return pages;
}
