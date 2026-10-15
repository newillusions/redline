//! Acceptance test for true redaction: the redacted text must be absent from the output
//! (as seen by `pdftotext`), the black box must be present, and unredacted text must survive.
//!
//! Requires `pdftotext` and `mutool` on PATH; skips (with a message) when missing so the
//! suite stays portable.

use lopdf::{dictionary, Document, Object, Stream};
use redline_lib::docops::{redact_regions, RedactRegion};
use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-v").output().is_ok()
        || Command::new(tool).arg("--help").output().is_ok()
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("redline-redact-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// 300x200 page: three text lines (Helvetica with explicit /Widths), one inside a Form
/// XObject, a vector box around line 1, and an RGB image carrying a dark square.
fn build_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    // Uniform 500/1000 em advance keeps the geometry easy to reason about.
    let widths: Vec<Object> = (32..=126).map(|_| Object::Integer(500)).collect();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "FirstChar" => 32, "LastChar" => 126, "Widths" => widths,
    });
    let form_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        },
        b"BT /F1 12 Tf 20 60 Td (FORMSECRET 4242) Tj ET".to_vec(),
    ));
    let mut px = vec![200u8; 8 * 8 * 3];
    for y in 2..6 {
        for x in 2..6 {
            let i = (y * 8 + x) * 3;
            px[i..i + 3].copy_from_slice(&[255, 0, 0]);
        }
    }
    let img_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => 8, "Height" => 8,
            "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
        },
        px,
    ));
    let content = b"BT /F1 12 Tf 20 150 Td (CONFIDENTIAL SALARY 123456) Tj ET\n\
        BT /F1 12 Tf 20 120 Td [(PUBLIC ) -20 (LINE TWO)] TJ ET\n\
        /Fm1 Do\n\
        q 40 0 0 40 230 20 cm /Im1 Do Q\n\
        10 140 200 25 re S\n"
        .to_vec();
    let content_id = doc.add_object(Stream::new(dictionary! {}, content));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
        "Contents" => content_id,
        "Resources" => dictionary! {
            "Font" => dictionary! { "F1" => font_id },
            "XObject" => dictionary! { "Fm1" => form_id, "Im1" => img_id },
        },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn pdftotext(path: &PathBuf) -> String {
    let o = Command::new("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn redact(bytes: &[u8], regions: &[RedactRegion]) -> Vec<u8> {
    let mut doc = Document::load_mem(bytes).unwrap();
    redact_regions(&mut doc, regions).unwrap();
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn redacted_text_is_absent_from_pdftotext_and_box_is_present() {
    if !have("pdftotext") || !have("mutool") {
        eprintln!("SKIP: pdftotext/mutool not installed");
        return;
    }
    let src = scratch("src.pdf");
    std::fs::write(&src, build_pdf()).unwrap();
    let before = pdftotext(&src);
    assert!(
        before.contains("CONFIDENTIAL SALARY 123456"),
        "fixture broken: {before}"
    );
    assert!(
        before.contains("FORMSECRET 4242"),
        "fixture broken: {before}"
    );

    // Line 1 sits at baseline y=150 (box 147.0..162); the form line at y=60 (57..71).
    let regions = [
        RedactRegion {
            page_index: 0,
            x: 15.0,
            y: 146.0,
            width: 170.0,
            height: 18.0,
        },
        RedactRegion {
            page_index: 0,
            x: 15.0,
            y: 56.0,
            width: 100.0,
            height: 16.0,
        },
        RedactRegion {
            page_index: 0,
            x: 230.0,
            y: 20.0,
            width: 20.0,
            height: 40.0,
        },
    ];
    let out = redact(&std::fs::read(&src).unwrap(), &regions);
    let dst = scratch("redacted.pdf");
    std::fs::write(&dst, &out).unwrap();

    let after = pdftotext(&dst);
    println!("pdftotext after redaction:\n{after}");
    assert!(
        !after.contains("CONFIDENTIAL"),
        "sensitive text survived: {after}"
    );
    assert!(
        !after.contains("123456"),
        "sensitive text survived: {after}"
    );
    assert!(!after.contains("FORMSECRET"), "form text survived: {after}");
    assert!(!after.contains("4242"), "form text survived: {after}");
    assert!(after.contains("PUBLIC"), "unredacted text lost: {after}");
    assert!(after.contains("LINE TWO"), "unredacted text lost: {after}");

    // Raw bytes: the secret must not be recoverable from the file at all, including the
    // orphaned original content streams.
    let mut d2 = Document::load_mem(&out).unwrap();
    d2.decompress();
    let mut all = Vec::new();
    d2.save_to(&mut all).unwrap();
    let hay = String::from_utf8_lossy(&all);
    assert!(
        !hay.contains("CONFIDENTIAL"),
        "secret still present in file objects"
    );
    assert!(
        !hay.contains("FORMSECRET"),
        "form secret still present in file objects"
    );

    // Visual box: render at 72 dpi and check the redacted regions are black.
    let png = scratch("redacted.png");
    let st = Command::new("mutool")
        .args(["draw", "-r", "72", "-o"])
        .arg(&png)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());
    let img = image::open(&png).unwrap().to_luma8();
    let h = img.height();
    // PDF (x=20..180, y=148..160) -> pixel rows from the top.
    let mut dark = 0;
    let mut total = 0;
    for px in 20..180u32 {
        for py_pdf in 149..159u32 {
            let py = h - py_pdf;
            total += 1;
            if img.get_pixel(px, py)[0] < 40 {
                dark += 1;
            }
        }
    }
    assert!(
        dark * 100 / total >= 95,
        "redaction box not solid black: {dark}/{total}"
    );

    // Image pixels under the third region are zeroed in the stored image.
    let mut d3 = Document::load_mem(&out).unwrap();
    d3.decompress();
    let mut found_img = false;
    for obj in d3.objects.values() {
        if let Object::Stream(s) = obj {
            if s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(b"Image")
                && s.content.len() == 8 * 8 * 3
            {
                found_img = true;
                // Region x 230..250 = left half of the 40pt-wide image => columns 0..3.
                for row in 0..8 {
                    for col in 0..4 {
                        let i = (row * 8 + col) * 3;
                        assert_eq!(
                            &s.content[i..i + 3],
                            &[0, 0, 0],
                            "pixel ({col},{row}) not blanked"
                        );
                    }
                }
                // Right half untouched (column 7 is background 200).
                assert_eq!(&s.content[7 * 3..7 * 3 + 3], &[200, 200, 200]);
            }
        }
    }
    assert!(found_img, "image XObject missing");
}

#[test]
fn kerned_tj_keeps_neighbours_in_place() {
    if !have("pdftotext") {
        return;
    }
    let src = scratch("kern.pdf");
    std::fs::write(&src, build_pdf()).unwrap();
    // Line 2 "PUBLIC LINE TWO" at y=120; redact just "PUBLIC " (7 glyphs * 6pt = 42pt).
    let out = redact(
        &std::fs::read(&src).unwrap(),
        &[RedactRegion {
            page_index: 0,
            x: 19.0,
            y: 118.0,
            width: 40.0,
            height: 14.0,
        }],
    );
    let dst = scratch("kern-out.pdf");
    std::fs::write(&dst, &out).unwrap();
    let t = pdftotext(&dst);
    assert!(!t.contains("PUBLIC"), "{t}");
    assert!(t.contains("LINE TWO"), "{t}");
    assert!(
        t.contains("CONFIDENTIAL SALARY 123456"),
        "line 1 must be untouched: {t}"
    );
}

/// Real-world smoke (ignored by default): `REDLINE_REDACT_PDF=/path/a.pdf cargo test --test
/// redact_true_removal real_pdf -- --ignored --nocapture`. Redacts the bounding boxes of the
/// first 5 words of page 1 (from `pdftotext -bbox`) and asserts each is gone from the output.
#[test]
#[ignore]
fn real_pdf_words_removed() {
    let Ok(path) = std::env::var("REDLINE_REDACT_PDF") else {
        return;
    };
    let bb = Command::new("pdftotext")
        .args(["-f", "1", "-l", "1", "-bbox"])
        .arg(&path)
        .arg("-")
        .output()
        .unwrap();
    let txt = String::from_utf8_lossy(&bb.stdout).to_string();
    let page_h: f64 = txt
        .split("<page width=\"")
        .nth(1)
        .and_then(|s| s.split("height=\"").nth(1))
        .and_then(|s| s.split('"').next())
        .unwrap()
        .parse()
        .unwrap();
    let mut words: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    for line in txt.lines().filter(|l| l.contains("<word ")) {
        let f = |k: &str| -> f64 {
            line.split(&format!("{k}=\""))
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        let w = line
            .split('>')
            .nth(1)
            .unwrap()
            .split('<')
            .next()
            .unwrap()
            .to_string();
        if w.len() >= 5 && w.chars().all(|c| c.is_ascii_alphabetic()) {
            words.push((w, f("xMin"), f("yMin"), f("xMax"), f("yMax")));
        }
        if words.len() == 5 {
            break;
        }
    }
    assert!(!words.is_empty(), "no usable words");
    let regions: Vec<RedactRegion> = words
        .iter()
        .map(|(_, x0, y0, x1, y1)| RedactRegion {
            page_index: 0,
            x: *x0,
            y: page_h - *y1,
            width: x1 - x0,
            height: y1 - y0,
        })
        .collect();
    let out = redact(&std::fs::read(&path).unwrap(), &regions);
    let dst = scratch("real-out.pdf");
    std::fs::write(&dst, &out).unwrap();
    let after = Command::new("pdftotext")
        .args(["-f", "1", "-l", "1"])
        .arg(&dst)
        .arg("-")
        .output()
        .unwrap();
    let after = String::from_utf8_lossy(&after.stdout).to_string();
    let before_all = Command::new("pdftotext")
        .args(["-f", "1", "-l", "1"])
        .arg(&path)
        .arg("-")
        .output()
        .unwrap();
    let before_all = String::from_utf8_lossy(&before_all.stdout).to_string();
    for (w, ..) in &words {
        let nb = before_all.matches(w.as_str()).count();
        let na = after.matches(w.as_str()).count();
        println!("{w}: before {nb} after {na}");
        assert!(na < nb, "{w} was not removed (before {nb}, after {na})");
    }
    println!("chars before {} after {}", before_all.len(), after.len());
}

fn single_page(content: &[u8], font: Object) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(font);
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
        ),
    );
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn font_without_widths_drops_the_whole_show_operation() {
    if !have("pdftotext") {
        return;
    }
    // Standard-14 font, no /Widths: extent unknowable, so the operation is dropped when its
    // line touches the region (over-removal is the safe direction).
    let pdf = single_page(
        b"BT /F1 12 Tf 20 150 Td (TOPSECRET LINE) Tj ET\nBT /F1 12 Tf 20 100 Td (KEEP ME) Tj ET",
        Object::Dictionary(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        ),
    );
    let out = redact(
        &pdf,
        &[RedactRegion {
            page_index: 0,
            x: 18.0,
            y: 147.0,
            width: 40.0,
            height: 14.0,
        }],
    );
    let dst = scratch("nowidths.pdf");
    std::fs::write(&dst, &out).unwrap();
    let t = pdftotext(&dst);
    assert!(!t.contains("TOPSECRET"), "{t}");
    assert!(t.contains("KEEP ME"), "{t}");
}

#[test]
fn vector_strokes_through_the_region_are_removed_others_kept() {
    let pdf = single_page(
        b"10 100 m 290 100 l S\n10 20 m 290 20 l S",
        Object::Dictionary(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        ),
    );
    let out = redact(
        &pdf,
        &[RedactRegion {
            page_index: 0,
            x: 100.0,
            y: 90.0,
            width: 50.0,
            height: 20.0,
        }],
    );
    let mut d = Document::load_mem(&out).unwrap();
    d.decompress();
    let page = *d.get_pages().values().next().unwrap();
    let content = String::from_utf8_lossy(&d.get_page_content(page)).to_string();
    assert!(
        !content.contains("100 m") && !content.contains(" 100 l"),
        "line through region survived: {content}"
    );
    assert!(content.contains("20 m"), "unrelated line lost: {content}");
}
