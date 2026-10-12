//! Callout extras: an explicit text box, extra leaders and auto-size, around the standard
//! FreeText callout (`/IT /FreeTextCallout`, `/CL`, `/RD`, `/LE`; ISO 32000-1 12.5.6.6).
//! Behaviour spec: `docs/callout-cloud.md`.
//!
//! The primary leader is the markup's `Polyline` geometry: `[tip, (knee,) attach]` - 2 or 3
//! points, written as `/CL` (4 or 6 numbers, tip first as the standard defines). Everything
//! else lives in [`CalloutProps`]:
//!   - the text box (`/RD` carries it for other viewers; `/RLCalloutBox` carries it exactly),
//!   - additional leaders (`/RLCalloutLeaders`, private; drawn into `/AP` so any viewer shows
//!     them),
//!   - the auto-size flag (`/RLCalloutAuto`).
//!
//! A callout without props (older files, foreign callouts with no `/RD`) keeps the legacy box:
//! `LEGACY_BOX` anchored at the last leader point.
//!
//! Every value is untrusted (PDF on read, JSON from the GUI / MCP on write): non-finite or
//! absurd values drop the props (legacy box), leaders are capped in number and point count.

use lopdf::{Dictionary, Object};
use serde::{Deserialize, Serialize};

use super::{Markup, MarkupGeometry, MarkupType};
use crate::geometry::PdfPoint;

/// Points in one leader (`/CL` is 4 or 6 numbers).
pub const MAX_LEADER_POINTS: usize = 3;
/// Extra leaders kept beyond the primary one.
pub const MAX_EXTRA_LEADERS: usize = 16;
/// Largest accepted text-box side, PDF points.
pub const MAX_BOX_SIDE: f64 = 20_000.0;
/// Text-box size of a callout with no props, PDF points (mirrors `markup-render.ts`).
pub const LEGACY_BOX: (f64, f64) = (144.0, 18.0);

/// The `/LE` (line ending) of a callout's leader: a foreign callout's value is read, kept and
/// written back; a callout redline authors gets a closed arrow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineEnding {
    /// No `/LE` key (the standard default is no ending).
    Absent,
    /// A name such as `OpenArrow`, `None`, `Circle` (short, `[A-Za-z0-9]` only).
    Named(String),
}

const MAX_LE_NAME: usize = 32;

impl LineEnding {
    /// What redline draws and writes for a callout it authored.
    pub fn closed_arrow() -> LineEnding {
        LineEnding::Named("ClosedArrow".into())
    }

    /// The annotation's `/LE`. A value that is not a short plain name (an array, an
    /// oversized or odd-looking name) reads as [`LineEnding::Absent`].
    pub fn read(d: &Dictionary) -> LineEnding {
        match d.get(b"LE").ok().and_then(|o| o.as_name().ok()) {
            Some(n)
                if !n.is_empty()
                    && n.len() <= MAX_LE_NAME
                    && n.iter().all(|b| b.is_ascii_alphanumeric()) =>
            {
                LineEnding::Named(String::from_utf8_lossy(n).into_owned())
            }
            _ => LineEnding::Absent,
        }
    }

    /// The name to draw by, `None` for no ending (absent or `/None`).
    pub fn drawn_name(&self) -> Option<&str> {
        match self {
            LineEnding::Named(n) if n != "None" => Some(n.as_str()),
            _ => None,
        }
    }
}

/// The ending to draw and write for `m`: the one it was read with, else a closed arrow.
pub fn line_ending(m: &Markup) -> LineEnding {
    m.source_le.clone().unwrap_or_else(LineEnding::closed_arrow)
}

/// Text box + extra leaders + auto-size of a [`MarkupType::Callout`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalloutProps {
    /// Text box corners, PDF user space (`min` is bottom-left).
    pub box_min: PdfPoint,
    pub box_max: PdfPoint,
    /// Extra leaders, each `[tip, (knee,) attach]`.
    #[serde(default)]
    pub extra_leaders: Vec<Vec<PdfPoint>>,
    /// The box follows its text (set by the GUI when the text changes).
    #[serde(default)]
    pub autosize: bool,
}

