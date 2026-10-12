//! Data-integrity tests for a Revu / Acrobat reviewed PDF edited and saved in redline:
//! deleting a foreign markup, reply keys, duplicate reply `/NM`, a foreign note's `/Rect`,
//! and `/QuadPoints` order. Every fixture uses Revu-style 16-char `/NM` values and runs
//! three load/save cycles, so a lossy or duplicating save shows up as a changed count.

use super::annotation::canonical_quad;
use super::*;
use crate::document::annots::tests::{one_page_doc, redline_markup};
use crate::document::annots::{
    page_annots, read_markups, write_markups, write_markups_removing,
};
use crate::document::store::MarkupStore;
use crate::geometry::{display_to_true, PdfPoint, Quad};
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeSet, HashSet};

// --- helpers --------------------------------------------------------------------

fn annots(doc: &Document) -> Vec<(Option<ObjectId>, Dictionary)> {
    let page = *doc.get_pages().values().next().unwrap();
    page_annots(doc, page).unwrap()
}

fn reload(doc: &mut Document) -> Document {
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    Document::load_mem(&buf).unwrap()
}

fn text(d: &Dictionary, key: &[u8]) -> Option<String> {
    d.get(key)
        .ok()?
        .as_str()
        .ok()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

fn subtype(d: &Dictionary) -> String {
    d.get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default()
}

fn count_of(doc: &Document, st: &str) -> usize {
    annots(doc).iter().filter(|(_, d)| subtype(d) == st).count()
}

fn nm_set(doc: &Document) -> BTreeSet<String> {
    annots(doc)
        .iter()
        .filter_map(|(_, d)| text(d, b"NM"))
        .collect()
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

fn set_annots(doc: &mut Document, page: ObjectId, ids: &[ObjectId]) {
    doc.get_dictionary_mut(page).unwrap().set(
        "Annots",
        Object::Array(ids.iter().map(|o| Object::Reference(*o)).collect()),
    );
}

fn square(doc: &mut Document, nm: &str, contents: &str) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "F" => 4,
        "NM" => Object::string_literal(nm),
        "Rect" => vec![10.into(), 10.into(), 60.into(), 40.into()],
        "Contents" => Object::string_literal(contents),
        "T" => Object::string_literal("Martin"),
    })
}

/// A reply / state `/Text` annotation answering `irt`.
fn note_on(
    doc: &mut Document,
    nm: &str,
    irt: ObjectId,
    contents: &str,
    extra: Vec<(&str, Object)>,
) -> ObjectId {
    let mut d = dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "NM" => Object::string_literal(nm),
        "Rect" => vec![0.into(), 0.into(), 5.into(), 5.into()],
        "T" => Object::string_literal("Architect"),
        "Contents" => Object::string_literal(contents),
        "IRT" => Object::Reference(irt),
        "RT" => Object::Name(b"R".to_vec()),
        "F" => 28,
    };
    for (k, v) in extra {
        d.set(k, v);
    }
    doc.add_object(d)
}

fn state_on(doc: &mut Document, nm: &str, irt: ObjectId, state: &str) -> ObjectId {
    note_on(
        doc,
        nm,
        irt,
        &format!("{state} set by Architect"),
        vec![
            ("StateModel", Object::string_literal("Review")),
            ("State", Object::string_literal(state)),
            ("F", 30.into()),
        ],
    )
}

fn new_store() -> MarkupStore {
    let store = MarkupStore::default();
    store.register("d", std::path::PathBuf::from("/tmp/x.pdf"), None);
    store
}

fn through_json(m: &Markup, patch: impl FnOnce(&mut serde_json::Value)) -> Markup {
    let mut v = serde_json::to_value(m).unwrap();
    patch(&mut v);
    serde_json::from_value(v).unwrap()
}

fn save_from_store(doc: &mut Document, store: &MarkupStore) {
    write_markups_removing(doc, &store.list("d").unwrap(), &store.removed_sources("d")).unwrap();
    *doc = reload(doc);
}

fn pt(x: f64, y: f64) -> PdfPoint {
    PdfPoint { x, y }
}

// --- 1. deleting a foreign markup -------------------------------------------------

/// Revu document: a square with a reply, a reply to the reply and a state, plus a circle.
fn revu_review() -> Document {
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, "RMCTWBEQGCYHWCHZ", "square");
    let circle = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Circle", "F" => 4,
        "NM" => Object::string_literal("ZZZZYYYYXXXXWWWW"),
        "Rect" => vec![100.into(), 10.into(), 150.into(), 40.into()],
        "Contents" => Object::string_literal("circle"),
    });
    let reply = note_on(&mut doc, "ABCDEFGHIJKLMNOP", sq, "a reply", vec![]);
    let nested = note_on(&mut doc, "NESTEDREPLY00001", reply, "nested", vec![]);
    let state = state_on(&mut doc, "QRSTUVWXYZABCDEF", sq, "Rejected");
    set_annots(&mut doc, page, &[sq, circle, reply, nested, state]);
    doc
}

fn load_into(store: &MarkupStore, doc: &Document) {
    store.seed_loaded("d", read_markups(doc).unwrap()).unwrap();
}

fn find(store: &MarkupStore, contents: &str) -> Markup {
    store
        .list("d")
        .unwrap()
        .into_iter()
        .find(|m| m.contents.as_deref() == Some(contents))
        .unwrap()
}

#[test]
fn deleting_a_revu_markup_removes_the_original_its_replies_and_states_on_save() {
    let mut doc = revu_review();
    let store = new_store();
    load_into(&store, &doc);
    store.delete("d", find(&store, "square").id()).unwrap();

    // The defect this test pins: saving the surviving set alone keeps the deleted
    // markup and everything answering it (square + 3 answers, plus the rewritten circle).
    let mut control = doc.clone();
    write_markups(&mut control, &store.list("d").unwrap()).unwrap();
    assert_eq!(annots(&control).len(), 5, "control: the original stays");

    for cycle in 1..=3 {
        if cycle > 1 {
            // A later session: fresh load of what was saved, nothing deleted.
            let store = new_store();
            load_into(&store, &doc);
            save_from_store(&mut doc, &store);
        } else {
            save_from_store(&mut doc, &store);
        }
        assert_eq!(annots(&doc).len(), 1, "cycle {cycle}: only the circle is left");
        assert_eq!(
            nm_set(&doc),
            BTreeSet::from(["ZZZZYYYYXXXXWWWW".to_string()]),
            "cycle {cycle}: the square, its replies and its state are gone"
        );
        assert_eq!(count_of(&doc, "Square"), 0);
    }
}

#[test]
fn undoing_the_delete_restores_the_markup_once_with_its_original_names() {
    let mut doc = revu_review();
    let nms0 = nm_set(&doc);
    let store = new_store();
    load_into(&store, &doc);
    let sq = find(&store, "square");
    store.delete("d", sq.id()).unwrap();
    assert!(!store.removed_sources("d").is_empty());
    store.add("d", through_json(&sq, |_| {})).unwrap();
    assert!(
        store.removed_sources("d").is_empty(),
        "an undone delete is not a deletion"
    );
    for cycle in 1..=3 {
        save_from_store(&mut doc, &store);
        assert_eq!(count_of(&doc, "Square"), 1, "cycle {cycle}");
        assert_eq!(annots(&doc).len(), 5, "cycle {cycle}");
        assert_eq!(nm_set(&doc), nms0, "cycle {cycle}: original /NMs kept");
    }
}

