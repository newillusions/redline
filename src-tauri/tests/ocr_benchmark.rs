//! Auto-OCR Phase 2a accuracy/latency benchmark — gates Phase 2b/2c (scoping
//! doc §D "Tests (unit + 3-5 scanned-CAD fixtures)"). Whole file is a no-op
//! without the `ocr` feature (see the `#![cfg(...)]` below), so it's safe to
//! leave registered unconditionally.
//!
//! Run:
//!   PDFIUM_DYNAMIC_LIB_PATH=.../libpdfium.dylib \
//!     cargo test --features ocr --test ocr_benchmark -- --ignored --nocapture
//!
//! `--ignored` because this needs PDFium (see scripts/fetch-pdfium.sh) AND a
//! working Tesseract install with English tessdata reachable (via
//! `OcrEngineHandle::load(None)`'s default lookup — a system install, e.g.
//! Homebrew's `tesseract`/`leptonica` on macOS or `tesseract-ocr-eng` on
//! Debian/Ubuntu, or an explicit `TESSDATA_PREFIX`/`REDLINE_TESSDATA_DIR`).
//! `.forgejo/Dockerfile.test-rust`'s `ocr` build-arg leg installs both and
//! sets `TESSDATA_PREFIX` explicitly, so this test runs for real in that CI
//! leg — unlike Phase 1's `ocrs` engine, no third-party model bucket fetch is
//! needed. `--nocapture` to actually see the report table; without it a
//! passing test swallows the printed numbers.
//!
//! Phase 1 -> Phase 2a: engine switched from `ocrs` (pure-Rust, 30% overall
//! recall on this corpus even with a rotation fix — see
//! observation:lr0cwsixkpbzei7vthon) to Tesseract 5 via `leptess` (93%/98%
//! baseline/rotate-4x). `OcrEngineHandle::recognize_page` now always runs the
//! rotate-4x-and-merge strategy internally (see the `ocr` module doc
//! comment), so this benchmark exercises that path by default — there is no
//! separate "with rotation" mode to opt into here.

#![cfg(feature = "ocr")]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use redline_lib::ocr::OcrEngineHandle;
use redline_lib::render::RenderEngine;
use serde::Deserialize;

#[derive(Deserialize)]
struct ExpectedLine {
    text: String,
    orientation: String, // "horizontal" | "vertical"
}

#[derive(Deserialize)]
struct ExpectedFixture {
    description: String,
    render_dpi: f32,
    lines: Vec<ExpectedLine>,
}

/// Normalize for matching: uppercase alphanumerics, everything else
/// (punctuation, symbols) collapsed to a single space, runs of whitespace
/// collapsed to one space, trimmed. Deliberately coarse — this benchmark is
/// gating "does the engine read the text and its rotation", not scoring
/// character-level edit distance.
fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn lines_match(recognized: &str, expected: &str) -> bool {
    let (r, e) = (normalize(recognized), normalize(expected));
    if r.is_empty() || e.is_empty() {
        return false;
    }
    r == e || r.contains(&e) || e.contains(&r)
}

struct FixtureResult {
    name: String,
    description: String,
    total_expected: usize,
    matched_expected: usize,
    total_vertical: usize,
    matched_vertical: usize,
    total_recognized: usize,
    matched_recognized: usize,
    mean_conf: f64,
    ms: u128,
}

impl FixtureResult {
    fn recall(&self) -> f64 {
        if self.total_expected == 0 {
            return 1.0;
        }
        self.matched_expected as f64 / self.total_expected as f64
    }
    fn precision(&self) -> f64 {
        if self.total_recognized == 0 {
            return 0.0;
        }
        self.matched_recognized as f64 / self.total_recognized as f64
    }
    fn vertical_recall(&self) -> Option<f64> {
        if self.total_vertical == 0 {
            None
        } else {
            Some(self.matched_vertical as f64 / self.total_vertical as f64)
        }
    }
}

