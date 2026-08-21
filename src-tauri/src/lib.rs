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
/// URL shape: `page/{index}/{dpi}/{revision}`
///
/// `revision` is not read by the handler; it exists so that editing the
/// document produces a new URL and the webview's own cache cannot serve a
/// stale page.
const PAGE_SCHEME: &str = "nxtpdf";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            commands::document_info,
            commands::save_document,
            commands::save_document_as,
            commands::rotate_page,
            commands::delete_pages,
            commands::move_page,
            commands::reorder_pages,
            commands::append_pdf,
            commands::extract_pages_to_file,
            commands::list_form_fields,
            commands::set_form_field,
            commands::create_form_field,
            commands::delete_form_field,
            commands::list_printers,
            commands::default_printer,
            commands::printer_capabilities,
            commands::print_document,
        ])
        .setup(|app| {
            let resource_dir = app.path().resource_dir().ok();

            if let Err(message) = state::init_pdfium(resource_dir.as_deref()) {
                // Page ops and form editing still work without PDFium; only
                // rendering and printing are lost. Report it rather than
                // refusing to start.
                log::error!("{message}");
                let _ = app.emit("pdfium-unavailable", message);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running NXTPDF");
}

/// Parses `page/{index}/{dpi}/{revision}` out of a page-scheme request.
fn parse_page_request(path: &str) -> Option<(usize, f32)> {
    let mut parts = path.trim_start_matches('/').split('/');

    if parts.next()? != "page" {
        return None;
    }

    let index = parts.next()?.parse::<usize>().ok()?;
    let dpi = parts.next()?.parse::<f32>().ok()?;
    Some((index, dpi))
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
    let Some((index, dpi)) = parse_page_request(request.uri().path()) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Expected a URL of the form page/{index}/{dpi}/{revision}",
        );
    };

    let state = app.state::<AppState>();

    let rendered = state.with_document(|session| {
        // Borrow the serialized bytes only long enough to copy them; holding
        // the session lock across a render would block every other command.
        let bytes = session.bytes()?.to_vec();
        Ok(bytes)
    });

    let bytes = match rendered {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(StatusCode::NOT_FOUND, &error.to_string());
        }
    };

    match pdf::render::render_page_png(&bytes, index, dpi, true) {
        Ok(png) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "image/png")
            // Every edit changes the URL, so a rendered page is immutable.
            .header("Cache-Control", "public, max-age=31536000, immutable")
            .header("Access-Control-Allow-Origin", "*")
            .body(png)
            .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "encode failed")),

        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_page_request;

    #[test]
    fn parses_a_well_formed_page_url() {
        assert_eq!(parse_page_request("/page/3/150/7"), Some((3, 150.0)));
    }

    #[test]
    fn tolerates_a_missing_leading_slash() {
        assert_eq!(parse_page_request("page/0/96/1"), Some((0, 96.0)));
    }

    #[test]
    fn rejects_the_wrong_prefix() {
        assert_eq!(parse_page_request("/thumb/0/96/1"), None);
    }

    #[test]
    fn rejects_a_non_numeric_index() {
        assert_eq!(parse_page_request("/page/x/96/1"), None);
    }

    #[test]
    fn rejects_a_truncated_url() {
        assert_eq!(parse_page_request("/page/0"), None);
    }
}