#[test]
fn deleting_an_acrobat_uuid_nm_markup_removes_it_and_its_reply_too() {
    for nm in [
        "3c6e1b0a-1d2f-4a9b-8c7d-0123456789ab",
        "3C6E1B0A-1D2F-4A9B-8C7D-0123456789AB", // uppercase spelling of a UUID
    ] {
        let (mut doc, page) = one_page_doc();
        let sq = square(&mut doc, nm, "acrobat square");
        let reply = note_on(&mut doc, "ACROBATREPLY0001", sq, "reply", vec![]);
        set_annots(&mut doc, page, &[sq, reply]);
        let store = new_store();
        load_into(&store, &doc);
        store.delete("d", find(&store, "acrobat square").id()).unwrap();
        for cycle in 1..=3 {
            save_from_store(&mut doc, &store);
            assert!(annots(&doc).is_empty(), "{nm} cycle {cycle}: {:?}", nm_set(&doc));
        }
    }
}

#[test]
fn removed_sources_only_ever_come_from_markups_the_store_held() {
    let (mut doc, page) = one_page_doc();
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "NM" => Object::string_literal("LINKNAMEFOREIGN01"),
        "Rect" => vec![0.into(), 0.into(), 100.into(), 20.into()],
    });
    set_annots(&mut doc, page, &[link]);
    let store = new_store();
    load_into(&store, &doc);

    // Deleting an id the store never held is refused and records nothing.
    assert!(store.delete("d", uuid::Uuid::new_v4()).is_err());
    assert!(store.removed_sources("d").is_empty());

    // A crafted markup claiming the Link's name: add clears the claim, so deleting it later
    // records its own id only.
    let crafted = through_json(&redline_markup(0), |v| {
        v["source_nm"] = "LINKNAMEFOREIGN01".into();
    });
    let id = crafted.id();
    store.add("d", crafted).unwrap();
    store.delete("d", id).unwrap();
    assert!(
        store.removed_sources("d").is_empty(),
        "the crafted markup has no source annotation, so nothing foreign is recorded"
    );
    save_from_store(&mut doc, &store);
    assert_eq!(count_of(&doc, "Link"), 1, "the Link survives");

    // A very large removal set is plain data: no panic, nothing else touched.
    let big: HashSet<SourceRef> = (0..100_000)
        .map(|i| SourceRef { oid: Some((i + 10_000, 0)), nm: Some(format!("NAME{i:012}")) })
        .collect();
    write_markups_removing(&mut doc, &[], &big).unwrap();
    assert_eq!(count_of(&doc, "Link"), 1);
}

#[test]
fn the_file_save_pipeline_applies_the_removed_sources() {
    let mut doc = revu_review();
    let dir = tempfile::tempdir().unwrap();
    let (src, dest) = (dir.path().join("in.pdf"), dir.path().join("out.pdf"));
    doc.save(&src).unwrap();
    let store = new_store();
    load_into(&store, &doc);
    store.delete("d", find(&store, "square").id()).unwrap();
    crate::document::save::save_with_markups_removing(
        &src,
        &dest,
        &store.list("d").unwrap(),
        &store.removed_sources("d"),
    )
    .unwrap();
    let saved = Document::load(&dest).unwrap();
    assert_eq!(
        nm_set(&saved),
        BTreeSet::from(["ZZZZYYYYXXXXWWWW".to_string()])
    );
}

// --- 2. reply keys survive a save -------------------------------------------------

const RC: &str = "<?xml version=\"1.0\"?><body><p>rich text</p></body>";

/// A Revu square with a rich reply (Subj, C, RC, Bluebeam keys, a pop-up) and a state, plus
/// hostile extras on the reply that must never be carried.
fn rich_reply_doc() -> Document {
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, "RMCTWBEQGCYHWCHZ", "square");
    let reply = note_on(
        &mut doc,
        "REPLYRICH0000001",
        sq,
        "plain text",
        vec![
            ("Subj", Object::string_literal("Sticky Note")),
            ("C", vec![Object::Real(1.0), Object::Real(0.5), Object::Real(0.0)].into()),
            ("RC", Object::string_literal(RC)),
            ("BSIColumnData", vec![Object::string_literal("Electrical")].into()),
            ("BBMeasure", dictionary! { "Scale" => Object::Real(0.5) }.into()),
            // never carried:
            ("A", dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert(1)") }.into()),
            ("AA", dictionary! { "E" => dictionary! { "S" => "JavaScript" } }.into()),
            ("Unlisted", Object::Integer(7)),
        ],
    );
    let popup = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Popup", "F" => 28, "Open" => true,
        "Rect" => vec![200.into(), 200.into(), 300.into(), 300.into()],
        "Parent" => Object::Reference(reply),
    });
    doc.get_dictionary_mut(reply)
        .unwrap()
        .set("Popup", Object::Reference(popup));
    let state = state_on(&mut doc, "STATERICH0000001", sq, "Accepted");
    doc.get_dictionary_mut(state)
        .unwrap()
        .set("Subj", Object::string_literal("Review"));
    set_annots(&mut doc, page, &[sq, reply, popup, state]);
    doc
}

fn reply_dict_of(doc: &Document, nm: &str) -> (ObjectId, Dictionary) {
    annots(doc)
        .into_iter()
        .find_map(|(o, d)| (text(&d, b"NM").as_deref() == Some(nm)).then(|| (o.unwrap(), d)))
        .unwrap()
}

#[test]
fn an_unedited_reply_keeps_subj_c_rc_popup_and_bluebeam_keys_over_three_cycles() {
    let mut doc = rich_reply_doc();
    for cycle in 1..=3 {
        let ms = read_markups(&doc).unwrap();
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);

        assert_eq!(annots(&doc).len(), 4, "cycle {cycle}: sq + reply + popup + state");
        assert_eq!(count_of(&doc, "Popup"), 1, "cycle {cycle}: one pop-up, not stacked");
        let (reply_oid, r) = reply_dict_of(&doc, "REPLYRICH0000001");
        assert_eq!(text(&r, b"Subj").as_deref(), Some("Sticky Note"));
        assert_eq!(floats(&r, b"C"), vec![1.0, 0.5, 0.0]);
        assert_eq!(text(&r, b"RC").as_deref(), Some(RC));
        assert_eq!(
            r.get(b"BSIColumnData").unwrap().as_array().unwrap().len(),
            1
        );
        assert!(r.has(b"BBMeasure"));
        for bad in [&b"A"[..], b"AA", b"Unlisted"] {
            assert!(!r.has(bad), "cycle {cycle}: /{} was carried", String::from_utf8_lossy(bad));
        }
        // The pop-up is rebuilt and parented to the rewritten reply.
        let popup_ref = r.get(b"Popup").unwrap().as_reference().unwrap();
        let (_, popup) = annots(&doc)
            .into_iter()
            .find(|(o, _)| *o == Some(popup_ref))
            .expect("/Popup points at a page annotation");
        assert_eq!(subtype(&popup), "Popup");
        assert_eq!(popup.get(b"Parent").unwrap().as_reference().unwrap(), reply_oid);
        assert_eq!(floats(&popup, b"Rect"), vec![200.0, 200.0, 300.0, 300.0]);
        assert!(matches!(popup.get(b"Open"), Ok(Object::Boolean(true))));
        // The state keeps its subject too.
        let (_, s) = reply_dict_of(&doc, "STATERICH0000001");
        assert_eq!(text(&s, b"Subj").as_deref(), Some("Review"));
        assert_eq!(nm_set(&doc).len(), 3, "cycle {cycle}: names unchanged");
    }
}

