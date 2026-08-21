//! Single-page PNG render via redline's own pdfium-render path.
//!
//! Standalone counterpart to `tools/render-matrix.mjs`'s poppler/Quartz/Chrome legs -
//! this is the "pdfium" (redline's own render engine) entry in the cross-viewer
//! verification matrix (2026-08-21, owed since obs:nx5nqon8k8xrty2vljsz). Reuses
//! `PdfDiffEngine::render_page_full`, the exact function M6 Compare's pixel-diff tier
//! already calls, so this exercises the real render path rather than a bespoke one.
//! Annotations ARE rendered (pdfium-render's `render_annotations` default is `true`,
//! and `render_page_full` never overrides it - see the tile-render bug writeup in
//! `src-tauri/src/render` for why that default matters).
//!
//! Usage:
//!   PDFIUM_DYNAMIC_LIB_PATH=/path/to/libpdfium.dylib \
//!   cargo run --example render_page -- <pdf_path> <out_png> [page_idx=0] [dpi=150]

use std::path::Path;
use std::process;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: render_page <pdf_path> <out_png> [page_idx=0] [dpi=150]");
        eprintln!("  PDFIUM_DYNAMIC_LIB_PATH must be set.");
        process::exit(1);
    }

    let pdf_path = Path::new(&args[1]);
    let out_png = Path::new(&args[2]);
    let page_idx: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let dpi: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(150.0);

    let mut engine = pdf_diff::PdfDiffEngine::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize PDFium: {e}");
        eprintln!("Set PDFIUM_DYNAMIC_LIB_PATH to the path of libpdfium.dylib / pdfium.dll");
        process::exit(1);
    });

    let doc = engine.open(pdf_path).unwrap_or_else(|e| {
        eprintln!("Cannot open {}: {e}", pdf_path.display());
        process::exit(1);
    });

    let pages = engine.page_count(&doc).unwrap_or(0);
    if page_idx >= pages {
        eprintln!(
            "page_idx {page_idx} out of range - {} has {pages} page(s)",
            pdf_path.display()
        );
        process::exit(1);
    }

    let img = engine.render_page_full(&doc, page_idx, dpi).unwrap_or_else(|e| {
        eprintln!("Render failed: {e}");
        process::exit(1);
    });

    if let Some(parent) = out_png.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    img.save(out_png).unwrap_or_else(|e| {
        eprintln!("Could not save {}: {e}", out_png.display());
        process::exit(1);
    });

    println!(
        "pdfium: {} page {page_idx} @ {dpi} DPI -> {} ({}x{})",
        pdf_path.display(),
        out_png.display(),
        img.width(),
        img.height()
    );
}
