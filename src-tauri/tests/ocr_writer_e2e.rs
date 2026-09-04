//! Auto-OCR Phase 2c-i end-to-end proof: a real scanned-CAD fixture (image
//! only, no extractable text — same corpus `ocr_benchmark.rs` scores) goes
//! through the REAL Tesseract engine, then the REAL `ocr::writer`, then gets
//! reopened and searched via the REAL PDFium in-document search path
//! (`render::RenderEngine::search_page`, the same API `commands::search`
//! wraps for the app's own search UI) AND via the REAL `lopdf::extract_text`
//! path (`search::indexer::extract_pdf_text`'s own extraction call, proving
//! the Tantivy folder-index pickup path independently of PDFium).
//!
//! This is the "add tests that a scanned fixture becomes searchable"
//! deliverable named by the Phase 2c dispatch — a synthetic unit test
//! proves the writer's mechanics (see `ocr::writer`'s own tests); this test
//! proves the FULL real pipeline: real OCR output -> real writer -> real
//! search, on a fixture that starts with provably zero extractable text.
//!
//! `#[ignore]`, matching `ocr_benchmark.rs`'s own convention — needs a real
//! PDFium dylib (`scripts/fetch-pdfium.sh`) AND a working Tesseract install
//! (see that file's doc comment for both). Run:
//!
//!   PDFIUM_DYNAMIC_LIB_PATH=.../libpdfium.dylib \
//!     cargo test --features ocr --test ocr_writer_e2e -- --ignored --nocapture

#![cfg(feature = "ocr")]

use std::env;
use std::fs;
use std::path::PathBuf;

use redline_lib::docops;
use redline_lib::ocr::writer::{write_ocr_pdf, DEFAULT_MIN_CONFIDENCE};
use redline_lib::ocr::OcrEngineHandle;
use redline_lib::render::RenderEngine;
use redline_lib::text::SearchOptions;

