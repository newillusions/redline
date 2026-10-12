//! Unedited foreign annotations survive a redline save untouched; edited ones are rebuilt
//! faithfully (cloud intensity, callout line ending, valid /CL); inline annotations sharing a
//! /NM are told apart; every case runs three load/save cycles with Revu-style 16-char /NM.

use super::callout::LineEnding;
use super::*;
use crate::document::annots::tests::one_page_doc;
use crate::document::annots::{page_annots, read_markups, write_markups, write_markups_removing};
use crate::document::store::MarkupStore;
use crate::geometry::PdfPoint;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

fn annots(doc: &Document) -> Vec<(Option<ObjectId>, Dictionary)> {
    let page = *doc.get_pages().values().next().unwrap();
    page_annots(doc, page).unwrap()
}

fn reload(doc: &mut Document) -> Document {
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    Document::load_mem(&buf).unwrap()
}

fn set_annots(doc: &mut Document, page: ObjectId, items: Vec<Object>) {
    doc.get_dictionary_mut(page)
        .unwrap()
        .set("Annots", Object::Array(items));
}

fn subtype(d: &Dictionary) -> String {
    d.get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default()
}

fn nm(d: &Dictionary) -> Option<String> {
    d.get(b"NM")
        .ok()?
        .as_str()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

fn floats(d: &Dictionary, key: &[u8]) -> Vec<f32> {
    d.get(key)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_float().unwrap())
        .collect()
}

fn ap_bytes(doc: &Document, d: &Dictionary) -> Vec<u8> {
    let ap = d.get(b"AP").unwrap();
    let ap = match ap {
        Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
        o => o.as_dict().unwrap(),
    };
    let n = ap.get(b"N").unwrap().as_reference().unwrap();
    let s = doc.get_object(n).unwrap().as_stream().unwrap();
    s.decompressed_content().unwrap_or_else(|_| s.content.clone())
}

const ORIGINAL_AP: &[u8] = b"q 1 0 0 RG 5 5 m 50 5 l 50 40 l S Q ORIGINAL-REVU-AP";

fn form(doc: &mut Document, content: &[u8]) -> ObjectId {
    doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
        },
        content.to_vec(),
    ))
}

fn revu_cloud(doc: &mut Document, nm: &str, intensity: i64) -> ObjectId {
    let ap = form(doc, ORIGINAL_AP);
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Polygon", "F" => 4,
        "NM" => Object::string_literal(nm),
        "IT" => "PolygonCloud",
        "BE" => dictionary! { "S" => "C", "I" => intensity },
        "Rect" => vec![10.into(), 10.into(), 100.into(), 80.into()],
        "Vertices" => vec![10.into(), 10.into(), 100.into(), 10.into(), 100.into(), 80.into(), 10.into(), 80.into()],
        "C" => vec![1.into(), 0.into(), 0.into()],
        "BSIColumnData" => vec![Object::string_literal("col")],
        "AP" => dictionary! { "N" => ap },
    })
}

fn revu_callout(doc: &mut Document, nm: &str, le: Option<&str>) -> ObjectId {
    let ap = form(doc, ORIGINAL_AP);
    let mut d = dictionary! {
        "Type" => "Annot", "Subtype" => "FreeText", "F" => 4,
        "NM" => Object::string_literal(nm),
        "IT" => "FreeTextCallout",
        "Contents" => Object::string_literal("see detail"),
        "DA" => Object::string_literal("0 0 0 rg /Helv 10 Tf"),
        "Rect" => vec![100.into(), 100.into(), 300.into(), 200.into()],
        "RD" => vec![60.into(), 5.into(), 10.into(), 20.into()],
        "CL" => vec![100.into(), 110.into(), 150.into(), 160.into(), 160.into(), 180.into()],
        "AP" => dictionary! { "N" => ap },
    };
    if let Some(le) = le {
        d.set("LE", Object::Name(le.as_bytes().to_vec()));
    }
    doc.add_object(d)
}

fn revu_polylength(doc: &mut Document, nm: &str) -> ObjectId {
    let ap = form(doc, ORIGINAL_AP);
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "PolyLine", "F" => 4,
        "NM" => Object::string_literal(nm),
        "IT" => "PolyLineDimension",
        "Rect" => vec![10.into(), 10.into(), 200.into(), 90.into()],
        "Vertices" => vec![10.into(), 10.into(), 100.into(), 50.into(), 200.into(), 90.into()],
        "Measure" => dictionary! {
            "Type" => "Measure", "Subtype" => "RL",
            "R" => Object::string_literal("1 in = 1 ft"),
            "X" => vec![dictionary! { "Type" => "NumberFormat", "U" => Object::string_literal("ft"), "C" => 1 }.into()],
        },
        "AP" => dictionary! { "N" => ap },
    })
}

