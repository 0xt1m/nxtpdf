//! NXTPDF application setup.

mod commands;
// These are `pub` so integration tests and the `printers` example can drive
// them without going through Tauri's command layer.
pub mod error;
pub mod pdf;
pub mod printing;
pub mod state;

use tauri::http::{Request, Response, StatusCode};
use tauri::{Emitter, Manager};

use crate::state::AppState;

/// Custom URI scheme used to stream rendered page images into the webview.
///
/// Page rasters are hundreds of kilobytes each. Sending them through the JSON
/// IPC channel would mean base64-encoding every one, roughly a 33% size penalty
/// on top of a text-protocol round trip. A URI scheme lets the webview fetch
/// them as ordinary images — binary, streamed, and cacheable by the webview.
///
/// URL shape: `page/{documentId}/{index}/{dpi}/{revision}`
///
/// Both `documentId` and `revision` exist to keep the webview's cache honest.
/// The response is marked immutable, so any two requests sharing a URL share
/// an image: without the id, page 1 of a newly opened file would collide with
/// page 1 of the previous one — both start at revision 1 — and the old page
/// would be served from cache until an edit happened to bump the counter.
const PAGE_SCHEME: &str = "nxtpdf";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Only the release-only single-instance registration below reassigns this.
    #[cfg_attr(debug_assertions, allow(unused_mut))]
    let mut builder = tauri::Builder::default();

    // Registered first, as Tauri requires. Double-clicking a PDF while NXTPDF
    // is already running would otherwise start a second copy, each with its
    // own document; instead the path is handed to the window already open.
    //
    // Debug builds opt out. The plugin identifies an instance by the app id,
    // which a dev build shares with the installed one - so launching
    // `pnpm app:dev` while the installed NXTPDF happened to be open handed the
    // arguments over to it and exited immediately, leaving no dev window and
    // no hint as to why.
    #[cfg(all(desktop, not(debug_assertions)))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(path) = pdf_path_from_args(&argv) {
                open_in_app(app, &path);
            }

            // Bring the existing window forward — the user just asked for it.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }));
    }

    builder
        // Without a logger installed, every `log::` call in this crate is
        // silently discarded — including the ones that report why PDFium or a
        // page render failed. Install it first so startup errors are visible.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ))
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Webview,
                ))
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .register_asynchronous_uri_scheme_protocol(PAGE_SCHEME, |context, request, responder| {
            let app = context.app_handle().clone();

            // Rasterizing is slow enough to matter; never do it on the caller's
            // thread, which would stall the webview's resource loading.
            std::thread::spawn(move || {
                responder.respond(serve_page(&app, &request));
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_document,
            commands::new_document,
            commands::close_document,
            commands::activate_document,
            commands::list_documents,
            commands::document_info,
            commands::save_document,
            commands::save_document_as,
            commands::rotate_page,
            commands::delete_pages,
            commands::move_page,
            commands::reorder_pages,
            commands::append_pdf,
            commands::extract_pages_to_file,
            commands::list_text_runs,
            commands::set_text_run,
            commands::list_form_fields,
            commands::set_form_field,
            commands::create_form_field,
            commands::set_form_field_rect,
            commands::set_form_field_font_size,
            commands::rename_form_field,
            commands::delete_form_field,
            commands::open_default_apps_settings,
            commands::list_printers,
            commands::default_printer,
            commands::printer_capabilities,
            commands::print_document,
        ])
        .setup(|app| {
            // Updating is a desktop concept; the plugins do not build for
            // mobile targets, so they are registered here behind a cfg rather
            // than in the builder chain above.
            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
                app.handle().plugin(tauri_plugin_process::init())?;
            }

            let resource_dir = app.path().resource_dir().ok();

            if let Err(message) = state::init_pdfium(resource_dir.as_deref()) {
                // Page ops and form editing still work without PDFium; only
                // rendering and printing are lost. Report it rather than
                // refusing to start.
                log::error!("{message}");
                let _ = app.emit("pdfium-unavailable", message);
            }

            // Debug builds can auto-open documents, which makes the render
            // path reproducible without driving the UI by hand. Semicolon
            // separated, one tab each:
            //   NXTPDF_DEV_OPEN="C:\a.pdf;C:\b.pdf" pnpm app:dev
            #[cfg(debug_assertions)]
            if let Ok(list) = std::env::var("NXTPDF_DEV_OPEN") {
                for path in list.split(';').filter(|entry| !entry.is_empty()) {
                    match pdf::document::open(std::path::Path::new(path)) {
                        Ok(document) => {
                            let state = app.state::<AppState>();
                            state.workspace.lock().open(document, Some(path.into()));
                            log::info!("NXTPDF_DEV_OPEN: opened {path}");
                        }
                        Err(error) => log::error!("NXTPDF_DEV_OPEN {path}: {error}"),
                    }
                }
            }

            // Semicolon-separated paths appended to the auto-opened document,
            // so the append path is reproducible without driving the UI.
            #[cfg(debug_assertions)]
            if let Ok(list) = std::env::var("NXTPDF_DEV_APPEND") {
                let state = app.state::<AppState>();
                for path in list.split(';').filter(|p| !p.is_empty()) {
                    let mut workspace = state.workspace.lock();
                    let Some(session) = workspace.active_mut() else {
                        continue;
                    };

                    match pdf::document::open(std::path::Path::new(path))
                        .and_then(|extra| pdf::document::append_document(&mut session.doc, extra))
                    {
                        Ok(()) => {
                            session.touch();
                            log::info!(
                                "NXTPDF_DEV_APPEND: {path} -> {} pages, revision {}",
                                pdf::document::page_count(&session.doc),
                                session.revision
                            );
                        }
                        Err(error) => log::error!("NXTPDF_DEV_APPEND {path}: {error}"),
                    }
                }
            }

            // A PDF double-clicked in Explorer arrives as an argument.
            if let Some(path) = pdf_path_from_args(&std::env::args().collect::<Vec<_>>()) {
                open_in_app(app.handle(), &path);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running NXTPDF");
}

/// Picks a PDF path out of a command line.
///
/// Explorer passes the file as the first argument, but a dev run adds flags of
/// its own, so scan for something that looks like a PDF rather than trusting
/// `argv[1]`.
fn pdf_path_from_args(args: &[String]) -> Option<std::path::PathBuf> {
    args.iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(std::path::PathBuf::from)
        .find(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                && path.is_file()
        })
}

/// Opens a document into the shared session and tells the UI to refresh.
fn open_in_app(app: &tauri::AppHandle, path: &std::path::Path) {
    match pdf::document::open(path) {
        Ok(document) => {
            let state = app.state::<AppState>();
            state
                .workspace
                .lock()
                .open(document, Some(path.to_path_buf()));
            log::info!("opened {} from the command line", path.display());

            // The frontend owns a snapshot, so it has to be told to re-read.
            let _ = app.emit("document-changed", ());
        }
        Err(error) => log::error!("could not open {}: {error}", path.display()),
    }
}

/// Parses `page/{documentId}/{index}/{dpi}/{revision}` out of a request.
fn parse_page_request(path: &str) -> Option<(state::DocumentId, usize, f32)> {
    let mut parts = path.trim_start_matches('/').split('/');

    if parts.next()? != "page" {
        return None;
    }

    let document = parts.next()?.parse::<state::DocumentId>().ok()?;
    let index = parts.next()?.parse::<usize>().ok()?;
    let dpi = parts.next()?.parse::<f32>().ok()?;
    Some((document, index, dpi))
}

fn error_response(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Renders the requested page to a PNG.
fn serve_page(app: &tauri::AppHandle, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    log::info!("page request: {}", request.uri());

    let Some((document, index, dpi)) = parse_page_request(request.uri().path()) else {
        log::error!("unparseable page URL: {}", request.uri());
        return error_response(
            StatusCode::BAD_REQUEST,
            "Expected a URL of the form page/{documentId}/{index}/{dpi}/{revision}",
        );
    };

    let state = app.state::<AppState>();

    // Addressed by id, not "whichever tab is active": a request still in
    // flight when the user switches tabs must resolve against the document it
    // was issued for.
    let rendered = state.with_document_id(document, |session| {
        // Borrow the serialized bytes only long enough to copy them; holding
        // the session lock across a render would block every other command.
        let bytes = session.bytes()?.to_vec();
        Ok(bytes)
    });

    let bytes = match rendered {
        Ok(bytes) => bytes,
        Err(error) => {
            log::error!("document {document}, page {index}: nothing to render ({error})");
            return error_response(StatusCode::NOT_FOUND, &error.to_string());
        }
    };

    match pdf::render::render_page_png(&bytes, index, dpi, true) {
        Ok(png) => {
            log::info!("page {index} rendered at {dpi} dpi: {} bytes", png.len());
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "image/png")
                // Every edit changes the URL, so a rendered page is immutable.
                .header("Cache-Control", "public, max-age=31536000, immutable")
                .header("Access-Control-Allow-Origin", "*")
                .body(png)
                .unwrap_or_else(|_| {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR, "encode failed")
                })
        }

        Err(error) => {
            log::error!("page {index}: render failed: {error}");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_page_request, pdf_path_from_args};

    #[test]
    fn ignores_the_executable_and_flags() {
        // Nothing here is an existing .pdf, so nothing should be picked up.
        let args: Vec<String> = ["nxtpdf.exe", "--flag", "-v"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pdf_path_from_args(&args), None);
    }

    #[test]
    fn ignores_paths_that_are_not_pdfs() {
        let args: Vec<String> = ["nxtpdf.exe", "notes.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pdf_path_from_args(&args), None);
    }

    #[test]
    fn finds_a_real_pdf_argument() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Report.PDF");
        std::fs::write(
            &path,
            b"%PDF-1.7
",
        )
        .expect("write");

        let args = vec![
            "nxtpdf.exe".to_string(),
            "--some-flag".to_string(),
            path.to_string_lossy().into_owned(),
        ];

        // The extension match is case-insensitive, as Explorer paths vary.
        assert_eq!(pdf_path_from_args(&args), Some(path));
    }

    #[test]
    fn parses_a_well_formed_page_url() {
        assert_eq!(parse_page_request("/page/2/3/150/7"), Some((2, 3, 150.0)));
    }

    #[test]
    fn tolerates_a_missing_leading_slash() {
        assert_eq!(parse_page_request("page/1/0/96/1"), Some((1, 0, 96.0)));
    }

    #[test]
    fn rejects_the_wrong_prefix() {
        assert_eq!(parse_page_request("/thumb/1/0/96/1"), None);
    }

    #[test]
    fn rejects_a_non_numeric_index() {
        assert_eq!(parse_page_request("/page/1/x/96/1"), None);
    }

    #[test]
    fn rejects_a_url_missing_the_document_id() {
        // The pre-tabs shape, which would otherwise parse as document 0.
        assert_eq!(parse_page_request("/page/0/96"), None);
    }

    #[test]
    fn rejects_a_truncated_url() {
        assert_eq!(parse_page_request("/page/1/0"), None);
    }
}
