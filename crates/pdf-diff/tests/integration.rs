//! Integration tests for pdf-diff.
//!
//! Tests against real PDFs require two environment variables to be set:
//!   PDFIUM_DYNAMIC_LIB_PATH  — path to libpdfium.dylib / pdfium.dll
//!   PDF_DIFF_TEST_A          — path to "old" PDF (MoE FEC DD drawing)
//!   PDF_DIFF_TEST_B          — path to "new" PDF (MoE FEC IFC drawing, rebadge-only)
//!
//! Without these, only the pure-Rust unit tests run (no PDFium dependency).
//!
//! Run all tests (portable, no PDFium):
//!   cargo test -p pdf-diff
//!
//! Run with PDFium + fixtures:
//!   PDFIUM_DYNAMIC_LIB_PATH=/path/to/libpdfium.dylib \
//!   PDF_DIFF_TEST_A=/path/to/dd.pdf \
//!   PDF_DIFF_TEST_B=/path/to/ifc.pdf \
//!   cargo test -p pdf-diff -- --nocapture

use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb};
use pdf_diff::{change_regions, change_regions_user_space, pixel_diff, CharDelta, PageGeometry};

// ---------------------------------------------------------------------------
// Pure-Rust helpers (run without PDFium)
// ---------------------------------------------------------------------------

fn solid_rgb(r: u8, g: u8, b: u8, w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(ImageBuffer::from_fn(w, h, |_, _| Rgb([r, g, b])))
}

// ---------------------------------------------------------------------------
// Pixel-diff unit tests (no PDFium needed)
// ---------------------------------------------------------------------------

#[test]
fn identical_pdfs_pixel_diff_passes() {
    // Same page rendered twice at the same DPI should be byte-for-byte identical.
    // Model this as two identical synthetic images.
    let img = solid_rgb(200, 180, 160, 100, 80);
    let result = pixel_diff(&img, &img, 5).unwrap();
    assert!(result.passed, "identical images must pass (tolerance=5)");
    assert_eq!(result.changed_pct, 0.0);
    assert_eq!(result.max_pixel_delta, 0);
}

#[test]
fn fully_different_images_pixel_diff_fails() {
    let white = solid_rgb(255, 255, 255, 60, 60);
    let black = solid_rgb(0, 0, 0, 60, 60);
    let result = pixel_diff(&white, &black, 0).unwrap();
    assert!(!result.passed, "completely different images must fail");
    assert!(result.changed_pct > 99.9, "expected ~100% changed");
    assert_eq!(result.max_pixel_delta, 255);
}

#[test]
fn partial_change_detected_correctly() {
    // Build an image that is half white, half black. Compare against all-white.
    // Only half the pixels should differ.
    let base = solid_rgb(255, 255, 255, 100, 100);
    let mut modified = base.clone().into_rgb8();
    // Fill bottom half with black.
    for y in 50..100u32 {
        for x in 0..100u32 {
            modified.put_pixel(x, y, Rgb([0, 0, 0]));
        }
    }
    let a = DynamicImage::ImageRgb8(base.into_rgb8());
    let b = DynamicImage::ImageRgb8(modified);

    let result = pixel_diff(&a, &b, 0).unwrap();
    assert!(!result.passed);
    // Exactly 50% of pixels changed.
    let pct = result.changed_pct;
    assert!(
        (pct - 50.0).abs() < 0.01,
        "expected 50% changed, got {pct:.2}%"
    );
}

#[test]
fn diff_image_has_red_for_changed_grey_for_unchanged() {
    let white = solid_rgb(255, 255, 255, 4, 4);
    let black = solid_rgb(0, 0, 0, 4, 4);
    let result = pixel_diff(&white, &black, 0).unwrap();
    let rgb = result.diff_image.to_rgb8();
    let px = rgb.get_pixel(0, 0);
    // Changed pixel = red (220, 30, 30).
    assert_eq!(px.0[0], 220, "R channel of changed pixel");
    assert_eq!(px.0[1], 30, "G channel of changed pixel");
    assert_eq!(px.0[2], 30, "B channel of changed pixel");
}