#[test]
fn an_edited_reply_keeps_subj_c_and_private_keys_but_drops_the_stale_rich_text() {
    let mut doc = rich_reply_doc();
    let mut ms = read_markups(&doc).unwrap();
    ms[0].workflow.thread[0].contents = "edited text".into();
    for cycle in 1..=3 {
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);
        let (_, r) = reply_dict_of(&doc, "REPLYRICH0000001");
        assert_eq!(text(&r, b"Contents").as_deref(), Some("edited text"));
        assert!(!r.has(b"RC"), "cycle {cycle}: a stale /RC would contradict /Contents");
        assert_eq!(text(&r, b"Subj").as_deref(), Some("Sticky Note"));
        assert_eq!(floats(&r, b"C"), vec![1.0, 0.5, 0.0]);
        assert!(r.has(b"BSIColumnData") && r.has(b"BBMeasure"));
        assert_eq!(count_of(&doc, "Popup"), 1);
        // Re-reading the saved file (now without /RC) and saving again changes nothing.
        ms = read_markups(&doc).unwrap();
        assert_eq!(ms[0].workflow.thread[0].contents, "edited text");
    }
}

#[test]
fn hostile_reply_keys_are_dropped_on_read_and_refused_on_write() {
    // Read side: an oversized /RC, a /Popup that is not a popup, a /C that is not data.
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, "RMCTWBEQGCYHWCHZ", "square");
    let not_a_popup = square(&mut doc, "NOTAPOPUP0000001", "decoy");
    let reply = note_on(
        &mut doc,
        "REPLYHOSTILE0001",
        sq,
        "r",
        vec![
            ("RC", Object::String(vec![b'x'; 70_000], lopdf::StringFormat::Literal)),
            ("Popup", Object::Reference(not_a_popup)),
            ("Subj", Object::Reference(sq)),
        ],
    );
    set_annots(&mut doc, page, &[sq, reply]);
    let ms = read_markups(&doc).unwrap();
    assert_eq!(ms.len(), 1, "the decoy was never in the page list");
    assert!(
        ms[0].workflow.thread[0].extras.is_none(),
        "{:?}",
        ms[0].workflow.thread[0].extras
    );
    write_markups(&mut doc, &ms).unwrap();
    let (_, r) = reply_dict_of(&doc, "REPLYHOSTILE0001");
    assert!(!r.has(b"RC") && !r.has(b"Popup") && !r.has(b"Subj"));

    // Write side: extras that arrive as data (JSON) are validated again; none of it is emitted.
    let mut m = redline_markup(0);
    let mut reply = Reply {
        id: uuid::Uuid::new_v4(),
        author: UserRef { user_id: uuid::Uuid::new_v4(), display_name: "A".into() },
        at: chrono::Utc::now(),
        contents: "x".into(),
        in_reply_to: None,
        nm: None,
        extras: None,
    };
    let key = |name: &str, value: foreign::PdfValue| foreign::ForeignKey { name: name.into(), value };
    reply.extras = Some(SourceExtras {
        keys: vec![
            key("A", foreign::PdfValue::Dict(vec![("S".into(), foreign::PdfValue::Name("JavaScript".into()))])),
            key("JS", foreign::PdfValue::Str(b"app.alert(1)".to_vec(), false)),
            key("Parent", foreign::PdfValue::Int(1)),
            key("Subj", foreign::PdfValue::Dict(vec![("JS".into(), foreign::PdfValue::Int(1))])),
            key("C", foreign::PdfValue::Real(1e300)),
            key("Evil)>>/A<<", foreign::PdfValue::Int(1)),
        ],
        popup: Some(PopupProps { rect: [f32::NAN, 0.0, 1.0, 1.0], open: true, flags: None }),
        contents_at_read: "x".into(),
    });
    m.workflow.thread = vec![reply];
    let (mut doc2, _) = one_page_doc();
    write_markups(&mut doc2, std::slice::from_ref(&m)).unwrap();
    for (_, d) in annots(&doc2) {
        for bad in [&b"A"[..], b"JS", b"Parent", b"Subj", b"C", b"Popup"] {
            if d.has(b"IRT") {
                assert!(!d.has(bad), "/{} reached the file", String::from_utf8_lossy(bad));
            }
        }
    }
    assert_eq!(count_of(&doc2, "Popup"), 0, "a non-finite pop-up rectangle is not written");

    // The store never accepts extras from outside.
    let mut finite = m.clone();
    if let Some(x) = finite.workflow.thread[0].extras.as_mut() {
        x.popup = Some(PopupProps { rect: [0.0, 0.0, 1.0, 1.0], open: true, flags: None });
    }
    let store = new_store();
    store.add("d", through_json(&finite, |_| {})).unwrap();
    assert!(store.list("d").unwrap()[0].workflow.thread[0].extras.is_none());
}

// --- 3. duplicate reply /NM -------------------------------------------------------

/// Revu square with three replies and a state that all share one `/NM`.
fn duplicate_nm_doc(reversed: bool) -> Document {
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, "RMCTWBEQGCYHWCHZ", "square");
    const NM: &str = "DUPLICATEREPLY01";
    let date = |m: &str| vec![("M", Object::string_literal(format!("D:20261001{m}00Z")))];
    let r1 = note_on(&mut doc, NM, sq, "first", date("0910"));
    let r2 = note_on(&mut doc, NM, r1, "second", date("0920"));
    let r3 = note_on(&mut doc, NM, sq, "third", date("0930"));
    let st = state_on(&mut doc, NM, sq, "Accepted");
    doc.get_dictionary_mut(st)
        .unwrap()
        .set("M", Object::string_literal("D:20261001094000Z"));
    let mut ids = vec![sq, r1, r2, r3, st];
    if reversed {
        ids.reverse();
    }
    set_annots(&mut doc, page, &ids);
    doc
}

fn thread_summary(m: &Markup) -> Vec<(String, uuid::Uuid, Option<uuid::Uuid>)> {
    let mut v: Vec<_> = m
        .workflow
        .thread
        .iter()
        .map(|r| (r.contents.clone(), r.id, r.in_reply_to))
        .collect();
    v.sort();
    v
}

