//! Per-markup checkmark: ticking writes the standard `Marked` state model (a hidden `/Text`
//! reply with `/StateModel /Marked` + `/State /Marked`), unticking writes `Unmarked`, the
//! history is kept, and both read back through a saved PDF.

use chrono::Utc;
use lopdf::{dictionary, Document, Object, Stream};
use redline_lib::document::save::{load_markups_from, save_with_markups_scaled};
use redline_lib::geometry::PdfPoint;
use redline_lib::markup::*;
use std::collections::HashSet;

fn user() -> UserRef {
    UserRef {
        user_id: uuid::Uuid::new_v4(),
        display_name: "Reviewer".into(),
    }
}

fn markup() -> Markup {
    Markup::new(
        MarkupType::Rectangle,
        0,
        MarkupGeometry::Rect {
            min: PdfPoint { x: 10.0, y: 10.0 },
            max: PdfPoint { x: 80.0, y: 40.0 },
        },
        Appearance::default(),
        user(),
    )
}

fn entry(model: &str, state: &str, on_reply: Option<uuid::Uuid>) -> StatusChange {
    StatusChange {
        id: uuid::Uuid::new_v4(),
        author: user(),
        at: Utc::now(),
        model: model.into(),
        state: state.into(),
        on_reply,
        nm: None,
        extras: None,
    }
}

#[test]
fn is_marked_follows_the_latest_markup_level_marked_entry_only() {
    let mut m = markup();
    assert!(!m.workflow.is_marked(), "no entry is unticked");
    m.workflow
        .status_history
        .push(entry("Marked", "Marked", None));
    assert!(m.workflow.is_marked());
    // Other models and entries on a reply never move the checkmark.
    m.workflow
        .status_history
        .push(entry("Review", "Accepted", None));
    m.workflow
        .status_history
        .push(entry("Marked", "Unmarked", Some(uuid::Uuid::new_v4())));
    assert!(m.workflow.is_marked());
    m.workflow
        .status_history
        .push(entry("Marked", "Unmarked", None));
    assert!(!m.workflow.is_marked());
}

#[test]
fn set_marked_appends_a_state_entry_and_is_idempotent() {
    let mut m = markup();
    assert!(
        !m.workflow.set_marked(false, user(), Utc::now()),
        "already unticked"
    );
    assert!(m.workflow.set_marked(true, user(), Utc::now()));
    assert!(
        !m.workflow.set_marked(true, user(), Utc::now()),
        "already ticked"
    );
    assert!(m.workflow.set_marked(false, user(), Utc::now()));
    let h = &m.workflow.status_history;
    assert_eq!(h.len(), 2, "history kept, nothing replaced");
    assert_eq!(
        (h[0].model.as_str(), h[0].state.as_str()),
        ("Marked", "Marked")
    );
    assert_eq!(
        (h[1].model.as_str(), h[1].state.as_str()),
        ("Marked", "Unmarked")
    );
    assert_eq!(
        m.workflow.status,
        MarkupStatus::None,
        "the Review status is untouched"
    );
}

#[test]
fn a_quick_untick_is_dated_after_the_tick_so_a_reload_keeps_the_order() {
    let mut m = markup();
    let now = Utc::now();
    m.workflow.set_marked(true, user(), now);
    m.workflow.set_marked(false, user(), now);
    let h = &m.workflow.status_history;
    assert!(
        h[1].at - h[0].at >= chrono::Duration::seconds(1),
        "PDF dates are whole seconds; same-second entries would swap on reload"
    );
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

/// (StateModel, State, F) of every state annotation in `path`.
fn state_annots(path: &std::path::Path) -> Vec<(String, String, i64)> {
    let doc = Document::load(path).unwrap();
    let text = |d: &lopdf::Dictionary, k: &[u8]| {
        d.get(k)
            .ok()
            .and_then(|o| o.as_str().ok())
            .map(|b| String::from_utf8_lossy(b).into_owned())
    };
    let mut v: Vec<_> = doc
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|d| d.has(b"StateModel"))
        .map(|d| {
            (
                text(d, b"StateModel").unwrap(),
                text(d, b"State").unwrap(),
                d.get(b"F").unwrap().as_i64().unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn ticking_writes_the_standard_marked_state_model_and_reads_back() {
    let dir = std::env::temp_dir().join(format!("redline-checkmark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (src, out1, out2) = (
        dir.join("src.pdf"),
        dir.join("one.pdf"),
        dir.join("two.pdf"),
    );
    std::fs::write(&src, one_page_pdf()).unwrap();

    let mut m = markup();
    m.workflow.set_marked(true, user(), Utc::now());
    save_with_markups_scaled(&src, &out1, &[m.clone()], &HashSet::new(), &[]).unwrap();
    assert_eq!(
        state_annots(&out1),
        vec![("Marked".to_string(), "Marked".to_string(), 30)],
        "one hidden Marked/Marked state annotation"
    );
    let back = load_markups_from(&out1, None).unwrap();
    assert_eq!(back.len(), 1);
    assert!(back[0].workflow.is_marked());

    // Unticking appends Unmarked; Marked stays in the history; the markup reads back unticked.
    let mut m2 = back[0].clone();
    m2.workflow.set_marked(false, user(), Utc::now());
    save_with_markups_scaled(&out1, &out2, &[m2], &HashSet::new(), &[]).unwrap();
    assert_eq!(
        state_annots(&out2),
        vec![
            ("Marked".to_string(), "Marked".to_string(), 30),
            ("Marked".to_string(), "Unmarked".to_string(), 30),
        ]
    );
    let back2 = load_markups_from(&out2, None).unwrap();
    assert!(!back2[0].workflow.is_marked());
    assert_eq!(
        back2[0].workflow.status,
        MarkupStatus::None,
        "Review status unaffected"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
