//! Regression proof for the two OCR defects the 2026-10-02 owner-file probe found
//! (KB observation:xwmsw4zfkbcr0i02hg4t), on a synthetic image-only contract rebuilt
//! here at run time (nothing binary is committed):
//!
//! * A. Re-running OCR on a scanner PDF whose content stream carries trailing NUL bytes
//!   stacked a duplicate invisible layer, and the Tantivy indexer's extractor could not
//!   see the layer at all.
//! * B. The rotate-4x passes added garbled rotated lines (16% of the layer characters in
//!   the owner's file).
//!
//! Locally the test SKIPs (with a printed reason) when PDFium, tessdata or fonts are
//! missing; with `CI` or `REDLINE_REQUIRE_OCR_TESTS` set it panics instead, so a CI run
//! cannot go green without asserting anything.
//!
//! `#[ignore]`, like `ocr_writer_e2e.rs` and `ocr_benchmark.rs` - needs a PDFium dylib
//! and a Tesseract install with English tessdata. Run:
//!
//!   PDFIUM_DYNAMIC_LIB_PATH=.../libpdfium.dylib REDLINE_TESSDATA_DIR=.../tessdata \
//!     cargo test --features ocr --test ocr_rerun_noise -- --ignored --nocapture

#![cfg(feature = "ocr")]

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use lopdf::{dictionary, Document, Object, Stream};
use redline_lib::ocr::writer::{write_ocr_pdf, DEFAULT_MIN_CONFIDENCE};
use redline_lib::ocr::{OcrEngineHandle, OcrLine};
use redline_lib::render::RenderEngine;
use redline_lib::search::indexer::extract_pdf_text;

const PAGE_W: f64 = 595.2;
const PAGE_H: f64 = 841.68;
/// Konica page measured 1159 trailing NULs on the owner's file.
const NUL_PAD: usize = 1159;

const PAGE_1: &str = "SERVICES AGREEMENT No. EL-2026-0147

This Services Agreement (the \"Agreement\") is entered into on 14 September 2026 between Emittiv L.L.C-FZ, a company incorporated in Dubai, United Arab Emirates (the \"Consultant\"), and Harbourfront Hospitality Holdings Ltd (the \"Client\").

1. SCOPE OF SERVICES
1.1 The Consultant shall provide architectural lighting design services for the Auditorium refurbishment at Level 3, including concept design, design development and tender documentation.
1.2 The Services exclude electrical engineering, controls programming, and on-site commissioning unless varied in writing under clause 9.

2. FEES AND PAYMENT
2.1 The Client shall pay the Consultant a lump sum fee of AED 245,000 (two hundred and forty-five thousand dirhams), exclusive of VAT at 5%.
2.2 Invoices are payable within thirty (30) days of the invoice date. Late payments accrue interest at 1.5% per month.
2.3 A mobilisation payment of 25% is due upon signature of this Agreement.";

const PAGE_2: &str = "3. PROGRAMME
3.1 Concept design shall be delivered within six (6) weeks of the commencement date. Design development shall follow within a further eight (8) weeks.
3.2 Time is of the essence in respect of the tender issue date of 30 January 2027.

4. INTELLECTUAL PROPERTY
4.1 Copyright in all drawings, specifications and models prepared by the Consultant remains vested in the Consultant. The Client is granted a non-exclusive licence to use them for the Project only.

5. LIABILITY
5.1 The Consultant's total aggregate liability under this Agreement shall not exceed the fees paid, save in cases of gross negligence or wilful default.
5.2 Neither party shall be liable for indirect or consequential loss, including loss of profit.

6. GOVERNING LAW
6.1 This Agreement is governed by the laws of the Emirate of Dubai and the federal laws of the UAE applicable therein.

Signed for the Consultant: ____________________   Date: 14/09/2026
Signed for the Client: ____________________   Date: 14/09/2026";

/// Greedy word wrap to `width` characters, keeping blank lines.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if !cur.is_empty() && cur.len() + 1 + word.len() > width {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        out.push(cur);
    }
    out
}

fn pdf_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// A born-digital text PDF (base-14 Times) - the "ground truth" page that gets rasterised
/// into the fake scan.
fn text_pdf(pages: &[&str]) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman",
        "Encoding" => "WinAnsiEncoding",
    });
    let mut kids = Vec::new();
    for text in pages {
        let mut content = String::from("BT\n/F1 10.5 Tf\n14.2 TL\n60 770 Td\n");
        for line in wrap(text, 92) {
            content.push_str(&format!("({}) Tj T*\n", pdf_escape(&line)));
        }
        content.push_str("ET\n");
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), PAGE_W.into(), PAGE_H.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save text pdf");
    out
}