fn finite(p: &PdfPoint) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// A leader reduced to at most [`MAX_LEADER_POINTS`] finite points (first, middle, last).
fn compress(pts: &[PdfPoint]) -> Vec<PdfPoint> {
    let pts: Vec<PdfPoint> = pts.iter().copied().filter(finite).collect();
    if pts.len() <= MAX_LEADER_POINTS {
        return pts;
    }
    vec![pts[0], pts[pts.len() / 2], pts[pts.len() - 1]]
}

impl CalloutProps {
    /// The props as safe to draw and write: `None` when the box is non-finite, empty or
    /// oversized; extra leaders with fewer than 2 finite points are dropped, the rest
    /// compressed and capped.
    pub fn sanitized(&self) -> Option<CalloutProps> {
        let (a, b) = (self.box_min, self.box_max);
        if !finite(&a) || !finite(&b) {
            return None;
        }
        let (min, max) = (
            PdfPoint {
                x: a.x.min(b.x),
                y: a.y.min(b.y),
            },
            PdfPoint {
                x: a.x.max(b.x),
                y: a.y.max(b.y),
            },
        );
        let (w, h) = (max.x - min.x, max.y - min.y);
        if !(w > 0.0 && h > 0.0 && w <= MAX_BOX_SIDE && h <= MAX_BOX_SIDE) {
            return None;
        }
        Some(CalloutProps {
            box_min: min,
            box_max: max,
            extra_leaders: self
                .extra_leaders
                .iter()
                .map(|l| compress(l))
                .filter(|l| l.len() >= 2)
                .take(MAX_EXTRA_LEADERS)
                .collect(),
            autosize: self.autosize,
        })
    }
}

/// The primary leader (`[tip, (knee,) attach]`), capped and finite. Empty for non-polyline
/// geometry.
pub fn primary_leader(m: &Markup) -> Vec<PdfPoint> {
    match &m.geometry {
        MarkupGeometry::Polyline(pts) => compress(pts),
        _ => Vec::new(),
    }
}

/// The sanitized props, if the markup has usable ones.
pub fn props(m: &Markup) -> Option<CalloutProps> {
    m.callout.as_ref().and_then(CalloutProps::sanitized)
}

/// Every leader to draw: the primary one, then the extras.
pub fn all_leaders(m: &Markup) -> Vec<Vec<PdfPoint>> {
    let mut out = Vec::new();
    let primary = primary_leader(m);
    if !primary.is_empty() {
        out.push(primary);
    }
    if let Some(p) = props(m) {
        out.extend(p.extra_leaders);
    }
    out
}

/// The text box `[x0, y0, x1, y1]`: the props' box, else [`LEGACY_BOX`] at the last leader
/// point (bottom-left corner, extending up and right).
pub fn text_box(m: &Markup) -> [f64; 4] {
    if let Some(p) = props(m) {
        return [p.box_min.x, p.box_min.y, p.box_max.x, p.box_max.y];
    }
    let anchor = primary_leader(m)
        .last()
        .copied()
        .unwrap_or(PdfPoint { x: 0.0, y: 0.0 });
    [
        anchor.x,
        anchor.y,
        anchor.x + LEGACY_BOX.0,
        anchor.y + LEGACY_BOX.1,
    ]
}