/// One load -> (optional edit) -> save -> reload cycle.
fn cycle(doc: &mut Document, edit: impl FnOnce(&mut Vec<Markup>)) {
    let mut ms = read_markups(doc).unwrap();
    edit(&mut ms);
    write_markups(doc, &ms).unwrap();
    *doc = reload(doc);
}

fn only(doc: &Document, st: &str) -> Dictionary {
    let all: Vec<_> = annots(doc)
        .into_iter()
        .filter(|(_, d)| subtype(d) == st)
        .collect();
    assert_eq!(all.len(), 1, "expected one {st}, got {}", all.len());
    all.into_iter().next().unwrap().1
}

fn intensity(d: &Dictionary) -> f32 {
    d.get(b"BE")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"I")
        .unwrap()
        .as_float()
        .unwrap()
}

// --- 1. unedited foreign cloud ---------------------------------------------------------

#[test]
fn unedited_revu_cloud_keeps_its_intensity_and_original_appearance_over_three_cycles() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = only(&doc, "Polygon");
        assert_eq!(intensity(&d), 1.0, "cycle {n}: /BE /I must stay 1");
        assert_eq!(nm(&d).as_deref(), Some("CLOUDREVU0000001"), "cycle {n}");
        assert_eq!(ap_bytes(&doc, &d), ORIGINAL_AP, "cycle {n}: original /AP byte-for-byte");
        assert!(!d.has(b"RLType"), "cycle {n}: not turned into a redline annotation");
        assert!(d.has(b"BSIColumnData"), "cycle {n}");
        assert_eq!(floats(&d, b"Vertices").len(), 8, "cycle {n}");
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

#[test]
fn edited_revu_cloud_keeps_its_intensity_and_gets_a_curved_appearance() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |ms| ms[0].contents = Some(format!("edit {n}")));
        let d = only(&doc, "Polygon");
        assert_eq!(intensity(&d), 1.0, "cycle {n}");
        assert_eq!(nm(&d).as_deref(), Some("CLOUDREVU0000001"), "cycle {n}");
        let ap = String::from_utf8_lossy(&ap_bytes(&doc, &d)).into_owned();
        assert!(ap.contains(" c\n"), "cycle {n}: scalloped Bezier arcs: {ap}");
        assert!(!ap.contains(" l\n"), "cycle {n}: no straight edges: {ap}");
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

#[test]
fn a_redline_authored_cloud_is_still_written_at_intensity_two() {
    let (mut doc, page) = one_page_doc();
    set_annots(&mut doc, page, vec![]);
    let m = Markup::new(
        MarkupType::Cloud,
        0,
        MarkupGeometry::Polyline(vec![
            PdfPoint { x: 0.0, y: 0.0 },
            PdfPoint { x: 50.0, y: 0.0 },
            PdfPoint { x: 50.0, y: 50.0 },
        ]),
        Appearance::default(),
        UserRef { user_id: uuid::Uuid::new_v4(), display_name: "A".into() },
    );
    write_markups(&mut doc, &[m]).unwrap();
    let doc = reload(&mut doc);
    assert_eq!(intensity(&only(&doc, "Polygon")), 2.0);
}

// --- 2. callout /LE ---------------------------------------------------------------------

#[test]
fn unedited_revu_callout_is_written_back_verbatim_for_every_line_ending() {
    for le in [None, Some("OpenArrow"), Some("None"), Some("Circle"), Some("ClosedArrow")] {
        let (mut doc, page) = one_page_doc();
        let c = revu_callout(&mut doc, "CALLOUTREVU00001", le);
        set_annots(&mut doc, page, vec![c.into()]);
        for n in 1..=3 {
            cycle(&mut doc, |_| {});
            let d = only(&doc, "FreeText");
            let got = d.get(b"LE").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec());
            assert_eq!(got, le.map(|s| s.as_bytes().to_vec()), "{le:?} cycle {n}");
            assert_eq!(floats(&d, b"CL"), vec![100.0, 110.0, 150.0, 160.0, 160.0, 180.0], "{le:?} cycle {n}");
            assert_eq!(floats(&d, b"RD"), vec![60.0, 5.0, 10.0, 20.0], "{le:?} cycle {n}");
            assert_eq!(ap_bytes(&doc, &d), ORIGINAL_AP, "{le:?} cycle {n}");
            assert_eq!(annots(&doc).len(), 1, "{le:?} cycle {n}");
        }
    }
}

