//! Headless pdf-diff example — Tauri-FREE proof of concept.
//!
//! This binary demonstrates `PdfDiffEngine` running with zero Tauri involvement.
//! The same binary can be compiled and run by cad-export-api (Linux/axum, no GUI).
//!
//! Usage:
//!   PDFIUM_DYNAMIC_LIB_PATH=/path/to/libpdfium.dylib \
//!   cargo run --example headless -- old.pdf new.pdf [dpi] [tolerance]
//!
//! Arguments:
//!   <path_a>     PDF file A (e.g. DD issue)
//!   <path_b>     PDF file B (e.g. IFC issue, rebadge of A)
//!   [dpi]        render DPI for pixel diff (default: 150)
//!   [tolerance]  per-channel pixel delta counted as "same" (default: 5)

use std::path::Path;
use std::process;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: headless <path_a> <path_b> [dpi] [tolerance]");
        eprintln!("  PDFIUM_DYNAMIC_LIB_PATH must be set.");
        process::exit(1);
    }

    let path_a = Path::new(&args[1]);
    let path_b = Path::new(&args[2]);
    let dpi: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(150.0);
    let tolerance: u8 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);

    println!("pdf-diff headless example");
    println!("  A: {}", path_a.display());
    println!("  B: {}", path_b.display());
    println!("  DPI: {dpi}, tolerance: {tolerance}");
    println!();

    // ---------- open engine ----------
    let mut engine = pdf_diff::PdfDiffEngine::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize PDFium: {e}");
        eprintln!("Set PDFIUM_DYNAMIC_LIB_PATH to the path of libpdfium.dylib / pdfium.dll");
        process::exit(1);
    });

    let doc_a = engine.open(path_a).unwrap_or_else(|e| {
        eprintln!("Cannot open {}: {e}", path_a.display());
        process::exit(1);
    });
    let doc_b = engine.open(path_b).unwrap_or_else(|e| {
        eprintln!("Cannot open {}: {e}", path_b.display());
        process::exit(1);
    });

    let pages_a = engine.page_count(&doc_a).unwrap_or(0);
    let pages_b = engine.page_count(&doc_b).unwrap_or(0);
    println!("Page count: A={pages_a}, B={pages_b}");
    println!();

    // ---------- tier 1: text diff on page 0 ----------
    println!("=== TIER 1: Text-layer diff (page 0) ===");
    match engine.text_diff(&doc_a, &doc_b, 0, 0) {
        Ok(text) => {
            println!("  char_sequence_match: {}", text.char_sequence_match);
            println!("  position_deltas count: {}", text.position_deltas.len());
            if !text.position_deltas.is_empty() {
                // Print top-5 largest position shifts.
                let mut deltas = text.position_deltas.clone();
                deltas.sort_by(|a, b| {
                    let da = (a.delta_x * a.delta_x + a.delta_y * a.delta_y) as f64;
                    let db = (b.delta_x * b.delta_x + b.delta_y * b.delta_y) as f64;
                    db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
                });
                println!("  Top-5 position shifts:");
                for d in deltas.iter().take(5) {
                    println!(
                        "    '{}': Δx={:.2} Δy={:.2} pts",
                        d.ch, d.delta_x, d.delta_y
                    );
                }
            }
        }
        Err(e) => eprintln!("  text_diff error: {e}"),
    }
    println!();

    // ---------- tier 2: pixel diff ----------
    println!("=== TIER 2: Pixel diff at {dpi} DPI (page 0) ===");
    let img_a = engine.render_page_full(&doc_a, 0, dpi).unwrap_or_else(|e| {
        eprintln!("  Render A failed: {e}");
        process::exit(1);
    });
    let img_b = engine.render_page_full(&doc_b, 0, dpi).unwrap_or_else(|e| {
        eprintln!("  Render B failed: {e}");
        process::exit(1);
    });
    println!("  Image size A: {}x{}", img_a.width(), img_a.height());
    println!("  Image size B: {}x{}", img_b.width(), img_b.height());

    match pdf_diff::pixel_diff(&img_a, &img_b, tolerance) {
        Ok(px) => {
            println!("  passed: {}", px.passed);
            println!("  changed_pct: {:.4}%", px.changed_pct);
            println!("  max_pixel_delta: {}", px.max_pixel_delta);

            // Save diff image.
            let out = "pdf-diff-output.png";
            if let Err(e) = px.diff_image.save(out) {
                eprintln!("  Warning: could not save diff image: {e}");
            } else {
                println!("  Diff image saved to: {out}");
            }
        }
        Err(e) => eprintln!("  pixel_diff error: {e}"),
    }

    println!();
    println!("Done — no Tauri framework involved.");
}
