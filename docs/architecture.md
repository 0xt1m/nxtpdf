# Architecture

Why NXTPDF is put together the way it is. Read this before making a structural
change; most of the decisions here exist because the obvious alternative fails
in a specific way.

## The shape of it

```
 React (webview)                    Rust (native)
┌──────────────────────┐          ┌────────────────────────────────┐
│ components/          │          │ commands.rs   ← IPC contract   │
│ state/store.ts       │  invoke  │                                │
│   DocumentInfo       │ ───────► │ state.rs      ← the open doc   │
│   snapshot           │ ◄─────── │   lopdf::Document (truth)      │
│                      │          │                                │
│ <img src=nxtpdf://…> │  images  │ pdf/document.rs  page tree     │
│                      │ ◄─────── │ pdf/forms.rs     AcroForm      │
└──────────────────────┘          │ pdf/render.rs    PDFium        │
                                  │ printing/windows.rs  GDI       │
                                  └────────────────────────────────┘
```

## Decision: lopdf is the truth, PDFium is only a renderer

Two libraries touch PDFs here, with a hard boundary between them.

- **lopdf** owns the document. Every edit — page moves, rotation, form values,
  new fields — mutates a `lopdf::Document`. It is what gets saved.
- **PDFium** never owns anything. To draw the current state, the lopdf model is
  serialized to bytes and those bytes are handed to PDFium.

This costs a serialization pass per render, mitigated by caching the bytes
against a `revision` counter that only changes on edit.

It buys three things:

1. **No ownership fight.** `pdfium-render`'s types borrow from the `Pdfium`
   instance. Storing a `PdfDocument` alongside its `Pdfium` in application state
   is self-referential and genuinely painful in Rust.
2. **A PDFium upgrade can never corrupt a saved file,** because PDFium is not
   in the write path at all.
3. **Editing works without PDFium.** If the library fails to load, the app
   still opens, reorganizes, and saves; only rendering and printing are lost.
   The backend emits `pdfium-unavailable` and the UI degrades rather than dying.

PDFium is held in a `OnceLock` for a `'static` lifetime, which is why the
`sync` feature is enabled — it is what marks `Pdfium` as `Send + Sync`.

### PDFium is single-threaded, and the type system will not tell you

`pdfium-render`'s `sync` feature adds `unsafe impl Send + Sync for Pdfium`. That
is an *assertion* to the compiler, not an implementation: the underlying library
keeps process-global state and does no locking of its own. Nothing in the type
system stops you calling it from several threads, and doing so does not produce
an error — it corrupts PDFium's internals and takes the process down with
`STATUS_ACCESS_VIOLATION` (0xc0000005).

This is easy to hit by accident. The page-image handler spawns a thread per
request, and opening an 8-page document fires nine requests at once (one per
thumbnail plus the viewer). That crashed the app on open.

Every entry point in `pdf/render.rs` therefore goes through `with_pdfium`, which
holds a process-wide `RENDER_LOCK` across the whole load-and-render sequence —
not just the load, because the `PdfDocument` and everything borrowed from it
touch the same global state. `concurrent_renders_do_not_crash` in that module is
the regression test; it fails by aborting the test binary if the lock is
removed.

Consequence: renders serialize. That is why thumbnails render at 32 DPI and page
images are cached hard by URL — the cheapest render is the one that never
happens. Lifting this properly means a dedicated render thread with a work
queue, so requests can be coalesced and stale ones dropped.

## Decision: page images bypass IPC

Rendered pages travel over a custom URI scheme (`nxtpdf://localhost/page/{index}/{dpi}/{revision}`)
rather than through `invoke`.

Tauri's IPC is JSON. A page raster is hundreds of kilobytes of binary, so
sending it through IPC means base64 — a 33% size penalty on top of a
text-protocol round trip, for every page, every render. The URI scheme lets the
webview fetch pages as ordinary images: binary, streamed, and cached by the
webview itself.

Neither the `documentId` nor the `revision` segment is read as data. Both exist
to keep the webview's cache honest: the response is marked `immutable`, so any
two requests sharing a URL share an image.

`revision` covers edits — without it the pre-edit image would be served
indefinitely. `documentId` covers *opening a different file*, and was added
after a real bug: a freshly opened document starts at revision 1, so its page 1
had the same URL as the previous document's page 1, and the old page was served
from cache until an edit happened to bump the counter. Form fields travel over
IPC and updated correctly, which made it look like a rendering problem rather
than a caching one.

The lesson generalizes: a cache key must name *everything* the response depends
on, and "which document" was invisible in the original key.

Platform note: Windows and Android webviews will not accept a registered custom
scheme, so Tauri routes them through `http://nxtpdf.localhost`. `src/lib/pageImage.ts`
handles both spellings and the CSP in `tauri.conf.json` permits both.

## Decision: every mutation returns a full snapshot

Commands that change the document return a fresh `DocumentInfo` rather than an
acknowledgement. The store replaces its snapshot wholesale; it never patches.

