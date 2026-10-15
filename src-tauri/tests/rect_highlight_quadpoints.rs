//! #149: a drag-rectangle Highlight saved to a real PDF must carry /QuadPoints so a second,
//! strict reader accepts it. Fixture-based: save with the real writer, then check the file with
//! qpdf and poppler (pdftoppm), which reported "Bad Annot Text Markup QuadPoints" before the fix.
//!
//! Needs `qpdf` and `pdftoppm` on PATH; skips with a message when missing.

use lopdf::{dictionary, Document, Object, Stream};
use redline_lib::document::save::save_with_markups_scaled;
use redline_lib::geometry::PdfPoint;
use redline_lib::markup::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("redline-quadpts-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn one_page_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(
        dictionary! {},
        b"0.9 g 0 0 300 200 re f".to_vec(),
    ));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 300.into(), 200.into()],
        "Contents" => content_id, "Resources" => dictionary! {},
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
fn rect_highlight_is_accepted_by_poppler_and_qpdf() {
    if !have("qpdf") || !have("pdftoppm") {
        eprintln!("SKIP: qpdf/pdftoppm not installed");
        return;
    }
    let src = scratch("src.pdf");
    let dest = scratch("out.pdf");
    std::fs::write(&src, one_page_pdf()).unwrap();
    let user = UserRef {
        user_id: uuid::Uuid::new_v4(),
        display_name: "T".into(),
    };
    let mut a = Appearance::default();
    a.color = "#ffe000".into();
    let m = Markup::new(
        MarkupType::Highlight,
        0,
        MarkupGeometry::Rect {
            min: PdfPoint { x: 40.0, y: 60.0 },
            max: PdfPoint { x: 200.0, y: 90.0 },
        },
        a,
        user,
    );
    save_with_markups_scaled(&src, &dest, &[m], &HashSet::new(), &[]).unwrap();

    let q = Command::new("qpdf")
        .arg("--check")
        .arg(&dest)
        .output()
        .unwrap();
    assert!(
        q.status.success(),
        "qpdf --check failed: {}",
        String::from_utf8_lossy(&q.stderr)
    );

    let r = Command::new("pdftoppm")
        .args(["-r", "36", "-png"])
        .arg(&dest)
        .arg(scratch("page"))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(r.status.success(), "pdftoppm failed: {err}");
    assert!(
        !err.contains("QuadPoints"),
        "poppler rejected the highlight: {err}"
    );
    assert!(
        !err.contains("Syntax"),
        "poppler reported a syntax problem: {err}"
    );
}
