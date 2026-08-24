//! Lists the editable text runs on a page, to check extraction against real
//! documents.
//!
//!   cargo run --example runs -- file.pdf [page]

use nxtpdf_lib::pdf::{document, text};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: runs <file.pdf> [page]");
        std::process::exit(2);
    };
    let page: usize = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);

    let doc = document::open(std::path::Path::new(&input)).expect("open");
    let runs = text::list_text_runs(&doc, page).expect("runs");

    println!("{} run(s) on page {}", runs.len(), page + 1);
    for run in runs.iter().take(40) {
        let [x0, y0, x1, y1] = run.rect;
        println!(
            "  #{:<4} {:>7.1},{:>7.1} {:>6.1}x{:<5.1} {:>5.1}pt {:<8} {} {:?}",
            run.id,
            x0,
            y0,
            x1 - x0,
            y1 - y0,
            run.font_size,
            run.font_name,
            if run.exact_edit { "exact" } else { "redraw" },
            run.text,
        );
    }
    if runs.len() > 40 {
        println!("  ... {} more", runs.len() - 40);
    }
}
