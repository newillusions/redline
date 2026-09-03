//! Dev tool: (re)generates the scanned-CAD OCR fixture corpus
//! (`tools/fixtures/ocr/`). See `src-tauri/Cargo.toml`'s `gen-ocr-fixtures`
//! `[[bin]]` comment for why this doesn't need the `ocr` feature.
//!
//! Usage:
//!   PDFIUM_DYNAMIC_LIB_PATH=/abs/path/libpdfium.dylib \
//!     cargo run --bin gen-ocr-fixtures
//!
//! Each fixture is built in three steps:
//!  1. A small "source" PDF is built in-memory with `lopdf`: real vector text
//!     (Helvetica/WinAnsiEncoding), laid out however the fixture needs (plain
//!     horizontal / rotated / mixed / dense / small-font — the five shapes
//!     the 2026-09-02 scoping doc's fixture proposal names).
//!  2. That source PDF's page 0 is rasterized via the SAME PDFium render path
//!     the app uses (`RenderEngine::render_page_full`), at the fixture's DPI.
//!  3. The raster is JPEG-encoded and wrapped into a NEW, separate single-page
//!     PDF whose only content is that one Image XObject sized to fill the
//!     original page's `MediaBox` — a synthetic "flattened scan" with NO text
//!     layer, matching the real-world image-only PDFs Auto-OCR targets.
//!
//! The exact strings placed in step 1 become the ground truth
//! (`<name>.expected.json`) — authored directly, never derived by running OCR
//! against the output (that would just measure the engine against itself).

use std::fs;
use std::path::PathBuf;

use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use redline_lib::render::RenderEngine;
use serde::Serialize;

#[derive(Serialize)]
struct ExpectedLine {
    text: String,
    /// "horizontal" | "vertical" — matches the scoping doc's requirement-2
    /// framing (a rotated dimension string vs. a plain horizontal callout).
    orientation: &'static str,
}

#[derive(Serialize)]
struct ExpectedFixture {
    description: &'static str,
    /// DPI this fixture's embedded image was rendered at. The benchmark
    /// re-renders the flattened PDF's own page at this SAME DPI before
    /// running OCR, so the pixel density the engine sees matches what
    /// produced the embedded image 1:1 (no resampling mismatch).
    render_dpi: f32,
    lines: Vec<ExpectedLine>,
}

/// One text run placed on a source-PDF page.
struct TextSpec {
    text: &'static str,
    /// Baseline origin, PDF user-space points.
    x: f64,
    y: f64,
    font_size: f64,
    /// Rotation, degrees counter-clockwise. 0 = horizontal; 90 = vertical,
    /// reading bottom-to-top — the standard CAD dimension-string convention.
    rotate_deg: f64,
    orientation: &'static str,
}

fn text_matrix(spec: &TextSpec) -> String {
    let theta = spec.rotate_deg.to_radians();
    let (cos, sin) = (theta.cos(), theta.sin());
    // Tm operands: a b c d e f (device = (x*a + y*c + e, x*b + y*d + f)).
    // Standard 2D rotation: a=cosθ b=sinθ c=-sinθ d=cosθ.
    format!(
        "{cos:.6} {sin:.6} {neg_sin:.6} {cos:.6} {x:.2} {y:.2}",
        cos = cos,
        sin = sin,
        neg_sin = -sin,
        x = spec.x,
        y = spec.y
    )
}

/// Escape a PDF literal string's three special characters. None of this
/// corpus's fixed strings currently use them; the helper makes that an
/// enforced invariant rather than an unstated assumption.
fn pdf_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Build a single-page source PDF (real vector text, Helvetica/WinAnsi) with
/// the given page size and text runs.
fn build_source_pdf(page_w: f64, page_h: f64, specs: &[TextSpec]) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let resources = dictionary! {
        "Font" => Object::Dictionary(dictionary! { "F1" => Object::Reference(font_id) }),
    };

    let mut content = String::from("BT\n");
    for spec in specs {
        content.push_str(&format!("/F1 {:.2} Tf\n", spec.font_size));
        content.push_str(&format!("{} Tm\n", text_matrix(spec)));
        content.push_str(&format!("({}) Tj\n", pdf_escape(spec.text)));
    }
    content.push_str("ET\n");

    let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            0.0.into(), 0.0.into(), (page_w as f32).into(), (page_h as f32).into(),
        ],
        "Resources" => Object::Dictionary(resources),
        "Contents" => Object::Reference(content_id),
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1_i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .expect("lopdf save_to failed for source fixture PDF");
    bytes
}

