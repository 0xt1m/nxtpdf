//! Runs the open-time form repairs over a directory of PDFs and reports what
//! each one needed, so a fix can be checked against real files in bulk.

use nxtpdf_lib::pdf::{forms, render};
use nxtpdf_lib::state::Workspace;

fn ink(bytes: &[u8]) -> Option<usize> {
    let raster = render::render_page(bytes, 0, 96.0, true).ok()?;
    Some(
        raster
            .rgba
            .chunks_exact(4)
            .filter(|p| p[0] < 200 || p[1] < 200 || p[2] < 200)
            .count(),
    )
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: sweep <dir>");
    nxtpdf_lib::state::init_pdfium(None).expect("pdfium");

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let mut doc = match lopdf::Document::load(&path) {
            Ok(doc) => doc,
            Err(err) => {
                println!("{name:<48} LOAD FAILED: {err}");
                continue;
            }
        };

        let mut before = Vec::new();
        doc.clone().save_to(&mut before).ok();
        let before_ink = ink(&before);

        let repaired = forms::regenerate_missing_appearances(&mut doc);
        let reattached = forms::reattach_orphaned_widgets(&mut doc);

        let mut workspace = Workspace::default();
        let id = workspace.open(doc, None);
        let session = workspace.by_id_mut(id).unwrap();
        let after_ink = session.bytes().ok().and_then(ink);

        let delta = match (before_ink, after_ink) {
            (Some(a), Some(b)) if b > a => format!("+{}", b - a),
            (Some(a), Some(b)) if a > b => format!("-{}", a - b),
            (Some(_), Some(_)) => "same".to_string(),
            _ => "render failed".to_string(),
        };

        println!("{name:<48} appearances={repaired:<3} reattached={reattached:<3} ink {delta}");
    }
}
