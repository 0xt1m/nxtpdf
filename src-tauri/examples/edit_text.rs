//! Edits a text run and renders the result, to check the round trip.
//!
//!   cargo run --example edit_text -- file.pdf <page> <run-id> "new text" out.png

use nxtpdf_lib::pdf::{document, render, text};
use nxtpdf_lib::state::Workspace;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, page, run_id, replacement, output] = args.as_slice() else {
        eprintln!("usage: edit_text <file.pdf> <page> <run-id> <text> <out.png>");
        std::process::exit(2);
    };

    let page: usize = page.parse().expect("page");
    let run_id: usize = run_id.parse().expect("run id");

    nxtpdf_lib::state::init_pdfium(None).expect("pdfium");
    let mut doc = document::open(std::path::Path::new(input)).expect("open");

    let before = text::list_text_runs(&doc, page).expect("runs");
    let found = before.iter().find(|run| run.id == run_id).expect("run id");
    println!("before: {:?} ({})", found.text, found.font_name);

    let outcome = text::set_text_run(&mut doc, page, run_id, replacement).expect("edit");
    println!("outcome: {outcome:?}");

    let after = text::list_text_runs(&doc, page).expect("runs");
    match after.iter().find(|run| run.id == run_id) {
        Some(run) => println!("after:  {:?}", run.text),
        None => println!("after:  (run no longer listed at that id)"),
    }

    // Render through the real session path, the same as the viewer does.
    let mut workspace = Workspace::default();
    let id = workspace.open(doc, None);
    let bytes = workspace.by_id_mut(id).unwrap().bytes().unwrap().to_vec();

    let png = render::render_page_png(&bytes, page, 144.0, true).expect("render");
    std::fs::write(output, &png).expect("write");
    println!("wrote {output}");
}