#[test]
fn edited_revu_callout_writes_the_line_ending_it_was_read_with_and_draws_it() {
    for (le, filled_head) in [
        (None, false),
        (Some("OpenArrow"), false),
        (Some("None"), false),
        (Some("Circle"), false),
        (Some("ClosedArrow"), true),
    ] {
        let (mut doc, page) = one_page_doc();
        let c = revu_callout(&mut doc, "CALLOUTREVU00001", le);
        set_annots(&mut doc, page, vec![c.into()]);
        for n in 1..=3 {
            cycle(&mut doc, |ms| ms[0].contents = Some(format!("edit {n}")));
            let d = only(&doc, "FreeText");
            let got = d.get(b"LE").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec());
            assert_eq!(got, le.map(|s| s.as_bytes().to_vec()), "{le:?} cycle {n}");
            let ap = String::from_utf8_lossy(&ap_bytes(&doc, &d)).into_owned();
            assert_eq!(ap.contains("h\nf\n"), filled_head, "{le:?} cycle {n} filled head: {ap}");
            if le == Some("Circle") {
                assert!(ap.contains(" c\n"), "circle ending drawn: {ap}");
            }
            assert_eq!(floats(&d, b"CL").len(), 6, "{le:?} cycle {n}");
            assert_eq!(annots(&doc).len(), 1, "{le:?} cycle {n}");
        }
    }
}

#[test]
fn a_callout_redline_authored_gets_a_closed_arrow() {
    let m = Markup::new(
        MarkupType::Callout,
        0,
        MarkupGeometry::Polyline(vec![PdfPoint { x: 10.0, y: 10.0 }, PdfPoint { x: 60.0, y: 60.0 }]),
        Appearance::default(),
        UserRef { user_id: uuid::Uuid::new_v4(), display_name: "A".into() },
    );
    let d = m.to_annotation_dict();
    assert_eq!(d.get(b"LE").unwrap().as_name().unwrap(), b"ClosedArrow");
    assert_eq!(callout::line_ending(&m), LineEnding::closed_arrow());
}

// --- 3. /CL validity --------------------------------------------------------------------

#[test]
fn cl_is_never_written_with_fewer_than_two_finite_leader_points() {
    let nan = f64::NAN;
    for pts in [
        vec![],
        vec![PdfPoint { x: 1.0, y: 1.0 }],
        vec![PdfPoint { x: nan, y: 1.0 }, PdfPoint { x: 5.0, y: 5.0 }],
        vec![PdfPoint { x: nan, y: nan }, PdfPoint { x: f64::INFINITY, y: 0.0 }],
    ] {
        let m = Markup::new(
            MarkupType::Callout,
            0,
            MarkupGeometry::Polyline(pts.clone()),
            Appearance::default(),
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "A".into() },
        );
        let d = m.to_annotation_dict();
        assert!(!d.has(b"CL"), "no /CL for {pts:?}");
    }
    // A valid leader still gets 4 or 6 numbers.
    let two = Markup::new(
        MarkupType::Callout,
        0,
        MarkupGeometry::Polyline(vec![PdfPoint { x: 1.0, y: 1.0 }, PdfPoint { x: 5.0, y: 5.0 }]),
        Appearance::default(),
        UserRef { user_id: uuid::Uuid::new_v4(), display_name: "A".into() },
    );
    assert_eq!(floats(&two.to_annotation_dict(), b"CL").len(), 4);
}

// --- 4. polylength /IT and /Measure ----------------------------------------------------

