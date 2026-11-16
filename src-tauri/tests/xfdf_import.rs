//! XFDF import: an export imports back into an empty document as the identical markup set (one
//! batch, nothing already present is added twice), a Revu-style file maps by element, and
//! hostile input is refused rather than guessed at.

use redline_lib::markup::xfdf_import::import_annots;
use std::collections::HashSet;

use redline_lib::geometry::PdfPoint;
use redline_lib::markup::xfdf::{export_xfdf, parse_xfdf};
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

fn imported(
    xml: &str,
    existing: &HashSet<String>,
    pages: u32,
) -> redline_lib::markup::xfdf_import::XfdfImport {
    import_annots(&parse_xfdf(xml).expect("parses"), existing, pages)
}

fn secs(d: chrono::DateTime<chrono::Utc>) -> i64 {
    d.timestamp()
}

#[test]
fn export_then_import_into_an_empty_document_gives_the_identical_markup_set() {
    let marks = sample();
    let xml = export_xfdf(&marks, "a.pdf");
    let got = imported(&xml, &HashSet::new(), 10);
    assert_eq!(got.markups.len(), marks.len());
    assert_eq!(
        (got.skipped_existing, got.ignored, got.simplified),
        (0, 0, 0)
    );
    for (want, have) in marks.iter().zip(&got.markups) {
        assert_eq!(have.id(), want.id());
        assert_eq!(have.markup_type, want.markup_type);
        assert_eq!(have.page, want.page);
        assert_eq!(have.geometry, want.geometry, "{:?}", want.markup_type);
        assert_eq!(have.appearance.color, want.appearance.color);
        assert_eq!(have.appearance.fill, want.appearance.fill);
        assert_eq!(have.appearance.line_weight, want.appearance.line_weight);
        assert_eq!(have.appearance.opacity, want.appearance.opacity);
        assert_eq!(have.contents, want.contents);
        assert_eq!(have.subject, want.subject);
        assert_eq!(
            have.audit.created_by.display_name,
            want.audit.created_by.display_name
        );
        assert_eq!(secs(have.audit.modified_at), secs(want.audit.modified_at));
        assert_eq!(secs(have.audit.created_at), secs(want.audit.created_at));
        assert_eq!(have.annot_flags, want.annot_flags);
        assert_eq!(have.workflow.status, want.workflow.status);
        assert_eq!(have.workflow.thread.len(), want.workflow.thread.len());
        for (r, h) in want.workflow.thread.iter().zip(&have.workflow.thread) {
            assert_eq!(h.id, r.id);
            assert_eq!(h.contents, r.contents);
            assert_eq!(h.author.display_name, r.author.display_name);
            assert_eq!(h.in_reply_to, r.in_reply_to);
        }
    }
}

#[test]
fn importing_what_the_document_already_has_adds_nothing() {
    let marks = sample();
    let xml = export_xfdf(&marks, "a.pdf");
    let existing: HashSet<String> = marks.iter().map(|m| m.id().to_string()).collect();
    let got = imported(&xml, &existing, 10);
    assert!(got.markups.is_empty());
    assert_eq!(got.skipped_existing, marks.len());
    // Replies and statuses of skipped markups are not ignored garbage, they just have no home.
    // Half present: only the missing ones come in.
    let half: HashSet<String> = marks.iter().take(3).map(|m| m.id().to_string()).collect();
    let got = imported(&xml, &half, 10);
    assert_eq!(got.markups.len(), marks.len() - 3);
    assert_eq!(got.skipped_existing, 3);
}

#[test]
fn the_flags_attribute_sets_the_locked_bits() {
    let xml = r##"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
      <square page="0" rect="0,0,10,10" name="A000000000000001" flags="print,locked"/>
      <square page="0" rect="0,0,10,10" name="A000000000000002" flags="print,lockedcontents"/>
      <square page="0" rect="0,0,10,10" name="A000000000000003"/>
    </annots></xfdf>"##;
    let got = imported(xml, &HashSet::new(), 1);
    assert!(got.markups[0].is_locked() && !got.markups[0].is_contents_locked());
    assert!(!got.markups[1].is_locked() && got.markups[1].is_contents_locked());
    assert_eq!(got.markups[2].annot_flags, 4, "no flags attribute = print");
}