/// Build a single-page, image-only "flattened scan" PDF: one JPEG Image
/// XObject sized to fill `(page_w, page_h)` exactly, no text layer at all.
fn build_flattened_pdf(
    page_w: f64,
    page_h: f64,
    jpeg_bytes: Vec<u8>,
    img_w: u32,
    img_h: u32,
) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();

    let img_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => img_w as i64,
        "Height" => img_h as i64,
        "BitsPerComponent" => 8_i64,
        "ColorSpace" => "DeviceRGB",
        "Filter" => "DCTDecode",
    };
    let img_id = doc.add_object(Stream::new(img_dict, jpeg_bytes));
    let resources = dictionary! {
        "XObject" => Object::Dictionary(dictionary! { "Im0" => Object::Reference(img_id) }),
    };
    let cm = format!("q {:.2} 0 0 {:.2} 0 0 cm /Im0 Do Q", page_w, page_h);
    let content_id = doc.add_object(Stream::new(Dictionary::new(), cm.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            0.0.into(), 0.0.into(), (page_w as f32).into(), (page_h as f32).into(),
        ],
        "Resources" => Object::Dictionary(resources),
        "Contents" => Object::Reference(content_id),
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1_i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .expect("lopdf save_to failed for flattened fixture PDF");
    bytes
}

fn encode_jpeg_rgb(rgb: &[u8], w: u32, h: u32, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .encode(rgb, w, h, image::ExtendedColorType::Rgb8)
        .expect("jpeg encode of fixture raster failed");
    out
}

/// Render `source_bytes`'s page 0 at `dpi` via the app's own render path, and
/// JPEG-encode the result. Opens/closes a scratch document on `engine` rather
/// than a fresh `RenderEngine` per fixture (PDFium global C state — see
/// `RenderEngine`'s own doc comments — is happiest with one engine reused
/// serially, matching `bench.rs`'s pattern).
fn rasterize_source_pdf(
    engine: &mut RenderEngine,
    source_bytes: &[u8],
    dpi: f32,
) -> (Vec<u8>, u32, u32) {
    let mut tmp = std::env::temp_dir();
    tmp.push(format!(
        "redline-ocr-fixture-src-{}-{}.pdf",
        std::process::id(),
        fastrand_like_suffix()
    ));
    fs::write(&tmp, source_bytes).expect("write temp source fixture pdf");

    let doc_id = format!("ocr-fixture-src-{}", std::process::id());
    engine
        .open_document(tmp.clone(), doc_id.clone(), None)
        .expect("open source fixture pdf")
        .into_page_count()
        .expect("source fixture pdf open outcome");

    let raster = engine
        .render_page_full(&doc_id, 0, dpi)
        .expect("render_page_full failed for source fixture pdf");

    engine.close_document(&doc_id);
    let _ = fs::remove_file(&tmp);

    let jpeg = encode_jpeg_rgb(&raster.rgb, raster.width_px, raster.height_px, 85);
    (jpeg, raster.width_px, raster.height_px)
}

/// Tiny dependency-free suffix so repeated runs in the same process/second
/// don't collide on the temp file path. Not a real RNG — just needs to differ
/// across the 5 sequential calls in `main`.
fn fastrand_like_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
}

struct Fixture {
    name: &'static str,
    description: &'static str,
    page_w: f64,
    page_h: f64,
    dpi: f32,
    specs: Vec<TextSpec>,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        // (a) plain horizontal text page.
        Fixture {
            name: "a-plain-horizontal",
            description: "Plain horizontal text page (drawing title block callouts)",
            page_w: 612.0,
            page_h: 792.0,
            dpi: 300.0,
            specs: vec![
                TextSpec { text: "FLOOR PLAN LEVEL 2", x: 72.0, y: 700.0, font_size: 14.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "SCALE 1 TO 100", x: 72.0, y: 660.0, font_size: 14.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "PROJECT NO 2026 118", x: 72.0, y: 620.0, font_size: 14.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "DRAWN BY MR CHECKED BY JR", x: 72.0, y: 580.0, font_size: 14.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "NORTH", x: 72.0, y: 540.0, font_size: 14.0, rotate_deg: 0.0, orientation: "horizontal" },
            ],
        },
        // (b) a page with a 90-degree rotated dimension string.
        Fixture {
            name: "b-rotated-dimension",
            description: "One 90-degree rotated CAD dimension string plus a small horizontal label",
            page_w: 400.0,
            page_h: 400.0,
            dpi: 300.0,
            specs: vec![
                TextSpec { text: "D1", x: 40.0, y: 370.0, font_size: 12.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "DIM 3650 MM", x: 200.0, y: 100.0, font_size: 16.0, rotate_deg: 90.0, orientation: "vertical" },
            ],
        },
        // (c) mixed horizontal callouts + vertical text on one sheet — the
        // exact requirement-2 case the scoping doc calls out.
        Fixture {
            name: "c-mixed-horizontal-vertical",
            description: "Mixed sheet: horizontal room callouts + two 90-degree rotated dimension strings",
            page_w: 700.0,
            page_h: 500.0,
            dpi: 300.0,
            specs: vec![
                TextSpec { text: "ROOM 101 OFFICE", x: 40.0, y: 440.0, font_size: 12.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "ROOM 102 MEETING", x: 40.0, y: 410.0, font_size: 12.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "CORRIDOR", x: 40.0, y: 380.0, font_size: 12.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "DIM 5000 MM", x: 650.0, y: 40.0, font_size: 14.0, rotate_deg: 90.0, orientation: "vertical" },
                TextSpec { text: "DIM 3200 MM", x: 600.0, y: 40.0, font_size: 14.0, rotate_deg: 90.0, orientation: "vertical" },
            ],
        },
        // (d) a dense A0-style sheet, for latency. Physical page size matches
        // the real A0-class corpus tier already used by bench/RUNBOOK-S20.md
        // (see bench/README.md C4: ~3370x2384 pt). Rendered at 150 DPI rather
        // than 300 — DELIBERATE, documented tradeoff: at 300 DPI this page
        // rasterizes to ~14000x9900px (~140 megapixels), which would make the
        // committed corpus asset far larger than "a few MB" for no benchmark
        // value the 150 DPI version doesn't already provide (same dense-content
        // code path, same relative latency signal, much smaller committed
        // file). A production OCR run would still default to 300 DPI; this
        // fixture is a lower-bound-latency proxy for the "many small text
        // items on one huge page" shape, not a claim about worst-case latency
        // at the real production DPI.
        Fixture {
            name: "d-dense-a0-latency",
            description: "Dense A0-style sheet (40 room labels) for latency measurement — rendered at 150 DPI, see source comment for why",
            page_w: 3370.0,
            page_h: 2384.0,
            dpi: 150.0,
            specs: dense_a0_specs(),
        },
        // (e) a small-font title-block crop.
        Fixture {
            name: "e-small-font-title-block",
            description: "Small-font (6pt) title-block crop",
            page_w: 250.0,
            page_h: 120.0,
            dpi: 300.0,
            specs: vec![
                TextSpec { text: "DWG NO A 101", x: 10.0, y: 100.0, font_size: 6.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "SCALE AS NOTED", x: 10.0, y: 85.0, font_size: 6.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "DATE 2026 09 02", x: 10.0, y: 70.0, font_size: 6.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "REV A", x: 10.0, y: 55.0, font_size: 6.0, rotate_deg: 0.0, orientation: "horizontal" },
                TextSpec { text: "DRAWN MR", x: 10.0, y: 40.0, font_size: 6.0, rotate_deg: 0.0, orientation: "horizontal" },
            ],
        },
    ]
}

