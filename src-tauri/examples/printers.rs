//! Dumps every printer this machine can see, with the capabilities NXTPDF
//! relies on. Run it to sanity-check the Win32 layer against real hardware:
//!
//! ```text
//! cargo run --example printers --manifest-path src-tauri/Cargo.toml
//! ```
//!
//! This is the fastest way to find out whether a given driver actually reports
//! its trays — plenty of them do not.

fn main() {
    let printers = match nxtpdf_lib::printing::list_printers() {
        Ok(printers) => printers,
        Err(error) => {
            eprintln!("Could not list printers: {error}");
            std::process::exit(1);
        }
    };

    if printers.is_empty() {
        println!("No printers found.");
        return;
    }

    println!("Found {} printer(s).\n", printers.len());

    for printer in &printers {
        println!(
            "=== {}{} ===",
            printer.name,
            if printer.is_default {
                "  [default]"
            } else {
                ""
            }
        );
        println!("  driver : {}", printer.driver);
        println!("  port   : {}", printer.port);
        println!("  status : {}", printer.status);
        if !printer.location.is_empty() {
            println!("  where  : {}", printer.location);
        }

        match nxtpdf_lib::printing::capabilities(&printer.name) {
            Ok(caps) => {
                println!("  duplex : {}", yes_no(caps.supports_duplex));
                println!("  color  : {}", yes_no(caps.supports_color));
                println!("  collate: {}", yes_no(caps.supports_collate));
                println!("  copies : up to {}", caps.max_copies);

                println!("  trays  : {}", count(caps.paper_sources.len()));
                for source in &caps.paper_sources {
                    println!("      [{}] {}", source.id, source.name);
                }

                println!("  papers : {}", count(caps.paper_sizes.len()));
                for size in caps.paper_sizes.iter().take(8) {
                    println!(
                        "      [{}] {} ({:.0}x{:.0} mm)",
                        size.id, size.name, size.width_mm, size.height_mm
                    );
                }
                if caps.paper_sizes.len() > 8 {
                    println!("      ... and {} more", caps.paper_sizes.len() - 8);
                }

                if !caps.resolutions.is_empty() {
                    let list: Vec<String> = caps
                        .resolutions
                        .iter()
                        .map(|r| format!("{}x{}", r.x_dpi, r.y_dpi))
                        .collect();
                    println!("  dpi    : {}", list.join(", "));
                }

                println!(
                    "  default: {:?} / {:?}",
                    caps.defaults.duplex, caps.defaults.color
                );
            }
            Err(error) => println!("  capabilities unavailable: {error}"),
        }

        println!();
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn count(n: usize) -> String {
    if n == 0 {
        "none reported".to_string()
    } else {
        n.to_string()
    }
}
