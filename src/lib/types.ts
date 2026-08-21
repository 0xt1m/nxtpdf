/**
 * Mirrors the serde types in `src-tauri/src`.
 *
 * Rust structs are serialized with `rename_all = "camelCase"`, so field names
 * here match one-for-one. Keep the two in sync — there is no code generation
 * step, so a rename on either side is a compile error only on that side.
 */

// ---------------------------------------------------------------------------
// Document — pdf/document.rs
// ---------------------------------------------------------------------------

export interface PageInfo {
  /** 0-based index in the current page order. */
  index: number;
  /** Width in PDF points (1/72 inch), after rotation. */
  widthPt: number;
  /** Height in PDF points, after rotation. */
  heightPt: number;
  /** Clockwise rotation: 0, 90, 180, or 270. */
  rotation: number;
  hasFormFields: boolean;
}

export interface DocumentInfo {
  /** Identifies this tab, and namespaces its page-image URLs. */
  id: number;
  name: string;
  path: string | null;
  pageCount: number;
  dirty: boolean;
  /** Increments on every edit. Used to bust the page-image cache. */
  revision: number;
  pdfVersion: string;
  hasAcroForm: boolean;
  pages: PageInfo[];
}

// ---------------------------------------------------------------------------
// Forms — pdf/forms.rs
// ---------------------------------------------------------------------------

export type FieldKind =
  'text' | 'checkbox' | 'radio' | 'pushButton' | 'choice' | 'signature' | 'unknown';

export interface FormField {
  /** Fully qualified name; the identifier used when setting a value. */
  name: string;
  kind: FieldKind;
  value: string | null;
  pageIndex: number | null;
  /** `[x0, y0, x1, y1]` in PDF user space — origin is bottom-left. */
  rect: [number, number, number, number] | null;
  readOnly: boolean;
  required: boolean;
  multiline: boolean;
  password: boolean;
  maxLength: number | null;
  /** Text size in points. `0` means auto-size to fit the box. */
  fontSize: number | null;
  options: string[];
}

/** A field that has a rectangle, so it can be drawn or copied. */
export type PositionedField = FormField & {
  rect: NonNullable<FormField['rect']>;
};

/** A field with both a rectangle and a page, so it can be moved. */
export type PlacedField = PositionedField & {
  pageIndex: number;
};

export function isPositioned(field: FormField): field is PositionedField {
  return field.rect !== null;
}

export function isPlaced(field: FormField): field is PlacedField {
  return field.rect !== null && field.pageIndex !== null;
}

export interface NewField {
  pageIndex: number;
  name: string;
  kind: FieldKind;
  rect: [number, number, number, number];
  fontSize?: number | null;
  multiline?: boolean;
  required?: boolean;
  maxLength?: number | null;
  options?: string[];
}

// ---------------------------------------------------------------------------
// Printing — printing/types.rs
// ---------------------------------------------------------------------------

export interface PrinterInfo {
  name: string;
  driver: string;
  port: string;
  isDefault: boolean;
  status: string;
  location: string;
  comment: string;
  jobsQueued: number;
  /** A software device — PDF/XPS writer, fax — rather than real hardware. */
  isVirtual: boolean;
}

export interface PaperSource {
  /** Driver bin id — pass back verbatim to select this tray. */
  id: number;
  name: string;
}

export interface PaperSize {
  id: number;
  name: string;
  widthMm: number;
  heightMm: number;
}

export interface Resolution {
  xDpi: number;
  yDpi: number;
}

export type DuplexMode = 'simplex' | 'longEdge' | 'shortEdge';
export type ColorMode = 'color' | 'monochrome';
export type Orientation = 'auto' | 'portrait' | 'landscape';
export type PageScaling = 'actualSize' | 'fitToPage' | 'shrinkOversized';

export interface PrinterDefaults {
  duplex: DuplexMode;
  color: ColorMode;
  paperSourceId: number | null;
  paperSizeId: number | null;
  orientation: Orientation;
  copies: number;
  collate: boolean;
}

export interface PrinterCapabilities {
  printerName: string;
  supportsDuplex: boolean;
  supportsColor: boolean;
  supportsCollate: boolean;
  maxCopies: number;
  paperSources: PaperSource[];
  paperSizes: PaperSize[];
  resolutions: Resolution[];
  defaults: PrinterDefaults;
}

export interface PrintSettings {
  printerName: string;
  /** 0-based page indices in print order. `null` prints everything. */
  pages: number[] | null;
  copies: number;
  collate: boolean;
  duplex: DuplexMode;
  color: ColorMode;
  paperSourceId: number | null;
  paperSizeId: number | null;
  orientation: Orientation;
  scaling: PageScaling;
  /** Rasterization DPI. `null` follows the device, capped at 300. */
  renderDpi: number | null;
  reverseOrder: boolean;
  jobName: string;
}

export interface PrintJobResult {
  printerName: string;
  pagesPrinted: number;
  copies: number;
  renderDpi: number;
  /** Settings the driver refused, so the UI can say so plainly. */
  warnings: string[];
}