const REVU: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<xfdf xmlns="http://ns.adobe.com/xfdf/" xml:space="preserve">
  <f href="plan.pdf"/>
  <annots>
    <square page="0" rect="10,20,110,80" name="REVUSQUARE000001" title="Rev One" date="D:20261110090000+04'00'" color="#FF0000" interior-color="#00FF00" width="2" opacity="0.5" flags="print">
      <contents>Check this</contents>
    </square>
    <circle page="1" rect="0,0,50,50" name="REVUCIRCLE000002" title="Rev One" date="D:20261110090000+04'00'" color="red"/>
    <highlight page="0" rect="0,0,20,10" name="REVUHILITE000003" title="Rev Two" date="D:20261110090000Z" color="#FFFF00" coords="0,10,20,10,0,0,20,0"/>
    <ink page="0" rect="0,0,9,9" name="REVUINK000000004" title="Rev Two" color="#0000FF"><inklist><gesture>1,1;2,2;3,3</gesture></inklist></ink>
    <line page="0" rect="0,0,100,50" name="REVULINE000000005" title="Rev Two" start="5,5" end="95,45"/>
    <polygon page="0" rect="0,0,50,40" name="REVUPOLY000000006" title="Rev Two"><vertices>0,0;50,0;50,40;0,40</vertices></polygon>
    <freetext page="0" rect="100,100,200,150" name="REVUTEXT000000007" title="Rev Two"><contents>Note it</contents></freetext>
    <text page="0" rect="30,30,50,50" name="REVUNOTE000000008" title="Rev Two"><contents>Sticky</contents></text>
    <text page="0" rect="30,30,50,50" name="REVUREPLY00000009" title="Rev Three" date="D:20261111090000Z" inreplyto="REVUSQUARE000001" replyType="R"><contents>Done</contents></text>
    <text page="0" rect="30,30,50,50" name="REVUSTATE00000010" title="Rev Three" date="D:20261111090001Z" inreplyto="REVUSQUARE000001" replyType="R" statemodel="Review" state="Accepted"/>
    <caret page="0" rect="0,0,5,5" name="REVUCARET00000011"/>
    <square page="9" rect="0,0,5,5" name="REVUFARPAGE00012"/>
    <square page="0" rect="NaN,0,5,5" name="REVUNAN0000000013"/>
  </annots>
</xfdf>"##;

#[test]
fn a_revu_style_file_maps_by_element_and_a_second_import_adds_nothing() {
    let got = imported(REVU, &HashSet::new(), 2);
    let types: Vec<_> = got.markups.iter().map(|m| m.markup_type).collect();
    assert_eq!(
        types,
        vec![
            MarkupType::Rectangle,
            MarkupType::Ellipse,
            MarkupType::Highlight,
            MarkupType::Ink,
            MarkupType::Line,
            MarkupType::Polygon,
            MarkupType::Text,
            MarkupType::Note,
        ]
    );
    // caret (no equivalent), page 9 of a 2-page document, and a NaN rect are ignored, not guessed.
    assert_eq!(got.ignored, 3);
    let sq = &got.markups[0];
    assert_eq!(sq.geometry, rect(10.0, 20.0, 110.0, 80.0));
    assert_eq!(sq.appearance.color, "#FF0000");
    assert_eq!(sq.appearance.fill.as_deref(), Some("#00FF00"));
    assert_eq!(sq.appearance.line_weight, 2.0);
    assert_eq!(sq.appearance.opacity, 0.5);
    assert_eq!(sq.contents.as_deref(), Some("Check this"));
    assert_eq!(sq.audit.created_by.display_name, "Rev One");
    assert_eq!(
        sq.audit.modified_at.to_rfc3339(),
        "2026-11-10T05:00:00+00:00"
    );
    assert_eq!(sq.workflow.thread.len(), 1);
    assert_eq!(sq.workflow.thread[0].contents, "Done");
    assert_eq!(sq.workflow.status, MarkupStatus::Accepted);
    // A named colour is not a hex colour: default, never passed through.
    assert_eq!(got.markups[1].appearance.color, "#000000");
    assert_eq!(
        got.markups[4].geometry,
        MarkupGeometry::Polyline(vec![p(5.0, 5.0), p(95.0, 45.0)])
    );
    match &got.markups[2].geometry {
        MarkupGeometry::Quads(q) => assert_eq!(q.len(), 1),
        g => panic!("highlight must import as quads, got {g:?}"),
    }
    // The same file again, against a document that now holds those markups.
    let existing: HashSet<String> = got.markups.iter().map(|m| m.id().to_string()).collect();
    let again = imported(REVU, &existing, 2);
    assert!(again.markups.is_empty());
    assert_eq!(again.skipped_existing, 8);
}