#[test]
fn replies_and_states_sharing_one_nm_keep_distinct_stable_identities() {
    let mut first: Option<Vec<(String, uuid::Uuid, Option<uuid::Uuid>)>> = None;
    for reversed in [false, true] {
        let mut doc = duplicate_nm_doc(reversed);
        for cycle in 1..=3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 1);
            let m = &ms[0];
            let mut ids: HashSet<uuid::Uuid> = m.workflow.thread.iter().map(|r| r.id).collect();
            ids.extend(m.workflow.status_history.iter().map(|c| c.id));
            assert_eq!(ids.len(), 4, "reversed={reversed} cycle {cycle}: ids are distinct");
            assert_eq!(m.workflow.thread.len(), 3);
            assert_eq!(m.workflow.status_history.len(), 1);
            let summary = thread_summary(m);
            let by_text = |t: &str| summary.iter().find(|(c, ..)| c == t).unwrap();
            assert_eq!(by_text("second").2, Some(by_text("first").1), "nesting kept");
            assert_eq!(by_text("first").2, None);
            // Same identities on every load and whatever order the file lists the notes in.
            match &first {
                None => first = Some(summary),
                Some(f) => assert_eq!(&summary, f, "reversed={reversed} cycle {cycle}"),
            }

            write_markups(&mut doc, &ms).unwrap();
            doc = reload(&mut doc);
            assert_eq!(annots(&doc).len(), 5, "reversed={reversed} cycle {cycle}: nothing lost or doubled");
            let dup_nm = annots(&doc)
                .iter()
                .filter(|(_, d)| text(d, b"NM").as_deref() == Some("DUPLICATEREPLY01"))
                .count();
            assert_eq!(dup_nm, 4, "the source /NM values are written back as they were");
        }
    }
}

#[test]
fn thousands_of_replies_sharing_one_nm_resolve_in_bounded_time_without_loss() {
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, "RMCTWBEQGCYHWCHZ", "square");
    let mut ids = vec![sq];
    for _ in 0..2_000 {
        // identical in every field: the worst case for telling them apart
        ids.push(note_on(&mut doc, "SAMENAMEEVERYWHE", sq, "same", vec![]));
    }
    set_annots(&mut doc, page, &ids);
    for cycle in 1..=3 {
        let ms = read_markups(&doc).unwrap();
        let unique: HashSet<_> = ms[0].workflow.thread.iter().map(|r| r.id).collect();
        assert_eq!(unique.len(), 2_000, "cycle {cycle}");
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);
        assert_eq!(annots(&doc).len(), 2_001, "cycle {cycle}");
    }
}

// --- 4. foreign sticky note /Rect -------------------------------------------------

fn foreign_note(doc: &mut Document, rect: [i64; 4]) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "F" => 28, "Name" => "Comment",
        "NM" => Object::string_literal("NOTERECT00000001"),
        "Rect" => rect.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>(),
        "Contents" => Object::string_literal("a sticky note"),
        "C" => vec![Object::Real(1.0), Object::Real(1.0), Object::Real(0.0)],
    })
}

fn note_rect(doc: &Document) -> Vec<f32> {
    let (_, d) = annots(doc).into_iter().find(|(_, d)| subtype(d) == "Text").unwrap();
    floats(&d, b"Rect")
}

fn ap_bbox_of(doc: &Document, d: &Dictionary) -> Vec<f32> {
    let ap = d.get(b"AP").unwrap().as_dict().unwrap();
    let n = ap.get(b"N").unwrap().as_reference().unwrap();
    floats(&doc.get_object(n).unwrap().as_stream().unwrap().dict, b"BBox")
}

#[test]
fn an_unmoved_foreign_sticky_note_keeps_its_rect_over_three_cycles() {
    for rotate in [0, 90, 270] {
        let (mut doc, page) = one_page_doc();
        if rotate != 0 {
            doc.get_dictionary_mut(page).unwrap().set("Rotate", rotate as i64);
        }
        let n = foreign_note(&mut doc, [100, 100, 140, 150]); // 40 x 50, not the 20 x 20 icon box
        set_annots(&mut doc, page, &[n]);
        for cycle in 1..=3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 1);
            assert_eq!(ms[0].markup_type, MarkupType::Note);
            write_markups(&mut doc, &ms).unwrap();
            doc = reload(&mut doc);
            assert_eq!(annots(&doc).len(), 1, "rotate {rotate} cycle {cycle}");
            let r = note_rect(&doc);
            let want = [100.0, 100.0, 140.0, 150.0];
            assert!(
                r.iter().zip(want).all(|(a, b)| (f64::from(*a) - b).abs() < 1e-3),
                "rotate {rotate} cycle {cycle}: /Rect {r:?}"
            );
            let (_, d) = annots(&doc).into_iter().next().unwrap();
            assert_eq!(ap_bbox_of(&doc, &d), r, "/AP /BBox equals /Rect");
        }
    }
}

#[test]
fn a_moved_foreign_sticky_note_gets_a_normal_icon_box_at_its_new_place() {
    let (mut doc, page) = one_page_doc();
    let n = foreign_note(&mut doc, [100, 100, 140, 150]);
    set_annots(&mut doc, page, &[n]);
    let mut ms = read_markups(&doc).unwrap();
    ms[0].geometry = MarkupGeometry::Point(pt(300.0, 400.0));
    write_markups(&mut doc, &ms).unwrap();
    assert_eq!(note_rect(&doc), vec![290.0, 390.0, 310.0, 410.0]);
}

#[test]
fn a_small_foreign_note_box_is_kept_and_the_icon_shrinks_to_fit_it() {
    let (mut doc, page) = one_page_doc();
    let n = foreign_note(&mut doc, [100, 100, 110, 110]);
    set_annots(&mut doc, page, &[n]);
    let ms = read_markups(&doc).unwrap();
    write_markups(&mut doc, &ms).unwrap();
    let (_, d) = annots(&doc).into_iter().next().unwrap();
    assert_eq!(floats(&d, b"Rect"), vec![100.0, 100.0, 110.0, 110.0]);
    let ap = d.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
    let stream = doc.get_object(ap).unwrap().as_stream().unwrap();
    let content = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
    let content = String::from_utf8_lossy(&content).into_owned();
    assert!(content.contains("0.5 0 0 0.5"), "icon scaled to half: {content}");
}

#[test]
fn a_hostile_source_rect_is_ignored() {
    let base = |rect: Option<[f64; 4]>| {
        let mut m = Markup::new(
            MarkupType::Note,
            0,
            MarkupGeometry::Point(pt(300.0, 400.0)),
            Appearance::default(),
            UserRef { user_id: uuid::Uuid::new_v4(), display_name: "A".into() },
        );
        m.source_rect = rect;
        m
    };
    let icon = vec![290.0, 390.0, 310.0, 410.0];
    for (label, rect) in [
        ("enormous", [0.0, 0.0, 1.0e9, 1.0e9]),
        ("non-finite", [f64::NAN, 0.0, 1.0, 1.0]),
        ("infinite", [0.0, 0.0, f64::INFINITY, 1.0]),
        ("empty", [300.0, 400.0, 300.0, 400.0]),
        ("elsewhere", [0.0, 0.0, 10.0, 10.0]), // not centred on the note: it was moved
    ] {
        let (mut doc, _) = one_page_doc();
        write_markups(&mut doc, &[base(Some(rect))]).unwrap();
        assert_eq!(note_rect(&doc), icon, "{label}");
    }
    // An inverted rectangle centred on the note is normalised, not rejected.
    let (mut doc, _) = one_page_doc();
    write_markups(&mut doc, &[base(Some([320.0, 430.0, 280.0, 370.0]))]).unwrap();
    assert_eq!(note_rect(&doc), vec![280.0, 370.0, 320.0, 430.0]);

    // Data from outside never sets it: the store clears it on add.
    let store = new_store();
    let crafted = through_json(&base(Some([0.0, 0.0, 600.0, 800.0])), |_| {});
    assert!(crafted.source_rect.is_some(), "precondition: JSON carries it");
    store.add("d", crafted).unwrap();
    assert!(store.list("d").unwrap()[0].source_rect.is_none());
}