/// Bounds of everything the callout draws: all leader points and the text box.
pub fn bounds(m: &Markup) -> Option<[f64; 4]> {
    let leaders = all_leaders(m);
    if leaders.is_empty() {
        return None;
    }
    let b = text_box(m);
    let (mut x0, mut y0, mut x1, mut y1) = (b[0], b[1], b[2], b[3]);
    for p in leaders.iter().flatten() {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Some([x0, y0, x1, y1])
}

/// Geometry as stored for a callout: the leader reduced to at most 3 points.
pub fn compress_geometry(g: MarkupGeometry) -> MarkupGeometry {
    match g {
        MarkupGeometry::Polyline(pts) if pts.len() > MAX_LEADER_POINTS => {
            MarkupGeometry::Polyline(compress(&pts))
        }
        other => other,
    }
}

fn real(v: f64) -> Object {
    Object::Real(v as f32)
}

fn reals(vs: &[f64]) -> Object {
    Object::Array(vs.iter().map(|v| real(*v)).collect())
}

fn get_reals(d: &Dictionary, key: &[u8]) -> Option<Vec<f64>> {
    d.get(key)
        .ok()?
        .as_array()
        .ok()?
        .iter()
        .map(|o| o.as_float().ok().map(|f| f as f64))
        .collect()
}

/// Write the callout keys that are not geometry: `/IT`, `/LE`, and, with props, `/RD` (box
/// as inset from `rect`, the standard way), `/RLCalloutBox`, `/RLCalloutLeaders`,
/// `/RLCalloutAuto`. `rect` is the `/Rect` already chosen for the annotation.
pub fn write_keys(d: &mut Dictionary, m: &Markup, rect: [f64; 4]) {
    if m.markup_type != MarkupType::Callout {
        return;
    }
    d.set("IT", Object::Name(b"FreeTextCallout".to_vec()));
    if let LineEnding::Named(n) = line_ending(m) {
        d.set("LE", Object::Name(n.into_bytes()));
    }
    let Some(p) = props(m) else { return };
    let (b0, b1) = (p.box_min, p.box_max);
    // /RD: left, top, right, bottom offsets from /Rect to the text box.
    d.set(
        "RD",
        reals(&[
            (b0.x - rect[0]).max(0.0),
            (rect[3] - b1.y).max(0.0),
            (rect[2] - b1.x).max(0.0),
            (b0.y - rect[1]).max(0.0),
        ]),
    );
    d.set("RLCalloutBox", reals(&[b0.x, b0.y, b1.x, b1.y]));
    if !p.extra_leaders.is_empty() {
        d.set(
            "RLCalloutLeaders",
            Object::Array(
                p.extra_leaders
                    .iter()
                    .map(|l| reals(&l.iter().flat_map(|p| [p.x, p.y]).collect::<Vec<_>>()))
                    .collect(),
            ),
        );
    }
    if p.autosize {
        d.set("RLCalloutAuto", Object::Boolean(true));
    }
}

/// Read the props of a callout annotation: the exact private keys when present (redline
/// wrote it), else the box a foreign viewer defined with `/Rect` + `/RD`.
pub fn read_props(d: &Dictionary) -> Option<CalloutProps> {
    let box4 = |v: Vec<f64>| -> Option<(PdfPoint, PdfPoint)> {
        let [x0, y0, x1, y1] = <[f64; 4]>::try_from(v).ok()?;
        Some((PdfPoint { x: x0, y: y0 }, PdfPoint { x: x1, y: y1 }))
    };
    if let Some((box_min, box_max)) = get_reals(d, b"RLCalloutBox").and_then(box4) {
        let extra_leaders = d
            .get(b"RLCalloutLeaders")
            .ok()
            .and_then(|o| o.as_array().ok())
            .map(|arr| {
                arr.iter()
                    .take(MAX_EXTRA_LEADERS)
                    .filter_map(|l| l.as_array().ok())
                    .map(|l| {
                        let r: Vec<f64> = l
                            .iter()
                            .take(MAX_LEADER_POINTS * 2)
                            .filter_map(|o| o.as_float().ok().map(|f| f as f64))
                            .collect();
                        r.chunks_exact(2)
                            .map(|c| PdfPoint { x: c[0], y: c[1] })
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();
        return CalloutProps {
            box_min,
            box_max,
            extra_leaders,
            autosize: matches!(d.get(b"RLCalloutAuto").ok(), Some(Object::Boolean(true))),
        }
        .sanitized();
    }
    // Foreign: text box = /Rect inset by /RD (left, top, right, bottom).
    let rect = get_reals(d, b"Rect")?;
    let rd = get_reals(d, b"RD")?;
    let [r0, r1, r2, r3] = <[f64; 4]>::try_from(rect).ok()?;
    let [l, t, r, b] = <[f64; 4]>::try_from(rd).ok()?;
    CalloutProps {
        box_min: PdfPoint {
            x: r0.min(r2) + l,
            y: r1.min(r3) + b,
        },
        box_max: PdfPoint {
            x: r0.max(r2) - r,
            y: r1.max(r3) - t,
        },
        extra_leaders: Vec::new(),
        autosize: false,
    }
    .sanitized()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::annots::tests::one_page_doc;
    use crate::document::annots::{page_annots, read_markups, write_markups};
    use crate::markup::{Appearance, UserRef};
    use lopdf::{dictionary, Document};

    fn pt(x: f64, y: f64) -> PdfPoint {
        PdfPoint { x, y }
    }

    fn callout(leader: Vec<PdfPoint>, props: Option<CalloutProps>) -> Markup {
        let mut m = Markup::new(
            MarkupType::Callout,
            0,
            MarkupGeometry::Polyline(leader),
            Appearance::default(),
            UserRef {
                user_id: uuid::Uuid::new_v4(),
                display_name: "Alice".into(),
            },
        );
        m.contents = Some("check clearance".into());
        m.callout = props;
        m
    }

    fn full_props() -> CalloutProps {
        CalloutProps {
            box_min: pt(200.0, 300.0),
            box_max: pt(320.0, 330.0),
            extra_leaders: vec![vec![pt(50.0, 400.0), pt(150.0, 330.0), pt(200.0, 320.0)]],
            autosize: true,
        }
    }

    fn reload(doc: &mut Document) -> Document {
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        Document::load_mem(&buf).unwrap()
    }

    fn first_annot(doc: &Document) -> Dictionary {
        let page = *doc.get_pages().values().next().unwrap();
        page_annots(doc, page)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .1
    }

    fn floats(d: &Dictionary, key: &[u8]) -> Vec<f64> {
        get_reals(d, key).unwrap_or_default()
    }

    #[test]
    fn writes_the_standard_callout_keys_with_a_knee() {
        let m = callout(
            vec![pt(10.0, 10.0), pt(100.0, 150.0), pt(200.0, 315.0)],
            Some(full_props()),
        );
        let d = m.to_annotation_dict();
        assert_eq!(d.get(b"IT").unwrap().as_name().unwrap(), b"FreeTextCallout");
        assert_eq!(d.get(b"LE").unwrap().as_name().unwrap(), b"ClosedArrow");
        assert_eq!(
            floats(&d, b"CL"),
            vec![10.0, 10.0, 100.0, 150.0, 200.0, 315.0]
        );
        // /Rect minus /RD is the text box: left, top, right, bottom insets.
        let (r, rd) = (floats(&d, b"Rect"), floats(&d, b"RD"));
        assert_eq!(
            [r[0] + rd[0], r[1] + rd[3], r[2] - rd[2], r[3] - rd[1]],
            [200.0, 300.0, 320.0, 330.0]
        );
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.markup_type, MarkupType::Callout);
        assert_eq!(back.callout, Some(full_props()));
        assert_eq!(back.geometry, m.geometry);
    }

    #[test]
    fn a_two_point_leader_writes_four_numbers_and_no_props_keep_the_legacy_box() {
        let m = callout(vec![pt(10.0, 10.0), pt(100.0, 50.0)], None);
        let d = m.to_annotation_dict();
        assert_eq!(floats(&d, b"CL").len(), 4);
        assert!(!d.has(b"RD") && !d.has(b"RLCalloutBox"));
        assert!(Markup::from_annotation_dict(&d).callout.is_none());
    }

    #[test]
    fn redline_callouts_are_unchanged_by_three_save_load_cycles() {
        let (mut doc, page) = one_page_doc();
        let m = callout(
            vec![pt(10.3, 10.7), pt(100.1, 150.9), pt(200.0, 315.0)],
            Some(full_props()),
        );
        let id = doc.add_object(m.to_annotation_dict());
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Annots", vec![id.into()]);
        let mut seen: Vec<Vec<Vec<f64>>> = Vec::new();
        for cycle in 0..3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 1, "cycle {cycle}");
            write_markups(&mut doc, &ms).unwrap();
            doc = reload(&mut doc);
            let d = first_annot(&doc);
            seen.push(
                [&b"CL"[..], b"Rect", b"RD", b"RLCalloutBox"]
                    .iter()
                    .map(|k| floats(&d, k))
                    .collect(),
            );
            assert!(d.has(b"RLCalloutLeaders") && d.has(b"RLCalloutAuto"));
        }
        assert_eq!(seen[0], seen[1], "cycle 2 differs from cycle 1");
        assert_eq!(seen[1], seen[2], "cycle 3 differs from cycle 2");
        let back = read_markups(&doc).unwrap().remove(0);
        assert_eq!(back.callout.as_ref().unwrap().extra_leaders.len(), 1);
        assert!(back.callout.unwrap().autosize);
    }

    fn revu_callout(doc: &mut Document) -> lopdf::ObjectId {
        doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "FreeText", "F" => 4,
            "IT" => "FreeTextCallout",
            "NM" => Object::string_literal("CALLOUTREVU00001"),
            "Rect" => vec![100.into(), 100.into(), 300.into(), 200.into()],
            "RD" => vec![60.into(), 5.into(), 10.into(), 20.into()],
            "CL" => vec![100.into(), 110.into(), 150.into(), 160.into(), 160.into(), 180.into()],
            "DA" => Object::string_literal("0 0 0 rg /Helv 12 Tf"),
            "Contents" => Object::string_literal("revu callout"),
        })
    }

    #[test]
    fn a_revu_callout_keeps_its_text_box_name_and_leader_over_three_cycles() {
        let (mut doc, page) = one_page_doc();
        let id = revu_callout(&mut doc);
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Annots", vec![id.into()]);
        let first = read_markups(&doc).unwrap();
        let p = first[0].callout.clone().expect("box from /Rect + /RD");
        assert_eq!((p.box_min, p.box_max), (pt(160.0, 120.0), pt(290.0, 195.0)));
        assert_eq!(first[0].source_nm.as_deref(), Some("CALLOUTREVU00001"));
        for cycle in 1..=3 {
            let ms = read_markups(&doc).unwrap();
            assert_eq!(ms.len(), 1, "cycle {cycle}");
            write_markups(&mut doc, &ms).unwrap();
            doc = reload(&mut doc);
            let annots = page_annots(&doc, page).unwrap();
            assert_eq!(annots.len(), 1, "cycle {cycle}: no duplicate");
            let d = &annots[0].1;
            assert_eq!(
                String::from_utf8_lossy(d.get(b"NM").unwrap().as_str().unwrap()),
                "CALLOUTREVU00001"
            );
            assert_eq!(
                floats(d, b"CL"),
                vec![100.0, 110.0, 150.0, 160.0, 160.0, 180.0]
            );
            let (r, rd) = (floats(d, b"Rect"), floats(d, b"RD"));
            assert_eq!(
                [r[0] + rd[0], r[1] + rd[3], r[2] - rd[2], r[3] - rd[1]],
                [160.0, 120.0, 290.0, 195.0],
                "cycle {cycle}: text box moved"
            );
        }
    }

    #[test]
    fn hostile_props_and_leaders_are_bounded_and_never_panic() {
        // Too many leaders, too many points, non-finite and oversized values.
        let many: Vec<Vec<PdfPoint>> = (0..1000)
            .map(|i| vec![pt(i as f64, 0.0), pt(0.0, i as f64)])
            .collect();
        let p = CalloutProps {
            box_min: pt(0.0, 0.0),
            box_max: pt(10.0, 10.0),
            extra_leaders: many,
            autosize: false,
        };
        assert_eq!(
            p.sanitized().unwrap().extra_leaders.len(),
            MAX_EXTRA_LEADERS
        );
        let bad_box = |a: PdfPoint, b: PdfPoint| {
            CalloutProps {
                box_min: a,
                box_max: b,
                extra_leaders: vec![],
                autosize: false,
            }
            .sanitized()
        };
        assert!(bad_box(pt(f64::NAN, 0.0), pt(1.0, 1.0)).is_none());
        assert!(bad_box(pt(0.0, 0.0), pt(f64::INFINITY, 1.0)).is_none());
        assert!(bad_box(pt(0.0, 0.0), pt(0.0, 5.0)).is_none(), "zero width");
        assert!(bad_box(pt(0.0, 0.0), pt(1.0e9, 5.0)).is_none(), "oversized");
        // A 200-point /CL and non-finite leader points on a markup arriving as JSON.
        let long: Vec<PdfPoint> = (0..200).map(|i| pt(i as f64, (i * 2) as f64)).collect();
        let mut m = callout(long, Some(p));
        assert_eq!(primary_leader(&m).len(), MAX_LEADER_POINTS);
        let d = m.to_annotation_dict();
        assert_eq!(floats(&d, b"CL").len(), MAX_LEADER_POINTS * 2);
        assert!(floats(&d, b"RLCalloutLeaders").is_empty()); // array of arrays, not reals
        assert_eq!(
            d.get(b"RLCalloutLeaders")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            MAX_EXTRA_LEADERS
        );
        m.geometry = MarkupGeometry::Polyline(vec![pt(f64::NAN, 1.0), pt(f64::INFINITY, 2.0)]);
        let d = m.to_annotation_dict();
        assert!(floats(&d, b"Rect").iter().all(|v| v.is_finite()));
        // Hostile PDF: a long /CL, thousands of leaders, wrong shapes.
        let mut d = Dictionary::new();
        d.set("Subtype", Object::Name(b"FreeText".to_vec()));
        d.set(
            "CL",
            Object::Array((0..4000).map(|i| Object::Real(i as f32)).collect()),
        );
        d.set(
            "RLCalloutBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(50.0),
                Object::Real(20.0),
            ]),
        );
        d.set(
            "RLCalloutLeaders",
            Object::Array(
                (0..5000)
                    .map(|_| Object::Array((0..50).map(|i| Object::Real(i as f32)).collect()))
                    .chain([
                        Object::Integer(7),
                        Object::Array(vec![Object::Name(b"x".to_vec())]),
                    ])
                    .collect(),
            ),
        );
        let back = Markup::from_annotation_dict(&d);
        assert_eq!(back.markup_type, MarkupType::Callout);
        let MarkupGeometry::Polyline(pts) = &back.geometry else {
            panic!()
        };
        assert!(pts.len() <= MAX_LEADER_POINTS);
        assert!(back.callout.unwrap().extra_leaders.len() <= MAX_EXTRA_LEADERS);
    }

    #[test]
    fn props_json_is_optional_and_omitted_when_absent() {
        let m = callout(vec![pt(0.0, 0.0), pt(5.0, 5.0)], None);
        let mut v = serde_json::to_value(&m).unwrap();
        assert!(v.get("callout").is_none());
        v["callout"] = serde_json::json!({
            "box_min": {"x": 1.0, "y": 2.0}, "box_max": {"x": 3.0, "y": 4.0}
        });
        let back: Markup = serde_json::from_value(v).unwrap();
        let p = back.callout.unwrap();
        assert!(p.extra_leaders.is_empty() && !p.autosize);
    }
}
