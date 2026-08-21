//! Diagnostic: exercises the exact pipeline the viewer uses —
//! lopdf model -> serialize -> PDFium load -> render page.
//!
//! Usage:
//!   cargo run --example render_check --manifest-path src-tauri/Cargo.toml [file.pdf]
//!
//! With no argument it uses generated blank documents. With a path it opens
//! that file, then appends it to itself to reproduce the append case.

use nxtpdf_lib::pdf::{document, render};

fn main() {
    if let Err(message) = nxtpdf_lib::state::init_pdfium(None) {
        eprintln!("PDFium failed to load:\n{message}");
        std::process::exit(1);
    }
    println!("PDFium loaded.\n");

    let paths: Vec<String> = std::env::args().skip(1).collect();
    match paths.split_first() {
        Some((first, rest)) => from_files(first, rest),
        None => from_blank(),
    }
}

fn from_blank() {
    println!("--- blank document ---");
    let mut doc = document::blank().expect("blank");
    probe("blank, 1 page", &mut doc);

    println!("\n--- blank + appended blank ---");
    let extra = document::blank().expect("blank");
    document::append_document(&mut doc, extra).expect("append");
    probe("blank, 2 pages", &mut doc);
}

fn from_files(first: &str, rest: &[String]) {
    println!("--- open {first} ---");
    let mut doc = match document::open(std::path::Path::new(first)) {
        Ok(doc) => doc,
        Err(error) => {
            eprintln!("could not open: {error}");
            std::process::exit(1);
        }
    };
    probe("opened", &mut doc);

    // Append each remaining file in turn, re-probing after every one so a
    // failure is attributed to the file that caused it.
    for path in rest {
        println!(
            "
--- append {path} ---"
        );
        let extra = match document::open(std::path::Path::new(path)) {
            Ok(extra) => extra,
            Err(error) => {
                eprintln!("  could not open: {error}");
                continue;
            }
        };
        match document::append_document(&mut doc, extra) {
            Ok(()) => probe("after append", &mut doc),
            Err(error) => eprintln!("  APPEND FAILED: {error}"),
        }
    }
}

/// Serializes the model and tries to render every page, exactly as the
/// `nxtpdf://` handler does.
fn probe(label: &str, doc: &mut lopdf::Document) {
    let pages = document::page_count(doc);
    println!("{label}: {pages} page(s) in the lopdf model");

    let mut bytes = Vec::new();
    if let Err(error) = doc.save_to(&mut bytes) {
        eprintln!("  SERIALIZE FAILED: {error}");
        return;
    }
    println!("  serialized to {} bytes", bytes.len());

    match render::page_count(&bytes) {
        Ok(count) => println!("  PDFium sees {count} page(s)"),
        Err(error) => {
            eprintln!("  PDFIUM LOAD FAILED: {error}");
            return;
        }
    }

    for index in 0..pages {
        match render::render_page_png(&bytes, index, 144.0, true) {
            Ok(png) => println!("  page {}: rendered {} bytes of PNG", index + 1, png.len()),
            Err(error) => eprintln!("  page {}: RENDER FAILED: {error}", index + 1),
        }
    }
}
