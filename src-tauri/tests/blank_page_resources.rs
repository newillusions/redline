//! #149: an inserted blank page must be a well-formed page in a second reader: /Resources
//! present in the saved file, `qpdf --check` clean, and mutool renders it without warnings.
//! Skips when qpdf/mutool are missing.

use lopdf::{dictionary, Document, Object, Stream};
use redline_lib::document::page_ops::insert_blank_page;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
        || Command::new(tool).arg("-v").output().is_ok()
}

fn one_page_doc() -> Document {
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
    doc
}

#[test]
fn inserted_blank_page_is_well_formed_in_a_second_reader() {
    let mut doc = one_page_doc();
    insert_blank_page(&mut doc, 1, 595.0, 842.0).unwrap();
    let dir = std::env::temp_dir().join(format!("redline-blank-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blank.pdf");
    doc.save(&path).unwrap();

    // Saved file, read back by an independent parse: page 2 has /Resources.
    let back = Document::load(&path).unwrap();
    let p2 = back.get_pages()[&2];
    assert!(
        back.get_dictionary(p2).unwrap().has(b"Resources"),
        "saved blank page lacks /Resources"
    );

    if !have("qpdf") || !have("mutool") {
        eprintln!("SKIP second-reader checks: qpdf/mutool not installed");
        return;
    }
    let q = Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    let qout = format!(
        "{}{}",
        String::from_utf8_lossy(&q.stdout),
        String::from_utf8_lossy(&q.stderr)
    );
    assert!(q.status.success(), "qpdf --check failed: {qout}");
    assert!(!qout.contains("WARNING"), "qpdf warned: {qout}");
    let m = Command::new("mutool")
        .args(["draw", "-o"])
        .arg(dir.join("p.png"))
        .arg(&path)
        .arg("2")
        .output()
        .unwrap();
    let merr = String::from_utf8_lossy(&m.stderr);
    assert!(m.status.success(), "mutool draw failed: {merr}");
    assert!(
        !merr.to_lowercase().contains("warning") && !merr.to_lowercase().contains("error"),
        "mutool: {merr}"
    );
}
