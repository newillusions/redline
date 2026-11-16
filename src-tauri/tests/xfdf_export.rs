//! Markups List "Export XFDF": every markup (type, rect, colour, contents, author, status,
//! replies via `inreplyto`) is written to XFDF and parses back with no field loss.

use redline_lib::geometry::PdfPoint;
use redline_lib::markup::xfdf::{export_xfdf, parse_xfdf, xfdf_rect};
use redline_lib::markup::*;
use uuid::Uuid;

fn user(name: &str) -> UserRef {
    UserRef {
        user_id: Uuid::new_v4(),
        display_name: name.into(),
    }
}

fn p(x: f64, y: f64) -> PdfPoint {
    PdfPoint { x, y }
}

fn app(color: &str) -> Appearance {
    Appearance {
        color: color.into(),
        ..Appearance::default()
    }
}

fn markup(t: MarkupType, page: u32, g: MarkupGeometry, color: &str, author: &str) -> Markup {
    Markup::new(t, page, g, app(color), user(author))
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> MarkupGeometry {
    MarkupGeometry::Rect {
        min: p(x0, y0),
        max: p(x1, y1),
    }
}

fn sample() -> Vec<Markup> {
    let mut rectm = markup(
        MarkupType::Rectangle,
        0,
        rect(10.5, 20.25, 110.0, 80.125),
        "#ff0000",
        "Ada Reviewer",
    );
    rectm.contents = Some("Move <this> & \"that\"\nsecond line\twith tab".into());
    rectm.subject = Some("Clash & coordination".into());
    rectm.appearance.fill = Some("#00ff00".into());
    rectm.workflow.status = MarkupStatus::Accepted;
    let reply1 = Reply {
        id: Uuid::new_v4(),
        author: user("Bob"),
        at: chrono::Utc::now(),
        contents: "Agreed, <done>".into(),
        in_reply_to: None,
        nm: None,
        extras: None,
    };
    let reply2 = Reply {
        id: Uuid::new_v4(),
        author: user("Cy"),
        at: chrono::Utc::now(),
        contents: "Nested reply".into(),
        in_reply_to: Some(reply1.id),
        nm: None,
        extras: None,
    };
    rectm.workflow.thread = vec![reply1, reply2];

    let mut cloud = markup(
        MarkupType::Cloud,
        1,
        MarkupGeometry::Polyline(vec![p(0.0, 0.0), p(50.0, 0.0), p(50.0, 40.0), p(0.0, 40.0)]),
        "#0000ff",
        "Dee",
    );
    cloud.contents = Some("Revise".into());
    cloud.workflow.status = MarkupStatus::Rejected;

    let mut ink = markup(
        MarkupType::Ink,
        1,
        MarkupGeometry::Ink(vec![
            vec![p(1.0, 2.0), p(3.0, 4.0)],
            vec![p(5.5, 6.5), p(7.5, 8.5)],
        ]),
        "#123456",
        "Eve",
    );
    ink.contents = None;

    let hl = markup(
        MarkupType::Highlight,
        2,
        MarkupGeometry::Quads(vec![[
            p(0.0, 10.0),
            p(20.0, 10.0),
            p(0.0, 0.0),
            p(20.0, 0.0),
        ]]),
        "#ffff00",
        "Fay",
    );
    let line = markup(
        MarkupType::Arrow,
        2,
        MarkupGeometry::Polyline(vec![p(5.0, 5.0), p(95.0, 45.0)]),
        "#abcdef",
        "Gus",
    );
    let mut note = markup(
        MarkupType::Note,
        3,
        MarkupGeometry::Point(p(30.0, 30.0)),
        "#ffcc00",
        "Hal \u{e9}\u{4e2d}",
    );
    note.contents = Some("Ünïcode ✓ note".into());
    vec![rectm, cloud, ink, hl, line, note]
}

#[test]
fn xfdf_round_trips_every_markup_with_no_field_loss() {
    let marks = sample();
    let xml = export_xfdf(&marks, "plans & specs.pdf");
    let parsed = parse_xfdf(&xml).expect("export must be well-formed XFDF");

    // Every markup is present, in order, with its own fields.
    let mains: Vec<_> = parsed.iter().filter(|a| a.markup_type.is_some()).collect();
    assert_eq!(mains.len(), marks.len());
    for (m, a) in marks.iter().zip(&mains) {
        assert_eq!(a.markup_type, Some(m.markup_type));
        assert_eq!(a.name, m.id().to_string());
        assert_eq!(a.page, m.page);
        assert_eq!(a.color.as_deref(), Some(m.appearance.color.as_str()));
        assert_eq!(a.interior_color, m.appearance.fill);
        assert_eq!(a.contents, m.contents.clone().unwrap_or_default());
        assert_eq!(a.subject, m.subject);
        assert_eq!(a.author, m.audit.created_by.display_name);
        assert!(a.irt.is_none());
        let bb = xfdf_rect(m);
        assert_eq!(a.rect, bb, "rect must round-trip exactly");
        match &m.geometry {
            MarkupGeometry::Ink(s) => assert_eq!(&a.gestures, s),
            MarkupGeometry::Quads(q) => {
                let flat: Vec<f64> = q
                    .iter()
                    .flat_map(|q| q.iter().flat_map(|p| [p.x, p.y]))
                    .collect();
                assert_eq!(a.coords, flat);
            }
            MarkupGeometry::Polyline(v) if a.element == "polygon" => assert_eq!(&a.vertices, v),
            _ => {}
        }
    }

    // Replies: one <text> per reply with inreplyto = what it answers.
    let rm = &marks[0];
    let replies: Vec<_> = parsed
        .iter()
        .filter(|a| a.element == "text" && a.irt.is_some() && a.state.is_none())
        .collect();
    assert_eq!(replies.len(), 2);
    for (r, a) in rm.workflow.thread.iter().zip(&replies) {
        assert_eq!(a.name, r.id.to_string());
        assert_eq!(a.contents, r.contents);
        assert_eq!(a.author, r.author.display_name);
        let want = r.in_reply_to.unwrap_or(rm.id()).to_string();
        assert_eq!(a.irt.as_deref(), Some(want.as_str()));
    }

    // Status: Review-model state entries pointing at their markup.
    for (m, want) in [(&marks[0], "Accepted"), (&marks[1], "Rejected")] {
        let s: Vec<_> = parsed
            .iter()
            .filter(|a| a.state.is_some() && a.irt.as_deref() == Some(m.id().to_string().as_str()))
            .collect();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].state.as_deref(), Some(want));
        assert_eq!(s[0].state_model.as_deref(), Some("Review"));
    }
    // Markups with no status get no state entry.
    assert_eq!(parsed.iter().filter(|a| a.state.is_some()).count(), 2);
}