#[test]
fn diff_image_unchanged_pixels_are_grey() {
    // Same pixels in both images -> grey.
    let img = solid_rgb(100, 100, 100, 4, 4);
    let result = pixel_diff(&img, &img, 0).unwrap();
    let rgb = result.diff_image.to_rgb8();
    let px = rgb.get_pixel(0, 0);
    // 50% grey blend of 100 and 100 = 100.
    assert_eq!(px.0[0], 100);
    assert_eq!(px.0[1], 100);
    assert_eq!(px.0[2], 100);
}

#[test]
fn dimension_mismatch_returns_error() {
    let a = solid_rgb(0, 0, 0, 10, 20);
    let b = solid_rgb(0, 0, 0, 10, 30);
    assert!(
        pixel_diff(&a, &b, 0).is_err(),
        "mismatched dimensions must return Err"
    );
}

#[test]
fn tolerance_zero_passes_identical() {
    let img = solid_rgb(77, 88, 99, 20, 20);
    let r = pixel_diff(&img, &img, 0).unwrap();
    assert!(r.passed);
    assert_eq!(r.changed_pct, 0.0);
}

// ---------------------------------------------------------------------------
// Serde round-trip tests (no PDFium)
// ---------------------------------------------------------------------------

#[test]
fn char_delta_serde_round_trip() {
    let d = CharDelta {
        ch: 'D',
        delta_x: 3.5,
        delta_y: -1.2,
    };
    let json = serde_json::to_string(&d).unwrap();
    let back: CharDelta = serde_json::from_str(&json).unwrap();
    assert_eq!(back.ch, 'D');
    assert!((back.delta_x - 3.5).abs() < 1e-5);
    assert!((back.delta_y - (-1.2)).abs() < 1e-5);
}

#[test]
fn pixel_diff_result_serde_skips_diff_image() {
    // PixelDiffResult has #[serde(skip)] on diff_image — check the JSON
    // doesn't include that field (avoids accidentally serializing huge blobs).
    let img = solid_rgb(0, 0, 0, 4, 4);
    let r = pixel_diff(&img, &img, 0).unwrap();
    let json = serde_json::to_string(&r).unwrap();
    assert!(
        !json.contains("diff_image"),
        "diff_image must be excluded from JSON"
    );
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.get("passed").is_some());
    assert!(parsed.get("changed_pct").is_some());
}

// ---------------------------------------------------------------------------
// PDFium integration tests (gated on PDFIUM_DYNAMIC_LIB_PATH)
// ---------------------------------------------------------------------------