// --- 5. /QuadPoints order ---------------------------------------------------------

const TL: PdfPoint = PdfPoint { x: 10.0, y: 20.0 };
const TR: PdfPoint = PdfPoint { x: 110.0, y: 20.0 };
const BL: PdfPoint = PdfPoint { x: 10.0, y: 10.0 };
const BR: PdfPoint = PdfPoint { x: 110.0, y: 10.0 };
const Z_ORDER: Quad = [TL, TR, BL, BR];

#[test]
fn acrobat_z_order_is_left_alone_and_polygon_orders_are_reordered_from_any_corner() {
    assert_eq!(canonical_quad(&Z_ORDER), Z_ORDER, "Acrobat / Revu order unchanged");
    // ISO counter-clockwise (BL, BR, TR, TL) started at each corner.
    let ccw = [BL, BR, TR, TL];
    // Clockwise (TL, TR, BR, BL) started at each corner.
    let cw = [TL, TR, BR, BL];
    for (label, poly) in [("ccw", ccw), ("cw", cw)] {
        for start in 0..4 {
            let q: Quad = std::array::from_fn(|i| poly[(start + i) % 4]);
            assert_eq!(canonical_quad(&q), Z_ORDER, "{label} starting at corner {start}");
        }
    }
    // A skewed (italic) line: Z order stays, polygon order is recovered.
    let (stl, str_, sbl, sbr) = (pt(14.0, 20.0), pt(114.0, 20.0), pt(10.0, 10.0), pt(110.0, 10.0));
    assert_eq!(canonical_quad(&[stl, str_, sbl, sbr]), [stl, str_, sbl, sbr]);
    assert_eq!(canonical_quad(&[sbl, sbr, str_, stl]), [stl, str_, sbl, sbr]);
}

#[test]
fn degenerate_and_hostile_quads_are_returned_unchanged_and_never_panic() {
    let flat: Quad = [pt(0.0, 0.0), pt(10.0, 0.0), pt(0.0, 0.0), pt(10.0, 0.0)];
    assert_eq!(canonical_quad(&flat), flat);
    let point: Quad = [pt(5.0, 5.0); 4];
    assert_eq!(canonical_quad(&point), point);
    let huge: Quad = [pt(f64::MAX, f64::MAX), pt(-f64::MAX, f64::MAX), pt(-f64::MAX, -f64::MAX), pt(f64::MAX, -f64::MAX)];
    let _ = canonical_quad(&huge);
    let nan: Quad = [pt(f64::NAN, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0)];
    let out = canonical_quad(&nan);
    assert!(out[0].x.is_nan() && out[1] == nan[1], "non-finite input is returned as is");
}

fn quad_annot(doc: &mut Document, st: &str, nm: &str, pts: &[PdfPoint; 4]) -> ObjectId {
    let flat: Vec<Object> = pts
        .iter()
        .flat_map(|p| [Object::Real(p.x as f32), Object::Real(p.y as f32)])
        .collect();
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => st.to_string(), "F" => 4,
        "NM" => Object::string_literal(nm),
        "Rect" => vec![10.into(), 10.into(), 110.into(), 20.into()],
        "QuadPoints" => flat,
        "C" => vec![Object::Real(1.0), Object::Real(0.0), Object::Real(0.0)],
    })
}

fn quads_of(m: &Markup) -> Vec<Quad> {
    match &m.geometry {
        MarkupGeometry::Quads(q) => q.clone(),
        other => panic!("expected quads, got {other:?}"),
    }
}

fn close(a: &Quad, b: &Quad) -> bool {
    a.iter().zip(b).all(|(p, q)| (p.x - q.x).abs() < 1e-3 && (p.y - q.y).abs() < 1e-3)
}

#[test]
fn text_markups_in_either_quadpoints_order_load_and_save_to_the_same_geometry() {
    for st in ["Underline", "StrikeOut", "Squiggly", "Highlight"] {
        for (label, order) in [
            ("z", [TL, TR, BL, BR]),
            ("iso-ccw", [BL, BR, TR, TL]),
            ("cw", [TL, TR, BR, BL]),
        ] {
            let (mut doc, page) = one_page_doc();
            let a = quad_annot(&mut doc, st, "QUADORDER0000001", &order);
            set_annots(&mut doc, page, &[a]);
            for cycle in 1..=3 {
                let ms = read_markups(&doc).unwrap();
                assert_eq!(ms.len(), 1, "{st} {label} cycle {cycle}");
                let q = quads_of(&ms[0]);
                assert!(close(&q[0], &Z_ORDER), "{st} {label} cycle {cycle}: {q:?}");
                // The drawn line is the bottom edge, whichever order the file used.
                let line = crate::markup::appearance::text_markup_polyline(
                    MarkupType::Underline,
                    &q[0],
                    1.0,
                    8,
                );
                assert_eq!(line, vec![BL, BR]);
                write_markups(&mut doc, &ms).unwrap();
                doc = reload(&mut doc);
                assert_eq!(annots(&doc).len(), 1, "{st} {label} cycle {cycle}");
                let (_, d) = annots(&doc).into_iter().next().unwrap();
                let written = floats(&d, b"QuadPoints");
                let z: Vec<f32> = Z_ORDER.iter().flat_map(|p| [p.x as f32, p.y as f32]).collect();
                assert_eq!(written, z, "{st} {label} cycle {cycle}: written in Acrobat order");
            }
        }
    }
}

#[test]
fn quadpoints_order_is_resolved_in_the_displayed_frame_on_rotated_pages() {
    for rotation in [90, 180, 270] {
        // The line as the reader sees it (upright), mapped back to the file's page space.
        let to_true = |p: PdfPoint| display_to_true(p, rotation, 612.0, 792.0, 0.0, 0.0);
        let (mut doc, page) = one_page_doc();
        doc.get_dictionary_mut(page).unwrap().set("Rotate", rotation as i64);
        let file_iso: [PdfPoint; 4] = [to_true(BL), to_true(BR), to_true(TR), to_true(TL)];
        let file_z: [PdfPoint; 4] = [to_true(TL), to_true(TR), to_true(BL), to_true(BR)];
        let a = quad_annot(&mut doc, "Underline", "QUADROTATED00001", &file_iso);
        let b = quad_annot(&mut doc, "Underline", "QUADROTATED00002", &file_z);
        set_annots(&mut doc, page, &[a, b]);
        for cycle in 1..=3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 2);
            for m in &ms {
                let q = quads_of(m);
                assert!(close(&q[0], &Z_ORDER), "rotation {rotation} cycle {cycle}: {q:?}");
            }
            write_markups(&mut doc, &ms).unwrap();
            doc = reload(&mut doc);
            assert_eq!(annots(&doc).len(), 2, "rotation {rotation} cycle {cycle}");
        }
    }
}