/// Wrap rasters into an image-only PDF whose page content stream ends in a run of NULs,
/// the way the owner's Konica scanner writes it.
fn scan_pdf(rasters: &[(Vec<u8>, u32, u32)]) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for (jpeg, w, h) in rasters {
        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => *w as i64,
                "Height" => *h as i64, "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8, "Filter" => "DCTDecode",
            },
            jpeg.clone(),
        ));
        let mut content = format!("q\n{PAGE_W} 0 0 {PAGE_H} 0 0 cm\n/Im0 Do\nQ\n").into_bytes();
        content.extend(std::iter::repeat(0u8).take(NUL_PAD));
        let content_id = doc.add_object(Stream::new(dictionary! {}, content));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), PAGE_W.into(), PAGE_H.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save scan pdf");
    out
}

/// Redline OCR layers per page, counted straight from content-stream bytes.
fn layers_per_page(pdf: &[u8]) -> Vec<usize> {
    let doc = Document::load_mem(pdf).expect("load for layer count");
    doc.get_pages()
        .values()
        .map(|page_id| {
            doc.get_page_contents(*page_id)
                .into_iter()
                .filter(|id| {
                    let Ok(Object::Stream(s)) = doc.get_object(*id) else {
                        return false;
                    };
                    let data = s
                        .decompressed_content()
                        .unwrap_or_else(|_| s.content.clone());
                    data.windows(10).any(|w| w == b"/RLOCRFont")
                })
                .count()
        })
        .collect()
}

fn vocab(text: &str) -> HashSet<String> {
    text.split_whitespace().filter_map(norm).collect()
}

/// Lower-cased alphanumerics of a token, `None` when it has none (rules, underscores).
fn norm(token: &str) -> Option<String> {
    let t: String = token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    (!t.is_empty()).then_some(t)
}

/// Share of the layer's non-whitespace characters that sit on lines where under half of
/// the tokens are words of the source contract: the junk the owner saw.
fn junk_ratio(layer_lines: &[String], truth: &HashSet<String>) -> (usize, usize) {
    let (mut junk, mut total) = (0, 0);
    for line in layer_lines {
        let chars = line.chars().filter(|c| !c.is_whitespace()).count();
        let toks: Vec<String> = line.split_whitespace().filter_map(norm).collect();
        if toks.is_empty() {
            continue; // pure rules / underscores: not counted either way
        }
        total += chars;
        let known = toks.iter().filter(|t| truth.contains(*t)).count();
        if known * 2 < toks.len() {
            junk += chars;
            eprintln!("  junk line: {line:?}");
        }
    }
    (junk, total)
}

/// Skip locally (prints why), but FAIL when `REDLINE_REQUIRE_OCR_TESTS` or `CI` is set:
/// a CI run must never go green by silently asserting nothing.
fn skip_or_fail(why: &str) {
    if env::var_os("REDLINE_REQUIRE_OCR_TESTS").is_some() || env::var_os("CI").is_some() {
        panic!("required OCR regression test cannot run in CI: {why}");
    }
    eprintln!("SKIP: {why}");
}

fn recognise(
    engine: &mut RenderEngine,
    ocr: &mut OcrEngineHandle,
    path: &std::path::Path,
    id: &str,
) -> Vec<(u32, Vec<OcrLine>)> {
    engine
        .open_document(path.to_path_buf(), id.to_string(), None)
        .expect("open scan");
    let mut pages = Vec::new();
    for page in 0..2u32 {
        let raster = engine
            .render_page_full(id, page, 300.0)
            .expect("render 300dpi");
        pages.push((page, ocr.recognize_page(&raster).expect("recognize")));
    }
    engine.close_document(id);
    pages
}