fn dense_a0_specs() -> Vec<TextSpec> {
    // Static strings (TextSpec::text is &'static str) — leak the 40 generated
    // Strings deliberately. This binary is a short-lived one-shot dev tool
    // (generate the corpus, exit), so the leak is bounded and irrelevant; it's
    // the simplest way to satisfy the 'static bound without restructuring
    // TextSpec into an owned-String type used nowhere else.
    let mut specs = Vec::with_capacity(40);
    for r in 0..5u32 {
        for c in 0..8u32 {
            let room_no = 101 + r * 8 + c;
            let text: &'static str = Box::leak(format!("ROOM {room_no}").into_boxed_str());
            specs.push(TextSpec {
                text,
                x: 150.0 + c as f64 * 400.0,
                y: 2200.0 - r as f64 * 420.0,
                font_size: 28.0,
                rotate_deg: 0.0,
                orientation: "horizontal",
            });
        }
    }
    specs
}

fn main() {
    let pdfium_set = std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_some();
    if !pdfium_set {
        eprintln!("PDFIUM_DYNAMIC_LIB_PATH is not set — see scripts/fetch-pdfium.sh. Aborting.");
        std::process::exit(1);
    }

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tools/fixtures/ocr");
    fs::create_dir_all(&out_dir).expect("failed to create tools/fixtures/ocr");

    let mut engine = RenderEngine::new().expect("RenderEngine::new failed");

    for fx in fixtures() {
        let source_bytes = build_source_pdf(fx.page_w, fx.page_h, &fx.specs);
        let (jpeg, img_w, img_h) = rasterize_source_pdf(&mut engine, &source_bytes, fx.dpi);
        let flattened = build_flattened_pdf(fx.page_w, fx.page_h, jpeg, img_w, img_h);

        let pdf_path = out_dir.join(format!("{}.pdf", fx.name));
        fs::write(&pdf_path, &flattened).expect("write flattened fixture pdf");

        let expected = ExpectedFixture {
            description: fx.description,
            render_dpi: fx.dpi,
            lines: fx
                .specs
                .iter()
                .map(|s| ExpectedLine {
                    text: s.text.to_string(),
                    orientation: s.orientation,
                })
                .collect(),
        };
        let json_path = out_dir.join(format!("{}.expected.json", fx.name));
        let json = serde_json::to_vec_pretty(&expected).expect("serialize expected fixture");
        fs::write(&json_path, json).expect("write expected fixture json");

        println!(
            "{:32} {:>6}x{:<6} px  {:>7} bytes  -> {}",
            fx.name,
            img_w,
            img_h,
            fs::metadata(&pdf_path).map(|m| m.len()).unwrap_or(0),
            pdf_path.display()
        );
    }

    println!(
        "Done: {} fixtures written to {}",
        fixtures().len(),
        out_dir.display()
    );
}