// --- 6. object identity: a shared /NM never lets a save touch a non-imported annotation ----

const SHARED: &str = "SHAREDSHAREDSHAR";

fn popup_for(doc: &mut Document, parent: ObjectId, rect: [i64; 4]) -> ObjectId {
    let popup = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Popup", "F" => 28, "Open" => true,
        "Rect" => rect.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>(),
        "Parent" => Object::Reference(parent),
    });
    doc.get_dictionary_mut(parent)
        .unwrap()
        .set("Popup", Object::Reference(popup));
    popup
}

fn file_attachment(doc: &mut Document, nm: &str) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FileAttachment", "F" => 4,
        "NM" => Object::string_literal(nm),
        "Rect" => vec![200.into(), 10.into(), 220.into(), 30.into()],
        "Contents" => Object::string_literal("attachment"),
    })
}

/// A Revu square (own pop-up, reply, state) whose /NM is shared with three annotations
/// redline never imports: a FileAttachment (own pop-up) named differently, a reply to that
/// FileAttachment and a second FileAttachment, both named like the square.
fn shared_nm_doc() -> Document {
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, SHARED, "square");
    let sq_popup = popup_for(&mut doc, sq, [300, 300, 400, 400]);
    let sq_reply = note_on(&mut doc, "SQREPLY00000001", sq, "square reply", vec![]);
    let sq_state = state_on(&mut doc, "SQSTATE000000001", sq, "Accepted");
    let fa = file_attachment(&mut doc, "FILEATTACH000001");
    let fa_popup = popup_for(&mut doc, fa, [300, 100, 400, 200]);
    let fa_reply = note_on(&mut doc, SHARED, fa, "attachment reply", vec![]);
    let fa2 = file_attachment(&mut doc, SHARED);
    set_annots(
        &mut doc,
        page,
        &[sq, sq_popup, sq_reply, sq_state, fa, fa_popup, fa_reply, fa2],
    );
    doc
}

/// No dangling `/Popup` or `/Parent`, and no pop-up whose owner does not point back at it.
fn assert_popups_consistent(doc: &Document, what: &str) {
    let all = annots(doc);
    let by_oid: std::collections::HashMap<ObjectId, &Dictionary> =
        all.iter().filter_map(|(o, d)| o.map(|o| (o, d))).collect();
    for (oid, d) in &all {
        if let Ok(Object::Reference(p)) = d.get(b"Popup") {
            let pd = by_oid.get(p).unwrap_or_else(|| panic!("{what}: dangling /Popup"));
            assert_eq!(subtype(pd), "Popup", "{what}");
            assert_eq!(pd.get(b"Parent").unwrap().as_reference().unwrap(), oid.unwrap(), "{what}");
        }
        if subtype(d) == "Popup" {
            let parent = d.get(b"Parent").unwrap().as_reference().unwrap();
            let owner = by_oid
                .get(&parent)
                .unwrap_or_else(|| panic!("{what}: orphaned pop-up"));
            assert_eq!(
                owner.get(b"Popup").unwrap().as_reference().unwrap(),
                oid.unwrap(),
                "{what}: the owner does not point at its pop-up"
            );
        }
    }
}

/// The kept, non-imported annotations survive untouched: both FileAttachments, the reply to
/// the first still answering it, and the first's pop-up still parented to it.
fn assert_non_imported_intact(doc: &Document, what: &str) {
    assert_eq!(count_of(doc, "FileAttachment"), 2, "{what}: FileAttachments");
    let all = annots(doc);
    let (fa_oid, fa) = all
        .iter()
        .find(|(_, d)| text(d, b"NM").as_deref() == Some("FILEATTACH000001"))
        .map(|(o, d)| (o.unwrap(), d))
        .unwrap();
    assert_eq!(subtype(fa), "FileAttachment");
    assert!(
        all.iter().any(|(_, d)| subtype(d) == "Text"
            && text(d, b"Contents").as_deref() == Some("attachment reply")
            && text(d, b"NM").as_deref() == Some(SHARED)
            && d.get(b"IRT").ok().and_then(|o| o.as_reference().ok()) == Some(fa_oid)),
        "{what}: the reply to the FileAttachment"
    );
    let popup_ref = fa.get(b"Popup").unwrap().as_reference().unwrap();
    assert!(
        all.iter().any(|(o, d)| *o == Some(popup_ref)
            && d.get(b"Parent").unwrap().as_reference().unwrap() == fa_oid),
        "{what}: the FileAttachment's pop-up"
    );
}

#[test]
fn a_non_imported_annotation_sharing_an_imported_nm_survives_three_save_cycles() {
    // A later session each time: fresh load of what was saved.
    let mut doc = shared_nm_doc();
    assert_eq!(annots(&doc).len(), 8);
    for cycle in 1..=3 {
        let store = new_store();
        load_into(&store, &doc);
        assert_eq!(store.list("d").unwrap().len(), 1, "only the square is imported");
        save_from_store(&mut doc, &store);
        assert_eq!(annots(&doc).len(), 8, "cycle {cycle}");
        assert_eq!(count_of(&doc, "Square"), 1, "cycle {cycle}");
        assert_non_imported_intact(&doc, &format!("cycle {cycle}"));
        assert_popups_consistent(&doc, &format!("cycle {cycle}"));
    }

    // One session saving three times: the store keeps the object id of the original, which
    // the first save replaced.
    let mut doc = shared_nm_doc();
    let store = new_store();
    load_into(&store, &doc);
    for cycle in 1..=3 {
        save_from_store(&mut doc, &store);
        assert_eq!(annots(&doc).len(), 8, "in-session cycle {cycle}");
        assert_eq!(count_of(&doc, "Square"), 1, "in-session cycle {cycle}");
        assert_non_imported_intact(&doc, &format!("in-session cycle {cycle}"));
        assert_popups_consistent(&doc, &format!("in-session cycle {cycle}"));
    }
}

#[test]
fn deleting_an_imported_markup_removes_only_it_and_its_own_replies_states_and_popup() {
    for in_session_resave in [false, true] {
        let mut doc = shared_nm_doc();
        let store = new_store();
        load_into(&store, &doc);
        if in_session_resave {
            // The delete comes after a save that replaced the original.
            save_from_store(&mut doc, &store);
        }
        store.delete("d", find(&store, "square").id()).unwrap();
        for cycle in 1..=3 {
            save_from_store(&mut doc, &store);
            let what = format!("resave={in_session_resave} cycle {cycle}");
            assert_eq!(count_of(&doc, "Square"), 0, "{what}");
            // Left: FileAttachment x2, its reply, its pop-up. Gone: square, its reply, its
            // state, its pop-up.
            assert_eq!(annots(&doc).len(), 4, "{what}: {:?}", nm_set(&doc));
            assert_non_imported_intact(&doc, &what);
            assert_popups_consistent(&doc, &what);
            assert!(!nm_set(&doc).contains("SQREPLY00000001"), "{what}");
        }
    }
    // A fresh session after the delete is a stable no-op.
    let mut doc = shared_nm_doc();
    let store = new_store();
    load_into(&store, &doc);
    store.delete("d", find(&store, "square").id()).unwrap();
    save_from_store(&mut doc, &store);
    for cycle in 1..=2 {
        let store = new_store();
        load_into(&store, &doc);
        assert!(store.list("d").unwrap().is_empty());
        save_from_store(&mut doc, &store);
        assert_eq!(annots(&doc).len(), 4, "reopen cycle {cycle}");
        assert_non_imported_intact(&doc, &format!("reopen cycle {cycle}"));
    }
}

