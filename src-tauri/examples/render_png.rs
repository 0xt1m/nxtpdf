//! Renders one page to a PNG through the exact path the viewer and printer
//! use, so the output can be looked at directly.
//!
//!   cargo run --example render_png --manifest-path src-tauri/Cargo.toml -- file.pdf out.png

use nxtpdf_lib::pdf::{document, render};
use nxtpdf_lib::state::Workspace;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: render_png <file.pdf> <out.png>");
        std::process::exit(2);
    };

    nxtpdf_lib::state::init_pdfium(None).expect("pdfium");

    // Drive the real session, so this exercises the same serialization the
    // viewer and the printer get - including appearance flattening.
    let doc = document::open(std::path::Path::new(&input)).expect("open");
    let mut workspace = Workspace::default();
    let id = workspace.open(doc, None);
    let session = workspace.by_id_mut(id).expect("session");
    let bytes = session.bytes().expect("serialize").to_vec();

    println!("serialized {} bytes", bytes.len());

    let png = render::render_page_png(&bytes, 0, 144.0, true).expect("render");
    std::fs::write(&output, &png).expect("write");
    println!("wrote {output}");
}
