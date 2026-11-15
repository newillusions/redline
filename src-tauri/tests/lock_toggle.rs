//! Lock / Unlock toggle: the store flips only the standard `Locked` /F bit, a locked markup
//! refuses edits and deletes (GUI `update_markup` and the MCP bridge share that guard), unlocking
//! makes it editable again, and the flag round-trips through a saved PDF.

use lopdf::{dictionary, Document, Object, Stream};
use redline_lib::document::save::{load_markups_from, save_with_markups_scaled};
use redline_lib::document::store::MarkupStore;
use redline_lib::geometry::PdfPoint;
use redline_lib::markup::*;
use std::collections::HashSet;
use std::path::PathBuf;

fn markup() -> Markup {
    Markup::new(
        MarkupType::Rectangle,
        0,
        MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 10.0 },
            max: PdfPoint { x: 80.0, y: 40.0 },
        },
        Appearance::default(),
        UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "T".into(),
        },
    )
}

fn store_with(m: &Markup) -> MarkupStore {
    let s = MarkupStore::default();
    s.register("d1", PathBuf::from("/tmp/lock-toggle.pdf"), None);
    s.add("d1", m.clone()).unwrap();
    s
}

#[test]
fn lock_sets_only_the_locked_bit_and_unlock_clears_only_it() {
    let mut m = markup();
    m.annot_flags = 4 | 0x200; // Print + LockedContents, both must survive
    let s = store_with(&m);
    s.set_locked("d1", m.id(), true).unwrap();
    let got = &s.list("d1").unwrap()[0];
    assert!(got.is_locked());
    assert_eq!(got.annot_flags, 4 | 0x200 | 0x80);
    s.set_locked("d1", m.id(), false).unwrap();
    let got = &s.list("d1").unwrap()[0];
    assert!(!got.is_locked());
    assert_eq!(got.annot_flags, 4 | 0x200);
}

#[test]
fn locked_markup_refuses_move_edit_and_delete_then_unlock_allows_them() {
    let m = markup();
    let s = store_with(&m);
    s.set_locked("d1", m.id(), true).unwrap();

    let mut edited = s.list("d1").unwrap()[0].clone();
    edited.contents = Some("edit".into());
    let err = s.update("d1", edited.clone()).unwrap_err();
    assert!(err.contains("markup_locked"), "got: {err}");
    let err = s.delete("d1", m.id()).unwrap_err();
    assert!(err.contains("markup_locked"), "got: {err}");

    s.set_locked("d1", m.id(), false).unwrap();
    edited.set_locked(false);
    s.update("d1", edited).unwrap();
    assert_eq!(s.list("d1").unwrap()[0].contents, Some("edit".into()));
    s.delete("d1", m.id()).unwrap();
}

#[test]
fn toggle_marks_the_document_dirty_and_a_repeat_is_a_no_op() {
    let m = markup();
    let s = store_with(&m);
    s.clear_dirty("d1");
    s.set_locked("d1", m.id(), true).unwrap();
    assert!(s.is_dirty("d1"), "a real toggle makes the document dirty");
    s.clear_dirty("d1");
    s.set_locked("d1", m.id(), true).unwrap();
    assert!(!s.is_dirty("d1"), "locking a locked markup changes nothing");
}

#[test]
fn unknown_ids_are_errors() {
    let m = markup();
    let s = store_with(&m);
    assert!(s.set_locked("nope", m.id(), true).is_err());
    assert!(s.set_locked("d1", uuid::Uuid::new_v4(), true).is_err());
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
fn locked_flag_saves_as_the_standard_f_bit_and_reads_back() {
    let dir = std::env::temp_dir().join(format!("redline-lock-toggle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (src, dest) = (dir.join("src.pdf"), dir.join("out.pdf"));
    std::fs::write(&src, one_page_pdf()).unwrap();

    let mut m = markup();
    m.set_locked(true);
    save_with_markups_scaled(&src, &dest, &[m.clone()], &HashSet::new(), &[]).unwrap();

    let doc = Document::load(&dest).unwrap();
    let f = doc
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .find(|d| {
            d.get(b"Subtype")
                .and_then(|s| s.as_name())
                .map(|n| n == b"Square")
                .unwrap_or(false)
        })
        .and_then(|d| d.get(b"F").ok().and_then(|f| f.as_i64().ok()))
        .expect("annotation /F written");
    assert_eq!(f & 0x80, 0x80, "Locked bit (128) set in /F, got {f}");

    let back = load_markups_from(&dest, None).unwrap();
    assert_eq!(back.len(), 1);
    assert!(back[0].is_locked());
    let _ = std::fs::remove_dir_all(&dir);
}