#[test]
fn unedited_revu_polylength_keeps_it_and_measure_over_three_cycles() {
    let (mut doc, page) = one_page_doc();
    let p = revu_polylength(&mut doc, "POLYLENREVU0001");
    set_annots(&mut doc, page, vec![p.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = only(&doc, "PolyLine");
        assert_eq!(
            d.get(b"IT").unwrap().as_name().unwrap(),
            b"PolyLineDimension",
            "cycle {n}"
        );
        let measure = d.get(b"Measure").unwrap().as_dict().unwrap();
        assert_eq!(measure.get(b"Subtype").unwrap().as_name().unwrap(), b"RL", "cycle {n}");
        assert_eq!(nm(&d).as_deref(), Some("POLYLENREVU0001"), "cycle {n}");
        assert_eq!(ap_bytes(&doc, &d), ORIGINAL_AP, "cycle {n}");
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

// --- 5. inline annotations sharing a /NM -----------------------------------------------

#[test]
fn two_inline_annotations_sharing_an_nm_are_not_confused_on_save() {
    let (mut doc, page) = one_page_doc();
    let square = dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
    };
    let attach = dictionary! {
        "Type" => "Annot", "Subtype" => "FileAttachment", "F" => 4,
        "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![200.into(), 200.into(), 220.into(), 220.into()],
    };
    set_annots(&mut doc, page, vec![Object::Dictionary(square), Object::Dictionary(attach)]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let all = annots(&doc);
        assert_eq!(all.len(), 2, "cycle {n}");
        assert_eq!(all.iter().filter(|(_, d)| subtype(d) == "FileAttachment").count(), 1, "cycle {n}");
        assert_eq!(all.iter().filter(|(_, d)| subtype(d) == "Square").count(), 1, "cycle {n}");
    }
    // Deleting the inline square removes only it.
    let (mut doc, page) = one_page_doc();
    let sq = dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
    };
    let fa = dictionary! {
        "Type" => "Annot", "Subtype" => "FileAttachment", "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![200.into(), 200.into(), 220.into(), 220.into()],
    };
    set_annots(&mut doc, page, vec![Object::Dictionary(sq), Object::Dictionary(fa)]);
    let store = MarkupStore::default();
    store.register("d", std::path::PathBuf::from("/tmp/x.pdf"), None);
    store.seed_loaded("d", read_markups(&doc).unwrap()).unwrap();
    let id = store.list("d").unwrap()[0].id();
    store.delete("d", id).unwrap();
    write_markups_removing(&mut doc, &store.list("d").unwrap(), &store.removed_sources("d")).unwrap();
    let doc = reload(&mut doc);
    let all = annots(&doc);
    assert_eq!(all.len(), 1);
    assert_eq!(subtype(&all[0].1), "FileAttachment");
}

// --- 6. edit tracking through the store -------------------------------------------------

fn loaded_store(doc: &Document) -> MarkupStore {
    let store = MarkupStore::default();
    store.register("d", std::path::PathBuf::from("/tmp/x.pdf"), None);
    store.seed_loaded("d", read_markups(doc).unwrap()).unwrap();
    store
}

#[test]
fn a_markup_sent_back_unchanged_through_json_stays_unedited_and_an_edit_does_not() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    let store = loaded_store(&doc);
    let m = store.list("d").unwrap().remove(0);
    assert!(m.is_unedited());
    // The frontend round-trip: serialise, deserialise, update (reader-owned fields come back
    // from the stored slot, never from the input).
    let echoed: Markup = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert!(!echoed.is_unedited(), "reader-owned state is not in the JSON");
    store.update("d", echoed.clone()).unwrap();
    assert!(store.list("d").unwrap()[0].is_unedited());
    let mut edited = echoed;
    edited.contents = Some("changed".into());
    store.update("d", edited).unwrap();
    let got = store.list("d").unwrap().remove(0);
    assert!(!got.is_unedited());
    assert!(got.source_dict.is_some(), "still the foreign markup, just edited");
    // Editing it back to the original makes it unedited again.
    let mut back = got.clone();
    back.contents = m.contents.clone();
    store.update("d", back).unwrap();
    assert!(store.list("d").unwrap()[0].is_unedited());
}

#[test]
fn reader_owned_original_never_comes_from_input() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    let m = read_markups(&doc).unwrap().remove(0);
    let store = new_empty_store();
    let mut input = m.clone();
    input.contents = Some("crafted".into());
    store.add("d", input).unwrap();
    let got = store.list("d").unwrap().remove(0);
    assert!(got.source_dict.is_none() && got.source_fp.is_none());
    assert!(got.source_intensity.is_none() && got.source_le.is_none());
    assert!(!got.is_unedited());
}

fn new_empty_store() -> MarkupStore {
    let store = MarkupStore::default();
    store.register("d", std::path::PathBuf::from("/tmp/x.pdf"), None);
    store
}

#[test]
fn delete_after_an_in_session_save_of_an_unedited_foreign_markup_still_reaches_the_file() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    let store = loaded_store(&doc);
    // Save 1 (unedited -> written back with the redline marker), then delete, save 2.
    write_markups_removing(&mut doc, &store.list("d").unwrap(), &store.removed_sources("d")).unwrap();
    doc = reload(&mut doc);
    assert_eq!(annots(&doc).len(), 1);
    // A second save of the unedited markup must not duplicate it either.
    write_markups_removing(&mut doc, &store.list("d").unwrap(), &store.removed_sources("d")).unwrap();
    doc = reload(&mut doc);
    assert_eq!(annots(&doc).len(), 1, "no duplicate on a second in-session save");
    let id = store.list("d").unwrap()[0].id();
    store.delete("d", id).unwrap();
    write_markups_removing(&mut doc, &store.list("d").unwrap(), &store.removed_sources("d")).unwrap();
    let doc = reload(&mut doc);
    assert_eq!(annots(&doc).len(), 0, "the delete reaches the file");
}

// --- 7. hostile originals ---------------------------------------------------------------

