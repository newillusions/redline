// Measurement harness for the image-aware Optimize pipeline against real corpus PDFs.
// Gated (#[ignore]) since the corpus is machine-local and gitignored - run explicitly:
//   cargo test --test diag_optimize_corpus -- --ignored --nocapture
use lopdf::Document;
use redline_lib::docops::{optimize_in_place_with_images, ImageQualityPreset};
use std::path::PathBuf;
use std::time::Instant;

fn corpus(rel: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bench")
        .join("corpus")
        .join(rel);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

fn measure_one(rel: &str, preset: ImageQualityPreset) {
    let Some(path) = corpus(rel) else {
        eprintln!("skip {rel}: corpus file not present");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let t0 = Instant::now();
    let mut doc = Document::load_from(std::io::Cursor::new(&bytes)).unwrap();
    let load_time = t0.elapsed();

    let t1 = Instant::now();
    let stats = optimize_in_place_with_images(&mut doc, 2, Some(preset)).unwrap();
    let optimize_time = t1.elapsed();

    let t2 = Instant::now();
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    let save_time = t2.elapsed();

    let before = bytes.len() as u64;
    let after = out.len() as u64;
    println!(
        "{rel} | preset={preset:?} | {before} -> {after} bytes ({:.1}% reduction) | \
         images: total={} recompressed={} downsampled={} passed_through={} | \
         image_bytes: {} -> {} | skip_reasons={:?} | \
         timing: load={load_time:?} optimize={optimize_time:?} save={save_time:?}",
        100.0 * (before as i64 - after as i64) as f64 / before as f64,
        stats.images_total,
        stats.images_recompressed,
        stats.images_downsampled,
        stats.images_passed_through,
        stats.image_bytes_before,
        stats.image_bytes_after,
        stats.skip_reasons,
    );

    // Sanity: output must still be a loadable PDF with the same page count.
    let out_doc = Document::load_from(std::io::Cursor::new(&out)).unwrap();
    assert_eq!(
        out_doc.get_pages().len(),
        Document::load_from(std::io::Cursor::new(&bytes))
            .unwrap()
            .get_pages()
            .len(),
        "page count must be preserved by {rel} at {preset:?}"
    );
}

#[test]
#[ignore]
fn measure_c1_typical_all_presets() {
    for preset in [
        ImageQualityPreset::High,
        ImageQualityPreset::Balanced,
        ImageQualityPreset::Small,
    ] {
        measure_one("c1-typical/c1-contract-691pg-A4.pdf", preset);
    }
}

#[test]
#[ignore]
fn measure_c4_dense_all_presets() {
    for preset in [
        ImageQualityPreset::High,
        ImageQualityPreset::Balanced,
        ImageQualityPreset::Small,
    ] {
        measure_one("c4-dense/c4-overall-plan-A0.pdf", preset);
    }
}

#[test]
#[ignore]
fn measure_c2_large_balanced() {
    measure_one(
        "c2-large/c2-observatory-854pg-largeformat.pdf",
        ImageQualityPreset::Balanced,
    );
}

// ---------------------------------------------------------------------------
// Render fidelity: the optimized output must not just parse as a valid PDF (already
// checked above via `Document::load_from` + page-count), it must actually OPEN and
// RENDER through the real PDFium engine this app uses for display — proving the
// rewritten image streams (new /Filter, /Width, /Height, /ColorSpace) are consistent
// with each other and with the rest of the document, not just individually well-formed.
//
// Gated on PDFIUM_DYNAMIC_LIB_PATH like every other PDFium-touching test in this repo
// (see render::tests::corpus's doc comment) — self-skips when unset, never runs in CI.
// ---------------------------------------------------------------------------

fn pdfium_available() -> bool {
    std::env::var("PDFIUM_DYNAMIC_LIB_PATH").is_ok()
}

fn render_fidelity_check(rel: &str, preset: ImageQualityPreset) {
    if !pdfium_available() {
        eprintln!("skip render fidelity for {rel}: PDFIUM_DYNAMIC_LIB_PATH not set");
        return;
    }
    let Some(path) = corpus(rel) else {
        eprintln!("skip {rel}: corpus file not present");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let mut doc = Document::load_from(std::io::Cursor::new(&bytes)).unwrap();
    let page_count_before = doc.get_pages().len();
    optimize_in_place_with_images(&mut doc, 2, Some(preset)).unwrap();
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("optimized.pdf");
    std::fs::write(&work, &out).unwrap();

    use redline_lib::render::{RenderEngine, TileRequest};
    let mut engine = RenderEngine::new().expect("PDFium must load (PDFIUM_DYNAMIC_LIB_PATH set)");
    engine
        .open_document(work, "fidelity-check".into(), None)
        .unwrap_or_else(|e| panic!("PDFium must open the optimized {rel}: {e}"));

    // Render one tile per page (bounded — large corpora have hundreds of pages) to
    // prove the recompressed images composite correctly, not just that page 0 works.
    let pages_to_sample = page_count_before.min(5) as u32;
    for page_index in 0..pages_to_sample {
        let req = TileRequest {
            doc_id: "fidelity-check".into(),
            page_index,
            tile_size_css: 512,
            tile_x: 0,
            tile_y: 0,
            zoom: 1.0,
            dpr: 2.0,
        };
        let tile = engine.render_tile(&req).unwrap_or_else(|e| {
            panic!("PDFium must render page {page_index} of optimized {rel}: {e}")
        });
        assert!(
            !tile.png_base64.is_empty(),
            "rendered tile for page {page_index} of {rel} must be non-empty"
        );
    }
    println!("render fidelity OK: {rel} at {preset:?}, {pages_to_sample} page(s) sampled");
}

#[test]
#[ignore]
fn render_fidelity_c1_typical_balanced() {
    render_fidelity_check(
        "c1-typical/c1-contract-691pg-A4.pdf",
        ImageQualityPreset::Balanced,
    );
}

#[test]
#[ignore]
fn render_fidelity_c4_dense_small() {
    render_fidelity_check("c4-dense/c4-overall-plan-A0.pdf", ImageQualityPreset::Small);
}

/// Dump page-0 renders of the ORIGINAL and OPTIMIZED c1 corpus file as real PNGs to a
/// fixed scratch path, for direct human/visual inspection - not just "PDFium didn't
/// error", but "the page still looks like a legitimate document page after lossy image
/// recompression". Path printed to stdout so a human (or another tool) can open it.
#[test]
#[ignore]
fn dump_before_after_pngs_c1_page0() {
    dump_before_after_page("c1-typical/c1-contract-691pg-A4.pdf", 0, "page0");
}

/// Same as above but for a page deep in the document, since page 0 of c1 is a
/// text-only title page (no images to show a recompression effect on).
#[test]
#[ignore]
fn dump_before_after_pngs_c1_page_with_images() {
    dump_before_after_page("c1-typical/c1-contract-691pg-A4.pdf", 100, "page100");
}

fn dump_before_after_page(rel: &str, page_index: u32, label_suffix: &str) {
    if !pdfium_available() {
        eprintln!("skip: PDFIUM_DYNAMIC_LIB_PATH not set");
        return;
    }
    let Some(path) = corpus(rel) else {
        eprintln!("skip: corpus file not present");
        return;
    };
    let bytes = std::fs::read(&path).unwrap();

    let mut optimized_doc = Document::load_from(std::io::Cursor::new(&bytes)).unwrap();
    optimize_in_place_with_images(&mut optimized_doc, 2, Some(ImageQualityPreset::Balanced))
        .unwrap();
    let mut optimized_bytes = Vec::new();
    optimized_doc.save_to(&mut optimized_bytes).unwrap();

    let out_dir = PathBuf::from("/tmp/redline-optimize-fidelity");
    std::fs::create_dir_all(&out_dir).unwrap();
    let orig_path = out_dir.join(format!("c1-{label_suffix}-original.pdf"));
    let opt_path = out_dir.join(format!("c1-{label_suffix}-optimized.pdf"));
    std::fs::write(&orig_path, &bytes).unwrap();
    std::fs::write(&opt_path, &optimized_bytes).unwrap();

    use base64::Engine as _;
    use redline_lib::render::{RenderEngine, TileRequest};

    for (label, doc_path) in [("original", &orig_path), ("optimized", &opt_path)] {
        let mut engine = RenderEngine::new().unwrap();
        engine
            .open_document(doc_path.clone(), format!("dump-{label}"), None)
            .unwrap();
        let req = TileRequest {
            doc_id: format!("dump-{label}"),
            page_index,
            tile_size_css: 900,
            tile_x: 0,
            tile_y: 0,
            zoom: 1.0,
            dpr: 1.0,
        };
        let tile = engine.render_tile(&req).unwrap();
        let png_bytes = base64::engine::general_purpose::STANDARD
            .decode(&tile.png_base64)
            .unwrap();
        let png_path = out_dir.join(format!("c1-{label_suffix}-{label}.png"));
        std::fs::write(&png_path, &png_bytes).unwrap();
        println!("wrote {}", png_path.display());
    }
    println!(
        "original: {} bytes, optimized: {} bytes",
        bytes.len(),
        optimized_bytes.len()
    );
}