Patching would mean the frontend duplicating backend logic — "after deleting
page 3, page 4 becomes page 3" — and the two would eventually disagree. A full
snapshot is a few hundred bytes and makes divergence structurally impossible.

## Tabs

`Workspace` owns a `Vec<DocumentSession>` and an active id. Tabs are addressed
by **id, not index**: ids are monotonic and never reused, so closing a tab
cannot silently re-point a request that is already in flight — a page image
issued for tab 3 must not come back holding whatever moved into slot 3.

The page-image handler resolves against `with_document_id` rather than "the
active document" for the same reason: switching tabs while a render is pending
must not swap the answer.

Opening a path that is already open focuses that tab instead of loading it
twice. Two independent models of one file would let their saves overwrite each
other with no warning.

## Page tree flattening

Reordering pages rewrites the `/Kids` array of the root `Pages` node, which
flattens any nested page tree into a single level.

The subtlety: `/Resources`, `/MediaBox`, `/CropBox`, and `/Rotate` are
**inheritable**. A page can rely on a value held by an ancestor node. Flatten
naively and those pages lose their size or their fonts.

So `push_down_inherited` runs first, copying each inherited attribute onto the
page dictionaries that were relying on it. Only then is the tree rebuilt.
`prune_objects` then drops the now-unreachable intermediate nodes.

## Form appearances

A field has a value (`/V`) and a drawn appearance (`/AP` `/N`). Setting a value
alone and asking the viewer to repaint — via the form's `/NeedAppearances`
flag — is correct per the spec and honoured by mainstream viewers, but PDFium
ignores it. Since PDFium renders both the viewer and the printed output, a form
filled that way came out blank in this app.

So values are painted here: `set_field_value` writes `/V` **and** builds the
`/AP` `/N` stream to match, with the font metrics, auto-sizing and clipping
that implies. `/NeedAppearances` is still set, for viewers that would rather
repaint than trust ours.

Two repairs run when a document is opened, both driven by files that render
blank everywhere except Acrobat:

- `regenerate_missing_appearances` paints values that arrived with none.
- `reattach_orphaned_widgets` puts widgets back into their page's `/Annots`.
  A widget is only drawn because a page lists it; the AcroForm `/Fields` array
  only says the field exists. Several generators write the field tree and leave
  `/Annots` empty, which Acrobat papers over by rebuilding the list from each
  widget's `/P` back-pointer.

Even attached and painted, PDFium does not draw widget annotations during page
rendering — its form-fill module owns them. So `DocumentSession::bytes`, which
feeds both the viewer and the printer, flattens each appearance into the page
content as an ordinary form XObject. That happens on a throwaway copy; the
document that gets saved keeps its live, editable fields.

Checkboxes are subtler than they look: the "on" state is not `/On`. It is
whatever key appears in the widget's `/AP` `/N` dictionary that is not `/Off` —
often `/Yes`, sometimes `/1`. The code reads the actual state name from the
widget rather than assuming.

## Printing

The whole reason this app exists rather than being a web page.

### Why not `window.print()`

The webview's print path opens the browser's dialog. There is no API surface
for tray, duplex, or color. It is a dead end for the requirement.

### The GDI pipeline

1. `EnumPrintersW` → the device list.
2. `DeviceCapabilitiesW` → what this driver supports. `DC_BINS` gives tray ids
   and `DC_BINNAMES` their display names; `DC_DUPLEX` and `DC_COLORDEVICE`
   report two-sided and color support; `DC_PAPERS`/`DC_PAPERNAMES`/`DC_PAPERSIZE`
   the media list.
3. `DocumentPropertiesW` → the driver's default `DEVMODEW`, allocated at
   `dmSize + dmDriverExtra` bytes so driver-private settings travel with it.
4. Write our settings into that `DEVMODEW`, then `DocumentPropertiesW` again
   with `DM_IN_BUFFER | DM_OUT_BUFFER` so the driver can validate and normalize
   impossible combinations.
5. `CreateDCW` with the resulting `DEVMODEW`, then
   `StartDoc` / `StartPage` / `StretchDIBits` / `EndPage` / `EndDoc`.

### The two traps

**`dmFields` is not optional.** It is a bitmask declaring which `DEVMODEW`
members are meaningful. A driver ignores any member whose bit is absent. Setting
`dmDuplex` without setting `DM_DUPLEX` in `dmFields` silently prints
single-sided — this is the single most common cause of "duplex doesn't work".

**Bitmap orientation.** PDFium produces top-down rasters; a Windows DIB is
bottom-up by default. The `BITMAPINFOHEADER.biHeight` is therefore **negative**,
which declares top-down. Omit the minus sign and every page prints upside down.

Channel order also differs: PDFium gives RGBA, a 32bpp `BI_RGB` DIB wants BGRA,
hence `rgba_to_bgra`.

### Rasterize, don't replay