#[test]
#[ignore]
fn ocr_corpus_benchmark() {
    if env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
        eprintln!("SKIP ocr_corpus_benchmark: PDFIUM_DYNAMIC_LIB_PATH not set (see scripts/fetch-pdfium.sh)");
        return;
    }

    // REDLINE_TESSDATA_DIR overrides; otherwise None defers to Tesseract's
    // own standard lookup (TESSDATA_PREFIX env var or compiled-in default —
    // see the `ocr` module doc comment).
    let tessdata_dir = env::var_os("REDLINE_TESSDATA_DIR").map(PathBuf::from);
    let mut ocr = match OcrEngineHandle::load(tessdata_dir.as_deref()) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!(
                "SKIP ocr_corpus_benchmark: OcrEngineHandle::load failed ({e}) — is a \
                 Tesseract install with English tessdata reachable? Set \
                 REDLINE_TESSDATA_DIR or TESSDATA_PREFIX."
            );
            return;
        }
    };

    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/fixtures/ocr");
    let mut fixture_names: Vec<String> = fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| panic!("failed to read fixtures dir {:?}: {e}", fixtures_dir))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("pdf") {
                path.file_stem().and_then(|s| s.to_str()).map(String::from)
            } else {
                None
            }
        })
        .collect();
    fixture_names.sort();
    assert!(
        !fixture_names.is_empty(),
        "no fixture PDFs found under {:?} (run `cargo run --bin gen-ocr-fixtures`)",
        fixtures_dir
    );

    let mut engine = RenderEngine::new().expect("RenderEngine::new failed");

    let mut results = Vec::new();

    for name in &fixture_names {
        let pdf_path = fixtures_dir.join(format!("{name}.pdf"));
        let json_path = fixtures_dir.join(format!("{name}.expected.json"));
        let expected: ExpectedFixture = serde_json::from_slice(
            &fs::read(&json_path).unwrap_or_else(|e| panic!("failed to read {:?}: {e}", json_path)),
        )
        .unwrap_or_else(|e| panic!("failed to parse {:?}: {e}", json_path));

        let doc_id = format!("ocr-bench-{name}");
        engine
            .open_document(pdf_path.clone(), doc_id.clone(), None)
            .unwrap_or_else(|e| panic!("failed to open fixture {:?}: {e}", pdf_path))
            .into_page_count()
            .unwrap_or_else(|e| panic!("open outcome for {:?}: {e}", pdf_path));

        let t0 = Instant::now();
        let raster = engine
            .render_page_full(&doc_id, 0, expected.render_dpi)
            .unwrap_or_else(|e| panic!("render_page_full failed for {name}: {e}"));
        let recognized = ocr
            .recognize_page(&raster)
            .unwrap_or_else(|e| panic!("recognize_page failed for {name}: {e}"));
        let ms = t0.elapsed().as_millis();

        engine.close_document(&doc_id);

        let mut matched_expected = 0usize;
        let mut matched_vertical = 0usize;
        let mut total_vertical = 0usize;
        for exp in &expected.lines {
            let is_vertical = exp.orientation == "vertical";
            if is_vertical {
                total_vertical += 1;
            }
            if recognized.iter().any(|r| lines_match(&r.text, &exp.text)) {
                matched_expected += 1;
                if is_vertical {
                    matched_vertical += 1;
                }
            }
        }
        let matched_recognized = recognized
            .iter()
            .filter(|r| expected.lines.iter().any(|e| lines_match(&r.text, &e.text)))
            .count();
        let mean_conf = if recognized.is_empty() {
            0.0
        } else {
            recognized
                .iter()
                .filter_map(|r| r.confidence)
                .map(|c| c as f64)
                .sum::<f64>()
                / recognized.len() as f64
        };

        results.push(FixtureResult {
            name: name.clone(),
            description: expected.description.clone(),
            total_expected: expected.lines.len(),
            matched_expected,
            total_vertical,
            matched_vertical,
            total_recognized: recognized.len(),
            matched_recognized,
            mean_conf,
            ms,
        });
    }

    println!();
    println!(
        "Auto-OCR Phase 2a benchmark (Tesseract via leptess, rotate-4x, corpus: {})",
        fixtures_dir.display()
    );
    println!(
        "{:<32} {:>8} {:>10} {:>10} {:>9} {:>12} {:>10}",
        "fixture", "ms/page", "recall", "precision", "mean_conf", "vert.recall", "exp/rec"
    );
    for r in &results {
        let vert = r
            .vertical_recall()
            .map(|v| format!("{:.0}%", v * 100.0))
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "{:<32} {:>8} {:>9.0}% {:>9.0}% {:>8.0}% {:>12} {:>4}/{:<5}",
            r.name,
            r.ms,
            r.recall() * 100.0,
            r.precision() * 100.0,
            r.mean_conf * 100.0,
            vert,
            r.matched_expected,
            r.total_expected,
        );
        println!("  {}", r.description);
    }
    println!();

    let overall_recall = results.iter().map(|r| r.matched_expected).sum::<usize>() as f64
        / results
            .iter()
            .map(|r| r.total_expected)
            .sum::<usize>()
            .max(1) as f64;
    let vertical_total = results.iter().map(|r| r.total_vertical).sum::<usize>();
    let vertical_matched = results.iter().map(|r| r.matched_vertical).sum::<usize>();
    println!(
        "OVERALL recall: {:.0}% ({}/{} lines); vertical-text recall: {}",
        overall_recall * 100.0,
        results.iter().map(|r| r.matched_expected).sum::<usize>(),
        results.iter().map(|r| r.total_expected).sum::<usize>(),
        if vertical_total > 0 {
            format!(
                "{:.0}% ({vertical_matched}/{vertical_total})",
                vertical_matched as f64 / vertical_total as f64 * 100.0
            )
        } else {
            "n/a".to_string()
        }
    );

    // Phase 2a gate, target >=95%/>=95% per the bake-off report
    // (dev-reports/2026-09-02-redline-ocr-bakeoff.md): the corpus is
    // synthetic text rendered-then-rescanned, not real scanned CAD paper, so
    // a 95% bar here would be tighter than that measurement justifies on a
    // possibly-regenerated corpus. The HARD FLOOR asserted below is 90%
    // overall recall — below that, the engine is no longer doing its job and
    // the build should fail, not just print a worse number.
    for r in &results {
        assert!(
            r.total_recognized > 0,
            "fixture {} recognized ZERO lines — engine or pipeline is broken, not just inaccurate",
            r.name
        );
    }
    assert!(
        overall_recall >= 0.90,
        "OVERALL recall {:.1}% is below the Phase 2a hard floor of 90% — see the printed \
         per-fixture table above for which fixture regressed",
        overall_recall * 100.0
    );
}