#[test]
fn an_unedited_originals_action_keys_are_stripped_at_every_level() {
    let (mut doc, page) = one_page_doc();
    let ap = form(&mut doc, ORIGINAL_AP);
    let js = dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert(1)") };
    let a = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("HOSTILEREVU00001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "A" => js.clone(),
        "AA" => dictionary! { "E" => js.clone() },
        "JS" => Object::string_literal("x"),
        "OpenAction" => js.clone(),
        "URI" => Object::string_literal("http://x"),
        "Launch" => js.clone(),
        "Dest" => vec![0.into()],
        "Custom" => dictionary! { "Keep" => 1, "A" => js.clone(), "Act" => js.clone(), "Nested" => vec![Object::Dictionary(js.clone())] },
        "AP" => dictionary! { "N" => ap },
    });
    set_annots(&mut doc, page, vec![a.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = only(&doc, "Square");
        for k in [&b"A"[..], b"AA", b"JS", b"OpenAction", b"URI", b"Launch", b"Dest"] {
            assert!(!d.has(k), "cycle {n}: /{} survived", String::from_utf8_lossy(k));
        }
        let custom = d.get(b"Custom").unwrap().as_dict().unwrap();
        assert!(custom.has(b"Keep") && !custom.has(b"A"), "cycle {n}");
        // Action dictionaries are dropped wherever they sit.
        assert!(
            custom.get(b"Act").is_err(),
            "cycle {n}: nested action dictionary survived"
        );
        assert!(custom.get(b"Nested").unwrap().as_array().unwrap().is_empty(), "cycle {n}");
        assert_eq!(ap_bytes(&doc, &d), ORIGINAL_AP, "cycle {n}");
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

#[test]
fn an_absurdly_large_original_is_rebuilt_not_carried_and_does_not_panic() {
    let (mut doc, page) = one_page_doc();
    let big: Vec<Object> = (0..300_000).map(|i| Object::Integer(i % 7)).collect();
    let a = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("HUGEREVU00000001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "Huge" => Object::Array(big),
    });
    set_annots(&mut doc, page, vec![a.into()]);
    let ms = read_markups(&doc).unwrap();
    assert!(ms[0].source_dict.is_none(), "too large to carry");
    assert!(!ms[0].is_unedited());
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = only(&doc, "Square");
        assert!(!d.has(b"Huge"), "cycle {n}");
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

#[test]
fn a_dangling_ap_reference_is_not_carried() {
    let (mut doc, page) = one_page_doc();
    let a = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("DANGLEREVU00001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "AP" => dictionary! { "N" => Object::Reference((9999, 0)) },
    });
    set_annots(&mut doc, page, vec![a.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = only(&doc, "Square");
        // Unedited, but with nothing usable to keep: redline supplies a valid appearance.
        assert!(!ap_bytes(&doc, &d).is_empty(), "cycle {n}");
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

// --- round 1 review fixes ---------------------------------------------------------------

fn reverse_annots(doc: &mut Document) {
    let page = *doc.get_pages().values().next().unwrap();
    let d = doc.get_dictionary_mut(page).unwrap();
    let mut items = d.get(b"Annots").unwrap().as_array().unwrap().clone();
    items.reverse();
    d.set("Annots", Object::Array(items));
}

fn save_in_session(doc: &mut Document, store: &MarkupStore) {
    write_markups_removing(doc, &store.list("d").unwrap(), &store.removed_sources("d")).unwrap();
    *doc = reload(doc);
    store.rebaseline_after_write("d");
}

fn names(doc: &Document) -> Vec<String> {
    let mut v: Vec<String> = annots(doc).iter().map(|(_, d)| subtype(d)).collect();
    v.sort();
    v
}

#[test]
fn b2_inline_refs_stay_correct_across_three_in_session_saves_with_reordering() {
    let (mut doc, page) = one_page_doc();
    let sq = dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
    };
    let fa = dictionary! {
        "Type" => "Annot", "Subtype" => "FileAttachment", "F" => 4,
        "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![200.into(), 200.into(), 220.into(), 220.into()],
    };
    set_annots(&mut doc, page, vec![Object::Dictionary(sq), Object::Dictionary(fa)]);
    let store = loaded_store(&doc);
    for n in 1..=3 {
        save_in_session(&mut doc, &store);
        assert_eq!(names(&doc), vec!["FileAttachment", "Square"], "save {n}");
        reverse_annots(&mut doc);
    }
    // Delete the square, then keep saving: the attachment must stay.
    let id = store.list("d").unwrap()[0].id();
    store.delete("d", id).unwrap();
    for n in 1..=3 {
        save_in_session(&mut doc, &store);
        assert_eq!(names(&doc), vec!["FileAttachment"], "after delete, save {n}");
        reverse_annots(&mut doc);
    }
}

#[test]
fn b2_an_inline_markup_deleted_and_undone_after_a_save_is_not_matched_to_a_neighbour() {
    let (mut doc, page) = one_page_doc();
    let sq = dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
    };
    let fa = dictionary! {
        "Type" => "Annot", "Subtype" => "FileAttachment", "F" => 4,
        "NM" => Object::string_literal("SHAREDSHAREDSHAR"),
        "Rect" => vec![200.into(), 200.into(), 220.into(), 220.into()],
    };
    set_annots(&mut doc, page, vec![Object::Dictionary(sq), Object::Dictionary(fa)]);
    let store = loaded_store(&doc);
    let m = store.list("d").unwrap().remove(0);
    store.delete("d", m.id()).unwrap();
    save_in_session(&mut doc, &store);
    store.add("d", m).unwrap(); // undo
    for n in 1..=3 {
        save_in_session(&mut doc, &store);
        assert_eq!(names(&doc), vec!["FileAttachment", "Square"], "save {n}");
        reverse_annots(&mut doc);
    }
}

#[test]
fn b1_a_preserved_appearance_that_is_no_longer_a_stream_is_rebuilt_not_pointed_at() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    let ms = read_markups(&doc).unwrap();
    assert!(ms[0].is_unedited());
    // The AP object is now something else (a dictionary): not an appearance any more.
    let ap_id = doc
        .get_dictionary(c)
        .unwrap()
        .get(b"AP")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"N")
        .unwrap()
        .as_reference()
        .unwrap();
    doc.objects.insert(ap_id, Object::Dictionary(dictionary! { "Imposter" => true }));
    write_markups(&mut doc, &ms).unwrap();
    let doc = reload(&mut doc);
    let d = only(&doc, "Polygon");
    assert_ne!(ap_bytes(&doc, &d), ORIGINAL_AP);
    assert!(String::from_utf8_lossy(&ap_bytes(&doc, &d)).contains(" c\n"));
    assert_eq!(annots(&doc).len(), 1);
}