/// Skip the test if PDFium env var isn't set (CI without PDFium dylib).
macro_rules! require_pdfium {
    () => {
        if std::env::var("PDFIUM_DYNAMIC_LIB_PATH").is_err() {
            eprintln!("SKIP: PDFIUM_DYNAMIC_LIB_PATH not set");
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// change_regions (no PDFium needed)
// ---------------------------------------------------------------------------

fn with_blocks(base: &DynamicImage, blocks: &[(u32, u32, u32, u32)]) -> DynamicImage {
    let mut img = base.to_rgb8();
    for &(x0, y0, x1, y1) in blocks {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
    }
    DynamicImage::ImageRgb8(img)
}

#[test]
fn change_regions_three_edits_yield_three_regions() {
    let a = solid_rgb(255, 255, 255, 400, 300);
    // Three well separated edits.
    let b = with_blocks(
        &a,
        &[(20, 20, 40, 30), (200, 100, 230, 140), (50, 250, 60, 260)],
    );
    let r = change_regions(&a, &b, 5, 8, 72.0).unwrap();
    assert_eq!(r.len(), 3, "{r:?}");
    // Ordered top-to-bottom: first is the block at y=20..30 (PDF y 270..280 at 72 DPI).
    assert_eq!((r[0].x0, r[0].x1), (20.0, 40.0));
    assert_eq!((r[0].y0, r[0].y1), (270.0, 280.0));
    assert_eq!(r[0].changed_pixels, 200);
    assert_eq!(
        (r[1].x0, r[1].y0, r[1].x1, r[1].y1),
        (200.0, 160.0, 230.0, 200.0)
    );
}

#[test]
fn change_regions_merges_nearby_pixels_and_proximity_controls_count() {
    let a = solid_rgb(255, 255, 255, 200, 100);
    // Two blocks 10 px apart horizontally.
    let b = with_blocks(&a, &[(10, 10, 20, 20), (30, 10, 40, 20)]);
    assert_eq!(change_regions(&a, &b, 5, 2, 72.0).unwrap().len(), 2);
    let merged = change_regions(&a, &b, 5, 12, 72.0).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!((merged[0].x0, merged[0].x1), (10.0, 40.0));
}

#[test]
fn change_regions_scales_to_pdf_points_by_dpi() {
    let a = solid_rgb(255, 255, 255, 300, 300);
    let b = with_blocks(&a, &[(150, 150, 300, 300)]);
    let r = change_regions(&a, &b, 5, 4, 144.0).unwrap();
    assert_eq!(r.len(), 1);
    // 2 px per point: pixels 150..300 -> points 75..150; bottom of image -> y 0.
    assert_eq!(
        (r[0].x0, r[0].y0, r[0].x1, r[0].y1),
        (75.0, 0.0, 150.0, 75.0)
    );
}

#[test]
fn change_regions_identical_is_empty_and_mismatch_errors() {
    let a = solid_rgb(10, 20, 30, 50, 50);
    assert!(change_regions(&a, &a, 0, 4, 72.0).unwrap().is_empty());
    let b = solid_rgb(10, 20, 30, 40, 50);
    assert!(change_regions(&a, &b, 0, 4, 72.0).is_err());
    assert!(change_regions(&a, &a, 0, 4, 0.0).is_err());
}

#[test]
fn change_regions_ignores_changes_within_tolerance() {
    let a = solid_rgb(100, 100, 100, 40, 40);
    let b = solid_rgb(103, 100, 100, 40, 40);
    assert!(change_regions(&a, &b, 5, 4, 72.0).unwrap().is_empty());
}

/// Skip if a specific fixture env var isn't set.
macro_rules! require_fixture {
    ($var:literal) => {
        match std::env::var($var) {
            Ok(v) => v,
            Err(_) => {
                eprintln!("SKIP: {} not set", $var);
                return;
            }
        }
    };
}

#[test]
fn engine_new_with_pdfium() {
    require_pdfium!();
    let engine = pdf_diff::PdfDiffEngine::new();
    assert!(
        engine.is_ok(),
        "PdfDiffEngine::new() failed: {:?}",
        engine.err()
    );
}

#[test]
fn open_and_render_page() {
    require_pdfium!();
    let fixture_a = require_fixture!("PDF_DIFF_TEST_A");
    let mut engine = pdf_diff::PdfDiffEngine::new().expect("PDFium init");
    let doc = engine
        .open(std::path::Path::new(&fixture_a))
        .expect("open PDF_DIFF_TEST_A");
    let img = engine
        .render_page_full(&doc, 0, 72.0)
        .expect("render page 0 at 72 dpi");
    let (w, h) = img.dimensions();
    assert!(
        w > 100 && h > 100,
        "render produced implausibly small image {w}x{h}"
    );
    println!("Rendered page 0 of {fixture_a}: {w}x{h} at 72 DPI");
}

/// MoE FEC DD vs IFC fixture test — rebadge-only change.
///
/// Expected behaviour:
/// - text_diff: char_sequence_match = false (title-block text changed: status, date, rev)
/// - pixel_diff at 150 DPI: passed = false (title-block region has changed pixels)
#[test]
fn moe_fec_dd_vs_ifc_two_tier_diff() {
    require_pdfium!();
    let fixture_a = require_fixture!("PDF_DIFF_TEST_A");
    let fixture_b = require_fixture!("PDF_DIFF_TEST_B");

    let mut engine = pdf_diff::PdfDiffEngine::new().expect("PDFium init");
    let doc_a = engine
        .open(std::path::Path::new(&fixture_a))
        .expect("open PDF_DIFF_TEST_A");
    let doc_b = engine
        .open(std::path::Path::new(&fixture_b))
        .expect("open PDF_DIFF_TEST_B");

    // --- Tier 1: text-layer diff ---
    let text = engine.text_diff(&doc_a, &doc_b, 0, 0).expect("text_diff");

    println!(
        "text_diff: char_sequence_match={}",
        text.char_sequence_match
    );
    println!(
        "text_diff: position_deltas count={}",
        text.position_deltas.len()
    );

    // Rebadge-only: status, date, revision changed => sequences differ.
    assert!(
        !text.char_sequence_match,
        "MoE DD→IFC rebadge: title-block text changed, sequences must NOT match"
    );
    // No position deltas when sequences differ (undefined pairing).
    assert!(
        text.position_deltas.is_empty(),
        "position_deltas must be empty when char_sequence_match=false"
    );

    // --- Tier 2: pixel diff at 150 DPI ---
    let img_a = engine
        .render_page_full(&doc_a, 0, 150.0)
        .expect("render doc_a page 0");
    let img_b = engine
        .render_page_full(&doc_b, 0, 150.0)
        .expect("render doc_b page 0");

    // Images must be same size (both at 150 DPI from same-size pages).
    assert_eq!(
        img_a.dimensions(),
        img_b.dimensions(),
        "DD and IFC pages must have identical dimensions"
    );

    let px = pixel_diff(&img_a, &img_b, 5).expect("pixel_diff");

    println!(
        "pixel_diff: passed={}, changed_pct={:.3}%, max_delta={}",
        px.passed, px.changed_pct, px.max_pixel_delta
    );

    // Rebadge changes the title block, so some pixels must differ.
    assert!(
        !px.passed,
        "MoE DD→IFC rebadge: pixel diff must detect title-block changes (passed=false)"
    );
    assert!(
        px.changed_pct > 0.0,
        "changed_pct must be > 0 for a rebadge change, got {:.4}%",
        px.changed_pct
    );
    // Sanity: only title-block area changed (< 30% total page area).
    assert!(
        px.changed_pct < 30.0,
        "only title-block changed; expect < 30% changed pixels, got {:.2}%",
        px.changed_pct
    );
}

/// Identical-PDF test: comparing a PDF against itself must show zero changes.
#[test]
fn identical_pdf_shows_no_diff() {
    require_pdfium!();
    let fixture_a = require_fixture!("PDF_DIFF_TEST_A");

    let mut engine = pdf_diff::PdfDiffEngine::new().expect("PDFium init");
    let doc_a = engine
        .open(std::path::Path::new(&fixture_a))
        .expect("open PDF_DIFF_TEST_A (a)");
    let doc_b = engine
        .open(std::path::Path::new(&fixture_a))
        .expect("open PDF_DIFF_TEST_A (b)");

    // Tier 1: same file, same page.
    let text = engine
        .text_diff(&doc_a, &doc_b, 0, 0)
        .expect("text_diff identical");
    assert!(
        text.char_sequence_match,
        "identical PDF: char_sequence_match must be true"
    );
    // All position deltas must be near zero.
    for d in &text.position_deltas {
        assert!(
            d.delta_x.abs() < 0.01 && d.delta_y.abs() < 0.01,
            "identical PDF: delta for '{}' should be ~zero, got ({:.4}, {:.4})",
            d.ch,
            d.delta_x,
            d.delta_y
        );
    }

    // Tier 2.
    let img_a = engine.render_page_full(&doc_a, 0, 150.0).expect("render a");
    let img_b = engine.render_page_full(&doc_b, 0, 150.0).expect("render b");
    let px = pixel_diff(&img_a, &img_b, 0).expect("pixel_diff identical");

    println!(
        "identical PDF pixel_diff: passed={}, changed_pct={:.6}%",
        px.passed, px.changed_pct
    );
    assert!(
        px.passed,
        "identical PDF must produce passed=true, got changed={:.4}%",
        px.changed_pct
    );
    assert_eq!(px.changed_pct, 0.0, "identical PDF: 0% changed expected");
}

/// Verify that PdfDiffEngine is headless (no Tauri dependency).
/// This is a compile-time guarantee — if this file compiles without Tauri,
/// the headless contract is maintained. This test just confirms it runs.
#[test]
fn engine_is_headless_no_tauri_required() {
    // This test body intentionally does almost nothing.
    // Its existence proves that the test binary compiles and links
    // without any Tauri framework dependency.
    let _ = std::env::var("PDFIUM_DYNAMIC_LIB_PATH");
}

// ---------------------------------------------------------------------------
// display space -> PDF user space (rotation and non-zero box origin)
// ---------------------------------------------------------------------------

/// Page box 100 x 50 (landscape) at origin (10, 20). An edit at unrotated
/// box offset u 10..30, v 5..15 (a block near the lower-left).
fn user_rect() -> (f32, f32, f32, f32) {
    (20.0, 25.0, 40.0, 35.0)
}

fn geom(rot: i32) -> PageGeometry {
    PageGeometry {
        rotation_deg: rot,
        x0: 10.0,
        y0: 20.0,
        x1: 110.0,
        y1: 70.0,
    }
}

/// Build the display-space image for `rot` containing the edit, then check the
/// recovered user-space rect equals `user_rect()`.
fn check_rotation(rot: i32, disp_w: u32, disp_h: u32, block: (u32, u32, u32, u32)) {
    let a = solid_rgb(255, 255, 255, disp_w, disp_h);
    let b = with_blocks(&a, &[block]);
    let r = change_regions_user_space(&a, &b, 5, 4, 72.0, &geom(rot)).unwrap();
    assert_eq!(r.len(), 1, "rot {rot}: {r:?}");
    let (x0, y0, x1, y1) = user_rect();
    assert_eq!(
        (r[0].x0, r[0].y0, r[0].x1, r[0].y1),
        (x0, y0, x1, y1),
        "rot {rot}"
    );
}

#[test]
fn change_regions_user_space_rotation_0_with_box_origin() {
    // Display 100x50; edit u 10..30, v 5..15 -> pixel x 10..30, y (50-15)..(50-5).
    check_rotation(0, 100, 50, (10, 35, 30, 45));
}

#[test]
fn change_regions_user_space_rotation_90() {
    // Display 50x100. (u,v)->(dx,dy)=(v, w-u): dx 5..15, dy 70..90 -> pixel y 10..30.
    check_rotation(90, 50, 100, (5, 10, 15, 30));
}

#[test]
fn change_regions_user_space_rotation_180() {
    // Display 100x50. (dx,dy)=(w-u, h-v): dx 70..90, dy 35..45 -> pixel y 5..15.
    check_rotation(180, 100, 50, (70, 5, 90, 15));
}

#[test]
fn change_regions_user_space_rotation_270() {
    // Display 50x100. (dx,dy)=(h-v, u): dx 35..45, dy 10..30 -> pixel y 70..90.
    check_rotation(270, 50, 100, (35, 70, 45, 90));
}

#[test]
fn page_geometry_rejects_non_right_angle_rotation() {
    let g = PageGeometry {
        rotation_deg: 45,
        ..geom(0)
    };
    assert!(g.display_to_user(1.0, 1.0).is_err());
    // Negative multiples normalise: -90 == 270.
    let a = geom(-90).display_to_user(5.0, 10.0).unwrap();
    assert_eq!(a, geom(270).display_to_user(5.0, 10.0).unwrap());
}

#[test]
fn change_regions_tiny_proximity_uses_bounded_cells() {
    // proximity 0 and 1 are clamped to the minimum cell: still correct, and
    // gaps of 2 px merge (cell 4) while the result stays one region.
    let a = solid_rgb(255, 255, 255, 64, 64);
    let b = with_blocks(&a, &[(10, 10, 14, 14), (16, 10, 20, 14)]);
    for p in [0, 1] {
        let r = change_regions(&a, &b, 5, p, 72.0).unwrap();
        assert_eq!(r.len(), 1, "proximity {p}: {r:?}");
    }
}
