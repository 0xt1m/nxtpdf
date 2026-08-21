# NXTPDF

A simple, fast PDF editor with the print control Acrobat gives you and browsers do not.

Tauri 2 + React 19 + TypeScript on the front, Rust on the back.

## What it does today

- **Open several files at once** — tabs, each remembering its page and zoom.
  Opening a file that is already open focuses its tab rather than making a
  second copy.
- **View and navigate** — page rendering via PDFium, thumbnails, zoom
  (Ctrl+scroll), keyboard shortcuts throughout.
- **Reorganize** — rotate, delete, drag-to-reorder, append another PDF, extract
  pages to a new file.
- **Fill forms** — read and write existing AcroForm fields: text, checkboxes,
  radios, dropdowns. Edit in place on the page, or in the side panel.
- **Build forms** — arm a tool from the Add strip, then drag on the page to
  size the field. Move and resize with the mouse or the arrow keys, rename,
  copy and paste, and set text size including auto-fit.
- **Print properly** — choose the **tray**, one- or two-sided (long or short
  edge), **color or black and white**, paper size, orientation, copies,
  collation, page range, and scaling. Every option is read from the driver, so
  you only see what your printer can actually do, and virtual devices
  (Print to PDF, XPS, Fax) are hidden by default.
- **Update itself** — checks on launch, downloads in the background, and
  installs silently when you close the app. A banner offers **Update now** if
  you would rather not wait. See [docs/releasing.md](docs/releasing.md).
- **Open PDFs from Explorer** — registers as a `.pdf` handler; double-clicking
  a file loads it into the running window as a new tab.

## What it does not do

- **Editing existing page text.** Deliberately out of scope — see
  [docs/architecture.md](docs/architecture.md) for why it is the expensive part.
- **Flattening forms.** Field values are written with `/NeedAppearances`, so
  viewers regenerate the visuals. A renderer that ignores that flag shows stale
  appearances.
- **Undo.** There is no command stack yet; Save early.
- **Radio group creation, digital signatures, redaction.**
- **Printing on macOS or Linux.** The print backend is Windows-only; everything
  else is cross-platform. See `src-tauri/src/printing/unsupported.rs` for what a
  CUPS implementation needs.

## Requirements

| Tool                      | Version | Notes                                                                       |
| ------------------------- | ------- | --------------------------------------------------------------------------- |
| Rust                      | 1.77+   | Install via [rustup](https://rustup.rs)                                     |
| Node.js                   | 20+     |                                                                             |
| pnpm                      | 9+      | `npm i -g pnpm`                                                             |
| Visual Studio Build Tools | 2022    | "Desktop development with C++" workload — provides MSVC and the Windows SDK |
| WebView2 Runtime          | any     | Preinstalled on Windows 11                                                  |

## Getting started

```bash
pnpm install          # also downloads PDFium into src-tauri/lib/
pnpm app:dev          # launches the app with hot reload
```

`pnpm install` runs `scripts/fetch-pdfium.mjs`, which pulls the prebuilt PDFium
library for your platform from
[bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries). That
binary is ~7 MB and is deliberately **not** committed. Re-run it by hand with
`pnpm setup:pdfium`.

## Scripts

| Command                     | Does                                                                  |
| --------------------------- | --------------------------------------------------------------------- |
| `pnpm app:dev`              | Run the desktop app with hot reload                                   |
| `pnpm app:build`            | Build installers (MSI + NSIS) into `src-tauri/target/release/bundle/` |
| `pnpm dev`                  | Frontend only, in a browser (backend commands will fail)              |
| `pnpm typecheck`            | `tsc -b` across both TS projects                                      |
| `pnpm lint` / `pnpm format` | ESLint / Prettier                                                     |
| `pnpm rs:lint`              | `cargo clippy -D warnings`                                            |
| `pnpm rs:test`              | `cargo test`                                                          |
| `pnpm check`                | Everything above — run before pushing                                 |
| `pnpm setup:pdfium`         | Re-download the PDFium library                                        |

Releases must be signed or the auto-updater will reject them — see
[docs/releasing.md](docs/releasing.md) before cutting one.

### Diagnosing a printer

The fastest way to see what a driver actually reports:

```bash
cargo run --example printers --manifest-path src-tauri/Cargo.toml
```

It lists every printer with its trays, duplex and color support, paper sizes,
and resolutions. If a tray is missing here, the driver is not reporting it and
no amount of UI work will surface it.

## Layout

```
src/                      React frontend
  components/             UI, one file per region
  lib/         ipc.ts     the only place a Tauri command name is written
               types.ts   mirrors the Rust serde types
               geometry.ts PDF <-> screen coordinate maths
  state/store.ts          Zustand store; holds a snapshot, never patches it
src-tauri/
  src/pdf/     document.rs page tree surgery (lopdf)
               forms.rs    AcroForm read/write/create (lopdf)
               render.rs   rasterization (PDFium)
  src/printing/types.rs    platform-neutral print vocabulary
               windows.rs  GDI + DEVMODE: the tray/duplex/color implementation
  src/commands.rs          the entire IPC contract
  examples/printers.rs     printer capability dump
scripts/fetch-pdfium.mjs   downloads the native PDF engine
```

See [docs/architecture.md](docs/architecture.md) for how the pieces fit and why.

## Licensing

NXTPDF is MIT licensed. Its dependencies are all permissive and safe for
commercial use:

| Component                      | License          |
| ------------------------------ | ---------------- |
| Tauri, Rust, React, TypeScript | MIT / Apache-2.0 |
| PDFium (Google)                | BSD-3-Clause     |
| lopdf                          | MIT              |
| `windows` crate (Microsoft)    | MIT / Apache-2.0 |

Nothing here requires a license from Adobe: PDF is ISO 32000 and Adobe grants a
royalty-free patent license for it.

**Do not add `mupdf-rs`.** MuPDF is AGPL, which would force this project open
source or require a commercial license from Artifex.

Run `cargo about generate` before shipping to produce the third-party notice
file installers should carry.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for commit conventions and the
pre-push checklist.