#[test]
fn measurement_and_legend_types_come_in_as_their_plain_shape_and_are_counted() {
    let mut m = markup(
        MarkupType::MeasurementArea,
        0,
        MarkupGeometry::Polyline(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)]),
        "#ff0000",
        "Ada",
    );
    m.contents = Some("area".into());
    let got = imported(&export_xfdf(&[m], "a.pdf"), &HashSet::new(), 1);
    assert_eq!(got.markups[0].markup_type, MarkupType::Polygon);
    assert_eq!(got.simplified, 1);
}

#[test]
fn a_reply_to_a_skipped_markup_is_ignored_not_attached_elsewhere() {
    let marks = sample();
    let xml = export_xfdf(&marks[..1], "a.pdf");
    let existing: HashSet<String> = [marks[0].id().to_string()].into();
    let got = imported(&xml, &existing, 10);
    assert!(got.markups.is_empty());
    assert!(got.ignored >= 2, "the two replies have no markup to join");
}

const NAMELESS: &str = r##"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
  <square page="0" rect="0,0,10,10" title="A"><contents>one</contents></square>
  <square page="0" rect="0,0,10,10" title="A"><contents>one</contents></square>
  <square page="0" rect="20,20,30,30" title="A"><contents>two</contents></square>
  <text page="0" rect="0,0,5,5" title="B" inreplyto="x"/>
</annots></xfdf>"##;

#[test]
fn nameless_annotations_get_distinct_ids_and_a_reimport_skips_them_all() {
    let got = imported(NAMELESS, &HashSet::new(), 1);
    assert_eq!(
        got.markups.len(),
        3,
        "{:?}",
        (got.skipped_existing, got.ignored)
    );
    let ids: HashSet<_> = got.markups.iter().map(|m| m.id()).collect();
    assert_eq!(ids.len(), 3, "every nameless annotation has its own id");
    // Same file again against a document holding them: nothing new, nothing silently merged.
    let existing: HashSet<String> = got.markups.iter().map(|m| m.id().to_string()).collect();
    let again = imported(NAMELESS, &existing, 1);
    assert!(again.markups.is_empty());
    assert_eq!(again.skipped_existing, 3);
    // And the ids are stable across runs.
    let third = imported(NAMELESS, &HashSet::new(), 1);
    let ids3: HashSet<_> = third.markups.iter().map(|m| m.id()).collect();
    assert_eq!(ids, ids3);
}

#[test]
fn one_malformed_annotation_is_skipped_with_a_warning_not_the_whole_file() {
    use redline_lib::markup::xfdf::parse_xfdf_lenient;
    let xml = r##"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
      <square page="0" rect="0,0,10,10" name="A000000000000001"/>
      <square rect="0,0,10,10" name="A000000000000002"/>
      <square page="0" name="A000000000000003"/>
      <square page="0" rect="1,2,3" name="A000000000000004"/>
      <circle page="0" rect="5,5,9,9" name="A000000000000005"/>
    </annots></xfdf>"##;
    assert!(
        parse_xfdf(xml).is_err(),
        "the strict parser still refuses it"
    );
    let (annots, warnings) = parse_xfdf_lenient(xml).unwrap();
    assert_eq!(annots.len(), 2);
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(warnings[0].starts_with("annotation 2:"), "{warnings:?}");
    let got = import_annots(&annots, &HashSet::new(), 1);
    assert_eq!(got.markups.len(), 2);
    // A file that is not XFDF at all is still an error.
    assert!(parse_xfdf_lenient("<html/>").is_err());
}
