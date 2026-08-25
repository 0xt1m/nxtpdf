//! Lists the editable text runs on a page, and can draw their boxes onto a
//! render so the geometry can be checked against the actual glyphs.
//!
//!   cargo run --example runs -- file.pdf [page] [out.png]

use nxtpdf_lib::pdf::{document, render, text};
use nxtpdf_lib::state::Workspace;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: runs <file.pdf> [page] [out.png]");
        std::process::exit(2);
    };
    let page: usize = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let output = args.next();

    let doc = document::open(std::path::Path::new(&input)).expect("open");
    let runs = text::list_text_runs(&doc, page).expect("runs");

    println!("{} run(s) on page {}", runs.len(), page + 1);
    for run in runs.iter().take(50) {
        let [x0, y0, x1, y1] = run.rect;
        println!(
            "  #{:<4} x {:>7.1}..{:<7.1} y {:>7.1}..{:<7.1} {:>5.1}pt {:<8} {} {:?}",
            run.id,
            x0,
            x1,
            y0,
            y1,
            run.font_size,
            run.font_name,
            if run.exact_edit { "exact" } else { "redraw" },
            run.text,
        );
    }
    if runs.len() > 50 {
        println!("  ... {} more", runs.len() - 50);
    }

    let Some(output) = output else { return };

    // Draw each box over the page at the same DPI, so any offset between a box
    // and its glyphs is directly visible.
    const DPI: f32 = 144.0;
    nxtpdf_lib::state::init_pdfium(None).expect("pdfium");

    let mut workspace = Workspace::default();
    let id = workspace.open(doc, None);
    let bytes = workspace.by_id_mut(id).unwrap().bytes().unwrap().to_vec();
    let raster = render::render_page(&bytes, page, DPI, true).expect("render");

    let mut image =
        image::RgbaImage::from_raw(raster.width, raster.height, raster.rgba).expect("raster");
    let scale = DPI / 72.0;
    let height = raster.height as f32;

    for run in &runs {
        let [x0, y0, x1, y1] = run.rect;
        // PDF space has its origin bottom-left; the image's is top-left.
        let left = (x0 * scale) as i32;
        let right = (x1 * scale) as i32;
        let top = (height - y1 * scale) as i32;
        let bottom = (height - y0 * scale) as i32;

        let mut plot = |x: i32, y: i32| {
            if x >= 0 && y >= 0 && (x as u32) < raster.width && (y as u32) < raster.height {
                image.put_pixel(x as u32, y as u32, image::Rgba([220, 0, 0, 255]));
            }
        };

        for x in left..=right {
            plot(x, top);
            plot(x, bottom);
        }
        for y in top..=bottom {
            plot(left, y);
            plot(right, y);
        }
    }

    image.save(&output).expect("write");
    println!("wrote {output}");
}