#[test]
fn n1_the_marker_alone_does_not_make_a_link_or_widget_ours() {
    let (mut doc, page) = one_page_doc();
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "RLPreserved" => true,
        "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
    });
    let widget = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "RLPreserved" => true, "FT" => "Tx",
        "Rect" => vec![20.into(), 20.into(), 40.into(), 30.into()],
    });
    let sq = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "NM" => Object::string_literal("SQUAREREVU000001"),
        "Rect" => vec![50.into(), 50.into(), 90.into(), 90.into()],
    });
    set_annots(&mut doc, page, vec![link.into(), widget.into(), sq.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        assert_eq!(names(&doc), vec!["Link", "Square", "Widget"], "cycle {n}");
    }
}

fn js_action(doc: &mut Document) -> ObjectId {
    doc.add_object(dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert(1)") })
}

#[test]
fn n2_indirect_action_objects_are_not_carried_and_other_refs_are_inlined() {
    let (mut doc, page) = one_page_doc();
    let js = js_action(&mut doc);
    let measure = doc.add_object(dictionary! {
        "Type" => "Measure", "Subtype" => "RL", "A" => js, "R" => Object::string_literal("1 in = 1 ft"),
    });
    let ap = form(&mut doc, ORIGINAL_AP);
    let a = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("INDIRECTREVU0001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "A" => js, "Measure" => measure, "Nest" => vec![Object::Reference(js)],
        "AP" => dictionary! { "N" => ap },
    });
    set_annots(&mut doc, page, vec![a.into()]);
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        let d = only(&doc, "Square");
        assert!(!d.has(b"A"), "cycle {n}");
        let m = d.get(b"Measure").unwrap().as_dict().expect("inlined, not a reference");
        assert_eq!(m.get(b"Subtype").unwrap().as_name().unwrap(), b"RL", "cycle {n}");
        assert!(!m.has(b"A"), "cycle {n}");
        assert!(d.get(b"Nest").unwrap().as_array().unwrap().is_empty(), "cycle {n}");
        let any_js = doc.objects.values().any(|o| {
            o.as_dict().is_ok_and(|d| d.get(b"S").ok().and_then(|s| s.as_name().ok()) == Some(&b"JavaScript"[..]))
        });
        assert!(!any_js, "cycle {n}: no JavaScript action object left in the file");
        assert_eq!(ap_bytes(&doc, &d), ORIGINAL_AP, "cycle {n}");
    }
}

#[test]
fn n3_indirect_refs_survive_save_delete_save_undo_save() {
    let (mut doc, page) = one_page_doc();
    let measure = doc.add_object(dictionary! {
        "Type" => "Measure", "Subtype" => "RL", "R" => Object::string_literal("1 in = 1 ft"),
    });
    let a = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("MEASUREREVU0001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "Measure" => measure,
    });
    set_annots(&mut doc, page, vec![a.into()]);
    let store = loaded_store(&doc);
    let m = store.list("d").unwrap().remove(0);
    save_in_session(&mut doc, &store);
    store.delete("d", m.id()).unwrap();
    save_in_session(&mut doc, &store);
    assert!(annots(&doc).is_empty());
    store.add("d", m).unwrap();
    save_in_session(&mut doc, &store);
    let d = only(&doc, "Square");
    let meas = d.get(b"Measure").unwrap().as_dict().expect("direct dictionary, nothing to dangle");
    assert_eq!(meas.get(b"Subtype").unwrap().as_name().unwrap(), b"RL");
}

#[test]
fn n5_a_single_oversized_string_is_not_carried() {
    let (mut doc, page) = one_page_doc();
    let a = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("BIGSTRINGREVU001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "Big" => Object::String(vec![b'a'; 1_500_000], lopdf::StringFormat::Literal),
    });
    set_annots(&mut doc, page, vec![a.into()]);
    let ms = read_markups(&doc).unwrap();
    assert!(ms[0].source_dict.is_none());
    assert!(!ms[0].is_unedited());
}