#[test]
fn undoing_the_delete_of_a_shared_nm_markup_restores_exactly_one_copy() {
    let mut doc = shared_nm_doc();
    let store = new_store();
    load_into(&store, &doc);
    let sq = find(&store, "square");
    store.delete("d", sq.id()).unwrap();
    save_from_store(&mut doc, &store);
    store.add("d", through_json(&sq, |_| {})).unwrap();
    for cycle in 1..=3 {
        save_from_store(&mut doc, &store);
        assert_eq!(count_of(&doc, "Square"), 1, "cycle {cycle}");
        assert_eq!(annots(&doc).len(), 8, "cycle {cycle}");
        assert_non_imported_intact(&doc, &format!("cycle {cycle}"));
        assert_popups_consistent(&doc, &format!("cycle {cycle}"));
    }
}

#[test]
fn an_annotation_is_only_replaced_by_the_object_it_was_imported_from() {
    // Same /NM and subtype as the imported square, but not the object the store read: kept.
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, SHARED, "imported");
    let twin = square(&mut doc, SHARED, "not imported");
    set_annots(&mut doc, page, &[sq]);
    let store = new_store();
    load_into(&store, &doc);
    set_annots(&mut doc, page, &[sq, twin]); // appears after the load (another viewer)
    for cycle in 1..=3 {
        save_from_store(&mut doc, &store);
        let contents: Vec<_> = annots(&doc)
            .iter()
            .filter_map(|(_, d)| text(d, b"Contents"))
            .collect();
        assert!(contents.contains(&"not imported".to_string()), "cycle {cycle}: {contents:?}");
    }
}

#[test]
fn a_foreign_markups_own_popup_is_kept_over_three_cycles() {
    let (mut doc, page) = one_page_doc();
    let sq = square(&mut doc, "RMCTWBEQGCYHWCHZ", "square");
    let popup = popup_for(&mut doc, sq, [300, 300, 400, 400]);
    set_annots(&mut doc, page, &[sq, popup]);
    for cycle in 1..=3 {
        let ms = read_markups(&doc).unwrap();
        assert!(ms[0].source_popup.is_some(), "cycle {cycle}");
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);
        assert_eq!(count_of(&doc, "Popup"), 1, "cycle {cycle}: one pop-up, not 0 or 2");
        assert_eq!(annots(&doc).len(), 2, "cycle {cycle}");
        assert_popups_consistent(&doc, &format!("cycle {cycle}"));
        let (_, p) = annots(&doc).into_iter().find(|(_, d)| subtype(d) == "Popup").unwrap();
        assert_eq!(floats(&p, b"Rect"), vec![300.0, 300.0, 400.0, 400.0]);
        assert!(matches!(p.get(b"Open"), Ok(Object::Boolean(true))));
    }
    // Deleting the markup takes its pop-up with it.
    let store = new_store();
    load_into(&store, &doc);
    store.delete("d", find(&store, "square").id()).unwrap();
    save_from_store(&mut doc, &store);
    assert!(annots(&doc).is_empty());
}

#[test]
fn source_ref_and_popup_never_come_from_input() {
    let mut doc = shared_nm_doc();
    let store = new_store();
    load_into(&store, &doc);
    let sq = find(&store, "square");
    assert!(sq.source_ref.is_some() && sq.source_popup.is_some());
    let json = serde_json::to_value(&sq).unwrap();
    assert!(json.get("source_ref").is_none() && json.get("source_popup").is_none());

    // An IPC update keeps the stored refs; an IPC add of a new markup gets none.
    let edited = through_json(&sq, |v| v["contents"] = "edited".into());
    assert!(edited.source_ref.is_none());
    store.update("d", edited).unwrap();
    assert_eq!(find(&store, "edited").source_ref, sq.source_ref);
    let fresh = redline_markup(0);
    let fresh_id = fresh.id();
    store.add("d", fresh).unwrap();
    let got = store.list("d").unwrap().into_iter().find(|m| m.id() == fresh_id).unwrap();
    assert!(got.source_ref.is_none() && got.source_popup.is_none());
    save_from_store(&mut doc, &store);
    assert_non_imported_intact(&doc, "after update");
}

// --- 6. count groups and polylength (docs/count-polylength.md) --------------------

fn count_item(group: Option<uuid::Uuid>, x: f64, set: &CountSet) -> Markup {
    let mut m = Markup::new(
        MarkupType::MeasurementCount,
        0,
        MarkupGeometry::Point(pt(x, 30.0)),
        Appearance::default(),
        UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "Alice".into(),
        },
    );
    m.count_set = Some(set.clone());
    m.count_group = group;
    m.subject = Some("Type A downlight".into());
    m.measurement = Some(Measurement {
        scale_ref: None,
        raw_measure: 1.0,
        unit: "ea".into(),
        computed_quantity: 1.0,
        depth: None,
        count_value: Some(1),
        custom_columns: Default::default(),
        rise_drop: Vec::new(),
    });
    m
}

fn a_set() -> CountSet {
    CountSet {
        id: uuid::Uuid::new_v4(),
        name: "Type A".into(),
        color: "#e02424".into(),
        symbol: CountSymbol::Circle,
    }
}

#[test]
fn count_groups_survive_three_cycles_with_stable_identity() {
    let (mut doc, _page) = one_page_doc();
    let set = a_set();
    let g1 = uuid::Uuid::new_v4();
    let g2 = uuid::Uuid::new_v4();
    let items = vec![
        count_item(Some(g1), 10.0, &set),
        count_item(Some(g1), 20.0, &set),
        count_item(Some(g2), 30.0, &set),
        count_item(None, 40.0, &set),
    ];
    let ids: BTreeSet<String> = items.iter().map(|m| m.id().to_string()).collect();
    write_markups(&mut doc, &items).unwrap();
    doc = reload(&mut doc);
    for cycle in 1..=3 {
        let ms = read_markups(&doc).unwrap();
        assert_eq!(ms.len(), 4, "cycle {cycle}");
        let in_g1 = ms.iter().filter(|m| m.count_group == Some(g1)).count();
        let in_g2 = ms.iter().filter(|m| m.count_group == Some(g2)).count();
        let none = ms.iter().filter(|m| m.count_group.is_none()).count();
        assert_eq!(
            (in_g1, in_g2, none),
            (2, 1, 1),
            "cycle {cycle}: grouping preserved"
        );
        let got: BTreeSet<String> = ms.iter().map(|m| m.id().to_string()).collect();
        assert_eq!(got, ids, "cycle {cycle}: ids stable");
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);
        assert_eq!(
            count_of(&doc, "Stamp"),
            4,
            "cycle {cycle}: items stay Stamps"
        );
        assert_eq!(annots(&doc).len(), 4, "cycle {cycle}: no duplication");
    }
}