Each page is rendered to a bitmap and blitted onto the printer DC. Replaying
PDF vectors as GDI calls would be sharper at high DPI but would have to
reimplement the PDF imaging model against a much weaker one, with per-driver
quirks. Rasterizing behaves identically everywhere.

The cost is memory: a Letter page at 600 DPI is 5100×6600×4 bytes ≈ 135 MB.
`effective_dpi` therefore defaults to the device DPI capped at 300 (≈35 MB) and
hard-caps at 600, and pages are rendered one at a time.

### Copies are the driver's job

`dmCopies` is set rather than looping the render. One rasterization, N sheets —
much faster, and it enables the printer's own hardware collation.

### Drivers lie

Drivers routinely accept a `DEVMODEW` and then quietly change it. After the
merge step, `diff_warnings` compares what we asked for against what survived and
reports the differences to the UI as warnings. Better to tell the user the tray
was overridden than to let them wonder why the job came out of the wrong one.

## Coordinate systems

Three of them, and mixing them up is the classic source of "the field is in the
wrong place" bugs.

| Space | Origin | Y axis | Unit |
|---|---|---|---|
| PDF user space | bottom-left | up | points (1/72 in) |
| Screen | top-left | down | CSS pixels |
| Device (printer) | top-left of *printable area* | down | device pixels |

`src/lib/geometry.ts` converts between the first two, including page rotation.
The wrinkle: a widget's `/Rect` is always stored in **unrotated** user space,
while `PageInfo` reports post-rotation dimensions. `unrotatedSize` undoes that
swap before the transform is applied.

For the printer, note that GDI's coordinate origin is the printable area, not
the sheet. Aligning to the physical page — which "Actual size" does, so the
margins match what a ruler measures — means subtracting `PHYSICALOFFSETX/Y`.

## Editing page text

Everything on a page that is not a form field is drawing commands. PDF has no
notion of paragraphs, words, or even lines: text is a sequence of positioned
glyph-drawing operators, usually in a **subsetted** embedded font whose
program contains only the glyphs already used.

`pdf/text.rs` works with those directly.

**Reading** walks the content stream tracking the graphics and text state
(`cm`, `q`/`Q`, `Tm`/`Td`/`TD`/`T*`, `Tf`, `Tc`/`Tw`/`Tz`/`TL`/`Ts`), so every
show-text operator gets a position, a size and a bounding box. The bytes it
shows are decoded through the font: simple fonts via their encoding plus any
`/Differences`, composite fonts via `/ToUnicode`.

**Merging** happens before anything is shown to the user. Producers routinely
split a word across operators to apply kerning, so `MILEAGE` arrives as `MIL`,
`E`, `AGE`. Listing those separately would describe the file accurately and
help nobody. The rule is deterministic, so reading and editing group
identically. A word space drawn as a gap rather than as a space character is
restored, or `SALES TAX` reads back as `SALESTAX` — and would be written back
that way.

**Writing** takes one of two paths:

- The run's own font can spell the replacement → the string is rewritten in
  place. Visually seamless; the original typesetting is preserved.
- It cannot → the original operators are emptied and the text is redrawn in
  Helvetica. Visibly a substitution, which is why the outcome is reported back
  and surfaced in the UI.

Two things constrain the first path. An embedded font is a subset, so having an
*encoding* for a character does not mean the *outline* is present; the codes the
page already draws are the only available evidence, and an in-place edit stays
within them. And lopdf parses an inline image into an operation holding a
stream, which cannot be written back as valid inline-image syntax — a page
containing one is only ever appended to, never rewritten, and is the single
case where old text is covered by a white patch rather than removed.

What is still not here is **reflow**. A run is edited where it sits. Re-wrapping
a paragraph would require paragraph structure that the file does not contain,
and reconstructing it is a document-analysis project rather than a feature.

## Known limitations

- **Printing is Windows-only.** `printing/unsupported.rs` documents the CUPS
  mapping for whoever implements it. CUPS should take the PDF directly rather
  than a raster, which is both simpler and better quality than the GDI path.
- **No undo.** The command stack that would provide it does not exist yet, and
  retrofitting one into a document editor is unpleasant. Worth doing early.
- **Renders are serialized** behind `RENDER_LOCK`, so a page-heavy document
  fills its thumbnails one at a time. Correct, but a render thread with a work
  queue would let stale requests be dropped instead of queued.
- **`Orientation::Auto` follows the first page.** A single `DEVMODEW` governs
  the whole job, so a document mixing portrait and landscape cannot switch
  sheets mid-job. Doing it properly means splitting into several spool jobs.
- **`extract_pages` round-trips through bytes** to get an independent object
  graph. Deep-copying an arbitrary subgraph with shared resources by hand is
  where correctness bugs live; the round trip is slower and obviously right.
- **Form field creation covers text, checkbox, and dropdown.** Radio groups
  need a shared parent field with per-widget export values, which the current
  merged field/widget model does not express.
- **Edited text does not reflow**, and new characters missing from a subset
  font are redrawn in Helvetica rather than added to the font. Extending a
  subset means rewriting the font program.