#[test]
#[ignore]
fn scanned_fixture_becomes_searchable_end_to_end() {
    if env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
        eprintln!(
            "SKIP scanned_fixture_becomes_searchable_end_to_end: PDFIUM_DYNAMIC_LIB_PATH not set \
             (see scripts/fetch-pdfium.sh)"
        );
        return;
    }

    let tessdata_dir = env::var_os("REDLINE_TESSDATA_DIR").map(PathBuf::from);
    let mut ocr = match OcrEngineHandle::load(tessdata_dir.as_deref()) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!(
                "SKIP scanned_fixture_becomes_searchable_end_to_end: OcrEngineHandle::load \
                 failed ({e}) — is a Tesseract install with English tessdata reachable? Set \
                 REDLINE_TESSDATA_DIR or TESSDATA_PREFIX."
            );
            return;
        }
    };

    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/fixtures/ocr");
    let source_pdf = fixtures_dir.join("a-plain-horizontal.pdf");
    assert!(
        source_pdf.exists(),
        "fixture not found at {:?} — same corpus ocr_benchmark.rs uses",
        source_pdf
    );

    // Search term from a-plain-horizontal.expected.json: a short, distinctive
    // single word unlikely to appear anywhere else on the fixture page.
    const SEARCH_TERM: &str = "NORTH";
    const RENDER_DPI: f32 = 300.0;

    let mut engine = RenderEngine::new().expect("RenderEngine::new failed");

    // --- Step 1: baseline — confirm the SOURCE fixture has NO extractable
    // text at all (image-only scan), so a later positive search result is
    // proof the writer actually added something, not a pre-existing layer.
    let baseline_doc_id = "ocr-e2e-baseline";
    engine
        .open_document(source_pdf.clone(), baseline_doc_id.to_string(), None)
        .unwrap_or_else(|e| panic!("failed to open source fixture: {e}"));
    let baseline_hits = engine
        .search_page(baseline_doc_id, 0, SEARCH_TERM, &SearchOptions::default())
        .expect("search_page on baseline should not error, even with no text layer");
    assert!(
        baseline_hits.is_empty(),
        "fixture already has an extractable '{SEARCH_TERM}' — not the image-only baseline \
         this test needs; got hits: {baseline_hits:?}"
    );

    // --- Step 2: real OCR over the real raster.
    let raster = engine
        .render_page_full(baseline_doc_id, 0, RENDER_DPI)
        .expect("render_page_full failed");
    let recognized = ocr.recognize_page(&raster).expect("recognize_page failed");
    assert!(
        !recognized.is_empty(),
        "OCR recognized zero lines on a fixture the benchmark scores at 100% recall — \
         Tesseract/tessdata setup problem, not a writer problem"
    );
    engine.close_document(baseline_doc_id);

    // --- Step 3: real writer — embed the recognized lines as an invisible
    // text layer into a COPY of the fixture bytes (never mutate the
    // checked-in fixture file itself).
    let source_bytes = fs::read(&source_pdf).expect("read source fixture bytes");
    let ocred_bytes = write_ocr_pdf(
        &source_bytes,
        &[(0, recognized.clone())],
        DEFAULT_MIN_CONFIDENCE,
    )
    .expect("write_ocr_pdf failed");
    assert!(
        ocred_bytes.len() > source_bytes.len(),
        "OCR'd output should be larger than the source (a real text layer was added)"
    );

    let tmp_dir = env::temp_dir().join(format!("redline-ocr-e2e-{}", std::process::id()));
    fs::create_dir_all(&tmp_dir).expect("create scratch dir");
    let ocred_path = tmp_dir.join("a-plain-horizontal-ocred.pdf");
    fs::write(&ocred_path, &ocred_bytes).expect("write OCR'd fixture copy");

    // --- Step 4a: PDFium in-document search finds it (the app's own
    // search UI path — commands::search wraps exactly this).
    let ocred_doc_id = "ocr-e2e-ocred";
    engine
        .open_document(ocred_path.clone(), ocred_doc_id.to_string(), None)
        .unwrap_or_else(|e| panic!("failed to open OCR'd fixture: {e}"));
    let hits = engine
        .search_page(ocred_doc_id, 0, SEARCH_TERM, &SearchOptions::default())
        .expect("search_page on OCR'd fixture failed");
    assert!(
        !hits.is_empty(),
        "expected PDFium search_page to find '{SEARCH_TERM}' after write_ocr_pdf, found none"
    );
    for hit in &hits {
        assert_eq!(hit.page, 0);
        let [left, bottom, right, top] = hit.rect;
        assert!(left < right, "search hit rect should have left < right");
        assert!(
            bottom < top,
            "search hit rect should have bottom < top (y-up)"
        );
    }
    engine.close_document(ocred_doc_id);

    // --- Step 4b: lopdf's own extract_text finds it too — the EXACT call
    // search::indexer::extract_pdf_text makes for the Tantivy folder index,
    // proving folder-search pickup independently of PDFium.
    let lopdf_doc = lopdf::Document::load(&ocred_path).expect("lopdf load OCR'd fixture");
    let extracted = lopdf_doc
        .extract_text(&[1]) // lopdf get_pages() is 1-based
        .expect("lopdf extract_text failed");
    assert!(
        extracted.to_uppercase().contains(SEARCH_TERM),
        "expected lopdf::Document::extract_text (the Tantivy indexer's own extraction path) \
         to contain '{SEARCH_TERM}', got: {extracted:?}"
    );

    // Sanity: docops helpers this writer reused are still independently
    // sound after a full save/reload cycle (flatten/optimize would each
    // load-modify-save the SAME bytes in the real pipeline; confirm the
    // OCR'd file survives an unrelated optimize pass without losing the
    // text layer, since a future combined "OCR + optimize" flow is plausible).
    let optimized = {
        use docops::{DocOps, LopdfDocOps};
        LopdfDocOps
            .optimize(&ocred_bytes, 1)
            .expect("optimize OCR'd bytes")
    };
    let optimized_doc = lopdf::Document::load_from(std::io::Cursor::new(&optimized))
        .expect("reload optimized bytes");
    let optimized_text = optimized_doc
        .extract_text(&[1])
        .expect("extract_text on optimized bytes");
    assert!(
        optimized_text.to_uppercase().contains(SEARCH_TERM),
        "OCR text layer should survive an optimize(level=1) pass, got: {optimized_text:?}"
    );

    let _ = fs::remove_dir_all(&tmp_dir);
}