#[test]
fn a_hostile_count_group_value_is_ignored_not_fatal() {
    let set = a_set();
    let mut d = count_item(Some(uuid::Uuid::new_v4()), 5.0, &set).to_annotation_dict();
    d.set("RLCountGroup", Object::string_literal("not-a-uuid"));
    let back = Markup::from_annotation_dict(&d);
    assert!(back.count_group.is_none());
    d.set("RLCountGroup", Object::string_literal("x".repeat(100_000)));
    assert!(Markup::from_annotation_dict(&d).count_group.is_none());
}

fn polylength(n: usize, rise: Vec<RiseDrop>) -> Markup {
    let pts: Vec<PdfPoint> = (0..n)
        .map(|i| pt(10.0 * i as f64, (i % 2) as f64 * 5.0))
        .collect();
    let mut m = Markup::new(
        MarkupType::MeasurementLength,
        0,
        MarkupGeometry::Polyline(pts),
        Appearance::default(),
        UserRef {
            user_id: uuid::Uuid::new_v4(),
            display_name: "Alice".into(),
        },
    );
    m.contents = Some("cable run".into());
    m.measurement = Some(Measurement {
        scale_ref: Some("sc".into()),
        raw_measure: 100.0,
        unit: "m".into(),
        computed_quantity: 10.0,
        depth: None,
        count_value: None,
        custom_columns: Default::default(),
        rise_drop: rise,
    });
    m
}

fn rd(segment: u32, amount: f64) -> RiseDrop {
    RiseDrop {
        segment,
        amount,
        unit: "m".into(),
    }
}

#[test]
fn a_polylength_is_a_standard_polyline_dimension_and_keeps_rise_drop_over_three_cycles() {
    let (mut doc, _page) = one_page_doc();
    let m = polylength(4, vec![rd(0, 2.0), rd(2, -1.5)]);
    let id = m.id();
    write_markups(&mut doc, &[m]).unwrap();
    doc = reload(&mut doc);
    for cycle in 1..=3 {
        let (_, d) = annots(&doc).into_iter().next().unwrap();
        assert_eq!(subtype(&d), "PolyLine", "cycle {cycle}");
        assert_eq!(
            d.get(b"IT").unwrap().as_name().unwrap(),
            b"PolyLineDimension",
            "cycle {cycle}"
        );
        assert_eq!(
            floats(&d, b"Vertices").len(),
            8,
            "cycle {cycle}: all 4 vertices written"
        );
        let ms = read_markups(&doc).unwrap();
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].id(), id, "cycle {cycle}");
        assert_eq!(ms[0].markup_type, MarkupType::MeasurementLength);
        let MarkupGeometry::Polyline(p) = &ms[0].geometry else {
            panic!()
        };
        assert_eq!(p.len(), 4);
        assert_eq!(
            ms[0].measurement.as_ref().unwrap().rise_drop,
            vec![rd(0, 2.0), rd(2, -1.5)],
            "cycle {cycle}"
        );
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);
        assert_eq!(annots(&doc).len(), 1, "cycle {cycle}: no duplication");
    }
}

#[test]
fn a_two_point_length_stays_a_standard_line() {
    let (mut doc, _page) = one_page_doc();
    write_markups(&mut doc, &[polylength(2, vec![rd(0, 1.0)])]).unwrap();
    doc = reload(&mut doc);
    let (_, d) = annots(&doc).into_iter().next().unwrap();
    assert_eq!(subtype(&d), "Line");
    assert!(d.get(b"IT").is_err());
    let ms = read_markups(&doc).unwrap();
    assert_eq!(
        ms[0].measurement.as_ref().unwrap().rise_drop,
        vec![rd(0, 1.0)]
    );
}

#[test]
fn hostile_rise_drop_is_bounded_on_read_and_write() {
    // Out-of-range segment, non-finite, oversized, duplicate segment, unit too long.
    let rise = vec![
        rd(0, 1.0),
        rd(0, 9.0),
        rd(7, 1.0),
        rd(1, f64::NAN),
        rd(1, f64::INFINITY),
        rd(1, 1e12),
        RiseDrop {
            segment: 1,
            amount: 1.0,
            unit: "x".repeat(100),
        },
        rd(1, 3.0),
    ];
    let m = polylength(4, rise.clone());
    // Written bounded...
    let d = m.to_annotation_dict();
    let back = Markup::from_annotation_dict(&d);
    assert_eq!(
        back.measurement.as_ref().unwrap().rise_drop,
        vec![rd(0, 1.0), rd(1, 3.0)]
    );
    // ...and read bounded even when a hostile writer bypassed the sanitiser: hand-craft JSON.
    let mut meas = m.measurement.clone().unwrap();
    meas.rise_drop = rise;
    let mut d2 = m.to_annotation_dict();
    d2.set(
        "RLMeasure",
        Object::string_literal(serde_json::to_string(&meas).unwrap()),
    );
    let back2 = Markup::from_annotation_dict(&d2);
    assert_eq!(
        back2.measurement.unwrap().rise_drop,
        vec![rd(0, 1.0), rd(1, 3.0)]
    );
}

#[test]
fn an_oversized_rlmeasure_blob_is_refused_without_parsing() {
    let m = polylength(3, vec![]);
    let mut d = m.to_annotation_dict();
    d.set(
        "RLMeasure",
        Object::string_literal(" ".repeat(3 * 1024 * 1024)),
    );
    assert!(Markup::from_annotation_dict(&d).measurement.is_none());
}

#[test]
fn rise_drop_entries_are_capped() {
    let many: Vec<RiseDrop> = (0..30_000u32).map(|i| rd(i, 1.0)).collect();
    let mut meas = polylength(3, vec![]).measurement.unwrap();
    meas.rise_drop = many;
    meas.sanitise_rise_drop(30_001);
    assert_eq!(meas.rise_drop.len(), MAX_RISE_DROP_ENTRIES);
}

#[test]
fn foreign_bluebeam_count_and_rise_drop_keys_are_still_preserved_verbatim() {
    let (mut doc, page) = one_page_doc();
    let set = a_set();
    let mut d = count_item(Some(uuid::Uuid::new_v4()), 10.0, &set).to_annotation_dict();
    d.set("NumCounts", 7);
    d.set("RiseDrop", Object::Array(vec![2.into(), 5.into()]));
    d.set("BBMeasure", Object::Dictionary(dictionary! { "Q" => 1 }));
    let oid = doc.add_object(d);
    set_annots(&mut doc, page, &[oid]);
    for cycle in 1..=3 {
        let ms = read_markups(&doc).unwrap();
        write_markups(&mut doc, &ms).unwrap();
        doc = reload(&mut doc);
        let (_, out) = annots(&doc).into_iter().next().unwrap();
        assert_eq!(
            out.get(b"NumCounts").unwrap().as_i64().unwrap(),
            7,
            "cycle {cycle}"
        );
        assert!(out.get(b"RiseDrop").is_ok(), "cycle {cycle}");
        assert!(out.get(b"BBMeasure").is_ok(), "cycle {cycle}");
        assert!(out.get(b"RLCountGroup").is_ok(), "cycle {cycle}");
    }
}