#[test]
fn n6_a_markup_without_a_fingerprint_is_never_unedited() {
    let (mut doc, page) = one_page_doc();
    let c = revu_cloud(&mut doc, "CLOUDREVU0000001", 1);
    set_annots(&mut doc, page, vec![c.into()]);
    let mut m = read_markups(&doc).unwrap().remove(0);
    assert!(m.is_unedited());
    m.source_fp = None;
    assert!(!m.is_unedited());
}

// --- security: reference graphs in a preserved original ---------------------------------

fn annot_with(doc: &mut Document, extra: Vec<(&str, Object)>) -> ObjectId {
    let mut d = dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal("GRAPHREVU0000001"),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
    };
    for (k, v) in extra {
        d.set(k, v);
    }
    doc.add_object(d)
}

fn timed_read(doc: &Document) -> (Vec<Markup>, std::time::Duration) {
    let t = std::time::Instant::now();
    (read_markups(doc).unwrap(), t.elapsed())
}

#[test]
fn security_a_two_object_reference_cycle_is_cut_not_followed() {
    let (mut doc, page) = one_page_doc();
    let (a, b) = (doc.new_object_id(), doc.new_object_id());
    doc.objects.insert(a, Object::Dictionary(dictionary! { "Loop" => Object::Reference(b), "X" => 1 }));
    doc.objects.insert(b, Object::Dictionary(dictionary! { "Loop" => Object::Reference(a), "Y" => 2 }));
    let ann = annot_with(&mut doc, vec![("Cyc", Object::Reference(a))]);
    set_annots(&mut doc, page, vec![ann.into()]);
    let (ms, took) = timed_read(&doc);
    assert!(took.as_millis() < 500, "{took:?}");
    let src = ms[0].source_dict.as_ref().expect("carried with the cycle cut");
    let cyc = src.get(b"Cyc").unwrap().as_dict().unwrap();
    let inner = cyc.get(b"Loop").unwrap().as_dict().unwrap();
    assert_eq!(cyc.get(b"X").unwrap().as_i64().unwrap(), 1);
    assert_eq!(inner.get(b"Y").unwrap().as_i64().unwrap(), 2);
    assert!(!inner.has(b"Loop"), "the reference back to the first object is dropped");
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

#[test]
fn security_a_self_reference_is_dropped() {
    let (mut doc, page) = one_page_doc();
    let s = doc.new_object_id();
    doc.objects.insert(s, Object::Dictionary(dictionary! { "Me" => Object::Reference(s), "Z" => 3 }));
    let ann = annot_with(&mut doc, vec![("Selfish", Object::Reference(s)), ("Arr", Object::Array(vec![Object::Reference(s); 50]))]);
    set_annots(&mut doc, page, vec![ann.into()]);
    let (ms, took) = timed_read(&doc);
    assert!(took.as_millis() < 500, "{took:?}");
    let src = ms[0].source_dict.as_ref().unwrap();
    let me = src.get(b"Selfish").unwrap().as_dict().unwrap();
    assert!(!me.has(b"Me") && me.get(b"Z").unwrap().as_i64().unwrap() == 3);
}

#[test]
fn security_a_thirty_level_chain_hits_the_depth_limit_and_is_rebuilt() {
    let (mut doc, page) = one_page_doc();
    let mut next: Option<ObjectId> = None;
    for i in 0..30 {
        let id = doc.new_object_id();
        let mut d = dictionary! { "Level" => i };
        if let Some(n) = next {
            d.set("Down", Object::Reference(n));
        }
        doc.objects.insert(id, Object::Dictionary(d));
        next = Some(id);
    }
    let ann = annot_with(&mut doc, vec![("Chain", Object::Reference(next.unwrap()))]);
    set_annots(&mut doc, page, vec![ann.into()]);
    let (ms, took) = timed_read(&doc);
    assert!(took.as_millis() < 500, "{took:?}");
    assert!(ms[0].source_dict.is_none(), "too deep to carry: rebuilt from the model");
    for n in 1..=3 {
        cycle(&mut doc, |_| {});
        assert_eq!(annots(&doc).len(), 1, "cycle {n}");
    }
}

#[test]
fn security_fan_out_to_one_big_object_is_charged_per_expansion_and_dropped() {
    let (mut doc, page) = one_page_doc();
    let big = doc.new_object_id();
    doc.objects.insert(big, Object::String(vec![b'x'; 1_000_000], lopdf::StringFormat::Literal));
    let refs: Vec<Object> = (0..1_000).map(|_| Object::Reference(big)).collect();
    let ann = annot_with(
        &mut doc,
        vec![("Fan", Object::Array(refs)), ("Small", Object::Integer(7))],
    );
    set_annots(&mut doc, page, vec![ann.into()]);
    let (ms, took) = timed_read(&doc);
    assert!(took.as_secs() < 2, "{took:?}");
    let src = ms[0].source_dict.as_ref().expect("the rest is still carried");
    assert!(!src.has(b"Fan"), "over the inlining budget: the key is left out");
    assert_eq!(src.get(b"Small").unwrap().as_i64().unwrap(), 7);
    let total: usize = format!("{src:?}").len();
    assert!(total < 100_000, "bounded output, got {total} bytes of debug text");
    // Nested arrays of references share the same budget.
    let nested: Vec<Object> = (0..200)
        .map(|_| Object::Array((0..200).map(|_| Object::Reference(big)).collect()))
        .collect();
    let (mut doc2, page2) = one_page_doc();
    let big2 = doc2.new_object_id();
    doc2.objects.insert(big2, Object::String(vec![b'y'; 500_000], lopdf::StringFormat::Literal));
    let nested: Vec<Object> = nested
        .into_iter()
        .map(|_| Object::Array(vec![Object::Reference(big2); 200]))
        .collect();
    let ann2 = annot_with(&mut doc2, vec![("Nest2", Object::Array(nested))]);
    set_annots(&mut doc2, page2, vec![ann2.into()]);
    let (ms2, took2) = timed_read(&doc2);
    assert!(took2.as_secs() < 2, "{took2:?}");
    assert!(!ms2[0].source_dict.as_ref().unwrap().has(b"Nest2"));
}

// --- security: document-wide budget across many annotations ------------------------------

fn many_annots_sharing(doc: &mut Document, shared: ObjectId, annots_n: usize, refs_per_annot: usize) -> Vec<Object> {
    (0..annots_n)
        .map(|i| {
            let refs: Vec<Object> = (0..refs_per_annot).map(|_| Object::Reference(shared)).collect();
            let a = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Square", "F" => 4,
                "NM" => Object::string_literal(format!("SHARED{i:010}")),
                "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
                "Big" => if refs_per_annot == 1 { Object::Reference(shared) } else { Object::Array(refs) },
            });
            Object::Reference(a)
        })
        .collect()
}