#[test]
fn xfdf_is_standard_shape_and_escapes_hostile_text() {
    let mut m = markup(
        MarkupType::Rectangle,
        0,
        rect(0.0, 0.0, 5.0, 5.0),
        "#000000",
        "A\u{1}B",
    );
    m.contents = Some("</contents><evil a=\"1\"/>".into());
    let xml = export_xfdf(&[m], "x.pdf");
    assert!(xml.contains("xmlns=\"http://ns.adobe.com/xfdf/\""));
    assert!(xml.contains("<square "));
    let parsed = parse_xfdf(&xml).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].contents, "</contents><evil a=\"1\"/>");
    // A control char XML 1.0 cannot carry becomes U+FFFD, never invalid XML.
    assert_eq!(parsed[0].author, "A\u{FFFD}B");
}

#[test]
fn empty_document_exports_an_empty_annots_list() {
    let xml = export_xfdf(&[], "x.pdf");
    assert!(parse_xfdf(&xml).unwrap().is_empty());
}

#[test]
fn export_writes_the_markups_own_flags_including_locked() {
    let mut locked = markup(
        MarkupType::Rectangle,
        0,
        rect(0.0, 0.0, 10.0, 10.0),
        "#ff0000",
        "Ada",
    );
    locked.set_locked(true);
    locked.annot_flags |= 0x200; // LockedContents too
    let plain = markup(
        MarkupType::Rectangle,
        0,
        rect(20.0, 20.0, 30.0, 30.0),
        "#00ff00",
        "Ada",
    );
    let xml = export_xfdf(&[locked, plain], "a.pdf");
    assert!(
        xml.contains("flags=\"print,locked,lockedcontents\""),
        "locked markup keeps its flags:\n{xml}"
    );
    assert_eq!(
        xml.matches("flags=\"print\"").count(),
        1,
        "the unlocked markup is still plain print:\n{xml}"
    );
}