#[test]
#[ignore]
fn rerun_replaces_the_layer_and_rotation_noise_stays_under_three_percent() {
    if env::var_os("PDFIUM_DYNAMIC_LIB_PATH").is_none() {
        skip_or_fail("PDFIUM_DYNAMIC_LIB_PATH not set (see scripts/fetch-pdfium.sh)");
        return;
    }
    let tessdata = env::var_os("REDLINE_TESSDATA_DIR").map(PathBuf::from);
    let Ok(mut ocr) = OcrEngineHandle::load(tessdata.as_deref()) else {
        skip_or_fail("Tesseract/tessdata not loadable (set REDLINE_TESSDATA_DIR)");
        return;
    };
    let mut engine = RenderEngine::new().expect("RenderEngine::new");

    let tmp = env::temp_dir().join(format!("redline-ocr-rerun-{}", std::process::id()));
    fs::create_dir_all(&tmp).expect("scratch dir");

    // Ground-truth page -> 200 dpi raster -> JPEG -> image-only NUL-padded scan.
    let truth_path = tmp.join("truth.pdf");
    fs::write(&truth_path, text_pdf(&[PAGE_1, PAGE_2])).expect("write truth pdf");
    engine
        .open_document(truth_path, "truth".to_string(), None)
        .expect("open truth");
    let mut rasters = Vec::new();
    for page in 0..2u32 {
        let r = engine
            .render_page_full("truth", page, 200.0)
            .expect("render truth");
        // A runner with no usable fonts renders the text page blank; that would make
        // every later assertion about OCR output meaningless, so skip loudly instead.
        let dark = r.rgb.chunks(3).filter(|p| p[0] < 128).count();
        if dark * 200 < r.rgb.len() / 3 {
            engine.close_document("truth");
            let _ = fs::remove_dir_all(&tmp);
            skip_or_fail(&format!(
                "truth page {page} rendered (nearly) blank - no system fonts?"
            ));
            return;
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 70)
            .encode(
                &r.rgb,
                r.width_px,
                r.height_px,
                image::ExtendedColorType::Rgb8,
            )
            .expect("jpeg encode");
        rasters.push((jpeg, r.width_px, r.height_px));
    }
    engine.close_document("truth");
    let scan_bytes = scan_pdf(&rasters);
    assert!(
        scan_bytes.len() < 1_500_000,
        "fixture should stay small, got {}",
        scan_bytes.len()
    );
    let scan_path = tmp.join("scan.pdf");
    fs::write(&scan_path, &scan_bytes).expect("write scan");
    assert_eq!(
        extract_pdf_text(&scan_path)
            .unwrap()
            .iter()
            .map(|(_, t)| t.trim().len())
            .sum::<usize>(),
        0,
        "fixture must start with no extractable text"
    );

    // --- Run 1 -------------------------------------------------------------------
    let pages = recognise(&mut engine, &mut ocr, &scan_path, "scan1");
    let run1 = write_ocr_pdf(&scan_bytes, &pages, DEFAULT_MIN_CONFIDENCE).expect("write run 1");
    assert_eq!(
        layers_per_page(&run1),
        vec![1, 1],
        "run 1 must write one layer per page"
    );

    // The NUL-padded stream must not hide the layer from the indexer's extractor.
    let run1_path = tmp.join("run1.pdf");
    fs::write(&run1_path, &run1).expect("write run1");
    let extracted = extract_pdf_text(&run1_path).expect("extract run 1");
    for (page, text) in &extracted {
        let n = text.chars().filter(|c| !c.is_whitespace()).count();
        assert!(
            n > 500,
            "page {page}: extractor sees only {n} chars of the OCR layer"
        );
    }

    // --- Run 2 over the OCR'd file (what happens on a re-run) ----------------------
    let pages2 = recognise(&mut engine, &mut ocr, &run1_path, "scan2");
    let run2 = write_ocr_pdf(&run1, &pages2, DEFAULT_MIN_CONFIDENCE).expect("write run 2");
    assert_eq!(
        layers_per_page(&run2),
        vec![1, 1],
        "a second OCR run must replace the layer, not stack a second one"
    );

    // --- Noise: junk lines as a share of layer characters --------------------------
    let truth = vocab(&format!("{PAGE_1} {PAGE_2}"));
    let run2_path = tmp.join("run2.pdf");
    fs::write(&run2_path, &run2).expect("write run2");
    let layer_lines: Vec<String> = extract_pdf_text(&run2_path)
        .expect("extract run 2")
        .into_iter()
        .flat_map(|(_, t)| t.lines().map(str::to_string).collect::<Vec<_>>())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let (junk, total) = junk_ratio(&layer_lines, &truth);
    let ratio = junk as f64 / total.max(1) as f64;
    eprintln!(
        "layer lines {}, layer chars {total}, junk chars {junk}, junk ratio {:.2}%",
        layer_lines.len(),
        ratio * 100.0
    );
    assert!(
        ratio < 0.03,
        "junk lines are {:.1}% of layer characters (limit 3%)",
        ratio * 100.0
    );

    // The filter must not eat real text: nearly every contract word is still found.
    let got = vocab(&layer_lines.join(" "));
    let recall = truth.iter().filter(|w| got.contains(*w)).count() as f64 / truth.len() as f64;
    eprintln!("word recall {:.2}%", recall * 100.0);
    assert!(recall > 0.95, "word recall fell to {:.1}%", recall * 100.0);

    let _ = fs::remove_dir_all(&tmp);
}