fn size_of(doc: &mut Document) -> usize {
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf.len()
}

fn read_write_bounded(mut doc: Document, n_annots: usize, check_size: bool) {
    let before = if check_size { size_of(&mut doc) } else { 0 };
    let t = std::time::Instant::now();
    let ms = read_markups(&doc).unwrap();
    assert_eq!(ms.len(), n_annots);
    let read = t.elapsed();
    assert!(read.as_millis() < 1000, "read took {read:?}");
    if check_size {
        let t = std::time::Instant::now();
        write_markups(&mut doc, &ms).unwrap();
        let write = t.elapsed();
        assert!(write.as_millis() < 1000, "write took {write:?}");
        let after = size_of(&mut doc);
        assert!(after < before + 3_000_000, "file grew from {before} to {after}");
        assert_eq!(annots(&doc).len(), n_annots);
    }
}

#[test]
fn security_one_150k_array_referenced_by_500_annotations_is_bounded() {
    let (mut doc, page) = one_page_doc();
    let big = doc.add_object(Object::Array((0..150_000).map(Object::Integer).collect()));
    let items = many_annots_sharing(&mut doc, big, 500, 1);
    set_annots(&mut doc, page, items);
    read_write_bounded(doc, 500, true);
}

#[test]
fn security_a_1000_element_object_x_190_refs_x_500_annotations_is_bounded() {
    let (mut doc, page) = one_page_doc();
    let obj = doc.add_object(Object::Array((0..1_000).map(Object::Integer).collect()));
    let items = many_annots_sharing(&mut doc, obj, 500, 190);
    set_annots(&mut doc, page, items);
    read_write_bounded(doc, 500, true);
}

#[test]
fn security_a_5m_element_array_referenced_by_200_annotations_is_bounded() {
    let (mut doc, page) = one_page_doc();
    let big = doc.add_object(Object::Array(vec![Object::Integer(1); 5_000_000]));
    let items = many_annots_sharing(&mut doc, big, 200, 1);
    set_annots(&mut doc, page, items);
    read_write_bounded(doc, 200, false);
}

#[test]
fn security_small_shared_objects_still_preserve_across_annotations() {
    // A /Measure shared by several annotations is cheap and keeps being preserved.
    let (mut doc, page) = one_page_doc();
    let measure = doc.add_object(dictionary! { "Type" => "Measure", "Subtype" => "RL" });
    let items: Vec<Object> = (0..5)
        .map(|i| {
            let a = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Square", "F" => 4,
                "NM" => Object::string_literal(format!("SMALLSHARE{i:06}")),
                "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
                "Measure" => measure,
            });
            Object::Reference(a)
        })
        .collect();
    set_annots(&mut doc, page, items);
    let ms = read_markups(&doc).unwrap();
    assert!(ms.iter().all(|m| m.is_unedited() && m.source_dict.as_ref().unwrap().has(b"Measure")));
}
